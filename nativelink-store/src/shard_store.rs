// Copyright 2023 The NativeLink Authors. All rights reserved.
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

use std::ops::BitXor;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::{FuturesUnordered, TryStreamExt};
use nativelink_error::{error_if, Error, ResultExt};
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::metrics_utils::Registry;
use nativelink_util::store_trait::{Store, UploadSizeInfo};

pub struct ShardStore {
    // The weights will always be in ascending order a specific store is choosen based on the
    // the hash of the digest hash that is nearest-binary searched using the u32 as the index.
    weights_and_stores: Vec<(u32, Arc<dyn Store>)>,
}

impl ShardStore {
    pub fn new(config: &nativelink_config::stores::ShardStore, stores: Vec<Arc<dyn Store>>) -> Result<Self, Error> {
        error_if!(
            config.stores.len() != stores.len(),
            "Config shards do not match stores length"
        );
        error_if!(config.stores.is_empty(), "ShardStore must have at least one store");
        let total_weight: u64 = config
            .stores
            .iter()
            .map(|shard_config| shard_config.weight.unwrap_or(1) as u64)
            .sum();
        let mut weights: Vec<u32> = config
            .stores
            .iter()
            .map(|shard_config| (u32::MAX as u64 * shard_config.weight.unwrap_or(1) as u64 / total_weight) as u32)
            .scan(0, |state, weight| {
                *state += weight;
                Some(*state)
            })
            .collect();
        // Our last item should always be the max.
        *weights.last_mut().unwrap() = u32::MAX;
        Ok(Self {
            weights_and_stores: weights.into_iter().zip(stores).collect(),
        })
    }

    fn get_store_index(&self, digest: &DigestInfo) -> usize {
        // Quote from std primitive array documentation:
        //     Array’s try_from(slice) implementations (and the corresponding slice.try_into()
        //     array implementations) succeed if the input slice length is the same as the result
        //     array length. They optimize especially well when the optimizer can easily determine
        //     the slice length, e.g. <[u8; 4]>::try_from(&slice[4..8]).unwrap(). Array implements
        //     TryFrom returning.
        let size_bytes = digest.size_bytes.to_le_bytes();
        let key: u32 = 0
            .bitxor(u32::from_le_bytes(digest.packed_hash[0..4].try_into().unwrap()))
            .bitxor(u32::from_le_bytes(digest.packed_hash[4..8].try_into().unwrap()))
            .bitxor(u32::from_le_bytes(digest.packed_hash[8..12].try_into().unwrap()))
            .bitxor(u32::from_le_bytes(digest.packed_hash[12..16].try_into().unwrap()))
            .bitxor(u32::from_le_bytes(digest.packed_hash[16..20].try_into().unwrap()))
            .bitxor(u32::from_le_bytes(digest.packed_hash[20..24].try_into().unwrap()))
            .bitxor(u32::from_le_bytes(digest.packed_hash[24..28].try_into().unwrap()))
            .bitxor(u32::from_le_bytes(digest.packed_hash[28..32].try_into().unwrap()))
            .bitxor(u32::from_le_bytes(size_bytes[0..4].try_into().unwrap()))
            .bitxor(u32::from_le_bytes(size_bytes[4..8].try_into().unwrap()));
        self.weights_and_stores
            .binary_search_by_key(&key, |(weight, _)| *weight)
            .unwrap_or_else(|index| index)
    }

    fn get_store(&self, digest: &DigestInfo) -> Pin<&dyn Store> {
        let index = self.get_store_index(digest);
        Pin::new(self.weights_and_stores[index].1.as_ref())
    }
}

#[async_trait]
impl Store for ShardStore {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        if digests.len() == 1 {
            // Hot path: It is very common to lookup only one digest.
            let store_idx = self.get_store_index(&digests[0]);
            let store = Pin::new(self.weights_and_stores[store_idx].1.as_ref());
            return store
                .has_with_results(digests, results)
                .await
                .err_tip(|| "In ShardStore::has_with_results() for store {store_idx}}");
        }
        type DigestIdxVec = Vec<usize>;
        type DigestVec = Vec<DigestInfo>;
        let mut digests_for_store: Vec<(DigestIdxVec, DigestVec)> = self
            .weights_and_stores
            .iter()
            .map(|_| (Vec::new(), Vec::new()))
            .collect();
        // Bucket each digest into the store that it belongs to.
        digests
            .iter()
            .enumerate()
            .map(|(digest_idx, digest)| (digest, digest_idx, self.get_store_index(digest)))
            .for_each(|(digest, digest_idx, store_idx)| {
                digests_for_store[store_idx].0.push(digest_idx);
                digests_for_store[store_idx].1.push(*digest);
            });

        // Build all our futures for each store.
        let mut future_stream: FuturesUnordered<_> = digests_for_store
            .into_iter()
            .enumerate()
            .map(|(store_idx, (digest_idxs, digests))| async move {
                let store = Pin::new(self.weights_and_stores[store_idx].1.as_ref());
                let mut inner_results = vec![None; digests.len()];
                store
                    .has_with_results(&digests, &mut inner_results)
                    .await
                    .err_tip(|| "In ShardStore::has_with_results() for store {store_idx}")?;
                Result::<_, Error>::Ok((digest_idxs, inner_results))
            })
            .collect();

        // Wait for all the stores to finish and populate our output results.
        while let Some((digest_idxs, inner_results)) = future_stream.try_next().await? {
            for (digest_idx, inner_result) in digest_idxs.into_iter().zip(inner_results) {
                results[digest_idx] = inner_result;
            }
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        let store = self.get_store(&digest);
        store
            .update(digest, reader, size_info)
            .await
            .err_tip(|| "In ShardStore::update()")
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        let store = self.get_store(&digest);
        store
            .get_part_ref(digest, writer, offset, length)
            .await
            .err_tip(|| "In ShardStore::get_part_ref()")
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }

    fn inner_store(self: Arc<Self>, digest: Option<DigestInfo>) -> Arc<dyn Store> {
        let Some(digest) = digest else {
            return self;
        };
        let index = self.get_store_index(&digest);
        self.weights_and_stores[index].1.clone().inner_store(Some(digest))
    }

    fn register_metrics(self: Arc<Self>, registry: &mut Registry) {
        for (i, (_, store)) in self.weights_and_stores.iter().enumerate() {
            let store_registry = registry.sub_registry_with_prefix(format!("store_{i}"));
            store.clone().register_metrics(store_registry);
        }
    }
}
