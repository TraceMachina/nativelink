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

use futures::stream::{FuturesUnordered, TryStreamExt};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_proto::build::bazel::remote::execution::v2::{
    Directory as ProtoDirectory, DirectoryNode, FileNode, SymlinkNode,
};
use nativelink_store::ac_utils::get_and_decode_digest;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::filesystem_store::{FileEntry, FilesystemStore};
use nativelink_util::common::{DigestInfo, fs};
use nativelink_util::fs_util::hardlink_directory_tree;
use tokio::fs as tokio_fs;
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
/// Uses `FilesystemStore` hardlinks for maximum performance - files are hardlinked
/// directly from the `FilesystemStore` instead of being copied from CAS.
///
/// This dramatically reduces I/O and improves action startup time.
pub struct DirectoryCache {
    /// Configuration
    config: DirectoryCacheConfig,
    /// Cache mapping digest -> metadata
    cache: Arc<RwLock<HashMap<DigestInfo, CachedDirectoryMetadata>>>,
    /// Lock for cache construction to prevent stampedes
    construction_locks: Arc<Mutex<HashMap<DigestInfo, Arc<Mutex<()>>>>>,
    /// `FastSlowStore` for fetching directories and populating filesystem store
    cas_store: Arc<FastSlowStore>,
    /// `FilesystemStore` for hardlinking files (must be the "fast" store of `cas_store`)
    filesystem_store: Arc<FilesystemStore>,
}

// Manual Debug implementation since we can't auto-derive with Arc<FastSlowStore>
impl core::fmt::Debug for DirectoryCache {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DirectoryCache")
            .field("config", &self.config)
            .field("cache_entries", &self.cache.try_read().map(|c| c.len()))
            .finish()
    }
}

impl DirectoryCache {
    /// Creates a new `DirectoryCache`
    ///
    /// # Arguments
    /// * `config` - Cache configuration
    /// * `cas_store` - `FastSlowStore` for fetching data (must have `FilesystemStore` as fast store)
    /// * `filesystem_store` - `FilesystemStore` for hardlinking (must be the fast store of `cas_store`)
    pub async fn new(
        config: DirectoryCacheConfig,
        cas_store: Arc<FastSlowStore>,
        filesystem_store: Arc<FilesystemStore>,
    ) -> Result<Self, Error> {
        // Ensure cache root exists
        tokio_fs::create_dir_all(&config.cache_root)
            .await
            .err_tip(|| {
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
            filesystem_store,
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
                    Ok(()) => Ok(true),
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

        // NOTE: We do NOT set the cache directory to read-only because:
        // 1. Hardlinked files share the same inode and thus share permissions
        // 2. Actions need write access to their work directories
        // 3. Setting cache read-only would make all hardlinked work dirs read-only
        // The cache is still protected from accidental modification because actions
        // work in separate directories, and the cache itself is in a different location.

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
    /// Uses parallel processing for better performance with large directory trees
    fn construct_directory<'a>(
        &'a self,
        digest: DigestInfo,
        dest_path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            debug!(?digest, ?dest_path, "Constructing directory");

            // Fetch the Directory proto
            let directory: ProtoDirectory = get_and_decode_digest(&*self.cas_store, digest.into())
                .await
                .err_tip(|| format!("Failed to fetch directory digest: {digest:?}"))?;

            // Create the destination directory
            tokio_fs::create_dir_all(dest_path)
                .await
                .err_tip(|| format!("Failed to create directory: {}", dest_path.display()))?;

            // Process files, subdirectories, and symlinks in parallel
            let mut futures: FuturesUnordered<
                Pin<Box<dyn Future<Output = Result<(), Error>> + Send>>,
            > = FuturesUnordered::new();

            // Queue all file creation tasks
            for file in directory.files {
                let dest_path = dest_path.to_path_buf();
                futures.push(Box::pin(async move {
                    self.create_file(&dest_path, &file).await
                }));
            }

            // Queue all subdirectory creation tasks
            for dir_node in directory.directories {
                let dest_path = dest_path.to_path_buf();
                futures.push(Box::pin(async move {
                    self.create_subdirectory(&dest_path, &dir_node).await
                }));
            }

            // Queue all symlink creation tasks
            for symlink in directory.symlinks {
                let dest_path = dest_path.to_path_buf();
                futures.push(Box::pin(async move {
                    self.create_symlink(&dest_path, &symlink).await
                }));
            }

            // Wait for all operations to complete
            while futures.try_next().await?.is_some() {}

            Ok(())
        })
    }

    /// Creates a file from a `FileNode` using hardlinks from `FilesystemStore`
    /// This is much faster than copying data from CAS
    async fn create_file(&self, parent: &Path, file_node: &FileNode) -> Result<(), Error> {
        let file_path = parent.join(&file_node.name);
        let digest = DigestInfo::try_from(
            file_node
                .digest
                .clone()
                .ok_or_else(|| make_err!(Code::InvalidArgument, "File node missing digest"))?,
        )
        .err_tip(|| "Invalid file digest")?;

        trace!(?file_path, ?digest, "Creating file via hardlink");

        // First, ensure the file is in the FilesystemStore (fast store)
        self.cas_store
            .populate_fast_store(digest.into())
            .await
            .err_tip(|| format!("Failed to populate fast store for: {}", file_path.display()))?;

        // Get the file entry from FilesystemStore
        let file_entry = self
            .filesystem_store
            .get_file_entry_for_digest(&digest)
            .await
            .err_tip(|| format!("Failed to get file entry for: {}", file_path.display()))?;

        // Hardlink from FilesystemStore to destination
        file_entry
            .get_file_path_locked(|src| fs::hard_link(src, &file_path))
            .await
            .map_err(|e| {
                if e.code == Code::NotFound {
                    make_err!(
                        Code::Internal,
                        "Could not hardlink file from cache, likely evicted: {e:?} : {file_path:?}\n\
                        Consider increasing filesystem store's max_bytes configuration."
                    )
                } else {
                    make_err!(Code::Internal, "Could not hardlink file: {e:?} : {file_path:?}")
                }
            })?;

        // Set permissions if executable
        #[cfg(unix)]
        if file_node.is_executable {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = tokio_fs::metadata(&file_path)
                .await
                .err_tip(|| "Failed to get file metadata")?
                .permissions();
            // Set to rwxr-xr-x (755) for executable files
            perms.set_mode(0o755);
            tokio_fs::set_permissions(&file_path, perms)
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
        tokio_fs::symlink(&target, &link_path)
            .await
            .err_tip(|| format!("Failed to create symlink: {}", link_path.display()))?;

        #[cfg(windows)]
        {
            // On Windows, we need to know if target is a directory
            // For now, assume files (can be improved later)
            tokio_fs::symlink_file(&target, &link_path)
                .await
                .err_tip(|| format!("Failed to create symlink: {:?}", link_path))?;
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
            self.evict_lru(cache).await?;
        }

        // Check total size
        if self.config.max_size_bytes > 0 {
            let current_size: u64 = cache.values().map(|m| m.size).sum();
            let mut size_after = current_size + incoming_size;

            while size_after > self.config.max_size_bytes {
                let evicted_size = self.evict_lru(cache).await?;
                size_after -= evicted_size;
            }
        }

        Ok(())
    }

    /// Evicts the least recently used entry
    async fn evict_lru(
        &self,
        cache: &mut HashMap<DigestInfo, CachedDirectoryMetadata>,
    ) -> Result<u64, Error> {
        // Find LRU entry that isn't currently in use
        let to_evict = cache
            .iter()
            .filter(|(_, m)| m.ref_count == 0)
            .min_by_key(|(_, m)| m.last_access)
            .map(|(digest, _)| *digest);

        if let Some(digest) = to_evict {
            if let Some(metadata) = cache.remove(&digest) {
                debug!(?digest, size = metadata.size, "Evicting cached directory");

                // Remove from disk
                if let Err(e) = tokio_fs::remove_dir_all(&metadata.path).await {
                    warn!(
                        ?digest,
                        path = ?metadata.path,
                        error = ?e,
                        "Failed to remove evicted directory from disk"
                    );
                }

                return Ok(metadata.size);
            }
        }

        Ok(0)
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

/// Statistics about the directory cache
#[derive(Debug, Clone, Copy)]
pub struct CacheStats {
    pub entries: usize,
    pub total_size_bytes: u64,
    pub in_use_entries: usize,
}
