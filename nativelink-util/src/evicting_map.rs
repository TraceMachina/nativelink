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
use core::fmt::{Debug, Display};
use core::future::Future;
use core::hash::Hash;
use core::marker::PhantomData;
use core::ops::RangeBounds;
use core::pin::Pin;
use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, OnceLock};

use futures::StreamExt;
use futures::stream::FuturesUnordered;
use lru::LruCache;
use nativelink_config::stores::EvictionPolicy;
use nativelink_metric::MetricsComponent;
use opentelemetry::KeyValue;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{Level, debug, info};

use crate::instant_wrapper::InstantWrapper;
use crate::metrics::{record_cache_entries_delta, saturating_i64};
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
    /// Runs outside the map lock. Another task may insert a replacement for
    /// the same key before this call finishes; cleanup must only affect the
    /// removed entry's resources.
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
    /// A mirror of `lru` that contains only currently unleased keys.
    /// Keeping this index in LRU order lets eviction visit candidates without
    /// scanning entries that an active operation has pinned.
    evictable_lru: LruCache<K, ()>,
    /// Reference counts for keys that must not be evicted while an active
    /// operation is using them. A key can be leased before it is inserted,
    /// which closes the admission-to-insert race for cache populations.
    leases: HashMap<K, u32>,
    #[metric(help = "Total size of all items in the store")]
    sum_store_size: u64,
    #[metric(help = "Number of items currently leased and not evictable")]
    leased_items: u64,
    #[metric(help = "Number of bytes currently leased and not evictable")]
    leased_bytes: u64,

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
    /// Size and entry-count changes waiting to be reported as `cache.size` and
    /// `cache.entries`. Recording to `OpenTelemetry` costs far more than the
    /// plain atomic counters beside it, so the mutation paths only do integer
    /// arithmetic here. Each critical section takes what has accumulated just
    /// before it releases the lock and records it afterwards, so reporting
    /// never acquires the lock a second time. A section that does not take
    /// the changes only delays them to the next one, it never loses them.
    pending_size_delta: i64,
    pending_entries_delta: i64,
    /// Set under the lock when `cache.size` and `cache.entries` reporting is
    /// enabled, so every change is either part of the initial totals or taken
    /// by exactly one later critical section, never both.
    cache_size_metrics_enabled: bool,
}

type RemoveFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Size and entry-count changes taken from the state under its lock, to be
/// reported as `cache.size` and `cache.entries` once the lock is released.
#[derive(Debug, Clone, Copy)]
struct CacheSizeDelta {
    size: i64,
    entries: i64,
}

/// Most keys a batched call handles per acquisition of the state lock, so a
/// large batch still lets other callers in between chunks.
const MAX_KEYS_PER_LOCK: usize = 1024;

/// Entries a read found expired and removed under the state lock. Logging,
/// remove callbacks and `unref()` for them run once the lock is released.
struct Reaped<K, T> {
    entries: Vec<(K, T)>,
    removal_futures: Vec<RemoveFuture>,
}

impl<K, T> Reaped<K, T> {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            removal_futures: Vec::new(),
        }
    }
}

/// Counters captured when an eviction pass finds every resident entry leased.
#[derive(Debug, Clone, Copy)]
struct AllLeasedSnapshot {
    resident_items: usize,
    evictable_items: usize,
    lease_keys: usize,
    leased_items: u64,
    leased_bytes: u64,
    resident_bytes: u64,
}

/// Eviction and replacement log events gathered under the state lock and
/// written by `emit` once it is released. The stdout log layer writes
/// synchronously, so logging inside the critical section would hold the
/// store-wide lock across a `write(2)` per entry.
#[derive(Debug)]
struct EvictionLogs<K> {
    replaced: Vec<K>,
    evicted: Vec<K>,
    all_leased: Option<AllLeasedSnapshot>,
}

impl<K: Debug> EvictionLogs<K> {
    const fn new() -> Self {
        Self {
            replaced: Vec::new(),
            evicted: Vec::new(),
            all_leased: None,
        }
    }

    fn emit(self) {
        for key in &self.replaced {
            debug!(?key, "Evicting old item");
        }
        if let Some(snapshot) = self.all_leased {
            debug!(
                resident_items = snapshot.resident_items,
                evictable_items = snapshot.evictable_items,
                lease_keys = snapshot.lease_keys,
                leased_items = snapshot.leased_items,
                leased_bytes = snapshot.leased_bytes,
                resident_bytes = snapshot.resident_bytes,
                "Eviction requested, but every resident entry is leased",
            );
        }
        for key in &self.evicted {
            debug!(?key, "Evicting");
        }
    }
}

impl<
    K: Ord + Hash + Eq + Clone + Debug + Send + Sync + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug + Sync,
    T: LenEntry + Debug + Sync + Send,
    C: RemoveItemCallback<Q>,
> State<K, Q, T, C>
{
    fn is_leased(&self, key: &Q) -> bool {
        self.leases.contains_key(key)
    }

    /// Looks up a resident entry and reports whether it is leased. Unless
    /// `peek` is set, an unleased entry becomes the most recently used
    /// eviction candidate.
    ///
    /// Only `evictable_lru` is re-linked. Eviction walks that index alone,
    /// so the recency order of `lru` is never read and promoting there as
    /// well would be a second re-link for nothing. A resident key is in
    /// `evictable_lru` exactly when it is unleased, so the promoting lookup
    /// also answers whether the key is leased.
    fn get_resident(&mut self, key: &Q, peek: bool) -> Option<(&mut EvictionItem<T>, bool)> {
        let entry = self.lru.peek_mut(key)?;
        let is_leased = if peek {
            self.leases.contains_key(key)
        } else {
            self.evictable_lru.get(key).is_none()
        };
        Some((entry, is_leased))
    }

    /// Add a newly written key to the candidate index unless it is reserved
    /// by a lease that was acquired before insertion.
    fn insert_evictable(&mut self, key: K) {
        if !self.is_leased(key.borrow()) {
            self.evictable_lru.put(key, ());
        }
    }

    fn lease(&mut self, key: K) {
        if let Some(lease_count) = self.leases.get_mut(key.borrow()) {
            *lease_count = lease_count.checked_add(1).expect("lease count overflow");
            return;
        }

        self.evictable_lru.pop(key.borrow());
        if let Some(entry) = self.lru.peek(key.borrow()) {
            self.leased_bytes += entry.data.len();
        }
        self.leased_items += 1;
        self.leases.insert(key, 1);
    }

    /// Release one reference and return the owned key when the lease ends.
    fn release_lease(&mut self, key: &Q) -> Option<K> {
        let lease_count = self.leases.get_mut(key)?;
        if *lease_count > 1 {
            *lease_count -= 1;
            return None;
        }

        let leased_key = self
            .leases
            .remove_entry(key)
            .expect("Lease must exist when its final reference is released")
            .0;
        self.leased_items -= 1;
        if let Some(entry) = self.lru.peek(leased_key.borrow()) {
            self.leased_bytes -= entry.data.len();
        }
        Some(leased_key)
    }

    /// Re-enter a resident key as the newest eviction candidate. Its age is
    /// intentionally preserved, so a long lease does not extend the TTL.
    fn reinsert_evictable(&mut self, key: K) {
        if self.lru.peek(key.borrow()).is_some() {
            self.evictable_lru.put(key, ());
        }
    }

    fn peek_evictable(&self) -> Option<&EvictionItem<T>> {
        let (key, ()) = self.evictable_lru.peek_lru()?;
        Some(
            self.lru
                .peek(key.borrow())
                .expect("Evictable LRU key must be resident in the main LRU"),
        )
    }

    fn pop_evictable(&mut self) -> Option<(K, EvictionItem<T>)> {
        let (candidate_key, ()) = self.evictable_lru.pop_lru()?;
        let (key, entry) = self
            .lru
            .pop_entry(candidate_key.borrow())
            .expect("Evictable LRU key must be resident in the main LRU");
        Some((key, entry))
    }

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
        // Keep every auxiliary resident index in sync with the main LRU.
        self.evictable_lru.pop(key);
        if self.is_leased(key) {
            self.leased_bytes -= eviction_item.data.len();
        }
        self.sum_store_size -= eviction_item.data.len();
        self.pending_size_delta -= saturating_i64(eviction_item.data.len());
        self.pending_entries_delta -= 1;
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
        let is_leased = self.is_leased(key.borrow());
        let new_item_size = eviction_item.data.len();
        let replaced_item = self
            .lru
            .put(key.clone(), eviction_item)
            .map(|old_item| self.remove(key.borrow(), &old_item, true));

        if is_leased {
            self.leased_bytes += new_item_size;
        }
        // `remove()` drops the old key from both indexes. Reinsert it after
        // replacement so an unleased write is MRU in both LRU indexes.
        self.insert_evictable(key.clone());
        if let Some(btree) = &mut self.btree {
            btree.insert(key.clone());
        }

        replaced_item
    }

    fn add_remove_callback(&mut self, callback: C) {
        self.remove_callbacks.push(callback);
    }

    /// Takes the size and entry-count changes accumulated since the last
    /// call, or `None` while reporting is off or nothing changed.
    fn take_cache_size_delta(&mut self) -> Option<CacheSizeDelta> {
        if !self.cache_size_metrics_enabled {
            return None;
        }
        let delta = CacheSizeDelta {
            size: core::mem::take(&mut self.pending_size_delta),
            entries: core::mem::take(&mut self.pending_entries_delta),
        };
        (delta.size != 0 || delta.entries != 0).then_some(delta)
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
    /// Attributes to report `cache.size` and `cache.entries` under. Unset until
    /// a `cache_metrics` wrapper enables it, which is what keeps those two
    /// instruments off by default.
    cache_size_attrs: OnceLock<Vec<KeyValue>>,
}

// debugging helper used mostly to get a snapshot of what eviction threshold might be causing issues
#[derive(Debug, Copy, Clone)]
pub struct EvictionSnapshot {
    max_bytes: u64,
    current_bytes: u64,
    max_items: u64,
    current_items: usize,
    max_seconds: i32,
}

impl Display for EvictionSnapshot {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.max_bytes != 0 {
            write!(
                f,
                "Bytes: {} of {} ({:.3}%); ",
                self.current_bytes,
                self.max_bytes,
                (self.current_bytes as f64 * 100.0) / (self.max_bytes as f64)
            )?;
        } else {
            write!(f, "Bytes: {} of unlimited; ", self.current_bytes)?;
        }
        if self.max_items != 0 {
            write!(
                f,
                "Items: {} of {} ({:.3}%); ",
                self.current_items,
                self.max_items,
                (self.current_items as f64 * 100.0) / (self.max_items as f64)
            )?;
        } else {
            write!(f, "Items: {} of unlimited; ", self.current_items)?;
        }
        if self.max_seconds > 0 {
            write!(f, "Timeout: {}s", self.max_seconds)?;
        } else {
            write!(f, "Timeout: unlimited")?;
        }
        Ok(())
    }
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
                evictable_lru: LruCache::unbounded(),
                leases: HashMap::new(),
                sum_store_size: 0,
                leased_items: 0,
                leased_bytes: 0,
                evicted_bytes: Counter::default(),
                evicted_items: CounterWithTime::default(),
                replaced_bytes: Counter::default(),
                replaced_items: CounterWithTime::default(),
                lifetime_inserted_bytes: Counter::default(),
                _key_type: PhantomData,
                remove_callbacks: Vec::new(),
                pending_size_delta: 0,
                pending_entries_delta: 0,
                cache_size_metrics_enabled: false,
            }),
            anchor_time,
            max_bytes: config.max_bytes as u64,
            evict_bytes: config.evict_bytes as u64,
            max_seconds: config.max_seconds.try_into().unwrap_or(i32::MAX),
            max_count: config.max_count,
            cache_size_attrs: OnceLock::new(),
        }
    }

    /// Reports this map's size and entry count as `cache.size` and
    /// `cache.entries` under `attrs`, starting from what it already holds.
    /// Only the first call takes effect, so a map is never counted twice.
    pub fn enable_cache_size_metrics(&self, attrs: Vec<KeyValue>) {
        if self.cache_size_attrs.set(attrs).is_err() {
            return;
        }
        let cache_size_delta = {
            let mut state = self.state.lock();
            // Nothing has been reported yet, so the first report is the map's
            // current totals. Overwrite rather than add: the mutations that
            // filled the map accumulated deltas too, and counting those as well
            // would report everything already resident twice.
            state.pending_size_delta = saturating_i64(state.sum_store_size);
            state.pending_entries_delta = i64::try_from(state.lru.len()).unwrap_or(i64::MAX);
            state.cache_size_metrics_enabled = true;
            state.take_cache_size_delta()
        };
        self.record_cache_size_delta(cache_size_delta);
    }

    /// Records changes taken with `State::take_cache_size_delta`.
    ///
    /// Call after releasing the state lock: reporting to `OpenTelemetry` is
    /// much costlier than the counters updated inside it, and the state lock is
    /// contended across the whole store.
    fn record_cache_size_delta(&self, delta: Option<CacheSizeDelta>) {
        let (Some(delta), Some(attrs)) = (delta, self.cache_size_attrs.get()) else {
            return;
        };
        record_cache_entries_delta(delta.size, delta.entries, attrs);
    }

    // Only used for tests
    pub fn enable_filtering(&self) {
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

    /// `now` is `elapsed_seconds()` read once per operation, before taking the
    /// state lock, so the clock is never read inside the critical section.
    fn should_evict(
        &self,
        lru_len: usize,
        peek_entry: &EvictionItem<T>,
        sum_store_size: u64,
        max_bytes: u64,
        now: i32,
    ) -> bool {
        let is_over_size = max_bytes != 0 && sum_store_size >= max_bytes;

        let evict_older_than_seconds = now.saturating_sub(self.max_seconds);
        let old_item_exists =
            self.max_seconds != 0 && peek_entry.seconds_since_anchor < evict_older_than_seconds;

        let is_over_count =
            self.max_count != 0 && u64::try_from(lru_len).unwrap_or(u64::MAX) > self.max_count;

        is_over_size || old_item_exists || is_over_count
    }

    fn elapsed_seconds(&self) -> i32 {
        i32::try_from(self.anchor_time.elapsed().as_secs()).unwrap_or(i32::MAX)
    }

    // Gets a debugging snapshot of the state of the map. It's inevitably out of date by the time it gets sent to the user
    // but does provide a momentary glimpse into possible issues e.g. if current_bytes is close to max_bytes
    pub fn get_snapshot(&self) -> EvictionSnapshot {
        let state = self.state.lock();
        EvictionSnapshot {
            max_bytes: self.max_bytes,
            current_bytes: state.sum_store_size,
            max_items: self.max_count,
            current_items: state.lru.len(),
            max_seconds: self.max_seconds,
        }
    }

    #[must_use]
    fn evict_items(
        &self,
        state: &mut State<K, Q, T, C>,
        now: i32,
        logs: &mut EvictionLogs<K>,
    ) -> (Vec<T>, Vec<RemoveFuture>) {
        let Some(first_entry) = state.peek_evictable() else {
            if !state.lru.is_empty() {
                logs.all_leased = Some(AllLeasedSnapshot {
                    resident_items: state.lru.len(),
                    evictable_items: state.evictable_lru.len(),
                    lease_keys: state.leases.len(),
                    leased_items: state.leased_items,
                    leased_bytes: state.leased_bytes,
                    resident_bytes: state.sum_store_size,
                });
            }
            return (Vec::new(), Vec::new());
        };

        // Preserve the configured low-watermark behavior: once pressure is
        // detected, keep evicting until `evict_bytes` is reclaimed.
        let max_bytes = if self.max_bytes != 0
            && self.evict_bytes != 0
            && self.should_evict(
                state.lru.len(),
                first_entry,
                state.sum_store_size,
                self.max_bytes,
                now,
            ) {
            self.max_bytes.saturating_sub(self.evict_bytes)
        } else {
            self.max_bytes
        };

        let mut items_to_unref = Vec::new();
        let mut removal_futures = Vec::new();
        // Evicted keys are only kept for the log when debug logging is on;
        // otherwise they are dropped here, as before.
        let log_evictions = tracing::enabled!(Level::DEBUG);

        // Pop the candidate index directly instead of collecting and cloning
        // all victim keys. Both indexes are protected by the same lock.
        while let Some(entry) = state.peek_evictable() {
            if !self.should_evict(state.lru.len(), entry, state.sum_store_size, max_bytes, now) {
                break;
            }

            let (key, eviction_item) = state
                .pop_evictable()
                .expect("Evictable LRU key disappeared while state was locked");
            let (data, futures) = state.remove(key.borrow(), &eviction_item, false);
            items_to_unref.push(data);
            removal_futures.extend(futures);
            if log_evictions {
                logs.evicted.push(key);
            }
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
        let now = self.elapsed_seconds();
        let (removal_futures, data_to_unref, expired_keys, cache_size_delta) = {
            let mut state = self.state.lock();

            let lru_len = state.lru.len();
            let mut data_to_unref = Vec::new();
            let mut removal_futures = Vec::new();
            let mut expired_keys = Vec::new();
            for (key, result) in keys.into_iter().zip(results.iter_mut()) {
                match state.get_resident(key.borrow(), peek) {
                    Some((entry, is_leased)) => {
                        // Note: We need to check eviction because the item might be expired
                        // based on the current time. In such case, we remove the item while
                        // we are here.
                        if !is_leased && self.should_evict(lru_len, entry, 0, u64::MAX, now) {
                            *result = None;
                            if let Some((key, eviction_item)) = state.lru.pop_entry(key.borrow()) {
                                let (data, futures) =
                                    state.remove(key.borrow(), &eviction_item, false);
                                // Store data for later unref - we can't drop state here as we're still iterating
                                data_to_unref.push(data);
                                removal_futures.extend(futures);
                                expired_keys.push(key);
                            }
                        } else {
                            if !peek {
                                entry.seconds_since_anchor = now;
                            }
                            *result = Some(entry.data.len());
                        }
                    }
                    None => *result = None,
                }
            }
            (
                removal_futures,
                data_to_unref,
                expired_keys,
                state.take_cache_size_delta(),
            )
        };

        // Log and perform the async callbacks outside of the lock
        for key in &expired_keys {
            info!(?key, "Item expired, evicting");
        }
        self.record_cache_size_delta(cache_size_delta);
        let mut callbacks: FuturesUnordered<_> = removal_futures.into_iter().collect();
        while callbacks.next().await.is_some() {}
        let mut callbacks: FuturesUnordered<_> =
            data_to_unref.iter().map(LenEntry::unref).collect();
        while callbacks.next().await.is_some() {}
    }

    /// Fires the registered remove callbacks for `key` without touching the map.
    /// A store uses this to invalidate downstream listeners (e.g. an
    /// `ExistenceCacheStore`) when a write is rejected outright and never
    /// inserted — mirroring the callbacks an insert-then-immediate-evict would
    /// otherwise have fired. It only *notifies*; it does not remove anything from
    /// the map (there is nothing to remove). A no-op when no callbacks are
    /// registered.
    pub async fn fire_remove_callbacks(&self, key: &Q) {
        let mut callbacks: FuturesUnordered<_> = {
            let state = self.state.lock();
            state
                .remove_callbacks
                .iter()
                .map(|callback| callback.callback(key))
                .collect()
        };
        while callbacks.next().await.is_some() {}
    }

    /// Returns the value for `key` if present and not expired, refreshing
    /// its LRU/atime position. If the entry is present but TTL- or
    /// count-expired, it is reaped and `None` is returned.
    ///
    /// A read never cascades into other entries — only the queried key is
    /// ever touched. Global eviction (size/count overflow trim) runs on
    /// inserts; it is not driven by reads, since `sum_store_size` cannot
    /// grow without an insert.
    pub async fn get(&self, key: &Q) -> Option<T> {
        let now = self.elapsed_seconds();
        let mut reaped = Reaped::new();
        let (data, cache_size_delta) = {
            let mut state = self.state.lock();
            let data = self.get_locked(&mut state, key, now, &mut reaped);
            (data, state.take_cache_size_delta())
        };
        self.finish_reaped(reaped, cache_size_delta).await;
        data
    }

    /// Same as `get()` for each of `keys`, returning the results in input
    /// order.
    ///
    /// Takes the state lock once per `MAX_KEYS_PER_LOCK` keys instead of once
    /// per key. Every key gets exactly what a `get()` of it would at that
    /// point: a live entry is promoted and its age refreshed, and an expired,
    /// unleased entry is reaped and reported as `None`.
    pub async fn get_many<It, R>(&self, keys: It) -> Vec<Option<T>>
    where
        It: IntoIterator<Item = R> + Send,
        // Note: It's not enough to have the keys themselves be Send. The
        // returned iterator should be Send as well.
        <It as IntoIterator>::IntoIter: Send,
        R: Borrow<Q> + Send,
    {
        let mut keys = keys.into_iter().peekable();
        let mut results = Vec::with_capacity(keys.size_hint().0);
        while keys.peek().is_some() {
            let now = self.elapsed_seconds();
            let mut reaped = Reaped::new();
            let cache_size_delta = {
                let mut state = self.state.lock();
                for key in keys.by_ref().take(MAX_KEYS_PER_LOCK) {
                    results.push(self.get_locked(&mut state, key.borrow(), now, &mut reaped));
                }
                state.take_cache_size_delta()
            };
            self.finish_reaped(reaped, cache_size_delta).await;
        }
        results
    }

    /// The part of `get()` that runs under the state lock. Returns a live
    /// entry's value after refreshing its age, or moves an expired entry into
    /// `reaped` and returns `None`.
    ///
    /// Lazily reaps *only* the requested entry if it is itself expired; the
    /// rest are left for inserts, which already run the global eviction loop.
    fn get_locked(
        &self,
        state: &mut State<K, Q, T, C>,
        key: &Q,
        now: i32,
        reaped: &mut Reaped<K, T>,
    ) -> Option<T> {
        let lru_len = state.lru.len();
        let (entry, is_leased) = state.get_resident(key, false)?;
        // Pass `sum_store_size=0` and `max_bytes=u64::MAX` so we only
        // consult TTL / count predicates — never the global byte budget.
        // Mirrors the per-key reap path in `sizes_for_keys`.
        if !is_leased && self.should_evict(lru_len, entry, 0, u64::MAX, now) {
            let (popped_key, eviction_item) = state
                .lru
                .pop_entry(key)
                .expect("entry was just observed via get_resident");
            let (data, futures) = state.remove(popped_key.borrow(), &eviction_item, false);
            reaped.removal_futures.extend(futures);
            reaped.entries.push((popped_key, data));
            return None;
        }
        entry.seconds_since_anchor = now;
        Some(entry.data.clone())
    }

    /// Logs, drains remove callbacks and unrefs what reads reaped. Call after
    /// releasing the state lock.
    async fn finish_reaped(&self, reaped: Reaped<K, T>, cache_size_delta: Option<CacheSizeDelta>) {
        self.record_cache_size_delta(cache_size_delta);
        if reaped.entries.is_empty() {
            return;
        }
        for (popped_key, _) in &reaped.entries {
            info!(?popped_key, "Item expired, evicting");
        }
        let mut callbacks: FuturesUnordered<_> = reaped.removal_futures.into_iter().collect();
        while callbacks.next().await.is_some() {}
        let mut callbacks: FuturesUnordered<_> = reaped
            .entries
            .iter()
            .map(|(_, data)| data.unref())
            .collect();
        while callbacks.next().await.is_some() {}
    }

    /// Returns the replaced item if any.
    pub async fn insert(&self, key: K, data: T) -> Option<T>
    where
        K: 'static,
    {
        self.insert_with_time(key, data, self.elapsed_seconds())
            .await
    }

    /// Same as `insert()`, but allows for a conditional to be applied to the
    /// entry before insertion in an atomic fashion.
    pub async fn insert_if<F>(&self, key: K, data: T, cond: F) -> (bool, Option<T>)
    where
        F: FnOnce(&T, &T) -> bool + Send,
    {
        self.insert_with_time_if(key, data, cond, self.elapsed_seconds())
            .await
    }

    /// Returns the replaced item if any.
    pub async fn insert_with_time(&self, key: K, data: T, seconds_since_anchor: i32) -> Option<T> {
        self.insert_with_time_if(key, data, |_, _| true, seconds_since_anchor)
            .await
            .1
    }

    /// Conditional insertion with an explicit timestamp. The predicate runs
    /// under the map lock; a rejected value is left for the caller to clean up.
    pub async fn insert_with_time_if<F>(
        &self,
        key: K,
        data: T,
        cond: F,
        seconds_since_anchor: i32,
    ) -> (bool, Option<T>)
    where
        F: FnOnce(&T, &T) -> bool + Send,
    {
        let now = self.elapsed_seconds();
        let mut logs = EvictionLogs::new();
        let (items_to_unref, removal_futures, cache_size_delta) = {
            let mut state = self.state.lock();

            if let Some((old_entry, _)) = state.get_resident(key.borrow(), false)
                && !cond(&old_entry.data, &data)
            {
                return (false, None);
            }

            let (items_to_unref, removal_futures) = self.inner_insert_many(
                &mut state,
                [(key, data)],
                seconds_since_anchor,
                now,
                &mut logs,
            );
            (
                items_to_unref,
                removal_futures,
                state.take_cache_size_delta(),
            )
        };

        logs.emit();
        self.record_cache_size_delta(cache_size_delta);
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
        (true, futures.collect::<Vec<_>>().await.into_iter().next())
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

        let now = self.elapsed_seconds();
        let mut logs = EvictionLogs::new();
        let (items_to_unref, removal_futures, cache_size_delta) = {
            let mut state = self.state.lock();
            let (items_to_unref, removal_futures) =
                self.inner_insert_many(&mut state, inserts, now, now, &mut logs);
            (
                items_to_unref,
                removal_futures,
                state.take_cache_size_delta(),
            )
        };

        logs.emit();
        self.record_cache_size_delta(cache_size_delta);
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

    /// Lease a key so automatic size, count, and expiry eviction cannot remove it.
    ///
    /// Leasing before the value is inserted is intentional: an action can
    /// reserve a digest before populating the fast tier, so insertion and
    /// eviction cannot race with the start of input materialization. Explicit
    /// `remove` and `remove_if` calls still remove leased entries; those paths
    /// are used for deliberate deletion and filesystem self-healing.
    pub fn lease_key(&self, key: K) {
        self.state.lock().lease(key);
    }

    /// Same as `lease_key()` for each of `keys`, taking the state lock once
    /// per `MAX_KEYS_PER_LOCK` keys instead of once per key.
    pub fn lease_keys<It>(&self, keys: It)
    where
        It: IntoIterator<Item = K>,
    {
        let mut keys = keys.into_iter().peekable();
        while keys.peek().is_some() {
            let mut state = self.state.lock();
            for key in keys.by_ref().take(MAX_KEYS_PER_LOCK) {
                state.lease(key);
            }
        }
    }

    /// Release one lease and trim any entries that were retained while the
    /// lease was active.
    pub async fn release_key(&self, key: &Q) {
        self.release_keys([key]).await;
    }

    /// Release a batch of leases and trim retained entries once.
    ///
    /// Action input leases commonly contain thousands of digests. Batching
    /// avoids rescanning the LRU and running deferred cleanup once per digest
    /// during action teardown.
    pub async fn release_keys<It, R>(&self, keys: It)
    where
        It: IntoIterator<Item = R> + Send,
        <It as IntoIterator>::IntoIter: Send,
        R: Borrow<Q> + Send,
    {
        let now = self.elapsed_seconds();
        let mut logs = EvictionLogs::new();
        let (items_to_unref, removal_futures, cache_size_delta) = {
            let mut state = self.state.lock();
            let mut released_any = false;
            for key in keys {
                if let Some(leased_key) = state.release_lease(key.borrow()) {
                    // A released key re-enters as MRU, giving an actively used
                    // input a fair chance to remain cached after its action.
                    state.reinsert_evictable(leased_key);
                    released_any = true;
                }
            }
            if released_any {
                let (items_to_unref, removal_futures) =
                    self.evict_items(&mut state, now, &mut logs);
                (
                    items_to_unref,
                    removal_futures,
                    state.take_cache_size_delta(),
                )
            } else {
                (Vec::new(), Vec::new(), None)
            }
        };

        logs.emit();
        self.record_cache_size_delta(cache_size_delta);
        let mut futures: FuturesUnordered<_> = removal_futures.into_iter().collect();
        while futures.next().await.is_some() {}

        let mut futures: FuturesUnordered<_> = items_to_unref.iter().map(LenEntry::unref).collect();
        while futures.next().await.is_some() {}
    }

    /// `seconds_since_anchor` stamps the inserted entries; `now` is the
    /// current time the eviction pass that follows compares ages against.
    fn inner_insert_many<It>(
        &self,
        state: &mut State<K, Q, T, C>,
        inserts: It,
        seconds_since_anchor: i32,
        now: i32,
        logs: &mut EvictionLogs<K>,
    ) -> (Vec<T>, Vec<RemoveFuture>)
    where
        It: IntoIterator<Item = (K, T)> + Send,
        // Note: It's not enough to have the inserts themselves be Send. The
        // returned iterator should be Send as well.
        <It as IntoIterator>::IntoIter: Send,
    {
        let mut replaced_items = Vec::new();
        let mut removal_futures = Vec::new();
        let log_replacements = tracing::enabled!(Level::DEBUG);
        for (key, data) in inserts {
            let new_item_size = data.len();
            let eviction_item = EvictionItem {
                seconds_since_anchor,
                data,
            };

            if let Some((old_item, futures)) = state.put(&key, eviction_item) {
                removal_futures.extend(futures);
                replaced_items.push(old_item);
                if log_replacements {
                    logs.replaced.push(key);
                }
            }
            state.sum_store_size += new_item_size;
            state.lifetime_inserted_bytes.add(new_item_size);
            state.pending_size_delta += saturating_i64(new_item_size);
            state.pending_entries_delta += 1;
        }

        // Perform eviction after all insertions
        let (items_to_unref, futures) = self.evict_items(state, now, logs);
        removal_futures.extend(futures);

        // Note: We cannot drop the state lock here since we're borrowing it,
        // but the caller will handle unreffing these items after releasing the lock
        replaced_items.extend(items_to_unref);

        (replaced_items, removal_futures)
    }

    pub async fn remove(&self, key: &Q) -> bool {
        let now = self.elapsed_seconds();
        let mut logs = EvictionLogs::new();
        let (items_to_unref, removed_item, removal_futures, cache_size_delta) = {
            let mut state = self.state.lock();

            // First perform eviction
            let (evicted_items, mut removal_futures) = self.evict_items(&mut state, now, &mut logs);

            // Then try to remove the requested item
            let removed = if let Some(entry) = state.lru.pop(key.borrow()) {
                let (removed_item, more_removal_futures) = state.remove(key, &entry, false);
                removal_futures.extend(more_removal_futures);
                Some(removed_item)
            } else {
                None
            };

            (
                evicted_items,
                removed,
                removal_futures,
                state.take_cache_size_delta(),
            )
        };

        logs.emit();
        self.record_cache_size_delta(cache_size_delta);
        let mut callbacks: FuturesUnordered<_> = removal_futures.into_iter().collect();
        while callbacks.next().await.is_some() {}

        // Unref evicted items outside of lock
        let mut callbacks: FuturesUnordered<_> =
            items_to_unref.iter().map(LenEntry::unref).collect();
        while callbacks.next().await.is_some() {}

        // Unref removed item if any
        if let Some(item) = removed_item {
            debug!(?key, "Evicting (direct remove)");
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
        let now = self.elapsed_seconds();
        let mut logs = EvictionLogs::new();
        let (evicted_items, removal_futures, removed_item, cache_size_delta) = {
            let mut state = self.state.lock();
            let Some((entry, _)) = state.get_resident(key, false) else {
                return false;
            };
            if !cond(&entry.data) {
                return false;
            }

            // First perform eviction
            let (evicted_items, mut removal_futures) = self.evict_items(&mut state, now, &mut logs);

            // Then try to remove the requested item
            let removed_item = if let Some(entry) = state.lru.pop(key.borrow()) {
                let (item, more_removal_futures) = state.remove(key, &entry, false);
                removal_futures.extend(more_removal_futures);
                Some(item)
            } else {
                None
            };

            (
                evicted_items,
                removal_futures,
                removed_item,
                state.take_cache_size_delta(),
            )
        };

        // Log and perform the async callbacks outside of the lock
        logs.emit();
        self.record_cache_size_delta(cache_size_delta);
        let mut removal_futures: FuturesUnordered<_> = removal_futures.into_iter().collect();
        while removal_futures.next().await.is_some() {}

        // Unref evicted items
        let mut callbacks: FuturesUnordered<_> =
            evicted_items.iter().map(LenEntry::unref).collect();
        while callbacks.next().await.is_some() {}

        // Unref removed item if any
        if let Some(item) = removed_item {
            debug!(?key, "Evicting (conditional remove)");
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
