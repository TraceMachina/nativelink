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
use nativelink_util::fs_util::{hardlink_directory_tree, set_readonly_recursive};
use nativelink_util::store_trait::{Store, StoreKey, StoreLike};
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
        // Fast path: check if already in cache
        {
            let mut cache = self.cache.write().await;
            if let Some(metadata) = cache.get_mut(&digest) {
                // Bump ref_count to prevent eviction during hardlink
                metadata.last_access = SystemTime::now();
                metadata.ref_count += 1;
                let src_path = metadata.path.clone();
                drop(cache);

                debug!(?digest, path = ?src_path, "Directory cache HIT");

                let result = hardlink_directory_tree(&src_path, dest_path).await;

                // Always decrement ref_count
                let mut cache = self.cache.write().await;
                if let Some(metadata) = cache.get_mut(&digest) {
                    metadata.ref_count -= 1;
                }
                drop(cache);

                match result {
                    Ok(()) => return Ok(true),
                    Err(e) => {
                        warn!(
                            ?digest,
                            error = ?e,
                            "Failed to hardlink from cache, will reconstruct"
                        );
                        // Fall through to reconstruction
                    }
                }
            }
        }

        debug!(?digest, "Directory cache MISS");

        // Get or create construction lock to prevent stampede (Bug 3: we clean up below)
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
        {
            let mut cache = self.cache.write().await;
            if let Some(metadata) = cache.get_mut(&digest) {
                metadata.last_access = SystemTime::now();
                metadata.ref_count += 1;
                let src_path = metadata.path.clone();
                drop(cache);

                let result = hardlink_directory_tree(&src_path, dest_path).await;

                let mut cache = self.cache.write().await;
                if let Some(metadata) = cache.get_mut(&digest) {
                    metadata.ref_count -= 1;
                }
                drop(cache);

                match result {
                    Ok(()) => {
                        self.cleanup_construction_lock(&digest, &construction_lock);
                        return Ok(true);
                    }
                    Err(e) => {
                        warn!(
                            ?digest,
                            error = ?e,
                            "Failed to hardlink after construction lock acquire"
                        );
                        // Fall through to reconstruct
                    }
                }
            }
        }

        // Bug 2: Construct in a temp path, rename to final path on success.
        // This prevents orphaned partial directories on failure.
        let cache_path = self.get_cache_path(&digest);
        let temp_path = self.config.cache_root.join(format!(
            ".tmp-{digest}-{}",
            std::process::id()
        ));

        // Clean up any stale temp path from a previous crashed attempt
        drop(fs::remove_dir_all(&temp_path).await);

        match self.construct_directory(digest, &temp_path).await {
            Ok(()) => {}
            Err(e) => {
                // Clean up partial construction (best-effort)
                drop(fs::remove_dir_all(&temp_path).await);
                self.cleanup_construction_lock(&digest, &construction_lock);
                return Err(e).err_tip(|| "Failed to construct directory for cache");
            }
        }

        // Make it read-only to prevent modifications
        if let Err(e) = set_readonly_recursive(&temp_path).await {
            drop(fs::remove_dir_all(&temp_path).await);
            self.cleanup_construction_lock(&digest, &construction_lock);
            return Err(e).err_tip(|| "Failed to set cache directory to readonly");
        }

        // Calculate size
        let size = match nativelink_util::fs_util::calculate_directory_size(&temp_path).await {
            Ok(s) => s,
            Err(e) => {
                drop(fs::remove_dir_all(&temp_path).await);
                self.cleanup_construction_lock(&digest, &construction_lock);
                return Err(e).err_tip(|| "Failed to calculate directory size");
            }
        };

        // Atomic rename from temp to final cache path
        if let Err(e) = fs::rename(&temp_path, &cache_path).await {
            drop(fs::remove_dir_all(&temp_path).await);
            self.cleanup_construction_lock(&digest, &construction_lock);
            return Err(e).err_tip(|| {
                format!(
                    "Failed to rename temp dir {} to cache path {}",
                    temp_path.display(),
                    cache_path.display()
                )
            });
        }

        // Bug 5: Insert with ref_count=1 to prevent eviction during hardlink
        {
            let mut cache = self.cache.write().await;
            self.evict_if_needed(size, &mut cache).await?;
            cache.insert(
                digest,
                CachedDirectoryMetadata {
                    path: cache_path.clone(),
                    size,
                    last_access: SystemTime::now(),
                    ref_count: 1,
                },
            );
        }

        // Hardlink to destination (safe — ref_count=1 prevents eviction)
        let hardlink_result = hardlink_directory_tree(&cache_path, dest_path).await;

        // Decrement ref_count regardless of hardlink result
        {
            let mut cache = self.cache.write().await;
            if let Some(metadata) = cache.get_mut(&digest) {
                metadata.ref_count -= 1;
            }
        }

        hardlink_result.err_tip(|| "Failed to hardlink newly cached directory")?;

        // Bug 3: Clean up construction lock if no other waiters
        self.cleanup_construction_lock(&digest, &construction_lock);

        Ok(false)
    }

    /// Removes the construction lock entry if no other task is waiting on it.
    fn cleanup_construction_lock(&self, digest: &DigestInfo, lock: &Arc<Mutex<()>>) {
        // Arc::strong_count == 2 means: our `lock` clone + the one in the HashMap.
        // No other task is holding a clone, so it's safe to remove.
        if Arc::strong_count(lock) <= 2 {
            // Use try_lock to avoid blocking — if we can't get it, another task
            // will clean up.
            if let Ok(mut locks) = self.construction_locks.try_lock() {
                locks.remove(digest);
            }
        }
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

    /// Evicts entries if cache is too full.
    /// Returns `Ok(())` on success. Logs a warning if the cache is over capacity
    /// but all entries are in use and cannot be evicted.
    async fn evict_if_needed(
        &self,
        incoming_size: u64,
        cache: &mut HashMap<DigestInfo, CachedDirectoryMetadata>,
    ) -> Result<(), Error> {
        // Check entry count
        while cache.len() >= self.config.max_entries {
            if self.evict_lru(cache).await?.is_none() {
                warn!(
                    entries = cache.len(),
                    max = self.config.max_entries,
                    "Directory cache over entry limit but all entries are in use"
                );
                break;
            }
        }

        // Check total size
        if self.config.max_size_bytes > 0 {
            let current_size: u64 = cache.values().map(|m| m.size).sum();
            let mut size_after = current_size + incoming_size;

            while size_after > self.config.max_size_bytes {
                if let Some(evicted_size) = self.evict_lru(cache).await? {
                    size_after -= evicted_size;
                } else {
                    warn!(
                        size_after,
                        max = self.config.max_size_bytes,
                        "Directory cache over size limit but all entries are in use"
                    );
                    break;
                }
            }
        }

        Ok(())
    }

    /// Evicts the least recently used entry with ref_count == 0.
    /// Returns `Ok(Some(size))` if an entry was evicted, `Ok(None)` if no
    /// evictable entry exists.
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

        if let Some(digest) = to_evict {
            if let Some(metadata) = cache.remove(&digest) {
                debug!(?digest, size = metadata.size, "Evicting cached directory");

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

        let cache = DirectoryCache::new(config, store).await?;

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

        let cache = DirectoryCache::new(config, store).await?;

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
            max_entries: 1, // Only 1 entry allowed
            max_size_bytes: 0, // No size limit
            cache_root,
        };

        let cache = DirectoryCache::new(config, store).await?;

        // Fill the cache
        let dest1 = temp_dir.path().join("dest1");
        cache.get_or_create(dir_digest, &dest1).await?;

        // Simulate all entries being in-use by bumping ref_count
        {
            let mut cache_map = cache.cache.write().await;
            if let Some(metadata) = cache_map.get_mut(&dir_digest) {
                metadata.ref_count = 1;
            }
        }

        // Bug 4 fix: evict_if_needed should not loop infinitely.
        // We can't insert a new entry (max_entries=1, existing has ref_count>0),
        // but evict_if_needed should return Ok without looping forever.
        // Test this by directly calling evict_if_needed.
        {
            let mut cache_map = cache.cache.write().await;
            // This should NOT hang — it should break out of the loop
            let result = cache.evict_if_needed(100, &mut cache_map).await;
            assert!(result.is_ok(), "evict_if_needed should not fail");
            assert_eq!(
                cache_map.len(),
                1,
                "Entry should still be present (not evictable)"
            );
        }

        // Clean up ref_count so test teardown works
        {
            let mut cache_map = cache.cache.write().await;
            if let Some(metadata) = cache_map.get_mut(&dir_digest) {
                metadata.ref_count = 0;
            }
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_concurrent_same_digest() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store().await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
        };

        let cache = Arc::new(DirectoryCache::new(config, store).await?);

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

        let cache = DirectoryCache::new(config, store).await?;

        // Access the cache
        let dest = temp_dir.path().join("dest");
        cache.get_or_create(dir_digest, &dest).await?;

        // Bug 3 fix: construction lock should be cleaned up
        let locks = cache.construction_locks.lock().await;
        assert!(
            locks.is_empty(),
            "Construction lock should be removed after get_or_create completes"
        );

        Ok(())
    }
}
