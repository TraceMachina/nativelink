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

use std::sync::Arc;

use nativelink_config::stores::{
    CacheMetricsSpec, FastSlowSpec, MemorySpec, ShardSpec, StoreDirection, StoreSpec, VerifySpec,
};
use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_store::cache_metrics_store::CacheMetricsStore;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::shard_store::ShardStore;
use nativelink_store::verify_store::VerifyStore;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use nativelink_util::store_trait::{Store, StoreKey, StoreLike};
use pretty_assertions::assert_eq;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

const MEGABYTE_SZ: usize = 1024 * 1024;

fn make_stores(weights: &[u32]) -> (Arc<ShardStore>, Vec<Arc<MemoryStore>>) {
    let memory_store_config = MemorySpec::default();
    let store_config = StoreSpec::Memory(memory_store_config);
    let stores: Vec<_> = weights
        .iter()
        .map(|_| MemoryStore::new(&memory_store_config))
        .collect();

    let shard_store = ShardStore::new(
        &ShardSpec {
            stores: weights
                .iter()
                .map(|weight| nativelink_config::stores::ShardConfig {
                    store: store_config.clone(),
                    weight: Some(*weight),
                })
                .collect(),
        },
        stores
            .iter()
            .map(|store| Store::new(store.clone()))
            .collect(),
    )
    .unwrap();
    (shard_store, stores)
}

fn make_random_data(sz: usize) -> Vec<u8> {
    let mut value = vec![0u8; sz];
    let mut rng = SmallRng::seed_from_u64(1);
    rng.fill(&mut value[..]);
    value
}

async fn verify_weights(
    weights: &[u32],
    expected_hits: &[usize],
    rounds: u64,
    print_results: bool,
) -> Result<(), Error> {
    let (shard_store, stores) = make_stores(weights);
    let data = make_random_data(MEGABYTE_SZ);

    for counter in 0..rounds {
        let mut hasher = DigestHasherFunc::Blake3.hasher();
        hasher.update(&counter.to_le_bytes());
        let digest = hasher.finalize_digest();
        shard_store
            .update_oneshot(digest, data.clone().into())
            .await?;
    }

    for (index, (store, expected_hit)) in stores.iter().zip(expected_hits.iter()).enumerate() {
        let total_hits = store.len_for_test();
        #[expect(clippy::print_stdout, reason = "improves debugging")]
        if print_results {
            println!("expected_hit: {expected_hit} - total_hits: {total_hits}");
        } else {
            assert_eq!(
                *expected_hit, total_hits,
                "Index {index} failed with expected_hit: {expected_hit} != total_hits: {total_hits}"
            );
        }
    }
    Ok(())
}

const STORE0_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const STORE1_HASH: &str = "00000000EEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE";

#[nativelink_test]
async fn has_with_one_digest() -> Result<(), Error> {
    let (shard_store, stores) = make_stores(&[1, 1]);

    let original_data = make_random_data(MEGABYTE_SZ);
    let digest1 = DigestInfo::try_new(STORE0_HASH, 100).unwrap();
    stores[0]
        .update_oneshot(digest1, original_data.clone().into())
        .await?;

    assert_eq!(shard_store.has(digest1).await, Ok(Some(MEGABYTE_SZ as u64)));
    Ok(())
}

#[nativelink_test]
async fn has_with_many_digests_both_missing() -> Result<(), Error> {
    let (shard_store, _stores) = make_stores(&[1, 1]);

    let missing_digest1 = DigestInfo::try_new(STORE0_HASH, 100).unwrap();
    let missing_digest2 = DigestInfo::try_new(STORE1_HASH, 100).unwrap();

    assert_eq!(
        shard_store
            .has_many(&[missing_digest1.into(), missing_digest2.into()])
            .await,
        Ok(vec![None, None])
    );
    Ok(())
}

#[nativelink_test]
async fn has_with_many_digests_one_missing() -> Result<(), Error> {
    let (shard_store, stores) = make_stores(&[1, 1]);

    let original_data = make_random_data(MEGABYTE_SZ);
    let digest1 = DigestInfo::try_new(STORE0_HASH, 100).unwrap();
    let missing_digest = DigestInfo::try_new(STORE1_HASH, 100).unwrap();
    stores[0]
        .update_oneshot(digest1, original_data.clone().into())
        .await?;

    assert_eq!(
        shard_store
            .has_many(&[digest1.into(), missing_digest.into()])
            .await,
        Ok(vec![Some(MEGABYTE_SZ as u64), None])
    );
    Ok(())
}

#[nativelink_test]
async fn has_with_many_digests_both_exist() -> Result<(), Error> {
    let (shard_store, stores) = make_stores(&[1, 1]);

    let original_data1 = make_random_data(MEGABYTE_SZ);
    let original_data2 = make_random_data(2 * MEGABYTE_SZ);
    let digest1 = DigestInfo::try_new(STORE0_HASH, 100).unwrap();
    let digest2 = DigestInfo::try_new(STORE1_HASH, 100).unwrap();
    stores[0]
        .update_oneshot(digest1, original_data1.clone().into())
        .await?;
    stores[1]
        .update_oneshot(digest2, original_data2.clone().into())
        .await?;

    assert_eq!(
        shard_store
            .has_many(&[digest1.into(), digest2.into()])
            .await,
        Ok(vec![
            Some(original_data1.len() as u64),
            Some(original_data2.len() as u64)
        ])
    );
    Ok(())
}

#[nativelink_test]
async fn get_part_reads_store0() -> Result<(), Error> {
    let (shard_store, stores) = make_stores(&[1, 1]);

    let original_data1 = make_random_data(MEGABYTE_SZ);
    let digest1 = DigestInfo::try_new(STORE0_HASH, 100).unwrap();
    stores[0]
        .update_oneshot(digest1, original_data1.clone().into())
        .await?;

    assert_eq!(
        shard_store.get_part_unchunked(digest1, 0, None).await,
        Ok(original_data1.into())
    );
    Ok(())
}

#[nativelink_test]
async fn get_part_reads_store1() -> Result<(), Error> {
    let (shard_store, stores) = make_stores(&[1, 1]);

    let original_data1 = make_random_data(MEGABYTE_SZ);
    let digest1 = DigestInfo::try_new(STORE1_HASH, 100).unwrap();
    stores[1]
        .update_oneshot(digest1, original_data1.clone().into())
        .await?;

    assert_eq!(
        shard_store.get_part_unchunked(digest1, 0, None).await,
        Ok(original_data1.into())
    );
    Ok(())
}

#[nativelink_test]
async fn upload_store0() -> Result<(), Error> {
    let (shard_store, stores) = make_stores(&[1, 1]);

    let original_data1 = make_random_data(MEGABYTE_SZ);
    let digest1 = DigestInfo::try_new(STORE0_HASH, 100).unwrap();
    shard_store
        .update_oneshot(digest1, original_data1.clone().into())
        .await?;

    assert_eq!(
        stores[0].get_part_unchunked(digest1, 0, None).await,
        Ok(original_data1.into())
    );
    Ok(())
}

#[nativelink_test]
async fn upload_store1() -> Result<(), Error> {
    let (shard_store, stores) = make_stores(&[1, 1]);

    let original_data1 = make_random_data(MEGABYTE_SZ);
    let digest1 = DigestInfo::try_new(STORE1_HASH, 100).unwrap();
    shard_store
        .update_oneshot(digest1, original_data1.clone().into())
        .await?;

    assert_eq!(
        stores[1].get_part_unchunked(digest1, 0, None).await,
        Ok(original_data1.into())
    );
    Ok(())
}

#[nativelink_test]
async fn upload_download_has_check() -> Result<(), Error> {
    let (shard_store, _stores) = make_stores(&[1, 1]);

    let original_data1 = make_random_data(MEGABYTE_SZ);
    let digest1 = DigestInfo::try_new(STORE1_HASH, 100).unwrap();

    assert_eq!(shard_store.has(digest1).await, Ok(None));
    shard_store
        .update_oneshot(digest1, original_data1.clone().into())
        .await?;
    assert_eq!(
        shard_store.get_part_unchunked(digest1, 0, None).await,
        Ok(original_data1.into())
    );
    assert_eq!(shard_store.has(digest1).await, Ok(Some(MEGABYTE_SZ as u64)));
    Ok(())
}

#[nativelink_test]
async fn weights_send_to_proper_store() -> Result<(), Error> {
    // Very low chance anything will ever go to second store due to weights being so much diff.
    let (shard_store, stores) = make_stores(&[100_000, 1]);

    let original_data1 = make_random_data(MEGABYTE_SZ);
    let digest1 = DigestInfo::try_new(STORE1_HASH, 100).unwrap();
    shard_store
        .update_oneshot(digest1, original_data1.clone().into())
        .await?;

    assert_eq!(stores[0].has(digest1).await, Ok(Some(MEGABYTE_SZ as u64)));
    assert_eq!(stores[1].has(digest1).await, Ok(None));
    Ok(())
}

#[nativelink_test]
async fn verify_weights_even_weights() -> Result<(), Error> {
    verify_weights(
        &[1, 1, 1, 1, 1, 1],
        &[188, 168, 158, 175, 147, 164],
        1000,
        false,
    )
    .await
}

#[nativelink_test]
async fn verify_weights_mid_right_bias() -> Result<(), Error> {
    verify_weights(&[1, 1, 1, 100, 1, 1], &[5, 13, 12, 956, 4, 10], 1000, false).await
}

#[nativelink_test]
async fn verify_weights_mid_left_bias() -> Result<(), Error> {
    verify_weights(&[1, 1, 100, 1, 1, 1], &[5, 13, 962, 6, 4, 10], 1000, false).await
}

#[nativelink_test]
async fn verify_weights_left_bias() -> Result<(), Error> {
    verify_weights(&[100, 1, 1, 1, 1, 1], &[961, 11, 8, 6, 4, 10], 1000, false).await
}

#[nativelink_test]
async fn verify_weights_right_bias() -> Result<(), Error> {
    verify_weights(&[1, 1, 1, 1, 1, 100], &[5, 13, 12, 5, 11, 954], 1000, false).await
}

/// A removal reaches the one store the key is sharded to, for digest and
/// string keys alike, and leaves every other key where it is. Without it a
/// repair asked of a shard never reaches the tier that holds the bad copy.
#[nativelink_test]
async fn remove_reaches_the_store_the_key_is_sharded_to() -> Result<(), Error> {
    let (shard_store, stores) = make_stores(&[1, 1, 1]);
    let store = Store::new(shard_store);
    let keys: Vec<StoreKey<'static>> = (0u64..32)
        .map(|i| {
            let mut hasher = DigestHasherFunc::Blake3.hasher();
            hasher.update(&i.to_le_bytes());
            StoreKey::from(hasher.finalize_digest())
        })
        .chain((0..8).map(|i| StoreKey::from(format!("name-{i}"))))
        .collect();
    for key in &keys {
        store.update_oneshot(key.borrow(), "1234".into()).await?;
    }

    for (removed, key) in keys.iter().enumerate() {
        let mut holders = 0;
        for inner in &stores {
            if inner.has(key.borrow()).await?.is_some() {
                holders += 1;
            }
        }
        assert_eq!(holders, 1, "one shard holds {key:?}");

        assert!(store.as_store_driver_pin().remove(key.borrow()).await?);
        for inner in &stores {
            assert_eq!(inner.has(key.borrow()).await?, None);
        }
        // Nothing is left to remove.
        assert!(!store.as_store_driver_pin().remove(key.borrow()).await?);
        // Every key not yet removed is still where it was put.
        for kept in &keys[removed + 1..] {
            assert_eq!(store.has(kept.borrow()).await?, Some(4), "{kept:?}");
        }
    }
    Ok(())
}

/// The repair a failed read asks for, through a sharded cache tier: the
/// truncated cached copy is evicted from the shard that holds it, the
/// protected durable copy is kept, and the next read refills from it.
#[nativelink_test]
async fn a_truncated_copy_in_a_sharded_cache_is_evicted_and_refilled() -> Result<(), Error> {
    let (shard_store, shards) = make_stores(&[1, 1]);
    let durable = MemoryStore::new(&MemorySpec::default());
    let store = Store::new(VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash: false,
        },
        Store::new(FastSlowStore::new(
            &FastSlowSpec {
                fast: StoreSpec::Memory(MemorySpec::default()),
                slow: StoreSpec::Memory(MemorySpec::default()),
                fast_direction: StoreDirection::default(),
                slow_direction: StoreDirection::default(),
                bypass_dedup_threshold_bytes: 0,
            },
            Store::new(shard_store.clone()),
            // The durable copy refuses repair removal.
            Store::new(CacheMetricsStore::new(
                &CacheMetricsSpec {
                    cache_type: "cas_durable".to_string(),
                    allow_remove: false,
                    backend: StoreSpec::Memory(MemorySpec::default()),
                },
                Store::new(durable.clone()),
            )),
        )),
    ));
    let digest = DigestInfo::try_new(
        "0123456789abcdef000000000000000000010000000000000123456789abcdef",
        10,
    )?;
    durable.update_oneshot(digest, "0123456789".into()).await?;
    // Ten bytes claimed, four cached.
    shard_store.update_oneshot(digest, "1234".into()).await?;

    let err = store.get_part_unchunked(digest, 0, None).await.unwrap_err();
    assert_eq!(err.code, Code::DataLoss, "{err:?}");
    for shard in &shards {
        assert_eq!(shard.has(digest).await?, None, "the cached copy is evicted");
    }
    assert_eq!(
        durable.has(digest).await?,
        Some(10),
        "the durable copy is kept"
    );

    assert_eq!(
        store.get_part_unchunked(digest, 0, None).await?,
        "0123456789"
    );
    assert_eq!(
        shard_store.has(digest).await?,
        Some(10),
        "the read refilled the shard"
    );
    Ok(())
}
