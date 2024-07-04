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
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use nativelink_util::action_messages::{
    ActionInfo, ActionInfoHashKey, ActionStage, ActionState, OperationId, WorkerId,
};
use nativelink_util::evicting_map::InstantWrapper;
use nativelink_util::metrics_utils::{CollectorState, MetricsComponent};
use parking_lot::{RwLock, RwLockReadGuard, RwLockUpgradableReadGuard, RwLockWriteGuard};
use static_assertions::{assert_eq_size, const_assert, const_assert_eq};
use tokio::sync::watch;

enum ReadOrWriteGuard<'a, T> {
    Read(RwLockReadGuard<'a, T>),
    Write(RwLockWriteGuard<'a, T>),
}

#[derive(Debug)]
struct SortInfo {
    priority: i32,
    sort_key: AwaitedActionSortKey,
}

/// This struct's main purpose is to allow the caller of `set_priority` to
/// get the previous sort key and perform any additional operations
/// that may be needed after the sort key has been updated without allowing
/// anyone else to read or modify the sort key until the caller is done.
pub struct SortInfoLock<'a> {
    previous_sort_key: AwaitedActionSortKey,
    new_sort_info: ReadOrWriteGuard<'a, SortInfo>,
}

impl SortInfoLock<'_> {
    /// Gets the previous sort key of the action.
    pub fn get_previous_sort_key(&self) -> AwaitedActionSortKey {
        self.previous_sort_key
    }

    /// Gets the new sort key of the action.
    pub fn get_new_sort_key(&self) -> AwaitedActionSortKey {
        match &self.new_sort_info {
            ReadOrWriteGuard::Read(sort_info) => sort_info.sort_key,
            ReadOrWriteGuard::Write(sort_info) => sort_info.sort_key,
        }
    }
}

/// An action that is being awaited on and last known state.
#[derive(Debug)]
pub struct AwaitedAction {
    /// The action that is being awaited on.
    action_info: Arc<ActionInfo>,

    // The unique identifier of the operation.
    // TODO(operation_id should be stored here).
    // operation_id: OperationId,
    /// The data that is used to sort the action in the queue.
    /// The first item in the tuple is the current priority,
    /// the second item is the sort key.
    sort_info: RwLock<SortInfo>,

    /// Number of attempts the job has been tried.
    attempts: AtomicUsize,

    /// The time the action was last updated.
    last_worker_updated_timestamp: AtomicU64,

    /// Worker that is currently running this action, None if unassigned.
    worker_id: RwLock<Option<WorkerId>>,

    /// The channel to notify subscribers of state changes when updated, completed or retrying.
    notify_channel: watch::Sender<Arc<ActionState>>,
}

impl AwaitedAction {
    pub fn new_with_subscription(
        action_info: Arc<ActionInfo>,
    ) -> (
        Self,
        AwaitedActionSortKey,
        watch::Receiver<Arc<ActionState>>,
    ) {
        let stage = ActionStage::Queued;
        let sort_key = AwaitedActionSortKey::new_with_action_hash(
            action_info.priority,
            &action_info.insert_timestamp,
            &action_info.unique_qualifier,
        );
        let sort_info = RwLock::new(SortInfo {
            priority: action_info.priority,
            sort_key,
        });
        let current_state = Arc::new(ActionState {
            stage,
            id: OperationId::new(action_info.unique_qualifier.clone()),
        });
        let (tx, rx) = watch::channel(current_state);
        (
            Self {
                action_info,
                notify_channel: tx,
                sort_info,
                attempts: AtomicUsize::new(0),
                last_worker_updated_timestamp: AtomicU64::new(SystemTime::now().unix_timestamp()),
                worker_id: RwLock::new(None),
            },
            sort_key,
            rx,
        )
    }

    /// Gets the action info.
    pub fn get_action_info(&self) -> &Arc<ActionInfo> {
        &self.action_info
    }

    pub fn get_operation_id(&self) -> OperationId {
        self.notify_channel.borrow().id.clone()
    }

    pub fn get_last_worker_updated_timestamp(&self) -> SystemTime {
        let timestamp = self.last_worker_updated_timestamp.load(Ordering::Acquire);
        SystemTime::UNIX_EPOCH + Duration::from_secs(timestamp)
    }

    /// Updates the timestamp of the action.
    fn update_worker_timestamp(&self) {
        self.last_worker_updated_timestamp
            .store(SystemTime::now().unix_timestamp(), Ordering::Release);
    }

    /// Sets the priority of the action.
    ///
    /// If the priority was already set to `new_priority`, this function will
    /// return `None`. If the priority was different, it will return a
    /// struct that contains the previous sort key and the new sort key and
    /// will hold a lock preventing anyone else from reading or modifying the
    /// sort key until the result is dropped.
    #[must_use]
    pub fn set_priority<'a>(&'a self, new_priority: i32) -> Option<SortInfoLock<'a>> {
        let sort_info_lock = self.sort_info.upgradable_read();
        if sort_info_lock.priority == new_priority {
            return None;
        }
        let mut sort_info_lock = RwLockUpgradableReadGuard::upgrade(sort_info_lock);
        let previous_sort_key = sort_info_lock.sort_key;
        sort_info_lock.priority = new_priority;
        sort_info_lock.sort_key = AwaitedActionSortKey::new_with_action_hash(
            new_priority,
            &self.action_info.insert_timestamp,
            &self.action_info.unique_qualifier,
        );
        Some(SortInfoLock {
            previous_sort_key,
            new_sort_info: ReadOrWriteGuard::Write(sort_info_lock),
        })
    }

    /// Gets the sort info of the action.
    pub fn get_sort_info<'a>(&'a self) -> SortInfoLock<'a> {
        let sort_info = self.sort_info.read();
        SortInfoLock {
            previous_sort_key: sort_info.sort_key,
            new_sort_info: ReadOrWriteGuard::Read(sort_info),
        }
    }

    /// Gets the number of times the action has been attempted
    /// to be executed.
    pub fn get_attempts(&self) -> usize {
        self.attempts.load(Ordering::Acquire)
    }

    /// Adds one to the number of attempts the action has been tried.
    pub fn inc_attempts(&self) {
        self.attempts.fetch_add(1, Ordering::Release);
    }

    // /// Subtracts one from the number of attempts the action has been tried.
    // pub fn dec_attempts(&self) {
    //     self.attempts.fetch_sub(1, Ordering::Release);
    // }

    /// Gets the worker id that is currently processing this action.
    pub fn get_worker_id(&self) -> Option<WorkerId> {
        self.worker_id.read().clone()
    }

    /// Sets the worker id that is currently processing this action.
    pub fn set_worker_id(&self, new_worker_id: Option<WorkerId>) {
        let mut worker_id = self.worker_id.write();
        if *worker_id != new_worker_id {
            self.update_worker_timestamp();
            *worker_id = new_worker_id;
        }
    }

    /// Gets the current state of the action.
    pub fn get_current_state(&self) -> Arc<ActionState> {
        self.notify_channel.borrow().clone()
    }

    /// Sets the current state of the action and notifies subscribers.
    /// Returns true if the state was set, false if there are no subscribers.
    #[must_use]
    // TODO(allada) IMPORTANT: This function should only be visible to ActionActionDb.
    pub(crate) fn set_current_state(&self, state: Arc<ActionState>) -> bool {
        self.update_worker_timestamp();
        // Note: Use `send_replace()`. Using `send()` will not change the value if
        // there are no subscribers.
        self.notify_channel.send_replace(state);
        self.notify_channel.receiver_count() > 0
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<ActionState>> {
        self.notify_channel.subscribe()
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
    const fn new(priority: i32, insert_timestamp: u64, hash: [u8; 4]) -> Self {
        // Shift `new_priority` so [`i32::MIN`] is represented by zero.
        // This makes it so any nagative values are positive, but
        // maintains ordering.
        const MIN_I32: i64 = (i32::MIN as i64).abs();
        let priority = ((priority as i64 + MIN_I32) as u32).to_be_bytes();

        let timestamp = (insert_timestamp ^ u64::MAX).to_be_bytes();

        #[cfg_attr(rustfmt, rustfmt_skip)]
        AwaitedActionSortKey(u128::from_be_bytes([
            priority[0], priority[1], priority[2], priority[3],
            timestamp[0], timestamp[1], timestamp[2], timestamp[3],
            timestamp[4], timestamp[5], timestamp[6], timestamp[7],
            hash[0], hash[1], hash[2], hash[3]
        ]))
    }

    fn new_with_action_hash(
        priority: i32,
        insert_timestamp: &SystemTime,
        action_hash: &ActionInfoHashKey,
    ) -> Self {
        let hash = {
            let mut hasher = DefaultHasher::new();
            ActionInfoHashKey::hash(action_hash, &mut hasher);
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
    AwaitedActionSortKey(0x92345678_6543210fedcba987_9abcdef0).0
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

impl MetricsComponent for AwaitedAction {
    fn gather_metrics(&self, c: &mut CollectorState) {
        c.publish(
            "action_digest",
            &self.action_info.unique_qualifier.action_name(),
            "The digest of the action.",
        );
        c.publish(
            "current_state",
            self.get_current_state().as_ref(),
            "The current stage of the action.",
        );
        c.publish(
            "attempts",
            &self.get_attempts(),
            "The number of attempts this action has tried.",
        );
        c.publish(
            "worker_id",
            &format!(
                "{}",
                self.get_worker_id()
                    .map_or(String::new(), |v| v.to_string())
            ),
            "The current worker processing the action (if any).",
        );
    }
}
