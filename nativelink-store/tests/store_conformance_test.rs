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

//! Store Topology & Fault-Injection Conformance Harness.
//!
//! This suite executes a shared battery of contract, fault-injection, and
//! self-healing invariants across a matrix of leaf and multi-layer composed
//! store topologies using realistic production defaults (e.g. `FilesystemSpec`
//! with its default 4 KiB `block_size`).
//!
//! ```text
//!                    +-----------------------------------------------+
//!                    |         6 Conformance Contract Suites         |
//!                    |  1. Round-trip & exact `has()` content size   |
//!                    |  2. Cold fast-tier fill across slow backends  |
//!                    |  3. Torn / invalid upload rejection & safety  |
//!                    |  4. Retryable error code preservation         |
//!                    |  5. Self-healing `remove()` cache repair      |
//!                    |  6. Concurrent fill & 4 KiB eviction stress   |
//!                    +-----------------------+-----------------------+
//!                                            |
//!                                            v
//!                    +-----------------------------------------------+
//!                    |                VerifyStore                    |
//!                    |     (size + hash check, triggers remove())    |
//!                    +-----------------------+-----------------------+
//!                                            |
//!                                            v
//!                    +-----------------------------------------------+
//!                    |              ExistenceCacheStore              |
//!                    +-----------------------+-----------------------+
//!                                            |
//!                                            v
//!                    +-----------------------------------------------+
//!                    |                 FastSlowStore                 |
//!                    +-----------+-----------------------+-----------+
//!                                |                       |
//!               fast tier (cache)|                       |slow tier (durable)
//!                                v                       v
//!        +-------------------------------+       +-------------------------------+
//!        |  CacheMetrics(allow_remove)   |       |       CompressionStore        |
//!        |  Shard / SizePartitioning     |       |         (LZ4 frames)          |
//!        |  RefStore                     |       +---------------+---------------+
//!        +---------------+---------------+                       |
//!                        |                                       v
//!                        v                       +-------------------------------+
//!        +-------------------------------+       |          DedupStore           |
//!        |      FaultInjectingStore      |       |     (FastCDC index + chunks)  |
//!        |  - TruncateReadAtBytes        |       +---------------+---------------+
//!        |  - FailUpdateAfterBytes       |                       |
//!        |  - FailGetAfterBytes          |                       v
//!        +---------------+---------------+       +-------------------------------+
//!                        |                       |      FaultInjectingStore      |
//!                        v                       +---------------+---------------+
//!        +-------------------------------+                       |
//!        |  FilesystemStore (4 KiB blk)  |                       v
//!        |  or MemoryStore               |       +-------------------------------+
//!        +-------------------------------+       |  FilesystemStore (4 KiB blk)  |
//!                                                |  or MemoryStore               |
//!                                                +-------------------------------+
//! ```

use core::pin::Pin;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use futures::future::join_all;
use nativelink_config::stores::{
    CacheMetricsSpec, CompressionAlgorithm, CompressionSpec, DedupSpec, EvictionPolicy,
    ExistenceCacheSpec, FastSlowSpec, FilesystemSpec, Lz4Config, MemorySpec, RefSpec, ShardConfig,
    ShardSpec, SizePartitioningSpec, StoreDirection, StoreSpec, VerifySpec,
};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_store::cache_metrics_store::CacheMetricsStore;
use nativelink_store::compression_store::CompressionStore;
use nativelink_store::dedup_store::DedupStore;
use nativelink_store::existence_cache_store::ExistenceCacheStore;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::filesystem_store::{FileEntryImpl, FilesystemStore};
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::ref_store::RefStore;
use nativelink_store::shard_store::ShardStore;
use nativelink_store::size_partitioning_store::SizePartitioningStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_store::verify_store::VerifyStore;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair,
};
use nativelink_util::common::{DigestInfo, make_temp_path};
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    RemoveCallback, Store, StoreDriver, StoreKey, StoreLike, StoreOptimizations, UploadSizeInfo,
};
use pretty_assertions::assert_eq;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

/// Fault-injection modes for [`FaultInjectingStore`].
#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
enum FaultMode {
    #[default]
    PassThrough,
    /// Fail `update` after reading `fail_after_bytes` from the stream.
    FailUpdateAfterBytes { fail_after_bytes: usize, code: Code },
    /// Fail `get_part` after forwarding `fail_after_bytes` to the writer.
    FailGetAfterBytes { fail_after_bytes: usize, code: Code },
    /// Truncate `get_part` cleanly (sending EOF) after `truncate_to_bytes`.
    TruncateReadAtBytes { truncate_to_bytes: usize },
    /// Append extra trailing garbage bytes on `get_part` before EOF.
    AppendExtraBytesOnRead { extra_bytes: Bytes },
    /// Override `has_with_results` to report a custom size for present keys.
    OverrideHasSize { reported_size: u64 },
    /// Fail `has_with_results` with a specific error code.
    FailHas { code: Code },
}

/// A deterministic fault-injecting [`StoreDriver`] wrapper that can wrap any
/// leaf or composite store tier to simulate network drops, torn streams,
/// truncated blobs, corrupted sizes, and transient backend outages.
#[derive(Debug, MetricsComponent)]
struct FaultInjectingStore {
    inner: Store,
    mode: Mutex<FaultMode>,
    has_calls: AtomicUsize,
    update_calls: AtomicUsize,
    get_calls: AtomicUsize,
    remove_calls: AtomicUsize,
    forward_optimizations: AtomicBool,
}

impl FaultInjectingStore {
    fn new(inner: Store) -> Arc<Self> {
        Arc::new(Self {
            inner,
            mode: Mutex::new(FaultMode::PassThrough),
            has_calls: AtomicUsize::new(0),
            update_calls: AtomicUsize::new(0),
            get_calls: AtomicUsize::new(0),
            remove_calls: AtomicUsize::new(0),
            forward_optimizations: AtomicBool::new(false),
        })
    }

    fn set_mode(&self, mode: FaultMode) {
        *self.mode.lock().unwrap() = mode;
    }

    fn current_mode(&self) -> FaultMode {
        self.mode.lock().unwrap().clone()
    }
}

#[async_trait]
impl StoreDriver for FaultInjectingStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        self.inner.clone().into_inner().post_init().await
    }

    async fn remove(self: Pin<&Self>, key: StoreKey<'_>) -> Result<bool, Error> {
        self.remove_calls.fetch_add(1, Ordering::SeqCst);
        self.inner.as_store_driver_pin().remove(key).await
    }

    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        self.has_calls.fetch_add(1, Ordering::SeqCst);
        match self.current_mode() {
            FaultMode::FailHas { code } => {
                Err(make_err!(code, "Injected has_with_results failure"))
            }
            FaultMode::OverrideHasSize { reported_size } => {
                self.inner.has_with_results(digests, results).await?;
                for slot in results.iter_mut().flatten() {
                    *slot = reported_size;
                }
                Ok(())
            }
            _ => self.inner.has_with_results(digests, results).await,
        }
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
    ) -> Result<u64, Error> {
        self.update_calls.fetch_add(1, Ordering::SeqCst);
        match self.current_mode() {
            FaultMode::FailUpdateAfterBytes {
                fail_after_bytes,
                code,
            } => {
                let mut read_so_far = 0usize;
                while read_so_far < fail_after_bytes {
                    let chunk = reader
                        .recv()
                        .await
                        .err_tip(|| "In FaultInjectingStore::update")?;
                    if chunk.is_empty() {
                        break;
                    }
                    read_so_far += chunk.len();
                }
                Err(make_err!(
                    code,
                    "Injected update failure after {read_so_far} bytes"
                ))
            }
            _ => self.inner.update(key, reader, upload_size).await,
        }
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        self.get_calls.fetch_add(1, Ordering::SeqCst);
        match self.current_mode() {
            FaultMode::FailGetAfterBytes {
                fail_after_bytes,
                code,
            } => {
                let (mut tx, mut rx) = make_buf_channel_pair();
                let inner_store = self.inner.clone();
                let owned_key = key.into_owned();
                let read_fut = async move {
                    inner_store
                        .get_part(owned_key, &mut tx, offset, length)
                        .await
                };
                let forward_fut = async move {
                    let mut forwarded = 0usize;
                    loop {
                        let chunk = rx
                            .recv()
                            .await
                            .err_tip(|| "In FaultInjectingStore::get_part")?;
                        if chunk.is_empty() {
                            break;
                        }
                        let remaining = fail_after_bytes.saturating_sub(forwarded);
                        if remaining > 0 {
                            let to_send = chunk.len().min(remaining);
                            writer.send(chunk.slice(0..to_send)).await?;
                        }
                        forwarded += chunk.len();
                        if forwarded >= fail_after_bytes {
                            return Err(make_err!(
                                code,
                                "Injected get_part failure after {forwarded} bytes"
                            ));
                        }
                    }
                    writer.send_eof()
                };
                let (read_res, forward_res) = tokio::join!(read_fut, forward_fut);
                forward_res.merge(read_res)
            }
            FaultMode::TruncateReadAtBytes { truncate_to_bytes } => {
                let bytes = self.inner.get_part_unchunked(key, offset, length).await?;
                let end = bytes.len().min(truncate_to_bytes);
                if end > 0 {
                    writer.send(bytes.slice(0..end)).await?;
                }
                writer.send_eof()
            }
            FaultMode::AppendExtraBytesOnRead { extra_bytes } => {
                let bytes = self.inner.get_part_unchunked(key, offset, length).await?;
                if !bytes.is_empty() {
                    writer.send(bytes).await?;
                }
                writer.send(extra_bytes).await?;
                writer.send_eof()
            }
            _ => self.inner.get_part(key, writer, offset, length).await,
        }
    }

    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        if self.forward_optimizations.load(Ordering::SeqCst) {
            self.inner.optimized_for(optimization)
        } else {
            false
        }
    }

    fn inner_store(&self, _digest: Option<StoreKey<'_>>) -> &dyn StoreDriver {
        self
    }

    fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_remove_callback(self: Arc<Self>, callback: RemoveCallback) -> Result<(), Error> {
        self.inner.register_remove_callback(callback)
    }
}

default_health_status_indicator!(FaultInjectingStore);

/// Computes a SHA-256 [`DigestInfo`] for `payload`.
fn sha256_digest(payload: &[u8]) -> DigestInfo {
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(payload);
    hasher.finalize_digest()
}

/// Generates deterministic pseudo-random bytes of length `len` for `seed`.
fn deterministic_bytes(seed: u64, len: usize) -> Bytes {
    let mut buf = vec![0u8; len];
    let mut rng = SmallRng::seed_from_u64(seed);
    rng.fill(&mut buf[..]);
    Bytes::from(buf)
}

/// Builds a `FilesystemStore` using **production default** settings (including
/// the default 4 KiB `block_size`, not `block_size: 1`).
async fn new_default_filesystem_store(tag: &str) -> Result<Store, Error> {
    let spec = FilesystemSpec {
        content_path: make_temp_path(&format!("conformance_content_{tag}")),
        temp_path: make_temp_path(&format!("conformance_temp_{tag}")),
        ..Default::default()
    };
    Ok(Store::new(
        FilesystemStore::<FileEntryImpl>::new(&spec).await?,
    ))
}

fn new_memory_store() -> Store {
    Store::new(MemoryStore::new(&MemorySpec::default()))
}

fn wrap_verify(inner: Store, verify_hash: bool) -> Store {
    Store::new(VerifyStore::new(
        &VerifySpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            verify_size: true,
            verify_hash,
        },
        inner,
    ))
}

fn wrap_compression(inner: Store) -> Result<Store, Error> {
    Ok(Store::new(CompressionStore::new(
        &CompressionSpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            compression_algorithm: CompressionAlgorithm::Lz4(Lz4Config::default()),
        },
        inner,
    )?))
}

fn wrap_cache_metrics(inner: Store, allow_remove: bool) -> Store {
    Store::new(CacheMetricsStore::new(
        &CacheMetricsSpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            cache_type: "conformance_tier".to_string(),
            allow_remove,
        },
        inner,
    ))
}

fn wrap_existence_cache(inner: Store) -> Store {
    Store::new(ExistenceCacheStore::new(
        &ExistenceCacheSpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            eviction_policy: Some(EvictionPolicy {
                max_bytes: 0,
                max_count: 10_000,
                ..Default::default()
            }),
        },
        inner,
    ))
}

fn wrap_fast_slow(fast: Store, slow: Store) -> Store {
    Store::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::Both,
            slow_direction: StoreDirection::Both,
            bypass_dedup_threshold_bytes: 0,
        },
        fast,
        slow,
    ))
}

fn wrap_shard(stores: Vec<Store>) -> Result<Store, Error> {
    let spec = ShardSpec {
        stores: stores
            .iter()
            .map(|_| ShardConfig {
                store: StoreSpec::Memory(MemorySpec::default()),
                weight: Some(1),
            })
            .collect(),
    };
    Ok(Store::new(ShardStore::new(&spec, stores)?))
}

fn wrap_size_partitioning(lower: Store, upper: Store, threshold: u64) -> Store {
    Store::new(SizePartitioningStore::new(
        &SizePartitioningSpec {
            size: threshold,
            lower_store: StoreSpec::Memory(MemorySpec::default()),
            upper_store: StoreSpec::Memory(MemorySpec::default()),
        },
        lower,
        upper,
    ))
}

fn wrap_dedup(index_store: Store, content_store: Store) -> Result<Store, Error> {
    Ok(Store::new(DedupStore::new(
        &DedupSpec {
            index_store: StoreSpec::Memory(MemorySpec::default()),
            content_store: StoreSpec::Memory(MemorySpec::default()),
            min_size: 512,
            normal_size: 1024,
            max_size: 2048,
            max_concurrent_fetch_per_get: 4,
        },
        index_store,
        content_store,
    )?))
}

/// Named store topology under test.
struct StoreTopology {
    name: &'static str,
    store: Store,
    /// Keeps `StoreManager` alive for topologies that use `RefStore`.
    _store_manager: Option<Arc<StoreManager>>,
}

/// Constructs the matrix of single-tier and multi-tier store compositions.
async fn build_topology_matrix() -> Result<Vec<StoreTopology>, Error> {
    let mut topologies = Vec::new();

    // 1. Leaf stores with default configs.
    topologies.push(StoreTopology {
        name: "memory",
        store: new_memory_store(),
        _store_manager: None,
    });
    topologies.push(StoreTopology {
        name: "filesystem_default_4k_block",
        store: new_default_filesystem_store("leaf_fs").await?,
        _store_manager: None,
    });

    // 2. Transforming & routing wrappers over default FilesystemStore.
    topologies.push(StoreTopology {
        name: "compression_lz4 -> filesystem_default_4k",
        store: wrap_compression(new_default_filesystem_store("comp_fs").await?)?,
        _store_manager: None,
    });
    topologies.push(StoreTopology {
        name: "dedup -> (memory, filesystem_default_4k)",
        store: wrap_dedup(
            new_memory_store(),
            new_default_filesystem_store("dedup_fs").await?,
        )?,
        _store_manager: None,
    });
    topologies.push(StoreTopology {
        name: "shard(filesystem_default_4k, memory)",
        store: wrap_shard(vec![
            new_default_filesystem_store("shard_0").await?,
            new_memory_store(),
        ])?,
        _store_manager: None,
    });
    topologies.push(StoreTopology {
        name: "size_partitioning(2KiB: filesystem_default_4k, compression -> memory)",
        store: wrap_size_partitioning(
            new_default_filesystem_store("sp_low").await?,
            wrap_compression(new_memory_store())?,
            2048,
        ),
        _store_manager: None,
    });

    // 3. Multi-tier FastSlow compositions (including the exact #2999 config:
    // worker FastSlow(fast: Filesystem, slow: RefStore -> Filesystem)).
    let store_manager = Arc::new(StoreManager::new());
    let cas_fs = new_default_filesystem_store("shared_cas_fs").await?;
    store_manager.add_store("SHARED_CAS", cas_fs)?;
    let ref_store = RefStore::new(
        &RefSpec {
            name: "SHARED_CAS".to_string(),
        },
        Arc::downgrade(&store_manager),
    );
    ref_store.clone().post_init().await?;
    topologies.push(StoreTopology {
        name: "verify -> fast_slow(fast: filesystem_4k, slow: ref_store -> filesystem_4k) [#2999]",
        store: wrap_verify(
            wrap_fast_slow(
                new_default_filesystem_store("worker_fast_fs").await?,
                Store::new(ref_store),
            ),
            true,
        ),
        _store_manager: Some(store_manager),
    });

    // 4. Deep production-style stack:
    // Verify -> ExistenceCache -> FastSlow(
    //   fast: CacheMetrics -> Shard(Filesystem, Filesystem),
    //   slow: Compression -> Dedup(Memory, Filesystem)
    // )
    let fast_shard = wrap_shard(vec![
        new_default_filesystem_store("deep_fast_0").await?,
        new_default_filesystem_store("deep_fast_1").await?,
    ])?;
    let slow_dedup = wrap_compression(wrap_dedup(
        new_memory_store(),
        new_default_filesystem_store("deep_slow_content").await?,
    )?)?;
    topologies.push(StoreTopology {
        name: "verify -> existence_cache -> fast_slow(cache_metrics -> shard(fs), compression -> dedup(mem, fs))",
        store: wrap_verify(
            wrap_existence_cache(wrap_fast_slow(
                wrap_cache_metrics(fast_shard, true),
                slow_dedup,
            )),
            true,
        ),
        _store_manager: None,
    });

    Ok(topologies)
}

// ============================================================================
// Contract 1: Exact Content Size & Round-Trip Across Payload Boundaries
// (Catches #2960 CompressionStore::has() compressed size, #2993 incompressible
// LZ4 > digest size, and #2999 FilesystemStore 4 KiB block rounding)
// ============================================================================

#[nativelink_test]
async fn conformance_round_trip_and_has_size_contract() -> Result<(), Error> {
    let topologies = build_topology_matrix().await?;
    // Sizes chosen to exercise sub-block (1, 146), exact 4K block (4096),
    // non-multiple block (4097), multi-chunk LZ4 (65536 + 333), and incompressible random data.
    let payload_sizes = [1usize, 146, 2047, 2048, 4095, 4096, 4097, 68_869];

    for topo in &topologies {
        for (idx, &size) in payload_sizes.iter().enumerate() {
            let payload = deterministic_bytes(1000 + idx as u64, size);
            let digest = sha256_digest(&payload);

            // Upload payload.
            topo.store
                .update_oneshot(digest, payload.clone())
                .await
                .err_tip(|| format!("update_oneshot failed on {} (size={size})", topo.name))?;

            // Contract: has() MUST return Some(exact logical payload length),
            // NEVER the 4 KiB disk block-rounded size or LZ4 compressed frame size.
            let reported_size = topo
                .store
                .has(digest)
                .await
                .err_tip(|| format!("has() failed on {} (size={size})", topo.name))?;
            assert_eq!(
                reported_size,
                Some(size as u64),
                "Topology '{}' violated has() size contract for payload of {} bytes (got {:?})",
                topo.name,
                size,
                reported_size
            );

            // Contract: unbounded get_part_unchunked(0, None) returns exact bytes.
            let read_back = topo
                .store
                .get_part_unchunked(digest, 0, None)
                .await
                .err_tip(|| format!("unbounded get failed on {} (size={size})", topo.name))?;
            assert_eq!(
                read_back, payload,
                "Topology '{}' corrupted unbounded read for size={}",
                topo.name, size
            );

            // Contract: partial range reads return the exact slice.
            if size > 20 {
                let offset = 7usize;
                let length = (size - offset).min(31);
                let slice = topo
                    .store
                    .get_part_unchunked(digest, offset as u64, Some(length as u64))
                    .await
                    .err_tip(|| format!("sliced get_part failed on {} (size={size})", topo.name))?;
                assert_eq!(
                    slice,
                    payload.slice(offset..offset + length),
                    "Topology '{}' returned wrong slice for offset={offset}, len={length}",
                    topo.name
                );
            }
        }
    }
    Ok(())
}

// ============================================================================
// Contract 2: Cold Fast-Tier Cache Fill From Every Slow-Tier Backend
// (Directly reproduces #2999: FastSlowStore with cold fast tier and 4 KiB
// FilesystemStore / CompressionStore / ShardStore / DedupStore slow tier)
// ============================================================================

#[nativelink_test]
async fn conformance_cold_fast_tier_cache_fill_across_slow_backends() -> Result<(), Error> {
    struct SlowBackendCase {
        name: &'static str,
        slow_store: Store,
        _store_manager: Option<Arc<StoreManager>>,
    }

    let store_manager = Arc::new(StoreManager::new());
    let shared_fs = new_default_filesystem_store("cold_fill_shared_fs").await?;
    store_manager.add_store("CAS_FS", shared_fs)?;
    let ref_to_fs = RefStore::new(
        &RefSpec {
            name: "CAS_FS".to_string(),
        },
        Arc::downgrade(&store_manager),
    );
    ref_to_fs.clone().post_init().await?;

    let slow_cases = vec![
        SlowBackendCase {
            name: "filesystem_default_4k_block",
            slow_store: new_default_filesystem_store("cold_fill_direct_fs").await?,
            _store_manager: None,
        },
        SlowBackendCase {
            name: "ref_store -> filesystem_default_4k_block [#2999]",
            slow_store: Store::new(ref_to_fs),
            _store_manager: Some(store_manager),
        },
        SlowBackendCase {
            name: "compression_lz4 -> filesystem_default_4k_block",
            slow_store: wrap_compression(new_default_filesystem_store("cold_fill_comp_fs").await?)?,
            _store_manager: None,
        },
        SlowBackendCase {
            name: "shard(filesystem_default_4k_block, memory)",
            slow_store: wrap_shard(vec![
                new_default_filesystem_store("cold_fill_shard_fs").await?,
                new_memory_store(),
            ])?,
            _store_manager: None,
        },
        SlowBackendCase {
            name: "dedup(memory, filesystem_default_4k_block)",
            slow_store: wrap_dedup(
                new_memory_store(),
                new_default_filesystem_store("cold_fill_dedup_fs").await?,
            )?,
            _store_manager: None,
        },
    ];

    // Test sub-4KiB (146 bytes from #2999), 1 byte, 4096 bytes, and 10 KiB.
    for case in &slow_cases {
        let fast_store = new_default_filesystem_store(&format!(
            "cold_fast_{}",
            case.name
                .replace([' ', ':', '>', '[', ']', '#', ',', '(', ')'], "_")
        ))
        .await?;
        let fast_slow = wrap_verify(
            wrap_fast_slow(fast_store.clone(), case.slow_store.clone()),
            true,
        );

        for (seed, size) in [(1u64, 1usize), (2, 146), (3, 4095), (4, 4096), (5, 10_245)] {
            let payload = deterministic_bytes(seed, size);
            let digest = sha256_digest(&payload);

            // Populate ONLY the slow store (simulating a cold worker or evicted fast tier).
            case.slow_store
                .update_oneshot(digest, payload.clone())
                .await
                .err_tip(|| format!("Failed to seed slow store '{}' (size={size})", case.name))?;
            assert_eq!(
                fast_store.has(digest).await?,
                None,
                "Fast store must start cold for {}",
                case.name
            );

            // Read through FastSlowStore: this must populate fast_store and stream back
            // without failing with `Expected 4096 upload bytes, received 146`.
            let fetched = fast_slow
                .get_part_unchunked(digest, 0, None)
                .await
                .err_tip(|| {
                    format!(
                        "Cold fast-tier cache fill failed for slow backend '{}' at size={size}",
                        case.name
                    )
                })?;
            assert_eq!(
                fetched, payload,
                "Data mismatch on cold fill for {}",
                case.name
            );
            assert_eq!(
                fast_store.has(digest).await?,
                Some(size as u64),
                "Fast store should hold exact size after fill for {}",
                case.name
            );
        }
    }

    Ok(())
}

// ============================================================================
// Contract 3: Torn / Short / Oversized Upload Rejection & Existing-Value Safety
// (Catches #2974, #2881, #2951: short or oversized writes must fail and must
// NEVER clobber a previously committed valid blob or leave a partial blob live)
// ============================================================================

#[nativelink_test]
async fn conformance_torn_and_invalid_uploads_preserve_existing_data() -> Result<(), Error> {
    let topologies = build_topology_matrix().await?;

    for topo in &topologies {
        // 3a. Short upload to a fresh digest must be rejected and not published.
        let fresh_payload = deterministic_bytes(42, 512);
        let fresh_digest = sha256_digest(&fresh_payload);

        let (mut tx, rx) = make_buf_channel_pair();
        let send_short = async move {
            tx.send(fresh_payload.slice(0..200)).await?;
            tx.send_eof()
        };
        let (_, short_res) = tokio::join!(
            send_short,
            topo.store
                .update(fresh_digest, rx, UploadSizeInfo::ExactSize(512))
        );
        assert!(
            short_res.is_err(),
            "Topology '{}' accepted a truncated 200/512 byte upload",
            topo.name
        );
        assert_eq!(
            topo.store.has(fresh_digest).await?,
            None,
            "Topology '{}' published a truncated upload under {}",
            topo.name,
            fresh_digest
        );

        // 3b. Empty payload under a non-empty digest must be rejected (#2974 / #2993).
        let (mut tx_empty, rx_empty) = make_buf_channel_pair();
        let send_empty = async move { tx_empty.send_eof() };
        let (_, empty_res) = tokio::join!(
            send_empty,
            topo.store
                .update(fresh_digest, rx_empty, UploadSizeInfo::ExactSize(512))
        );
        assert!(
            empty_res.is_err(),
            "Topology '{}' accepted a 0-byte upload for a 512-byte digest",
            topo.name
        );
        assert_eq!(
            topo.store.has(fresh_digest).await?,
            None,
            "Topology '{}' published a 0-byte object for a non-empty digest",
            topo.name
        );

        // 3b-2. Oversized upload to a fresh digest must be rejected and not published.
        for size_info in [UploadSizeInfo::ExactSize(512), UploadSizeInfo::MaxSize(512)] {
            let (mut tx_over, rx_over) = make_buf_channel_pair();
            let oversized_fresh = deterministic_bytes(43, 700);
            let send_over = async move {
                tx_over.send(oversized_fresh).await?;
                tx_over.send_eof()
            };
            let (_, over_res) = tokio::join!(
                send_over,
                topo.store.update(fresh_digest, rx_over, size_info)
            );
            assert!(
                over_res.is_err(),
                "Topology '{}' accepted an oversized 700/512 byte upload ({size_info:?})",
                topo.name
            );
            assert_eq!(
                topo.store.has(fresh_digest).await?,
                None,
                "Topology '{}' published an oversized upload under {} ({size_info:?})",
                topo.name,
                fresh_digest
            );
        }

        // 3c. Commit a valid blob, then attempt a torn overwrite and an oversized overwrite;
        // the original valid blob must remain intact and readable.
        let committed_payload = deterministic_bytes(99, 1024);
        let committed_digest = sha256_digest(&committed_payload);
        topo.store
            .update_oneshot(committed_digest, committed_payload.clone())
            .await?;

        let (mut tx_torn, rx_torn) = make_buf_channel_pair();
        let bad_prefix = deterministic_bytes(100, 300);
        let send_torn = async move {
            tx_torn.send(bad_prefix).await?;
            tx_torn.send_eof()
        };
        let (_, torn_overwrite_res) = tokio::join!(
            send_torn,
            topo.store
                .update(committed_digest, rx_torn, UploadSizeInfo::ExactSize(1024))
        );
        assert!(
            torn_overwrite_res.is_err(),
            "Topology '{}' accepted a torn overwrite",
            topo.name
        );

        // 3c-2. Attempt an oversized overwrite (1500 bytes against a 1024-byte digest).
        for size_info in [
            UploadSizeInfo::ExactSize(1024),
            UploadSizeInfo::MaxSize(1024),
        ] {
            let (mut tx_over_commit, rx_over_commit) = make_buf_channel_pair();
            let oversized_overwrite = deterministic_bytes(101, 1500);
            let send_over_commit = async move {
                tx_over_commit.send(oversized_overwrite).await?;
                tx_over_commit.send_eof()
            };
            let (_, over_overwrite_res) = tokio::join!(
                send_over_commit,
                topo.store
                    .update(committed_digest, rx_over_commit, size_info)
            );
            assert!(
                over_overwrite_res.is_err(),
                "Topology '{}' accepted an oversized 1500/1024 byte overwrite ({size_info:?})",
                topo.name
            );
        }

        // Verify the previously committed blob is untouched after both short and oversized overwrites.
        let surviving = topo
            .store
            .get_part_unchunked(committed_digest, 0, None)
            .await
            .err_tip(|| {
                format!(
                    "Existing blob lost after invalid overwrites on {}",
                    topo.name
                )
            })?;
        assert_eq!(
            surviving, committed_payload,
            "Topology '{}' corrupted existing blob after failed overwrites",
            topo.name
        );
    }

    Ok(())
}

// ============================================================================
// Contract 4: Retryable Error Code Preservation Across Store Stacks
// (Catches #2915, #2966, #2815, #2816: `Unavailable` / `ResourceExhausted`
// mid-stream failures must not be masked as `Internal` or `InvalidArgument`)
// ============================================================================

#[nativelink_test]
async fn conformance_retryable_error_codes_preserved_through_wrappers() -> Result<(), Error> {
    let retryable_codes = [Code::Unavailable, Code::ResourceExhausted, Code::Aborted];

    for &expected_code in &retryable_codes {
        let leaf = new_memory_store();
        let fault_store = FaultInjectingStore::new(leaf);

        // Stack: Verify -> ExistenceCache -> CacheMetrics -> FastSlow(fast: Memory, slow: Compression -> FaultInjectingStore)
        let stack = wrap_verify(
            wrap_existence_cache(wrap_cache_metrics(
                wrap_fast_slow(
                    new_memory_store(),
                    wrap_compression(Store::new(fault_store.clone()))?,
                ),
                true,
            )),
            false,
        );

        let payload = deterministic_bytes(777, 16 * 1024);
        let digest = sha256_digest(&payload);

        // Inject mid-stream upload failure in the slow backend.
        fault_store.set_mode(FaultMode::FailUpdateAfterBytes {
            fail_after_bytes: 64,
            code: expected_code,
        });

        let err = stack
            .update_oneshot(digest, payload.clone())
            .await
            .expect_err("Upload should fail when slow backend fails mid-stream");
        assert_eq!(
            err.code, expected_code,
            "Wrapper stack masked retryable upload error {expected_code:?} as {:?}: {err:?}",
            err.code
        );

        // Now allow upload to succeed to the backend, then inject mid-stream read failure
        // through Verify -> ExistenceCache -> CacheMetrics -> SizePartitioning -> Compression.
        fault_store.set_mode(FaultMode::PassThrough);
        let compressed_backend = wrap_compression(Store::new(fault_store.clone()))?;
        compressed_backend
            .update_oneshot(digest, payload.clone())
            .await?;

        let read_stack = wrap_verify(
            wrap_existence_cache(wrap_cache_metrics(
                wrap_size_partitioning(new_memory_store(), compressed_backend, 1024),
                true,
            )),
            true,
        );
        fault_store.set_mode(FaultMode::FailGetAfterBytes {
            fail_after_bytes: 32,
            code: expected_code,
        });

        let read_err = read_stack
            .get_part_unchunked(digest, 0, None)
            .await
            .expect_err("Read should fail when backend drops stream mid-read");
        assert_eq!(
            read_err.code, expected_code,
            "Wrapper stack masked retryable read error {expected_code:?} as {:?}: {read_err:?}",
            read_err.code
        );
    }

    Ok(())
}

// ============================================================================
// Contract 5: Self-Healing Cache Repair Through Wrapper Topologies
// (Catches #2968: when the fast tier serves a truncated/corrupted entry,
// `VerifyStore` returns `DataLoss`, calls `remove()` through intermediate
// wrappers to evict the bad fast copy without deleting the durable slow copy,
// and the very next read succeeds!)
// ============================================================================

#[nativelink_test]
async fn conformance_self_healing_cache_repair_across_wrappers() -> Result<(), Error> {
    struct RepairCase {
        name: &'static str,
        /// The top-level `Verify -> ... -> FastSlow(fast, slow)` store.
        top_store: Store,
        /// The underlying fast leaf store so we can verify actual eviction and refill.
        fast_leaf: Store,
        /// Fault injector sitting in the fast tier to simulate a truncated cached copy.
        fast_fault: Arc<FaultInjectingStore>,
        /// The underlying durable slow tier (protected from `remove()`).
        durable_slow: Store,
        /// Keep `StoreManager` alive for `RefStore` cases.
        _store_manager: Option<Arc<StoreManager>>,
    }

    let mut cases = Vec::new();

    // Case 1: Verify -> FastSlow(fast: CacheMetrics(allow_remove=true) -> Fault -> Memory,
    //                            slow: CacheMetrics(allow_remove=false) -> Memory)
    {
        let fast_leaf = new_memory_store();
        let fast_fault = FaultInjectingStore::new(fast_leaf.clone());
        let fast = wrap_cache_metrics(Store::new(fast_fault.clone()), true);
        let slow_leaf = new_memory_store();
        let slow = wrap_cache_metrics(slow_leaf.clone(), false);
        cases.push(RepairCase {
            name: "cache_metrics(allow_remove=true) fast tier + protected slow tier [#2968]",
            top_store: wrap_verify(wrap_fast_slow(fast, slow), true),
            fast_leaf,
            fast_fault,
            durable_slow: slow_leaf,
            _store_manager: None,
        });
    }

    // Case 2: Verify -> FastSlow(fast: SizePartitioning -> Fault -> Filesystem, slow: protected)
    {
        let fast_leaf = new_default_filesystem_store("repair_sp_fast").await?;
        let fast_fault = FaultInjectingStore::new(fast_leaf.clone());
        let fast = wrap_size_partitioning(new_memory_store(), Store::new(fast_fault.clone()), 1024);
        let slow_leaf = new_default_filesystem_store("repair_sp_slow").await?;
        let slow = wrap_cache_metrics(slow_leaf.clone(), false);
        cases.push(RepairCase {
            name: "size_partitioning -> filesystem fast tier + protected slow tier",
            top_store: wrap_verify(wrap_fast_slow(fast, slow), true),
            fast_leaf,
            fast_fault,
            durable_slow: slow_leaf,
            _store_manager: None,
        });
    }

    // Case 3: Verify -> FastSlow(fast: RefStore -> CacheMetrics(allow_remove=true) -> Fault -> Memory,
    //                            slow: protected)
    {
        let store_manager = Arc::new(StoreManager::new());
        let fast_leaf = new_memory_store();
        let fast_fault = FaultInjectingStore::new(fast_leaf.clone());
        let fast_metrics = wrap_cache_metrics(Store::new(fast_fault.clone()), true);
        store_manager.add_store("REPAIR_FAST", fast_metrics)?;
        let ref_fast = RefStore::new(
            &RefSpec {
                name: "REPAIR_FAST".to_string(),
            },
            Arc::downgrade(&store_manager),
        );
        ref_fast.clone().post_init().await?;

        let slow_leaf = new_memory_store();
        let slow = wrap_cache_metrics(slow_leaf.clone(), false);
        cases.push(RepairCase {
            name: "ref_store -> cache_metrics fast tier + protected slow tier",
            top_store: wrap_verify(wrap_fast_slow(Store::new(ref_fast), slow), true),
            fast_leaf,
            fast_fault,
            durable_slow: slow_leaf,
            _store_manager: Some(store_manager),
        });
    }

    let payload = deterministic_bytes(2026, 4096);
    let digest = sha256_digest(&payload);

    let mut unhealed_wrappers = Vec::new();

    for case in &cases {
        // 1. Upload valid blob through the stack so both fast and slow tiers have it.
        case.top_store
            .update_oneshot(digest, payload.clone())
            .await
            .err_tip(|| format!("Initial upload failed for {}", case.name))?;
        assert_eq!(
            case.fast_leaf.has(digest).await?,
            Some(payload.len() as u64),
            "Fast leaf must hold the blob after initial upload for {}",
            case.name
        );

        // 2. Corrupt the fast tier by truncating its reads to 10 bytes.
        case.fast_fault.set_mode(FaultMode::TruncateReadAtBytes {
            truncate_to_bytes: 10,
        });

        // 3. First read detects truncation via VerifyStore (or wrapper framing) and triggers `remove()`.
        let first_read = case.top_store.get_part_unchunked(digest, 0, None).await;
        assert!(
            first_read.is_err(),
            "Corrupted fast tier read must fail on first attempt for {}",
            case.name
        );

        // Fast leaf copy MUST be evicted before clearing the fault!
        assert_eq!(
            case.fast_leaf.has(digest).await?,
            None,
            "Cache repair failed to evict the corrupted entry from fast_leaf in {}",
            case.name
        );

        // Durable slow tier copy MUST never be deleted by cache repair!
        assert_eq!(
            case.durable_slow.has(digest).await?,
            Some(payload.len() as u64),
            "Cache repair deleted the durable slow copy in {}",
            case.name
        );

        // 4. Clear the fault injection (so a subsequent repopulation of fast tier works)
        // and verify that `remove()` reached the fast tier AND the next read heals and
        // repopulates `fast_leaf` from `durable_slow`!
        case.fast_fault.set_mode(FaultMode::PassThrough);
        let remove_count = case.fast_fault.remove_calls.load(Ordering::SeqCst);
        if remove_count == 0 {
            unhealed_wrappers.push(case.name);
            continue;
        }

        let healed = case
            .top_store
            .get_part_unchunked(digest, 0, None)
            .await
            .err_tip(|| format!("Second read after repair failed for {}", case.name))?;
        assert_eq!(
            healed, payload,
            "Second read after repair did not return valid data from slow tier for {}",
            case.name
        );
        assert_eq!(
            case.fast_leaf.has(digest).await?,
            Some(payload.len() as u64),
            "Second read after repair did not repopulate fast_leaf from durable_slow for {}",
            case.name
        );
    }

    assert!(
        unhealed_wrappers.is_empty(),
        "Cache repair (StoreDriver::remove) did not reach fast-tier storage for: {unhealed_wrappers:#?}"
    );

    Ok(())
}

// ============================================================================
// Contract 6: Concurrent Read/Write/Has/Eviction Stress Without Cascading Loss
// (Catches #2840 EvictingMap has() cascading eviction & #2926/#2928 races)
// ============================================================================

#[nativelink_test]
async fn conformance_concurrent_fill_and_eviction_invariants() -> Result<(), Error> {
    let fast_spec = FilesystemSpec {
        content_path: make_temp_path("conformance_concurrent_fast_content"),
        temp_path: make_temp_path("conformance_concurrent_fast_temp"),
        // Small eviction cap (8 x 4 KiB blocks) so 32 concurrent fills actively evict.
        eviction_policy: Some(EvictionPolicy {
            max_bytes: 32 * 1024,
            ..Default::default()
        }),
        ..Default::default()
    };
    let fast_fs = Store::new(FilesystemStore::<FileEntryImpl>::new(&fast_spec).await?);
    let slow_fs = new_default_filesystem_store("conformance_concurrent_slow").await?;
    let store = wrap_verify(wrap_fast_slow(fast_fs, slow_fs.clone()), true);

    // Pre-populate 32 distinct blobs of < 4 KiB in the slow store.
    let mut blobs = Vec::with_capacity(32);
    for i in 0..32usize {
        let data = deterministic_bytes(5000 + i as u64, 500 + (i * 13));
        let digest = sha256_digest(&data);
        slow_fs.update_oneshot(digest, data.clone()).await?;
        blobs.push((digest, data));
    }

    // Issue 32 concurrent reads and `has()` calls across the 32 digests while the
    // fast tier is thrashing and evicting 4 KiB blocks.
    let mut tasks = Vec::new();
    for (digest, expected_data) in blobs {
        let store = store.clone();
        tasks.push(async move {
            let size = store.has(digest).await?;
            assert_eq!(
                size,
                Some(expected_data.len() as u64),
                "Concurrent has() returned wrong size for {digest}"
            );
            let actual = store.get_part_unchunked(digest, 0, None).await?;
            assert_eq!(actual, expected_data, "Concurrent get corrupted {digest}");
            Result::<_, Error>::Ok(())
        });
    }

    for res in join_all(tasks).await {
        res?;
    }

    Ok(())
}
