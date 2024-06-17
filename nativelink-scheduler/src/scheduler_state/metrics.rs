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

use nativelink_util::metrics_utils::{CollectorState, CounterWithTime};

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) add_action_joined_running_action: CounterWithTime,
    pub(crate) add_action_joined_queued_action: CounterWithTime,
    pub(crate) add_action_new_action_created: CounterWithTime,
}

impl Metrics {
    pub fn gather_metrics(&self, c: &mut CollectorState) {
        c.publish_with_labels(
            "add_action",
            &self.add_action_joined_running_action,
            "Stats about add_action().",
            vec![("result".into(), "joined_running_action".into())],
        );
        c.publish_with_labels(
            "add_action",
            &self.add_action_joined_queued_action,
            "Stats about add_action().",
            vec![("result".into(), "joined_queued_action".into())],
        );
        c.publish_with_labels(
            "add_action",
            &self.add_action_new_action_created,
            "Stats about add_action().",
            vec![("result".into(), "new_action_created".into())],
        );
    }
}
