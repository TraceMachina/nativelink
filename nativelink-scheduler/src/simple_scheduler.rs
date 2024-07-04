// Copyright 2023 The NativeLink Authors. All rights reserved.
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

use std::pin::Pin;
use std::sync::Arc;

use async_lock::Mutex;
use async_trait::async_trait;
use futures::{Future, Stream};
use nativelink_error::{make_err, make_input_err, Code, Error, ResultExt};
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionState, ClientOperationId, OperationId, WorkerId,
};
use nativelink_util::metrics_utils::Registry;
use nativelink_util::spawn;
use nativelink_util::task::JoinHandleDropGuard;
use tokio::sync::{watch, Notify};
use tokio::time::Duration;
use tokio_stream::StreamExt;
use tracing::{event, Level};

use crate::action_scheduler::ActionScheduler;
use crate::operation_state_manager::{
    ActionStateResult, ClientStateManager, MatchingEngineStateManager, OperationFilter,
    OperationStageFlags,
};
use crate::platform_property_manager::PlatformPropertyManager;
use crate::scheduler_state::state_manager::StateManager;
use crate::scheduler_state::workers::Workers;
use crate::worker::{Worker, WorkerTimestamp};
use crate::worker_scheduler::WorkerScheduler;

/// Default timeout for workers in seconds.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_WORKER_TIMEOUT_S: u64 = 5;

/// Default timeout for recently completed actions in seconds.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_RETAIN_COMPLETED_FOR_S: u64 = 60;

/// Default times a job can retry before failing.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_MAX_JOB_RETRIES: usize = 3;

/// Engine used to manage the queued/running tasks and relationship with
/// the worker nodes. All state on how the workers and actions are interacting
/// should be held in this struct.
pub struct SimpleScheduler {
    matching_engine_state_manager: Arc<dyn MatchingEngineStateManager>,
    client_state_manager: Arc<dyn ClientStateManager>,

    platform_property_manager: Arc<PlatformPropertyManager>,
    // metrics: Arc<Metrics>,
    // Triggers `drop()`` call if scheduler is dropped.
    _task_worker_matching_future: JoinHandleDropGuard<()>,

    /// The duration that actions are kept in recently_completed_actions for.
    retain_completed_for: Duration,

    /// Timeout of how long to evict workers if no response in this given amount of time in seconds.
    worker_timeout_s: u64,

    /// A `Workers` pool that contains all workers that are available to execute actions in a priority
    /// order based on the allocation strategy.
    workers: Mutex<Workers>,
}

impl SimpleScheduler {
    /// Attempts to find a worker to execute an action and begins executing it.
    /// If an action is already running that is cacheable it may merge this action
    /// with the results and state changes of the already running action.
    /// If the task cannot be executed immediately it will be queued for execution
    /// based on priority and other metrics.
    /// All further updates to the action will be provided through `listener`.
    async fn add_action(
        &self,
        client_operation_id: ClientOperationId,
        action_info: ActionInfo,
    ) -> Result<(ClientOperationId, watch::Receiver<Arc<ActionState>>), Error> {
        let add_action_result = self
            .client_state_manager
            .add_action(client_operation_id.clone(), action_info)
            .await?;
        add_action_result
            .as_receiver()
            .await
            .map(move |receiver| (client_operation_id, receiver.into_owned()))
    }

    async fn clean_recently_completed_actions(&self) {
        todo!();
        // let expiry_time = SystemTime::now()
        //     .checked_sub(self.retain_completed_for)
        //     .unwrap();
        // self.state_manager
        //     .inner
        //     .lock()
        //     .await
        //     .recently_completed_actions
        //     .retain(|action| action.completed_time > expiry_time);
    }

    async fn find_by_client_operation_id(
        &self,
        client_operation_id: &ClientOperationId,
    ) -> Result<Option<watch::Receiver<Arc<ActionState>>>, Error> {
        let filter_result = self
            .client_state_manager
            .filter_operations(&OperationFilter {
                client_operation_id: Some(client_operation_id.clone()),
                ..Default::default()
            })
            .await;

        let mut stream = filter_result
            .err_tip(|| "In SimpleScheduler::find_by_client_operation_id getting filter result")?;
        let Some(result) = stream.next().await else {
            return Ok(None);
        };
        Ok(Some(
            result
                .as_receiver()
                .await
                .err_tip(|| "In SimpleScheduler::find_by_client_operation_id getting receiver")?
                .into_owned(),
        ))
    }

    async fn get_queued_operations(
        &self,
    ) -> Result<Pin<Box<dyn Stream<Item = Arc<dyn ActionStateResult + 'static>> + Send>>, Error>
    {
        self.matching_engine_state_manager
            .filter_operations(&OperationFilter {
                stages: OperationStageFlags::Queued,
                ..Default::default()
            })
            .await
            .err_tip(|| "In SimpleScheduler::get_queued_operations getting filter result")
    }

    // TODO(blaise.bruer) This is an O(n*m) (aka n^2) algorithm. In theory we can create a map
    // of capabilities of each worker and then try and match the actions to the worker using
    // the map lookup (ie. map reduce).
    async fn do_try_match(&self) -> Result<(), Error> {
        async fn match_action_to_worker(
            action_state_result: &dyn ActionStateResult,
            workers: &mut Workers,
            matching_engine_state_manager: &dyn MatchingEngineStateManager,
        ) -> Result<(), Error> {
            let action_info = action_state_result
                .as_action_info()
                .await
                .err_tip(|| "Failed to get action_info from as_action_info_result stream")?;

            // Try to find a worker for the action.
            let worker_id = {
                let platform_properties = &action_info.platform_properties;
                match workers.find_worker_for_action(platform_properties) {
                    Some(worker_id) => worker_id,
                    // If we could not find a worker for the action,
                    // we have nothing to do.
                    None => return Ok(()),
                }
            };

            // Extract the operation_id from the action_state.
            let operation_id = {
                let action_state = action_state_result
                    .as_state()
                    .await
                    .err_tip(|| "Failed to get action_info from as_state_result stream")?;
                action_state.id.clone()
            };

            // Tell the matching engine that the operation is being assigned to a worker.
            {
                let assign_operation_result = matching_engine_state_manager
                    .assign_operation(&operation_id, Ok(&worker_id))
                    .await
                    .err_tip(|| "Failed to assign operation in do_try_match");
                if let Err(err) = assign_operation_result {
                    return workers
                        .immediate_evict_worker(&worker_id, err.clone())
                        .await
                        .err_tip(|| {
                            format!("Update operation failed for {operation_id} in do_try_match")
                        })
                        .merge(Err(err));
                }
            }

            // Notify the worker to run the action.
            {
                workers
                    .worker_notify_run_action(
                        worker_id,
                        operation_id,
                        action_info,
                    )
                    .await
                    .err_tip(|| {
                        "Failed to run worker_notify_run_action in SimpleScheduler::do_try_match"
                    })
            }
        }

        let mut result = Ok(());

        let mut workers = self.workers.lock().await;
        let mut stream = self
            .get_queued_operations()
            .await
            .err_tip(|| "Failed to get queued operations in do_try_match")?;

        while let Some(action_state_result) = stream.next().await {
            result = result.merge(
                match_action_to_worker(
                    action_state_result.as_ref(),
                    &mut workers,
                    self.matching_engine_state_manager.as_ref(),
                )
                .await,
            );
        }
        result
    }
}

impl SimpleScheduler {
    pub fn new(scheduler_cfg: &nativelink_config::schedulers::SimpleScheduler) -> Arc<Self> {
        Self::new_with_callback(scheduler_cfg, || {
            // The cost of running `do_try_match()` is very high, but constant
            // in relation to the number of changes that have happened. This means
            // that grabbing this lock to process `do_try_match()` should always
            // yield to any other tasks that might want the lock. The easiest and
            // most fair way to do this is to sleep for a small amount of time.
            // Using something like tokio::task::yield_now() does not yield as
            // aggresively as we'd like if new futures are scheduled within a future.
            tokio::time::sleep(Duration::from_millis(1))
        })
    }

    pub fn new_with_callback<
        Fut: Future<Output = ()> + Send,
        F: Fn() -> Fut + Send + Sync + 'static,
    >(
        scheduler_cfg: &nativelink_config::schedulers::SimpleScheduler,
        on_matching_engine_run: F,
    ) -> Arc<Self> {
        let platform_property_manager = Arc::new(PlatformPropertyManager::new(
            scheduler_cfg
                .supported_platform_properties
                .clone()
                .unwrap_or_default(),
        ));

        let mut worker_timeout_s = scheduler_cfg.worker_timeout_s;
        if worker_timeout_s == 0 {
            worker_timeout_s = DEFAULT_WORKER_TIMEOUT_S;
        }

        let mut retain_completed_for_s = scheduler_cfg.retain_completed_for_s;
        if retain_completed_for_s == 0 {
            retain_completed_for_s = DEFAULT_RETAIN_COMPLETED_FOR_S;
        }

        let mut max_job_retries = scheduler_cfg.max_job_retries;
        if max_job_retries == 0 {
            max_job_retries = DEFAULT_MAX_JOB_RETRIES;
        }

        let tasks_or_worker_change_notify = Arc::new(Notify::new());
        let state_manager = Arc::new(StateManager::new(
            tasks_or_worker_change_notify.clone(),
            max_job_retries,
        ));

        Arc::new_cyclic(move |weak_self| -> Self {
            let weak_inner = weak_self.clone();
            let tasks_or_worker_change_notify_copy = tasks_or_worker_change_notify.clone();
            let task_worker_matching_future =
                spawn!("simple_scheduler_task_worker_matching", async move {
                    // Break out of the loop only when the inner is dropped.
                    loop {
                        tasks_or_worker_change_notify_copy.notified().await;
                        let result = match weak_inner.upgrade() {
                            Some(scheduler) => scheduler.do_try_match().await,
                            // If the inner went away it means the scheduler is shutting
                            // down, so we need to resolve our future.
                            None => return,
                        };
                        if let Err(err) = result {
                            event!(Level::ERROR, ?err, "Error while running do_try_match");
                        }

                        on_matching_engine_run().await;
                    }
                    // Unreachable.
                });
            SimpleScheduler {
                matching_engine_state_manager: state_manager.clone(),
                client_state_manager: state_manager.clone(),
                retain_completed_for: Duration::new(retain_completed_for_s, 0),
                worker_timeout_s,
                workers: Mutex::new(Workers::new(
                    state_manager.clone(),
                    scheduler_cfg.allocation_strategy,
                    tasks_or_worker_change_notify,
                )),
                platform_property_manager,
                _task_worker_matching_future: task_worker_matching_future,
            }
        })
    }

    /// Checks to see if the worker exists in the worker pool. Should only be used in unit tests.
    #[must_use]
    pub async fn contains_worker_for_test(&self, worker_id: &WorkerId) -> bool {
        let workers = self.workers.lock().await;
        workers.workers.contains(worker_id)
    }

    /// A unit test function used to send the keep alive message to the worker from the server.
    pub async fn send_keep_alive_to_worker_for_test(
        &self,
        worker_id: &WorkerId,
    ) -> Result<(), Error> {
        let mut workers = self.workers.lock().await;
        let worker = workers.workers.get_mut(worker_id).ok_or_else(|| {
            make_input_err!("WorkerId '{}' does not exist in workers map", worker_id)
        })?;
        worker.keep_alive()
    }
}

#[async_trait]
impl ActionScheduler for SimpleScheduler {
    async fn get_platform_property_manager(
        &self,
        _instance_name: &str,
    ) -> Result<Arc<PlatformPropertyManager>, Error> {
        Ok(self.platform_property_manager.clone())
    }

    async fn add_action(
        &self,
        client_operation_id: ClientOperationId,
        action_info: ActionInfo,
    ) -> Result<(ClientOperationId, watch::Receiver<Arc<ActionState>>), Error> {
        self.add_action(client_operation_id, action_info).await
    }

    async fn find_by_client_operation_id(
        &self,
        client_operation_id: &ClientOperationId,
    ) -> Result<Option<watch::Receiver<Arc<ActionState>>>, Error> {
        let maybe_receiver = self
            .find_by_client_operation_id(&client_operation_id)
            .await
            .err_tip(|| {
                format!("Error while finding action with client id: {client_operation_id:?}")
            })?;
        Ok(maybe_receiver)
    }

    async fn clean_recently_completed_actions(&self) {
        self.clean_recently_completed_actions().await;
    }

    fn register_metrics(self: Arc<Self>, _registry: &mut Registry) {}
}

#[async_trait]
impl WorkerScheduler for SimpleScheduler {
    fn get_platform_property_manager(&self) -> &PlatformPropertyManager {
        self.platform_property_manager.as_ref()
    }

    async fn add_worker(&self, worker: Worker) -> Result<(), Error> {
        let worker_id = worker.id;
        let mut workers = self.workers.lock().await;
        let result = workers
            .add_worker(worker)
            .err_tip(|| "Error while adding worker, removing from pool");
        if let Err(err) = result {
            return Result::<(), _>::Err(err.clone()).merge(
                workers
                    .immediate_evict_worker(&worker_id, err)
                    .await,
            );
        }
        Ok(())
    }

    async fn update_action(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        action_stage: Result<ActionStage, Error>,
    ) -> Result<(), Error> {
        let mut workers = self.workers.lock().await;
        workers.update_action(worker_id, operation_id, action_stage)
            .await
    }

    async fn worker_keep_alive_received(
        &self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
    ) -> Result<(), Error> {
        let mut workers = self.workers.lock().await;
        workers
            .refresh_lifetime(worker_id, timestamp)
            .err_tip(|| "Error refreshing lifetime in worker_keep_alive_received()")
    }

    async fn remove_worker(&self, worker_id: &WorkerId) -> Result<(), Error> {
        let mut workers = self.workers.lock().await;
        workers
            .immediate_evict_worker(
                worker_id,
                make_err!(Code::Internal, "Received request to remove worker"),
            )
            .await
    }

    async fn remove_timedout_workers(&self, now_timestamp: WorkerTimestamp) -> Result<(), Error> {
        let mut workers = self.workers.lock().await;
        let worker_timeout_s = self.worker_timeout_s;

        let mut result = Ok(());
        // Items should be sorted based on last_update_timestamp, so we don't need to iterate the entire
        // map most of the time.
        let worker_ids_to_remove: Vec<WorkerId> = workers
            .workers
            .iter()
            .rev()
            .map_while(|(worker_id, worker)| {
                if worker.last_update_timestamp <= now_timestamp - worker_timeout_s {
                    Some(*worker_id)
                } else {
                    None
                }
            })
            .collect();
        for worker_id in &worker_ids_to_remove {
            event!(
                Level::WARN,
                ?worker_id,
                "Worker timed out, removing from pool"
            );
            result = result.merge(
                workers
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

    async fn set_drain_worker(&self, worker_id: &WorkerId, is_draining: bool) -> Result<(), Error> {
        self.workers.lock().await.set_drain_worker(worker_id, is_draining).await
    }

    fn register_metrics(self: Arc<Self>, _registry: &mut Registry) {
        // We do not register anything here because we only want to register metrics
        // once and we rely on the `ActionScheduler::register_metrics()` to do that.
    }
}
