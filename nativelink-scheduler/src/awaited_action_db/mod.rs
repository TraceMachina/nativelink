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
use std::ops::Bound;
use std::sync::Arc;

pub use awaited_action::{AwaitedAction, AwaitedActionSortKey};
use futures::{Future, Stream};
use nativelink_error::{make_input_err, Error, ResultExt};
use nativelink_metric::MetricsComponent;
use nativelink_util::action_messages::{ActionInfo, ActionStage, OperationId};
use serde::{Deserialize, Serialize};

mod awaited_action;

/// A simple enum to represent the state of an AwaitedAction.
#[derive(Debug, Clone, Copy)]
pub enum SortedAwaitedActionState {
    CacheCheck,
    Queued,
    Executing,
    Completed,
}

impl TryFrom<&ActionStage> for SortedAwaitedActionState {
    type Error = Error;
    fn try_from(value: &ActionStage) -> Result<Self, Error> {
        match value {
            ActionStage::CacheCheck => Ok(Self::CacheCheck),
            ActionStage::Executing => Ok(Self::Executing),
            ActionStage::Completed(_) => Ok(Self::Completed),
            ActionStage::Queued => Ok(Self::Queued),
            _ => Err(make_input_err!("Invalid State")),
        }
    }
}

impl TryFrom<ActionStage> for SortedAwaitedActionState {
    type Error = Error;
    fn try_from(value: ActionStage) -> Result<Self, Error> {
        Self::try_from(&value)
    }
}

/// A struct pointing to an AwaitedAction that can be sorted.
#[derive(Debug, Clone, Serialize, Deserialize, MetricsComponent)]
pub struct SortedAwaitedAction {
    #[metric(help = "The sort key of the AwaitedAction")]
    pub sort_key: AwaitedActionSortKey,
    #[metric(help = "The operation id")]
    pub operation_id: OperationId,
}

impl PartialEq for SortedAwaitedAction {
    fn eq(&self, other: &Self) -> bool {
        self.sort_key == other.sort_key && self.operation_id == other.operation_id
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
        self.sort_key
            .cmp(&other.sort_key)
            .then_with(|| self.operation_id.cmp(&other.operation_id))
    }
}

impl std::fmt::Display for SortedAwaitedAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::write(
            f,
            format_args!("{}-{}", self.sort_key.as_u64(), self.operation_id),
        )
    }
}

impl From<&AwaitedAction> for SortedAwaitedAction {
    fn from(value: &AwaitedAction) -> Self {
        Self {
            operation_id: value.operation_id().clone(),
            sort_key: value.sort_key(),
        }
    }
}

impl From<AwaitedAction> for SortedAwaitedAction {
    fn from(value: AwaitedAction) -> Self {
        Self::from(&value)
    }
}

impl TryInto<Vec<u8>> for SortedAwaitedAction {
    type Error = Error;
    fn try_into(self) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(&self)
            .map_err(|e| make_input_err!("{}", e.to_string()))
            .err_tip(|| "In SortedAwaitedAction::TryInto::<Vec<u8>>")
    }
}

impl TryFrom<&[u8]> for SortedAwaitedAction {
    type Error = Error;
    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        serde_json::from_slice(value)
            .map_err(|e| make_input_err!("{}", e.to_string()))
            .err_tip(|| "In AwaitedAction::TryFrom::&[u8]")
    }
}

/// Subscriber that can be used to monitor when AwaitedActions change.
pub trait AwaitedActionSubscriber: Send + Sync + Sized + 'static {
    /// Wait for AwaitedAction to change.
    fn changed(&mut self) -> impl Future<Output = Result<AwaitedAction, Error>> + Send;

    /// Get the current awaited action.
    fn borrow(&self) -> impl Future<Output = Result<AwaitedAction, Error>> + Send;
}

/// A trait that defines the interface for an AwaitedActionDb.
pub trait AwaitedActionDb: Send + Sync + MetricsComponent + Unpin + 'static {
    type Subscriber: AwaitedActionSubscriber;

    /// Get the AwaitedAction by the client operation id.
    fn get_awaited_action_by_id(
        &self,
        client_operation_id: &OperationId,
    ) -> impl Future<Output = Result<Option<Self::Subscriber>, Error>> + Send;

    /// Get all AwaitedActions. This call should be avoided as much as possible.
    fn get_all_awaited_actions(
        &self,
    ) -> impl Future<
        Output = Result<impl Stream<Item = Result<Self::Subscriber, Error>> + Send, Error>,
    > + Send;

    /// Get the AwaitedAction by the operation id.
    fn get_by_operation_id(
        &self,
        operation_id: &OperationId,
    ) -> impl Future<Output = Result<Option<Self::Subscriber>, Error>> + Send;

    /// Get a range of AwaitedActions of a specific state in sorted order.
    fn get_range_of_actions(
        &self,
        state: SortedAwaitedActionState,
        start: Bound<SortedAwaitedAction>,
        end: Bound<SortedAwaitedAction>,
        desc: bool,
    ) -> impl Future<
        Output = Result<impl Stream<Item = Result<Self::Subscriber, Error>> + Send, Error>,
    > + Send;

    /// Process a change changed AwaitedAction and notify any listeners.
    fn update_awaited_action(
        &self,
        new_awaited_action: AwaitedAction,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    /// Add (or join) an action to the AwaitedActionDb and subscribe
    /// to changes.
    fn add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> impl Future<Output = Result<Self::Subscriber, Error>> + Send;
}
