// Copyright 2022 The Turbo Cache Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::fmt::Debug;
use std::ops::DerefMut;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use async_lock::Mutex;
use async_trait::async_trait;
use lru::LruCache;
use serde::{Deserialize, Serialize};

use common::{log, DigestInfo};
use config::stores::EvictionPolicy;

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct SerializedLRU {
    pub data: Vec<(DigestInfo, i32)>,
    pub anchor_time: u64,
}

/// Wrapper used to abstract away which underlying Instant impl we are using.
/// This is needed for testing.
pub trait InstantWrapper {
    fn from_secs(secs: u64) -> Self;
    fn unix_timestamp(&self) -> u64;
    fn elapsed(&self) -> Duration;
}

impl InstantWrapper for SystemTime {
    fn from_secs(secs: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(secs)).unwrap()
    }

    fn unix_timestamp(&self) -> u64 {
        self.duration_since(UNIX_EPOCH).unwrap().as_secs()
    }

    fn elapsed(&self) -> Duration {
        <SystemTime>::elapsed(self).unwrap()
    }
}

#[derive(Debug)]
struct EvictionItem<T: LenEntry + Debug> {
    seconds_since_anchor: i32,
    data: T,
}

#[async_trait]
pub trait LenEntry {
    /// Length of referenced data.
    fn len(&self) -> usize;

    /// Returns `true` if `self` has zero length.
    fn is_empty(&self) -> bool;

    /// Called when an entry is touched.
    #[inline]
    async fn touch(&self) {}

    /// This will be called when object is removed from map.
    /// Note: There may still be a reference to it held somewhere else, which
    /// is why it can't be mutable. This is a good place to mark the item
    /// to be deleted and then in the Drop call actually do the deleting.
    /// This will ensure nowhere else in the program still holds a reference
    /// to this object.
    /// You should not rely only on the Drop trait. Doing so might result in the
    /// program safely shutting down and calling the Drop method on each object,
    /// which if you are deleting items you may not want to do.
    /// It is undefined behavior to have `unref()` called more than once.
    /// During the execution of `unref()` no items can be added or removed to/from
    /// the EvictionMap globally (including inside `unref()`).
    #[inline]
    async fn unref(&self) {}
}

#[async_trait]
impl<T: LenEntry + Send + Sync> LenEntry for Arc<T> {
    #[inline]
    fn len(&self) -> usize {
        T::len(self.as_ref())
    }

    #[inline]
    fn is_empty(&self) -> bool {
        T::is_empty(self.as_ref())
    }

    #[inline]
    async fn touch(&self) {
        self.as_ref().touch().await;
    }

    #[inline]
    async fn unref(&self) {
        self.as_ref().unref().await;
    }
}

struct State<T: LenEntry + Debug> {
    lru: LruCache<DigestInfo, EvictionItem<T>>,
    sum_store_size: u64,
}

pub struct EvictingMap<T: LenEntry + Debug, I: InstantWrapper> {
    state: Mutex<State<T>>,
    anchor_time: I,
    max_bytes: u64,
    evict_bytes: u64,
    max_seconds: i32,
    max_count: u64,
}

impl<T, I> EvictingMap<T, I>
where
    T: LenEntry + Debug + Clone + Send + Sync,
    I: InstantWrapper,
{
    pub fn new(config: &EvictionPolicy, anchor_time: I) -> Self {
        EvictingMap {
            // We use unbounded because if we use the bounded version we can't call the delete
            // function on the LenEntry properly.
            state: Mutex::new(State {
                lru: LruCache::unbounded(),
                sum_store_size: 0,
            }),
            anchor_time,
            max_bytes: config.max_bytes as u64,
            evict_bytes: config.evict_bytes as u64,
            max_seconds: config.max_seconds as i32,
            max_count: config.max_count,
        }
    }

    pub async fn build_lru_index(&self) -> SerializedLRU {
        let state = self.state.lock().await;
        let mut serialized_lru = SerializedLRU {
            data: Vec::with_capacity(state.lru.len()),
            anchor_time: self.anchor_time.unix_timestamp(),
        };
        for (digest, eviction_item) in state.lru.iter() {
            serialized_lru.data.push((*digest, eviction_item.seconds_since_anchor));
        }
        serialized_lru
    }

    pub async fn restore_lru(&mut self, seiralized_lru: SerializedLRU, entry_builder: impl Fn(&DigestInfo) -> T) {
        let mut state = self.state.lock().await;
        self.anchor_time = I::from_secs(seiralized_lru.anchor_time);
        state.lru.clear();
        for (digest, seconds_since_anchor) in seiralized_lru.data {
            let entry = entry_builder(&digest);
            state.lru.put(
                digest,
                EvictionItem {
                    seconds_since_anchor,
                    data: entry,
                },
            );
        }
        // Just in case we allow for some cleanup (eg: old items).
        self.evict_items(state.deref_mut()).await;
    }

    fn should_evict(&self, lru_len: usize, peek_entry: &EvictionItem<T>, sum_store_size: u64, max_bytes: u64) -> bool {
        let is_over_size = max_bytes != 0 && sum_store_size >= max_bytes;

        let evict_older_than_seconds = (self.anchor_time.elapsed().as_secs() as i32) - self.max_seconds;
        let old_item_exists = self.max_seconds != 0 && peek_entry.seconds_since_anchor < evict_older_than_seconds;

        let is_over_count = self.max_count != 0 && (lru_len as u64) > self.max_count;

        is_over_size || old_item_exists || is_over_count
    }

    async fn evict_items(&self, state: &mut State<T>) {
        let Some((_, mut peek_entry)) = state.lru.peek_lru() else { return; };

        let max_bytes = if self.max_bytes != 0
            && self.evict_bytes != 0
            && self.should_evict(state.lru.len(), peek_entry, state.sum_store_size, self.max_bytes)
        {
            if self.max_bytes > self.evict_bytes {
                self.max_bytes - self.evict_bytes
            } else {
                0
            }
        } else {
            self.max_bytes
        };

        while self.should_evict(state.lru.len(), peek_entry, state.sum_store_size, max_bytes) {
            let (key, eviction_item) = state.lru.pop_lru().expect("Tried to peek() then pop() but failed");
            state.sum_store_size -= eviction_item.data.len() as u64;
            // Note: See comment in `unref()` requring global lock of insert/remove.
            eviction_item.data.unref().await;
            log::info!("\x1b[0;31mEvicting Map\x1b[0m: Evicting {}", key.str());

            peek_entry = if let Some((_, entry)) = state.lru.peek_lru() {
                entry
            } else {
                return;
            };
        }
    }

    pub async fn size_for_key(&self, digest: &DigestInfo) -> Option<usize> {
        let mut state = self.state.lock().await;
        if let Some(entry) = state.lru.get_mut(digest) {
            entry.seconds_since_anchor = self.anchor_time.elapsed().as_secs() as i32;
            let data = entry.data.clone();
            drop(state);
            data.touch().await;
            return Some(data.len());
        }
        None
    }

    pub async fn get(&self, digest: &DigestInfo) -> Option<T> {
        let mut state = self.state.lock().await;
        if let Some(entry) = state.lru.get_mut(digest) {
            entry.seconds_since_anchor = self.anchor_time.elapsed().as_secs() as i32;
            let data = entry.data.clone();
            drop(state);
            data.touch().await;
            return Some(data);
        }
        None
    }

    /// Returns the replaced item if any.
    pub async fn insert(&self, digest: DigestInfo, data: T) -> Option<T> {
        self.insert_with_time(digest, data, self.anchor_time.elapsed().as_secs() as i32)
            .await
    }

    /// Returns the replaced item if any.
    pub async fn insert_with_time(&self, digest: DigestInfo, data: T, seconds_since_anchor: i32) -> Option<T> {
        let new_item_size = data.len() as u64;
        let eviction_item = EvictionItem {
            seconds_since_anchor,
            data,
        };
        let mut state = self.state.lock().await;

        let maybe_old_item = if let Some(old_item) = state.lru.put(digest, eviction_item) {
            state.sum_store_size -= old_item.data.len() as u64;
            // Note: See comment in `unref()` requring global lock of insert/remove.
            old_item.data.unref().await;
            Some(old_item.data)
        } else {
            None
        };
        state.sum_store_size += new_item_size;
        self.evict_items(state.deref_mut()).await;
        maybe_old_item
    }

    pub async fn remove(&self, digest: &DigestInfo) -> bool {
        let mut state = self.state.lock().await;
        self.inner_remove(&mut state, digest).await
    }

    async fn inner_remove(&self, state: &mut State<T>, digest: &DigestInfo) -> bool {
        if let Some(entry) = state.lru.pop(digest) {
            state.sum_store_size -= entry.data.len() as u64;
            // Note: See comment in `unref()` requring global lock of insert/remove.
            entry.data.unref().await;
            return true;
        }
        false
    }

    /// Same as remove(), but allows for a conditional to be applied to the entry before removal
    /// in an atomic fashion.
    pub async fn remove_if<F: FnOnce(&T) -> bool>(&self, digest: &DigestInfo, cond: F) -> bool {
        let mut state = self.state.lock().await;
        if let Some(entry) = state.lru.get(digest) {
            if !cond(&entry.data) {
                return false;
            }
            return self.inner_remove(&mut state, digest).await;
        }
        false
    }
}
