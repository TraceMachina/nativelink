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
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::sync::Arc;

use async_trait::async_trait;
use mock_instant::thread_local::MockClock;
use nativelink_config::stores::{
    EvictionPolicy, ExistenceCacheSpec, FastSlowSpec, MemorySpec, NoopSpec, StoreDirection,
    StoreSpec, VerifySpec,
};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_store::existence_cache_store::ExistenceCacheStore;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::verify_store::VerifyStore;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::spawn;
use nativelink_util::store_trait::{
    RemoveCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use pretty_assertions::assert_eq;
use tokio::sync::Notify;

const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

#[nativelink_test]
async fn simple_exist_cache_test() -> Result<(), Error> {
    const VALUE: &str = "123";
    let spec = ExistenceCacheSpec {
        backend: StoreSpec::Noop(NoopSpec::default()), // Note: Not used.
        eviction_policy: Option::default(),
    };
    let inner_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = ExistenceCacheStore::new(&spec, inner_store.clone());

    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    store
        .update_oneshot(digest, VALUE.into())
        .await
        .err_tip(|| "Failed to update store")?;
    store.remove_from_cache(&digest).await;

    assert!(
        !store.exists_in_cache(&digest).await,
        "Expected digest to not exist in cache"
    );

    assert_eq!(
        store
            .has(digest)
            .await
            .err_tip(|| "Failed to check store")?,
        Some(VALUE.len() as u64),
        "Expected digest to exist in store"
    );

    assert!(
        store.exists_in_cache(&digest).await,
        "Expected digest to exist in cache in direct check"
    );
    Ok(())
}

#[nativelink_test]
async fn update_flags_existence_cache_test() -> Result<(), Error> {
    const VALUE: &str = "123";
    let spec = ExistenceCacheSpec {
        backend: StoreSpec::Noop(NoopSpec::default()),
        eviction_policy: Option::default(),
    };
    let inner_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = ExistenceCacheStore::new(&spec, inner_store.clone());

    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    store
        .update_oneshot(digest, VALUE.into())
        .await
        .err_tip(|| "Failed to update store")?;

    assert!(
        store.exists_in_cache(&digest).await,
        "Expected digest to exist in cache"
    );
    Ok(())
}

#[nativelink_test]
async fn get_part_caches_if_exact_size_set() -> Result<(), Error> {
    const VALUE: &str = "123";
    let spec = ExistenceCacheSpec {
        backend: StoreSpec::Noop(NoopSpec::default()),
        eviction_policy: Option::default(),
    };
    let inner_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    inner_store
        .update_oneshot(digest, VALUE.into())
        .await
        .err_tip(|| "Failed to update store")?;
    let store = ExistenceCacheStore::new(&spec, inner_store.clone());

    drop(
        store
            .get_part_unchunked(digest, 0, None)
            .await
            .err_tip(|| "Expected get_part to succeed")?,
    );

    assert!(
        store.exists_in_cache(&digest).await,
        "Expected digest to exist in cache"
    );
    Ok(())
}

// Regression test for: https://github.com/TraceMachina/nativelink/issues/1199.
#[nativelink_test]
async fn ensure_has_requests_do_let_evictions_happen() -> Result<(), Error> {
    const VALUE: &str = "123";
    let inner_store = MemoryStore::new(&MemorySpec::default());
    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    inner_store
        .update_oneshot(digest, VALUE.into())
        .await
        .err_tip(|| "Failed to update store")?;
    let store = ExistenceCacheStore::new_with_time(
        &ExistenceCacheSpec {
            backend: StoreSpec::Noop(NoopSpec::default()),
            eviction_policy: Some(EvictionPolicy {
                max_seconds: 0, // Explicitly set this level to "don't timeout"
                ..Default::default()
            }),
        },
        Store::new(inner_store.clone()),
        MockInstantWrapped::default(),
    );

    assert_eq!(store.has(digest).await, Ok(Some(VALUE.len() as u64)));
    MockClock::advance(Duration::from_secs(3));

    // Now that our existence cache has been populated, remove
    // it from the inner store.
    inner_store.remove_entry(digest.into()).await;

    // It should be immediately evicted from the existence cache.
    assert_eq!(store.has(digest).await, Ok(None));

    Ok(())
}

#[nativelink_test]
async fn copes_with_dropped_items() -> Result<(), Error> {
    const VALUE: &str = "123";
    let spec = ExistenceCacheSpec {
        backend: StoreSpec::Noop(NoopSpec::default()), // Note: Not used.
        eviction_policy: Option::default(),
    };
    let inner_store = Store::new(MemoryStore::new(&MemorySpec {
        eviction_policy: Some(EvictionPolicy {
            max_bytes: 1,
            ..Default::default()
        }),
    }));
    let store = ExistenceCacheStore::new(&spec, inner_store.clone());

    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    store
        .update_oneshot(digest, VALUE.into())
        .await
        .err_tip(|| "Failed to update store")?;

    let inner_store_item = inner_store.has(digest).await;
    assert!(
        inner_store_item.is_ok(),
        "Failed inner item: {inner_store_item:#?}",
    );
    let unwrapped_inner = inner_store_item.unwrap();
    assert!(
        unwrapped_inner.is_none(),
        "Failed inner item: {unwrapped_inner:#?}"
    );

    let store_item = store.has(digest).await;
    assert!(store_item.is_ok(), "Failed item: {store_item:#?}");
    let unwrapped_store = store_item.unwrap();
    assert!(
        unwrapped_store.is_none(),
        "Failed item: {unwrapped_store:#?}"
    );

    Ok(())
}

/// A store that loses entries without telling anyone: Redis under
/// allkeys-lru whose keyspace events never reach the subscriber, which is
/// what the September 2026 capture showed. The remove callback is accepted
/// and never called.
#[derive(Debug, MetricsComponent)]
struct SilentlyEvictingStore {
    inner: Arc<MemoryStore>,
}

impl SilentlyEvictingStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(&MemorySpec::default()),
        })
    }

    /// The eviction nobody hears about.
    async fn lose(&self, digest: DigestInfo) {
        assert!(self.inner.remove_entry(digest.into()).await);
    }
}

#[async_trait]
impl StoreDriver for SilentlyEvictingStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        Pin::new(self.inner.as_ref())
            .has_with_results(keys, results)
            .await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
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

    fn register_remove_callback(self: Arc<Self>, _callback: RemoveCallback) -> Result<(), Error> {
        Ok(())
    }
}

default_health_status_indicator!(SilentlyEvictingStore);

/// The September failure. A blob the cache has seen is lost by the tier under
/// it, and the cache is not told. Within its TTL the cache keeps saying
/// present, which is the accepted window; what must never happen is the
/// upload of a client that sends the blob anyway being thrown away, because
/// that turns a late answer into a blob nothing holds.
#[nativelink_test]
async fn an_upload_for_a_blob_the_cache_believes_present_still_lands() -> Result<(), Error> {
    const VALUE: &str = "123";
    let lossy = SilentlyEvictingStore::new();
    let store = ExistenceCacheStore::new(
        &ExistenceCacheSpec {
            backend: StoreSpec::Noop(NoopSpec::default()),
            eviction_policy: Some(EvictionPolicy {
                max_seconds: 1800,
                ..Default::default()
            }),
        },
        Store::new(lossy.clone()),
    );
    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    store.update_oneshot(digest, VALUE.into()).await?;
    assert!(store.exists_in_cache(&digest).await);

    lossy.lose(digest).await;
    assert_eq!(
        lossy.inner.has(digest).await?,
        None,
        "the tier lost the blob"
    );
    assert_eq!(
        store.has(digest).await?,
        Some(VALUE.len() as u64),
        "within the TTL the cache still answers present; that is the accepted window"
    );

    // A client uploads it anyway: another pod said missing, or a retry.
    store.update_oneshot(digest, VALUE.into()).await?;
    assert_eq!(
        lossy.inner.has(digest).await?,
        Some(VALUE.len() as u64),
        "the upload was discarded because the cache believed the blob present"
    );
    assert_eq!(
        store.get_part_unchunked(digest, 0, None).await?,
        VALUE.as_bytes(),
        "the blob is not readable after its re-upload"
    );
    Ok(())
}

/// The TTL, not a callback, is what bounds a stale positive: once an entry is
/// older than `max_seconds` the cache asks the inner store again.
#[nativelink_test]
async fn an_aged_positive_asks_the_inner_store_again() -> Result<(), Error> {
    const VALUE: &str = "123";
    let lossy = SilentlyEvictingStore::new();
    let store = ExistenceCacheStore::new_with_time(
        &ExistenceCacheSpec {
            backend: StoreSpec::Noop(NoopSpec::default()),
            eviction_policy: Some(EvictionPolicy {
                max_seconds: 600,
                ..Default::default()
            }),
        },
        Store::new(lossy.clone()),
        MockInstantWrapped::default(),
    );
    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    store.update_oneshot(digest, VALUE.into()).await?;
    lossy.lose(digest).await;

    MockClock::advance(Duration::from_secs(599));
    assert_eq!(store.has(digest).await?, Some(VALUE.len() as u64));
    MockClock::advance(Duration::from_secs(2));
    assert_eq!(
        store.has(digest).await?,
        None,
        "a positive older than max_seconds must not be trusted"
    );
    Ok(())
}

/// A read that finds no blob below the cache drops the digest from the
/// cache at once. Without that, a blob that left the object store keeps
/// being reported present for the whole TTL while every read of it fails.
#[nativelink_test]
async fn a_read_that_finds_nothing_stops_the_cache_vouching() -> Result<(), Error> {
    let spec = ExistenceCacheSpec {
        backend: StoreSpec::Noop(NoopSpec::default()),
        eviction_policy: Option::default(),
    };
    let inner = SilentlyEvictingStore::new();
    let store = ExistenceCacheStore::new(&spec, Store::new(inner.clone()));
    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    Store::new(inner.clone())
        .update_oneshot(digest, "123".into())
        .await?;
    assert_eq!(store.has(digest).await?, Some(3));
    inner.lose(digest).await;
    assert_eq!(store.has(digest).await?, Some(3), "the cache still vouches");

    let err = store
        .get_part_unchunked(digest, 0, None)
        .await
        .expect_err("the blob is gone");
    assert_eq!(err.code, Code::NotFound);
    assert!(!store.exists_in_cache(&digest).await);
    assert_eq!(
        store.has(digest).await?,
        None,
        "the next check tells the truth"
    );
    Ok(())
}

fn existence_spec() -> ExistenceCacheSpec {
    ExistenceCacheSpec {
        backend: StoreSpec::Noop(NoopSpec::default()),
        eviction_policy: Option::default(),
    }
}

/// Inner store whose `remove` / `remove_cached` fail. Presence metadata must
/// still be dropped so the old positive is not trusted.
#[derive(Debug, MetricsComponent)]
struct FailingRemoveStore {
    inner: Arc<MemoryStore>,
}

impl FailingRemoveStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(&MemorySpec::default()),
        })
    }
}

#[async_trait]
impl StoreDriver for FailingRemoveStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        Pin::new(self.inner.as_ref())
            .has_with_results(keys, results)
            .await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
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

    async fn remove(self: Pin<&Self>, _key: StoreKey<'_>) -> Result<bool, Error> {
        Err(make_err!(Code::Internal, "backend remove failed"))
    }

    async fn remove_cached(self: Pin<&Self>, _key: StoreKey<'_>) -> Result<bool, Error> {
        Err(make_err!(Code::Internal, "backend remove_cached failed"))
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

    fn register_remove_callback(self: Arc<Self>, callback: RemoveCallback) -> Result<(), Error> {
        self.inner.clone().register_remove_callback(callback)
    }
}

default_health_status_indicator!(FailingRemoveStore);

/// Inner store that can hold `has()` until `release` is signalled, then
/// answers with a stale positive instead of consulting the live backend.
#[derive(Debug, MetricsComponent)]
struct GatedHasStore {
    inner: Arc<MemoryStore>,
    gate_has: AtomicBool,
    started: Notify,
    release: Notify,
}

impl GatedHasStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: MemoryStore::new(&MemorySpec::default()),
            gate_has: AtomicBool::new(false),
            started: Notify::new(),
            release: Notify::new(),
        })
    }
}

#[async_trait]
impl StoreDriver for GatedHasStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        if self.gate_has.load(Ordering::SeqCst) {
            self.started.notify_one();
            self.release.notified().await;
            for (key, result) in keys.iter().zip(results.iter_mut()) {
                *result = Some(key.borrow().into_digest().size_bytes());
            }
            return Ok(());
        }
        Pin::new(self.inner.as_ref())
            .has_with_results(keys, results)
            .await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
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

    async fn remove(self: Pin<&Self>, key: StoreKey<'_>) -> Result<bool, Error> {
        Pin::new(self.inner.as_ref()).remove(key).await
    }

    async fn remove_cached(self: Pin<&Self>, key: StoreKey<'_>) -> Result<bool, Error> {
        Pin::new(self.inner.as_ref()).remove_cached(key).await
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

    fn register_remove_callback(self: Arc<Self>, callback: RemoveCallback) -> Result<(), Error> {
        self.inner.clone().register_remove_callback(callback)
    }
}

default_health_status_indicator!(GatedHasStore);

/// Warm a positive entry, remove the underlying eligible cached blob through
/// the wrapper, and confirm the next `has()` queries the backend and reports
/// missing.
#[nativelink_test]
async fn remove_evicts_presence_and_the_next_has_reports_missing() -> Result<(), Error> {
    const VALUE: &str = "123";
    let inner = MemoryStore::new(&MemorySpec::default());
    let store = ExistenceCacheStore::new(&existence_spec(), Store::new(inner.clone()));
    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    store.update_oneshot(digest, VALUE.into()).await?;
    assert!(store.exists_in_cache(&digest).await);

    assert!(store.as_store_driver_pin().remove(digest.into()).await?);
    assert!(!store.exists_in_cache(&digest).await);
    assert_eq!(inner.has(digest).await?, None);
    assert_eq!(store.has(digest).await?, None);
    Ok(())
}

/// A no-op inner `remove` still drops the old positive. Durable data stays,
/// so a later backend lookup may legitimately repopulate the index.
/// `removed=true` is not returned for metadata-only invalidation.
#[nativelink_test]
async fn remove_noop_backend_drops_presence_and_keeps_durable_data() -> Result<(), Error> {
    const VALUE: &str = "123";
    let inner = SilentlyEvictingStore::new();
    let store = ExistenceCacheStore::new(&existence_spec(), Store::new(inner.clone()));
    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    store.update_oneshot(digest, VALUE.into()).await?;
    assert!(store.exists_in_cache(&digest).await);

    assert!(
        !store.as_store_driver_pin().remove(digest.into()).await?,
        "dropping the index must not report a blob copy was removed"
    );
    assert!(!store.exists_in_cache(&digest).await);
    assert_eq!(
        inner.inner.has(digest).await?,
        Some(VALUE.len() as u64),
        "durable data remains"
    );
    assert_eq!(
        store.has(digest).await?,
        Some(VALUE.len() as u64),
        "a later backend lookup may repopulate the index"
    );
    assert!(store.exists_in_cache(&digest).await);
    Ok(())
}

#[nativelink_test]
async fn remove_backend_error_drops_presence() -> Result<(), Error> {
    const VALUE: &str = "123";
    let inner = FailingRemoveStore::new();
    let store = ExistenceCacheStore::new(&existence_spec(), Store::new(inner.clone()));
    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    store.update_oneshot(digest, VALUE.into()).await?;
    assert!(store.exists_in_cache(&digest).await);

    let err = store
        .as_store_driver_pin()
        .remove(digest.into())
        .await
        .unwrap_err();
    assert_eq!(err.code, Code::Internal);
    assert!(!store.exists_in_cache(&digest).await);
    assert_eq!(inner.inner.has(digest).await?, Some(VALUE.len() as u64));
    Ok(())
}

/// Inner `remove` fires the registered callback, which drops the presence
/// entry even when the wrapper's own `remove` is not the caller.
#[nativelink_test]
async fn inner_remove_callback_drops_presence() -> Result<(), Error> {
    const VALUE: &str = "123";
    let inner = MemoryStore::new(&MemorySpec::default());
    let store = ExistenceCacheStore::new(&existence_spec(), Store::new(inner.clone()));
    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    store.update_oneshot(digest, VALUE.into()).await?;
    assert!(store.exists_in_cache(&digest).await);

    assert!(Pin::new(inner.as_ref()).remove(digest.into()).await?);
    assert!(!store.exists_in_cache(&digest).await);
    assert_eq!(store.has(digest).await?, None);
    Ok(())
}

/// A truncated fast-tier copy is dropped through `remove_cached` so the
/// verifier's follow-up read refills from the durable copy. Dropping presence
/// metadata is not durable-object repair.
#[nativelink_test]
async fn truncated_cache_entry_is_evicted_and_refills_from_durable() -> Result<(), Error> {
    const VALUE: &str = "123";
    let fast = MemoryStore::new(&MemorySpec::default());
    let slow = MemoryStore::new(&MemorySpec::default());
    let fast_slow = Store::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
            bypass_dedup_threshold_bytes: 0,
            has_slow_write_wait_timeout_s: 0,
            leader_inactivity_timeout_s: 0,
        },
        Store::new(fast.clone()),
        Store::new(slow.clone()),
    ));
    let existence = ExistenceCacheStore::new(&existence_spec(), fast_slow);
    let store = Store::new(VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Noop(NoopSpec::default()),
            verify_size: true,
            verify_hash: false,
        },
        Store::new(existence.clone()),
    ));
    let digest = DigestInfo::try_new(VALID_HASH1, VALUE.len()).unwrap();
    store.update_oneshot(digest, VALUE.into()).await?;
    assert!(existence.exists_in_cache(&digest).await);

    Store::new(fast.clone())
        .update_oneshot(digest, "12".into())
        .await?;
    assert_eq!(
        fast.get_part_unchunked(digest, 0, None).await?,
        b"12".as_slice(),
        "fast tier holds the truncated copy"
    );
    let err = store
        .get_part_unchunked(digest, 0, None)
        .await
        .expect_err("truncated fast copy must fail size verification");
    assert_eq!(err.code, Code::DataLoss);
    assert_eq!(
        slow.get_part_unchunked(digest, 0, None).await?,
        VALUE.as_bytes(),
        "durable copy remains"
    );
    assert_eq!(
        fast.get_part_unchunked(digest, 0, None).await?,
        VALUE.as_bytes(),
        "the truncated copy was dropped and the fast tier refilled from durable"
    );

    assert_eq!(
        store.get_part_unchunked(digest, 0, None).await?,
        VALUE.as_bytes(),
        "the next verified read succeeds"
    );
    Ok(())
}

/// Direct `remove_cached` through the wrapper drops the fast copy and the
/// presence entry, and leaves the durable copy.
#[nativelink_test]
async fn remove_cached_drops_fast_copy_and_keeps_durable() -> Result<(), Error> {
    const VALUE: &str = "123";
    let fast = MemoryStore::new(&MemorySpec::default());
    let slow = MemoryStore::new(&MemorySpec::default());
    let fast_slow = Store::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
            bypass_dedup_threshold_bytes: 0,
            has_slow_write_wait_timeout_s: 0,
            leader_inactivity_timeout_s: 0,
        },
        Store::new(fast.clone()),
        Store::new(slow.clone()),
    ));
    let store = ExistenceCacheStore::new(&existence_spec(), fast_slow);
    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    store.update_oneshot(digest, VALUE.into()).await?;
    assert!(store.exists_in_cache(&digest).await);

    assert!(
        store
            .as_store_driver_pin()
            .remove_cached(digest.into())
            .await?
    );
    assert!(!store.exists_in_cache(&digest).await);
    assert_eq!(fast.has(digest).await?, None);
    assert_eq!(
        slow.get_part_unchunked(digest, 0, None).await?,
        VALUE.as_bytes()
    );
    assert_eq!(
        store.has(digest).await?,
        Some(VALUE.len() as u64),
        "has() queries the backend and sees the durable copy"
    );
    Ok(())
}

/// An in-flight `has()` that queried the backend before `remove` finished
/// must not restore the stale positive.
#[nativelink_test]
async fn gated_has_during_remove_does_not_restore_stale_positive() -> Result<(), Error> {
    const VALUE: &str = "123";
    let inner = GatedHasStore::new();
    let store = ExistenceCacheStore::new(&existence_spec(), Store::new(inner.clone()));
    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    store.update_oneshot(digest, VALUE.into()).await?;
    store.remove_from_cache(&digest).await;
    assert!(!store.exists_in_cache(&digest).await);

    inner.gate_has.store(true, Ordering::SeqCst);
    let has_store = store.clone();
    let has_task = spawn!(
        "gated_existence_has",
        async move { has_store.has(digest).await }
    );
    inner.started.notified().await;

    assert!(store.as_store_driver_pin().remove(digest.into()).await?);
    inner.release.notify_one();
    let _ = has_task
        .await
        .expect("gated has() should finish")
        .expect("gated has() should not error");

    inner.gate_has.store(false, Ordering::SeqCst);
    assert!(
        !store.exists_in_cache(&digest).await,
        "in-flight lookup must not restore the stale positive"
    );
    assert_eq!(store.has(digest).await?, None);
    assert_eq!(inner.inner.has(digest).await?, None);
    Ok(())
}
