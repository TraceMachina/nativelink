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
use std::time::SystemTime;

use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_proto::build::bazel::remote::execution::v2::{
    Directory as ProtoDirectory, DirectoryNode, FileNode, SymlinkNode,
};
use nativelink_store::ac_utils::get_and_decode_digest;
use nativelink_util::common::DigestInfo;
use nativelink_util::fs_util::{
    hardlink_directory_tree, set_readonly_recursive, set_readwrite_recursive,
};
use nativelink_util::store_trait::{Store, StoreKey, StoreLike};
use nativelink_util::{background_spawn, spawn_blocking};
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
    /// CAS store for fetching directories
    cas_store: Store,
}

impl DirectoryCache {
    /// Creates a new `DirectoryCache`
    pub async fn new(config: DirectoryCacheConfig, cas_store: Store) -> Result<Self, Error> {
        // Ensure cache root exists
        fs::create_dir_all(&config.cache_root).await.err_tip(|| {
            format!(
                "Failed to create cache root: {}",
                config.cache_root.display()
            )
        })?;

        Ok(Self {
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
            construction_locks: Arc::new(Mutex::new(HashMap::new())),
            cas_store,
        })
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
        // Fast path: check if already in cache
        {
            let mut cache = self.cache.write().await;
            if let Some(metadata) = cache.get_mut(&digest) {
                // Update access time and ref count
                metadata.last_access = SystemTime::now();
                metadata.ref_count += 1;

                debug!(
                    ?digest,
                    path = ?metadata.path,
                    "Directory cache HIT"
                );

                // Try to hardlink from cache
                match hardlink_directory_tree(&metadata.path, dest_path).await {
                    Ok(()) => {
                        metadata.ref_count -= 1;
                        // Fire-and-forget: warm the page cache for all files in
                        // this freshly-hardlinked tree. The action that triggered
                        // this materialization is about to read them; doing the
                        // readahead now lets disk I/O overlap with the
                        // action-launch overhead. Cheap on Linux (posix_fadvise)
                        // and macOS (F_RDADVISE); no-op elsewhere.
                        let warm_path = dest_path.to_path_buf();
                        background_spawn!("directory_cache_warm_page_cache", async move {
                            warm_page_cache(&warm_path).await;
                        });
                        return Ok(true);
                    }
                    Err(e) => {
                        warn!(
                            ?digest,
                            error = ?e,
                            "Failed to hardlink from cache, will reconstruct"
                        );
                        metadata.ref_count -= 1;
                        // Fall through to reconstruction
                    }
                }
            }
        }

        debug!(?digest, "Directory cache MISS");

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

        // Check again in case another task just constructed it
        {
            let cache = self.cache.read().await;
            if let Some(metadata) = cache.get(&digest) {
                return match hardlink_directory_tree(&metadata.path, dest_path).await {
                    Ok(()) => {
                        // Same fire-and-forget page-cache warming as the
                        // fast-path cache hit above.
                        let warm_path = dest_path.to_path_buf();
                        background_spawn!("directory_cache_warm_page_cache", async move {
                            warm_page_cache(&warm_path).await;
                        });
                        Ok(true)
                    }
                    Err(e) => {
                        warn!(
                            ?digest,
                            error = ?e,
                            "Failed to hardlink after construction"
                        );
                        // Construct directly at dest_path
                        self.construct_directory(digest, dest_path).await?;
                        Ok(false)
                    }
                };
            }
        }

        // Construct the directory in cache
        let cache_path = self.get_cache_path(&digest);
        self.construct_directory(digest, &cache_path).await?;

        // Make it read-only to prevent modifications
        set_readonly_recursive(&cache_path)
            .await
            .err_tip(|| "Failed to set cache directory to readonly")?;

        // Calculate size
        let size = nativelink_util::fs_util::calculate_directory_size(&cache_path)
            .await
            .err_tip(|| "Failed to calculate directory size")?;

        // Add to cache
        {
            let mut cache = self.cache.write().await;

            // Evict if necessary
            self.evict_if_needed(size, &mut cache).await?;

            cache.insert(
                digest,
                CachedDirectoryMetadata {
                    path: cache_path.clone(),
                    size,
                    last_access: SystemTime::now(),
                    ref_count: 0,
                },
            );
        }

        // Hardlink to destination
        hardlink_directory_tree(&cache_path, dest_path)
            .await
            .err_tip(|| "Failed to hardlink newly cached directory")?;

        Ok(false)
    }

    /// Constructs a directory from the CAS at the given path
    fn construct_directory<'a>(
        &'a self,
        digest: DigestInfo,
        dest_path: &'a Path,
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
                self.create_file(dest_path, file).await?;
            }

            // Process subdirectories recursively
            for dir_node in &directory.directories {
                self.create_subdirectory(dest_path, dir_node).await?;
            }

            // Process symlinks
            for symlink in &directory.symlinks {
                self.create_symlink(dest_path, symlink).await?;
            }

            Ok(())
        })
    }

    /// Creates a file from a `FileNode`
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
    ) -> Result<(), Error> {
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

    /// Evicts entries if cache is too full
    async fn evict_if_needed(
        &self,
        incoming_size: u64,
        cache: &mut HashMap<DigestInfo, CachedDirectoryMetadata>,
    ) -> Result<(), Error> {
        // Check entry count
        while cache.len() >= self.config.max_entries {
            let evicted_size = self.evict_lru(cache).await?;
            if evicted_size.is_none() {
                // nothing evicted, so have to exit
                warn!(
                    current_items = cache.len(),
                    max_entries = self.config.max_entries,
                    "Unable to evict anything from directory_cache, will exceed max entries"
                );
                break;
            }
        }

        // Check total size
        if self.config.max_size_bytes > 0 {
            let current_size: u64 = cache.values().map(|m| m.size).sum();
            let mut size_after = current_size + incoming_size;

            while size_after > self.config.max_size_bytes {
                let evicted_size = self.evict_lru(cache).await?;
                match evicted_size {
                    None => {
                        // nothing evicted, so have to exit
                        warn!(
                            size_after,
                            max_size_bytes = self.config.max_size_bytes,
                            "Unable to evict anything from directory_cache, will exceed max size"
                        );
                        break;
                    }
                    Some(e_size) => {
                        size_after -= e_size;
                    }
                }
            }
        }

        Ok(())
    }

    /// Evicts the least recently used entry
    async fn evict_lru(
        &self,
        cache: &mut HashMap<DigestInfo, CachedDirectoryMetadata>,
    ) -> Result<Option<u64>, Error> {
        // Find LRU entry that isn't currently in use
        let to_evict = cache
            .iter()
            .filter(|(_, m)| m.ref_count == 0)
            .min_by_key(|(_, m)| m.last_access)
            .map(|(digest, _)| *digest);

        if let Some(digest) = to_evict
            && let Some(metadata) = cache.remove(&digest)
        {
            debug!(?digest, size = metadata.size, "Evicting cached directory");

            if let Err(e) = set_readwrite_recursive(&metadata.path).await {
                warn!(
                    ?digest,
                    path = ?metadata.path,
                    error = ?e,
                    "Unable to mark evicted directory as read/write, will probably fail to remove"
                );
            }

            // Remove from disk
            if let Err(e) = fs::remove_dir_all(&metadata.path).await {
                warn!(
                    ?digest,
                    path = ?metadata.path,
                    error = ?e,
                    "Failed to remove evicted directory from disk"
                );
            }

            return Ok(Some(metadata.size));
        }

        Ok(None)
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
        }
    }
}

/// Walk `root` and hint the kernel to fault every regular file into the page
/// cache. Best-effort: any I/O or open errors are swallowed, since this is a
/// performance optimization and not a correctness step.
///
/// On Linux this issues `posix_fadvise(POSIX_FADV_WILLNEED)`; on macOS it
/// issues `fcntl(F_RDADVISE)` over the file's full length (capped at the
/// `radvisory` `int` ceiling internally). On other platforms the per-file
/// helper is a no-op, so this entire function effectively reduces to a tree
/// walk that exits early — cheap.
async fn warm_page_cache(root: &Path) {
    // Bound recursion depth to keep stack usage predictable; well under any
    // reasonable real-world action input tree.
    const MAX_DEPTH: usize = 64;

    fn walk_blocking(dir: &Path, depth: usize, out: &mut Vec<(PathBuf, u64)>) {
        if depth >= MAX_DEPTH {
            return;
        }
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_dir() {
                walk_blocking(&path, depth + 1, out);
            } else if ft.is_file() {
                let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
                out.push((path, size));
            }
        }
    }

    let root = root.to_path_buf();
    // Do the directory walk on a blocking thread so we don't stall the runtime.
    let Ok(files) = spawn_blocking!("directory_cache_warm_page_cache_walk", move || {
        let mut files = Vec::new();
        walk_blocking(&root, 0, &mut files);
        files
    })
    .await
    else {
        return;
    };

    for (path, size) in files {
        // Skip empty files: no pages to warm.
        if size == 0 {
            continue;
        }
        // `open_file` acquires an FD permit and bounds concurrent opens for us.
        if let Ok(file) = nativelink_util::fs::open_file(&path, 0, u64::MAX).await {
            let len_hint = usize::try_from(size).unwrap_or(usize::MAX);
            file.get_ref().advise_willneed(0, len_hint);
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

        let cache = DirectoryCache::new(config, store).await?;

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
}
