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
use std::collections::HashMap;
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
use nativelink_store::filesystem_store::FilesystemStore;
use nativelink_util::common::DigestInfo;
use nativelink_util::fs_util::hardlink_directory_tree;
use nativelink_util::store_trait::{Store, StoreKey, StoreLike};
use tokio::fs;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, info, trace, warn};

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
    /// Cumulative hit count for stats logging
    hit_count: AtomicU64,
    /// Cumulative miss count for stats logging
    miss_count: AtomicU64,
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

        Ok(Self {
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
            construction_locks: Arc::new(Mutex::new(HashMap::new())),
            cas_store,
            fast_slow_store,
            filesystem_store,
            hit_count: AtomicU64::new(0),
            miss_count: AtomicU64::new(0),
        })
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
                fs::create_dir_all(&temp_path).await.err_tip(|| {
                    format!("Failed to create temp dir: {}", temp_path.display())
                })?;
                info!(
                    hash = %&digest.packed_hash().to_string()[..12],
                    "DirectoryCache: fast download_to_directory starting",
                );
                let construction_start = Instant::now();
                let result = crate::running_actions_manager::download_to_directory(
                    fss, fs_pin, &digest, &temp_str,
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
                        drop(fs::remove_dir_all(&temp_path).await);
                        Some(Err(e.clone()))
                    }
                }
            } else {
                None
            };

            // Use the fast path result, or fall back to serial construction.
            match fast_path_result {
                Some(Ok(())) => {
                    // Fast path succeeded -- directory is populated in temp_path
                }
                Some(Err(_)) | None => {
                    // Fall back to serial construct_directory_impl
                    if fast_path_result.is_none() {
                        info!(
                            hash = %&digest.packed_hash().to_string()[..12],
                            "DirectoryCache: using serial construct_directory_impl (no fast path available)",
                        );
                    }
                    let serial_start = Instant::now();
                    self.construct_directory(digest, &temp_path).await
                        .err_tip(|| "Failed to construct directory for cache")?;
                    info!(
                        hash = %&digest.packed_hash().to_string()[..12],
                        elapsed_ms = serial_start.elapsed().as_millis() as u64,
                        "DirectoryCache: serial construct_directory_impl completed",
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
            fs::rename(&temp_path, &cache_path).await.err_tip(|| {
                format!(
                    "Failed to rename temp dir {} to cache path {}",
                    temp_path.display(),
                    cache_path.display()
                )
            })?;
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
        // writable before removal.
        for path in evicted_paths {
            Self::remove_readonly_dir(&path).await;
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

                // Set directory to r-xr-xr-x (0o555)
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

                // Set file to r--r--r-- (0o444)
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let mut perms = metadata.permissions();
                    perms.set_mode(0o444);
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

                Ok(size)
            } else {
                Ok(0)
            }
        })
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
}
