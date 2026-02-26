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
use nativelink_error::{Code, Error, ResultExt};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::evicting_map::{EvictingMap, LenEntry};
use nativelink_util::health_utils::{HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::store_trait::{
    RemoveItemCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use parking_lot::Mutex;
use tracing::{debug, trace};

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
    existence_cache: EvictingMap<DigestInfo, DigestInfo, ExistenceItem, I>,

    // We need to pause them temporarily when inserting into the inner store
    // as if it immediately expires them, we should only apply the remove callbacks
    // afterwards. If this is None, we're not pausing; if it's Some it's the location to
    // store them in temporarily
    pause_remove_callbacks: Mutex<Option<Vec<StoreKey<'static>>>>,
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
                debug!(?store_key, "Failed to delete key from cache on callback");
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
            if let Some(callbacks) = local_cache.pause_remove_callbacks.lock().as_mut() {
                callbacks.push(store_key.into_owned());
            } else {
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

    fn on_remove(&self, store_key: &StoreKey<'_>) {
        if let Some(local_cache) = self.cache.upgrade() {
            let digest = store_key.borrow().into_digest();
            local_cache.existence_cache.remove_sync(&digest);
        }
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
            pause_remove_callbacks: Mutex::new(None),
        });
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

    async fn inner_has_with_results(
        self: Pin<&Self>,
        keys: &[DigestInfo],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        // Always query the inner store. This:
        // 1. Returns ground-truth results (no stale positives)
        // 2. Promotes items in the inner store's LRU (peek=false),
        //    protecting them from eviction between FindMissingBlobs and Execute
        let store_keys: Vec<StoreKey<'_>> = keys.iter().map(|k| (*k).into()).collect();
        self.inner_store
            .has_with_results(&store_keys, results)
            .await
            .err_tip(|| "In ExistenceCacheStore::inner_has_with_results")
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
        // The existence cache may have a stale positive entry for a blob
        // that was evicted from the inner store (the async eviction callback
        // may not have fired yet). If we trusted the cache here, we would
        // skip the upload and the blob would remain missing — causing
        // Bazel's "Lost inputs no longer available remotely" error.
        let mut exists = [None];
        self.inner_store
            .has_with_results(&[digest.into()], &mut exists)
            .await
            .err_tip(|| "In ExistenceCacheStore::update")?;
        if exists[0].is_some() {
            // Blob genuinely exists in the inner store. Safe to skip.
            debug!(
                ?digest,
                size = exists[0].unwrap(),
                "ExistenceCacheStore: skipping upload, blob verified in inner store"
            );
            reader
                .drain()
                .await
                .err_tip(|| "In ExistenceCacheStore::update")?;
            // Refresh the existence cache entry since we verified it exists.
            let _ = self
                .existence_cache
                .insert(digest, ExistenceItem(exists[0].unwrap()))
                .await;
            return Ok(());
        }
        // If the existence cache had a stale entry, remove it now.
        self.existence_cache.remove(&digest).await;
        {
            let mut locked_callbacks = self.pause_remove_callbacks.lock();
            if locked_callbacks.is_none() {
                locked_callbacks.replace(vec![]);
            }
        }
        trace!(?digest, "Inserting into inner cache");
        let result = self.inner_store.update(digest, reader, size_info).await;
        if result.is_ok() {
            trace!(?digest, "Inserting into existence cache");
            // Always cache after a successful upload, regardless of whether
            // the size was ExactSize or MaxSize. The digest carries the
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
            let maybe_keys = self.pause_remove_callbacks.lock().take();
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
            Err(err) if err.code == Code::NotFound => {
                // The blob was evicted from the inner store. Remove the
                // stale entry from the existence cache so that subsequent
                // has() calls go to the inner store and get an accurate
                // result. Without this, CompletenessCheckingStore would
                // keep returning stale AC entries whose CAS blobs are gone.
                debug!(
                    ?digest,
                    "Blob not found in inner store, removing stale existence cache entry"
                );
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

    fn register_remove_callback(
        self: Arc<Self>,
        callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
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
