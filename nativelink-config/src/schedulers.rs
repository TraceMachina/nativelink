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

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::serde_utils::{
    convert_duration_with_shellexpand, convert_duration_with_shellexpand_and_negative,
    convert_numeric_with_shellexpand,
};
use crate::stores::{GrpcEndpoint, Retry, StoreRefName};
// Import warm worker pool configuration
#[cfg(feature = "warm-worker-pools")]
use crate::warm_worker_pools::WarmWorkerPoolsConfig;

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerSpec {
    Simple(SimpleSpec),
    Grpc(GrpcSpec),
    CacheLookup(CacheLookupSpec),
    PropertyModifier(PropertyModifierSpec),
}

/// When the scheduler matches tasks to workers that are capable of running
/// the task, this value will be used to determine how the property is treated.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, Hash, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
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
    /// TODO(palfrey) In the future this will be used by the scheduler and worker
    /// to cause the scheduler to prefer certain workers over others, but not
    /// restrict them based on these values.
    Priority,
}

/// When a worker is being searched for to run a job, this will be used
/// on how to choose which worker should run the job when multiple
/// workers are able to run the task.
#[derive(Copy, Clone, Deserialize, Serialize, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum WorkerAllocationStrategy {
    /// Prefer workers that have been least recently used to run a job.
    #[default]
    LeastRecentlyUsed,
    /// Prefer workers that have been most recently used to run a job.
    MostRecentlyUsed,
}

// defaults to every 10s
const fn default_worker_match_logging_interval_s() -> i64 {
    10
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct SimpleSpec {
    /// A list of supported platform properties mapped to how these properties
    /// are used when the scheduler looks for worker nodes capable of running
    /// the task.
    ///
    /// For example, a value of:
    /// ```json
    /// { "cpu_count": "minimum", "cpu_arch": "exact" }
    /// ```
    /// With a job that contains:
    /// ```json
    /// { "cpu_count": "8", "cpu_arch": "arm" }
    /// ```
    /// Will result in the scheduler filtering out any workers that do not have
    /// `"cpu_arch" = "arm"` and filter out any workers that have less than 8 cpu
    /// cores available.
    ///
    /// The property names here must match the property keys provided by the
    /// worker nodes when they join the pool. In other words, the workers will
    /// publish their capabilities to the scheduler when they join the worker
    /// pool. If the worker fails to notify the scheduler of its (for example)
    /// `"cpu_arch"`, the scheduler will never send any jobs to it, if all jobs
    /// have the `"cpu_arch"` label. There is no special treatment of any platform
    /// property labels other and entirely driven by worker configs and this
    /// config.
    pub supported_platform_properties: Option<HashMap<String, PropertyType>>,

    /// The amount of time to retain completed actions in memory for in case
    /// a `WaitExecution` is called after the action has completed.
    /// Default: 60 (seconds)
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub retain_completed_for_s: u32,

    /// Mark operations as completed with error if no client has updated them
    /// within this duration.
    /// Default: 60 (seconds)
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub client_action_timeout_s: u64,

    /// Remove workers from pool once the worker has not responded in this
    /// amount of time in seconds.
    /// Default: 5 (seconds)
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
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

    /// The strategy used to assign workers jobs.
    #[serde(default)]
    pub allocation_strategy: WorkerAllocationStrategy,

    /// The storage backend to use for the scheduler.
    /// Default: memory
    pub experimental_backend: Option<ExperimentalSimpleSchedulerBackend>,

    /// Every N seconds, do logging of worker matching
    /// e.g. "worker busy", "can't find any worker"
    /// Defaults to 10s. Can be set to -1 to disable
    #[serde(
        default = "default_worker_match_logging_interval_s",
        deserialize_with = "convert_duration_with_shellexpand_and_negative"
    )]
    pub worker_match_logging_interval_s: i64,

    /// Optional configuration for warm worker pools (CRI-O based).
    /// When configured, actions matching specific criteria will be routed
    /// to pre-warmed worker containers, significantly reducing build times
    /// for languages with slow cold-start (Java, TypeScript, etc).
    ///
    /// Example:
    /// ```json5
    /// {
    ///   pools: [{
    ///     name: "java-pool",
    ///     language: "jvm",
    ///     container_image: "nativelink-worker-java:latest",
    ///     min_warm_workers: 5,
    ///     max_workers: 50,
    ///     warmup: {
    ///       commands: [{ argv: ["/opt/warmup/jvm-warmup.sh"] }]
    ///     }
    ///   }]
    /// }
    /// ```
    #[cfg(feature = "warm-worker-pools")]
    #[serde(default)]
    pub warm_worker_pools: Option<WarmWorkerPoolsConfig>,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ExperimentalSimpleSchedulerBackend {
    /// Use an in-memory store for the scheduler.
    Memory,
    /// Use a redis store for the scheduler.
    Redis(ExperimentalRedisSchedulerBackend),
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
pub struct ExperimentalRedisSchedulerBackend {
    /// A reference to the redis store to use for the scheduler.
    /// Note: This MUST resolve to a `RedisSpec`.
    pub redis_store: StoreRefName,
}

/// A scheduler that simply forwards requests to an upstream scheduler.  This
/// is useful to use when doing some kind of local action cache or CAS away from
/// the main cluster of workers.  In general, it's more efficient to point the
/// build at the main scheduler directly though.
#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct GrpcSpec {
    /// The upstream scheduler to forward requests to.
    pub endpoint: GrpcEndpoint,

    /// Retry configuration to use when a network request fails.
    #[serde(default)]
    pub retry: Retry,

    /// Limit the number of simultaneous upstream requests to this many.  A
    /// value of zero is treated as unlimited.  If the limit is reached the
    /// request is queued.
    #[serde(default)]
    pub max_concurrent_requests: usize,

    /// The number of connections to make to each specified endpoint to balance
    /// the load over multiple TCP connections.  Default 1.
    #[serde(default)]
    pub connections_per_endpoint: usize,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct CacheLookupSpec {
    /// The reference to the action cache store used to return cached
    /// actions from rather than running them again.
    /// To prevent unintended issues, this store should probably be a `CompletenessCheckingSpec`.
    pub ac_store: StoreRefName,

    /// The nested scheduler to use if cache lookup fails.
    pub scheduler: Box<SchedulerSpec>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct PlatformPropertyAddition {
    /// The name of the property to add.
    pub name: String,
    /// The value to assign to the property.
    pub value: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct PlatformPropertyReplacement {
    /// The name of the property to replace.
    pub name: String,
    /// The the value to match against, if unset then any instance matches.
    #[serde(default)]
    pub value: Option<String>,
    /// The new name of the property.
    pub new_name: String,
    /// The value to assign to the property, if unset will remain the same.
    #[serde(default)]
    pub new_value: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(rename_all = "snake_case")]
pub enum PropertyModification {
    /// Add a property to the action properties.
    Add(PlatformPropertyAddition),
    /// Remove a named property from the action.
    Remove(String),
    /// If a property is found, then replace it with another one.
    Replace(PlatformPropertyReplacement),
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct PropertyModifierSpec {
    /// A list of modifications to perform to incoming actions for the nested
    /// scheduler.  These are performed in order and blindly, so removing a
    /// property that doesn't exist is fine and overwriting an existing property
    /// is also fine.  If adding properties that do not exist in the nested
    /// scheduler is not supported and will likely cause unexpected behaviour.
    pub modifications: Vec<PropertyModification>,

    /// The nested scheduler to use after modifying the properties.
    pub scheduler: Box<SchedulerSpec>,
}
