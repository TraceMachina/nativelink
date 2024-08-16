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

use std::borrow::Borrow;
use std::cmp;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::iter::Map;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use futures::Future;
use hashbrown::hash_map::Iter;
use hashbrown::{HashMap, HashSet};
use lru::LruCache;
use nativelink_config::schedulers::WorkerAllocationStrategy;
use nativelink_error::{error_if, make_err, make_input_err, Code, Error, ResultExt};
use nativelink_proto::google::longrunning::Operation;
use nativelink_config::stores::EvictionPolicy;
use nativelink_error::{Error, ResultExt};
use nativelink_metric::{MetricsComponent, RootMetricsComponent};
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionState, OperationId, WorkerId,
};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, MatchingEngineStateManager,
    OperationFilter, OperationStageFlags, OrderDirection,
};
use nativelink_util::spawn;
use nativelink_util::task::JoinHandleDropGuard;
use parking_lot::{Mutex, MutexGuard, RawMutex};
use tokio::sync::{watch, Notify};
use tokio::sync::Notify;
use tokio::time::Duration;
use tokio_stream::StreamExt;
use tracing::{event, Level};

use crate::action_scheduler::ActionScheduler;
use crate::operations::Operations;
use crate::api_worker_scheduler::ApiWorkerScheduler;
use crate::memory_awaited_action_db::MemoryAwaitedActionDb;
use crate::platform_property_manager::PlatformPropertyManager;
use crate::simple_scheduler_state_manager::SimpleSchedulerStateManager;
use crate::worker::{ActionInfoWithProps, Worker, WorkerTimestamp};
use crate::worker_scheduler::WorkerScheduler;

/// Default timeout for workers in seconds.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_WORKER_TIMEOUT_S: u64 = 5;

/// Default timeout for recently completed actions in seconds.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_RETAIN_COMPLETED_FOR_S: u32 = 60;

/// Default times a job can retry before failing.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_MAX_JOB_RETRIES: usize = 3;

/// An action that is being awaited on and last known state.
#[derive(Clone, Debug)]
struct AwaitedAction {
    action_info: Arc<ActionInfo>,
    current_state: Arc<ActionState>,
    notify_channel: watch::Sender<Arc<ActionState>>,

    /// Number of attempts the job has been tried.
    attempts: usize,
    /// Possible last error set by the worker. If empty and attempts is set, it may be due to
    /// something like a worker timeout.
    last_error: Option<Error>,

    /// Worker that is currently running this action, None if unassigned.
    worker_id: Option<WorkerId>,
}

struct Workers {
    workers: LruCache<WorkerId, Worker>,
    /// The allocation strategy for workers.
    allocation_strategy: WorkerAllocationStrategy,
}

struct SimpleSchedulerActionStateResult {
    client_operation_id: OperationId,
    action_state_result: Box<dyn ActionStateResult>,
}

impl SimpleSchedulerActionStateResult {
    fn new(
        client_operation_id: OperationId,
        action_state_result: Box<dyn ActionStateResult>,
    ) -> Self {
        Self {
            client_operation_id,
            action_state_result,
        }
    }

    /// Refreshes the lifetime of the worker with the given timestamp.
    fn refresh_lifetime(
        &mut self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
    ) -> Result<(), Error> {
        let worker = self.workers.get_mut(worker_id).ok_or_else(|| {
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
    fn add_worker(&mut self, worker: Worker) -> Result<(), Error> {
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
        res
    }

    /// Removes worker from pool.
    /// Note: The caller is responsible for any rescheduling of any tasks that might be
    /// running.
    fn remove_worker(&mut self, worker_id: &WorkerId) -> Option<Worker> {
        self.workers.pop(worker_id)
    }

    /// Attempts to find a worker that is capable of running this action.
    // TODO(blaise.bruer) This algorithm is not very efficient. Simple testing using a tree-like
    // structure showed worse performance on a 10_000 worker * 7 properties * 1000 queued tasks
    // simulation of worst cases in a single threaded environment.
    fn find_worker_for_action_mut<'a>(
        &'a mut self,
        awaited_action: &AwaitedAction,
    ) -> Option<&'a mut Worker> {
        assert!(matches!(
            awaited_action.current_state.stage,
            ActionStage::Queued
        ));
        let action_properties = &awaited_action.action_info.platform_properties;
        let mut workers_iter = self.workers.iter_mut();
        let workers_iter = match self.allocation_strategy {
            // Use rfind to get the least recently used that satisfies the properties.
            WorkerAllocationStrategy::least_recently_used => workers_iter.rfind(|(_, w)| {
                w.can_accept_work() && action_properties.is_satisfied_by(&w.platform_properties)
            }),
            // Use find to get the most recently used that satisfies the properties.
            WorkerAllocationStrategy::most_recently_used => workers_iter.find(|(_, w)| {
                w.can_accept_work() && action_properties.is_satisfied_by(&w.platform_properties)
            }),
        };
        let worker_id = workers_iter.map(|(_, w)| &w.id);
        // We need to "touch" the worker to ensure it gets re-ordered in the LRUCache, since it was selected.
        if let Some(&worker_id) = worker_id {
            self.workers.get_mut(&worker_id)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone)]
struct CompletedAction {
    completed_time: SystemTime,
    state: Arc<ActionState>,
}

#[async_trait]
impl ActionStateResult for SimpleSchedulerActionStateResult {
    async fn as_state(&self) -> Result<Arc<ActionState>, Error> {
        let mut action_state = self
            .action_state_result
            .as_state()
            .await
            .err_tip(|| "In SimpleSchedulerActionStateResult")?;
        // We need to ensure the client is not aware of the downstream
        // operation id, so override it before it goes out.
        Arc::make_mut(&mut action_state).operation_id = self.client_operation_id.clone();
        Ok(action_state)
    }

    async fn changed(&mut self) -> Result<Arc<ActionState>, Error> {
        let mut action_state = self
            .action_state_result
            .changed()
            .await
            .err_tip(|| "In SimpleSchedulerActionStateResult")?;
        // We need to ensure the client is not aware of the downstream
        // operation id, so override it before it goes out.
        Arc::make_mut(&mut action_state).operation_id = self.client_operation_id.clone();
        Ok(action_state)
    }

    async fn as_action_info(&self) -> Result<Arc<ActionInfo>, Error> {
        self.action_state_result
            .as_action_info()
            .await
            .err_tip(|| "In SimpleSchedulerActionStateResult")
    }
}

/// Engine used to manage the queued/running tasks and relationship with
/// the worker nodes. All state on how the workers and actions are interacting
/// should be held in this struct.
#[derive(MetricsComponent)]
pub struct SimpleScheduler {
    /// Manager for matching engine side of the state manager.
    matching_engine_state_manager: Arc<dyn MatchingEngineStateManager>,

    /// Manager for client state of this scheduler.
    #[metric(group = "client_state_manager")]
    client_state_manager: Arc<dyn ClientStateManager>,

    /// Manager for platform of this scheduler.
    #[metric(group = "platform_properties")]
    platform_property_manager: Arc<PlatformPropertyManager>,

    /// A `Workers` pool that contains all workers that are available to execute actions in a priority
    /// order based on the allocation strategy.
    worker_scheduler: Arc<ApiWorkerScheduler>,

    /// Background task that tries to match actions to workers. If this struct
    /// is dropped the spawn will be cancelled as well.
    _task_worker_matching_spawn: JoinHandleDropGuard<()>,
}

impl SimpleScheduler {
    /// Attempts to find a worker to execute an action and begins executing it.
    /// If an action is already running that is cacheable it may merge this
    /// action with the results and state changes of the already running
    /// action. If the task cannot be executed immediately it will be queued
    /// for execution based on priority and other metrics.
    /// All further updates to the action will be provided through the returned
    /// value.
    async fn inner_add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        let action_state_result = self
            .client_state_manager
            .add_action(client_operation_id.clone(), action_info)
            .await
            .err_tip(|| "In SimpleScheduler::add_action")?;
        Ok(Box::new(SimpleSchedulerActionStateResult::new(
            client_operation_id.clone(),
            action_state_result,
        )))
    }

    async fn inner_filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        self.client_state_manager
            .filter_operations(filter)
            .await
            .err_tip(|| "In SimpleScheduler::find_by_client_operation_id getting filter result")
    }

    async fn get_queued_operations(&self) -> Result<ActionStateResultStream, Error> {
        let filter = OperationFilter {
            stages: OperationStageFlags::Queued,
            order_by_priority_direction: Some(OrderDirection::Desc),
            ..Default::default()
        };
        self.matching_engine_state_manager
            .filter_operations(filter)
            .await
            .err_tip(|| "In SimpleScheduler::get_queued_operations getting filter result")
    }

    // TODO(blaise.bruer) This is an O(n*m) (aka n^2) algorithm. In theory we
    // can create a map of capabilities of each worker and then try and match
    // the actions to the worker using the map lookup (ie. map reduce).
    async fn do_try_match(&self) -> Result<(), Error> {
        async fn match_action_to_worker(
            action_state_result: &dyn ActionStateResult,
            workers: &ApiWorkerScheduler,
            matching_engine_state_manager: &dyn MatchingEngineStateManager,
            platform_property_manager: &PlatformPropertyManager,
        ) -> Result<(), Error> {
            let action_info = action_state_result
                .as_action_info()
                .await
                .err_tip(|| "Failed to get action_info from as_action_info_result stream")?;

            // TODO(allada) We should not compute this every time and instead store
            // it with the ActionInfo when we receive it.
            let platform_properties = platform_property_manager
                .make_platform_properties(action_info.platform_properties.clone())
                .err_tip(|| {
                    "Failed to make platform properties in SimpleScheduler::do_try_match"
                })?;

            let action_info = ActionInfoWithProps {
                inner: action_info,
                platform_properties,
            };

            // Try to find a worker for the action.
            let worker_id = {
                match workers
                    .find_worker_for_action(&action_info.platform_properties)
                    .await
                {
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
                action_state.operation_id.clone()
            };

            // Tell the matching engine that the operation is being assigned to a worker.
            matching_engine_state_manager
                .assign_operation(&operation_id, Ok(&worker_id))
                .await
                .err_tip(|| "Failed to assign operation in do_try_match")?;

            // Notify the worker to run the action.
            {
                workers
                    .worker_notify_run_action(worker_id, operation_id, action_info)
                    .await
                    .err_tip(|| {
                        "Failed to run worker_notify_run_action in SimpleScheduler::do_try_match"
                    })
            }
        }

        let mut result = Ok(());

        let mut stream = self
            .get_queued_operations()
            .await
            .err_tip(|| "Failed to get queued operations in do_try_match")?;

        while let Some(action_state_result) = stream.next().await {
            result = result.merge(
                match_action_to_worker(
                    action_state_result.as_ref(),
                    self.worker_scheduler.as_ref(),
                    self.matching_engine_state_manager.as_ref(),
                    self.platform_property_manager.as_ref(),
                )
                .await,
            );
        }
        result
    }
}

impl SimpleScheduler {
    pub fn new(
        scheduler_cfg: &nativelink_config::schedulers::SimpleScheduler,
    ) -> (Arc<Self>, Arc<dyn WorkerScheduler>) {
        Self::new_with_callback(
            scheduler_cfg,
            || {
                // The cost of running `do_try_match()` is very high, but constant
                // in relation to the number of changes that have happened. This
                // means that grabbing this lock to process `do_try_match()` should
                // always yield to any other tasks that might want the lock. The
                // easiest and most fair way to do this is to sleep for a small
                // amount of time. Using something like tokio::task::yield_now()
                // does not yield as aggresively as we'd like if new futures are
                // scheduled within a future.
                tokio::time::sleep(Duration::from_millis(1))
            },
            SystemTime::now,
        )
    }

    pub fn new_with_callback<
        Fut: Future<Output = ()> + Send,
        F: Fn() -> Fut + Send + Sync + 'static,
        I: InstantWrapper,
        NowFn: Fn() -> I + Clone + Send + Sync + 'static,
    >(
        scheduler_cfg: &nativelink_config::schedulers::SimpleScheduler,
        on_matching_engine_run: F,
        now_fn: NowFn,
    ) -> (Arc<Self>, Arc<dyn WorkerScheduler>) {
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
        let state_manager = SimpleSchedulerStateManager::new(
            tasks_or_worker_change_notify.clone(),
            max_job_retries,
            MemoryAwaitedActionDb::new(
                &EvictionPolicy {
                    max_seconds: retain_completed_for_s,
                    ..Default::default()
                },
                now_fn,
            ),
        );

        let worker_scheduler = ApiWorkerScheduler::new(
            state_manager.clone(),
            platform_property_manager.clone(),
            scheduler_cfg.allocation_strategy,
            tasks_or_worker_change_notify.clone(),
            worker_timeout_s,
        );

        let worker_scheduler_clone = worker_scheduler.clone();

        let action_scheduler = Arc::new_cyclic(move |weak_self| -> Self {
            let weak_inner = weak_self.clone();
            let task_worker_matching_spawn =
                spawn!("simple_scheduler_task_worker_matching", async move {
                    // Break out of the loop only when the inner is dropped.
                    loop {
                        tasks_or_worker_change_notify.notified().await;
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
                worker_scheduler,
                platform_property_manager,
                _task_worker_matching_spawn: task_worker_matching_spawn,
            }
        });
        (action_scheduler, worker_scheduler_clone)
    }
}

#[async_trait]
impl ClientStateManager for SimpleScheduler {
    async fn add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        self.inner_add_action(client_operation_id, action_info)
            .await
    }

    async fn filter_operations<'a>(
        &'a self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream<'a>, Error> {
        self.inner_filter_operations(filter).await
    }

    fn as_known_platform_property_provider(&self) -> Option<&dyn KnownPlatformPropertyProvider> {
        Some(self)
    }
}

#[async_trait]
impl KnownPlatformPropertyProvider for SimpleScheduler {
    async fn get_known_properties(&self, _instance_name: &str) -> Result<Vec<String>, Error> {
        Ok(self
            .worker_scheduler
            .get_platform_property_manager()
            .get_known_properties()
            .keys()
            .cloned()
            .collect())
    }
}

#[async_trait]
impl WorkerScheduler for SimpleScheduler {
    fn get_platform_property_manager(&self) -> &PlatformPropertyManager {
        self.worker_scheduler.get_platform_property_manager()
    }

    async fn add_worker(&self, worker: Worker) -> Result<(), Error> {
        self.worker_scheduler.add_worker(worker).await
    }

    async fn update_action(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        action_stage: Result<ActionStage, Error>,
    ) -> Result<(), Error> {
        self.worker_scheduler
            .update_action(worker_id, operation_id, action_stage)
            .await
    }

    async fn worker_keep_alive_received(
        &self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
    ) -> Result<(), Error> {
        self.worker_scheduler
            .worker_keep_alive_received(worker_id, timestamp)
            .await
    }

    async fn remove_worker(&self, worker_id: &WorkerId) -> Result<(), Error> {
        self.worker_scheduler.remove_worker(worker_id).await
    }
}

#[async_trait]
impl Operations for SimpleScheduler {
    fn list_actions(&self) -> Vec<Operation> {
        let lock = self.inner.lock();
        let active_actions: Iter<Arc<ActionInfo>, AwaitedAction> = lock.active_actions.iter();
        let queued_actions: std::collections::btree_map::Iter<Arc<ActionInfo>, AwaitedAction> =
            lock.queued_actions.iter();
        let recently_completed_actions = lock.recently_completed_actions.iter();

        let active_operations: Vec<Operation> = active_actions
            .map(|(_, awaited_action)| {
                Operation::from(awaited_action.current_state.as_ref().clone())
            })
            .collect();
        let queued_operations: Vec<Operation> = queued_actions
            .map(|(_, awaited_action)| {
                Operation::from(awaited_action.current_state.as_ref().clone())
            })
            .collect();
        let completed_operations: Vec<Operation> = recently_completed_actions
            .map(|completed_action| Operation::from(completed_action.state.as_ref().clone()))
            .collect();

        let operations: Vec<Operation> = active_operations
            .into_iter()
            .chain(queued_operations)
            .chain(completed_operations)
            .collect();
        operations
    }

    fn get_action(&self, action_info_hash_key: &ActionInfoHashKey) -> Option<Operation> {
        let lock: parking_lot::lock_api::MutexGuard<RawMutex, SimpleSchedulerImpl> =
            self.inner.lock();
        let active_actions: HashMap<Arc<ActionInfo>, AwaitedAction> = lock.active_actions.clone();
        let queued_actions: BTreeMap<Arc<ActionInfo>, AwaitedAction> = lock.queued_actions.clone();
        let queued_actions_set: HashSet<Arc<ActionInfo>> = lock.queued_actions_set.clone();
        let recently_completed_actions = lock.recently_completed_actions.clone();
        let action: Option<&ActionState> = queued_actions_set
            .get(action_info_hash_key)
            .and_then(|action_info| {
                queued_actions
                    .get(action_info)
                    .map(|queued_action| queued_action.current_state.as_ref())
            })
            .or_else(|| {
                active_actions
                    .get(action_info_hash_key)
                    .map(|awaited_action| awaited_action.current_state.as_ref())
            })
            .or_else(|| {
                recently_completed_actions
                    .get(action_info_hash_key)
                    .map(|completed_action| completed_action.state.as_ref())
            });
        let action: Option<Operation> = action.map(|a| Operation::from(a.clone()));
        action
    }

    async fn delete_action(&self) {
        todo!()
    }

    async fn cancel_action(&self) {
        todo!()
    }

    async fn wait_action(&self) {
        todo!()
    }
}

impl MetricsComponent for SimpleScheduler {
    fn gather_metrics(&self, c: &mut CollectorState) {
        self.metrics.gather_metrics(c);
        {
            // We use the raw lock because we dont gather stats about gathering stats.
            let inner = self.inner.lock();
            c.publish(
                "queued_actions_total",
                &inner.queued_actions.len(),
                "The number actions in the queue.",
            );
            c.publish(
                "workers_total",
                &inner.workers.workers.len(),
                "The number workers active.",
            );
            c.publish(
                "active_actions_total",
                &inner.active_actions.len(),
                "The number of running actions.",
            );
            c.publish(
                "recently_completed_actions_total",
                &inner.recently_completed_actions.len(),
                "The number of recently completed actions in the buffer.",
            );
            c.publish(
                "retain_completed_for_seconds",
                &inner.retain_completed_for,
                "The duration completed actions are retained for.",
            );
            c.publish(
                "worker_timeout_seconds",
                &inner.worker_timeout_s,
                "The configured timeout if workers have not responded for a while.",
            );
            c.publish(
                "max_job_retries",
                &inner.max_job_retries,
                "The amount of times a job is allowed to retry from an internal error before it is dropped.",
            );
            let mut props = HashMap::<&String, u64>::new();
            for (_worker_id, worker) in inner.workers.workers.iter() {
                c.publish_with_labels(
                    "workers",
                    worker,
                    "",
                    vec![("worker_id".into(), worker.id.to_string().into())],
                );
                for (property, prop_value) in &worker.platform_properties.properties {
                    let current_value = props.get(&property).unwrap_or(&0);
                    if let PlatformPropertyValue::Minimum(worker_value) = prop_value {
                        props.insert(property, *current_value + *worker_value);
                    }
                }
            }
            for (property, prop_value) in props {
                c.publish(
                    &format!("{property}_available_properties"),
                    &prop_value,
                    format!("Total sum of available properties for {property}"),
                );
            }
            for (_, active_action) in inner.active_actions.iter() {
                let action_name = active_action
                    .action_info
                    .unique_qualifier
                    .action_name()
                    .into();
                let worker_id_str = match active_action.worker_id {
                    Some(id) => id.to_string(),
                    None => "Unassigned".to_string(),
                };
                c.publish_with_labels(
                    "active_actions",
                    active_action,
                    "",
                    vec![
                        ("worker_id".into(), worker_id_str.into()),
                        ("digest".into(), action_name),
                    ],
                );
            }
            // Note: We don't publish queued_actions because it can be very large.
            // Note: We don't publish recently completed actions because it can be very large.
        }
      
  }
  
     async fn remove_timedout_workers(&self, now_timestamp: WorkerTimestamp) -> Result<(), Error> {
        self.worker_scheduler
            .remove_timedout_workers(now_timestamp)
            .await
    }

    async fn set_drain_worker(&self, worker_id: &WorkerId, is_draining: bool) -> Result<(), Error> {
        self.worker_scheduler
            .set_drain_worker(worker_id, is_draining)
            .await
    }
}

impl RootMetricsComponent for SimpleScheduler {}
