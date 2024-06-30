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

use std::sync::Arc;

use nativelink_error::Error;
use nativelink_util::action_messages::{ActionInfo, ActionState, WorkerId};
use nativelink_util::metrics_utils::{CollectorState, MetricsComponent};
use tokio::sync::watch;

/// An action that is being awaited on and last known state.
#[derive(Debug)]
pub struct AwaitedAction {
    /// The action that is being awaited on.
    pub(crate) action_info: Arc<ActionInfo>,

    /// The current state of the action.
    pub(crate) current_state: Arc<ActionState>,

    /// The channel to notify subscribers of state changes when updated, completed or retrying.
    pub(crate) notify_channel: watch::Sender<Arc<ActionState>>,

    /// Number of attempts the job has been tried.
    pub(crate) attempts: usize,

    /// Possible last error set by the worker. If empty and attempts is set, it may be due to
    /// something like a worker timeout.
    pub(crate) last_error: Option<Error>,

    /// Worker that is currently running this action, None if unassigned.
    pub(crate) worker_id: Option<WorkerId>,
}

impl MetricsComponent for AwaitedAction {
    fn gather_metrics(&self, c: &mut CollectorState) {
        c.publish(
            "action_digest",
            &self.action_info.unique_qualifier.action_name(),
            "The digest of the action.",
        );
        c.publish(
            "current_state",
            self.current_state.as_ref(),
            "The current stage of the action.",
        );
        c.publish(
            "attempts",
            &self.attempts,
            "The number of attempts this action has tried.",
        );
        c.publish(
            "last_error",
            &format!("{:?}", self.last_error),
            "The last error this action caused from a retry (if any).",
        );
    }
}
