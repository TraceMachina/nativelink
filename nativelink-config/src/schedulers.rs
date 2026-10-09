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
    /// Prefer the worker running the fewest actions, least recently used
    /// among equals. A worker that just joined has nothing running, so it
    /// takes the next action instead of waiting for every older worker to
    /// be used once more, and a burst spreads across the fleet instead of
    /// filling one worker to its concurrency cap.
    LeastLoaded,
    /// Prefer the worker the action fits most tightly: among the workers
    /// that can take it, the one whose remaining `minimum` properties
    /// (memory, CPU) would be smallest afterwards, as a share of what it
    /// advertises. Big actions are kept for the workers with big room and
    /// small ones fill the gaps, so a fleet of mixed sizes wastes the
    /// least. A `priority` property on the action that a worker carries
    /// with the same value prefers that worker before the fit is judged,
    /// which is what such a property is for (a zone, a cache locality).
    /// Ties go to the fewest running actions, then least recently used.
    BestFit,
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
    /// amount of time in seconds. Any message from the worker counts, not
    /// only keepalives. Eviction requeues everything the worker held, so
    /// keep this well above the longest pause a loaded worker can see.
    /// Default: 10 seconds (four default keepalive intervals)
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

    /// Requeue a dispatched action the worker has not acknowledged within
    /// this many seconds, untried and without counting an attempt, and
    /// pause the worker. The clock is the worker's own messages: a dispatch
    /// is dated by the worker's last message before it, and the check runs
    /// on each later message, so keep this above a few keepalive intervals.
    /// Workers from v1.7.3 acknowledge every dispatch;
    /// older workers never do, so leave this unset while any are
    /// connected or their every action would be requeued.
    /// Default: unset (off)
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub dispatch_ack_timeout_s: u64,

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
    /// actions fail as they age rather than in an immediate burst. An
    /// action's own wait is measured from its original submission, which
    /// is not reset when a second client attaches to the same action or
    /// when it is queued again after its worker is lost; in those cases
    /// the per-shape clock still guarantees no capable worker was seen for
    /// the full timeout before it is failed.
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
    /// (The 5 and 20 seconds follow from `FLEET_EXCHANGE_INTERVAL` and
    /// `FLEET_RECORD_TTL_INTERVALS` in the scheduler; if those change,
    /// remember to change this documentation.)
    ///
    /// Such actions are logged and counted in the
    /// `scheduler.unsatisfiable.queued` metric whatever this is set to.
    ///
    /// Default: 0 (never fail)
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub unsatisfiable_action_timeout_s: u64,

    /// Seconds a queued action waits while no worker at all is connected to
    /// this scheduler, nor, on a shared backend, to any peer, before it
    /// fails with `FailedPrecondition`. `unsatisfiable_action_timeout_s`
    /// only runs against workers that are there, so a pool that scales from
    /// zero is never failed on sight; but a provisioner that will not create
    /// a pod for a shape it cannot serve leaves such an action waiting for
    /// a fleet that never comes, and this is the bound on that. Keep it
    /// above the longest time a pool needs to bring up its first worker.
    /// Default: 0 (never fail)
    #[serde(default, deserialize_with = "convert_duration_with_shellexpand")]
    pub no_worker_action_timeout_s: u64,

    /// If a job returns an internal error or times out this many times the
    /// scheduler completes it with `FailedPrecondition` carrying the last
    /// error, a code clients do not retry. This is to help prevent one rogue
    /// job from infinitely retrying and taking up a lot of resources when the
    /// task itself is the one causing the server to go into a bad state.
    /// A lost worker and a memory escalation are not the action's failures
    /// and have their own budgets: `max_worker_loss_retries` and
    /// `memory_escalation.max_steps`.
    /// Default: 3
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_job_retries: usize,

    /// How many times an action whose worker was lost (disconnected, timed
    /// out, evicted, or OOM-killed as a whole) is queued again before the
    /// scheduler completes it with `FailedPrecondition`. A lost worker is
    /// usually not the action's fault, so these do not spend
    /// `max_job_retries`; the cap only stops an action that takes a worker
    /// down every time it runs.
    /// Default: 10
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub max_worker_loss_retries: usize,

    /// The strategy used to assign workers jobs.
    #[serde(default)]
    pub allocation_strategy: WorkerAllocationStrategy,

    /// Keep a large action from waiting forever behind a stream of small
    /// ones. The matcher places whatever fits, so on a busy fleet a worker
    /// never shows eight free cores at once when one-core actions take each
    /// core the moment it frees, and an eight-core action at the head of
    /// the queue sits there indefinitely while everything behind it runs
    /// (`allocation_strategy: best_fit` only slows this down). With this
    /// set, once the action at the head of the queue has waited `after_s`
    /// since it was last queued and a matching pass places something
    /// queued after it, the scheduler holds for it the busy worker closest
    /// to fitting it (the fewest cores and KiB short, by share of what the
    /// worker advertises, among those that could take it once drained) and
    /// sends that worker nothing queued after the action until the action
    /// is placed, so the worker drains to it; every other worker keeps
    /// taking the small actions. This is the reservation of Slurm's
    /// backfill scheduler, without the time estimate: nothing here knows
    /// how long an action runs. A full fleet that places nothing holds
    /// nothing, and neither does an action that is only next in line.
    ///
    /// One hold at a time, and it follows the queue: an action that sorts
    /// before the held one (a higher priority, or an older action queued
    /// again after its worker was lost) takes the held worker's room first,
    /// in queue order, and takes the hold itself only if the held worker
    /// could never run it. The hold is released when the action is placed,
    /// on the held worker or on any other that has room first, and when
    /// the action leaves the queue (cancelled, its client gone, finished
    /// elsewhere). When the held worker can no longer be waited on (it
    /// disconnects, drains, pauses, declines the action, reports too little
    /// free memory even once drained, or a requeue raised what the action
    /// asks past what it registered) the hold moves to another worker that
    /// qualifies, or is dropped until the action is overtaken again; what a
    /// worker reports free while idle caps what draining it is expected to
    /// free, so one that refused the action drained is not held for it
    /// again until an idle report shows more. Holds
    /// are logged at `info` and counted by
    /// `scheduler.head_of_line.reservations`.
    ///
    /// The cost is idle capacity, by design: every core the held worker
    /// frees stays idle until the action lands, so one hold wastes up to
    /// the action's size for as long as the longest action running on that
    /// worker when it was held. On a two-worker fleet that is half the
    /// fleet's backfilling for the length of that action, an hour for an
    /// hour-long one. With a small `after_s` on a fleet where large
    /// actions are common, some worker is held almost all the time. Watch
    /// the `reserved` rate of the metric; a steady one means the fleet is
    /// sized for the small actions and not for the large ones.
    /// Default: unset (no reservation; a large action waits for room)
    #[serde(default)]
    pub head_of_line_reservation: Option<HeadOfLineReservationSpec>,

    /// The name of a `minimum` platform property, usually `memory_kb`, that
    /// the scheduler compares against the free memory each worker reports
    /// with its keepalive: a worker reporting less than the action asks for
    /// is skipped for that action, whatever the admission ledger says. The
    /// ledger only subtracts what actions declare; this catches the ones
    /// that declared too little. Workers older than v1.7.3 report nothing
    /// and are never vetoed.
    /// Default: unset (off)
    #[serde(default)]
    pub live_memory_veto: Option<String>,

    /// When a worker reports an action killed for memory (`KILLED_MEMORY`,
    /// by its own reservation enforcement or by the kernel), requeue the
    /// action with a larger reservation instead of failing it. Escalations
    /// have their own budget (`max_steps`) and do not spend
    /// `max_job_retries`. The reservation grows to the largest memory any
    /// connected worker advertises (or `max_kb`); the last step reserves
    /// that worker whole, memory and CPU, so the action runs alone, and only
    /// a kill there fails the action: nothing in the fleet could run it.
    /// Default: unset (a memory kill fails the action)
    #[serde(default)]
    pub memory_escalation: Option<MemoryEscalationSpec>,

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

#[derive(Deserialize, Serialize, Debug, Clone, Copy)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct HeadOfLineReservationSpec {
    /// How long the action at the head of the queue has waited, in
    /// seconds, before a worker is held for it. Measured from the last
    /// time the action was queued: its submission, or its requeue after a
    /// worker was lost or declined it. Set it above the typical queue
    /// wait, so a worker is only taken out of backfilling for an action
    /// that is actually stuck: a few minutes on a fleet of long actions,
    /// less on one of short ones. 0 holds on the first pass in which the
    /// action is overtaken.
    #[serde(deserialize_with = "convert_duration_with_shellexpand")]
    pub after_s: u64,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct MemoryEscalationSpec {
    /// The `minimum` platform property carrying the memory reservation.
    /// Default: `memory_kb`
    #[serde(default = "default_historical_resource_memory_property_name")]
    pub property: String,

    /// Each kill scales the reservation by this factor, in hundredths:
    /// 200 doubles it, 150 adds half.
    /// Default: 200
    #[serde(default = "default_memory_escalation_percent")]
    pub percent: u64,

    /// Never reserve more than this many KiB; 0 takes the largest memory
    /// any connected worker advertises.
    /// Default: 0
    #[serde(default)]
    pub max_kb: u64,

    /// The memory values of the fleet's size classes, ascending, in KiB.
    /// When set, a kill steps the reservation to the next class above it
    /// instead of scaling by `percent`; past the top class the last step
    /// reserves the largest worker whole. The chart fills this from the
    /// same classes the `historical_resource` scheduler uses.
    /// Default: empty (scale by `percent`)
    #[serde(default)]
    pub ladder_kb: Vec<u64>,

    /// The `minimum` platform property carrying the CPU reservation. The
    /// last escalation reserves the largest worker's whole memory and,
    /// through this property, its whole CPU, so nothing shares the worker
    /// with the action. Empty leaves CPU alone.
    /// Default: `cpu_count`
    #[serde(default = "default_memory_escalation_cpu_property")]
    pub cpu_property: String,

    /// The most escalations one action gets before the scheduler completes
    /// it with `FailedPrecondition`; 0 lets the ceiling alone bound them.
    /// Default: 8
    #[serde(default = "default_memory_escalation_max_steps")]
    pub max_steps: u64,
}

fn default_memory_escalation_cpu_property() -> String {
    "cpu_count".to_string()
}

const fn default_memory_escalation_max_steps() -> u64 {
    8
}

const fn default_memory_escalation_percent() -> u64 {
    200
}

#[derive(Deserialize, Serialize, Debug, Default, Clone, Copy)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct ColdStartSpec {
    /// Value for `cpu_property_name`, on the scale the workers advertise.
    /// 0 leaves the property alone.
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub cpu_count: u64,

    /// Value for `memory_property_name`, in KiB. 0 leaves the property alone.
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub memory_kb: u64,

    /// Value for `disk_property_name`, in KiB. 0 leaves the property alone.
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub disk_kb: u64,
}

fn default_disk_property_name_for_hints() -> String {
    "disk_kb".to_string()
}

/// A named point on the fleet's size ladder. List them ascending; each
/// should dominate the one before on every dimension it names. A zero
/// leaves that dimension alone.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct SizeClassSpec {
    /// Free text, referenced by hints and by the cold-start policy.
    pub name: String,

    /// Value for `cpu_property_name`, on the scale the workers advertise.
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub cpu_count: u64,

    /// Value for `memory_property_name`, in KiB.
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub memory_kb: u64,

    /// Value for `disk_property_name`, in KiB.
    #[serde(default, deserialize_with = "convert_numeric_with_shellexpand")]
    pub disk_kb: u64,
}

/// A class for actions whose timeout is at least `min_timeout_s`. Bazel's
/// test sizes arrive as timeouts (short 60 s, moderate 300 s, long 900 s,
/// eternal 3600 s), so this maps them to classes without a CAS fetch.
#[derive(Deserialize, Serialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]
pub struct TimeoutClassSpec {
    #[serde(deserialize_with = "convert_numeric_with_shellexpand")]
    pub min_timeout_s: u64,
    pub class: String,
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

    /// Platform property name used for CPU minimums. The nested scheduler
    /// must declare it as a `minimum` property (as it must the memory and
    /// disk names): an undeclared property is matched as an exact string,
    /// and an action carrying a number would then match no worker.
    /// Default: `cpu_count`
    #[serde(
        default = "default_historical_resource_cpu_property_name",
        deserialize_with = "convert_string_with_shellexpand"
    )]
    pub cpu_property_name: String,

    /// Platform property name used for memory minimums, expressed in KiB.
    /// Declared as `minimum` on the nested scheduler, like the CPU name.
    /// Default: `memory_kb`
    #[serde(
        default = "default_historical_resource_memory_property_name",
        deserialize_with = "convert_string_with_shellexpand"
    )]
    pub memory_property_name: String,

    /// Platform property name used for disk minimums, expressed in KiB.
    /// Declared as `minimum` on the nested scheduler, like the CPU name.
    /// Default: `disk_kb`
    #[serde(
        default = "default_disk_property_name_for_hints",
        deserialize_with = "convert_string_with_shellexpand"
    )]
    pub disk_property_name: String,

    /// The fleet's size ladder. A hint may name a `class` instead of
    /// numbers, and the cold-start policy below picks one for actions no
    /// hint matches. Empty means raw numbers only.
    #[serde(default)]
    pub classes: Vec<SizeClassSpec>,

    /// The class an untagged action gets when no hint and no rule below
    /// matches. Takes precedence over `cold_start` when both are set.
    #[serde(default)]
    pub default_class: Option<String>,

    /// Class by Bazel `action_mnemonic` for untagged actions; a known heavy
    /// mnemonic (`TestRunner`, `Link`) starts in the right class.
    #[serde(default)]
    pub class_by_mnemonic: HashMap<String, String>,

    /// Class by the action's timeout, the largest `min_timeout_s` at or
    /// below the timeout wins. Checked before `default_class`, after the
    /// mnemonic rule.
    #[serde(default)]
    pub class_by_timeout: Vec<TimeoutClassSpec>,

    /// Reservation given to an action that no hint matches and the client
    /// left untagged. Only a property that is absent is filled; a value the
    /// client sent is kept. Without this an untagged action costs the
    /// scheduler's ledger nothing, so any number of them can land on one
    /// worker.
    #[serde(default)]
    pub cold_start: Option<ColdStartSpec>,

    /// The nested scheduler to use after applying resource hints.
    pub scheduler: Box<SchedulerSpec>,
}
