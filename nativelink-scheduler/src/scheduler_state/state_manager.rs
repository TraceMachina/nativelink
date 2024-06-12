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

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use hashbrown::{HashMap, HashSet};
use nativelink_error::{make_err, make_input_err, Code, Error, ResultExt};
use nativelink_util::action_messages::{
    ActionInfo, ActionResult, ActionStage, ExecutionMetadata, OperationId, WorkerId,
};
use tracing::{event, Level};

use crate::operation_state_manager::WorkerStateManager;
use crate::scheduler_state::awaited_action::AwaitedAction;
use crate::scheduler_state::completed_action::CompletedAction;
use crate::scheduler_state::metrics::Metrics;
use crate::scheduler_state::workers::Workers;
use crate::worker::WorkerUpdate;

/// StateManager is responsible for maintaining the state of the scheduler. Scheduler state
/// includes the actions that are queued, active, and recently completed. It also includes the
/// workers that are available to execute actions based on allocation strategy.
pub(crate) struct StateManager {
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
}

impl StateManager {
    /// Evicts the worker from the pool and puts items back into the queue if anything was being executed on it.
    fn immediate_evict_worker(&mut self, worker_id: &WorkerId, err: Error) {
        if let Some(mut worker) = self.workers.remove_worker(worker_id) {
            self.metrics.workers_evicted.inc();
            // We don't care if we fail to send message to worker, this is only a best attempt.
            let _ = worker.notify_update(WorkerUpdate::Disconnect);
            // We create a temporary Vec to avoid doubt about a possible code
            // path touching the worker.running_action_infos elsewhere.
            for action_info in worker.running_action_infos.drain() {
                self.metrics.workers_evicted_with_running_action.inc();
                self.retry_action(&action_info, worker_id, err.clone());
            }
        }
        // TODO(adams): call this after eviction.
        // Note: Calling this many time is very cheap, it'll only trigger `do_try_match` once.
        //self.tasks_or_workers_change_notify.notify_one();
    }

    fn retry_action(&mut self, action_info: &Arc<ActionInfo>, worker_id: &WorkerId, err: Error) {
        match self.active_actions.remove(action_info) {
            Some(running_action) => {
                let mut awaited_action = running_action;
                let send_result = if awaited_action.attempts >= self.max_job_retries {
                    self.metrics.retry_action_max_attempts_reached.inc();
                    Arc::make_mut(&mut awaited_action.current_state).stage = ActionStage::Completed(ActionResult {
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
                    awaited_action
                        .notify_channel
                        .send(awaited_action.current_state.clone())
                    // Do not put the action back in the queue here, as this action attempted to run too many
                    // times.
                } else {
                    self.metrics.retry_action.inc();
                    Arc::make_mut(&mut awaited_action.current_state).stage = ActionStage::Queued;
                    let send_result = awaited_action
                        .notify_channel
                        .send(awaited_action.current_state.clone());
                    self.queued_actions_set.insert(action_info.clone());
                    self.queued_actions
                        .insert(action_info.clone(), awaited_action);
                    send_result
                };

                if send_result.is_err() {
                    self.metrics.retry_action_no_more_listeners.inc();
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
                self.metrics.retry_action_but_action_missing.inc();
                event!(
                    Level::ERROR,
                    ?action_info,
                    ?worker_id,
                    "Worker stated it was running an action, but it was not in the active_actions"
                );
            }
        }
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
        let action_stage = action_stage?;
        let action_info_hash_key = operation_id.unique_qualifier;
        if !action_stage.has_action_result() {
            self.metrics.update_action_missing_action_result.inc();
            event!(
                Level::ERROR,
                ?action_info_hash_key, /*?action_info_hash_key,*/
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
            .active_actions
            .remove_entry(&action_info_hash_key)
            .err_tip(|| {
                format!("Could not find action info in active actions : {action_info_hash_key:?}")
            })?;

        if running_action.worker_id != Some(worker_id) {
            self.metrics.update_action_from_wrong_worker.inc();
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
            self.active_actions.insert(action_info, running_action);
            self.immediate_evict_worker(&worker_id, err.clone());
            return Err(err);
        }

        Arc::make_mut(&mut running_action.current_state).stage = action_stage;

        let send_result = running_action
            .notify_channel
            .send(running_action.current_state.clone());

        if !running_action.current_state.stage.is_finished() {
            if send_result.is_err() {
                self.metrics.update_action_no_more_listeners.inc();
                event!(
                    Level::WARN,
                    ?action_info,
                    ?worker_id,
                    "Action has no more listeners during update_action()"
                );
            }
            // If the operation is not finished it means the worker is still working on it, so put it
            // back or else we will lose track of the task.
            self.active_actions.insert(action_info, running_action);
            return Ok(());
        }

        // Keep in case this is asked for soon.
        self.recently_completed_actions.insert(CompletedAction {
            completed_time: SystemTime::now(),
            state: running_action.current_state,
        });

        let worker = self.workers.workers.get_mut(&worker_id).ok_or_else(|| {
            make_input_err!("WorkerId '{}' does not exist in workers map", worker_id)
        })?;
        worker.complete_action(&action_info);

        Ok(())
    }
}
