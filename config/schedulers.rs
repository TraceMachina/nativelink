// Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

use std::collections::HashMap;

use serde::Deserialize;

use serde_utils::convert_numeric_with_shellexpand;

#[allow(non_camel_case_types)]
#[derive(Deserialize, Debug)]
pub enum SchedulerConfig {
    simple(SimpleScheduler),
}

/// When the scheduler matches tasks to workers that are capable of running
/// the task, this value will be used to determine how the property is treated.
#[derive(Deserialize, Debug, Clone, Copy, Hash, Eq, PartialEq)]
pub enum PropertyType {
    /// Requires the platform property to be a u64 and when the scheduler looks
    /// for appropriate worker nodes that are capable of executing the task,
    /// the task will not run on a node that has less than this value.
    Minimum,

    /// Requires the platform property to be a string and when the scheduler
    /// looks for appropriate worker nodes that are capable of executing the
    /// task, the task will not run on a node that does not have this property
    /// set to the value with exact string match.
    Exact,

    /// Does not restrict on this value and instead will be passed to the worker
    /// as an informational piece.
    /// TODO(allada) In the future this will be used by the scheduler and worker
    /// to cause the scheduler to prefer certain workers over others, but not
    /// restrict them based on these values.
    Priority,
}

#[derive(Deserialize, Debug, Default)]
pub struct SimpleScheduler {
    /// A list of supported platform properties mapped to how these properties
    /// are used when the scheduler looks for worker nodes capable of running
    /// the task.
    ///
    /// For example, a value of:
    /// ```
    /// { "cpu_count": "Minimum", "cpu_arch": "Exact" }
    /// ```
    /// With a job that contains:
    /// ```
    /// { "cpu_count": "8", "cpu_arch": "arm" }
    /// ```
    /// Will result in the scheduler filtering out any workers that do not have
    /// "cpu_arch" = "arm" and filter out any workers that have less than 8 cpu
    /// cores available.
    ///
    /// The property names here must match the property keys provided by the
    /// worker nodes when they join the pool. In other words, the workers will
    /// publish their capabilities to the scheduler when they join the worker
    /// pool. If the worker fails to notify the scheduler of it's (for example)
    /// "cpu_arch", the scheduler will never send any jobs to it, if all jobs
    /// have the "cpu_arch" label. There is no special treatment of any platform
    /// property labels other and entirely driven by worker configs and this
    /// config.
    pub supported_platform_properties: Option<HashMap<String, PropertyType>>,

    /// Remove workers from pool once the worker has not responded in this
    /// amount of time in seconds.
    /// Default: 5 (seconds)
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub worker_timeout_s: u64,

    /// If a job returns an internal error or times out this many times when
    /// attempting to run on a worker the scheduler will return the last error
    /// to the client. Jobs will be retried and this configuration is to help
    /// prevent one rogue job from infinitely retrying and taking up a lot of
    /// resources when the task itself is the one causing the server to go
    /// into a bad state.
    /// Default: 3
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_job_retries: usize,
}
