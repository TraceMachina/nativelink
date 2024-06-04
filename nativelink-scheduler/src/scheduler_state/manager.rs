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

use std::collections::BTreeMap;
use std::sync::Arc;

use hashbrown::{HashMap, HashSet};
use nativelink_util::action_messages::ActionInfo;

use crate::scheduler_state::awaited_action::AwaitedAction;
use crate::scheduler_state::completed_action::CompletedAction;
use crate::scheduler_state::workers::Workers;

pub(crate) struct Manager {
    // BTreeMap uses `cmp` to do it's comparisons, this is a problem because we want to sort our
    // actions based on priority and insert timestamp but also want to find and join new actions
    // onto already executing (or queued) actions. We don't know the insert timestamp of queued
    // actions, so we won't be able to find it in a BTreeMap without iterating the entire map. To
    // get around this issue, we use two containers, one that will search using `Eq` which will
    // only match on the `unique_qualifier` field, which ignores fields that would prevent
    // multiplexing, and another which uses `Ord` for sorting.
    //
    // Important: These two fields must be kept in-sync, so if you modify one, you likely need to
    // modify the other.
    pub(crate) queued_actions_set: HashSet<Arc<ActionInfo>>,
    pub(crate) queued_actions: BTreeMap<Arc<ActionInfo>, AwaitedAction>,
    pub(crate) workers: Workers,
    pub(crate) active_actions: HashMap<Arc<ActionInfo>, AwaitedAction>,
    // These actions completed recently but had no listener, they might have
    // completed while the caller was thinking about calling wait_execution, so
    // keep their completion state around for a while to send back.
    // TODO(#192) Revisit if this is the best way to handle recently completed actions.
    pub(crate) recently_completed_actions: HashSet<CompletedAction>,
}
