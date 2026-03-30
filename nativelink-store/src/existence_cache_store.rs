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
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use nativelink_config::stores::{EvictionPolicy, ExistenceCacheSpec};
use nativelink_error::{Error, ResultExt, error_if};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::evicting_map::{LenEntry, ShardedEvictingMap};
use nativelink_util::health_utils::{HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::store_trait::{
    ItemCallback, Store, StoreDriver, StoreKey, StoreLike, StoreOptimizations, UploadSizeInfo,
};
use parking_lot::Mutex;
use tracing::{debug, error, info, trace};

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

#[derive(Debug, MetricsComponent)]
pub struct ExistenceCacheStore<I: InstantWrapper> {
    #[metric(group = "inner_store")]
    inner_store: Store,
    existence_cache: Arc<ShardedEvictingMap<DigestInfo, DigestInfo, ExistenceItem, I>>,

    // We need to pause them temporarily when inserting into the inner store
    // as if it immediately expires them, we should only apply the remove callbacks
    // afterwards. If this is None, we're not pausing; if it's Some it's the location to
    // store them in temporarily
    pause_item_callbacks: Mutex<Option<Vec<StoreKey<'static>>>>,
}

impl ExistenceCacheStore<SystemTime> {
    pub fn new(spec: &ExistenceCacheSpec, inner_store: Store) -> Arc<Self> {
        Self::new_with_time(spec, inner_store, SystemTime::now())
    }
}

impl<I: InstantWrapper> ItemCallback for ExistenceCacheStore<I> {
    fn callback<'a>(
        &'a self,
        store_key: StoreKey<'a>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        debug!(?store_key, "ExistenceCacheStore: eviction callback received");
        let digest = store_key.borrow().into_digest();
        Box::pin(async move {
            let deleted_key = self.existence_cache.remove(&digest).await;
            if deleted_key {
                debug!(?store_key, "ExistenceCacheStore: eviction callback removed key from cache");
            } else {
                debug!(?store_key, "ExistenceCacheStore: eviction callback key not in cache (already removed or never cached)");
            }
        })
    }
}

#[derive(Debug)]
struct ExistenceCacheCallback<I: InstantWrapper> {
    cache: Weak<ExistenceCacheStore<I>>,
}

impl<I: InstantWrapper> ItemCallback for ExistenceCacheCallback<I> {
    fn callback<'a>(
        &'a self,
        store_key: StoreKey<'a>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        let cache = self.cache.upgrade();
        if let Some(local_cache) = cache {
            if let Some(callbacks) = local_cache.pause_item_callbacks.lock().as_mut() {
                callbacks.push(store_key.into_owned());
            } else {
                let store_key = store_key.into_owned();
                return Box::pin(async move {
                    local_cache.callback(store_key).await;
                });
            }
        } else {
            debug!("ExistenceCacheStore: eviction callback skipped (cache dropped)");
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
        let existence_cache = Arc::new(ShardedEvictingMap::new(eviction_policy, anchor_time));
        existence_cache.start_background_eviction();
        let existence_cache_store = Arc::new(Self {
            inner_store,
            existence_cache,
            pause_item_callbacks: Mutex::new(None),
        });
        let other_ref = Arc::downgrade(&existence_cache_store);
        existence_cache_store
            .inner_store
            .register_item_callback(Arc::new(ExistenceCacheCallback { cache: other_ref }))
            .expect("Register item callback should work");
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

        // Now query only the items not found in the cache.
        let mut inner_results = vec![None; not_cached_keys.len()];
        self.inner_store
            .has_with_results(&not_cached_keys, &mut inner_results)
            .await
            .err_tip(|| "In ExistenceCacheStore::inner_has_with_results")?;

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
    ) -> Result<(), Error> {
        let digest = key.into_digest();
        // Check the inner store directly, bypassing the existence cache.
        // The existence cache may have a stale positive for a blob that was
        // evicted from the inner store (the async eviction callback may not
        // have fired yet). Trusting the cache here would skip the upload,
        // causing Bazel's "Lost inputs no longer available remotely" error.
        let mut exists = [None];
        self.inner_store
            .has_with_results(&[digest.into()], &mut exists)
            .await
            .err_tip(|| "In ExistenceCacheStore::update")?;
        if exists[0].is_some() {
            // Blob genuinely exists in the inner store — safe to skip.
            reader
                .drain()
                .await
                .err_tip(|| "In ExistenceCacheStore::update")?;
            // Refresh the existence cache since we verified it exists.
            let _ = self
                .existence_cache
                .insert(digest, ExistenceItem(exists[0].unwrap()))
                .await;
            return Ok(());
        }
        // If the existence cache had a stale entry, remove it now.
        self.existence_cache.remove(&digest).await;
        {
            let mut locked_callbacks = self.pause_item_callbacks.lock();
            if locked_callbacks.is_none() {
                locked_callbacks.replace(vec![]);
            }
        }
        trace!(?digest, "Inserting into inner cache");
        let update_start = std::time::Instant::now();
        let result = self.inner_store.update(digest, reader, size_info).await;
        let elapsed_ms = update_start.elapsed().as_millis() as u64;
        if let Err(ref err) = result {
            error!(
                ?digest,
                elapsed_ms,
                ?err,
                "ExistenceCacheStore::update: inner store write failed",
            );
        } else if elapsed_ms > 100 {
            info!(
                ?digest,
                elapsed_ms,
                "ExistenceCacheStore::update: inner store write slow",
            );
        }
        if result.is_ok() {
            trace!(?digest, "Inserting into existence cache");
            // Cache on both ExactSize and MaxSize — the digest carries the
            // authoritative size for content-addressed blobs.
            let size = match size_info {
                UploadSizeInfo::ExactSize(size) => size,
                UploadSizeInfo::MaxSize(_) => digest.size_bytes(),
            };
            let _ = self
                .existence_cache
                .insert(digest, ExistenceItem(size))
                .await;

        }
        {
            let maybe_keys = self.pause_item_callbacks.lock().take();
            if let Some(keys) = maybe_keys {
                let mut callbacks: FuturesUnordered<_> = keys
                    .into_iter()
                    .map(|store_key| self.callback(store_key))
                    .collect();
                while callbacks.next().await.is_some() {}
            }
        }
        result
    }

    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        optimization == StoreOptimizations::SubscribesToUpdateOneshot
    }

    async fn update_oneshot(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        data: Bytes,
    ) -> Result<(), Error> {
        let digest = key.into_digest();
        // Bypass the existence cache and check inner store directly.
        // Same stale-positive prevention as update().
        let mut exists = [None];
        self.inner_store
            .has_with_results(&[digest.into()], &mut exists)
            .await
            .err_tip(|| "In ExistenceCacheStore::update_oneshot")?;
        if exists[0].is_some() {
            // Blob genuinely exists in the inner store — safe to skip.
            let _ = self
                .existence_cache
                .insert(digest, ExistenceItem(exists[0].unwrap()))
                .await;
            return Ok(());
        }
        // If the existence cache had a stale entry, remove it now.
        self.existence_cache.remove(&digest).await;
        {
            let mut locked_callbacks = self.pause_item_callbacks.lock();
            if locked_callbacks.is_none() {
                locked_callbacks.replace(vec![]);
            }
        }
        trace!(?digest, "Inserting into inner cache via update_oneshot");
        let update_start = std::time::Instant::now();
        let size = u64::try_from(data.len())
            .err_tip(|| "Could not convert data.len() to u64 in update_oneshot")?;
        let result = self.inner_store.update_oneshot(digest, data).await;
        let elapsed_ms = update_start.elapsed().as_millis() as u64;
        if let Err(ref err) = result {
            error!(
                ?digest,
                elapsed_ms,
                ?err,
                "ExistenceCacheStore::update_oneshot: inner store write failed",
            );
        } else if elapsed_ms > 100 {
            info!(
                ?digest,
                elapsed_ms,
                "ExistenceCacheStore::update_oneshot: inner store write slow",
            );
        }
        if result.is_ok() {
            trace!(?digest, "Inserting into existence cache via update_oneshot");
            let _ = self
                .existence_cache
                .insert(digest, ExistenceItem(size))
                .await;
        }
        {
            let maybe_keys = self.pause_item_callbacks.lock().take();
            if let Some(keys) = maybe_keys {
                let mut callbacks: FuturesUnordered<_> = keys
                    .into_iter()
                    .map(|store_key| self.callback(store_key))
                    .collect();
                while callbacks.next().await.is_some() {}
            }
        }
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
        let result = self
            .inner_store
            .get_part(digest, writer, offset, length)
            .await;
        match &result {
            Ok(()) => {
                let _ = self
                    .existence_cache
                    .insert(digest, ExistenceItem(digest.size_bytes()))
                    .await;
            }
            Err(err) if err.code == nativelink_error::Code::NotFound => {
                // Blob was evicted from the inner store — remove the stale
                // existence cache entry so subsequent has() calls get an
                // accurate result.
                self.existence_cache.remove(&digest).await;
            }
            Err(_) => {}
        }
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

    fn register_item_callback(
        self: Arc<Self>,
        callback: Arc<dyn ItemCallback>,
    ) -> Result<(), Error> {
        self.inner_store.register_item_callback(callback)
    }

    fn drain_stable_digests(&self) -> Vec<DigestInfo> {
        self.inner_store.drain_stable_digests()
    }

    fn pin_digests(&self, digests: &[DigestInfo]) {
        self.inner_store.pin_digests(digests);
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
