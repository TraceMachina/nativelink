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
use std::time::SystemTime;

use async_trait::async_trait;
use futures::{Future, FutureExt};
use nativelink_config::schedulers::SimpleSpec;
use nativelink_error::{Code, Error, ResultExt};
use nativelink_proto::com::github::trace_machina::nativelink::events::OriginEvent;
use nativelink_util::action_messages::{ActionInfo, ActionState, OperationId, WorkerId};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, MatchingEngineStateManager,
    OperationFilter, OperationStageFlags, OrderDirection, UpdateOperationType,
};
use nativelink_util::origin_event::OriginMetadata;
use nativelink_util::spawn;
use nativelink_util::task::JoinHandleDropGuard;
use opentelemetry::baggage::BaggageExt;
use opentelemetry::context::{Context, FutureExt as OtelFutureExt};
use opentelemetry::{InstrumentationScope, KeyValue, global, metrics};
use opentelemetry_semantic_conventions::attribute::ENDUSER_ID;
use tokio::sync::{Notify, mpsc};
use tokio::time::Duration;
use tokio_stream::StreamExt;
use tracing::{debug, error, info, info_span, instrument, warn};

use crate::api_worker_scheduler::ApiWorkerScheduler;
use crate::awaited_action_db::{AwaitedActionDb, CLIENT_KEEPALIVE_DURATION};
use crate::platform_property_manager::PlatformPropertyManager;
use crate::simple_scheduler_state_manager::SimpleSchedulerStateManager;
use crate::worker::{ActionInfoWithProps, Worker, WorkerTimestamp};
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

fn init_metrics() -> SimpleSchedulerMetrics {
    let meter = global::meter_with_scope(InstrumentationScope::builder("simple_scheduler").build());

    SimpleSchedulerMetrics {
        matching_attempts: meter
            .u64_counter("scheduler_matching_attempts")
            .with_description("Number of attempts to match actions to workers")
            .build(),
        successful_matches: meter
            .u64_counter("scheduler_successful_matches")
            .with_description("Number of successful action to worker matches")
            .build(),
    }
}

#[derive(Debug)]
struct SimpleSchedulerMetrics {
    matching_attempts: metrics::Counter<u64>,
    successful_matches: metrics::Counter<u64>,
}

/// Engine used to manage the queued/running tasks and relationship with
/// the worker nodes. All state on how the workers and actions are interacting
/// should be held in this struct.
pub struct SimpleScheduler {
    /// Manager for matching engine side of the state manager.
    matching_engine_state_manager: Arc<dyn MatchingEngineStateManager>,

    /// Manager for client state of this scheduler.
    client_state_manager: Arc<dyn ClientStateManager>,

    /// Manager for platform of this scheduler.
    platform_property_manager: Arc<PlatformPropertyManager>,

    /// A `Workers` pool that contains all workers that are available to execute actions in a priority
    /// order based on the allocation strategy.
    worker_scheduler: Arc<ApiWorkerScheduler>,

    /// The sender to send origin events to the origin events.
    maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,

    /// Background task that tries to match actions to workers. If this struct
    /// is dropped the spawn will be cancelled as well.
    _task_worker_matching_spawn: JoinHandleDropGuard<()>,

    metrics: SimpleSchedulerMetrics,
}

impl core::fmt::Debug for SimpleScheduler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SimpleScheduler")
            .field("platform_property_manager", &self.platform_property_manager)
            .field("worker_scheduler", &self.worker_scheduler)
            .field("maybe_origin_event_tx", &self.maybe_origin_event_tx)
            .field(
                "_task_worker_matching_spawn",
                &self._task_worker_matching_spawn,
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
    #[instrument(
        skip(self, action_info),
        fields(client_op_id = ?client_operation_id),
    )]
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

        debug!("Action added");

        Ok(Box::new(SimpleSchedulerActionStateResult::new(
            client_operation_id.clone(),
            action_state_result,
        )))
    }

    #[instrument(skip(self))]
    async fn inner_filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        debug!(?filter, "Filtering operations");
        self.client_state_manager
            .filter_operations(filter)
            .await
            .err_tip(|| "In SimpleScheduler::find_by_client_operation_id getting filter result")
    }

    #[instrument(skip(self))]
    async fn get_queued_operations(&self) -> Result<ActionStateResultStream, Error> {
        let filter = OperationFilter {
            stages: OperationStageFlags::Queued,
            order_by_priority_direction: Some(OrderDirection::Desc),
            ..Default::default()
        };

        debug!("Getting queued operations");

        self.matching_engine_state_manager
            .filter_operations(filter)
            .await
            .err_tip(|| "In SimpleScheduler::get_queued_operations getting filter result")
    }

    #[instrument(skip(self))]
    pub async fn do_try_match_for_test(&self) -> Result<(), Error> {
        self.do_try_match().await
    }

    // TODO(blaise.bruer) This is an O(n*m) (aka n^2) algorithm. In theory we
    // can create a map of capabilities of each worker and then try and match
    // the actions to the worker using the map lookup (ie. map reduce).
    #[instrument(skip(self))]
    async fn do_try_match(&self) -> Result<(), Error> {
        async fn match_action_to_worker(
            action_state_result: &dyn ActionStateResult,
            workers: &ApiWorkerScheduler,
            matching_engine_state_manager: &dyn MatchingEngineStateManager,
            platform_property_manager: &PlatformPropertyManager,
            maybe_origin_event_tx: Option<&mpsc::Sender<OriginEvent>>,
        ) -> Result<(), Error> {
            let (action_info, maybe_origin_metadata) =
                action_state_result
                    .as_action_info()
                    .await
                    .err_tip(|| "Failed to get action_info from as_action_info_result stream")?;

            debug!(action_id = ?action_info.unique_qualifier.digest(), "Processing action for matching");

            // TODO(allada) We should not compute this every time and instead store
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
                    .find_worker_for_action(&action_info.platform_properties)
                    .await
                {
                    Some(worker_id) => {
                        debug!(
                            worker_id = ?worker_id,
                            action_id = ?action_info.inner.unique_qualifier.digest(),
                            "Found worker for action"
                        );
                        worker_id
                    }
                    // If we could not find a worker for the action,
                    // we have nothing to do.
                    None => {
                        warn!(
                            action_id = ?action_info.inner.unique_qualifier.digest(),
                            "No worker found for action"
                        );
                        return Ok(());
                    }
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

                debug!(
                    worker_id = ?worker_id,
                    operation_id = ?operation_id,
                    "Assigning operation to worker"
                );

                // Tell the matching engine that the operation is being assigned to a worker.
                let assign_result = matching_engine_state_manager
                    .assign_operation(&operation_id, Ok(&worker_id))
                    .await
                    .err_tip(|| "Failed to assign operation in do_try_match");
                if let Err(err) = assign_result {
                    if err.code == Code::Aborted {
                        // If the operation was aborted, it means that the operation was
                        // cancelled due to another operation being assigned to the worker.
                        debug!(
                            operation_id = ?operation_id,
                            worker_id = ?worker_id,
                            "Operation assignment aborted"
                        );
                        return Ok(());
                    }
                    // Any other error is a real error.
                    return Err(err);
                }

                info!(
                    operation_id = ?operation_id,
                    worker_id = ?worker_id,
                    "Operation assigned to worker, notifying worker"
                );

                workers
                    .worker_notify_run_action(worker_id, operation_id, action_info)
                    .await
                    .err_tip(|| {
                        "Failed to run worker_notify_run_action in SimpleScheduler::do_try_match"
                    })
            };
            tokio::pin!(attach_operation_fut);

            let attach_operation_fut = if let Some(_origin_event_tx) = maybe_origin_event_tx {
                let origin_metadata = maybe_origin_metadata.unwrap_or_default();

                let ctx = Context::current_with_baggage(vec![KeyValue::new(
                    ENDUSER_ID,
                    origin_metadata.identity,
                )]);

                info_span!("do_try_match")
                    .in_scope(|| attach_operation_fut)
                    .with_context(ctx)
                    .left_future()
            } else {
                attach_operation_fut.right_future()
            };
            attach_operation_fut.await
        }

        debug!("Attempting to match action to worker");

        let mut result = Ok(());

        let mut stream = self
            .get_queued_operations()
            .await
            .err_tip(|| "Failed to get queued operations in do_try_match")?;

        let mut processed_count = 0;
        let mut matched_count = 0;

        while let Some(action_state_result) = stream.next().await {
            processed_count += 1;

            let match_result = match_action_to_worker(
                action_state_result.as_ref(),
                self.worker_scheduler.as_ref(),
                self.matching_engine_state_manager.as_ref(),
                self.platform_property_manager.as_ref(),
                self.maybe_origin_event_tx.as_ref(),
            )
            .await;

            if match_result.is_ok() {
                matched_count += 1;
            }
            result = result.merge(match_result);
        }

        self.metrics.matching_attempts.add(processed_count, &[]);
        self.metrics.successful_matches.add(matched_count, &[]);

        debug!(matched_count, "Matching attempt completed");
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
                // does not yield as aggresively as we'd like if new futures are
                // scheduled within a future.
                tokio::time::sleep(Duration::from_millis(1))
            },
            task_change_notify,
            SystemTime::now,
            maybe_origin_event_tx,
        )
    }

    #[instrument(
        skip_all,
        fields(worker_timeout_s, client_action_timeout_s, max_job_retries)
    )]
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

        info!(
            worker_timeout_s,
            client_action_timeout_s, max_job_retries, "Initializing SimpleScheduler"
        );

        let worker_change_notify = Arc::new(Notify::new());
        let state_manager = SimpleSchedulerStateManager::new(
            max_job_retries,
            Duration::from_secs(worker_timeout_s),
            Duration::from_secs(client_action_timeout_s),
            awaited_action_db,
            now_fn,
        );

        let worker_scheduler = ApiWorkerScheduler::new(
            state_manager.clone(),
            platform_property_manager.clone(),
            spec.allocation_strategy,
            worker_change_notify.clone(),
            worker_timeout_s,
        );

        let worker_scheduler_clone = worker_scheduler.clone();

        let action_scheduler = Arc::new_cyclic(move |weak_self| -> Self {
            let weak_inner = weak_self.clone();
            let task_worker_matching_spawn =
                spawn!("simple_scheduler_task_worker_matching", async move {
                    info!("Starting worker matching task");

                    // Break out of the loop only when the inner is dropped.
                    loop {
                        let task_change_fut = task_change_notify.notified();
                        let worker_change_fut = worker_change_notify.notified();
                        tokio::pin!(task_change_fut);
                        tokio::pin!(worker_change_fut);
                        // Wait for either of these futures to be ready.
                        let _ = futures::future::select(task_change_fut, worker_change_fut).await;
                        debug!("Task or worker change notification received");

                        let result = match weak_inner.upgrade() {
                            Some(scheduler) => scheduler.do_try_match().await,
                            // If the inner went away it means the scheduler is shutting
                            // down, so we need to resolve our future.
                            None => {
                                info!("Scheduler dropped, exiting worker matching task");
                                return;
                            }
                        };
                        if let Err(err) = result {
                            error!(?err, "Error while running do_try_match");
                        }

                        on_matching_engine_run().await;
                    }
                    // Unreachable.
                });

            info!("SimpleScheduler initialized");

            SimpleScheduler {
                matching_engine_state_manager: state_manager.clone(),
                client_state_manager: state_manager.clone(),
                worker_scheduler,
                platform_property_manager,
                maybe_origin_event_tx,
                _task_worker_matching_spawn: task_worker_matching_spawn,
                metrics: init_metrics(),
            }
        });
        (action_scheduler, worker_scheduler_clone)
    }
}

#[async_trait]
impl ClientStateManager for SimpleScheduler {
    #[instrument(
        skip(self, action_info),
        fields(client_op_id = ?client_operation_id),
    )]
    async fn add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        self.inner_add_action(client_operation_id, action_info)
            .await
    }

    #[instrument(skip(self))]
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
    #[instrument(skip(self))]
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

    #[instrument(
        skip(self, worker),
        fields(worker_id = ?worker.id),
    )]
    async fn add_worker(&self, worker: Worker) -> Result<(), Error> {
        info!("Adding worker to scheduler");
        self.worker_scheduler.add_worker(worker).await
    }

    #[instrument(
        skip(self),
        fields(worker_id = ?worker_id, operation_id = ?operation_id),
    )]
    async fn update_action(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        debug!(?update, "Updating action");
        self.worker_scheduler
            .update_action(worker_id, operation_id, update)
            .await
    }

    #[instrument(
        skip(self),
        fields(worker_id = ?worker_id),
    )]
    async fn worker_keep_alive_received(
        &self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
    ) -> Result<(), Error> {
        debug!("Received worker keep-alive");
        self.worker_scheduler
            .worker_keep_alive_received(worker_id, timestamp)
            .await
    }

    #[instrument(
        skip(self),
        fields(worker_id = ?worker_id),
    )]
    async fn remove_worker(&self, worker_id: &WorkerId) -> Result<(), Error> {
        info!("Removing worker from scheduler");
        self.worker_scheduler.remove_worker(worker_id).await
    }

    #[instrument(skip(self))]
    async fn remove_timedout_workers(&self, now_timestamp: WorkerTimestamp) -> Result<(), Error> {
        debug!("Removing timed out workers");
        self.worker_scheduler
            .remove_timedout_workers(now_timestamp)
            .await
    }

    #[instrument(
        skip(self),
        fields(worker_id = ?worker_id, is_draining),
    )]
    async fn set_drain_worker(&self, worker_id: &WorkerId, is_draining: bool) -> Result<(), Error> {
        info!("Setting worker drain state");
        self.worker_scheduler
            .set_drain_worker(worker_id, is_draining)
            .await
    }
}
