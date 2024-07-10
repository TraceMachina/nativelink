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

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::SystemTime;

use nativelink_config::stores::EvictionPolicy;
use nativelink_error::{make_err, make_input_err, Code, Error, ResultExt};
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionState, ActionUniqueKey, ActionUniqueQualifier,
    ClientOperationId, OperationId,
};
use nativelink_util::evicting_map::EvictingMap;
use tokio::sync::{mpsc, watch};
use tracing::{event, Level};

use super::client_awaited_action::ClientAwaitedAction;
use super::{AwaitedAction, SortedAwaitedAction};

#[derive(Default)]
struct SortedAwaitedActions {
    unknown: BTreeSet<SortedAwaitedAction>,
    cache_check: BTreeSet<SortedAwaitedAction>,
    queued: BTreeSet<SortedAwaitedAction>,
    executing: BTreeSet<SortedAwaitedAction>,
    completed: BTreeSet<SortedAwaitedAction>,
    completed_from_cache: BTreeSet<SortedAwaitedAction>,
}

/// The database for storing the state of all actions.
/// IMPORTANT: Any time an item is removed from
/// [`AwaitedActionDb::client_operation_to_awaited_action`], it must
/// also remove the entries from all the other maps.
pub struct AwaitedActionDb {
    /// A lookup table to lookup the state of an action by its client operation id.
    client_operation_to_awaited_action:
        EvictingMap<ClientOperationId, Arc<ClientAwaitedAction>, SystemTime>,

    /// A lookup table to lookup the state of an action by its worker operation id.
    operation_id_to_awaited_action: HashMap<OperationId, Arc<AwaitedAction>>,

    /// A lookup table to lookup the state of an action by its unique qualifier.
    action_info_hash_key_to_awaited_action: HashMap<ActionUniqueKey, Arc<AwaitedAction>>,

    /// A sorted set of [`AwaitedAction`]s. A wrapper is used to perform sorting
    /// based on the [`AwaitedActionSortKey`] of the [`AwaitedAction`].
    ///
    /// See [`AwaitedActionSortKey`] for more information on the ordering.
    sorted_action_info_hash_keys: SortedAwaitedActions,
}

#[allow(clippy::mutable_key_type)]
impl AwaitedActionDb {
    pub fn new(eviction_config: &EvictionPolicy) -> Self {
        Self {
            client_operation_to_awaited_action: EvictingMap::new(
                eviction_config,
                SystemTime::now(),
            ),
            operation_id_to_awaited_action: HashMap::new(),
            action_info_hash_key_to_awaited_action: HashMap::new(),
            sorted_action_info_hash_keys: SortedAwaitedActions::default(),
        }
    }

    /// Refreshes/Updates the time to live of the [`ClientOperationId`] in
    /// the [`EvictingMap`] by touching the key.
    pub async fn refresh_client_operation_id(
        &self,
        client_operation_id: &ClientOperationId,
    ) -> bool {
        self.client_operation_to_awaited_action
            .size_for_key(client_operation_id)
            .await
            .is_some()
    }

    pub async fn get_by_client_operation_id(
        &self,
        client_operation_id: &ClientOperationId,
    ) -> Option<Arc<ClientAwaitedAction>> {
        self.client_operation_to_awaited_action
            .get(client_operation_id)
            .await
    }

    /// When a client operation is dropped, we need to remove it from the
    /// other maps and update the listening clients count on the [`AwaitedAction`].
    pub fn on_client_operations_drop(
        &mut self,
        awaited_actions: impl IntoIterator<Item = Arc<AwaitedAction>>,
    ) {
        for awaited_action in awaited_actions.into_iter() {
            if awaited_action.get_listening_clients() != 0 {
                // We still have other clients listening to this action.
                continue;
            }

            let operation_id = awaited_action.get_operation_id();

            // Cleanup operation_id_to_awaited_action.
            if self
                .operation_id_to_awaited_action
                .remove(&operation_id)
                .is_none()
            {
                event!(
                    Level::ERROR,
                    ?operation_id,
                    ?awaited_action,
                    "operation_id_to_awaited_action and client_operation_to_awaited_action are out of sync",
                );
            }

            // Cleanup action_info_hash_key_to_awaited_action if it was marked cached.
            let action_info = awaited_action.get_action_info();
            match &action_info.unique_qualifier {
                ActionUniqueQualifier::Cachable(action_key) => {
                    let maybe_awaited_action = self
                        .action_info_hash_key_to_awaited_action
                        .remove(action_key);
                    if maybe_awaited_action.is_none() {
                        event!(
                            Level::ERROR,
                            ?operation_id,
                            ?awaited_action,
                            ?action_key,
                            "action_info_hash_key_to_awaited_action and operation_id_to_awaited_action are out of sync",
                        );
                    }
                }
                ActionUniqueQualifier::Uncachable(_action_key) => {
                    // This Operation should not be in the hash_key map.
                }
            }

            // Cleanup sorted_awaited_action.
            let sort_info = awaited_action.get_sort_info();
            let sort_key = sort_info.get_previous_sort_key();
            let sort_map_for_state =
                self.get_sort_map_for_state(&awaited_action.get_current_state().stage);
            drop(sort_info);
            let maybe_sorted_awaited_action = sort_map_for_state.take(&SortedAwaitedAction {
                sort_key,
                awaited_action,
            });
            if maybe_sorted_awaited_action.is_none() {
                event!(
                    Level::ERROR,
                    ?operation_id,
                    ?sort_key,
                    "Expected maybe_sorted_awaited_action to have {sort_key:?}",
                );
            }
        }
    }

    pub fn get_all_awaited_actions(&self) -> impl Iterator<Item = &Arc<AwaitedAction>> {
        self.operation_id_to_awaited_action.values()
    }

    pub fn get_by_operation_id(&self, operation_id: &OperationId) -> Option<&Arc<AwaitedAction>> {
        self.operation_id_to_awaited_action.get(operation_id)
    }

    pub fn get_cache_check_actions(&self) -> &BTreeSet<SortedAwaitedAction> {
        &self.sorted_action_info_hash_keys.cache_check
    }

    pub fn get_queued_actions(&self) -> &BTreeSet<SortedAwaitedAction> {
        &self.sorted_action_info_hash_keys.queued
    }

    pub fn get_executing_actions(&self) -> &BTreeSet<SortedAwaitedAction> {
        &self.sorted_action_info_hash_keys.executing
    }

    pub fn get_completed_actions(&self) -> &BTreeSet<SortedAwaitedAction> {
        &self.sorted_action_info_hash_keys.completed
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
        let newly_inserted = match stage {
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
        if !newly_inserted {
            event!(
                Level::ERROR,
                "Tried to insert an action that was already in the sorted map. This should never happen.",
            );
        }
    }

    /// Sets the state of the action to the provided `action_state` and notifies all listeners.
    /// If the action has no more listeners, returns `false`.
    pub fn set_action_state(
        &mut self,
        awaited_action: Arc<AwaitedAction>,
        new_action_state: Arc<ActionState>,
    ) -> bool {
        // We need to first get a lock on the awaited action to ensure
        // another operation doesn't update it while we are looking up
        // the sorted key.
        let sort_info = awaited_action.get_sort_info();
        let old_state = awaited_action.get_current_state();

        let has_listeners = awaited_action.set_current_state(new_action_state.clone());

        if !old_state.stage.is_same_stage(&new_action_state.stage) {
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

            self.insert_sort_map_for_stage(&new_action_state.stage, sorted_awaited_action);
        }
        has_listeners
    }

    pub async fn subscribe_or_add_action(
        &mut self,
        client_operation_id: ClientOperationId,
        action_info: Arc<ActionInfo>,
        client_operation_drop_tx: &mpsc::UnboundedSender<Arc<AwaitedAction>>,
    ) -> watch::Receiver<Arc<ActionState>> {
        // Check to see if the action is already known and subscribe if it is.
        let subscription_result = self
            .try_subscribe(
                &client_operation_id,
                &action_info.unique_qualifier,
                action_info.priority,
                client_operation_drop_tx,
            )
            .await;
        let action_info = match subscription_result {
            Ok(subscription) => return subscription,
            // TODO!(we should not ignore the error here.)
            Err(_) => action_info,
        };

        let maybe_unique_key = match &action_info.unique_qualifier {
            ActionUniqueQualifier::Cachable(unique_key) => Some(unique_key.clone()),
            ActionUniqueQualifier::Uncachable(_unique_key) => None,
        };
        let (awaited_action, sort_key, subscription) =
            AwaitedAction::new_with_subscription(action_info);
        let awaited_action = Arc::new(awaited_action);
        self.client_operation_to_awaited_action
            .insert(
                client_operation_id,
                Arc::new(ClientAwaitedAction::new(
                    awaited_action.clone(),
                    client_operation_drop_tx.clone(),
                )),
            )
            .await;
        // Note: We only put items in the map that are cachable.
        if let Some(unique_key) = maybe_unique_key {
            self.action_info_hash_key_to_awaited_action
                .insert(unique_key, awaited_action.clone());
        }
        self.operation_id_to_awaited_action
            .insert(awaited_action.get_operation_id(), awaited_action.clone());

        self.insert_sort_map_for_stage(
            &awaited_action.get_current_state().stage,
            SortedAwaitedAction {
                sort_key,
                awaited_action,
            },
        );
        subscription
    }

    async fn try_subscribe(
        &mut self,
        client_operation_id: &ClientOperationId,
        unique_qualifier: &ActionUniqueQualifier,
        priority: i32,
        client_operation_drop_tx: &mpsc::UnboundedSender<Arc<AwaitedAction>>,
    ) -> Result<watch::Receiver<Arc<ActionState>>, Error> {
        let unique_key = match unique_qualifier {
            ActionUniqueQualifier::Cachable(unique_key) => unique_key,
            ActionUniqueQualifier::Uncachable(_unique_key) => {
                return Err(make_err!(
                    Code::InvalidArgument,
                    "Cannot subscribe to an existing item when skip_cache_lookup is true."
                ));
            }
        };

        let awaited_action = self
            .action_info_hash_key_to_awaited_action
            .get(unique_key)
            .ok_or(make_input_err!(
                "Could not find existing action with name: {unique_qualifier}"
            ))
            .err_tip(|| {
                format!(
                    "In AwaitedActionDb::try_subscribe - {client_operation_id} - {unique_key:?}"
                )
            })?;

        // Do not subscribe if the action is already completed,
        // this is the responsibility of the CacheLookupScheduler.
        if awaited_action.get_current_state().stage.is_finished() {
            return Err(make_input_err!(
                "Subscribing an item that is already completed should be handled by CacheLookupScheduler. {}",
                format!("State was: {:?} - client_id: {}", awaited_action.get_current_state(),
                client_operation_id
            )));
        }
        let awaited_action = awaited_action.clone();
        if let Some(sort_info_lock) = awaited_action.upgrade_priority(priority) {
            let state = awaited_action.get_current_state();
            let maybe_sorted_awaited_action =
                self.get_sort_map_for_state(&state.stage)
                    .take(&SortedAwaitedAction {
                        sort_key: sort_info_lock.get_previous_sort_key(),
                        awaited_action: awaited_action.clone(),
                    });
            let Some(mut sorted_awaited_action) = maybe_sorted_awaited_action else {
                // TODO!(Either use event on all of the above error here, but both is overkill).
                let err = make_err!(
                    Code::Internal,
                    "sorted_action_info_hash_keys and action_info_hash_key_to_awaited_action are out of sync");
                event!(Level::ERROR, ?unique_qualifier, ?awaited_action, "{err:?}",);
                return Err(err);
            };
            sorted_awaited_action.sort_key = sort_info_lock.get_new_sort_key();
            self.insert_sort_map_for_stage(&state.stage, sorted_awaited_action);
        }

        let subscription = awaited_action.subscribe();

        self.client_operation_to_awaited_action
            .insert(
                client_operation_id.clone(),
                Arc::new(ClientAwaitedAction::new(
                    awaited_action,
                    client_operation_drop_tx.clone(),
                )),
            )
            .await;

        Ok(subscription)
    }
}
