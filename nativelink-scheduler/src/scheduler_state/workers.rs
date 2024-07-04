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

use std::sync::Arc;

use lru::LruCache;
use nativelink_config::schedulers::WorkerAllocationStrategy;
use nativelink_error::{error_if, make_err, make_input_err, Code, Error, ResultExt};
use nativelink_util::action_messages::{ActionInfo, ActionStage, OperationId, WorkerId};
use nativelink_util::platform_properties::PlatformProperties;
use tokio::sync::Notify;
use tracing::{event, Level};

use crate::{
    operation_state_manager::WorkerStateManager,
    worker::{Worker, WorkerTimestamp, WorkerUpdate},
};

/// A collection of workers that are available to run tasks.
pub struct Workers {
    /// A `LruCache` of workers availabled based on `allocation_strategy`.
    pub(crate) workers: LruCache<WorkerId, Worker>,

    /// The worker state manager.
    worker_state_manager: Arc<dyn WorkerStateManager>,
    /// The allocation strategy for workers.
    pub(crate) allocation_strategy: WorkerAllocationStrategy,
    /// A channel to notify the matching engine that the worker pool has changed.
    pub(crate) worker_change_notify: Arc<Notify>,
}

impl Workers {
    pub(crate) fn new(
        worker_state_manager: Arc<dyn WorkerStateManager>,
        allocation_strategy: WorkerAllocationStrategy,
        worker_change_notify: Arc<Notify>,
    ) -> Self {
        Self {
            workers: LruCache::unbounded(),
            worker_state_manager,
            allocation_strategy,
            worker_change_notify,
        }
    }

    /// Refreshes the lifetime of the worker with the given timestamp.
    pub(crate) fn refresh_lifetime(
        &mut self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
    ) -> Result<(), Error> {
        let worker = self.workers.peek_mut(worker_id).ok_or_else(|| {
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
        Ok(())
    }

    /// Adds a worker to the pool.
    /// Note: This function will not do any task matching.
    pub(crate) fn add_worker(&mut self, worker: Worker) -> Result<(), Error> {
        let worker_id = worker.id;
        self.workers.put(worker_id, worker);

        // Worker is not cloneable, and we do not want to send the initial connection results until
        // we have added it to the map, or we might get some strange race conditions due to the way
        // the multi-threaded runtime works.
        let worker = self.workers.peek_mut(&worker_id).unwrap();
        let res = worker
            .send_initial_connection_result()
            .err_tip(|| "Failed to send initial connection result to worker");
        if let Err(err) = &res {
            event!(
                Level::ERROR,
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
    pub(crate) fn remove_worker(&mut self, worker_id: &WorkerId) -> Option<Worker> {
        let result = self.workers.pop(worker_id);
        self.worker_change_notify.notify_one();
        result
    }

    /// Sets if the worker is draining or not.
    pub async fn set_drain_worker(&mut self, worker_id: &WorkerId, is_draining: bool) -> Result<(), Error> {
        let worker = self
            .workers
            .get_mut(&worker_id)
            .err_tip(|| format!("Worker {worker_id} doesn't exist in the pool"))?;
        worker.is_draining = is_draining;
        // TODO(allada) This should move to inside the Workers struct.
        self.worker_change_notify.notify_one();
        Ok(())
    }

    // Attempts to find a worker that is capable of running this action.
    // TODO(blaise.bruer) This algorithm is not very efficient. Simple testing using a tree-like
    // structure showed worse performance on a 10_000 worker * 7 properties * 1000 queued tasks
    // simulation of worst cases in a single threaded environment.
    pub(crate) fn find_worker_for_action(
        &self,
        platform_properties: &PlatformProperties,
    ) -> Option<WorkerId> {
        let mut workers_iter = self.workers.iter();
        let workers_iter = match self.allocation_strategy {
            // Use rfind to get the least recently used that satisfies the properties.
            WorkerAllocationStrategy::least_recently_used => workers_iter.rfind(|(_, w)| {
                w.can_accept_work() && platform_properties.is_satisfied_by(&w.platform_properties)
            }),
            // Use find to get the most recently used that satisfies the properties.
            WorkerAllocationStrategy::most_recently_used => workers_iter.find(|(_, w)| {
                w.can_accept_work() && platform_properties.is_satisfied_by(&w.platform_properties)
            }),
        };
        workers_iter.map(|(_, w)| &w.id).copied()
    }

    pub async fn update_action(
        &mut self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        action_stage: Result<ActionStage, Error>,
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
            return Result::<(), _>::Err(err.clone()).merge(
                self
                    .immediate_evict_worker(worker_id, err)
                    .await,
            );
        }

        // Update the operation in the worker state manager.
        {
            let update_operation_res = self
                .worker_state_manager
                .update_operation(operation_id, worker_id, action_stage.clone())
                .await
                .err_tip(|| "in update_operation on SimpleScheduler::update_action");
            if let Err(err) = update_operation_res {
                let result = Result::<(), _>::Err(err.clone()).merge(
                    self
                        .immediate_evict_worker(worker_id, err)
                        .await,
                );

                event!(
                    Level::ERROR,
                    ?operation_id,
                    ?worker_id,
                    ?result,
                    "Failed to update_operation on update_action"
                );
                return result;
            }
        }

        // Clear this action from the current worker.
        let complete_action_res = {
            let was_paused = !worker.can_accept_work();

            // Note: We need to run this before dealing with backpressure logic.
            let complete_action_res = worker.complete_action(operation_id);

            let due_to_backpressure = action_stage
                .as_ref()
                .map_or_else(|e| e.code == Code::ResourceExhausted, |_| false);
            // Only pause if there's an action still waiting that will unpause.
            if (was_paused || due_to_backpressure) && worker.has_actions() {
                worker.is_paused = true;
            }
            complete_action_res
        };

        // TODO(allada) This should move to inside the Workers struct.
        self.worker_change_notify.notify_one();

        complete_action_res
    }

    /// Notifies the specified worker to run the given action and handles errors by evicting
    /// the worker if the notification fails.
    pub async fn worker_notify_run_action(
        &mut self,
        worker_id: WorkerId,
        operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<(), Error> {
        if let Some(worker) = self.workers.get_mut(&worker_id) {
            let notify_worker_result =
                worker.notify_update(WorkerUpdate::RunAction((operation_id, action_info.clone())));

            if notify_worker_result.is_err() {
                event!(
                    Level::WARN,
                    ?worker_id,
                    ?action_info,
                    ?notify_worker_result,
                    "Worker command failed, removing worker",
                );

                let err = make_err!(
                    Code::Internal,
                    "Worker command failed, removing worker {worker_id} -- {notify_worker_result:?}",
                );

                return Result::<(), _>::Err(err.clone()).merge(
                    self.immediate_evict_worker(&worker_id, err)
                        .await,
                );
            }
        }
        Ok(())
    }

    /// Evicts the worker from the pool and puts items back into the queue if anything was being executed on it.
    pub async fn immediate_evict_worker(
        &mut self,
        worker_id: &WorkerId,
        err: Error,
    ) -> Result<(), Error> {
        let mut result = Ok(());
        if let Some(mut worker) = self.remove_worker(worker_id) {
            // We don't care if we fail to send message to worker, this is only a best attempt.
            let _ = worker.notify_update(WorkerUpdate::Disconnect);
            // We create a temporary Vec to avoid doubt about a possible code
            // path touching the worker.running_action_infos elsewhere.
            for (operation_id, _) in worker.running_action_infos.drain() {
                result = result.merge(
                    self.worker_state_manager
                        .update_operation(&operation_id, &worker_id, Err(err.clone()))
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
