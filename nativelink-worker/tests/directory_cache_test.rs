// Copyright 2026 The NativeLink Authors. All rights reserved.
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
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use nativelink_config::stores::{
    FastSlowSpec, FilesystemSpec, MemorySpec, StoreDirection, StoreSpec,
};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
};
use nativelink_proto::build::bazel::remote::execution::v2::{
    Directory as ProtoDirectory, DirectoryNode, FileNode, SymlinkNode,
};
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::filesystem_store::FilesystemStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::{DigestInfo, make_temp_path};
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    RemoveItemCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use nativelink_worker::directory_cache::{DirectoryCache, DirectoryCacheConfig};
use prost::Message;
use tonic::Code;
use uuid::Uuid;

/// Wraps a `MemoryStore` as the slow tier of a `FastSlowStore` whose fast
/// tier is a real `FilesystemStore` — the store shape `DirectoryCache`
/// expects from the worker. Blobs seeded into `slow_store` are reachable via
/// the returned `FastSlowStore`.
async fn make_cas_store(slow_store: Arc<MemoryStore>) -> Arc<FastSlowStore> {
    let fast_spec = FilesystemSpec {
        content_path: make_temp_path("cas_content"),
        temp_path: make_temp_path("cas_temp"),
        eviction_policy: None,
        ..Default::default()
    };
    let fast_store: Arc<FilesystemStore> = FilesystemStore::new(&fast_spec).await.unwrap();
    FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Filesystem(fast_spec),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
            bypass_dedup_threshold_bytes: 0,
        },
        Store::new(fast_store),
        Store::new(slow_store),
    )
}

#[nativelink_test]
async fn create_directory_cache() -> Result<(), Error> {
    let config = DirectoryCacheConfig {
        cache_root: make_temp_path("directory_cache").into(),
        ..Default::default()
    };
    let store = MemoryStore::new(&MemorySpec::default());
    DirectoryCache::new(config, make_cas_store(store).await).await?;
    assert!(!logs_contain("ERROR"));
    assert!(!logs_contain("WARN"));
    Ok(())
}

#[nativelink_test]
async fn add_missing_file_to_cache() -> Result<(), Error> {
    let config = DirectoryCacheConfig {
        cache_root: make_temp_path("directory_cache").into(),
        ..Default::default()
    };
    let store = MemoryStore::new(&MemorySpec::default());
    let cache = DirectoryCache::new(config, make_cas_store(store).await).await?;
    let digest = DigestInfo::new([1u8; 32], 5);
    let uuid = Uuid::new_v4();
    let res = cache
        .get_or_create(digest, Path::new(&uuid.to_string()))
        .await;
    let err = res.expect_err("missing directory digest should fail");
    assert_eq!(err.code, Code::NotFound);
    assert!(
        err.messages.iter().any(|m| m.contains(
            "Failed to fetch directory digest: \
             DigestInfo(\"0101010101010101010101010101010101010101010101010101010101010101-5\")"
        )),
        "unexpected error: {err:?}",
    );
    assert!(!logs_contain("ERROR"));
    assert!(!logs_contain("WARN"));
    Ok(())
}

async fn single_insert(config: DirectoryCacheConfig) -> Result<(), Error> {
    let digest1 = DigestInfo::new([1u8; 32], 5);
    let store = MemoryStore::new(&MemorySpec::default());
    assert_eq!(
        store.update_oneshot(digest1, Bytes::from_static(b"")).await,
        Ok(())
    );
    let cache = DirectoryCache::new(config, make_cas_store(store).await).await?;
    assert_eq!(
        cache
            .get_or_create(digest1, Path::new(&Uuid::new_v4().to_string()))
            .await,
        Ok(false)
    );
    Ok(())
}

async fn double_insert(config: DirectoryCacheConfig) -> Result<(), Error> {
    let store = MemoryStore::new(&MemorySpec::default());
    double_insert_with_data(
        config,
        store,
        Bytes::from_static(b""),
        Bytes::from_static(b""),
    )
    .await
}

async fn double_insert_with_data(
    config: DirectoryCacheConfig,
    store: Arc<MemoryStore>,
    first: Bytes,
    second: Bytes,
) -> Result<(), Error> {
    let working_directory = PathBuf::from(make_temp_path("working_directory"));
    let digest1 = DigestInfo::new([1u8; 32], 5);
    let digest2 = DigestInfo::new([2u8; 32], 5);
    assert_eq!(store.update_oneshot(digest1, first).await, Ok(()));
    assert_eq!(store.update_oneshot(digest2, second).await, Ok(()));
    let cache = DirectoryCache::new(config, make_cas_store(store).await).await?;
    assert_eq!(
        cache
            .get_or_create(
                digest1,
                working_directory.join(Uuid::new_v4().to_string()).as_path()
            )
            .await,
        Ok(false)
    );
    assert_eq!(
        cache
            .get_or_create(
                digest2,
                working_directory.join(Uuid::new_v4().to_string()).as_path()
            )
            .await,
        Ok(false)
    );
    Ok(())
}

#[nativelink_test]
async fn add_file_to_cache() -> Result<(), Error> {
    let config = DirectoryCacheConfig {
        cache_root: make_temp_path("directory_cache").into(),
        ..Default::default()
    };
    single_insert(config).await?;
    assert!(!logs_contain("ERROR"));
    assert!(!logs_contain("WARN"));
    Ok(())
}

#[nativelink_test]
async fn fails_to_evicts_because_max_entries() -> Result<(), Error> {
    let config = DirectoryCacheConfig {
        max_entries: 0,
        cache_root: make_temp_path("directory_cache").into(),
        ..Default::default()
    };
    single_insert(config).await?;
    assert!(!logs_contain("ERROR"));
    assert!(logs_contain(
        "Unable to evict anything from directory_cache, will exceed max entries current_items=0 max_entries=0"
    ));
    Ok(())
}

#[nativelink_test]
async fn evicts_because_max_entries() -> Result<(), Error> {
    let config = DirectoryCacheConfig {
        max_entries: 1,
        cache_root: make_temp_path("directory_cache").into(),
        ..Default::default()
    };
    double_insert(config).await?;
    assert!(!logs_contain("ERROR"));
    assert!(!logs_contain("WARN"));
    assert!(logs_contain(
        "Evicting cached directory digest=DigestInfo(\"0101010101010101010101010101010101010101010101010101010101010101-5\") size=0"
    ));
    Ok(())
}

#[nativelink_test]
async fn evict_with_directory_entry() -> Result<(), Error> {
    let config = DirectoryCacheConfig {
        max_entries: 1,
        cache_root: make_temp_path("directory_cache").into(),
        ..Default::default()
    };
    let store = MemoryStore::new(&MemorySpec::default());
    let file_digest = DigestInfo::new([3u8; 32], 5);
    assert_eq!(
        store
            .update_oneshot(file_digest, Bytes::from_static(b""))
            .await,
        Ok(())
    );
    let directory_digest = DigestInfo::new([4u8; 32], 5);
    assert_eq!(
        store
            .update_oneshot(
                directory_digest,
                Into::<Bytes>::into(
                    ProtoDirectory {
                        files: vec![],
                        directories: vec![],
                        symlinks: vec![],
                        node_properties: None
                    }
                    .encode_to_vec()
                )
            )
            .await,
        Ok(())
    );
    let encoded_directory = Into::<Bytes>::into(
        ProtoDirectory {
            files: vec![FileNode {
                name: "demo file".to_string(),
                digest: Some(file_digest.into()),
                is_executable: false,
                node_properties: None,
            }],
            directories: vec![DirectoryNode {
                name: "demo_subdir".to_string(),
                digest: Some(directory_digest.into()),
            }],
            symlinks: vec![SymlinkNode {
                name: "demo_symlink".to_string(),
                target: "demo file".to_string(),
                node_properties: None,
            }],
            node_properties: None,
        }
        .encode_to_vec(),
    );
    double_insert_with_data(config, store, encoded_directory.clone(), encoded_directory).await?;
    assert!(!logs_contain("ERROR"));
    assert!(!logs_contain("WARN"));
    // The cache entry's size is the sum of its FileNode digest sizes (the
    // single `demo file` node declares size 5); the empty `demo_subdir`
    // contributes 0. This is digest-derived accounting (OPT #2), not a
    // filesystem walk, so it reflects the declared digest size.
    assert!(logs_contain(
        "Evicting cached directory digest=DigestInfo(\"0101010101010101010101010101010101010101010101010101010101010101-5\") size=5"
    ));
    Ok(())
}

/// `MemoryStore` wrapper that records the maximum number of concurrent
/// `get_part` calls it has observed. Each call holds its slot briefly so
/// that overlapping fetches are reliably observed as overlapping.
#[derive(Debug)]
struct ConcurrencyTrackingStore {
    inner: Store,
    in_flight: AtomicU64,
    max_in_flight: AtomicU64,
}

impl ConcurrencyTrackingStore {
    fn new(inner: Arc<MemoryStore>) -> Arc<Self> {
        Arc::new(Self {
            inner: Store::new(inner),
            in_flight: AtomicU64::new(0),
            max_in_flight: AtomicU64::new(0),
        })
    }
}

impl MetricsComponent for ConcurrencyTrackingStore {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        Ok(MetricPublishKnownKindData::Component)
    }
}

#[async_trait]
impl StoreDriver for ConcurrencyTrackingStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        self.inner
            .as_store_driver_pin()
            .has_with_results(keys, results)
            .await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        self.inner
            .as_store_driver_pin()
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
        let now_in_flight = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight
            .fetch_max(now_in_flight, Ordering::SeqCst);
        // Hold the slot long enough for concurrent fetches to overlap.
        tokio::time::sleep(Duration::from_millis(5)).await;
        let result = self
            .inner
            .as_store_driver_pin()
            .get_part(key, writer, offset, length)
            .await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        result
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

default_health_status_indicator!(ConcurrencyTrackingStore);

/// Cold-cache construction must fetch a directory's blobs concurrently, not
/// one at a time: every fast-tier miss pays a full slow-store round trip, so
/// sequential fetches serialize N round trips (regression test for the
/// `for … await` loops `construct_directory` used to have).
#[nativelink_test]
async fn cold_construct_fetches_concurrently() -> Result<(), Error> {
    const NUM_FILES: u32 = 32;

    let memory_store = MemoryStore::new(&MemorySpec::default());
    let mut file_nodes = Vec::new();
    for i in 0..NUM_FILES {
        let content = format!("content-{i}").into_bytes();
        let mut hash = [0u8; 32];
        hash[0] = 1;
        hash[1..5].copy_from_slice(&i.to_le_bytes());
        let digest = DigestInfo::new(hash, content.len() as u64);
        memory_store
            .update_oneshot(digest, Bytes::from(content))
            .await?;
        file_nodes.push(FileNode {
            name: format!("file_{i}"),
            digest: Some(digest.into()),
            is_executable: false,
            node_properties: None,
        });
    }
    let root = ProtoDirectory {
        files: file_nodes,
        directories: vec![],
        symlinks: vec![],
        node_properties: None,
    }
    .encode_to_vec();
    let root_digest = DigestInfo::new([7u8; 32], root.len() as u64);
    memory_store
        .update_oneshot(root_digest, Bytes::from(root))
        .await?;

    let tracking_store = ConcurrencyTrackingStore::new(memory_store);
    let tracking_store_for_asserts = tracking_store.clone();

    let fast_spec = FilesystemSpec {
        content_path: make_temp_path("cas_content"),
        temp_path: make_temp_path("cas_temp"),
        eviction_policy: None,
        ..Default::default()
    };
    let fast_store: Arc<FilesystemStore> = FilesystemStore::new(&fast_spec).await.unwrap();
    let cas_store = FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Filesystem(fast_spec),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
            bypass_dedup_threshold_bytes: 0,
        },
        Store::new(fast_store),
        Store::new(tracking_store),
    );

    let cache = DirectoryCache::new(
        DirectoryCacheConfig {
            cache_root: make_temp_path("directory_cache").into(),
            ..Default::default()
        },
        cas_store,
    )
    .await?;

    let working_directory = PathBuf::from(make_temp_path("working_directory"));
    let hit = cache
        .get_or_create(
            root_digest,
            working_directory.join(Uuid::new_v4().to_string()).as_path(),
        )
        .await?;
    assert!(!hit, "first call must be a cache miss");

    let max_in_flight = tracking_store_for_asserts
        .max_in_flight
        .load(Ordering::SeqCst);
    assert!(
        max_in_flight > 1,
        "expected concurrent slow-store fetches during cold construction, \
         but max in-flight was {max_in_flight}"
    );
    Ok(())
}
