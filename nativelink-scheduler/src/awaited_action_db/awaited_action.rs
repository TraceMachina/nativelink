// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use nativelink_error::{Error, ResultExt, make_input_err};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
};
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionState, OperationId, WorkerId,
};
use nativelink_util::origin_event::{
    BAZEL_METADATA_KEY, OriginMetadata, request_metadata_from_baggage,
};
use opentelemetry::baggage::BaggageExt;
use opentelemetry::context::Context;
use opentelemetry_semantic_conventions::attribute::ENDUSER_ID;
use serde::{Deserialize, Serialize};
use static_assertions::{assert_eq_size, const_assert, const_assert_eq};

/// The version of the awaited action.
/// This number will always increment by one each time
/// the action is updated.
#[derive(Debug, Clone, Copy, Eq, PartialEq, Serialize, Deserialize)]
struct AwaitedActionVersion(i64);

impl MetricsComponent for AwaitedActionVersion {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        Ok(MetricPublishKnownKindData::Counter(u64::from_ne_bytes(
            self.0.to_ne_bytes(),
        )))
    }
}

/// An action that is being awaited on and last known state.
#[derive(Debug, Clone, MetricsComponent, Serialize, Deserialize)]
pub struct AwaitedAction {
    /// The current version of the action.
    #[metric(help = "The version of the AwaitedAction")]
    version: AwaitedActionVersion,

    /// The action that is being awaited on.
    #[metric(help = "The action info of the AwaitedAction")]
    action_info: Arc<ActionInfo>,

    /// The operation id of the action.
    // If you need the client operation id, it may be set in
    // ActionState::operation_id.
    #[metric(help = "The operation id of the AwaitedAction")]
    operation_id: OperationId,

    /// The currentsort key used to order the actions.
    #[metric(help = "The sort key of the AwaitedAction")]
    sort_key: AwaitedActionSortKey,

    /// The time the action was last updated.
    #[metric(help = "The last time the worker updated the AwaitedAction")]
    last_worker_updated_timestamp: SystemTime,

    /// The last time the client sent a keepalive message.
    #[metric(help = "The last time the client sent a keepalive message")]
    last_client_keepalive_timestamp: SystemTime,

    /// Worker that is currently running this action, None if unassigned.
    #[metric(help = "The worker id of the AwaitedAction")]
    worker_id: Option<WorkerId>,

    /// The current state of the action.
    #[metric(help = "The state of the AwaitedAction")]
    state: Arc<ActionState>,

    /// The origin metadata of the action.
    maybe_origin_metadata: Option<OriginMetadata>,

    /// Number of attempts the job has been tried.
    #[metric(help = "The number of attempts the AwaitedAction has been tried")]
    pub attempts: usize,
}

impl AwaitedAction {
    pub fn new(operation_id: OperationId, action_info: Arc<ActionInfo>, now: SystemTime) -> Self {
        let sort_key = AwaitedActionSortKey::new_with_unique_key(
            action_info.priority,
            &action_info.insert_timestamp,
        );
        let action_state = Arc::new(ActionState {
            stage: ActionStage::Queued,
            // Note: We don't use the real client_operation_id here because
            // the only place AwaitedAction::new should ever be called is
            // when the action is first created and this struct will be stored
            // in the database, so we don't want to accidentally leak the
            // client_operation_id to all clients.
            client_operation_id: operation_id.clone(),
            action_digest: action_info.unique_qualifier.digest(),
            last_transition_timestamp: now,
        });

        let ctx = Context::current();
        let baggage = ctx.baggage();

        let maybe_origin_metadata = if baggage.is_empty() {
            None
        } else {
            let bazel_metadata = baggage
                .get(BAZEL_METADATA_KEY)
                .and_then(|value| request_metadata_from_baggage(value.as_str()).ok());
            Some(OriginMetadata {
                identity: baggage
                    .get(ENDUSER_ID)
                    .map(|v| v.as_str().to_string())
                    .unwrap_or_default(),
                bazel_metadata,
            })
        };

        Self {
            version: AwaitedActionVersion(0),
            action_info,
            operation_id,
            sort_key,
            attempts: 0,
            last_worker_updated_timestamp: now,
            last_client_keepalive_timestamp: now,
            maybe_origin_metadata,
            worker_id: None,
            state: action_state,
        }
    }

    pub(crate) const fn version(&self) -> i64 {
        self.version.0
    }

    pub(crate) const fn set_version(&mut self, version: i64) {
        self.version = AwaitedActionVersion(version);
    }

    pub(crate) const fn increment_version(&mut self) {
        self.version = AwaitedActionVersion(self.version.0 + 1);
    }

    pub const fn action_info(&self) -> &Arc<ActionInfo> {
        &self.action_info
    }

    pub const fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    pub(crate) const fn sort_key(&self) -> AwaitedActionSortKey {
        self.sort_key
    }

    pub const fn state(&self) -> &Arc<ActionState> {
        &self.state
    }

    pub fn is_complete(&self) -> bool {
        match &self.state.stage {
            ActionStage::Unknown
            | ActionStage::CacheCheck
            | ActionStage::Queued
            | ActionStage::Executing => false,
            ActionStage::Completed(_) | ActionStage::CompletedFromCache(_) => true,
        }
    }

    pub(crate) const fn maybe_origin_metadata(&self) -> Option<&OriginMetadata> {
        self.maybe_origin_metadata.as_ref()
    }

    pub(crate) const fn worker_id(&self) -> Option<&WorkerId> {
        self.worker_id.as_ref()
    }

    pub(crate) const fn last_worker_updated_timestamp(&self) -> SystemTime {
        self.last_worker_updated_timestamp
    }

    pub(crate) const fn worker_keep_alive(&mut self, now: SystemTime) {
        self.last_worker_updated_timestamp = now;
    }

    pub(crate) const fn last_client_keepalive_timestamp(&self) -> SystemTime {
        self.last_client_keepalive_timestamp
    }

    pub(crate) const fn update_client_keep_alive(&mut self, now: SystemTime) {
        self.last_client_keepalive_timestamp = now;
    }

    pub(crate) fn set_client_operation_id(&mut self, client_operation_id: OperationId) {
        Arc::make_mut(&mut self.state).client_operation_id = client_operation_id;
    }

    /// Sets the worker id that is currently processing this action.
    pub fn set_worker_id(&mut self, new_maybe_worker_id: Option<WorkerId>, now: SystemTime) {
        if self.worker_id != new_maybe_worker_id {
            self.worker_id = new_maybe_worker_id;
            self.worker_keep_alive(now);
        }
    }

    /// Sets the current state of the action and updates the last worker updated timestamp.
    pub fn worker_set_state(&mut self, mut state: Arc<ActionState>, now: SystemTime) {
        core::mem::swap(&mut self.state, &mut state);
        self.worker_keep_alive(now);
    }
}

impl TryFrom<&[u8]> for AwaitedAction {
    type Error = Error;
    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        serde_json::from_slice(value)
            .map_err(|e| make_input_err!("{}", e.to_string()))
            .err_tip(|| "In AwaitedAction::TryFrom::&[u8]")
    }
}

/// The key used to sort the awaited actions.
///
/// The rules for sorting are as follows:
/// 1. priority of the action (higher runs sooner)
/// 2. insert order of the action (older runs sooner)
///
/// The key packs both into one `u128`: priority in bits 64 to 95 and the
/// insert time in nanoseconds since the epoch, inverted, in the low 64 bits,
/// so that reading the set in descending order gives the rules above. The
/// high 32 bits are zero. Before this the low half held whole seconds, so a
/// burst inserted within one second had no order at all and each backend
/// picked its own; nanoseconds order every arrival a clock can tell apart,
/// and the operation id breaks the rest.
///
/// A key written by an older scheduler is a smaller number with the same
/// priority prefix, so during a rolling upgrade its actions sort after new
/// ones of the same priority until they drain. Old keys still parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
pub struct AwaitedActionSortKey(u128);

impl MetricsComponent for AwaitedActionSortKey {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        // The priority and the top of the timestamp; the low bits are noise
        // for a metric.
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the high half is what is wanted"
        )]
        Ok(MetricPublishKnownKindData::Counter((self.0 >> 32) as u64))
    }
}

impl AwaitedActionSortKey {
    const fn new(priority: i32, insert_nanos: u64) -> Self {
        // Shift the signed i32 range [i32::MIN, i32::MAX] to the unsigned u32 range
        // [0, u32::MAX] to preserve ordering when we convert to bytes for sorting.
        let priority_u32 = i32::MIN.unsigned_abs().wrapping_add_signed(priority);

        // Invert the timestamp so the larger the timestamp the lower the
        // number: descending order reads the oldest first.
        let timestamp = insert_nanos ^ u64::MAX;

        Self(((priority_u32 as u128) << 64) | timestamp as u128)
    }

    fn new_with_unique_key(priority: i32, insert_timestamp: &SystemTime) -> Self {
        let nanos = u64::try_from(
            insert_timestamp
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        )
        .unwrap_or(u64::MAX);
        Self::new(priority, nanos)
    }

    pub(crate) const fn as_u128(self) -> u128 {
        self.0
    }
}

// Ensure the size of the sort key is the same as a `u128`.
assert_eq_size!(AwaitedActionSortKey, u128);

const_assert_eq!(
    AwaitedActionSortKey::new(0x1234_5678, 0x9abc_def0_1234_5678).0,
    // Note: Result has 0x12345678 + 0x80000000 = 0x92345678 because we need
    // to shift the `i32::MIN` value to be represented by zero.
    // Note: `6543210f_edcb_a987` are the inverted bits of the timestamp.
    AwaitedActionSortKey(0x9234_5678_6543_210f_edcb_a987).0
);
// Ensure the priority is used as the sort key first.
const_assert!(
    AwaitedActionSortKey::new(i32::MAX, 0).0 > AwaitedActionSortKey::new(i32::MAX - 1, 0).0
);
const_assert!(AwaitedActionSortKey::new(i32::MAX - 1, 0).0 > AwaitedActionSortKey::new(1, 0).0);
const_assert!(AwaitedActionSortKey::new(1, 0).0 > AwaitedActionSortKey::new(0, 0).0);
const_assert!(AwaitedActionSortKey::new(0, 0).0 > AwaitedActionSortKey::new(-1, 0).0);
const_assert!(AwaitedActionSortKey::new(-1, 0).0 > AwaitedActionSortKey::new(i32::MIN + 1, 0).0);
const_assert!(
    AwaitedActionSortKey::new(i32::MIN + 1, 0).0 > AwaitedActionSortKey::new(i32::MIN, 0).0
);

// Ensure the insert timestamp is used as the sort key second, and that one
// nanosecond is enough to tell two arrivals apart.
const_assert!(AwaitedActionSortKey::new(0, u64::MIN).0 > AwaitedActionSortKey::new(0, u64::MAX).0);
const_assert!(AwaitedActionSortKey::new(0, 1_000).0 > AwaitedActionSortKey::new(0, 1_001).0);
