// Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

use std::pin::Pin;
use std::sync::Arc;

use common::DigestInfo;
use error::Error;
use memory_store::MemoryStore;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use shard_store::ShardStore;
use traits::StoreTrait;

const MEGABYTE_SZ: usize = 1024 * 1024;

fn make_stores(weights: &[u32]) -> (Arc<ShardStore>, Vec<Arc<dyn StoreTrait>>) {
    let memory_store_config = config::stores::MemoryStore::default();
    let store_config = config::stores::StoreConfig::memory(memory_store_config.clone());
    let stores: Vec<_> = weights
        .iter()
        .map(|_| -> Arc<dyn StoreTrait> { Arc::new(MemoryStore::new(&memory_store_config)) })
        .collect();
    let shard_store = Arc::new(
        ShardStore::new(
            &config::stores::ShardStore {
                stores: weights
                    .iter()
                    .map(|weight| config::stores::ShardConfig {
                        store: store_config.clone(),
                        weight: Some(*weight),
                    })
                    .collect(),
            },
            stores.clone(),
        )
        .unwrap(),
    );
    (shard_store, stores)
}

fn make_random_data(sz: usize) -> Vec<u8> {
    let mut value = vec![0u8; sz];
    let mut rng = SmallRng::seed_from_u64(1);
    rng.fill(&mut value[..]);
    value
}

#[cfg(test)]
mod shard_store_tests {
    use pretty_assertions::assert_eq;

    use super::*; // Must be declared in every module.

    const STORE0_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
    const STORE1_HASH: &str = "00000000EEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE";

    #[tokio::test]
    async fn has_with_one_digest() -> Result<(), Error> {
        let (shard_store, stores) = make_stores(&[1, 1]);
        let shard_store = Pin::new(shard_store.as_ref());
        let stores: Vec<_> = stores.iter().map(|store| Pin::new(store.as_ref())).collect();

        let original_data = make_random_data(MEGABYTE_SZ);
        let digest1 = DigestInfo::try_new(STORE0_HASH, 100).unwrap();
        stores[0].update_oneshot(digest1, original_data.clone().into()).await?;

        assert_eq!(shard_store.has(digest1).await, Ok(Some(MEGABYTE_SZ)));
        Ok(())
    }

    #[tokio::test]
    async fn has_with_many_digests_both_missing() -> Result<(), Error> {
        let (shard_store, _stores) = make_stores(&[1, 1]);
        let shard_store = Pin::new(shard_store.as_ref());

        let missing_digest1 = DigestInfo::try_new(STORE0_HASH, 100).unwrap();
        let missing_digest2 = DigestInfo::try_new(STORE1_HASH, 100).unwrap();

        assert_eq!(
            shard_store.has_many(&[missing_digest1, missing_digest2]).await,
            Ok(vec![None, None])
        );
        Ok(())
    }

    #[tokio::test]
    async fn has_with_many_digests_one_missing() -> Result<(), Error> {
        let (shard_store, stores) = make_stores(&[1, 1]);
        let shard_store = Pin::new(shard_store.as_ref());
        let stores: Vec<_> = stores.iter().map(|store| Pin::new(store.as_ref())).collect();

        let original_data = make_random_data(MEGABYTE_SZ);
        let digest1 = DigestInfo::try_new(STORE0_HASH, 100).unwrap();
        let missing_digest = DigestInfo::try_new(STORE1_HASH, 100).unwrap();
        stores[0].update_oneshot(digest1, original_data.clone().into()).await?;

        assert_eq!(
            shard_store.has_many(&[digest1, missing_digest]).await,
            Ok(vec![Some(MEGABYTE_SZ), None])
        );
        Ok(())
    }

    #[tokio::test]
    async fn has_with_many_digests_both_exist() -> Result<(), Error> {
        let (shard_store, stores) = make_stores(&[1, 1]);
        let shard_store = Pin::new(shard_store.as_ref());
        let stores: Vec<_> = stores.iter().map(|store| Pin::new(store.as_ref())).collect();

        let original_data1 = make_random_data(MEGABYTE_SZ);
        let original_data2 = make_random_data(2 * MEGABYTE_SZ);
        let digest1 = DigestInfo::try_new(STORE0_HASH, 100).unwrap();
        let digest2 = DigestInfo::try_new(STORE1_HASH, 100).unwrap();
        stores[0].update_oneshot(digest1, original_data1.clone().into()).await?;
        stores[1].update_oneshot(digest2, original_data2.clone().into()).await?;

        assert_eq!(
            shard_store.has_many(&[digest1, digest2]).await,
            Ok(vec![Some(original_data1.len()), Some(original_data2.len())])
        );
        Ok(())
    }

    #[tokio::test]
    async fn get_part_reads_store0() -> Result<(), Error> {
        let (shard_store, stores) = make_stores(&[1, 1]);
        let shard_store = Pin::new(shard_store.as_ref());
        let stores: Vec<_> = stores.iter().map(|store| Pin::new(store.as_ref())).collect();

        let original_data1 = make_random_data(MEGABYTE_SZ);
        let digest1 = DigestInfo::try_new(STORE0_HASH, 100).unwrap();
        stores[0].update_oneshot(digest1, original_data1.clone().into()).await?;

        assert_eq!(
            shard_store.get_part_unchunked(digest1, 0, None, None).await,
            Ok(original_data1.into())
        );
        Ok(())
    }

    #[tokio::test]
    async fn get_part_reads_store1() -> Result<(), Error> {
        let (shard_store, stores) = make_stores(&[1, 1]);
        let shard_store = Pin::new(shard_store.as_ref());
        let stores: Vec<_> = stores.iter().map(|store| Pin::new(store.as_ref())).collect();

        let original_data1 = make_random_data(MEGABYTE_SZ);
        let digest1 = DigestInfo::try_new(STORE1_HASH, 100).unwrap();
        stores[1].update_oneshot(digest1, original_data1.clone().into()).await?;

        assert_eq!(
            shard_store.get_part_unchunked(digest1, 0, None, None).await,
            Ok(original_data1.into())
        );
        Ok(())
    }

    #[tokio::test]
    async fn upload_store0() -> Result<(), Error> {
        let (shard_store, stores) = make_stores(&[1, 1]);
        let shard_store = Pin::new(shard_store.as_ref());
        let stores: Vec<_> = stores.iter().map(|store| Pin::new(store.as_ref())).collect();

        let original_data1 = make_random_data(MEGABYTE_SZ);
        let digest1 = DigestInfo::try_new(STORE0_HASH, 100).unwrap();
        shard_store
            .update_oneshot(digest1, original_data1.clone().into())
            .await?;

        assert_eq!(
            stores[0].get_part_unchunked(digest1, 0, None, None).await,
            Ok(original_data1.into())
        );
        Ok(())
    }

    #[tokio::test]
    async fn upload_store1() -> Result<(), Error> {
        let (shard_store, stores) = make_stores(&[1, 1]);
        let shard_store = Pin::new(shard_store.as_ref());
        let stores: Vec<_> = stores.iter().map(|store| Pin::new(store.as_ref())).collect();

        let original_data1 = make_random_data(MEGABYTE_SZ);
        let digest1 = DigestInfo::try_new(STORE1_HASH, 100).unwrap();
        shard_store
            .update_oneshot(digest1, original_data1.clone().into())
            .await?;

        assert_eq!(
            stores[1].get_part_unchunked(digest1, 0, None, None).await,
            Ok(original_data1.into())
        );
        Ok(())
    }

    #[tokio::test]
    async fn upload_download_has_check() -> Result<(), Error> {
        let (shard_store, _stores) = make_stores(&[1, 1]);
        let shard_store = Pin::new(shard_store.as_ref());

        let original_data1 = make_random_data(MEGABYTE_SZ);
        let digest1 = DigestInfo::try_new(STORE1_HASH, 100).unwrap();

        assert_eq!(shard_store.has(digest1).await, Ok(None));
        shard_store
            .update_oneshot(digest1, original_data1.clone().into())
            .await?;
        assert_eq!(
            shard_store.get_part_unchunked(digest1, 0, None, None).await,
            Ok(original_data1.into())
        );
        assert_eq!(shard_store.has(digest1).await, Ok(Some(MEGABYTE_SZ)));
        Ok(())
    }

    #[tokio::test]
    async fn weights_send_to_proper_store() -> Result<(), Error> {
        // Very low chance anything will ever go to second store due to weights being so much diff.
        let (shard_store, stores) = make_stores(&[100000, 1]);
        let shard_store = Pin::new(shard_store.as_ref());
        let stores: Vec<_> = stores.iter().map(|store| Pin::new(store.as_ref())).collect();

        let original_data1 = make_random_data(MEGABYTE_SZ);
        let digest1 = DigestInfo::try_new(STORE1_HASH, 100).unwrap();
        shard_store
            .update_oneshot(digest1, original_data1.clone().into())
            .await?;

        assert_eq!(stores[0].has(digest1).await, Ok(Some(MEGABYTE_SZ)));
        assert_eq!(stores[1].has(digest1).await, Ok(None));
        Ok(())
    }
}
