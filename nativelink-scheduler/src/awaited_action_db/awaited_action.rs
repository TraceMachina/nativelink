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

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;
use std::time::SystemTime;

use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionState, ActionUniqueKey, ActionUniqueQualifier, OperationId,
    WorkerId,
};
use nativelink_util::evicting_map::InstantWrapper;
use static_assertions::{assert_eq_size, const_assert, const_assert_eq};

/// An action that is being awaited on and last known state.
#[derive(Debug, Clone)]
pub struct AwaitedAction {
    /// The action that is being awaited on.
    action_info: Arc<ActionInfo>,

    /// The operation id of the action.
    operation_id: OperationId,

    sort_key: AwaitedActionSortKey,

    /// The time the action was last updated.
    last_worker_updated_timestamp: SystemTime,

    /// Worker that is currently running this action, None if unassigned.
    worker_id: Option<WorkerId>,

    state: Arc<ActionState>,

    /// Number of attempts the job has been tried.
    pub attempts: usize,

    /// Number of clients listening to the state of the action.
    pub connected_clients: usize,
}

impl AwaitedAction {
    pub fn new(operation_id: OperationId, action_info: Arc<ActionInfo>) -> Self {
        let unique_key = match &action_info.unique_qualifier {
            ActionUniqueQualifier::Cachable(unique_key) => unique_key,
            ActionUniqueQualifier::Uncachable(unique_key) => unique_key,
        };
        let stage = ActionStage::Queued;
        let sort_key = AwaitedActionSortKey::new_with_unique_key(
            action_info.priority,
            &action_info.insert_timestamp,
            unique_key,
        );
        let state = Arc::new(ActionState {
            stage,
            id: operation_id.clone(),
        });
        Self {
            action_info,
            operation_id,
            sort_key,
            attempts: 0,
            last_worker_updated_timestamp: SystemTime::now(),
            connected_clients: 1,
            worker_id: None,
            state,
        }
    }

    pub fn action_info(&self) -> &Arc<ActionInfo> {
        &self.action_info
    }

    pub fn operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    pub fn sort_key(&self) -> AwaitedActionSortKey {
        self.sort_key
    }

    pub fn state(&self) -> &Arc<ActionState> {
        &self.state
    }

    pub fn worker_id(&self) -> Option<WorkerId> {
        self.worker_id
    }

    pub fn last_worker_updated_timestamp(&self) -> SystemTime {
        self.last_worker_updated_timestamp
    }

    /// Sets the worker id that is currently processing this action.
    pub fn set_worker_id(&mut self, new_maybe_worker_id: Option<WorkerId>) {
        if self.worker_id != new_maybe_worker_id {
            self.worker_id = new_maybe_worker_id;
            self.last_worker_updated_timestamp = SystemTime::now();
        }
    }

    /// Sets the current state of the action and notifies subscribers.
    /// Returns true if the state was set, false if there are no subscribers.
    pub fn set_state(&mut self, mut state: Arc<ActionState>) {
        std::mem::swap(&mut self.state, &mut state);
        self.last_worker_updated_timestamp = SystemTime::now();
    }
}

/// The key used to sort the awaited actions.
///
/// The rules for sorting are as follows:
/// 1. priority of the action
/// 2. insert order of the action (lower = higher priority)
/// 3. (mostly random hash based on the action info)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(transparent)]
pub struct AwaitedActionSortKey(u128);

impl AwaitedActionSortKey {
    #[rustfmt::skip]
    const fn new(priority: i32, insert_timestamp: u64, hash: [u8; 4]) -> Self {
        // Shift `new_priority` so [`i32::MIN`] is represented by zero.
        // This makes it so any nagative values are positive, but
        // maintains ordering.
        const MIN_I32: i64 = (i32::MIN as i64).abs();
        let priority = ((priority as i64 + MIN_I32) as u32).to_be_bytes();

        // Invert our timestamp so the larger the timestamp the lower the number.
        // This makes timestamp descending order instead of ascending.
        let timestamp = (insert_timestamp ^ u64::MAX).to_be_bytes();

        AwaitedActionSortKey(u128::from_be_bytes([
            priority[0], priority[1], priority[2], priority[3],
            timestamp[0], timestamp[1], timestamp[2], timestamp[3],
            timestamp[4], timestamp[5], timestamp[6], timestamp[7],
            hash[0], hash[1], hash[2], hash[3]
        ]))
    }

    fn new_with_unique_key(
        priority: i32,
        insert_timestamp: &SystemTime,
        action_hash: &ActionUniqueKey,
    ) -> Self {
        let hash = {
            let mut hasher = DefaultHasher::new();
            ActionUniqueKey::hash(action_hash, &mut hasher);
            hasher.finish().to_le_bytes()[0..4].try_into().unwrap()
        };
        Self::new(priority, insert_timestamp.unix_timestamp(), hash)
    }
}

// Ensure the size of the sort key is the same as a `u64`.
assert_eq_size!(AwaitedActionSortKey, u128);

const_assert_eq!(
    AwaitedActionSortKey::new(0x1234_5678, 0x9abc_def0_1234_5678, [0x9a, 0xbc, 0xde, 0xf0]).0,
    // Note: Result has 0x12345678 + 0x80000000 = 0x92345678 because we need
    // to shift the `i32::MIN` value to be represented by zero.
    // Note: `6543210fedcba987` are the inverted bits of `9abcdef012345678`.
    // This effectively inverts the priority to now have the highest priority
    // be the lowest timestamps.
    AwaitedActionSortKey(0x9234_5678_6543_210f_edcb_a987_9abc_def0).0
);
// Ensure the priority is used as the sort key first.
const_assert!(
    AwaitedActionSortKey::new(i32::MAX, 0, [0xff; 4]).0
        > AwaitedActionSortKey::new(i32::MAX - 1, 0, [0; 4]).0
);
const_assert!(
    AwaitedActionSortKey::new(i32::MAX - 1, 0, [0xff; 4]).0
        > AwaitedActionSortKey::new(1, 0, [0; 4]).0
);
const_assert!(
    AwaitedActionSortKey::new(1, 0, [0xff; 4]).0 > AwaitedActionSortKey::new(0, 0, [0; 4]).0
);
const_assert!(
    AwaitedActionSortKey::new(0, 0, [0xff; 4]).0 > AwaitedActionSortKey::new(-1, 0, [0; 4]).0
);
const_assert!(
    AwaitedActionSortKey::new(-1, 0, [0xff; 4]).0
        > AwaitedActionSortKey::new(i32::MIN + 1, 0, [0; 4]).0
);
const_assert!(
    AwaitedActionSortKey::new(i32::MIN + 1, 0, [0xff; 4]).0
        > AwaitedActionSortKey::new(i32::MIN, 0, [0; 4]).0
);

// Ensure the insert timestamp is used as the sort key second.
const_assert!(
    AwaitedActionSortKey::new(0, u64::MIN, [0; 4]).0
        > AwaitedActionSortKey::new(0, u64::MAX, [0; 4]).0
);

// Ensure the hash is used as the sort key third.
const_assert!(
    AwaitedActionSortKey::new(0, 0, [0xff, 0xff, 0xff, 0xff]).0
        > AwaitedActionSortKey::new(0, 0, [0; 4]).0
);
const_assert!(
    AwaitedActionSortKey::new(1, 0, [0xff, 0xff, 0xff, 0xff]).0
        > AwaitedActionSortKey::new(0, 0, [0; 4]).0
);
const_assert!(
    AwaitedActionSortKey::new(0, 0, [0; 4]).0 > AwaitedActionSortKey::new(0, 1, [0; 4]).0
);
const_assert!(
    AwaitedActionSortKey::new(0, 0, [0xff, 0xff, 0xff, 0xff]).0
        > AwaitedActionSortKey::new(0, 0, [0; 4]).0
);
