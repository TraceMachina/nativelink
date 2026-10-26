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
use core::time::Duration;
use std::sync::Arc;

use async_trait::async_trait;
use mock_instant::thread_local::MockClock;
use nativelink_config::stores::{
    EvictionPolicy, ExistenceCacheSpec, MemorySpec, NoopSpec, StoreSpec,
};
use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_store::existence_cache_store::ExistenceCacheStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::store_trait::{
    RemoveCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use pretty_assertions::assert_eq;

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
    assert_eq!(err.code, nativelink_error::Code::NotFound);
    assert!(!store.exists_in_cache(&digest).await);
    assert_eq!(
        store.has(digest).await?,
        None,
        "the next check tells the truth"
    );
    Ok(())
}
