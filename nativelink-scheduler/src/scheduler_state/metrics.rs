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

use nativelink_util::metrics_utils::CounterWithTime;

#[derive(Default)]
pub(crate) struct Metrics {
    pub(crate) update_action_missing_action_result: CounterWithTime,
    pub(crate) update_action_from_wrong_worker: CounterWithTime,
    pub(crate) update_action_no_more_listeners: CounterWithTime,
    pub(crate) workers_evicted: CounterWithTime,
    pub(crate) workers_evicted_with_running_action: CounterWithTime,
    pub(crate) retry_action: CounterWithTime,
    pub(crate) retry_action_max_attempts_reached: CounterWithTime,
    pub(crate) retry_action_no_more_listeners: CounterWithTime,
    pub(crate) retry_action_but_action_missing: CounterWithTime,
}
