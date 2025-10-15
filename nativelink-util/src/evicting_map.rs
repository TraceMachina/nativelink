// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::borrow::Borrow;
use core::cmp::Eq;
use core::fmt::Debug;
use core::future::Future;
use core::hash::Hash;
use core::ops::RangeBounds;
use std::collections::BTreeSet;
use std::sync::Arc;

use lru::LruCache;
use nativelink_config::stores::EvictionPolicy;
use nativelink_metric::MetricsComponent;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tonic::async_trait;
use tracing::{debug, info};

use crate::instant_wrapper::InstantWrapper;
use crate::metrics_utils::{Counter, CounterWithTime};

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub struct SerializedLRU<K> {
    pub data: Vec<(K, u32)>,
    pub anchor_time: u64,
}

#[derive(Debug)]
struct EvictionItem<T: LenEntry + Debug> {
    seconds_since_anchor: u32,
    data: T,
}

pub trait LenEntry: 'static {
    /// Length of referenced data.
    fn len(&self) -> u64;

    /// Returns `true` if `self` has zero length.
    fn is_empty(&self) -> bool;

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
    /// the `EvictionMap` globally (including inside `unref()`).
    #[inline]
    fn unref(&self) -> impl Future<Output = ()> + Send {
        core::future::ready(())
    }
}

impl<T: LenEntry + Send + Sync> LenEntry for Arc<T> {
    #[inline]
    fn len(&self) -> u64 {
        T::len(self.as_ref())
    }

    #[inline]
    fn is_empty(&self) -> bool {
        T::is_empty(self.as_ref())
    }

    #[inline]
    async fn unref(&self) {
        self.as_ref().unref().await;
    }
}

// Callback to be called when the EvictingMap removes an item
// either via eviction or direct deletion. This will be called with
// whatever key type the EvictingMap uses.
#[async_trait]
pub trait RemoveStateCallback<Q>: Debug + Send + Sync {
    async fn callback(&self, key: &Q);
}

#[derive(Debug, MetricsComponent)]
struct State<
    K: Ord + Hash + Eq + Clone + Debug + Send + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug,
    T: LenEntry + Debug + Send,
> {
    lru: LruCache<K, EvictionItem<T>>,
    btree: Option<BTreeSet<K>>,
    #[metric(help = "Total size of all items in the store")]
    sum_store_size: u64,

    #[metric(help = "Number of bytes evicted from the store")]
    evicted_bytes: Counter,
    #[metric(help = "Number of items evicted from the store")]
    evicted_items: CounterWithTime,
    #[metric(help = "Number of bytes replaced in the store")]
    replaced_bytes: Counter,
    #[metric(help = "Number of items replaced in the store")]
    replaced_items: CounterWithTime,
    #[metric(help = "Number of bytes inserted into the store since it was created")]
    lifetime_inserted_bytes: Counter,
    #[metric(help = "Number of eviction attempts blocked by grace period")]
    grace_period_blocks: Counter,

    remove_callbacks: Arc<Mutex<Vec<Box<dyn RemoveStateCallback<Q>>>>>,
}

impl<
    K: Ord + Hash + Eq + Clone + Debug + Send + Sync + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug + Sync,
    T: LenEntry + Debug + Sync + Send,
> State<K, Q, T>
{
    /// Removes an item from the cache and returns the data for deferred cleanup.
    /// The caller is responsible for calling `unref()` on the returned data outside of the lock.
    #[must_use]
    async fn remove(&mut self, key: &Q, eviction_item: &EvictionItem<T>, replaced: bool) -> T
    where
        T: Clone,
    {
        if let Some(btree) = &mut self.btree {
            btree.remove(key.borrow());
        }
        self.sum_store_size -= eviction_item.data.len();
        if replaced {
            self.replaced_items.inc();
            self.replaced_bytes.add(eviction_item.data.len());
        } else {
            self.evicted_items.inc();
            self.evicted_bytes.add(eviction_item.data.len());
        }

        let locked_callbacks = self.remove_callbacks.lock_arc();
        for callback in locked_callbacks.iter() {
            callback.callback(key).await;
        }

        // Return the data for deferred unref outside of lock
        eviction_item.data.clone()
    }

    /// Inserts a new item into the cache. If the key already exists, the old item is returned
    /// for deferred cleanup.
    #[must_use]
    async fn put(&mut self, key: &K, eviction_item: EvictionItem<T>) -> Option<T>
    where
        K: Clone,
        T: Clone,
    {
        // If we are maintaining a btree index, we need to update it.
        if let Some(btree) = &mut self.btree {
            btree.insert(key.clone());
        }
        if let Some(old_item) = self.lru.put(key.clone(), eviction_item) {
            let old_data = self.remove(key.borrow(), &old_item, true).await;
            return Some(old_data);
        }
        None
    }

    fn add_remove_callback(&self, callback: Box<dyn RemoveStateCallback<Q>>) {
        self.remove_callbacks.lock_arc().push(callback);
    }
}

#[derive(Debug, MetricsComponent)]
pub struct EvictingMap<
    K: Ord + Hash + Eq + Clone + Debug + Send + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug,
    T: LenEntry + Debug + Send,
    I: InstantWrapper,
> {
    #[metric]
    state: Arc<Mutex<State<K, Q, T>>>,
    anchor_time: I,
    #[metric(help = "Maximum size of the store in bytes")]
    max_bytes: u64,
    #[metric(help = "Number of bytes to evict when the store is full")]
    evict_bytes: u64,
    #[metric(help = "Maximum number of seconds to keep an item in the store")]
    max_seconds: u32,
    #[metric(help = "Maximum number of items to keep in the store")]
    max_count: u64,
    #[metric(help = "Grace period in seconds to prevent eviction of recently accessed items")]
    eviction_grace_period_seconds: u32,
}

impl<K, Q, T, I> EvictingMap<K, Q, T, I>
where
    K: Ord + Hash + Eq + Clone + Debug + Send + Sync + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug + Sync,
    T: LenEntry + Debug + Clone + Send + Sync,
    I: InstantWrapper,
{
    pub fn new(config: &EvictionPolicy, anchor_time: I) -> Self {
        Self {
            // We use unbounded because if we use the bounded version we can't call the delete
            // function on the LenEntry properly.
            state: Arc::new(Mutex::new(State {
                lru: LruCache::unbounded(),
                btree: None,
                sum_store_size: 0,
                evicted_bytes: Counter::default(),
                evicted_items: CounterWithTime::default(),
                replaced_bytes: Counter::default(),
                replaced_items: CounterWithTime::default(),
                lifetime_inserted_bytes: Counter::default(),
                grace_period_blocks: Counter::default(),
                remove_callbacks: Arc::new(Mutex::new(vec![])),
            })),
            anchor_time,
            max_bytes: config.max_bytes as u64,
            evict_bytes: config.evict_bytes as u64,
            max_seconds: config.max_seconds,
            max_count: config.max_count,
            eviction_grace_period_seconds: config.eviction_grace_period_seconds,
        }
    }

    pub async fn enable_filtering(&self) {
        let mut state = self.state.lock_arc();
        if state.btree.is_none() {
            Self::rebuild_btree_index(&mut state);
        }
    }

    fn rebuild_btree_index(state: &mut State<K, Q, T>) {
        state.btree = Some(state.lru.iter().map(|(k, _)| k).cloned().collect());
    }

    /// Run the `handler` function on each key-value pair that matches the `prefix_range`
    /// and return the number of items that were processed.
    /// The `handler` function should return `true` to continue processing the next item
    /// or `false` to stop processing.
    pub async fn range<F>(&self, prefix_range: impl RangeBounds<Q> + Send, mut handler: F) -> u64
    where
        F: FnMut(&K, &T) -> bool + Send,
        K: Ord,
    {
        let mut state = self.state.lock_arc();
        let btree = if let Some(ref btree) = state.btree {
            btree
        } else {
            Self::rebuild_btree_index(&mut state);
            state.btree.as_ref().unwrap()
        };
        let mut continue_count = 0;
        for key in btree.range(prefix_range) {
            let value = &state.lru.peek(key.borrow()).unwrap().data;
            let should_continue = handler(key, value);
            if !should_continue {
                break;
            }
            continue_count += 1;
        }
        continue_count
    }

    /// Returns the number of key-value pairs that are currently in the the cache.
    /// Function is not for production code paths.
    pub async fn len_for_test(&self) -> usize {
        self.state.lock_arc().lru.len()
    }

    /// Returns (should_evict, within_grace_period)
    /// When force_evict is true, grace period is ignored (for critical disk situations)
    fn should_evict_with_grace_check(
        &self,
        lru_len: usize,
        peek_entry: &EvictionItem<T>,
        sum_store_size: u64,
        max_bytes: u64,
        force_evict: bool,
    ) -> (bool, bool) {
        // Check if item is within grace period (unless forced)
        let within_grace_period = if !force_evict && self.eviction_grace_period_seconds > 0 {
            let current_time_seconds = self.anchor_time.elapsed().as_secs() as u32;
            let item_age = current_time_seconds.saturating_sub(peek_entry.seconds_since_anchor);
            item_age < self.eviction_grace_period_seconds
        } else {
            false
        };

        // If within grace period and not forced, do not evict
        if within_grace_period {
            return (false, true);
        }

        let is_over_size = max_bytes != 0 && sum_store_size >= max_bytes;

        let evict_older_than_seconds = (self.anchor_time.elapsed().as_secs() as u32)
            .saturating_sub(self.max_seconds);
        let old_item_exists =
            self.max_seconds != 0 && peek_entry.seconds_since_anchor < evict_older_than_seconds;

        let is_over_count = self.max_count != 0 && (lru_len as u64) > self.max_count;

        (is_over_size || old_item_exists || is_over_count, false)
    }

    #[must_use]
    async fn evict_items(&self, state: &mut State<K, Q, T>) -> Vec<T> {
        let Some((_, mut peek_entry)) = state.lru.peek_lru() else {
            return Vec::new();
        };

        // Optimize: Only check eviction if we have eviction policies configured
        // Calculate the eviction watermark (lower threshold to evict to)
        let max_bytes = if self.max_bytes != 0 && self.evict_bytes != 0 {
            // Check if we should start evicting
            let (should_evict, _) = self.should_evict_with_grace_check(
                state.lru.len(),
                peek_entry,
                state.sum_store_size,
                self.max_bytes,
                false, // Not forced yet
            );

            // If evicting, set a lower watermark to reduce thrashing
            if should_evict {
                self.max_bytes.saturating_sub(self.evict_bytes)
            } else {
                self.max_bytes
            }
        } else {
            self.max_bytes
        };

        let mut items_to_unref = Vec::new();

        loop {
            // Force eviction only if we're at a critical threshold (150% of max_bytes)
            // This prevents disk space issues while still respecting grace period under normal conditions
            let force_evict = self.max_bytes != 0
                && state.sum_store_size >= self.max_bytes.saturating_add(self.max_bytes / 2);

            let (should_evict, within_grace_period) = self.should_evict_with_grace_check(
                state.lru.len(),
                peek_entry,
                state.sum_store_size,
                max_bytes,
                force_evict,
            );

            if !should_evict {
                if within_grace_period {
                    // Track that grace period blocked an eviction
                    state.grace_period_blocks.inc();
                    debug!(
                        "Grace period preventing eviction of LRU item (age < {} seconds)",
                        self.eviction_grace_period_seconds
                    );
                }
                break;
            }

            let (key, eviction_item) = state
                .lru
                .pop_lru()
                .expect("Tried to peek() then pop() but failed");
            debug!(?key, "Evicting",);
            let data = state.remove(key.borrow(), &eviction_item, false).await;
            items_to_unref.push(data);

            peek_entry = if let Some((_, entry)) = state.lru.peek_lru() {
                entry
            } else {
                break;
            };
        }

        items_to_unref
    }

    /// Return the size of a `key`, if not found `None` is returned.
    pub async fn size_for_key(&self, key: &Q) -> Option<u64>
    where
        Q: Sync,
    {
        let mut results = [None];
        self.sizes_for_keys([key], &mut results[..], false).await;
        results[0]
    }

    /// Return the sizes of a collection of `keys`. Expects `results` collection
    /// to be provided for storing the resulting key sizes. Each index value in
    /// `keys` maps directly to the size value for the key in `results`.
    /// If no key is found in the internal map, `None` is filled in its place.
    /// If `peek` is set to `true`, the items are not promoted to the front of the
    /// LRU cache. Note: peek may still evict, but won't promote.
    pub async fn sizes_for_keys<It, R>(&self, keys: It, results: &mut [Option<u64>], peek: bool)
    where
        It: IntoIterator<Item = R> + Send,
        // Note: It's not enough to have the inserts themselves be Send. The
        // returned iterator should be Send as well.
        <It as IntoIterator>::IntoIter: Send,
        // This may look strange, but what we are doing is saying:
        // * `K` must be able to borrow `Q`
        // * `R` (the input stream item type) must also be able to borrow `Q`
        // Note: That K and R do not need to be the same type, they just both need
        // to be able to borrow a `Q`.
        R: Borrow<Q> + Send,
    {
        let mut state = self.state.lock_arc();

        let lru_len = state.lru.len();
        for (key, result) in keys.into_iter().zip(results.iter_mut()) {
            let maybe_entry = if peek {
                state.lru.peek_mut(key.borrow())
            } else {
                state.lru.get_mut(key.borrow())
            };
            match maybe_entry {
                Some(entry) => {
                    // Note: We need to check eviction because the item might be expired
                    // based on the current time. In such case, we remove the item while
                    // we are here.
                    let (should_evict, _) =
                        self.should_evict_with_grace_check(lru_len, entry, 0, u64::MAX, false);
                    if should_evict {
                        *result = None;
                        if let Some((key, eviction_item)) = state.lru.pop_entry(key.borrow()) {
                            info!(?key, "Item expired, evicting");
                            let data = state.remove(key.borrow(), &eviction_item, false).await;
                            // Store data for later unref - we can't drop state here as we're still iterating
                            // The unref will happen after the method completes
                            // For now, we just do inline unref
                            data.unref().await;
                        }
                    } else {
                        if !peek {
                            entry.seconds_since_anchor =
                                self.anchor_time.elapsed().as_secs() as u32;
                        }
                        *result = Some(entry.data.len());
                    }
                }
                None => *result = None,
            }
        }
    }

    pub async fn get(&self, key: &Q) -> Option<T> {
        // Fast path: Check if we need eviction before acquiring lock for eviction
        let needs_eviction = {
            let state = self.state.lock_arc();
            if let Some((_, peek_entry)) = state.lru.peek_lru() {
                let (should_evict, _) = self.should_evict_with_grace_check(
                    state.lru.len(),
                    peek_entry,
                    state.sum_store_size,
                    self.max_bytes,
                    false,
                );
                should_evict
            } else {
                false
            }
        };

        // Perform eviction if needed
        if needs_eviction {
            let items_to_unref = {
                let mut state = self.state.lock_arc();
                self.evict_items(&mut *state).await
            };
            // Unref items outside of lock
            for item in items_to_unref {
                item.unref().await;
            }
        }

        // Now get the item
        let mut state = self.state.lock_arc();
        let entry = state.lru.get_mut(key.borrow())?;
        entry.seconds_since_anchor = self.anchor_time.elapsed().as_secs() as u32;
        Some(entry.data.clone())
    }

    /// Returns the replaced item if any.
    pub async fn insert(&self, key: K, data: T) -> Option<T>
    where
        K: 'static,
    {
        self.insert_with_time(key, data, self.anchor_time.elapsed().as_secs() as u32)
            .await
    }

    /// Returns the replaced item if any.
    pub async fn insert_with_time(&self, key: K, data: T, seconds_since_anchor: u32) -> Option<T> {
        let items_to_unref = {
            let mut state = self.state.lock_arc();
            self.inner_insert_many(&mut state, [(key, data)], seconds_since_anchor)
                .await
        };

        // Unref items outside of lock
        let mut results = Vec::new();
        for item in items_to_unref {
            item.unref().await;
            results.push(item);
        }

        results.into_iter().next()
    }

    /// Same as `insert()`, but optimized for multiple inserts.
    /// Returns the replaced items if any.
    pub async fn insert_many<It>(&self, inserts: It) -> Vec<T>
    where
        It: IntoIterator<Item = (K, T)> + Send,
        // Note: It's not enough to have the inserts themselves be Send. The
        // returned iterator should be Send as well.
        <It as IntoIterator>::IntoIter: Send,
        K: 'static,
    {
        let mut inserts = inserts.into_iter().peekable();
        // Shortcut for cases where there are no inserts, so we don't need to lock.
        if inserts.peek().is_none() {
            return Vec::new();
        }

        let items_to_unref = {
            let state = &mut self.state.lock_arc();
            self.inner_insert_many(state, inserts, self.anchor_time.elapsed().as_secs() as u32)
                .await
        };

        // Unref items outside of lock
        let mut results = Vec::new();
        for item in items_to_unref {
            item.unref().await;
            results.push(item);
        }

        results
    }

    async fn inner_insert_many<It>(
        &self,
        state: &mut State<K, Q, T>,
        inserts: It,
        seconds_since_anchor: u32,
    ) -> Vec<T>
    where
        It: IntoIterator<Item = (K, T)> + Send,
        // Note: It's not enough to have the inserts themselves be Send. The
        // returned iterator should be Send as well.
        <It as IntoIterator>::IntoIter: Send,
    {
        let mut replaced_items = Vec::new();
        for (key, data) in inserts {
            let new_item_size = data.len();
            let eviction_item = EvictionItem {
                seconds_since_anchor,
                data,
            };

            if let Some(old_item) = state.put(&key, eviction_item).await {
                replaced_items.push(old_item);
            }
            state.sum_store_size += new_item_size;
            state.lifetime_inserted_bytes.add(new_item_size);
        }

        // Perform eviction after all insertions
        let items_to_unref = self.evict_items(state).await;

        // Note: We cannot drop the state lock here since we're borrowing it,
        // but the caller will handle unreffing these items after releasing the lock
        for item in items_to_unref {
            replaced_items.push(item);
        }

        replaced_items
    }

    pub async fn remove(&self, key: &Q) -> bool {
        let (items_to_unref, removed_item) = {
            let mut state = self.state.lock_arc();

            // First perform eviction
            let evicted_items = self.evict_items(&mut *state).await;

            // Then try to remove the requested item
            let removed = if let Some(entry) = state.lru.pop(key.borrow()) {
                Some(state.remove(key, &entry, false).await)
            } else {
                None
            };

            (evicted_items, removed)
        };

        // Unref evicted items outside of lock
        for item in items_to_unref {
            item.unref().await;
        }

        // Unref removed item if any
        if let Some(item) = removed_item {
            item.unref().await;
            return true;
        }

        false
    }

    /// Same as `remove()`, but allows for a conditional to be applied to the
    /// entry before removal in an atomic fashion.
    pub async fn remove_if<F>(&self, key: &Q, cond: F) -> bool
    where
        F: FnOnce(&T) -> bool + Send,
    {
        let mut state = self.state.lock_arc();
        if let Some(entry) = state.lru.get(key.borrow()) {
            if !cond(&entry.data) {
                return false;
            }
            // First perform eviction
            let evicted_items = self.evict_items(&mut state).await;

            // Then try to remove the requested item
            let removed_item = if let Some(entry) = state.lru.pop(key.borrow()) {
                Some(state.remove(key, &entry, false).await)
            } else {
                None
            };

            // Drop the lock before unref operations
            drop(state);

            // Unref evicted items
            for item in evicted_items {
                item.unref().await;
            }

            // Unref removed item if any
            if let Some(item) = removed_item {
                item.unref().await;
                return true;
            }

            return false;
        }
        false
    }

    pub fn add_remove_callback(&self, callback: Box<dyn RemoveStateCallback<Q>>) {
        self.state.lock_arc().add_remove_callback(callback);
    }
}
