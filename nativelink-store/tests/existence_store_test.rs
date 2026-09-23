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
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use mock_instant::thread_local::MockClock;
use nativelink_config::stores::{
    EvictionPolicy, ExistenceCacheSpec, MemorySpec, NoopSpec, StoreSpec,
};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_store::existence_cache_store::ExistenceCacheStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::spawn;
use nativelink_util::store_trait::{
    RemoveCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use parking_lot::Mutex;
use pretty_assertions::assert_eq;
use tokio::sync::oneshot;

const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";
const VALID_HASH2: &str = "0123456789abcdef000000000000000000020000000000000123456789abcdef";

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

/// Inner store that evicts `evicted_digest` as soon as it is inserted, then
/// holds that update open until released, so a test can finish another update
/// in between. Every other key is stored.
#[derive(Debug, MetricsComponent)]
struct SelfEvictingStore {
    evicted_digest: DigestInfo,
    stored: Mutex<HashMap<DigestInfo, u64>>,
    remove_callbacks: Mutex<Vec<RemoveCallback>>,
    /// Fired once the eviction's remove callbacks have run.
    evicted_tx: Mutex<Option<oneshot::Sender<()>>>,
    /// Awaited before the evicted key's update returns.
    release_rx: Mutex<Option<oneshot::Receiver<()>>>,
}

#[async_trait]
impl StoreDriver for SelfEvictingStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        let stored = self.stored.lock();
        for (key, result) in keys.iter().zip(results.iter_mut()) {
            *result = stored.get(&key.borrow().into_digest()).copied();
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        let size = reader.drain().await?;
        let digest = key.into_digest();
        if digest != self.evicted_digest {
            self.stored.lock().insert(digest, size);
            return Ok(size);
        }
        let remove_callbacks = self.remove_callbacks.lock().clone();
        for remove_callback in remove_callbacks {
            remove_callback.callback(digest.into()).await;
        }
        let evicted_tx = self.evicted_tx.lock().take();
        if let Some(evicted_tx) = evicted_tx {
            evicted_tx
                .send(())
                .map_err(|()| make_err!(Code::Internal, "Test stopped waiting for eviction"))?;
        }
        let release_rx = self.release_rx.lock().take();
        if let Some(release_rx) = release_rx {
            release_rx
                .await
                .map_err(|e| make_err!(Code::Internal, "Update never released: {e:?}"))?;
        }
        Ok(size)
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        _writer: &mut DropCloserWriteHalf,
        _offset: u64,
        _length: Option<u64>,
    ) -> Result<(), Error> {
        Err(make_err!(Code::NotFound, "{key:?} not readable in test"))
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
        self.remove_callbacks.lock().push(callback);
        Ok(())
    }
}

default_health_status_indicator!(SelfEvictingStore);

// The inner store evicts a key while its update is in flight, and another
// update finishes before it does. The existence cache must not then cache the
// evicted key, or `has()` reports it present until the entry expires.
#[nativelink_test]
async fn key_evicted_during_its_update_is_not_cached_when_another_update_finishes_first()
-> Result<(), Error> {
    const VALUE: &str = "123";
    let spec = ExistenceCacheSpec {
        backend: StoreSpec::Noop(NoopSpec::default()), // Note: Not used.
        eviction_policy: Option::default(),
    };
    let evicted_digest = DigestInfo::try_new(VALID_HASH1, VALUE.len())?;
    let kept_digest = DigestInfo::try_new(VALID_HASH2, VALUE.len())?;
    let (evicted_tx, evicted_rx) = oneshot::channel();
    let (release_tx, release_rx) = oneshot::channel();
    let inner_store = Arc::new(SelfEvictingStore {
        evicted_digest,
        stored: Mutex::new(HashMap::new()),
        remove_callbacks: Mutex::new(Vec::new()),
        evicted_tx: Mutex::new(Some(evicted_tx)),
        release_rx: Mutex::new(Some(release_rx)),
    });
    let store = ExistenceCacheStore::new(&spec, Store::new(inner_store));

    // Start updating the key the inner store evicts, and wait for the eviction.
    let evicted_update = spawn!("evicted_update", {
        let store = store.clone();
        async move { store.update_oneshot(evicted_digest, VALUE.into()).await }
    });
    evicted_rx
        .await
        .map_err(|e| make_err!(Code::Internal, "Eviction never happened: {e:?}"))?;

    // Finish another update while that one is still in flight.
    store
        .update_oneshot(kept_digest, VALUE.into())
        .await
        .err_tip(|| "Failed to update kept key")?;

    // Only now let the evicted key's update finish.
    release_tx
        .send(())
        .map_err(|()| make_err!(Code::Internal, "Failed to release evicted update"))?;
    evicted_update
        .await
        .map_err(|e| make_err!(Code::Internal, "Evicted update panicked: {e:?}"))?
        .err_tip(|| "Failed to update evicted key")?;

    assert!(
        !store.exists_in_cache(&evicted_digest).await,
        "Evicted key must not be in the existence cache"
    );
    assert_eq!(
        store.has(evicted_digest).await?,
        None,
        "Evicted key must be reported missing"
    );
    assert_eq!(
        store.has(kept_digest).await?,
        Some(VALUE.len() as u64),
        "Kept key must be reported present"
    );
    Ok(())
}
