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
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use mock_instant::thread_local::MockClock;
use nativelink_config::stores::{
    EvictionPolicy, ExistenceCacheSpec, MemorySpec, NoopSpec, StoreSpec,
};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
};
use nativelink_store::existence_cache_store::ExistenceCacheStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::store_trait::{
    RemoveCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use parking_lot::Mutex;
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

// Reproduces the pause_remove_callbacks race: the pause slot is a single
// non-refcounted Option shared by all concurrent update() calls. The first
// update to finish take()s it, ending the pause for every other in-flight
// update. A remove callback (eviction under pressure) firing inside the
// second update's inner_store.update await then runs immediately, BEFORE
// that update inserts into the existence cache, leaving a permanent stale
// exists-entry for a blob the inner store does not hold. Subsequent uploads
// of that key are drained and silently discarded.
#[nativelink_test]
async fn concurrent_update_pause_refcount_race_test() -> Result<(), Error> {
    use core::pin::Pin;
    use std::collections::HashMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use nativelink_metric::MetricsComponent;
    use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
    use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
    use nativelink_util::store_trait::{RemoveCallback, StoreDriver, StoreKey, UploadSizeInfo};
    use tokio::sync::Notify;

    const VALID_HASH2: &str = "0223456789abcdef000000000000000000010000000000000123456789abcdef";

    type GateMap = HashMap<DigestInfo, (Arc<Notify>, Arc<Notify>)>;

    #[derive(MetricsComponent)]
    struct GatedStore {
        inner: Store,
        mem: Arc<MemoryStore>,
        // Captured ExistenceCacheCallback registered by ExistenceCacheStore.
        callback: Mutex<Option<RemoveCallback>>,
        // (entered, release) per digest. Each gate fires at most once; once
        // consumed the digest is removed so a later re-upload of the same key
        // passes straight through to the inner store.
        gates: Mutex<GateMap>,
        // Key whose insertion should trigger an inline eviction (i.e. the
        // inner store evicts the just-inserted entry under cache pressure
        // and fires the remove callback while still inside update()).
        evict_key: DigestInfo,
    }

    impl core::fmt::Debug for GatedStore {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            f.write_str("GatedStore")
        }
    }

    #[async_trait]
    impl StoreDriver for GatedStore {
        async fn post_init(self: Arc<Self>) -> Result<(), Error> {
            Ok(())
        }

        async fn has_with_results(
            self: Pin<&Self>,
            digests: &[StoreKey<'_>],
            results: &mut [Option<u64>],
        ) -> Result<(), Error> {
            self.inner.has_with_results(digests, results).await
        }

        async fn update(
            self: Pin<&Self>,
            key: StoreKey<'_>,
            reader: DropCloserReadHalf,
            size_info: UploadSizeInfo,
        ) -> Result<u64, Error> {
            let digest = key.borrow().into_digest();
            let gate = self.gates.lock().remove(&digest);
            let was_gated = gate.is_some();
            if let Some((entered, release)) = gate {
                entered.notify_one();
                release.notified().await;
            }
            let result = self.inner.update(key, reader, size_info).await;
            // Only the first (gated) upload of evict_key simulates the
            // eviction-under-pressure; a later re-upload must persist.
            if was_gated && digest == self.evict_key {
                // Simulate eviction-under-pressure of the just-inserted
                // entry: the evicting map removes it and fires the remove
                // callback inline, inside this awaited update (see
                // filesystem_store.rs "Insertion can unref this entry
                // immediately under cache pressure").
                self.mem.remove_entry(digest.into()).await;
                let callback = self.callback.lock().clone();
                if let Some(callback) = callback {
                    callback.callback(digest.into()).await;
                }
            }
            result
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

        fn inner_store(&self, _digest: Option<StoreKey>) -> &dyn StoreDriver {
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
            callback: RemoveCallback,
        ) -> Result<(), Error> {
            *self.callback.lock() = Some(callback);
            Ok(())
        }
    }
    default_health_status_indicator!(GatedStore);

    let digest_x = DigestInfo::try_new(VALID_HASH1, 2).unwrap();
    let digest_y = DigestInfo::try_new(VALID_HASH2, 2).unwrap();

    let entered_x = Arc::new(Notify::new());
    let release_x = Arc::new(Notify::new());
    let entered_y = Arc::new(Notify::new());
    let release_y = Arc::new(Notify::new());

    let mem = MemoryStore::new(&MemorySpec::default());
    let gated = Arc::new(GatedStore {
        inner: Store::new(mem.clone()),
        mem: mem.clone(),
        callback: Mutex::new(None),
        gates: Mutex::new(HashMap::from([
            (digest_x, (entered_x.clone(), release_x.clone())),
            (digest_y, (entered_y.clone(), release_y.clone())),
        ])),
        evict_key: digest_y,
    });

    let spec = ExistenceCacheSpec {
        backend: StoreSpec::Noop(NoopSpec::default()),
        eviction_policy: Option::default(),
    };
    let store = ExistenceCacheStore::new(&spec, Store::new(gated.clone()));

    // Update A (key X): reaches inner update, having installed the pause.
    let store_a = store.clone();
    let task_a = tokio::spawn(async move { store_a.update_oneshot(digest_x, "aa".into()).await });
    entered_x.notified().await;

    // Update B (key Y): sees the pause already installed and relies on it.
    let store_b = store.clone();
    let task_b = tokio::spawn(async move { store_b.update_oneshot(digest_y, "bb".into()).await });
    entered_y.notified().await;

    // A finishes completely: inserts X and take()s the shared pause slot,
    // ending the pause window for B as well.
    release_x.notify_one();
    task_a.await.unwrap()?;

    // B finishes its inner update; the just-inserted Y is evicted under
    // pressure and the remove callback fires inline (during B's await),
    // then B inserts Y into the existence cache.
    release_y.notify_one();
    task_b.await.unwrap()?;

    // The inner store does not hold Y (it was evicted).
    assert_eq!(mem.has(digest_y).await?, None, "Y was evicted from inner");

    // Correct behavior: the eviction notification for Y, delivered during
    // B's update, must not be lost - the existence cache must not claim Y
    // exists. With the non-refcounted pause slot, A's take() ended the
    // pause, the removal ran before B's insert, and the cache is stale.
    assert!(
        !store.exists_in_cache(&digest_y).await,
        "existence cache claims Y exists but inner store does not hold it"
    );

    // Demonstrate the harm: a re-upload of Y is drained and discarded.
    store.update_oneshot(digest_y, "bb".into()).await?;
    assert_eq!(
        mem.has(digest_y).await?,
        Some(2),
        "re-upload of Y was silently discarded due to stale existence cache entry"
    );
    Ok(())
}

const VALID_HASH2: &str = "aa23456789abcdef000000000000000000010000000000000123456789abcdef";

// Repro for the pause_remove_callbacks race: it is an Option-as-boolean with no
// refcount. Two concurrent update() calls share one pause window; the first
// finisher unconditionally take()s the flag, unpausing the second update while
// its inner-store write is still in flight. If the inner store then fires a
// remove callback for the in-flight key (eviction/oversized skip), the callback
// runs immediately as a no-op (key not yet cached), and the second update then
// inserts a stale "exists" entry for a blob the inner store does not hold.
#[nativelink_test]
async fn concurrent_update_unpause_race_stale_existence_test() -> Result<(), Error> {
    let spec = ExistenceCacheSpec {
        backend: StoreSpec::Noop(NoopSpec::default()),
        eviction_policy: Option::default(),
    };
    // max_bytes small enough that digest_b's write is skipped (and remove
    // callbacks fired) by the inner memory store.
    let inner_store = Store::new(MemoryStore::new(&MemorySpec {
        eviction_policy: Some(EvictionPolicy {
            max_bytes: 150,
            ..Default::default()
        }),
    }));
    let store = ExistenceCacheStore::new_with_time(
        &spec,
        inner_store.clone(),
        MockInstantWrapped::default(),
    );

    let digest_a = DigestInfo::try_new(VALID_HASH1, 10).unwrap();
    let digest_b = DigestInfo::try_new(VALID_HASH2, 200).unwrap();

    // Task B: starts update(), sets the pause flag, then parks inside the inner
    // store's update awaiting stream data.
    let (mut tx_b, rx_b) = nativelink_util::buf_channel::make_buf_channel_pair();
    let store_b = store.clone();
    let task_b = nativelink_util::spawn!("update_b", async move {
        store_b
            .update(digest_b, rx_b, UploadSizeInfo::ExactSize(200))
            .await
    });
    // Single-threaded runtime: let B run until it awaits the reader.
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }

    // Task A: full update while B is in flight. A finds the pause flag Some
    // (set by B), and at the end unconditionally take()s it -- stealing B's
    // pause window.
    store
        .update_oneshot(digest_a, vec![0u8; 10].into())
        .await
        .err_tip(|| "update A failed")?;

    // Now finish B. The inner memory store drains it, refuses to store it
    // (200 >= max_bytes) and fires remove callbacks for digest_b -- but the
    // pause flag is already None, so the removal runs immediately as a no-op.
    // B then inserts digest_b into the existence cache: stale entry.
    tx_b.send(Bytes::from(vec![0u8; 200]))
        .await
        .err_tip(|| "send b")?;
    tx_b.send_eof().err_tip(|| "eof b")?;
    task_b
        .await
        .expect("join b")
        .err_tip(|| "update B failed")?;

    assert_eq!(
        inner_store.has(digest_b).await?,
        None,
        "precondition: inner store must not hold digest_b"
    );
    assert_eq!(
        store.has(digest_b).await.err_tip(|| "has b")?,
        None,
        "existence cache claims digest_b exists but the inner store evicted it \
         (pause window stolen by concurrent update)"
    );
    Ok(())
}

/// Inner store standing in for a filesystem store: a read that is in-flight
/// keeps streaming successfully even if the entry is evicted mid-read
/// (filesystem reads hold the `FileEntry` Arc). The test controls exactly
/// where the read is suspended via `Notify` rendezvous points.
#[derive(Debug)]
struct PausingReadStore {
    present: AtomicBool,
    read_started: Notify,
    allow_read_finish: Notify,
    remove_callbacks: Mutex<Vec<RemoveCallback>>,
    data: Bytes,
}

impl PausingReadStore {
    fn new(data: Bytes) -> Arc<Self> {
        Arc::new(Self {
            present: AtomicBool::new(true),
            read_started: Notify::new(),
            allow_read_finish: Notify::new(),
            remove_callbacks: Mutex::new(vec![]),
            data,
        })
    }

    /// Simulate the evicting map evicting `key`: drop residency and fire the
    /// registered remove callbacks, exactly like `EvictingMap` does.
    async fn evict(&self, key: StoreKey<'static>) {
        self.present.store(false, Ordering::SeqCst);
        let callbacks: Vec<RemoveCallback> = self.remove_callbacks.lock().clone();
        for callback in callbacks {
            callback.callback(key.borrow()).await;
        }
    }
}

impl MetricsComponent for PausingReadStore {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        Ok(MetricPublishKnownKindData::Component)
    }
}

#[async_trait]
impl StoreDriver for PausingReadStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        _keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        let value = if self.present.load(Ordering::SeqCst) {
            Some(self.data.len() as u64)
        } else {
            None
        };
        for result in results.iter_mut() {
            *result = value;
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        let size = reader.drain().await?;
        self.present.store(true, Ordering::SeqCst);
        Ok(size)
    }

    async fn get_part(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        _offset: u64,
        _length: Option<u64>,
    ) -> Result<(), Error> {
        if !self.present.load(Ordering::SeqCst) {
            return Err(make_err!(Code::NotFound, "Not found in PausingReadStore"));
        }
        // Rendezvous: tell the test the read is parked at a real await point,
        // then wait until the test lets the (still valid) stream complete.
        self.read_started.notify_one();
        self.allow_read_finish.notified().await;
        writer.send(self.data.clone()).await?;
        writer.send_eof()?;
        Ok(())
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

    fn register_remove_callback(self: Arc<Self>, callback: RemoveCallback) -> Result<(), Error> {
        self.remove_callbacks.lock().push(callback);
        Ok(())
    }
}

#[async_trait]
impl HealthStatusIndicator for PausingReadStore {
    fn get_name(&self) -> &'static str {
        "PausingReadStore"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}

// Repro: get_part() re-inserts the existence entry after the inner read
// succeeds without participating in the pause_remove_callbacks protocol.
// If the inner store evicts the digest while the read is in flight (legal:
// filesystem reads hold the FileEntry Arc and stream to completion), the
// eviction's remove-callback purges the existence cache first, and get_part
// then resurrects a zombie exists-entry for data the inner store no longer
// has. A subsequent update() sees has()==true and silently discards the
// re-upload: data loss.
#[nativelink_test]
async fn get_part_races_inner_eviction_and_resurrects_zombie_entry() -> Result<(), Error> {
    const VALUE: &str = "123";
    let inner = PausingReadStore::new(Bytes::from_static(VALUE.as_bytes()));
    let store = ExistenceCacheStore::new_with_time(
        &ExistenceCacheSpec {
            backend: StoreSpec::Noop(NoopSpec::default()),
            eviction_policy: Option::default(),
        },
        Store::new(inner.clone()),
        MockInstantWrapped::default(),
    );
    let digest = DigestInfo::try_new(VALID_HASH1, VALUE.len() as u64)?;

    // Task A: client read. Parks inside inner get_part at a genuine await
    // point (stand-in for the filesystem store's read/send loop).
    let store_clone = store.clone();
    let read_task =
        tokio::spawn(async move { store_clone.get_part_unchunked(digest, 0, None).await });
    inner.read_started.notified().await;

    // Task B: cache pressure evicts D from the inner store while the read is
    // suspended. The remove callback purges the existence cache (correctly).
    inner.evict(digest.into()).await;
    assert!(
        !store.exists_in_cache(&digest).await,
        "Eviction callback should have purged the existence cache"
    );

    // Task A resumes; the in-flight read still completes Ok.
    inner.allow_read_finish.notify_one();
    let read_result = read_task.await.expect("read task panicked")?;
    assert_eq!(read_result, Bytes::from_static(VALUE.as_bytes()));

    // BUG: get_part re-inserted the existence entry after the eviction
    // notification was already consumed. The inner store no longer has D.
    assert!(
        !store.exists_in_cache(&digest).await,
        "Zombie exists-entry: existence cache claims digest is present but \
         the inner store evicted it"
    );

    // Consequence: a re-upload of D is drained and discarded by update(),
    // so the data is lost while has() keeps reporting true.
    store.update_oneshot(digest, VALUE.into()).await?;
    let mut inner_result = [None];
    Pin::new(&*inner)
        .has_with_results(&[digest.into()], &mut inner_result)
        .await?;
    assert_eq!(
        inner_result[0],
        Some(VALUE.len() as u64),
        "Re-upload was silently discarded due to zombie existence entry"
    );

    Ok(())
}

/// Inner store whose `update` parks at a real await point until released, so a
/// test can drop the outer `update` future while the pause is held. `evict`
/// fires the registered remove callbacks like `EvictingMap` does.
#[derive(Debug)]
struct BlockingUpdateStore {
    present: AtomicBool,
    update_started: Notify,
    allow_update_finish: Notify,
    remove_callbacks: Mutex<Vec<RemoveCallback>>,
    len: u64,
}

impl BlockingUpdateStore {
    fn new(len: u64) -> Arc<Self> {
        Arc::new(Self {
            present: AtomicBool::new(true),
            update_started: Notify::new(),
            allow_update_finish: Notify::new(),
            remove_callbacks: Mutex::new(vec![]),
            len,
        })
    }

    async fn evict(&self, key: StoreKey<'static>) {
        self.present.store(false, Ordering::SeqCst);
        let callbacks: Vec<RemoveCallback> = self.remove_callbacks.lock().clone();
        for callback in callbacks {
            callback.callback(key.borrow()).await;
        }
    }
}

impl MetricsComponent for BlockingUpdateStore {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        Ok(MetricPublishKnownKindData::Component)
    }
}

#[async_trait]
impl StoreDriver for BlockingUpdateStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        _keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        let value = if self.present.load(Ordering::SeqCst) {
            Some(self.len)
        } else {
            None
        };
        for result in results.iter_mut() {
            *result = value;
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        // Park at a genuine await point with the pause held, then wait for
        // the test. The test drops the future before ever releasing this.
        self.update_started.notify_one();
        self.allow_update_finish.notified().await;
        let size = reader.drain().await?;
        self.present.store(true, Ordering::SeqCst);
        Ok(size)
    }

    async fn get_part(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        _writer: &mut DropCloserWriteHalf,
        _offset: u64,
        _length: Option<u64>,
    ) -> Result<(), Error> {
        Err(make_err!(Code::NotFound, "not exercised"))
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

    fn register_remove_callback(self: Arc<Self>, callback: RemoveCallback) -> Result<(), Error> {
        self.remove_callbacks.lock().push(callback);
        Ok(())
    }
}

#[async_trait]
impl HealthStatusIndicator for BlockingUpdateStore {
    fn get_name(&self) -> &'static str {
        "BlockingUpdateStore"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}

/// Regression for N6: a cancelled `update` must not leave remove callbacks
/// paused forever. Start an `update` that parks with the pause held, drop its
/// future (as a timeout would), then evict a cached key through the inner
/// store. With the RAII pause guard the drop releases the hold, so the
/// eviction callback runs immediately and the existence cache stops reporting
/// the key. Without the guard the pause count stays elevated and the cache
/// keeps claiming the evicted key exists.
#[nativelink_test]
async fn cancelled_update_does_not_leave_callbacks_paused() -> Result<(), Error> {
    const LEN: u64 = 2;
    let inner = BlockingUpdateStore::new(LEN);
    let store = ExistenceCacheStore::new_with_time(
        &ExistenceCacheSpec {
            backend: StoreSpec::Noop(NoopSpec::default()),
            eviction_policy: Option::default(),
        },
        Store::new(inner.clone()),
        MockInstantWrapped::default(),
    );

    let digest_a = DigestInfo::try_new(VALID_HASH1, LEN)?;
    let digest_b = DigestInfo::try_new(VALID_HASH2, LEN)?;

    // Populate A's existence entry via a has() that the inner store answers
    // positively (present == true).
    assert_eq!(store.has(digest_a).await?, Some(LEN));
    assert!(store.exists_in_cache(&digest_a).await);

    // Start an update of B that parks inside the inner store holding the pause.
    let store_b = store.clone();
    let mut update_b = Box::pin(async move {
        store_b
            .update_oneshot(digest_b, vec![0u8; LEN as usize].into())
            .await
    });
    // Drive it until it parks at the inner update await, then cancel it by
    // dropping the future.
    tokio::select! {
        _ = &mut update_b => panic!("update should be parked, not complete"),
        () = inner.update_started.notified() => {}
    }
    drop(update_b);

    // Evict A through the inner store; the remove callback must take effect
    // now that the cancelled update released its pause.
    inner.evict(digest_a.into()).await;

    assert!(
        !store.exists_in_cache(&digest_a).await,
        "existence cache still claims A exists: a cancelled update left \
         remove callbacks permanently paused (N6)"
    );

    Ok(())
}
