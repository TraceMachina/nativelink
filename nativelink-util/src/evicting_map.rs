// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use core::borrow::Borrow;
use core::cmp::Eq;
use core::fmt::Debug;
use core::future::Future;
use core::hash::Hash;
use core::ops::RangeBounds;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use async_lock::Mutex;
use lru::LruCache;
use nativelink_config::stores::EvictionPolicy;
use nativelink_metric::MetricsComponent;
use opentelemetry::KeyValue;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::instant_wrapper::InstantWrapper;
use crate::metrics::{CACHE_METRICS, CacheMetricAttrs};

// Sentinel value for overflows so that we don't introduce branches or error
// handling in highly unlikely error branches of size conversions that would
// only be hit beyond ~9 Exabytes.
const METRIC_SIZE_OVERFLOW: i64 = i64::MAX - 1;

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug, Clone)]
pub struct SerializedLRU<K> {
    pub data: Vec<(K, i32)>,
    pub anchor_time: u64,
}

#[derive(Debug)]
struct EvictionItem<T: LenEntry + Debug> {
    seconds_since_anchor: i32,
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

#[derive(Debug, MetricsComponent)]
struct State<K: Ord + Hash + Eq + Clone + Debug + Send, T: LenEntry + Debug + Send> {
    lru: LruCache<K, EvictionItem<T>>,
    btree: Option<BTreeSet<K>>,
    // Total size of all items in the store.
    sum_store_size: u64,
}

impl<K: Ord + Hash + Eq + Clone + Debug + Send + Sync, T: LenEntry + Debug + Sync + Send>
    State<K, T>
{
    /// Removes an item from the cache.
    async fn remove<Q>(&mut self, key: &Q, eviction_item: &EvictionItem<T>)
    where
        K: Borrow<Q>,
        Q: Ord + Hash + Eq + Debug + Sync,
    {
        if let Some(btree) = &mut self.btree {
            btree.remove(key.borrow());
        }

        self.sum_store_size = self.sum_store_size.saturating_sub(eviction_item.data.len());
        // Note: See comment in `unref()` requiring global lock of insert/remove.
        eviction_item.data.unref().await;
    }

    /// Inserts a new item into the cache. If the key already exists, the old item is returned.
    async fn put(&mut self, key: K, eviction_item: EvictionItem<T>) -> (Option<T>, u64) {
        // If we are maintaining a btree index, we need to update it.
        if let Some(btree) = &mut self.btree {
            btree.insert(key.clone());
        }

        let new_item_size = eviction_item.data.len();

        // Update value if it already exists.
        if let Some(old_item) = self.lru.put(key, eviction_item) {
            let old_item_size = old_item.data.len();
            // Update sum_store_size with net difference
            self.sum_store_size = self
                .sum_store_size
                .saturating_sub(old_item_size)
                .saturating_add(new_item_size);

            old_item.data.unref().await;

            return (Some(old_item.data), old_item_size);
        }

        // If the value didn't exist, it's a new insertion.
        self.sum_store_size = self.sum_store_size.saturating_add(new_item_size);
        (None, 0)
    }
}

#[derive(Debug, MetricsComponent)]
pub struct EvictingMap<
    K: Ord + Hash + Eq + Clone + Debug + Send,
    T: LenEntry + Debug + Send,
    I: InstantWrapper,
> {
    #[metric]
    state: Mutex<State<K, T>>,
    anchor_time: I,
    /// Maximum size of the store in bytes.
    max_bytes: u64,
    /// Number of bytes to evict when the store is full.
    evict_bytes: u64,
    /// Maximum number of seconds to keep an item in the store.
    max_seconds: i32,
    // Maximum number of items to keep in the store.
    max_count: u64,
    /// Pre-allocated cache metric attributes.
    metric_attrs: CacheMetricAttrs,
    /// Pre-allocated attributes for metrics.
    base_metric_attrs: Vec<KeyValue>,
}

impl<K, T, I> EvictingMap<K, T, I>
where
    K: Ord + Hash + Eq + Clone + Debug + Send + Sync,
    T: LenEntry + Debug + Clone + Send + Sync,
    I: InstantWrapper,
{
    pub fn new(config: &EvictionPolicy, anchor_time: I, attributes: &[KeyValue]) -> Self {
        // TODO(aaronmondal): This is out of place. The proper way to handle
        //                    this seems to be removing the StoreManager and
        //                    constructing the store layout by directly using
        //                    the graph structure implied by the configuration.
        //
        //                    In the meantime, the allocation here is not the
        //                    end of the world as it only happens once for each
        //                    construction.
        let mut base_metric_attrs = vec![KeyValue::new("instance_name", "unknown")];
        base_metric_attrs.extend_from_slice(attributes);

        Self {
            // We use unbounded because if we use the bounded version we can't call the delete
            // function on the LenEntry properly.
            state: Mutex::new(State {
                lru: LruCache::unbounded(),
                btree: None,
                sum_store_size: 0,
            }),
            anchor_time,
            max_bytes: config.max_bytes as u64,
            evict_bytes: config.evict_bytes as u64,
            max_seconds: config.max_seconds as i32,
            max_count: config.max_count,
            metric_attrs: CacheMetricAttrs::new(&base_metric_attrs),
            base_metric_attrs,
        }
    }

    pub async fn enable_filtering(&self) {
        let mut state = self.state.lock().await;
        if state.btree.is_none() {
            Self::rebuild_btree_index(&mut state);
        }
    }

    fn rebuild_btree_index(state: &mut State<K, T>) {
        state.btree = Some(state.lru.iter().map(|(k, _)| k).cloned().collect());
    }

    /// Run the `handler` function on each key-value pair that matches the `prefix_range`
    /// and return the number of items that were processed.
    /// The `handler` function should return `true` to continue processing the next item
    /// or `false` to stop processing.
    pub async fn range<F, Q>(&self, prefix_range: impl RangeBounds<Q> + Send, mut handler: F) -> u64
    where
        F: FnMut(&K, &T) -> bool + Send,
        K: Borrow<Q> + Ord,
        Q: Ord + Hash + Eq + Debug + Sync,
    {
        let mut state = self.state.lock().await;
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
        self.state.lock().await.lru.len()
    }

    fn should_evict(
        &self,
        lru_len: usize,
        peek_entry: &EvictionItem<T>,
        sum_store_size: u64,
        max_bytes: u64,
    ) -> bool {
        let is_over_size = max_bytes != 0 && sum_store_size >= max_bytes;

        let evict_older_than_seconds =
            (self.anchor_time.elapsed().as_secs() as i32) - self.max_seconds;
        let old_item_exists =
            self.max_seconds != 0 && peek_entry.seconds_since_anchor < evict_older_than_seconds;

        let is_over_count = self.max_count != 0 && (lru_len as u64) > self.max_count;

        is_over_size || old_item_exists || is_over_count
    }

    async fn evict_items(&self, state: &mut State<K, T>) {
        let Some((_, mut peek_entry)) = state.lru.peek_lru() else {
            return;
        };

        let max_bytes = if self.max_bytes != 0
            && self.evict_bytes != 0
            && self.should_evict(
                state.lru.len(),
                peek_entry,
                state.sum_store_size,
                self.max_bytes,
            ) {
            self.max_bytes.saturating_sub(self.evict_bytes)
        } else {
            self.max_bytes
        };

        while self.should_evict(state.lru.len(), peek_entry, state.sum_store_size, max_bytes) {
            let (key, eviction_item) = state
                .lru
                .pop_lru()
                .expect("Tried to peek() then pop() but failed");

            let item_size = eviction_item.data.len();
            debug!(?key, "Evicting",);

            let metrics = &*CACHE_METRICS;

            state.remove(&key, &eviction_item).await;

            // TODO(aaronmondal): Is it true that this cannot fail?
            metrics
                .cache_operations
                .add(1, self.metric_attrs.evict_success());
            metrics
                .cache_io
                .add(item_size, self.metric_attrs.evict_success());
            metrics.cache_size.add(
                -(i64::try_from(item_size).unwrap_or(METRIC_SIZE_OVERFLOW)),
                &self.base_metric_attrs,
            );
            metrics.cache_entries.add(-1, &self.base_metric_attrs);
            metrics
                .cache_entry_size
                .record(item_size, &self.base_metric_attrs);

            peek_entry = if let Some((_, entry)) = state.lru.peek_lru() {
                entry
            } else {
                return;
            };
        }
    }

    /// Return the size of a `key`, if not found `None` is returned.
    pub async fn size_for_key<Q>(&self, key: &Q) -> Option<u64>
    where
        K: Borrow<Q>,
        Q: Ord + Hash + Eq + Debug + Sync,
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
    pub async fn sizes_for_keys<It, Q, R>(&self, keys: It, results: &mut [Option<u64>], peek: bool)
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
        K: Borrow<Q>,
        R: Borrow<Q> + Send,
        Q: Ord + Hash + Eq + Debug + Sync,
    {
        let metrics = &*CACHE_METRICS;
        let mut state = self.state.lock().await;

        let lru_len = state.lru.len();
        for (key, result) in keys.into_iter().zip(results.iter_mut()) {
            let start_time = Instant::now();

            let maybe_entry = if peek {
                state.lru.peek_mut(key.borrow())
            } else {
                state.lru.get_mut(key.borrow())
            };

            if let Some(entry) = maybe_entry {
                // Note: We need to check eviction because the item might be expired
                // based on the current time. In such case, we remove the item while
                // we are here.
                if self.should_evict(lru_len, entry, 0, u64::MAX) {
                    *result = None;
                    if let Some((key, eviction_item)) = state.lru.pop_entry(key.borrow()) {
                        info!(?key, "Item expired, evicting");
                        let item_size = eviction_item.data.len();

                        state.remove(key.borrow(), &eviction_item).await;

                        metrics
                            .cache_operations
                            .add(1, self.metric_attrs.evict_expired());
                        metrics
                            .cache_io
                            .add(item_size, self.metric_attrs.evict_expired());
                        metrics.cache_operation_duration.record(
                            start_time.elapsed().as_secs_f64() * 1000.0,
                            self.metric_attrs.evict_expired(),
                        );
                        metrics.cache_size.add(
                            -(i64::try_from(item_size).unwrap_or(METRIC_SIZE_OVERFLOW)),
                            &self.base_metric_attrs,
                        );
                        metrics.cache_entries.add(-1, &self.base_metric_attrs);
                    }
                } else {
                    if !peek {
                        entry.seconds_since_anchor = self.anchor_time.elapsed().as_secs() as i32;
                    }
                    let data_len = entry.data.len();
                    *result = Some(data_len);

                    metrics
                        .cache_operations
                        .add(1, self.metric_attrs.read_hit());
                    metrics.cache_io.add(data_len, self.metric_attrs.read_hit());
                    metrics.cache_operation_duration.record(
                        start_time.elapsed().as_secs_f64() * 1000.0,
                        self.metric_attrs.read_hit(),
                    );
                    metrics
                        .cache_entry_size
                        .record(data_len, &self.base_metric_attrs);
                }
            } else {
                *result = None;

                metrics
                    .cache_operations
                    .add(1, self.metric_attrs.read_miss());
                metrics.cache_operation_duration.record(
                    start_time.elapsed().as_secs_f64() * 1000.0,
                    self.metric_attrs.read_miss(),
                );
            }
        }
    }

    pub async fn get<Q>(&self, key: &Q) -> Option<T>
    where
        K: Borrow<Q>,
        Q: Ord + Hash + Eq + Debug + Sync,
    {
        let start_time = Instant::now();
        let metrics = &*CACHE_METRICS;
        let mut state = self.state.lock().await;

        self.evict_items(&mut *state).await;

        let entry = state.lru.get_mut(key.borrow());

        if let Some(entry) = entry {
            entry.seconds_since_anchor = self.anchor_time.elapsed().as_secs() as i32;
            let data = entry.data.clone();
            let data_len = data.len();

            metrics
                .cache_operations
                .add(1, self.metric_attrs.read_hit());
            metrics.cache_io.add(data_len, self.metric_attrs.read_hit());
            metrics.cache_operation_duration.record(
                start_time.elapsed().as_secs_f64() * 1000.0,
                self.metric_attrs.read_hit(),
            );
            metrics
                .cache_entry_size
                .record(data_len, &self.base_metric_attrs);

            Some(data)
        } else {
            metrics
                .cache_operations
                .add(1, self.metric_attrs.read_miss());
            metrics.cache_operation_duration.record(
                start_time.elapsed().as_secs_f64() * 1000.0,
                self.metric_attrs.read_miss(),
            );
            None
        }
    }

    /// Returns the replaced item if any.
    pub async fn insert(&self, key: K, data: T) -> Option<T> {
        self.insert_with_time(key, data, self.anchor_time.elapsed().as_secs() as i32)
            .await
    }

    /// Returns the replaced item if any.
    pub async fn insert_with_time(&self, key: K, data: T, seconds_since_anchor: i32) -> Option<T> {
        let mut state = self.state.lock().await;
        let results = self
            .inner_insert_many(&mut state, [(key, data)], seconds_since_anchor)
            .await;
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
    {
        let mut inserts = inserts.into_iter().peekable();
        // Shortcut for cases where there are no inserts, so we don't need to lock.
        if inserts.peek().is_none() {
            return Vec::new();
        }
        let state = &mut self.state.lock().await;
        self.inner_insert_many(state, inserts, self.anchor_time.elapsed().as_secs() as i32)
            .await
    }

    async fn inner_insert_many<It>(
        &self,
        state: &mut State<K, T>,
        inserts: It,
        seconds_since_anchor: i32,
    ) -> Vec<T>
    where
        It: IntoIterator<Item = (K, T)> + Send,
        // Note: It's not enough to have the inserts themselves be Send. The
        // returned iterator should be Send as well.
        <It as IntoIterator>::IntoIter: Send,
    {
        let metrics = &*CACHE_METRICS;
        let mut replaced_items = Vec::new();

        for (key, data) in inserts {
            let start_time = Instant::now();
            let new_item_size = data.len();

            let eviction_item = EvictionItem {
                seconds_since_anchor,
                data,
            };

            let (old_item_data, old_item_size) = state.put(key, eviction_item).await;

            metrics
                .cache_operations
                .add(1, self.metric_attrs.write_success());
            metrics
                .cache_io
                .add(new_item_size, self.metric_attrs.write_success());
            metrics.cache_operation_duration.record(
                start_time.elapsed().as_secs_f64() * 1000.0,
                self.metric_attrs.write_success(),
            );

            if let Some(old_item) = old_item_data {
                metrics.cache_size.add(
                    i64::try_from(new_item_size).unwrap_or(METRIC_SIZE_OVERFLOW)
                        - i64::try_from(old_item_size).unwrap_or(METRIC_SIZE_OVERFLOW),
                    &self.base_metric_attrs,
                );
                metrics
                    .cache_entry_size
                    .record(new_item_size, &self.base_metric_attrs);

                replaced_items.push(old_item);
            } else {
                metrics.cache_size.add(
                    i64::try_from(new_item_size).unwrap_or(METRIC_SIZE_OVERFLOW),
                    &self.base_metric_attrs,
                );
                metrics.cache_entries.add(1, &self.base_metric_attrs);
                metrics
                    .cache_entry_size
                    .record(new_item_size, &self.base_metric_attrs);
            }

            self.evict_items(state).await;
        }

        replaced_items
    }

    pub async fn remove<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + Hash + Eq + Debug + Sync,
    {
        let mut state = self.state.lock().await;
        self.inner_remove(&mut state, key).await
    }

    async fn inner_remove<Q>(&self, state: &mut State<K, T>, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + Hash + Eq + Debug + Sync,
    {
        let start_time = Instant::now();
        let metrics = &*CACHE_METRICS;

        self.evict_items(state).await;

        if let Some(entry) = state.lru.pop(key.borrow()) {
            let item_size = entry.data.len();

            state.remove(key, &entry).await;

            metrics
                .cache_operations
                .add(1, self.metric_attrs.delete_success());
            metrics
                .cache_io
                .add(item_size, self.metric_attrs.delete_success());
            metrics.cache_operation_duration.record(
                start_time.elapsed().as_secs_f64() * 1000.0,
                self.metric_attrs.delete_success(),
            );
            metrics.cache_size.add(
                -(i64::try_from(item_size).unwrap_or(METRIC_SIZE_OVERFLOW)),
                &self.base_metric_attrs,
            );
            metrics.cache_entries.add(-1, &self.base_metric_attrs);

            return true;
        }

        metrics
            .cache_operations
            .add(1, self.metric_attrs.delete_miss());
        metrics.cache_operation_duration.record(
            start_time.elapsed().as_secs_f64() * 1000.0,
            self.metric_attrs.delete_miss(),
        );

        false
    }

    /// Same as `remove()`, but allows for a conditional to be applied to the
    /// entry before removal in an atomic fashion.
    pub async fn remove_if<Q, F>(&self, key: &Q, cond: F) -> bool
    where
        K: Borrow<Q>,
        Q: Ord + Hash + Eq + Debug + Sync,
        F: FnOnce(&T) -> bool + Send,
    {
        let mut state = self.state.lock().await;
        if let Some(entry) = state.lru.get(key.borrow()) {
            if !cond(&entry.data) {
                return false;
            }
            return self.inner_remove(&mut state, key).await;
        }
        false
    }
}
