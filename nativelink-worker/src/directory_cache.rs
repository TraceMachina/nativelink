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
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use futures::future::BoxFuture;
use futures::stream::TryStreamExt;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent, group, publish,
};
use nativelink_proto::build::bazel::remote::execution::v2::{
    Directory as ProtoDirectory, DirectoryNode, FileNode, GetTreeRequest, SymlinkNode,
};
use nativelink_store::ac_utils::get_and_decode_digest;
use nativelink_store::cas_utils::is_zero_digest;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::filesystem_store::{FileEntry, FilesystemStore};
use nativelink_store::grpc_store::GrpcStore;
use nativelink_util::background_spawn;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc, default_digest_hasher_func};
use nativelink_util::fs_util::{CloneMethod, hardlink_directory_tree, set_dir_writable_recursive};
use nativelink_util::store_trait::{StoreKey, StoreLike};
use prost::Message;
use tokio::fs;

/// Maximum number of concurrently-polled node materializations (file
/// fetches, subdirectory recursions, symlink creations) per directory
/// level. Matches `DOWNLOAD_TO_DIRECTORY_CONCURRENCY` in
/// `running_actions_manager.rs`: enough parallelism to overlap slow-store
/// round trips, bounded so hardlink/copy syscalls do not fight the
/// filesystem's metadata locks (see the gate comment in
/// `download_to_directory`).
const CONSTRUCT_DIRECTORY_CONCURRENCY: usize = 64;

/// Default for `DirectoryCacheConfig::max_concurrent_fetches`, preserving
/// the historical cache-wide fetch bound.
const DEFAULT_MAX_CONCURRENT_FETCHES: usize = 64;
use tokio::sync::{Mutex, RwLock, Semaphore};
use tracing::{debug, info, trace, warn};

/// Prefix for in-progress construction trees under `cache_root`. Cache-entry
/// names are digest strings (hex), so dot-prefixed scratch names can never
/// collide with a published entry.
const TEMP_PREFIX: &str = ".tmp-";

/// Prefix for eviction tombstones awaiting background deletion.
const TOMBSTONE_PREFIX: &str = ".del-";

/// Minimum interval between summary log lines, in nanoseconds (one minute).
const SUMMARY_LOG_INTERVAL_NANOS: u64 = 60_000_000_000;

/// Current time as nanoseconds since the unix epoch, for lock-free LRU
/// timestamps. Saturates instead of failing (`u64` nanos overflow in 2554).
fn unix_nanos_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX))
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
    /// Experimental: additionally cache every subdirectory by its own
    /// `Directory` digest so unchanged subtrees are shared across different
    /// root digests. Subtree entries participate in normal eviction and
    /// multiply the entry count, so `max_entries` should be raised when
    /// enabling this. Default: false (root-only caching, existing behavior).
    pub experimental_subtree_caching: bool,
    /// Maximum number of concurrent slow-store fetches across ALL directory
    /// constructions of this cache. Permits are held only across leaf I/O
    /// (directory-proto fetches, blob fetches/populates) and never across
    /// subdirectory recursion, so recursion depth cannot deadlock the
    /// semaphore. This is the bound that protects backing stores from RPC
    /// storms: per-level construction concurrency compounds multiplicatively
    /// across tree levels and concurrent actions (measured >1800 in-flight
    /// fetches for 6 concurrent ~500-file trees without this cap). Low
    /// values fragment coalesced/batched reads to at most this many items;
    /// see the config-crate doc for the `experimental_read_batching`
    /// interaction. Must be > 0. Default: 64.
    pub max_concurrent_fetches: usize,
    /// See `nativelink-config`'s `DirectoryCacheConfig::experimental_get_tree_prefetch`.
    pub experimental_get_tree_prefetch: bool,
}

impl Default for DirectoryCacheConfig {
    fn default() -> Self {
        Self {
            max_entries: 1000,
            max_size_bytes: 10 * 1024 * 1024 * 1024, // 10 GB
            cache_root: std::env::temp_dir().join("nativelink_directory_cache"),
            experimental_subtree_caching: false,
            max_concurrent_fetches: DEFAULT_MAX_CONCURRENT_FETCHES,
            experimental_get_tree_prefetch: false,
        }
    }
}

/// Metadata for a cached directory
#[derive(Debug)]
struct CachedDirectoryMetadata {
    /// Path to the cached directory
    path: PathBuf,
    /// Recorded size in bytes. With subtree caching enabled this covers only
    /// bytes not owned by a descendant entry, so the sum over the map
    /// approximates unique materialized bytes.
    size: u64,
    /// Last access time (nanoseconds since the unix epoch) for LRU eviction.
    /// Atomic so cache hits can refresh it under the cache READ lock instead
    /// of serializing on the write lock.
    last_access: AtomicU64,
    /// Number of active users. Shared with the [`EntryPin`] RAII guards
    /// handed out by `acquire_entry`, which decrement it on drop — even when
    /// the future holding the pin is cancelled mid-await. Eviction skips
    /// entries with a non-zero count.
    ref_count: Arc<AtomicUsize>,
}

/// RAII pin on a cache entry: while alive, eviction will not select the
/// entry, so its on-disk tree cannot be deleted out from under an unlocked
/// materialization. Dropping the pin releases it — including implicitly when
/// a construction future is cancelled (e.g. `buffer_unordered` dropping
/// sibling futures after another node errors), so pins can never leak and
/// permanently block eviction.
#[derive(Debug)]
struct EntryPin {
    ref_count: Arc<AtomicUsize>,
}

impl Drop for EntryPin {
    fn drop(&mut self) {
        self.ref_count.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Drop guard for an in-progress temp construction tree: if the owning
/// future is dropped before [`Self::disarm`] (cancellation — e.g.
/// `buffer_unordered` drops sibling constructions on the first error), the
/// partial tree is deleted in the background. Error paths instead disarm and
/// clean up inline so a failed construction leaves nothing behind by the
/// time the error propagates.
#[derive(Debug)]
struct ScratchGuard {
    path: Option<PathBuf>,
}

impl ScratchGuard {
    const fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for ScratchGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            DirectoryCache::dispatch_evictions(vec![path]);
        }
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
    /// Global slow-fetch budget shared by all constructions of this cache;
    /// see `DirectoryCacheConfig::max_concurrent_fetches`.
    fetch_permits: Semaphore,
    /// Count of materializations that used APFS `clonefile(2)` (macOS only;
    /// always zero on other platforms).
    clonefile_hits: AtomicU64,
    /// Count of materializations that used per-file `fs::hard_link`.
    hardlink_hits: AtomicU64,
    /// Count of subtree materializations served from the cache by the
    /// subtree's own `Directory` digest. Only ever non-zero when
    /// `experimental_subtree_caching` is enabled.
    subtree_hits: AtomicU64,
    /// Count of subtree constructions that could not be served from the
    /// cache. Only ever non-zero when `experimental_subtree_caching` is
    /// enabled.
    subtree_misses: AtomicU64,
    /// Count of entries removed by LRU eviction. A high rate relative to
    /// hits means the cache is thrashing (`max_entries`/`max_size_bytes` too
    /// small for the workload — especially with subtree caching enabled,
    /// which multiplies the entry count).
    evictions: AtomicU64,
    /// Monotonic counter for process-unique scratch names (temp construction
    /// trees and eviction tombstones) under `cache_root`.
    scratch_seq: AtomicU64,
    /// Mirror of the map's entry count, maintained at insert/evict/invalidate
    /// so the sync `MetricsComponent::publish` and the rate-limited log
    /// summary can read it without touching the async `RwLock`.
    map_entries: AtomicU64,
    /// Mirror of the map's total recorded size in bytes (see `map_entries`).
    map_size_bytes: AtomicU64,
    /// Timestamp (unix nanos) of the last summary log line, for rate
    /// limiting.
    last_summary_log: AtomicU64,
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
        // A zero-permit semaphore would deadlock every construction.
        if config.max_concurrent_fetches == 0 {
            return Err(make_err!(
                Code::InvalidArgument,
                "directory_cache max_concurrent_fetches must be greater than 0"
            ));
        }
        let max_concurrent_fetches = config.max_concurrent_fetches;

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

        let cache = Self {
            config,
            cache: Arc::new(RwLock::new(HashMap::new())),
            construction_locks: Arc::new(Mutex::new(HashMap::new())),
            cas_store,
            filesystem_store,
            fetch_permits: Semaphore::new(max_concurrent_fetches),
            clonefile_hits: AtomicU64::new(0),
            hardlink_hits: AtomicU64::new(0),
            subtree_hits: AtomicU64::new(0),
            subtree_misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            scratch_seq: AtomicU64::new(0),
            map_entries: AtomicU64::new(0),
            map_size_bytes: AtomicU64::new(0),
            last_summary_log: AtomicU64::new(0),
        };
        cache.sweep_cache_root().await;
        Ok(cache)
    }

    /// Sweeps pre-existing content of `cache_root`. The in-memory map starts
    /// empty, so anything already on disk is orphaned: entries from a
    /// previous process (unusable — they are not in the map, and a partial
    /// one would poison re-construction of its digest), plus stale temp
    /// trees and tombstones. Each orphan is renamed to a tombstone
    /// synchronously — after `new` returns, a fresh construction could
    /// otherwise publish to the very canonical path a detached deletion is
    /// about to remove — and the tombstones are deleted in the background.
    async fn sweep_cache_root(&self) {
        let mut orphans = Vec::new();
        match fs::read_dir(&self.config.cache_root).await {
            Ok(mut dir) => loop {
                match dir.next_entry().await {
                    Ok(Some(entry)) => orphans.push(entry.path()),
                    Ok(None) => break,
                    Err(e) => {
                        warn!(error = ?e, "Failed reading cache root during orphan sweep");
                        break;
                    }
                }
            },
            Err(e) => {
                warn!(error = ?e, "Failed to scan cache root for orphaned entries");
                return;
            }
        }
        if orphans.is_empty() {
            return;
        }
        debug!(
            count = orphans.len(),
            "Sweeping orphaned directory cache content"
        );
        let tombstones = self.tombstone_victims(orphans).await;
        Self::dispatch_evictions(tombstones);
    }

    /// Acquires a permit from the cache-wide slow-fetch budget. Held only
    /// across leaf I/O awaits, never across subdirectory recursion.
    async fn acquire_fetch_permit(&self) -> Result<tokio::sync::SemaphorePermit<'_>, Error> {
        self.fetch_permits
            .acquire()
            .await
            .map_err(|e| make_err!(Code::Internal, "Fetch semaphore closed: {e:?}"))
    }

    /// Emits an `info!` summary of the cache counters, rate-limited to at
    /// most one line per [`SUMMARY_LOG_INTERVAL_NANOS`]. Called from the
    /// hit/miss path so deployments without a metrics pipeline can still
    /// verify the cache is working by grepping worker logs.
    fn maybe_log_summary(&self) {
        let now = unix_nanos_now();
        let last = self.last_summary_log.load(Ordering::Relaxed);
        if now.saturating_sub(last) < SUMMARY_LOG_INTERVAL_NANOS {
            return;
        }
        // Single winner per interval; losers skip the log line.
        if self
            .last_summary_log
            .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        info!(
            clonefile_hits = self.clonefile_hits.load(Ordering::Relaxed),
            hardlink_hits = self.hardlink_hits.load(Ordering::Relaxed),
            subtree_hits = self.subtree_hits.load(Ordering::Relaxed),
            subtree_misses = self.subtree_misses.load(Ordering::Relaxed),
            evictions = self.evictions.load(Ordering::Relaxed),
            entries = self.map_entries.load(Ordering::Relaxed),
            total_size_bytes = self.map_size_bytes.load(Ordering::Relaxed),
            "DirectoryCache summary"
        );
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
        let (hit, _size) = self
            .get_or_create_entry(digest, dest_path, None, true)
            .await?;
        Ok(hit)
    }

    /// Core get-or-create flow shared by root materializations
    /// ([`Self::get_or_create`]) and, with `experimental_subtree_caching`
    /// enabled, subtree materializations (`create_subdirectory`). Returns
    /// `(hit, size)` where `hit` is true when the destination was served by
    /// hardlinking an existing cache entry, and `size` is the entry's
    /// recorded size in bytes.
    ///
    /// `protos` is a map of prefetched `Directory` protos consulted during
    /// construction (see [`Self::prefetch_tree_protos`]); `prefetch_on_miss`
    /// is true only for root-level calls, which issue the tree's logical
    /// `GetTree` prefetch on a cold miss. Subtree flights pass the root's
    /// map down instead, so cache hits (root or subtree) never prefetch. A
    /// server may paginate that traversal across multiple RPCs.
    async fn get_or_create_entry(
        &self,
        digest: DigestInfo,
        dest_path: &Path,
        protos: Option<&HashMap<DigestInfo, ProtoDirectory>>,
        prefetch_on_miss: bool,
    ) -> Result<(bool, u64), Error> {
        self.maybe_log_summary();

        // Fast path: serve from an existing entry.
        if let Some(size) = self.try_materialize_from_cache(&digest, dest_path).await {
            return Ok((true, size));
        }

        debug!(?digest, "Directory cache MISS");

        // Single-flight: only one task constructs a given digest at a time.
        // Concurrent callers for the same digest block on this per-digest
        // mutex; when the constructor finishes and inserts the entry, the
        // waiters wake, find it in the cache, and materialize their own
        // destination from the single shared cache entry. Root and subtree
        // flights for the same digest share one mutex, and the lock order is
        // deadlock-free by construction: a flight only ever waits on locks
        // of its strict descendants, and the content-addressed Merkle DAG is
        // acyclic.
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
        let result = self
            .construct_and_materialize(digest, dest_path, protos, prefetch_on_miss)
            .await;
        self.forget_construction_lock(&digest).await;
        result
    }

    /// Attempts to serve `digest` from an existing cache entry by hardlinking
    /// it to `dest_path`, returning the entry's recorded size on success.
    ///
    /// The entry is pinned (see [`EntryPin`]) across the unlocked
    /// `hardlink_directory_tree` so eviction cannot delete its tree
    /// mid-materialization; only the brief refcount/LRU bookkeeping runs
    /// under the cache lock (the READ lock — hits never serialize on the
    /// write lock).
    ///
    /// Returns `None` when the digest is not cached, or when the entry's
    /// on-disk tree failed to hardlink (missing/corrupt). In the failure
    /// case the dead entry has been invalidated (removed + tombstoned, so it
    /// cannot fail every future request forever) and the possibly partially
    /// populated destination cleared, so the caller can construct fresh.
    async fn try_materialize_from_cache(
        &self,
        digest: &DigestInfo,
        dest_path: &Path,
    ) -> Option<u64> {
        let (cache_path, size, pin) = self.acquire_entry(digest).await?;
        debug!(?digest, ?cache_path, "Directory cache HIT");
        match hardlink_directory_tree(&cache_path, dest_path).await {
            Ok(method) => {
                drop(pin);
                self.record_clone_method(method);
                Some(size)
            }
            Err(e) => {
                warn!(
                    ?digest,
                    error = ?e,
                    "Failed to hardlink from cache, invalidating entry and reconstructing"
                );
                self.invalidate_entry(digest, &pin).await;
                drop(pin);
                // The failed walk may have partially populated the
                // destination; clear it so the reconstruction starts clean
                // (`hardlink_directory_tree` refuses an existing destination
                // and `create_file` fails on leftovers).
                Self::remove_tree_best_effort(dest_path).await;
                None
            }
        }
    }

    /// The cache-miss body, run while holding the per-digest construction
    /// guard. Split out so `get_or_create_entry` can unconditionally clean up
    /// the construction-lock map entry afterwards on every exit path.
    /// `protos` and `prefetch_on_miss` are documented on
    /// [`Self::get_or_create_entry`].
    async fn construct_and_materialize(
        &self,
        digest: DigestInfo,
        dest_path: &Path,
        protos: Option<&HashMap<DigestInfo, ProtoDirectory>>,
        prefetch_on_miss: bool,
    ) -> Result<(bool, u64), Error> {
        // Re-check: another task may have just constructed this digest while
        // we waited on the construction lock. If the entry turns out to be
        // damaged it is invalidated and we fall through to rebuild it fresh
        // (exactly once — we hold the construction lock).
        if let Some(size) = self.try_materialize_from_cache(&digest, dest_path).await {
            return Ok((true, size));
        }

        // Construct the directory into a UNIQUE temp path first, then
        // atomically publish it to the canonical `cache_root/<digest>` path
        // via rename. Constructing at the canonical path directly would
        // poison the digest on failure: the partial tree left behind makes
        // every future construction of the digest fail on its leftovers —
        // and subtree digests are stable across builds, so one poisoned
        // popular subtree would fail every subsequent build.
        //
        // `construct_directory` returns the total tree size accumulated from
        // `FileNode.digest.size_bytes` as it builds — no post-hoc filesystem
        // walk is needed. It also sets every cache-entry directory's mode at
        // creation time (0o755), so no separate permission-fixup walk is
        // needed either.
        //
        // The cache entry's *files* are deliberately never chmod'd here:
        // non-executable files are hardlinks to FilesystemStore CAS blobs (see
        // `create_file`), and chmoding such a file mutates the inode shared
        // with the CAS and every other in-flight action that hardlinked the
        // same blob — the inode-corruption bug PR #2347 fixed.
        let cache_path = self.get_cache_path(&digest);
        // Root-level cold miss: prefetch the whole tree's `Directory` protos
        // with one logical `GetTree` traversal, following server pagination.
        // Subtree flights never prefetch — they inherit the root's map (or
        // `None`) through `protos` instead.
        let prefetched = if prefetch_on_miss {
            Box::pin(self.prefetch_tree_protos(digest)).await
        } else {
            None
        };
        let protos = prefetched.as_ref().or(protos);
        let temp_path = self.allocate_scratch_path(TEMP_PREFIX);
        let mut temp_guard = ScratchGuard::new(temp_path.clone());
        let size = match self.construct_directory(digest, &temp_path, protos).await {
            Ok(size) => size,
            Err(e) => {
                temp_guard.disarm();
                Self::remove_tree_best_effort(&temp_path).await;
                return Err(e);
            }
        };

        // Publish. A pre-existing canonical dir can only be crash leftovers
        // from a previous process that the startup sweep has not yet
        // removed: live entries are tracked in the map (this digest has no
        // entry — we hold its construction lock and just re-checked), and
        // eviction renames trees to tombstones under the cache write lock
        // before deleting them.
        Self::remove_tree_best_effort(&cache_path).await;
        temp_guard.disarm();
        if let Err(e) = fs::rename(&temp_path, &cache_path).await {
            Self::remove_tree_best_effort(&temp_path).await;
            return Err(e)
                .err_tip(|| format!("Failed to publish cache entry: {}", cache_path.display()));
        }

        // Insert into the cache. The in-memory map mutation and the
        // metadata-cheap tombstone renames of evicted victims run under the
        // write lock (see `tombstone_victims` for why the renames must);
        // the expensive filesystem deletion is dispatched off the lock so
        // eviction I/O never serializes other callers.
        //
        // The new entry is inserted pre-pinned. The hardlink-to-destination
        // below runs unlocked, and a concurrent caller for an unrelated
        // digest could otherwise pick this brand-new entry as an eviction
        // victim and delete its tree mid-hardlink. Dropping the pin releases
        // it once the hardlink is done.
        let ref_count = Arc::new(AtomicUsize::new(1));
        let pin = EntryPin {
            ref_count: Arc::clone(&ref_count),
        };
        let tombstones = {
            let mut cache = self.cache.write().await;
            let victims = self.evict_if_needed(size, &mut cache);
            let replaced = cache.insert(
                digest,
                CachedDirectoryMetadata {
                    path: cache_path.clone(),
                    size,
                    last_access: AtomicU64::new(unix_nanos_now()),
                    ref_count,
                },
            );
            // Maintain the lock-free metric mirrors. A replaced entry cannot
            // happen (we hold the construction lock and just re-checked), but
            // account for it defensively so the mirrors can never drift.
            if let Some(old) = replaced {
                self.map_size_bytes.fetch_sub(old.size, Ordering::Relaxed);
            } else {
                self.map_entries.fetch_add(1, Ordering::Relaxed);
            }
            self.map_size_bytes.fetch_add(size, Ordering::Relaxed);
            self.tombstone_victims(victims).await
        };
        Self::dispatch_evictions(tombstones);

        // Hardlink to destination (unlocked). The entry is pinned so it
        // cannot be evicted from under this hardlink.
        let result = hardlink_directory_tree(&cache_path, dest_path).await;
        drop(pin);
        let method = result.err_tip(|| "Failed to hardlink newly cached directory")?;
        self.record_clone_method(method);

        Ok((false, size))
    }

    /// Removes a cache entry whose on-disk tree failed to materialize
    /// (missing or corrupt), renames the tree to a tombstone, and deletes it
    /// in the background — the same mechanism eviction uses. Without this, a
    /// damaged entry would stay in the map and fail every future request for
    /// its digest until it happened to be evicted.
    ///
    /// `pin` is the guard returned by `acquire_entry` for the failed
    /// attempt: invalidation is skipped if the map now holds a DIFFERENT
    /// entry for this digest (single-flight already rebuilt it), identified
    /// by refcount-handle pointer identity.
    async fn invalidate_entry(&self, digest: &DigestInfo, pin: &EntryPin) {
        let tombstones = {
            let mut cache = self.cache.write().await;
            let is_same_entry = cache
                .get(digest)
                .is_some_and(|m| Arc::ptr_eq(&m.ref_count, &pin.ref_count));
            if !is_same_entry {
                return;
            }
            let Some(metadata) = cache.remove(digest) else {
                return;
            };
            self.map_entries.fetch_sub(1, Ordering::Relaxed);
            self.map_size_bytes
                .fetch_sub(metadata.size, Ordering::Relaxed);
            self.tombstone_victims(vec![metadata.path]).await
        };
        Self::dispatch_evictions(tombstones);
    }

    /// Best-effort `remove_dir_all` that treats `NotFound` as success and
    /// only warns on other failures. Used to clear partially populated
    /// destinations before a retry, partially built temp trees after a
    /// failed construction, and stale crash leftovers before publishing.
    async fn remove_tree_best_effort(path: &Path) {
        if let Err(e) = fs::remove_dir_all(path).await
            && e.kind() != std::io::ErrorKind::NotFound
        {
            warn!(?path, error = ?e, "Failed to remove partial directory tree");
        }
    }

    /// Allocates a process-unique scratch path under `cache_root` (temp
    /// construction trees, eviction tombstones). Unique within the process
    /// via a monotonic counter; leftovers from previous processes are
    /// removed by the startup sweep in [`Self::new`].
    fn allocate_scratch_path(&self, prefix: &str) -> PathBuf {
        let seq = self.scratch_seq.fetch_add(1, Ordering::Relaxed);
        self.config.cache_root.join(format!("{prefix}{seq}"))
    }

    /// Renames each evicted tree to a unique tombstone path and returns the
    /// tombstones for background deletion.
    ///
    /// MUST be called while still holding the cache write lock that removed
    /// the victims from the map (or before the cache is shared, as in the
    /// startup sweep). That ordering is what makes eviction race-free
    /// against re-construction of an evicted digest: a constructor only
    /// constructs after a cache re-check, the re-check needs the read lock,
    /// so it is ordered after these renames — the background deletion can
    /// therefore never touch a canonical path a fresh construction publishes
    /// to. The rename itself is metadata-only and cheap; the expensive
    /// `remove_dir_all` still happens off-lock in the background.
    async fn tombstone_victims(&self, victims: Vec<PathBuf>) -> Vec<PathBuf> {
        let mut tombstones = Vec::with_capacity(victims.len());
        for path in victims {
            let tombstone = self.allocate_scratch_path(TOMBSTONE_PREFIX);
            match fs::rename(&path, &tombstone).await {
                Ok(()) => tombstones.push(tombstone),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Already gone from disk; nothing to delete.
                }
                Err(e) => {
                    warn!(
                        ?path,
                        error = ?e,
                        "Failed to tombstone evicted directory, deleting in place"
                    );
                    tombstones.push(path);
                }
            }
        }
        tombstones
    }

    /// If `digest` is cached, pins it against eviction and returns a
    /// snapshot of its on-disk path, its recorded size, and the RAII pin.
    /// Runs under the cache READ lock — the refcount and LRU timestamp are
    /// atomics, so hits do not serialize on the write lock. The increment is
    /// race-free against eviction because eviction requires the write lock,
    /// which excludes every read-lock holder: an entry with an outstanding
    /// pin is always observed with a non-zero count by `evict_lru`.
    async fn acquire_entry(&self, digest: &DigestInfo) -> Option<(PathBuf, u64, EntryPin)> {
        let cache = self.cache.read().await;
        let metadata = cache.get(digest)?;
        metadata
            .last_access
            .store(unix_nanos_now(), Ordering::Relaxed);
        metadata.ref_count.fetch_add(1, Ordering::SeqCst);
        Some((
            metadata.path.clone(),
            metadata.size,
            EntryPin {
                ref_count: Arc::clone(&metadata.ref_count),
            },
        ))
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
    /// Prefetches every `Directory` proto of `root`'s tree with a logical
    /// `GetTree` traversal, following `next_page_token` when the server
    /// paginates it, and keys each proto by its re-computed digest (the
    /// responses carry protos, not digests). Returns `None` — meaning
    /// "use the per-level fetch path" — when the feature is disabled, the
    /// slow tier is not a `GrpcStore`, or the stream fails; a `Some` map
    /// may also be incomplete, which `construct_directory` tolerates by
    /// fetching any missing proto individually. One fetch permit covers
    /// the entire paginated traversal.
    async fn prefetch_tree_protos(
        &self,
        root: DigestInfo,
    ) -> Option<HashMap<DigestInfo, ProtoDirectory>> {
        if !self.config.experimental_get_tree_prefetch {
            return None;
        }
        let grpc_store = self
            .cas_store
            .slow_store()
            .downcast_ref::<GrpcStore>(None)?;
        let digest_hasher = opentelemetry::Context::current()
            .get::<DigestHasherFunc>()
            .map_or_else(default_digest_hasher_func, |v| *v);
        let result: Result<HashMap<DigestInfo, ProtoDirectory>, Error> = async {
            let _permit = self.acquire_fetch_permit().await?;
            let mut protos = HashMap::new();
            let mut page_token = String::new();
            loop {
                let mut stream = grpc_store
                    .get_tree(tonic::Request::new(GetTreeRequest {
                        instance_name: String::new(),
                        root_digest: Some(root.into()),
                        page_size: 0,
                        page_token,
                        digest_function: digest_hasher.proto_digest_func().into(),
                    }))
                    .await
                    .err_tip(|| "in prefetch_tree_protos")?
                    .into_inner();
                let mut next_page_token = String::new();
                while let Some(response) = stream
                    .message()
                    .await
                    .map_err(Error::from)
                    .err_tip(|| "reading GetTree stream in prefetch_tree_protos")?
                {
                    next_page_token = response.next_page_token;
                    for directory in response.directories {
                        // GetTree yields protos without digests; recompute with
                        // the caller's digest function so lookups match the
                        // digests embedded in parent directories.
                        let encoded = directory.encode_to_vec();
                        let mut hasher = digest_hasher.hasher();
                        hasher.update(&encoded);
                        protos.insert(hasher.finalize_digest(), directory);
                    }
                }
                if next_page_token.is_empty() {
                    break;
                }
                page_token = next_page_token;
            }
            Ok(protos)
        }
        .await;
        match result {
            Ok(protos) => {
                trace!(?root, protos = protos.len(), "GetTree prefetch complete");
                Some(protos)
            }
            Err(err) => {
                debug!(
                    ?err,
                    ?root,
                    "GetTree prefetch failed; using per-level fetches"
                );
                None
            }
        }
    }

    fn construct_directory<'a>(
        &'a self,
        digest: DigestInfo,
        dest_path: &'a Path,
        protos: Option<&'a HashMap<DigestInfo, ProtoDirectory>>,
    ) -> Pin<Box<dyn Future<Output = Result<u64, Error>> + Send + 'a>> {
        Box::pin(async move {
            debug!(?digest, ?dest_path, "Constructing directory");

            // Use the prefetched proto when available; otherwise fetch it
            // (permit held only for the fetch). A prefetch-map miss (e.g.
            // an incomplete GetTree response) degrades to the fetch path.
            let prefetched = protos.and_then(|map| map.get(&digest));
            let directory: ProtoDirectory = if let Some(directory) = prefetched {
                directory.clone()
            } else {
                let _permit = self.acquire_fetch_permit().await?;
                get_and_decode_digest(self.cas_store.as_ref(), digest.into())
                    .await
                    .err_tip(|| format!("Failed to fetch directory digest: {digest:?}"))?
            };

            // Create the destination directory. It must be writable while it
            // is being populated; 0o755 is its final mode too, so set it now
            // (umask-independent) — no post-construction permission walk.
            self.create_dir_writable(dest_path).await?;

            let mut total_size: u64 = 0;
            for file in &directory.files {
                if let Some(file_digest) = &file.digest {
                    // size_bytes is non-negative; clamp defensively.
                    total_size += u64::try_from(file_digest.size_bytes).unwrap_or(0);
                }
            }

            // Materialize all nodes of this directory level concurrently,
            // sharing one concurrency budget — the same shape as
            // `download_to_directory`. Every file that is not in the fast
            // tier pays a full slow-store round trip, so awaiting nodes
            // sequentially serializes N round trips and dominates cold-cache
            // construction time for large trees. Futures yield the subtree
            // size they materialized (0 for files, whose sizes are already
            // summed from their digests above, and for symlinks).
            let mut node_futures: Vec<BoxFuture<'_, Result<u64, Error>>> = Vec::with_capacity(
                directory.files.len() + directory.directories.len() + directory.symlinks.len(),
            );
            for file in &directory.files {
                node_futures.push(Box::pin(async move {
                    self.create_file(dest_path, file).await.map(|()| 0)
                }));
            }
            for dir_node in &directory.directories {
                node_futures.push(Box::pin(
                    self.create_subdirectory(dest_path, dir_node, protos),
                ));
            }
            for symlink in &directory.symlinks {
                node_futures.push(Box::pin(async move {
                    self.create_symlink(dest_path, symlink).await.map(|()| 0)
                }));
            }
            total_size += futures::stream::iter(node_futures)
                .buffer_unordered(CONSTRUCT_DIRECTORY_CONCURRENCY)
                .try_fold(0u64, |acc, size| async move { Ok(acc + size) })
                .await?;

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
        // but still made read-only (0o444) by copy_file_to so the materialized
        // input is immutable like the hardlink path.
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
        // on-disk file we can hardlink. This may fetch from the slow store,
        // so it holds a fetch permit.
        {
            let _permit = self.acquire_fetch_permit().await?;
            self.cas_store
                .populate_fast_store(StoreKey::Digest(*digest))
                .await
                .err_tip(|| format!("Failed to populate fast store for {digest}"))?;
        }

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
    /// `file_path`, then chmods it read-only (`0o555` when `executable`, else
    /// `0o444`). This is safe because the copy has its own inode, unshared with
    /// the CAS, so the chmod cannot mutate a shared blob.
    async fn copy_file_to(
        &self,
        digest: &DigestInfo,
        file_path: &Path,
        executable: bool,
    ) -> Result<(), Error> {
        let data = {
            let _permit = self.acquire_fetch_permit().await?;
            self.cas_store
                .get_part_unchunked(StoreKey::Digest(*digest), 0, None)
                .await
                .err_tip(|| format!("Failed to fetch file: {}", file_path.display()))?
        };

        fs::write(file_path, data.as_ref())
            .await
            .err_tip(|| format!("Failed to write file: {}", file_path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Read-only, matching the hermeticity contract for materialized
            // inputs: 0o555 (r-xr-xr-x) for executables so the +x bit survives,
            // 0o444 (r--r--r--) for data files. This file has its own private
            // inode (just written above), so chmoding it cannot affect any CAS
            // blob or another action's hardlink — unlike the hardlink path,
            // where the shared read-only blob must never be chmod'd.
            let mode = if executable { 0o555 } else { 0o444 };
            fs::set_permissions(file_path, std::fs::Permissions::from_mode(mode))
                .await
                .err_tip(|| format!("Failed to set permissions: {}", file_path.display()))?;
        }
        #[cfg(not(unix))]
        let _ = executable;

        Ok(())
    }

    /// Creates a subdirectory from a `DirectoryNode`, returning the total size
    /// of the subtree it materializes.
    ///
    /// With `experimental_subtree_caching` enabled, the subtree is looked up
    /// in (and inserted into) the cache by its own `Directory` digest, so a
    /// subtree already constructed under any previous root is one
    /// `hardlink_directory_tree` call instead of a full recursive rebuild.
    /// When disabled (the default), the subtree is always constructed
    /// directly under the parent with no cache interaction — today's
    /// behavior, unchanged.
    async fn create_subdirectory(
        &self,
        parent: &Path,
        dir_node: &DirectoryNode,
        protos: Option<&HashMap<DigestInfo, ProtoDirectory>>,
    ) -> Result<u64, Error> {
        let dir_path = parent.join(&dir_node.name);
        let digest =
            DigestInfo::try_from(dir_node.digest.clone().ok_or_else(|| {
                make_err!(Code::InvalidArgument, "Directory node missing digest")
            })?)
            .err_tip(|| "Invalid directory digest")?;

        trace!(?dir_path, ?digest, "Creating subdirectory");

        if self.config.experimental_subtree_caching {
            // Subtree caching: look the subtree up in (and insert it into)
            // the cache by its own digest through the same core flow root
            // materializations use. REAPI `Directory` nodes are
            // content-addressed, so a subtree digest seen under one root is
            // byte-identical wherever else it appears; construction recurses
            // through this method, so nested subtrees get their own entries
            // too (leaf-level reuse).
            let (hit, _size) = self
                .get_or_create_entry(digest, &dir_path, protos, false)
                .await?;
            let counter = if hit {
                &self.subtree_hits
            } else {
                &self.subtree_misses
            };
            counter.fetch_add(1, Ordering::Relaxed);
            // The child's cache entry owns its bytes: contribute 0 to the
            // parent's recorded size, so an entry's size covers only bytes
            // not owned by a descendant entry and the sum over the map
            // approximates unique materialized bytes. Counting the real
            // subtree size here instead would tally every file once per
            // ancestor level (~depth-fold inflation of `max_size_bytes`
            // pressure) and make the cache self-defeat via phantom
            // evictions.
            return Ok(0);
        }

        // Recursively construct subdirectory
        self.construct_directory(digest, &dir_path, protos).await
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
            let Some((e_size, path)) = Self::evict_lru(cache) else {
                // nothing evictable (all entries pinned) — have to exit
                warn!(
                    current_items = cache.len(),
                    max_entries = self.config.max_entries,
                    "Unable to evict anything from directory_cache, will exceed max entries"
                );
                break;
            };
            self.evictions.fetch_add(1, Ordering::Relaxed);
            self.map_entries.fetch_sub(1, Ordering::Relaxed);
            self.map_size_bytes.fetch_sub(e_size, Ordering::Relaxed);
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
                self.evictions.fetch_add(1, Ordering::Relaxed);
                self.map_entries.fetch_sub(1, Ordering::Relaxed);
                self.map_size_bytes.fetch_sub(e_size, Ordering::Relaxed);
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
            .filter(|(_, m)| m.ref_count.load(Ordering::SeqCst) == 0)
            .min_by_key(|(_, m)| m.last_access.load(Ordering::Relaxed))
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
                match fs::metadata(&path).await {
                    Err(_) => {
                        // Already gone (e.g. a prior cleanup, or the cache
                        // root was torn down). Nothing to do, not an error.
                        continue;
                    }
                    Ok(metadata) if !metadata.is_dir() => {
                        // Stray file (only plausible from an orphan sweep of
                        // a polluted cache root).
                        if let Err(e) = fs::remove_file(&path).await {
                            warn!(?path, error = ?e, "Failed to remove evicted file from disk");
                        }
                        continue;
                    }
                    Ok(_) => {}
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
        let in_use = cache
            .values()
            .filter(|m| m.ref_count.load(Ordering::SeqCst) > 0)
            .count();

        CacheStats {
            entries: cache.len(),
            total_size_bytes: total_size,
            in_use_entries: in_use,
            clonefile_hits: self.clonefile_hits.load(Ordering::Relaxed),
            hardlink_hits: self.hardlink_hits.load(Ordering::Relaxed),
            subtree_hits: self.subtree_hits.load(Ordering::Relaxed),
            subtree_misses: self.subtree_misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }
}

impl MetricsComponent for DirectoryCache {
    fn publish(
        &self,
        _kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        let _enter = group!(field_metadata.name).entered();
        publish!(
            "clonefile_hits",
            &self.clonefile_hits,
            MetricKind::Counter,
            "Materializations that used APFS clonefile(2) (macOS only)"
        );
        publish!(
            "hardlink_hits",
            &self.hardlink_hits,
            MetricKind::Counter,
            "Materializations that used per-file hardlinks"
        );
        publish!(
            "subtree_hits",
            &self.subtree_hits,
            MetricKind::Counter,
            "Subtree materializations served from the cache (experimental_subtree_caching)"
        );
        publish!(
            "subtree_misses",
            &self.subtree_misses,
            MetricKind::Counter,
            "Subtree materializations that had to be constructed (experimental_subtree_caching)"
        );
        publish!(
            "evictions",
            &self.evictions,
            MetricKind::Counter,
            "Cache entries removed by LRU eviction (high rate = cache thrash)"
        );
        publish!(
            "entries",
            &self.map_entries,
            MetricKind::Default,
            "Current number of cached directory entries"
        );
        publish!(
            "total_size_bytes",
            &self.map_size_bytes,
            MetricKind::Default,
            "Current recorded size of all cached entries in bytes"
        );
        Ok(MetricPublishKnownKindData::Component)
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
    /// Subtree materializations served from the cache by digest. Always zero
    /// unless `experimental_subtree_caching` is enabled.
    pub subtree_hits: u64,
    /// Subtree constructions not servable from the cache. Always zero unless
    /// `experimental_subtree_caching` is enabled.
    pub subtree_misses: u64,
    /// Entries removed by LRU eviction. A high rate relative to hits means
    /// the cache is thrashing and `max_entries`/`max_size_bytes` should be
    /// raised.
    pub evictions: u64,
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
                bypass_dedup_threshold_bytes: 0,
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
            ..Default::default()
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
            ..Default::default()
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

        // Lock the cached tree down the way DirectoryCache does after
        // construction (set_readonly_recursive). This makes every file
        // read-only (0o555) and leaves every directory writable (0o755).
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

    /// Builds a nested directory tree in the CAS: a root directory containing
    /// one file plus a subdirectory, and the subdirectory in turn containing a
    /// file. Returns the `FastSlowStore` and the root directory's digest. Uses
    /// the same `make_fast_slow_store` + `upload_blob` shape as the other tests
    /// so the store matches `DirectoryCache::new`'s `Arc<FastSlowStore>` arg.
    ///
    /// Only used by `test_materialized_tree_dirs_writable_files_readonly`,
    /// which is `#[cfg(unix)]`; gated to match so non-unix builds (Windows)
    /// do not flag this helper as dead code.
    #[cfg(unix)]
    async fn setup_nested_test_store(temp_dir: &TempDir) -> (Arc<FastSlowStore>, DigestInfo) {
        let (cas_store, slow_store) = make_fast_slow_store(temp_dir).await;

        // A file shared by both the root and the nested subdirectory.
        let file_digest = upload_blob(&slow_store, 10, b"Hello, World!").await;

        // The nested subdirectory: contains a single file.
        let subdir = ProtoDirectory {
            files: vec![FileNode {
                name: "nested.txt".to_string(),
                digest: Some(file_digest.into()),
                is_executable: false,
                ..Default::default()
            }],
            directories: vec![],
            symlinks: vec![],
            ..Default::default()
        };
        let mut subdir_data = Vec::new();
        subdir.encode(&mut subdir_data).unwrap();
        let subdir_digest = upload_blob(&slow_store, 11, &subdir_data).await;

        // The root directory: one file plus the subdirectory above.
        let root = ProtoDirectory {
            files: vec![FileNode {
                name: "root.txt".to_string(),
                digest: Some(file_digest.into()),
                is_executable: false,
                ..Default::default()
            }],
            directories: vec![DirectoryNode {
                name: "subdir".to_string(),
                digest: Some(subdir_digest.into()),
            }],
            symlinks: vec![],
            ..Default::default()
        };
        let mut root_data = Vec::new();
        root.encode(&mut root_data).unwrap();
        let root_digest = upload_blob(&slow_store, 12, &root_data).await;

        (cas_store, root_digest)
    }

    /// Asserts every directory in `root` (the root itself and every nested
    /// subdirectory) is writable and every file is read-only.
    #[cfg(unix)]
    fn assert_dirs_writable_files_readonly(
        root: &Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + '_>> {
        Box::pin(async move {
            use std::os::unix::fs::PermissionsExt;

            let metadata = fs::symlink_metadata(root)
                .await
                .err_tip(|| format!("metadata for {}", root.display()))?;
            let mode = metadata.permissions().mode() & 0o777;

            if metadata.is_dir() {
                assert_eq!(
                    mode & 0o200,
                    0o200,
                    "directory {} must be writable (mode 0o{mode:o})",
                    root.display(),
                );
                let mut entries = fs::read_dir(root).await?;
                while let Some(entry) = entries.next_entry().await? {
                    assert_dirs_writable_files_readonly(&entry.path()).await?;
                }
            } else if metadata.is_file() {
                assert_eq!(
                    mode & 0o222,
                    0,
                    "file {} must be read-only (mode 0o{mode:o})",
                    root.display(),
                );
            }
            // Symlinks: mode is not meaningful, skip.

            Ok(())
        })
    }

    /// After `get_or_create` materializes a tree — on both the fresh
    /// cache-miss path and the cache-hit path — every directory in the
    /// destination must be writable (so Bazel actions can create outputs at
    /// nested declared paths) and every file must be read-only (the file
    /// inodes are CAS-hardlinked; chmoding them would corrupt the shared
    /// inode). `prepare_action_inputs` relies on this so it no longer needs a
    /// separate `set_dir_writable_recursive` post-walk.
    #[cfg(unix)]
    #[nativelink_test]
    async fn test_materialized_tree_dirs_writable_files_readonly() -> Result<(), Error> {
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, root_digest) = setup_nested_test_store(&temp_dir).await;

        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
            ..Default::default()
        };
        let cache = DirectoryCache::new(config, store).await?;

        // Fresh-materialize path (cache miss).
        let miss_dest = temp_dir.path().join("dest_miss");
        let hit = cache.get_or_create(root_digest, &miss_dest).await?;
        assert!(!hit, "first access must be a cache miss");
        assert!(miss_dest.join("subdir").join("nested.txt").exists());
        assert_dirs_writable_files_readonly(&miss_dest).await?;

        // A nested output can be created with no separate chmod walk.
        let nested_output = miss_dest.join("subdir").join("output.o");
        fs::write(&nested_output, b"action output").await.err_tip(
            || "creating a nested output must succeed without set_dir_writable_recursive",
        )?;

        // Cache-hit path: a second materialization of the same digest.
        let hit_dest = temp_dir.path().join("dest_hit");
        let hit = cache.get_or_create(root_digest, &hit_dest).await?;
        assert!(hit, "second access must be a cache hit");
        assert!(hit_dest.join("subdir").join("nested.txt").exists());
        assert_dirs_writable_files_readonly(&hit_dest).await?;

        // The cache-hit destination also accepts a nested output directly.
        fs::write(hit_dest.join("subdir").join("output.o"), b"action output")
            .await
            .err_tip(|| "cache-hit destination must accept a nested output directly")?;

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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        };
        let cache = Arc::new(DirectoryCache::new(config, store).await?);

        // Fire 16 concurrent requests for the same digest, each to its own
        // destination.
        #[allow(clippy::items_after_statements)]
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

    #[nativelink_test]
    async fn get_tree_prefetch_falls_back_without_grpc_store() -> Result<(), Error> {
        // With the flag on but a non-grpc slow tier, prefetch must return
        // None and construction must fall back to per-level fetches with
        // identical results.
        let temp_dir = TempDir::new().unwrap();
        let cache_root = temp_dir.path().join("cache");
        let (store, dir_digest) = setup_test_store(&temp_dir).await;
        let config = DirectoryCacheConfig {
            max_entries: 10,
            max_size_bytes: 1024 * 1024,
            cache_root,
            experimental_get_tree_prefetch: true,
            ..Default::default()
        };
        let cache = DirectoryCache::new(config, store).await?;
        let dest = temp_dir.path().join("dest");
        assert!(!cache.get_or_create(dir_digest, &dest).await?);
        assert!(dest.join("test.txt").exists());
        Ok(())
    }
}
