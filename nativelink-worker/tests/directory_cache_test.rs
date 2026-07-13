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
use std::collections::HashMap;
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

/// SHA-256 of zero bytes, matching `cas_utils::ZERO_BYTE_DIGESTS`. Never
/// stored in the CAS; `DirectoryCache` writes such files directly.
fn empty_file_digest() -> DigestInfo {
    DigestInfo::try_new(
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        0,
    )
    .unwrap()
}

/// Expected on-disk state for a seeded tree: relative path -> (content,
/// `is_executable`), plus symlink targets.
#[derive(Debug, Default)]
struct ExpectedTree {
    files: HashMap<PathBuf, (Vec<u8>, bool)>,
    symlinks: HashMap<PathBuf, String>,
}

/// Deterministic synthetic digest for a stress-tree file. Shared digests are
/// identical across trees; unique ones are namespaced by `tree_id`.
fn stress_file_digest(tree_id: u32, idx: u32, shared: bool, content: &[u8]) -> DigestInfo {
    let mut hash = [0u8; 32];
    hash[0] = 1;
    hash[1..5].copy_from_slice(&(if shared { 0 } else { tree_id + 1 }).to_le_bytes());
    hash[5..9].copy_from_slice(&idx.to_le_bytes());
    hash[9] = u8::from(shared);
    DigestInfo::new(hash, content.len() as u64)
}

/// Builds a three-level tree (root -> `mid_*` -> `leaf_*` -> files) seeded
/// ONLY into the slow store, returning the root digest and the expected
/// on-disk layout. Per leaf: `files_per_leaf` regular files (every 5th shared
/// across trees, every 7th executable, `f % 11 == 3` empty via the zero-byte
/// digest) plus one symlink on unix.
///
/// With `mutate_one_file` set, the result is the "one changed file" variant
/// of the same tree: `mid_0/leaf_0/file_1` gets different content, so that
/// leaf, `mid_0`, and the root all get new digests while every other subtree
/// digest is byte-identical to the unmutated tree.
async fn seed_stress_tree(
    slow_store: &Arc<MemoryStore>,
    tree_id: u32,
    mid_dirs: u32,
    leaf_dirs: u32,
    files_per_leaf: u32,
    mutate_one_file: bool,
) -> Result<(DigestInfo, ExpectedTree), Error> {
    let mut expected = ExpectedTree::default();
    let mut dir_seq = 0u32;
    let mut mid_nodes = Vec::new();
    for m in 0..mid_dirs {
        let mut leaf_nodes = Vec::new();
        for l in 0..leaf_dirs {
            let mutated_leaf = mutate_one_file && m == 0 && l == 0;
            let mut file_nodes = Vec::new();
            let mut symlink_nodes = Vec::new();
            for f in 0..files_per_leaf {
                let idx = (m * leaf_dirs + l) * files_per_leaf + f;
                let shared = f % 5 == 0;
                let is_executable = f % 7 == 0;
                let empty = f % 11 == 3;
                let mutated_file = mutated_leaf && f == 1;
                let name = format!("file_{f}");
                let rel = PathBuf::from(format!("mid_{m}/leaf_{l}/{name}"));
                if empty {
                    // Zero-digest shortcut: never stored in the CAS, the
                    // cache writes such files directly.
                    expected.files.insert(rel, (Vec::new(), false));
                    file_nodes.push(FileNode {
                        name,
                        digest: Some(empty_file_digest().into()),
                        is_executable: false,
                        node_properties: None,
                    });
                    continue;
                }
                let content: Vec<u8> = if mutated_file {
                    b"MUTATED-".repeat(60)
                } else if shared {
                    format!("shared-{idx}").into_bytes().repeat(50)
                } else {
                    format!("tree{tree_id}-file{idx}-").into_bytes().repeat(40)
                };
                let digest = if mutated_file {
                    let mut hash = [0u8; 32];
                    hash[0] = 9;
                    DigestInfo::new(hash, content.len() as u64)
                } else {
                    stress_file_digest(tree_id, idx, shared, &content)
                };
                slow_store
                    .update_oneshot(digest, Bytes::from(content.clone()))
                    .await?;
                expected.files.insert(rel, (content, is_executable));
                file_nodes.push(FileNode {
                    name,
                    digest: Some(digest.into()),
                    is_executable,
                    node_properties: None,
                });
            }
            // One symlink per leaf pointing at its first file (unix only:
            // Windows CI runners lack symlink privileges).
            if cfg!(unix) {
                symlink_nodes.push(SymlinkNode {
                    name: "link_to_first".to_string(),
                    target: "file_0".to_string(),
                    node_properties: None,
                });
                expected.symlinks.insert(
                    PathBuf::from(format!("mid_{m}/leaf_{l}/link_to_first")),
                    "file_0".to_string(),
                );
            }
            let leaf = ProtoDirectory {
                files: file_nodes,
                directories: vec![],
                symlinks: symlink_nodes,
                node_properties: None,
            }
            .encode_to_vec();
            let mut hash = [0u8; 32];
            if mutated_leaf {
                hash[0] = 10;
            } else {
                hash[0] = 2;
                hash[1..5].copy_from_slice(&(tree_id + 1).to_le_bytes());
                hash[5..9].copy_from_slice(&dir_seq.to_le_bytes());
            }
            dir_seq += 1;
            let digest = DigestInfo::new(hash, leaf.len() as u64);
            slow_store.update_oneshot(digest, Bytes::from(leaf)).await?;
            leaf_nodes.push(DirectoryNode {
                name: format!("leaf_{l}"),
                digest: Some(digest.into()),
            });
        }
        let mid = ProtoDirectory {
            files: vec![],
            directories: leaf_nodes,
            symlinks: vec![],
            node_properties: None,
        }
        .encode_to_vec();
        let mut hash = [0u8; 32];
        if mutate_one_file && m == 0 {
            hash[0] = 11;
        } else {
            hash[0] = 3;
            hash[1..5].copy_from_slice(&(tree_id + 1).to_le_bytes());
            hash[5..9].copy_from_slice(&dir_seq.to_le_bytes());
        }
        dir_seq += 1;
        let digest = DigestInfo::new(hash, mid.len() as u64);
        slow_store.update_oneshot(digest, Bytes::from(mid)).await?;
        mid_nodes.push(DirectoryNode {
            name: format!("mid_{m}"),
            digest: Some(digest.into()),
        });
    }
    let root = ProtoDirectory {
        files: vec![],
        directories: mid_nodes,
        symlinks: vec![],
        node_properties: None,
    }
    .encode_to_vec();
    let mut hash = [0u8; 32];
    hash[0] = if mutate_one_file { 12 } else { 4 };
    hash[1..5].copy_from_slice(&(tree_id + 1).to_le_bytes());
    let digest = DigestInfo::new(hash, root.len() as u64);
    slow_store.update_oneshot(digest, Bytes::from(root)).await?;
    Ok((digest, expected))
}

/// Asserts the materialized tree at `dest` matches `expected` byte-for-byte
/// (content, executable bits on unix, and symlink targets).
async fn verify_tree(dest: &Path, expected: &ExpectedTree) {
    for (rel, (content, is_executable)) in &expected.files {
        let path = dest.join(rel);
        let actual = tokio::fs::read(&path)
            .await
            .unwrap_or_else(|e| panic!("missing file {}: {e}", path.display()));
        assert_eq!(&actual, content, "content mismatch at {}", path.display());
        #[cfg(unix)]
        {
            if *is_executable {
                use std::os::unix::fs::PermissionsExt;
                let mode = tokio::fs::metadata(&path)
                    .await
                    .unwrap_or_else(|e| panic!("stat {}: {e}", path.display()))
                    .permissions()
                    .mode();
                assert_ne!(mode & 0o111, 0, "expected +x on {}", path.display());
            }
        }
        #[cfg(not(unix))]
        let _ = is_executable;
    }
    for (rel, target) in &expected.symlinks {
        let path = dest.join(rel);
        let actual = tokio::fs::read_link(&path)
            .await
            .unwrap_or_else(|e| panic!("missing symlink {}: {e}", path.display()));
        assert_eq!(
            actual.to_str(),
            Some(target.as_str()),
            "symlink target mismatch at {}",
            path.display()
        );
    }
}

/// Churn reuse: with `experimental_subtree_caching` enabled, constructing a
/// tree that differs from an already-cached tree by ONE file must reuse
/// every unchanged subtree via per-subtree hardlink hits, reconstructing
/// only the changed leaf and its ancestor mid directory.
#[nativelink_test]
async fn subtree_caching_reuses_unchanged_subtrees_on_churn() -> Result<(), Error> {
    const MID_DIRS: u32 = 4;
    const LEAF_DIRS: u32 = 3;
    const FILES_PER_LEAF: u32 = 5;

    let memory_store = MemoryStore::new(&MemorySpec::default());
    let (root_base, expected_base) =
        seed_stress_tree(&memory_store, 0, MID_DIRS, LEAF_DIRS, FILES_PER_LEAF, false).await?;
    let (root_mutant, expected_mutant) =
        seed_stress_tree(&memory_store, 0, MID_DIRS, LEAF_DIRS, FILES_PER_LEAF, true).await?;
    assert_ne!(
        root_base, root_mutant,
        "mutation must change the root digest"
    );

    let cache = DirectoryCache::new(
        DirectoryCacheConfig {
            cache_root: make_temp_path("directory_cache").into(),
            experimental_subtree_caching: true,
            ..Default::default()
        },
        make_cas_store(memory_store).await,
    )
    .await?;
    let working_directory = PathBuf::from(make_temp_path("working_directory"));
    tokio::fs::create_dir_all(&working_directory).await?;

    // Cold construction of the base tree: every subtree is a miss.
    let base_dest = working_directory.join("base");
    assert!(!cache.get_or_create(root_base, &base_dest).await?);
    verify_tree(&base_dest, &expected_base).await;
    let cold_stats = cache.stats().await;
    assert_eq!(
        cold_stats.subtree_hits, 0,
        "no subtree can hit on a cold cache"
    );
    assert_eq!(
        cold_stats.subtree_misses,
        u64::from(MID_DIRS * LEAF_DIRS + MID_DIRS),
        "every leaf and mid directory must be constructed exactly once"
    );

    // One-file-changed tree: only the changed leaf and its parent mid are
    // constructed; every other subtree hardlinks from the cache. Hits are
    // counted per hardlinked subtree ROOT: an unchanged mid is a single hit
    // that covers all of its leaves, so the expected count is
    // (unchanged mids) + (unchanged leaves inside the changed mid).
    let mutant_dest = working_directory.join("mutant");
    assert!(!cache.get_or_create(root_mutant, &mutant_dest).await?);
    verify_tree(&mutant_dest, &expected_mutant).await;
    let churn_stats = cache.stats().await;
    assert_eq!(
        churn_stats.subtree_hits - cold_stats.subtree_hits,
        u64::from((MID_DIRS - 1) + (LEAF_DIRS - 1)),
        "all unchanged subtrees must be cache hits"
    );
    assert_eq!(
        churn_stats.subtree_misses - cold_stats.subtree_misses,
        2,
        "only the changed leaf and the changed mid may be constructed"
    );

    // The changed file's bytes must be the mutant content, not the base's.
    assert_eq!(
        tokio::fs::read(mutant_dest.join("mid_0/leaf_0/file_1")).await?,
        b"MUTATED-".repeat(60),
        "changed file must have the mutated content"
    );
    Ok(())
}

/// Regression guard for the default: with `experimental_subtree_caching`
/// left off, the same base-then-mutant sequence must never touch the subtree
/// cache — the counters stay zero, only root entries exist, and the results
/// are still byte-correct.
#[nativelink_test]
async fn subtree_caching_disabled_by_default() -> Result<(), Error> {
    const MID_DIRS: u32 = 4;
    const LEAF_DIRS: u32 = 3;
    const FILES_PER_LEAF: u32 = 5;

    let memory_store = MemoryStore::new(&MemorySpec::default());
    let (root_base, expected_base) =
        seed_stress_tree(&memory_store, 0, MID_DIRS, LEAF_DIRS, FILES_PER_LEAF, false).await?;
    let (root_mutant, expected_mutant) =
        seed_stress_tree(&memory_store, 0, MID_DIRS, LEAF_DIRS, FILES_PER_LEAF, true).await?;

    let cache = DirectoryCache::new(
        DirectoryCacheConfig {
            cache_root: make_temp_path("directory_cache").into(),
            ..Default::default()
        },
        make_cas_store(memory_store).await,
    )
    .await?;
    let working_directory = PathBuf::from(make_temp_path("working_directory"));
    tokio::fs::create_dir_all(&working_directory).await?;

    let base_dest = working_directory.join("base");
    assert!(!cache.get_or_create(root_base, &base_dest).await?);
    verify_tree(&base_dest, &expected_base).await;

    let mutant_dest = working_directory.join("mutant");
    assert!(!cache.get_or_create(root_mutant, &mutant_dest).await?);
    verify_tree(&mutant_dest, &expected_mutant).await;

    let stats = cache.stats().await;
    assert_eq!(stats.subtree_hits, 0, "flag off: no subtree cache lookups");
    assert_eq!(
        stats.subtree_misses, 0,
        "flag off: no subtree cache constructions"
    );
    assert_eq!(stats.entries, 2, "flag off: only the two roots are cached");
    Ok(())
}

/// Cross-root sharing: two DIFFERENT roots containing a byte-identical
/// subtree (same `Directory` digest, even under different names) must share
/// one cached subtree entry — the second root's construction gets a subtree
/// hit instead of rebuilding it.
#[nativelink_test]
async fn subtree_caching_shares_subtree_across_roots() -> Result<(), Error> {
    let memory_store = MemoryStore::new(&MemorySpec::default());
    let shared_content = b"cross-root shared file".to_vec();
    let file_digest = DigestInfo::new([0x31u8; 32], shared_content.len() as u64);
    memory_store
        .update_oneshot(file_digest, Bytes::from(shared_content.clone()))
        .await?;
    let shared_dir = ProtoDirectory {
        files: vec![FileNode {
            name: "data.txt".to_string(),
            digest: Some(file_digest.into()),
            is_executable: false,
            node_properties: None,
        }],
        directories: vec![],
        symlinks: vec![],
        node_properties: None,
    }
    .encode_to_vec();
    let shared_dir_digest = DigestInfo::new([0x32u8; 32], shared_dir.len() as u64);
    memory_store
        .update_oneshot(shared_dir_digest, Bytes::from(shared_dir))
        .await?;

    let extra_content = b"only in the second root".to_vec();
    let extra_digest = DigestInfo::new([0x33u8; 32], extra_content.len() as u64);
    memory_store
        .update_oneshot(extra_digest, Bytes::from(extra_content.clone()))
        .await?;

    // The subtree name lives in the parent's DirectoryNode, not in the
    // subtree itself, so the two roots may even use different names.
    let root_one = ProtoDirectory {
        files: vec![],
        directories: vec![DirectoryNode {
            name: "sub".to_string(),
            digest: Some(shared_dir_digest.into()),
        }],
        symlinks: vec![],
        node_properties: None,
    }
    .encode_to_vec();
    let root_one_digest = DigestInfo::new([0x34u8; 32], root_one.len() as u64);
    memory_store
        .update_oneshot(root_one_digest, Bytes::from(root_one))
        .await?;
    let root_two = ProtoDirectory {
        files: vec![FileNode {
            name: "extra.txt".to_string(),
            digest: Some(extra_digest.into()),
            is_executable: false,
            node_properties: None,
        }],
        directories: vec![DirectoryNode {
            name: "renamed_sub".to_string(),
            digest: Some(shared_dir_digest.into()),
        }],
        symlinks: vec![],
        node_properties: None,
    }
    .encode_to_vec();
    let root_two_digest = DigestInfo::new([0x35u8; 32], root_two.len() as u64);
    memory_store
        .update_oneshot(root_two_digest, Bytes::from(root_two))
        .await?;

    let cache = DirectoryCache::new(
        DirectoryCacheConfig {
            cache_root: make_temp_path("directory_cache").into(),
            experimental_subtree_caching: true,
            ..Default::default()
        },
        make_cas_store(memory_store).await,
    )
    .await?;
    let working_directory = PathBuf::from(make_temp_path("working_directory"));
    tokio::fs::create_dir_all(&working_directory).await?;

    let dest_one = working_directory.join("root_one");
    assert!(!cache.get_or_create(root_one_digest, &dest_one).await?);
    let stats = cache.stats().await;
    assert_eq!(stats.subtree_misses, 1, "first root constructs the subtree");
    assert_eq!(stats.subtree_hits, 0);

    let dest_two = working_directory.join("root_two");
    assert!(
        !cache.get_or_create(root_two_digest, &dest_two).await?,
        "second root is itself a root-level miss"
    );
    let stats = cache.stats().await;
    assert_eq!(
        stats.subtree_hits, 1,
        "second root must reuse the identical subtree from the first root"
    );
    assert_eq!(stats.subtree_misses, 1, "the subtree is never rebuilt");
    assert_eq!(
        stats.entries, 3,
        "two roots plus one shared subtree are cached"
    );

    // Both materializations are byte-correct.
    assert_eq!(
        tokio::fs::read(dest_one.join("sub").join("data.txt")).await?,
        shared_content
    );
    assert_eq!(
        tokio::fs::read(dest_two.join("renamed_sub").join("data.txt")).await?,
        shared_content
    );
    assert_eq!(
        tokio::fs::read(dest_two.join("extra.txt")).await?,
        extra_content
    );
    Ok(())
}

/// Eviction safety: with `experimental_subtree_caching` enabled and
/// `max_entries` far below what a single tree needs, several trees
/// constructed concurrently must all materialize byte-correct results with
/// no errors — eviction may never delete an entry that a construction still
/// holds a `ref_count` pin on.
#[nativelink_test(flavor = "multi_thread", worker_threads = 4)]
async fn subtree_caching_eviction_safety_under_concurrency() -> Result<(), Error> {
    const NUM_TREES: u32 = 4;

    let memory_store = MemoryStore::new(&MemorySpec::default());
    let mut trees = Vec::new();
    for tree_id in 1..=NUM_TREES {
        trees.push(seed_stress_tree(&memory_store, tree_id, 2, 2, 4, false).await?);
    }

    let cache = Arc::new(
        DirectoryCache::new(
            DirectoryCacheConfig {
                // Far below the 7 entries (1 root + 2 mids + 4 leaves) a
                // single tree wants, so eviction races construction.
                max_entries: 3,
                cache_root: make_temp_path("directory_cache").into(),
                experimental_subtree_caching: true,
                ..Default::default()
            },
            make_cas_store(memory_store).await,
        )
        .await?,
    );
    let working_directory = PathBuf::from(make_temp_path("working_directory"));
    tokio::fs::create_dir_all(&working_directory).await?;

    let mut handles = Vec::new();
    for (i, (root, _)) in trees.iter().enumerate() {
        let cache = Arc::clone(&cache);
        let root = *root;
        let dest = working_directory.join(format!("tree_{i}"));
        handles.push(tokio::spawn(async move {
            cache.get_or_create(root, &dest).await
        }));
    }
    for handle in handles {
        handle.await.expect("construction task panicked")?;
    }
    for (i, (_, expected)) in trees.iter().enumerate() {
        verify_tree(&working_directory.join(format!("tree_{i}")), expected).await;
    }
    Ok(())
}

/// Seeds a minimal three-level tree `root -> mid -> leaf -> a.txt` with
/// digests derived from `tag`, returning the root digest, the leaf
/// `Directory` proto digest, and the file content.
async fn seed_simple_nested(
    slow_store: &Arc<MemoryStore>,
    tag: u8,
) -> Result<(DigestInfo, DigestInfo, Vec<u8>), Error> {
    let content = format!("nested-{tag}").into_bytes();
    let file_digest = DigestInfo::new([tag; 32], content.len() as u64);
    slow_store
        .update_oneshot(file_digest, Bytes::from(content.clone()))
        .await?;
    let leaf = ProtoDirectory {
        files: vec![FileNode {
            name: "a.txt".to_string(),
            digest: Some(file_digest.into()),
            is_executable: false,
            node_properties: None,
        }],
        directories: vec![],
        symlinks: vec![],
        node_properties: None,
    }
    .encode_to_vec();
    let leaf_digest = DigestInfo::new([tag + 1; 32], leaf.len() as u64);
    slow_store
        .update_oneshot(leaf_digest, Bytes::from(leaf))
        .await?;
    let mid = ProtoDirectory {
        files: vec![],
        directories: vec![DirectoryNode {
            name: "leaf".to_string(),
            digest: Some(leaf_digest.into()),
        }],
        symlinks: vec![],
        node_properties: None,
    }
    .encode_to_vec();
    let mid_digest = DigestInfo::new([tag + 2; 32], mid.len() as u64);
    slow_store
        .update_oneshot(mid_digest, Bytes::from(mid))
        .await?;
    let root = ProtoDirectory {
        files: vec![],
        directories: vec![DirectoryNode {
            name: "mid".to_string(),
            digest: Some(mid_digest.into()),
        }],
        symlinks: vec![],
        node_properties: None,
    }
    .encode_to_vec();
    let root_digest = DigestInfo::new([tag + 3; 32], root.len() as u64);
    slow_store
        .update_oneshot(root_digest, Bytes::from(root))
        .await?;
    Ok((root_digest, leaf_digest, content))
}

/// Asserts `cache_root` holds no in-progress temp trees (`.tmp-*`) or
/// undeleted tombstones that a failed construction could have leaked.
async fn assert_no_scratch_leftovers(cache_root: &Path) {
    let mut dir = tokio::fs::read_dir(cache_root)
        .await
        .expect("cache root must be readable");
    while let Some(entry) = dir.next_entry().await.expect("read_dir must not fail") {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        assert!(
            !name.starts_with(".tmp-"),
            "stale temp construction tree leaked: {name}"
        );
    }
}

/// `MemoryStore` wrapper that fails `get_part` for one digest a fixed number
/// of times, then behaves normally — a transient slow-store failure.
#[derive(Debug)]
struct FlakyStore {
    inner: Store,
    fail_digest: DigestInfo,
    failures_remaining: AtomicU64,
}

impl FlakyStore {
    fn new(inner: Arc<MemoryStore>, fail_digest: DigestInfo, failures: u64) -> Arc<Self> {
        Arc::new(Self {
            inner: Store::new(inner),
            fail_digest,
            failures_remaining: AtomicU64::new(failures),
        })
    }
}

impl MetricsComponent for FlakyStore {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        Ok(MetricPublishKnownKindData::Component)
    }
}

#[async_trait]
impl StoreDriver for FlakyStore {
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
        if let StoreKey::Digest(digest) = &key
            && *digest == self.fail_digest
            && self.failures_remaining.load(Ordering::SeqCst) > 0
        {
            self.failures_remaining.fetch_sub(1, Ordering::SeqCst);
            return Err(nativelink_error::make_err!(
                Code::Internal,
                "synthetic transient fetch failure"
            ));
        }
        self.inner
            .as_store_driver_pin()
            .get_part(key, writer, offset, length)
            .await
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

default_health_status_indicator!(FlakyStore);

/// Construction-failure recovery: a failed construction (transient slow-store
/// error) must leave NOTHING behind — no partial tree at the canonical
/// `cache_root/<digest>` path (which would poison every retry of that stable
/// digest with `AlreadyExists`) and no stale temp trees — and the retry must
/// succeed.
#[nativelink_test]
async fn construction_failure_does_not_poison_digest() -> Result<(), Error> {
    let memory_store = MemoryStore::new(&MemorySpec::default());
    let (root_digest, leaf_digest, content) = seed_simple_nested(&memory_store, 0x50).await?;
    // Fail the leaf's Directory-proto fetch exactly once.
    let flaky = FlakyStore::new(memory_store, leaf_digest, 1);

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
        Store::new(flaky),
    );

    let cache_root = PathBuf::from(make_temp_path("directory_cache"));
    let cache = DirectoryCache::new(
        DirectoryCacheConfig {
            cache_root: cache_root.clone(),
            experimental_subtree_caching: true,
            ..Default::default()
        },
        cas_store,
    )
    .await?;
    let working_directory = PathBuf::from(make_temp_path("working_directory"));
    tokio::fs::create_dir_all(&working_directory).await?;

    let result = cache
        .get_or_create(root_digest, &working_directory.join("first"))
        .await;
    assert!(result.is_err(), "first construction must fail");
    assert_no_scratch_leftovers(&cache_root).await;

    // Retry: no AlreadyExists poisoning from the failed attempt.
    let hit = cache
        .get_or_create(root_digest, &working_directory.join("second"))
        .await?;
    assert!(
        !hit,
        "retry is a plain miss that must construct successfully"
    );
    assert_eq!(
        tokio::fs::read(working_directory.join("second/mid/leaf/a.txt")).await?,
        content
    );
    assert_no_scratch_leftovers(&cache_root).await;
    Ok(())
}

/// Pin-leak / cancellation safety: a construction error makes
/// `buffer_unordered` drop sibling node futures mid-flight. The RAII
/// `EntryPin` guards must release their refcounts on drop, so no entry is
/// left permanently pinned (unevictable) and the cache stays fully usable.
#[nativelink_test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelled_sibling_constructions_leak_no_pins() -> Result<(), Error> {
    let memory_store = MemoryStore::new(&MemorySpec::default());
    let (root_x, leaf_x, content) = seed_simple_nested(&memory_store, 0x41).await?;

    // Tree Y reuses X's leaf and adds a subtree whose Directory proto is
    // deliberately absent from the CAS, so constructing Y always fails and
    // cancels the sibling (leaf) future mid-flight.
    let missing_digest = DigestInfo::new([0x66u8; 32], 4);
    let mid_y = ProtoDirectory {
        files: vec![],
        directories: vec![
            DirectoryNode {
                name: "leaf".to_string(),
                digest: Some(leaf_x.into()),
            },
            DirectoryNode {
                name: "missing".to_string(),
                digest: Some(missing_digest.into()),
            },
        ],
        symlinks: vec![],
        node_properties: None,
    }
    .encode_to_vec();
    let mid_y_digest = DigestInfo::new([0x67u8; 32], mid_y.len() as u64);
    memory_store
        .update_oneshot(mid_y_digest, Bytes::from(mid_y))
        .await?;
    let root_y = ProtoDirectory {
        files: vec![],
        directories: vec![DirectoryNode {
            name: "mid".to_string(),
            digest: Some(mid_y_digest.into()),
        }],
        symlinks: vec![],
        node_properties: None,
    }
    .encode_to_vec();
    let root_y_digest = DigestInfo::new([0x68u8; 32], root_y.len() as u64);
    memory_store
        .update_oneshot(root_y_digest, Bytes::from(root_y))
        .await?;

    let cache = DirectoryCache::new(
        DirectoryCacheConfig {
            // Small budget so leaked pins would visibly block eviction.
            max_entries: 2,
            cache_root: make_temp_path("directory_cache").into(),
            experimental_subtree_caching: true,
            ..Default::default()
        },
        make_cas_store(memory_store).await,
    )
    .await?;
    let working_directory = PathBuf::from(make_temp_path("working_directory"));
    tokio::fs::create_dir_all(&working_directory).await?;

    // Warm X so Y's shared leaf is (likely) served as a cache hit whose
    // hardlink is what gets cancelled.
    assert!(
        !cache
            .get_or_create(root_x, &working_directory.join("x"))
            .await?
    );
    assert_eq!(
        tokio::fs::read(working_directory.join("x/mid/leaf/a.txt")).await?,
        content
    );

    for i in 0..3 {
        let result = cache
            .get_or_create(root_y_digest, &working_directory.join(format!("y{i}")))
            .await;
        assert!(result.is_err(), "tree Y must fail to construct");
    }
    let stats = cache.stats().await;
    assert_eq!(
        stats.in_use_entries, 0,
        "cancelled/failed constructions must drain every refcount pin"
    );

    // Everything must still be evictable and the cache fully usable.
    let dest = working_directory.join("x_again");
    cache.get_or_create(root_x, &dest).await?;
    assert_eq!(tokio::fs::read(dest.join("mid/leaf/a.txt")).await?, content);
    let stats = cache.stats().await;
    assert_eq!(
        stats.in_use_entries, 0,
        "no pins may remain after completion"
    );
    assert!(
        stats.entries <= 2,
        "eviction must be able to keep the cache within max_entries"
    );
    Ok(())
}

/// Damaged-entry recovery: a cache entry whose on-disk tree was deleted out
/// from under the map (manual cleanup, disk tooling) must not fail its
/// digest forever. The hit path invalidates the dead entry, rebuilds it, and
/// returns byte-correct results; the rebuilt entry then serves normal hits.
/// Unix-only: the damage step deletes trees containing read-only files.
#[cfg(unix)]
#[nativelink_test]
async fn damaged_cache_entry_recovers() -> Result<(), Error> {
    let memory_store = MemoryStore::new(&MemorySpec::default());
    let (root_digest, _leaf_digest, content) = seed_simple_nested(&memory_store, 0x21).await?;
    let cache_root = PathBuf::from(make_temp_path("directory_cache"));
    let cache = DirectoryCache::new(
        DirectoryCacheConfig {
            cache_root: cache_root.clone(),
            experimental_subtree_caching: true,
            ..Default::default()
        },
        make_cas_store(memory_store).await,
    )
    .await?;
    let working_directory = PathBuf::from(make_temp_path("working_directory"));
    tokio::fs::create_dir_all(&working_directory).await?;

    assert!(
        !cache
            .get_or_create(root_digest, &working_directory.join("before"))
            .await?
    );

    // Damage: delete every entry tree on disk while the map still holds it.
    let mut dir = tokio::fs::read_dir(&cache_root).await?;
    while let Some(entry) = dir.next_entry().await? {
        tokio::fs::remove_dir_all(entry.path()).await?;
    }

    // Recovery: the hit path fails to hardlink, invalidates the dead
    // entries, and rebuilds them. Result must be byte-correct.
    cache
        .get_or_create(root_digest, &working_directory.join("after"))
        .await?;
    assert_eq!(
        tokio::fs::read(working_directory.join("after/mid/leaf/a.txt")).await?,
        content
    );

    // The rebuilt entry serves the next request as a normal hit.
    let hit = cache
        .get_or_create(root_digest, &working_directory.join("again"))
        .await?;
    assert!(hit, "rebuilt entry must serve as a cache hit");
    assert_eq!(
        tokio::fs::read(working_directory.join("again/mid/leaf/a.txt")).await?,
        content
    );
    Ok(())
}

/// Size accounting (flag on): an entry's recorded size covers only bytes not
/// owned by a descendant entry, so the cache's size total equals the tree's
/// unique byte total — not a depth-multiplied figure that would inflate
/// `max_size_bytes` pressure and self-defeat the cache via phantom
/// evictions.
#[nativelink_test]
async fn subtree_caching_size_accounting_not_depth_multiplied() -> Result<(), Error> {
    const MID_DIRS: u32 = 3;
    const LEAF_DIRS: u32 = 2;
    const FILES_PER_LEAF: u32 = 4;

    let memory_store = MemoryStore::new(&MemorySpec::default());
    let (root_digest, expected) =
        seed_stress_tree(&memory_store, 0, MID_DIRS, LEAF_DIRS, FILES_PER_LEAF, false).await?;
    let unique_bytes: u64 = expected
        .files
        .values()
        .map(|(content, _)| content.len() as u64)
        .sum();

    let cache = DirectoryCache::new(
        DirectoryCacheConfig {
            cache_root: make_temp_path("directory_cache").into(),
            experimental_subtree_caching: true,
            ..Default::default()
        },
        make_cas_store(memory_store).await,
    )
    .await?;
    let working_directory = PathBuf::from(make_temp_path("working_directory"));
    tokio::fs::create_dir_all(&working_directory).await?;

    assert!(
        !cache
            .get_or_create(root_digest, &working_directory.join("tree"))
            .await?
    );
    verify_tree(&working_directory.join("tree"), &expected).await;

    let stats = cache.stats().await;
    assert_eq!(
        stats.total_size_bytes, unique_bytes,
        "cache size total must equal the tree's unique bytes"
    );
    assert_eq!(
        stats.entries,
        (MID_DIRS * LEAF_DIRS + MID_DIRS + 1) as usize,
        "every subtree plus the root must have its own entry"
    );
    Ok(())
}
