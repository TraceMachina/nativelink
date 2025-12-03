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
use core::marker::PhantomData;
use core::ops::RangeBounds;
use core::pin::Pin;
use std::collections::BTreeSet;
use std::sync::Arc;

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use lru::LruCache;
use nativelink_config::stores::EvictionPolicy;
use nativelink_metric::MetricsComponent;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

use crate::instant_wrapper::InstantWrapper;
use crate::metrics_utils::{Counter, CounterWithTime};

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

// Callback to be called when the EvictingMap removes an item
// either via eviction or direct deletion. This will be called with
// whatever key type the EvictingMap uses.
pub trait RemoveItemCallback<Q>: Debug + Send + Sync {
    fn callback(&self, store_key: &Q) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}

#[derive(Debug, MetricsComponent)]
struct State<
    K: Ord + Hash + Eq + Clone + Debug + Send + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug,
    T: LenEntry + Debug + Send,
    C: RemoveItemCallback<Q>,
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

    _key_type: PhantomData<Q>,
    remove_callbacks: Vec<C>,
}

type RemoveFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

impl<
    K: Ord + Hash + Eq + Clone + Debug + Send + Sync + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug + Sync,
    T: LenEntry + Debug + Sync + Send,
    C: RemoveItemCallback<Q>,
> State<K, Q, T, C>
{
    /// Removes an item from the cache and returns the data for deferred cleanup.
    /// The caller is responsible for calling `unref()` on the returned data outside of the lock.
    #[must_use]
    fn remove(
        &mut self,
        key: &Q,
        eviction_item: &EvictionItem<T>,
        replaced: bool,
    ) -> (T, Vec<RemoveFuture>)
    where
        T: Clone,
    {
        if let Some(btree) = &mut self.btree {
            btree.remove(key);
        }
        self.sum_store_size -= eviction_item.data.len();
        if replaced {
            self.replaced_items.inc();
            self.replaced_bytes.add(eviction_item.data.len());
        } else {
            self.evicted_items.inc();
            self.evicted_bytes.add(eviction_item.data.len());
        }

        let callbacks = self
            .remove_callbacks
            .iter()
            .map(|callback| callback.callback(key))
            .collect();

        // Return the data for deferred unref outside of lock
        (eviction_item.data.clone(), callbacks)
    }

    /// Inserts a new item into the cache. If the key already exists, the old item is returned
    /// for deferred cleanup.
    #[must_use]
    fn put(&mut self, key: &K, eviction_item: EvictionItem<T>) -> Option<(T, Vec<RemoveFuture>)>
    where
        K: Clone,
        T: Clone,
    {
        // If we are maintaining a btree index, we need to update it.
        if let Some(btree) = &mut self.btree {
            btree.insert(key.clone());
        }
        self.lru
            .put(key.clone(), eviction_item)
            .map(|old_item| self.remove(key.borrow(), &old_item, true))
    }

    fn add_remove_callback(&mut self, callback: C) {
        self.remove_callbacks.push(callback);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NoopRemove;

impl<Q> RemoveItemCallback<Q> for NoopRemove {
    fn callback(&self, _store_key: &Q) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async {})
    }
}

#[derive(Debug, MetricsComponent)]
pub struct EvictingMap<
    K: Ord + Hash + Eq + Clone + Debug + Send + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug,
    T: LenEntry + Debug + Send,
    I: InstantWrapper,
    C: RemoveItemCallback<Q> = NoopRemove,
> {
    #[metric]
    state: Mutex<State<K, Q, T, C>>,
    anchor_time: I,
    #[metric(help = "Maximum size of the store in bytes")]
    max_bytes: u64,
    #[metric(help = "Number of bytes to evict when the store is full")]
    evict_bytes: u64,
    #[metric(help = "Maximum number of seconds to keep an item in the store")]
    max_seconds: i32,
    #[metric(help = "Maximum number of items to keep in the store")]
    max_count: u64,
}

impl<K, Q, T, I, C> EvictingMap<K, Q, T, I, C>
where
    K: Ord + Hash + Eq + Clone + Debug + Send + Sync + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug + Sync,
    T: LenEntry + Debug + Clone + Send + Sync,
    I: InstantWrapper,
    C: RemoveItemCallback<Q>,
{
    pub fn new(config: &EvictionPolicy, anchor_time: I) -> Self {
        Self {
            // We use unbounded because if we use the bounded version we can't call the delete
            // function on the LenEntry properly.
            state: Mutex::new(State {
                lru: LruCache::unbounded(),
                btree: None,
                sum_store_size: 0,
                evicted_bytes: Counter::default(),
                evicted_items: CounterWithTime::default(),
                replaced_bytes: Counter::default(),
                replaced_items: CounterWithTime::default(),
                lifetime_inserted_bytes: Counter::default(),
                _key_type: PhantomData,
                remove_callbacks: Vec::new(),
            }),
            anchor_time,
            max_bytes: config.max_bytes as u64,
            evict_bytes: config.evict_bytes as u64,
            max_seconds: config.max_seconds as i32,
            max_count: config.max_count,
        }
    }

    pub async fn enable_filtering(&self) {
        let mut state = self.state.lock();
        if state.btree.is_none() {
            Self::rebuild_btree_index(&mut state);
        }
    }

    fn rebuild_btree_index(state: &mut State<K, Q, T, C>) {
        state.btree = Some(state.lru.iter().map(|(k, _)| k).cloned().collect());
    }

    /// Run the `handler` function on each key-value pair that matches the `prefix_range`
    /// and return the number of items that were processed.
    /// The `handler` function should return `true` to continue processing the next item
    /// or `false` to stop processing.
    pub fn range<F>(&self, prefix_range: impl RangeBounds<Q> + Send, mut handler: F) -> u64
    where
        F: FnMut(&K, &T) -> bool + Send,
        K: Ord,
    {
        let mut state = self.state.lock();
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
    pub fn len_for_test(&self) -> usize {
        self.state.lock().lru.len()
    }

    fn should_evict(
        &self,
        lru_len: usize,
        peek_entry: &EvictionItem<T>,
        sum_store_size: u64,
        max_bytes: u64,
    ) -> bool {
        let is_over_size = max_bytes != 0 && sum_store_size >= max_bytes;

        let elapsed_seconds =
            i32::try_from(self.anchor_time.elapsed().as_secs()).unwrap_or(i32::MAX);
        let evict_older_than_seconds = elapsed_seconds.saturating_sub(self.max_seconds);
        let old_item_exists =
            self.max_seconds != 0 && peek_entry.seconds_since_anchor < evict_older_than_seconds;

        let is_over_count =
            self.max_count != 0 && u64::try_from(lru_len).unwrap_or(u64::MAX) > self.max_count;

        is_over_size || old_item_exists || is_over_count
    }

    #[must_use]
    fn evict_items(&self, state: &mut State<K, Q, T, C>) -> (Vec<T>, Vec<RemoveFuture>) {
        let Some((_, mut peek_entry)) = state.lru.peek_lru() else {
            return (Vec::new(), Vec::new());
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

        let mut items_to_unref = Vec::new();
        let mut removal_futures = Vec::new();

        while self.should_evict(state.lru.len(), peek_entry, state.sum_store_size, max_bytes) {
            let (key, eviction_item) = state
                .lru
                .pop_lru()
                .expect("Tried to peek() then pop() but failed");
            debug!(?key, "Evicting",);
            let (data, futures) = state.remove(key.borrow(), &eviction_item, false);
            items_to_unref.push(data);
            removal_futures.extend(futures.into_iter());

            peek_entry = if let Some((_, entry)) = state.lru.peek_lru() {
                entry
            } else {
                break;
            };
        }

        (items_to_unref, removal_futures)
    }

    /// Return the size of a `key`, if not found `None` is returned.
    pub async fn size_for_key(&self, key: &Q) -> Option<u64> {
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
        let (removal_futures, data_to_unref) = {
            let mut state = self.state.lock();

            let lru_len = state.lru.len();
            let mut data_to_unref = Vec::new();
            let mut removal_futures = Vec::new();
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
                        if self.should_evict(lru_len, entry, 0, u64::MAX) {
                            *result = None;
                            if let Some((key, eviction_item)) = state.lru.pop_entry(key.borrow()) {
                                info!(?key, "Item expired, evicting");
                                let (data, futures) =
                                    state.remove(key.borrow(), &eviction_item, false);
                                // Store data for later unref - we can't drop state here as we're still iterating
                                data_to_unref.push(data);
                                removal_futures.extend(futures.into_iter());
                            }
                        } else {
                            if !peek {
                                entry.seconds_since_anchor =
                                    i32::try_from(self.anchor_time.elapsed().as_secs())
                                        .unwrap_or(i32::MAX);
                            }
                            *result = Some(entry.data.len());
                        }
                    }
                    None => *result = None,
                }
            }
            (removal_futures, data_to_unref)
        };

        // Perform the async callbacks outside of the lock
        let mut callbacks: FuturesUnordered<_> = removal_futures.into_iter().collect();
        while callbacks.next().await.is_some() {}
        let mut callbacks: FuturesUnordered<_> =
            data_to_unref.iter().map(LenEntry::unref).collect();
        while callbacks.next().await.is_some() {}
    }

    pub async fn get(&self, key: &Q) -> Option<T> {
        // Fast path: Check if we need eviction before acquiring lock for eviction
        let needs_eviction = {
            let state = self.state.lock();
            if let Some((_, peek_entry)) = state.lru.peek_lru() {
                self.should_evict(
                    state.lru.len(),
                    peek_entry,
                    state.sum_store_size,
                    self.max_bytes,
                )
            } else {
                false
            }
        };

        // Perform eviction if needed
        if needs_eviction {
            let (items_to_unref, removal_futures) = {
                let mut state = self.state.lock();
                self.evict_items(&mut *state)
            };
            // Unref items outside of lock
            let mut callbacks: FuturesUnordered<_> = removal_futures.into_iter().collect();
            while callbacks.next().await.is_some() {}
            let mut callbacks: FuturesUnordered<_> =
                items_to_unref.iter().map(LenEntry::unref).collect();
            while callbacks.next().await.is_some() {}
        }

        // Now get the item
        let mut state = self.state.lock();
        let entry = state.lru.get_mut(key.borrow())?;
        entry.seconds_since_anchor =
            i32::try_from(self.anchor_time.elapsed().as_secs()).unwrap_or(i32::MAX);
        Some(entry.data.clone())
    }

    /// Returns the replaced item if any.
    pub async fn insert(&self, key: K, data: T) -> Option<T>
    where
        K: 'static,
    {
        self.insert_with_time(
            key,
            data,
            i32::try_from(self.anchor_time.elapsed().as_secs()).unwrap_or(i32::MAX),
        )
        .await
    }

    /// Returns the replaced item if any.
    pub async fn insert_with_time(&self, key: K, data: T, seconds_since_anchor: i32) -> Option<T> {
        let (items_to_unref, removal_futures) = {
            let mut state = self.state.lock();
            self.inner_insert_many(&mut state, [(key, data)], seconds_since_anchor)
        };

        let mut futures: FuturesUnordered<_> = removal_futures.into_iter().collect();
        while futures.next().await.is_some() {}

        // Unref items outside of lock
        let futures: FuturesUnordered<_> = items_to_unref
            .into_iter()
            .map(|item| async move {
                item.unref().await;
                item
            })
            .collect();
        futures.collect::<Vec<_>>().await.into_iter().next()
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

        let (items_to_unref, removal_futures) = {
            let mut state = self.state.lock();
            self.inner_insert_many(
                &mut state,
                inserts,
                i32::try_from(self.anchor_time.elapsed().as_secs()).unwrap_or(i32::MAX),
            )
        };

        let mut futures: FuturesUnordered<_> = removal_futures.into_iter().collect();
        while futures.next().await.is_some() {}

        // Unref items outside of lock
        items_to_unref
            .into_iter()
            .map(|item| async move {
                item.unref().await;
                item
            })
            .collect::<FuturesUnordered<_>>()
            .collect::<Vec<_>>()
            .await
    }

    fn inner_insert_many<It>(
        &self,
        state: &mut State<K, Q, T, C>,
        inserts: It,
        seconds_since_anchor: i32,
    ) -> (Vec<T>, Vec<RemoveFuture>)
    where
        It: IntoIterator<Item = (K, T)> + Send,
        // Note: It's not enough to have the inserts themselves be Send. The
        // returned iterator should be Send as well.
        <It as IntoIterator>::IntoIter: Send,
    {
        let mut replaced_items = Vec::new();
        let mut removal_futures = Vec::new();
        for (key, data) in inserts {
            let new_item_size = data.len();
            let eviction_item = EvictionItem {
                seconds_since_anchor,
                data,
            };

            if let Some((old_item, futures)) = state.put(&key, eviction_item) {
                removal_futures.extend(futures.into_iter());
                replaced_items.push(old_item);
            }
            state.sum_store_size += new_item_size;
            state.lifetime_inserted_bytes.add(new_item_size);
        }

        // Perform eviction after all insertions
        let (items_to_unref, futures) = self.evict_items(state);
        removal_futures.extend(futures);

        // Note: We cannot drop the state lock here since we're borrowing it,
        // but the caller will handle unreffing these items after releasing the lock
        replaced_items.extend(items_to_unref);

        (replaced_items, removal_futures)
    }

    pub async fn remove(&self, key: &Q) -> bool {
        let (items_to_unref, removed_item, removal_futures) = {
            let mut state = self.state.lock();

            // First perform eviction
            let (evicted_items, mut removal_futures) = self.evict_items(&mut *state);

            // Then try to remove the requested item
            let removed = if let Some(entry) = state.lru.pop(key.borrow()) {
                let (removed_item, more_removal_futures) = state.remove(key, &entry, false);
                removal_futures.extend(more_removal_futures.into_iter());
                Some(removed_item)
            } else {
                None
            };

            (evicted_items, removed, removal_futures)
        };

        let mut callbacks: FuturesUnordered<_> = removal_futures.into_iter().collect();
        while callbacks.next().await.is_some() {}

        // Unref evicted items outside of lock
        let mut callbacks: FuturesUnordered<_> =
            items_to_unref.iter().map(LenEntry::unref).collect();
        while callbacks.next().await.is_some() {}

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
        let (evicted_items, removal_futures, removed_item) = {
            let mut state = self.state.lock();
            if let Some(entry) = state.lru.get(key.borrow()) {
                if !cond(&entry.data) {
                    return false;
                }
                // First perform eviction
                let (evicted_items, mut removal_futures) = self.evict_items(&mut state);

                // Then try to remove the requested item
                let removed_item = if let Some(entry) = state.lru.pop(key.borrow()) {
                    let (item, more_removal_futures) = state.remove(key, &entry, false);
                    removal_futures.extend(more_removal_futures.into_iter());
                    Some(item)
                } else {
                    None
                };

                (evicted_items, removal_futures, removed_item)
            } else {
                (vec![], vec![].into_iter().collect(), None)
            }
        };

        // Perform the async callbacks outside of the lock
        let mut removal_futures: FuturesUnordered<_> = removal_futures.into_iter().collect();
        while removal_futures.next().await.is_some() {}

        // Unref evicted items
        let mut callbacks: FuturesUnordered<_> =
            evicted_items.iter().map(LenEntry::unref).collect();
        while callbacks.next().await.is_some() {}

        // Unref removed item if any
        if let Some(item) = removed_item {
            item.unref().await;
            true
        } else {
            false
        }
    }

    pub fn add_remove_callback(&self, callback: C) {
        self.state.lock().add_remove_callback(callback);
    }
}
