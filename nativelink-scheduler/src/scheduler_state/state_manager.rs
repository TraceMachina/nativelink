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
use std::pin::Pin;
use std::sync::Arc;
use std::vec::IntoIter;

use async_trait::async_trait;
use futures::stream;
use futures::stream::Iter;
use hashbrown::{HashMap, HashSet};
use nativelink_error::{Error, ResultExt};
use nativelink_util::action_messages::{ActionInfo, ActionStage, ActionState, OperationId};
use tokio::sync::watch;

use crate::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, OperationFilter,
};
use crate::scheduler_state::awaited_action::AwaitedAction;
use crate::scheduler_state::client_action_state_result::ClientActionStateResult;
use crate::scheduler_state::completed_action::CompletedAction;
use crate::scheduler_state::metrics::Metrics;
use crate::scheduler_state::workers::Workers;

#[repr(transparent)]
pub(crate) struct StateManager {
    pub inner: StateManagerImpl,
}

impl StateManager {
    pub(crate) fn new(
        queued_actions_set: HashSet<Arc<ActionInfo>>,
        queued_actions: BTreeMap<Arc<ActionInfo>, AwaitedAction>,
        workers: Workers,
        active_actions: HashMap<Arc<ActionInfo>, AwaitedAction>,
        recently_completed_actions: HashSet<CompletedAction>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            inner: StateManagerImpl {
                queued_actions_set,
                queued_actions,
                workers,
                active_actions,
                recently_completed_actions,
                metrics,
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
        let unique_qualifier = &filter
            .unique_qualifier
            .err_tip(|| "No unique qualifier provided")?;
        let awaited_action: Option<&AwaitedAction> = self
            .inner
            .queued_actions_set
            .get(unique_qualifier)
            .and_then(|action_info| self.inner.queued_actions.get(action_info))
            .or_else(|| self.inner.active_actions.get(unique_qualifier));
        let client_action_state_result: Pin<Box<Iter<IntoIter<Arc<dyn ActionStateResult>>>>> =
            Box::pin(
                awaited_action
                    .map(|aa| {
                        let rx = aa.notify_channel.subscribe();
                        let action_result: Vec<Arc<dyn ActionStateResult>> =
                            vec![Arc::new(ClientActionStateResult::new(rx))];
                        stream::iter(action_result)
                    })
                    .unwrap_or_else(|| stream::iter(Vec::new())),
            );
        Ok(client_action_state_result)
    }
}
