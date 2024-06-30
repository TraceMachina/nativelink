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
    pub(crate) update_action_missing_action_result: CounterWithTime,
    pub(crate) update_action_from_wrong_worker: CounterWithTime,
    pub(crate) update_action_no_more_listeners: CounterWithTime,
    pub(crate) update_action_with_internal_error: CounterWithTime,
    pub(crate) update_action_with_internal_error_no_action: CounterWithTime,
    pub(crate) update_action_with_internal_error_backpressure: CounterWithTime,
    pub(crate) update_action_with_internal_error_from_wrong_worker: CounterWithTime,
    pub(crate) workers_evicted: CounterWithTime,
    pub(crate) workers_evicted_with_running_action: CounterWithTime,
    pub(crate) retry_action: CounterWithTime,
    pub(crate) retry_action_max_attempts_reached: CounterWithTime,
    pub(crate) retry_action_no_more_listeners: CounterWithTime,
    pub(crate) retry_action_but_action_missing: CounterWithTime,
    pub(crate) update_operation_with_internal_error: CounterWithTime,
    pub(crate) update_operation_with_internal_error_no_action: CounterWithTime,
    pub(crate) update_operation_with_internal_error_backpressure: CounterWithTime,
    pub(crate) update_operation_with_internal_error_from_wrong_worker: CounterWithTime,
}

impl Metrics {
    pub fn gather_metrics(&self, c: &mut CollectorState) {
        {
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
        {
            c.publish_with_labels(
                "update_action_errors",
                &self.update_action_missing_action_result,
                "Stats about errors when worker sends update_action() to scheduler. These errors are not complete, just the most common.",
                vec![("result".into(), "missing_action_result".into())],
            );
            c.publish_with_labels(
                "update_action_errors",
                &self.update_action_from_wrong_worker,
                "Stats about errors when worker sends update_action() to scheduler. These errors are not complete, just the most common.",
                vec![("result".into(), "from_wrong_worker".into())],
            );
            c.publish_with_labels(
                "update_action_errors",
                &self.update_action_no_more_listeners,
                "Stats about errors when worker sends update_action() to scheduler. These errors are not complete, just the most common.",
                vec![("result".into(), "no_more_listeners".into())],
            );
        }
        {
            c.publish(
                "update_action_with_internal_error",
                &self.update_action_with_internal_error,
                "The number of times update_action_with_internal_error was triggered.",
            );
            c.publish_with_labels(
                "update_action_with_internal_error_errors",
                &self.update_action_with_internal_error_no_action,
                "Stats about what errors caused update_action_with_internal_error() in scheduler.",
                vec![("result".into(), "no_action".into())],
            );
            c.publish_with_labels(
                "update_action_with_internal_error_errors",
                &self.update_action_with_internal_error_backpressure,
                "Stats about what errors caused update_action_with_internal_error() in scheduler.",
                vec![("result".into(), "backpressure".into())],
            );
            c.publish_with_labels(
                "update_action_with_internal_error_errors",
                &self.update_action_with_internal_error_from_wrong_worker,
                "Stats about what errors caused update_action_with_internal_error() in scheduler.",
                vec![("result".into(), "from_wrong_worker".into())],
            );
        }
        {
            c.publish(
                "workers_evicted_total",
                &self.workers_evicted,
                "The number of workers evicted from scheduler.",
            );
            c.publish(
                "workers_evicted_with_running_action",
                &self.workers_evicted_with_running_action,
                "The number of jobs cancelled because worker was evicted from scheduler.",
            );
        }
        {
            c.publish_with_labels(
                "retry_action",
                &self.retry_action,
                "Stats about retry_action().",
                vec![("result".into(), "success".into())],
            );
            c.publish_with_labels(
                "retry_action",
                &self.retry_action_max_attempts_reached,
                "Stats about retry_action().",
                vec![("result".into(), "max_attempts_reached".into())],
            );
            c.publish_with_labels(
                "retry_action",
                &self.retry_action_no_more_listeners,
                "Stats about retry_action().",
                vec![("result".into(), "no_more_listeners".into())],
            );
            c.publish_with_labels(
                "retry_action",
                &self.retry_action_but_action_missing,
                "Stats about retry_action().",
                vec![("result".into(), "action_missing".into())],
            );
        }
    }
}
