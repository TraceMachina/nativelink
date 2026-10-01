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

use core::pin::Pin;
use std::borrow::Cow;
use std::sync::{Arc, Weak};
use std::time::SystemTime;

use async_trait::async_trait;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use nativelink_config::stores::{EvictionPolicy, ExistenceCacheSpec};
use nativelink_error::{Error, ResultExt, error_if};
use nativelink_metric::MetricsComponent;
use nativelink_util::background_spawn;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::evicting_map::{EvictingMap, LenEntry};
use nativelink_util::health_utils::{HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::store_trait::{
    RemoveCallback, RemoveItemCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use parking_lot::Mutex;
use tracing::{debug, info, trace};

#[derive(Clone, Debug)]
struct ExistenceItem(u64);

impl LenEntry for ExistenceItem {
    #[inline]
    fn len(&self) -> u64 {
        self.0
    }

    #[inline]
    fn is_empty(&self) -> bool {
        false
    }
}

/// Tracks in-flight operations that must defer remove callbacks.
///
/// `pause_count` is the number of concurrent operations (e.g. `update()`
/// calls) currently holding the pause. While it is non-zero, remove
/// callbacks from the inner store are queued in `pending` instead of being
/// applied immediately. Only when the *last* holder releases the pause
/// (count drops to zero) is the queue drained, after every holder has
/// inserted its key into the existence cache. This guarantees that an
/// eviction notification for a key delivered during that key's own
/// operation is applied *after* the insert, so the cache cannot be left
/// claiming existence of a blob the inner store dropped.
#[derive(Debug, Default)]
struct PauseState {
    pause_count: usize,
    pending: Vec<StoreKey<'static>>,
}

impl PauseState {
    /// Release one hold. Returns the queued removals to drain iff this was the
    /// last holder; otherwise returns an empty vec and leaves the queue for a
    /// later holder to drain.
    fn release(&mut self) -> Vec<StoreKey<'static>> {
        self.pause_count = self
            .pause_count
            .checked_sub(1)
            .expect("PauseState::release called without a matching pause");
        if self.pause_count == 0 {
            core::mem::take(&mut self.pending)
        } else {
            Vec::new()
        }
    }
}

/// RAII hold on the remove-callback pause.
///
/// Dropping the guard always decrements the pause count, so a cancelled
/// operation cannot leave invalidation globally suspended (finding N6). The
/// normal path calls [`PauseGuard::resume`] instead. If the guard is dropped
/// while it is the last holder with a non-empty queue (only reachable on
/// cancellation), those removals must still be applied — otherwise the cache
/// keeps claiming a blob exists after the inner store evicted it (C20/N6 drop
/// residual). Since `Drop` cannot await and the removal is async, `Drop` spawns
/// a detached task (via the weak self-reference) to drain them.
struct PauseGuard<'a, I: InstantWrapper> {
    store: Option<&'a ExistenceCacheStore<I>>,
}

impl<I: InstantWrapper> PauseGuard<'_, I> {
    /// Release the hold and drain queued removals if this was the last holder.
    async fn resume(mut self) {
        // Take the store so `Drop` becomes a no-op; we handle release here.
        let Some(store) = self.store.take() else {
            return;
        };
        let keys = store.pause_remove_callbacks.lock().release();
        store.drain_pending_removals(keys).await;
    }
}

impl<I: InstantWrapper> Drop for PauseGuard<'_, I> {
    fn drop(&mut self) {
        let Some(store) = self.store.take() else {
            return;
        };
        // Cancellation path: release the count so the pause mechanism is never
        // left permanently engaged (finding N6).
        let keys = store.pause_remove_callbacks.lock().release();
        if keys.is_empty() {
            return;
        }
        // We were the last holder and there are removals that were queued
        // *before* this operation was cancelled (C20/N6 drop residual). These
        // must still be applied, otherwise the cache can keep claiming a blob
        // exists after the inner store evicted it. `Drop` cannot await, and the
        // removal is async, so spawn a detached task that drains them. The
        // weak self-reference keeps this `'static` without extending the
        // store's lifetime past its last strong owner.
        let weak = store.self_ref.lock().clone();
        if let Some(store) = weak.upgrade() {
            background_spawn!("existence_cache_drain_pending_removals", async move {
                store.drain_pending_removals(keys).await;
            });
        }
    }
}

#[derive(Debug, MetricsComponent)]
pub struct ExistenceCacheStore<I: InstantWrapper> {
    #[metric(group = "inner_store")]
    inner_store: Store,
    existence_cache: EvictingMap<DigestInfo, DigestInfo, ExistenceItem, I>,

    // We need to pause remove callbacks temporarily while operating on the
    // inner store; see `PauseState`.
    pause_remove_callbacks: Mutex<PauseState>,

    // A weak self-reference, set once at construction. The cancellation path in
    // `PauseGuard::drop` needs an owned (`'static`) handle to the store so it
    // can spawn the async drain of any removals queued before cancellation; a
    // borrowed `&self` cannot outlive the `Drop` call. See `PauseGuard::drop`.
    self_ref: Mutex<Weak<Self>>,
}

impl ExistenceCacheStore<SystemTime> {
    pub fn new(spec: &ExistenceCacheSpec, inner_store: Store) -> Arc<Self> {
        Self::new_with_time(spec, inner_store, SystemTime::now())
    }
}

impl<I: InstantWrapper> RemoveItemCallback for ExistenceCacheStore<I> {
    fn callback<'a>(
        &'a self,
        store_key: StoreKey<'a>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        debug!(?store_key, "Removing item from cache due to callback");
        let digest = store_key.borrow().into_digest();
        Box::pin(async move {
            let deleted_key = self.existence_cache.remove(&digest).await;
            if !deleted_key {
                info!(?store_key, "Failed to delete key from cache on callback");
            }
        })
    }
}

#[derive(Debug)]
struct ExistenceCacheCallback<I: InstantWrapper> {
    cache: Weak<ExistenceCacheStore<I>>,
}

impl<I: InstantWrapper> RemoveItemCallback for ExistenceCacheCallback<I> {
    fn callback<'a>(
        &'a self,
        store_key: StoreKey<'a>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        let cache = self.cache.upgrade();
        if let Some(local_cache) = cache {
            let mut state = local_cache.pause_remove_callbacks.lock();
            if state.pause_count > 0 {
                state.pending.push(store_key.into_owned());
            } else {
                drop(state);
                let store_key = store_key.into_owned();
                return Box::pin(async move {
                    local_cache.callback(store_key).await;
                });
            }
        } else {
            debug!("Cache dropped, so not doing callback");
        }
        Box::pin(async {})
    }
}

impl<I: InstantWrapper> ExistenceCacheStore<I> {
    pub fn new_with_time(
        spec: &ExistenceCacheSpec,
        inner_store: Store,
        anchor_time: I,
    ) -> Arc<Self> {
        let empty_policy = EvictionPolicy::default();
        let eviction_policy = spec.eviction_policy.as_ref().unwrap_or(&empty_policy);
        let existence_cache_store = Arc::new(Self {
            inner_store,
            existence_cache: EvictingMap::new(eviction_policy, anchor_time),
            pause_remove_callbacks: Mutex::new(PauseState::default()),
            self_ref: Mutex::new(Weak::new()),
        });
        *existence_cache_store.self_ref.lock() = Arc::downgrade(&existence_cache_store);
        let other_ref = Arc::downgrade(&existence_cache_store);
        existence_cache_store
            .inner_store
            .register_remove_callback(Arc::new(ExistenceCacheCallback { cache: other_ref }))
            .expect("Register remove callback should work");
        existence_cache_store
    }

    pub async fn exists_in_cache(&self, digest: &DigestInfo) -> bool {
        let mut results = [None];
        self.existence_cache
            .sizes_for_keys([digest], &mut results[..], true /* peek */)
            .await;
        results[0].is_some()
    }

    pub async fn remove_from_cache(&self, digest: &DigestInfo) {
        self.existence_cache.remove(digest).await;
    }

    /// Begin deferring remove callbacks for the duration of an operation and
    /// return an RAII guard that releases the hold when dropped.
    ///
    /// The guard's `Drop` decrements the pause count unconditionally, so a
    /// cancelled operation (e.g. an `update`/`get_part` future dropped mid-await
    /// by a timeout) can never leave the pause count permanently elevated and
    /// disable invalidation globally. On the normal path, call
    /// [`PauseGuard::resume`] to release the hold *and* drain any queued
    /// removals once the last holder is gone.
    fn pause_remove_callbacks(&self) -> PauseGuard<'_, I> {
        self.pause_remove_callbacks.lock().pause_count += 1;
        PauseGuard { store: Some(self) }
    }

    /// Drain the queued removals when the last holder has released. Called by
    /// [`PauseGuard::resume`]; the count has already been decremented there.
    async fn drain_pending_removals(&self, keys: Vec<StoreKey<'static>>) {
        let mut callbacks: FuturesUnordered<_> = keys
            .into_iter()
            .map(|store_key| self.callback(store_key))
            .collect();
        while callbacks.next().await.is_some() {}
    }

    async fn inner_has_with_results(
        self: Pin<&Self>,
        keys: &[DigestInfo],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        self.existence_cache
            .sizes_for_keys(keys, results, true /* peek */)
            .await;

        let not_cached_keys: Vec<_> = keys
            .iter()
            .zip(results.iter())
            .filter_map(|(digest, result)| result.map_or_else(|| Some(digest.into()), |_| None))
            .collect();

        // Hot path optimization when all keys are cached.
        if not_cached_keys.is_empty() {
            return Ok(());
        }

        // Pause remove callbacks over the inner `has` + the insert below
        // (finding C13). The inner store may evict one of `not_cached_keys`
        // between answering `has` positively and our insert; without the pause
        // that eviction's callback would run before the insert and be a no-op,
        // leaving the cache claiming existence of a blob the inner store
        // dropped. Queuing it until after the insert lets the drain remove it.
        let pause = self.pause_remove_callbacks();

        // Now query only the items not found in the cache.
        let mut inner_results = vec![None; not_cached_keys.len()];
        let has_result = self
            .inner_store
            .has_with_results(&not_cached_keys, &mut inner_results)
            .await
            .err_tip(|| "In ExistenceCacheStore::inner_has_with_results");
        if let Err(err) = has_result {
            pause.resume().await;
            return Err(err);
        }

        // Insert found from previous query into our cache.
        {
            // Note: Sadly due to some weird lifetime issues we need to collect here, but
            // in theory we don't actually need to collect.
            let inserts = not_cached_keys
                .iter()
                .zip(inner_results.iter())
                .filter_map(|(key, result)| {
                    result.map(|size| (key.borrow().into_digest(), ExistenceItem(size)))
                })
                .collect::<Vec<_>>();
            drop(self.existence_cache.insert_many(inserts).await);
        }

        // Insert is done; release the pause and drain any eviction queued
        // during the inner `has` so it lands after our insert (C13).
        pause.resume().await;

        // Merge the results from the cache and the query.
        {
            let mut inner_results_iter = inner_results.into_iter();
            // We know at this point that any None in results was queried and will have
            // a result in inner_results_iter, so use this knowledge to fill in the results.
            for result in results.iter_mut() {
                if result.is_none() {
                    *result = inner_results_iter
                        .next()
                        .expect("has_with_results returned less results than expected");
                }
            }
            // Ensure that there was no logic error by ensuring our iterator is not empty.
            error_if!(
                inner_results_iter.next().is_some(),
                "has_with_results returned more results than expected"
            );
        }

        Ok(())
    }
}

#[async_trait]
impl<I: InstantWrapper> StoreDriver for ExistenceCacheStore<I> {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        self.inner_store.clone().into_inner().post_init().await?;
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        // TODO(palfrey) This is a bit of a hack to get around the lifetime issues with the
        // existence_cache. We need to convert the digests to owned values to be able to
        // insert them into the cache. In theory it should be able to elide this conversion
        // but it seems to be a bit tricky to get right.
        let digests: Vec<_> = digests
            .iter()
            .map(|key| key.borrow().into_digest())
            .collect();
        self.inner_has_with_results(&digests, results).await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        let digest = key.into_digest();
        let mut exists = [None];
        self.inner_has_with_results(&[digest], &mut exists)
            .await
            .err_tip(|| "In ExistenceCacheStore::update")?;
        if exists[0].is_some() {
            // We need to drain the reader to avoid the writer complaining that we dropped
            // the connection prematurely.
            let size = reader
                .drain()
                .await
                .err_tip(|| "In ExistenceCacheStore::update")?;
            return Ok(size);
        }
        let pause = self.pause_remove_callbacks();
        trace!(?digest, "Inserting into inner cache");
        let result = self.inner_store.update(digest, reader, size_info).await;
        if let Ok(size) = &result {
            trace!(?digest, "Inserting into existence cache");
            let _ = self
                .existence_cache
                .insert(digest, ExistenceItem(*size))
                .await;
        }
        // Note: the insert above happens *before* releasing the pause, so a
        // removal for `digest` queued during the inner update is applied
        // after the insert and cannot be lost. If this future is cancelled
        // before here, `pause`'s Drop still releases the hold.
        pause.resume().await;
        result
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        let digest = key.into_digest();
        // Pause remove callbacks over the read + insert: the inner store may
        // evict `digest` while a read of it is still in flight (e.g. a
        // filesystem store keeps streaming from an open file handle). The
        // eviction is queued and replayed after our insert, so the cache
        // cannot resurrect an entry the inner store dropped mid-read.
        let pause = self.pause_remove_callbacks();
        let result = self
            .inner_store
            .get_part(digest, writer, offset, length)
            .await;
        if result.is_ok() {
            let _ = self
                .existence_cache
                .insert(digest, ExistenceItem(digest.size_bytes()))
                .await;
        }
        pause.resume().await;
        result
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any<'a>(&'a self) -> &'a (dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_remove_callback(self: Arc<Self>, callback: RemoveCallback) -> Result<(), Error> {
        self.inner_store.register_remove_callback(callback)
    }
}

#[async_trait]
impl<I: InstantWrapper> HealthStatusIndicator for ExistenceCacheStore<I> {
    fn get_name(&self) -> &'static str {
        "ExistenceCacheStore"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}
