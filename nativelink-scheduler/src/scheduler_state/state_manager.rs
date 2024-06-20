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
use std::collections::btree_map::Keys;
use std::collections::BTreeMap;
use std::iter::{Cloned, Map, Rev};
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream;
use hashbrown::{HashMap, HashSet};
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_util::action_messages::{
    ActionInfo, ActionResult, ActionStage, ActionState, ExecutionMetadata, OperationId, WorkerId,
};
use tokio::sync::{watch, Notify};
use tracing::{event, Level};

use crate::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, MatchingEngineStateManager,
    OperationFilter,
};
use crate::scheduler_state::awaited_action::AwaitedAction;
use crate::scheduler_state::client_action_state_result::ClientActionStateResult;
use crate::scheduler_state::completed_action::CompletedAction;
use crate::scheduler_state::matching_engine_action_state_result::MatchingEngineActionStateResult;
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

    fn immediate_evict_worker(&mut self, worker_id: &WorkerId, err: Error) {
        if let Some(mut worker) = self.inner.workers.remove_worker(worker_id) {
            self.inner.metrics.workers_evicted.inc();
            // We don't care if we fail to send message to worker, this is only a best attempt.
            let _ = worker.notify_update(WorkerUpdate::Disconnect);
            // We create a temporary Vec to avoid doubt about a possible code
            // path touching the worker.running_action_infos elsewhere.
            for action_info in worker.running_action_infos.drain() {
                self.inner.metrics.workers_evicted_with_running_action.inc();
                self.retry_action(&action_info, worker_id, err.clone());
            }
            // Note: Calling this many time is very cheap, it'll only trigger `do_try_match` once.
            self.inner.tasks_or_workers_change_notify.notify_one();
        }
    }

    fn retry_action(&mut self, action_info: &Arc<ActionInfo>, worker_id: &WorkerId, err: Error) {
        match self.inner.active_actions.remove(action_info) {
            Some(running_action) => {
                let mut awaited_action = running_action;
                let send_result = if awaited_action.attempts >= self.inner.max_job_retries {
                    self.inner.metrics.retry_action_max_attempts_reached.inc();
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
                    self.inner.metrics.retry_action.inc();
                    Arc::make_mut(&mut awaited_action.current_state).stage = ActionStage::Queued;
                    let send_result = awaited_action
                        .notify_channel
                        .send(awaited_action.current_state.clone());
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
            Arc::make_mut(&mut arc_action_info).priority = new_priority;

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
impl MatchingEngineStateManager for StateManager {
    async fn filter_operations(
        &self,
        _filter: OperationFilter, // TODO(adam): reference filter
    ) -> Result<ActionStateResultStream, Error> {
        // TODO(adams): use OperationFilter vs directly encoding it.
        let action_infos: Map<
            Cloned<Rev<Keys<Arc<ActionInfo>, AwaitedAction>>>,
            fn(Arc<ActionInfo>) -> Arc<dyn ActionStateResult>,
        > = self
            .inner
            .queued_actions
            .keys()
            .rev()
            .cloned()
            .map(|action_info| {
                // TODO(adam): ActionState is always available and can be returned from here.
                //   later we might want to rewrite this to return ActionState.
                let cloned_action_info = action_info.clone();
                Arc::new(MatchingEngineActionStateResult::new(cloned_action_info))
            });

        let action_infos: Vec<Arc<dyn ActionStateResult>> = action_infos.collect();
        Ok(Box::pin(stream::iter(action_infos)))
    }

    async fn update_operation(
        &mut self,
        operation_id: OperationId,
        worker_id: Option<WorkerId>,
        action_stage: Result<ActionStage, Error>,
    ) -> Result<(), Error> {
        if let Some(action_info) = self
            .inner
            .queued_actions_set
            .get(&operation_id.unique_qualifier)
        {
            // worker_id related updates
            // We do not have the infra to handle this code path currently

            if let Some(worker_id) = worker_id {
                // Here we would remove the worker and assign it to a new one

                let worker = self.inner.workers.workers.get_mut(&worker_id);

                let action_info = action_info.clone();

                if let Some(worker) = worker {
                    let notify_worker_result =
                        worker.notify_update(WorkerUpdate::RunAction(action_info.clone()));

                    if notify_worker_result.is_err() {
                        // Remove worker, as it is no longer receiving messages and let it try to find another worker.
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

                        self.immediate_evict_worker(&worker_id, err.clone());

                        return Err(err);
                    }
                }

                // At this point everything looks good, so remove it from the queue and add it to active actions.
                let (action_info, mut awaited_action) = self
                    .inner
                    .queued_actions
                    .remove_entry(action_info.as_ref())
                    .unwrap();

                assert!(
                    self.inner.queued_actions_set.remove(&action_info),
                    "queued_actions_set should always have same keys as queued_actions"
                );

                match action_stage {
                    Ok(action_stage) => {
                        Arc::make_mut(&mut awaited_action.current_state).stage = action_stage;
                    }
                    Err(e) => {
                        event!(
                            Level::WARN,
                            ?operation_id,
                            ?worker_id,
                            "Action stage setting error during do_try_match()"
                        );
                        awaited_action.last_error = Some(e);
                    }
                }

                awaited_action.worker_id = Some(worker_id);

                let send_result = awaited_action
                    .notify_channel
                    .send(awaited_action.current_state.clone());

                if send_result.is_err() {
                    // Don't remove this task, instead we keep them around for a bit just in case
                    // the client disconnected and will reconnect and ask for same job to be executed
                    // again.
                    event!(
                        Level::WARN,
                        ?action_info,
                        ?worker_id,
                        "Action has no more listeners during do_try_match()"
                    );
                }

                awaited_action.attempts += 1;
                self.inner
                    .active_actions
                    .insert(action_info, awaited_action);
            } else {
                event!(
                    Level::WARN,
                    ?operation_id,
                    ?worker_id,
                    "No worker found in do_try_match()"
                );
            }
        } else {
            event!(
                Level::WARN,
                ?operation_id,
                ?worker_id,
                "No action info found in do_try_match()"
            );
        }

        Ok(())
    }

    async fn remove_operation(&self, _operation_id: OperationId) -> Result<(), Error> {
        todo!()
    }
}
