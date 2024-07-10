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

use std::collections::{BTreeSet, VecDeque};
use std::ops::Bound;
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_lock::Mutex;
use async_trait::async_trait;
use futures::stream::{self, unfold};
use nativelink_config::stores::EvictionPolicy;
use nativelink_error::{make_err, Code, Error};
use nativelink_util::action_messages::{
    ActionInfo, ActionResult, ActionStage, ActionState, ActionUniqueQualifier, ClientOperationId,
    ExecutionMetadata, OperationId, WorkerId,
};
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, MatchingEngineStateManager,
    OperationFilter, OperationStageFlags, OrderDirection, WorkerStateManager,
};
use nativelink_util::spawn;
use nativelink_util::task::JoinHandleDropGuard;
use tokio::sync::{mpsc, watch, Notify};
use tracing::{event, Level};

use super::awaited_action_db::{AwaitedAction, AwaitedActionDb, SortedAwaitedAction};
use super::client_action_state_result::ClientActionStateResult;
use super::matching_engine_action_state_result::MatchingEngineActionStateResult;

/// How often the owning database will have the AwaitedAction touched
/// to keep it from being evicted.
const KEEPALIVE_DURATION: Duration = Duration::from_secs(10);

/// Number of client drop events to pull from the stream at a time.
const MAX_CLIENT_DROP_HANDLES_PER_CYCLE: usize = 1024;

fn apply_filter_predicate(awaited_action: &AwaitedAction, filter: &OperationFilter) -> bool {
    // Note: The caller must filter `client_operation_id`.

    if let Some(operation_id) = &filter.operation_id {
        if operation_id != &awaited_action.get_operation_id() {
            return false;
        }
    }

    if filter.worker_id.is_some() && filter.worker_id != awaited_action.get_worker_id() {
        return false;
    }

    {
        let action_info = awaited_action.get_action_info();
        if let Some(filter_unique_key) = &filter.unique_key {
            match &action_info.unique_qualifier {
                ActionUniqueQualifier::Cachable(unique_key) => {
                    if filter_unique_key != unique_key {
                        return false;
                    }
                }
                ActionUniqueQualifier::Uncachable(_) => {
                    return false;
                }
            }
        }
        if let Some(action_digest) = filter.action_digest {
            if action_digest != action_info.digest() {
                return false;
            }
        }
    }

    {
        let last_worker_update_timestamp = awaited_action.get_last_worker_updated_timestamp();
        if let Some(worker_update_before) = filter.worker_update_before {
            if worker_update_before < last_worker_update_timestamp {
                return false;
            }
        }
        let state = awaited_action.get_current_state();
        if let Some(completed_before) = filter.completed_before {
            if state.stage.is_finished() && completed_before < last_worker_update_timestamp {
                return false;
            }
        }
        if filter.stages != OperationStageFlags::Any {
            let stage_flag = match state.stage {
                ActionStage::Unknown => OperationStageFlags::Any,
                ActionStage::CacheCheck => OperationStageFlags::CacheCheck,
                ActionStage::Queued => OperationStageFlags::Queued,
                ActionStage::Executing => OperationStageFlags::Executing,
                ActionStage::Completed(_) => OperationStageFlags::Completed,
                ActionStage::CompletedFromCache(_) => OperationStageFlags::Completed,
            };
            if !filter.stages.intersects(stage_flag) {
                return false;
            }
        }
    }

    true
}

/// Utility struct to create a background task that keeps the client operation id alive.
fn make_client_keepalive_spawn(
    client_operation_id: ClientOperationId,
    inner_weak: Weak<Mutex<MemorySchedulerStateManagerImpl>>,
) -> JoinHandleDropGuard<()> {
    spawn!("client_action_state_result_keepalive", async move {
        loop {
            tokio::time::sleep(KEEPALIVE_DURATION).await;
            let Some(inner) = inner_weak.upgrade() else {
                return; // Nothing to do.
            };
            let inner = inner.lock().await;
            let refresh_success = inner
                .action_db
                .refresh_client_operation_id(&client_operation_id)
                .await;
            if !refresh_success {
                event! {
                    Level::ERROR,
                    ?client_operation_id,
                    "Client operation id not found in MemorySchedulerStateManager::add_action keepalive"
                };
            }
        }
    })
}

/// MemorySchedulerStateManager is responsible for maintaining the state of the scheduler.
/// Scheduler state includes the actions that are queued, active, and recently completed.
/// It also includes the workers that are available to execute actions based on allocation
/// strategy.
struct MemorySchedulerStateManagerImpl {
    /// Database for storing the state of all actions.
    action_db: AwaitedActionDb,

    /// Notify task<->worker matching engine that work needs to be done.
    tasks_change_notify: Arc<Notify>,

    /// Maximum number of times a job can be retried.
    max_job_retries: usize,

    /// Channel to notify when a client operation id is dropped.
    client_operation_drop_tx: mpsc::UnboundedSender<Arc<AwaitedAction>>,

    /// Task to cleanup client operation ids that are no longer being listened to.
    // Note: This has a custom Drop function on it. It should stay alive only while
    // the MemorySchedulerStateManager is alive.
    _client_operation_cleanup_spawn: JoinHandleDropGuard<()>,
}

impl MemorySchedulerStateManagerImpl {
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
                    "Could not find action info MemorySchedulerStateManager::update_operation {}",
                    format!(
                        "for operation_id: {operation_id}, maybe_worker_id: {maybe_worker_id:?}"
                    ),
                )
            })?
            .clone();

        // Make sure we don't update an action that is already completed.
        if awaited_action.get_current_state().stage.is_finished() {
            return Err(make_err!(
                Code::Internal,
                "Action {operation_id:?} is already completed with state {:?} - maybe_worker_id: {:?}",
                awaited_action.get_current_state().stage,
                maybe_worker_id,
            ));
        }

        // Make sure the worker id matches the awaited action worker id.
        // This might happen if the worker sending the update is not the
        // worker that was assigned.
        let awaited_action_worker_id = awaited_action.get_worker_id();
        if awaited_action_worker_id.is_some()
            && maybe_worker_id.is_some()
            && maybe_worker_id != awaited_action_worker_id.as_ref()
        {
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
                if !due_to_backpressure {
                    awaited_action.inc_attempts();
                }

                if awaited_action.get_attempts() > self.max_job_retries {
                    ActionStage::Completed(ActionResult {
                        execution_metadata: ExecutionMetadata {
                            worker: maybe_worker_id.map_or_else(String::default, |v| v.to_string()),
                            ..ExecutionMetadata::default()
                        },
                        error: Some(err.clone().merge(make_err!(
                            Code::Internal,
                            "Job cancelled because it attempted to execute too many times and failed {}",
                            format!("for operation_id: {operation_id}, maybe_worker_id: {maybe_worker_id:?}"),
                        ))),
                        ..ActionResult::default()
                    })
                } else {
                    ActionStage::Queued
                }
            }
        };
        if matches!(stage, ActionStage::Queued) {
            // If the action is queued, we need to unset the worker id regardless of
            // which worker sent the update.
            awaited_action.set_worker_id(None);
        } else {
            awaited_action.set_worker_id(maybe_worker_id.copied());
        }
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

    async fn inner_add_operation(
        &mut self,
        new_client_operation_id: ClientOperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<watch::Receiver<Arc<ActionState>>, Error> {
        let rx = self
            .action_db
            .subscribe_or_add_action(
                new_client_operation_id,
                action_info,
                &self.client_operation_drop_tx,
            )
            .await;
        self.tasks_change_notify.notify_one();
        Ok(rx)
    }
}

#[repr(transparent)]
pub struct MemorySchedulerStateManager {
    inner: Arc<Mutex<MemorySchedulerStateManagerImpl>>,
}

impl MemorySchedulerStateManager {
    pub fn new(
        eviction_config: &EvictionPolicy,
        tasks_change_notify: Arc<Notify>,
        max_job_retries: usize,
    ) -> Self {
        Self {
            inner: Arc::new_cyclic(move |weak_self| -> Mutex<MemorySchedulerStateManagerImpl> {
                let weak_inner = weak_self.clone();
                let (client_operation_drop_tx, mut client_operation_drop_rx) =
                    mpsc::unbounded_channel();
                let client_operation_cleanup_spawn =
                    spawn!("state_manager_client_drop_rx", async move {
                        let mut dropped_client_ids =
                            Vec::with_capacity(MAX_CLIENT_DROP_HANDLES_PER_CYCLE);
                        loop {
                            dropped_client_ids.clear();
                            client_operation_drop_rx
                                .recv_many(
                                    &mut dropped_client_ids,
                                    MAX_CLIENT_DROP_HANDLES_PER_CYCLE,
                                )
                                .await;
                            let Some(inner) = weak_inner.upgrade() else {
                                return; // Nothing to cleanup, our struct is dropped.
                            };
                            let mut inner_mux = inner.lock().await;
                            inner_mux
                                .action_db
                                .on_client_operations_drop(dropped_client_ids.drain(..));
                        }
                    });
                Mutex::new(MemorySchedulerStateManagerImpl {
                    action_db: AwaitedActionDb::new(eviction_config),
                    tasks_change_notify,
                    max_job_retries,
                    client_operation_drop_tx,
                    _client_operation_cleanup_spawn: client_operation_cleanup_spawn,
                })
            }),
        }
    }

    async fn inner_filter_operations<F>(
        &self,
        filter: &OperationFilter,
        to_action_state_result: F,
    ) -> Result<ActionStateResultStream, Error>
    where
        F: Fn(Arc<AwaitedAction>) -> Arc<dyn ActionStateResult> + Send + Sync + 'static,
    {
        fn get_tree_for_stage(
            action_db: &AwaitedActionDb,
            stage: OperationStageFlags,
        ) -> Option<&BTreeSet<SortedAwaitedAction>> {
            match stage {
                OperationStageFlags::CacheCheck => Some(action_db.get_cache_check_actions()),
                OperationStageFlags::Queued => Some(action_db.get_queued_actions()),
                OperationStageFlags::Executing => Some(action_db.get_executing_actions()),
                OperationStageFlags::Completed => Some(action_db.get_completed_actions()),
                _ => None,
            }
        }

        let inner = self.inner.lock().await;

        if let Some(operation_id) = &filter.operation_id {
            return Ok(inner
                .action_db
                .get_by_operation_id(operation_id)
                .filter(|awaited_action| apply_filter_predicate(awaited_action.as_ref(), filter))
                .cloned()
                .map(|awaited_action| -> ActionStateResultStream {
                    Box::pin(stream::once(async move {
                        to_action_state_result(awaited_action)
                    }))
                })
                .unwrap_or_else(|| Box::pin(stream::empty())));
        }
        if let Some(client_operation_id) = &filter.client_operation_id {
            return Ok(inner
                .action_db
                .get_by_client_operation_id(client_operation_id)
                .await
                .filter(|client_awaited_action| {
                    apply_filter_predicate(client_awaited_action.awaited_action().as_ref(), filter)
                })
                .map(|client_awaited_action| -> ActionStateResultStream {
                    Box::pin(stream::once(async move {
                        to_action_state_result(client_awaited_action.awaited_action().clone())
                    }))
                })
                .unwrap_or_else(|| Box::pin(stream::empty())));
        }

        if get_tree_for_stage(&inner.action_db, filter.stages).is_none() {
            let mut all_items: Vec<Arc<AwaitedAction>> = inner
                .action_db
                .get_all_awaited_actions()
                .filter(|awaited_action| apply_filter_predicate(awaited_action.as_ref(), filter))
                .cloned()
                .collect();
            match filter.order_by_priority_direction {
                Some(OrderDirection::Asc) => all_items.sort_unstable_by(|a, b| {
                    a.get_sort_info()
                        .get_new_sort_key()
                        .cmp(&b.get_sort_info().get_new_sort_key())
                }),
                Some(OrderDirection::Desc) => all_items.sort_unstable_by(|a, b| {
                    b.get_sort_info()
                        .get_new_sort_key()
                        .cmp(&a.get_sort_info().get_new_sort_key())
                }),
                None => {}
            }
            return Ok(Box::pin(stream::iter(
                all_items.into_iter().map(to_action_state_result),
            )));
        }

        drop(inner);

        struct State<
            F: Fn(Arc<AwaitedAction>) -> Arc<dyn ActionStateResult> + Send + Sync + 'static,
        > {
            inner: Arc<Mutex<MemorySchedulerStateManagerImpl>>,
            filter: OperationFilter,
            buffer: VecDeque<SortedAwaitedAction>,
            start_key: Bound<SortedAwaitedAction>,
            to_action_state_result: F,
        }
        let state = State {
            inner: self.inner.clone(),
            filter: filter.clone(),
            buffer: VecDeque::new(),
            start_key: Bound::Unbounded,
            to_action_state_result,
        };

        const STREAM_BUFF_SIZE: usize = 64;

        Ok(Box::pin(unfold(state, move |mut state| async move {
            if let Some(sorted_awaited_action) = state.buffer.pop_front() {
                if state.buffer.is_empty() {
                    state.start_key = Bound::Excluded(sorted_awaited_action.clone());
                }
                return Some((
                    (state.to_action_state_result)(sorted_awaited_action.awaited_action),
                    state,
                ));
            }

            let inner = state.inner.lock().await;

            #[allow(clippy::mutable_key_type)]
            let btree = get_tree_for_stage(&inner.action_db, state.filter.stages)
                .expect("get_tree_for_stage() should have already returned Some but in iteration it returned None");

            let range = (state.start_key.as_ref(), Bound::Unbounded);
            if state.filter.order_by_priority_direction == Some(OrderDirection::Asc) {
                btree
                    .range(range)
                    .filter(|item| {
                        apply_filter_predicate(item.awaited_action.as_ref(), &state.filter)
                    })
                    .take(STREAM_BUFF_SIZE)
                    .for_each(|item| state.buffer.push_back(item.clone()));
            } else {
                btree
                    .range(range)
                    .rev()
                    .filter(|item| {
                        apply_filter_predicate(item.awaited_action.as_ref(), &state.filter)
                    })
                    .take(STREAM_BUFF_SIZE)
                    .for_each(|item| state.buffer.push_back(item.clone()));
            }
            drop(inner);
            let sorted_awaited_action = state.buffer.pop_front()?;
            if state.buffer.is_empty() {
                state.start_key = Bound::Excluded(sorted_awaited_action.clone());
            }
            Some((
                (state.to_action_state_result)(sorted_awaited_action.awaited_action),
                state,
            ))
        })))
    }
}

#[async_trait]
impl ClientStateManager for MemorySchedulerStateManager {
    async fn add_action(
        &self,
        client_operation_id: ClientOperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Arc<dyn ActionStateResult>, Error> {
        let mut inner = self.inner.lock().await;
        let rx = inner
            .inner_add_operation(client_operation_id.clone(), action_info.clone())
            .await?;

        let inner_weak = Arc::downgrade(&self.inner);
        Ok(Arc::new(ClientActionStateResult::new(
            action_info,
            rx,
            Some(make_client_keepalive_spawn(client_operation_id, inner_weak)),
        )))
    }

    async fn filter_operations(
        &self,
        filter: &OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        let maybe_client_operation_id = filter.client_operation_id.clone();
        let inner_weak = Arc::downgrade(&self.inner);
        self.inner_filter_operations(filter, move |awaited_action| {
            Arc::new(ClientActionStateResult::new(
                awaited_action.get_action_info().clone(),
                awaited_action.subscribe(),
                maybe_client_operation_id
                    .as_ref()
                    .map(|client_operation_id| {
                        make_client_keepalive_spawn(client_operation_id.clone(), inner_weak.clone())
                    }),
            ))
        })
        .await
    }
}

#[async_trait]
impl WorkerStateManager for MemorySchedulerStateManager {
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
impl MatchingEngineStateManager for MemorySchedulerStateManager {
    async fn filter_operations(
        &self,
        filter: &OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        self.inner_filter_operations(filter, |awaited_action| {
            Arc::new(MatchingEngineActionStateResult::new(awaited_action))
        })
        .await
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
}
