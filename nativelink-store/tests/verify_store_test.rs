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

use futures::future::pending;
use futures::try_join;
use nativelink_config::stores::{MemorySpec, StoreSpec, VerifySpec};
use nativelink_error::{Code, Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::verify_store::VerifyStore;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasherFunc, make_ctx_for_hash_func};
use nativelink_util::spawn;
use nativelink_util::store_trait::{Store, StoreLike, UploadSizeInfo};
use opentelemetry::context::FutureExt;
use pretty_assertions::assert_eq;
use tracing::{Instrument, info_span};

const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

#[nativelink_test]
async fn verify_size_false_passes_on_update() -> Result<(), Error> {
    const VALUE1: &str = "123";

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: false,
            verify_hash: false,
        },
        Store::new(inner_store.clone()),
    );

    let digest = DigestInfo::try_new(VALID_HASH1, 100).unwrap();
    let result = store.update_oneshot(digest, VALUE1.into()).await;
    assert_eq!(
        result,
        Ok(()),
        "Should have succeeded when verify_size = false, got: {:?}",
        result
    );
    assert_eq!(
        inner_store.has(digest).await,
        Ok(Some(VALUE1.len() as u64)),
        "Expected data to exist in store after update"
    );
    Ok(())
}

#[nativelink_test]
async fn verify_size_true_fails_on_update() -> Result<(), Error> {
    const VALUE1: &str = "123";
    const EXPECTED_ERR: &str = "Expected size 100 but got size 3 on insert";

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash: false,
        },
        Store::new(inner_store.clone()),
    );

    let digest = DigestInfo::try_new(VALID_HASH1, 100).unwrap();
    let (mut tx, rx) = make_buf_channel_pair();
    let send_fut = async move {
        tx.send(VALUE1.into()).await?;
        tx.send_eof()
    };
    let result = try_join!(
        send_fut,
        store.update(digest, rx, UploadSizeInfo::ExactSize(100))
    );
    assert!(result.is_err(), "Expected error, got: {result:?}");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains(EXPECTED_ERR),
        "Error should contain '{EXPECTED_ERR}', got: {err:?}"
    );
    assert_eq!(
        inner_store.has(digest).await,
        Ok(None),
        "Expected data to not exist in store after update"
    );
    Ok(())
}

#[nativelink_test]
async fn verify_size_true_succeeds_on_update() -> Result<(), Error> {
    const VALUE1: &str = "123";

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash: false,
        },
        Store::new(inner_store.clone()),
    );

    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    let result = store.update_oneshot(digest, VALUE1.into()).await;
    assert_eq!(result, Ok(()), "Expected success, got: {:?}", result);
    assert_eq!(
        inner_store.has(digest).await,
        Ok(Some(VALUE1.len() as u64)),
        "Expected data to exist in store after update"
    );
    Ok(())
}

#[nativelink_test]
async fn verify_size_true_succeeds_on_multi_chunk_stream_update() -> Result<(), Error> {
    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash: false,
        },
        Store::new(inner_store.clone()),
    );

    let (mut tx, rx) = make_buf_channel_pair();

    let digest = DigestInfo::try_new(VALID_HASH1, 6).unwrap();
    let future = spawn!(
        "verify_size_true_succeeds_on_multi_chunk_stream_update",
        async move {
            Pin::new(&store)
                .update(digest, rx, UploadSizeInfo::ExactSize(6))
                .await
        },
    );
    tx.send("foo".into()).await?;
    tx.send("bar".into()).await?;
    tx.send_eof()?;
    let result = future.await.err_tip(|| "Failed to join spawn future")?;
    assert_eq!(result, Ok(6), "Expected success, got: {:?}", result);
    assert_eq!(
        inner_store.has(digest).await,
        Ok(Some(6)),
        "Expected data to exist in store after update"
    );
    Ok(())
}

#[nativelink_test]
async fn verify_sha256_hash_true_succeeds_on_update() -> Result<(), Error> {
    /// This value is sha256("123").
    const HASH: &str = "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3";
    const VALUE: &str = "123";

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: false,
            verify_hash: true,
        },
        Store::new(inner_store.clone()),
    );

    let digest = DigestInfo::try_new(HASH, 3).unwrap();
    let result = store
        .update_oneshot(digest, VALUE.into())
        .instrument(info_span!("update_oneshot"))
        .with_context(make_ctx_for_hash_func(DigestHasherFunc::Sha256)?)
        .await;
    assert_eq!(result, Ok(()), "Expected success, got: {:?}", result);
    assert_eq!(
        inner_store.has(digest).await,
        Ok(Some(VALUE.len() as u64)),
        "Expected data to exist in store after update"
    );
    Ok(())
}

#[nativelink_test]
async fn verify_sha256_hash_true_fails_on_update() -> Result<(), Error> {
    /// This value is sha256("12").
    const HASH: &str = "6b51d431df5d7f141cbececcf79edf3dd861c3b4069f0b11661a3eefacbba918";
    const VALUE: &str = "123";
    const ACTUAL_HASH: &str = "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3";

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: false,
            verify_hash: true,
        },
        Store::new(inner_store.clone()),
    );

    let digest = DigestInfo::try_new(HASH, 3).unwrap();
    let result = store
        .update_oneshot(digest, VALUE.into())
        .instrument(info_span!("update_oneshot"))
        .with_context(make_ctx_for_hash_func(DigestHasherFunc::Sha256)?)
        .await;
    let err = result.unwrap_err().to_string();
    let expected_err = format!(
        "Hash verification using SHA256 failed: client declared SHA256:{HASH}, but the server computed SHA256:{ACTUAL_HASH}"
    );
    assert!(
        err.contains(&expected_err),
        "Error should contain '{expected_err}', got: {err:?}"
    );
    assert_eq!(
        inner_store.has(digest).await,
        Ok(None),
        "Expected data to not exist in store after update"
    );
    Ok(())
}

#[nativelink_test]
async fn verify_blake3_hash_true_succeeds_on_update() -> Result<(), Error> {
    /// This value is blake3("123").
    const HASH: &str = "b3d4f8803f7e24b8f389b072e75477cdbcfbe074080fb5e500e53e26e054158e";
    const VALUE: &str = "123";

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: false,
            verify_hash: true,
        },
        Store::new(inner_store.clone()),
    );

    let digest = DigestInfo::try_new(HASH, 3).unwrap();

    let result = store
        .update_oneshot(digest, VALUE.into())
        .instrument(info_span!("update_oneshot"))
        .with_context(make_ctx_for_hash_func(DigestHasherFunc::Blake3)?)
        .await;

    assert_eq!(result, Ok(()), "Expected success, got: {:?}", result);
    assert_eq!(
        inner_store.has(digest).await,
        Ok(Some(VALUE.len() as u64)),
        "Expected data to exist in store after update"
    );
    Ok(())
}

#[nativelink_test]
async fn verify_blake3_hash_true_fails_on_update() -> Result<(), Error> {
    /// This value is blake3("12").
    const HASH: &str = "b944a0a3b20cf5927e594ff306d256d16cd5b0ba3e27b3285f40d7ef5e19695b";
    const VALUE: &str = "123";
    const ACTUAL_HASH: &str = "b3d4f8803f7e24b8f389b072e75477cdbcfbe074080fb5e500e53e26e054158e";

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: false,
            verify_hash: true,
        },
        Store::new(inner_store.clone()),
    );

    let digest = DigestInfo::try_new(HASH, 3).unwrap();

    let result = store
        .update_oneshot(digest, VALUE.into())
        .instrument(info_span!("update_oneshot"))
        .with_context(make_ctx_for_hash_func(DigestHasherFunc::Blake3)?)
        .await;

    // let result = store.update_oneshot(digest, VALUE.into()).await;
    let err = result.unwrap_err().to_string();
    let expected_err = format!(
        "Hash verification using BLAKE3 failed: client declared BLAKE3:{HASH}, but the server computed BLAKE3:{ACTUAL_HASH}"
    );
    assert!(
        err.contains(&expected_err),
        "Error should contain '{expected_err}', got: {err:?}"
    );
    assert_eq!(
        inner_store.has(digest).await,
        Ok(None),
        "Expected data to not exist in store after update"
    );
    Ok(())
}

// A potential bug could happen if the down stream component ignores the EOF but will
// stop receiving data when the expected size is reached. We should ensure this edge
// case is double protected.
#[nativelink_test]
async fn verify_fails_immediately_on_too_much_data_sent_update() -> Result<(), Error> {
    const VALUE: &str = "123";
    const EXPECTED_ERR: &str = "Expected size 4 but already received 6 on insert";

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash: false,
        },
        Store::new(inner_store.clone()),
    );

    let digest = DigestInfo::try_new(VALID_HASH1, 4).unwrap();
    let (mut tx, rx) = make_buf_channel_pair();
    let send_fut = async move {
        tx.send(VALUE.into()).await?;
        tx.send(VALUE.into()).await?;
        pending::<()>().await;
        panic!("Should not reach here");
        #[expect(unreachable_code, reason = "needed to avoid inference errors")]
        Ok(())
    };
    let result = try_join!(
        send_fut,
        store.update(digest, rx, UploadSizeInfo::ExactSize(4))
    );
    assert!(result.is_err(), "Expected error, got: {result:?}");
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains(EXPECTED_ERR),
        "Error should contain '{EXPECTED_ERR}', got: {err:?}"
    );
    assert_eq!(
        inner_store.has(digest).await,
        Ok(None),
        "Expected data to not exist in store after update"
    );
    Ok(())
}

#[nativelink_test]
async fn verify_size_and_hash_succeeds_on_small_data() -> Result<(), Error> {
    /// This value is sha256("123").
    const HASH: &str = "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3";
    const VALUE: &str = "123";

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash: true,
        },
        Store::new(inner_store.clone()),
    );

    let digest = DigestInfo::try_new(HASH, 3).unwrap();
    let result = store
        .update_oneshot(digest, VALUE.into())
        .instrument(info_span!("update_oneshot"))
        .with_context(make_ctx_for_hash_func(DigestHasherFunc::Sha256)?)
        .await;
    assert_eq!(result, Ok(()), "Expected success, got: {:?}", result);
    assert_eq!(
        inner_store.has(digest).await,
        Ok(Some(VALUE.len() as u64)),
        "Expected data to exist in store after update"
    );
    Ok(())
}

// The tier below holds fewer bytes than the digest names: the read fails
// instead of ending early, so nothing above caches a truncated blob.
#[nativelink_test]
async fn verify_size_true_fails_a_short_read() -> Result<(), Error> {
    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash: false,
        },
        Store::new(inner_store.clone()),
    );
    // Ten bytes claimed, four stored: the inner store does not check.
    let digest = DigestInfo::try_new(VALID_HASH1, 10).unwrap();
    inner_store.update_oneshot(digest, "1234".into()).await?;

    // A range read that stops before the end is judged against the range,
    // and passes while the bytes are there.
    let result = store.get_part_unchunked(digest, 1, Some(3)).await?;
    assert_eq!(result, "234");

    let result = store.get_part_unchunked(digest, 0, None).await;
    let err = result.unwrap_err();
    assert_eq!(err.code, Code::DataLoss, "{err:?}");
    assert!(
        err.to_string().contains("Read 4 bytes of the 10 expected"),
        "{err:?}"
    );
    // The failed read dropped the copy, so a range past its end now finds
    // nothing at all.
    let result = store.get_part_unchunked(digest, 1, Some(5)).await;
    assert_eq!(result.unwrap_err().code, Code::NotFound);
    Ok(())
}

#[nativelink_test]
async fn verify_size_true_passes_a_whole_read() -> Result<(), Error> {
    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash: false,
        },
        Store::new(inner_store.clone()),
    );
    let digest = DigestInfo::try_new(VALID_HASH1, 4).unwrap();
    inner_store.update_oneshot(digest, "1234".into()).await?;

    assert_eq!(store.get_part_unchunked(digest, 0, None).await?, "1234");
    assert_eq!(store.get_part_unchunked(digest, 2, None).await?, "34");
    assert_eq!(store.get_part_unchunked(digest, 1, Some(2)).await?, "23");
    // Past the end there is nothing to read and nothing expected.
    assert_eq!(store.get_part_unchunked(digest, 4, None).await?, "");
    Ok(())
}

/// The tier below sends part of the blob and then fails, as the Redis store
/// does for a key evicted between two chunks: the read fails with that
/// error and does not wait for bytes that will never come.
#[nativelink_test]
async fn verify_size_true_fails_when_the_inner_read_errors_after_some_data() -> Result<(), Error> {
    use core::pin::Pin;
    use std::sync::Arc;

    use async_trait::async_trait;
    use nativelink_error::{Code, make_err};
    use nativelink_metric::{
        MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
    };
    use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
    use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
    use nativelink_util::store_trait::{RemoveItemCallback, StoreDriver, StoreKey};

    #[derive(Debug)]
    struct PartialThenErrorStore;

    impl MetricsComponent for PartialThenErrorStore {
        fn publish(
            &self,
            _kind: MetricKind,
            _field_metadata: MetricFieldData,
        ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
            Ok(MetricPublishKnownKindData::Component)
        }
    }

    #[async_trait]
    impl StoreDriver for PartialThenErrorStore {
        async fn post_init(self: Arc<Self>) -> Result<(), Error> {
            Ok(())
        }
        async fn has_with_results(
            self: Pin<&Self>,
            _keys: &[StoreKey<'_>],
            results: &mut [Option<u64>],
        ) -> Result<(), Error> {
            results.fill(Some(10));
            Ok(())
        }
        async fn update(
            self: Pin<&Self>,
            _key: StoreKey<'_>,
            _reader: DropCloserReadHalf,
            _size_info: UploadSizeInfo,
        ) -> Result<u64, Error> {
            Ok(0)
        }
        async fn get_part(
            self: Pin<&Self>,
            _key: StoreKey<'_>,
            writer: &mut DropCloserWriteHalf,
            _offset: u64,
            _length: Option<u64>,
        ) -> Result<(), Error> {
            writer.send("1234".into()).await?;
            Err(make_err!(Code::NotFound, "key removed under the read"))
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
            _callback: Arc<dyn RemoveItemCallback>,
        ) -> Result<(), Error> {
            Ok(())
        }
    }
    default_health_status_indicator!(PartialThenErrorStore);

    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash: false,
        },
        Store::new(Arc::new(PartialThenErrorStore)),
    );
    let digest = DigestInfo::try_new(VALID_HASH1, 10).unwrap();
    let result = tokio::time::timeout(
        core::time::Duration::from_secs(5),
        store.get_part_unchunked(digest, 0, None),
    )
    .await
    .expect("a failed inner read must not hang the verify store");
    let err = result.unwrap_err();
    assert_eq!(err.code, Code::NotFound, "{err:?}");
    assert!(
        err.to_string().contains("key removed under the read"),
        "{err:?}"
    );
    Ok(())
}

/// The tier below serves more bytes than the digest names: the checker's
/// `DataLoss` is what the caller sees, not the inner store's failed send
/// once the checker stops reading.
#[nativelink_test]
async fn verify_size_true_reports_data_loss_on_an_over_long_read() -> Result<(), Error> {
    use core::pin::Pin;
    use std::sync::Arc;

    use async_trait::async_trait;
    use nativelink_error::Code;
    use nativelink_metric::{
        MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
    };
    use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
    use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
    use nativelink_util::store_trait::{RemoveItemCallback, StoreDriver, StoreKey};

    /// Serves forty bytes for every key, in four chunks, whatever the digest
    /// says.
    #[derive(Debug)]
    struct OverLongStore;

    impl MetricsComponent for OverLongStore {
        fn publish(
            &self,
            _kind: MetricKind,
            _field_metadata: MetricFieldData,
        ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
            Ok(MetricPublishKnownKindData::Component)
        }
    }

    #[async_trait]
    impl StoreDriver for OverLongStore {
        async fn post_init(self: Arc<Self>) -> Result<(), Error> {
            Ok(())
        }
        async fn has_with_results(
            self: Pin<&Self>,
            _keys: &[StoreKey<'_>],
            results: &mut [Option<u64>],
        ) -> Result<(), Error> {
            results.fill(Some(10));
            Ok(())
        }
        async fn update(
            self: Pin<&Self>,
            _key: StoreKey<'_>,
            _reader: DropCloserReadHalf,
            _size_info: UploadSizeInfo,
        ) -> Result<u64, Error> {
            Ok(0)
        }
        async fn get_part(
            self: Pin<&Self>,
            _key: StoreKey<'_>,
            writer: &mut DropCloserWriteHalf,
            _offset: u64,
            _length: Option<u64>,
        ) -> Result<(), Error> {
            for _ in 0..4 {
                writer.send("0123456789".into()).await?;
            }
            writer.send_eof()
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
            _callback: Arc<dyn RemoveItemCallback>,
        ) -> Result<(), Error> {
            Ok(())
        }
    }
    default_health_status_indicator!(OverLongStore);

    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash: false,
        },
        Store::new(Arc::new(OverLongStore)),
    );
    let digest = DigestInfo::try_new(VALID_HASH1, 10).unwrap();
    let err = tokio::time::timeout(
        core::time::Duration::from_secs(5),
        store.get_part_unchunked(digest, 0, None),
    )
    .await
    .expect("an over-long read must not hang")
    .unwrap_err();
    assert_eq!(err.code, Code::DataLoss, "{err:?}");
    assert!(
        err.to_string().contains("more than the 10 expected"),
        "{err:?}"
    );
    Ok(())
}

/// A copy that fails size verification is dropped from the tier that holds
/// it, so the next read repopulates from below instead of failing the same
/// way until the cache happens to evict it.
#[nativelink_test]
async fn verify_size_true_drops_a_copy_that_fails_verification() -> Result<(), Error> {
    use nativelink_error::Code;

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store = VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash: false,
        },
        Store::new(inner_store.clone()),
    );
    // Ten bytes claimed, four stored: a truncated copy.
    let digest = DigestInfo::try_new(VALID_HASH1, 10).unwrap();
    inner_store.update_oneshot(digest, "1234".into()).await?;
    assert!(inner_store.has(digest).await?.is_some());

    let err = store.get_part_unchunked(digest, 0, None).await.unwrap_err();
    assert_eq!(err.code, Code::DataLoss, "{err:?}");
    assert_eq!(
        inner_store.has(digest).await?,
        None,
        "the truncated copy is gone after the failed read"
    );
    // And the next read reports NotFound, the signal for a re-upload.
    let err = store.get_part_unchunked(digest, 0, None).await.unwrap_err();
    assert_eq!(err.code, Code::NotFound, "{err:?}");
    Ok(())
}
