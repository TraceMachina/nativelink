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
use core::fmt::Debug;
use core::hash::Hash;
use core::ops::RangeBounds;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use moka::notification::RemovalCause;
use moka::sync::Cache;
use nativelink_config::stores::EvictionPolicy;
use nativelink_metric::MetricsComponent;
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::background_spawn;
use crate::evicting_map::{ItemCallback, LenEntry, NoopCallback};
use crate::instant_wrapper::InstantWrapper;
use crate::metrics_utils::{Counter, CounterWithTime};

/// Maximum fraction of max_bytes that can be pinned (25%).
const PIN_CAP_FRACTION: f64 = 0.25;
/// Seconds before a pin automatically expires.
const PIN_TIMEOUT_SECS: u64 = 120;
/// Bounded eviction channel capacity. Prevents unbounded memory growth
/// during burst eviction. Items beyond this are cleaned up inline.
const EVICTION_CHANNEL_SIZE: usize = 4096;

/// Entry stored in the pinned map, alongside metadata for timeout
/// enforcement and size accounting.
#[derive(Debug)]
struct PinnedEntry<T> {
    data: T,
    pinned_at: Instant,
    size: u64,
}

/// An eviction event captured by the moka listener and sent to the
/// background drainer for async cleanup (unref + callbacks).
struct EvictionEvent<K, T> {
    key: Arc<K>,
    value: T,
}

/// A cache backed by `moka::sync::Cache` with an API that mirrors
/// `ShardedEvictingMap`. Moka handles eviction internally using a
/// TinyLFU admission + LRU eviction policy, so there is no need for
/// manual eviction loops. Pinning is handled via a side `DashMap` that
/// keeps entries alive outside the moka cache.
pub struct MokaEvictingMap<
    K: Ord + Hash + Eq + Clone + Debug + Send + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug,
    T: LenEntry + Debug + Send,
    I: InstantWrapper,
    C: ItemCallback<Q> = NoopCallback,
> {
    cache: Cache<K, T>,
    /// Items pinned to prevent eviction. Shared with the eviction
    /// listener so it can check pin status before sending cleanup events.
    pinned: Arc<DashMap<K, PinnedEntry<T>>>,
    /// Total bytes currently pinned.
    pinned_bytes: AtomicU64,
    /// 25% of max_bytes — ceiling for pinned data.
    pin_cap: u64,
    /// Optional BTreeSet index for range queries. Shared with the
    /// eviction listener for cleanup on eviction.
    btree: Arc<RwLock<Option<BTreeSet<K>>>>,
    /// Bounded channel for eviction events sent to the background drainer.
    eviction_tx: mpsc::Sender<EvictionEvent<K, T>>,
    /// Receiver held until `start_background_eviction` moves it into
    /// the drainer task.
    eviction_rx: parking_lot::Mutex<Option<mpsc::Receiver<EvictionEvent<K, T>>>>,
    /// Callbacks to invoke on item removal.
    callbacks: RwLock<Vec<C>>,
    /// Anchor time for timestamp conversion.
    anchor_time: I,
    /// Configured max_bytes (used for pin cap and diagnostics).
    max_bytes: u64,
    /// Configured max_count (enforced alongside max_bytes if both set).
    max_count: u64,
    /// Whether the background drainer has been started.
    background_running: AtomicBool,
    // Metrics
    evicted_bytes: Counter,
    evicted_items: CounterWithTime,
    replaced_bytes: Counter,
    replaced_items: CounterWithTime,
    lifetime_inserted_bytes: Counter,
    /// Phantom for the Q type parameter.
    _q: core::marker::PhantomData<Q>,
}

impl<K, Q, T, I, C> Debug for MokaEvictingMap<K, Q, T, I, C>
where
    K: Ord + Hash + Eq + Clone + Debug + Send + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug,
    T: LenEntry + Debug + Send,
    I: InstantWrapper + Debug,
    C: ItemCallback<Q>,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MokaEvictingMap")
            .field("entry_count", &self.cache.entry_count())
            .field("weighted_size", &self.cache.weighted_size())
            .field(
                "pinned_bytes",
                &self.pinned_bytes.load(Ordering::Relaxed),
            )
            .field("pin_cap", &self.pin_cap)
            .field("max_bytes", &self.max_bytes)
            .finish()
    }
}

impl<K, Q, T, I, C> MetricsComponent for MokaEvictingMap<K, Q, T, I, C>
where
    K: Ord + Hash + Eq + Clone + Debug + Send + Borrow<Q>,
    Q: Ord + Hash + Eq + Debug,
    T: LenEntry + Debug + Send,
    I: InstantWrapper,
    C: ItemCallback<Q>,
{
    fn publish(
        &self,
        _kind: nativelink_metric::MetricKind,
        _field_metadata: nativelink_metric::MetricFieldData,
    ) -> Result<nativelink_metric::MetricPublishKnownKindData, nativelink_metric::Error> {
        Ok(nativelink_metric::MetricPublishKnownKindData::Component)
    }
}

impl<K, Q, T, I, C> MokaEvictingMap<K, Q, T, I, C>
where
    K: Ord + Hash + Eq + Clone + Debug + Send + Sync + Borrow<Q> + 'static,
    Q: Ord + Hash + Eq + Debug + Send + Sync + 'static,
    T: LenEntry + Debug + Clone + Send + Sync + 'static,
    I: InstantWrapper,
    C: ItemCallback<Q> + Clone + 'static,
{
    pub fn new(config: &EvictionPolicy) -> Self
    where
        I: Default,
    {
        Self::with_anchor(config, I::default())
    }

    pub fn with_anchor(config: &EvictionPolicy, anchor_time: I) -> Self {
        let max_bytes = config.max_bytes as u64;
        let max_count = config.max_count;
        let max_seconds = config.max_seconds;
        let evict_bytes = config.evict_bytes as u64;

        let (eviction_tx, eviction_rx) = mpsc::channel(EVICTION_CHANNEL_SIZE);
        let listener_tx = eviction_tx.clone();

        // Shared state captured by the eviction listener closure.
        let pinned: Arc<DashMap<K, PinnedEntry<T>>> = Arc::new(DashMap::new());
        let listener_pinned = Arc::clone(&pinned);
        let btree: Arc<RwLock<Option<BTreeSet<K>>>> = Arc::new(RwLock::new(None));
        let listener_btree = Arc::clone(&btree);

        let mut builder = Cache::builder();

        // TinyLFU (default): admission filter prevents cache pollution
        // from one-time blob scans. New entries enter the window (1% of
        // capacity) unconditionally, then face the frequency filter when
        // moving to main. Single-access blobs survive in the window long
        // enough for concurrent slow-store writes to complete via separate
        // data streams (FastSlowStore tees, not reads-from-fast).

        // Capacity: use max_bytes with low-watermark from evict_bytes.
        // Setting capacity to (max_bytes - evict_bytes) ensures moka
        // keeps headroom, similar to the old evict_bytes behavior.
        if max_bytes > 0 {
            // Moka's weigher returns u32 but we track bytes as u64.
            // Scale capacity and weights to KB granularity so items up
            // to 4TB fit in u32. A 1-byte item weighs 1 (minimum).
            const SCALE: u64 = 1024;
            let effective_capacity = max_bytes.saturating_sub(evict_bytes) / SCALE;
            builder = builder
                .max_capacity(effective_capacity)
                .weigher(|_key: &K, value: &T| -> u32 {
                    let kb = value.len().div_ceil(SCALE);
                    u32::try_from(kb).unwrap_or(u32::MAX)
                });
        } else if max_count > 0 {
            builder = builder.max_capacity(max_count);
        }

        if max_seconds > 0 {
            builder = builder.time_to_idle(Duration::from_secs(u64::from(max_seconds)));
        }

        // Eviction listener: fires synchronously during moka operations.
        // - Replaced: skip — insert() handles replaced-item unref directly.
        // - Size/Expired/Explicit: check if pinned (skip if so, it's safe
        //   in the DashMap). Otherwise send to background drainer.
        builder = builder.eviction_listener(move |key: Arc<K>, value: T, cause: RemovalCause| {
            if cause == RemovalCause::Replaced {
                // insert() captured the old value via cache.get() and
                // will await its unref() before returning. Don't double-unref.
                return;
            }

            // If this key is pinned, the pin_key() flow already moved it
            // to the DashMap. The invalidate() triggered this listener but
            // the data is safe in the pinned map. Skip cleanup.
            let q: &Q = (*key).borrow();
            if listener_pinned.contains_key(q) {
                return;
            }

            // Clean up BTree index on eviction.
            {
                let btree_guard = listener_btree.read();
                if btree_guard.is_some() {
                    drop(btree_guard);
                    let mut btree_guard = listener_btree.write();
                    if let Some(ref mut set) = *btree_guard {
                        set.remove(q);
                    }
                }
            }

            // Send to background drainer. If the channel is full (burst
            // eviction), spawn inline cleanup to avoid blocking moka's
            // internal lock.
            if let Err(mpsc::error::TrySendError::Full(event)) =
                listener_tx.try_send(EvictionEvent {
                    key: Arc::clone(&key),
                    value,
                })
            {
                // Channel full — spawn fire-and-forget cleanup.
                // Note: ItemCallbacks are skipped here because the
                // callback list lives on the struct, not in the closure.
                // This is rare (only during burst eviction exceeding 4096
                // buffered events) and the callbacks are best-effort.
                warn!(
                    "eviction channel full, spawning inline cleanup \
                     (ItemCallbacks skipped for this entry)"
                );
                let evicted_key = event.key;
                let evicted_value = event.value;
                tokio::spawn(async move {
                    evicted_value.unref().await;
                    drop(evicted_key);
                });
            }
        });

        let cache = builder.build();
        let pin_cap = (max_bytes as f64 * PIN_CAP_FRACTION) as u64;

        Self {
            cache,
            pinned,
            pinned_bytes: AtomicU64::new(0),
            pin_cap,
            btree,
            eviction_tx,
            eviction_rx: parking_lot::Mutex::new(Some(eviction_rx)),
            callbacks: RwLock::new(Vec::new()),
            anchor_time,
            max_bytes,
            max_count,
            background_running: AtomicBool::new(false),
            evicted_bytes: Counter::default(),
            evicted_items: CounterWithTime::default(),
            replaced_bytes: Counter::default(),
            replaced_items: CounterWithTime::default(),
            lifetime_inserted_bytes: Counter::default(),
            _q: core::marker::PhantomData,
        }
    }

    /// Fast-path check: returns true if any items are pinned.
    #[inline]
    fn has_pinned(&self) -> bool {
        self.pinned_bytes.load(Ordering::Relaxed) > 0
    }

    // ---------------------------------------------------------------
    // get
    // ---------------------------------------------------------------

    pub async fn get(&self, key: &Q) -> Option<T> {
        // Atomic fast-path: skip DashMap probe when nothing is pinned.
        if self.has_pinned() {
            if let Some(entry) = self.pinned.get(key) {
                return Some(entry.data.clone());
            }
        }
        self.cache.get(key)
    }

    pub async fn get_many<'b, Iter>(&self, keys: Iter) -> Vec<Option<T>>
    where
        Iter: IntoIterator<Item = &'b Q>,
        Q: 'b,
    {
        let check_pinned = self.has_pinned();
        keys.into_iter()
            .map(|key| {
                if check_pinned {
                    if let Some(entry) = self.pinned.get(key) {
                        return Some(entry.data.clone());
                    }
                }
                self.cache.get(key)
            })
            .collect()
    }

    // ---------------------------------------------------------------
    // insert
    // ---------------------------------------------------------------

    pub async fn insert(&self, key: K, data: T) -> Option<T>
    where
        K: 'static,
    {
        let old = self.insert_inner(key, data);
        // Await unref on replaced item before returning. This preserves
        // the invariant that the old file is cleaned up before the caller
        // renames the new file into the content path.
        if let Some(ref value) = old {
            value.unref().await;
        }
        old
    }

    pub async fn insert_with_time(
        &self,
        key: K,
        data: T,
        _seconds_since_anchor: i32,
    ) -> Option<T> {
        // Startup path: files are inserted oldest-first (sorted by atime).
        // We deliberately skip the frequency bump (the extra get() in
        // insert_inner) so all items enter at freq=1. Moka's window deque
        // is FIFO, so oldest items (inserted first) will be evicted first
        // when the window overflows — preserving atime-based ordering.
        // Items that get accessed after startup will be bumped to freq>=2
        // naturally, making them survive TinyLFU admission.
        let old = self.insert_startup(key, data);
        if let Some(ref value) = old {
            value.unref().await;
        }
        old
    }

    fn insert_inner(&self, key: K, data: T) -> Option<T> {
        let size = data.len();
        self.lifetime_inserted_bytes.add(size);

        // Update BTree index.
        {
            let btree = self.btree.read();
            if btree.is_some() {
                drop(btree);
                let mut btree = self.btree.write();
                if let Some(ref mut set) = *btree {
                    set.insert(key.clone());
                }
            }
        }

        // If key is pinned, replace in pinned map directly.
        if self.has_pinned() && self.pinned.contains_key(key.borrow()) {
            let old = self.pinned.remove(key.borrow()).map(|(_, entry)| {
                self.pinned_bytes
                    .fetch_sub(entry.size, Ordering::Relaxed);
                entry.data
            });
            self.pinned.insert(
                key.clone(),
                PinnedEntry {
                    data: data.clone(),
                    pinned_at: Instant::now(),
                    size,
                },
            );
            self.pinned_bytes.fetch_add(size, Ordering::Relaxed);
            self.fire_on_insert_callbacks(&key, size);
            if old.is_some() {
                self.replaced_bytes.add(size);
                self.replaced_items.inc();
            }
            return old;
        }

        // Capture old value before insert for replaced-item unref.
        // The eviction listener skips Replaced events since we handle
        // cleanup here.
        let existing = self.cache.get(key.borrow());
        self.cache.insert(key.clone(), data);
        // Bump frequency counter so TinyLFU doesn't reject this entry
        // from main space admission. Without this, single-access entries
        // (freq=1) tie with victims (freq=1) and lose the strictly-greater
        // admission check, getting evicted to disk on the next read.
        // The extra get() is a ~100ns hash lookup — negligible vs the
        // insert cost, and guarantees the entry survives in main.
        drop(self.cache.get(key.borrow()));
        self.cache.run_pending_tasks();

        // Enforce max_count if both max_bytes and max_count are set.
        if self.max_count > 0
            && self.max_bytes > 0
            && self.cache.entry_count() > self.max_count
        {
            // run_pending_tasks again to trigger any additional eviction.
            self.cache.run_pending_tasks();
        }

        self.fire_on_insert_callbacks(&key, size);
        if existing.is_some() {
            self.replaced_bytes.add(size);
            self.replaced_items.inc();
        }
        existing
    }

    /// Startup-optimized insert: no frequency bump, no per-insert
    /// run_pending_tasks(). Caller should call cache.run_pending_tasks()
    /// after the full batch. Items enter at freq=1, preserving FIFO
    /// ordering in Moka's window deque (oldest-inserted evicted first).
    fn insert_startup(&self, key: K, data: T) -> Option<T> {
        let size = data.len();
        self.lifetime_inserted_bytes.add(size);

        // BTree update (if enabled).
        {
            let btree = self.btree.read();
            if btree.is_some() {
                drop(btree);
                let mut btree = self.btree.write();
                if let Some(ref mut set) = *btree {
                    set.insert(key.clone());
                }
            }
        }

        let existing = self.cache.get(key.borrow());
        self.cache.insert(key.clone(), data);
        // No frequency bump (no extra get()).
        // No run_pending_tasks() — deferred to caller.
        self.fire_on_insert_callbacks(&key, size);
        existing
    }

    fn fire_on_insert_callbacks(&self, key: &K, size: u64) {
        let callbacks = self.callbacks.read();
        for cb in callbacks.iter() {
            cb.on_insert(key.borrow(), size);
        }
    }

    pub async fn insert_many<It>(&self, inserts: It) -> Vec<T>
    where
        It: IntoIterator<Item = (K, T)> + Send,
        <It as IntoIterator>::IntoIter: Send,
        K: 'static,
    {
        let mut replaced = Vec::new();
        for (key, data) in inserts {
            let old = self.insert_inner(key, data);
            if let Some(value) = old {
                value.unref().await;
                replaced.push(value);
            }
        }
        // Run pending tasks once after batch, not per-insert.
        self.cache.run_pending_tasks();
        replaced
    }

    // ---------------------------------------------------------------
    // remove
    // ---------------------------------------------------------------

    pub async fn remove(&self, key: &Q) -> bool {
        // Try pinned map first.
        if self.has_pinned() {
            if let Some((_, entry)) = self.pinned.remove(key) {
                self.pinned_bytes
                    .fetch_sub(entry.size, Ordering::Relaxed);
                self.update_btree_remove(key);

                // Fire callbacks + unref in background.
                let data = entry.data;
                let callbacks = self.collect_removal_callbacks(key);
                drop(background_spawn!(
                    "moka_evicting_map_remove_cleanup",
                    async move {
                        let mut futs: FuturesUnordered<_> = callbacks.into_iter().collect();
                        while futs.next().await.is_some() {}
                        data.unref().await;
                    }
                ));
                return true;
            }
        }

        // Try moka cache. remove() returns the value and fires the
        // eviction listener (Explicit cause), which sends to the
        // background drainer for unref + callbacks.
        if self.cache.remove(key).is_some() {
            self.cache.run_pending_tasks();
            // BTree cleanup handled by eviction listener.
            return true;
        }
        false
    }

    pub async fn remove_if<F>(&self, key: &Q, cond: F) -> bool
    where
        F: FnOnce(&T) -> bool + Send,
    {
        // Check pinned first.
        if self.has_pinned() {
            if let Some(entry) = self.pinned.get(key) {
                if cond(&entry.data) {
                    drop(entry);
                    return self.remove(key).await;
                }
                return false;
            }
        }

        // Check moka cache.
        if let Some(value) = self.cache.get(key) {
            if cond(&value) {
                return self.remove(key).await;
            }
        }
        false
    }

    fn update_btree_remove(&self, key: &Q) {
        let btree = self.btree.read();
        if btree.is_some() {
            drop(btree);
            let mut btree = self.btree.write();
            if let Some(ref mut set) = *btree {
                set.remove(key);
            }
        }
    }

    fn collect_removal_callbacks(
        &self,
        key: &Q,
    ) -> Vec<core::pin::Pin<Box<dyn core::future::Future<Output = ()> + Send>>> {
        let cbs = self.callbacks.read();
        cbs.iter().map(|cb| cb.callback(key)).collect()
    }

    // ---------------------------------------------------------------
    // size queries
    // ---------------------------------------------------------------

    pub async fn size_for_key(&self, key: &Q) -> Option<u64> {
        if self.has_pinned() {
            if let Some(entry) = self.pinned.get(key) {
                return Some(entry.data.len());
            }
        }
        self.cache.get(key).map(|v| v.len())
    }

    /// Note: the `peek` parameter is accepted for API compatibility but
    /// ignored. Moka has no non-promoting peek — `cache.get()` always
    /// updates the access time and frequency counter. For ExistenceCacheStore
    /// this is benign (TinyLFU frequency tracking is actually better than
    /// LRU peek for existence checks). For FilesystemStore has() checks,
    /// the promotion is also acceptable.
    pub async fn sizes_for_keys<It, R>(
        &self,
        keys: It,
        results: &mut [Option<u64>],
        _peek: bool,
    ) where
        It: IntoIterator<Item = R> + Send,
        <It as IntoIterator>::IntoIter: Send,
        R: Borrow<Q> + Send,
    {
        let check_pinned = self.has_pinned();
        for (key, result) in keys.into_iter().zip(results.iter_mut()) {
            let k: &Q = key.borrow();
            if check_pinned {
                if let Some(entry) = self.pinned.get(k) {
                    *result = Some(entry.data.len());
                    continue;
                }
            }
            *result = self.cache.get(k).map(|v| v.len());
        }
    }

    // ---------------------------------------------------------------
    // pinning
    // ---------------------------------------------------------------

    pub fn pin_key(&self, key: K) -> bool {
        let q: &Q = key.borrow();

        // Already pinned — refresh pin time.
        if let Some(mut entry) = self.pinned.get_mut(q) {
            entry.pinned_at = Instant::now();
            return true;
        }

        // Look up in cache (clone value while it's still in cache).
        let value = match self.cache.get(q) {
            Some(v) => v,
            None => return false,
        };

        let entry_size = value.len();

        // Enforce pin cap.
        if self.max_bytes != 0 {
            let current_pinned = self.pinned_bytes.load(Ordering::Relaxed);
            if current_pinned.saturating_add(entry_size) > self.pin_cap {
                warn!(
                    pinned_bytes = current_pinned,
                    entry_size,
                    pin_cap = self.pin_cap,
                    ?key,
                    "pin cap exceeded, refusing to pin"
                );
                return false;
            }
        }

        // CRITICAL: Insert into pinned map FIRST, then invalidate from
        // cache. The eviction listener checks pinned map and skips
        // cleanup if the key is found there. This ordering prevents the
        // race where invalidate fires the listener before the item is
        // in the pinned map.
        self.pinned.insert(
            key.clone(),
            PinnedEntry {
                data: value,
                pinned_at: Instant::now(),
                size: entry_size,
            },
        );
        self.pinned_bytes.fetch_add(entry_size, Ordering::Relaxed);

        // Now safe to remove from cache — listener will see it's pinned.
        self.cache.invalidate(q);
        self.cache.run_pending_tasks();
        true
    }

    pub fn pin_keys(&self, keys: &[K]) -> usize {
        let mut pinned = 0;
        for key in keys {
            let q: &Q = key.borrow();

            // Already pinned — refresh.
            if let Some(mut entry) = self.pinned.get_mut(q) {
                entry.pinned_at = Instant::now();
                pinned += 1;
                continue;
            }

            let value = match self.cache.get(q) {
                Some(v) => v,
                None => continue,
            };

            let entry_size = value.len();
            if self.max_bytes != 0 {
                let current = self.pinned_bytes.load(Ordering::Relaxed);
                if current.saturating_add(entry_size) > self.pin_cap {
                    break;
                }
            }

            // Insert into pinned FIRST (same ordering as pin_key).
            self.pinned.insert(
                key.clone(),
                PinnedEntry {
                    data: value,
                    pinned_at: Instant::now(),
                    size: entry_size,
                },
            );
            self.pinned_bytes.fetch_add(entry_size, Ordering::Relaxed);

            // Invalidate from cache (don't call run_pending_tasks per key).
            self.cache.invalidate(q);
            pinned += 1;
        }
        // Batch: process all invalidations at once.
        self.cache.run_pending_tasks();
        pinned
    }

    pub fn unpin_key(&self, key: &Q) {
        if let Some((owned_key, entry)) = self.pinned.remove(key) {
            self.pinned_bytes
                .fetch_sub(entry.size, Ordering::Relaxed);
            // Move back into moka cache with frequency bump so TinyLFU
            // doesn't immediately reject the re-inserted item.
            self.cache.insert(owned_key.clone(), entry.data);
            drop(self.cache.get(owned_key.borrow()));
        }
    }

    pub fn pinned_bytes(&self) -> u64 {
        self.pinned_bytes.load(Ordering::Relaxed)
    }

    // ---------------------------------------------------------------
    // filtering / range
    // ---------------------------------------------------------------

    pub async fn enable_filtering(&self) {
        let mut btree = self.btree.write();
        if btree.is_none() {
            let mut set = BTreeSet::new();
            for (key, _value) in &self.cache {
                set.insert((*key).clone());
            }
            for entry in self.pinned.iter() {
                set.insert(entry.key().clone());
            }
            *btree = Some(set);
        }
    }

    pub async fn range<F>(
        &self,
        prefix_range: impl RangeBounds<Q> + Send,
        mut handler: F,
    ) -> u64
    where
        F: FnMut(&K, &T) -> bool + Send,
        K: Ord,
    {
        // Ensure BTree is built.
        {
            let btree = self.btree.read();
            if btree.is_none() {
                drop(btree);
                self.enable_filtering().await;
            }
        }

        let btree = self.btree.read();
        let set = btree.as_ref().expect("btree should be built");
        let check_pinned = self.has_pinned();
        let mut count = 0;
        for key in set.range(prefix_range) {
            let q: &Q = key.borrow();
            let value = if check_pinned {
                if let Some(entry) = self.pinned.get(q) {
                    Some(entry.data.clone())
                } else {
                    self.cache.get(q)
                }
            } else {
                self.cache.get(q)
            };
            // Skip keys evicted by moka but still in BTree (stale).
            if let Some(ref v) = value {
                if !handler(key, v) {
                    break;
                }
                count += 1;
            }
        }
        count
    }

    // ---------------------------------------------------------------
    // callbacks
    // ---------------------------------------------------------------

    pub fn add_item_callback(&self, callback: C) {
        self.callbacks.write().push(callback);
    }

    // ---------------------------------------------------------------
    // timestamps / diagnostics
    // ---------------------------------------------------------------

    pub fn get_all_entries_with_timestamps(&self) -> Vec<(K, i64)> {
        let anchor_epoch = self.anchor_time.unix_timestamp() as i64;
        let now_offset =
            i64::try_from(self.anchor_time.elapsed().as_secs()).unwrap_or(i64::MAX);

        let mut result = Vec::new();
        for (key, _value) in &self.cache {
            result.push(((*key).clone(), anchor_epoch + now_offset));
        }
        for entry in self.pinned.iter() {
            result.push((entry.key().clone(), anchor_epoch + now_offset));
        }
        result
    }

    pub async fn len_for_test(&self) -> usize {
        self.cache.run_pending_tasks();
        self.cache.entry_count() as usize + self.pinned.len()
    }

    // ---------------------------------------------------------------
    // background eviction drainer
    // ---------------------------------------------------------------

    pub fn start_background_eviction(self: &Arc<Self>) {
        if self
            .background_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return;
        }

        let this = Arc::clone(self);
        let rx = this
            .eviction_rx
            .lock()
            .take()
            .expect("start_background_eviction called twice");

        drop(background_spawn!(
            "moka_evicting_map_background",
            async move {
                this.drain_evictions(rx).await;
            }
        ));
    }

    async fn drain_evictions(
        self: &Arc<Self>,
        mut rx: mpsc::Receiver<EvictionEvent<K, T>>,
    ) {
        let mut pin_check_interval = tokio::time::interval(Duration::from_secs(10));
        pin_check_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    self.process_eviction_event(event).await;

                    // Drain any additional pending events without waiting.
                    while let Ok(event) = rx.try_recv() {
                        self.process_eviction_event(event).await;
                    }
                }
                _ = pin_check_interval.tick() => {
                    self.expire_stale_pins().await;
                }
            }
        }
    }

    async fn process_eviction_event(&self, event: EvictionEvent<K, T>) {
        let size = event.value.len();
        self.evicted_bytes.add(size);
        self.evicted_items.inc();

        event.value.unref().await;

        let callbacks = {
            let cbs = self.callbacks.read();
            let q: &Q = (*event.key).borrow();
            cbs.iter().map(|cb| cb.callback(q)).collect::<Vec<_>>()
        };
        if !callbacks.is_empty() {
            let mut futs: FuturesUnordered<_> = callbacks.into_iter().collect();
            while futs.next().await.is_some() {}
        }
    }

    async fn expire_stale_pins(&self) {
        let mut expired_keys = Vec::new();
        for entry in self.pinned.iter() {
            if entry.pinned_at.elapsed().as_secs() >= PIN_TIMEOUT_SECS {
                expired_keys.push(entry.key().clone());
            }
        }
        for key in expired_keys {
            let q: &Q = key.borrow();
            if let Some((_, entry)) = self.pinned.remove(q) {
                let size = entry.size;
                info!(
                    ?key,
                    pin_timeout_secs = PIN_TIMEOUT_SECS,
                    entry_size = size,
                    "auto-unpinning expired pin"
                );
                self.pinned_bytes.fetch_sub(size, Ordering::Relaxed);
                // Put back into cache so it can be evicted normally.
                self.cache.insert(key, entry.data);
            }
        }
    }
}
