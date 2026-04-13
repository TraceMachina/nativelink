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
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::ops::RangeBounds;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::collections::hash_map::DefaultHasher;
use std::time::Instant;
use std::sync::Arc;

use tokio::sync::Notify;

use parking_lot::Mutex;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use lru::LruCache;
use nativelink_config::stores::EvictionPolicy;
use nativelink_metric::MetricsComponent;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::background_spawn;
use crate::instant_wrapper::InstantWrapper;
use crate::metrics_utils::{Counter, CounterWithTime};

/// Maximum fraction of max_bytes that can be pinned (25%).
const PIN_CAP_FRACTION: f64 = 0.25;
/// Seconds before a pin automatically expires.
const PIN_TIMEOUT_SECS: u64 = 120;

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

// Callback invoked when the EvictingMap inserts or removes an item.
pub trait ItemCallback<Q>: Debug + Send + Sync {
    fn callback(&self, store_key: &Q) -> Pin<Box<dyn Future<Output = ()> + Send>>;

    /// Called synchronously when a new item is inserted.
    /// Default is a no-op.
    fn on_insert(&self, _store_key: &Q, _size: u64) {}
}

#[derive(Debug, MetricsComponent)]
struct State<
    K: Ord + Hash + Eq + Clone + Debug + Send + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug,
    T: LenEntry + Debug + Send,
    C: ItemCallback<Q>,
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
    item_callbacks: Vec<C>,
    /// Keys that are pinned and should not be evicted.
    pinned_keys: HashSet<K>,
    /// Tracks when each key was pinned, for timeout enforcement.
    pin_times: HashMap<K, Instant>,
    /// Total size of pinned entries in bytes.
    pinned_bytes: u64,
}

type RemoveFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

impl<
    K: Ord + Hash + Eq + Clone + Debug + Send + Sync + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug + Sync,
    T: LenEntry + Debug + Sync + Send,
    C: ItemCallback<Q>,
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
        // Remove any stale pin for this key.
        if self.pinned_keys.remove(key) {
            self.pin_times.remove(key);
            self.pinned_bytes = self.pinned_bytes.saturating_sub(eviction_item.data.len());
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
            .item_callbacks
            .iter()
            .map(|callback| callback.callback(key))
            .collect();

        // Return the data for deferred unref outside of lock
        (eviction_item.data.clone(), callbacks)
    }

    /// Inserts a new item into the cache. If the key already exists, the old item is returned
    /// for deferred cleanup.
    ///
    /// Note: This method does NOT fire `on_insert` callbacks. The caller is
    /// responsible for collecting the key+size pairs and firing callbacks
    /// after releasing the State mutex to avoid nested locking.
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

    fn add_item_callback(&mut self, callback: C) {
        self.item_callbacks.push(callback);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct NoopCallback;

impl<Q> ItemCallback<Q> for NoopCallback {
    fn callback(&self, _store_key: &Q) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        Box::pin(async {})
    }

    fn on_insert(&self, _store_key: &Q, _size: u64) {}
}

/// Tracks lock contention metrics for EvictingMap.
#[derive(Debug, Default)]
pub struct LockMetrics {
    /// Maximum lock wait time observed, in milliseconds.
    pub max_lock_wait_ms: AtomicU64,
    /// Total number of lock contention events (wait > 0ms).
    pub lock_contention_count: AtomicU64,
}

/// Acquires `$self.state.lock()` with timing instrumentation.
/// Records contention metrics on `$self.lock_metrics` and logs a warning
/// when the wait exceeds 10ms.
///
/// Usage: `let mut state = lock_with_metrics!($self, "op_name");`
macro_rules! lock_with_metrics {
    ($self:expr, $op:expr) => {{
        let lock_start = std::time::Instant::now();
        let guard = $self.state.lock();
        let lock_wait = lock_start.elapsed();
        let wait_ms = lock_wait.as_millis() as u64;
        if wait_ms > 0 {
            $self
                .lock_metrics
                .max_lock_wait_ms
                .fetch_max(wait_ms, Ordering::Relaxed);
            $self
                .lock_metrics
                .lock_contention_count
                .fetch_add(1, Ordering::Relaxed);
            if wait_ms >= 10 {
                warn!(
                    lock_wait_ms = wait_ms,
                    max_lock_wait_ms =
                        $self.lock_metrics.max_lock_wait_ms.load(Ordering::Relaxed),
                    total_contentions =
                        $self.lock_metrics.lock_contention_count.load(Ordering::Relaxed),
                    op = $op,
                    "EvictingMap: lock contention",
                );
            }
        }
        guard
    }};
}

#[derive(Debug, MetricsComponent)]
pub struct EvictingMap<
    K: Ord + Hash + Eq + Clone + Debug + Send + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug,
    T: LenEntry + Debug + Send,
    I: InstantWrapper,
    C: ItemCallback<Q> = NoopCallback,
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
    /// Lock contention metrics (max wait, total contentions).
    pub lock_metrics: LockMetrics,
    /// Notify signal for the background eviction loop.
    eviction_notify: Arc<Notify>,
    /// Whether the background eviction loop has been started.
    background_eviction_running: AtomicBool,
}

impl<K, Q, T, I, C> EvictingMap<K, Q, T, I, C>
where
    K: Ord + Hash + Eq + Clone + Debug + Send + Sync + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug + Sync,
    T: LenEntry + Debug + Clone + Send + Sync,
    I: InstantWrapper,
    C: ItemCallback<Q> + Clone,
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
                item_callbacks: Vec::new(),
                pinned_keys: HashSet::new(),
                pin_times: HashMap::new(),
                pinned_bytes: 0,
            }),
            anchor_time,
            max_bytes: config.max_bytes as u64,
            evict_bytes: config.evict_bytes as u64,
            max_seconds: config.max_seconds as i32,
            max_count: config.max_count,
            lock_metrics: LockMetrics::default(),
            eviction_notify: Arc::new(Notify::new()),
            background_eviction_running: AtomicBool::new(false),
        }
    }

    /// Pin a key to prevent eviction. Returns `true` if the key was
    /// successfully pinned, `false` if pinning would exceed the pin cap
    /// or the key is not present in the map. Idempotent for already-pinned keys.
    pub fn pin_key(&self, key: K) -> bool {
        let mut state = lock_with_metrics!(self, "pin_key");

        // Already pinned — refresh the pin time.
        if state.pinned_keys.contains(key.borrow()) {
            state.pin_times.insert(key, Instant::now());
            return true;
        }

        // Look up the entry size; refuse to pin a key that isn't in the map.
        let entry_size = match state.lru.peek(key.borrow()) {
            Some(item) => item.data.len(),
            None => return false,
        };

        // Enforce pin cap.
        let pin_cap = (self.max_bytes as f64 * PIN_CAP_FRACTION) as u64;
        if self.max_bytes != 0 && state.pinned_bytes.saturating_add(entry_size) > pin_cap {
            warn!(
                pinned_bytes = state.pinned_bytes,
                entry_size,
                pin_cap,
                ?key,
                "pin cap exceeded, refusing to pin"
            );
            return false;
        }

        state.pinned_keys.insert(key.clone());
        state.pin_times.insert(key, Instant::now());
        state.pinned_bytes += entry_size;
        true
    }

    /// Pin multiple keys in a single critical section, reducing lock contention.
    /// Returns the number of keys successfully pinned (including already-pinned
    /// keys whose pin time was refreshed).
    pub fn pin_keys(&self, keys: &[K]) -> usize {
        let mut state = lock_with_metrics!(self, "pin_keys");
        let pin_cap = (self.max_bytes as f64 * PIN_CAP_FRACTION) as u64;
        let mut pinned = 0;
        for key in keys {
            // Already pinned — refresh the pin time.
            if state.pinned_keys.contains(key.borrow()) {
                state.pin_times.insert(key.clone(), Instant::now());
                pinned += 1;
                continue;
            }

            // Look up the entry size; skip keys that aren't in the map.
            let entry_size = match state.lru.peek(key.borrow()) {
                Some(item) => item.data.len(),
                None => continue,
            };

            // Enforce pin cap.
            if self.max_bytes != 0
                && state.pinned_bytes.saturating_add(entry_size) > pin_cap
            {
                warn!(
                    pinned_bytes = state.pinned_bytes,
                    entry_size,
                    pin_cap,
                    ?key,
                    batch_pinned = pinned,
                    remaining = keys.len() - pinned,
                    "pin cap exceeded in batch pin, stopping"
                );
                break;
            }

            state.pinned_keys.insert(key.clone());
            state.pin_times.insert(key.clone(), Instant::now());
            state.pinned_bytes += entry_size;
            pinned += 1;
        }
        pinned
    }

    /// Unpin a key, allowing eviction again. Idempotent.
    pub fn unpin_key(&self, key: &Q) {
        let mut state = lock_with_metrics!(self, "unpin_key");
        if state.pinned_keys.remove(key) {
            state.pin_times.remove(key);
            // Subtract the entry size from pinned_bytes if the entry still exists.
            let entry_size = state
                .lru
                .peek(key)
                .map(|item| item.data.len())
                .unwrap_or(0);
            state.pinned_bytes = state.pinned_bytes.saturating_sub(entry_size);
        }
    }

    /// Returns the total bytes currently pinned.
    pub fn pinned_bytes(&self) -> u64 {
        lock_with_metrics!(self, "pinned_bytes").pinned_bytes
    }

    pub async fn enable_filtering(&self) {
        let mut state = lock_with_metrics!(self, "enable_filtering");
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
    pub async fn range<F>(&self, prefix_range: impl RangeBounds<Q> + Send, mut handler: F) -> u64
    where
        F: FnMut(&K, &T) -> bool + Send,
        K: Ord,
    {
        let mut state = lock_with_metrics!(self, "range");
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
        lock_with_metrics!(self, "len_for_test").lru.len()
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

    /// Returns `true` if a specific entry has exceeded `max_seconds` TTL.
    fn is_entry_expired(&self, entry: &EvictionItem<T>) -> bool {
        if self.max_seconds == 0 {
            return false;
        }
        let elapsed_seconds =
            i32::try_from(self.anchor_time.elapsed().as_secs()).unwrap_or(i32::MAX);
        let evict_older_than_seconds = elapsed_seconds.saturating_sub(self.max_seconds);
        entry.seconds_since_anchor < evict_older_than_seconds
    }

    /// Check if the state needs eviction based on the LRU peek.
    /// Returns `true` if eviction is needed, `false` otherwise.
    fn state_needs_eviction(&self, state: &State<K, Q, T, C>) -> bool {
        let Some((_, peek_entry)) = state.lru.peek_lru() else {
            return false;
        };
        self.should_evict(
            state.lru.len(),
            peek_entry,
            state.sum_store_size,
            self.max_bytes,
        )
    }

    /// Evict at most `max_items` entries from the cache, returning the evicted
    /// data, removal callback futures, and whether more eviction is still needed.
    #[must_use]
    fn evict_items_batch(
        &self,
        state: &mut State<K, Q, T, C>,
        max_items: usize,
    ) -> (Vec<T>, Vec<RemoveFuture>, bool) {
        let Some((_, mut peek_entry)) = state.lru.peek_lru() else {
            return (Vec::new(), Vec::new(), false);
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

        let elapsed_seconds =
            i32::try_from(self.anchor_time.elapsed().as_secs()).unwrap_or(i32::MAX);

        let mut items_to_unref = Vec::new();
        let mut removal_futures = Vec::new();
        let mut skipped_pinned = Vec::new();
        let mut evicted_count = 0;

        while evicted_count < max_items
            && self.should_evict(
                state.lru.len() + skipped_pinned.len(),
                peek_entry,
                state.sum_store_size,
                max_bytes,
            )
        {
            let (key, eviction_item) = state
                .lru
                .pop_lru()
                .expect("Tried to peek() then pop() but failed");

            if state.pinned_keys.contains(key.borrow()) {
                // Check if the pin has expired.
                let pin_expired = state
                    .pin_times
                    .get(key.borrow())
                    .map_or(true, |t| t.elapsed().as_secs() >= PIN_TIMEOUT_SECS);

                if pin_expired {
                    let entry_size = eviction_item.data.len();
                    warn!(
                        ?key,
                        pin_timeout_secs = PIN_TIMEOUT_SECS,
                        entry_size,
                        "auto-unpinning expired pin"
                    );
                    state.pinned_keys.remove(key.borrow());
                    state.pin_times.remove(key.borrow());
                    state.pinned_bytes = state.pinned_bytes.saturating_sub(entry_size);
                    // Fall through to normal eviction below.
                } else {
                    skipped_pinned.push((key, eviction_item));
                    peek_entry = match state.lru.peek_lru() {
                        Some((_, entry)) => entry,
                        None => break,
                    };
                    continue;
                }
            }

            let age_secs = elapsed_seconds.saturating_sub(eviction_item.seconds_since_anchor);
            let size = eviction_item.data.len();
            let evict_older_than_seconds = elapsed_seconds.saturating_sub(self.max_seconds);
            let effective_count = state.lru.len() + skipped_pinned.len();
            let reason = if self.max_seconds != 0
                && eviction_item.seconds_since_anchor < evict_older_than_seconds
            {
                "max_seconds (TTL) expired"
            } else if self.max_count != 0
                && u64::try_from(effective_count).unwrap_or(u64::MAX) > self.max_count
            {
                "max_count exceeded"
            } else if max_bytes != 0 && state.sum_store_size > max_bytes {
                "max_bytes exceeded"
            } else {
                "evict_bytes headroom"
            };
            if age_secs < 120 {
                warn!(
                    ?key, age_secs, size, reason,
                    current_count = effective_count,
                    max_count = self.max_count,
                    current_bytes = state.sum_store_size,
                    max_bytes,
                    "EvictingMap: evicting recently-inserted item",
                );
            } else {
                debug!(
                    ?key, age_secs, size, reason,
                    current_count = effective_count,
                    max_count = self.max_count,
                    current_bytes = state.sum_store_size,
                    max_bytes,
                    "EvictingMap: evicting item",
                );
            }
            let (data, futures) = state.remove(key.borrow(), &eviction_item, false);
            items_to_unref.push(data);
            removal_futures.extend(futures.into_iter());
            evicted_count += 1;

            peek_entry = if let Some((_, entry)) = state.lru.peek_lru() {
                entry
            } else {
                break;
            };
        }

        // Re-insert pinned items back into LRU at LRU position (not MRU).
        for (key, item) in skipped_pinned {
            state.lru.push(key, item);
        }
        // Demote all pinned keys to LRU position after re-insertion.
        for pinned_key in &state.pinned_keys {
            state.lru.demote(pinned_key.borrow());
        }

        let more_to_evict = self.state_needs_eviction(state);
        (items_to_unref, removal_futures, more_to_evict)
    }

    /// Signal the background eviction loop, or perform a small inline safety
    /// valve eviction if the map has grown beyond 110% of max_bytes.
    /// Returns evicted items only when inline eviction was needed.
    fn notify_eviction_with_safety_valve(
        &self,
        state: &mut State<K, Q, T, C>,
    ) -> (Vec<T>, Vec<RemoveFuture>) {
        if self.background_eviction_running.load(Ordering::Relaxed) {
            // Check safety valve: if we exceed 110% of max_bytes, do a small
            // inline eviction to prevent unbounded growth.
            let safety_threshold = if self.max_bytes != 0 {
                self.max_bytes + self.max_bytes / 10
            } else {
                0
            };
            if safety_threshold != 0 && state.sum_store_size > safety_threshold {
                warn!(
                    sum_store_size = state.sum_store_size,
                    max_bytes = self.max_bytes,
                    safety_threshold,
                    "EvictingMap: safety valve triggered, inline eviction of up to 10 items"
                );
                let (items, futures, _) = self.evict_items_batch(state, 10);
                // Still signal background loop for remaining work.
                self.eviction_notify.notify_one();
                return (items, futures);
            }
            self.eviction_notify.notify_one();
            return (Vec::new(), Vec::new());
        }
        // Fallback: no background loop, evict inline (original behavior).
        let (items, futures, _) = self.evict_items_batch(state, usize::MAX);
        (items, futures)
    }

    /// Run the background eviction loop. Call this from a spawned task via
    /// `start_background_eviction()`. Waits for eviction signals and evicts
    /// in batches to limit lock hold time per acquisition.
    async fn eviction_loop(self: &Arc<Self>) {
        const BATCH_SIZE: usize = 100;
        loop {
            self.eviction_notify.notified().await;
            // Evict in batches to keep lock holds short.
            loop {
                let (items_to_unref, removal_futures, more_to_evict) = {
                    let mut state = lock_with_metrics!(self, "background_evict");
                    if !self.state_needs_eviction(&state) {
                        break;
                    }
                    self.evict_items_batch(&mut state, BATCH_SIZE)
                };
                // Process eviction callbacks and unrefs OUTSIDE the lock.
                if !removal_futures.is_empty() || !items_to_unref.is_empty() {
                    let mut futures: FuturesUnordered<_> =
                        removal_futures.into_iter().collect();
                    while futures.next().await.is_some() {}
                    let mut callbacks: FuturesUnordered<_> =
                        items_to_unref.iter().map(LenEntry::unref).collect();
                    while callbacks.next().await.is_some() {}
                }
                if !more_to_evict {
                    break;
                }
                // Yield between batches to let other operations proceed.
                tokio::task::yield_now().await;
            }
        }
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
        let (removal_futures, data_to_unref, needs_eviction) = {
            let mut state = lock_with_metrics!(self, "sizes_for_keys");

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
                        // we are here (TTL expiration is per-item and quick).
                        if self.should_evict(lru_len, entry, 0, u64::MAX) {
                            *result = None;
                            if let Some((key, eviction_item)) = state.lru.pop_entry(key.borrow()) {
                                let elapsed_seconds =
                                    i32::try_from(self.anchor_time.elapsed().as_secs()).unwrap_or(i32::MAX);
                                let age_secs = elapsed_seconds.saturating_sub(eviction_item.seconds_since_anchor);
                                let size = eviction_item.data.len();
                                if age_secs < 120 {
                                    warn!(
                                        ?key, age_secs, size,
                                        reason = "max_seconds (TTL) expired",
                                        max_seconds = self.max_seconds,
                                        "EvictingMap: expired recently-inserted item on lookup",
                                    );
                                } else {
                                    debug!(
                                        ?key, age_secs, size,
                                        reason = "max_seconds (TTL) expired",
                                        max_seconds = self.max_seconds,
                                        "EvictingMap: item expired on lookup, evicting",
                                    );
                                }
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
            // Check if size/count-based eviction is needed and signal background.
            let needs_eviction = self.state_needs_eviction(&state);
            (removal_futures, data_to_unref, needs_eviction)
        };

        // Signal background eviction for size/count-based eviction.
        if needs_eviction {
            self.eviction_notify.notify_one();
        }

        // Fire-and-forget TTL eviction cleanup in background.
        if !removal_futures.is_empty() || !data_to_unref.is_empty() {
            drop(background_spawn!("evicting_map_sizes_cleanup", async move {
                let mut callbacks: FuturesUnordered<_> = removal_futures.into_iter().collect();
                while callbacks.next().await.is_some() {}
                let mut callbacks: FuturesUnordered<_> =
                    data_to_unref.iter().map(LenEntry::unref).collect();
                while callbacks.next().await.is_some() {}
            }));
        }
    }

    pub async fn get(&self, key: &Q) -> Option<T> {
        let (result, needs_eviction) = {
            let mut state = lock_with_metrics!(self, "get");
            let needs_eviction = self.state_needs_eviction(&state);

            let result = state.lru.get_mut(key.borrow()).and_then(|entry| {
                // Check TTL: if the entry is expired, treat it as missing.
                if self.is_entry_expired(entry) {
                    return None;
                }
                entry.seconds_since_anchor =
                    i32::try_from(self.anchor_time.elapsed().as_secs()).unwrap_or(i32::MAX);
                Some(entry.data.clone())
            });

            (result, needs_eviction)
        };

        // Signal background eviction if needed (no inline eviction on read path).
        if needs_eviction {
            self.eviction_notify.notify_one();
        }

        result
    }

    /// Retrieves multiple entries in a single lock acquisition, reducing
    /// contention compared to calling `get()` in a loop.
    pub async fn get_many<'b, Iter>(&self, keys: Iter) -> Vec<Option<T>>
    where
        Iter: IntoIterator<Item = &'b Q>,
        Q: 'b,
    {
        let (results, needs_eviction) = {
            let mut state = lock_with_metrics!(self, "get_many");
            let needs_eviction = self.state_needs_eviction(&state);

            let now = i32::try_from(self.anchor_time.elapsed().as_secs()).unwrap_or(i32::MAX);
            let results: Vec<Option<T>> = keys
                .into_iter()
                .map(|key: &'b Q| {
                    state.lru.get_mut(key.borrow()).and_then(|entry| {
                        // Check TTL: if the entry is expired, treat it as missing.
                        if self.is_entry_expired(entry) {
                            return None;
                        }
                        entry.seconds_since_anchor = now;
                        Some(entry.data.clone())
                    })
                })
                .collect();

            (results, needs_eviction)
        };

        // Signal background eviction if needed (no inline eviction on read path).
        if needs_eviction {
            self.eviction_notify.notify_one();
        }

        results
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
        let (replaced_items, evicted_items, removal_futures, insert_notifications, callbacks) = {
            let mut state = lock_with_metrics!(self, "insert");
            let result =
                self.inner_insert_many(&mut state, [(key, data)], seconds_since_anchor);
            // Clone callback list while we hold the lock so we can fire
            // them after releasing, avoiding a second lock acquisition.
            let callbacks = if !result.3.is_empty() {
                state.item_callbacks.clone()
            } else {
                Vec::new()
            };
            (result.0, result.1, result.2, result.3, callbacks)
        };
        // Fire insert callbacks without holding the lock.
        for (key, size) in &insert_notifications {
            for cb in &callbacks {
                cb.on_insert(key.borrow(), *size);
            }
        }

        // Replaced items share the same key (and thus content path) as the
        // new insert. Their unrefs MUST complete before the caller continues
        // to rename the new file into the same path.
        let result = if !replaced_items.is_empty() {
            let futures: FuturesUnordered<_> = replaced_items
                .into_iter()
                .map(|item| async move {
                    item.unref().await;
                    item
                })
                .collect();
            futures.collect::<Vec<_>>().await.into_iter().next()
        } else {
            None
        };

        // Fire-and-forget eviction cleanup (different keys, no path conflict)
        // and removal callbacks (cache invalidation, protected by stale-positive handling).
        if !removal_futures.is_empty() || !evicted_items.is_empty() {
            drop(background_spawn!("evicting_map_insert_cleanup", async move {
                let mut futures: FuturesUnordered<_> = removal_futures.into_iter().collect();
                while futures.next().await.is_some() {}
                let mut callbacks: FuturesUnordered<_> =
                    evicted_items.iter().map(LenEntry::unref).collect();
                while callbacks.next().await.is_some() {}
            }));
        }

        result
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

        let (replaced_items, evicted_items, removal_futures, insert_notifications, callbacks) = {
            let mut state = lock_with_metrics!(self, "insert_many");
            let result = self.inner_insert_many(
                &mut state,
                inserts,
                i32::try_from(self.anchor_time.elapsed().as_secs()).unwrap_or(i32::MAX),
            );
            // Clone callback list while we hold the lock so we can fire
            // them after releasing, avoiding a second lock acquisition.
            let callbacks = if !result.3.is_empty() {
                state.item_callbacks.clone()
            } else {
                Vec::new()
            };
            (result.0, result.1, result.2, result.3, callbacks)
        };
        // Fire insert callbacks without holding the lock.
        for (key, size) in &insert_notifications {
            for cb in &callbacks {
                cb.on_insert(key.borrow(), *size);
            }
        }

        // Replaced items share the same key/path — must await their unrefs.
        let result: Vec<T> = replaced_items
            .into_iter()
            .map(|item| async move {
                item.unref().await;
                item
            })
            .collect::<FuturesUnordered<_>>()
            .collect::<Vec<_>>()
            .await;

        // Fire-and-forget eviction cleanup (different keys, no path conflict).
        if !removal_futures.is_empty() || !evicted_items.is_empty() {
            drop(background_spawn!("evicting_map_insert_many_cleanup", async move {
                let mut futures: FuturesUnordered<_> = removal_futures.into_iter().collect();
                while futures.next().await.is_some() {}
                let mut callbacks: FuturesUnordered<_> =
                    evicted_items.iter().map(LenEntry::unref).collect();
                while callbacks.next().await.is_some() {}
            }));
        }

        result
    }

    /// Returns `(replaced_items, evicted_items, removal_futures, insert_notifications)`.
    /// - `replaced_items`: items that were replaced by new inserts (same key).
    /// - `evicted_items`: items evicted due to size/age/count limits.
    /// - `removal_futures`: callbacks from item_callbacks for all removed items.
    /// - `insert_notifications`: (key, size) pairs for firing on_insert callbacks
    ///   outside the State mutex critical section.
    ///
    /// Callers should fire-and-forget the eviction cleanup (evicted_items unrefs
    /// + removal_futures) via `background_spawn!` to avoid blocking the caller.
    /// Callers MUST fire on_insert callbacks for each insert_notification after
    /// releasing the State mutex to avoid nested locking.
    fn inner_insert_many<It>(
        &self,
        state: &mut State<K, Q, T, C>,
        inserts: It,
        seconds_since_anchor: i32,
    ) -> (Vec<T>, Vec<T>, Vec<RemoveFuture>, Vec<(K, u64)>)
    where
        It: IntoIterator<Item = (K, T)> + Send,
        // Note: It's not enough to have the inserts themselves be Send. The
        // returned iterator should be Send as well.
        <It as IntoIterator>::IntoIter: Send,
    {
        let mut replaced_items = Vec::new();
        let mut removal_futures = Vec::new();
        let mut insert_notifications = Vec::new();
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
            insert_notifications.push((key, new_item_size));
        }

        // Signal background eviction or do a small inline safety valve
        // eviction if the map has grown beyond 110% of max_bytes.
        let (evicted_items, futures) = self.notify_eviction_with_safety_valve(state);
        removal_futures.extend(futures);

        (replaced_items, evicted_items, removal_futures, insert_notifications)
    }

    pub async fn remove(&self, key: &Q) -> bool {
        let (removed_item, removal_futures, needs_eviction, was_expired) = {
            let mut state = lock_with_metrics!(self, "remove");
            let needs_eviction = self.state_needs_eviction(&state);

            // Try to remove the requested item.
            let (removed_item, removal_futures, was_expired) =
                if let Some(entry) = state.lru.pop(key.borrow()) {
                    // If the entry was TTL-expired, still remove it but report
                    // it as "not found" to the caller.
                    let expired = self.is_entry_expired(&entry);
                    let (item, futures) = state.remove(key, &entry, false);
                    (Some(item), futures, expired)
                } else {
                    (None, Vec::new(), false)
                };

            (removed_item, removal_futures, needs_eviction, was_expired)
        };

        // Signal background eviction if needed.
        if needs_eviction {
            self.eviction_notify.notify_one();
        }

        let was_removed = removed_item.is_some() && !was_expired;

        // Fire-and-forget cleanup for the removed item and callbacks.
        if !removal_futures.is_empty() || removed_item.is_some() {
            drop(background_spawn!("evicting_map_remove_cleanup", async move {
                let mut futures: FuturesUnordered<_> = removal_futures.into_iter().collect();
                while futures.next().await.is_some() {}
                let mut callbacks: FuturesUnordered<_> = removed_item
                    .iter()
                    .map(LenEntry::unref)
                    .collect();
                while callbacks.next().await.is_some() {}
            }));
        }

        was_removed
    }

    /// Same as `remove()`, but allows for a conditional to be applied to the
    /// entry before removal in an atomic fashion.
    pub async fn remove_if<F>(&self, key: &Q, cond: F) -> bool
    where
        F: FnOnce(&T) -> bool + Send,
    {
        let (removal_futures, removed_item, needs_eviction) = {
            let mut state = lock_with_metrics!(self, "remove_if");
            if let Some(entry) = state.lru.get(key.borrow()) {
                if !cond(&entry.data) {
                    return false;
                }
                let needs_eviction = self.state_needs_eviction(&state);

                // Try to remove the requested item.
                let (removed_item, removal_futures) =
                    if let Some(entry) = state.lru.pop(key.borrow()) {
                        let (item, futures) = state.remove(key, &entry, false);
                        (Some(item), futures)
                    } else {
                        (None, Vec::new())
                    };

                (removal_futures, removed_item, needs_eviction)
            } else {
                return false;
            }
        };

        // Signal background eviction if needed.
        if needs_eviction {
            self.eviction_notify.notify_one();
        }

        let was_removed = removed_item.is_some();

        // Fire-and-forget cleanup for the removed item and callbacks.
        if !removal_futures.is_empty() || removed_item.is_some() {
            drop(background_spawn!("evicting_map_remove_if_cleanup", async move {
                let mut futures: FuturesUnordered<_> = removal_futures.into_iter().collect();
                while futures.next().await.is_some() {}
                let mut callbacks: FuturesUnordered<_> = removed_item
                    .iter()
                    .map(LenEntry::unref)
                    .collect();
                while callbacks.next().await.is_some() {}
            }));
        }

        was_removed
    }

    pub fn add_item_callback(&self, callback: C) {
        lock_with_metrics!(self, "add_item_callback").add_item_callback(callback);
    }

    /// Returns all entries in the cache with their LRU timestamps as absolute
    /// seconds since UNIX epoch. Each entry is (key, unix_timestamp_secs).
    ///
    /// This is a peek-only operation: it does NOT promote entries in the LRU.
    pub fn get_all_entries_with_timestamps(&self) -> Vec<(K, i64)> {
        let anchor_epoch = self.anchor_time.unix_timestamp() as i64;
        let state = lock_with_metrics!(self, "get_all_entries_with_timestamps");
        let mut result = Vec::with_capacity(state.lru.len());
        result.extend(state.lru.iter().map(|(k, v)| {
            (k.clone(), anchor_epoch + v.seconds_since_anchor as i64)
        }));
        result
    }
}

/// Separate impl block for `start_background_eviction` which requires
/// `'static` + `Send` bounds for spawning a background task.
impl<K, Q, T, I, C> EvictingMap<K, Q, T, I, C>
where
    K: Ord + Hash + Eq + Clone + Debug + Send + Sync + Borrow<Q> + 'static,
    Q: Ord + Hash + Eq + Debug + Send + Sync + 'static,
    T: LenEntry + Debug + Clone + Send + Sync + 'static,
    I: InstantWrapper + 'static,
    C: ItemCallback<Q> + Clone + 'static,
{
    /// Start the background eviction loop. Should be called once after
    /// construction when a tokio runtime is available. Safe to call multiple
    /// times (only the first call spawns the loop).
    pub fn start_background_eviction(self: &Arc<Self>) {
        if self
            .background_eviction_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return; // Already running.
        }
        let this = Arc::clone(self);
        drop(background_spawn!("evicting_map_background_eviction", async move {
            this.eviction_loop().await;
        }));
    }
}

/// Target number of independent shards used by `ShardedEvictingMap`.
/// Power of 2 for fast modulo via bitmask. The actual count may be
/// reduced when configured limits are too small for meaningful sharding.
const TARGET_NUM_SHARDS: usize = 64;

/// Minimum per-shard capacity in bytes (or count) required for sharding
/// to be meaningful. If the total divided by shards is below this, we
/// reduce the shard count. These thresholds ensure each shard can hold
/// enough items to provide useful LRU ordering.
const MIN_PER_SHARD_BYTES: usize = 256 * 1024; // 256 KiB
const MIN_PER_SHARD_COUNT: u64 = 64;

/// A sharded wrapper around `EvictingMap` that distributes keys across
/// multiple independent instances, each with its own lock.
/// This reduces lock contention proportionally to the shard count compared
/// to a single `EvictingMap`.
///
/// The public API mirrors `EvictingMap` so callers are unaware of sharding.
#[derive(Debug)]
pub struct ShardedEvictingMap<
    K: Ord + Hash + Eq + Clone + Debug + Send + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug,
    T: LenEntry + Debug + Send,
    I: InstantWrapper,
    C: ItemCallback<Q> = NoopCallback,
> {
    shards: Vec<Arc<EvictingMap<K, Q, T, I, C>>>,
    /// Bitmask for fast shard index computation. Equal to `shards.len() - 1`.
    shard_mask: usize,
}

impl<K, Q, T, I, C> MetricsComponent for ShardedEvictingMap<K, Q, T, I, C>
where
    K: Ord + Hash + Eq + Clone + Debug + Send + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug,
    T: LenEntry + Debug + Send,
    I: InstantWrapper,
    C: ItemCallback<Q>,
{
    fn publish(
        &self,
        kind: nativelink_metric::MetricKind,
        field_metadata: nativelink_metric::MetricFieldData,
    ) -> Result<nativelink_metric::MetricPublishKnownKindData, nativelink_metric::Error> {
        // Publish metrics from shard 0 as representative.
        // Note: counter values (evicted_bytes, etc.) represent 1/num_shards
        // of the total. Config values (max_bytes) show per-shard limits.
        // TODO: Aggregate counters across all shards for accurate totals.
        self.shards[0].publish(kind, field_metadata)
    }
}

impl<K, Q, T, I, C> ShardedEvictingMap<K, Q, T, I, C>
where
    K: Ord + Hash + Eq + Clone + Debug + Send + Sync + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug + Sync,
    T: LenEntry + Debug + Clone + Send + Sync,
    I: InstantWrapper + Clone,
    C: ItemCallback<Q> + Clone,
{
    pub fn new(config: &EvictionPolicy, anchor_time: I) -> Self {
        // Choose shard count: start at TARGET_NUM_SHARDS and reduce (halving)
        // until each shard has at least MIN_PER_SHARD_BYTES bytes capacity
        // and MIN_PER_SHARD_COUNT count capacity (when the respective limits
        // are non-zero). Always at least 1 shard.
        //
        // When no eviction limits are configured (all zeros), use a single
        // shard to avoid spawning unnecessary background eviction tasks.
        let has_any_limit =
            config.max_bytes > 0 || config.max_count > 0 || config.max_seconds > 0;
        let mut num_shards = if has_any_limit {
            TARGET_NUM_SHARDS
        } else {
            1
        };
        if config.max_bytes > 0 {
            while num_shards > 1 && config.max_bytes / num_shards < MIN_PER_SHARD_BYTES {
                num_shards /= 2;
            }
        }
        if config.max_count > 0 {
            while num_shards > 1 && config.max_count / num_shards as u64 <= MIN_PER_SHARD_COUNT {
                num_shards /= 2;
            }
        }

        let mut shard_config = config.clone();
        shard_config.max_bytes /= num_shards;
        if shard_config.max_count > 0 {
            shard_config.max_count /= num_shards as u64;
        }
        if shard_config.evict_bytes > 0 {
            shard_config.evict_bytes /= num_shards;
        }
        // max_seconds is a per-item TTL — stays the same.

        let shards = (0..num_shards)
            .map(|_| Arc::new(EvictingMap::new(&shard_config, anchor_time.clone())))
            .collect();
        let shard_mask = num_shards - 1;
        Self { shards, shard_mask }
    }

    /// Compute the shard index for a given key.
    #[inline]
    fn shard_index(&self, key: &Q) -> usize {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        hasher.finish() as usize & self.shard_mask
    }

    /// Return a reference to the shard for a given key.
    #[inline]
    fn shard_for_key(&self, key: &Q) -> &Arc<EvictingMap<K, Q, T, I, C>> {
        &self.shards[self.shard_index(key)]
    }

    // --- Single-key operations ---

    pub fn pin_key(&self, key: K) -> bool {
        self.shard_for_key(key.borrow()).pin_key(key)
    }

    pub fn pin_keys(&self, keys: &[K]) -> usize {
        // Group keys by shard to batch pin operations within each shard.
        let mut groups: Vec<Vec<K>> = (0..self.shards.len()).map(|_| Vec::new()).collect();
        for key in keys {
            groups[self.shard_index(key.borrow())].push(key.clone());
        }
        let mut total = 0;
        for (idx, group) in groups.iter().enumerate() {
            if !group.is_empty() {
                total += self.shards[idx].pin_keys(group);
            }
        }
        total
    }

    pub fn unpin_key(&self, key: &Q) {
        self.shard_for_key(key).unpin_key(key);
    }

    pub fn pinned_bytes(&self) -> u64 {
        self.shards.iter().map(|s| s.pinned_bytes()).sum()
    }

    pub async fn enable_filtering(&self) {
        for shard in &self.shards {
            shard.enable_filtering().await;
        }
    }

    pub async fn range<F>(&self, prefix_range: impl RangeBounds<Q> + Clone + Send, mut handler: F) -> u64
    where
        F: FnMut(&K, &T) -> bool + Send,
        K: Ord,
    {
        // Collect all matching (key, value) pairs from all shards, then sort
        // by key so the caller sees globally-sorted order.
        let mut all_entries: Vec<(K, T)> = Vec::new();
        for shard in &self.shards {
            shard
                .range(prefix_range.clone(), |k, v| {
                    all_entries.push((k.clone(), v.clone()));
                    true
                })
                .await;
        }
        all_entries.sort_by(|(a, _), (b, _)| a.cmp(b));

        let mut count = 0;
        for (key, value) in &all_entries {
            if !handler(key, value) {
                break;
            }
            count += 1;
        }
        count
    }

    pub async fn len_for_test(&self) -> usize {
        let mut total = 0;
        for shard in &self.shards {
            total += shard.len_for_test().await;
        }
        total
    }

    pub async fn size_for_key(&self, key: &Q) -> Option<u64> {
        self.shard_for_key(key).size_for_key(key).await
    }

    pub async fn sizes_for_keys<It, R>(&self, keys: It, results: &mut [Option<u64>], peek: bool)
    where
        It: IntoIterator<Item = R> + Send,
        <It as IntoIterator>::IntoIter: Send,
        R: Borrow<Q> + Send,
    {
        // Group (original_index, key_ref) by shard, then batch-lookup each shard
        // concurrently. Each shard has an independent lock, so parallel queries
        // avoid head-of-line blocking behind a slow shard.
        let keys_vec: Vec<R> = keys.into_iter().collect();
        let mut shard_groups: Vec<Vec<usize>> = vec![Vec::new(); self.shards.len()];
        for (i, key) in keys_vec.iter().enumerate() {
            let shard_idx = self.shard_index(key.borrow());
            shard_groups[shard_idx].push(i);
        }

        let mut futures: FuturesUnordered<_> = shard_groups
            .iter()
            .enumerate()
            .filter(|(_, indices)| !indices.is_empty())
            .map(|(shard_idx, indices)| {
                let shard = &self.shards[shard_idx];
                let shard_keys: Vec<&Q> = indices.iter().map(|&i| keys_vec[i].borrow()).collect();
                async move {
                    let mut shard_results = vec![None; shard_keys.len()];
                    shard
                        .sizes_for_keys(shard_keys.into_iter(), &mut shard_results, peek)
                        .await;
                    (shard_idx, shard_results)
                }
            })
            .collect();

        while let Some((shard_idx, shard_results)) = futures.next().await {
            // Scatter results back to the original positions.
            for (j, &orig_idx) in shard_groups[shard_idx].iter().enumerate() {
                results[orig_idx] = shard_results[j];
            }
        }
    }

    pub async fn get(&self, key: &Q) -> Option<T> {
        self.shard_for_key(key).get(key).await
    }

    pub async fn get_many<'b, Iter>(&self, keys: Iter) -> Vec<Option<T>>
    where
        Iter: IntoIterator<Item = &'b Q>,
        Q: 'b,
    {
        // Group keys by shard, batch-lookup each concurrently, scatter results back.
        let keys_vec: Vec<&'b Q> = keys.into_iter().collect();
        let mut results = vec![None; keys_vec.len()];
        let mut shard_groups: Vec<Vec<usize>> = vec![Vec::new(); self.shards.len()];
        for (i, key) in keys_vec.iter().enumerate() {
            shard_groups[self.shard_index(*key)].push(i);
        }

        let mut futures: FuturesUnordered<_> = shard_groups
            .iter()
            .enumerate()
            .filter(|(_, indices)| !indices.is_empty())
            .map(|(shard_idx, indices)| {
                let shard = &self.shards[shard_idx];
                let shard_keys: Vec<&'b Q> = indices.iter().map(|&i| keys_vec[i]).collect();
                async move {
                    let shard_results = shard.get_many(shard_keys).await;
                    (shard_idx, shard_results)
                }
            })
            .collect();

        while let Some((shard_idx, shard_results)) = futures.next().await {
            for (j, &orig_idx) in shard_groups[shard_idx].iter().enumerate() {
                results[orig_idx] = shard_results[j].clone();
            }
        }
        results
    }

    pub async fn insert(&self, key: K, data: T) -> Option<T>
    where
        K: 'static,
    {
        self.shard_for_key(key.borrow()).insert(key, data).await
    }

    pub async fn insert_with_time(&self, key: K, data: T, seconds_since_anchor: i32) -> Option<T> {
        self.shard_for_key(key.borrow())
            .insert_with_time(key, data, seconds_since_anchor)
            .await
    }

    pub async fn insert_many<It>(&self, inserts: It) -> Vec<T>
    where
        It: IntoIterator<Item = (K, T)> + Send,
        <It as IntoIterator>::IntoIter: Send,
        K: 'static,
    {
        // Group inserts by shard, then insert_many each batch concurrently.
        let mut shard_groups: Vec<Vec<(K, T)>> = (0..self.shards.len()).map(|_| Vec::new()).collect();
        for (key, data) in inserts {
            let idx = self.shard_index(key.borrow());
            shard_groups[idx].push((key, data));
        }

        let mut futures: FuturesUnordered<_> = shard_groups
            .into_iter()
            .enumerate()
            .filter(|(_, group)| !group.is_empty())
            .map(|(shard_idx, group)| {
                let shard = &self.shards[shard_idx];
                async move { shard.insert_many(group).await }
            })
            .collect();

        let mut all_replaced = Vec::new();
        while let Some(replaced) = futures.next().await {
            all_replaced.extend(replaced);
        }
        all_replaced
    }

    pub async fn remove(&self, key: &Q) -> bool {
        self.shard_for_key(key).remove(key).await
    }

    pub async fn remove_if<F>(&self, key: &Q, cond: F) -> bool
    where
        F: FnOnce(&T) -> bool + Send,
    {
        self.shard_for_key(key).remove_if(key, cond).await
    }

    pub fn add_item_callback(&self, callback: C) {
        for shard in &self.shards {
            shard.add_item_callback(callback.clone());
        }
    }

    pub fn get_all_entries_with_timestamps(&self) -> Vec<(K, i64)> {
        let mut all_entries = Vec::new();
        for shard in &self.shards {
            all_entries.extend(shard.get_all_entries_with_timestamps());
        }
        all_entries
    }

    /// Provides direct read access to the lock contention metrics from
    /// all shards. Returns a reference to the underlying shard `LockMetrics`.
    /// For aggregate reporting, callers should iterate `lock_metrics_all_shards()`.
    pub fn lock_metrics_all_shards(&self) -> impl Iterator<Item = &LockMetrics> {
        self.shards.iter().map(|s| &s.lock_metrics)
    }
}

/// Separate impl block for `start_background_eviction` which requires
/// `'static` + `Send` bounds for spawning background tasks.
impl<K, Q, T, I, C> ShardedEvictingMap<K, Q, T, I, C>
where
    K: Ord + Hash + Eq + Clone + Debug + Send + Sync + Borrow<Q> + 'static,
    Q: Ord + Hash + Eq + Debug + Send + Sync + 'static,
    T: LenEntry + Debug + Clone + Send + Sync + 'static,
    I: InstantWrapper + 'static,
    C: ItemCallback<Q> + Clone + 'static,
{
    /// Start the background eviction loop on every shard.
    pub fn start_background_eviction(&self) {
        for shard in &self.shards {
            shard.start_background_eviction();
        }
    }
}
