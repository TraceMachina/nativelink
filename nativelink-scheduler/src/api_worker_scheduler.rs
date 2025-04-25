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

use core::ops::{Deref, DerefMut};
use std::sync::Arc;

use async_lock::Mutex;
use lru::LruCache;
use nativelink_config::schedulers::WorkerAllocationStrategy;
use nativelink_error::{Code, Error, ResultExt, error_if, make_err, make_input_err};
use nativelink_util::action_messages::{OperationId, WorkerId};
use nativelink_util::operation_state_manager::{UpdateOperationType, WorkerStateManager};
use nativelink_util::platform_properties::PlatformProperties;
use nativelink_util::spawn;
use nativelink_util::task::JoinHandleDropGuard;
use opentelemetry::{InstrumentationScope, KeyValue, global, metrics};
use tokio::sync::Notify;
use tokio::sync::mpsc::{self, UnboundedSender};
use tonic::async_trait;
use tracing::{error, instrument, warn};

use crate::platform_property_manager::PlatformPropertyManager;
use crate::worker::{ActionInfoWithProps, Worker, WorkerTimestamp, WorkerUpdate};
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

fn init_metrics() -> ApiWorkerSchedulerMetrics {
    let meter =
        global::meter_with_scope(InstrumentationScope::builder("api_worker_scheduler").build());

    ApiWorkerSchedulerMetrics {
        workers_count: meter
            .i64_up_down_counter("worker_count")
            .with_description("Number of workers available in the scheduler")
            .build(),
        worker_actions_total: meter
            .u64_counter("worker_actions_total")
            .with_description("Total number of actions assigned to workers")
            .build(),
        worker_evictions: meter
            .u64_counter("worker_evictions")
            .with_description("Number of workers evicted from the pool")
            .build(),
        worker_operation_duration: meter
            .f64_histogram("worker_operation_duration")
            .with_description("Duration of operations on workers")
            .with_unit("ms")
            .build(),
        worker_backpressure_events: meter
            .u64_counter("worker_backpressure_events")
            .with_description("Number of times workers have applied backpressure")
            .build(),
    }
}

#[derive(Debug, Clone)]
struct ApiWorkerSchedulerMetrics {
    workers_count: metrics::UpDownCounter<i64>,
    worker_actions_total: metrics::Counter<u64>,
    worker_evictions: metrics::Counter<u64>,
    worker_operation_duration: metrics::Histogram<f64>,
    worker_backpressure_events: metrics::Counter<u64>,
}

/// A collection of workers that are available to run tasks.
struct ApiWorkerSchedulerImpl {
    /// A `LruCache` of workers availabled based on `allocation_strategy`.
    workers: Workers,
    /// The worker state manager.
    worker_state_manager: Arc<dyn WorkerStateManager>,
    /// The allocation strategy for workers.
    allocation_strategy: WorkerAllocationStrategy,
    /// A channel to notify the matching engine that the worker pool has changed.
    worker_change_notify: Arc<Notify>,
    /// A channel to notify that an operation is still alive.
    operation_keep_alive_tx: UnboundedSender<(OperationId, WorkerId)>,
    metrics: ApiWorkerSchedulerMetrics,
}

impl core::fmt::Debug for ApiWorkerSchedulerImpl {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ApiWorkerSchedulerImpl")
            .field("workers", &self.workers)
            .field("allocation_strategy", &self.allocation_strategy)
            .field("worker_change_notify", &self.worker_change_notify)
            .field("operation_keep_alive_tx", &self.operation_keep_alive_tx)
            .finish_non_exhaustive()
    }
}

impl ApiWorkerSchedulerImpl {
    /// Refreshes the lifetime of the worker with the given timestamp.
    #[instrument(
        skip(self, worker_id, timestamp),
        fields(worker_id = ?worker_id),
    )]
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
        for operation_id in worker.running_action_infos.keys() {
            if self
                .operation_keep_alive_tx
                .send((operation_id.clone(), worker_id.clone()))
                .is_err()
            {
                error!(
                    ?operation_id,
                    ?worker_id,
                    "OperationKeepAliveTx stream closed"
                );
            }
        }
        Ok(())
    }

    /// Adds a worker to the pool.
    /// Note: This function will not do any task matching.
    #[instrument(
        skip(self, worker),
        fields(worker_id = ?worker.id),
    )]
    fn add_worker(&mut self, worker: Worker) -> Result<(), Error> {
        let worker_id = worker.id.clone();
        self.workers.put(worker_id.clone(), worker);

        self.metrics.workers_count.add(1, &[]);

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
    #[instrument(
        skip(self, worker_id),
        fields(worker_id = ?worker_id),
    )]
    fn remove_worker(&mut self, worker_id: &WorkerId) -> Option<Worker> {
        let result = self.workers.pop(worker_id);

        if result.is_some() {
            self.metrics.workers_count.add(-1, &[]);
        }

        self.worker_change_notify.notify_one();
        result
    }

    /// Sets if the worker is draining or not.
    #[instrument(
        skip(self, worker_id),
        fields(worker_id = ?worker_id, is_draining),
    )]
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

    #[instrument(skip(self, platform_properties))]
    fn inner_find_worker_for_action(
        &self,
        platform_properties: &PlatformProperties,
    ) -> Option<WorkerId> {
        let mut workers_iter = self.workers.iter();
        let workers_iter = match self.allocation_strategy {
            // Use rfind to get the least recently used that satisfies the properties.
            WorkerAllocationStrategy::LeastRecentlyUsed => workers_iter.rfind(|(_, w)| {
                w.can_accept_work() && platform_properties.is_satisfied_by(&w.platform_properties)
            }),
            // Use find to get the most recently used that satisfies the properties.
            WorkerAllocationStrategy::MostRecentlyUsed => workers_iter.find(|(_, w)| {
                w.can_accept_work() && platform_properties.is_satisfied_by(&w.platform_properties)
            }),
        };
        workers_iter.map(|(_, w)| w.id.clone())
    }

    #[instrument(
        skip(self, worker_id, operation_id, update),
        fields(worker_id = ?worker_id, operation_id = ?operation_id),
    )]
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
                .merge(self.immediate_evict_worker(worker_id, err).await);
        }

        let (is_finished, due_to_backpressure) = match &update {
            UpdateOperationType::UpdateWithActionStage(action_stage) => {
                (action_stage.is_finished(), false)
            }
            UpdateOperationType::KeepAlive => (false, false),
            UpdateOperationType::UpdateWithError(err) => {
                (true, err.code == Code::ResourceExhausted)
            }
        };

        if due_to_backpressure {
            self.metrics.worker_backpressure_events.add(1, &[]);
        }

        // Update the operation in the worker state manager.
        {
            let update_operation_res = self
                .worker_state_manager
                .update_operation(operation_id, worker_id, update)
                .await
                .err_tip(|| "in update_operation on SimpleScheduler::update_action");
            if let Err(err) = update_operation_res {
                error!(
                    ?operation_id,
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
            let was_paused = !worker.can_accept_work();

            // Note: We need to run this before dealing with backpressure logic.
            let complete_action_res = worker.complete_action(operation_id).await;

            // Only pause if there's an action still waiting that will unpause.
            if (was_paused || due_to_backpressure) && worker.has_actions() {
                worker.is_paused = true;
            }
            complete_action_res
        };

        self.worker_change_notify.notify_one();

        complete_action_res
    }

    /// Notifies the specified worker to run the given action and handles errors by evicting
    /// the worker if the notification fails.
    #[instrument(
        skip(self, worker_id, operation_id, action_info),
        fields(worker_id = ?worker_id, operation_id = ?operation_id),
    )]
    async fn worker_notify_run_action(
        &mut self,
        worker_id: WorkerId,
        operation_id: OperationId,
        action_info: ActionInfoWithProps,
    ) -> Result<(), Error> {
        let start_time = std::time::Instant::now();

        if let Some(worker) = self.workers.get_mut(&worker_id) {
            let notify_worker_result = worker
                .notify_update(WorkerUpdate::RunAction((operation_id, action_info.clone())))
                .await;

            if notify_worker_result.is_ok() {
                self.metrics.worker_actions_total.add(1, &[]);
                self.metrics.worker_operation_duration.record(
                    start_time.elapsed().as_millis() as f64,
                    &[KeyValue::new("operation", "assign_action")],
                );
            } else {
                warn!(
                    ?worker_id,
                    ?action_info,
                    ?notify_worker_result,
                    "Worker command failed, removing worker",
                );

                let err = make_err!(
                    Code::Internal,
                    "Worker command failed, removing worker {worker_id} -- {notify_worker_result:?}",
                );

                return Result::<(), _>::Err(err.clone())
                    .merge(self.immediate_evict_worker(&worker_id, err).await);
            }
        } else {
            warn!(
                ?worker_id,
                ?operation_id,
                ?action_info,
                "Worker not found in worker map in worker_notify_run_action"
            );
        }
        Ok(())
    }

    /// Evicts the worker from the pool and puts items back into the queue if anything was being executed on it.
    #[instrument(
        skip(self, worker_id, err),
        fields(worker_id = ?worker_id, err_code = ?err.code),
    )]
    async fn immediate_evict_worker(
        &mut self,
        worker_id: &WorkerId,
        err: Error,
    ) -> Result<(), Error> {
        self.metrics
            .worker_evictions
            .add(1, &[KeyValue::new("reason", err.code.to_string())]);
        let mut result = Ok(());
        if let Some(mut worker) = self.remove_worker(worker_id) {
            // We don't care if we fail to send message to worker, this is only a best attempt.
            drop(worker.notify_update(WorkerUpdate::Disconnect).await);
            for (operation_id, _) in worker.running_action_infos.drain() {
                result = result.merge(
                    self.worker_state_manager
                        .update_operation(
                            &operation_id,
                            worker_id,
                            UpdateOperationType::UpdateWithError(err.clone()),
                        )
                        .await,
                );
            }
        }
        // Note: Calling this many time is very cheap, it'll only trigger `do_try_match` once.
        // TODO(allada) This should be moved to inside the Workers struct.
        self.worker_change_notify.notify_one();
        result
    }
}

#[derive(Debug)]
pub struct ApiWorkerScheduler {
    inner: Mutex<ApiWorkerSchedulerImpl>,
    platform_property_manager: Arc<PlatformPropertyManager>,
    worker_timeout_s: u64,
    _operation_keep_alive_spawn: JoinHandleDropGuard<()>,
}

impl ApiWorkerScheduler {
    #[instrument(
        skip(
            worker_state_manager,
            platform_property_manager,
            allocation_strategy,
            worker_change_notify
        ),
        fields(worker_timeout_s),
        level = "info"
    )]
    pub fn new(
        worker_state_manager: Arc<dyn WorkerStateManager>,
        platform_property_manager: Arc<PlatformPropertyManager>,
        allocation_strategy: WorkerAllocationStrategy,
        worker_change_notify: Arc<Notify>,
        worker_timeout_s: u64,
    ) -> Arc<Self> {
        let (operation_keep_alive_tx, mut operation_keep_alive_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            inner: Mutex::new(ApiWorkerSchedulerImpl {
                workers: Workers(LruCache::unbounded()),
                worker_state_manager: worker_state_manager.clone(),
                allocation_strategy,
                worker_change_notify,
                operation_keep_alive_tx,
                metrics: init_metrics(),
            }),
            platform_property_manager,
            worker_timeout_s,
            _operation_keep_alive_spawn: spawn!(
                "simple_scheduler_operation_keep_alive",
                async move {
                    const RECV_MANY_LIMIT: usize = 256;
                    let mut messages = Vec::with_capacity(RECV_MANY_LIMIT);
                    loop {
                        messages.clear();
                        operation_keep_alive_rx
                            .recv_many(&mut messages, RECV_MANY_LIMIT)
                            .await;
                        if messages.is_empty() {
                            return; // Looks like our sender has been dropped.
                        }
                        for (operation_id, worker_id) in messages.drain(..) {
                            let update_operation_res = worker_state_manager
                                .update_operation(
                                    &operation_id,
                                    &worker_id,
                                    UpdateOperationType::KeepAlive,
                                )
                                .await;
                            if let Err(err) = update_operation_res {
                                warn!(
                                    ?err,
                                    "Error while running worker_keep_alive_received, maybe job is done?"
                                );
                            }
                        }
                    }
                }
            ),
        })
    }

    #[instrument(
        skip(self, worker_id, operation_id, action_info),
        fields(worker_id = ?worker_id, operation_id = ?operation_id),
    )]
    pub async fn worker_notify_run_action(
        &self,
        worker_id: WorkerId,
        operation_id: OperationId,
        action_info: ActionInfoWithProps,
    ) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        inner
            .worker_notify_run_action(worker_id, operation_id, action_info)
            .await
    }

    /// Attempts to find a worker that is capable of running this action.
    // TODO(blaise.bruer) This algorithm is not very efficient. Simple testing using a tree-like
    // structure showed worse performance on a 10_000 worker * 7 properties * 1000 queued tasks
    // simulation of worst cases in a single threaded environment.
    #[instrument(skip(self, platform_properties))]
    pub async fn find_worker_for_action(
        &self,
        platform_properties: &PlatformProperties,
    ) -> Option<WorkerId> {
        let inner = self.inner.lock().await;
        inner.inner_find_worker_for_action(platform_properties)
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

    #[instrument(
        skip(self, worker),
        fields(worker_id = ?worker.id),
    )]
    async fn add_worker(&self, worker: Worker) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        let worker_id = worker.id.clone();
        let result = inner
            .add_worker(worker)
            .err_tip(|| "Error while adding worker, removing from pool");
        if let Err(err) = result {
            return Result::<(), _>::Err(err.clone())
                .merge(inner.immediate_evict_worker(&worker_id, err).await);
        }
        Ok(())
    }

    #[instrument(
        skip(self, worker_id, operation_id, update),
        fields(worker_id = ?worker_id, operation_id = ?operation_id),
    )]
    async fn update_action(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        inner.update_action(worker_id, operation_id, update).await
    }

    #[instrument(
        skip(self, worker_id, timestamp),
        fields(worker_id = ?worker_id),
    )]
    async fn worker_keep_alive_received(
        &self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
    ) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        inner
            .refresh_lifetime(worker_id, timestamp)
            .err_tip(|| "Error refreshing lifetime in worker_keep_alive_received()")
    }

    #[instrument(
        skip(self, worker_id),
        fields(worker_id = ?worker_id),
    )]
    async fn remove_worker(&self, worker_id: &WorkerId) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        inner
            .immediate_evict_worker(
                worker_id,
                make_err!(Code::Internal, "Received request to remove worker"),
            )
            .await
    }

    #[instrument(skip(self, now_timestamp))]
    async fn remove_timedout_workers(&self, now_timestamp: WorkerTimestamp) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;

        let mut result = Ok(());
        // Items should be sorted based on last_update_timestamp, so we don't need to iterate the entire
        // map most of the time.
        let worker_ids_to_remove: Vec<WorkerId> = inner
            .workers
            .iter()
            .rev()
            .map_while(|(worker_id, worker)| {
                if worker.last_update_timestamp <= now_timestamp - self.worker_timeout_s {
                    Some(worker_id.clone())
                } else {
                    None
                }
            })
            .collect();
        for worker_id in &worker_ids_to_remove {
            warn!(?worker_id, "Worker timed out, removing from pool");
            result = result.merge(
                inner
                    .immediate_evict_worker(
                        worker_id,
                        make_err!(
                            Code::Internal,
                            "Worker {worker_id} timed out, removing from pool"
                        ),
                    )
                    .await,
            );
        }

        result
    }

    #[instrument(
        skip(self, worker_id),
        fields(worker_id = ?worker_id, is_draining),
    )]
    async fn set_drain_worker(&self, worker_id: &WorkerId, is_draining: bool) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        inner.set_drain_worker(worker_id, is_draining).await
    }
}
