// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use core::num::NonZeroUsize;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_lock::RwLock;
use bytes::Bytes;
use lru::LruCache;
use nativelink_config::schedulers::WorkerAllocationStrategy;
use nativelink_config::stores::{GrpcEndpoint, GrpcSpec, Retry, StoreType};
use nativelink_error::{Code, Error, ResultExt, error_if, make_err, make_input_err};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
    RootMetricsComponent, group,
};
use nativelink_proto::build::bazel::remote::execution::v2::{Digest, Directory};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    BlobsInStableStorage, PeerHint, StartExecute, UpdateForWorker, update_for_worker,
};
use nativelink_store::grpc_store::GrpcStore;
use nativelink_util::action_messages::{OperationId, WorkerId};
use nativelink_util::blob_locality_map::SharedBlobLocalityMap;
use nativelink_util::common::DigestInfo;
use nativelink_util::metrics_utils::CounterWithTime;
use nativelink_util::operation_state_manager::{UpdateOperationType, WorkerStateManager};
use nativelink_util::platform_properties::PlatformProperties;
use nativelink_util::shutdown_guard::ShutdownGuard;
use nativelink_util::store_trait::{Store, StoreKey, StoreLike};
use parking_lot::Mutex as ParkingMutex;
use prost::Message;
use tokio::sync::{Notify, Semaphore};
use tokio::sync::mpsc::UnboundedSender;
use tonic::async_trait;
use tracing::{debug, error, info, trace, warn};

/// Metrics for tracking scheduler performance.
#[derive(Debug, Default)]
pub struct SchedulerMetrics {
    /// Total number of worker additions.
    pub workers_added: AtomicU64,
    /// Total number of worker removals.
    pub workers_removed: AtomicU64,
    /// Total number of `find_worker_for_action` calls.
    pub find_worker_calls: AtomicU64,
    /// Total number of successful worker matches.
    pub find_worker_hits: AtomicU64,
    /// Total number of failed worker matches (no worker found).
    pub find_worker_misses: AtomicU64,
    /// Total time spent in `find_worker_for_action` (nanoseconds).
    pub find_worker_time_ns: AtomicU64,
    /// Total number of workers iterated during find operations.
    pub workers_iterated: AtomicU64,
    /// Total number of action dispatches.
    pub actions_dispatched: AtomicU64,
    /// Total number of keep-alive updates.
    pub keep_alive_updates: AtomicU64,
    /// Total number of worker timeouts.
    pub worker_timeouts: AtomicU64,
    /// Total number of prefetch tasks spawned.
    pub prefetch_tasks_spawned: AtomicU64,
    /// Total number of blobs successfully prefetched to workers.
    pub prefetch_blobs_sent: AtomicU64,
    /// Total bytes successfully prefetched to workers.
    pub prefetch_bytes_sent: AtomicU64,
    /// Total number of blobs that failed to prefetch.
    pub prefetch_blobs_failed: AtomicU64,
    /// Total number of blobs skipped because they were already on the worker.
    pub prefetch_blobs_already_present: AtomicU64,
    /// Total number of batch RPCs sent to workers during prefetch.
    pub prefetch_batches_sent: AtomicU64,
    /// Total number of server-side cache warm tasks spawned.
    pub cache_warm_spawned: CounterWithTime,
}

/// Cached result of `score_and_generate_hints`: endpoint scores and peer hints.
type ScoringResult = (HashMap<Arc<str>, (u64, SystemTime)>, Vec<PeerHint>);

use crate::platform_property_manager::PlatformPropertyManager;
use crate::worker::{
    ActionInfoWithProps, PendingActionInfoData, Worker, WorkerTimestamp, WorkerUpdate,
    reduce_platform_properties,
};
use crate::worker_capability_index::WorkerCapabilityIndex;
use crate::worker_registry::SharedWorkerRegistry;
use crate::worker_scheduler::WorkerScheduler;

/// Computes an effective load score for worker selection. Lower is better.
/// Workers with idle P-cores always beat workers with only idle E-cores,
/// creating a two-tier preference. Workers reporting only aggregate load
/// (Linux, old workers) compete in the P-core tier.
fn effective_load_score(p_load: u32, e_load: u32, aggregate_load: u32) -> u64 {
    if p_load > 0 || e_load > 0 {
        // Has per-core-type data.
        if p_load < 100 {
            // P-cores available: score in [0, 99].
            p_load as u64
        } else {
            // P-cores saturated, only E-cores left: score in [100, 199].
            100 + e_load as u64
        }
    } else if aggregate_load > 0 {
        // Aggregate only (Linux / old worker): treat as P-core tier.
        aggregate_load as u64
    } else {
        // Unknown: sort last.
        u64::MAX
    }
}

#[derive(Debug)]
struct Workers(LruCache<WorkerId, Worker>);

impl Deref for Workers {
    type Target = LruCache<WorkerId, Worker>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Workers {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// Note: This could not be a derive macro because this derive-macro
// does not support LruCache and nameless field structs.
impl MetricsComponent for Workers {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        let _enter = group!("workers").entered();
        for (worker_id, worker) in self.iter() {
            let _enter = group!(worker_id).entered();
            worker.publish(MetricKind::Component, MetricFieldData::default())?;
        }
        Ok(MetricPublishKnownKindData::Component)
    }
}

/// A collection of workers that are available to run tasks.
#[derive(MetricsComponent)]
struct ApiWorkerSchedulerImpl {
    /// A `LruCache` of workers available based on `allocation_strategy`.
    #[metric(group = "workers")]
    workers: Workers,

    /// The worker state manager.
    #[metric(group = "worker_state_manager")]
    worker_state_manager: Arc<dyn WorkerStateManager>,
    /// The allocation strategy for workers.
    allocation_strategy: WorkerAllocationStrategy,
    /// A channel to notify the matching engine that the worker pool has changed.
    worker_change_notify: Arc<Notify>,
    /// Worker registry for tracking worker liveness.
    worker_registry: SharedWorkerRegistry,

    /// Whether the worker scheduler is shutting down.
    shutting_down: bool,

    /// Index for fast worker capability lookup.
    /// Used to accelerate `find_worker_for_action` by filtering candidates
    /// based on properties before doing linear scan.
    capability_index: WorkerCapabilityIndex,

    /// Reverse map: CAS endpoint → WorkerId.
    /// Updated when workers are added/removed.
    endpoint_to_worker: HashMap<Arc<str>, WorkerId>,
}

impl core::fmt::Debug for ApiWorkerSchedulerImpl {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ApiWorkerSchedulerImpl")
            .field("workers", &self.workers)
            .field("allocation_strategy", &self.allocation_strategy)
            .field("worker_change_notify", &self.worker_change_notify)
            .field(
                "capability_index_size",
                &self.capability_index.worker_count(),
            )
            .field("worker_registry", &self.worker_registry)
            .field("endpoint_to_worker_len", &self.endpoint_to_worker.len())
            .finish_non_exhaustive()
    }
}

impl ApiWorkerSchedulerImpl {
    /// Refreshes the lifetime of the worker with the given timestamp.
    ///
    /// Instead of sending N keepalive messages (one per operation),
    /// we now send a single worker heartbeat. The worker registry tracks worker liveness,
    /// and timeout detection checks the worker's `last_seen` instead of per-operation timestamps.
    ///
    /// Note: This only updates the local worker state. The worker registry is updated
    /// separately after releasing the inner lock to reduce contention.
    fn refresh_lifetime(
        &mut self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
    ) -> Result<(), Error> {
        let worker = self.workers.0.peek_mut(worker_id).ok_or_else(|| {
            make_input_err!(
                "Worker not found in worker map in refresh_lifetime() {}",
                worker_id
            )
        })?;
        error_if!(
            worker.last_update_timestamp > timestamp,
            "Worker already had a timestamp of {}, but tried to update it with {}",
            worker.last_update_timestamp,
            timestamp
        );
        worker.last_update_timestamp = timestamp;

        // If the worker was in quarantine, clear it now that it has checked in.
        if worker.quarantined_at.take().is_some() {
            info!(
                ?worker_id,
                "Worker exited quarantine after sending keepalive"
            );
        }

        trace!(
            ?worker_id,
            running_operations = worker.running_action_infos.len(),
            "Worker keepalive received"
        );

        Ok(())
    }

    /// Adds a worker to the pool.
    /// Note: This function will not do any task matching.
    fn add_worker(&mut self, worker: Worker) -> Result<(), Error> {
        let worker_id = worker.id.clone();
        let platform_properties = worker.platform_properties.clone();

        // Update endpoint → worker reverse map for locality scoring.
        if !worker.cas_endpoint.is_empty() {
            self.endpoint_to_worker
                .insert(Arc::from(worker.cas_endpoint.as_str()), worker_id.clone());
        }

        self.workers.put(worker_id.clone(), worker);

        // Add to capability index for fast matching
        self.capability_index
            .add_worker(&worker_id, &platform_properties);

        // Worker is not cloneable, and we do not want to send the initial connection results until
        // we have added it to the map, or we might get some strange race conditions due to the way
        // the multi-threaded runtime works.
        let worker = self.workers.peek_mut(&worker_id).unwrap();
        let res = worker
            .send_initial_connection_result()
            .err_tip(|| "Failed to send initial connection result to worker");
        if let Err(err) = &res {
            error!(
                ?worker_id,
                ?err,
                "Worker connection appears to have been closed while adding to pool"
            );
        }
        self.worker_change_notify.notify_one();
        res
    }

    /// Removes worker from pool.
    /// Note: The caller is responsible for any rescheduling of any tasks that might be
    /// running.
    fn remove_worker(&mut self, worker_id: &WorkerId) -> Option<Worker> {
        // Remove from capability index
        self.capability_index.remove_worker(worker_id);

        let result = self.workers.pop(worker_id);

        // Remove from endpoint → worker reverse map.
        if let Some(ref worker) = result {
            if !worker.cas_endpoint.is_empty() {
                self.endpoint_to_worker.remove(worker.cas_endpoint.as_str());
            }
        }

        self.worker_change_notify.notify_one();
        result
    }

    /// Sets if the worker is draining or not.
    async fn set_drain_worker(
        &mut self,
        worker_id: &WorkerId,
        is_draining: bool,
    ) -> Result<(), Error> {
        let worker = self
            .workers
            .get_mut(worker_id)
            .err_tip(|| format!("Worker {worker_id} doesn't exist in the pool"))?;
        worker.is_draining = is_draining;
        self.worker_change_notify.notify_one();
        Ok(())
    }

    fn inner_find_worker_for_action(
        &mut self,
        platform_properties: &PlatformProperties,
        full_worker_logging: bool,
    ) -> Option<WorkerId> {
        // Do a fast check to see if any workers are available at all for work allocation
        if !self.workers.iter().any(|(_, w)| w.can_accept_work()) {
            if full_worker_logging {
                info!("All workers are fully allocated");
            }
            return None;
        }

        // Use capability index to get candidate workers that match STATIC properties
        // (Exact, Unknown) and have the required property keys (Priority, Minimum).
        // This reduces complexity from O(W × P) to O(P × log(W)) for exact properties.
        let candidates = self
            .capability_index
            .find_matching_workers(platform_properties, full_worker_logging);

        if candidates.is_empty() {
            if full_worker_logging {
                debug!("No workers in capability index match required properties");
            }
            return None;
        }

        // Clear is_paused for candidate workers that now have capacity,
        // but only if they were paused due to a capacity check (not explicit
        // worker backpressure like ResourceExhausted). Workers that reported
        // ResourceExhausted should remain paused until they complete an action.
        for wid in &candidates {
            if let Some(worker) = self.workers.0.peek_mut(wid) {
                if worker.is_paused && !worker.is_draining && !worker.paused_due_to_backpressure {
                    let has_capacity = worker.max_inflight_tasks == 0
                        || u64::try_from(worker.running_action_infos.len()).unwrap_or(u64::MAX)
                            < worker.max_inflight_tasks;
                    if has_capacity {
                        worker.is_paused = false;
                    }
                }
            }
        }

        // Check function for availability AND dynamic Minimum property verification.
        // The index only does presence checks for Minimum properties since their
        // values change dynamically as jobs are assigned to workers.
        let worker_matches = |(worker_id, w): &(&WorkerId, &Worker)| -> bool {
            // Quarantined workers must not receive new actions.
            if w.quarantined_at.is_some() {
                if full_worker_logging {
                    debug!(
                        "Worker {worker_id} is quarantined, skipping for new work"
                    );
                }
                return false;
            }

            if !w.can_accept_work() {
                if full_worker_logging {
                    debug!(
                        "Worker {worker_id} cannot accept work: is_paused={}, is_draining={}, inflight={}/{}",
                        w.is_paused,
                        w.is_draining,
                        w.running_action_infos.len(),
                        w.max_inflight_tasks
                    );
                }
                return false;
            }

            // Verify Minimum properties at runtime (their values are dynamic)
            if !platform_properties.is_satisfied_by(&w.platform_properties, full_worker_logging) {
                return false;
            }

            true
        };

        // Now check constraints on filtered candidates.
        // Iterate in LRU order based on allocation strategy.
        // Note: iter() does not promote entries in the LRU. We find the worker
        // first via iter(), then promote it via get_mut() below to avoid
        // multiple consecutive actions all matching the same "least recently used" worker.
        let workers_iter = self.workers.iter();

        // Collect viable candidates with their effective load score for selection.
        // effective_load_score produces a two-tier ranking: idle P-cores beat
        // idle E-cores, and aggregate-only workers compete in the P-core tier.
        let viable: Vec<_> = match self.allocation_strategy {
            WorkerAllocationStrategy::LeastRecentlyUsed => workers_iter
                .rev()
                .filter(|(worker_id, _)| candidates.contains(worker_id))
                .filter(|pair| worker_matches(pair))
                .map(|(_, w)| (w.id.clone(), effective_load_score(w.p_core_load_pct, w.e_core_load_pct, w.cpu_load_pct)))
                .collect(),
            WorkerAllocationStrategy::MostRecentlyUsed => workers_iter
                .filter(|(worker_id, _)| candidates.contains(worker_id))
                .filter(|pair| worker_matches(pair))
                .map(|(_, w)| (w.id.clone(), effective_load_score(w.p_core_load_pct, w.e_core_load_pct, w.cpu_load_pct)))
                .collect(),
        };

        // Pick the lightest-loaded worker among viable candidates.
        // Workers with score == u64::MAX (unknown) are sorted last.
        // Falls back to LRU/MRU order when no workers have reported load.
        let worker_id = if viable.iter().any(|(_, score)| *score < u64::MAX) {
            viable
                .iter()
                .min_by_key(|(_, score)| *score)
                .map(|(id, _)| id.clone())
        } else {
            viable.first().map(|(id, _)| id.clone())
        };

        // Log load-aware selection decision.
        if let Some(ref wid) = worker_id {
            let viable_loads: Vec<_> = viable
                .iter()
                .map(|(id, score)| {
                    let short_id = id.0.chars().take(12).collect::<String>();
                    (short_id, *score)
                })
                .collect();
            let winner_score = viable
                .iter()
                .find(|(id, _)| id == wid)
                .map(|(_, s)| *s)
                .unwrap_or(0);
            info!(
                candidates = viable.len(),
                worker_id = %wid,
                winner_load_score = winner_score,
                ?viable_loads,
                "Load-aware worker selection"
            );
        }

        // Promote the found worker in the LRU so the next find_worker_for_action
        // call won't pick the same worker again (prevents work bunching).
        if let Some(ref wid) = worker_id {
            self.workers.get_mut(wid);
        }

        if full_worker_logging && worker_id.is_none() {
            debug!("No workers matched!");
        }
        worker_id
    }

    /// Atomically finds a suitable worker AND reserves it for the given
    /// operation by mutating the worker's state (reducing platform properties,
    /// inserting into `running_action_infos`). Returns the worker ID, the
    /// channel sender, and pre-built protobuf message so the caller can
    /// send the notification after releasing the lock.
    ///
    /// Uses locality-aware scheduling:
    /// - Primary: score candidates by total bytes of cached input blobs
    ///   using pre-computed endpoint scores (computed outside the lock).
    /// - Fallback: existing LRU/MRU strategy.
    ///
    /// This prevents two concurrent match operations from selecting the
    /// same worker, which is the key enabler for `MATCH_CONCURRENCY > 1`.
    ///
    /// `endpoint_scores` and `peer_hints` are pre-computed outside the write
    /// lock to avoid holding it during O(files) iterations over the locality
    /// map. Both are passed by reference from a shared `Arc<ScoringResult>`
    /// to avoid cloning per action match — the proto clone is deferred to
    /// `prepare_worker_run_action` and only happens when a worker is found.
    fn inner_find_and_reserve_worker(
        &mut self,
        platform_properties: &PlatformProperties,
        operation_id: &OperationId,
        action_info: &ActionInfoWithProps,
        full_worker_logging: bool,
        endpoint_scores: Option<&HashMap<Arc<str>, (u64, SystemTime)>>,
        peer_hints: &[PeerHint],
        resolved_tree: Option<&ResolvedTree>,
        pre_computed_tree: Option<(Vec<Directory>, Vec<Digest>)>,
    ) -> Option<(WorkerId, UnboundedSender<UpdateForWorker>, UpdateForWorker)> {
        let input_root_digest = action_info.inner.input_root_digest;

        // Build the set of capability-matching candidates that can accept work.
        let candidates = self
            .capability_index
            .find_matching_workers(platform_properties, full_worker_logging);

        if candidates.is_empty() {
            if full_worker_logging {
                debug!("No workers in capability index match required properties");
            }
            return None;
        }

        // Helper: check if a specific worker is a valid candidate.
        let worker_is_viable = |worker_id: &WorkerId| -> bool {
            if !candidates.contains(worker_id) {
                return false;
            }
            let Some(w) = self.workers.0.peek(worker_id) else {
                return false;
            };
            if w.quarantined_at.is_some() || !w.can_accept_work() {
                return false;
            }
            platform_properties.is_satisfied_by(&w.platform_properties, false)
        };

        // Workers above this load score are excluded from cache-affinity
        // tiers — the CPU cost outweighs the I/O savings from cache hits.
        const CACHE_AFFINITY_LOAD_CUTOFF: u64 = 99;

        // ── Tier 1: Exact root match ──
        // If a viable worker has the action's input_root_digest in its directory
        // cache (either as a root or as a subtree of a previously cached tree),
        // it can hardlink the entire input tree in milliseconds instead of
        // reconstructing it from CAS. Workers above the load cutoff are
        // excluded; among the rest, pick the lightest-loaded.
        let dir_cache_winner: Option<WorkerId> = {
            let mut best: Option<(WorkerId, u64)> = None; // (id, load_score)
            let mut best_overloaded: Option<(WorkerId, u64)> = None; // least-loaded among overloaded
            for wid in &candidates {
                if let Some(w) = self.workers.0.peek(wid) {
                    let has_root_match = w.cached_directory_digests.contains(&input_root_digest);
                    let has_subtree_match = w.cached_subtree_digests.contains(&input_root_digest);
                    if (has_root_match || has_subtree_match)
                        && worker_is_viable(wid)
                    {
                        let score = effective_load_score(w.p_core_load_pct, w.e_core_load_pct, w.cpu_load_pct);
                        if score > CACHE_AFFINITY_LOAD_CUTOFF {
                            let dominated = best_overloaded.as_ref().is_some_and(|(_, s)| score >= *s);
                            if !dominated {
                                best_overloaded = Some((wid.clone(), score));
                            }
                            continue;
                        }
                        let dominated = best.as_ref().is_some_and(|(_, best_score)| {
                            score >= *best_score
                        });
                        if !dominated {
                            best = Some((wid.clone(), score));
                        }
                    }
                }
            }
            // If no candidate is under the cutoff, pick the least-loaded
            // among overloaded cache matches — still better than a cache-cold
            // worker from the LRU fallback.
            if best.is_none() {
                if let Some((ref wid, score)) = best_overloaded {
                    warn!(
                        ?wid,
                        load_score = score,
                        cutoff = CACHE_AFFINITY_LOAD_CUTOFF,
                        %input_root_digest,
                        "Directory cache hit -- all matches overloaded, picking least-loaded"
                    );
                }
                best = best_overloaded;
            }
            if let Some((ref wid, score)) = best {
                if score <= CACHE_AFFINITY_LOAD_CUTOFF {
                    info!(
                        ?wid,
                        load_score = score,
                        %input_root_digest,
                        "Directory cache hit -- worker has input_root_digest cached (root or subtree), giving scheduling priority"
                    );
                }
            }
            best.map(|(wid, _)| wid)
        };

        // ── Tier 1.5: Partial subtree coverage scoring ──
        // When no worker has the exact root cached, score workers by a blended
        // metric of cached bytes and cached file count. Each cached file is
        // worth PER_FILE_WEIGHT bytes in the score because hardlink/clonefile
        // operations have a fixed per-file I/O cost (~0.1ms each, equivalent
        // to ~100KB of network transfer at 10Gbps).
        const PER_FILE_WEIGHT: u64 = 100 * 1024; // 100KB per file
        let subtree_coverage_winner: Option<WorkerId> = if dir_cache_winner.is_some() {
            None // exact match found, skip coverage scoring
        } else if let Some(tree) = resolved_tree {
            let total_bytes: u64 = tree.subtree_bytes.get(&input_root_digest).copied().unwrap_or(0);
            let total_files: u64 = tree.subtree_files.get(&input_root_digest).copied().unwrap_or(0);
            let total_score = total_bytes + total_files * PER_FILE_WEIGHT;
            if tree.dir_digests.len() <= 1 || total_score == 0 {
                None // only root (or empty), no subtrees to match
            } else {
                // (id, cached_score, cached_bytes, cached_files, load_score)
                let mut best: Option<(WorkerId, u64, u64, u64, u64)> = None;
                let mut best_overloaded: Option<(WorkerId, u64, u64, u64, u64)> = None;
                for wid in &candidates {
                    if let Some(w) = self.workers.0.peek(wid) {
                        if !worker_is_viable(wid) {
                            continue;
                        }
                        // Sum bytes and files for each of the action's directory
                        // digests that this worker has cached.
                        let (cached_bytes, cached_files): (u64, u64) = tree.dir_digests.iter()
                            .filter(|d| w.cached_subtree_digests.contains(d))
                            .fold((0u64, 0u64), |(ab, af), d| {
                                (
                                    ab + tree.subtree_bytes.get(d).copied().unwrap_or(0),
                                    af + tree.subtree_files.get(d).copied().unwrap_or(0),
                                )
                            });
                        let cached_score = cached_bytes + cached_files * PER_FILE_WEIGHT;
                        if cached_score == 0 {
                            continue;
                        }
                        let load_score = effective_load_score(w.p_core_load_pct, w.e_core_load_pct, w.cpu_load_pct);
                        if load_score > CACHE_AFFINITY_LOAD_CUTOFF {
                            // Track best among overloaded for soft fallback.
                            let dominated = best_overloaded.as_ref().is_some_and(|(_, bs, _, _, bl)| {
                                if cached_score != *bs { return cached_score < *bs; }
                                load_score >= *bl
                            });
                            if !dominated {
                                best_overloaded = Some((wid.clone(), cached_score, cached_bytes, cached_files, load_score));
                            }
                            continue;
                        }
                        let dominated = best.as_ref().is_some_and(|(_, best_score, _, _, best_load)| {
                            if cached_score != *best_score {
                                return cached_score < *best_score;
                            }
                            // Same cache score — prefer lower load score.
                            load_score >= *best_load
                        });
                        if !dominated {
                            best = Some((wid.clone(), cached_score, cached_bytes, cached_files, load_score));
                        }
                    }
                }
                // If no candidate is under the cutoff, pick the least-loaded
                // among overloaded cache matches — still better than a
                // cache-cold worker from the LRU fallback.
                let used_overloaded = best.is_none() && best_overloaded.is_some();
                if best.is_none() {
                    best = best_overloaded;
                }
                if let Some((ref wid, cached_score, cached_bytes, cached_files, load_score)) = best {
                    let pct = if total_score > 0 { cached_score * 100 / total_score } else { 0 };
                    if used_overloaded {
                        warn!(
                            ?wid,
                            load_score,
                            cutoff = CACHE_AFFINITY_LOAD_CUTOFF,
                            cached_score,
                            coverage_pct = pct,
                            %input_root_digest,
                            "Subtree coverage -- all candidates overloaded, picking least-loaded cache match"
                        );
                    } else {
                        info!(
                            ?wid,
                            cached_bytes,
                            cached_files,
                            total_bytes,
                            total_files,
                            cached_score,
                            total_score,
                            load_score,
                            coverage_pct = pct,
                            %input_root_digest,
                            "Subtree coverage winner -- worker has {}% of input tree (bytes+files) cached",
                            pct,
                        );
                    }
                }
                best.map(|(wid, _, _, _, _)| wid)
            }
        } else {
            None
        };

        // ── Locality scoring ──
        // Convert pre-computed endpoint scores to worker scores, filtering
        // to the candidate set. This is O(endpoints) not O(files).
        let locality_winner = if let Some(ep_scores) = endpoint_scores {
            let scores = endpoint_scores_to_worker_scores(
                ep_scores,
                &self.endpoint_to_worker,
                &candidates,
            );
            if !scores.is_empty() {
                // Sort workers by score descending, then by timestamp
                // descending as a tiebreaker. Workers within 10% of the
                // top score are considered tied and the most recently
                // refreshed one wins.
                let mut sorted: Vec<_> = scores.into_iter().collect();
                // Look up effective load score for tiebreaking within 10% score range.
                let load_score_for_worker = |wid: &WorkerId| -> u64 {
                    self.workers.0.peek(wid)
                        .map(|w| effective_load_score(w.p_core_load_pct, w.e_core_load_pct, w.cpu_load_pct))
                        .unwrap_or(u64::MAX)
                };
                sorted.sort_by(|a, b| {
                    let (score_a, ts_a) = a.1;
                    let (score_b, ts_b) = b.1;
                    let max_score = score_a.max(score_b);
                    // Within 10% of each other? Use load score, then timestamp.
                    let threshold = max_score / 10; // 10% of the larger score
                    if score_a.abs_diff(score_b) <= threshold {
                        // Scores are similar — prefer lower load score.
                        let load_a = load_score_for_worker(&a.0);
                        let load_b = load_score_for_worker(&b.0);
                        if load_a != load_b {
                            load_a.cmp(&load_b)
                        } else {
                            // Same load or both unknown — prefer more recent timestamp.
                            ts_b.cmp(&ts_a)
                        }
                    } else {
                        // Scores differ significantly, prefer higher score.
                        score_b.cmp(&score_a)
                    }
                });

                let best = sorted.first().map(|(_, (s, _))| *s).unwrap_or(0);
                if best > 0 {
                    sorted.into_iter()
                        .find(|(wid, (score, _))| *score > 0 && worker_is_viable(wid))
                        .map(|(wid, (score, _))| {
                            info!(
                                ?wid,
                                score,
                                %input_root_digest,
                                "Locality scoring -- worker has {} cached input bytes",
                                score
                            );
                            wid
                        })
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        let worker_id = if let Some(wid) = dir_cache_winner {
            // Exact root match trumps all other scoring.
            self.workers.get_mut(&wid);
            wid
        } else if let Some(wid) = subtree_coverage_winner {
            // Partial subtree coverage beats blob-level locality.
            self.workers.get_mut(&wid);
            wid
        } else if let Some(wid) = locality_winner {
            // Blob-level locality scoring.
            self.workers.get_mut(&wid);
            wid
        } else {
            // ── Fallback: existing LRU/MRU strategy ──
            let wid = self.inner_find_worker_for_action(platform_properties, full_worker_logging)?;
            wid
        };

        // Atomically reserve the worker by mutating its state under the same lock.
        let (tx, msg) = self.prepare_worker_run_action(
            &worker_id,
            operation_id,
            action_info,
            peer_hints,
            pre_computed_tree,
        )?;

        Some((worker_id, tx, msg))
    }

    /// Undoes a reservation made by `inner_find_and_reserve_worker`.
    /// This removes the operation from the worker's `running_action_infos`
    /// and restores the reduced platform properties.
    fn inner_unreserve_worker(
        &mut self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
    ) {
        if let Some(worker) = self.workers.get_mut(worker_id) {
            if let Some(pending) = worker.running_action_infos.remove(operation_id) {
                if !worker.restored_platform_properties.remove(operation_id) {
                    worker.restore_platform_properties(
                        &pending.action_info.platform_properties,
                    );
                }
            }
        }
    }

    async fn update_action(
        &mut self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        let worker = self.workers.get_mut(worker_id).err_tip(|| {
            format!("Worker {worker_id} does not exist in SimpleScheduler::update_action")
        })?;

        // ExecutionComplete is sent by the worker after ExecuteResult to
        // signal that post-execution I/O (CAS upload, AC write) has
        // finished and the worker's platform resources can be fully
        // reclaimed. Because ExecuteResult(Completed) already calls
        // complete_action() which removes the operation from
        // running_action_infos, the operation will not be present when
        // ExecutionComplete arrives. This is expected — not an error.
        if matches!(update, UpdateOperationType::ExecutionComplete) {
            if worker.running_action_infos.contains_key(operation_id) {
                worker.execution_complete(operation_id);
            }
            self.worker_change_notify.notify_one();
            return Ok(());
        }

        // Ensure the worker is supposed to be running the operation.
        if !worker.running_action_infos.contains_key(operation_id) {
            let err = make_err!(
                Code::Internal,
                "Operation {operation_id} should not be running on worker {worker_id} in SimpleScheduler::update_action"
            );
            return Result::<(), _>::Err(err.clone())
                .merge(self.immediate_evict_worker(worker_id, err, false).await);
        }

        let (is_finished, due_to_backpressure) = match &update {
            UpdateOperationType::UpdateWithActionStage(action_stage) => {
                (action_stage.is_finished(), false)
            }
            UpdateOperationType::KeepAlive => (false, false),
            UpdateOperationType::UpdateWithError(err) => {
                (true, err.code == Code::ResourceExhausted)
            }
            UpdateOperationType::UpdateWithDisconnect => (true, false),
            // Handled above before the contains_key check.
            UpdateOperationType::ExecutionComplete => unreachable!(),
        };

        // Update the operation in the worker state manager.
        {
            let update_operation_res = self
                .worker_state_manager
                .update_operation(operation_id, worker_id, update)
                .await
                .err_tip(|| "in update_operation on SimpleScheduler::update_action");
            if let Err(err) = update_operation_res {
                error!(
                    %operation_id,
                    ?worker_id,
                    ?err,
                    "Failed to update_operation on update_action"
                );
                return Err(err);
            }
        }

        if !is_finished {
            return Ok(());
        }

        // Clear this action from the current worker if finished.
        let complete_action_res = {
            // Note: We need to run this before dealing with backpressure logic.
            let complete_action_res = worker.complete_action(operation_id).await;

            if (due_to_backpressure || !worker.can_accept_work()) && worker.has_actions() {
                worker.is_paused = true;
                worker.paused_due_to_backpressure = due_to_backpressure;
            }
            complete_action_res
        };

        self.worker_change_notify.notify_one();

        complete_action_res
    }

    /// Prepares a worker to run an action by mutating its state (reducing platform
    /// properties, recording the running action), then returns the cloned `tx` sender
    /// and pre-built message so the caller can send the notification *after* releasing
    /// the write lock.
    ///
    /// `peer_hints` are pre-computed outside the write lock from the resolved
    /// input tree and passed as a shared slice reference to avoid cloning
    /// per action match. The slice is cloned into the protobuf message only
    /// here, and only when a worker was actually found. When no resolved
    /// tree is available the hints will be empty.
    ///
    /// `pre_computed_tree` contains directory and digest Vecs that were built
    /// outside the write lock to avoid cloning Directory protos while holding it.
    ///
    /// Returns `None` if the worker was not found.
    fn prepare_worker_run_action(
        &mut self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        action_info: &ActionInfoWithProps,
        peer_hints: &[PeerHint],
        pre_computed_tree: Option<(Vec<Directory>, Vec<Digest>)>,
    ) -> Option<(UnboundedSender<UpdateForWorker>, UpdateForWorker)> {
        let worker = self.workers.get_mut(worker_id)?;
        // Clone the tx so we can send outside the lock.
        let tx = worker.tx.clone();

        if !peer_hints.is_empty() {
            debug!(
                ?worker_id,
                hints = peer_hints.len(),
                "generated peer hints for StartExecute"
            );
        }

        let (resolved_directories, resolved_directory_digests) =
            pre_computed_tree.unwrap_or_default();

        // Build the protobuf message while we still have access to worker state.
        // peer_hints is cloned here (the only place) — deferred from the cache
        // lookup so actions that don't find a worker avoid the clone entirely.
        let start_execute = StartExecute {
            execute_request: Some(action_info.inner.as_ref().into()),
            operation_id: operation_id.to_string(),
            queued_timestamp: Some(action_info.inner.insert_timestamp.into()),
            platform: Some((&action_info.platform_properties).into()),
            worker_id: worker.id.clone().into(),
            peer_hints: peer_hints.to_vec(),
            resolved_directories,
            resolved_directory_digests,
        };
        let msg = UpdateForWorker {
            update: Some(update_for_worker::Update::StartAction(start_execute)),
        };

        // If the operation is already reserved on this worker (a concurrent
        // do_try_match beat us), skip — otherwise the later unreserve_worker
        // on the losing match would remove the winning reservation, leaving
        // the worker's running_action_infos empty and preventing the action
        // from being re-queued when the worker is removed.
        if worker.running_action_infos.contains_key(operation_id) {
            return None;
        }

        // Perform the state mutation that run_action would do:
        // reduce platform properties and record the running action.
        reduce_platform_properties(
            &mut worker.platform_properties,
            &action_info.platform_properties,
        );
        worker.running_action_infos.insert(
            operation_id.clone(),
            PendingActionInfoData {
                action_info: action_info.clone(),
            },
        );
        Some((tx, msg))
    }

    /// Evicts the worker from the pool and puts items back into the queue if anything was being executed on it.
    async fn immediate_evict_worker(
        &mut self,
        worker_id: &WorkerId,
        err: Error,
        is_disconnect: bool,
    ) -> Result<(), Error> {
        let mut result = Ok(());
        if let Some(mut worker) = self.remove_worker(worker_id) {
            // We don't care if we fail to send message to worker, this is only a best attempt.
            drop(worker.notify_update(WorkerUpdate::Disconnect).await);
            let update = if is_disconnect {
                UpdateOperationType::UpdateWithDisconnect
            } else {
                UpdateOperationType::UpdateWithError(err)
            };
            for (operation_id, _) in worker.running_action_infos.drain() {
                result = result.merge(
                    self.worker_state_manager
                        .update_operation(&operation_id, worker_id, update.clone())
                        .await,
                );
            }
        }
        // Note: Calling this many time is very cheap, it'll only trigger `do_try_match` once.
        // TODO(palfrey) This should be moved to inside the Workers struct.
        self.worker_change_notify.notify_one();
        result
    }
}

#[derive(Debug, MetricsComponent)]
pub struct ApiWorkerScheduler {
    #[metric]
    inner: RwLock<ApiWorkerSchedulerImpl>,
    #[metric(group = "platform_property_manager")]
    platform_property_manager: Arc<PlatformPropertyManager>,

    #[metric(
        help = "Timeout of how long to evict workers if no response in this given amount of time in seconds."
    )]
    worker_timeout_s: u64,
    /// Shared worker registry for checking worker liveness.
    worker_registry: SharedWorkerRegistry,

    /// Performance metrics for observability.
    metrics: Arc<SchedulerMetrics>,

    /// Blob locality map for peer-to-peer blob sharing.
    /// Used to generate peer hints in StartExecute messages.
    locality_map: Option<SharedBlobLocalityMap>,

    /// CAS store for resolving input trees (reading Directory protos).
    /// When set, enables tier-2 locality scoring.
    cas_store: Option<Store>,

    /// Cached resolved input trees: input_root_digest → ResolvedTree.
    /// Held under a tokio::Mutex briefly for get/put, not during I/O.
    tree_cache: Arc<tokio::sync::Mutex<LruCache<DigestInfo, Arc<ResolvedTree>>>>,

    /// Digests currently being resolved in background tasks. Prevents
    /// duplicate spawns when many actions share the same input root.
    tree_resolution_in_progress: Arc<tokio::sync::Mutex<HashSet<DigestInfo>>>,

    /// Negative cache: root digests whose tree resolution failed recently.
    /// Entries carry (timestamp, attempt_count) for exponential backoff:
    /// attempt 1 → 60s, attempt 2 → 300s, attempt 3 → 1500s, attempt 4+ → 1800s (capped).
    tree_resolution_failures: Arc<tokio::sync::Mutex<HashMap<DigestInfo, (Instant, u32)>>>,

    /// Negative cache for individual directory digests that failed during
    /// BFS resolution. Keyed by the specific subdirectory that was missing,
    /// not the root digest. This prevents N different root digests that
    /// share a common failing subdirectory from each triggering independent
    /// resolution attempts. Entries expire after 60s.
    failed_directory_digests: Arc<tokio::sync::Mutex<HashMap<DigestInfo, Instant>>>,

    /// Cache of endpoint scores keyed by input_root_digest.
    /// Avoids recomputing locality scores for identical input trees.
    /// Bounded LRU (1024 entries) — stale entries from worker churn are
    /// naturally evicted rather than cleared wholesale.
    scores_cache: Arc<tokio::sync::Mutex<LruCache<DigestInfo, Arc<ScoringResult>>>>,

    /// Cached GrpcStore connections to worker CAS endpoints for prefetch.
    /// Protected by a sync Mutex since we only hold it briefly to clone a Store.
    prefetch_connections: ParkingMutex<HashMap<Arc<str>, Store>>,

    /// Per-worker semaphore limiting concurrent prefetch streams.
    /// Key is the worker CAS endpoint.
    prefetch_semaphores: ParkingMutex<HashMap<Arc<str>, Arc<Semaphore>>>,
}

/// Capacity for the resolved input tree LRU cache.
const TREE_CACHE_CAPACITY: usize = 1024;

/// Maximum size of a single blob eligible for prefetch (1MiB).
/// Larger blobs are more efficiently handled by the worker's parallel
/// ByteStream fetch (128-512 concurrent streams). Prefetch targets
/// small-to-medium blobs where per-blob RPC overhead dominates.
const PREFETCH_MAX_SINGLE_BLOB_SIZE: u64 = 4 * 1024 * 1024;

/// Maximum number of concurrent prefetch batch RPCs per worker.
const PREFETCH_MAX_CONCURRENT_PER_WORKER: usize = 8;

/// Maximum total bytes in-flight for prefetch per dispatch (200MB).
const PREFETCH_MAX_INFLIGHT_BYTES: u64 = 200 * 1024 * 1024;

/// Maximum number of blobs to prefetch per dispatch. High count
/// because small blobs are cheap to push via BatchUpdateBlobs.
const PREFETCH_MAX_BLOBS: usize = 1024;

/// Maximum total bytes per BatchUpdateBlobs RPC batch (4MiB).
/// Matches PREFETCH_MAX_SINGLE_BLOB_SIZE so all prefetched blobs
/// can go through the efficient batch path.
const PREFETCH_BATCH_SIZE_BYTES: u64 = 4 * 1024 * 1024;

/// Maximum concurrent get_part_unchunked calls during server cache warm.
const CACHE_WARM_CONCURRENCY: usize = 64;

/// Maximum total bytes to warm in a single cache warm pass (256MB).
const CACHE_WARM_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// Maximum number of blobs to warm in a single cache warm pass.
const CACHE_WARM_MAX_BLOBS: usize = 4096;

/// Base backoff duration after a failed tree resolution (first attempt).
const FAILURE_BACKOFF: Duration = Duration::from_secs(60);

/// Maximum backoff duration for repeated tree resolution failures.
const MAX_FAILURE_BACKOFF: Duration = Duration::from_secs(1800);

/// When a negative cache map exceeds this many entries, sweep expired ones.
const NEGATIVE_CACHE_SWEEP_THRESHOLD: usize = 1000;

/// Computes exponential backoff for tree resolution failures.
/// attempt 1 → base (60s), attempt 2 → 300s, attempt 3 → 1500s, attempt 4+ → 1800s (capped).
fn backoff_for_attempt(base: Duration, attempts: u32) -> Duration {
    if attempts <= 1 {
        return base;
    }
    let multiplier = 5u64.saturating_pow(attempts - 1);
    let backoff_secs = base.as_secs().saturating_mul(multiplier);
    Duration::from_secs(backoff_secs.min(MAX_FAILURE_BACKOFF.as_secs()))
}

impl ApiWorkerScheduler {
    pub fn new(
        worker_state_manager: Arc<dyn WorkerStateManager>,
        platform_property_manager: Arc<PlatformPropertyManager>,
        allocation_strategy: WorkerAllocationStrategy,
        worker_change_notify: Arc<Notify>,
        worker_timeout_s: u64,
        worker_registry: SharedWorkerRegistry,
    ) -> Arc<Self> {
        Self::new_with_locality_map(
            worker_state_manager,
            platform_property_manager,
            allocation_strategy,
            worker_change_notify,
            worker_timeout_s,
            worker_registry,
            None,
            None,
        )
    }

    pub fn new_with_locality_map(
        worker_state_manager: Arc<dyn WorkerStateManager>,
        platform_property_manager: Arc<PlatformPropertyManager>,
        allocation_strategy: WorkerAllocationStrategy,
        worker_change_notify: Arc<Notify>,
        worker_timeout_s: u64,
        worker_registry: SharedWorkerRegistry,
        locality_map: Option<SharedBlobLocalityMap>,
        cas_store: Option<Store>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: RwLock::new(ApiWorkerSchedulerImpl {
                workers: Workers(LruCache::unbounded()),
                worker_state_manager,
                allocation_strategy,
                worker_change_notify,
                worker_registry: worker_registry.clone(),
                shutting_down: false,
                capability_index: WorkerCapabilityIndex::new(),
                endpoint_to_worker: HashMap::new(),
            }),
            platform_property_manager,
            worker_timeout_s,
            worker_registry,
            metrics: Arc::new(SchedulerMetrics::default()),
            locality_map,
            cas_store,
            tree_cache: Arc::new(tokio::sync::Mutex::new(LruCache::new(
                NonZeroUsize::new(TREE_CACHE_CAPACITY).unwrap(),
            ))),
            tree_resolution_in_progress: Arc::new(tokio::sync::Mutex::new(HashSet::new())),
            tree_resolution_failures: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            failed_directory_digests: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            scores_cache: Arc::new(tokio::sync::Mutex::new(LruCache::new(
                NonZeroUsize::new(TREE_CACHE_CAPACITY).unwrap(),
            ))),
            prefetch_connections: ParkingMutex::new(HashMap::new()),
            prefetch_semaphores: ParkingMutex::new(HashMap::new()),
        })
    }

    /// Returns a reference to the worker registry.
    pub const fn worker_registry(&self) -> &SharedWorkerRegistry {
        &self.worker_registry
    }

    /// Removes cached prefetch connection and semaphore for a specific endpoint.
    fn remove_prefetch_for_endpoint(&self, endpoint: &str) {
        self.prefetch_connections.lock().remove(endpoint);
        self.prefetch_semaphores.lock().remove(endpoint);
    }

    /// Removes prefetch entries whose endpoint is no longer associated with
    /// any active worker. Called after bulk worker evictions to prevent
    /// unbounded growth of the prefetch maps.
    fn cleanup_stale_prefetch_entries(&self, active_endpoints: &HashSet<Arc<str>>) {
        self.prefetch_connections
            .lock()
            .retain(|ep, _| active_endpoints.contains(ep));
        self.prefetch_semaphores
            .lock()
            .retain(|ep, _| active_endpoints.contains(ep));
    }

    pub async fn worker_notify_run_action(
        &self,
        worker_id: WorkerId,
        operation_id: OperationId,
        action_info: ActionInfoWithProps,
    ) -> Result<(), Error> {
        self.metrics
            .actions_dispatched
            .fetch_add(1, Ordering::Relaxed);

        // Phase 1: Acquire write lock, mutate worker state, extract tx + message,
        // then drop the lock BEFORE sending on the channel.
        let prepare_result = {
            let mut inner = self.inner.write().await;
            let result =
                inner.prepare_worker_run_action(&worker_id, &operation_id, &action_info, &[], None);
            if result.is_none() {
                // Worker not found - handle under the lock since we need worker_state_manager.
                warn!(
                    ?worker_id,
                    %operation_id,
                    ?action_info,
                    "Worker not found in worker map in worker_notify_run_action"
                );
                return inner
                    .worker_state_manager
                    .update_operation(
                        &operation_id,
                        &worker_id,
                        UpdateOperationType::UpdateWithDisconnect,
                    )
                    .await;
            }
            result
            // inner (write lock) is dropped here
        };

        // Phase 2: Send notification outside the lock to avoid blocking other
        // scheduler operations if the channel has backpressure.
        if let Some((tx, msg)) = prepare_result {
            if let Err(_send_err) = tx.send(msg) {
                // Worker disconnected. Re-acquire lock to evict.
                warn!(
                    ?worker_id,
                    ?action_info,
                    "Worker command failed (disconnected), removing worker",
                );
                let err = make_err!(
                    Code::Internal,
                    "Worker command failed, removing worker {worker_id} -- Worker Disconnected",
                );
                let mut inner = self.inner.write().await;
                return Result::<(), _>::Err(err.clone()).merge(
                    inner
                        .immediate_evict_worker(&worker_id, err, true)
                        .await,
                );
            }
        }

        Ok(())
    }

    /// Sends the start-execution notification for a worker that was already
    /// reserved by `find_and_reserve_worker`. The worker's state has already
    /// been mutated (platform properties reduced, action recorded in
    /// `running_action_infos`), so this method only sends the pre-built
    /// message over the channel and handles disconnection errors.
    pub async fn send_reserved_worker_notification(
        &self,
        worker_id: &WorkerId,
        tx: UnboundedSender<UpdateForWorker>,
        msg: UpdateForWorker,
    ) -> Result<(), Error> {
        self.metrics
            .actions_dispatched
            .fetch_add(1, Ordering::Relaxed);

        if let Err(_send_err) = tx.send(msg) {
            // Worker disconnected. Re-acquire lock to evict.
            warn!(
                ?worker_id,
                "Worker command failed (disconnected) after reservation, removing worker",
            );
            let err = make_err!(
                Code::Internal,
                "Worker command failed, removing worker {worker_id} -- Worker Disconnected",
            );
            let mut inner = self.inner.write().await;
            return Result::<(), _>::Err(err.clone()).merge(
                inner
                    .immediate_evict_worker(worker_id, err, true)
                    .await,
            );
        }

        Ok(())
    }

    /// Returns the scheduler metrics for observability.
    #[must_use]
    pub const fn get_metrics(&self) -> &Arc<SchedulerMetrics> {
        &self.metrics
    }

    /// Attempts to find a worker that is capable of running this action.
    // TODO(palfrey) This algorithm is not very efficient. Simple testing using a tree-like
    // structure showed worse performance on a 10_000 worker * 7 properties * 1000 queued tasks
    // simulation of worst cases in a single threaded environment.
    pub async fn find_worker_for_action(
        &self,
        platform_properties: &PlatformProperties,
        full_worker_logging: bool,
    ) -> Option<WorkerId> {
        let start = Instant::now();
        self.metrics
            .find_worker_calls
            .fetch_add(1, Ordering::Relaxed);

        let mut inner = self.inner.write().await;
        let worker_count = inner.workers.len() as u64;
        let result = inner.inner_find_worker_for_action(platform_properties, full_worker_logging);

        // Track workers iterated (worst case is all workers)
        self.metrics
            .workers_iterated
            .fetch_add(worker_count, Ordering::Relaxed);

        if result.is_some() {
            self.metrics
                .find_worker_hits
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.metrics
                .find_worker_misses
                .fetch_add(1, Ordering::Relaxed);
        }

        #[allow(clippy::cast_possible_truncation)]
        self.metrics
            .find_worker_time_ns
            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        result
    }

    /// Atomically finds a suitable worker AND reserves it for the given
    /// operation. This combines the find and reservation into a single lock
    /// acquisition, preventing two concurrent match operations from selecting
    /// the same worker.
    ///
    /// Returns `(worker_id, tx, msg)` where `tx` and `msg` can be used to
    /// send the start-execution notification to the worker outside the lock.
    /// Returns `None` if no suitable worker was found.
    ///
    /// If the caller later decides not to use this reservation (e.g., because
    /// `assign_operation` fails), it MUST call `unreserve_worker` to undo
    /// the reservation.
    pub async fn find_and_reserve_worker(
        &self,
        platform_properties: &PlatformProperties,
        operation_id: &OperationId,
        action_info: &ActionInfoWithProps,
        full_worker_logging: bool,
    ) -> Option<(WorkerId, UnboundedSender<UpdateForWorker>, UpdateForWorker)> {
        let start = Instant::now();
        self.metrics
            .find_worker_calls
            .fetch_add(1, Ordering::Relaxed);

        // ── Phase 1: async tree resolution (BEFORE write lock) ──
        let resolved_tree = self
            .resolve_input_tree(action_info.inner.input_root_digest)
            .await;

        // ── Phase 2: pre-compute locality scores and peer hints (BEFORE write lock) ──
        // These are O(files × endpoints_per_blob) operations that previously
        // ran inside the write lock, blocking all scheduler operations for
        // 2-5ms on large actions (50K+ inputs).
        // Results are cached by input_root_digest so identical input trees
        // skip the recomputation entirely.
        //
        // The result is kept as Arc<ScoringResult> and passed by reference
        // into the write-lock phase. This eliminates the per-action deep
        // clone of Vec<PeerHint> (up to 16K entries with Vec<String>
        // endpoints) and HashMap<Arc<str>, ...> that previously consumed
        // ~61% of scheduler CPU during active builds.
        let input_root_digest = action_info.inner.input_root_digest;
        let scoring_result: Option<Arc<ScoringResult>> = match (&resolved_tree, &self.locality_map) {
            (Some(tree), Some(loc_map)) => {
                // Check the scores cache first (lock briefly, no await while held).
                let cached = self.scores_cache.lock().await.get(&input_root_digest).cloned();
                if let Some(arc) = cached {
                    Some(arc)
                } else {
                    let result = score_and_generate_hints(&tree.file_digests, loc_map);
                    let arc = Arc::new(result);
                    self.scores_cache.lock().await.put(
                        input_root_digest,
                        Arc::clone(&arc),
                    );
                    Some(arc)
                }
            }
            _ => None,
        };

        // ── Phase 2.5: pre-compute tree proto data (BEFORE write lock) ──
        // Cloning Directory protos is expensive and should not happen under
        // the write lock. We size-check and build the Vecs here; the lock
        // phase just passes them through to the protobuf message.
        // Worker API listener has max_encoding_message_size=64MiB.
        const MAX_TREE_PROTO_BYTES: usize = 32 * 1024 * 1024;
        let pre_computed_tree: Option<(Vec<Directory>, Vec<Digest>)> =
            resolved_tree.as_deref().and_then(|tree| {
                let estimated_bytes: usize = tree
                    .directories
                    .values()
                    .map(|d| Message::encoded_len(d))
                    .sum();
                if estimated_bytes > MAX_TREE_PROTO_BYTES {
                    debug!(
                        estimated_bytes,
                        max = MAX_TREE_PROTO_BYTES,
                        dirs = tree.directories.len(),
                        "pre-resolved tree exceeds size threshold, omitting from StartExecute"
                    );
                    None
                } else {
                    debug!(
                        dirs = tree.directories.len(),
                        estimated_bytes,
                        "including pre-resolved tree in StartExecute"
                    );
                    Some(tree.to_proto_vecs())
                }
            });

        // ── Phase 3: acquire write lock, do selection + reservation ──
        // Inside the lock we only do O(workers) work: candidate filtering,
        // endpoint→WorkerId mapping, and state mutation. Peer hints are
        // passed as a slice reference — cloned into the proto only when a
        // worker is actually found (inside prepare_worker_run_action).
        let mut inner = self.inner.write().await;
        let worker_count = inner.workers.len() as u64;
        let (endpoint_scores, peer_hints_slice): (
            Option<&HashMap<Arc<str>, (u64, SystemTime)>>,
            &[PeerHint],
        ) = match scoring_result.as_deref() {
            Some((scores, hints)) => (Some(scores), hints.as_slice()),
            None => (None, &[]),
        };
        let result = inner.inner_find_and_reserve_worker(
            platform_properties,
            operation_id,
            action_info,
            full_worker_logging,
            endpoint_scores,
            peer_hints_slice,
            resolved_tree.as_deref(),
            pre_computed_tree,
        );

        // Extract the selected worker's CAS endpoint while we still hold
        // the lock, for use in the prefetch spawn below.
        let worker_cas_endpoint: Option<Arc<str>> = result.as_ref().and_then(|(wid, _, _)| {
            inner
                .workers
                .peek(wid)
                .filter(|w| !w.cas_endpoint.is_empty())
                .map(|w| Arc::from(w.cas_endpoint.as_str()))
        });

        // Track workers iterated (worst case is all workers)
        self.metrics
            .workers_iterated
            .fetch_add(worker_count, Ordering::Relaxed);

        if result.is_some() {
            self.metrics
                .find_worker_hits
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.metrics
                .find_worker_misses
                .fetch_add(1, Ordering::Relaxed);
        }

        #[allow(clippy::cast_possible_truncation)]
        self.metrics
            .find_worker_time_ns
            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);

        // Drop the write lock before spawning prefetch.
        drop(inner);

        // ── Phase 4: spawn targeted prefetch (AFTER write lock released) ──
        // If we have a resolved tree, a locality map, and the selected
        // worker has a CAS endpoint, compute the set of missing blobs and
        // push them to the worker concurrently with the StartExecute dispatch.
        if let (Some(tree), Some(loc_map), Some(endpoint)) =
            (&resolved_tree, &self.locality_map, worker_cas_endpoint)
        {
            let missing = Self::compute_missing_blobs(
                &tree.file_digests,
                &endpoint,
                loc_map,
            );
            if !missing.is_empty() {
                self.spawn_prefetch(
                    endpoint,
                    missing,
                    operation_id.to_string(),
                );
            }
        }

        // ── Phase 5: spawn server-side cache warm (AFTER write lock released) ──
        // Read blobs through the full CAS chain so MemoryStore gets populated.
        // Already-warm blobs are a ~5us no-op; cold blobs get read from disk.
        if let Some(tree) = &resolved_tree {
            self.spawn_server_cache_warm(&tree.file_digests, operation_id);
        }

        result
    }

    /// Undoes a reservation made by `find_and_reserve_worker`. This must
    /// be called if the match is abandoned after reservation (e.g., if
    /// `assign_operation` returns an error).
    pub async fn unreserve_worker(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
    ) {
        let mut inner = self.inner.write().await;
        inner.inner_unreserve_worker(worker_id, operation_id);
    }

    /// Returns true if any registered worker could match the given platform
    /// properties (static check only — does not consider dynamic resource
    /// availability like current cpu_count).
    pub async fn has_matching_workers(&self, platform_properties: &PlatformProperties) -> bool {
        let inner = self.inner.read().await;
        !inner
            .capability_index
            .find_matching_workers(platform_properties, false)
            .is_empty()
    }

    /// Checks to see if the worker exists in the worker pool. Should only be used in unit tests.
    #[must_use]
    pub async fn contains_worker_for_test(&self, worker_id: &WorkerId) -> bool {
        let inner = self.inner.read().await;
        inner.workers.contains(worker_id)
    }

    /// A unit test function used to send the keep alive message to the worker from the server.
    pub async fn send_keep_alive_to_worker_for_test(
        &self,
        worker_id: &WorkerId,
    ) -> Result<(), Error> {
        let mut inner = self.inner.write().await;
        let worker = inner.workers.get_mut(worker_id).ok_or_else(|| {
            make_input_err!("WorkerId '{}' does not exist in workers map", worker_id)
        })?;
        worker.keep_alive()
    }

    /// Resolves the full input tree for the given `input_root_digest`,
    /// returning a cached result if available. On cache miss, returns
    /// `None` immediately (falling back to load-based scoring) and
    /// spawns a background task to resolve the tree from CAS so that
    /// future actions with the same input root hit the cache.
    ///
    /// Returns `None` if no CAS store is configured or on cache miss
    /// (the background task will warm the cache for next time).
    ///
    /// This keeps CAS I/O off the scheduling critical path — only a
    /// brief `tokio::Mutex` lock for the cache lookup is performed
    /// synchronously.
    async fn resolve_input_tree(
        &self,
        input_root_digest: DigestInfo,
    ) -> Option<Arc<ResolvedTree>> {
        let cas_store = self.cas_store.as_ref()?;

        // Check positive cache first (brief lock).
        {
            let mut cache = self.tree_cache.lock().await;
            if let Some(cached) = cache.get(&input_root_digest) {
                info!(
                    %input_root_digest,
                    file_count = cached.file_digests.len(),
                    dir_count = cached.dir_digests.len(),
                    "tree resolution cache hit"
                );
                return Some(cached.clone());
            }
        }

        // Check negative cache: skip if this digest failed recently.
        // Uses exponential backoff: 60s, 300s, 1500s, 1800s (capped).
        {
            let mut failures = self.tree_resolution_failures.lock().await;
            // Sweep expired entries to prevent unbounded growth.
            if failures.len() > NEGATIVE_CACHE_SWEEP_THRESHOLD {
                failures.retain(|_, &mut (failed_at, attempts)| {
                    failed_at.elapsed() < backoff_for_attempt(FAILURE_BACKOFF, attempts)
                });
            }
            if let Some(&(failed_at, attempts)) = failures.get(&input_root_digest) {
                let backoff = backoff_for_attempt(FAILURE_BACKOFF, attempts);
                if failed_at.elapsed() < backoff {
                    return None;
                }
            }
        }

        // Atomically check and mark as in-progress to avoid TOCTOU race.
        {
            let mut in_progress = self.tree_resolution_in_progress.lock().await;
            if in_progress.contains(&input_root_digest) {
                return None;
            }
            in_progress.insert(input_root_digest);
        }

        // Cache miss — spawn background resolution to warm cache for
        // future actions. This action proceeds with load-based scoring.
        let tree_cache = self.tree_cache.clone();
        let in_progress_ref = self.tree_resolution_in_progress.clone();
        let failures_ref = self.tree_resolution_failures.clone();
        let failed_dirs_ref = self.failed_directory_digests.clone();
        let store = cas_store.clone();
        let digest = input_root_digest;
        tokio::spawn(async move {
            match resolve_tree_from_cas(&store, digest, &failed_dirs_ref).await {
                Ok(resolved) => {
                    info!(
                        %digest,
                        file_count = resolved.file_digests.len(),
                        dir_count = resolved.dir_digests.len(),
                        "background tree resolution complete, cached for future actions"
                    );
                    let mut cache = tree_cache.lock().await;
                    cache.put(digest, Arc::new(resolved));
                    // Clear any stale failure entry.
                    failures_ref.lock().await.remove(&digest);
                }
                Err(err) => {
                    // Increment attempt counter for exponential backoff.
                    let mut failures = failures_ref.lock().await;
                    let attempts = failures
                        .get(&digest)
                        .map(|&(_, a)| a)
                        .unwrap_or(0)
                        + 1;
                    let backoff = backoff_for_attempt(FAILURE_BACKOFF, attempts);
                    warn!(
                        %digest,
                        ?err,
                        attempts,
                        backoff_secs = backoff.as_secs(),
                        "background tree resolution failed, suppressing retries"
                    );
                    failures.insert(digest, (Instant::now(), attempts));
                }
            }
            // Always remove from in-progress set.
            in_progress_ref.lock().await.remove(&digest);
        });

        info!(
            %input_root_digest,
            "tree cache miss, using load-based scoring (background resolution started)"
        );
        None
    }

    /// Returns the per-worker prefetch semaphore, creating it if needed.
    fn get_prefetch_semaphore(&self, endpoint: &str) -> Arc<Semaphore> {
        let mut sems = self.prefetch_semaphores.lock();
        sems.entry(Arc::from(endpoint))
            .or_insert_with(|| Arc::new(Semaphore::new(PREFETCH_MAX_CONCURRENT_PER_WORKER)))
            .clone()
    }

    /// Computes the set of small blobs that the target worker is missing
    /// from the resolved input tree, using the locality map to determine
    /// what the worker already has. Returns blobs sorted by size ascending
    /// (smallest first), capped at `PREFETCH_MAX_BLOBS` and
    /// `PREFETCH_MAX_INFLIGHT_BYTES`.
    ///
    /// Only blobs under `PREFETCH_MAX_SINGLE_BLOB_SIZE` are included —
    /// large blobs are better handled by the worker's parallel ByteStream
    /// fetch. The goal is to eliminate per-blob RPC overhead for many
    /// small blobs by batching them via `BatchUpdateBlobs`.
    fn compute_missing_blobs(
        file_digests: &[(DigestInfo, u64)],
        worker_endpoint: &str,
        locality_map: &SharedBlobLocalityMap,
    ) -> Vec<(DigestInfo, u64)> {
        let map = locality_map.read();
        let blobs = map.blobs_map();

        // Collect small blobs the worker doesn't have.
        let mut missing: Vec<(DigestInfo, u64)> = file_digests
            .iter()
            .filter(|(_, size)| *size > 0 && *size <= PREFETCH_MAX_SINGLE_BLOB_SIZE)
            .filter(|(digest, _)| {
                // Blob is "missing" if the locality map has no entry for this
                // worker endpoint, or the digest is not in the map at all.
                blobs
                    .get(digest)
                    .map_or(true, |endpoints| endpoints.get(worker_endpoint).is_none())
            })
            .copied()
            .collect();

        // Sort by size ascending -- smallest blobs first maximizes the
        // number of blobs per BatchUpdateBlobs RPC, eliminating the most
        // per-blob RPC overhead.
        missing.sort_by_key(|(_, size)| *size);

        // Cap by count and total bytes.
        let mut total_bytes: u64 = 0;
        missing.truncate(PREFETCH_MAX_BLOBS);
        missing.retain(|(_, size)| {
            if total_bytes + size > PREFETCH_MAX_INFLIGHT_BYTES {
                return false;
            }
            total_bytes += size;
            true
        });

        missing
    }

    /// Spawns a background task that prefetches missing small blobs from
    /// the server's CAS to the selected worker's CAS endpoint. Blobs are
    /// read into memory and pushed via `update_oneshot`, which routes them
    /// through `BatchUpdateBlobs` on the worker's GrpcStore connection.
    /// This batches many small blobs into few RPCs, eliminating per-blob
    /// RPC overhead that dominates the worker's demand fetch path.
    ///
    /// This is best-effort: failures are logged but do not affect the
    /// action dispatch. The worker's normal demand fetch handles anything
    /// prefetch doesn't deliver.
    ///
    /// This method is synchronous (no `.await`) — all I/O including
    /// connection creation happens inside the spawned task, keeping the
    /// dispatch path non-blocking.
    fn spawn_prefetch(
        &self,
        worker_endpoint: Arc<str>,
        missing_blobs: Vec<(DigestInfo, u64)>,
        operation_id: String,
    ) {
        let cas_store = match &self.cas_store {
            Some(s) => s.clone(),
            None => return,
        };

        if missing_blobs.is_empty() {
            return;
        }

        let total_bytes: u64 = missing_blobs.iter().map(|(_, s)| *s).sum();
        let blob_count = missing_blobs.len();
        let metrics = self.metrics.clone();
        let endpoint_str = worker_endpoint.clone();
        let semaphore = self.get_prefetch_semaphore(&worker_endpoint);

        // Snapshot the cached connection under a brief sync lock. The
        // actual TCP connect (if needed) happens inside the spawned task.
        let cached_connection = {
            let conns = self.prefetch_connections.lock();
            conns.get(&*worker_endpoint).cloned()
        };

        metrics
            .prefetch_tasks_spawned
            .fetch_add(1, Ordering::Relaxed);

        info!(
            %operation_id,
            worker_endpoint = %endpoint_str,
            blob_count,
            total_bytes,
            "prefetch: spawning batched push of small blobs to worker"
        );

        tokio::spawn(async move {
            let start = Instant::now();

            // Get or create connection to worker. This may do TCP connect
            // but happens inside the spawned task, not on the dispatch path.
            let worker_store = if let Some(store) = cached_connection {
                store
            } else {
                match create_worker_cas_connection(&endpoint_str).await {
                    Ok(store) => store,
                    Err(e) => {
                        warn!(
                            %operation_id,
                            worker_endpoint = %endpoint_str,
                            ?e,
                            "prefetch: failed to connect to worker CAS"
                        );
                        return;
                    }
                }
            };

            // Bulk has() check to filter out blobs the worker already has.
            // This avoids re-reading and re-pushing blobs that arrived via
            // concurrent actions or peer sharing.
            let store_keys: Vec<StoreKey<'_>> = missing_blobs
                .iter()
                .map(|(digest, _)| (*digest).into())
                .collect();
            let mut has_results = vec![None; store_keys.len()];
            let has_check_ok = worker_store
                .has_with_results(&store_keys, &mut has_results)
                .await
                .is_ok();

            let mut actually_missing: Vec<(DigestInfo, u64)> = Vec::new();
            let mut blobs_already_present: u64 = 0;

            if has_check_ok {
                for (i, (digest, size)) in missing_blobs.iter().enumerate() {
                    if has_results[i].is_some() {
                        blobs_already_present += 1;
                    } else {
                        actually_missing.push((*digest, *size));
                    }
                }
            } else {
                // has() failed, try pushing everything anyway
                actually_missing = missing_blobs;
            }

            if actually_missing.is_empty() {
                metrics
                    .prefetch_blobs_already_present
                    .fetch_add(blobs_already_present, Ordering::Relaxed);
                info!(
                    %operation_id,
                    worker_endpoint = %endpoint_str,
                    blobs_already_present,
                    elapsed_ms = start.elapsed().as_millis() as u64,
                    "prefetch: all blobs already present on worker"
                );
                return;
            }

            // Group blobs into batches of up to PREFETCH_BATCH_SIZE_BYTES.
            // Each batch will be read from CAS and pushed via update_oneshot,
            // which routes through BatchUpdateBlobs on the GrpcStore.
            let mut batches: Vec<Vec<(DigestInfo, u64)>> = Vec::new();
            let mut current_batch: Vec<(DigestInfo, u64)> = Vec::new();
            let mut current_batch_bytes: u64 = 0;

            for (digest, size) in &actually_missing {
                if !current_batch.is_empty()
                    && current_batch_bytes + size > PREFETCH_BATCH_SIZE_BYTES
                {
                    batches.push(core::mem::take(&mut current_batch));
                    current_batch_bytes = 0;
                }
                current_batch.push((*digest, *size));
                current_batch_bytes += size;
            }
            if !current_batch.is_empty() {
                batches.push(current_batch);
            }

            let batch_count = batches.len();
            let mut blobs_sent: u64 = 0;
            let mut bytes_sent: u64 = 0;
            let mut blobs_failed: u64 = 0;
            let mut batches_sent: u64 = 0;

            // Process batches with concurrency limited by the per-worker
            // semaphore. Each batch task reads blobs from server CAS and
            // pushes them via update_oneshot (-> BatchUpdateBlobs).
            let mut join_set = tokio::task::JoinSet::new();

            for batch in batches {
                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => break, // semaphore closed
                };

                let cas = cas_store.clone();
                let worker = worker_store.clone();
                let op_id = operation_id.clone();
                let ep = endpoint_str.clone();

                join_set.spawn(async move {
                    let _permit = permit; // held until this batch completes

                    let mut batch_blobs_sent: u64 = 0;
                    let mut batch_bytes_sent: u64 = 0;
                    let mut batch_blobs_failed: u64 = 0;

                    // Read each blob from server CAS into memory (safe -- all
                    // blobs are under PREFETCH_MAX_SINGLE_BLOB_SIZE) and push
                    // via update_oneshot which routes through BatchUpdateBlobs.
                    for (digest, size) in &batch {
                        let key: StoreKey<'_> = (*digest).into();

                        let data: Bytes = match cas
                            .get_part_unchunked(key.borrow(), 0, None)
                            .await
                        {
                            Ok(d) => d,
                            Err(e) => {
                                debug!(
                                    %op_id,
                                    %digest,
                                    size,
                                    ?e,
                                    "prefetch: failed to read blob from server CAS"
                                );
                                batch_blobs_failed += 1;
                                continue;
                            }
                        };

                        match worker.update_oneshot(key.borrow(), data).await {
                            Ok(()) => {
                                batch_blobs_sent += 1;
                                batch_bytes_sent += size;
                            }
                            Err(e) => {
                                debug!(
                                    %op_id,
                                    worker_endpoint = %ep,
                                    %digest,
                                    size,
                                    ?e,
                                    "prefetch: failed to push blob to worker"
                                );
                                batch_blobs_failed += 1;
                            }
                        }
                    }

                    (batch_blobs_sent, batch_bytes_sent, batch_blobs_failed)
                });
            }

            // Collect results.
            while let Some(result) = join_set.join_next().await {
                match result {
                    Ok((sent, bytes, failed)) => {
                        blobs_sent += sent;
                        bytes_sent += bytes;
                        blobs_failed += failed;
                        batches_sent += 1;
                    }
                    Err(e) => {
                        warn!(?e, "prefetch: batch task panicked");
                        blobs_failed += 1;
                    }
                }
            }

            // Update global metrics.
            metrics
                .prefetch_blobs_sent
                .fetch_add(blobs_sent, Ordering::Relaxed);
            metrics
                .prefetch_bytes_sent
                .fetch_add(bytes_sent, Ordering::Relaxed);
            metrics
                .prefetch_blobs_failed
                .fetch_add(blobs_failed, Ordering::Relaxed);
            metrics
                .prefetch_blobs_already_present
                .fetch_add(blobs_already_present, Ordering::Relaxed);
            metrics
                .prefetch_batches_sent
                .fetch_add(batches_sent, Ordering::Relaxed);

            let elapsed = start.elapsed();
            info!(
                %operation_id,
                worker_endpoint = %endpoint_str,
                blob_count,
                batch_count,
                batches_sent,
                blobs_sent,
                bytes_sent,
                blobs_failed,
                blobs_already_present,
                elapsed_ms = elapsed.as_millis() as u64,
                "prefetch: completed batched push to worker"
            );
        });
    }

    /// Spawns a background task that warms the server-side MemoryStore by
    /// reading blobs through the full CAS store chain. For blobs already in
    /// MemoryStore, `FastSlowStore::get_part()` returns from the fast store
    /// in ~1-5us (near-no-op). For cold blobs, the read populates MemoryStore
    /// via `populate_and_maybe_stream`. The returned `Bytes` are dropped
    /// immediately — we only need the warming side effect.
    fn spawn_server_cache_warm(
        &self,
        file_digests: &[(DigestInfo, u64)],
        operation_id: &OperationId,
    ) {
        let cas_store = match &self.cas_store {
            Some(s) => s.clone(),
            None => return,
        };

        if file_digests.is_empty() {
            return;
        }

        // Sort by size ascending so we warm many small blobs first.
        let mut sorted: Vec<(DigestInfo, u64)> = file_digests.to_vec();
        sorted.sort_unstable_by_key(|(_, size)| *size);

        // Cap at CACHE_WARM_MAX_BLOBS and CACHE_WARM_MAX_BYTES total.
        let mut total_bytes: u64 = 0;
        let mut selected: Vec<DigestInfo> = Vec::with_capacity(
            sorted.len().min(CACHE_WARM_MAX_BLOBS),
        );
        for (digest, size) in &sorted {
            if selected.len() >= CACHE_WARM_MAX_BLOBS {
                break;
            }
            if total_bytes + size > CACHE_WARM_MAX_BYTES && !selected.is_empty() {
                break;
            }
            total_bytes += size;
            selected.push(*digest);
        }

        let blob_count = selected.len();
        let op_id = operation_id.to_string();

        self.metrics.cache_warm_spawned.inc();

        info!(
            %operation_id,
            blob_count,
            total_bytes,
            "cache_warm: spawning server-side MemoryStore warm"
        );

        tokio::spawn(async move {
            let start = Instant::now();
            let semaphore = Arc::new(Semaphore::new(CACHE_WARM_CONCURRENCY));
            let mut join_set = tokio::task::JoinSet::new();

            for digest in selected {
                let permit = match semaphore.clone().acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => break,
                };
                let store = cas_store.clone();

                join_set.spawn(async move {
                    let _permit = permit;
                    let key: StoreKey<'_> = digest.into();
                    match store.get_part_unchunked(key.borrow(), 0, None).await {
                        Ok(_bytes) => true,
                        Err(e) => {
                            warn!(
                                %digest,
                                ?e,
                                "cache_warm: failed to warm blob"
                            );
                            false
                        }
                    }
                });
            }

            let mut warmed: u64 = 0;
            let mut failed: u64 = 0;
            while let Some(result) = join_set.join_next().await {
                match result {
                    Ok(true) => warmed += 1,
                    Ok(false) => failed += 1,
                    Err(e) => {
                        warn!(?e, "cache_warm: task panicked");
                        failed += 1;
                    }
                }
            }

            let elapsed_ms = start.elapsed().as_millis() as u64;
            info!(
                op_id = %op_id,
                blob_count,
                warmed,
                failed,
                total_bytes,
                elapsed_ms,
                "cache_warm: completed server-side MemoryStore warm"
            );
        });
    }

    /// Broadcast a `BlobsInStableStorage` message to all connected workers.
    /// Disconnected workers are silently skipped (they will be reaped by the
    /// timeout mechanism). Takes a read lock on the worker map briefly to
    /// clone the sender handles, then sends outside the lock.
    pub async fn broadcast_blobs_in_stable_storage(&self, digests: Vec<DigestInfo>) {
        if digests.is_empty() {
            return;
        }
        let proto_digests: Vec<Digest> = digests.iter().map(Digest::from).collect();
        let msg = update_for_worker::Update::BlobsInStableStorage(BlobsInStableStorage {
            digests: proto_digests,
        });

        // Collect sender handles under a brief read lock, then send outside.
        let senders: Vec<_> = {
            let inner = self.inner.read().await;
            inner
                .workers
                .iter()
                .map(|(_, w)| w.tx.clone())
                .collect()
        };

        let worker_count = senders.len();
        let mut send_failures = 0usize;
        for tx in &senders {
            if tx
                .send(UpdateForWorker {
                    update: Some(msg.clone()),
                })
                .is_err()
            {
                send_failures += 1;
            }
        }

        let digest_count = digests.len();
        if send_failures > 0 {
            debug!(
                digest_count,
                worker_count,
                send_failures,
                "broadcast blobs_in_stable_storage had send failures"
            );
        } else {
            trace!(
                digest_count,
                worker_count,
                "broadcast blobs_in_stable_storage"
            );
        }
    }
}

/// Resolved input tree containing file digests, directory digests,
/// per-subtree file byte totals for coverage scoring, and the decoded
/// Directory protos (for forwarding to workers so they skip GetTree).
struct ResolvedTree {
    /// (file_digest, file_size) pairs, deduplicated.
    file_digests: Vec<(DigestInfo, u64)>,
    /// All directory digests in the tree (including root), deduplicated.
    dir_digests: HashSet<DigestInfo>,
    /// Total file bytes under each directory subtree (recursive).
    /// Used to weight subtree coverage scoring — a subtree with 10GB
    /// of files is worth more than one with 100 bytes.
    subtree_bytes: HashMap<DigestInfo, u64>,
    /// Total file count under each directory subtree (recursive).
    /// Blended with subtree_bytes for coverage scoring: many small files
    /// have higher per-file I/O cost (hardlinks, clonefile) than fewer
    /// large files at the same total byte count.
    subtree_files: HashMap<DigestInfo, u64>,
    /// Decoded Directory protos keyed by their digest. Forwarded to workers
    /// in StartExecute so they can skip the redundant GetTree RPC.
    directories: HashMap<DigestInfo, Directory>,
}

impl ResolvedTree {
    /// Converts the directory map into protobuf-ready Vecs. This involves
    /// cloning each Directory proto and is intentionally called outside the
    /// scheduler write lock to avoid blocking dispatch.
    fn to_proto_vecs(&self) -> (Vec<Directory>, Vec<Digest>) {
        let mut dirs = Vec::with_capacity(self.directories.len());
        let mut digests = Vec::with_capacity(self.directories.len());
        for (digest_info, directory) in &self.directories {
            digests.push((*digest_info).into());
            dirs.push(directory.clone());
        }
        (dirs, digests)
    }
}

/// Creates a GrpcStore connection to a worker's CAS endpoint for
/// prefetching blobs. This is a standalone function so it can be
/// called from both `get_or_create_prefetch_connection` and from
/// inside spawned tasks without holding a reference to `self`.
async fn create_worker_cas_connection(endpoint: &str) -> Result<Store, Error> {
    let spec = GrpcSpec {
        instance_name: String::new(),
        endpoints: vec![GrpcEndpoint {
            address: endpoint.to_string(),
            tls_config: None,
            concurrency_limit: None,
            connect_timeout_s: 5,
            tcp_keepalive_s: 30,
            http2_keepalive_interval_s: 30,
            http2_keepalive_timeout_s: 20,
            tcp_nodelay: true,
            use_http3: false,
        }],
        store_type: StoreType::Cas,
        retry: Retry::default(),
        max_concurrent_requests: 0,
        connections_per_endpoint: 16,
        rpc_timeout_s: 120,
        batch_update_threshold_bytes: 4 * 1024 * 1024,
        batch_coalesce_delay_ms: 0,
        max_concurrent_batch_rpcs: 8,
        parallel_chunk_read_threshold: 8 * 1024 * 1024,
        parallel_chunk_count: 4,
    };
    let store = GrpcStore::new(&spec)
        .await
        .err_tip(|| format!("Creating prefetch connection to worker {endpoint}"))?;
    Ok(Store::new(store))
}

/// Resolves a directory tree from the CAS store by recursively reading
/// Directory protos and collecting file digests (for locality scoring),
/// directory digests (for subtree coverage scoring), and per-subtree
/// file byte totals (for weighted coverage scoring). Deduplicates both
/// file and directory digests.
///
/// `failed_dir_digests` is a shared negative cache for individual directory
/// digests that failed during BFS. Before fetching each directory, we check
/// this cache and fail fast if the digest is known-bad. On NotFound errors,
/// the failing digest is recorded with a 60s expiry.
async fn resolve_tree_from_cas(
    cas_store: &Store,
    root_digest: DigestInfo,
    failed_dir_digests: &Arc<tokio::sync::Mutex<HashMap<DigestInfo, Instant>>>,
) -> Result<ResolvedTree, Error> {
    use futures::stream::FuturesUnordered;
    use futures::StreamExt;

    /// How long individual directory digest failures are cached.
    const DIR_FAILURE_TTL: Duration = Duration::from_secs(60);

    let mut file_digests: Vec<(DigestInfo, u64)> = Vec::new();
    let mut seen_files: HashSet<DigestInfo> = HashSet::new();
    let mut dirs_to_visit: Vec<DigestInfo> = vec![root_digest];
    let mut seen_dirs: HashSet<DigestInfo> = HashSet::new();
    seen_dirs.insert(root_digest);
    let mut directories: HashMap<DigestInfo, Directory> = HashMap::new();

    // Track tree structure for bottom-up subtree size/file-count computation.
    let mut dir_direct_bytes: HashMap<DigestInfo, u64> = HashMap::new();
    let mut dir_direct_files: HashMap<DigestInfo, u64> = HashMap::new();
    let mut dir_children: HashMap<DigestInfo, Vec<DigestInfo>> = HashMap::new();
    // BFS order — used for bottom-up traversal (reverse of BFS = leaves first).
    let mut bfs_order: Vec<DigestInfo> = vec![root_digest];

    while !dirs_to_visit.is_empty() {
        // Check subdirectory negative cache before fetching this BFS level.
        {
            let mut cache = failed_dir_digests.lock().await;
            // Sweep expired entries to prevent unbounded growth.
            if cache.len() > NEGATIVE_CACHE_SWEEP_THRESHOLD {
                cache.retain(|_, failed_at: &mut Instant| {
                    failed_at.elapsed() < DIR_FAILURE_TTL
                });
            }
            for dir_digest in &dirs_to_visit {
                if let Some(&failed_at) = cache.get(dir_digest) {
                    if failed_at.elapsed() < DIR_FAILURE_TTL {
                        return Err(make_err!(
                            Code::NotFound,
                            "directory {dir_digest} is in subdirectory negative cache (failed {:.1}s ago)",
                            failed_at.elapsed().as_secs_f64()
                        ));
                    }
                    // Entry has expired — remove it inline since we hold the lock.
                    cache.remove(dir_digest);
                }
            }
        }

        let failed_dir_digests_clone = failed_dir_digests.clone();
        let fetches: FuturesUnordered<_> = dirs_to_visit
            .drain(..)
            .map(|dir_digest| {
                let cas_store = cas_store.clone();
                let failed_dirs = failed_dir_digests_clone.clone();
                async move {
                    let key: StoreKey<'_> = dir_digest.into();
                    let result = cas_store
                        .get_part_unchunked(key, 0, None)
                        .await
                        .err_tip(|| {
                            format!(
                                "Reading directory {dir_digest} from CAS for tree resolution"
                            )
                        });
                    match result {
                        Ok(bytes) => {
                            let directory = Directory::decode(bytes).map_err(|e| {
                                make_err!(Code::Internal, "Failed to decode Directory proto: {e}")
                            })?;
                            Ok::<_, Error>((dir_digest, directory))
                        }
                        Err(err) => {
                            // Record the specific failing subdirectory digest.
                            if err.code == Code::NotFound {
                                warn!(
                                    %dir_digest,
                                    "directory blob not found in CAS, caching as failed subdirectory"
                                );
                                failed_dirs.lock().await.insert(dir_digest, Instant::now());
                            }
                            Err(err)
                        }
                    }
                }
            })
            .collect();

        let results: Vec<Result<(DigestInfo, Directory), Error>> = fetches.collect().await;
        for result in results {
            let (parent_digest, directory) = result?;

            // Sum direct file bytes and count for this directory.
            let mut direct_bytes: u64 = 0;
            let mut direct_files: u64 = 0;
            for file_node in &directory.files {
                if let Some(ref digest) = file_node.digest {
                    if let Ok(digest_info) = DigestInfo::try_from(digest) {
                        let size = digest_info.size_bytes();
                        direct_bytes += size;
                        direct_files += 1;
                        if seen_files.insert(digest_info) {
                            file_digests.push((digest_info, size));
                        }
                    }
                }
            }
            dir_direct_bytes.insert(parent_digest, direct_bytes);
            dir_direct_files.insert(parent_digest, direct_files);

            // Queue subdirectories for visiting (dedup via seen_dirs).
            let mut children = Vec::new();
            for dir_node in &directory.directories {
                if let Some(ref digest) = dir_node.digest {
                    if let Ok(digest_info) = DigestInfo::try_from(digest) {
                        children.push(digest_info);
                        if seen_dirs.insert(digest_info) {
                            dirs_to_visit.push(digest_info);
                            bfs_order.push(digest_info);
                        }
                    }
                }
            }
            dir_children.insert(parent_digest, children);
            directories.insert(parent_digest, directory);
        }
    }

    // Bottom-up pass: compute total file bytes and file count under each subtree.
    // Reverse BFS order gives us leaves-first, so children are always
    // computed before parents.
    let mut subtree_bytes: HashMap<DigestInfo, u64> = HashMap::new();
    let mut subtree_files: HashMap<DigestInfo, u64> = HashMap::new();
    for &dir_digest in bfs_order.iter().rev() {
        let direct_b = dir_direct_bytes.get(&dir_digest).copied().unwrap_or(0);
        let direct_f = dir_direct_files.get(&dir_digest).copied().unwrap_or(0);
        let (children_bytes, children_files): (u64, u64) = dir_children
            .get(&dir_digest)
            .map(|children| {
                children.iter().fold((0u64, 0u64), |(ab, af), c| {
                    (
                        ab + subtree_bytes.get(c).copied().unwrap_or(0),
                        af + subtree_files.get(c).copied().unwrap_or(0),
                    )
                })
            })
            .unwrap_or((0, 0));
        subtree_bytes.insert(dir_digest, direct_b + children_bytes);
        subtree_files.insert(dir_digest, direct_f + children_files);
    }

    Ok(ResolvedTree {
        file_digests,
        dir_digests: seen_dirs,
        subtree_bytes,
        subtree_files,
        directories,
    })
}

/// Scores endpoints by the total bytes of input blobs they have cached
/// AND generates peer hints in a single pass over the file digests,
/// acquiring the locality map read lock only once.
///
/// Returns:
/// - `HashMap<Arc<str>, (u64, SystemTime)>`: endpoint scores (total cached
///   bytes, most recent blob timestamp)
/// - `Vec<PeerHint>`: peer hints sorted by file size descending, truncated
///   to MAX_PEER_HINTS
///
/// This is called OUTSIDE the scheduler write lock, so it does not need
/// access to `endpoint_to_worker` or the candidate set. The caller maps
/// endpoints to WorkerIds and filters to candidates inside the lock.
fn score_and_generate_hints(
    file_digests: &[(DigestInfo, u64)],
    locality_map: &SharedBlobLocalityMap,
) -> (HashMap<Arc<str>, (u64, SystemTime)>, Vec<PeerHint>) {
    /// Maximum number of peer hints to include in a StartExecute message
    /// to avoid oversized messages.
    const MAX_PEER_HINTS: usize = 16384;

    let map = locality_map.read();
    let blobs = map.blobs_map();
    let mut scores: HashMap<Arc<str>, (u64, SystemTime)> = HashMap::new();
    let mut hint_candidates: Vec<(DigestInfo, u64, Vec<Arc<str>>)> = Vec::new();

    for &(digest, size) in file_digests {
        if let Some(endpoints) = blobs.get(&digest) {
            // Accumulate endpoint scores.
            for (endpoint, ts) in endpoints {
                let entry = scores
                    .entry(endpoint.clone())
                    .or_insert((0, UNIX_EPOCH));
                entry.0 += size;
                if *ts > entry.1 {
                    entry.1 = *ts;
                }
            }
            // Collect hint candidate if this digest has peer locations.
            if !endpoints.is_empty() {
                let peer_eps: Vec<Arc<str>> =
                    endpoints.keys().cloned().collect();
                hint_candidates.push((digest, size, peer_eps));
            }
        }
    }

    // Sort by size descending to prioritize large files.
    hint_candidates.sort_by(|a, b| b.1.cmp(&a.1));
    hint_candidates.truncate(MAX_PEER_HINTS);

    let peer_hints: Vec<PeerHint> = hint_candidates
        .into_iter()
        .map(|(digest, _size, peer_endpoints)| PeerHint {
            digest: Some(digest.into()),
            peer_endpoints: peer_endpoints.iter().map(|e| e.to_string()).collect(),
        })
        .collect();

    (scores, peer_hints)
}

/// Converts endpoint scores to worker scores using the endpoint-to-worker
/// mapping, filtering to the given candidate set.
///
/// Returns `HashMap<WorkerId, (u64, SystemTime)>` where the tuple is
/// (total cached bytes, most recent blob timestamp across all endpoints
/// belonging to this worker).
fn endpoint_scores_to_worker_scores(
    endpoint_scores: &HashMap<Arc<str>, (u64, SystemTime)>,
    endpoint_to_worker: &HashMap<Arc<str>, WorkerId>,
    candidates: &HashSet<WorkerId>,
) -> HashMap<WorkerId, (u64, SystemTime)> {
    let mut worker_scores: HashMap<WorkerId, (u64, SystemTime)> = HashMap::new();
    for (endpoint, &(score, ts)) in endpoint_scores {
        if let Some(worker_id) = endpoint_to_worker.get(endpoint) {
            if candidates.contains(worker_id) {
                let entry = worker_scores
                    .entry(worker_id.clone())
                    .or_insert((0, UNIX_EPOCH));
                entry.0 += score;
                if ts > entry.1 {
                    entry.1 = ts;
                }
            }
        }
    }
    worker_scores
}

/// Backward-compatible wrapper used by existing tests. Scores candidate
/// workers by the total bytes of input blobs they have cached.
/// Returns only the byte score (drops the timestamp) for simpler assertions.
#[cfg(test)]
fn score_workers(
    candidates: &HashSet<WorkerId>,
    file_digests: &[(DigestInfo, u64)],
    locality_map: &SharedBlobLocalityMap,
    endpoint_to_worker: &HashMap<Arc<str>, WorkerId>,
) -> HashMap<WorkerId, u64> {
    let (endpoint_scores, _hints) = score_and_generate_hints(file_digests, locality_map);
    let full_scores = endpoint_scores_to_worker_scores(&endpoint_scores, endpoint_to_worker, candidates);
    full_scores.into_iter().map(|(wid, (score, _))| (wid, score)).collect()
}

#[async_trait]
impl WorkerScheduler for ApiWorkerScheduler {
    fn get_platform_property_manager(&self) -> &PlatformPropertyManager {
        self.platform_property_manager.as_ref()
    }

    async fn add_worker(&self, worker: Worker) -> Result<(), Error> {
        let worker_id = worker.id.clone();
        let worker_timestamp = worker.last_update_timestamp;
        let mut inner = self.inner.write().await;
        if inner.shutting_down {
            warn!("Rejected worker add during shutdown: {}", worker_id);
            return Err(make_err!(
                Code::Unavailable,
                "Received request to add worker while shutting down"
            ));
        }
        let result = inner
            .add_worker(worker)
            .err_tip(|| "Error while adding worker, removing from pool");
        if let Err(err) = result {
            return Result::<(), _>::Err(err.clone())
                .merge(inner.immediate_evict_worker(&worker_id, err, false).await);
        }

        let now = UNIX_EPOCH + Duration::from_secs(worker_timestamp);
        self.worker_registry.register_worker(&worker_id, now).await;

        // Scores cache is NOT cleared here. The LRU cache (1024 entries) will
        // naturally evict stale entries. Slightly stale scores only produce
        // suboptimal worker selection for one scheduling cycle, which is
        // acceptable compared to losing the entire cache on every worker churn.

        self.metrics.workers_added.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn update_action(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        let mut inner = self.inner.write().await;
        inner.update_action(worker_id, operation_id, update).await
    }

    async fn worker_keep_alive_received(
        &self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
    ) -> Result<(), Error> {
        {
            let mut inner = self.inner.write().await;
            inner
                .refresh_lifetime(worker_id, timestamp)
                .err_tip(|| "Error refreshing lifetime in worker_keep_alive_received()")?;
        }
        let now = UNIX_EPOCH + Duration::from_secs(timestamp);
        self.worker_registry
            .update_worker_heartbeat(worker_id, now)
            .await;
        Ok(())
    }

    async fn remove_worker(&self, worker_id: &WorkerId) -> Result<(), Error> {
        self.worker_registry.remove_worker(worker_id).await;

        // Scores cache is NOT cleared here — see add_worker comment.

        // Grab the worker's CAS endpoint before eviction so we can clean
        // up prefetch state after the lock is released.
        let cas_endpoint: Option<Arc<str>> = {
            let inner = self.inner.read().await;
            inner
                .workers
                .peek(worker_id)
                .filter(|w| !w.cas_endpoint.is_empty())
                .map(|w| Arc::from(w.cas_endpoint.as_str()))
        };

        let result = {
            let mut inner = self.inner.write().await;
            inner
                .immediate_evict_worker(
                    worker_id,
                    make_err!(Code::Internal, "Received request to remove worker"),
                    false,
                )
                .await
        };

        // Clean up prefetch connection and semaphore for this endpoint.
        if let Some(ep) = cas_endpoint {
            self.remove_prefetch_for_endpoint(&ep);
        }

        result
    }

    async fn shutdown(&self, shutdown_guard: ShutdownGuard) {
        let mut inner = self.inner.write().await;
        inner.shutting_down = true; // should reject further worker registration
        while let Some(worker_id) = inner
            .workers
            .peek_lru()
            .map(|(worker_id, _worker)| worker_id.clone())
        {
            if let Err(err) = inner
                .immediate_evict_worker(
                    &worker_id,
                    make_err!(Code::Internal, "Scheduler shutdown"),
                    true,
                )
                .await
            {
                error!(?err, "Error evicting worker on shutdown.");
            }
        }
        drop(shutdown_guard);
    }

    async fn remove_timedout_workers(&self, now_timestamp: WorkerTimestamp) -> Result<(), Error> {
        // Check worker liveness using both the local timestamp (from LRU)
        // and the worker registry. A worker is alive if either source says it's alive.
        //
        // Quarantine phase: workers that miss keepalive for > worker_timeout but
        // < 2*worker_timeout are quarantined (stop receiving new work) rather than
        // immediately evicted. Workers that miss keepalive for >= 2*worker_timeout
        // are fully evicted.
        let timeout = Duration::from_secs(self.worker_timeout_s);
        let now = UNIX_EPOCH + Duration::from_secs(now_timestamp);
        let timeout_threshold = now_timestamp.saturating_sub(self.worker_timeout_s);
        let evict_threshold = now_timestamp.saturating_sub(self.worker_timeout_s * 2);

        // Collect (worker_id, local_alive, already_quarantined) for workers that
        // have not responded within the base timeout window.
        let workers_to_check: Vec<(WorkerId, bool, bool)> = {
            let inner = self.inner.read().await;
            inner
                .workers
                .iter()
                .filter_map(|(worker_id, worker)| {
                    let local_alive = worker.last_update_timestamp > timeout_threshold;
                    if local_alive {
                        None
                    } else {
                        let already_quarantined = worker.quarantined_at.is_some();
                        // Check if past the eviction threshold (2x timeout)
                        let past_evict_threshold =
                            worker.last_update_timestamp <= evict_threshold;
                        Some((worker_id.clone(), past_evict_threshold, already_quarantined))
                    }
                })
                .collect()
        };

        if workers_to_check.is_empty() {
            return Ok(());
        }

        // For each candidate, consult the registry to determine actual liveness.
        let mut workers_to_quarantine = Vec::new();
        let mut worker_ids_to_remove = Vec::new();
        for (worker_id, past_evict_threshold, already_quarantined) in workers_to_check {
            let registry_alive = self
                .worker_registry
                .is_worker_alive(&worker_id, timeout, now)
                .await;

            if registry_alive {
                // Registry says alive — no action needed.
                continue;
            }

            if past_evict_threshold {
                // Has been unresponsive for >= 2x the timeout — evict.
                trace!(
                    ?worker_id,
                    past_evict_threshold,
                    "Worker exceeded double-timeout, evicting from pool"
                );
                worker_ids_to_remove.push(worker_id);
            } else if !already_quarantined {
                // Has been unresponsive for > timeout but < 2x timeout — quarantine.
                trace!(
                    ?worker_id,
                    "Worker missed keepalive, entering quarantine (stops receiving work)"
                );
                workers_to_quarantine.push(worker_id);
            }
            // If already_quarantined && !past_evict_threshold: still waiting, no action.
        }

        if workers_to_quarantine.is_empty() && worker_ids_to_remove.is_empty() {
            return Ok(());
        }

        let mut inner = self.inner.write().await;

        // Apply quarantine to workers that just crossed the first timeout.
        let quarantine_time = SystemTime::now();
        for worker_id in &workers_to_quarantine {
            if let Some(worker) = inner.workers.peek_mut(worker_id) {
                warn!(
                    ?worker_id,
                    "Worker missed keepalive, quarantining (will not receive new work)"
                );
                worker.quarantined_at = Some(quarantine_time);
            }
        }
        // Notify the matching engine so it skips quarantined workers on next cycle.
        if !workers_to_quarantine.is_empty() {
            inner.worker_change_notify.notify_one();
        }

        // Scores cache is NOT cleared on worker eviction — see add_worker comment.

        let mut result = Ok(());
        for worker_id in &worker_ids_to_remove {
            warn!(?worker_id, "Worker timed out (2x timeout), removing from pool");
            result = result.merge(
                inner
                    .immediate_evict_worker(
                        worker_id,
                        make_err!(
                            Code::Internal,
                            "Worker {worker_id} timed out, removing from pool"
                        ),
                        false,
                    )
                    .await,
            );
        }

        // Clean up prefetch maps for endpoints no longer in the worker pool.
        if !worker_ids_to_remove.is_empty() {
            let active_endpoints: HashSet<Arc<str>> =
                inner.endpoint_to_worker.keys().cloned().collect();
            drop(inner);
            self.cleanup_stale_prefetch_entries(&active_endpoints);
        }

        result
    }

    async fn set_drain_worker(&self, worker_id: &WorkerId, is_draining: bool) -> Result<(), Error> {
        let mut inner = self.inner.write().await;
        inner.set_drain_worker(worker_id, is_draining).await
    }

    async fn update_worker_load(
        &self,
        worker_id: &WorkerId,
        cpu_load_pct: u32,
        p_core_load_pct: u32,
        e_core_load_pct: u32,
    ) -> Result<(), Error> {
        // Use peek_mut to avoid promoting the worker in the LRU cache —
        // load updates should not affect scheduling order.
        let mut inner = self.inner.write().await;
        let worker = inner.workers.0.peek_mut(worker_id).ok_or_else(|| {
            make_input_err!(
                "Worker not found in worker map in update_worker_load() {}",
                worker_id
            )
        })?;
        worker.cpu_load_pct = cpu_load_pct;
        worker.p_core_load_pct = p_core_load_pct;
        worker.e_core_load_pct = e_core_load_pct;
        debug!(%worker_id, cpu_load_pct, p_core_load_pct, e_core_load_pct, "Worker load updated");
        Ok(())
    }

    async fn update_cached_directories(
        &self,
        worker_id: &WorkerId,
        digests: HashSet<DigestInfo>,
    ) -> Result<(), Error> {
        let mut inner = self.inner.write().await;
        let worker = inner.workers.0.peek_mut(worker_id).ok_or_else(|| {
            make_input_err!(
                "Worker not found in worker map in update_cached_directories() {}",
                worker_id
            )
        })?;
        let count = digests.len();
        worker.cached_directory_digests = digests;
        debug!(%worker_id, count, "Worker cached directory digests updated");
        Ok(())
    }

    async fn update_cached_subtrees(
        &self,
        worker_id: &WorkerId,
        is_full_snapshot: bool,
        full_set: Vec<DigestInfo>,
        added: Vec<DigestInfo>,
        removed: Vec<DigestInfo>,
    ) -> Result<(), Error> {
        let mut inner = self.inner.write().await;
        let worker = inner.workers.0.peek_mut(worker_id).ok_or_else(|| {
            make_input_err!(
                "Worker not found in worker map in update_cached_subtrees() {}",
                worker_id
            )
        })?;
        if is_full_snapshot {
            let count = full_set.len();
            worker.cached_subtree_digests = full_set.into_iter().collect();
            debug!(%worker_id, count, "Worker cached subtree digests replaced (full snapshot)");
        } else {
            let added_count = added.len();
            let removed_count = removed.len();
            for digest in added {
                worker.cached_subtree_digests.insert(digest);
            }
            for digest in &removed {
                worker.cached_subtree_digests.remove(digest);
            }
            let total = worker.cached_subtree_digests.len();
            debug!(
                %worker_id,
                added_count,
                removed_count,
                total,
                "Worker cached subtree digests updated (delta)"
            );
        }
        Ok(())
    }

    async fn broadcast_blobs_in_stable_storage(&self, digests: Vec<DigestInfo>) {
        self.broadcast_blobs_in_stable_storage(digests).await;
    }
}

impl RootMetricsComponent for ApiWorkerScheduler {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use bytes::Bytes;
    use nativelink_config::stores::MemorySpec;
    use nativelink_proto::build::bazel::remote::execution::v2::{
        Digest as ProtoDigest, DirectoryNode, FileNode,
    };
    use nativelink_store::memory_store::MemoryStore;
    use nativelink_util::blob_locality_map::new_shared_blob_locality_map;
    use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};

    #[test]
    fn test_effective_load_score_per_type_p_cores_available() {
        // P-cores not saturated: score equals p_load.
        assert_eq!(effective_load_score(50, 30, 70), 50);
        assert_eq!(effective_load_score(1, 100, 80), 1);
        assert_eq!(effective_load_score(99, 0, 50), 99);
    }

    #[test]
    fn test_effective_load_score_per_type_p_cores_saturated() {
        // P-cores at 100%: score = 100 + e_load, always worse than any
        // worker with available P-cores.
        assert_eq!(effective_load_score(100, 50, 95), 150);
        assert_eq!(effective_load_score(100, 0, 100), 100);
        assert_eq!(effective_load_score(100, 100, 100), 200);
    }

    #[test]
    fn test_effective_load_score_aggregate_only() {
        // Old worker or Linux: p=0, e=0, aggregate>0 → use aggregate.
        assert_eq!(effective_load_score(0, 0, 60), 60);
        assert_eq!(effective_load_score(0, 0, 1), 1);
        assert_eq!(effective_load_score(0, 0, 100), 100);
    }

    #[test]
    fn test_effective_load_score_unknown() {
        // All zeros: unknown → sort last.
        assert_eq!(effective_load_score(0, 0, 0), u64::MAX);
    }

    #[test]
    fn test_effective_load_score_p_core_only_idle() {
        // P-core-only Apple Silicon (no E-cores): reports p=0, e=100.
        // Machine is idle → score should be 0 (best).
        assert_eq!(effective_load_score(0, 100, 0), 0);
    }

    #[test]
    fn test_effective_load_score_p_core_only_saturated() {
        // P-core-only fully loaded: p=100, e=100.
        // Score = 100 + 100 = 200 (worst among per-type reporters).
        assert_eq!(effective_load_score(100, 100, 100), 200);
    }

    #[test]
    fn test_effective_load_score_ordering() {
        // Verify the two-tier preference: idle P-cores always beat
        // workers with only idle E-cores.
        let idle_p = effective_load_score(30, 80, 50);
        let saturated_p = effective_load_score(100, 20, 90);
        let aggregate = effective_load_score(0, 0, 40);
        let unknown = effective_load_score(0, 0, 0);

        assert!(idle_p < saturated_p, "idle P-cores should beat saturated P-cores");
        assert!(aggregate < saturated_p, "aggregate-only in P-tier should beat E-core-only");
        assert!(saturated_p < unknown, "known load should beat unknown");
    }

    /// Helper: encode a Directory proto and compute its DigestInfo (SHA256).
    fn encode_directory(dir: &Directory) -> (Vec<u8>, DigestInfo) {
        let dir_bytes = dir.encode_to_vec();
        let mut hasher = DigestHasherFunc::Sha256.hasher();
        hasher.update(&dir_bytes);
        let digest_info = hasher.finalize_digest();
        (dir_bytes, digest_info)
    }

    /// Helper: create a FileNode with a deterministic fake digest.
    fn make_file_node(name: &str, hash_byte: u8, size: i64) -> FileNode {
        FileNode {
            name: name.to_string(),
            digest: Some(ProtoDigest {
                hash: format!("{:02x}", hash_byte).repeat(32), // 64-char hex
                size_bytes: size,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn test_score_workers_basic() {
        let locality_map = new_shared_blob_locality_map();
        let d1 = DigestInfo::new([1u8; 32], 1000);
        let d2 = DigestInfo::new([2u8; 32], 2000);
        let d3 = DigestInfo::new([3u8; 32], 3000);

        // worker-a has d1 and d2 (3000 bytes total)
        // worker-b has d2 and d3 (5000 bytes total)
        {
            let mut map = locality_map.write();
            map.register_blobs("grpc://worker-a:50081", &[d1, d2]);
            map.register_blobs("grpc://worker-b:50081", &[d2, d3]);
        }

        let worker_a = WorkerId::from("worker-a-id".to_string());
        let worker_b = WorkerId::from("worker-b-id".to_string());

        let mut endpoint_to_worker = HashMap::new();
        endpoint_to_worker.insert(Arc::from("grpc://worker-a:50081"), worker_a.clone());
        endpoint_to_worker.insert(Arc::from("grpc://worker-b:50081"), worker_b.clone());

        let mut candidates = HashSet::new();
        candidates.insert(worker_a.clone());
        candidates.insert(worker_b.clone());

        let file_digests = vec![(d1, 1000), (d2, 2000), (d3, 3000)];

        let scores = score_workers(&candidates, &file_digests, &locality_map, &endpoint_to_worker);

        assert_eq!(scores.get(&worker_a), Some(&3000)); // d1(1000) + d2(2000)
        assert_eq!(scores.get(&worker_b), Some(&5000)); // d2(2000) + d3(3000)
    }

    #[test]
    fn test_score_workers_non_candidate_excluded() {
        let locality_map = new_shared_blob_locality_map();
        let d1 = DigestInfo::new([1u8; 32], 1000);

        {
            let mut map = locality_map.write();
            map.register_blobs("grpc://worker-a:50081", &[d1]);
        }

        let worker_a = WorkerId::from("worker-a-id".to_string());
        let mut endpoint_to_worker = HashMap::new();
        endpoint_to_worker.insert(Arc::from("grpc://worker-a:50081"), worker_a.clone());

        // worker_a is NOT in candidates
        let candidates = HashSet::new();
        let file_digests = vec![(d1, 1000)];

        let scores = score_workers(&candidates, &file_digests, &locality_map, &endpoint_to_worker);
        assert!(scores.is_empty());
    }

    #[test]
    fn test_score_workers_empty_locality_map() {
        let locality_map = new_shared_blob_locality_map();
        let d1 = DigestInfo::new([1u8; 32], 1000);

        let worker_a = WorkerId::from("worker-a-id".to_string());
        let mut candidates = HashSet::new();
        candidates.insert(worker_a.clone());

        let endpoint_to_worker = HashMap::new();
        let file_digests = vec![(d1, 1000)];

        let scores = score_workers(&candidates, &file_digests, &locality_map, &endpoint_to_worker);
        assert!(scores.is_empty());
    }

    // ---------------------------------------------------------------
    // resolve_tree_from_cas tests
    // ---------------------------------------------------------------

    #[tokio::test]
    async fn test_resolve_tree_single_directory() {
        // A single directory with 3 files, no subdirectories.
        let dir = Directory {
            files: vec![
                make_file_node("file1.txt", 0xaa, 1000),
                make_file_node("file2.txt", 0xbb, 2000),
                make_file_node("file3.txt", 0xcc, 3000),
            ],
            directories: vec![],
            ..Default::default()
        };

        let (dir_bytes, dir_digest) = encode_directory(&dir);
        let store = Store::new(MemoryStore::new(&MemorySpec::default()));
        let key: StoreKey<'_> = dir_digest.into();
        store
            .update_oneshot(key, Bytes::from(dir_bytes))
            .await
            .expect("store update_oneshot failed");

        let failed_dirs = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let result = resolve_tree_from_cas(&store, dir_digest, &failed_dirs)
            .await
            .expect("resolve_tree_from_cas failed");

        assert_eq!(result.file_digests.len(), 3, "Expected 3 file digests");
        assert_eq!(result.dir_digests.len(), 1, "Expected 1 directory digest (root)");
        assert!(result.dir_digests.contains(&dir_digest));

        // Root subtree contains all files: 1000+2000+3000 = 6000
        assert_eq!(result.subtree_bytes.get(&dir_digest), Some(&6000));

        // Verify all three sizes are present (order may vary).
        let mut sizes: Vec<u64> = result.file_digests.iter().map(|&(_, s)| s).collect();
        sizes.sort();
        assert_eq!(sizes, vec![1000, 2000, 3000]);
    }

    #[tokio::test]
    async fn test_resolve_tree_nested_directories() {
        // Subdirectory with 2 files.
        let sub_dir = Directory {
            files: vec![
                make_file_node("sub_file1.txt", 0x11, 500),
                make_file_node("sub_file2.txt", 0x22, 700),
            ],
            directories: vec![],
            ..Default::default()
        };
        let (sub_dir_bytes, sub_dir_digest) = encode_directory(&sub_dir);

        // Root directory with 1 file and a reference to the subdirectory.
        let root_dir = Directory {
            files: vec![make_file_node("root_file.txt", 0x33, 1200)],
            directories: vec![DirectoryNode {
                name: "subdir".to_string(),
                digest: Some(sub_dir_digest.into()),
            }],
            ..Default::default()
        };
        let (root_dir_bytes, root_dir_digest) = encode_directory(&root_dir);

        let store = Store::new(MemoryStore::new(&MemorySpec::default()));
        let root_key: StoreKey<'_> = root_dir_digest.into();
        store
            .update_oneshot(root_key, Bytes::from(root_dir_bytes))
            .await
            .expect("store root dir");
        let sub_key: StoreKey<'_> = sub_dir_digest.into();
        store
            .update_oneshot(sub_key, Bytes::from(sub_dir_bytes))
            .await
            .expect("store sub dir");

        let failed_dirs = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let result = resolve_tree_from_cas(&store, root_dir_digest, &failed_dirs)
            .await
            .expect("resolve_tree_from_cas failed");

        assert_eq!(result.file_digests.len(), 3, "Expected 3 files (1 root + 2 subdir)");
        assert_eq!(result.dir_digests.len(), 2, "Expected 2 directory digests (root + subdir)");
        assert!(result.dir_digests.contains(&root_dir_digest));
        assert!(result.dir_digests.contains(&sub_dir_digest));

        // subdir has 500+700=1200 bytes of files
        assert_eq!(result.subtree_bytes.get(&sub_dir_digest), Some(&1200));
        // root has 1200 (own file) + 1200 (subdir subtree) = 2400
        assert_eq!(result.subtree_bytes.get(&root_dir_digest), Some(&2400));

        let mut sizes: Vec<u64> = result.file_digests.iter().map(|&(_, s)| s).collect();
        sizes.sort();
        assert_eq!(sizes, vec![500, 700, 1200]);
    }

    #[tokio::test]
    async fn test_resolve_tree_deduplicates_files() {
        // Two directories both referencing the same file digest.
        let shared_file = make_file_node("shared.txt", 0xdd, 999);

        let sub_dir = Directory {
            files: vec![shared_file.clone()],
            directories: vec![],
            ..Default::default()
        };
        let (sub_dir_bytes, sub_dir_digest) = encode_directory(&sub_dir);

        let root_dir = Directory {
            files: vec![
                // Same digest as the file in sub_dir (same hash_byte 0xdd, same size).
                make_file_node("also_shared.txt", 0xdd, 999),
            ],
            directories: vec![DirectoryNode {
                name: "subdir".to_string(),
                digest: Some(sub_dir_digest.into()),
            }],
            ..Default::default()
        };
        let (root_dir_bytes, root_dir_digest) = encode_directory(&root_dir);

        let store = Store::new(MemoryStore::new(&MemorySpec::default()));
        let root_key: StoreKey<'_> = root_dir_digest.into();
        store
            .update_oneshot(root_key, Bytes::from(root_dir_bytes))
            .await
            .expect("store root dir");
        let sub_key: StoreKey<'_> = sub_dir_digest.into();
        store
            .update_oneshot(sub_key, Bytes::from(sub_dir_bytes))
            .await
            .expect("store sub dir");

        let failed_dirs = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let result = resolve_tree_from_cas(&store, root_dir_digest, &failed_dirs)
            .await
            .expect("resolve_tree_from_cas failed");

        // The same digest should appear only once.
        assert_eq!(
            result.file_digests.len(),
            1,
            "Duplicate file digest should be deduplicated"
        );
        assert_eq!(result.file_digests[0].1, 999);
        assert_eq!(result.dir_digests.len(), 2, "Expected root + subdir");
        assert!(result.dir_digests.contains(&root_dir_digest));
        assert!(result.dir_digests.contains(&sub_dir_digest));

        // Both dirs have the same file (999 bytes) — subtree_bytes counts
        // each occurrence (not deduplicated, since it's per-directory).
        assert_eq!(result.subtree_bytes.get(&sub_dir_digest), Some(&999));
        assert_eq!(result.subtree_bytes.get(&root_dir_digest), Some(&1998)); // 999 + 999
    }

    #[tokio::test]
    async fn test_resolve_tree_circular_directory() {
        // A true hash cycle (A->B->A) is impossible with content-addressed
        // hashes: the digest of A depends on B's digest and vice versa.
        // Instead, we test the seen_dirs guard with a diamond structure:
        //   root -> {dir_left, dir_right}, both -> dir_shared
        // Without the seen_dirs set, dir_shared would be visited twice.
        let dir_shared = Directory {
            files: vec![make_file_node("shared.txt", 0x11, 100)],
            directories: vec![],
            ..Default::default()
        };
        let (shared_bytes, shared_digest) = encode_directory(&dir_shared);

        let dir_left = Directory {
            files: vec![make_file_node("left.txt", 0x22, 200)],
            directories: vec![DirectoryNode {
                name: "shared".to_string(),
                digest: Some(shared_digest.into()),
            }],
            ..Default::default()
        };
        let (left_bytes, left_digest) = encode_directory(&dir_left);

        let dir_right = Directory {
            files: vec![make_file_node("right.txt", 0x33, 300)],
            directories: vec![DirectoryNode {
                name: "shared".to_string(),
                digest: Some(shared_digest.into()),
            }],
            ..Default::default()
        };
        let (right_bytes, right_digest) = encode_directory(&dir_right);

        let root = Directory {
            files: vec![],
            directories: vec![
                DirectoryNode {
                    name: "left".to_string(),
                    digest: Some(left_digest.into()),
                },
                DirectoryNode {
                    name: "right".to_string(),
                    digest: Some(right_digest.into()),
                },
            ],
            ..Default::default()
        };
        let (root_bytes, root_digest) = encode_directory(&root);

        let store = Store::new(MemoryStore::new(&MemorySpec::default()));
        for (bytes, digest) in [
            (root_bytes, root_digest),
            (left_bytes, left_digest),
            (right_bytes, right_digest),
            (shared_bytes, shared_digest),
        ] {
            let key: StoreKey<'_> = digest.into();
            store
                .update_oneshot(key, Bytes::from(bytes))
                .await
                .expect("store update");
        }

        let failed_dirs = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let result = resolve_tree_from_cas(&store, root_digest, &failed_dirs)
            .await
            .expect("resolve_tree_from_cas failed");

        // dir_shared is referenced by both dir_left and dir_right, but
        // seen_dirs ensures it's only visited once. Files: shared(0x11),
        // left(0x22), right(0x33) — all unique digests, so 3 total.
        assert_eq!(
            result.file_digests.len(),
            3,
            "Diamond structure: shared dir visited once, 3 unique files"
        );
        // 4 directories: root, left, right, shared
        assert_eq!(result.dir_digests.len(), 4, "Expected 4 directory digests");
        assert!(result.dir_digests.contains(&root_digest));
        assert!(result.dir_digests.contains(&left_digest));
        assert!(result.dir_digests.contains(&right_digest));
        assert!(result.dir_digests.contains(&shared_digest));

        // shared: 100 bytes (its own file)
        assert_eq!(result.subtree_bytes.get(&shared_digest), Some(&100));
        // left: 200 (own) + 100 (shared) = 300
        assert_eq!(result.subtree_bytes.get(&left_digest), Some(&300));
        // right: 300 (own) + 100 (shared) = 400
        assert_eq!(result.subtree_bytes.get(&right_digest), Some(&400));
        // root: 0 (no own files) + 300 (left) + 400 (right) = 700
        assert_eq!(result.subtree_bytes.get(&root_digest), Some(&700));

        let mut sizes: Vec<u64> = result.file_digests.iter().map(|&(_, s)| s).collect();
        sizes.sort();
        assert_eq!(sizes, vec![100, 200, 300]);
    }

    #[tokio::test]
    async fn test_resolve_tree_missing_directory() {
        // Attempt to resolve a digest that doesn't exist in the store.
        let store = Store::new(MemoryStore::new(&MemorySpec::default()));

        let missing_digest = DigestInfo::new([0xff; 32], 42);
        let failed_dirs = Arc::new(tokio::sync::Mutex::new(HashMap::new()));
        let result = resolve_tree_from_cas(&store, missing_digest, &failed_dirs).await;

        assert!(
            result.is_err(),
            "Should return an error for a missing directory"
        );

        // The failing digest should be recorded in the subdirectory negative cache.
        let cache = failed_dirs.lock().await;
        assert!(
            cache.contains_key(&missing_digest),
            "Missing digest should be in failed_directory_digests cache"
        );
    }

    #[test]
    fn test_score_workers_empty_file_list() {
        let locality_map = new_shared_blob_locality_map();

        // Even with data in the locality map, empty file_digests => empty scores.
        {
            let mut map = locality_map.write();
            let d1 = DigestInfo::new([1u8; 32], 1000);
            map.register_blobs("grpc://worker-a:50081", &[d1]);
        }

        let worker_a = WorkerId::from("worker-a-id".to_string());
        let mut endpoint_to_worker = HashMap::new();
        endpoint_to_worker.insert(Arc::from("grpc://worker-a:50081"), worker_a.clone());

        let mut candidates = HashSet::new();
        candidates.insert(worker_a);

        let file_digests: Vec<(DigestInfo, u64)> = vec![];

        let scores = score_workers(&candidates, &file_digests, &locality_map, &endpoint_to_worker);
        assert!(
            scores.is_empty(),
            "Expected empty scores for empty file_digests, got {scores:?}"
        );
    }

    #[tokio::test]
    async fn test_resolve_input_tree_cache_hit_returns_same_arc() {
        use nativelink_config::schedulers::WorkerAllocationStrategy;
        use nativelink_metric::MetricsComponent;
        use nativelink_util::operation_state_manager::{UpdateOperationType, WorkerStateManager};
        use crate::platform_property_manager::PlatformPropertyManager;
        use crate::worker_registry::WorkerRegistry;

        // Minimal mock WorkerStateManager for constructing ApiWorkerScheduler.
        #[derive(Debug)]
        struct NoopWorkerStateManager;

        impl MetricsComponent for NoopWorkerStateManager {
            fn publish(
                &self,
                _kind: MetricKind,
                _field_metadata: MetricFieldData,
            ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
                Ok(MetricPublishKnownKindData::Component)
            }
        }

        #[tonic::async_trait]
        impl WorkerStateManager for NoopWorkerStateManager {
            async fn update_operation(
                &self,
                _operation_id: &OperationId,
                _worker_id: &WorkerId,
                _update: UpdateOperationType,
            ) -> Result<(), Error> {
                Ok(())
            }
        }

        // Create a store with a single-directory tree (one file).
        let store = Store::new(MemoryStore::new(&MemorySpec::default()));

        let dir = Directory {
            files: vec![make_file_node("test.txt", 0xaa, 1000)],
            directories: vec![],
            ..Default::default()
        };
        let (dir_bytes, dir_digest) = encode_directory(&dir);
        let key: StoreKey<'_> = dir_digest.into();
        store
            .update_oneshot(key, Bytes::from(dir_bytes))
            .await
            .expect("store update");

        // Build scheduler with CAS store.
        let scheduler = ApiWorkerScheduler::new_with_locality_map(
            Arc::new(NoopWorkerStateManager),
            Arc::new(PlatformPropertyManager::new(HashMap::new())),
            WorkerAllocationStrategy::default(),
            Arc::new(Notify::new()),
            100,
            Arc::new(WorkerRegistry::new()),
            None,
            Some(store),
        );

        // First call: cache miss, returns None and spawns background resolution.
        let result1 = scheduler.resolve_input_tree(dir_digest).await;
        assert!(result1.is_none(), "Expected None from first resolve (lazy cache miss)");

        // Wait for the background resolution task to populate the cache.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Second call: cache hit from background resolution.
        let result2 = scheduler.resolve_input_tree(dir_digest).await;
        assert!(result2.is_some(), "Expected Some from second resolve (cache hit)");

        // Third call: should return the same Arc (pointer equality).
        let result3 = scheduler.resolve_input_tree(dir_digest).await;
        assert!(result3.is_some(), "Expected Some from third resolve (cache hit)");

        let arc2 = result2.unwrap();
        let arc3 = result3.unwrap();
        assert!(
            Arc::ptr_eq(&arc2, &arc3),
            "Expected resolve_input_tree to return the same Arc on cache hit (pointer equality)"
        );
    }
}
