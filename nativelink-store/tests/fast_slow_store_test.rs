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
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use futures::future::join_all;
use nativelink_config::stores::{FastSlowSpec, MemorySpec, NoopSpec, StoreDirection, StoreSpec};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::noop_store::NoopStore;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    RemoveItemCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use pretty_assertions::assert_eq;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

const MEGABYTE_SZ: usize = 1024 * 1024;

fn make_stores_direction(
    fast_direction: StoreDirection,
    slow_direction: StoreDirection,
) -> (Store, Store, Store) {
    let fast_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let slow_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let fast_slow_store = Store::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction,
            slow_direction,
        },
        fast_store.clone(),
        slow_store.clone(),
    ));
    (fast_slow_store, fast_store, slow_store)
}

fn make_stores() -> (Store, Store, Store) {
    make_stores_direction(StoreDirection::default(), StoreDirection::default())
}

fn make_random_data(sz: usize) -> Vec<u8> {
    let mut value = vec![0u8; sz];
    let mut rng = SmallRng::seed_from_u64(1);
    rng.fill(&mut value[..]);
    value
}

async fn check_data(
    check_store: &Store,
    digest: DigestInfo,
    original_data: &Vec<u8>,
    debug_name: &str,
) -> Result<(), Error> {
    assert!(
        check_store.has(digest).await?.is_some(),
        "Expected data to exist in {debug_name} store"
    );

    let store_data = check_store.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(
        store_data, original_data,
        "Expected data to match in {debug_name} store"
    );
    Ok(())
}

const VALID_HASH: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

#[nativelink_test]
async fn write_large_amount_to_both_stores_test() -> Result<(), Error> {
    let (store, fast_store, slow_store) = make_stores();

    let original_data = make_random_data(20 * MEGABYTE_SZ);
    let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
    store
        .update_oneshot(digest, original_data.clone().into())
        .await?;

    check_data(&store, digest, &original_data, "fast_slow").await?;
    check_data(&fast_store, digest, &original_data, "fast").await?;
    check_data(&slow_store, digest, &original_data, "slow").await?;

    Ok(())
}

#[nativelink_test]
async fn fetch_slow_store_puts_in_fast_store_test() -> Result<(), Error> {
    let (fast_slow_store, fast_store, slow_store) = make_stores();

    let original_data = make_random_data(MEGABYTE_SZ);
    let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
    slow_store
        .update_oneshot(digest, original_data.clone().into())
        .await?;

    assert_eq!(
        fast_slow_store.has(digest).await,
        Ok(Some(original_data.len() as u64))
    );
    assert_eq!(fast_store.has(digest).await, Ok(None));
    assert_eq!(
        slow_store.has(digest).await,
        Ok(Some(original_data.len() as u64))
    );

    // This get() request should place the data in fast_store too.
    fast_slow_store.get_part_unchunked(digest, 0, None).await?;

    // Now the data should exist in all the stores.
    check_data(&fast_store, digest, &original_data, "fast_store").await?;
    check_data(&slow_store, digest, &original_data, "slow_store").await?;

    Ok(())
}

#[nativelink_test]
async fn partial_reads_copy_full_to_fast_store_test() -> Result<(), Error> {
    let (fast_slow_store, fast_store, slow_store) = make_stores();

    let original_data = make_random_data(MEGABYTE_SZ);
    let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
    slow_store
        .update_oneshot(digest, original_data.clone().into())
        .await?;

    // This get() request should place the data in fast_store too.
    assert_eq!(
        original_data[10..60],
        fast_slow_store
            .get_part_unchunked(digest, 10, Some(50))
            .await?
    );

    // Full data should exist in the fast store even though only partially
    // read.
    check_data(&slow_store, digest, &original_data, "slow_store").await?;
    check_data(&fast_store, digest, &original_data, "fast_store").await?;

    Ok(())
}

#[test]
fn calculate_range_test() {
    let test =
        |start_range, end_range| FastSlowStore::calculate_range(&start_range, &end_range).unwrap();
    {
        // Exact match.
        let received_range = 0..1;
        let send_range = 0..1;
        let expected_results = Some(0..1);
        assert_eq!(test(received_range, send_range), expected_results);
    }
    {
        // Minus one on received_range.
        let received_range = 1..4;
        let send_range = 1..5;
        let expected_results = Some(0..3);
        assert_eq!(test(received_range, send_range), expected_results);
    }
    {
        // Minus one on send_range.
        let received_range = 1..5;
        let send_range = 1..4;
        let expected_results = Some(0..3);
        assert_eq!(test(received_range, send_range), expected_results);
    }
    {
        // Should have already sent all data (start fence post).
        let received_range = 1..2;
        let send_range = 0..1;
        let expected_results = None;
        assert_eq!(test(received_range, send_range), expected_results);
    }
    {
        // Definiltly already sent data.
        let received_range = 2..3;
        let send_range = 0..1;
        let expected_results = None;
        assert_eq!(test(received_range, send_range), expected_results);
    }
    {
        // All data should be sent (inside range).
        let received_range = 3..4;
        let send_range = 0..100;
        let expected_results = Some(0..1); // Note: This is relative received_range.start.
        assert_eq!(test(received_range, send_range), expected_results);
    }
    {
        // Subset of received data should be sent.
        let received_range = 1..100;
        let send_range = 3..4;
        let expected_results = Some(2..3); // Note: This is relative received_range.start.
        assert_eq!(test(received_range, send_range), expected_results);
    }
    {
        // We are clearly not at the offset yet.
        let received_range = 0..1;
        let send_range = 3..4;
        let expected_results = None;
        assert_eq!(test(received_range, send_range), expected_results);
    }
    {
        // Not at offset yet (fence post).
        let received_range = 0..1;
        let send_range = 1..2;
        let expected_results = None;
        assert_eq!(test(received_range, send_range), expected_results);
    }
    {
        // Head part of the received data should be sent.
        let received_range = 1..3;
        let send_range = 2..5;
        let expected_results = Some(1..2);
        assert_eq!(test(received_range, send_range), expected_results);
    }
}

#[nativelink_test]
async fn drop_on_eof_completes_store_futures() -> Result<(), Error> {
    #[derive(MetricsComponent)]
    struct DropCheckStore {
        drop_flag: Arc<AtomicBool>,
        read_rx: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        eof_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
        digest: Option<DigestInfo>,
    }

    #[async_trait]
    impl StoreDriver for DropCheckStore {
        async fn has_with_results(
            self: Pin<&Self>,
            digests: &[StoreKey<'_>],
            results: &mut [Option<u64>],
        ) -> Result<(), Error> {
            if let Some(has_digest) = self.digest {
                for (digest, result) in digests.iter().zip(results.iter_mut()) {
                    if *digest == has_digest.into() {
                        *result = Some(has_digest.size_bytes());
                    }
                }
            }
            Ok(())
        }

        async fn update(
            self: Pin<&Self>,
            _digest: StoreKey<'_>,
            mut reader: DropCloserReadHalf,
            _size_info: UploadSizeInfo,
        ) -> Result<(), Error> {
            // Gets called in the fast store and we don't need to do
            // anything.  Should only complete when drain has finished.
            reader.drain().await?;
            let eof_tx = self.eof_tx.lock().unwrap().take();
            if let Some(tx) = eof_tx {
                tx.send(())
                    .map_err(|e| make_err!(Code::Internal, "{:?}", e))?;
            }
            let read_rx = self.read_rx.lock().unwrap().take();
            if let Some(rx) = read_rx {
                rx.await.map_err(|e| make_err!(Code::Internal, "{:?}", e))?;
            }
            Ok(())
        }

        async fn get_part(
            self: Pin<&Self>,
            key: StoreKey<'_>,
            writer: &mut DropCloserWriteHalf,
            offset: u64,
            length: Option<u64>,
        ) -> Result<(), Error> {
            // Gets called in the slow store and we provide the data that's
            // sent to the upstream and the fast store.
            let bytes = length.unwrap_or_else(|| key.into_digest().size_bytes()) - offset;
            let data = vec![0_u8; usize::try_from(bytes).unwrap_or(usize::MAX)];
            writer.send(Bytes::copy_from_slice(&data)).await?;
            writer.send_eof()
        }

        fn inner_store(&self, _digest: Option<StoreKey>) -> &'_ dyn StoreDriver {
            self
        }

        fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
            self
        }

        fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
            self
        }

        fn register_remove_callback(
            self: Arc<Self>,
            _callback: Arc<dyn RemoveItemCallback>,
        ) -> Result<(), Error> {
            Ok(())
        }
    }

    impl Drop for DropCheckStore {
        fn drop(&mut self) {
            self.drop_flag.store(true, Ordering::Release);
        }
    }

    default_health_status_indicator!(DropCheckStore);

    let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
    let (fast_store_read_tx, fast_store_read_rx) = tokio::sync::oneshot::channel();
    let (fast_store_eof_tx, fast_store_eof_rx) = tokio::sync::oneshot::channel();
    let fast_store_dropped = Arc::new(AtomicBool::new(false));
    let fast_store = Store::new(Arc::new(DropCheckStore {
        drop_flag: fast_store_dropped.clone(),
        eof_tx: Mutex::new(Some(fast_store_eof_tx)),
        read_rx: Mutex::new(Some(fast_store_read_rx)),
        digest: None,
    }));
    let slow_store_dropped = Arc::new(AtomicBool::new(false));
    let slow_store = Store::new(Arc::new(DropCheckStore {
        drop_flag: slow_store_dropped,
        eof_tx: Mutex::new(None),
        read_rx: Mutex::new(None),
        digest: Some(digest),
    }));

    let fast_slow_store = FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
        },
        fast_store,
        slow_store,
    );

    let (tx, mut rx) = make_buf_channel_pair();
    let (get_res, read_res) = tokio::join!(
        async move {
            // Drop get_part as soon as rx.drain() completes
            tokio::select!(
                res = rx.drain() => res,
                res = fast_slow_store.get_part(digest, tx, 0, Some(digest.size_bytes())) => res,
            )
        },
        async move {
            fast_store_eof_rx
                .await
                .map_err(|e| make_err!(Code::Internal, "{:?}", e))?;
            // Give a couple of cycles for dropping to occur if it's going to.
            tokio::task::yield_now().await;
            tokio::task::yield_now().await;
            if fast_store_dropped.load(Ordering::Acquire) {
                return Err(make_err!(Code::Internal, "Fast store was dropped!"));
            }
            fast_store_read_tx
                .send(())
                .map_err(|e| make_err!(Code::Internal, "{:?}", e))?;
            Ok::<_, Error>(())
        }
    );
    get_res.merge(read_res)
}

// Previously `has()` returned `None` for a blob that was present only in the
// fast store. That behavior caused redundant work: a worker that had already
// cached a blob locally would still get NotFound from `has()` and re-fetch
// (or re-upload) the same data. `has_with_results` now falls back to the
// fast store when the slow store reports nothing, so a fast-only hit
// correctly returns the blob's size.
#[nativelink_test]
async fn fast_store_only_value_is_reported_by_has() -> Result<(), Error> {
    let fast_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let slow_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let fast_slow_store = Arc::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
        },
        fast_store.clone(),
        slow_store,
    ));
    let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
    fast_store
        .update_oneshot(digest, make_random_data(100).into())
        .await?;
    assert_eq!(
        fast_slow_store.has(digest).await?,
        Some(100),
        "Expected fast-store-only blob to be reported as present",
    );
    Ok(())
}

// Regression test for https://github.com/TraceMachina/nativelink/issues/665
#[nativelink_test]
async fn has_checks_fast_store_when_noop() -> Result<(), Error> {
    let fast_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let slow_store = Store::new(NoopStore::new());
    let fast_slow_store_config = FastSlowSpec {
        fast: StoreSpec::Memory(MemorySpec::default()),
        slow: StoreSpec::Noop(NoopSpec::default()),
        fast_direction: StoreDirection::default(),
        slow_direction: StoreDirection::default(),
    };
    let fast_slow_store = Arc::new(FastSlowStore::new(
        &fast_slow_store_config,
        fast_store.clone(),
        slow_store.clone(),
    ));

    let data = make_random_data(100);
    let digest = DigestInfo::try_new(VALID_HASH, data.len()).unwrap();

    assert_eq!(
        fast_slow_store.has(digest).await,
        Ok(None),
        "Expected data to not exist in store"
    );

    // Upload some dummy data.
    fast_store
        .update_oneshot(digest, data.clone().into())
        .await?;

    assert_eq!(
        fast_slow_store.has(digest).await,
        Ok(Some(data.len() as u64)),
        "Expected data to exist in store"
    );

    assert_eq!(
        fast_slow_store.get_part_unchunked(digest, 0, None).await,
        Ok(data.into()),
        "Data read from store is not correct"
    );
    Ok(())
}

#[nativelink_test]
async fn fast_get_only_not_updated() -> Result<(), Error> {
    let (fast_slow_store, fast_store, slow_store) =
        make_stores_direction(StoreDirection::Get, StoreDirection::Both);
    let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
    fast_slow_store
        .update_oneshot(digest, make_random_data(100).into())
        .await?;
    assert!(
        fast_store.has(digest).await?.is_none(),
        "Expected data to not be in the fast store"
    );
    assert!(
        slow_store.has(digest).await?.is_some(),
        "Expected data in the slow store"
    );
    Ok(())
}

#[nativelink_test]
async fn fast_readonly_only_not_updated() -> Result<(), Error> {
    let (fast_slow_store, fast_store, slow_store) =
        make_stores_direction(StoreDirection::ReadOnly, StoreDirection::Both);
    let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
    fast_slow_store
        .update_oneshot(digest, make_random_data(100).into())
        .await?;
    assert!(
        fast_store.has(digest).await?.is_none(),
        "Expected data to not be in the fast store"
    );
    assert!(
        slow_store.has(digest).await?.is_some(),
        "Expected data in the slow store"
    );
    Ok(())
}

#[nativelink_test]
async fn slow_readonly_only_not_updated() -> Result<(), Error> {
    let (fast_slow_store, fast_store, slow_store) =
        make_stores_direction(StoreDirection::Both, StoreDirection::ReadOnly);
    let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
    fast_slow_store
        .update_oneshot(digest, make_random_data(100).into())
        .await?;
    assert!(
        fast_store.has(digest).await?.is_some(),
        "Expected data to be in the fast store"
    );
    assert!(
        slow_store.has(digest).await?.is_none(),
        "Expected data to not be in the slow store"
    );
    Ok(())
}

#[nativelink_test]
async fn slow_get_only_not_updated() -> Result<(), Error> {
    let (fast_slow_store, fast_store, slow_store) =
        make_stores_direction(StoreDirection::Both, StoreDirection::Get);
    let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
    fast_slow_store
        .update_oneshot(digest, make_random_data(100).into())
        .await?;
    assert!(
        fast_store.has(digest).await?.is_some(),
        "Expected data to be in the fast store"
    );
    assert!(
        slow_store.has(digest).await?.is_none(),
        "Expected data to not be in the slow store"
    );
    Ok(())
}

#[nativelink_test]
async fn fast_put_only_not_updated() -> Result<(), Error> {
    let (fast_slow_store, fast_store, slow_store) =
        make_stores_direction(StoreDirection::Update, StoreDirection::Both);
    let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
    slow_store
        .update_oneshot(digest, make_random_data(100).into())
        .await?;
    fast_slow_store.get_part_unchunked(digest, 0, None).await?;
    assert!(
        fast_store.has(digest).await?.is_none(),
        "Expected data to not be in the fast store"
    );
    Ok(())
}

#[nativelink_test]
async fn fast_readonly_only_not_updated_on_get() -> Result<(), Error> {
    let (fast_slow_store, fast_store, slow_store) =
        make_stores_direction(StoreDirection::ReadOnly, StoreDirection::Both);
    let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
    slow_store
        .update_oneshot(digest, make_random_data(100).into())
        .await?;
    assert!(
        !fast_slow_store
            .get_part_unchunked(digest, 0, None)
            .await?
            .is_empty(),
        "Data not found in slow store"
    );
    assert!(
        fast_store.has(digest).await?.is_none(),
        "Expected data to not be in the fast store"
    );
    assert!(
        slow_store.has(digest).await?.is_some(),
        "Expected data in the slow store"
    );
    Ok(())
}

fn make_stores_with_lazy_slow() -> (Store, Store, Store) {
    #[derive(MetricsComponent)]
    struct LazyStore {
        inner: Arc<MemoryStore>,
    }

    #[async_trait]
    impl StoreDriver for LazyStore {
        async fn has_with_results(
            self: Pin<&Self>,
            digests: &[StoreKey<'_>],
            results: &mut [Option<u64>],
        ) -> Result<(), Error> {
            Pin::new(self.inner.as_ref())
                .has_with_results(digests, results)
                .await
        }

        async fn update(
            self: Pin<&Self>,
            digest: StoreKey<'_>,
            reader: DropCloserReadHalf,
            size_info: UploadSizeInfo,
        ) -> Result<(), Error> {
            Pin::new(self.inner.as_ref())
                .update(digest, reader, size_info)
                .await
        }

        async fn get_part(
            self: Pin<&Self>,
            key: StoreKey<'_>,
            writer: &mut DropCloserWriteHalf,
            offset: u64,
            length: Option<u64>,
        ) -> Result<(), Error> {
            Pin::new(self.inner.as_ref())
                .get_part(key, writer, offset, length)
                .await
        }

        fn optimized_for(
            &self,
            optimization: nativelink_util::store_trait::StoreOptimizations,
        ) -> bool {
            matches!(
                optimization,
                nativelink_util::store_trait::StoreOptimizations::LazyExistenceOnSync
            )
        }

        fn inner_store(&self, _digest: Option<StoreKey>) -> &'_ dyn StoreDriver {
            self
        }

        fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
            self
        }

        fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
            self
        }

        fn register_remove_callback(
            self: Arc<Self>,
            _callback: Arc<dyn RemoveItemCallback>,
        ) -> Result<(), Error> {
            Ok(())
        }
    }

    default_health_status_indicator!(LazyStore);

    let fast_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let slow_store = Store::new(Arc::new(LazyStore {
        inner: MemoryStore::new(&MemorySpec::default()),
    }));
    let fast_slow_store = Store::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
        },
        fast_store.clone(),
        slow_store.clone(),
    ));
    (fast_slow_store, fast_store, slow_store)
}

#[nativelink_test]
async fn lazy_not_found_returns_error_when_missing() -> Result<(), Error> {
    let (fast_slow_store, _fast_store, _slow_store) = make_stores_with_lazy_slow();
    let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();

    let result = fast_slow_store.get_part_unchunked(digest, 0, None).await;

    assert!(result.is_err(), "Expected error when key doesn't exist");
    assert_eq!(
        result.unwrap_err().code,
        Code::NotFound,
        "Expected NotFound error code"
    );
    Ok(())
}

#[nativelink_test]
async fn lazy_not_found_syncs_to_fast_store() -> Result<(), Error> {
    let (fast_slow_store, fast_store, slow_store) = make_stores_with_lazy_slow();
    let original_data = make_random_data(100);
    let digest = DigestInfo::try_new(VALID_HASH, original_data.len()).unwrap();

    slow_store
        .update_oneshot(digest, original_data.clone().into())
        .await?;

    assert!(
        fast_store.has(digest).await?.is_none(),
        "Expected data to not be in fast store initially"
    );

    let retrieved_data = fast_slow_store.get_part_unchunked(digest, 0, None).await?;

    assert_eq!(
        retrieved_data.as_ref(),
        original_data.as_slice(),
        "Retrieved data should match"
    );
    assert!(
        fast_store.has(digest).await?.is_some(),
        "Expected data to be synced to fast store"
    );
    Ok(())
}

#[derive(MetricsComponent)]
struct InstrumentedSlowStore {
    digest: DigestInfo,
    data: Vec<u8>,
    get_part_count: AtomicU64,
    /// If set, awaited at the start of `get_part` before any data flows.
    gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
}

#[async_trait]
impl StoreDriver for InstrumentedSlowStore {
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        for (key, result) in keys.iter().zip(results.iter_mut()) {
            if *key == self.digest.into() {
                *result = Some(self.digest.size_bytes());
            }
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        // Drain anything sent so the writer side does not deadlock.
        reader.drain().await
    }

    async fn get_part(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        _offset: u64,
        _length: Option<u64>,
    ) -> Result<(), Error> {
        self.get_part_count.fetch_add(1, Ordering::Acquire);
        // If a gate is configured, wait for the test to release it so the
        // populate can be held in flight long enough to exercise the
        // dedup paths.
        let gate = self.gate.lock().unwrap().take();
        if let Some(rx) = gate {
            let _ = rx.await;
        }
        writer.send(Bytes::copy_from_slice(&self.data)).await?;
        writer.send_eof()
    }

    fn inner_store(&self, _key: Option<StoreKey>) -> &'_ dyn StoreDriver {
        self
    }

    fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_remove_callback(
        self: Arc<Self>,
        _callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        Ok(())
    }
}

default_health_status_indicator!(InstrumentedSlowStore);

fn make_fast_slow_with_instrumented_slow(
    digest: DigestInfo,
    data: Vec<u8>,
    gate: Option<tokio::sync::oneshot::Receiver<()>>,
) -> (Store, Arc<InstrumentedSlowStore>) {
    let slow = Arc::new(InstrumentedSlowStore {
        digest,
        data,
        get_part_count: AtomicU64::new(0),
        gate: Mutex::new(gate),
    });
    let fast = Store::new(MemoryStore::new(&MemorySpec::default()));
    let fast_slow = Store::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
        },
        fast,
        Store::new(slow.clone()),
    ));
    (fast_slow, slow)
}

/// Many concurrent reads of the same digest must dedup down to a single
/// `slow_store.get_part` call.
#[nativelink_test]
async fn concurrent_reads_dedup_to_a_single_slow_store_call() -> Result<(), Error> {
    const N_CONCURRENT: usize = 16;
    let original_data = make_random_data(2048);
    let digest = DigestInfo::try_new(VALID_HASH, original_data.len()).unwrap();

    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel();
    let (fast_slow_store, slow) =
        make_fast_slow_with_instrumented_slow(digest, original_data.clone(), Some(gate_rx));

    let mut handles = Vec::with_capacity(N_CONCURRENT);
    for _ in 0..N_CONCURRENT {
        let store = fast_slow_store.clone();
        handles.push(tokio::spawn(async move {
            store.get_part_unchunked(digest, 0, None).await
        }));
    }

    // Give the spawned tasks a chance to register as followers on the
    // OnceCell before we let the leader's slow read complete.
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }

    gate_tx
        .send(())
        .map_err(|()| make_err!(Code::Internal, "Failed to release slow-store gate"))?;

    let results = join_all(handles).await;
    for r in results {
        let bytes = r
            .map_err(|e| make_err!(Code::Internal, "join error: {e:?}"))?
            .err_tip(|| "Concurrent get_part_unchunked failed")?;
        assert_eq!(
            bytes.as_ref(),
            original_data.as_slice(),
            "Every concurrent reader must observe the full, correct payload"
        );
    }

    let slow_calls = slow.get_part_count.load(Ordering::Acquire);
    assert_eq!(
        slow_calls, 1,
        "Expected the LoaderGuard dedup to collapse {N_CONCURRENT} concurrent reads to a single slow_store.get_part call, got {slow_calls}",
    );

    Ok(())
}

/// Dropping a follower's outer future must not cancel the leader's
/// populate.
#[nativelink_test]
async fn dropping_a_follower_does_not_cancel_the_leader() -> Result<(), Error> {
    let original_data = make_random_data(1024);
    let digest = DigestInfo::try_new(VALID_HASH, original_data.len()).unwrap();

    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel();
    let (fast_slow_store, slow) =
        make_fast_slow_with_instrumented_slow(digest, original_data.clone(), Some(gate_rx));

    let store_for_a = fast_slow_store.clone();
    let leader_handle =
        tokio::spawn(async move { store_for_a.get_part_unchunked(digest, 0, None).await });

    for _ in 0..16 {
        tokio::task::yield_now().await;
    }

    let store_for_b = fast_slow_store.clone();
    let b_result = tokio::time::timeout(
        Duration::from_millis(50),
        store_for_b.get_part_unchunked(digest, 0, None),
    )
    .await;
    assert!(
        b_result.is_err(),
        "Follower should still be waiting on the leader at this point",
    );

    gate_tx
        .send(())
        .map_err(|()| make_err!(Code::Internal, "Failed to release slow-store gate"))?;

    let leader_bytes = leader_handle
        .await
        .map_err(|e| make_err!(Code::Internal, "leader join error: {e:?}"))?
        .err_tip(|| "Leader's get_part_unchunked failed after follower drop")?;
    assert_eq!(
        leader_bytes.as_ref(),
        original_data.as_slice(),
        "Leader must observe the full, correct payload after a follower drop",
    );

    let slow_calls = slow.get_part_count.load(Ordering::Acquire);
    assert_eq!(
        slow_calls, 1,
        "Leader's populate must complete exactly once, got {slow_calls} slow_store.get_part calls",
    );

    Ok(())
}

/// While one writer's slow-store write is in flight, a concurrent `has()`
/// must report the blob as present so the second writer does not race and
/// re-upload the same data.
#[nativelink_test]
async fn has_sees_in_flight_slow_writes() -> Result<(), Error> {
    #[derive(MetricsComponent)]
    struct GatedSlowStore {
        /// Released by the test to let the in-flight slow write complete.
        gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
        /// Signalled once the slow-store `update` has begun draining.
        started_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    }

    #[async_trait]
    impl StoreDriver for GatedSlowStore {
        async fn has_with_results(
            self: Pin<&Self>,
            _keys: &[StoreKey<'_>],
            _results: &mut [Option<u64>],
        ) -> Result<(), Error> {
            // Slow store reports nothing — the in-flight tracking is what
            // should fill the result in.
            Ok(())
        }

        async fn update(
            self: Pin<&Self>,
            _key: StoreKey<'_>,
            mut reader: DropCloserReadHalf,
            _size_info: UploadSizeInfo,
        ) -> Result<(), Error> {
            let started_tx = self.started_tx.lock().unwrap().take();
            if let Some(tx) = started_tx {
                let _ = tx.send(());
            }
            let gate = self.gate.lock().unwrap().take();
            if let Some(rx) = gate {
                let _ = rx.await;
            }
            reader.drain().await
        }

        async fn get_part(
            self: Pin<&Self>,
            _key: StoreKey<'_>,
            writer: &mut DropCloserWriteHalf,
            _offset: u64,
            _length: Option<u64>,
        ) -> Result<(), Error> {
            writer.send_eof()
        }

        fn inner_store(&self, _key: Option<StoreKey>) -> &'_ dyn StoreDriver {
            self
        }

        fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
            self
        }

        fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
            self
        }

        fn register_remove_callback(
            self: Arc<Self>,
            _callback: Arc<dyn RemoveItemCallback>,
        ) -> Result<(), Error> {
            Ok(())
        }
    }

    default_health_status_indicator!(GatedSlowStore);

    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let slow = Arc::new(GatedSlowStore {
        gate: Mutex::new(Some(gate_rx)),
        started_tx: Mutex::new(Some(started_tx)),
    });
    let fast = Store::new(MemoryStore::new(&MemorySpec::default()));
    let fast_slow = Arc::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
        },
        fast,
        Store::new(slow.clone()),
    ));

    let data = make_random_data(256);
    let digest = DigestInfo::try_new(VALID_HASH, data.len()).unwrap();

    // Sanity: nothing in flight, slow store has nothing, fast store has
    // nothing -> NotFound.
    assert_eq!(
        fast_slow.has(digest).await?,
        None,
        "Pre-condition: blob should be absent before any writer starts",
    );

    let writer_store = fast_slow.clone();
    let writer_data = data.clone();
    let writer = tokio::spawn(async move {
        writer_store
            .update_oneshot(digest, writer_data.into())
            .await
    });

    // Wait until the slow store's update is actually being driven, which
    // proves the in-flight registration is live.
    started_rx
        .await
        .map_err(|e| make_err!(Code::Internal, "started signal lost: {e:?}"))?;

    // Concurrent observer: the slow store will return None (its
    // has_with_results above), so the only way this can be Some is via
    // the in-flight map.
    assert_eq!(
        fast_slow.has(digest).await?,
        Some(data.len() as u64),
        "Concurrent has() must see in-flight slow write",
    );

    // Release the writer and confirm the in-flight tracker is cleaned up.
    gate_tx
        .send(())
        .map_err(|()| make_err!(Code::Internal, "Failed to release slow-store gate"))?;
    writer
        .await
        .map_err(|e| make_err!(Code::Internal, "writer join error: {e:?}"))??;

    // After completion the fast store still has the blob, so has() should
    // remain Some via the fast-store fallback (the GatedSlowStore still
    // reports None).
    assert_eq!(
        fast_slow.has(digest).await?,
        Some(data.len() as u64),
        "Post-write has() should see the blob via fast-store fallback",
    );

    Ok(())
}

/// `has()` consults the fast store after the slow store reports `NotFound`.
/// This is asserted indirectly above by `fast_store_only_value_is_reported_by_has`;
/// here we additionally assert that when the slow store DOES have the blob,
/// the fast store is NOT consulted (avoiding the extra round trip).
#[nativelink_test]
async fn has_does_not_consult_fast_store_when_slow_store_hits() -> Result<(), Error> {
    #[derive(MetricsComponent)]
    struct CountingFastStore {
        inner: Arc<MemoryStore>,
        has_calls: Arc<AtomicU64>,
    }

    #[async_trait]
    impl StoreDriver for CountingFastStore {
        async fn has_with_results(
            self: Pin<&Self>,
            keys: &[StoreKey<'_>],
            results: &mut [Option<u64>],
        ) -> Result<(), Error> {
            self.has_calls.fetch_add(1, Ordering::Acquire);
            Pin::new(self.inner.as_ref())
                .has_with_results(keys, results)
                .await
        }

        async fn update(
            self: Pin<&Self>,
            key: StoreKey<'_>,
            reader: DropCloserReadHalf,
            size_info: UploadSizeInfo,
        ) -> Result<(), Error> {
            Pin::new(self.inner.as_ref())
                .update(key, reader, size_info)
                .await
        }

        async fn get_part(
            self: Pin<&Self>,
            key: StoreKey<'_>,
            writer: &mut DropCloserWriteHalf,
            offset: u64,
            length: Option<u64>,
        ) -> Result<(), Error> {
            Pin::new(self.inner.as_ref())
                .get_part(key, writer, offset, length)
                .await
        }

        fn inner_store(&self, _key: Option<StoreKey>) -> &'_ dyn StoreDriver {
            self
        }

        fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
            self
        }

        fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
            self
        }

        fn register_remove_callback(
            self: Arc<Self>,
            _callback: Arc<dyn RemoveItemCallback>,
        ) -> Result<(), Error> {
            Ok(())
        }
    }

    default_health_status_indicator!(CountingFastStore);

    let has_calls = Arc::new(AtomicU64::new(0));
    let fast_inner = MemoryStore::new(&MemorySpec::default());
    let fast = Store::new(Arc::new(CountingFastStore {
        inner: fast_inner,
        has_calls: has_calls.clone(),
    }));
    let slow = Store::new(MemoryStore::new(&MemorySpec::default()));
    let fast_slow = Arc::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
        },
        fast,
        slow.clone(),
    ));

    let data = make_random_data(128);
    let digest = DigestInfo::try_new(VALID_HASH, data.len()).unwrap();
    slow.update_oneshot(digest, data.clone().into()).await?;

    let before = has_calls.load(Ordering::Acquire);
    assert_eq!(
        fast_slow.has(digest).await?,
        Some(data.len() as u64),
        "Slow-store-only blob must be reported via slow lookup",
    );
    let after = has_calls.load(Ordering::Acquire);
    assert_eq!(
        after, before,
        "Fast store has() must not be consulted when the slow store already reports the blob",
    );

    Ok(())
}

// Helpers for the gated-slow-store tests below. A `GatedSlowStore2` that
// blocks `update()` on a oneshot gate and signals when it starts, with
// `has_with_results` always returning all-None so the in-flight map is the
// only thing that can satisfy a concurrent `has()`.
#[derive(MetricsComponent)]
struct GatedSlowStore2 {
    gate: Mutex<Option<tokio::sync::oneshot::Receiver<()>>>,
    started_tx: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}

#[async_trait]
impl StoreDriver for GatedSlowStore2 {
    async fn has_with_results(
        self: Pin<&Self>,
        _keys: &[StoreKey<'_>],
        _results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        let started_tx = self.started_tx.lock().unwrap().take();
        if let Some(tx) = started_tx {
            let _ = tx.send(());
        }
        let gate = self.gate.lock().unwrap().take();
        if let Some(rx) = gate {
            let _ = rx.await;
        }
        reader.drain().await
    }

    async fn get_part(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        _offset: u64,
        _length: Option<u64>,
    ) -> Result<(), Error> {
        writer.send_eof()
    }

    fn inner_store(&self, _key: Option<StoreKey>) -> &'_ dyn StoreDriver {
        self
    }
    fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
        self
    }
    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }
    fn register_remove_callback(
        self: Arc<Self>,
        _callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        Ok(())
    }
}

default_health_status_indicator!(GatedSlowStore2);

/// Cancel-safety: if the surrounding `update()` future is dropped before the
/// slow-store write completes, the `InFlightSlowWriteGuard` must remove the
/// key from `in_flight_slow_writes`. Otherwise the map would leak entries on
/// every cancelled upload and `has()` would falsely report cancelled blobs as
/// present forever.
///
/// This test fails against the pre-fix code (which had no guard at all) and
/// would also fail if the guard's `Drop` impl were removed/broken.
#[nativelink_test]
async fn dropping_update_future_cleans_up_in_flight_entry() -> Result<(), Error> {
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let slow = Arc::new(GatedSlowStore2 {
        gate: Mutex::new(Some(gate_rx)),
        started_tx: Mutex::new(Some(started_tx)),
    });
    // Use a fast store that holds nothing (NoopStore) AND set fast_direction
    // to ReadOnly so the update path is `ignore_fast` -> single slow call,
    // and so the fast-store fallback in `has` returns None. This isolates
    // the in-flight map as the only signal of presence during the upload.
    let fast = Store::new(NoopStore::new());
    let fast_slow = Arc::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Noop(NoopSpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::ReadOnly,
            slow_direction: StoreDirection::default(),
        },
        fast,
        Store::new(slow.clone()),
    ));

    let data = make_random_data(64);
    let digest = DigestInfo::try_new(VALID_HASH, data.len()).unwrap();

    let writer_store = fast_slow.clone();
    let writer_data = data.clone();
    let writer = tokio::spawn(async move {
        writer_store
            .update_oneshot(digest, writer_data.into())
            .await
    });

    // Confirm the writer is parked inside slow_store.update() — at this
    // point the in-flight guard is alive.
    started_rx
        .await
        .map_err(|e| make_err!(Code::Internal, "started signal lost: {e:?}"))?;
    assert_eq!(
        fast_slow.has(digest).await?,
        Some(data.len() as u64),
        "Pre-cancel: in-flight slow write must be visible via has()",
    );

    // Cancel the writer. The guard's Drop should remove the entry.
    writer.abort();
    // Awaiting a cancelled JoinHandle resolves; ignore the JoinError.
    drop(writer.await);

    // The gate is now stale (writer is gone) — drop the receiver explicitly
    // by releasing the sender to avoid any hang in unrelated code paths.
    drop(gate_tx);

    assert_eq!(
        fast_slow.has(digest).await?,
        None,
        "Post-cancel: in-flight entry must be removed; \
         has() must NOT see the cancelled upload",
    );

    Ok(())
}

// Two more valid 64-hex-char digests for the multi-key mixed test below.
const VALID_HASH_B: &str = "0123456789abcdef000000000000000000020000000000000123456789abcdef";
const VALID_HASH_C: &str = "0123456789abcdef000000000000000000030000000000000123456789abcdef";
const VALID_HASH_D: &str = "0123456789abcdef000000000000000000040000000000000123456789abcdef";

/// Wraps a `GatedSlowStore2` so `has_with_results` returns `Some` for any
/// digest pre-populated in `known` and `None` otherwise, while still
/// delegating writes through the gated inner so timing is controllable.
#[derive(MetricsComponent)]
struct MapBackedSlow {
    inner: Arc<GatedSlowStore2>,
    known: std::collections::HashMap<DigestInfo, u64>,
}
#[async_trait]
impl StoreDriver for MapBackedSlow {
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        for (k, r) in keys.iter().zip(results.iter_mut()) {
            if let StoreKey::Digest(d) = k
                && let Some(sz) = self.known.get(d)
            {
                *r = Some(*sz);
            }
        }
        Ok(())
    }
    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        // Delegate to the gated inner so timing is controllable.
        Pin::new(self.inner.as_ref())
            .update(key, reader, size_info)
            .await
    }
    async fn get_part(
        self: Pin<&Self>,
        _k: StoreKey<'_>,
        w: &mut DropCloserWriteHalf,
        _o: u64,
        _l: Option<u64>,
    ) -> Result<(), Error> {
        w.send_eof()
    }
    fn inner_store(&self, _k: Option<StoreKey>) -> &'_ dyn StoreDriver {
        self
    }
    fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
        self
    }
    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }
    fn register_remove_callback(
        self: Arc<Self>,
        _cb: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        Ok(())
    }
}
default_health_status_indicator!(MapBackedSlow);

/// `has_with_results` must independently classify each requested key. With
/// four keys — one only in the slow store, one with an in-flight slow write,
/// one only in the fast store, and one absent everywhere — the returned
/// slice must reflect each key's true state and not e.g. report the
/// in-flight size for unrelated keys.
#[nativelink_test]
async fn has_with_results_handles_mixed_key_sources() -> Result<(), Error> {
    let (gate_tx, gate_rx) = tokio::sync::oneshot::channel::<()>();
    let (started_tx, started_rx) = tokio::sync::oneshot::channel::<()>();
    let slow_inner = Arc::new(GatedSlowStore2 {
        gate: Mutex::new(Some(gate_rx)),
        started_tx: Mutex::new(Some(started_tx)),
    });

    // Populate the wrapper so `has_with_results` reports the slow-only key.
    let slow_only_size: u64 = 11;
    let in_flight_size: u64 = 22;
    let fast_only_size: u64 = 33;

    let slow_only_digest = DigestInfo::try_new(VALID_HASH, slow_only_size).unwrap();
    let in_flight_digest = DigestInfo::try_new(VALID_HASH_B, in_flight_size).unwrap();
    let fast_only_digest = DigestInfo::try_new(VALID_HASH_C, fast_only_size).unwrap();
    let missing_digest = DigestInfo::try_new(VALID_HASH_D, 44).unwrap();

    let mut known = std::collections::HashMap::new();
    known.insert(slow_only_digest, slow_only_size);
    let slow = Arc::new(MapBackedSlow {
        inner: slow_inner.clone(),
        known,
    });

    let fast = Store::new(MemoryStore::new(&MemorySpec::default()));
    let fast_slow = Arc::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::ReadOnly,
            slow_direction: StoreDirection::default(),
        },
        fast.clone(),
        Store::new(slow.clone()),
    ));

    // Seed the fast store with the fast-only blob directly.
    fast.update_oneshot(
        fast_only_digest,
        make_random_data(usize::try_from(fast_only_size).unwrap()).into(),
    )
    .await?;

    // Kick off the in-flight slow write and wait until it's parked.
    let writer_store = fast_slow.clone();
    let writer = tokio::spawn(async move {
        writer_store
            .update_oneshot(
                in_flight_digest,
                make_random_data(usize::try_from(in_flight_size).unwrap()).into(),
            )
            .await
    });
    started_rx
        .await
        .map_err(|e| make_err!(Code::Internal, "started signal lost: {e:?}"))?;

    // Now query all four keys in one call.
    let keys: [StoreKey<'static>; 4] = [
        StoreKey::Digest(slow_only_digest),
        StoreKey::Digest(in_flight_digest),
        StoreKey::Digest(fast_only_digest),
        StoreKey::Digest(missing_digest),
    ];
    let mut results: [Option<u64>; 4] = [None; 4];
    fast_slow
        .as_store_driver_pin()
        .has_with_results(&keys, &mut results)
        .await?;

    assert_eq!(results[0], Some(slow_only_size), "slow-only key");
    assert_eq!(results[1], Some(in_flight_size), "in-flight key");
    assert_eq!(results[2], Some(fast_only_size), "fast-only key");
    assert_eq!(results[3], None, "missing key must stay None");

    // Cleanup: release the gated writer.
    gate_tx
        .send(())
        .map_err(|()| make_err!(Code::Internal, "Failed to release slow-store gate"))?;
    writer
        .await
        .map_err(|e| make_err!(Code::Internal, "writer join error: {e:?}"))??;

    Ok(())
}
