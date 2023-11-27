// Copyright 2023 The Native Link Authors. All rights reserved.
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

use std::mem::size_of;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use error::{error_if, Error, ResultExt};
use native_link_config::stores::{EvictionPolicy, ExistenceStore as ExistenceStoreConfig};
use native_link_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use native_link_util::common::DigestInfo;
use native_link_util::evicting_map::{EvictingMap, LenEntry};
use native_link_util::store_trait::{Store, UploadSizeInfo};

#[derive(Clone, Debug)]
struct ExistanceItem(usize);

impl LenEntry for ExistanceItem {
    #[inline]
    fn len(&self) -> usize {
        size_of::<Self>()
    }

    #[inline]
    fn is_empty(&self) -> bool {
        false
    }
}

pub struct ExistenceStore {
    inner_store: Arc<dyn Store>,
    existence_cache: EvictingMap<ExistanceItem, SystemTime>,
}

impl ExistenceStore {
    pub fn new(config: &ExistenceStoreConfig, inner_store: Arc<dyn Store>) -> Self {
        let empty_policy = EvictionPolicy::default();
        let eviction_policy = config.eviction_policy.as_ref().unwrap_or(&empty_policy);
        Self {
            inner_store,
            existence_cache: EvictingMap::new(eviction_policy, SystemTime::now()),
        }
    }

    pub async fn exists_in_cache(&self, digest: &DigestInfo) -> bool {
        self.existence_cache.size_for_key(digest).await.is_some()
    }

    pub async fn remove_from_cache(&self, digest: &DigestInfo) {
        self.existence_cache.remove(digest).await;
    }

    async fn inner_has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        self.existence_cache.sizes_for_keys(digests, results).await;

        let not_cached_digests: Vec<DigestInfo> = digests
            .iter()
            .zip(results.iter())
            .filter_map(|(digest, result)| result.map_or_else(|| Some(*digest), |_| None))
            .collect();

        // Hot path optimization when all digests are cached.
        if not_cached_digests.is_empty() {
            return Ok(());
        }

        // Now query only the items not found in the cache.
        let mut inner_results = vec![None; not_cached_digests.len()];
        self.pin_inner()
            .has_with_results(&not_cached_digests, &mut inner_results)
            .await
            .err_tip(|| "In ExistenceStore::inner_has_with_results")?;

        // Insert found from previous query into our cache.
        {
            // Note: Sadly due to some weird lifetime issues we need to collect here, but
            // in theory we don't actually need to collect.
            let inserts = not_cached_digests
                .iter()
                .zip(inner_results.iter())
                .filter_map(|(digest, result)| result.map(|size| (*digest, ExistanceItem(size))))
                .collect::<Vec<_>>();
            let _ = self.existence_cache.insert_many(inserts).await;
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

    fn pin_inner(&self) -> Pin<&dyn Store> {
        Pin::new(self.inner_store.as_ref())
    }
}

#[async_trait]
impl Store for ExistenceStore {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        self.inner_has_with_results(digests, results).await
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        mut reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        let mut exists = [None];
        self.inner_has_with_results(&[digest], &mut exists)
            .await
            .err_tip(|| "In ExistenceStore::update")?;
        if exists[0].is_some() {
            // We need to drain the reader to avoid the writer complaining that we dropped
            // the connection prematurely.
            reader.drain().await.err_tip(|| "In ExistenceStore::update")?;
            return Ok(());
        }
        let result = self.pin_inner().update(digest, reader, size_info).await;
        if result.is_ok() {
            if let UploadSizeInfo::ExactSize(size) = size_info {
                let _ = self.existence_cache.insert(digest, ExistanceItem(size)).await;
            }
        }
        result
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        let result = self.pin_inner().get_part_ref(digest, writer, offset, length).await;
        if result.is_ok() {
            let size = usize::try_from(digest.size_bytes)
                .err_tip(|| "Could not convert size_bytes in ExistenceStore::get_part")?;
            let _ = self.existence_cache.insert(digest, ExistanceItem(size)).await;
        }
        result
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
