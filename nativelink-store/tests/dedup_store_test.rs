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
use std::sync::Arc;

use async_trait::async_trait;
use nativelink_config::stores::{
    CacheMetricsSpec, DedupSpec, FastSlowSpec, MemorySpec, StoreDirection, StoreSpec, VerifySpec,
};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_store::cache_metrics_store::CacheMetricsStore;
use nativelink_store::cas_utils::ZERO_BYTE_DIGESTS;
use nativelink_store::dedup_store::DedupStore;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::verify_store::VerifyStore;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    RemoveCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use pretty_assertions::assert_eq;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

fn make_default_config() -> DedupSpec {
    DedupSpec {
        index_store: StoreSpec::Memory(MemorySpec::default()),
        content_store: StoreSpec::Memory(MemorySpec::default()),
        min_size: 8 * 1024,
        normal_size: 32 * 1024,
        max_size: 128 * 1024,
        max_concurrent_fetch_per_get: 10,
    }
}

fn make_random_data(sz: usize) -> Vec<u8> {
    let mut value = vec![0u8; sz];
    let mut rng = SmallRng::seed_from_u64(1);
    rng.fill(&mut value[..]);
    value
}

const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";
const VALID_HASH2: &str = "0123456789abcdef000000000000000000020000000000000123456789abcdef";
const MEGABYTE_SZ: usize = 1024 * 1024;

#[nativelink_test]
async fn simple_round_trip_test() -> Result<(), Error> {
    let store = DedupStore::new(
        &make_default_config(),
        Store::new(MemoryStore::new(&MemorySpec::default())), // Index store.
        Store::new(MemoryStore::new(&MemorySpec::default())), // Content store.
    )?;

    let original_data = make_random_data(MEGABYTE_SZ);
    let digest = DigestInfo::try_new(VALID_HASH1, MEGABYTE_SZ).unwrap();

    store
        .update_oneshot(digest, original_data.clone().into())
        .await
        .err_tip(|| "Failed to write data to dedup store")?;

    let rt_data = store
        .get_part_unchunked(digest, 0, None)
        .await
        .err_tip(|| "Failed to get_part from dedup store")?;

    assert_eq!(rt_data, original_data, "Expected round trip data to match");
    Ok(())
}

#[nativelink_test]
async fn check_missing_last_chunk_test() -> Result<(), Error> {
    // This is the hash & size of the last chunk item in the content_store.
    const LAST_CHUNK_HASH: &str =
        "f6a29384357a77575b0a8cc79f731a4188d0155c00d5fb9a18becd92f6d1f074";
    const LAST_CHUNK_SIZE: usize = 10669;

    let content_store = MemoryStore::new(&MemorySpec::default());
    let store = DedupStore::new(
        &make_default_config(),
        Store::new(MemoryStore::new(&MemorySpec::default())), // Index store.
        Store::new(content_store.clone()),
    )?;

    let original_data = make_random_data(MEGABYTE_SZ);
    let digest = DigestInfo::try_new(VALID_HASH1, MEGABYTE_SZ).unwrap();

    store
        .update_oneshot(digest, original_data.into())
        .await
        .err_tip(|| "Failed to write data to dedup store")?;

    let did_delete = content_store
        .remove_entry(
            DigestInfo::try_new(LAST_CHUNK_HASH, LAST_CHUNK_SIZE)
                .unwrap()
                .into(),
        )
        .await;

    assert_eq!(did_delete, true, "Expected item to exist in store");

    let result = store.get_part_unchunked(digest, 0, None).await;
    assert!(result.is_err(), "Expected result to be an error");
    assert_eq!(
        result.unwrap_err().code,
        Code::NotFound,
        "Expected result to not be found"
    );
    Ok(())
}

/// Test to ensure if we upload a bit of data then request just a slice of it, we get the
/// proper data out. Internal to `DedupStore` we only download the slices that contain the
/// requested data; this test covers that use case.
#[nativelink_test]
async fn fetch_part_test() -> Result<(), Error> {
    const DATA_SIZE: usize = MEGABYTE_SZ / 4;
    const ONE_THIRD_SZ: usize = DATA_SIZE / 3;

    let store = DedupStore::new(
        &make_default_config(),
        Store::new(MemoryStore::new(&MemorySpec::default())), // Index store.
        Store::new(MemoryStore::new(&MemorySpec::default())), // Content store.
    )?;

    let original_data = make_random_data(DATA_SIZE);
    let digest = DigestInfo::try_new(VALID_HASH1, DATA_SIZE).unwrap();

    store
        .update_oneshot(digest, original_data.clone().into())
        .await
        .err_tip(|| "Failed to write data to dedup store")?;

    let rt_data = store
        .get_part_unchunked(digest, ONE_THIRD_SZ as u64, Some(ONE_THIRD_SZ as u64))
        .await
        .err_tip(|| "Failed to get_part from dedup store")?;

    assert_eq!(
        rt_data.len(),
        ONE_THIRD_SZ,
        "Expected round trip sizes to match"
    );
    assert_eq!(
        rt_data,
        original_data[ONE_THIRD_SZ..(ONE_THIRD_SZ * 2)],
        "Expected round trip data to match"
    );
    Ok(())
}

#[nativelink_test]
async fn check_length_not_set_with_chunk_read_beyond_first_chunk_regression_test()
-> Result<(), Error> {
    const DATA_SIZE: usize = 30;
    const START_READ_BYTE: usize = 7;

    let store = DedupStore::new(
        &DedupSpec {
            index_store: StoreSpec::Memory(MemorySpec::default()),
            content_store: StoreSpec::Memory(MemorySpec::default()),
            min_size: 5,
            normal_size: 6,
            max_size: 7,
            max_concurrent_fetch_per_get: 10,
        },
        Store::new(MemoryStore::new(&MemorySpec::default())), // Index store.
        Store::new(MemoryStore::new(&MemorySpec::default())), // Content store.
    )?;

    let original_data = make_random_data(DATA_SIZE);
    let digest = DigestInfo::try_new(VALID_HASH1, DATA_SIZE).unwrap();

    store
        .update_oneshot(digest, original_data.clone().into())
        .await
        .err_tip(|| "Failed to write data to dedup store")?;

    // This value must be larger than `max_size` in the config above.
    let rt_data = store
        .get_part_unchunked(digest, START_READ_BYTE as u64, None)
        .await
        .err_tip(|| "Failed to get_part from dedup store")?;

    assert_eq!(
        rt_data.len(),
        DATA_SIZE - START_READ_BYTE,
        "Expected round trip sizes to match"
    );
    assert_eq!(
        rt_data,
        original_data[START_READ_BYTE..],
        "Expected round trip data to match"
    );
    Ok(())
}

#[nativelink_test]
async fn check_chunk_boundary_reads_test() -> Result<(), Error> {
    const DATA_SIZE: usize = 30;
    const START_READ_BYTE: usize = 10;

    let store = DedupStore::new(
        &DedupSpec {
            index_store: StoreSpec::Memory(MemorySpec::default()),
            content_store: StoreSpec::Memory(MemorySpec::default()),
            min_size: 5,
            normal_size: 6,
            max_size: 7,
            max_concurrent_fetch_per_get: 10,
        },
        Store::new(MemoryStore::new(&MemorySpec::default())), // Index store.
        Store::new(MemoryStore::new(&MemorySpec::default())), // Content store.
    )?;

    let original_data = make_random_data(DATA_SIZE);
    let digest = DigestInfo::try_new(VALID_HASH1, DATA_SIZE).unwrap();
    store
        .update_oneshot(digest, original_data.clone().into())
        .await
        .err_tip(|| "Failed to write data to dedup store")?;

    for offset in 0..=DATA_SIZE {
        for len in 0..DATA_SIZE {
            // If reading at DATA_SIZE, we will set len to None to check that edge case.
            let maybe_len = if offset == DATA_SIZE {
                None
            } else {
                Some(len as u64)
            };
            let len = if maybe_len.is_none() { DATA_SIZE } else { len };

            let rt_data = store
                .get_part_unchunked(digest, offset as u64, maybe_len)
                .await
                .err_tip(|| "Failed to get_part from dedup store")?;

            let len_fenced = core::cmp::min(len, rt_data.len());
            assert_eq!(
                rt_data.len(),
                len_fenced,
                "Expected round trip sizes to match"
            );
            assert_eq!(
                rt_data,
                original_data[offset..(offset + len_fenced)],
                "Expected round trip data to match"
            );
        }
    }

    // This value must be larger than `max_size` in the config above.
    let rt_data = store
        .get_part_unchunked(digest, START_READ_BYTE as u64, None)
        .await
        .err_tip(|| "Failed to get_part from dedup store")?;

    assert_eq!(
        rt_data.len(),
        DATA_SIZE - START_READ_BYTE,
        "Expected round trip sizes to match"
    );
    assert_eq!(
        rt_data,
        original_data[START_READ_BYTE..],
        "Expected round trip data to match"
    );
    Ok(())
}

/// Ensure that when we run a `.has()` on a dedup store it will check to ensure all indexed
/// content items exist instead of just checking the entry in the index store.
#[nativelink_test]
async fn has_checks_content_store() -> Result<(), Error> {
    const DATA_SIZE: usize = MEGABYTE_SZ / 4;

    let index_store = MemoryStore::new(&MemorySpec::default());
    let content_store = MemoryStore::new(&MemorySpec {
        eviction_policy: Some(nativelink_config::stores::EvictionPolicy {
            max_bytes: DATA_SIZE + 1,
            ..Default::default()
        }),
    });

    let store = DedupStore::new(
        &make_default_config(),
        Store::new(index_store.clone()),
        Store::new(content_store.clone()),
    )?;

    let original_data = make_random_data(DATA_SIZE);
    let digest1 = DigestInfo::try_new(VALID_HASH1, DATA_SIZE).unwrap();

    store
        .update_oneshot(digest1, original_data.clone().into())
        .await
        .err_tip(|| "Failed to write data to dedup store")?;

    {
        // Check to ensure we our baseline `.has()` succeeds.
        let size_info = store.has(digest1).await.err_tip(|| "Failed to run .has")?;
        assert_eq!(size_info, Some(DATA_SIZE as u64), "Expected sizes to match");
    }
    {
        // We now add one more item to the store, which will trigger eviction of one of
        // the existing items because max_bytes will be exceeded.
        const DATA2: &str = "1234";
        let digest2 = DigestInfo::try_new(VALID_HASH2, DATA2.len()).unwrap();
        store
            .update_oneshot(digest2, DATA2.into())
            .await
            .err_tip(|| "Failed to write data to dedup store")?;

        {
            // Check our recently added entry is still valid.
            let size_info = store.has(digest2).await.err_tip(|| "Failed to run .has")?;
            assert_eq!(
                size_info,
                Some(DATA2.len() as u64),
                "Expected sizes to match"
            );
        }
        {
            // Check our first added entry is now invalid (because part of it was evicted).
            let size_info = store.has(digest1).await.err_tip(|| "Failed to run .has")?;
            assert_eq!(
                size_info, None,
                "Expected .has() to return None (not found)"
            );
        }
    }

    Ok(())
}

/// Ensure that when we run a `.has()` on a dedup store and the index does not exist it will
/// properly return None.
#[nativelink_test]
async fn has_with_no_existing_index_returns_none_test() -> Result<(), Error> {
    const DATA_SIZE: usize = 10;

    let index_store = MemoryStore::new(&MemorySpec::default());
    let content_store = MemoryStore::new(&MemorySpec {
        eviction_policy: Some(nativelink_config::stores::EvictionPolicy {
            max_count: 10,
            ..Default::default()
        }),
    });

    let store = DedupStore::new(
        &make_default_config(),
        Store::new(index_store.clone()),
        Store::new(content_store.clone()),
    )?;

    let digest = DigestInfo::try_new(VALID_HASH1, DATA_SIZE).unwrap();

    {
        let size_info = store.has(digest).await.err_tip(|| "Failed to run .has")?;
        assert_eq!(
            size_info, None,
            "Expected None to be returned, got {:?}",
            size_info
        );
    }
    Ok(())
}

/// Ensure that when we run a `.has()` on a dedup store to check for empty blobs it will
/// properly return Some(0).
#[nativelink_test]
async fn has_with_zero_digest_returns_some_test() -> Result<(), Error> {
    let index_store = MemoryStore::new(&MemorySpec::default());
    let content_store = MemoryStore::new(&MemorySpec {
        eviction_policy: Some(nativelink_config::stores::EvictionPolicy {
            max_count: 10,
            ..Default::default()
        }),
    });

    let store = DedupStore::new(
        &make_default_config(),
        Store::new(index_store.clone()),
        Store::new(content_store.clone()),
    )?;

    let digest = ZERO_BYTE_DIGESTS[0];

    {
        let size_info = store.has(digest).await.err_tip(|| "Failed to run .has")?;
        assert_eq!(
            size_info,
            Some(0),
            "Expected Sone(0) to be returned, got {:?}",
            size_info
        );
    }
    Ok(())
}

#[nativelink_test]
async fn incomplete_upload_does_not_publish_dedup_index() -> Result<(), Error> {
    use nativelink_util::buf_channel::make_buf_channel_pair;
    use nativelink_util::store_trait::UploadSizeInfo;

    let index = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = DedupStore::new(
        &make_default_config(),
        index.clone(),
        Store::new(MemoryStore::new(&MemorySpec::default())),
    )?;
    let key = DigestInfo::try_new(VALID_HASH1, 78)?;
    for payload in [bytes::Bytes::new(), bytes::Bytes::from_static(b"short")] {
        let (mut tx, rx) = make_buf_channel_pair();
        let send = async move {
            if !payload.is_empty() {
                tx.send(payload).await?;
            }
            tx.send_eof()
        };
        let (result, _) = tokio::join!(store.update(key, rx, UploadSizeInfo::ExactSize(78)), send);
        assert_eq!(result.unwrap_err().code, Code::InvalidArgument);
        assert_eq!(index.has(key).await?, None);
    }
    Ok(())
}

#[nativelink_test]
async fn empty_cas_upload_does_not_publish_dedup_index() -> Result<(), Error> {
    let index = MemoryStore::new(&MemorySpec::default());
    let content = MemoryStore::new(&MemorySpec::default());
    let store = DedupStore::new(
        &make_default_config(),
        Store::new(index.clone()),
        Store::new(content.clone()),
    )?;
    for digest in ZERO_BYTE_DIGESTS {
        store.update_oneshot(digest, bytes::Bytes::new()).await?;
    }
    assert_eq!(index.len_for_test(), 0);
    assert_eq!(content.len_for_test(), 0);
    Ok(())
}

/// The last chunk `make_random_data(MEGABYTE_SZ)` splits into under
/// `make_default_config()`.
const TAIL_CHUNK_HASH: &str = "f6a29384357a77575b0a8cc79f731a4188d0155c00d5fb9a18becd92f6d1f074";
const TAIL_CHUNK_SIZE: usize = 10669;

/// An index store that holds entries like a memory store but refuses every
/// removal.
#[derive(Debug, MetricsComponent)]
struct RemoveRefusingStore {
    inner: Store,
}

#[async_trait]
impl StoreDriver for RemoveRefusingStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn remove(self: Pin<&Self>, _key: StoreKey<'_>) -> Result<bool, Error> {
        Err(make_err!(
            Code::Unavailable,
            "index store refused the removal"
        ))
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        self.inner.has_with_results(keys, results).await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        self.inner.update(key, reader, size_info).await
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        self.inner.get_part(key, writer, offset, length).await
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

    fn register_remove_callback(self: Arc<Self>, _callback: RemoveCallback) -> Result<(), Error> {
        Ok(())
    }
}

default_health_status_indicator!(RemoveRefusingStore);

/// A size-verifying store over a fast tier that is a dedup store and a slow
/// tier that holds the durable copy, protected from repair removal as the
/// docs advise for a tier holding the only copy.
struct Composition {
    store: Store,
    index: Arc<MemoryStore>,
    content: Arc<MemoryStore>,
    durable: Arc<MemoryStore>,
}

fn make_composition() -> Result<Composition, Error> {
    let index = MemoryStore::new(&MemorySpec::default());
    let content = MemoryStore::new(&MemorySpec::default());
    let durable = MemoryStore::new(&MemorySpec::default());
    let dedup = DedupStore::new(
        &make_default_config(),
        Store::new(index.clone()),
        Store::new(content.clone()),
    )?;
    let protected = CacheMetricsStore::new(
        &CacheMetricsSpec {
            cache_type: "cas".to_string(),
            allow_remove: false,
            backend: StoreSpec::Memory(MemorySpec::default()),
        },
        Store::new(durable.clone()),
    );
    // The specs are read for their directions only.
    let fast_slow = FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
            bypass_dedup_threshold_bytes: 0,
        },
        Store::new(dedup),
        Store::new(protected),
    );
    let store = Store::new(VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash: false,
        },
        Store::new(fast_slow),
    ));
    Ok(Composition {
        store,
        index,
        content,
        durable,
    })
}

/// Removing a blob drops its index and nothing else: the chunks are keyed by
/// their own digests, and another blob built from them still reads.
#[nativelink_test]
async fn remove_drops_the_index_and_keeps_shared_chunks() -> Result<(), Error> {
    let index = MemoryStore::new(&MemorySpec::default());
    let content = MemoryStore::new(&MemorySpec::default());
    let store = DedupStore::new(
        &make_default_config(),
        Store::new(index.clone()),
        Store::new(content.clone()),
    )?;
    let data = make_random_data(MEGABYTE_SZ);
    // Two blobs made of the same chunks.
    let first = DigestInfo::try_new(VALID_HASH1, MEGABYTE_SZ)?;
    let second = DigestInfo::try_new(VALID_HASH2, MEGABYTE_SZ)?;
    store.update_oneshot(first, data.clone().into()).await?;
    store.update_oneshot(second, data.clone().into()).await?;
    let chunks = content.len_for_test();

    assert!(
        store
            .as_store_driver_pin()
            .remove(StoreKey::from(first))
            .await?
    );
    assert_eq!(index.has(first).await?, None);
    assert_eq!(store.has(first).await?, None);
    assert_eq!(
        store
            .get_part_unchunked(first, 0, None)
            .await
            .unwrap_err()
            .code,
        Code::NotFound
    );
    // `true` dropped an index, nothing more: every chunk is still held, and
    // the blob sharing them still reads.
    assert_eq!(content.len_for_test(), chunks);
    assert_eq!(store.get_part_unchunked(second, 0, None).await?, data);
    Ok(())
}

/// With no index under the key, or for the empty blob, which is never
/// indexed, nothing is removed.
#[nativelink_test]
async fn remove_of_a_missing_index_removes_nothing() -> Result<(), Error> {
    let store = DedupStore::new(
        &make_default_config(),
        Store::new(MemoryStore::new(&MemorySpec::default())),
        Store::new(MemoryStore::new(&MemorySpec::default())),
    )?;
    let missing = DigestInfo::try_new(VALID_HASH1, MEGABYTE_SZ)?;
    assert!(
        !store
            .as_store_driver_pin()
            .remove(StoreKey::from(missing))
            .await?
    );
    for zero in ZERO_BYTE_DIGESTS {
        assert!(
            !store
                .as_store_driver_pin()
                .remove(StoreKey::from(zero))
                .await?
        );
    }
    Ok(())
}

/// The index store's refusal reaches the caller unchanged, and the blob
/// stays readable.
#[nativelink_test]
async fn remove_reports_an_index_store_error() -> Result<(), Error> {
    let index = Arc::new(RemoveRefusingStore {
        inner: Store::new(MemoryStore::new(&MemorySpec::default())),
    });
    let store = DedupStore::new(
        &make_default_config(),
        Store::new(index),
        Store::new(MemoryStore::new(&MemorySpec::default())),
    )?;
    let data = make_random_data(MEGABYTE_SZ);
    let digest = DigestInfo::try_new(VALID_HASH1, MEGABYTE_SZ)?;
    store.update_oneshot(digest, data.clone().into()).await?;

    let err = store
        .as_store_driver_pin()
        .remove(StoreKey::from(digest))
        .await
        .unwrap_err();
    assert_eq!(err.code, Code::Unavailable, "{err:?}");
    assert!(
        err.to_string().contains("index store refused the removal"),
        "{err:?}"
    );
    assert_eq!(store.get_part_unchunked(digest, 0, None).await?, data);
    Ok(())
}

/// An index store behind a `cache_metrics` wrapper keeps its index unless
/// the wrapper allows removal.
#[nativelink_test]
async fn the_index_store_wrapper_decides_whether_its_index_is_removed() -> Result<(), Error> {
    for allow_remove in [false, true] {
        let index = CacheMetricsStore::new(
            &CacheMetricsSpec {
                cache_type: "cas".to_string(),
                allow_remove,
                backend: StoreSpec::Memory(MemorySpec::default()),
            },
            Store::new(MemoryStore::new(&MemorySpec::default())),
        );
        let store = DedupStore::new(
            &make_default_config(),
            Store::new(index),
            Store::new(MemoryStore::new(&MemorySpec::default())),
        )?;
        let data = make_random_data(MEGABYTE_SZ);
        let digest = DigestInfo::try_new(VALID_HASH1, MEGABYTE_SZ)?;
        store.update_oneshot(digest, data.clone().into()).await?;

        assert_eq!(
            store
                .as_store_driver_pin()
                .remove(StoreKey::from(digest))
                .await?,
            allow_remove
        );
        assert_eq!(
            store.has(digest).await?.is_some(),
            !allow_remove,
            "allow_remove: {allow_remove}"
        );
    }
    Ok(())
}

/// A cached index that names fewer bytes than its digest fails the read
/// with `DataLoss`, which drops the index; the next read refills it from the
/// durable copy, which the repair never touched.
#[nativelink_test]
async fn verify_repairs_a_truncated_cached_index_from_the_durable_copy() -> Result<(), Error> {
    let c = make_composition()?;
    let data = make_random_data(MEGABYTE_SZ);
    let digest = DigestInfo::try_new(VALID_HASH1, MEGABYTE_SZ)?;
    c.store.update_oneshot(digest, data.clone().into()).await?;

    // Index the cached copy to the first half of the blob: a well-formed
    // index that names fewer bytes than the digest.
    let truncating = DedupStore::new(
        &make_default_config(),
        Store::new(c.index.clone()),
        Store::new(c.content.clone()),
    )?;
    truncating
        .update_oneshot(digest, data[..MEGABYTE_SZ / 2].to_vec().into())
        .await?;

    let err = c
        .store
        .get_part_unchunked(digest, 0, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, Code::DataLoss, "{err:?}");
    assert_eq!(c.index.has(digest).await?, None, "the bad index is dropped");
    assert_eq!(
        c.durable.has(digest).await?,
        Some(MEGABYTE_SZ as u64),
        "the durable copy is untouched"
    );

    assert_eq!(c.store.get_part_unchunked(digest, 0, None).await?, data);
    assert!(
        c.index.has(digest).await?.is_some(),
        "the index is refilled"
    );
    assert_eq!(c.store.get_part_unchunked(digest, 0, None).await?, data);
    Ok(())
}

/// A cached chunk cut short under its own digest is not counted as present,
/// so the read fills from the durable copy and the fill replaces the chunk
/// instead of indexing the bad one again. Every read succeeds, and the
/// dedup tier then serves the blob on its own.
#[nativelink_test]
async fn a_corrupt_cached_chunk_is_replaced_by_the_next_fill() -> Result<(), Error> {
    let c = make_composition()?;
    let data = make_random_data(MEGABYTE_SZ);
    let digest = DigestInfo::try_new(VALID_HASH1, MEGABYTE_SZ)?;
    c.store.update_oneshot(digest, data.clone().into()).await?;

    let tail = DigestInfo::try_new(TAIL_CHUNK_HASH, TAIL_CHUNK_SIZE)?;
    assert_eq!(c.content.has(tail).await?, Some(TAIL_CHUNK_SIZE as u64));
    c.content
        .update_oneshot(tail, data[..100].to_vec().into())
        .await?;

    assert_eq!(c.store.get_part_unchunked(digest, 0, None).await?, data);
    assert_eq!(
        c.content.has(tail).await?,
        Some(TAIL_CHUNK_SIZE as u64),
        "the fill replaced the bad chunk"
    );
    assert_eq!(c.durable.has(digest).await?, Some(MEGABYTE_SZ as u64));

    // With the durable copy gone, the read can only come from the repaired
    // dedup tier.
    assert!(c.durable.remove_entry(StoreKey::from(digest)).await);
    assert_eq!(c.store.get_part_unchunked(digest, 0, None).await?, data);
    Ok(())
}

/// Without any store above it, an upload replaces a chunk held at the wrong
/// size, and until then the blob built from it is reported missing.
#[nativelink_test]
async fn update_replaces_a_chunk_stored_at_the_wrong_size() -> Result<(), Error> {
    let content = MemoryStore::new(&MemorySpec::default());
    let store = DedupStore::new(
        &make_default_config(),
        Store::new(MemoryStore::new(&MemorySpec::default())),
        Store::new(content.clone()),
    )?;
    let data = make_random_data(MEGABYTE_SZ);
    let digest = DigestInfo::try_new(VALID_HASH1, MEGABYTE_SZ)?;
    store.update_oneshot(digest, data.clone().into()).await?;

    let tail = DigestInfo::try_new(TAIL_CHUNK_HASH, TAIL_CHUNK_SIZE)?;
    content
        .update_oneshot(tail, data[..100].to_vec().into())
        .await?;
    assert_eq!(store.has(digest).await?, None);

    store.update_oneshot(digest, data.clone().into()).await?;
    assert_eq!(content.has(tail).await?, Some(TAIL_CHUNK_SIZE as u64));
    assert_eq!(store.has(digest).await?, Some(MEGABYTE_SZ as u64));
    assert_eq!(store.get_part_unchunked(digest, 0, None).await?, data);
    Ok(())
}
