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

#[cfg(feature = "dev-schema")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::serde_utils::{
    convert_duration_with_shellexpand, convert_duration_with_shellexpand_and_negative,
    convert_numeric_with_shellexpand, convert_string_with_shellexpand,
};
use crate::stores::{GrpcEndpoint, Retry, StoreRefName};

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub enum SchedulerSpec {
    Simple(SimpleSpec),
    Grpc(GrpcSpec),
    CacheLookup(CacheLookupSpec),
    PropertyModifier(PropertyModifierSpec),
    HistoricalResource(HistoricalResourceSpec),
}

/// When the scheduler matches tasks to workers that are capable of running
/// the task, this value will be used to determine how the property is treated.
#[derive(Deserialize, Serialize, Debug, Clone, Copy, Hash, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
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

    //// Allows jobs to be requested with said key, but without requiring workers
    //// to have that key
    Ignore,
}

/// When a worker is being searched for to run a job, this will be used
/// on how to choose which worker should run the job when multiple
/// workers are able to run the task.
#[derive(Copy, Clone, Deserialize, Serialize, Debug, Default)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
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

// defaults to every 5s
const fn default_fallback_match_interval_s() -> i64 {
    5
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
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
    /// `"cpu_arch" = "arm"` and filter out any workers that have less than 8 CPU
    /// cores available.
    ///
    /// The property names here must match the property keys provided by the
    /// worker nodes when they join the pool. In other words, the workers will
    /// publish their capabilities to the scheduler when they join the worker
    /// pool. If the worker fails to notify the scheduler of its (for example)
    /// `"cpu_arch"`, the scheduler will never send any jobs to it, if all jobs
    /// have the `"cpu_arch"` label. We have no special treatment of any platform
    /// property labels other and entirely driven by worker configs and this
    /// config.
    ///
    /// Properties that are not listed here are matched dynamically: workers
    /// that declare the key must match the value exactly, and workers that do
    /// not declare the key are not restricted by it. List a property here to
    /// enforce stricter matching.
    pub supported_platform_properties: Option<HashMap<String, PropertyType>>,

    /// The amount of time to retain completed actions for in case
    /// a `WaitExecution` is called after the action has completed.
    /// Default: 60 seconds
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub retain_completed_for_s: u32,

    /// Mark operations as completed with error if no client has updated them
    /// within this duration.
    /// Default: 60 seconds
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub client_action_timeout_s: u64,

    /// Periodically count the actions in each stage and report them as the
    /// `execution.active.count` metric.
    ///
    /// Only has an effect when scheduler state lives in a store, which today
    /// means Redis; the in-memory scheduler always maintains this count because
    /// it already holds the state in process. The count is a query against the
    /// scheduler store every 15 seconds from every scheduler replica, so it
    /// adds load to the same backend that serves action scheduling. Leave it
    /// off unless you want the metric.
    ///
    /// Every replica reports the same store-wide totals, so aggregate the
    /// series across replicas with `max`, not `sum`.
    /// Default: false
    #[serde(default)]
    pub enable_active_action_count_metric: bool,

    /// Remove workers from pool once the worker has not responded in this
    /// amount of time in seconds.
    /// Default: 5 seconds
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub worker_timeout_s: u64,

    /// Maximum time (seconds) an action can stay in Executing state without
    /// any worker update before being timed out and re-queued.
    /// This applies regardless of worker keepalive status, catching cases
    /// where a worker is alive (sending keepalives) but stuck on a specific
    /// action. Set to 0 to disable (relies only on `worker_timeout_s`).
    ///
    /// Default: 0 (disabled)
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub max_action_executing_timeout_s: u64,

    /// Evict a worker that has not reported back on an operation it was
    /// told to kill within this many seconds. A healthy worker
    /// acknowledges a kill in moments; one that cannot is wedged, and its
    /// keepalives would otherwise keep `worker_timeout_s` from ever firing
    /// while the dead operation holds its slot. Eviction requeues the
    /// worker's other operations.
    /// Default: 60 seconds
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub unacknowledged_kill_timeout_s: u64,

    /// Fail a queued action once no connected worker has been able to run
    /// it for this many seconds, instead of leaving it queued forever. An
    /// action counts as impossible to run when no connected worker could
    /// take it even when idle: an `exact` value no worker has, a required
    /// key no worker declares, or a `minimum` larger than any worker's
    /// total. An action that is only waiting for a busy worker is never
    /// failed by this setting, and neither is any action while no workers
    /// are connected at all.
    ///
    /// The action fails with `FAILED_PRECONDITION` and a message naming the
    /// properties that could not be satisfied. Bazel does not retry this,
    /// and runs the action locally if `--remote_local_fallback` is set.
    ///
    /// The time is measured per distinct set of platform properties, and
    /// each action must also have been queued for this long itself before
    /// it is failed. Actions that queue together therefore fail within
    /// about one timeout of each other, while during a pool outage later
    /// actions fail as they age rather than in an immediate burst.
    ///
    /// Roll this out with the timeout at 0 on every scheduler first and
    /// watch the `scheduler.unsatisfiable.queued` metric for a while:
    /// anything that appears there during normal operation is an action
    /// this setting would have failed. Then set it above the longest time
    /// a worker pool needs to provision or restart, such as scaling up from
    /// zero, a rolling restart of the whole pool, or spot preemption, not
    /// above the typical queue wait. Only properties listed in
    /// `supported_platform_properties` are enforced strictly; a property
    /// that is not listed does not restrict workers that do not declare it.
    ///
    /// When several schedulers share one Redis backend, each publishes what
    /// its workers can run whenever a worker joins or leaves and at least
    /// every 5 seconds, and reads what the others publish every 5 seconds.
    /// An action is only failed when no scheduler has a worker that could
    /// run it, and never while the other schedulers cannot be read. A
    /// worker that joins or leaves a peer is seen here within about 5
    /// seconds, and the workers of a peer that stops altogether stop
    /// counting within about 20, so keep this well above that. Schedulers
    /// publish whatever this is set to, so roll out a version that has this
    /// setting to every scheduler before setting it on any of them.
    ///
    /// Such actions are logged and counted in the
    /// `scheduler.unsatisfiable.queued` metric whatever this is set to.
    ///
    /// Default: 0 (never fail)
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub unsatisfiable_action_timeout_s: u64,

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

    /// Every N seconds, run a worker matching pass even if no task or worker
    /// change notification arrived. This is a safety net for missed
    /// notifications and for scheduler backends with eventually consistent
    /// searches (for example Redis), where an operation that was re-queued
    /// may not be visible to the search triggered by its own notification.
    /// Without this, such an operation can stay queued until an unrelated
    /// event triggers another matching pass.
    /// Defaults to 5s. Zero or any negative value disables it.
    #[serde(
        default = "default_fallback_match_interval_s",
        deserialize_with = "convert_duration_with_shellexpand_and_negative"
    )]
    pub fallback_match_interval_s: i64,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub enum ExperimentalSimpleSchedulerBackend {
    /// Use an in-memory store for the scheduler.
    Memory,
    /// Use a redis store for the scheduler.
    Redis(ExperimentalRedisSchedulerBackend),
}

#[derive(Deserialize, Serialize, Debug, Default)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct ExperimentalRedisSchedulerBackend {
    /// A reference to the redis store to use for the scheduler.
    /// Note: This MUST resolve to a `RedisSpec`.
    pub redis_store: StoreRefName,
}

/// A scheduler that forwards requests to an upstream scheduler. This
/// is useful to use when doing some kind of local action cache or CAS away from
/// the main cluster of workers. In general, it's more efficient to point the
/// build at the main scheduler directly though.
#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct GrpcSpec {
    /// The upstream scheduler to forward requests to.
    pub endpoint: GrpcEndpoint,

    /// Retry configuration to use when a network request fails.
    #[serde(default)]
    pub retry: Retry,

    /// Limit the number of simultaneous upstream requests to this many. A
    /// value of zero is treated as unlimited. If the limit is reached the
    /// request is queued.
    /// Default: unlimited
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_concurrent_requests: usize,

    /// The number of connections to make to each specified endpoint to balance
    /// the load over multiple TCP connections.
    /// Default: 1.
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub connections_per_endpoint: usize,
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
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
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct PlatformPropertyAddition {
    /// The name of the property to add.
    pub name: String,
    /// The value to assign to the property.
    pub value: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct PlatformPropertyReplacement {
    /// The name of the property to replace.
    pub name: String,
    /// The value to match against, if unset then any instance matches.
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
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
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
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct PropertyModifierSpec {
    /// A list of modifications to perform to incoming actions for the nested
    /// scheduler. These are performed in order and blindly, so removing a
    /// property that doesn't exist is fine and overwriting an existing property
    /// is also fine. If adding properties that do not exist in the nested
    /// scheduler is not supported and will likely cause unexpected behaviour.
    pub modifications: Vec<PropertyModification>,

    /// The nested scheduler to use after modifying the properties.
    pub scheduler: Box<SchedulerSpec>,
}

const fn default_historical_resource_refresh_interval_s() -> u64 {
    30
}

fn default_historical_resource_cpu_property_name() -> String {
    "cpu_count".to_string()
}

fn default_historical_resource_memory_property_name() -> String {
    "memory_kb".to_string()
}

#[derive(Deserialize, Serialize, Debug)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct HistoricalResourceSpec {
    /// JSON file containing historical resource hints keyed by Bazel
    /// `RequestMetadata` `target_id` and/or `action_mnemonic`.
    ///
    /// Supported file shapes:
    /// ```json
    /// [
    ///   { "target_id": "//pkg:test", "action_mnemonic": "TestRunner", "cpu_count": 2, "memory_kb": 12582912 }
    /// ]
    /// ```
    /// or:
    /// ```json
    /// { "hints": [ ... ] }
    /// ```
    #[serde(deserialize_with = "convert_string_with_shellexpand")]
    pub hints_file: String,

    /// Reload interval for `hints_file`. Set to 0 to load once.
    /// Default: 30 seconds
    #[serde(
        default = "default_historical_resource_refresh_interval_s",
        deserialize_with = "convert_duration_with_shellexpand"
    )]
    pub refresh_interval_s: u64,

    /// Platform property name used for CPU minimums.
    /// Default: `cpu_count`
    #[serde(
        default = "default_historical_resource_cpu_property_name",
        deserialize_with = "convert_string_with_shellexpand"
    )]
    pub cpu_property_name: String,

    /// Platform property name used for memory minimums, expressed in KiB.
    /// Default: `memory_kb`
    #[serde(
        default = "default_historical_resource_memory_property_name",
        deserialize_with = "convert_string_with_shellexpand"
    )]
    pub memory_property_name: String,

    /// The nested scheduler to use after applying resource hints.
    pub scheduler: Box<SchedulerSpec>,
}
