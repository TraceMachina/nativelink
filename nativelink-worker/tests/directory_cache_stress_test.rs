// Stress/regression test for DirectoryCache concurrent construction.
// Exercises:
//  - single-flight stampede (many tasks requesting the same root digest)
//  - concurrent distinct constructions
//  - eviction churn racing construction (tiny max_entries)
//  - shared file digests across trees (hardlink inode sharing)
//  - executables (copy path), symlinks, and empty files (zero-digest path)
//  - jittered slow-tier latency; records peak global in-flight fetches
// Verifies every materialized tree byte-for-byte afterwards.

use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

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
use uuid::Uuid;

const BASE_RTT_MS: u64 = 5;
const JITTER_MS: u64 = 10;

#[derive(Debug)]
struct JitteredDelayStore {
    inner: Store,
    fetches: AtomicU64,
    in_flight: AtomicU64,
    peak_in_flight: AtomicU64,
}

impl JitteredDelayStore {
    fn new(inner: Arc<MemoryStore>) -> Arc<Self> {
        Arc::new(Self {
            inner: Store::new(inner),
            fetches: AtomicU64::new(0),
            in_flight: AtomicU64::new(0),
            peak_in_flight: AtomicU64::new(0),
        })
    }
}

impl MetricsComponent for JitteredDelayStore {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        Ok(MetricPublishKnownKindData::Component)
    }
}

#[async_trait]
impl StoreDriver for JitteredDelayStore {
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
        self.fetches.fetch_add(1, Ordering::SeqCst);
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak_in_flight.fetch_max(now, Ordering::SeqCst);
        // Deterministic per-key jitter.
        let jitter = match &key {
            StoreKey::Digest(d) => u64::from(d.packed_hash()[2]) % JITTER_MS,
            StoreKey::Str(_) => 0,
        };
        tokio::time::sleep(Duration::from_millis(BASE_RTT_MS + jitter)).await;
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

default_health_status_indicator!(JitteredDelayStore);

/// Expected on-disk state: relative path -> (content, `is_executable`) or symlink target.
#[derive(Debug, Default)]
struct ExpectedTree {
    files: HashMap<PathBuf, (Vec<u8>, bool)>,
    symlinks: HashMap<PathBuf, String>,
}

fn file_digest(tree_id: u32, idx: u32, shared: bool, content: &[u8]) -> DigestInfo {
    let mut hash = [0u8; 32];
    hash[0] = 1;
    // Shared digests are identical across trees; unique ones namespace by tree.
    hash[1..5].copy_from_slice(&(if shared { 0 } else { tree_id + 1 }).to_le_bytes());
    hash[5..9].copy_from_slice(&idx.to_le_bytes());
    hash[9] = u8::from(shared);
    DigestInfo::new(hash, content.len() as u64)
}

/// Builds one tree seeded ONLY into the slow store. Returns root digest and
/// the expected on-disk layout. Layout per mid dir: `files_per_leaf` regular
/// files (every 5th shared across trees, every 7th executable, every 11th
/// empty) plus one symlink.
async fn seed_stress_tree(
    slow_store: &Arc<MemoryStore>,
    tree_id: u32,
    mid_dirs: u32,
    leaf_dirs: u32,
    files_per_leaf: u32,
) -> Result<(DigestInfo, ExpectedTree), Error> {
    let mut expected = ExpectedTree::default();
    let mut dir_seq = 0u32;
    let mut mid_nodes = Vec::new();
    for m in 0..mid_dirs {
        let mut leaf_nodes = Vec::new();
        for l in 0..leaf_dirs {
            let mut file_nodes = Vec::new();
            let mut symlink_nodes = Vec::new();
            for f in 0..files_per_leaf {
                let idx = (m * leaf_dirs + l) * files_per_leaf + f;
                let shared = f % 5 == 0;
                let is_executable = f % 7 == 0;
                let empty = f % 11 == 3;
                let content: Vec<u8> = if empty {
                    Vec::new()
                } else if shared {
                    format!("shared-{idx}").into_bytes().repeat(50)
                } else {
                    format!("tree{tree_id}-file{idx}-").into_bytes().repeat(40)
                };
                let name = format!("file_{f}");
                let rel = PathBuf::from(format!("mid_{m}/leaf_{l}/{name}"));
                if empty {
                    // Zero-digest shortcut: never stored, written directly.
                    // (Sha256 hash of zero bytes, matching cas_utils::ZERO_BYTE_DIGESTS.)
                    let digest = DigestInfo::new(
                        [
                            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8,
                            0x99, 0x6f, 0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c,
                            0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55,
                        ],
                        0,
                    );
                    expected.files.insert(rel, (Vec::new(), false));
                    file_nodes.push(FileNode {
                        name,
                        digest: Some(digest.into()),
                        is_executable: false,
                        node_properties: None,
                    });
                    continue;
                }
                let digest = file_digest(tree_id, idx, shared, &content);
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
            hash[0] = 2;
            hash[1..5].copy_from_slice(&(tree_id + 1).to_le_bytes());
            hash[5..9].copy_from_slice(&dir_seq.to_le_bytes());
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
        hash[0] = 3;
        hash[1..5].copy_from_slice(&(tree_id + 1).to_le_bytes());
        hash[5..9].copy_from_slice(&dir_seq.to_le_bytes());
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
    hash[0] = 4;
    hash[1..5].copy_from_slice(&(tree_id + 1).to_le_bytes());
    let digest = DigestInfo::new(hash, root.len() as u64);
    slow_store.update_oneshot(digest, Bytes::from(root)).await?;
    Ok((digest, expected))
}

async fn verify_tree(dest: &Path, expected: &ExpectedTree) -> Result<(), String> {
    for (rel, (content, is_executable)) in &expected.files {
        let path = dest.join(rel);
        let actual = tokio::fs::read(&path)
            .await
            .map_err(|e| format!("missing file {}: {e}", path.display()))?;
        if &actual != content {
            return Err(format!(
                "content mismatch at {} ({} vs {} bytes)",
                path.display(),
                actual.len(),
                content.len()
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = tokio::fs::metadata(&path)
                .await
                .map_err(|e| format!("stat {}: {e}", path.display()))?
                .permissions()
                .mode();
            if *is_executable && mode & 0o111 == 0 {
                return Err(format!("expected +x on {}", path.display()));
            }
        }
        #[cfg(not(unix))]
        let _ = is_executable;
    }
    for (rel, target) in &expected.symlinks {
        let path = dest.join(rel);
        let actual = tokio::fs::read_link(&path)
            .await
            .map_err(|e| format!("missing symlink {}: {e}", path.display()))?;
        if actual.to_str() != Some(target.as_str()) {
            return Err(format!("symlink target mismatch at {}", path.display()));
        }
    }
    Ok(())
}

#[nativelink_test(flavor = "multi_thread", worker_threads = 8)]
async fn stress_concurrent_construction() -> Result<(), Error> {
    const NUM_TREES: u32 = 4;
    const MID_DIRS: u32 = 4;
    const LEAF_DIRS: u32 = 4;
    const FILES_PER_LEAF: u32 = 20; // 320 files/tree, 1280 total
    const CONCURRENT_TASKS: u32 = 24; // 6x duplication per tree -> stampede
    const MAX_ENTRIES: usize = 2; // half the trees -> constant eviction churn

    let memory_store = MemoryStore::new(&MemorySpec::default());
    let mut trees = Vec::new();
    for t in 0..NUM_TREES {
        trees.push(seed_stress_tree(&memory_store, t, MID_DIRS, LEAF_DIRS, FILES_PER_LEAF).await?);
    }
    let delay_store = JitteredDelayStore::new(memory_store);
    let stats = delay_store.clone();

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
        Store::new(delay_store),
    );

    let cache = Arc::new(
        DirectoryCache::new(
            DirectoryCacheConfig {
                cache_root: make_temp_path("directory_cache").into(),
                max_entries: MAX_ENTRIES,
                ..Default::default()
            },
            cas_store,
        )
        .await?,
    );

    let working_directory = PathBuf::from(make_temp_path("working_directory"));
    tokio::fs::create_dir_all(&working_directory).await?;

    let trees = Arc::new(trees);
    let start = Instant::now();
    let mut handles = Vec::new();
    for task in 0..CONCURRENT_TASKS {
        let cache = cache.clone();
        let trees = trees.clone();
        let dest = working_directory.join(format!("task_{task}_{}", Uuid::new_v4()));
        handles.push(tokio::spawn(async move {
            let (root_digest, _) = &trees[(task % NUM_TREES) as usize];
            let t0 = Instant::now();
            let result = cache.get_or_create(*root_digest, &dest).await;
            (task, dest, result, t0.elapsed())
        }));
    }

    let mut construct_times = Vec::new();
    let mut hits = 0u32;
    for handle in handles {
        let (task, dest, result, elapsed) = handle.await.expect("task panicked");
        let was_hit = result.unwrap_or_else(|e| panic!("task {task} failed: {e:?}"));
        if was_hit {
            hits += 1;
        }
        construct_times.push(elapsed.as_millis());
        let (_, expected) = &trees[(task % NUM_TREES) as usize];
        if let Err(msg) = verify_tree(&dest, expected).await {
            panic!("task {task} tree verification failed: {msg}");
        }
    }
    let wall = start.elapsed();

    construct_times.sort_unstable();
    let p50 = construct_times[construct_times.len() / 2];
    let max = construct_times.last().unwrap();
    eprintln!("=== STRESS RESULT ===");
    eprintln!(
        "tasks: {CONCURRENT_TASKS} over {NUM_TREES} trees ({} files/tree), max_entries={MAX_ENTRIES}",
        MID_DIRS * LEAF_DIRS * FILES_PER_LEAF
    );
    eprintln!(
        "wall: {}ms, per-task p50: {p50}ms, max: {max}ms, cache hits: {hits}/{CONCURRENT_TASKS}",
        wall.as_millis()
    );
    eprintln!(
        "slow-tier fetches: {}, peak in-flight: {}",
        stats.fetches.load(Ordering::SeqCst),
        stats.peak_in_flight.load(Ordering::SeqCst)
    );
    eprintln!("all {CONCURRENT_TASKS} trees byte-verified OK");
    assert!(
        stats.peak_in_flight.load(Ordering::SeqCst) <= 64,
        "aggregate slow-tier concurrency must be bounded by the cache-wide budget"
    );
    Ok(())
}
