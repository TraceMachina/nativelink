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

use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::sync::Arc;
use std::time::{Instant, UNIX_EPOCH};

use async_lock::Mutex;
use futures::{StreamExt, future};
use lru::LruCache;
use nativelink_config::schedulers::WorkerAllocationStrategy;
use nativelink_error::{Code, Error, ResultExt, error_if, make_err, make_input_err};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
    RootMetricsComponent, group,
};
use nativelink_proto::com::github::trace_machina::nativelink::events::{
    Event, OriginEvent, ResponseEvent, event, response_event,
};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::ActionResourceUsage;
use nativelink_util::action_messages::{OperationId, WorkerId};
use nativelink_util::metrics::{
    WorkerDisconnectReason, record_execution_cpu_time, record_execution_peak_memory,
    record_worker_connected, record_worker_disconnected, record_worker_keepalive,
    record_worker_state,
};
use nativelink_util::operation_state_manager::{UpdateOperationType, WorkerStateManager};
use nativelink_util::origin_event::get_node_id;
use nativelink_util::platform_properties::PlatformProperties;
use nativelink_util::shutdown_guard::ShutdownGuard;
use tokio::sync::{Notify, mpsc};
use tonic::async_trait;
use tracing::{debug, error, info, trace, warn};

/// How many state-manager lookups `kill_revoked_operations` has in flight
/// at once while checking which running operations were revoked.
const MAX_CONCURRENT_REVOKED_CHECKS: usize = 32;
use uuid::Uuid;

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
use crate::worker::{ActionInfoWithProps, Worker, WorkerTimestamp, WorkerUpdate};
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

        trace!(
            ?worker_id,
            running_operations = worker.running_action_infos.len(),
            "Worker keepalive received"
        );
        record_worker_keepalive();

        Ok(())
    }

    /// Adds a worker to the pool.
    /// Note: This function will not do any task matching.
    fn add_worker(&mut self, worker: Worker) -> Result<(), Error> {
        let worker_id = worker.id.clone();
        let platform_properties = worker.platform_properties.clone();
        // A replacement is one out and one in, so the gauge only moves for a
        // genuinely new worker.
        let replaced = self.workers.put(worker_id.clone(), worker);

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
        if let Some(replaced) = &replaced {
            if replaced.is_draining {
                record_worker_state("draining", false);
            }
            if replaced.is_paused {
                record_worker_state("paused", false);
            }
        } else {
            record_worker_connected();
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
        if worker.is_draining != is_draining {
            record_worker_state("draining", is_draining);
        }
        worker.is_draining = is_draining;
        self.worker_change_notify.notify_one();
        Ok(())
    }

    fn inner_find_worker_for_action(
        &self,
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
                info!("No workers in capability index match required properties");
            }
            return None;
        }

        // Check function for availability AND dynamic Minimum property verification.
        // The index only does presence checks for Minimum properties since their
        // values change dynamically as jobs are assigned to workers.
        let worker_matches = |(worker_id, w): &(&WorkerId, &Worker)| -> bool {
            if !w.can_accept_work() {
                if full_worker_logging {
                    info!(
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
        let workers_iter = self.workers.iter();

        let worker_id = match self.allocation_strategy {
            // Use rfind to get the least recently used that satisfies the properties.
            WorkerAllocationStrategy::LeastRecentlyUsed => workers_iter
                .rev()
                .filter(|(worker_id, _)| candidates.contains(worker_id))
                .find(&worker_matches)
                .map(|(_, w)| w.id.clone()),

            // Use find to get the most recently used that satisfies the properties.
            WorkerAllocationStrategy::MostRecentlyUsed => workers_iter
                .filter(|(worker_id, _)| candidates.contains(worker_id))
                .find(&worker_matches)
                .map(|(_, w)| w.id.clone()),
        };
        if full_worker_logging && worker_id.is_none() {
            warn!("No workers matched!");
        }
        worker_id
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
        let update_operation_res = if worker.is_kill_requested(operation_id) {
            debug!(
                %operation_id,
                ?worker_id,
                "Ignoring update for operation the worker was told to kill"
            );
            Ok(())
        } else {
            let update_operation_res = self
                .worker_state_manager
                .update_operation(operation_id, worker_id, update)
                .await
                .err_tip(|| "in update_operation on SimpleScheduler::update_action");
            if let Err(err) = &update_operation_res {
                error!(
                    %operation_id,
                    ?worker_id,
                    ?err,
                    "Failed to update_operation on update_action"
                );
            }
            update_operation_res
        };

        if !is_finished {
            return update_operation_res;
        }
        // The worker is done with this action even if the state-manager update
        // failed (e.g. the operation was already torn down after its clients
        // timed out). The worker bookkeeping below must still run, or the
        // worker's platform properties leak until it can never match again.

        // Clear this action from the current worker if finished.
        let complete_action_res = {
            // Note: We need to run this before dealing with backpressure logic.
            let was_paused = worker.is_paused;
            let complete_action_res = worker.complete_action(operation_id).await;

            if (due_to_backpressure || !worker.can_accept_work()) && worker.has_actions() {
                worker.is_paused = true;
            }
            // complete_action clears is_paused on its way through, so compare
            // the state either side of it rather than testing the flag after.
            // Testing afterwards counts a re-pause every time and never
            // unwinds, which leaves the gauge climbing forever.
            if was_paused != worker.is_paused {
                record_worker_state("paused", worker.is_paused);
            }
            complete_action_res
        };

        self.worker_change_notify.notify_one();

        update_operation_res.merge(complete_action_res)
    }

    /// Notifies the specified worker to run the given action and handles errors by evicting
    /// the worker if the notification fails.
    async fn worker_notify_run_action(
        &mut self,
        worker_id: WorkerId,
        operation_id: OperationId,
        action_info: ActionInfoWithProps,
    ) -> Result<(), Error> {
        if let Some(worker) = self.workers.get_mut(&worker_id) {
            let notify_worker_result = worker
                .notify_update(WorkerUpdate::RunAction(Box::new((
                    operation_id,
                    action_info.clone(),
                ))))
                .await;

            if let Err(notify_worker_result) = notify_worker_result {
                warn!(
                    ?worker_id,
                    ?action_info,
                    ?notify_worker_result,
                    "Worker command failed, removing worker",
                );

                // A slightly nasty way of figuring out that the worker disconnected
                // from send_msg_to_worker without introducing complexity to the
                // code path from here to there.
                let is_disconnect = notify_worker_result.code == Code::Internal
                    && notify_worker_result.messages.len() == 1
                    && notify_worker_result.messages[0] == "Worker Disconnected";

                let err = make_err!(
                    Code::Internal,
                    "Worker command failed, removing worker {worker_id} -- {notify_worker_result:?}",
                );

                return Result::<(), _>::Err(err.clone()).merge(
                    self.immediate_evict_worker(&worker_id, err, is_disconnect)
                        .await,
                );
            }
            Ok(())
        } else {
            warn!(
                ?worker_id,
                %operation_id,
                ?action_info,
                "Worker not found in worker map in worker_notify_run_action"
            );
            // Ensure the operation is put back to queued state.
            self.worker_state_manager
                .update_operation(
                    &operation_id,
                    &worker_id,
                    UpdateOperationType::UpdateWithDisconnect,
                )
                .await
        }
    }

    /// Tells the worker to kill an operation it is still running but the
    /// state manager no longer has executing on it. A worker that cannot be
    /// reached is evicted, the same as for a failed run request.
    async fn worker_notify_kill_operation(
        &mut self,
        worker_id: &WorkerId,
        operation_id: OperationId,
    ) -> Result<(), Error> {
        let Some(worker) = self.workers.get_mut(worker_id) else {
            // Gone between the snapshot and now; its actions were requeued.
            return Ok(());
        };
        // Already told, or finished in the meantime; nothing more to send.
        if !worker.running_action_infos.contains_key(&operation_id)
            || worker.is_kill_requested(&operation_id)
        {
            return Ok(());
        }
        info!(
            ?worker_id,
            %operation_id,
            "Killing operation the state manager no longer has executing on this worker"
        );
        if let Err(err) = worker
            .notify_update(WorkerUpdate::KillOperation(operation_id.clone()))
            .await
        {
            warn!(
                ?worker_id,
                %operation_id,
                ?err,
                "Worker command failed, removing worker"
            );
            let err = make_err!(
                Code::Internal,
                "Worker command failed, removing worker {worker_id} -- {err:?}",
            );
            return Result::<(), _>::Err(err.clone())
                .merge(self.immediate_evict_worker(worker_id, err, true).await);
        }
        Ok(())
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
            // Log every eviction here rather than in each caller, so a worker
            // can never leave the pool unexplained. Without this, a worker that
            // vanished mid-build left nothing on the scheduler side to
            // attribute it to, and the only visible symptom was the worker
            // reconnecting with a fresh id.
            info!(
                ?worker_id,
                is_disconnect,
                running_actions = worker.running_action_infos.len(),
                reason = %err.message_string(),
                "Evicting worker from pool"
            );
            record_worker_disconnected(
                if is_disconnect {
                    WorkerDisconnectReason::Disconnected
                } else {
                    WorkerDisconnectReason::Evicted
                },
                worker.is_draining,
                worker.is_paused,
            );
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
    inner: Mutex<ApiWorkerSchedulerImpl>,
    #[metric(group = "platform_property_manager")]
    platform_property_manager: Arc<PlatformPropertyManager>,

    #[metric(
        help = "Timeout of how long to evict workers if no response in this given amount of time in seconds."
    )]
    worker_timeout_s: u64,
    #[metric(
        help = "How long a sent kill may go unacknowledged before the worker is evicted, in seconds."
    )]
    unacknowledged_kill_timeout_s: u64,
    /// Shared worker registry for checking worker liveness.
    worker_registry: SharedWorkerRegistry,

    /// Performance metrics for observability.
    metrics: Arc<SchedulerMetrics>,

    /// Channel for publishing origin events such as worker-observed action
    /// resource usage. `None` when origin events are disabled.
    maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,
}

impl ApiWorkerScheduler {
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        worker_state_manager: Arc<dyn WorkerStateManager>,
        platform_property_manager: Arc<PlatformPropertyManager>,
        allocation_strategy: WorkerAllocationStrategy,
        worker_change_notify: Arc<Notify>,
        worker_timeout_s: u64,
        unacknowledged_kill_timeout_s: u64,
        worker_registry: SharedWorkerRegistry,
        maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(ApiWorkerSchedulerImpl {
                workers: Workers(LruCache::unbounded()),
                worker_state_manager,
                allocation_strategy,
                worker_change_notify,
                worker_registry: worker_registry.clone(),
                shutting_down: false,
                capability_index: WorkerCapabilityIndex::new(),
            }),
            platform_property_manager,
            worker_timeout_s,
            unacknowledged_kill_timeout_s,
            worker_registry,
            metrics: Arc::new(SchedulerMetrics::default()),
            maybe_origin_event_tx,
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
        let mut inner = self.inner.lock().await;
        inner
            .worker_notify_run_action(worker_id, operation_id, action_info)
            .await
    }

    pub async fn running_action_info(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
    ) -> Option<ActionInfoWithProps> {
        let inner = self.inner.lock().await;
        inner
            .workers
            .peek(worker_id)
            .and_then(|worker| worker.running_action_infos.get(operation_id))
            .map(|pending_action_info| pending_action_info.action_info.clone())
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

        let inner = self.inner.lock().await;
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

    /// Checks to see if the worker exists in the worker pool. Should only be used in unit tests.
    #[must_use]
    pub async fn contains_worker_for_test(&self, worker_id: &WorkerId) -> bool {
        let inner = self.inner.lock().await;
        inner.workers.contains(worker_id)
    }

    /// A unit test function used to send the keep alive message to the worker from the server.
    pub async fn send_keep_alive_to_worker_for_test(
        &self,
        worker_id: &WorkerId,
    ) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        let worker = inner.workers.get_mut(worker_id).ok_or_else(|| {
            make_input_err!("WorkerId '{}' does not exist in workers map", worker_id)
        })?;
        worker.keep_alive()
    }
}

#[async_trait]
impl WorkerScheduler for ApiWorkerScheduler {
    fn get_platform_property_manager(&self) -> &PlatformPropertyManager {
        self.platform_property_manager.as_ref()
    }

    async fn record_action_resource_usage(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        mut resource_usage: ActionResourceUsage,
    ) -> Result<(), Error> {
        // The worker API talks to this `ApiWorkerScheduler` (it is the
        // `WorkerScheduler` returned by `SimpleScheduler::new`), so the
        // resource-usage origin event must be published here. Previously the
        // only override lived on `SimpleScheduler`, which this path never
        // reaches, so the event was silently dropped by the trait's no-op
        // default and `observed_worker_peak_memory_mib` was never recorded.
        // Sampling is optional, so only record when the worker actually took a
        // reading. A zero here means "not sampled", not "used no memory".
        let maybe_action_info = self.running_action_info(worker_id, operation_id).await;
        let action_mnemonic = maybe_action_info
            .as_ref()
            .and_then(|action_info| action_info.origin_metadata.bazel_metadata.as_ref())
            .map_or_else(String::new, |bazel_metadata| {
                bazel_metadata.action_mnemonic.clone()
            });

        if resource_usage.sampled {
            if resource_usage.peak_memory_kb > 0 {
                record_execution_peak_memory(resource_usage.peak_memory_kb, "", &action_mnemonic);
            }
            if resource_usage.cpu_time_ms > 0 {
                record_execution_cpu_time(resource_usage.cpu_time_ms, "", &action_mnemonic);
            }
        }

        let Some(origin_event_tx) = self.maybe_origin_event_tx.as_ref() else {
            return Ok(());
        };
        let Some(action_info) = maybe_action_info else {
            return Ok(());
        };

        if resource_usage.operation_id.is_empty() {
            resource_usage.operation_id = operation_id.to_string();
        }
        if resource_usage.worker_id.is_empty() {
            resource_usage.worker_id = worker_id.to_string();
        }

        let event = Event {
            event: Some(event::Event::Response(ResponseEvent {
                event: Some(response_event::Event::ActionResourceUsage(resource_usage)),
            })),
        };
        let origin_event = OriginEvent {
            version: 0,
            event_id: Uuid::now_v6(&get_node_id(Some(&event)))
                .hyphenated()
                .to_string(),
            parent_event_id: action_info
                .scheduler_start_execute_event_id
                .clone()
                .unwrap_or_default(),
            bazel_request_metadata: action_info.origin_metadata.bazel_metadata.clone(),
            identity: action_info.origin_metadata.identity,
            event: Some(event),
        };
        // Awaited send (not try_send): apply backpressure when the publisher
        // queue is full instead of silently dropping the resource-usage event,
        // which is what drives action-level resource sizing in the UI.
        if let Err(err) = origin_event_tx.send(origin_event).await {
            warn!(?err, "Failed to publish action resource usage origin event");
        }
        Ok(())
    }

    async fn add_worker(&self, worker: Worker) -> Result<(), Error> {
        let worker_id = worker.id.clone();
        let worker_timestamp = worker.last_update_timestamp;
        let mut inner = self.inner.lock().await;
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
        let mut inner = self.inner.lock().await;
        inner.update_action(worker_id, operation_id, update).await
    }

    async fn worker_keep_alive_received(
        &self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
    ) -> Result<(), Error> {
        {
            let mut inner = self.inner.lock().await;
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

        let mut inner = self.inner.lock().await;
        inner
            .immediate_evict_worker(
                worker_id,
                make_err!(Code::Internal, "Received request to remove worker"),
                false,
            )
            .await
    }

    async fn shutdown(&self, shutdown_guard: ShutdownGuard) {
        let mut inner = self.inner.lock().await;
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
        let timeout = Duration::from_secs(self.worker_timeout_s);
        let now = UNIX_EPOCH + Duration::from_secs(now_timestamp);
        let timeout_threshold = now_timestamp.saturating_sub(self.worker_timeout_s);

        let workers_to_check: Vec<(WorkerId, bool, bool)> = {
            let inner = self.inner.lock().await;
            inner
                .workers
                .iter()
                .map(|(worker_id, worker)| {
                    let local_alive = worker.last_update_timestamp > timeout_threshold;
                    let kill_overdue = worker.running_action_infos.values().any(|info| {
                        info.kill_requested_at.is_some_and(|at| {
                            now_timestamp.saturating_sub(at) > self.unacknowledged_kill_timeout_s
                        })
                    });
                    (worker_id.clone(), local_alive, kill_overdue)
                })
                .collect()
        };

        let mut worker_ids_to_remove = Vec::new();
        for (worker_id, local_alive, kill_overdue) in workers_to_check {
            // A healthy worker acknowledges a kill in moments; one that
            // cannot is wedged, and its keepalives keep the liveness checks
            // below from ever firing while the dead operation holds its
            // slot (the nativelink#2672 symptom). Evicting requeues its
            // other operations.
            if kill_overdue {
                warn!(
                    ?worker_id,
                    unacknowledged_kill_timeout_s = self.unacknowledged_kill_timeout_s,
                    "Worker did not acknowledge a kill in time, removing from pool"
                );
                worker_ids_to_remove.push((worker_id, true));
                continue;
            }

            if local_alive {
                continue;
            }

            let registry_alive = self
                .worker_registry
                .is_worker_alive(&worker_id, timeout, now)
                .await;

            if !registry_alive {
                trace!(
                    ?worker_id,
                    local_alive,
                    registry_alive,
                    timeout_threshold,
                    "Worker timed out - neither local nor registry shows alive"
                );
                worker_ids_to_remove.push((worker_id, false));
            }
        }

        if worker_ids_to_remove.is_empty() {
            return Ok(());
        }

        let mut inner = self.inner.lock().await;
        let mut result = Ok(());

        for (worker_id, kill_overdue) in &worker_ids_to_remove {
            let err = if *kill_overdue {
                make_err!(
                    Code::Internal,
                    "Worker {worker_id} did not acknowledge a kill within {}s, removing from pool",
                    self.unacknowledged_kill_timeout_s
                )
            } else {
                warn!(?worker_id, "Worker timed out, removing from pool");
                make_err!(
                    Code::Internal,
                    "Worker {worker_id} timed out, removing from pool"
                )
            };
            result = result.merge(inner.immediate_evict_worker(worker_id, err, false).await);
        }

        result
    }

    async fn set_drain_worker(&self, worker_id: &WorkerId, is_draining: bool) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        inner.set_drain_worker(worker_id, is_draining).await
    }

    async fn kill_revoked_operations(&self) -> Result<(), Error> {
        let (worker_state_manager, running) = {
            let inner = self.inner.lock().await;
            let running: Vec<(WorkerId, OperationId)> = inner
                .workers
                .iter()
                .flat_map(|(worker_id, worker)| {
                    worker
                        .running_action_infos
                        .iter()
                        // An operation already told to die is never swept
                        // again; the kill itself is not re-sent. A worker
                        // that received the kill but never reports back is
                        // evicted by remove_timedout_workers once the kill
                        // has gone unacknowledged longer than
                        // `unacknowledged_kill_timeout_s`; a dead worker by
                        // the ordinary keepalive timeout.
                        .filter(|(_, pending_action_info)| {
                            pending_action_info.kill_requested_at.is_none()
                        })
                        .map(|(operation_id, _)| (worker_id.clone(), operation_id.clone()))
                })
                .collect();
            (inner.worker_state_manager.clone(), running)
        };

        // On store-backed deployments each check is a network round-trip,
        // so run them lock-free with bounded concurrency.
        let revoked: Vec<(WorkerId, OperationId)> = futures::stream::iter(running)
            .map(|(worker_id, operation_id)| {
                let worker_state_manager = worker_state_manager.clone();
                async move {
                    match worker_state_manager
                        .is_executing_on_worker(&operation_id, &worker_id)
                        .await
                    {
                        Ok(true) => None,
                        Ok(false) => Some((worker_id, operation_id)),
                        // Only kill on positive evidence; try again next pass.
                        Err(err) => {
                            warn!(
                                ?worker_id,
                                %operation_id,
                                ?err,
                                "Could not check whether operation is still executing on worker"
                            );
                            None
                        }
                    }
                }
            })
            .buffer_unordered(MAX_CONCURRENT_REVOKED_CHECKS)
            .filter_map(future::ready)
            .collect()
            .await;

        if revoked.is_empty() {
            return Ok(());
        }

        // Re-check lock-free so the scheduler mutex is never held across
        // store I/O; the checks above may be stale by the time we get here.
        // The remaining TOCTOU window is fine: worker_notify_kill_operation
        // re-guards with contains_key + is_kill_requested under the lock.
        let mut confirmed = Vec::new();
        for (worker_id, operation_id) in revoked {
            // A result of Ok(true) or Err(_) means the operation was
            // reassigned to this worker after the first check, or the state
            // manager went quiet: nothing to kill on this pass.
            let revoked = worker_state_manager
                .is_executing_on_worker(&operation_id, &worker_id)
                .await
                .is_ok_and(|executing| !executing);
            if revoked {
                confirmed.push((worker_id, operation_id));
            }
        }

        if confirmed.is_empty() {
            return Ok(());
        }

        let mut inner = self.inner.lock().await;
        let mut result = Ok(());
        for (worker_id, operation_id) in confirmed {
            result = result.merge(
                inner
                    .worker_notify_kill_operation(&worker_id, operation_id)
                    .await,
            );
        }
        result
    }
}

impl RootMetricsComponent for ApiWorkerScheduler {}
