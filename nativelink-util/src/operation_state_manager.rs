// Copyright 2024 Trace Machina, Inc. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
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
use nativelink_metric::MetricsComponent;

use crate::action_messages::{
    ActionInfo, ActionStage, ActionState, ActionUniqueKey, OperationId, WorkerId,
};
use crate::common::DigestInfo;
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
    /// When a client last checked in on this operation, where the
    /// implementation tracks it. `None` where it does not.
    async fn client_last_seen(&self) -> Result<Option<SystemTime>, Error> {
        Ok(None)
    }
    /// What a matching pass needs of a queued action beyond its info, in
    /// one read of the record. Empty where the implementation does not
    /// track it.
    async fn dispatch_facts(&self) -> Result<DispatchFacts, Error> {
        Ok(DispatchFacts::default())
    }
}

/// The facts a matching pass reads of a queued action, together, so a pass
/// over the queue borrows each record once.
#[derive(Debug, Default, Clone)]
pub struct DispatchFacts {
    /// Workers a retry of this action should not go back to while another
    /// can take it: the ones it failed or timed out on.
    pub avoid_worker_ids: Vec<WorkerId>,
    /// Nodes a retry of this action should keep off while another can take
    /// it: the nodes of the workers it failed on, as they advertised them.
    pub avoid_node_names: Vec<String>,
    /// When the action last changed stage: since when it has been queued.
    pub queued_since: Option<SystemTime>,
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
    // TODO(palfrey): create rust builder pattern?
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
pub trait ClientStateManager: Sync + Send + Unpin + MetricsComponent + 'static {
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
}

/// The type of update to perform on an operation.
#[derive(Debug, PartialEq, Clone)]
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

    /// Notification that the worker disconnected. Carries the reservation
    /// to requeue with when the loss may have been this action's memory
    /// (RFC-0018); the state manager decides whether the evidence is
    /// enough.
    UpdateWithDisconnect(Option<LossEscalation>),

    /// Notification that the execution stage has completed and it's just IO happening now.
    ExecutionComplete,

    /// The worker never ran the action: it declined the dispatch, the
    /// dispatch never reached it, or it never acknowledged it. Requeue
    /// without counting an attempt; nothing was tried.
    UpdateWithDecline(Decline),

    /// The worker killed the action for a resource it had declared too
    /// little of; requeue it with a larger reservation, within the retry cap.
    UpdateWithEscalation(Escalation),
}

/// Why a dispatched action is going back to the queue untried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decline {
    pub reason: String,
}

/// A reservation to raise before an action runs again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Escalation {
    /// The platform property carrying the reservation.
    pub property: String,
    /// The new value.
    pub value: u64,
    /// Why: the error the kill produced, kept for the client if the cap
    /// ends the escalation.
    pub reason: Error,
    /// Set on the last step: the CPU property and the largest worker's
    /// whole CPU, so the action runs alone there.
    pub cpu: Option<(String, u64)>,
}

/// What a lost worker's action could be requeued with, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LossEscalation {
    /// The next reservation, on the same ladder a memory kill climbs.
    pub escalation: Escalation,
    /// The worker reported this action above its reservation before it
    /// was lost; without that, only a repeated loss escalates.
    pub sampled_over: bool,
}

#[async_trait]
pub trait WorkerStateManager: Sync + Send + MetricsComponent {
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

    /// `update_operation` from a worker whose node is known (its `node_name`
    /// property): a failure the worker is answerable for is remembered
    /// against the node too, so a retry keeps off the whole node, on every
    /// scheduler replica. Implementations that do not track it take the
    /// plain update.
    async fn update_operation_on_node(
        &self,
        operation_id: &OperationId,
        worker_id: &WorkerId,
        _worker_node: Option<&str>,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        self.update_operation(operation_id, worker_id, update).await
    }

    /// Whether the operation is still executing on this worker. False once
    /// it has finished, been requeued or reassigned, or no longer exists.
    async fn is_executing_on_worker(
        &self,
        operation_id: &OperationId,
        worker_id: &WorkerId,
    ) -> Result<bool, Error>;
}

#[async_trait]
pub trait MatchingEngineStateManager: Sync + Send + MetricsComponent {
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

    /// Completes a queued operation with `err`, without retrying it. Does
    /// nothing if the operation is no longer queued. Returns whether the
    /// operation was completed by this call.
    async fn fail_queued_operation(
        &self,
        operation_id: &OperationId,
        err: Error,
    ) -> Result<bool, Error>;
}
