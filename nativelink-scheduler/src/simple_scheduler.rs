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

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use async_trait::async_trait;
use futures::{Future, StreamExt, future};
use nativelink_config::schedulers::SimpleSpec;
use nativelink_error::{Code, Error, ResultExt};
use nativelink_metric::{MetricsComponent, RootMetricsComponent};
use nativelink_proto::com::github::trace_machina::nativelink::events::OriginEvent;
use nativelink_util::action_messages::{ActionInfo, ActionState, OperationId, WorkerId};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, MatchingEngineStateManager,
    OperationFilter, OperationStageFlags, OrderDirection, UpdateOperationType,
};
use nativelink_util::origin_event::OriginMetadata;
use nativelink_util::shutdown_guard::ShutdownGuard;
use nativelink_util::spawn;
use nativelink_util::task::JoinHandleDropGuard;
use opentelemetry::KeyValue;
use opentelemetry::baggage::BaggageExt;
use opentelemetry::context::{Context, FutureExt as OtelFutureExt};
use opentelemetry_semantic_conventions::attribute::ENDUSER_ID;
use tokio::sync::{Notify, mpsc};
use tokio::time::Duration;
use tracing::{error, info, info_span, warn};

use crate::api_worker_scheduler::ApiWorkerScheduler;
use crate::awaited_action_db::{AwaitedActionDb, CLIENT_KEEPALIVE_DURATION};
use crate::platform_property_manager::PlatformPropertyManager;
use crate::simple_scheduler_state_manager::SimpleSchedulerStateManager;
use crate::worker::{ActionInfoWithProps, Worker, WorkerTimestamp};
use crate::worker_registry::WorkerRegistry;
use crate::worker_scheduler::WorkerScheduler;

/// Default timeout for workers in seconds.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_WORKER_TIMEOUT_S: u64 = 5;

/// Mark operations as completed with error if no client has updated them
/// within this duration.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_CLIENT_ACTION_TIMEOUT_S: u64 = 60;

/// Default times a job can retry before failing.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_MAX_JOB_RETRIES: usize = 3;

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
}

#[async_trait]
impl ActionStateResult for SimpleSchedulerActionStateResult {
    async fn as_state(&self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        let (mut action_state, origin_metadata) = self
            .action_state_result
            .as_state()
            .await
            .err_tip(|| "In SimpleSchedulerActionStateResult")?;
        // We need to ensure the client is not aware of the downstream
        // operation id, so override it before it goes out.
        Arc::make_mut(&mut action_state).client_operation_id = self.client_operation_id.clone();
        Ok((action_state, origin_metadata))
    }

    async fn changed(&mut self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        let (mut action_state, origin_metadata) = self
            .action_state_result
            .changed()
            .await
            .err_tip(|| "In SimpleSchedulerActionStateResult")?;
        // We need to ensure the client is not aware of the downstream
        // operation id, so override it before it goes out.
        Arc::make_mut(&mut action_state).client_operation_id = self.client_operation_id.clone();
        Ok((action_state, origin_metadata))
    }

    async fn as_action_info(&self) -> Result<(Arc<ActionInfo>, Option<OriginMetadata>), Error> {
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
    #[metric(group = "matching_engine_state_manager")]
    matching_engine_state_manager: Arc<dyn MatchingEngineStateManager>,

    /// Manager for client state of this scheduler.
    #[metric(group = "client_state_manager")]
    client_state_manager: Arc<dyn ClientStateManager>,

    /// Manager for platform of this scheduler.
    #[metric(group = "platform_properties")]
    platform_property_manager: Arc<PlatformPropertyManager>,

    /// A `Workers` pool that contains all workers that are available to execute actions in a priority
    /// order based on the allocation strategy.
    #[metric(group = "worker_scheduler")]
    worker_scheduler: Arc<ApiWorkerScheduler>,

    /// The sender to send origin events to the origin events.
    maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,

    /// Background task that tries to match actions to workers. If this struct
    /// is dropped the spawn will be cancelled as well.
    task_worker_matching_spawn: JoinHandleDropGuard<()>,

    /// Every duration, do logging of worker matching
    /// e.g. "worker busy", "can't find any worker"
    /// Set to None to disable. This is quite noisy, so we limit it
    worker_match_logging_interval: Option<Duration>,
}

impl core::fmt::Debug for SimpleScheduler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SimpleScheduler")
            .field("platform_property_manager", &self.platform_property_manager)
            .field("worker_scheduler", &self.worker_scheduler)
            .field("maybe_origin_event_tx", &self.maybe_origin_event_tx)
            .field(
                "task_worker_matching_spawn",
                &self.task_worker_matching_spawn,
            )
            .finish_non_exhaustive()
    }
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
    ) -> Result<ActionStateResultStream<'_>, Error> {
        self.client_state_manager
            .filter_operations(filter)
            .await
            .err_tip(|| "In SimpleScheduler::find_by_client_operation_id getting filter result")
    }

    async fn get_queued_operations(&self) -> Result<ActionStateResultStream<'_>, Error> {
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

    pub async fn do_try_match_for_test(&self) -> Result<(), Error> {
        self.do_try_match(true).await
    }

    // TODO(palfrey) This is an O(n*m) (aka n^2) algorithm. In theory we
    // can create a map of capabilities of each worker and then try and match
    // the actions to the worker using the map lookup (ie. map reduce).
    async fn do_try_match(&self, full_worker_logging: bool) -> Result<(), Error> {
        async fn match_action_to_worker(
            action_state_result: &dyn ActionStateResult,
            workers: &ApiWorkerScheduler,
            matching_engine_state_manager: &dyn MatchingEngineStateManager,
            platform_property_manager: &PlatformPropertyManager,
            full_worker_logging: bool,
        ) -> Result<(), Error> {
            let (action_info, maybe_origin_metadata) =
                action_state_result
                    .as_action_info()
                    .await
                    .err_tip(|| "Failed to get action_info from as_action_info_result stream")?;

            // TODO(palfrey) We should not compute this every time and instead store
            // it with the ActionInfo when we receive it.
            let platform_properties = platform_property_manager
                .make_platform_properties(action_info.platform_properties.clone())
                .err_tip(
                    || "Failed to make platform properties in SimpleScheduler::do_try_match",
                )?;

            let action_info = ActionInfoWithProps {
                inner: action_info,
                platform_properties,
            };

            // Try to find a worker for the action.
            let worker_id = {
                match workers
                    .find_worker_for_action(&action_info.platform_properties, full_worker_logging)
                    .await
                {
                    Some(worker_id) => worker_id,
                    // If we could not find a worker for the action,
                    // we have nothing to do.
                    None => return Ok(()),
                }
            };

            let attach_operation_fut = async move {
                // Extract the operation_id from the action_state.
                let operation_id = {
                    let (action_state, _origin_metadata) = action_state_result
                        .as_state()
                        .await
                        .err_tip(|| "Failed to get action_info from as_state_result stream")?;
                    action_state.client_operation_id.clone()
                };

                // Tell the matching engine that the operation is being assigned to a worker.
                let assign_result = matching_engine_state_manager
                    .assign_operation(&operation_id, Ok(&worker_id))
                    .await
                    .err_tip(|| "Failed to assign operation in do_try_match");
                if let Err(err) = assign_result {
                    if err.code == Code::Aborted {
                        // If the operation was aborted, it means that the operation was
                        // cancelled due to another operation being assigned to the worker.
                        return Ok(());
                    }
                    // Any other error is a real error.
                    return Err(err);
                }

                workers
                    .worker_notify_run_action(worker_id, operation_id, action_info)
                    .await
                    .err_tip(|| {
                        "Failed to run worker_notify_run_action in SimpleScheduler::do_try_match"
                    })
            };
            tokio::pin!(attach_operation_fut);

            let origin_metadata = maybe_origin_metadata.unwrap_or_default();

            let ctx = Context::current_with_baggage(vec![KeyValue::new(
                ENDUSER_ID,
                origin_metadata.identity,
            )]);

            info_span!("do_try_match")
                .in_scope(|| attach_operation_fut)
                .with_context(ctx)
                .await
        }

        let mut result = Ok(());

        let start = Instant::now();

        let mut stream = self
            .get_queued_operations()
            .await
            .err_tip(|| "Failed to get queued operations in do_try_match")?;

        let query_elapsed = start.elapsed();
        if query_elapsed > Duration::from_secs(1) {
            warn!(
                elapsed_ms = query_elapsed.as_millis(),
                "Slow get_queued_operations query"
            );
        }

        while let Some(action_state_result) = stream.next().await {
            result = result.merge(
                match_action_to_worker(
                    action_state_result.as_ref(),
                    self.worker_scheduler.as_ref(),
                    self.matching_engine_state_manager.as_ref(),
                    self.platform_property_manager.as_ref(),
                    full_worker_logging,
                )
                .await,
            );
        }

        let total_elapsed = start.elapsed();
        if total_elapsed > Duration::from_secs(5) {
            warn!(
                total_ms = total_elapsed.as_millis(),
                query_ms = query_elapsed.as_millis(),
                "Slow do_try_match cycle"
            );
        }

        result
    }
}

impl SimpleScheduler {
    pub fn new<A: AwaitedActionDb>(
        spec: &SimpleSpec,
        awaited_action_db: A,
        task_change_notify: Arc<Notify>,
        maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,
    ) -> (Arc<Self>, Arc<dyn WorkerScheduler>) {
        Self::new_with_callback(
            spec,
            awaited_action_db,
            || {
                // The cost of running `do_try_match()` is very high, but constant
                // in relation to the number of changes that have happened. This
                // means that grabbing this lock to process `do_try_match()` should
                // always yield to any other tasks that might want the lock. The
                // easiest and most fair way to do this is to sleep for a small
                // amount of time. Using something like tokio::task::yield_now()
                // does not yield as aggressively as we'd like if new futures are
                // scheduled within a future.
                tokio::time::sleep(Duration::from_millis(1))
            },
            task_change_notify,
            SystemTime::now,
            maybe_origin_event_tx,
        )
    }

    pub fn new_with_callback<
        Fut: Future<Output = ()> + Send,
        F: Fn() -> Fut + Send + Sync + 'static,
        A: AwaitedActionDb,
        I: InstantWrapper,
        NowFn: Fn() -> I + Clone + Send + Unpin + Sync + 'static,
    >(
        spec: &SimpleSpec,
        awaited_action_db: A,
        on_matching_engine_run: F,
        task_change_notify: Arc<Notify>,
        now_fn: NowFn,
        maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,
    ) -> (Arc<Self>, Arc<dyn WorkerScheduler>) {
        let platform_property_manager = Arc::new(PlatformPropertyManager::new(
            spec.supported_platform_properties
                .clone()
                .unwrap_or_default(),
        ));

        let mut worker_timeout_s = spec.worker_timeout_s;
        if worker_timeout_s == 0 {
            worker_timeout_s = DEFAULT_WORKER_TIMEOUT_S;
        }

        let mut client_action_timeout_s = spec.client_action_timeout_s;
        if client_action_timeout_s == 0 {
            client_action_timeout_s = DEFAULT_CLIENT_ACTION_TIMEOUT_S;
        }
        // This matches the value of CLIENT_KEEPALIVE_DURATION which means that
        // tasks are going to be dropped all over the place, this isn't a good
        // setting.
        if client_action_timeout_s <= CLIENT_KEEPALIVE_DURATION.as_secs() {
            error!(
                client_action_timeout_s,
                "Setting client_action_timeout_s to less than the client keep alive interval is going to cause issues, please set above {}.",
                CLIENT_KEEPALIVE_DURATION.as_secs()
            );
        }

        let mut max_job_retries = spec.max_job_retries;
        if max_job_retries == 0 {
            max_job_retries = DEFAULT_MAX_JOB_RETRIES;
        }

        let worker_change_notify = Arc::new(Notify::new());

        // Create shared worker registry for single heartbeat per worker.
        let worker_registry = Arc::new(WorkerRegistry::new());

        let state_manager = SimpleSchedulerStateManager::new(
            max_job_retries,
            Duration::from_secs(worker_timeout_s),
            Duration::from_secs(client_action_timeout_s),
            awaited_action_db,
            now_fn,
            Some(worker_registry.clone()),
        );

        let worker_scheduler = ApiWorkerScheduler::new(
            state_manager.clone(),
            platform_property_manager.clone(),
            spec.allocation_strategy,
            worker_change_notify.clone(),
            worker_timeout_s,
            worker_registry,
        );

        let worker_scheduler_clone = worker_scheduler.clone();

        let action_scheduler = Arc::new_cyclic(move |weak_self| -> Self {
            let weak_inner = weak_self.clone();
            let task_worker_matching_spawn =
                spawn!("simple_scheduler_task_worker_matching", async move {
                    let mut last_match_successful = true;
                    let mut worker_match_logging_last: Option<Instant> = None;
                    // Break out of the loop only when the inner is dropped.
                    loop {
                        let task_change_fut = task_change_notify.notified();
                        let worker_change_fut = worker_change_notify.notified();
                        tokio::pin!(task_change_fut);
                        tokio::pin!(worker_change_fut);
                        // Wait for either of these futures to be ready.
                        let state_changed = future::select(task_change_fut, worker_change_fut);
                        if last_match_successful {
                            let _ = state_changed.await;
                        } else {
                            // If the last match failed, then run again after a short sleep.
                            // This resolves issues where we tried to re-schedule a job to
                            // a disconnected worker.  The sleep ensures we don't enter a
                            // hard loop if there's something wrong inside do_try_match.
                            let sleep_fut = tokio::time::sleep(Duration::from_millis(100));
                            tokio::pin!(sleep_fut);
                            let _ = future::select(state_changed, sleep_fut).await;
                        }

                        let result = match weak_inner.upgrade() {
                            Some(scheduler) => {
                                let now = Instant::now();
                                let full_worker_logging = {
                                    match scheduler.worker_match_logging_interval {
                                        None => false,
                                        Some(duration) => match worker_match_logging_last {
                                            None => true,
                                            Some(when) => now.duration_since(when) >= duration,
                                        },
                                    }
                                };

                                let res = scheduler.do_try_match(full_worker_logging).await;
                                if full_worker_logging {
                                    let operations_stream = scheduler
                                        .matching_engine_state_manager
                                        .filter_operations(OperationFilter::default())
                                        .await
                                        .err_tip(|| "In action_scheduler getting filter result");

                                    let mut oldest_actions_in_state: HashMap<
                                        String,
                                        BTreeSet<Arc<ActionState>>,
                                    > = HashMap::new();
                                    let max_items = 5;

                                    match operations_stream {
                                        Ok(stream) => {
                                            let actions = stream
                                                .filter_map(|item| async move {
                                                    match item.as_ref().as_state().await {
                                                        Ok((action_state, _origin_metadata)) => {
                                                            Some(action_state)
                                                        }
                                                        Err(e) => {
                                                            error!(
                                                                ?e,
                                                                "Failed to get action state!"
                                                            );
                                                            None
                                                        }
                                                    }
                                                })
                                                .collect::<Vec<_>>()
                                                .await;
                                            for action_state in &actions {
                                                let name = action_state.stage.name();
                                                if let Some(values) =
                                                    oldest_actions_in_state.get_mut(&name)
                                                {
                                                    values.insert(action_state.clone());
                                                    if values.len() > max_items {
                                                        values.pop_first();
                                                    }
                                                } else {
                                                    let mut values = BTreeSet::new();
                                                    values.insert(action_state.clone());
                                                    oldest_actions_in_state.insert(name, values);
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            error!(?e, "Failed to get operations list!");
                                        }
                                    }

                                    for value in oldest_actions_in_state.values() {
                                        let mut items = vec![];
                                        for item in value {
                                            items.push(item.to_string());
                                        }
                                        info!(?items, "Oldest actions in state");
                                    }

                                    worker_match_logging_last.replace(now);
                                }
                                res
                            }
                            // If the inner went away it means the scheduler is shutting
                            // down, so we need to resolve our future.
                            None => return,
                        };
                        last_match_successful = result.is_ok();
                        if let Err(err) = result {
                            error!(?err, "Error while running do_try_match");
                        }

                        on_matching_engine_run().await;
                    }
                    // Unreachable.
                });

            let worker_match_logging_interval = match spec.worker_match_logging_interval_s {
                // -1 or 0 means disabled (0 used to cause expensive logging on every call)
                -1 | 0 => None,
                signed_secs => {
                    if let Ok(secs) = TryInto::<u64>::try_into(signed_secs) {
                        Some(Duration::from_secs(secs))
                    } else {
                        error!(
                            worker_match_logging_interval_s = spec.worker_match_logging_interval_s,
                            "Valid values for worker_match_logging_interval_s are -1, 0, or a positive integer, setting to disabled",
                        );
                        None
                    }
                }
            };
            Self {
                matching_engine_state_manager: state_manager.clone(),
                client_state_manager: state_manager.clone(),
                worker_scheduler,
                platform_property_manager,
                maybe_origin_event_tx,
                task_worker_matching_spawn,
                worker_match_logging_interval,
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
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        self.worker_scheduler
            .update_action(worker_id, operation_id, update)
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

    async fn shutdown(&self, shutdown_guard: ShutdownGuard) {
        self.worker_scheduler.shutdown(shutdown_guard).await;
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
