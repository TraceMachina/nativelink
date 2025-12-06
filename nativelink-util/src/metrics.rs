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

use opentelemetry::{InstrumentationScope, KeyValue, Value, global, metrics};

use crate::action_messages::ActionStage;

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
            ActionStage::Unknown => ExecutionStage::Unknown,
            ActionStage::CacheCheck => ExecutionStage::CacheCheck,
            ActionStage::Queued => ExecutionStage::Queued,
            ActionStage::Executing => ExecutionStage::Executing,
            ActionStage::Completed(_) | ActionStage::CompletedFromCache(_) => {
                ExecutionStage::Completed
            }
        }
    }
}

impl From<&ActionStage> for ExecutionStage {
    fn from(stage: &ActionStage) -> Self {
        match stage {
            ActionStage::Unknown => ExecutionStage::Unknown,
            ActionStage::CacheCheck => ExecutionStage::CacheCheck,
            ActionStage::Queued => ExecutionStage::Queued,
            ActionStage::Executing => ExecutionStage::Executing,
            ActionStage::Completed(_) | ActionStage::CompletedFromCache(_) => {
                ExecutionStage::Completed
            }
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

        execution_cpu_time: meter
            .f64_histogram("execution.cpu.time")
            .with_description("CPU time consumed by action execution in seconds")
            .with_unit("s")
            .with_boundaries(vec![
                0.01,   // 10ms
                0.1,    // 100ms
                1.0,    // 1s
                10.0,   // 10s
                60.0,   // 1 minute
                300.0,  // 5 minutes
                600.0,  // 10 minutes
                1800.0, // 30 minutes
                3600.0, // 1 hour
            ])
            .build(),

        execution_memory_usage: meter
            .u64_histogram("execution.memory.usage")
            .with_description("Peak memory usage during execution in bytes")
            .with_unit("By")
            .with_boundaries(vec![
                1_048_576.0,      // 1MB
                10_485_760.0,     // 10MB
                104_857_600.0,    // 100MB
                524_288_000.0,    // 500MB
                1_073_741_824.0,  // 1GB
                5_368_709_120.0,  // 5GB
                10_737_418_240.0, // 10GB
                53_687_091_200.0, // 50GB
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
    /// Histogram of CPU time in seconds
    pub execution_cpu_time: metrics::Histogram<f64>,
    /// Histogram of peak memory usage in bytes
    pub execution_memory_usage: metrics::Histogram<u64>,
    /// Counter for execution retries
    pub execution_retry_count: metrics::Counter<u64>,
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
