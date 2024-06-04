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

use std::borrow::Borrow;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::SystemTime;

use nativelink_util::action_messages::{ActionInfoHashKey, ActionState, OperationId};
use nativelink_util::metrics_utils::{CollectorState, MetricsComponent};

pub struct CompletedAction {
    pub(crate) completed_time: SystemTime,
    pub(crate) state: Arc<ActionState>,
}

impl Hash for CompletedAction {
    fn hash<H: Hasher>(&self, state: &mut H) {
        OperationId::hash(&self.state.id, state);
    }
}

impl PartialEq for CompletedAction {
    fn eq(&self, other: &Self) -> bool {
        OperationId::eq(&self.state.id, &other.state.id)
    }
}

impl Eq for CompletedAction {}

impl Borrow<OperationId> for CompletedAction {
    #[inline]
    fn borrow(&self) -> &OperationId {
        &self.state.id
    }
}

impl Borrow<ActionInfoHashKey> for CompletedAction {
    #[inline]
    fn borrow(&self) -> &ActionInfoHashKey {
        &self.state.id.unique_qualifier
    }
}

impl MetricsComponent for CompletedAction {
    fn gather_metrics(&self, c: &mut CollectorState) {
        c.publish(
            "completed_timestamp",
            &self.completed_time,
            "The timestamp this action was completed",
        );
        c.publish(
            "current_state",
            self.state.as_ref(),
            "The current stage of the action.",
        );
    }
}
