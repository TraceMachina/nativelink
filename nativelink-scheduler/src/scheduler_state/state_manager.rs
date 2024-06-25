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

use std::cmp;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use futures::stream;
use hashbrown::{HashMap, HashSet};
use nativelink_error::{make_err, make_input_err, Code, Error, ResultExt};
use nativelink_util::action_messages::{
    ActionInfo, ActionInfoHashKey, ActionResult, ActionStage, ActionState, ExecutionMetadata,
    OperationId, WorkerId,
};
use tokio::sync::watch::error::SendError;
use tokio::sync::{watch, Notify};
use tracing::{event, Level};

use crate::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, OperationFilter,
    WorkerStateManager,
};
use crate::scheduler_state::awaited_action::AwaitedAction;
use crate::scheduler_state::client_action_state_result::ClientActionStateResult;
use crate::scheduler_state::completed_action::CompletedAction;
use crate::scheduler_state::metrics::Metrics;
use crate::scheduler_state::workers::Workers;
use crate::worker::WorkerUpdate;

#[repr(transparent)]
pub(crate) struct StateManager {
    pub inner: StateManagerImpl,
}

impl StateManager {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        queued_actions_set: HashSet<Arc<ActionInfo>>,
        queued_actions: BTreeMap<Arc<ActionInfo>, AwaitedAction>,
        workers: Workers,
        active_actions: HashMap<Arc<ActionInfo>, AwaitedAction>,
        recently_completed_actions: HashSet<CompletedAction>,
        metrics: Arc<Metrics>,
        max_job_retries: usize,
        tasks_or_workers_change_notify: Arc<Notify>,
    ) -> Self {
        Self {
            inner: StateManagerImpl {
                queued_actions_set,
                queued_actions,
                workers,
                active_actions,
                recently_completed_actions,
                metrics,
                max_job_retries,
                tasks_or_workers_change_notify,
            },
        }
    }
}

/// StateManager is responsible for maintaining the state of the scheduler. Scheduler state
/// includes the actions that are queued, active, and recently completed. It also includes the
/// workers that are available to execute actions based on allocation strategy.
pub(crate) struct StateManagerImpl {
    // TODO(adams): Move `queued_actions_set` and `queued_actions` into a single struct that
    //  provides a unified interface for interacting with the two containers.

    // Important: `queued_actions_set` and `queued_actions` are two containers that provide
    // different search and sort capabilities. We are using the two different containers to
    // optimize different use cases. `HashSet` is used to look up actions in O(1) time. The
    // `BTreeMap` is used to sort actions in O(log n) time based on priority and timestamp.
    // These two fields must be kept in-sync, so if you modify one, you likely need to modify the
    // other.
    /// A `HashSet` of all actions that are queued. A hashset is used to find actions that are queued
    /// in O(1) time. This set allows us to find and join on new actions onto already existing
    /// (or queued) actions where insert timestamp of queued actions is not known. Using an
    /// additional `HashSet` will prevent us from having to iterate the `BTreeMap` to find actions.
    ///
    /// Important: `queued_actions_set` and `queued_actions` must be kept in sync.
    pub(crate) queued_actions_set: HashSet<Arc<ActionInfo>>,

    /// A BTreeMap of sorted actions that are primarily based on priority and insert timestamp.
    /// `ActionInfo` implements `Ord` that defines the `cmp` function for order. Using a BTreeMap
    /// gives us to sorted actions that are queued in O(log n) time.
    ///
    /// Important: `queued_actions_set` and `queued_actions` must be kept in sync.
    pub(crate) queued_actions: BTreeMap<Arc<ActionInfo>, AwaitedAction>,

    /// A `Workers` pool that contains all workers that are available to execute actions in a priority
    /// order based on the allocation strategy.
    pub(crate) workers: Workers,

    /// A map of all actions that are active. A hashmap is used to find actions that are active in
    /// O(1) time. The key is the `ActionInfo` struct. The value is the `AwaitedAction` struct.
    pub(crate) active_actions: HashMap<Arc<ActionInfo>, AwaitedAction>,

    /// These actions completed recently but had no listener, they might have
    /// completed while the caller was thinking about calling wait_execution, so
    /// keep their completion state around for a while to send back.
    /// TODO(#192) Revisit if this is the best way to handle recently completed actions.
    pub(crate) recently_completed_actions: HashSet<CompletedAction>,

    pub(crate) metrics: Arc<Metrics>,

    /// Default times a job can retry before failing.
    pub(crate) max_job_retries: usize,

    /// Notify task<->worker matching engine that work needs to be done.
    pub(crate) tasks_or_workers_change_notify: Arc<Notify>,
}

impl StateManager {
    /// Modifies the `stage` of `current_state` within `AwaitedAction`. Sends notification channel
    /// the new state.
    ///
    ///
    /// # Discussion
    ///
    /// The use of `Arc::make_mut` is potentially dangerous because it clones the data and
    /// invalidates all weak references to it. However, in this context, it is considered
    /// safe because the data is going to be re-sent back out. The primary reason for using
    /// `Arc` is to reduce the number of copies, not to enforce read-only access. This approach
    /// ensures that all downstream components receive the same pointer. If an update occurs
    /// while another thread is operating on the data, it is acceptable, since the other thread
    /// will receive another update with the new version.
    ///
    fn mutate_stage(
        awaited_action: &mut AwaitedAction,
        action_stage: ActionStage,
    ) -> Result<(), SendError<Arc<ActionState>>> {
        Arc::make_mut(&mut awaited_action.current_state).stage = action_stage;
        awaited_action
            .notify_channel
            .send(awaited_action.current_state.clone())
    }

    /// Modifies the `priority` of `action_info` within `ActionInfo`.
    ///
    fn mutate_priority(action_info: &mut Arc<ActionInfo>, priority: i32) {
        Arc::make_mut(action_info).priority = priority;
    }

    /// Evicts the worker from the pool and puts items back into the queue if anything was being executed on it.
    fn immediate_evict_worker(&mut self, worker_id: &WorkerId, err: Error) {
        if let Some(mut worker) = self.inner.workers.remove_worker(worker_id) {
            self.inner.metrics.workers_evicted.inc();
            // We don't care if we fail to send message to worker, this is only a best attempt.
            let _ = worker.notify_update(WorkerUpdate::Disconnect);
            for action_info in worker.running_action_infos.drain() {
                self.inner.metrics.workers_evicted_with_running_action.inc();
                self.retry_action(&action_info, worker_id, err.clone());
            }
        }
    }

    fn retry_action(&mut self, action_info: &Arc<ActionInfo>, worker_id: &WorkerId, err: Error) {
        match self.inner.active_actions.remove(action_info) {
            Some(running_action) => {
                let mut awaited_action: AwaitedAction = running_action;
                let send_result = if awaited_action.attempts >= self.inner.max_job_retries {
                    self.inner.metrics.retry_action_max_attempts_reached.inc();
                    let action_stage_completed = ActionStage::Completed(ActionResult {
                        execution_metadata: ExecutionMetadata {
                            worker: format!("{worker_id}"),
                            ..ExecutionMetadata::default()
                        },
                        error: Some(err.merge(make_err!(
                            Code::Internal,
                            "Job cancelled because it attempted to execute too many times and failed"
                        ))),
                        ..ActionResult::default()
                    });
                    StateManager::mutate_stage(&mut awaited_action, action_stage_completed)
                    // Do not put the action back in the queue here, as this action attempted to run too many
                    // times.
                } else {
                    self.inner.metrics.retry_action.inc();
                    let send_result =
                        StateManager::mutate_stage(&mut awaited_action, ActionStage::Queued);
                    self.inner.queued_actions_set.insert(action_info.clone());
                    self.inner
                        .queued_actions
                        .insert(action_info.clone(), awaited_action);
                    send_result
                };

                if send_result.is_err() {
                    self.inner.metrics.retry_action_no_more_listeners.inc();
                    // Don't remove this task, instead we keep them around for a bit just in case
                    // the client disconnected and will reconnect and ask for same job to be executed
                    // again.
                    event!(
                        Level::WARN,
                        ?action_info,
                        ?worker_id,
                        "Action has no more listeners during evict_worker()"
                    );
                }
            }
            None => {
                self.inner.metrics.retry_action_but_action_missing.inc();
                event!(
                    Level::ERROR,
                    ?action_info,
                    ?worker_id,
                    "Worker stated it was running an action, but it was not in the active_actions"
                );
            }
        }
    }

    fn update_action_with_internal_error(
        &mut self,
        worker_id: &WorkerId,
        action_info_hash_key: ActionInfoHashKey,
        err: Error,
    ) {
        self.inner.metrics.update_action_with_internal_error.inc();
        let Some((action_info, mut running_action)) = self
            .inner
            .active_actions
            .remove_entry(&action_info_hash_key)
        else {
            self.inner
                .metrics
                .update_action_with_internal_error_no_action
                .inc();
            event!(
                Level::ERROR,
                ?action_info_hash_key,
                ?worker_id,
                "Could not find action info in active actions"
            );
            return;
        };

        let due_to_backpressure = err.code == Code::ResourceExhausted;
        // Don't count a backpressure failure as an attempt for an action.
        if due_to_backpressure {
            self.inner
                .metrics
                .update_action_with_internal_error_backpressure
                .inc();
            running_action.attempts -= 1;
        }
        let Some(running_action_worker_id) = running_action.worker_id else {
            event!(
                Level::ERROR,
                ?action_info_hash_key,
                ?worker_id,
                "Got a result from a worker that should not be running the action, Removing worker. Expected action to be unassigned got worker",
          );
            return;
        };
        if running_action_worker_id == *worker_id {
            // Don't set the error on an action that's running somewhere else.
            event!(
                Level::WARN,
                ?action_info_hash_key,
                ?worker_id,
                ?running_action_worker_id,
                ?err,
                "Internal worker error",
            );
            running_action.last_error = Some(err.clone());
        } else {
            self.inner
                .metrics
                .update_action_with_internal_error_from_wrong_worker
                .inc();
        }

        // Now put it back. retry_action() needs it to be there to send errors properly.
        self.inner
            .active_actions
            .insert(action_info.clone(), running_action);

        // Clear this action from the current worker.
        if let Some(worker) = self.inner.workers.workers.get_mut(worker_id) {
            let was_paused = !worker.can_accept_work();
            // This unpauses, but since we're completing with an error, don't
            // unpause unless all actions have completed.
            worker.complete_action(&action_info);
            // Only pause if there's an action still waiting that will unpause.
            if (was_paused || due_to_backpressure) && worker.has_actions() {
                worker.is_paused = true;
            }
        }

        // Re-queue the action or fail on max attempts.
        self.retry_action(&action_info, worker_id, err);
        self.inner.tasks_or_workers_change_notify.notify_one();
    }
}

#[async_trait]
impl ClientStateManager for StateManager {
    async fn add_action(
        &mut self,
        action_info: ActionInfo,
    ) -> Result<Arc<dyn ActionStateResult>, Error> {
        // Check to see if the action is running, if it is and cacheable, merge the actions.
        if let Some(running_action) = self.inner.active_actions.get_mut(&action_info) {
            self.inner.metrics.add_action_joined_running_action.inc();
            return Ok(Arc::new(ClientActionStateResult::new(
                running_action.notify_channel.subscribe(),
            )));
        }

        // Check to see if the action is queued, if it is and cacheable, merge the actions.
        if let Some(mut arc_action_info) = self.inner.queued_actions_set.take(&action_info) {
            let (original_action_info, queued_action) = self
                .inner
                .queued_actions
                .remove_entry(&arc_action_info)
                .err_tip(|| "Internal error queued_actions and queued_actions_set should match")?;
            self.inner.metrics.add_action_joined_queued_action.inc();

            let new_priority = cmp::max(original_action_info.priority, action_info.priority);
            drop(original_action_info); // This increases the chance Arc::make_mut won't copy.

            // In the event our task is higher priority than the one already scheduled, increase
            // the priority of the scheduled one.
            StateManager::mutate_priority(&mut arc_action_info, new_priority);

            let result = Arc::new(ClientActionStateResult::new(
                queued_action.notify_channel.subscribe(),
            ));

            // Even if we fail to send our action to the client, we need to add this action back to the
            // queue because it was remove earlier.
            self.inner
                .queued_actions
                .insert(arc_action_info.clone(), queued_action);
            self.inner.queued_actions_set.insert(arc_action_info);
            return Ok(result);
        }

        self.inner.metrics.add_action_new_action_created.inc();
        // Action needs to be added to queue or is not cacheable.
        let action_info = Arc::new(action_info);

        let operation_id = OperationId::new(action_info.unique_qualifier.clone());
        let current_state = Arc::new(ActionState {
            stage: ActionStage::Queued,
            id: operation_id,
        });

        let (tx, rx) = watch::channel(current_state.clone());

        self.inner.queued_actions_set.insert(action_info.clone());
        self.inner.queued_actions.insert(
            action_info.clone(),
            AwaitedAction {
                action_info,
                current_state,
                notify_channel: tx,
                attempts: 0,
                last_error: None,
                worker_id: None,
            },
        );

        return Ok(Arc::new(ClientActionStateResult::new(rx)));
    }

    async fn filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        // TODO(adams): Build out a proper filter for other fields for state, at the moment
        //  this only supports the unique qualifier.
        let unique_qualifier = &filter
            .unique_qualifier
            .err_tip(|| "No unique qualifier provided")?;
        let maybe_awaited_action = self
            .inner
            .queued_actions_set
            .get(unique_qualifier)
            .and_then(|action_info| self.inner.queued_actions.get(action_info))
            .or_else(|| self.inner.active_actions.get(unique_qualifier));

        let Some(awaited_action) = maybe_awaited_action else {
            return Ok(Box::pin(stream::empty()));
        };

        let rx = awaited_action.notify_channel.subscribe();
        let action_result: [Arc<dyn ActionStateResult>; 1] =
            [Arc::new(ClientActionStateResult::new(rx))];
        Ok(Box::pin(stream::iter(action_result)))
    }
}

#[async_trait]
impl WorkerStateManager for StateManager {
    async fn update_operation(
        &mut self,
        operation_id: OperationId,
        worker_id: WorkerId,
        action_stage: Result<ActionStage, Error>,
    ) -> Result<(), Error> {
        match action_stage {
            Ok(action_stage) => {
                let action_info_hash_key = operation_id.unique_qualifier;
                if !action_stage.has_action_result() {
                    self.inner.metrics.update_action_missing_action_result.inc();
                    event!(
                        Level::ERROR,
                        ?action_info_hash_key,
                        ?worker_id,
                        ?action_stage,
                        "Worker sent error while updating action. Removing worker"
                    );
                    let err = make_err!(
                Code::Internal,
                "Worker '{worker_id}' set the action_stage of running action {action_info_hash_key:?} to {action_stage:?}. Removing worker.",
            );
                    self.immediate_evict_worker(&worker_id, err.clone());
                    return Err(err);
                }

                let (action_info, mut running_action) = self
                    .inner
                    .active_actions
                    .remove_entry(&action_info_hash_key)
                    .err_tip(|| {
                        format!("Could not find action info in active actions : {action_info_hash_key:?}")
                    })?;

                if running_action.worker_id != Some(worker_id) {
                    self.inner.metrics.update_action_from_wrong_worker.inc();
                    let err = match running_action.worker_id {

                        Some(running_action_worker_id) => make_err!(
                    Code::Internal,
                    "Got a result from a worker that should not be running the action, Removing worker. Expected worker {running_action_worker_id} got worker {worker_id}",
                ),
                        None => make_err!(
                    Code::Internal,
                    "Got a result from a worker that should not be running the action, Removing worker. Expected action to be unassigned got worker {worker_id}",
                ),
                    };
                    event!(
                        Level::ERROR,
                        ?action_info,
                        ?worker_id,
                        ?running_action.worker_id,
                        ?err,
                        "Got a result from a worker that should not be running the action, Removing worker"
                    );
                    // First put it back in our active_actions or we will drop the task.
                    self.inner
                        .active_actions
                        .insert(action_info, running_action);
                    self.immediate_evict_worker(&worker_id, err.clone());
                    return Err(err);
                }

                let send_result = StateManager::mutate_stage(&mut running_action, action_stage);

                if !running_action.current_state.stage.is_finished() {
                    if send_result.is_err() {
                        self.inner.metrics.update_action_no_more_listeners.inc();
                        event!(
                            Level::WARN,
                            ?action_info,
                            ?worker_id,
                            "Action has no more listeners during update_action()"
                        );
                    }
                    // If the operation is not finished it means the worker is still working on it, so put it
                    // back or else we will lose track of the task.
                    self.inner
                        .active_actions
                        .insert(action_info, running_action);
                    return Ok(());
                }

                // Keep in case this is asked for soon.
                self.inner
                    .recently_completed_actions
                    .insert(CompletedAction {
                        completed_time: SystemTime::now(),
                        state: running_action.current_state,
                    });

                let worker = self
                    .inner
                    .workers
                    .workers
                    .get_mut(&worker_id)
                    .ok_or_else(|| {
                        make_input_err!("WorkerId '{}' does not exist in workers map", worker_id)
                    })?;
                worker.complete_action(&action_info);

                Ok(())
            }
            Err(e) => {
                self.update_action_with_internal_error(
                    &worker_id,
                    operation_id.unique_qualifier,
                    e.clone(),
                );
                return Err(e);
            }
        }
    }
}
