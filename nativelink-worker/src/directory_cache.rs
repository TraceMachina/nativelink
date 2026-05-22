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
use core::sync::atomic::{AtomicU64, Ordering};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_proto::build::bazel::remote::execution::v2::{
    Directory as ProtoDirectory, DirectoryNode, FileNode, SymlinkNode,
};
use nativelink_store::ac_utils::get_and_decode_digest;
use nativelink_store::cas_utils::is_zero_digest;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::filesystem_store::{FileEntry, FilesystemStore};
use nativelink_util::background_spawn;
use nativelink_util::common::DigestInfo;
use nativelink_util::fs_util::{CloneMethod, hardlink_directory_tree, set_dir_writable_recursive};
use nativelink_util::store_trait::{StoreKey, StoreLike};
use tokio::fs;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, trace, warn};

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

/// Metadata for a cached directory
#[derive(Debug, Clone)]
struct CachedDirectoryMetadata {
    /// Path to the cached directory
    path: PathBuf,
    /// Size in bytes
    size: u64,
    /// Last access time for LRU eviction
    last_access: SystemTime,
    /// Reference count (number of active users)
    ref_count: usize,
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
#[derive(Debug)]
pub struct DirectoryCache {
    /// Configuration
    config: DirectoryCacheConfig,
    /// Cache mapping digest -> metadata
    cache: Arc<RwLock<HashMap<DigestInfo, CachedDirectoryMetadata>>>,
    /// Lock for cache construction to prevent stampedes
    construction_locks: Arc<Mutex<HashMap<DigestInfo, Arc<Mutex<()>>>>>,
    /// CAS store for fetching directory protos and (fallback) file content.
    cas_store: Arc<FastSlowStore>,
    /// The `FastSlowStore`'s fast tier, if it is a `FilesystemStore`. When
    /// present, CAS blobs can be hardlinked directly into the cache entry
    /// (zero-copy) instead of fetched into RAM and rewritten. When absent
    /// (e.g. an unusual store layout) the cache falls back to fetch+write.
    filesystem_store: Option<Arc<FilesystemStore>>,
    /// Count of materializations that used APFS `clonefile(2)` (macOS only;
    /// always zero on other platforms).
    clonefile_hits: AtomicU64,
    /// Count of materializations that used per-file `fs::hard_link`.
    hardlink_hits: AtomicU64,
}

impl DirectoryCache {
    /// Creates a new `DirectoryCache`.
    ///
    /// `cas_store` is the worker's `FastSlowStore`. Its fast tier is expected
    /// to be a `FilesystemStore`; when it is, `construct_directory` hardlinks
    /// CAS blobs directly into the cache entry instead of copying them.
    pub async fn new(
        config: DirectoryCacheConfig,
        cas_store: Arc<FastSlowStore>,
    ) -> Result<Self, Error> {
        // Ensure cache root exists
        fs::create_dir_all(&config.cache_root).await.err_tip(|| {
            format!(
                "Failed to create cache root: {}",
                config.cache_root.display()
            )
        })?;

        // Mirror RunningActionsManagerImpl: the fast tier is normally a
        // FilesystemStore. If the downcast fails the cache still works — it
        // just falls back to the fetch+write path for every file.
        let filesystem_store = cas_store
            .fast_store()
            .downcast_ref::<FilesystemStore>(None)
            .and_then(FilesystemStore::get_arc);
        if filesystem_store.is_none() {
            warn!(
                "DirectoryCache fast store is not a FilesystemStore; \
                 CAS blobs will be copied instead of hardlinked"
            );
        }

        Ok(Self {
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
            construction_locks: Arc::new(Mutex::new(HashMap::new())),
            cas_store,
            filesystem_store,
            clonefile_hits: AtomicU64::new(0),
            hardlink_hits: AtomicU64::new(0),
        })
    }

    /// Records which kernel mechanism materialized a tree, for observability.
    fn record_clone_method(&self, method: CloneMethod) {
        let counter = match method {
            CloneMethod::Clonefile => &self.clonefile_hits,
            CloneMethod::Hardlink => &self.hardlink_hits,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// Gets or creates a directory in the cache, then hardlinks it to the destination
    ///
    /// # Arguments
    /// * `digest` - Digest of the root Directory proto
    /// * `dest_path` - Where to hardlink/create the directory
    ///
    /// # Returns
    /// * `Ok(true)` - Cache hit (directory was hardlinked)
    /// * `Ok(false)` - Cache miss (directory was constructed)
    /// * `Err` - Error during construction or hardlinking
    pub async fn get_or_create(&self, digest: DigestInfo, dest_path: &Path) -> Result<bool, Error> {
        // Fast path: check if already in cache.
        //
        // The cache write lock is held only long enough to bump `ref_count`
        // and snapshot the entry's path — NOT across the syscall-heavy
        // `hardlink_directory_tree`. A global lock held across that
        // materialization serializes every concurrent `get_or_create`. The
        // `ref_count` bump is what makes releasing the lock safe: eviction
        // skips any entry with `ref_count > 0`, so the cache path cannot be
        // deleted out from under the unlocked materialization.
        if let Some(cache_path) = self.acquire_entry(&digest).await {
            debug!(?digest, ?cache_path, "Directory cache HIT");
            let result = hardlink_directory_tree(&cache_path, dest_path).await;
            self.release_entry(&digest).await;
            match result {
                Ok(method) => {
                    self.record_clone_method(method);
                    return Ok(true);
                }
                Err(e) => {
                    warn!(
                        ?digest,
                        error = ?e,
                        "Failed to hardlink from cache, will reconstruct"
                    );
                    // Fall through to reconstruction.
                }
            }
        }

        debug!(?digest, "Directory cache MISS");

        // Single-flight: only one task constructs a given digest at a time.
        // Concurrent callers for the same digest block on this per-digest
        // mutex; when the constructor finishes and inserts the entry, the
        // waiters wake, find it in the cache, and materialize their own
        // destination from the single shared cache entry.
        let construction_lock = {
            let mut locks = self.construction_locks.lock().await;
            locks
                .entry(digest)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = construction_lock.lock().await;

        // Run the construction/materialization under the per-digest guard,
        // then drop the per-digest mutex from the stampede map regardless of
        // outcome so it cannot grow unbounded. The guard (`_guard`) is still
        // held until the end of this function — `forget_construction_lock`
        // only unmaps the Arc; any waiter already cloned it before blocking.
        let result = self.construct_and_materialize(digest, dest_path).await;
        self.forget_construction_lock(&digest).await;
        result
    }

    /// The cache-miss body, run while holding the per-digest construction
    /// guard. Split out so `get_or_create` can unconditionally clean up the
    /// construction-lock map entry afterwards on every exit path.
    async fn construct_and_materialize(
        &self,
        digest: DigestInfo,
        dest_path: &Path,
    ) -> Result<bool, Error> {
        // Re-check: another task may have just constructed this digest while
        // we waited on the construction lock.
        if let Some(cache_path) = self.acquire_entry(&digest).await {
            let result = hardlink_directory_tree(&cache_path, dest_path).await;
            self.release_entry(&digest).await;
            match result {
                Ok(method) => {
                    self.record_clone_method(method);
                    return Ok(true);
                }
                Err(e) => {
                    warn!(
                        ?digest,
                        error = ?e,
                        "Failed to hardlink after construction"
                    );
                    // Construct directly at dest_path as a last resort.
                    self.construct_directory(digest, dest_path).await?;
                    return Ok(false);
                }
            }
        }

        // Construct the directory in cache. `construct_directory` returns the
        // total tree size accumulated from `FileNode.digest.size_bytes` as it
        // builds — no post-hoc filesystem walk is needed. It also sets every
        // cache-entry directory's mode at creation time (0o755), so no
        // separate permission-fixup walk is needed either.
        //
        // The cache entry's *files* are deliberately never chmod'd here:
        // non-executable files are hardlinks to FilesystemStore CAS blobs (see
        // `create_file`), and chmoding such a file mutates the inode shared
        // with the CAS and every other in-flight action that hardlinked the
        // same blob — the inode-corruption bug PR #2347 fixed.
        let cache_path = self.get_cache_path(&digest);
        let size = self.construct_directory(digest, &cache_path).await?;

        // Insert into the cache. Only the in-memory map mutation runs under
        // the write lock: `evict_if_needed` selects victims and removes them
        // from the map here, but their filesystem deletion is dispatched off
        // the lock so eviction I/O never serializes other callers.
        //
        // The new entry is inserted with `ref_count: 1` — pinned. The
        // hardlink-to-destination below runs unlocked, and a concurrent
        // `get_or_create` for an unrelated digest could otherwise pick this
        // brand-new, last-accessed-now entry as an eviction victim and delete
        // its tree mid-hardlink. The pin blocks that; `release_entry` drops it
        // to 0 once the hardlink is done.
        let evicted = {
            let mut cache = self.cache.write().await;
            let evicted = self.evict_if_needed(size, &mut cache);
            cache.insert(
                digest,
                CachedDirectoryMetadata {
                    path: cache_path.clone(),
                    size,
                    last_access: SystemTime::now(),
                    ref_count: 1,
                },
            );
            evicted
        };
        Self::dispatch_evictions(evicted);

        // Hardlink to destination (unlocked). The entry is pinned
        // (`ref_count == 1`) so it cannot be evicted from under this hardlink.
        let result = hardlink_directory_tree(&cache_path, dest_path).await;
        self.release_entry(&digest).await;
        let method = result.err_tip(|| "Failed to hardlink newly cached directory")?;
        self.record_clone_method(method);

        Ok(false)
    }

    /// If `digest` is cached, bumps its `ref_count` (pinning it against
    /// eviction) and returns a snapshot of its on-disk path. The cache write
    /// lock is released before returning, so the caller can perform unlocked
    /// I/O against the returned path. Every successful `acquire_entry` MUST be
    /// balanced by exactly one `release_entry`.
    async fn acquire_entry(&self, digest: &DigestInfo) -> Option<PathBuf> {
        let mut cache = self.cache.write().await;
        let metadata = cache.get_mut(digest)?;
        metadata.last_access = SystemTime::now();
        metadata.ref_count += 1;
        Some(metadata.path.clone())
    }

    /// Releases a pin taken by [`Self::acquire_entry`]. Tolerates the entry
    /// having been removed from the map in the interim (it cannot have been,
    /// because a non-zero `ref_count` blocks eviction, but be defensive).
    async fn release_entry(&self, digest: &DigestInfo) {
        let mut cache = self.cache.write().await;
        if let Some(metadata) = cache.get_mut(digest) {
            metadata.ref_count = metadata.ref_count.saturating_sub(1);
        }
    }

    /// Drops the per-digest construction mutex from the stampede map once
    /// construction (or the post-construction recheck) for `digest` is done.
    /// Without this the map grows unbounded over the worker's lifetime.
    ///
    /// Safe to call while holding the construction guard: a concurrent waiter
    /// already cloned the `Arc<Mutex>` before blocking, so removing the map
    /// entry only prevents *future* callers from joining this exact mutex —
    /// they will create a fresh one, re-check the cache, find the entry, and
    /// take the fast hardlink path. It never causes a redundant construct.
    async fn forget_construction_lock(&self, digest: &DigestInfo) {
        self.construction_locks.lock().await.remove(digest);
    }

    /// Constructs a directory from the CAS at the given path and returns the
    /// total size of the materialized tree in bytes.
    ///
    /// The size is accumulated from `FileNode.digest.size_bytes` in the
    /// `Directory` protos as the tree is built, rather than walking the
    /// filesystem afterwards with `fs::metadata` per file. Symlinks contribute
    /// nothing — a symlink's own inode is negligible and following it could
    /// double-count a file already counted via its `FileNode`.
    ///
    /// Each directory's final mode (0o755) is set at creation time, so no
    /// separate recursive permission pass is needed after construction.
    fn construct_directory<'a>(
        &'a self,
        digest: DigestInfo,
        dest_path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + 'a>> {
        Box::pin(async move {
            debug!(?digest, ?dest_path, "Constructing directory");

            // Fetch the Directory proto
            let directory: ProtoDirectory =
                get_and_decode_digest(self.cas_store.as_ref(), digest.into())
                    .await
                    .err_tip(|| format!("Failed to fetch directory digest: {digest:?}"))?;

            // Create the destination directory. It must be writable while it
            // is being populated; 0o755 is its final mode too, so set it now
            // (umask-independent) — no post-construction permission walk.
            self.create_dir_writable(dest_path).await?;

            let mut total_size: u64 = 0;

            // Process files
            for file in &directory.files {
                self.create_file(dest_path, file).await?;
                if let Some(file_digest) = &file.digest {
                    // size_bytes is non-negative; clamp defensively.
                    total_size += u64::try_from(file_digest.size_bytes).unwrap_or(0);
                }
            }

            // Process subdirectories recursively
            for dir_node in &directory.directories {
                total_size += self.create_subdirectory(dest_path, dir_node).await?;
            }

            // Process symlinks
            for symlink in &directory.symlinks {
                self.create_symlink(dest_path, symlink).await?;
            }

            Ok(total_size)
        })
    }

    /// Creates `dir` (and any missing parents) and sets its mode to 0o755 so
    /// that it is writable while the cache entry is being populated and stays
    /// at a stable, umask-independent final mode afterwards.
    async fn create_dir_writable(&self, dir: &Path) -> Result<(), Error> {
        fs::create_dir_all(dir)
            .await
            .err_tip(|| format!("Failed to create directory: {}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
                .await
                .err_tip(|| format!("Failed to set directory mode: {}", dir.display()))?;
        }
        Ok(())
    }

    /// Creates a file from a `FileNode` inside a cache entry.
    ///
    /// The fast path hardlinks the `FilesystemStore` CAS blob directly into the
    /// cache entry — zero-copy, metadata-only — exactly like
    /// `download_to_directory`. A hardlinked file shares its inode with the CAS
    /// store (and every other action that hardlinked the same blob), so it MUST
    /// NOT be chmod'd: doing so is the inode-corruption bug PR #2347 fixed.
    ///
    /// This imposes two correctness rules, both handled here:
    ///  * Executable files (`FileNode.is_executable`) need the `+x` bit, which
    ///    cannot be applied to a shared CAS inode. They are given their own
    ///    private inode via fetch+write and then chmod'd — never hardlinked.
    ///  * If the blob is not locally hardlinkable (the fast tier is not a
    ///    `FilesystemStore`, or the blob is not present in it / was evicted),
    ///    fall back to fetch+write for that file rather than failing.
    async fn create_file(&self, parent: &Path, file_node: &FileNode) -> Result<(), Error> {
        let file_path = parent.join(&file_node.name);
        let digest = DigestInfo::try_from(
            file_node
                .digest
                .clone()
                .ok_or_else(|| make_err!(Code::InvalidArgument, "File node missing digest"))?,
        )
        .err_tip(|| "Invalid file digest")?;

        trace!(?file_path, ?digest, "Creating file");

        // Zero-byte files (digest af1349b9...-0) are not stored in
        // FilesystemStore / many CAS backends, so fetching here returns
        // NotFound. In Bazel-style trees these show up frequently as empty
        // marker / config files (.linksearchpaths, empty .env, .toml, etc.),
        // and a single failure aborts the whole DirectoryCache construction.
        // Short-circuit and write the empty file directly.
        if is_zero_digest(digest) {
            fs::write(&file_path, b"")
                .await
                .err_tip(|| format!("Failed to write empty file: {}", file_path.display()))?;
            return Ok(());
        }

        // Executable files need their own inode to carry the +x bit without
        // mutating the shared CAS blob — copy, never hardlink.
        if file_node.is_executable {
            return self.copy_file_to(&digest, &file_path, true).await;
        }

        // Non-executable file: try to hardlink the CAS blob directly.
        if let Some(filesystem_store) = &self.filesystem_store {
            match self
                .hardlink_cas_blob(filesystem_store, &digest, &file_path)
                .await
            {
                Ok(()) => return Ok(()),
                Err(e) if e.code == Code::NotFound => {
                    // The blob is not in the filesystem tier (e.g. it lives
                    // only in the slow store, or was evicted). Fall through
                    // to fetch+write rather than failing the whole build.
                    trace!(
                        ?digest,
                        ?file_path,
                        "CAS blob not locally hardlinkable, copying instead"
                    );
                }
                Err(e) => return Err(e),
            }
        }

        // Fallback: fetch the blob and write a private copy. Non-executable,
        // so no chmod is needed — the copy keeps its default mode.
        self.copy_file_to(&digest, &file_path, false).await
    }

    /// Hardlinks the `FilesystemStore` CAS blob for `digest` into `file_path`.
    /// Mirrors `download_to_directory`: populate the fast store, resolve the
    /// blob's on-disk path under the entry lock, then `fs::hard_link`.
    ///
    /// Returns a `NotFound` error if the blob is not present in the filesystem
    /// tier; callers fall back to fetch+write in that case.
    async fn hardlink_cas_blob(
        &self,
        filesystem_store: &FilesystemStore,
        digest: &DigestInfo,
        file_path: &Path,
    ) -> Result<(), Error> {
        // Ensure the blob is in the fast (filesystem) tier so it has an
        // on-disk file we can hardlink.
        self.cas_store
            .populate_fast_store(StoreKey::Digest(*digest))
            .await
            .err_tip(|| format!("Failed to populate fast store for {digest}"))?;

        let file_entry = filesystem_store
            .get_file_entry_for_digest(digest)
            .await
            .err_tip(|| "Resolving CAS file entry for hardlink")?;

        let file_path = file_path.to_path_buf();
        file_entry
            .get_file_path_locked(move |src| async move {
                fs::hard_link(&src, &file_path).await.err_tip(|| {
                    format!(
                        "Failed to hardlink CAS blob into cache entry: {}",
                        file_path.display()
                    )
                })
            })
            .await
    }

    /// Fetches the blob for `digest` from the CAS and writes a private copy at
    /// `file_path`. When `executable` is set, the copy is chmod'd `0o555` —
    /// this is safe because the copy has its own inode, unshared with the CAS.
    async fn copy_file_to(
        &self,
        digest: &DigestInfo,
        file_path: &Path,
        executable: bool,
    ) -> Result<(), Error> {
        let data = self
            .cas_store
            .get_part_unchunked(StoreKey::Digest(*digest), 0, None)
            .await
            .err_tip(|| format!("Failed to fetch file: {}", file_path.display()))?;

        fs::write(file_path, data.as_ref())
            .await
            .err_tip(|| format!("Failed to write file: {}", file_path.display()))?;

        #[cfg(unix)]
        if executable {
            use std::os::unix::fs::PermissionsExt;
            // 0o555 (r-xr-xr-x): executable, read-only. This file has its own
            // private inode (just written above), so chmoding it cannot affect
            // any CAS blob or another action's hardlink.
            fs::set_permissions(file_path, std::fs::Permissions::from_mode(0o555))
                .await
                .err_tip(|| {
                    format!(
                        "Failed to set executable permissions: {}",
                        file_path.display()
                    )
                })?;
        }
        #[cfg(not(unix))]
        let _ = executable;

        Ok(())
    }

    /// Creates a subdirectory from a `DirectoryNode`, returning the total size
    /// of the subtree it materializes.
    async fn create_subdirectory(
        &self,
        parent: &Path,
        dir_node: &DirectoryNode,
    ) -> Result<u64, Error> {
        let dir_path = parent.join(&dir_node.name);
        let digest =
            DigestInfo::try_from(dir_node.digest.clone().ok_or_else(|| {
                make_err!(Code::InvalidArgument, "Directory node missing digest")
            })?)
            .err_tip(|| "Invalid directory digest")?;

        trace!(?dir_path, ?digest, "Creating subdirectory");

        // Recursively construct subdirectory
        self.construct_directory(digest, &dir_path).await
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

    /// Selects and removes victim entries from the in-memory `cache` map until
    /// it is within the entry-count and size budgets, and returns the on-disk
    /// paths of the removed entries.
    ///
    /// This is a pure in-memory operation — it does NO filesystem I/O and is
    /// not `async`. The caller runs it under the cache write lock and then,
    /// after releasing the lock, dispatches the returned paths for deletion
    /// via [`Self::dispatch_evictions`]. Keeping eviction's `remove_dir_all`
    /// off the write lock prevents one caller's eviction I/O from serializing
    /// every other concurrent `get_or_create`.
    fn evict_if_needed(
        &self,
        incoming_size: u64,
        cache: &mut HashMap<DigestInfo, CachedDirectoryMetadata>,
    ) -> Vec<PathBuf> {
        let mut evicted_paths = Vec::new();

        // Check entry count
        while cache.len() >= self.config.max_entries {
            let Some((_size, path)) = Self::evict_lru(cache) else {
                // nothing evictable (all entries pinned) — have to exit
                warn!(
                    current_items = cache.len(),
                    max_entries = self.config.max_entries,
                    "Unable to evict anything from directory_cache, will exceed max entries"
                );
                break;
            };
            evicted_paths.push(path);
        }

        // Check total size
        if self.config.max_size_bytes > 0 {
            let current_size: u64 = cache.values().map(|m| m.size).sum();
            let mut size_after = current_size + incoming_size;

            while size_after > self.config.max_size_bytes {
                let Some((e_size, path)) = Self::evict_lru(cache) else {
                    // nothing evictable (all entries pinned) — have to exit
                    warn!(
                        size_after,
                        max_size_bytes = self.config.max_size_bytes,
                        "Unable to evict anything from directory_cache, will exceed max size"
                    );
                    break;
                };
                size_after -= e_size;
                evicted_paths.push(path);
            }
        }

        evicted_paths
    }

    /// Removes the least-recently-used unpinned entry from the in-memory map
    /// and returns its `(size, path)`. Entries with `ref_count > 0` are
    /// in-flight materializations and are never selected — their on-disk tree
    /// must not be deleted while a caller is hardlinking from it.
    ///
    /// Pure in-memory; the actual filesystem deletion is the caller's job.
    fn evict_lru(
        cache: &mut HashMap<DigestInfo, CachedDirectoryMetadata>,
    ) -> Option<(u64, PathBuf)> {
        let to_evict = cache
            .iter()
            .filter(|(_, m)| m.ref_count == 0)
            .min_by_key(|(_, m)| m.last_access)
            .map(|(digest, _)| *digest)?;
        let metadata = cache.remove(&to_evict)?;
        debug!(
            digest = ?to_evict,
            size = metadata.size,
            "Evicting cached directory"
        );
        Some((metadata.size, metadata.path))
    }

    /// Dispatches filesystem deletion of evicted cache-entry trees onto a
    /// background task, so eviction I/O never runs under the cache write lock.
    ///
    /// Each tree's directories are chmod'd writable first
    /// (`set_dir_writable_recursive`) — never its files: a cache-entry file
    /// shares an inode with the `FilesystemStore` CAS blob and every action
    /// that hardlinked it, so chmoding it would corrupt that shared inode (the
    /// PR #2347 bug). Directory write permission alone is sufficient to unlink
    /// files on unix.
    fn dispatch_evictions(paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        background_spawn!("directory_cache_evict", async move {
            for path in paths {
                if !path.exists() {
                    // Already gone (e.g. a prior cleanup, or the cache root
                    // was torn down). Nothing to do, and not an error.
                    continue;
                }
                if let Err(e) = set_dir_writable_recursive(&path).await {
                    warn!(
                        ?path,
                        error = ?e,
                        "Unable to mark evicted directory writable, removal may fail"
                    );
                }
                if let Err(e) = fs::remove_dir_all(&path).await {
                    warn!(
                        ?path,
                        error = ?e,
                        "Failed to remove evicted directory from disk"
                    );
                }
            }
        });
    }

    /// Gets the cache path for a digest
    fn get_cache_path(&self, digest: &DigestInfo) -> PathBuf {
        self.config.cache_root.join(format!("{digest}"))
    }

    /// Returns cache statistics
    pub async fn stats(&self) -> CacheStats {
        let cache = self.cache.read().await;
        let total_size: u64 = cache.values().map(|m| m.size).sum();
        let in_use = cache.values().filter(|m| m.ref_count > 0).count();

        CacheStats {
            entries: cache.len(),
            total_size_bytes: total_size,
            in_use_entries: in_use,
            clonefile_hits: self.clonefile_hits.load(Ordering::Relaxed),
            hardlink_hits: self.hardlink_hits.load(Ordering::Relaxed),
        }
    }
}

/// Statistics about the directory cache
#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    pub entries: usize,
    pub total_size_bytes: u64,
    pub in_use_entries: usize,
    /// Materializations that used APFS `clonefile(2)` (macOS).
    pub clonefile_hits: u64,
    /// Materializations that used per-file `fs::hard_link`.
    pub hardlink_hits: u64,
}

#[cfg(test)]
mod tests {
    use nativelink_config::stores::{
        FastSlowSpec, FilesystemSpec, MemorySpec, StoreDirection, StoreSpec,
    };
    use nativelink_macro::nativelink_test;
    use nativelink_store::memory_store::MemoryStore;
    use nativelink_util::store_trait::Store;
    use prost::Message;
    use tempfile::TempDir;

    use super::*;

    /// Builds a `FastSlowStore` whose fast tier is a real `FilesystemStore`
    /// and whose slow tier is a `MemoryStore` — the same shape the worker
    /// wires up. Returns the `FastSlowStore` plus the slow `Store` handle so
    /// tests can seed blobs/protos into the slow tier.
    async fn make_fast_slow_store(temp_dir: &TempDir) -> (Arc<FastSlowStore>, Store) {
        let fast_spec = FilesystemSpec {
            content_path: temp_dir
                .path()
                .join("cas_content")
                .to_string_lossy()
                .into_owned(),
            temp_path: temp_dir
                .path()
                .join("cas_temp")
                .to_string_lossy()
                .into_owned(),
            eviction_policy: None,
            ..Default::default()
        };
        let slow_spec = MemorySpec::default();
        let fast_store: Arc<FilesystemStore> = FilesystemStore::new(&fast_spec).await.unwrap();
        let slow_store = MemoryStore::new(&slow_spec);
        let cas_store = FastSlowStore::new(
            &FastSlowSpec {
                fast: StoreSpec::Filesystem(fast_spec),
                slow: StoreSpec::Memory(slow_spec),
                fast_direction: StoreDirection::default(),
                slow_direction: StoreDirection::default(),
            },
            Store::new(fast_store),
            Store::new(slow_store.clone()),
        );
        (cas_store, Store::new(slow_store))
    }

    /// Uploads `content` to `store` under a digest derived from `tag`, returns
    /// the digest. `FastSlowStore`/`MemoryStore`/`FilesystemStore` do not
    /// verify content hashes, so a synthetic-but-unique digest is sufficient.
    async fn upload_blob(store: &Store, tag: u8, content: &[u8]) -> DigestInfo {
        let digest = DigestInfo::new([tag; 32], content.len() as u64);
        store
            .as_store_driver_pin()
            .update_oneshot(digest.into(), content.to_vec().into())
            .await
            .unwrap();
        digest
    }

    /// Seeds a one-file directory ("test.txt" = "Hello, World!") into the slow
    /// store and returns the `FastSlowStore` + the root directory digest.
    async fn setup_test_store(temp_dir: &TempDir) -> (Arc<FastSlowStore>, DigestInfo) {
        let (cas_store, slow_store) = make_fast_slow_store(temp_dir).await;

        let file_digest = upload_blob(&slow_store, 1, b"Hello, World!").await;

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
        let mut dir_data = Vec::new();
        directory.encode(&mut dir_data).unwrap();
        let dir_digest = upload_blob(&slow_store, 2, &dir_data).await;

        (cas_store, dir_digest)
    }

    #[nativelink_test]
    async fn test_directory_cache_basic() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store(&temp_dir).await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };

        let cache = DirectoryCache::new(config, store).await?;

        // First access - cache miss
        let dest1 = temp_dir.path().join("dest1");
        let hit = cache.get_or_create(dir_digest, &dest1).await?;
        assert!(!hit, "First access should be cache miss");
        assert!(dest1.join("test.txt").exists());
        assert_eq!(
            fs::read(dest1.join("test.txt")).await.unwrap(),
            b"Hello, World!",
            "materialized file content must be byte-identical to the CAS blob"
        );

        // Second access - cache hit
        let dest2 = temp_dir.path().join("dest2");
        let hit = cache.get_or_create(dir_digest, &dest2).await?;
        assert!(hit, "Second access should be cache hit");
        assert!(dest2.join("test.txt").exists());
        assert_eq!(
            fs::read(dest2.join("test.txt")).await.unwrap(),
            b"Hello, World!",
            "cache-hit materialized content must be byte-identical"
        );

        // Verify stats
        let stats = cache.stats().await;
        assert_eq!(stats.entries, 1);

        // Two get_or_create calls succeeded → two materializations were
        // recorded. On macOS both should be clonefile; on Linux both hardlink.
        #[cfg(target_os = "macos")]
        {
            assert_eq!(stats.clonefile_hits, 2, "macOS should record 2 clones");
            assert_eq!(stats.hardlink_hits, 0);
        }
        #[cfg(not(target_os = "macos"))]
        {
            assert_eq!(stats.clonefile_hits, 0);
            assert_eq!(
                stats.hardlink_hits, 2,
                "non-macOS should record 2 hardlinks"
            );
        }

        Ok(())
    }

    /// A Directory containing a zero-byte file must be constructible even when
    /// the CAS has no entry for the zero-byte digest. In production CAS
    /// backends (`FilesystemStore` in particular) refuse to store zero-byte
    /// blobs, so without the short-circuit this is a `NotFound` error and 30%+
    /// of cache constructions fail (per PR #2243).
    #[nativelink_test]
    async fn test_directory_cache_zero_byte_file() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, slow_store) = make_fast_slow_store(&temp_dir).await;

        // RFC 6234 / Bazel zero-byte SHA-256 digest, hash for b"".
        let zero_digest = DigestInfo::try_new(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            0,
        )
        .unwrap();
        // Deliberately do NOT upload the zero-byte blob — that's the whole
        // point: real CAS backends won't have it.

        let directory = ProtoDirectory {
            files: vec![FileNode {
                name: "empty.txt".to_string(),
                digest: Some(zero_digest.into()),
                is_executable: false,
                ..Default::default()
            }],
            directories: vec![],
            symlinks: vec![],
            ..Default::default()
        };
        let mut dir_data = Vec::new();
        directory.encode(&mut dir_data).unwrap();
        let dir_digest = upload_blob(&slow_store, 3, &dir_data).await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };
        let cache = DirectoryCache::new(config, store).await?;

        let dest = temp_dir.path().join("dest_empty");
        let hit = cache.get_or_create(dir_digest, &dest).await?;
        assert!(!hit, "First construction should be a cache miss");

        let empty_path = dest.join("empty.txt");
        assert!(empty_path.exists(), "zero-byte file should be created");
        let metadata = fs::metadata(&empty_path).await.unwrap();
        assert_eq!(metadata.len(), 0, "zero-byte file must be 0 bytes");

        Ok(())
    }

    /// Regression test for CAS inode corruption during directory cache
    /// eviction.
    ///
    /// Background: a cached directory's files share an inode (via hardlink)
    /// with both the `FilesystemStore` CAS entry and every action workspace
    /// that has consumed this cached directory. The cleanup path that runs
    /// before `fs::remove_dir_all` used to call `set_readwrite_recursive`,
    /// which chmods every file in the tree to 0o644 — silently mutating the
    /// shared inode's mode for every in-flight action holding a hardlink to
    /// the same blob. In production this surfaced as EACCES on exec for
    /// `cc_wrapper.sh` (whose CAS mode is 0o555, but eviction turned it into
    /// 0o644, dropping the +x bit).
    ///
    /// This test models the real scenario: a "cached" directory tree whose
    /// files are hardlinked to a still-active "action workspace" file. The
    /// eviction cleanup runs on the cached tree, and we assert the active
    /// workspace file's mode is untouched. Before the fix this test fails
    /// because `set_readwrite_recursive` chmods the cached-side file, and
    /// the same inode underlies the workspace file.
    #[cfg(unix)]
    #[nativelink_test]
    async fn test_eviction_cleanup_preserves_hardlinked_file_mode() -> Result<(), Error> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        use nativelink_util::fs_util::{set_dir_writable_recursive, set_readonly_recursive};

        let temp_dir = TempDir::new().unwrap();

        // Build a "cached" directory tree mimicking what DirectoryCache
        // produces: a top-level dir with a nested subdir and an executable
        // file inside. Mode 0o555 matches the CAS mode of an executable like
        // `cc_wrapper.sh`.
        let cache_entry_dir = temp_dir.path().join("cache_entry");
        let nested_dir = cache_entry_dir.join("nested");
        fs::create_dir_all(&nested_dir).await.unwrap();
        let cached_file = nested_dir.join("cc_wrapper.sh");
        fs::write(&cached_file, b"#!/bin/sh\necho hi\n")
            .await
            .unwrap();
        fs::set_permissions(&cached_file, std::fs::Permissions::from_mode(0o555))
            .await
            .unwrap();

        // Mark the cached tree read-only the way DirectoryCache does after
        // construction (set_readonly_recursive). This sets every file and
        // directory to 0o555 (read + execute, no write).
        set_readonly_recursive(&cache_entry_dir).await?;

        // Simulate an in-flight action workspace that has hardlinked the
        // cached file. This is what `hardlink_directory_tree` does in
        // `get_or_create` after the cached tree is built and locked down.
        let action_workspace = temp_dir.path().join("action_workspace");
        fs::create_dir_all(&action_workspace).await.unwrap();
        let workspace_file = action_workspace.join("cc_wrapper.sh");
        fs::hard_link(&cached_file, &workspace_file).await.unwrap();

        // Sanity check: cached file and workspace file share the same inode.
        let cached_ino = fs::metadata(&cached_file).await.unwrap().ino();
        let workspace_ino = fs::metadata(&workspace_file).await.unwrap().ino();
        assert_eq!(
            cached_ino, workspace_ino,
            "workspace file must share inode with cached file (hardlinked)",
        );
        let workspace_mode_before = fs::metadata(&workspace_file)
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(workspace_mode_before, 0o555);

        // Run the cleanup that `evict_lru` runs before removing the tree.
        // After the fix this only chmods directories; before the fix this
        // chmoded every file to 0o644, mutating the shared inode.
        set_dir_writable_recursive(&cache_entry_dir).await?;

        // Critical assertion: the action workspace file's mode (i.e. the
        // shared inode's mode) MUST be unchanged. If this fails, the cleanup
        // path corrupted the inode for an in-flight action — that is the
        // bug we are guarding against.
        let workspace_mode_after = fs::metadata(&workspace_file)
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(
            workspace_mode_after, workspace_mode_before,
            "eviction cleanup mutated the inode mode of an active workspace \
             file (was 0o{workspace_mode_before:o}, now 0o{workspace_mode_after:o}); \
             this is the CAS inode corruption bug",
        );

        // We should still be able to remove the cached tree. This proves
        // directory writability alone is sufficient to unlink files on unix.
        fs::remove_dir_all(&cache_entry_dir).await.unwrap();
        assert!(!cache_entry_dir.exists());

        // The workspace's file is still intact and the mode survives even
        // after the cached tree is gone.
        assert!(workspace_file.exists());
        let workspace_mode_after_remove = fs::metadata(&workspace_file)
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(workspace_mode_after_remove, workspace_mode_before);

        Ok(())
    }

    /// OPT #1: a non-executable file in a cache entry must be a hardlink to
    /// the `FilesystemStore` CAS blob — sharing the same inode — rather than a
    /// fresh copy. This is the zero-copy materialization the optimization
    /// delivers.
    #[cfg(unix)]
    #[nativelink_test]
    async fn test_construct_hardlinks_cas_blob() -> Result<(), Error> {
        use std::os::unix::fs::MetadataExt;

        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store(&temp_dir).await;

        // Resolve the filesystem-tier CAS blob path for the file before
        // construction so we can compare inodes afterwards.
        let filesystem_store = store
            .fast_store()
            .downcast_ref::<FilesystemStore>(None)
            .unwrap()
            .get_arc()
            .unwrap();
        // Pull the blob into the fast tier (construction does this too).
        store
            .populate_fast_store(StoreKey::Digest(DigestInfo::new([1u8; 32], 13)))
            .await?;
        let cas_ino = filesystem_store
            .get_file_entry_for_digest(&DigestInfo::new([1u8; 32], 13))
            .await?
            .get_file_path_locked(|p| async move { Ok(fs::metadata(&p).await?.ino()) })
            .await?;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };
        let cache = DirectoryCache::new(config, store).await?;

        let dest = temp_dir.path().join("dest");
        let hit = cache.get_or_create(dir_digest, &dest).await?;
        assert!(!hit, "first access is a miss");

        // The cache entry's file (not yet the dest, which is a clone on macOS)
        // must share the CAS inode. The cache entry path is cache_root/<digest>.
        let cache_entry_file = cache.get_cache_path(&dir_digest).join("test.txt");
        let entry_ino = fs::metadata(&cache_entry_file).await?.ino();
        assert_eq!(
            entry_ino, cas_ino,
            "cache-entry file must be hardlinked to the CAS blob inode (zero-copy)"
        );

        // Content must still be byte-identical.
        assert_eq!(
            fs::read(&cache_entry_file).await?,
            b"Hello, World!",
            "hardlinked file content must match the CAS blob"
        );

        Ok(())
    }

    /// OPT #1 correctness: an executable file must NOT be hardlinked to the
    /// shared CAS blob (chmoding it would corrupt the inode shared with the
    /// CAS and every other action — the PR #2347 bug). It must instead get
    /// its own private inode AND carry the +x bit.
    #[cfg(unix)]
    #[nativelink_test]
    async fn test_construct_executable_gets_private_inode() -> Result<(), Error> {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (cas_store, slow_store) = make_fast_slow_store(&temp_dir).await;

        let script = b"#!/bin/sh\necho ran\n";
        let file_digest = upload_blob(&slow_store, 7, script).await;

        let directory = ProtoDirectory {
            files: vec![FileNode {
                name: "run.sh".to_string(),
                digest: Some(file_digest.into()),
                is_executable: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut dir_data = Vec::new();
        directory.encode(&mut dir_data).unwrap();
        let dir_digest = upload_blob(&slow_store, 8, &dir_data).await;

        // Resolve the CAS blob inode for the executable.
        cas_store
            .populate_fast_store(StoreKey::Digest(file_digest))
            .await?;
        let filesystem_store = cas_store
            .fast_store()
            .downcast_ref::<FilesystemStore>(None)
            .unwrap()
            .get_arc()
            .unwrap();
        let (cas_ino, cas_mode) = filesystem_store
            .get_file_entry_for_digest(&file_digest)
            .await?
            .get_file_path_locked(|p| async move {
                let m = fs::metadata(&p).await?;
                Ok((m.ino(), m.permissions().mode() & 0o777))
            })
            .await?;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };
        let cache = DirectoryCache::new(config, cas_store).await?;

        let dest = temp_dir.path().join("dest");
        cache.get_or_create(dir_digest, &dest).await?;

        let cache_entry_file = cache.get_cache_path(&dir_digest).join("run.sh");
        let entry_meta = fs::metadata(&cache_entry_file).await?;
        let entry_mode = entry_meta.permissions().mode() & 0o777;

        // Private inode: distinct from the shared CAS blob.
        assert_ne!(
            entry_meta.ino(),
            cas_ino,
            "executable must have its own inode, not the shared CAS blob inode"
        );
        // The +x bit is set on the cache entry.
        assert_ne!(entry_mode & 0o111, 0, "executable bit must be set");
        // Content byte-identical.
        assert_eq!(fs::read(&cache_entry_file).await?, script);
        // The CAS blob's mode was NOT mutated by the chmod of the private copy.
        let cas_mode_after = filesystem_store
            .get_file_entry_for_digest(&file_digest)
            .await?
            .get_file_path_locked(|p| async move {
                Ok(fs::metadata(&p).await?.permissions().mode() & 0o777)
            })
            .await?;
        assert_eq!(
            cas_mode_after, cas_mode,
            "CAS blob mode must be untouched by the executable's private chmod"
        );

        Ok(())
    }

    /// OPT #1 fallback: when the CAS blob lives only in the slow tier and is
    /// not locally hardlinkable, construction must still succeed by copying.
    /// `populate_fast_store` resolves this in practice, but the fetch+write
    /// fallback must remain correct and produce identical content.
    #[nativelink_test]
    async fn test_construct_file_content_roundtrip() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store(&temp_dir).await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };
        let cache = DirectoryCache::new(config, store).await?;

        let dest = temp_dir.path().join("dest");
        cache.get_or_create(dir_digest, &dest).await?;
        assert_eq!(
            fs::read(dest.join("test.txt")).await?,
            b"Hello, World!",
            "materialized content must round-trip the CAS blob exactly"
        );

        Ok(())
    }

    /// OPT #2: the cache entry's recorded size must equal the sum of
    /// `FileNode.digest.size_bytes` across the whole (nested) tree —
    /// accumulated during construction, with no post-hoc filesystem walk.
    #[nativelink_test]
    async fn test_size_accounting_from_digest_sizes() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (cas_store, slow_store) = make_fast_slow_store(&temp_dir).await;

        // Two files at the root, one file in a nested subdir.
        let f1 = upload_blob(&slow_store, 10, b"aaaaaaaa").await; // 8 bytes
        let f2 = upload_blob(&slow_store, 11, b"bbb").await; // 3 bytes
        let f3 = upload_blob(&slow_store, 12, b"ccccc").await; // 5 bytes

        let sub = ProtoDirectory {
            files: vec![FileNode {
                name: "nested.bin".to_string(),
                digest: Some(f3.into()),
                is_executable: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut sub_data = Vec::new();
        sub.encode(&mut sub_data).unwrap();
        let sub_digest = upload_blob(&slow_store, 13, &sub_data).await;

        let root = ProtoDirectory {
            files: vec![
                FileNode {
                    name: "a.bin".to_string(),
                    digest: Some(f1.into()),
                    is_executable: false,
                    ..Default::default()
                },
                FileNode {
                    name: "b.bin".to_string(),
                    digest: Some(f2.into()),
                    is_executable: false,
                    ..Default::default()
                },
            ],
            directories: vec![DirectoryNode {
                name: "sub".to_string(),
                digest: Some(sub_digest.into()),
            }],
            ..Default::default()
        };
        let mut root_data = Vec::new();
        root.encode(&mut root_data).unwrap();
        let root_digest = upload_blob(&slow_store, 14, &root_data).await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };
        let cache = DirectoryCache::new(config, cas_store).await?;

        let dest = temp_dir.path().join("dest");
        cache.get_or_create(root_digest, &dest).await?;

        let stats = cache.stats().await;
        assert_eq!(
            stats.total_size_bytes,
            8 + 3 + 5,
            "cache size must be the sum of all FileNode digest sizes (incl. nested)"
        );

        Ok(())
    }

    /// OPT #2: every directory in a cache entry — root and nested — must be
    /// left at mode 0o755, set at creation time without a separate walk.
    #[cfg(unix)]
    #[nativelink_test]
    async fn test_cache_entry_dirs_are_writable() -> Result<(), Error> {
        use std::os::unix::fs::PermissionsExt;

        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (cas_store, slow_store) = make_fast_slow_store(&temp_dir).await;

        let f = upload_blob(&slow_store, 20, b"data").await;
        let sub = ProtoDirectory {
            files: vec![FileNode {
                name: "leaf.txt".to_string(),
                digest: Some(f.into()),
                is_executable: false,
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut sub_data = Vec::new();
        sub.encode(&mut sub_data).unwrap();
        let sub_digest = upload_blob(&slow_store, 21, &sub_data).await;

        let root = ProtoDirectory {
            directories: vec![DirectoryNode {
                name: "sub".to_string(),
                digest: Some(sub_digest.into()),
            }],
            ..Default::default()
        };
        let mut root_data = Vec::new();
        root.encode(&mut root_data).unwrap();
        let root_digest = upload_blob(&slow_store, 22, &root_data).await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };
        let cache = DirectoryCache::new(config, cas_store).await?;
        let dest = temp_dir.path().join("dest");
        cache.get_or_create(root_digest, &dest).await?;

        let entry_root = cache.get_cache_path(&root_digest);
        for dir in [entry_root.clone(), entry_root.join("sub")] {
            let mode = fs::metadata(&dir).await?.permissions().mode() & 0o777;
            assert_eq!(
                mode,
                0o755,
                "cache-entry directory {} must be 0o755",
                dir.display()
            );
        }

        Ok(())
    }

    /// OPT #5: many concurrent `get_or_create` calls for the *same* digest
    /// must be single-flighted — the directory is constructed exactly once
    /// and every caller materializes its own destination from that single
    /// cache entry. Single-flight is observable through the hit/miss return:
    /// exactly one call sees a miss (`false` — it did the construct), all
    /// others see a hit (`true`). Every destination must also be correct.
    ///
    /// Runs on a multi-threaded runtime so the calls truly race.
    #[nativelink_test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_concurrent_get_or_create_single_flight() -> Result<(), Error> {
        use futures::future::join_all;

        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store(&temp_dir).await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };
        let cache = Arc::new(DirectoryCache::new(config, store).await?);

        // Fire 16 concurrent requests for the same digest, each to its own
        // destination.
        const N: usize = 16;
        let dests: Vec<PathBuf> = (0..N)
            .map(|i| temp_dir.path().join(format!("dest_{i}")))
            .collect();
        let futures = dests.iter().map(|dest| {
            let cache = Arc::clone(&cache);
            let dest = dest.clone();
            async move { cache.get_or_create(dir_digest, &dest).await }
        });
        let results: Vec<bool> = join_all(futures)
            .await
            .into_iter()
            .collect::<Result<_, _>>()?;

        // Exactly one construction (one miss); the rest are cache hits.
        let misses = results.iter().filter(|hit| !**hit).count();
        assert_eq!(
            misses, 1,
            "exactly one caller should construct the directory (single-flight)"
        );
        assert_eq!(results.iter().filter(|hit| **hit).count(), N - 1);

        // The cache holds exactly one entry, and every destination has the
        // correct, byte-identical content.
        let stats = cache.stats().await;
        assert_eq!(stats.entries, 1, "digest must be cached exactly once");
        for dest in &dests {
            assert_eq!(
                fs::read(dest.join("test.txt")).await?,
                b"Hello, World!",
                "every concurrently-materialized destination must be correct"
            );
        }

        Ok(())
    }
}
