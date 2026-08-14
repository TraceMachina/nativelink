// Copyright 2025 The NativeLink Authors. All rights reserved.
//
// Licensed under the Business Source License, Version 1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may requested a copy of the License by emailing contact@nativelink.com.
//
// Use of this module requires an enterprise license agreement, which can be
// attained by emailing contact@nativelink.com or signing up for Nativelink
// Cloud at app.nativelink.com.
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::LazyLock;
use std::time::SystemTime;

use opentelemetry::{InstrumentationScope, KeyValue, Value, global, metrics};

use crate::action_messages::{ActionResult, ActionStage};

// Metric attribute keys for cache operations.
pub const CACHE_TYPE: &str = "cache.type";
pub const CACHE_OPERATION: &str = "cache.operation.name";
pub const CACHE_RESULT: &str = "cache.operation.result";

// Metric attribute keys for remote execution operations.
pub const EXECUTION_STAGE: &str = "execution.stage";
pub const EXECUTION_RESULT: &str = "execution.result";
pub const EXECUTION_INSTANCE: &str = "execution.instance";
pub const EXECUTION_PRIORITY: &str = "execution.priority";
pub const EXECUTION_WORKER_ID: &str = "execution.worker_id";
pub const EXECUTION_EXIT_CODE: &str = "execution.exit_code";
pub const EXECUTION_ACTION_DIGEST: &str = "execution.action_digest";

// Metric attribute keys for gRPC serving, following OTel rpc semconv.
pub const RPC_SERVICE: &str = "rpc.service";
pub const RPC_METHOD: &str = "rpc.method";
pub const RPC_STATUS_CODE: &str = "rpc.grpc.status_code";

// Metric attribute keys for the scheduler.
pub const SCHEDULER_MATCH_RESULT: &str = "scheduler.match.result";

// Metric attribute keys for tiered stores.
pub const STORE_TIER: &str = "store.tier";
pub const STORE_RESULT: &str = "store.result";
pub const STORE_DIRECTION: &str = "store.direction";

// Metric attribute keys for connection pools.
pub const CONNECTION_POOL: &str = "connection.pool";
pub const CONNECTION_RESULT: &str = "connection.result";

// Metric attribute keys for health checks.
pub const HEALTH_NAMESPACE: &str = "health.namespace";
pub const HEALTH_STATUS: &str = "health.status";

// Metric attribute keys for the worker fleet.
pub const WORKER_STATE: &str = "worker.state";
pub const WORKER_DISCONNECT_REASON: &str = "worker.disconnect.reason";

/// Why a worker left the pool.
#[derive(Debug, Clone, Copy)]
pub enum WorkerDisconnectReason {
    /// The worker's connection ended.
    Disconnected,
    /// The scheduler evicted it, usually after a timeout or an error.
    Evicted,
}

impl WorkerDisconnectReason {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disconnected => "disconnected",
            Self::Evicted => "evicted",
        }
    }
}

/// Cache operation types for metrics classification.
#[derive(Debug, Clone, Copy)]
pub enum CacheOperationName {
    /// Data retrieval operations (get, peek, contains, etc.)
    Read,
    /// Data storage operations (insert, update, replace, etc.)
    Write,
    /// Explicit data removal operations
    Delete,
    /// Automatic cache maintenance (evictions, TTL cleanup, etc.)
    Evict,
}

impl From<CacheOperationName> for Value {
    fn from(op: CacheOperationName) -> Self {
        match op {
            CacheOperationName::Read => Self::from("read"),
            CacheOperationName::Write => Self::from("write"),
            CacheOperationName::Delete => Self::from("delete"),
            CacheOperationName::Evict => Self::from("evict"),
        }
    }
}

/// Results of cache operations.
///
/// Result semantics vary by operation type:
/// - Read: Hit/Miss/Expired indicate data availability
/// - Write/Delete/Evict: Success/Error indicate completion status
#[derive(Debug, Clone, Copy)]
pub enum CacheOperationResult {
    /// Data found and valid (Read operations)
    Hit,
    /// Data not found (Read operations)
    Miss,
    /// Data found but invalid/expired (Read operations)
    Expired,
    /// Operation completed successfully (Write/Delete/Evict operations)
    Success,
    /// Operation failed (any operation type)
    Error,
}

impl From<CacheOperationResult> for Value {
    fn from(result: CacheOperationResult) -> Self {
        match result {
            CacheOperationResult::Hit => Self::from("hit"),
            CacheOperationResult::Miss => Self::from("miss"),
            CacheOperationResult::Expired => Self::from("expired"),
            CacheOperationResult::Success => Self::from("success"),
            CacheOperationResult::Error => Self::from("error"),
        }
    }
}

/// Remote execution stages for metrics classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionStage {
    /// Unknown stage
    Unknown,
    /// Checking cache for existing results
    CacheCheck,
    /// Action is queued waiting for execution
    Queued,
    /// Action is being executed by a worker
    Executing,
    /// Action execution completed
    Completed,
}

impl From<ExecutionStage> for Value {
    fn from(stage: ExecutionStage) -> Self {
        match stage {
            ExecutionStage::Unknown => Self::from("unknown"),
            ExecutionStage::CacheCheck => Self::from("cache_check"),
            ExecutionStage::Queued => Self::from("queued"),
            ExecutionStage::Executing => Self::from("executing"),
            ExecutionStage::Completed => Self::from("completed"),
        }
    }
}

impl From<ActionStage> for ExecutionStage {
    fn from(stage: ActionStage) -> Self {
        match stage {
            ActionStage::Unknown => Self::Unknown,
            ActionStage::CacheCheck => Self::CacheCheck,
            ActionStage::Queued => Self::Queued,
            ActionStage::Executing => Self::Executing,
            ActionStage::Completed(_) | ActionStage::CompletedFromCache(_) => Self::Completed,
        }
    }
}

impl From<&ActionStage> for ExecutionStage {
    fn from(stage: &ActionStage) -> Self {
        match stage {
            ActionStage::Unknown => Self::Unknown,
            ActionStage::CacheCheck => Self::CacheCheck,
            ActionStage::Queued => Self::Queued,
            ActionStage::Executing => Self::Executing,
            ActionStage::Completed(_) | ActionStage::CompletedFromCache(_) => Self::Completed,
        }
    }
}

/// Results of remote execution operations.
#[derive(Debug, Clone, Copy)]
pub enum ExecutionResult {
    /// Execution completed successfully
    Success,
    /// Execution failed
    Failure,
    /// Execution was cancelled
    Cancelled,
    /// Execution timed out
    Timeout,
    /// Result was found in cache
    CacheHit,
}

impl From<ExecutionResult> for Value {
    fn from(result: ExecutionResult) -> Self {
        match result {
            ExecutionResult::Success => Self::from("success"),
            ExecutionResult::Failure => Self::from("failure"),
            ExecutionResult::Cancelled => Self::from("cancelled"),
            ExecutionResult::Timeout => Self::from("timeout"),
            ExecutionResult::CacheHit => Self::from("cache_hit"),
        }
    }
}

/// Pre-allocated attribute combinations for efficient cache metrics collection.
///
/// Avoids runtime allocation by pre-computing common attribute combinations
/// for cache operations and results.
#[derive(Debug)]
pub struct CacheMetricAttrs {
    // Read operation attributes
    read_hit: Vec<KeyValue>,
    read_miss: Vec<KeyValue>,
    read_expired: Vec<KeyValue>,
    read_error: Vec<KeyValue>,

    // Write operation attributes
    write_success: Vec<KeyValue>,
    write_error: Vec<KeyValue>,

    // Delete operation attributes
    delete_success: Vec<KeyValue>,
    delete_miss: Vec<KeyValue>,
    delete_error: Vec<KeyValue>,

    // Evict operation attributes
    evict_success: Vec<KeyValue>,
    evict_expired: Vec<KeyValue>,
}

impl CacheMetricAttrs {
    /// Creates a new set of pre-computed attributes.
    ///
    /// The `base_attrs` are included in all attribute combinations (e.g., cache
    /// type, instance ID).
    #[must_use]
    pub fn new(base_attrs: &[KeyValue]) -> Self {
        let make_attrs = |op: CacheOperationName, result: CacheOperationResult| {
            let mut attrs = base_attrs.to_vec();
            attrs.push(KeyValue::new(CACHE_OPERATION, op));
            attrs.push(KeyValue::new(CACHE_RESULT, result));
            attrs
        };

        Self {
            read_hit: make_attrs(CacheOperationName::Read, CacheOperationResult::Hit),
            read_miss: make_attrs(CacheOperationName::Read, CacheOperationResult::Miss),
            read_expired: make_attrs(CacheOperationName::Read, CacheOperationResult::Expired),
            read_error: make_attrs(CacheOperationName::Read, CacheOperationResult::Error),

            write_success: make_attrs(CacheOperationName::Write, CacheOperationResult::Success),
            write_error: make_attrs(CacheOperationName::Write, CacheOperationResult::Error),

            delete_success: make_attrs(CacheOperationName::Delete, CacheOperationResult::Success),
            delete_miss: make_attrs(CacheOperationName::Delete, CacheOperationResult::Miss),
            delete_error: make_attrs(CacheOperationName::Delete, CacheOperationResult::Error),

            evict_success: make_attrs(CacheOperationName::Evict, CacheOperationResult::Success),
            evict_expired: make_attrs(CacheOperationName::Evict, CacheOperationResult::Expired),
        }
    }

    // Attribute accessors
    #[must_use]
    pub fn read_hit(&self) -> &[KeyValue] {
        &self.read_hit
    }
    #[must_use]
    pub fn read_miss(&self) -> &[KeyValue] {
        &self.read_miss
    }
    #[must_use]
    pub fn read_expired(&self) -> &[KeyValue] {
        &self.read_expired
    }
    #[must_use]
    pub fn read_error(&self) -> &[KeyValue] {
        &self.read_error
    }
    #[must_use]
    pub fn write_success(&self) -> &[KeyValue] {
        &self.write_success
    }
    #[must_use]
    pub fn write_error(&self) -> &[KeyValue] {
        &self.write_error
    }
    #[must_use]
    pub fn delete_success(&self) -> &[KeyValue] {
        &self.delete_success
    }
    #[must_use]
    pub fn delete_miss(&self) -> &[KeyValue] {
        &self.delete_miss
    }
    #[must_use]
    pub fn delete_error(&self) -> &[KeyValue] {
        &self.delete_error
    }
    #[must_use]
    pub fn evict_success(&self) -> &[KeyValue] {
        &self.evict_success
    }
    #[must_use]
    pub fn evict_expired(&self) -> &[KeyValue] {
        &self.evict_expired
    }
}

/// Pre-allocated attribute combinations for efficient remote execution metrics collection.
#[derive(Debug)]
pub struct ExecutionMetricAttrs {
    // Stage transition attributes
    unknown: Vec<KeyValue>,
    cache_check: Vec<KeyValue>,
    queued: Vec<KeyValue>,
    executing: Vec<KeyValue>,
    completed_success: Vec<KeyValue>,
    completed_failure: Vec<KeyValue>,
    completed_cancelled: Vec<KeyValue>,
    completed_timeout: Vec<KeyValue>,
    completed_cache_hit: Vec<KeyValue>,
}

impl ExecutionMetricAttrs {
    /// Creates a new set of pre-computed attributes.
    ///
    /// The `base_attrs` are included in all attribute combinations (e.g., instance
    /// name, worker ID).
    #[must_use]
    pub fn new(base_attrs: &[KeyValue]) -> Self {
        let make_attrs = |stage: ExecutionStage, result: Option<ExecutionResult>| {
            let mut attrs = base_attrs.to_vec();
            attrs.push(KeyValue::new(EXECUTION_STAGE, stage));
            if let Some(result) = result {
                attrs.push(KeyValue::new(EXECUTION_RESULT, result));
            }
            attrs
        };

        Self {
            unknown: make_attrs(ExecutionStage::Unknown, None),
            cache_check: make_attrs(ExecutionStage::CacheCheck, None),
            queued: make_attrs(ExecutionStage::Queued, None),
            executing: make_attrs(ExecutionStage::Executing, None),
            completed_success: make_attrs(
                ExecutionStage::Completed,
                Some(ExecutionResult::Success),
            ),
            completed_failure: make_attrs(
                ExecutionStage::Completed,
                Some(ExecutionResult::Failure),
            ),
            completed_cancelled: make_attrs(
                ExecutionStage::Completed,
                Some(ExecutionResult::Cancelled),
            ),
            completed_timeout: make_attrs(
                ExecutionStage::Completed,
                Some(ExecutionResult::Timeout),
            ),
            completed_cache_hit: make_attrs(
                ExecutionStage::Completed,
                Some(ExecutionResult::CacheHit),
            ),
        }
    }

    // Attribute accessors
    #[must_use]
    pub fn unknown(&self) -> &[KeyValue] {
        &self.unknown
    }
    #[must_use]
    pub fn cache_check(&self) -> &[KeyValue] {
        &self.cache_check
    }
    #[must_use]
    pub fn queued(&self) -> &[KeyValue] {
        &self.queued
    }
    #[must_use]
    pub fn executing(&self) -> &[KeyValue] {
        &self.executing
    }
    #[must_use]
    pub fn completed_success(&self) -> &[KeyValue] {
        &self.completed_success
    }
    #[must_use]
    pub fn completed_failure(&self) -> &[KeyValue] {
        &self.completed_failure
    }
    #[must_use]
    pub fn completed_cancelled(&self) -> &[KeyValue] {
        &self.completed_cancelled
    }
    #[must_use]
    pub fn completed_timeout(&self) -> &[KeyValue] {
        &self.completed_timeout
    }
    #[must_use]
    pub fn completed_cache_hit(&self) -> &[KeyValue] {
        &self.completed_cache_hit
    }
}

/// Global cache metrics instruments.
pub static CACHE_METRICS: LazyLock<CacheMetrics> = LazyLock::new(|| {
    let meter = global::meter_with_scope(InstrumentationScope::builder("nativelink").build());

    CacheMetrics {
        cache_operation_duration: meter
            .f64_histogram("cache.operation.duration")
            .with_description("Duration of cache operations in milliseconds")
            .with_unit("ms")
            // The range of these is quite large as a cache might be backed by
            // memory, a filesystem, or network storage. The current values were
            // determined empirically and might need adjustment.
            .with_boundaries(vec![
                // Microsecond range
                0.001, // 1μs
                0.005, // 5μs
                0.01,  // 10μs
                0.05,  // 50μs
                0.1,   // 100μs
                // Sub-millisecond range
                0.2, // 200μs
                0.5, // 500μs
                1.0, // 1ms
                // Low millisecond range
                2.0,   // 2ms
                5.0,   // 5ms
                10.0,  // 10ms
                20.0,  // 20ms
                50.0,  // 50ms
                100.0, // 100ms
                // Higher latency range
                200.0,  // 200ms
                500.0,  // 500ms
                1000.0, // 1 second
                2000.0, // 2 seconds
                5000.0, // 5 seconds
            ])
            .build(),

        cache_operations: meter
            .u64_counter("cache.operations")
            .with_description("Total cache operations by type and result")
            .build(),

        cache_io: meter
            .u64_counter("cache.io")
            .with_description("Total bytes processed by cache operations")
            .with_unit("By")
            .build(),

        cache_size: meter
            .i64_up_down_counter("cache.size")
            .with_description("Current total size of cached data")
            .with_unit("By")
            .build(),

        cache_entries: meter
            .i64_up_down_counter("cache.entries")
            .with_description("Current number of cached entries")
            .with_unit("{entry}")
            .build(),

        cache_entry_size: meter
            .u64_histogram("cache.item.size")
            .with_description("Size distribution of cached entries")
            .with_unit("By")
            .build(),
    }
});

/// OpenTelemetry metrics instruments for cache monitoring.
#[derive(Debug)]
pub struct CacheMetrics {
    /// Histogram of cache operation durations in milliseconds
    pub cache_operation_duration: metrics::Histogram<f64>,
    /// Counter of cache operations by type and result
    pub cache_operations: metrics::Counter<u64>,
    /// Counter of bytes read/written during cache operations
    pub cache_io: metrics::Counter<u64>,
    /// Current total size of all cached data in bytes
    pub cache_size: metrics::UpDownCounter<i64>,
    /// Current number of entries in cache
    pub cache_entries: metrics::UpDownCounter<i64>,
    /// Histogram of individual cache entry sizes in bytes
    pub cache_entry_size: metrics::Histogram<u64>,
}

/// Global remote execution metrics instruments.
pub static EXECUTION_METRICS: LazyLock<ExecutionMetrics> = LazyLock::new(|| {
    let meter = global::meter_with_scope(InstrumentationScope::builder("nativelink").build());

    ExecutionMetrics {
        execution_stage_duration: meter
            .f64_histogram("execution.stage.duration")
            .with_description("Duration of each execution stage in seconds")
            .with_unit("s")
            .with_boundaries(vec![
                // Sub-second range
                0.001, // 1ms
                0.01,  // 10ms
                0.1,   // 100ms
                0.5,   // 500ms
                1.0,   // 1s
                // Multi-second range
                2.0,    // 2s
                5.0,    // 5s
                10.0,   // 10s
                30.0,   // 30s
                60.0,   // 1 minute
                120.0,  // 2 minutes
                300.0,  // 5 minutes
                600.0,  // 10 minutes
                1800.0, // 30 minutes
                3600.0, // 1 hour
            ])
            .build(),

        execution_total_duration: meter
            .f64_histogram("execution.total.duration")
            .with_description(
                "Total duration of action execution from submission to completion in seconds",
            )
            .with_unit("s")
            .with_boundaries(vec![
                // Sub-second range
                0.01, // 10ms
                0.1,  // 100ms
                0.5,  // 500ms
                1.0,  // 1s
                // Multi-second range
                5.0,    // 5s
                10.0,   // 10s
                30.0,   // 30s
                60.0,   // 1 minute
                300.0,  // 5 minutes
                600.0,  // 10 minutes
                1800.0, // 30 minutes
                3600.0, // 1 hour
                7200.0, // 2 hours
            ])
            .build(),

        execution_queue_time: meter
            .f64_histogram("execution.queue.time")
            .with_description("Time spent waiting in queue before execution in seconds")
            .with_unit("s")
            .with_boundaries(vec![
                0.001, // 1ms
                0.01,  // 10ms
                0.1,   // 100ms
                0.5,   // 500ms
                1.0,   // 1s
                2.0,   // 2s
                5.0,   // 5s
                10.0,  // 10s
                30.0,  // 30s
                60.0,  // 1 minute
                300.0, // 5 minutes
                600.0, // 10 minutes
            ])
            .build(),

        execution_active_count: meter
            .i64_up_down_counter("execution.active.count")
            .with_description("Number of actions currently in each stage")
            .with_unit("{action}")
            .build(),

        execution_completed_count: meter
            .u64_counter("execution.completed.count")
            .with_description("Total number of completed executions by result")
            .with_unit("{action}")
            .build(),

        execution_stage_transitions: meter
            .u64_counter("execution.stage.transitions")
            .with_description("Number of stage transitions")
            .with_unit("{transition}")
            .build(),

        execution_cpu_time: meter
            .f64_histogram("execution.cpu.time")
            .with_description("CPU time consumed by an action in seconds")
            .with_unit("s")
            .with_boundaries(vec![
                0.1, 0.5, 1.0, 5.0, 10.0, 30.0, 60.0, 300.0, 600.0, 1800.0, 3600.0, 7200.0,
            ])
            .build(),

        execution_peak_memory: meter
            .u64_histogram("execution.peak.memory")
            .with_description("Peak resident memory observed while running an action in bytes")
            .with_unit("By")
            .with_boundaries(vec![
                1_048_576.0,      // 1MiB
                16_777_216.0,     // 16MiB
                67_108_864.0,     // 64MiB
                268_435_456.0,    // 256MiB
                1_073_741_824.0,  // 1GiB
                4_294_967_296.0,  // 4GiB
                17_179_869_184.0, // 16GiB
                68_719_476_736.0, // 64GiB
            ])
            .build(),

        execution_output_size: meter
            .u64_histogram("execution.output.size")
            .with_description("Size of execution outputs in bytes")
            .with_unit("By")
            .with_boundaries(vec![
                1_024.0,          // 1KB
                10_240.0,         // 10KB
                102_400.0,        // 100KB
                1_048_576.0,      // 1MB
                10_485_760.0,     // 10MB
                104_857_600.0,    // 100MB
                1_073_741_824.0,  // 1GB
                10_737_418_240.0, // 10GB
            ])
            .build(),

        execution_retry_count: meter
            .u64_counter("execution.retry.count")
            .with_description("Number of execution retries")
            .with_unit("{retry}")
            .build(),
    }
});

/// OpenTelemetry metrics instruments for remote execution monitoring.
#[derive(Debug)]
pub struct ExecutionMetrics {
    /// Histogram of stage durations in seconds
    pub execution_stage_duration: metrics::Histogram<f64>,
    /// Histogram of total execution durations in seconds
    pub execution_total_duration: metrics::Histogram<f64>,
    /// Histogram of queue wait times in seconds
    pub execution_queue_time: metrics::Histogram<f64>,
    /// Current number of actions in each stage
    pub execution_active_count: metrics::UpDownCounter<i64>,
    /// Total number of completed executions
    pub execution_completed_count: metrics::Counter<u64>,
    /// Number of stage transitions
    pub execution_stage_transitions: metrics::Counter<u64>,
    /// Histogram of output sizes in bytes
    pub execution_output_size: metrics::Histogram<u64>,
    /// Counter for execution retries
    pub execution_retry_count: metrics::Counter<u64>,
    /// Peak memory an action used, as sampled by the worker.
    pub execution_peak_memory: metrics::Histogram<u64>,
    /// CPU time an action used, as sampled by the worker.
    pub execution_cpu_time: metrics::Histogram<f64>,
}

/// Records the CPU time a worker observed for an action.
///
/// Wall time is already covered by `execution.stage.duration`. This is the
/// other half: an action pinning eight cores for a minute and one sleeping
/// for a minute look identical by wall time and nothing alike here.
pub fn record_execution_cpu_time(cpu_time_ms: u64, instance_name: &str) {
    #[expect(clippy::cast_precision_loss)] // Milliseconds; f64 is exact well past any real action.
    let seconds = cpu_time_ms as f64 / 1000.0;
    EXECUTION_METRICS.execution_cpu_time.record(
        seconds,
        &[KeyValue::new(EXECUTION_INSTANCE, instance_name.to_string())],
    );
}

/// Records the peak memory a worker observed while running an action.
///
/// The worker samples this, so it is only present when sampling ran. #2614
/// listed `execution_memory_usage` as unimplementable because
/// `ExecutionMetadata` carries no memory figure; `ActionResourceUsage` does,
/// and this is that number.
pub fn record_execution_peak_memory(peak_memory_kb: u64, instance_name: &str) {
    EXECUTION_METRICS.execution_peak_memory.record(
        peak_memory_sample(peak_memory_kb),
        &[KeyValue::new(EXECUTION_INSTANCE, instance_name.to_string())],
    );
}

/// Ceiling on a recorded peak-memory sample.
///
/// Well above any real action and far below the point where a histogram's
/// `u64` sum can wrap, so it filters a garbage reading without touching a
/// plausible one. Everything past the largest bucket already shares the same
/// bucket, so this changes no bucket count.
const PEAK_MEMORY_MAX_BYTES: u64 = 1 << 50; // 1 PiB.

/// Converts a worker's peak-memory reading to bytes, bounded.
///
/// The figure crosses a gRPC boundary from a worker, so a malfunctioning one
/// could report something absurd. An unbounded value overflows the
/// histogram's `u64` sum, which panics a debug build and takes the thread
/// with it. Kept separate so it is directly testable: the overflow only
/// happens once a meter provider is installed, which no unit test does.
#[must_use]
pub fn peak_memory_sample(peak_memory_kb: u64) -> u64 {
    peak_memory_kb
        .saturating_mul(1024)
        .min(PEAK_MEMORY_MAX_BYTES)
}

/// Helper function to create attributes for execution metrics
#[must_use]
pub fn make_execution_attributes(
    instance_name: &str,
    worker_id: Option<&str>,
    priority: Option<i32>,
) -> Vec<KeyValue> {
    let mut attrs = vec![KeyValue::new(EXECUTION_INSTANCE, instance_name.to_string())];

    if let Some(worker_id) = worker_id {
        attrs.push(KeyValue::new(EXECUTION_WORKER_ID, worker_id.to_string()));
    }

    if let Some(priority) = priority {
        attrs.push(KeyValue::new(EXECUTION_PRIORITY, i64::from(priority)));
    }

    attrs
}

/// Records the histogram metrics derivable from a completed action's result.
pub fn record_completed_execution_metrics(
    action_result: &ActionResult,
    instance_name: &str,
    worker_id: Option<&str>,
    priority: Option<i32>,
) {
    let m = &*EXECUTION_METRICS;
    let md = &action_result.execution_metadata;
    let base = make_execution_attributes(instance_name, worker_id, priority);

    let record_secs =
        |hist: &metrics::Histogram<f64>, start: SystemTime, end: SystemTime, attrs: &[KeyValue]| {
            if start > SystemTime::UNIX_EPOCH
                && let Ok(d) = end.duration_since(start)
            {
                hist.record(d.as_secs_f64(), attrs);
            }
        };

    // Queue wait (queued -> worker picked it up) and end-to-end duration.
    record_secs(
        &m.execution_queue_time,
        md.queued_timestamp,
        md.worker_start_timestamp,
        &base,
    );
    record_secs(
        &m.execution_total_duration,
        md.queued_timestamp,
        md.worker_completed_timestamp,
        &base,
    );

    // Per-phase stage durations, labeled by phase on the stage attribute.
    for (phase, start, end) in [
        (
            "input_fetch",
            md.input_fetch_start_timestamp,
            md.input_fetch_completed_timestamp,
        ),
        (
            "execution",
            md.execution_start_timestamp,
            md.execution_completed_timestamp,
        ),
        (
            "output_upload",
            md.output_upload_start_timestamp,
            md.output_upload_completed_timestamp,
        ),
    ] {
        let mut attrs = base.clone();
        attrs.push(KeyValue::new(EXECUTION_STAGE, phase));
        record_secs(&m.execution_stage_duration, start, end, &attrs);
    }

    // Total bytes produced: output files plus stdout/stderr.
    m.execution_output_size
        .record(execution_output_bytes(action_result), &base);
}

/// Total output bytes produced by an action: the output file digests plus the
/// stdout and stderr digests.
#[must_use]
pub fn execution_output_bytes(action_result: &ActionResult) -> u64 {
    action_result
        .output_files
        .iter()
        .map(|f| f.digest.size_bytes())
        .sum::<u64>()
        + action_result.stdout_digest.size_bytes()
        + action_result.stderr_digest.size_bytes()
}

/// Global worker fleet metrics instruments.
pub static WORKER_METRICS: LazyLock<WorkerMetrics> = LazyLock::new(|| {
    let meter = global::meter_with_scope(InstrumentationScope::builder("nativelink").build());

    WorkerMetrics {
        worker_connected_count: meter
            .i64_up_down_counter("worker.connected.count")
            .with_description("Number of workers currently connected to this scheduler instance")
            .with_unit("{worker}")
            .build(),

        worker_connections: meter
            .u64_counter("worker.connections")
            .with_description("Total number of workers that have joined the pool")
            .with_unit("{worker}")
            .build(),

        worker_disconnections: meter
            .u64_counter("worker.disconnections")
            .with_description("Total number of workers that have left the pool, by reason")
            .with_unit("{worker}")
            .build(),

        worker_keepalives: meter
            .u64_counter("worker.keepalives")
            .with_description("Total worker keepalives received")
            .with_unit("{keepalive}")
            .build(),

        worker_state_count: meter
            .i64_up_down_counter("worker.state.count")
            .with_description("Number of connected workers in each non-default state")
            .with_unit("{worker}")
            .build(),
    }
});

/// Worker fleet metrics.
///
/// These are per-scheduler-instance. A worker only appears in the registry of
/// the instance holding its stream, so with several schedulers behind a load
/// balancer the fleet total is the sum across instances, not any one of them.
///
/// Deliberately not attributed by worker id. Ids are per connection, so a
/// fleet that churns would grow the label set without bound, and the
/// autoscaling and fleet-health questions these answer are all aggregate.
#[derive(Debug)]
pub struct WorkerMetrics {
    /// Workers currently connected.
    pub worker_connected_count: metrics::UpDownCounter<i64>,
    /// Workers that have joined, cumulative.
    pub worker_connections: metrics::Counter<u64>,
    /// Workers that have left, cumulative, by reason.
    pub worker_disconnections: metrics::Counter<u64>,
    /// Keepalives received, cumulative.
    pub worker_keepalives: metrics::Counter<u64>,
    /// Connected workers currently paused or draining.
    pub worker_state_count: metrics::UpDownCounter<i64>,
}

/// Records a worker joining the pool.
pub fn record_worker_connected() {
    WORKER_METRICS.worker_connected_count.add(1, &[]);
    WORKER_METRICS.worker_connections.add(1, &[]);
}

/// Records a worker leaving the pool.
///
/// `was_draining` and `was_paused` unwind the state gauges, which would
/// otherwise keep counting a worker that is already gone.
pub fn record_worker_disconnected(
    reason: WorkerDisconnectReason,
    was_draining: bool,
    was_paused: bool,
) {
    WORKER_METRICS.worker_connected_count.add(-1, &[]);
    WORKER_METRICS.worker_disconnections.add(
        1,
        &[KeyValue::new(WORKER_DISCONNECT_REASON, reason.as_str())],
    );
    if was_draining {
        record_worker_state("draining", false);
    }
    if was_paused {
        record_worker_state("paused", false);
    }
}

/// Records a worker entering or leaving `state`.
pub fn record_worker_state(state: &'static str, entered: bool) {
    WORKER_METRICS.worker_state_count.add(
        if entered { 1 } else { -1 },
        &[KeyValue::new(WORKER_STATE, state)],
    );
}

/// Records a keepalive from a worker.
pub fn record_worker_keepalive() {
    WORKER_METRICS.worker_keepalives.add(1, &[]);
}

/// Global gRPC serving metrics.
///
/// One duration histogram covers rate, errors and latency: the count gives
/// rate, the status attribute separates errors, and the buckets give latency.
pub static RPC_METRICS: LazyLock<RpcMetrics> = LazyLock::new(|| {
    let meter = global::meter_with_scope(InstrumentationScope::builder("nativelink").build());

    RpcMetrics {
        rpc_server_duration: meter
            .f64_histogram("rpc.server.duration")
            .with_description("Duration of inbound gRPC calls in seconds")
            .with_unit("s")
            .with_boundaries(vec![
                0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0, 30.0, 60.0,
                300.0,
            ])
            .build(),
    }
});

/// gRPC serving metrics.
#[derive(Debug)]
pub struct RpcMetrics {
    /// Duration of inbound gRPC calls, by service, method and status.
    pub rpc_server_duration: metrics::Histogram<f64>,
}

/// Records a served gRPC call.
///
/// `full_path` is the HTTP path tonic routes on, `/package.Service/Method`.
pub fn record_rpc_served(full_path: &str, grpc_status: i32, duration_secs: f64) {
    let (service, method) = split_grpc_path(full_path);
    RPC_METRICS.rpc_server_duration.record(
        duration_secs,
        &[
            KeyValue::new(RPC_SERVICE, service.to_string()),
            KeyValue::new(RPC_METHOD, method.to_string()),
            KeyValue::new(RPC_STATUS_CODE, i64::from(grpc_status)),
        ],
    );
}

/// Splits `/package.Service/Method` into its service and method halves.
///
/// Anything that does not look like a gRPC path is reported whole under an
/// `unknown` method, so an unexpected route still shows up rather than
/// silently vanishing. Both halves come from the route table, not user input,
/// so the label set stays bounded.
#[must_use]
pub fn split_grpc_path(full_path: &str) -> (&str, &str) {
    let trimmed = full_path.strip_prefix('/').unwrap_or(full_path);
    trimmed
        .split_once('/')
        .map_or((trimmed, "unknown"), |(service, method)| (service, method))
}

/// Global scheduler metrics.
pub static SCHEDULER_METRICS: LazyLock<SchedulerOtlpMetrics> = LazyLock::new(|| {
    let meter = global::meter_with_scope(InstrumentationScope::builder("nativelink").build());

    SchedulerOtlpMetrics {
        matching_duration: meter
            .f64_histogram("scheduler.matching.duration")
            .with_description("Duration of one queued-action to worker matching pass in seconds")
            .with_unit("s")
            .with_boundaries(vec![
                0.0001, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
            ])
            .build(),

        matching_passes: meter
            .u64_counter("scheduler.matching.passes")
            .with_description("Matching passes run, by result")
            .with_unit("{pass}")
            .build(),
    }
});

/// Scheduler matching metrics.
///
/// Named to avoid colliding with the existing `#[metric]` component struct of
/// the same idea in the scheduler crate.
#[derive(Debug)]
pub struct SchedulerOtlpMetrics {
    /// How long a matching pass takes. The saturation signal: this climbing
    /// while the queue is non-empty means matching is the bottleneck.
    pub matching_duration: metrics::Histogram<f64>,
    /// Matching passes, by result.
    pub matching_passes: metrics::Counter<u64>,
}

/// Records a completed matching pass.
pub fn record_matching_pass(duration_secs: f64, succeeded: bool) {
    SCHEDULER_METRICS
        .matching_duration
        .record(duration_secs, &[]);
    SCHEDULER_METRICS.matching_passes.add(
        1,
        &[KeyValue::new(
            SCHEDULER_MATCH_RESULT,
            if succeeded { "ok" } else { "error" },
        )],
    );
}

/// Global tiered-store metrics.
pub static STORE_TIER_METRICS: LazyLock<StoreTierMetrics> = LazyLock::new(|| {
    let meter = global::meter_with_scope(InstrumentationScope::builder("nativelink").build());

    StoreTierMetrics {
        tier_operations: meter
            .u64_counter("store.tier.operations")
            .with_description("Reads served by each tier of a tiered store, by result")
            .with_unit("{operation}")
            .build(),

        tier_io: meter
            .u64_counter("store.tier.io")
            .with_description("Bytes moved through each tier of a tiered store")
            .with_unit("By")
            .build(),
    }
});

/// Tiered-store metrics. The hit ratio is `tier_operations{result="hit"}` over
/// the sum, which is the number worth alerting on for a fast/slow store.
#[derive(Debug)]
pub struct StoreTierMetrics {
    /// Reads served per tier, by result.
    pub tier_operations: metrics::Counter<u64>,
    /// Bytes moved per tier and direction.
    pub tier_io: metrics::Counter<u64>,
}

/// Records a read served, or not served, by a tier.
pub fn record_store_tier_read(tier: &'static str, result: &'static str) {
    STORE_TIER_METRICS.tier_operations.add(
        1,
        &[
            KeyValue::new(STORE_TIER, tier),
            KeyValue::new(STORE_RESULT, result),
        ],
    );
}

/// Records bytes moved through a tier.
pub fn record_store_tier_io(tier: &'static str, direction: &'static str, bytes: u64) {
    STORE_TIER_METRICS.tier_io.add(
        bytes,
        &[
            KeyValue::new(STORE_TIER, tier),
            KeyValue::new(STORE_DIRECTION, direction),
        ],
    );
}

/// Global health-check metrics.
pub static HEALTH_METRICS: LazyLock<HealthMetrics> = LazyLock::new(|| {
    let meter = global::meter_with_scope(InstrumentationScope::builder("nativelink").build());

    HealthMetrics {
        health_checks: meter
            .u64_counter("health.checks")
            .with_description("Health check results, by component and status")
            .with_unit("{check}")
            .build(),
    }
});

/// Health-check metrics.
#[derive(Debug)]
pub struct HealthMetrics {
    /// Health check results, by namespace and status.
    pub health_checks: metrics::Counter<u64>,
}

/// Records one component's health check result.
pub fn record_health_check(namespace: &str, status: &'static str) {
    HEALTH_METRICS.health_checks.add(
        1,
        &[
            KeyValue::new(HEALTH_NAMESPACE, namespace.to_string()),
            KeyValue::new(HEALTH_STATUS, status),
        ],
    );
}

/// Global connection-pool metrics.
pub static CONNECTION_METRICS: LazyLock<ConnectionMetrics> = LazyLock::new(|| {
    let meter = global::meter_with_scope(InstrumentationScope::builder("nativelink").build());

    ConnectionMetrics {
        pool_available: meter
            .u64_histogram("connection.pool.available")
            .with_description("Free slots in a connection pool, sampled when one is taken")
            .with_unit("{connection}")
            .with_boundaries(vec![
                0.0, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0, 512.0,
            ])
            .build(),

        pool_acquisitions: meter
            .u64_counter("connection.pool.acquisitions")
            .with_description("Connection acquisitions, by pool and whether one was free")
            .with_unit("{acquisition}")
            .build(),

        reconnects: meter
            .u64_counter("connection.reconnects")
            .with_description("Reconnects performed, by pool")
            .with_unit("{reconnect}")
            .build(),
    }
});

/// Connection-pool metrics.
///
/// `pool_available` is the saturation signal: a pool that keeps reporting zero
/// free slots is the bottleneck, whatever the latency elsewhere says. Counting
/// queued acquisitions gives the same answer from the other direction.
#[derive(Debug)]
pub struct ConnectionMetrics {
    /// Free slots at the moment a connection was taken.
    pub pool_available: metrics::Histogram<u64>,
    /// Acquisitions, by pool and result.
    pub pool_acquisitions: metrics::Counter<u64>,
    /// Reconnects, by pool.
    pub reconnects: metrics::Counter<u64>,
}

/// Ceiling on a recorded free-slot count.
///
/// An unbounded pool is a semaphore holding `Semaphore::MAX_PERMITS`, around
/// 2.3e18. Recording that overflows the histogram's `u64` sum after a handful
/// of samples, which panics a debug build. Callers pass `None` for an
/// unbounded pool, and this clamp is the backstop if one ever does not: it
/// sits above the largest bucket boundary, so bucket counts are unchanged.
const POOL_AVAILABLE_MAX: u64 = 1024;

/// Records a connection being taken from `pool`.
///
/// `available` is the free-slot count, or `None` when the pool is unbounded
/// and the figure would be meaningless. `queued` means nothing was free and
/// the caller had to wait, which is the case worth alerting on.
/// Clamps a free-slot count to something a histogram can accumulate.
///
/// Kept separate so it is directly testable: the failure this guards against
/// only appears once a meter provider is installed, so a test that merely
/// calls `record_connection_acquired` cannot catch it.
#[must_use]
pub fn pool_available_sample(available: usize) -> u64 {
    u64::try_from(available)
        .unwrap_or(POOL_AVAILABLE_MAX)
        .min(POOL_AVAILABLE_MAX)
}

pub fn record_connection_acquired(pool: &'static str, available: Option<usize>, queued: bool) {
    if let Some(available) = available {
        CONNECTION_METRICS.pool_available.record(
            pool_available_sample(available),
            &[KeyValue::new(CONNECTION_POOL, pool)],
        );
    }
    CONNECTION_METRICS.pool_acquisitions.add(
        1,
        &[
            KeyValue::new(CONNECTION_POOL, pool),
            KeyValue::new(
                CONNECTION_RESULT,
                if queued { "queued" } else { "immediate" },
            ),
        ],
    );
}

/// Records a reconnect. A rising count means the backend is flapping, which
/// on Redis usually means a failover.
pub fn record_connection_reconnect(pool: &'static str) {
    CONNECTION_METRICS
        .reconnects
        .add(1, &[KeyValue::new(CONNECTION_POOL, pool)]);
}
