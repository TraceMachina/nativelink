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

use std::borrow::Cow;
use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use bitflags::bitflags;
use futures::Stream;
use nativelink_error::Error;
use nativelink_util::action_messages::{
    ActionInfo, ActionInfoHashKey, ActionStage, ActionState, ClientOperationId, OperationId,
    WorkerId,
};
use nativelink_util::common::DigestInfo;
use tokio::sync::watch;

bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct OperationStageFlags: u32 {
        const CacheCheck = 1 << 1;
        const Queued     = 1 << 2;
        const Executing  = 1 << 3;
        const Completed  = 1 << 4;
        const Any        = u32::MAX;
    }
}

impl Default for OperationStageFlags {
    fn default() -> Self {
        Self::Any
    }
}

#[async_trait]
pub trait ActionStateResult: Send + Sync + 'static {
    // Provides the current state of the action.
    async fn as_state(&self) -> Result<Arc<ActionState>, Error>;
    // Subscribes to the state of the action, receiving updates as they are published.
    async fn as_receiver(&self) -> Result<Cow<'_, watch::Receiver<Arc<ActionState>>>, Error>;
    // Provide result as action info. This behavior will not be supported by all implementations.
    // TODO(adams): Expectation is this to experimental and removed in the future.
    async fn as_action_info(&self) -> Result<Arc<ActionInfo>, Error>;
}

/// The direction in which the results are ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OrderDirection {
    Asc,
    Desc,
}

/// The filters used to query operations from the state manager.
#[derive(Default, Debug, Clone, PartialEq, Eq, Hash)]
pub struct OperationFilter {
    // TODO(adams): create rust builder pattern?
    /// The stage(s) that the operation must be in.
    pub stages: OperationStageFlags,

    /// The client operation id.
    pub client_operation_id: Option<ClientOperationId>,

    /// The operation id.
    pub operation_id: Option<OperationId>,

    /// The worker that the operation must be assigned to.
    pub worker_id: Option<WorkerId>,

    /// The digest of the action that the operation must have.
    pub action_digest: Option<DigestInfo>,

    /// The operation must have it's worker timestamp before this time.
    pub worker_update_before: Option<SystemTime>,

    /// The operation must have been completed before this time.
    pub completed_before: Option<SystemTime>,

    // /// The operation must have it's last client update before this time.
    // NOTE: NOT PART OF ANY FILTERING!
    // pub last_client_update_before: Option<SystemTime>,
    /// The unique key for filtering specific action results.
    pub unique_qualifier: Option<ActionInfoHashKey>,

    /// If the results should be ordered by priority and in which direction.
    pub order_by_priority_direction: Option<OrderDirection>,
}

pub type ActionStateResultStream = Pin<Box<dyn Stream<Item = Arc<dyn ActionStateResult>> + Send>>;

#[async_trait]
pub trait ClientStateManager: Sync + Send + 'static {
    /// Add a new action to the queue or joins an existing action.
    async fn add_action(
        &self,
        client_operation_id: ClientOperationId,
        action_info: ActionInfo,
    ) -> Result<Arc<dyn ActionStateResult>, Error>;

    /// Returns a stream of operations that match the filter.
    async fn filter_operations(
        &self,
        filter: &OperationFilter,
    ) -> Result<ActionStateResultStream, Error>;
}

#[async_trait]
pub trait WorkerStateManager: Sync + Send + 'static {
    /// Update that state of an operation.
    /// The worker must also send periodic updates even if the state
    /// did not change with a modified timestamp in order to prevent
    /// the operation from being considered stale and being rescheduled.
    async fn update_operation(
        &self,
        operation_id: &OperationId,
        worker_id: &WorkerId,
        action_stage: Result<ActionStage, Error>,
    ) -> Result<(), Error>;
}

#[async_trait]
pub trait MatchingEngineStateManager: Sync + Send + 'static {
    /// Returns a stream of operations that match the filter.
    async fn filter_operations(
        &self,
        filter: &OperationFilter,
    ) -> Result<ActionStateResultStream, Error>;

    /// Assign an operation to a worker or unassign it.
    async fn assign_operation(
        &self,
        operation_id: &OperationId,
        worker_id_or_reason_for_unsassign: Result<&WorkerId, Error>,
    ) -> Result<(), Error>;

    /// Remove an operation from the state manager.
    /// It is important to use this function to remove operations
    /// that are no longer needed to prevent memory leaks.
    async fn remove_operation(&self, operation_id: OperationId) -> Result<(), Error>;
}
