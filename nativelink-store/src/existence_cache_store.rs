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
use nativelink_config::stores::{EvictionPolicy, ExistenceCacheSpec};
use nativelink_error::{Error, ResultExt, error_if};
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

#[derive(Debug, MetricsComponent)]
pub struct ExistenceCacheStore<I: InstantWrapper> {
    #[metric(group = "inner_store")]
    inner_store: Store,
    existence_cache: EvictingMap<DigestInfo, DigestInfo, ExistenceItem, I>,

    // We need to pause them temporarily when inserting into the inner store
    // as if it immediately expires them, we should only apply the remove callbacks
    // afterwards. If this is None, we're not pausing; if it's Some it's the location to
    // store them in temporarily
    pause_remove_callbacks: Arc<Mutex<Option<Vec<StoreKey<'static>>>>>,
}

impl ExistenceCacheStore<SystemTime> {
    pub fn new(spec: &ExistenceCacheSpec, inner_store: Store) -> Arc<Self> {
        Self::new_with_time(spec, inner_store, SystemTime::now())
    }
}

#[async_trait]
impl<I: InstantWrapper> RemoveItemCallback for ExistenceCacheStore<I> {
    async fn callback(&self, store_key: &StoreKey<'_>) {
        debug!(?store_key, "Removing item from cache due to callback");
        let new_key = store_key.borrow();
        let deleted_key = self.existence_cache.remove(&new_key.into_digest()).await;
        if !deleted_key {
            info!(?store_key, "Failed to delete key from cache on callback");
        }
    }
}

#[derive(Debug)]
struct ExistenceCacheCallback<I: InstantWrapper> {
    cache: Weak<ExistenceCacheStore<I>>,
}

#[async_trait]
impl<I: InstantWrapper> RemoveItemCallback for ExistenceCacheCallback<I> {
    async fn callback(&self, store_key: &StoreKey<'_>) {
        let cache = self.cache.upgrade();
        if let Some(local_cache) = cache {
            if let Some(callbacks) = &mut *local_cache.pause_remove_callbacks.lock_arc() {
                callbacks.push(store_key.borrow().into_owned());
            } else {
                local_cache.callback(store_key).await;
            }
        } else {
            debug!("Cache dropped, so not doing callback");
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
            pause_remove_callbacks: Arc::new(Mutex::new(None)),
        });
        let other_ref = Arc::downgrade(&existence_cache_store);
        existence_cache_store
            .inner_store
            .register_remove_callback(&Arc::new(Box::new(ExistenceCacheCallback {
                cache: other_ref,
            })))
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
        let mut exists = [None];
        self.inner_has_with_results(&[digest], &mut exists)
            .await
            .err_tip(|| "In ExistenceCacheStore::update")?;
        if exists[0].is_some() {
            // We need to drain the reader to avoid the writer complaining that we dropped
            // the connection prematurely.
            reader
                .drain()
                .await
                .err_tip(|| "In ExistenceCacheStore::update")?;
            return Ok(());
        }
        {
            let mut locked_callbacks = self.pause_remove_callbacks.lock_arc();
            if locked_callbacks.is_none() {
                locked_callbacks.replace(vec![]);
            }
        }
        trace!(?digest, "Inserting into inner cache");
        let result = self.inner_store.update(digest, reader, size_info).await;
        if result.is_ok() {
            trace!(?digest, "Inserting into existence cache");
            if let UploadSizeInfo::ExactSize(size) = size_info {
                let _ = self
                    .existence_cache
                    .insert(digest, ExistenceItem(size))
                    .await;
            }
        }
        {
            let mut locked_callbacks = self.pause_remove_callbacks.lock_arc();
            if let Some(callbacks) = locked_callbacks.take() {
                for store_key in callbacks {
                    self.callback(&store_key).await;
                }
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
        if result.is_ok() {
            let _ = self
                .existence_cache
                .insert(digest, ExistenceItem(digest.size_bytes()))
                .await;
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
        callback: &Arc<Box<dyn RemoveItemCallback>>,
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
