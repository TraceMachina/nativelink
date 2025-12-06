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

use core::ops::Bound;
use core::time::Duration;
use std::string::ToString;
use std::sync::{Arc, Weak};

use async_lock::Mutex;
use async_trait::async_trait;
use futures::{StreamExt, TryStreamExt, stream};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::action_messages::{
    ActionInfo, ActionResult, ActionStage, ActionState, ActionUniqueQualifier, ExecutionMetadata,
    OperationId, WorkerId,
};
use nativelink_util::metrics::{
    EXECUTION_METRICS, EXECUTION_RESULT, EXECUTION_STAGE, ExecutionResult, ExecutionStage,
};
use opentelemetry::KeyValue;
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, MatchingEngineStateManager,
    OperationFilter, OperationStageFlags, OrderDirection, UpdateOperationType, WorkerStateManager,
};
use nativelink_util::origin_event::OriginMetadata;
use tracing::{info, warn};

use super::awaited_action_db::{
    AwaitedAction, AwaitedActionDb, AwaitedActionSubscriber, SortedAwaitedActionState,
};

/// Maximum number of times an update to the database
/// can fail before giving up.
const MAX_UPDATE_RETRIES: usize = 5;

/// Simple struct that implements the `ActionStateResult` trait and always returns an error.
struct ErrorActionStateResult(Error);

#[async_trait]
impl ActionStateResult for ErrorActionStateResult {
    async fn as_state(&self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        Err(self.0.clone())
    }

    async fn changed(&mut self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        Err(self.0.clone())
    }

    async fn as_action_info(&self) -> Result<(Arc<ActionInfo>, Option<OriginMetadata>), Error> {
        Err(self.0.clone())
    }
}

struct ClientActionStateResult<U, T, I, NowFn>
where
    U: AwaitedActionSubscriber,
    T: AwaitedActionDb,
    I: InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Unpin + Sync + 'static,
{
    inner: MatchingEngineActionStateResult<U, T, I, NowFn>,
}

impl<U, T, I, NowFn> ClientActionStateResult<U, T, I, NowFn>
where
    U: AwaitedActionSubscriber,
    T: AwaitedActionDb,
    I: InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Unpin + Sync + 'static,
{
    const fn new(
        sub: U,
        simple_scheduler_state_manager: Weak<SimpleSchedulerStateManager<T, I, NowFn>>,
        no_event_action_timeout: Duration,
        now_fn: NowFn,
    ) -> Self {
        Self {
            inner: MatchingEngineActionStateResult::new(
                sub,
                simple_scheduler_state_manager,
                no_event_action_timeout,
                now_fn,
            ),
        }
    }
}

#[async_trait]
impl<U, T, I, NowFn> ActionStateResult for ClientActionStateResult<U, T, I, NowFn>
where
    U: AwaitedActionSubscriber,
    T: AwaitedActionDb,
    I: InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Unpin + Sync + 'static,
{
    async fn as_state(&self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        self.inner.as_state().await
    }

    async fn changed(&mut self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        self.inner.changed().await
    }

    async fn as_action_info(&self) -> Result<(Arc<ActionInfo>, Option<OriginMetadata>), Error> {
        self.inner.as_action_info().await
    }
}

struct MatchingEngineActionStateResult<U, T, I, NowFn>
where
    U: AwaitedActionSubscriber,
    T: AwaitedActionDb,
    I: InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Unpin + Sync + 'static,
{
    awaited_action_sub: U,
    simple_scheduler_state_manager: Weak<SimpleSchedulerStateManager<T, I, NowFn>>,
    no_event_action_timeout: Duration,
    now_fn: NowFn,
}
impl<U, T, I, NowFn> MatchingEngineActionStateResult<U, T, I, NowFn>
where
    U: AwaitedActionSubscriber,
    T: AwaitedActionDb,
    I: InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Unpin + Sync + 'static,
{
    const fn new(
        awaited_action_sub: U,
        simple_scheduler_state_manager: Weak<SimpleSchedulerStateManager<T, I, NowFn>>,
        no_event_action_timeout: Duration,
        now_fn: NowFn,
    ) -> Self {
        Self {
            awaited_action_sub,
            simple_scheduler_state_manager,
            no_event_action_timeout,
            now_fn,
        }
    }
}

#[async_trait]
impl<U, T, I, NowFn> ActionStateResult for MatchingEngineActionStateResult<U, T, I, NowFn>
where
    U: AwaitedActionSubscriber,
    T: AwaitedActionDb,
    I: InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Unpin + Sync + 'static,
{
    async fn as_state(&self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        let awaited_action = self
            .awaited_action_sub
            .borrow()
            .await
            .err_tip(|| "In MatchingEngineActionStateResult::as_state")?;
        Ok((
            awaited_action.state().clone(),
            awaited_action.maybe_origin_metadata().cloned(),
        ))
    }

    async fn changed(&mut self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        let mut timeout_attempts = 0;
        loop {
            tokio::select! {
                awaited_action_result = self.awaited_action_sub.changed() => {
                    return awaited_action_result
                        .err_tip(|| "In MatchingEngineActionStateResult::changed")
                        .map(|v| (v.state().clone(), v.maybe_origin_metadata().cloned()));
                }
                () = (self.now_fn)().sleep(self.no_event_action_timeout) => {
                    // Timeout happened, do additional checks below.
                }
            }

            let awaited_action = self
                .awaited_action_sub
                .borrow()
                .await
                .err_tip(|| "In MatchingEngineActionStateResult::changed")?;

            if matches!(awaited_action.state().stage, ActionStage::Queued) {
                // Actions in queued state do not get periodically updated,
                // so we don't need to timeout them.
                continue;
            }

            let simple_scheduler_state_manager = self
                .simple_scheduler_state_manager
                .upgrade()
                .err_tip(|| format!("Failed to upgrade weak reference to SimpleSchedulerStateManager in MatchingEngineActionStateResult::changed at attempt: {timeout_attempts}"))?;

            warn!(
                ?awaited_action,
                "OperationId {} / {} timed out after {} seconds issuing a retry",
                awaited_action.operation_id(),
                awaited_action.state().client_operation_id,
                self.no_event_action_timeout.as_secs_f32(),
            );

            simple_scheduler_state_manager
                .timeout_operation_id(awaited_action.operation_id())
                .await
                .err_tip(|| "In MatchingEngineActionStateResult::changed")?;

            if timeout_attempts >= MAX_UPDATE_RETRIES {
                return Err(make_err!(
                    Code::Internal,
                    "Failed to update action after {} retries with no error set in MatchingEngineActionStateResult::changed - {} {:?}",
                    MAX_UPDATE_RETRIES,
                    awaited_action.operation_id(),
                    awaited_action.state().stage,
                ));
            }
            timeout_attempts += 1;
        }
    }

    async fn as_action_info(&self) -> Result<(Arc<ActionInfo>, Option<OriginMetadata>), Error> {
        let awaited_action = self
            .awaited_action_sub
            .borrow()
            .await
            .err_tip(|| "In MatchingEngineActionStateResult::as_action_info")?;
        Ok((
            awaited_action.action_info().clone(),
            awaited_action.maybe_origin_metadata().cloned(),
        ))
    }
}

/// `SimpleSchedulerStateManager` is responsible for maintaining the state of the scheduler.
/// Scheduler state includes the actions that are queued, active, and recently completed.
/// It also includes the workers that are available to execute actions based on allocation
/// strategy.
#[derive(MetricsComponent, Debug)]
pub struct SimpleSchedulerStateManager<T, I, NowFn>
where
    T: AwaitedActionDb,
    I: InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Unpin + Sync + 'static,
{
    /// Database for storing the state of all actions.
    #[metric(group = "action_db")]
    action_db: T,

    /// Maximum number of times a job can be retried.
    // TODO(palfrey) This should be a scheduler decorator instead
    // of always having it on every SimpleScheduler.
    #[metric(help = "Maximum number of times a job can be retried")]
    max_job_retries: usize,

    /// Duration after which an action is considered to be timed out if
    /// no event is received.
    #[metric(
        help = "Duration after which an action is considered to be timed out if no event is received"
    )]
    no_event_action_timeout: Duration,

    /// Mark operation as timed out if the worker has not updated in this duration.
    /// This is used to prevent operations from being stuck in the queue forever
    /// if it is not being processed by any worker.
    client_action_timeout: Duration,

    // A lock to ensure only one timeout operation is running at a time
    // on this service.
    timeout_operation_mux: Mutex<()>,

    /// Weak reference to self.
    // We use a weak reference to reduce the risk of a memory leak from
    // future changes. If this becomes some kind of performance issue,
    // we can consider using a strong reference.
    weak_self: Weak<Self>,

    /// Function to get the current time.
    now_fn: NowFn,
}

impl<T, I, NowFn> SimpleSchedulerStateManager<T, I, NowFn>
where
    T: AwaitedActionDb,
    I: InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Unpin + Sync + 'static,
{
    pub fn new(
        max_job_retries: usize,
        no_event_action_timeout: Duration,
        client_action_timeout: Duration,
        action_db: T,
        now_fn: NowFn,
    ) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| Self {
            action_db,
            max_job_retries,
            no_event_action_timeout,
            client_action_timeout,
            timeout_operation_mux: Mutex::new(()),
            weak_self: weak_self.clone(),
            now_fn,
        })
    }

    async fn apply_filter_predicate(
        &self,
        awaited_action: &AwaitedAction,
        subscriber: &T::Subscriber,
        filter: &OperationFilter,
    ) -> bool {
        // Note: The caller must filter `client_operation_id`.

        let mut maybe_reloaded_awaited_action: Option<AwaitedAction> = None;
        let now = (self.now_fn)().now();
        if awaited_action.last_client_keepalive_timestamp() + self.client_action_timeout < now {
            // This may change if the version is out of date.
            let mut timed_out = true;
            if !awaited_action.state().stage.is_finished() {
                let mut state = awaited_action.state().as_ref().clone();
                state.stage = ActionStage::Completed(ActionResult {
                    error: Some(make_err!(
                        Code::DeadlineExceeded,
                        "Operation timed out {} seconds of having no more clients listening",
                        self.client_action_timeout.as_secs_f32(),
                    )),
                    ..ActionResult::default()
                });
                state.last_transition_timestamp = now;
                let state = Arc::new(state);
                // We may be competing with an client timestamp update, so try
                // this a few times.
                for attempt in 1..=MAX_UPDATE_RETRIES {
                    let mut new_awaited_action = match &maybe_reloaded_awaited_action {
                        None => awaited_action.clone(),
                        Some(reloaded_awaited_action) => reloaded_awaited_action.clone(),
                    };
                    new_awaited_action.worker_set_state(state.clone(), (self.now_fn)().now());
                    let err = match self
                        .action_db
                        .update_awaited_action(new_awaited_action)
                        .await
                    {
                        Ok(()) => break,
                        Err(err) => err,
                    };
                    // Reload from the database if the action was outdated.
                    let maybe_awaited_action =
                        if attempt == MAX_UPDATE_RETRIES || err.code != Code::Aborted {
                            None
                        } else {
                            subscriber.borrow().await.ok()
                        };
                    if let Some(reloaded_awaited_action) = maybe_awaited_action {
                        maybe_reloaded_awaited_action = Some(reloaded_awaited_action);
                    } else {
                        warn!(
                            "Failed to update action to timed out state after client keepalive timeout. This is ok if multiple schedulers tried to set the state at the same time: {err}",
                        );
                        break;
                    }
                    // Re-check the predicate after reload.
                    if maybe_reloaded_awaited_action
                        .as_ref()
                        .is_some_and(|awaited_action| {
                            awaited_action.last_client_keepalive_timestamp()
                                + self.client_action_timeout
                                >= (self.now_fn)().now()
                        })
                    {
                        timed_out = false;
                        break;
                    } else if maybe_reloaded_awaited_action
                        .as_ref()
                        .is_some_and(|awaited_action| awaited_action.state().stage.is_finished())
                    {
                        break;
                    }
                }
            }
            if timed_out {
                return false;
            }
        }
        // If the action was reloaded, then use that for the rest of the checks
        // instead of the input parameter.
        let awaited_action = maybe_reloaded_awaited_action
            .as_ref()
            .unwrap_or(awaited_action);

        if let Some(operation_id) = &filter.operation_id {
            if operation_id != awaited_action.operation_id() {
                return false;
            }
        }

        if filter.worker_id.is_some() && filter.worker_id.as_ref() != awaited_action.worker_id() {
            return false;
        }

        {
            if let Some(filter_unique_key) = &filter.unique_key {
                match &awaited_action.action_info().unique_qualifier {
                    ActionUniqueQualifier::Cacheable(unique_key) => {
                        if filter_unique_key != unique_key {
                            return false;
                        }
                    }
                    ActionUniqueQualifier::Uncacheable(_) => {
                        return false;
                    }
                }
            }
            if let Some(action_digest) = filter.action_digest {
                if action_digest != awaited_action.action_info().digest() {
                    return false;
                }
            }
        }

        {
            let last_worker_update_timestamp = awaited_action.last_worker_updated_timestamp();
            if let Some(worker_update_before) = filter.worker_update_before {
                if worker_update_before < last_worker_update_timestamp {
                    return false;
                }
            }
            if let Some(completed_before) = filter.completed_before {
                if awaited_action.state().stage.is_finished()
                    && completed_before < last_worker_update_timestamp
                {
                    return false;
                }
            }
            if filter.stages != OperationStageFlags::Any {
                let stage_flag = match awaited_action.state().stage {
                    ActionStage::Unknown => OperationStageFlags::Any,
                    ActionStage::CacheCheck => OperationStageFlags::CacheCheck,
                    ActionStage::Queued => OperationStageFlags::Queued,
                    ActionStage::Executing => OperationStageFlags::Executing,
                    ActionStage::Completed(_) | ActionStage::CompletedFromCache(_) => {
                        OperationStageFlags::Completed
                    }
                };
                if !filter.stages.intersects(stage_flag) {
                    return false;
                }
            }
        }

        true
    }

    /// Let the scheduler know that an operation has timed out from
    /// the client side (ie: worker has not updated in a while).
    async fn timeout_operation_id(&self, operation_id: &OperationId) -> Result<(), Error> {
        // Ensure that only one timeout operation is running at a time.
        // Failing to do this could result in the same operation being
        // timed out multiple times at the same time.
        // Note: We could implement this on a per-operation_id basis, but it is quite
        // complex to manage the locks.
        let _lock = self.timeout_operation_mux.lock().await;

        let awaited_action_subscriber = self
            .action_db
            .get_by_operation_id(operation_id)
            .await
            .err_tip(|| "In SimpleSchedulerStateManager::timeout_operation_id")?
            .err_tip(|| {
                format!("Operation id {operation_id} does not exist in SimpleSchedulerStateManager::timeout_operation_id")
            })?;

        let awaited_action = awaited_action_subscriber
            .borrow()
            .await
            .err_tip(|| "In SimpleSchedulerStateManager::timeout_operation_id")?;

        // If the action is not executing, we should not timeout the action.
        if !matches!(awaited_action.state().stage, ActionStage::Executing) {
            return Ok(());
        }

        let worker_should_update_before = awaited_action
            .last_worker_updated_timestamp()
            .checked_add(self.no_event_action_timeout)
            .ok_or_else(|| {
                make_err!(
                    Code::Internal,
                    "Timestamp overflow for operation {operation_id} in SimpleSchedulerStateManager::timeout_operation_id"
                )
            })?;
        if worker_should_update_before >= (self.now_fn)().now() {
            // The action was updated recently, we should not timeout the action.
            // This is to prevent timing out actions that have recently been updated
            // (like multiple clients timeout the same action at the same time).
            return Ok(());
        }

        self.assign_operation(
            operation_id,
            Err(make_err!(
                Code::DeadlineExceeded,
                "Operation timed out after {} seconds",
                self.no_event_action_timeout.as_secs_f32(),
            )),
        )
        .await
    }

    async fn inner_update_operation(
        &self,
        operation_id: &OperationId,
        maybe_worker_id: Option<&WorkerId>,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        let mut last_err = None;
        for _ in 0..MAX_UPDATE_RETRIES {
            let maybe_awaited_action_subscriber = self
                .action_db
                .get_by_operation_id(operation_id)
                .await
                .err_tip(|| "In SimpleSchedulerStateManager::update_operation")?;
            let Some(awaited_action_subscriber) = maybe_awaited_action_subscriber else {
                // No action found. It is ok if the action was not found. It
                // probably means that the action was dropped, but worker was
                // still processing it.
                warn!(
                    %operation_id,
                    "Unable to update action due to it being missing, probably dropped"
                );
                return Ok(());
            };

            let mut awaited_action = awaited_action_subscriber
                .borrow()
                .await
                .err_tip(|| "In SimpleSchedulerStateManager::update_operation")?;

            // Make sure the worker id matches the awaited action worker id.
            // This might happen if the worker sending the update is not the
            // worker that was assigned.
            if awaited_action.worker_id().is_some()
                && maybe_worker_id.is_some()
                && maybe_worker_id != awaited_action.worker_id()
            {
                // If another worker is already assigned to the action, another
                // worker probably picked up the action. We should not update the
                // action in this case and abort this operation.
                let err = make_err!(
                    Code::Aborted,
                    "Worker ids do not match - {:?} != {:?} for {:?}",
                    maybe_worker_id,
                    awaited_action.worker_id(),
                    awaited_action,
                );
                info!(
                    "Worker ids do not match - {:?} != {:?} for {:?}. This is probably due to another worker picking up the action.",
                    maybe_worker_id,
                    awaited_action.worker_id(),
                    awaited_action,
                );
                return Err(err);
            }

            // Make sure we don't update an action that is already completed.
            if awaited_action.state().stage.is_finished() {
                match &update {
                    UpdateOperationType::UpdateWithDisconnect | UpdateOperationType::KeepAlive => {
                        // No need to error a keep-alive when it's completed, it's just
                        // unnecessary log noise.
                        return Ok(());
                    }
                    _ => {
                        return Err(make_err!(
                            Code::Internal,
                            "Action {operation_id} is already completed with state {:?} - maybe_worker_id: {:?}",
                            awaited_action.state().stage,
                            maybe_worker_id,
                        ));
                    }
                }
            }

            let stage = match &update {
                UpdateOperationType::KeepAlive => {
                    awaited_action.worker_keep_alive((self.now_fn)().now());
                    match self
                        .action_db
                        .update_awaited_action(awaited_action)
                        .await
                        .err_tip(|| "Failed to send KeepAlive in SimpleSchedulerStateManager::update_operation") {
                        // Try again if there was a version mismatch.
                        Err(err) if err.code == Code::Aborted => {
                            last_err = Some(err);
                            continue;
                        }
                        result => return result,
                    }
                }
                UpdateOperationType::UpdateWithActionStage(stage) => stage.clone(),
                UpdateOperationType::UpdateWithError(err) => {
                    // Don't count a backpressure failure as an attempt for an action.
                    let due_to_backpressure = err.code == Code::ResourceExhausted;
                    if !due_to_backpressure {
                        awaited_action.attempts += 1;
                    }

                    if awaited_action.attempts > self.max_job_retries {
                        ActionStage::Completed(ActionResult {
                            execution_metadata: ExecutionMetadata {
                                worker: maybe_worker_id.map_or_else(String::default, ToString::to_string),
                                ..ExecutionMetadata::default()
                            },
                            error: Some(err.clone().merge(make_err!(
                                Code::Internal,
                                "Job cancelled because it attempted to execute too many times {} > {} times {}",
                                awaited_action.attempts,
                                self.max_job_retries,
                                format!("for operation_id: {operation_id}, maybe_worker_id: {maybe_worker_id:?}"),
                            ))),
                            ..ActionResult::default()
                        })
                    } else {
                        ActionStage::Queued
                    }
                }
                UpdateOperationType::UpdateWithDisconnect => ActionStage::Queued,
                // We shouldn't get here, but we just ignore it if we do.
                UpdateOperationType::ExecutionComplete => {
                    warn!("inner_update_operation got an ExecutionComplete, that's unexpected.");
                    return Ok(());
                }
            };
            let now = (self.now_fn)().now();
            if matches!(stage, ActionStage::Queued) {
                // If the action is queued, we need to unset the worker id regardless of
                // which worker sent the update.
                awaited_action.set_worker_id(None, now);
            } else {
                awaited_action.set_worker_id(maybe_worker_id.cloned(), now);
            }
            awaited_action.worker_set_state(
                Arc::new(ActionState {
                    stage,
                    // Client id is not known here, it is the responsibility of
                    // the the subscriber impl to replace this with the
                    // correct client id.
                    client_operation_id: operation_id.clone(),
                    action_digest: awaited_action.action_info().digest(),
                    last_transition_timestamp: now,
                }),
                now,
            );

            let update_action_result = self
                .action_db
                .update_awaited_action(awaited_action.clone())
                .await
                .err_tip(|| "In SimpleSchedulerStateManager::update_operation");
            if let Err(err) = update_action_result {
                // We use Aborted to signal that the action was not
                // updated due to the data being set was not the latest
                // but can be retried.
                if err.code == Code::Aborted {
                    last_err = Some(err);
                    continue;
                }
                return Err(err);
            }

            // Record execution metrics after successful state update
            let action_state = awaited_action.state();
            let instance_name = awaited_action
                .action_info()
                .unique_qualifier
                .instance_name()
                .as_str();
            let worker_id = awaited_action.worker_id().map(|w| w.to_string());
            let priority = Some(awaited_action.action_info().priority);

            // Build base attributes for metrics
            let mut attrs = nativelink_util::metrics::make_execution_attributes(
                instance_name,
                worker_id.as_deref(),
                priority,
            );

            // Add stage attribute
            let execution_stage: ExecutionStage = (&action_state.stage).into();
            attrs.push(KeyValue::new(EXECUTION_STAGE, execution_stage));

            // Record stage transition
            EXECUTION_METRICS.execution_stage_transitions.add(1, &attrs);

            // For completed actions, record the completion count with result
            match &action_state.stage {
                ActionStage::Completed(action_result) => {
                    let result = if action_result.exit_code == 0 {
                        ExecutionResult::Success
                    } else {
                        ExecutionResult::Failure
                    };
                    attrs.push(KeyValue::new(EXECUTION_RESULT, result));
                    EXECUTION_METRICS.execution_completed_count.add(1, &attrs);
                }
                ActionStage::CompletedFromCache(_) => {
                    attrs.push(KeyValue::new(EXECUTION_RESULT, ExecutionResult::CacheHit));
                    EXECUTION_METRICS.execution_completed_count.add(1, &attrs);
                }
                _ => {}
            }

            return Ok(());
        }
        Err(last_err.unwrap_or_else(|| {
            make_err!(
                Code::Internal,
                "Failed to update action after {} retries with no error set",
                MAX_UPDATE_RETRIES,
            )
        }))
    }

    async fn inner_add_operation(
        &self,
        new_client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<T::Subscriber, Error> {
        self.action_db
            .add_action(
                new_client_operation_id,
                action_info,
                self.no_event_action_timeout,
            )
            .await
            .err_tip(|| "In SimpleSchedulerStateManager::add_operation")
    }

    async fn inner_filter_operations<'a, F>(
        &'a self,
        filter: OperationFilter,
        to_action_state_result: F,
    ) -> Result<ActionStateResultStream<'a>, Error>
    where
        F: Fn(T::Subscriber) -> Box<dyn ActionStateResult> + Send + Sync + 'a,
    {
        const fn sorted_awaited_action_state_for_flags(
            stage: OperationStageFlags,
        ) -> Option<SortedAwaitedActionState> {
            match stage {
                OperationStageFlags::CacheCheck => Some(SortedAwaitedActionState::CacheCheck),
                OperationStageFlags::Queued => Some(SortedAwaitedActionState::Queued),
                OperationStageFlags::Executing => Some(SortedAwaitedActionState::Executing),
                OperationStageFlags::Completed => Some(SortedAwaitedActionState::Completed),
                _ => None,
            }
        }

        if let Some(operation_id) = &filter.operation_id {
            let maybe_subscriber = self
                .action_db
                .get_by_operation_id(operation_id)
                .await
                .err_tip(|| "In SimpleSchedulerStateManager::filter_operations")?;
            let Some(subscriber) = maybe_subscriber else {
                return Ok(Box::pin(stream::empty()));
            };
            let awaited_action = subscriber
                .borrow()
                .await
                .err_tip(|| "In SimpleSchedulerStateManager::filter_operations")?;
            if !self
                .apply_filter_predicate(&awaited_action, &subscriber, &filter)
                .await
            {
                return Ok(Box::pin(stream::empty()));
            }
            return Ok(Box::pin(stream::once(async move {
                to_action_state_result(subscriber)
            })));
        }
        if let Some(client_operation_id) = &filter.client_operation_id {
            let maybe_subscriber = self
                .action_db
                .get_awaited_action_by_id(client_operation_id)
                .await
                .err_tip(|| "In SimpleSchedulerStateManager::filter_operations")?;
            let Some(subscriber) = maybe_subscriber else {
                return Ok(Box::pin(stream::empty()));
            };
            let awaited_action = subscriber
                .borrow()
                .await
                .err_tip(|| "In SimpleSchedulerStateManager::filter_operations")?;
            if !self
                .apply_filter_predicate(&awaited_action, &subscriber, &filter)
                .await
            {
                return Ok(Box::pin(stream::empty()));
            }
            return Ok(Box::pin(stream::once(async move {
                to_action_state_result(subscriber)
            })));
        }

        let Some(sorted_awaited_action_state) =
            sorted_awaited_action_state_for_flags(filter.stages)
        else {
            let mut all_items: Vec<_> = self
                .action_db
                .get_all_awaited_actions()
                .await
                .err_tip(|| "In SimpleSchedulerStateManager::filter_operations")?
                .and_then(|awaited_action_subscriber| async move {
                    let awaited_action = awaited_action_subscriber
                        .borrow()
                        .await
                        .err_tip(|| "In SimpleSchedulerStateManager::filter_operations")?;
                    Ok((awaited_action_subscriber, awaited_action))
                })
                .try_filter_map(|(subscriber, awaited_action)| {
                    let filter = filter.clone();
                    async move {
                        Ok(self
                            .apply_filter_predicate(&awaited_action, &subscriber, &filter)
                            .await
                            .then_some((subscriber, awaited_action.sort_key())))
                    }
                })
                .try_collect()
                .await
                .err_tip(|| "In SimpleSchedulerStateManager::filter_operations")?;
            match filter.order_by_priority_direction {
                Some(OrderDirection::Asc) => all_items.sort_unstable_by(|(_, a), (_, b)| a.cmp(b)),
                Some(OrderDirection::Desc) => all_items.sort_unstable_by(|(_, a), (_, b)| b.cmp(a)),
                None => {}
            }
            return Ok(Box::pin(stream::iter(
                all_items
                    .into_iter()
                    .map(move |(subscriber, _)| to_action_state_result(subscriber)),
            )));
        };

        let desc = matches!(
            filter.order_by_priority_direction,
            Some(OrderDirection::Desc)
        );
        let stream = self
            .action_db
            .get_range_of_actions(
                sorted_awaited_action_state,
                Bound::Unbounded,
                Bound::Unbounded,
                desc,
            )
            .await
            .err_tip(|| "In SimpleSchedulerStateManager::filter_operations")?
            .and_then(|awaited_action_subscriber| async move {
                let awaited_action = awaited_action_subscriber
                    .borrow()
                    .await
                    .err_tip(|| "In SimpleSchedulerStateManager::filter_operations")?;
                Ok((awaited_action_subscriber, awaited_action))
            })
            .try_filter_map(move |(subscriber, awaited_action)| {
                let filter = filter.clone();
                async move {
                    Ok(self
                        .apply_filter_predicate(&awaited_action, &subscriber, &filter)
                        .await
                        .then_some(subscriber))
                }
            })
            .map(move |result| -> Box<dyn ActionStateResult> {
                result.map_or_else(
                    |e| -> Box<dyn ActionStateResult> { Box::new(ErrorActionStateResult(e)) },
                    |v| -> Box<dyn ActionStateResult> { to_action_state_result(v) },
                )
            });
        Ok(Box::pin(stream))
    }
}

#[async_trait]
impl<T, I, NowFn> ClientStateManager for SimpleSchedulerStateManager<T, I, NowFn>
where
    T: AwaitedActionDb,
    I: InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Unpin + Sync + 'static,
{
    async fn add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        let sub = self
            .inner_add_operation(client_operation_id, action_info.clone())
            .await?;

        Ok(Box::new(ClientActionStateResult::new(
            sub,
            self.weak_self.clone(),
            self.no_event_action_timeout,
            self.now_fn.clone(),
        )))
    }

    async fn filter_operations<'a>(
        &'a self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream<'a>, Error> {
        self.inner_filter_operations(filter, move |rx| {
            Box::new(ClientActionStateResult::new(
                rx,
                self.weak_self.clone(),
                self.no_event_action_timeout,
                self.now_fn.clone(),
            ))
        })
        .await
    }

    fn as_known_platform_property_provider(&self) -> Option<&dyn KnownPlatformPropertyProvider> {
        None
    }
}

#[async_trait]
impl<T, I, NowFn> WorkerStateManager for SimpleSchedulerStateManager<T, I, NowFn>
where
    T: AwaitedActionDb,
    I: InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Unpin + Sync + 'static,
{
    async fn update_operation(
        &self,
        operation_id: &OperationId,
        worker_id: &WorkerId,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        self.inner_update_operation(operation_id, Some(worker_id), update)
            .await
    }
}

#[async_trait]
impl<T, I, NowFn> MatchingEngineStateManager for SimpleSchedulerStateManager<T, I, NowFn>
where
    T: AwaitedActionDb,
    I: InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Unpin + Sync + 'static,
{
    async fn filter_operations<'a>(
        &'a self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream<'a>, Error> {
        self.inner_filter_operations(filter, |rx| {
            Box::new(MatchingEngineActionStateResult::new(
                rx,
                self.weak_self.clone(),
                self.no_event_action_timeout,
                self.now_fn.clone(),
            ))
        })
        .await
    }

    async fn assign_operation(
        &self,
        operation_id: &OperationId,
        worker_id_or_reason_for_unassign: Result<&WorkerId, Error>,
    ) -> Result<(), Error> {
        let (maybe_worker_id, update) = match worker_id_or_reason_for_unassign {
            Ok(worker_id) => (
                Some(worker_id),
                UpdateOperationType::UpdateWithActionStage(ActionStage::Executing),
            ),
            Err(err) => (None, UpdateOperationType::UpdateWithError(err)),
        };
        self.inner_update_operation(operation_id, maybe_worker_id, update)
            .await
    }
}
