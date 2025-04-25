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

use core::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use bitflags::bitflags;
use futures::Stream;
use nativelink_error::Error;

use crate::action_messages::{
    ActionInfo, ActionStage, ActionState, ActionUniqueKey, OperationId, WorkerId,
};
use crate::common::DigestInfo;
use crate::known_platform_property_provider::KnownPlatformPropertyProvider;
use crate::origin_event::OriginMetadata;

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
    /// Provides the current state of the action.
    async fn as_state(&self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error>;
    /// Waits for the state of the action to change.
    async fn changed(&mut self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error>;
    /// Provide result as action info. This behavior will not be supported by all implementations.
    async fn as_action_info(&self) -> Result<(Arc<ActionInfo>, Option<OriginMetadata>), Error>;
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
    pub client_operation_id: Option<OperationId>,

    /// The operation id.
    pub operation_id: Option<OperationId>,

    /// The worker that the operation must be assigned to.
    pub worker_id: Option<WorkerId>,

    /// The digest of the action that the operation must have.
    pub action_digest: Option<DigestInfo>,

    /// The operation must have its worker timestamp before this time.
    pub worker_update_before: Option<SystemTime>,

    /// The operation must have been completed before this time.
    pub completed_before: Option<SystemTime>,

    /// The unique key for filtering specific action results.
    pub unique_key: Option<ActionUniqueKey>,

    /// If the results should be ordered by priority and in which direction.
    pub order_by_priority_direction: Option<OrderDirection>,
}

pub type ActionStateResultStream<'a> =
    Pin<Box<dyn Stream<Item = Box<dyn ActionStateResult>> + Send + 'a>>;

#[async_trait]
pub trait ClientStateManager: Sync + Send + Unpin + 'static {
    /// Add a new action to the queue or joins an existing action.
    async fn add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error>;

    /// Returns a stream of operations that match the filter.
    async fn filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error>;

    /// Returns the known platform property provider for the given instance
    /// if this implementation supports it.
    // TODO(https://github.com/rust-lang/rust/issues/65991) When this lands we can
    // remove this and have the call sites instead try to cast the ClientStateManager
    // into a KnownPlatformPropertyProvider instead. Rust currently does not support
    // casting traits to other traits.
    fn as_known_platform_property_provider(&self) -> Option<&dyn KnownPlatformPropertyProvider>;
}

/// The type of update to perform on an operation.
#[derive(Debug, PartialEq)]
#[allow(
    clippy::large_enum_variant,
    reason = "TODO Fix this. Breaks on stable, but not on nightly"
)]
pub enum UpdateOperationType {
    /// Notification that the operation is still alive.
    KeepAlive,

    /// Notification that the operation has been updated.
    UpdateWithActionStage(ActionStage),

    /// Notification that the operation has been completed.
    UpdateWithError(Error),
}

#[async_trait]
pub trait WorkerStateManager: Sync + Send {
    /// Update that state of an operation.
    /// The worker must also send periodic updates even if the state
    /// did not change with a modified timestamp in order to prevent
    /// the operation from being considered stale and being rescheduled.
    async fn update_operation(
        &self,
        operation_id: &OperationId,
        worker_id: &WorkerId,
        update: UpdateOperationType,
    ) -> Result<(), Error>;
}

#[async_trait]
pub trait MatchingEngineStateManager: Sync + Send {
    /// Returns a stream of operations that match the filter.
    async fn filter_operations<'a>(
        &'a self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream<'a>, Error>;

    /// Assign an operation to a worker or unassign it.
    async fn assign_operation(
        &self,
        operation_id: &OperationId,
        worker_id_or_reason_for_unassign: Result<&WorkerId, Error>,
    ) -> Result<(), Error>;
}
