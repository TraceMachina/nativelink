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
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_lock::RwLock;
use lru::LruCache;
use nativelink_config::schedulers::WorkerAllocationStrategy;
use nativelink_error::{Code, Error, ResultExt, error_if, make_err, make_input_err};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
    RootMetricsComponent, group,
};
use nativelink_proto::build::bazel::remote::execution::v2::Directory;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    PeerHint, StartExecute, UpdateForWorker, update_for_worker,
};
use nativelink_util::blob_locality_map::SharedBlobLocalityMap;
use nativelink_util::action_messages::{OperationId, WorkerId};
use nativelink_util::common::DigestInfo;
use nativelink_util::operation_state_manager::{UpdateOperationType, WorkerStateManager};
use nativelink_util::platform_properties::PlatformProperties;
use nativelink_util::shutdown_guard::ShutdownGuard;
use nativelink_util::store_trait::{Store, StoreKey, StoreLike};
use prost::Message;
use tokio::sync::Notify;
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
}

use crate::platform_property_manager::PlatformPropertyManager;
use crate::worker::{
    ActionInfoWithProps, PendingActionInfoData, Worker, WorkerTimestamp, WorkerUpdate,
    reduce_platform_properties,
};
use crate::worker_capability_index::WorkerCapabilityIndex;
use crate::worker_registry::SharedWorkerRegistry;
use crate::worker_scheduler::WorkerScheduler;

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
    endpoint_to_worker: HashMap<String, WorkerId>,
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
                .insert(worker.cas_endpoint.clone(), worker_id.clone());
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
                self.endpoint_to_worker.remove(&worker.cas_endpoint);
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

        // Collect viable candidates with their load info for load-aware selection.
        let viable: Vec<_> = match self.allocation_strategy {
            WorkerAllocationStrategy::LeastRecentlyUsed => workers_iter
                .rev()
                .filter(|(worker_id, _)| candidates.contains(worker_id))
                .filter(|pair| worker_matches(pair))
                .map(|(_, w)| (w.id.clone(), w.cpu_load_pct))
                .collect(),
            WorkerAllocationStrategy::MostRecentlyUsed => workers_iter
                .filter(|(worker_id, _)| candidates.contains(worker_id))
                .filter(|pair| worker_matches(pair))
                .map(|(_, w)| (w.id.clone(), w.cpu_load_pct))
                .collect(),
        };

        // Pick the lightest-loaded worker among viable candidates.
        // Workers with cpu_load_pct == 0 (unknown) are sorted last among
        // workers that have reported load. Falls back to LRU/MRU order
        // (first in the vec) when no workers have reported load.
        let worker_id = if viable.iter().any(|(_, load)| *load > 0) {
            // At least one worker has reported load — pick lightest.
            viable
                .iter()
                .min_by_key(|(_, load)| if *load == 0 { u32::MAX } else { *load })
                .map(|(id, _)| id.clone())
        } else {
            // No load data — use first viable (LRU/MRU order).
            viable.first().map(|(id, _)| id.clone())
        };

        // Log load-aware selection decision.
        if let Some(ref wid) = worker_id {
            let viable_loads: Vec<_> = viable
                .iter()
                .map(|(id, load)| {
                    let short_id = id.0.chars().take(12).collect::<String>();
                    (short_id, *load)
                })
                .collect();
            let winner_load = viable
                .iter()
                .find(|(id, _)| id == wid)
                .map(|(_, l)| *l)
                .unwrap_or(0);
            debug!(
                candidates = viable.len(),
                worker_id = %wid,
                winner_load_pct = winner_load,
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
    /// map.
    fn inner_find_and_reserve_worker(
        &mut self,
        platform_properties: &PlatformProperties,
        operation_id: &OperationId,
        action_info: &ActionInfoWithProps,
        full_worker_logging: bool,
        endpoint_scores: Option<&HashMap<String, (u64, SystemTime)>>,
        peer_hints: Vec<PeerHint>,
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
                // Look up cpu_load_pct for tiebreaking within 10% score range.
                let load_for_worker = |wid: &WorkerId| -> u32 {
                    self.workers.0.peek(wid)
                        .map(|w| w.cpu_load_pct)
                        .unwrap_or(0)
                };
                sorted.sort_by(|a, b| {
                    let (score_a, ts_a) = a.1;
                    let (score_b, ts_b) = b.1;
                    let max_score = score_a.max(score_b);
                    // Within 10% of each other? Use CPU load, then timestamp.
                    let threshold = max_score / 10; // 10% of the larger score
                    if score_a.abs_diff(score_b) <= threshold {
                        // Scores are similar — prefer lower CPU load.
                        let load_a = load_for_worker(&a.0);
                        let load_b = load_for_worker(&b.0);
                        if load_a != load_b && (load_a > 0 || load_b > 0) {
                            // Sort unknown (0) after known loads.
                            let effective_a = if load_a == 0 { u32::MAX } else { load_a };
                            let effective_b = if load_b == 0 { u32::MAX } else { load_b };
                            effective_a.cmp(&effective_b)
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

        let worker_id = if let Some(wid) = locality_winner {
            // Promote in LRU.
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
            UpdateOperationType::ExecutionComplete => {
                // No update here, just restoring platform properties.
                worker.execution_complete(operation_id);
                self.worker_change_notify.notify_one();
                return Ok(());
            }
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
    /// input tree. When no resolved tree is available the hints will be empty
    /// -- the old fallback that generated a single hint for `input_root_digest`
    /// never worked because workers register individual file digests, not
    /// directory digests.
    ///
    /// Returns `None` if the worker was not found.
    fn prepare_worker_run_action(
        &mut self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        action_info: &ActionInfoWithProps,
        peer_hints: Vec<PeerHint>,
    ) -> Option<(UnboundedSender<UpdateForWorker>, UpdateForWorker)> {
        let worker = self.workers.get_mut(worker_id)?;
        // Clone the tx so we can send outside the lock.
        let tx = worker.tx.clone();

        if !peer_hints.is_empty() {
            info!(
                ?worker_id,
                hints = peer_hints.len(),
                "Generated peer hints for StartExecute"
            );
        }

        // Build the protobuf message while we still have access to worker state.
        let start_execute = StartExecute {
            execute_request: Some(action_info.inner.as_ref().into()),
            operation_id: operation_id.to_string(),
            queued_timestamp: Some(action_info.inner.insert_timestamp.into()),
            platform: Some((&action_info.platform_properties).into()),
            worker_id: worker.id.clone().into(),
            peer_hints,
        };
        let msg = UpdateForWorker {
            update: Some(update_for_worker::Update::StartAction(start_execute)),
        };

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

    /// Cached resolved input trees: input_root_digest → (file_digest, size) pairs.
    /// Held under a tokio::Mutex briefly for get/put, not during I/O.
    tree_cache: Arc<tokio::sync::Mutex<LruCache<DigestInfo, Arc<Vec<(DigestInfo, u64)>>>>>,
}

/// Capacity for the resolved input tree LRU cache.
const TREE_CACHE_CAPACITY: usize = 1024;

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
        })
    }

    /// Returns a reference to the worker registry.
    pub const fn worker_registry(&self) -> &SharedWorkerRegistry {
        &self.worker_registry
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
                inner.prepare_worker_run_action(&worker_id, &operation_id, &action_info, Vec::new());
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
        let (endpoint_scores, peer_hints) = match (&resolved_tree, &self.locality_map) {
            (Some(tree), Some(loc_map)) => {
                let (scores, hints) = score_and_generate_hints(tree, loc_map);
                (Some(scores), hints)
            }
            _ => (None, Vec::new()),
        };

        // ── Phase 3: acquire write lock, do selection + reservation ──
        // Inside the lock we only do O(workers) work: candidate filtering,
        // endpoint→WorkerId mapping, and state mutation.
        let mut inner = self.inner.write().await;
        let worker_count = inner.workers.len() as u64;
        let result = inner.inner_find_and_reserve_worker(
            platform_properties,
            operation_id,
            action_info,
            full_worker_logging,
            endpoint_scores.as_ref(),
            peer_hints,
        );

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

    /// Resolves the full input tree for the given `input_root_digest` by
    /// reading Directory protos from the CAS store and collecting all file
    /// digests and sizes. Results are cached in `tree_cache`.
    ///
    /// Returns `None` if no CAS store is configured or on any error (errors
    /// are logged but do not fail scheduling — we just skip locality scoring).
    ///
    /// Runs *outside* the scheduler write lock, so multiple actions can
    /// resolve concurrently. The `tokio::Mutex` on `tree_cache` is held
    /// only briefly for get/put, not during store I/O.
    async fn resolve_input_tree(
        &self,
        input_root_digest: DigestInfo,
    ) -> Option<Arc<Vec<(DigestInfo, u64)>>> {
        let cas_store = self.cas_store.as_ref()?;

        // Check cache first (brief lock).
        {
            let mut cache = self.tree_cache.lock().await;
            if let Some(cached) = cache.get(&input_root_digest) {
                info!(
                    %input_root_digest,
                    file_count = cached.len(),
                    "Tree resolution cache hit"
                );
                return Some(cached.clone());
            }
        }

        // Cache miss — resolve the tree by reading Directory protos from CAS.
        let result = resolve_tree_from_cas(cas_store, input_root_digest).await;
        match result {
            Ok(file_digests) => {
                info!(
                    %input_root_digest,
                    file_count = file_digests.len(),
                    "Resolved input tree from CAS (cache miss)"
                );
                let arc = Arc::new(file_digests);
                // Store in cache (brief lock).
                {
                    let mut cache = self.tree_cache.lock().await;
                    cache.put(input_root_digest, arc.clone());
                }
                Some(arc)
            }
            Err(err) => {
                warn!(
                    %input_root_digest,
                    ?err,
                    "Failed to resolve input tree for locality scoring, skipping"
                );
                None
            }
        }
    }
}

/// Resolves a directory tree from the CAS store by recursively reading
/// Directory protos and collecting all (file_digest, file_size) pairs.
/// Deduplicates by digest.
async fn resolve_tree_from_cas(
    cas_store: &Store,
    root_digest: DigestInfo,
) -> Result<Vec<(DigestInfo, u64)>, Error> {
    use std::collections::HashSet;
    use futures::stream::FuturesUnordered;
    use futures::StreamExt;

    let mut file_digests: Vec<(DigestInfo, u64)> = Vec::new();
    let mut seen_files: HashSet<DigestInfo> = HashSet::new();
    let mut dirs_to_visit: Vec<DigestInfo> = vec![root_digest];
    let mut seen_dirs: HashSet<DigestInfo> = HashSet::new();
    seen_dirs.insert(root_digest);

    while !dirs_to_visit.is_empty() {
        // Fetch all directories at current level in parallel.
        let fetches: FuturesUnordered<_> = dirs_to_visit
            .drain(..)
            .map(|dir_digest| {
                let cas_store = cas_store.clone();
                async move {
                    let key: StoreKey<'_> = dir_digest.into();
                    let bytes = cas_store
                        .get_part_unchunked(key, 0, None)
                        .await
                        .err_tip(|| {
                            format!(
                                "Reading directory {dir_digest} from CAS for tree resolution"
                            )
                        })?;
                    let directory = Directory::decode(bytes).map_err(|e| {
                        make_err!(Code::Internal, "Failed to decode Directory proto: {e}")
                    })?;
                    Ok::<_, Error>(directory)
                }
            })
            .collect();

        let results: Vec<Result<Directory, Error>> = fetches.collect().await;
        for result in results {
            let directory = result?;

            // Collect file digests.
            for file_node in &directory.files {
                if let Some(ref digest) = file_node.digest {
                    if let Ok(digest_info) = DigestInfo::try_from(digest) {
                        if seen_files.insert(digest_info) {
                            file_digests.push((digest_info, digest_info.size_bytes()));
                        }
                    }
                }
            }

            // Queue subdirectories for visiting (dedup via seen_dirs).
            for dir_node in &directory.directories {
                if let Some(ref digest) = dir_node.digest {
                    if let Ok(digest_info) = DigestInfo::try_from(digest) {
                        if seen_dirs.insert(digest_info) {
                            dirs_to_visit.push(digest_info);
                        }
                    }
                }
            }
        }
    }

    Ok(file_digests)
}

/// Scores endpoints by the total bytes of input blobs they have cached
/// AND generates peer hints in a single pass over the file digests,
/// acquiring the locality map read lock only once.
///
/// Returns:
/// - `HashMap<String, (u64, SystemTime)>`: endpoint scores (total cached
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
) -> (HashMap<String, (u64, SystemTime)>, Vec<PeerHint>) {
    /// Maximum number of peer hints to include in a StartExecute message
    /// to avoid oversized messages.
    const MAX_PEER_HINTS: usize = 16384;

    let map = locality_map.read();
    let blobs = map.blobs_map();
    let mut scores: HashMap<String, (u64, SystemTime)> = HashMap::new();
    let mut hint_candidates: Vec<(DigestInfo, u64, Vec<String>)> = Vec::new();

    for &(digest, size) in file_digests {
        if let Some(endpoints) = blobs.get(&digest) {
            // Accumulate endpoint scores.
            for (endpoint, ts) in endpoints {
                let entry = scores
                    .entry(endpoint.to_string())
                    .or_insert((0, UNIX_EPOCH));
                entry.0 += size;
                if *ts > entry.1 {
                    entry.1 = *ts;
                }
            }
            // Collect hint candidate if this digest has peer locations.
            if !endpoints.is_empty() {
                let peer_eps: Vec<String> =
                    endpoints.keys().map(|e| e.to_string()).collect();
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
            peer_endpoints,
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
    endpoint_scores: &HashMap<String, (u64, SystemTime)>,
    endpoint_to_worker: &HashMap<String, WorkerId>,
    candidates: &std::collections::HashSet<WorkerId>,
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
    candidates: &std::collections::HashSet<WorkerId>,
    file_digests: &[(DigestInfo, u64)],
    locality_map: &SharedBlobLocalityMap,
    endpoint_to_worker: &HashMap<String, WorkerId>,
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

        let mut inner = self.inner.write().await;
        inner
            .immediate_evict_worker(
                worker_id,
                make_err!(Code::Internal, "Received request to remove worker"),
                false,
            )
            .await
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

        result
    }

    async fn set_drain_worker(&self, worker_id: &WorkerId, is_draining: bool) -> Result<(), Error> {
        let mut inner = self.inner.write().await;
        inner.set_drain_worker(worker_id, is_draining).await
    }

    async fn update_worker_load(&self, worker_id: &WorkerId, cpu_load_pct: u32) -> Result<(), Error> {
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
        debug!(%worker_id, cpu_load_pct, "Worker load updated");
        Ok(())
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
        endpoint_to_worker.insert("grpc://worker-a:50081".to_string(), worker_a.clone());
        endpoint_to_worker.insert("grpc://worker-b:50081".to_string(), worker_b.clone());

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
        endpoint_to_worker.insert("grpc://worker-a:50081".to_string(), worker_a.clone());

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

        let result = resolve_tree_from_cas(&store, dir_digest)
            .await
            .expect("resolve_tree_from_cas failed");

        assert_eq!(result.len(), 3, "Expected 3 file digests");

        // Verify all three sizes are present (order may vary).
        let mut sizes: Vec<u64> = result.iter().map(|&(_, s)| s).collect();
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

        let result = resolve_tree_from_cas(&store, root_dir_digest)
            .await
            .expect("resolve_tree_from_cas failed");

        assert_eq!(result.len(), 3, "Expected 3 files (1 root + 2 subdir)");

        let mut sizes: Vec<u64> = result.iter().map(|&(_, s)| s).collect();
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

        let result = resolve_tree_from_cas(&store, root_dir_digest)
            .await
            .expect("resolve_tree_from_cas failed");

        // The same digest should appear only once.
        assert_eq!(
            result.len(),
            1,
            "Duplicate file digest should be deduplicated"
        );
        assert_eq!(result[0].1, 999);
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

        let result = resolve_tree_from_cas(&store, root_digest)
            .await
            .expect("resolve_tree_from_cas failed");

        // dir_shared is referenced by both dir_left and dir_right, but
        // seen_dirs ensures it's only visited once. Files: shared(0x11),
        // left(0x22), right(0x33) — all unique digests, so 3 total.
        assert_eq!(
            result.len(),
            3,
            "Diamond structure: shared dir visited once, 3 unique files"
        );

        let mut sizes: Vec<u64> = result.iter().map(|&(_, s)| s).collect();
        sizes.sort();
        assert_eq!(sizes, vec![100, 200, 300]);
    }

    #[tokio::test]
    async fn test_resolve_tree_missing_directory() {
        // Attempt to resolve a digest that doesn't exist in the store.
        let store = Store::new(MemoryStore::new(&MemorySpec::default()));

        let missing_digest = DigestInfo::new([0xff; 32], 42);
        let result = resolve_tree_from_cas(&store, missing_digest).await;

        assert!(
            result.is_err(),
            "Should return an error for a missing directory"
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
        endpoint_to_worker.insert("grpc://worker-a:50081".to_string(), worker_a.clone());

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

        // First call: cache miss, resolves from CAS.
        let result1 = scheduler.resolve_input_tree(dir_digest).await;
        assert!(result1.is_some(), "Expected Some from first resolve");

        // Second call: cache hit, should return the same Arc.
        let result2 = scheduler.resolve_input_tree(dir_digest).await;
        assert!(result2.is_some(), "Expected Some from second resolve");

        let arc1 = result1.unwrap();
        let arc2 = result2.unwrap();
        assert!(
            Arc::ptr_eq(&arc1, &arc2),
            "Expected resolve_input_tree to return the same Arc on cache hit (pointer equality)"
        );
    }
}
