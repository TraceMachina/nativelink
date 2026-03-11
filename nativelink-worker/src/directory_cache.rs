// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::future::Future;
use core::pin::Pin;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Instant, SystemTime};

use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_proto::build::bazel::remote::execution::v2::{
    Directory as ProtoDirectory, DirectoryNode, FileNode, SymlinkNode,
};
use nativelink_store::ac_utils::get_and_decode_digest;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::filesystem_store::{FileEntry, FilesystemStore};
use nativelink_util::common::DigestInfo;
use nativelink_util::fs_util::hardlink_directory_tree;
use nativelink_util::store_trait::{Store, StoreKey, StoreLike};
use tokio::fs;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, trace, warn};

/// Name of the merkle tree metadata file stored alongside each cached directory.
const MERKLE_METADATA_FILENAME: &str = ".merkle_tree_meta";

/// Cache format version file. Bump when the on-disk format changes in a way
/// that makes old entries invalid (e.g., permission semantics). On startup,
/// if the version file is missing or stale, the entire cache is wiped.
const CACHE_VERSION_FILENAME: &str = ".cache_version";
/// Bump this when the cache format changes.
const CACHE_FORMAT_VERSION: u32 = 2;

/// Merkle tree metadata for a cached directory entry.
///
/// Stores the mapping from each directory digest in the tree to its relative
/// path within the cached directory on disk. This allows us to index subtrees
/// so that future cache misses can reuse already-cached subtrees via symlinks.
#[derive(Debug, Clone)]
pub struct MerkleTreeMetadata {
    /// Map from directory digest -> relative path within the cache entry.
    /// For the root directory, the relative path is "" (empty string).
    pub digest_to_relpath: HashMap<DigestInfo, String>,
}

impl MerkleTreeMetadata {
    /// Serialize to a simple line-based text format:
    /// `hash:size_bytes:relative_path\n`
    fn serialize(&self) -> String {
        let mut lines = Vec::with_capacity(self.digest_to_relpath.len());
        for (digest, relpath) in &self.digest_to_relpath {
            lines.push(format!("{}:{}:{}", digest.packed_hash(), digest.size_bytes(), relpath));
        }
        // Sort for deterministic output
        lines.sort();
        lines.join("\n")
    }

    /// Deserialize from the line-based text format.
    fn deserialize(data: &str) -> Result<Self, Error> {
        let mut digest_to_relpath = HashMap::new();
        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Format: hash:size_bytes:relative_path
            // The relative path may contain colons, so split at most 3 parts.
            let mut parts = line.splitn(3, ':');
            let hash = parts.next().ok_or_else(|| {
                make_err!(Code::Internal, "Missing hash in merkle metadata line: {line}")
            })?;
            let size_str = parts.next().ok_or_else(|| {
                make_err!(Code::Internal, "Missing size in merkle metadata line: {line}")
            })?;
            let relpath = parts.next().unwrap_or("");

            let size: i64 = size_str.parse().map_err(|e| {
                make_err!(Code::Internal, "Invalid size in merkle metadata line: {line}: {e}")
            })?;

            let digest = DigestInfo::try_new(hash, size)
                .err_tip(|| format!("Invalid digest in merkle metadata line: {line}"))?;

            digest_to_relpath.insert(digest, relpath.to_string());
        }
        Ok(Self { digest_to_relpath })
    }

    /// Build merkle tree metadata by walking a resolved directory tree.
    ///
    /// `tree` is the map from digest -> Directory proto (as returned by
    /// `resolve_directory_tree`). `root_digest` is the root of the tree.
    ///
    /// Returns a mapping from each directory digest to its relative path
    /// within the cache entry (root = "").
    fn from_directory_tree(
        tree: &HashMap<DigestInfo, ProtoDirectory>,
        root_digest: &DigestInfo,
    ) -> Self {
        let mut digest_to_relpath = HashMap::with_capacity(tree.len());
        let mut queue = VecDeque::new();
        queue.push_back((*root_digest, String::new()));

        while let Some((digest, relpath)) = queue.pop_front() {
            if digest_to_relpath.contains_key(&digest) {
                continue; // Already visited (handles diamond dependencies)
            }
            digest_to_relpath.insert(digest, relpath.clone());

            if let Some(dir) = tree.get(&digest) {
                for subdir_node in &dir.directories {
                    if let Some(child_digest) = subdir_node
                        .digest
                        .as_ref()
                        .and_then(|d| DigestInfo::try_from(d).ok())
                    {
                        let child_relpath = if relpath.is_empty() {
                            subdir_node.name.clone()
                        } else {
                            format!("{}/{}", relpath, subdir_node.name)
                        };
                        queue.push_back((child_digest, child_relpath));
                    }
                }
            }
        }

        Self { digest_to_relpath }
    }
}

/// Configuration for the directory cache
#[derive(Debug, Clone)]
pub struct DirectoryCacheConfig {
    /// Maximum number of cached directories
    pub max_entries: usize,
    /// Maximum total size in bytes (0 = unlimited)
    pub max_size_bytes: u64,
    /// Base directory for cache storage
    pub cache_root: PathBuf,
}

impl Default for DirectoryCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 1000,
            max_size_bytes: 10 * 1024 * 1024 * 1024, // 10 GB
            cache_root: std::env::temp_dir().join("nativelink_directory_cache"),
        }
    }
}

/// Metadata for a cached directory.
///
/// `ref_count` and `last_access` use atomics so that the cache hit fast path
/// only needs a *read* lock on the cache HashMap (no write lock contention).
#[derive(Debug)]
struct CachedDirectoryMetadata {
    /// Path to the cached directory
    path: PathBuf,
    /// Size in bytes
    size: u64,
    /// Last access time as duration-since-EPOCH in millis (atomic for read-lock access)
    last_access_millis: AtomicU64,
    /// Reference count (number of active hardlink operations in flight)
    ref_count: AtomicUsize,
}

impl CachedDirectoryMetadata {
    fn touch(&self) {
        let millis = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        self.last_access_millis.store(millis, Ordering::Relaxed);
    }
}

/// High-performance directory cache that uses hardlinks to avoid repeated
/// directory reconstruction from the CAS.
///
/// When actions need input directories, instead of fetching and reconstructing
/// files from the CAS each time, we:
/// 1. Check if we've already constructed this exact directory (by digest)
/// 2. If yes, hardlink the entire tree to the action's workspace
/// 3. If no, construct it once and cache for future use
///
/// This dramatically reduces I/O and improves action startup time.
///
/// ## Security Note
///
/// Hardlinked files share inodes. If an action process has elevated privileges
/// (e.g. root, `CAP_DAC_OVERRIDE`), it can bypass read-only permissions and
/// modify cached files through the workspace hardlink, poisoning the cache for
/// subsequent actions. For multi-tenant clusters, consider running actions in
/// user namespaces or using copy-on-write (reflink) instead of hardlinks.
#[derive(Debug)]
pub struct DirectoryCache {
    /// Configuration
    config: DirectoryCacheConfig,
    /// Cache mapping digest -> metadata
    cache: Arc<RwLock<HashMap<DigestInfo, CachedDirectoryMetadata>>>,
    /// Per-digest construction locks to prevent stampedes.
    ///
    /// Protocol:
    /// 1. A task entering construction clones the `Arc<Mutex<()>>`, incrementing
    ///    strong_count to >= 2 (HashMap entry + task clone).
    /// 2. On completion, if strong_count == 2 and the entry is still *our* Arc
    ///    (checked via `Arc::ptr_eq`), no other task is waiting, so we remove it.
    /// 3. If another task is waiting (strong_count > 2), we leave cleanup to the
    ///    last finisher. The worst case of a missed cleanup is a stale empty Mutex
    ///    in the HashMap, which is harmless.
    construction_locks: Arc<Mutex<HashMap<DigestInfo, Arc<Mutex<()>>>>>,
    /// CAS store for fetching directories (used as fallback in construct_directory_impl)
    cas_store: Store,
    /// Concrete FastSlowStore for the fast `download_to_directory` path.
    /// When available, cache-miss construction uses batch RPCs instead of
    /// serial per-file fetches.
    fast_slow_store: Option<Arc<FastSlowStore>>,
    /// Concrete FilesystemStore (the fast store inside FastSlowStore).
    /// Required for hardlinking files from the CAS to the cache directory.
    filesystem_store: Option<Arc<FilesystemStore>>,
    /// Subtree index: maps each directory digest to its absolute path on disk
    /// within a cached entry. This allows partial reuse of cached subtrees
    /// when a new root digest is requested that shares subtrees with an
    /// already-cached root.
    ///
    /// Updated when cache entries are inserted or evicted.
    subtree_index: RwLock<HashMap<DigestInfo, PathBuf>>,
    /// Reference count for each subtree digest across all cached entries.
    /// When a digest's count drops to zero, it is truly removed and should
    /// be reported in the "removed" delta.
    subtree_refcount: RwLock<HashMap<DigestInfo, usize>>,
    /// Pending subtree digest changes since the last `take_pending_subtree_changes()` call.
    /// Protected by a Mutex for interior mutability from insertion/eviction paths.
    pending_subtree_changes: Mutex<PendingSubtreeChanges>,
    /// Cumulative hit count for stats logging
    hit_count: AtomicU64,
    /// Cumulative miss count for stats logging
    miss_count: AtomicU64,
    /// Cumulative subtree hit count for stats logging
    subtree_hit_count: AtomicU64,
}

/// Accumulated subtree digest changes between periodic reports.
#[derive(Debug, Default)]
pub struct PendingSubtreeChanges {
    /// Subtree digests added since last report.
    pub added: HashSet<DigestInfo>,
    /// Subtree digests removed since last report (only those no longer in ANY cached entry).
    pub removed: HashSet<DigestInfo>,
}

impl DirectoryCache {
    /// Creates a new `DirectoryCache`.
    ///
    /// If `fast_slow_store` is provided, cache-miss construction will use the
    /// fast batch `download_to_directory` path (GetTree + BatchReadBlobs +
    /// parallel hardlinks). Otherwise falls back to the serial
    /// `construct_directory_impl` method.
    pub async fn new(
        config: DirectoryCacheConfig,
        cas_store: Store,
        fast_slow_store: Option<Arc<FastSlowStore>>,
    ) -> Result<Self, Error> {
        // Ensure cache root exists
        fs::create_dir_all(&config.cache_root).await.err_tip(|| {
            format!(
                "Failed to create cache root: {}",
                config.cache_root.display()
            )
        })?;

        // Try to extract the FilesystemStore from the FastSlowStore if provided.
        let filesystem_store = fast_slow_store.as_ref().and_then(|fss| {
            fss.fast_store()
                .downcast_ref::<FilesystemStore>(None)
                .and_then(|fs| fs.get_arc())
        });

        let has_fast_path = fast_slow_store.is_some() && filesystem_store.is_some();

        if has_fast_path {
            info!(
                cache_root = %config.cache_root.display(),
                max_entries = config.max_entries,
                max_size_bytes = config.max_size_bytes,
                fast_path = true,
                "DirectoryCache initialized: using fast download_to_directory path for cache misses",
            );
        } else if fast_slow_store.is_some() {
            warn!(
                cache_root = %config.cache_root.display(),
                max_entries = config.max_entries,
                max_size_bytes = config.max_size_bytes,
                "DirectoryCache initialized: FastSlowStore provided but could not extract FilesystemStore; falling back to serial construction",
            );
        } else {
            info!(
                cache_root = %config.cache_root.display(),
                max_entries = config.max_entries,
                max_size_bytes = config.max_size_bytes,
                fast_path = false,
                "DirectoryCache initialized: no FastSlowStore, using serial construction",
            );
        }

        let mut initial_cache = HashMap::new();
        let mut initial_subtree_index = HashMap::new();
        let mut initial_subtree_refcount: HashMap<DigestInfo, usize> = HashMap::new();

        // Check cache format version. If stale or missing, wipe the cache.
        let version_path = config.cache_root.join(CACHE_VERSION_FILENAME);
        let version_ok = match fs::read_to_string(&version_path).await {
            Ok(v) => v.trim().parse::<u32>().ok() == Some(CACHE_FORMAT_VERSION),
            Err(_) => false,
        };
        if !version_ok {
            info!(
                expected = CACHE_FORMAT_VERSION,
                "DirectoryCache: format version mismatch, clearing stale entries",
            );
            if let Ok(mut entries) = fs::read_dir(&config.cache_root).await {
                while let Ok(Some(entry)) = entries.next_entry().await {
                    let p = entry.path();
                    // chmod +rw so we can delete read-only entries
                    drop(tokio::process::Command::new("chmod")
                        .args(["-R", "u+rw"])
                        .arg(&p)
                        .status()
                        .await);
                    drop(fs::remove_dir_all(&p).await);
                    drop(fs::remove_file(&p).await);
                }
            }
            fs::write(&version_path, format!("{CACHE_FORMAT_VERSION}\n"))
                .await
                .err_tip(|| "Failed to write cache version file")?;
        }

        // Load existing cache entries from disk on startup.
        let load_start = Instant::now();
        let mut loaded_count = 0u64;
        let mut loaded_subtrees = 0u64;
        let mut loaded_errors = 0u64;
        if let Ok(mut entries) = fs::read_dir(&config.cache_root).await {
            while let Ok(Some(entry)) = entries.next_entry().await {
                let entry_name = entry.file_name().to_string_lossy().to_string();
                // Skip temp directories and the merkle metadata files
                if entry_name.starts_with(".tmp-") || entry_name == MERKLE_METADATA_FILENAME {
                    continue;
                }
                let entry_path = entry.path();
                let Ok(metadata) = fs::symlink_metadata(&entry_path).await else {
                    continue;
                };
                if !metadata.is_dir() {
                    continue;
                }

                // Try to parse the entry name as a DigestInfo
                let Some(digest) = Self::parse_digest_from_dirname(&entry_name) else {
                    debug!(name = %entry_name, "Skipping non-digest cache directory entry");
                    continue;
                };

                // Calculate the directory size
                let size = match Self::set_readonly_and_calculate_size(&entry_path).await {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(
                            name = %entry_name,
                            ?e,
                            "Failed to calculate size for existing cache entry, skipping",
                        );
                        loaded_errors += 1;
                        continue;
                    }
                };

                // Load merkle tree metadata if available
                let merkle_path = entry_path.join(MERKLE_METADATA_FILENAME);
                if let Ok(data) = fs::read_to_string(&merkle_path).await {
                    match MerkleTreeMetadata::deserialize(&data) {
                        Ok(merkle) => {
                            for (sub_digest, relpath) in &merkle.digest_to_relpath {
                                let abs_path = if relpath.is_empty() {
                                    entry_path.clone()
                                } else {
                                    entry_path.join(relpath)
                                };
                                initial_subtree_index.insert(*sub_digest, abs_path);
                                *initial_subtree_refcount.entry(*sub_digest).or_insert(0) += 1;
                                loaded_subtrees += 1;
                            }
                        }
                        Err(e) => {
                            debug!(
                                name = %entry_name,
                                ?e,
                                "Failed to parse merkle metadata, subtrees won't be indexed",
                            );
                        }
                    }
                }

                let now_millis = SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;

                initial_cache.insert(
                    digest,
                    CachedDirectoryMetadata {
                        path: entry_path,
                        size,
                        last_access_millis: AtomicU64::new(now_millis),
                        ref_count: AtomicUsize::new(0),
                    },
                );
                loaded_count += 1;
            }
        }

        let load_elapsed = load_start.elapsed();
        if loaded_count > 0 || loaded_errors > 0 {
            info!(
                loaded_entries = loaded_count,
                loaded_subtrees,
                load_errors = loaded_errors,
                elapsed_ms = load_elapsed.as_millis() as u64,
                "DirectoryCache: loaded existing entries from disk on startup",
            );
        }

        Ok(Self {
            config,
            cache: Arc::new(RwLock::new(initial_cache)),
            construction_locks: Arc::new(Mutex::new(HashMap::new())),
            cas_store,
            fast_slow_store,
            filesystem_store,
            subtree_index: RwLock::new(initial_subtree_index),
            subtree_refcount: RwLock::new(initial_subtree_refcount),
            pending_subtree_changes: Mutex::new(PendingSubtreeChanges::default()),
            hit_count: AtomicU64::new(0),
            miss_count: AtomicU64::new(0),
            subtree_hit_count: AtomicU64::new(0),
        })
    }

    /// Returns the digests of all currently cached input root directories.
    /// The scheduler uses this to give routing preference to workers that
    /// already have an action's input_root_digest cached.
    pub async fn cached_digests(&self) -> Vec<DigestInfo> {
        let cache = self.cache.read().await;
        cache.keys().copied().collect()
    }

    /// Returns ALL subtree digests currently tracked across all cached entries.
    /// Used for the initial full snapshot on (re)connect.
    pub async fn all_subtree_digests(&self) -> Vec<DigestInfo> {
        let refcount = self.subtree_refcount.read().await;
        refcount.keys().copied().collect()
    }

    /// Atomically takes the pending subtree changes since the last call,
    /// returning (added, removed) digest lists and clearing the internal state.
    pub async fn take_pending_subtree_changes(&self) -> (Vec<DigestInfo>, Vec<DigestInfo>) {
        let mut pending = self.pending_subtree_changes.lock().await;
        let added: Vec<DigestInfo> = pending.added.drain().collect();
        let removed: Vec<DigestInfo> = pending.removed.drain().collect();
        (added, removed)
    }

    /// Records that subtree digests from a merkle tree were added (new cache entry).
    /// Increments refcounts and records newly-appearing digests in pending added.
    async fn record_subtree_insertion(&self, merkle: &MerkleTreeMetadata) {
        let mut refcount = self.subtree_refcount.write().await;
        let mut pending = self.pending_subtree_changes.lock().await;
        for sub_digest in merkle.digest_to_relpath.keys() {
            let count = refcount.entry(*sub_digest).or_insert(0);
            if *count == 0 {
                // This digest is newly appearing across all cached entries.
                pending.added.insert(*sub_digest);
                // If it was in the removed set (evicted then re-added before
                // the delta was taken), cancel it out.
                pending.removed.remove(sub_digest);
            }
            *count += 1;
        }
    }

    /// Records that subtree digests from a merkle tree were removed (evicted cache entry).
    /// Decrements refcounts and records fully-removed digests in pending removed.
    async fn record_subtree_removal(&self, merkle_digests: &[DigestInfo]) {
        let mut refcount = self.subtree_refcount.write().await;
        let mut pending = self.pending_subtree_changes.lock().await;
        for sub_digest in merkle_digests {
            if let Some(count) = refcount.get_mut(sub_digest) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    refcount.remove(sub_digest);
                    // This digest is no longer in ANY cached entry.
                    pending.removed.insert(*sub_digest);
                    // If it was in the added set (added then evicted before
                    // the delta was taken), cancel it out.
                    pending.added.remove(sub_digest);
                }
            }
        }
    }

    /// Gets or creates a directory in the cache, then hardlinks it to the destination.
    ///
    /// # Arguments
    /// * `digest` - Digest of the root Directory proto
    /// * `dest_path` - Where to hardlink/create the directory (may already exist)
    ///
    /// # Returns
    /// * `Ok(true)` - Cache hit (directory was hardlinked)
    /// * `Ok(false)` - Cache miss (directory was constructed and cached)
    /// * `Err` - Error during construction or hardlinking
    pub async fn get_or_create(&self, digest: DigestInfo, dest_path: &Path) -> Result<bool, Error> {
        let overall_start = Instant::now();

        // Fast path: check if already in cache (read lock only for the lookup)
        if self.try_hardlink_cached(&digest, dest_path).await? {
            let hits = self.hit_count.fetch_add(1, Ordering::Relaxed) + 1;
            let misses = self.miss_count.load(Ordering::Relaxed);
            let total = hits + misses;
            let hit_rate = if total > 0 { (hits as f64 / total as f64) * 100.0 } else { 0.0 };
            info!(
                hash = %&digest.packed_hash().to_string()[..12],
                elapsed_ms = overall_start.elapsed().as_millis() as u64,
                hits,
                misses,
                hit_rate = format!("{hit_rate:.1}%"),
                "DirectoryCache HIT (hardlinked from cache)",
            );
            return Ok(true);
        }

        let misses = self.miss_count.fetch_add(1, Ordering::Relaxed) + 1;
        let hits = self.hit_count.load(Ordering::Relaxed);
        let total = hits + misses;
        let hit_rate = if total > 0 { (hits as f64 / total as f64) * 100.0 } else { 0.0 };
        info!(
            hash = %&digest.packed_hash().to_string()[..12],
            size_bytes = digest.size_bytes(),
            hits,
            misses,
            hit_rate = format!("{hit_rate:.1}%"),
            has_fast_path = self.fast_slow_store.is_some() && self.filesystem_store.is_some(),
            "DirectoryCache MISS, starting construction",
        );

        // Get or create construction lock to prevent stampede
        let construction_lock = {
            let mut locks = self.construction_locks.lock().await;
            locks
                .entry(digest)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };

        // Only one task constructs at a time for this digest
        let _guard = construction_lock.lock().await;

        // Double-check after acquiring lock — another task may have just constructed it
        if self.try_hardlink_cached(&digest, dest_path).await? {
            self.cleanup_construction_lock(&digest, &construction_lock);
            return Ok(true);
        }

        // Construct in a temp path, rename to final path on success.
        // This prevents orphaned partial directories on failure.
        let cache_path = self.get_cache_path(&digest);
        let temp_path = self.config.cache_root.join(format!(
            ".tmp-{digest}-{}-{}",
            std::process::id(),
            self.next_temp_id(),
        ));

        // Clean up any stale temp path from a previous crashed attempt
        drop(fs::remove_dir_all(&temp_path).await);

        let construction_result: Result<u64, Error> = async {
            fs::create_dir_all(&temp_path).await.err_tip(|| {
                format!("Failed to create temp dir: {}", temp_path.display())
            })?;

            // Step 1: Resolve the merkle tree if we have a FastSlowStore.
            // This gives us the full directory tree structure, which we use for:
            //   (a) subtree matching against the subtree_index
            //   (b) storing merkle metadata alongside the cache entry
            let resolved_tree = if let Some(fss) = &self.fast_slow_store {
                match crate::running_actions_manager::resolve_directory_tree(fss, &digest).await {
                    Ok(tree) => Some(tree),
                    Err(e) => {
                        warn!(
                            hash = %&digest.packed_hash().to_string()[..12],
                            ?e,
                            "DirectoryCache: failed to resolve directory tree, skipping subtree matching",
                        );
                        None
                    }
                }
            } else {
                None
            };

            // Step 2: Check for cached subtrees and construct a partial build plan.
            // A "subtree hit" means a directory node in the requested tree is
            // already materialized on disk from a different cached root. We can
            // symlink to it instead of downloading.
            let subtree_hits: HashMap<DigestInfo, PathBuf> = if let Some(tree) = &resolved_tree {
                let index = self.subtree_index.read().await;
                let mut hits = HashMap::new();
                for dir_digest in tree.keys() {
                    // Don't count the root itself (that's a full cache hit, handled above)
                    if *dir_digest == digest {
                        continue;
                    }
                    if let Some(cached_path) = index.get(dir_digest) {
                        // Verify the cached path still exists on disk
                        if cached_path.exists() {
                            hits.insert(*dir_digest, cached_path.clone());
                        }
                    }
                }
                hits
            } else {
                HashMap::new()
            };

            if !subtree_hits.is_empty() {
                let subtree_count = subtree_hits.len();
                let total_dirs = resolved_tree.as_ref().map_or(0, |t| t.len());
                self.subtree_hit_count.fetch_add(subtree_count as u64, Ordering::Relaxed);
                info!(
                    hash = %&digest.packed_hash().to_string()[..12],
                    subtree_hits = subtree_count,
                    total_dirs,
                    "DirectoryCache: found cached subtrees, will symlink instead of downloading",
                );
            }

            // Step 3: Build the directory tree.
            // If we have subtree hits and a resolved tree, use subtree-aware
            // construction. Otherwise, fall back to full construction.
            if let Some(tree) = &resolved_tree {
                if !subtree_hits.is_empty() {
                    // Subtree-aware construction: walk the tree, symlink cached
                    // subtrees, and only download uncached portions.
                    self.construct_with_subtrees(
                        &digest,
                        tree,
                        &subtree_hits,
                        &temp_path,
                    )
                    .await
                    .err_tip(|| "Failed subtree-aware construction")?;
                } else {
                    // No subtree hits -- use fast download_to_directory if available.
                    self.construct_full(&digest, &temp_path).await
                        .err_tip(|| "Failed full construction")?;
                }
            } else {
                // No resolved tree -- use full construction.
                self.construct_full(&digest, &temp_path).await
                    .err_tip(|| "Failed full construction (no resolved tree)")?;
            }

            // Step 4: Store merkle tree metadata alongside the cache entry.
            if let Some(tree) = &resolved_tree {
                let merkle_meta = MerkleTreeMetadata::from_directory_tree(tree, &digest);
                let merkle_path = temp_path.join(MERKLE_METADATA_FILENAME);
                let serialized = merkle_meta.serialize();
                if let Err(e) = fs::write(&merkle_path, serialized.as_bytes()).await {
                    warn!(
                        hash = %&digest.packed_hash().to_string()[..12],
                        ?e,
                        "DirectoryCache: failed to write merkle metadata, subtrees won't be indexed",
                    );
                }
            }

            // Combined walk: set read-only permissions and calculate size in one pass.
            let readonly_start = Instant::now();
            let size = Self::set_readonly_and_calculate_size(&temp_path).await
                .err_tip(|| "Failed to set readonly and calculate size for cache directory")?;
            info!(
                hash = %&digest.packed_hash().to_string()[..12],
                size_bytes = size,
                size_mb = format!("{:.2}", size as f64 / (1024.0 * 1024.0)),
                elapsed_ms = readonly_start.elapsed().as_millis() as u64,
                "DirectoryCache: set_readonly_and_calculate_size completed",
            );
            // macOS requires the source directory to be writable for rename(2).
            // Temporarily restore write permission on the root, rename, then
            // lock it down again.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&temp_path).await
                    .err_tip(|| "Failed to get temp dir metadata before rename")?
                    .permissions();
                perms.set_mode(0o755);
                fs::set_permissions(&temp_path, perms).await
                    .err_tip(|| "Failed to make temp dir writable before rename")?;
            }
            fs::rename(&temp_path, &cache_path).await.err_tip(|| {
                format!(
                    "Failed to rename temp dir {} to cache path {}",
                    temp_path.display(),
                    cache_path.display()
                )
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs::metadata(&cache_path).await
                    .err_tip(|| "Failed to get cache dir metadata after rename")?
                    .permissions();
                perms.set_mode(0o555);
                fs::set_permissions(&cache_path, perms).await
                    .err_tip(|| "Failed to lock down cache dir after rename")?;
            }

            // Step 5: Update the subtree index with all directories from this entry,
            // and record the insertion for delta reporting.
            if let Some(tree) = &resolved_tree {
                let merkle_meta = MerkleTreeMetadata::from_directory_tree(tree, &digest);
                let mut index = self.subtree_index.write().await;
                for (sub_digest, relpath) in &merkle_meta.digest_to_relpath {
                    let abs_path = if relpath.is_empty() {
                        cache_path.clone()
                    } else {
                        cache_path.join(relpath)
                    };
                    index.insert(*sub_digest, abs_path);
                }
                drop(index);
                self.record_subtree_insertion(&merkle_meta).await;
            }

            Ok(size)
        }
        .await;

        let size = match construction_result {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    hash = %&digest.packed_hash().to_string()[..12],
                    ?e,
                    elapsed_ms = overall_start.elapsed().as_millis() as u64,
                    "DirectoryCache MISS construction FAILED",
                );
                Self::remove_readonly_dir(&temp_path).await;
                self.cleanup_construction_lock(&digest, &construction_lock);
                return Err(e);
            }
        };

        // Insert with ref_count=1 to prevent eviction during hardlink.
        // Collect eviction candidates while holding the lock, then delete outside.
        let (evicted_paths, cache_entries, cache_total_size) = {
            let mut cache = self.cache.write().await;
            let evicted = self.collect_evictions(size, &mut cache);
            cache.insert(
                digest,
                CachedDirectoryMetadata {
                    path: cache_path.clone(),
                    size,
                    last_access_millis: AtomicU64::new(
                        SystemTime::now()
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64,
                    ),
                    ref_count: AtomicUsize::new(1),
                },
            );
            let total_size: u64 = cache.values().map(|m| m.size).sum();
            (evicted, cache.len(), total_size)
        };

        info!(
            hash = %&digest.packed_hash().to_string()[..12],
            size_bytes = size,
            size_mb = format!("{:.2}", size as f64 / (1024.0 * 1024.0)),
            cache_entries,
            cache_total_size_mb = format!("{:.2}", cache_total_size as f64 / (1024.0 * 1024.0)),
            evicted_count = evicted_paths.len(),
            elapsed_ms = overall_start.elapsed().as_millis() as u64,
            "DirectoryCache MISS construction complete, inserted into cache",
        );

        // Delete evicted directories outside the lock.
        // Cached directories are read-only (0o555/0o444), so we must make them
        // writable before removal. Also clean up the subtree index.
        if !evicted_paths.is_empty() {
            let mut index = self.subtree_index.write().await;
            for path in &evicted_paths {
                self.remove_subtree_index_for_path(path, &mut index).await;
            }
            drop(index);
            for path in evicted_paths {
                Self::remove_readonly_dir(&path).await;
            }
        }

        // Hardlink to destination (safe — ref_count=1 prevents eviction)
        let hardlink_start = Instant::now();
        let hardlink_result = hardlink_directory_tree(&cache_path, dest_path).await;
        let hardlink_elapsed = hardlink_start.elapsed();

        // Decrement ref_count regardless of hardlink result
        {
            let cache = self.cache.read().await;
            if let Some(metadata) = cache.get(&digest) {
                metadata.ref_count.fetch_sub(1, Ordering::Relaxed);
            }
        }

        // Drop the construction lock guard before cleanup
        drop(_guard);
        self.cleanup_construction_lock(&digest, &construction_lock);

        match &hardlink_result {
            Ok(()) => {
                info!(
                    hash = %&digest.packed_hash().to_string()[..12],
                    hardlink_ms = hardlink_elapsed.as_millis() as u64,
                    total_ms = overall_start.elapsed().as_millis() as u64,
                    "DirectoryCache: hardlinked newly constructed directory to dest",
                );
            }
            Err(e) => {
                warn!(
                    hash = %&digest.packed_hash().to_string()[..12],
                    ?e,
                    hardlink_ms = hardlink_elapsed.as_millis() as u64,
                    "DirectoryCache: failed to hardlink newly constructed directory to dest",
                );
            }
        }

        hardlink_result.err_tip(|| "Failed to hardlink newly cached directory")?;

        Ok(false)
    }

    /// Attempts to hardlink a cached directory to dest, guarding eviction with ref_count.
    /// Returns `Ok(true)` on cache hit + successful hardlink, `Ok(false)` on cache miss
    /// or failed hardlink (caller should fall through to reconstruction).
    async fn try_hardlink_cached(
        &self,
        digest: &DigestInfo,
        dest_path: &Path,
    ) -> Result<bool, Error> {
        let (src_path, cached_size) = {
            // Read lock is sufficient — ref_count and last_access are atomic.
            let cache = self.cache.read().await;
            let Some(metadata) = cache.get(digest) else {
                debug!(
                    hash = %&digest.packed_hash().to_string()[..12],
                    "DirectoryCache: not in cache (miss)",
                );
                return Ok(false);
            };
            metadata.touch();
            metadata.ref_count.fetch_add(1, Ordering::Relaxed);
            (metadata.path.clone(), metadata.size)
        };

        debug!(
            hash = %&digest.packed_hash().to_string()[..12],
            cached_size_bytes = cached_size,
            "DirectoryCache: found in cache, hardlinking",
        );

        let hardlink_start = Instant::now();
        let result = hardlink_directory_tree(&src_path, dest_path).await;
        let hardlink_elapsed = hardlink_start.elapsed();

        // Always decrement ref_count
        {
            let cache = self.cache.read().await;
            if let Some(metadata) = cache.get(digest) {
                metadata.ref_count.fetch_sub(1, Ordering::Relaxed);
            }
        }

        match result {
            Ok(()) => {
                info!(
                    hash = %&digest.packed_hash().to_string()[..12],
                    cached_size_bytes = cached_size,
                    hardlink_ms = hardlink_elapsed.as_millis() as u64,
                    "DirectoryCache: hardlink from cache succeeded",
                );
                Ok(true)
            }
            Err(e) => {
                warn!(
                    hash = %&digest.packed_hash().to_string()[..12],
                    error = ?e,
                    hardlink_ms = hardlink_elapsed.as_millis() as u64,
                    "DirectoryCache: hardlink from cache FAILED, will reconstruct",
                );
                Ok(false)
            }
        }
    }

    /// Removes the construction lock entry if no other task is waiting on it.
    fn cleanup_construction_lock(&self, digest: &DigestInfo, lock: &Arc<Mutex<()>>) {
        // Acquire the outer mutex to make the check+remove atomic with respect
        // to new tasks cloning from the HashMap.
        if let Ok(mut locks) = self.construction_locks.try_lock() {
            // Only remove if the entry is still *our* lock (not a replacement)
            // and no other task is holding a clone.
            if let Some(existing) = locks.get(digest) {
                if Arc::ptr_eq(existing, lock) && Arc::strong_count(lock) <= 2 {
                    locks.remove(digest);
                }
            }
        }
    }

    /// Recursively removes a read-only directory by first restoring write permissions.
    async fn remove_readonly_dir(path: &Path) {
        // Make writable so remove_dir_all can delete contents
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = fs::symlink_metadata(path).await {
                if metadata.is_dir() {
                    drop(fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).await);
                    if let Ok(mut entries) = fs::read_dir(path).await {
                        while let Ok(Some(entry)) = entries.next_entry().await {
                            if let Ok(meta) = fs::symlink_metadata(entry.path()).await {
                                if meta.is_dir() {
                                    Box::pin(Self::remove_readonly_dir(&entry.path())).await;
                                } else if meta.is_file() {
                                    drop(fs::set_permissions(
                                        entry.path(),
                                        std::fs::Permissions::from_mode(0o644),
                                    )
                                    .await);
                                }
                            }
                        }
                    }
                }
            }
        }

        if let Err(e) = fs::remove_dir_all(path).await {
            warn!(path = ?path, error = ?e, "Failed to remove evicted directory from disk");
        }
    }

    /// Monotonically increasing counter for unique temp paths.
    fn next_temp_id(&self) -> u64 {
        use std::sync::atomic::AtomicU64 as StaticAtomicU64;
        static COUNTER: StaticAtomicU64 = StaticAtomicU64::new(0);
        COUNTER.fetch_add(1, Ordering::Relaxed)
    }

    /// Validates that a node name is a single safe path component.
    /// Rejects path separators, traversal components, empty names, and null bytes.
    fn validate_node_name(name: &str) -> Result<(), Error> {
        if name.is_empty()
            || name == "."
            || name == ".."
            || name.contains('/')
            || name.contains('\\')
            || name.contains('\0')
        {
            return Err(make_err!(
                Code::InvalidArgument,
                "Invalid node name in Directory proto: {:?}",
                name
            ));
        }
        Ok(())
    }

    /// Validates that a symlink target does not escape the workspace root.
    /// Rejects absolute paths. For relative paths, verifies the resolved path
    /// stays within the workspace by counting `..` components.
    fn validate_symlink_target(target: &str, depth: usize) -> Result<(), Error> {
        if target.is_empty() || target.contains('\0') {
            return Err(make_err!(
                Code::InvalidArgument,
                "Invalid symlink target: {:?}",
                target
            ));
        }

        // Reject absolute symlink targets
        if target.starts_with('/') || target.starts_with('\\') {
            return Err(make_err!(
                Code::InvalidArgument,
                "Absolute symlink target not allowed: {:?}",
                target
            ));
        }

        // Count net upward traversals. `depth` is how deep we are in the tree.
        let mut net_up: usize = 0;
        for component in target.split('/') {
            match component {
                ".." => {
                    net_up += 1;
                    if net_up > depth {
                        return Err(make_err!(
                            Code::InvalidArgument,
                            "Symlink target escapes workspace root: {:?}",
                            target
                        ));
                    }
                }
                "" | "." => {}
                _ => {
                    net_up = net_up.saturating_sub(1);
                }
            }
        }

        Ok(())
    }

    /// Walks a directory tree, setting all entries to read-only and computing
    /// the total file size in a single traversal (avoiding two separate walks).
    /// Directories are set to 0o555, files have write bits stripped.
    fn set_readonly_and_calculate_size<'a>(
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + 'a>> {
        Box::pin(async move {
            let metadata = fs::symlink_metadata(path)
                .await
                .err_tip(|| format!("Failed to get metadata for: {}", path.display()))?;

            // Skip symlinks -- do not follow them or change permissions.
            if metadata.is_symlink() {
                return Ok(0);
            }

            if metadata.is_dir() {
                let mut entries = fs::read_dir(path)
                    .await
                    .err_tip(|| format!("Failed to read directory: {}", path.display()))?;

                let mut total_size = 0u64;
                while let Some(entry) = entries
                    .next_entry()
                    .await
                    .err_tip(|| format!("Failed to get next entry in: {}", path.display()))?
                {
                    total_size += Self::set_readonly_and_calculate_size(&entry.path()).await?;
                }

                // Set directory to read-only (0o555) to protect cache integrity.
                // Since we use hardlinks (not symlinks), actions never access
                // cached directories directly — they get fresh writable copies.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = metadata.permissions();
                    perms.set_mode(0o555);
                    fs::set_permissions(path, perms)
                        .await
                        .err_tip(|| format!("Failed to set permissions for: {}", path.display()))?;
                }
                #[cfg(windows)]
                {
                    let mut perms = metadata.permissions();
                    perms.set_readonly(true);
                    fs::set_permissions(path, perms)
                        .await
                        .err_tip(|| format!("Failed to set permissions for: {}", path.display()))?;
                }

                Ok(total_size)
            } else if metadata.is_file() {
                let size = metadata.len();

                // Strip write bits only, preserving read+execute.
                // This avoids corrupting CAS inodes (hardlinks share the inode)
                // while correctly making cached files read-only.
                // 0o555 files (CAS default) stay 0o555 — no syscall needed.
                // 0o644 files (serial fallback) become 0o444.
                // 0o755 files (serial fallback executable) become 0o555.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let current_mode = metadata.permissions().mode() & 0o777;
                    let new_mode = current_mode & 0o555; // strip write bits
                    if new_mode != current_mode {
                        let mut perms = metadata.permissions();
                        perms.set_mode(new_mode);
                        fs::set_permissions(path, perms)
                            .await
                            .err_tip(|| format!("Failed to set permissions for: {}", path.display()))?;
                    }
                }
                #[cfg(windows)]
                {
                    let mut perms = metadata.permissions();
                    perms.set_readonly(true);
                    fs::set_permissions(path, perms)
                        .await
                        .err_tip(|| format!("Failed to set permissions for: {}", path.display()))?;
                }

                Ok(size)
            } else {
                Ok(0)
            }
        })
    }

    /// Full construction path: tries fast download_to_directory, falls back to serial.
    /// Used when there are no subtree hits.
    async fn construct_full(&self, digest: &DigestInfo, temp_path: &Path) -> Result<(), Error> {
        // Try the fast batch path first if concrete stores are available.
        let fast_path_result = if let (Some(fss), Some(_fs_store)) =
            (&self.fast_slow_store, &self.filesystem_store)
        {
            let fs_pin = Pin::new(
                fss.fast_store()
                    .downcast_ref::<FilesystemStore>(None)
                    .err_tip(|| "Could not downcast fast store to FilesystemStore")?,
            );
            let temp_str = temp_path.to_string_lossy().to_string();
            info!(
                hash = %&digest.packed_hash().to_string()[..12],
                "DirectoryCache: fast download_to_directory starting",
            );
            let construction_start = Instant::now();
            let result = crate::running_actions_manager::download_to_directory(
                fss, fs_pin, digest, &temp_str,
            )
            .await;
            let elapsed = construction_start.elapsed();
            match &result {
                Ok(()) => {
                    info!(
                        hash = %&digest.packed_hash().to_string()[..12],
                        elapsed_ms = elapsed.as_millis() as u64,
                        "DirectoryCache: fast download_to_directory completed",
                    );
                    Some(Ok(()))
                }
                Err(e) => {
                    warn!(
                        hash = %&digest.packed_hash().to_string()[..12],
                        ?e,
                        elapsed_ms = elapsed.as_millis() as u64,
                        "DirectoryCache: fast download_to_directory failed, trying serial fallback",
                    );
                    // Clean up the partial temp directory before fallback
                    drop(fs::remove_dir_all(temp_path).await);
                    drop(fs::create_dir_all(temp_path).await);
                    Some(Err(e.clone()))
                }
            }
        } else {
            None
        };

        // Use the fast path result, or fall back to serial construction.
        match fast_path_result {
            Some(Ok(())) => Ok(()),
            Some(Err(_)) | None => {
                if fast_path_result.is_none() {
                    info!(
                        hash = %&digest.packed_hash().to_string()[..12],
                        "DirectoryCache: using serial construct_directory_impl (no fast path available)",
                    );
                }
                let serial_start = Instant::now();
                self.construct_directory(*digest, temp_path).await
                    .err_tip(|| "Failed to construct directory for cache")?;
                info!(
                    hash = %&digest.packed_hash().to_string()[..12],
                    elapsed_ms = serial_start.elapsed().as_millis() as u64,
                    "DirectoryCache: serial construct_directory_impl completed",
                );
                Ok(())
            }
        }
    }

    /// Subtree-aware construction: walks the resolved directory tree, creates
    /// hardlinked subtrees for cached portions, and only downloads uncached
    /// portions via `download_to_directory` or serial fallback.
    ///
    /// Uses file hardlinks (creating fresh directories) rather than directory
    /// symlinks because Bazel actions create output directories inside the
    /// input tree — symlinks would mutate the cache.
    async fn construct_with_subtrees(
        &self,
        root_digest: &DigestInfo,
        tree: &HashMap<DigestInfo, ProtoDirectory>,
        subtree_hits: &HashMap<DigestInfo, PathBuf>,
        dest_path: &Path,
    ) -> Result<(), Error> {
        let construction_start = Instant::now();

        // BFS walk of the tree, creating directories and symlinks.
        // When we encounter a subtree hit, we create a directory symlink and
        // skip its entire subtree (no need to traverse children).
        let mut queue = VecDeque::new();
        queue.push_back((*root_digest, dest_path.to_path_buf()));

        let mut dirs_created = 0usize;
        let mut subtrees_linked = 0usize;
        let mut files_to_download = Vec::new();
        let mut symlinks_to_create: Vec<(String, PathBuf)> = Vec::new();

        while let Some((dir_digest, dir_path)) = queue.pop_front() {
            let directory = tree.get(&dir_digest).ok_or_else(|| {
                make_err!(
                    Code::Internal,
                    "Directory {:?} not found in resolved tree during subtree construction",
                    dir_digest
                )
            })?;

            // Process subdirectories
            for subdir_node in &directory.directories {
                Self::validate_node_name(&subdir_node.name)?;
                let child_digest: DigestInfo = subdir_node
                    .digest
                    .as_ref()
                    .ok_or_else(|| {
                        make_err!(Code::InvalidArgument, "Directory node missing digest")
                    })?
                    .try_into()
                    .err_tip(|| "Invalid directory digest in subtree construction")?;

                let child_path = dir_path.join(&subdir_node.name);

                if let Some(cached_path) = subtree_hits.get(&child_digest) {
                    // Subtree hit: hardlink files from cached subtree into
                    // fresh writable directories. We can't use directory symlinks
                    // because Bazel creates output directories inside the input
                    // tree, which would mutate the cache.
                    hardlink_directory_tree(cached_path, &child_path)
                        .await
                        .err_tip(|| format!(
                            "Failed to hardlink cached subtree from {} to {}",
                            cached_path.display(),
                            child_path.display(),
                        ))?;
                    subtrees_linked += 1;
                    debug!(
                        child_hash = %&child_digest.packed_hash().to_string()[..12],
                        src = %cached_path.display(),
                        dst = %child_path.display(),
                        "DirectoryCache: hardlinked cached subtree",
                    );
                    // Do NOT enqueue children -- the hardlink covers the entire subtree.
                } else {
                    // No subtree hit -- create the directory and recurse.
                    fs::create_dir_all(&child_path).await.err_tip(|| {
                        format!("Failed to create directory: {}", child_path.display())
                    })?;
                    dirs_created += 1;
                    queue.push_back((child_digest, child_path));
                }
            }

            // Collect files that need to be downloaded for this (non-symlinked) directory.
            for file_node in &directory.files {
                Self::validate_node_name(&file_node.name)?;
                let file_digest: DigestInfo = file_node
                    .digest
                    .as_ref()
                    .ok_or_else(|| {
                        make_err!(Code::InvalidArgument, "File node missing digest")
                    })?
                    .try_into()
                    .err_tip(|| "Invalid file digest in subtree construction")?;

                let file_path = dir_path.join(&file_node.name);
                files_to_download.push((file_digest, file_path, file_node.is_executable));
            }

            // Collect symlinks from the proto
            for symlink_node in &directory.symlinks {
                Self::validate_node_name(&symlink_node.name)?;
                let link_path = dir_path.join(&symlink_node.name);
                symlinks_to_create.push((symlink_node.target.clone(), link_path));
            }
        }

        info!(
            hash = %&root_digest.packed_hash().to_string()[..12],
            dirs_created,
            subtrees_linked,
            files_to_download = files_to_download.len(),
            symlinks = symlinks_to_create.len(),
            "DirectoryCache: subtree-aware construction plan",
        );

        // Create symlinks from the proto
        #[cfg(target_family = "unix")]
        for (target, link_path) in &symlinks_to_create {
            fs::symlink(target, link_path)
                .await
                .err_tip(|| format!("Failed to create symlink: {} -> {}", link_path.display(), target))?;
        }

        // Download uncached files.
        // If we have a FastSlowStore + FilesystemStore, use hardlinks from CAS.
        // Otherwise fall back to serial CAS fetch.
        if !files_to_download.is_empty() {
            if let (Some(fss), Some(_fs_store)) = (&self.fast_slow_store, &self.filesystem_store) {
                let fs_store_pin = Pin::new(
                    fss.fast_store()
                        .downcast_ref::<FilesystemStore>(None)
                        .err_tip(|| "Could not downcast fast store to FilesystemStore")?,
                );

                // Check which blobs are already in the fast store.
                let unique_digests: Vec<DigestInfo> = {
                    let mut seen = HashSet::new();
                    files_to_download
                        .iter()
                        .filter_map(|(d, _, _)| if seen.insert(*d) { Some(*d) } else { None })
                        .collect()
                };
                let store_keys: Vec<StoreKey<'_>> =
                    unique_digests.iter().map(|d| (*d).into()).collect();
                let mut has_results = vec![None; store_keys.len()];
                Pin::new(fss.fast_store())
                    .has_with_results(&store_keys, &mut has_results)
                    .await
                    .err_tip(|| "Batch has_with_results in subtree construction")?;

                // Populate missing blobs into the fast store.
                let missing: Vec<&DigestInfo> = unique_digests
                    .iter()
                    .zip(has_results.iter())
                    .filter_map(|(d, r)| if r.is_none() { Some(d) } else { None })
                    .collect();

                if !missing.is_empty() {
                    info!(
                        hash = %&root_digest.packed_hash().to_string()[..12],
                        missing = missing.len(),
                        "DirectoryCache: fetching missing blobs for uncached files",
                    );
                    for d in &missing {
                        let key: StoreKey<'_> = (**d).into();
                        fss.populate_fast_store(key).await
                            .err_tip(|| format!("Failed to populate fast store for {:?}", d))?;
                    }
                }

                // Hardlink files from the fast store to their destination paths.
                for (file_digest, file_path, is_executable) in &files_to_download {
                    let file_entry = fs_store_pin
                        .get_file_entry_for_digest(file_digest)
                        .await
                        .err_tip(|| format!("Getting file entry for {:?}", file_digest))?;
                    let dest = file_path.clone();
                    file_entry
                        .get_file_path_locked(|src_path| async move {
                            fs::hard_link(&src_path, &dest)
                                .await
                                .err_tip(|| format!(
                                    "Failed to hardlink {:?} to {}",
                                    src_path,
                                    dest.display(),
                                ))
                        })
                        .await?;

                    // Set executable permission if needed
                    #[cfg(unix)]
                    if *is_executable {
                        use std::os::unix::fs::PermissionsExt;
                        let meta = fs::metadata(&file_path).await
                            .err_tip(|| "Failed to get file metadata for exec bit")?;
                        let current_mode = meta.permissions().mode() & 0o777;
                        let new_mode = current_mode | 0o111;
                        if new_mode != current_mode {
                            let mut perms = meta.permissions();
                            perms.set_mode(new_mode);
                            fs::set_permissions(&file_path, perms).await
                                .err_tip(|| "Failed to set executable permission")?;
                        }
                    }
                }
            } else {
                // Serial fallback: fetch each file from CAS individually.
                for (file_digest, file_path, is_executable) in &files_to_download {
                    let data = self
                        .cas_store
                        .get_part_unchunked(StoreKey::Digest(*file_digest), 0, None)
                        .await
                        .err_tip(|| format!("Failed to fetch file: {}", file_path.display()))?;
                    fs::write(&file_path, data.as_ref())
                        .await
                        .err_tip(|| format!("Failed to write file: {}", file_path.display()))?;

                    #[cfg(unix)]
                    if *is_executable {
                        use std::os::unix::fs::PermissionsExt;
                        let mut perms = fs::metadata(&file_path).await
                            .err_tip(|| "Failed to get file metadata")?
                            .permissions();
                        perms.set_mode(0o755);
                        fs::set_permissions(&file_path, perms).await
                            .err_tip(|| "Failed to set file permissions")?;
                    }
                }
            }
        }

        let elapsed = construction_start.elapsed();
        info!(
            hash = %&root_digest.packed_hash().to_string()[..12],
            dirs_created,
            subtrees_linked,
            files_downloaded = files_to_download.len(),
            elapsed_ms = elapsed.as_millis() as u64,
            "DirectoryCache: subtree-aware construction completed",
        );

        Ok(())
    }

    /// Removes subtree index entries that belong to a given cache entry path.
    /// Loads the merkle metadata file from the cache entry to determine which
    /// digests to remove. Also decrements subtree refcounts and records
    /// fully-removed digests for delta reporting.
    async fn remove_subtree_index_for_path(
        &self,
        cache_entry_path: &Path,
        index: &mut HashMap<DigestInfo, PathBuf>,
    ) {
        let merkle_path = cache_entry_path.join(MERKLE_METADATA_FILENAME);
        if let Ok(data) = fs::read_to_string(&merkle_path).await {
            if let Ok(merkle) = MerkleTreeMetadata::deserialize(&data) {
                let mut removed = 0usize;
                let merkle_digests: Vec<DigestInfo> =
                    merkle.digest_to_relpath.keys().copied().collect();
                for (sub_digest, relpath) in &merkle.digest_to_relpath {
                    // Only remove if the index entry points to this specific cache entry.
                    let abs_path = if relpath.is_empty() {
                        cache_entry_path.to_path_buf()
                    } else {
                        cache_entry_path.join(relpath)
                    };
                    if let Some(existing) = index.get(sub_digest) {
                        if *existing == abs_path {
                            index.remove(sub_digest);
                            removed += 1;
                        }
                    }
                }
                // Record subtree removals for delta reporting.
                // This decrements refcounts and only marks digests as removed
                // when they are no longer present in ANY cached entry.
                self.record_subtree_removal(&merkle_digests).await;
                debug!(
                    path = %cache_entry_path.display(),
                    removed_subtrees = removed,
                    "DirectoryCache: cleaned up subtree index for evicted entry",
                );
            }
        }
    }

    /// Try to parse a directory entry name as a DigestInfo.
    /// Expected format is the same as `DigestInfo::to_string()`,
    /// i.e., `{hash}-{size_bytes}`.
    fn parse_digest_from_dirname(name: &str) -> Option<DigestInfo> {
        // DigestInfo::to_string() produces "{hash}-{size}", so split on the last '-'
        let last_dash = name.rfind('-')?;
        let hash = &name[..last_dash];
        let size_str = &name[last_dash + 1..];
        let size: i64 = size_str.parse().ok()?;
        DigestInfo::try_new(hash, size).ok()
    }

    /// Constructs a directory from the CAS at the given path.
    /// `depth` tracks nesting depth for symlink target validation.
    fn construct_directory_impl<'a>(
        &'a self,
        digest: DigestInfo,
        dest_path: &'a Path,
        depth: usize,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            debug!(?digest, ?dest_path, "Constructing directory");

            // Fetch the Directory proto
            let directory: ProtoDirectory = get_and_decode_digest(&self.cas_store, digest.into())
                .await
                .err_tip(|| format!("Failed to fetch directory digest: {digest:?}"))?;

            // Create the destination directory
            fs::create_dir_all(dest_path)
                .await
                .err_tip(|| format!("Failed to create directory: {}", dest_path.display()))?;

            // Process files
            for file in &directory.files {
                Self::validate_node_name(&file.name)?;
                self.create_file(dest_path, file).await?;
            }

            // Process subdirectories recursively
            for dir_node in &directory.directories {
                Self::validate_node_name(&dir_node.name)?;
                self.create_subdirectory(dest_path, dir_node, depth + 1)
                    .await?;
            }

            // Process symlinks
            for symlink in &directory.symlinks {
                Self::validate_node_name(&symlink.name)?;
                Self::validate_symlink_target(&symlink.target, depth)?;
                self.create_symlink(dest_path, symlink).await?;
            }

            Ok(())
        })
    }

    /// Constructs a directory from the CAS at the given path
    fn construct_directory<'a>(
        &'a self,
        digest: DigestInfo,
        dest_path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        self.construct_directory_impl(digest, dest_path, 0)
    }

    /// Creates a file from a `FileNode`
    async fn create_file(&self, parent: &Path, file_node: &FileNode) -> Result<(), Error> {
        let file_path = parent.join(&file_node.name);
        let digest = DigestInfo::try_from(
            file_node
                .digest
                .as_ref()
                .ok_or_else(|| make_err!(Code::InvalidArgument, "File node missing digest"))?
                .clone(),
        )
        .err_tip(|| "Invalid file digest")?;

        trace!(?file_path, ?digest, "Creating file");

        // Fetch file content from CAS
        let data = self
            .cas_store
            .get_part_unchunked(StoreKey::Digest(digest), 0, None)
            .await
            .err_tip(|| format!("Failed to fetch file: {}", file_path.display()))?;

        // Write to disk
        fs::write(&file_path, data.as_ref())
            .await
            .err_tip(|| format!("Failed to write file: {}", file_path.display()))?;

        // Set permissions
        #[cfg(unix)]
        if file_node.is_executable {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&file_path)
                .await
                .err_tip(|| "Failed to get file metadata")?
                .permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&file_path, perms)
                .await
                .err_tip(|| "Failed to set file permissions")?;
        }

        Ok(())
    }

    /// Creates a subdirectory from a `DirectoryNode`
    async fn create_subdirectory(
        &self,
        parent: &Path,
        dir_node: &DirectoryNode,
        depth: usize,
    ) -> Result<(), Error> {
        let dir_path = parent.join(&dir_node.name);
        let digest = DigestInfo::try_from(
            dir_node
                .digest
                .as_ref()
                .ok_or_else(|| {
                    make_err!(Code::InvalidArgument, "Directory node missing digest")
                })?
                .clone(),
        )
        .err_tip(|| "Invalid directory digest")?;

        trace!(?dir_path, ?digest, "Creating subdirectory");

        // Recursively construct subdirectory
        self.construct_directory_impl(digest, &dir_path, depth)
            .await
    }

    /// Creates a symlink from a `SymlinkNode`
    async fn create_symlink(&self, parent: &Path, symlink: &SymlinkNode) -> Result<(), Error> {
        let link_path = parent.join(&symlink.name);
        let target = Path::new(&symlink.target);

        trace!(?link_path, ?target, "Creating symlink");

        #[cfg(unix)]
        fs::symlink(&target, &link_path)
            .await
            .err_tip(|| format!("Failed to create symlink: {}", link_path.display()))?;

        #[cfg(windows)]
        {
            // On Windows, we need to know if target is a directory
            // For now, assume files (can be improved later)
            fs::symlink_file(&target, &link_path)
                .await
                .err_tip(|| format!("Failed to create symlink: {}", link_path.display()))?;
        }

        Ok(())
    }

    /// Collects entries to evict to make room for `incoming_size` bytes.
    /// Removes them from the HashMap and returns their paths for disk cleanup.
    /// This is called while holding the write lock; actual disk I/O happens after
    /// the lock is released.
    fn collect_evictions(
        &self,
        incoming_size: u64,
        cache: &mut HashMap<DigestInfo, CachedDirectoryMetadata>,
    ) -> Vec<PathBuf> {
        let mut evicted_paths = Vec::new();

        // Evict by entry count
        while cache.len() >= self.config.max_entries {
            if let Some((path, digest, size)) = self.evict_lru_entry(cache) {
                info!(
                    hash = %&digest.packed_hash().to_string()[..12],
                    size_bytes = size,
                    reason = "count_limit",
                    entries_remaining = cache.len(),
                    max_entries = self.config.max_entries,
                    "DirectoryCache: evicting entry",
                );
                evicted_paths.push(path);
            } else {
                warn!(
                    entries = cache.len(),
                    max = self.config.max_entries,
                    "DirectoryCache: over entry limit but all entries are in use"
                );
                break;
            }
        }

        // Evict by size
        if self.config.max_size_bytes > 0 {
            loop {
                let current_size: u64 = cache.values().map(|m| m.size).sum();
                if current_size + incoming_size <= self.config.max_size_bytes {
                    break;
                }
                if let Some((path, digest, size)) = self.evict_lru_entry(cache) {
                    info!(
                        hash = %&digest.packed_hash().to_string()[..12],
                        size_bytes = size,
                        size_freed_mb = format!("{:.2}", size as f64 / (1024.0 * 1024.0)),
                        reason = "size_limit",
                        entries_remaining = cache.len(),
                        current_total_mb = format!("{:.2}", cache.values().map(|m| m.size).sum::<u64>() as f64 / (1024.0 * 1024.0)),
                        max_size_mb = format!("{:.2}", self.config.max_size_bytes as f64 / (1024.0 * 1024.0)),
                        "DirectoryCache: evicting entry",
                    );
                    evicted_paths.push(path);
                } else {
                    warn!(
                        current_size = current_size + incoming_size,
                        max = self.config.max_size_bytes,
                        "DirectoryCache: over size limit but all entries are in use"
                    );
                    break;
                }
            }
        }

        evicted_paths
    }

    /// Removes the LRU entry with ref_count == 0 from the cache HashMap.
    /// Returns the evicted entry's (path, digest, size) for logging and disk
    /// cleanup, or `None` if no evictable entry exists.
    fn evict_lru_entry(
        &self,
        cache: &mut HashMap<DigestInfo, CachedDirectoryMetadata>,
    ) -> Option<(PathBuf, DigestInfo, u64)> {
        let to_evict = cache
            .iter()
            .filter(|(_, m)| m.ref_count.load(Ordering::Relaxed) == 0)
            .min_by_key(|(_, m)| m.last_access_millis.load(Ordering::Relaxed))
            .map(|(digest, _)| *digest);

        if let Some(digest) = to_evict {
            if let Some(metadata) = cache.remove(&digest) {
                return Some((metadata.path, digest, metadata.size));
            }
        }

        None
    }

    /// Gets the cache path for a digest
    fn get_cache_path(&self, digest: &DigestInfo) -> PathBuf {
        self.config.cache_root.join(digest.to_string())
    }

    /// Returns cache statistics
    pub async fn stats(&self) -> CacheStats {
        let cache = self.cache.read().await;
        let total_size: u64 = cache.values().map(|m| m.size).sum();
        let in_use = cache
            .values()
            .filter(|m| m.ref_count.load(Ordering::Relaxed) > 0)
            .count();

        CacheStats {
            entries: cache.len(),
            total_size_bytes: total_size,
            in_use_entries: in_use,
        }
    }
}

/// Statistics about the directory cache
#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    pub entries: usize,
    pub total_size_bytes: u64,
    pub in_use_entries: usize,
}

#[cfg(test)]
mod tests {
    use nativelink_config::stores::MemorySpec;
    use nativelink_macro::nativelink_test;
    use nativelink_store::memory_store::MemoryStore;
    use nativelink_util::common::DigestInfo;
    use nativelink_util::store_trait::StoreLike;
    use prost::Message;
    use tempfile::TempDir;

    use super::*;

    async fn setup_test_store() -> (Store, DigestInfo) {
        let store = Store::new(MemoryStore::new(&MemorySpec::default()));

        // Create a simple directory structure
        let file_content = b"Hello, World!";
        // SHA256 hash of "Hello, World!"
        let file_digest = DigestInfo::try_new(
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f",
            13,
        )
        .unwrap();

        // Upload file
        store
            .as_store_driver_pin()
            .update_oneshot(file_digest.into(), file_content.to_vec().into())
            .await
            .unwrap();

        // Create Directory proto
        let directory = ProtoDirectory {
            files: vec![FileNode {
                name: "test.txt".to_string(),
                digest: Some(file_digest.into()),
                is_executable: false,
                ..Default::default()
            }],
            directories: vec![],
            symlinks: vec![],
            ..Default::default()
        };

        // Encode and upload directory
        let mut dir_data = Vec::new();
        directory.encode(&mut dir_data).unwrap();
        // Use a fixed hash for the directory
        let dir_digest = DigestInfo::try_new(
            "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            dir_data.len() as i64,
        )
        .unwrap();

        store
            .as_store_driver_pin()
            .update_oneshot(dir_digest.into(), dir_data.into())
            .await
            .unwrap();

        (store, dir_digest)
    }

    /// Creates a store with two different directory digests for eviction testing.
    async fn setup_two_digest_store() -> (Store, DigestInfo, DigestInfo) {
        let store = Store::new(MemoryStore::new(&Default::default()));

        // File A
        let content_a = b"File A content";
        let digest_a = DigestInfo::try_new(
            "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
            content_a.len() as i64,
        )
        .unwrap();
        store
            .as_store_driver_pin()
            .update_oneshot(digest_a.into(), content_a.to_vec().into())
            .await
            .unwrap();

        // Directory A
        let dir_a = ProtoDirectory {
            files: vec![FileNode {
                name: "a.txt".to_string(),
                digest: Some(digest_a.into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut dir_a_data = Vec::new();
        dir_a.encode(&mut dir_a_data).unwrap();
        let dir_digest_a = DigestInfo::try_new(
            "aaaa567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            dir_a_data.len() as i64,
        )
        .unwrap();
        store
            .as_store_driver_pin()
            .update_oneshot(dir_digest_a.into(), dir_a_data.into())
            .await
            .unwrap();

        // File B
        let content_b = b"File B content!!";
        let digest_b = DigestInfo::try_new(
            "b1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6b1b2",
            content_b.len() as i64,
        )
        .unwrap();
        store
            .as_store_driver_pin()
            .update_oneshot(digest_b.into(), content_b.to_vec().into())
            .await
            .unwrap();

        // Directory B
        let dir_b = ProtoDirectory {
            files: vec![FileNode {
                name: "b.txt".to_string(),
                digest: Some(digest_b.into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut dir_b_data = Vec::new();
        dir_b.encode(&mut dir_b_data).unwrap();
        let dir_digest_b = DigestInfo::try_new(
            "bbbb567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            dir_b_data.len() as i64,
        )
        .unwrap();
        store
            .as_store_driver_pin()
            .update_oneshot(dir_digest_b.into(), dir_b_data.into())
            .await
            .unwrap();

        (store, dir_digest_a, dir_digest_b)
    }

    #[nativelink_test]
    async fn test_directory_cache_basic() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store().await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };

        let cache = DirectoryCache::new(config, store, None).await?;

        // First access - cache miss
        let dest1 = temp_dir.path().join("dest1");
        let hit = cache.get_or_create(dir_digest, &dest1).await?;
        assert!(!hit, "First access should be cache miss");
        assert!(dest1.join("test.txt").exists());

        // Second access - cache hit
        let dest2 = temp_dir.path().join("dest2");
        let hit = cache.get_or_create(dir_digest, &dest2).await?;
        assert!(hit, "Second access should be cache hit");
        assert!(dest2.join("test.txt").exists());

        // Verify stats
        let stats = cache.stats().await;
        assert_eq!(stats.entries, 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_hardlink_into_existing_directory() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store().await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };

        let cache = DirectoryCache::new(config, store, None).await?;

        // Pre-create destination directory (simulates work_directory already existing)
        let dest = temp_dir.path().join("existing_dest");
        fs::create_dir(&dest).await.unwrap();

        // Should succeed even though dest already exists (Bug 1 fix)
        let hit = cache.get_or_create(dir_digest, &dest).await?;
        assert!(!hit, "First access should be cache miss");
        assert!(dest.join("test.txt").exists());

        // Cache hit into another pre-existing directory
        let dest2 = temp_dir.path().join("existing_dest2");
        fs::create_dir(&dest2).await.unwrap();
        let hit = cache.get_or_create(dir_digest, &dest2).await?;
        assert!(hit, "Second access should be cache hit");
        assert!(dest2.join("test.txt").exists());

        Ok(())
    }

    #[tokio::test]
    async fn test_construction_failure_cleanup() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");

        // Create a store with no data — construction will fail when fetching the digest
        let store = Store::new(MemoryStore::new(&Default::default()));

        let bogus_digest = DigestInfo::try_new(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            42,
        )
        .unwrap();

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root: cache_root.clone(),
        };

        let cache = DirectoryCache::new(config, store, None).await?;

        let dest = temp_dir.path().join("dest");
        let result = cache.get_or_create(bogus_digest, &dest).await;
        assert!(result.is_err(), "Should fail when digest not in store");

        // Bug 2 fix: No orphaned temp directories should remain
        let mut entries = fs::read_dir(&cache_root).await.unwrap();
        let mut leftover = Vec::new();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            leftover.push(entry.file_name().to_string_lossy().to_string());
        }
        assert!(
            leftover.is_empty(),
            "No orphaned temp dirs should remain in cache_root, found: {leftover:?}"
        );

        // Verify construction lock was cleaned up (Bug 3 fix)
        let locks = cache.construction_locks.lock().await;
        assert!(
            locks.is_empty(),
            "Construction lock should be cleaned up after failure"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_eviction_all_in_use() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store().await;

        let config = DirectoryCacheConfig {
            max_entries: 1,
            max_size_bytes: 0,
            cache_root,
        };

        let cache = DirectoryCache::new(config, store, None).await?;

        // Fill the cache
        let dest1 = temp_dir.path().join("dest1");
        cache.get_or_create(dir_digest, &dest1).await?;

        // Simulate all entries being in-use
        {
            let cache_map = cache.cache.read().await;
            if let Some(metadata) = cache_map.get(&dir_digest) {
                metadata.ref_count.store(1, Ordering::Relaxed);
            }
        }

        // Bug 4 fix: collect_evictions should not loop infinitely.
        {
            let mut cache_map = cache.cache.write().await;
            let evicted = cache.collect_evictions(100, &mut cache_map);
            assert!(evicted.is_empty(), "Nothing should be evictable");
            assert_eq!(cache_map.len(), 1, "Entry should still be present");
        }

        // Clean up ref_count
        {
            let cache_map = cache.cache.read().await;
            if let Some(metadata) = cache_map.get(&dir_digest) {
                metadata.ref_count.store(0, Ordering::Relaxed);
            }
        }

        Ok(())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_same_digest() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store().await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };

        let cache = Arc::new(DirectoryCache::new(config, store, None).await?);

        // Spawn multiple concurrent requests for the same digest
        let mut handles = Vec::new();
        for i in 0..5 {
            let cache = Arc::clone(&cache);
            let dest = temp_dir.path().join(format!("concurrent_dest_{i}"));
            handles.push(tokio::spawn(async move {
                cache.get_or_create(dir_digest, &dest).await
            }));
        }

        let mut hits = 0;
        let mut misses = 0;
        for handle in handles {
            let result = handle.await.unwrap()?;
            if result {
                hits += 1;
            } else {
                misses += 1;
            }
        }

        // Exactly one task should construct (miss), the rest should hit cache
        assert_eq!(misses, 1, "Exactly one task should construct the directory");
        assert_eq!(hits, 4, "Other tasks should get cache hits");

        // Verify only one cache entry exists
        let stats = cache.stats().await;
        assert_eq!(stats.entries, 1);
        assert_eq!(stats.in_use_entries, 0, "All ref_counts should be back to 0");

        // Verify construction locks are cleaned up (Bug 3)
        let locks = cache.construction_locks.lock().await;
        assert!(
            locks.is_empty(),
            "Construction locks should be cleaned up, found: {}",
            locks.len()
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_construction_lock_cleanup() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store().await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };

        let cache = DirectoryCache::new(config, store, None).await?;

        let dest = temp_dir.path().join("dest");
        cache.get_or_create(dir_digest, &dest).await?;

        let locks = cache.construction_locks.lock().await;
        assert!(
            locks.is_empty(),
            "Construction lock should be removed after get_or_create completes"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_eviction_removes_oldest_entry() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, digest_a, digest_b) = setup_two_digest_store().await;

        let config = DirectoryCacheConfig {
            max_entries: 1, // Only 1 entry allowed
            max_size_bytes: 0,
            cache_root: cache_root.clone(),
        };

        let cache = DirectoryCache::new(config, store, None).await?;

        // Insert entry A
        let dest_a = temp_dir.path().join("dest_a");
        cache.get_or_create(digest_a, &dest_a).await?;
        assert_eq!(cache.stats().await.entries, 1);

        // Insert entry B — should evict A
        let dest_b = temp_dir.path().join("dest_b");
        cache.get_or_create(digest_b, &dest_b).await?;
        assert_eq!(cache.stats().await.entries, 1);

        // A's cache directory should be gone from disk
        let cache_path_a = cache_root.join(digest_a.to_string());
        assert!(
            !cache_path_a.exists(),
            "Evicted entry A should be removed from disk"
        );

        // B should be in cache
        let cache_path_b = cache_root.join(digest_b.to_string());
        assert!(cache_path_b.exists(), "Entry B should be on disk");

        // Requesting A again should be a miss (reconstruct)
        let dest_a2 = temp_dir.path().join("dest_a2");
        let hit = cache.get_or_create(digest_a, &dest_a2).await?;
        assert!(!hit, "A should be a cache miss after eviction");
        assert!(dest_a2.join("a.txt").exists());

        Ok(())
    }

    #[tokio::test]
    async fn test_path_traversal_rejected() -> Result<(), Error> {
        // Test validate_node_name directly
        assert!(DirectoryCache::validate_node_name("good_file.txt").is_ok());
        assert!(DirectoryCache::validate_node_name("subdir").is_ok());

        // These should all be rejected
        assert!(DirectoryCache::validate_node_name("").is_err());
        assert!(DirectoryCache::validate_node_name(".").is_err());
        assert!(DirectoryCache::validate_node_name("..").is_err());
        assert!(DirectoryCache::validate_node_name("../etc/passwd").is_err());
        assert!(DirectoryCache::validate_node_name("/etc/passwd").is_err());
        assert!(DirectoryCache::validate_node_name("foo/bar").is_err());
        assert!(DirectoryCache::validate_node_name("foo\\bar").is_err());
        assert!(DirectoryCache::validate_node_name("foo\0bar").is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_symlink_target_validation() -> Result<(), Error> {
        // Valid relative targets
        assert!(DirectoryCache::validate_symlink_target("file.txt", 0).is_ok());
        assert!(DirectoryCache::validate_symlink_target("subdir/file.txt", 0).is_ok());
        assert!(DirectoryCache::validate_symlink_target("../sibling", 1).is_ok());

        // Absolute targets rejected
        assert!(DirectoryCache::validate_symlink_target("/etc/shadow", 0).is_err());
        assert!(DirectoryCache::validate_symlink_target("\\windows\\system32", 0).is_err());

        // Traversal beyond root rejected
        assert!(DirectoryCache::validate_symlink_target("..", 0).is_err());
        assert!(DirectoryCache::validate_symlink_target("../..", 1).is_err());
        assert!(DirectoryCache::validate_symlink_target("../../escape", 1).is_err());

        // Deep enough to allow traversal
        assert!(DirectoryCache::validate_symlink_target("../..", 2).is_ok());

        // Empty and null rejected
        assert!(DirectoryCache::validate_symlink_target("", 0).is_err());
        assert!(DirectoryCache::validate_symlink_target("foo\0bar", 0).is_err());

        Ok(())
    }

    #[tokio::test]
    async fn test_path_traversal_in_directory_proto() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let store = Store::new(MemoryStore::new(&Default::default()));

        // Create a malicious directory proto with a path-traversal file name
        let file_content = b"malicious";
        let file_digest = DigestInfo::try_new(
            "c0535e4be2b79ffd93291305436bf889314e4a3faec05ecffcbb7df31ad9e51a",
            9,
        )
        .unwrap();
        store
            .as_store_driver_pin()
            .update_oneshot(file_digest.into(), file_content.to_vec().into())
            .await
            .unwrap();

        let malicious_dir = ProtoDirectory {
            files: vec![FileNode {
                name: "../escape.txt".to_string(),
                digest: Some(file_digest.into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut dir_data = Vec::new();
        malicious_dir.encode(&mut dir_data).unwrap();
        let dir_digest = DigestInfo::try_new(
            "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            dir_data.len() as i64,
        )
        .unwrap();
        store
            .as_store_driver_pin()
            .update_oneshot(dir_digest.into(), dir_data.into())
            .await
            .unwrap();

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };
        let cache = DirectoryCache::new(config, store, None).await?;

        let dest = temp_dir.path().join("dest");
        let result = cache.get_or_create(dir_digest, &dest).await;
        assert!(result.is_err(), "Path traversal should be rejected");

        // The escape file should NOT exist in the parent directory
        assert!(
            !temp_dir.path().join("escape.txt").exists(),
            "Path traversal should not create files outside dest"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_absolute_symlink_rejected() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let store = Store::new(MemoryStore::new(&Default::default()));

        let malicious_dir = ProtoDirectory {
            symlinks: vec![SymlinkNode {
                name: "evil_link".to_string(),
                target: "/etc/shadow".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut dir_data = Vec::new();
        malicious_dir.encode(&mut dir_data).unwrap();
        let dir_digest = DigestInfo::try_new(
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
            dir_data.len() as i64,
        )
        .unwrap();
        store
            .as_store_driver_pin()
            .update_oneshot(dir_digest.into(), dir_data.into())
            .await
            .unwrap();

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };
        let cache = DirectoryCache::new(config, store, None).await?;

        let dest = temp_dir.path().join("dest");
        let result = cache.get_or_create(dir_digest, &dest).await;
        assert!(result.is_err(), "Absolute symlink target should be rejected");

        Ok(())
    }

    #[tokio::test]
    async fn test_ref_count_returns_to_zero_after_operations() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store().await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };

        let cache = DirectoryCache::new(config, store, None).await?;

        // Cache miss
        let dest1 = temp_dir.path().join("dest1");
        cache.get_or_create(dir_digest, &dest1).await?;

        // Cache hit
        let dest2 = temp_dir.path().join("dest2");
        cache.get_or_create(dir_digest, &dest2).await?;

        // ref_count should be 0 after both operations
        let stats = cache.stats().await;
        assert_eq!(stats.in_use_entries, 0, "ref_count should be 0 after all operations");

        Ok(())
    }

    #[tokio::test]
    async fn test_size_based_eviction() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, digest_a, digest_b) = setup_two_digest_store().await;

        let config = DirectoryCacheConfig {
            max_entries: 100,       // High entry limit
            max_size_bytes: 20,     // Very small — forces size-based eviction
            cache_root: cache_root.clone(),
        };

        let cache = DirectoryCache::new(config, store, None).await?;

        // Insert entry A (14 bytes for "File A content")
        let dest_a = temp_dir.path().join("dest_a");
        cache.get_or_create(digest_a, &dest_a).await?;
        assert_eq!(cache.stats().await.entries, 1);

        // Insert entry B (16 bytes for "File B content!!") — total would be 30 > 20,
        // so A should be evicted
        let dest_b = temp_dir.path().join("dest_b");
        cache.get_or_create(digest_b, &dest_b).await?;
        assert_eq!(cache.stats().await.entries, 1);

        // A should have been evicted
        let cache_map = cache.cache.read().await;
        assert!(
            !cache_map.contains_key(&digest_a),
            "Digest A should have been evicted due to size limit"
        );
        assert!(
            cache_map.contains_key(&digest_b),
            "Digest B should be present"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_merkle_tree_metadata_roundtrip() -> Result<(), Error> {
        // Test serialization/deserialization of MerkleTreeMetadata
        let mut digest_to_relpath = HashMap::new();
        let d1 = DigestInfo::try_new(
            "aaaa567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            100,
        )
        .unwrap();
        let d2 = DigestInfo::try_new(
            "bbbb567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            200,
        )
        .unwrap();

        digest_to_relpath.insert(d1, String::new()); // root
        digest_to_relpath.insert(d2, "subdir/nested".to_string());

        let meta = MerkleTreeMetadata { digest_to_relpath };
        let serialized = meta.serialize();
        let deserialized = MerkleTreeMetadata::deserialize(&serialized)?;

        assert_eq!(deserialized.digest_to_relpath.len(), 2);
        assert_eq!(deserialized.digest_to_relpath.get(&d1).unwrap(), "");
        assert_eq!(
            deserialized.digest_to_relpath.get(&d2).unwrap(),
            "subdir/nested"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_merkle_tree_metadata_from_directory_tree() -> Result<(), Error> {
        // Build a small directory tree and verify MerkleTreeMetadata generation
        let file_digest = DigestInfo::try_new(
            "dffd6021bb2bd5b0af676290809ec3a53191dd81c7f70a4b28688a362182986f",
            13,
        )
        .unwrap();

        // Child directory
        let child_dir = ProtoDirectory {
            files: vec![FileNode {
                name: "child_file.txt".to_string(),
                digest: Some(file_digest.into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut child_data = Vec::new();
        child_dir.encode(&mut child_data).unwrap();
        let child_digest = DigestInfo::try_new(
            "cccc567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            child_data.len() as i64,
        )
        .unwrap();

        // Root directory referencing the child
        let root_dir = ProtoDirectory {
            files: vec![FileNode {
                name: "root_file.txt".to_string(),
                digest: Some(file_digest.into()),
                ..Default::default()
            }],
            directories: vec![DirectoryNode {
                name: "child".to_string(),
                digest: Some(child_digest.into()),
            }],
            ..Default::default()
        };
        let mut root_data = Vec::new();
        root_dir.encode(&mut root_data).unwrap();
        let root_digest = DigestInfo::try_new(
            "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
            root_data.len() as i64,
        )
        .unwrap();

        let mut tree = HashMap::new();
        tree.insert(root_digest, root_dir);
        tree.insert(child_digest, child_dir);

        let meta = MerkleTreeMetadata::from_directory_tree(&tree, &root_digest);
        assert_eq!(meta.digest_to_relpath.len(), 2);
        assert_eq!(meta.digest_to_relpath.get(&root_digest).unwrap(), "");
        assert_eq!(meta.digest_to_relpath.get(&child_digest).unwrap(), "child");

        Ok(())
    }

    #[tokio::test]
    async fn test_parse_digest_from_dirname() -> Result<(), Error> {
        // Valid format: hash-size
        let name = "aaaa567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef-100";
        let parsed = DirectoryCache::parse_digest_from_dirname(name);
        assert!(parsed.is_some());
        let d = parsed.unwrap();
        assert_eq!(d.size_bytes(), 100);

        // Invalid: no dash
        assert!(DirectoryCache::parse_digest_from_dirname("nodashhere").is_none());

        // Invalid: not a number after dash
        assert!(DirectoryCache::parse_digest_from_dirname("hash-notanumber").is_none());

        // Invalid: empty
        assert!(DirectoryCache::parse_digest_from_dirname("").is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_merkle_metadata_stored_on_construction() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store().await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root: cache_root.clone(),
        };

        let cache = DirectoryCache::new(config, store, None).await?;

        // Construct a directory (serial path, no FastSlowStore)
        let dest = temp_dir.path().join("dest");
        cache.get_or_create(dir_digest, &dest).await?;

        // Merkle metadata file should NOT exist because we don't have
        // FastSlowStore (resolve_directory_tree requires it).
        // This is expected -- subtree indexing is only available with
        // the fast path.
        let cache_path = cache.get_cache_path(&dir_digest);
        let merkle_path = cache_path.join(MERKLE_METADATA_FILENAME);
        // Without FastSlowStore, no merkle metadata is generated
        assert!(
            !merkle_path.exists(),
            "Merkle metadata should not exist without FastSlowStore"
        );

        Ok(())
    }

    #[tokio::test]
    async fn test_subtree_index_populated_and_cleaned_on_eviction() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, digest_a, digest_b) = setup_two_digest_store().await;

        let config = DirectoryCacheConfig {
            max_entries: 1,
            max_size_bytes: 0,
            cache_root: cache_root.clone(),
        };

        let cache = DirectoryCache::new(config, store, None).await?;

        // Insert entry A
        let dest_a = temp_dir.path().join("dest_a");
        cache.get_or_create(digest_a, &dest_a).await?;

        // Without FastSlowStore, subtree index should be empty (no merkle tree resolved)
        {
            let index = cache.subtree_index.read().await;
            assert!(
                index.is_empty(),
                "Subtree index should be empty without FastSlowStore"
            );
        }

        // Insert entry B (evicts A)
        let dest_b = temp_dir.path().join("dest_b");
        cache.get_or_create(digest_b, &dest_b).await?;
        assert_eq!(cache.stats().await.entries, 1);

        Ok(())
    }

    #[tokio::test]
    async fn test_cache_reload_from_disk() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store().await;

        // Create a cache and populate it
        {
            let config = DirectoryCacheConfig {
                max_entries: 10,
                max_size_bytes: 1024 * 1024,
                cache_root: cache_root.clone(),
            };
            let cache = DirectoryCache::new(config, store.clone(), None).await?;
            let dest = temp_dir.path().join("dest1");
            cache.get_or_create(dir_digest, &dest).await?;
            assert_eq!(cache.stats().await.entries, 1);
        }

        // Create a NEW cache pointing to the same cache_root -- it should
        // reload the existing entry from disk.
        {
            let config = DirectoryCacheConfig {
                max_entries: 10,
                max_size_bytes: 1024 * 1024,
                cache_root: cache_root.clone(),
            };
            let cache = DirectoryCache::new(config, store, None).await?;
            assert_eq!(
                cache.stats().await.entries,
                1,
                "Cache should have reloaded the entry from disk"
            );

            // The reloaded entry should be usable (cache hit)
            let dest2 = temp_dir.path().join("dest2");
            let hit = cache.get_or_create(dir_digest, &dest2).await?;
            assert!(hit, "Reloaded entry should produce a cache hit");
            assert!(dest2.join("test.txt").exists());
        }

        Ok(())
    }
}
