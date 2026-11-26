// Copyright 2024 Trace Machina, Inc. All rights reserved.
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
use std::sync::Arc;

use nativelink_config::stores::{
    FastSlowSpec, MemorySpec, ShardSpec, StoreDirection, StoreSpec, VerifySpec,
};
use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::shard_store::ShardStore;
use nativelink_store::verify_store::VerifyStore;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use nativelink_util::store_trait::{Store, StoreDriver, StoreKey, StoreLike};
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

fn fast_slow(fast: Store, slow: Store) -> Arc<FastSlowStore> {
    FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
            bypass_dedup_threshold_bytes: 0,
            has_slow_write_wait_timeout_s: 0,
            leader_inactivity_timeout_s: 0,
        },
        fast,
        slow,
    )
}

fn verify(inner: Store) -> Arc<VerifyStore> {
    VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash: false,
        },
        inner,
    )
}

#[nativelink_test]
async fn repair_remove_routes_digest_and_string_keys_to_only_the_owner() -> Result<(), Error> {
    let (shard, stores) = make_stores(&[1, 2, 3]);
    let mut keys = Vec::new();
    for counter in 0u64..32 {
        let mut hasher = DigestHasherFunc::Blake3.hasher();
        hasher.update(&counter.to_le_bytes());
        keys.push(StoreKey::from(hasher.finalize_digest()));
    }
    for counter in 0..8 {
        keys.push(StoreKey::from(format!("repair-key-{counter}")));
    }
    for key in &keys {
        // Identify the owner using the real write path, not a second copy of the hash.
        shard.update_oneshot(key.borrow(), "owner".into()).await?;
        let mut owner = None;
        for (index, store) in stores.iter().enumerate() {
            if store.has(key.borrow()).await?.is_some() {
                assert!(owner.replace(index).is_none());
            }
        }
        let owner = owner.unwrap();
        // Also put the same key in other shards: a broadcast deletion must fail this test.
        for (index, store) in stores.iter().enumerate() {
            if index != owner {
                store
                    .update_oneshot(key.borrow(), "sentinel".into())
                    .await?;
            }
        }
        assert!(Pin::new(shard.as_ref()).remove(key.borrow()).await?);
        assert!(!Pin::new(shard.as_ref()).remove(key.borrow()).await?);
        for (index, store) in stores.iter().enumerate() {
            assert_eq!(
                store.has(key.borrow()).await?,
                if index == owner { None } else { Some(8) }
            );
            Pin::new(store.as_ref()).remove(key.borrow()).await?;
        }
    }
    Ok(())
}

#[nativelink_test]
async fn repair_cached_does_not_remove_a_bare_memory_shard() -> Result<(), Error> {
    let (shard, stores) = make_stores(&[1, 1]);
    let key = DigestInfo::try_new(STORE0_HASH, 4)?;
    shard.update_oneshot(key, "safe".into()).await?;
    assert!(!Pin::new(shard.as_ref()).remove_cached(key.into()).await?);
    assert_eq!(shard.get_part_unchunked(key, 0, None).await?, "safe");
    assert_eq!(
        stores
            .iter()
            .map(|store| store.len_for_test())
            .sum::<usize>(),
        1
    );
    Ok(())
}

#[nativelink_test]
async fn repair_truncated_sharded_fast_tier_refills_from_durable_copy() -> Result<(), Error> {
    let (shard, _) = make_stores(&[1, 1]);
    let durable = MemoryStore::new(&MemorySpec::default());
    let key = DigestInfo::try_new(STORE0_HASH, 4)?;
    shard.update_oneshot(key, "ba".into()).await?;
    durable.update_oneshot(key, "good".into()).await?;
    let checked = verify(Store::new(fast_slow(
        Store::new(shard.clone()),
        Store::new(durable.clone()),
    )));
    assert_eq!(
        checked
            .get_part_unchunked(key, 0, None)
            .await
            .unwrap_err()
            .code,
        Code::DataLoss
    );
    assert_eq!(durable.get_part_unchunked(key, 0, None).await?, "good");
    assert_eq!(checked.get_part_unchunked(key, 0, None).await?, "good");
    assert_eq!(shard.get_part_unchunked(key, 0, None).await?, "good");
    Ok(())
}

#[nativelink_test]
async fn repair_cached_reaches_fast_slow_inside_a_shard_and_keeps_durable_copy() -> Result<(), Error>
{
    let fast = MemoryStore::new(&MemorySpec::default());
    let durable = MemoryStore::new(&MemorySpec::default());
    let key = DigestInfo::try_new(STORE0_HASH, 4)?;
    fast.update_oneshot(key, "ba".into()).await?;
    durable.update_oneshot(key, "good".into()).await?;
    let shard = ShardStore::new(
        &ShardSpec {
            stores: vec![nativelink_config::stores::ShardConfig {
                store: StoreSpec::Memory(MemorySpec::default()),
                weight: Some(1),
            }],
        },
        vec![Store::new(fast_slow(
            Store::new(fast.clone()),
            Store::new(durable.clone()),
        ))],
    )?;
    assert!(Pin::new(shard.as_ref()).remove_cached(key.into()).await?);
    assert_eq!(fast.has(key).await?, None);
    assert_eq!(durable.get_part_unchunked(key, 0, None).await?, "good");
    assert!(!Pin::new(shard.as_ref()).remove_cached(key.into()).await?);
    // Re-seed the corrupt cache and exercise the verification caller as well.
    fast.update_oneshot(key, "ba".into()).await?;
    let checked = verify(Store::new(shard));
    assert_eq!(
        checked
            .get_part_unchunked(key, 0, None)
            .await
            .unwrap_err()
            .code,
        Code::DataLoss
    );
    assert_eq!(durable.get_part_unchunked(key, 0, None).await?, "good");
    assert_eq!(checked.get_part_unchunked(key, 0, None).await?, "good");
    assert_eq!(fast.get_part_unchunked(key, 0, None).await?, "good");
    Ok(())
}

#[nativelink_test]
async fn repair_preserves_backend_errors_and_the_operation() -> Result<(), Error> {
    use async_trait::async_trait;
    use nativelink_error::make_err;
    use nativelink_metric::MetricsComponent;
    use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
    use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
    use nativelink_util::store_trait::{RemoveCallback, UploadSizeInfo};

    #[derive(Debug, MetricsComponent)]
    struct FailingRepairStore {}

    #[async_trait]
    impl StoreDriver for FailingRepairStore {
        async fn post_init(self: Arc<Self>) -> Result<(), Error> {
            Ok(())
        }
        async fn remove(self: Pin<&Self>, _key: StoreKey<'_>) -> Result<bool, Error> {
            Err(make_err!(
                Code::PermissionDenied,
                "disposable removal refused"
            ))
        }
        async fn remove_cached(self: Pin<&Self>, _key: StoreKey<'_>) -> Result<bool, Error> {
            Err(make_err!(
                Code::Unavailable,
                "cache-only removal unavailable"
            ))
        }
        async fn has_with_results(
            self: Pin<&Self>,
            _keys: &[StoreKey<'_>],
            _results: &mut [Option<u64>],
        ) -> Result<(), Error> {
            panic!("repair must not probe or read the backend");
        }
        async fn update(
            self: Pin<&Self>,
            _key: StoreKey<'_>,
            _reader: DropCloserReadHalf,
            _size_info: UploadSizeInfo,
        ) -> Result<u64, Error> {
            panic!("repair must not update the backend");
        }
        async fn get_part(
            self: Pin<&Self>,
            _key: StoreKey<'_>,
            _writer: &mut DropCloserWriteHalf,
            _offset: u64,
            _length: Option<u64>,
        ) -> Result<(), Error> {
            panic!("repair must not read the backend");
        }
        fn inner_store(&self, _key: Option<StoreKey>) -> &dyn StoreDriver {
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
            _callback: RemoveCallback,
        ) -> Result<(), Error> {
            Ok(())
        }
    }
    default_health_status_indicator!(FailingRepairStore);
    let shard = ShardStore::new(
        &ShardSpec {
            stores: vec![nativelink_config::stores::ShardConfig {
                store: StoreSpec::Memory(MemorySpec::default()),
                weight: None,
            }],
        },
        vec![Store::new(Arc::new(FailingRepairStore {}))],
    )?;
    let key = DigestInfo::try_new(STORE0_HASH, 4)?;
    let disposable_error = Pin::new(shard.as_ref())
        .remove(key.into())
        .await
        .unwrap_err();
    assert_eq!(disposable_error.code, Code::PermissionDenied);
    assert!(
        disposable_error
            .messages
            .iter()
            .any(|message| message == "disposable removal refused")
    );
    let cached_error = Pin::new(shard.as_ref())
        .remove_cached(key.into())
        .await
        .unwrap_err();
    assert_eq!(cached_error.code, Code::Unavailable);
    assert!(
        cached_error
            .messages
            .iter()
            .any(|message| message == "cache-only removal unavailable")
    );
    Ok(())
}

#[nativelink_test]
async fn repair_cached_routes_to_only_the_owner_for_digest_and_string_keys() -> Result<(), Error> {
    let fast: Vec<_> = (0..3)
        .map(|_| MemoryStore::new(&MemorySpec::default()))
        .collect();
    let durable: Vec<_> = (0..3)
        .map(|_| MemoryStore::new(&MemorySpec::default()))
        .collect();
    let shard = ShardStore::new(
        &ShardSpec {
            stores: [1, 2, 3]
                .into_iter()
                .map(|weight| nativelink_config::stores::ShardConfig {
                    store: StoreSpec::Memory(MemorySpec::default()),
                    weight: Some(weight),
                })
                .collect(),
        },
        fast.iter()
            .zip(&durable)
            .map(|(fast, slow)| {
                Store::new(fast_slow(
                    Store::new(fast.clone()),
                    Store::new(slow.clone()),
                ))
            })
            .collect(),
    )?;
    let keys = [
        StoreKey::from(DigestInfo::try_new(STORE1_HASH, 4)?),
        StoreKey::from("cache-only-string"),
    ];
    for key in keys {
        shard.update_oneshot(key.borrow(), "good".into()).await?;
        let mut owner = None;
        for (index, store) in fast.iter().enumerate() {
            if store.has(key.borrow()).await?.is_some() {
                assert!(owner.replace(index).is_none());
            }
        }
        let owner = owner.unwrap();
        for (cache, backing) in fast.iter().zip(&durable) {
            cache.update_oneshot(key.borrow(), "good".into()).await?;
            backing.update_oneshot(key.borrow(), "good".into()).await?;
        }
        assert!(Pin::new(shard.as_ref()).remove_cached(key.borrow()).await?);
        assert!(!Pin::new(shard.as_ref()).remove_cached(key.borrow()).await?);
        for (index, (cache, backing)) in fast.iter().zip(&durable).enumerate() {
            assert_eq!(
                cache.has(key.borrow()).await?,
                if index == owner { None } else { Some(4) }
            );
            assert_eq!(
                backing.get_part_unchunked(key.borrow(), 0, None).await?,
                "good"
            );
        }
    }
    Ok(())
}
