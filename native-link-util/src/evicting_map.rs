// Copyright 2023 The Native Link Authors. All rights reserved.
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
use futures::stream::FuturesUnordered;
use futures::{future, StreamExt};
use lru::LruCache;
use native_link_config::stores::EvictionPolicy;
use serde::{Deserialize, Serialize};

use crate::common::{log, DigestInfo};
use crate::metrics_utils::{CollectorState, Counter, CounterWithTime, MetricsComponent};

#[derive(Serialize, Deserialize, PartialEq, Debug, Clone)]
pub struct SerializedLRU {
    pub data: Vec<(DigestInfo, i32)>,
    pub anchor_time: u64,
}

/// Wrapper used to abstract away which underlying Instant impl we are using.
/// This is needed for testing.
pub trait InstantWrapper: 'static {
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
pub trait LenEntry: 'static {
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

    // Metrics.
    evicted_bytes: Counter,
    evicted_items: CounterWithTime,
    replaced_bytes: Counter,
    replaced_items: CounterWithTime,
    removed_bytes: Counter,
    removed_items: CounterWithTime,
    lifetime_inserted_bytes: Counter,
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
                evicted_bytes: Counter::default(),
                evicted_items: CounterWithTime::default(),
                replaced_bytes: Counter::default(),
                replaced_items: CounterWithTime::default(),
                removed_bytes: Counter::default(),
                removed_items: CounterWithTime::default(),
                lifetime_inserted_bytes: Counter::default(),
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
        let Some((_, mut peek_entry)) = state.lru.peek_lru() else {
            return;
        };

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
            state.evicted_items.inc();
            state.evicted_bytes.add(eviction_item.data.len() as u64);
            // Note: See comment in `unref()` requring global lock of insert/remove.
            eviction_item.data.unref().await;
            log::info!("\x1b[0;31mEvicting Map\x1b[0m: Evicting {}", key.hash_str());

            peek_entry = if let Some((_, entry)) = state.lru.peek_lru() {
                entry
            } else {
                return;
            };
        }
    }

    pub async fn size_for_key(&self, digest: &DigestInfo) -> Option<usize> {
        let mut state = self.state.lock().await;
        let Some(entry) = state.lru.get_mut(digest) else {
            return None;
        };
        entry.seconds_since_anchor = self.anchor_time.elapsed().as_secs() as i32;
        let data = entry.data.clone();
        drop(state);
        data.touch().await;
        Some(data.len())
    }

    pub async fn sizes_for_keys(&self, digests: &[DigestInfo], results: &mut [Option<usize>]) {
        let mut state = self.state.lock().await;
        let seconds_since_anchor = self.anchor_time.elapsed().as_secs() as i32;
        let to_touch: Vec<T> = digests
            .iter()
            .zip(results.iter_mut())
            .filter_map(|(digest, result)| {
                let Some(entry) = state.lru.get_mut(digest) else {
                    return None;
                };
                entry.seconds_since_anchor = seconds_since_anchor;
                let data = entry.data.clone();
                *result = Some(data.len());
                Some(data)
            })
            .collect();
        drop(state);
        to_touch
            .iter()
            .map(|data| data.touch())
            .collect::<FuturesUnordered<_>>()
            .for_each(|_| future::ready(()))
            .await;
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
        let mut state = self.state.lock().await;
        let results = self
            .inner_insert_many(&mut state, [(digest, data)], seconds_since_anchor)
            .await;
        results.into_iter().next()
    }

    /// Same as insert(), but optimized for multiple inserts.
    /// Returns the replaced items if any.
    pub async fn insert_many(&self, inserts: impl IntoIterator<Item = (DigestInfo, T)>) -> Vec<T> {
        let mut inserts = inserts.into_iter().peekable();
        // Shortcut for cases where there are no inserts, so we don't need to lock.
        if inserts.peek().is_none() {
            return Vec::new();
        }
        let state = &mut self.state.lock().await;
        self.inner_insert_many(state, inserts, self.anchor_time.elapsed().as_secs() as i32)
            .await
    }

    async fn inner_insert_many(
        &self,
        mut state: &mut State<T>,
        inserts: impl IntoIterator<Item = (DigestInfo, T)>,
        seconds_since_anchor: i32,
    ) -> Vec<T> {
        let mut replaced_items = Vec::new();
        for (digest, data) in inserts.into_iter() {
            let new_item_size = data.len() as u64;
            let eviction_item = EvictionItem {
                seconds_since_anchor,
                data,
            };

            if let Some(old_item) = state.lru.put(digest, eviction_item) {
                state.sum_store_size -= old_item.data.len() as u64;
                state.replaced_items.inc();
                state.replaced_bytes.add(old_item.data.len() as u64);
                // Note: See comment in `unref()` requring global lock of insert/remove.
                old_item.data.unref().await;
                replaced_items.push(old_item.data);
            }
            state.sum_store_size += new_item_size;
            state.lifetime_inserted_bytes.add(new_item_size);
            self.evict_items(state.deref_mut()).await;
        }
        replaced_items
    }

    pub async fn remove(&self, digest: &DigestInfo) -> bool {
        let mut state = self.state.lock().await;
        self.inner_remove(&mut state, digest).await
    }

    async fn inner_remove(&self, state: &mut State<T>, digest: &DigestInfo) -> bool {
        if let Some(entry) = state.lru.pop(digest) {
            let data_len = entry.data.len() as u64;
            state.sum_store_size -= data_len;
            state.removed_items.inc();
            state.removed_bytes.add(data_len);
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

impl<T: LenEntry + Debug, I: InstantWrapper> MetricsComponent for EvictingMap<T, I> {
    fn gather_metrics(&self, c: &mut CollectorState) {
        c.publish("max_bytes", &self.max_bytes, "Maximum size of the store in bytes");
        c.publish(
            "evict_bytes",
            &self.evict_bytes,
            "Number of bytes to evict when the store is full",
        );
        c.publish(
            "anchor_time_timestamp",
            &self.anchor_time.unix_timestamp(),
            "Anchor time for the store",
        );
        c.publish(
            "max_seconds",
            &self.max_seconds,
            "Maximum number of seconds to keep an item in the store",
        );
        c.publish(
            "max_count",
            &self.max_count,
            "Maximum number of items to keep in the store",
        );
        futures::executor::block_on(async move {
            let state = self.state.lock().await;
            c.publish(
                "sum_store_size_bytes",
                &state.sum_store_size,
                "Total size of all items in the store",
            );
            c.publish("items_in_store_total", &state.lru.len(), "Mumber of items in the store");
            c.publish(
                "oldest_item_timestamp",
                &state
                    .lru
                    .peek_lru()
                    .map(|(_, v)| self.anchor_time.unix_timestamp() as i64 - v.seconds_since_anchor as i64)
                    .unwrap_or(-1),
                "Timestamp of the oldest item in the store",
            );
            c.publish(
                "newest_item_timestamp",
                &state
                    .lru
                    .iter()
                    .next()
                    .map(|(_, v)| self.anchor_time.unix_timestamp() as i64 - v.seconds_since_anchor as i64)
                    .unwrap_or(-1),
                "Timestamp of the newest item in the store",
            );
            c.publish(
                "evicted_items_total",
                &state.evicted_items,
                "Number of items evicted from the store",
            );
            c.publish(
                "evicted_bytes",
                &state.evicted_bytes,
                "Number of bytes evicted from the store",
            );
            c.publish(
                "lifetime_inserted_bytes",
                &state.lifetime_inserted_bytes,
                "Number of bytes inserted into the store since it was created",
            );
            c.publish(
                "replaced_bytes",
                &state.replaced_bytes,
                "Number of bytes replaced in the store",
            );
            c.publish(
                "replaced_items_total",
                &state.replaced_items,
                "Number of items replaced in the store",
            );
            c.publish(
                "removed_bytes",
                &state.removed_bytes,
                "Number of bytes explicitly removed from the store",
            );
            c.publish(
                "removed_items_total",
                &state.removed_items,
                "Number of items explicitly removed from the store",
            );
            c.publish_stats(
                "item_size_bytes",
                state.lru.iter().take(1_000_000).map(|(_, v)| v.data.len()),
                "Stats about the first 1_000_000 items in the store (these are newest items in the store)",
            );
        });
    }
}
