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
use std::collections::BTreeSet;
use std::sync::Arc;

use async_lock::Mutex;
use async_trait::async_trait;
use hashbrown::HashMap;
use nativelink_error::{make_err, Code, Error};
use nativelink_util::action_messages::{
    ActionInfo, ActionInfoHashKey, ActionResult, ActionStage, ActionState, ExecutionMetadata,
    OperationId, WorkerId,
};
use tokio::sync::Notify;
use tracing::{event, Level};
use uuid::Uuid;

use crate::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, MatchingEngineStateManager,
    OperationFilter, WorkerStateManager,
};
use crate::scheduler_state::awaited_action::AwaitedAction;
use crate::scheduler_state::client_action_state_result::ClientActionStateResult;
use crate::scheduler_state::metrics::Metrics;

use super::awaited_action::AwaitedActionSortKey;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ClientOperationId(Uuid);

impl ToString for ClientOperationId {
    fn to_string(&self) -> String {
        self.0.to_string()
    }
}

#[derive(Debug, Clone)]
struct SortedAwaitedAction {
    sort_key: AwaitedActionSortKey,
    awaited_action: Arc<AwaitedAction>,
}

impl PartialEq for SortedAwaitedAction {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.awaited_action, &other.awaited_action) && self.sort_key == other.sort_key
    }
}

impl Eq for SortedAwaitedAction {}

impl PartialOrd for SortedAwaitedAction {
    fn partial_cmp(&self, other: &Self) -> Option<cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for SortedAwaitedAction {
    fn cmp(&self, other: &Self) -> cmp::Ordering {
        self.sort_key.cmp(&other.sort_key).then_with(|| {
            Arc::as_ptr(&self.awaited_action).cmp(&Arc::as_ptr(&other.awaited_action))
        })
    }
}

#[derive(Default)]
struct SortedAwaitedActions {
    unknown: BTreeSet<SortedAwaitedAction>,
    cache_check: BTreeSet<SortedAwaitedAction>,
    queued: BTreeSet<SortedAwaitedAction>,
    executing: BTreeSet<SortedAwaitedAction>,
    completed: BTreeSet<SortedAwaitedAction>,
    completed_from_cache: BTreeSet<SortedAwaitedAction>,
}

#[derive(Default)]
pub struct AwaitedActionDb {
    /// A lookup table to lookup the state of an action by its client operation id.
    client_operation_to_awaited_action: HashMap<ClientOperationId, Arc<AwaitedAction>>,

    /// A lookup table to lookup the state of an action by its worker operation id.
    operation_id_to_awaited_action: HashMap<OperationId, Arc<AwaitedAction>>,

    /// A lookup table to lookup the state of an action by its unique qualifier.
    action_info_hash_key_to_awaited_action: HashMap<ActionInfoHashKey, Arc<AwaitedAction>>,

    /// A sorted set of [`AwaitedAction`]s. A wrapper is used to perform sorting
    /// based on the [`AwaitedActionSortKey`] of the [`AwaitedAction`].
    ///
    /// See [`AwaitedActionSortKey`] for more information on the ordering.
    sorted_action_info_hash_keys: SortedAwaitedActions,
}

impl AwaitedActionDb {
    fn get_by_client_operation_id(
        &self,
        client_operation_id: &ClientOperationId,
    ) -> Option<&Arc<AwaitedAction>> {
        self.client_operation_to_awaited_action
            .get(client_operation_id)
    }

    fn get_by_operation_id(&self, operation_id: &OperationId) -> Option<&Arc<AwaitedAction>> {
        self.operation_id_to_awaited_action.get(operation_id)
    }

    fn get_sort_map_for_state(
        &mut self,
        state: &ActionStage,
    ) -> &mut BTreeSet<SortedAwaitedAction> {
        match state {
            ActionStage::Unknown => &mut self.sorted_action_info_hash_keys.unknown,
            ActionStage::CacheCheck => &mut self.sorted_action_info_hash_keys.cache_check,
            ActionStage::Queued => &mut self.sorted_action_info_hash_keys.queued,
            ActionStage::Executing => &mut self.sorted_action_info_hash_keys.executing,
            ActionStage::Completed(_) => &mut self.sorted_action_info_hash_keys.completed,
            ActionStage::CompletedFromCache(_) => {
                &mut self.sorted_action_info_hash_keys.completed_from_cache
            }
        }
    }

    fn insert_sort_map_for_stage(
        &mut self,
        stage: &ActionStage,
        sorted_awaited_action: SortedAwaitedAction,
    ) {
        let did_insert = match stage {
            ActionStage::Unknown => self
                .sorted_action_info_hash_keys
                .unknown
                .insert(sorted_awaited_action),
            ActionStage::CacheCheck => self
                .sorted_action_info_hash_keys
                .cache_check
                .insert(sorted_awaited_action),
            ActionStage::Queued => self
                .sorted_action_info_hash_keys
                .queued
                .insert(sorted_awaited_action),
            ActionStage::Executing => self
                .sorted_action_info_hash_keys
                .executing
                .insert(sorted_awaited_action),
            ActionStage::Completed(_) => self
                .sorted_action_info_hash_keys
                .completed
                .insert(sorted_awaited_action),
            ActionStage::CompletedFromCache(_) => self
                .sorted_action_info_hash_keys
                .completed_from_cache
                .insert(sorted_awaited_action),
        };
        if did_insert {
            event!(
                Level::ERROR,
                "Tried to insert an action that was already in the sorted map. This should never happen.",
            );
        }
    }

    /// Sets the state of the action to the provided `action_state` and notifies all listeners.
    /// If the action has no more listeners, returns `false`.
    fn set_action_state(
        &mut self,
        awaited_action: Arc<AwaitedAction>,
        action_state: Arc<ActionState>,
    ) -> bool {
        // We need to first get a lock on the awaited action to ensure
        // another operation doesn't update it while we are looking up
        // the sorted key.
        let sort_info = awaited_action.get_sort_info();
        let old_state = awaited_action.get_current_state();

        let has_listeners = awaited_action.set_current_state(action_state.clone());

        if !old_state.stage.is_same_stage(&action_state.stage) {
            let sort_key = sort_info.get_previous_sort_key();
            let btree = self.get_sort_map_for_state(&old_state.stage);
            drop(sort_info);
            let maybe_sorted_awaited_action = btree.take(&SortedAwaitedAction {
                sort_key,
                awaited_action,
            });

            let Some(sorted_awaited_action) = maybe_sorted_awaited_action else {
                event!(
                    Level::ERROR,
                    "sorted_action_info_hash_keys and action_info_hash_key_to_awaited_action are out of sync",
                );
                return false;
            };

            self.insert_sort_map_for_stage(&action_state.stage, sorted_awaited_action);
        }
        has_listeners
    }

    fn subscribe_or_add_action(&mut self, action_info: ActionInfo) -> Arc<ClientActionStateResult> {
        // Check to see if the action is already known and subscribe if it is.
        let action_info = match self.try_subscribe(action_info) {
            Ok(subscription) => return subscription,
            Err(action_info) => Arc::new(action_info),
        };

        let (awaited_action, sort_key, subscription) =
            AwaitedAction::new_with_subscription(action_info.clone());
        let awaited_action = Arc::new(awaited_action);
        self.action_info_hash_key_to_awaited_action
            .insert(action_info.unique_qualifier.clone(), awaited_action.clone());

        self.insert_sort_map_for_stage(
            &awaited_action.get_current_state().stage,
            SortedAwaitedAction {
                sort_key,
                awaited_action,
            },
        );
        return Arc::new(ClientActionStateResult::new(subscription));
    }

    fn try_subscribe(
        &mut self,
        action_info: ActionInfo,
    ) -> Result<Arc<ClientActionStateResult>, ActionInfo> {
        let Some(awaited_action) = self
            .action_info_hash_key_to_awaited_action
            .get(&action_info.unique_qualifier)
        else {
            return Err(action_info);
        };
        let awaited_action = awaited_action.clone();
        if let Some(sort_info_lock) = awaited_action.set_priority(action_info.priority) {
            let state = awaited_action.get_current_state();
            let maybe_sorted_awaited_action =
                self.get_sort_map_for_state(&state.stage)
                    .take(&SortedAwaitedAction {
                        sort_key: sort_info_lock.get_previous_sort_key(),
                        awaited_action: awaited_action.clone(),
                    });
            let Some(mut sorted_awaited_action) = maybe_sorted_awaited_action else {
                event!(
                    Level::ERROR,
                    ?action_info,
                    ?awaited_action,
                    "sorted_action_info_hash_keys and action_info_hash_key_to_awaited_action are out of sync",
                );
                return Err(action_info);
            };
            sorted_awaited_action.sort_key = sort_info_lock.get_new_sort_key();
            self.insert_sort_map_for_stage(&state.stage, sorted_awaited_action);
        }

        let subscription = awaited_action.subscribe();
        Ok(Arc::new(ClientActionStateResult::new(subscription)))
    }
}

#[repr(transparent)]
pub(crate) struct StateManager {
    pub inner: Mutex<StateManagerImpl>,
}

impl StateManager {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        metrics: Arc<Metrics>,
        tasks_change_notify: Arc<Notify>,
        max_job_retries: usize,
    ) -> Self {
        Self {
            inner: Mutex::new(StateManagerImpl {
                action_db: AwaitedActionDb::default(),
                metrics,
                tasks_change_notify,
                max_job_retries,
            }),
        }
    }
}

/// StateManager is responsible for maintaining the state of the scheduler. Scheduler state
/// includes the actions that are queued, active, and recently completed. It also includes the
/// workers that are available to execute actions based on allocation strategy.
pub(crate) struct StateManagerImpl {
    pub(crate) action_db: AwaitedActionDb,

    pub(crate) metrics: Arc<Metrics>,

    /// Notify task<->worker matching engine that work needs to be done.
    pub(crate) tasks_change_notify: Arc<Notify>,

    pub(crate) max_job_retries: usize,
}

// /// Modifies the `stage` of `current_state` within `AwaitedAction`. Sends notification channel
// /// the new state.
// ///
// ///
// /// # Discussion
// ///
// /// The use of `Arc::make_mut` is potentially dangerous because it clones the data and
// /// invalidates all weak references to it. However, in this context, it is considered
// /// safe because the data is going to be re-sent back out. The primary reason for using
// /// `Arc` is to reduce the number of copies, not to enforce read-only access. This approach
// /// ensures that all downstream components receive the same pointer. If an update occurs
// /// while another thread is operating on the data, it is acceptable, since the other thread
// /// will receive another update with the new version.
// ///
// pub(crate) fn mutate_stage(
//     awaited_action: &mut AwaitedAction,
//     action_stage: ActionStage,
// ) -> Result<(), SendError<Arc<ActionState>>> {
//     Arc::make_mut(&mut awaited_action.current_state).stage = action_stage;
//     awaited_action
//         .notify_channel
//         .send(awaited_action.current_state.clone())
// }

// /// Updates the `last_error` field of the provided `AwaitedAction` and sends the current state
// /// to the notify channel.
// ///
// fn mutate_last_error(
//     awaited_action: &mut AwaitedAction,
//     last_error: Error,
// ) -> Result<(), SendError<Arc<ActionState>>> {
//     awaited_action.last_error = Some(last_error);
//     awaited_action
//         .notify_channel
//         .send(awaited_action.current_state.clone())
// }

// /// Sets the action stage for the given `AwaitedAction` based on the result of the provided
// /// `action_stage`. If the `action_stage` is an error, it updates the `last_error` field
// /// and logs a warning.
// ///
// /// # Note
// ///
// /// Intended utility function for matching engine.
// ///
// /// # Errors
// ///
// /// This function will return an error if updating the state of the `awaited_action` fails.
// ///
// async fn worker_set_action_stage(
//     awaited_action: &mut AwaitedAction,
//     action_stage: Result<ActionStage, Error>,
//     worker_id: WorkerId,
// ) -> Result<(), SendError<Arc<ActionState>>> {
//     match action_stage {
//         Ok(action_stage) => mutate_stage(awaited_action, action_stage),
//         Err(e) => {
//             event!(
//                 Level::WARN,
//                 ?worker_id,
//                 "Action stage setting error during do_try_match()"
//             );
//             mutate_last_error(awaited_action, e)
//         }
//     }
// }

/// Modifies the `priority` of `action_info` within `ActionInfo`.
///
fn mutate_priority(action_info: &mut Arc<ActionInfo>, priority: i32) {
    Arc::make_mut(action_info).priority = priority;
}

impl StateManagerImpl {
    // /// Marks the specified action as active, assigns it to the given worker, and updates the
    // /// action stage. This function removes the action from the queue, updates the action's state
    // /// or error, and inserts it into the set of active actions.
    // ///
    // /// # Note
    // ///
    // /// Intended utility function for matching engine.
    // ///
    // /// # Errors
    // ///
    // /// This function will return an error if it fails to update the action's state or if any other
    // /// error occurs during the process.
    // ///
    // async fn worker_set_as_active(
    //     &mut self,
    //     action_info: Arc<ActionInfo>,
    //     worker_id: WorkerId,
    //     action_stage: Result<ActionStage, Error>,
    // ) -> Result<(), Error> {
    //     if let Some((action_info, mut awaited_action)) =
    //         self.queued_actions.remove_entry(action_info.as_ref())
    //     {
    //         assert!(
    //             self.queued_actions_set.remove(&action_info),
    //             "queued_actions_set should always have same keys as queued_actions"
    //         );

    //         awaited_action.worker_id = Some(worker_id);

    //         let send_result =
    //             worker_set_action_stage(&mut awaited_action, action_stage, worker_id).await;

    //         if send_result.is_err() {
    //             event!(
    //                 Level::WARN,
    //                 ?action_info,
    //                 ?worker_id,
    //                 "Action has no more listeners during do_try_match()"
    //             );
    //         }

    //         awaited_action.attempts += 1;
    //         self.active_actions.insert(action_info, awaited_action);
    //         Ok(())
    //     } else {
    //         Err(make_err!(
    //             Code::Internal,
    //             "Action not found in queued_actions_set or queued_actions"
    //         ))
    //     }
    // }

    // fn update_operation_with_internal_error(
    //     &mut self,
    //     maybe_worker_id: Option<&WorkerId>,
    //     action_info: Arc<ActionInfo>,
    //     mut awaited_action: AwaitedAction,
    //     operation_id: &OperationId,
    //     err: Error,
    // ) -> Result<(), Error> {
    //     let action_info_hash_key = &operation_id.unique_qualifier;
    //     self.metrics.update_operation_with_internal_error.inc();

    //     if maybe_worker_id.is_some() && maybe_worker_id != awaited_action.worker_id.as_ref() {
    //         self.metrics
    //             .update_operation_with_internal_error_from_wrong_worker
    //             .inc();
    //         let err = err.append(format!(
    //             "Worker ids do not match - {:?} != {:?} for {:?}",
    //             maybe_worker_id, awaited_action.worker_id, awaited_action,
    //         ));
    //         // Don't set the error on an action that's running somewhere else.
    //         event!(
    //             Level::WARN,
    //             ?action_info_hash_key,
    //             ?maybe_worker_id,
    //             ?awaited_action.worker_id,
    //             "{err}",
    //         );
    //         awaited_action.last_error = Some(err.clone());
    //         return Err(err);
    //     }

    //     event!(
    //         Level::WARN,
    //         ?action_info_hash_key,
    //         ?maybe_worker_id,
    //         ?awaited_action.worker_id,
    //         ?err,
    //         "Internal worker error on worker",
    //     );
    //     awaited_action.last_error = Some(err.clone());

    //     let due_to_backpressure = err.code == Code::ResourceExhausted;
    //     // Don't count a backpressure failure as an attempt for an action.
    //     if due_to_backpressure {
    //         self.metrics
    //             .update_operation_with_internal_error_backpressure
    //             .inc();
    //         awaited_action.attempts -= 1;
    //     }

    //     let send_result = if awaited_action.attempts >= self.max_job_retries {
    //         self.metrics.retry_action_max_attempts_reached.inc();

    //         let worker = awaited_action
    //             .worker_id
    //             .map_or_else(String::default, |v| v.to_string());
    //         awaited_action.set_current_state(Arc::new(ActionStage::Completed(ActionResult {
    //             execution_metadata: ExecutionMetadata {
    //                 worker,
    //                 ..ExecutionMetadata::default()
    //             },
    //             error: Some(err.clone().merge(make_err!(
    //                 Code::Internal,
    //                 "Job cancelled because it attempted to execute too many times and failed"
    //             ))),
    //             ..ActionResult::default()
    //         })))
    //         // Do not put the action back in the queue here, as this action attempted to run too many
    //         // times.
    //     } else {
    //         self.metrics.retry_action.inc();
    //         let send_result = mutate_stage(&mut awaited_action, ActionStage::Queued);
    //         self.queued_actions_set.insert(action_info.clone());
    //         self.queued_actions
    //             .insert(action_info.clone(), awaited_action);
    //         send_result
    //     };

    //     if send_result.is_err() {
    //         self.metrics.retry_action_no_more_listeners.inc();
    //         // Don't remove this task, instead we keep them around for a bit just in case
    //         // the client disconnected and will reconnect and ask for same job to be executed
    //         // again.
    //         event!(
    //             Level::WARN,
    //             ?action_info,
    //             ?maybe_worker_id,
    //             ?err,
    //             "Action has no more listeners during evict_worker()"
    //         );
    //     }

    //     // // Now put it back. retry_action() needs it to be there to send errors properly.
    //     // self.active_actions.insert(action_info.clone(), awaited_action);

    //     self.tasks_change_notify.notify_one();
    //     return Ok(());
    // }

    fn inner_update_operation(
        &mut self,
        operation_id: &OperationId,
        maybe_worker_id: Option<&WorkerId>,
        action_stage_result: Result<ActionStage, Error>,
    ) -> Result<(), Error> {
        let awaited_action = self
            .action_db
            .get_by_operation_id(operation_id)
            .ok_or_else(|| {
                make_err!(
                    Code::Internal,
                    "Could not find action info StateManager::update_operation"
                )
            })?
            .clone();

        // Make sure we don't update an action that is already completed.
        if awaited_action.get_current_state().stage.is_finished() {
            return Err(make_err!(
                Code::Internal,
                "Action {operation_id:?} is already completed with state {:?}",
                awaited_action.get_current_state().stage,
            ));
        }

        // Make sure the worker id matches the awaited action worker id.
        // This might happen if the worker sending the update is not the
        // worker that was assigned.
        let awaited_action_worker_id = awaited_action.get_worker_id();
        if maybe_worker_id.is_some() && maybe_worker_id != awaited_action_worker_id.as_ref() {
            let err = make_err!(
                Code::Internal,
                "Worker ids do not match - {:?} != {:?} for {:?}",
                maybe_worker_id,
                awaited_action_worker_id,
                awaited_action,
            );
            event!(
                Level::ERROR,
                ?operation_id,
                ?maybe_worker_id,
                ?awaited_action_worker_id,
                "{}",
                err.to_string(),
            );
            return Err(err);
        }

        let stage = match action_stage_result {
            Ok(stage) => stage,
            Err(err) => {
                // Don't count a backpressure failure as an attempt for an action.
                let due_to_backpressure = err.code == Code::ResourceExhausted;
                if due_to_backpressure {
                    awaited_action.dec_attempts();
                }

                awaited_action.set_last_error(err.clone());

                if awaited_action.get_attempts() >= self.max_job_retries {
                    ActionStage::Completed(ActionResult {
                        execution_metadata: ExecutionMetadata {
                            worker: maybe_worker_id.map_or_else(String::default, |v| v.to_string()),
                            ..ExecutionMetadata::default()
                        },
                        error: Some(err.clone().merge(make_err!(
                            Code::Internal,
                            "Job cancelled because it attempted to execute too many times and failed"
                        ))),
                        ..ActionResult::default()
                    })
                } else {
                    ActionStage::Queued
                }
            }
        };
        let has_listeners = self.action_db.set_action_state(
            awaited_action.clone(),
            Arc::new(ActionState {
                stage,
                id: operation_id.clone(),
            }),
        );
        if !has_listeners {
            let action_state = awaited_action.get_current_state();
            event!(
                Level::WARN,
                ?awaited_action,
                ?action_state,
                "Action has no more listeners during AwaitedActionDb::set_action_state"
            );
        }

        self.tasks_change_notify.notify_one();
        Ok(())
    }

    fn inner_add_operation(
        &mut self,
        action_info: ActionInfo,
    ) -> Result<Arc<dyn ActionStateResult>, Error> {
        let subscription = self.action_db.subscribe_or_add_action(action_info);
        self.tasks_change_notify.notify_one();
        return Ok(subscription);
    }
}

#[async_trait]
impl ClientStateManager for StateManager {
    async fn add_action(
        &self,
        action_info: ActionInfo,
    ) -> Result<Arc<dyn ActionStateResult>, Error> {
        let mut inner = self.inner.lock().await;
        inner.inner_add_operation(action_info)
    }

    async fn filter_operations(
        &self,
        _filter: &OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        todo!();
        // let inner = self.inner.lock().await;
        // // TODO(adams): Build out a proper filter for other fields for state, at the moment
        // //  this only supports the unique qualifier.
        // let unique_qualifier = filter
        //     .unique_qualifier
        //     .as_ref()
        //     .err_tip(|| "No unique qualifier provided")?;
        // let maybe_awaited_action = inner
        //     .queued_actions_set
        //     .get(unique_qualifier)
        //     .and_then(|action_info| inner.queued_actions.get(action_info))
        //     .or_else(|| inner.active_actions.get(unique_qualifier));

        // let Some(awaited_action) = maybe_awaited_action else {
        //     return Ok(Box::pin(stream::empty()));
        // };

        // let rx = awaited_action.notify_channel.subscribe();
        // let action_result: [Arc<dyn ActionStateResult>; 1] =
        //     [Arc::new(ClientActionStateResult::new(rx))];
        // Ok(Box::pin(stream::iter(action_result)))
    }
}

#[async_trait]
impl WorkerStateManager for StateManager {
    async fn update_operation(
        &self,
        operation_id: &OperationId,
        worker_id: &WorkerId,
        action_stage_result: Result<ActionStage, Error>,
    ) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        inner.inner_update_operation(operation_id, Some(worker_id), action_stage_result)
    }
}

#[async_trait]
impl MatchingEngineStateManager for StateManager {
    async fn filter_operations(
        &self,
        _filter: &OperationFilter, // TODO(adam): reference filter
    ) -> Result<ActionStateResultStream, Error> {
        todo!();
        // let inner = self.inner.lock().await;
        // // TODO(adams): use OperationFilter vs directly encoding it.
        // let action_infos =
        //     inner
        //         .queued_actions
        //         .iter()
        //         .rev()
        //         .map(|(action_info, awaited_action)| {
        //             let cloned_action_info = action_info.clone();
        //             Arc::new(MatchingEngineActionStateResult::new(
        //                 cloned_action_info,
        //                 awaited_action.notify_channel.subscribe(),
        //             )) as Arc<dyn ActionStateResult>
        //         });

        // let action_infos: Vec<Arc<dyn ActionStateResult>> = action_infos.collect();
        // Ok(Box::pin(stream::iter(action_infos)))
    }

    async fn assign_operation(
        &self,
        operation_id: &OperationId,
        worker_id_or_reason_for_unsassign: Result<&WorkerId, Error>,
    ) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;

        let (maybe_worker_id, stage_result) = match worker_id_or_reason_for_unsassign {
            Ok(worker_id) => (Some(worker_id), Ok(ActionStage::Executing)),
            Err(err) => (None, Err(err)),
        };
        inner.inner_update_operation(operation_id, maybe_worker_id, stage_result)
    }

    async fn remove_operation(&self, _operation_id: OperationId) -> Result<(), Error> {
        todo!()
    }
}
