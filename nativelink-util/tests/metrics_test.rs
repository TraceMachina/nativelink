// Copyright 2025 The NativeLink Authors. All rights reserved.
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

use nativelink_util::action_messages::{ActionResult, ActionStage};
use nativelink_util::metrics::{
    CACHE_METRICS, CacheMetricAttrs, EXECUTION_METRICS, ExecutionMetricAttrs, ExecutionStage,
    make_execution_attributes,
};
use opentelemetry::KeyValue;

#[test]
fn test_cache_metric_attrs() {
    let base_attrs = vec![
        KeyValue::new("cache.type", "test_cache"),
        KeyValue::new("instance", "test_instance"),
    ];

    let attrs = CacheMetricAttrs::new(&base_attrs);

    // Verify that the pre-computed attributes contain the expected values
    let read_hit_attrs = attrs.read_hit();
    assert_eq!(read_hit_attrs.len(), 4);
    assert!(
        read_hit_attrs
            .iter()
            .any(|kv| kv.key.as_str() == "cache.type" && kv.value.to_string() == "test_cache")
    );
    assert!(
        read_hit_attrs
            .iter()
            .any(|kv| kv.key.as_str() == "cache.operation.name" && kv.value.to_string() == "read")
    );
    assert!(
        read_hit_attrs
            .iter()
            .any(|kv| kv.key.as_str() == "cache.operation.result" && kv.value.to_string() == "hit")
    );
}

#[test]
fn test_execution_metric_attrs() {
    let base_attrs = vec![
        KeyValue::new("execution.instance", "test_instance"),
        KeyValue::new("execution.worker_id", "worker_123"),
    ];

    let attrs = ExecutionMetricAttrs::new(&base_attrs);

    // Verify that the pre-computed attributes contain the expected values
    let queued_attrs = attrs.queued();
    assert_eq!(queued_attrs.len(), 3);
    assert!(queued_attrs.iter().any(
        |kv| kv.key.as_str() == "execution.instance" && kv.value.to_string() == "test_instance"
    ));
    assert!(
        queued_attrs
            .iter()
            .any(|kv| kv.key.as_str() == "execution.stage" && kv.value.to_string() == "queued")
    );

    let completed_success_attrs = attrs.completed_success();
    assert_eq!(completed_success_attrs.len(), 4);
    assert!(
        completed_success_attrs
            .iter()
            .any(|kv| kv.key.as_str() == "execution.stage" && kv.value.to_string() == "completed")
    );
    assert!(
        completed_success_attrs
            .iter()
            .any(|kv| kv.key.as_str() == "execution.result" && kv.value.to_string() == "success")
    );
}

#[test]
fn test_make_execution_attributes() {
    let attrs = make_execution_attributes("test_instance", Some("worker_456"), Some(100));

    assert_eq!(attrs.len(), 3);
    assert!(attrs.iter().any(
        |kv| kv.key.as_str() == "execution.instance" && kv.value.to_string() == "test_instance"
    ));
    assert!(
        attrs
            .iter()
            .any(|kv| kv.key.as_str() == "execution.worker_id"
                && kv.value.to_string() == "worker_456")
    );
    assert!(
        attrs
            .iter()
            .any(|kv| kv.key.as_str() == "execution.priority"
                && kv.value == opentelemetry::Value::I64(100))
    );
}

#[test]
fn test_metrics_lazy_initialization() {
    // Verify that the lazy static initialization works
    let _cache_metrics = &*CACHE_METRICS;
    let _execution_metrics = &*EXECUTION_METRICS;

    // If we got here without panicking, the metrics were initialized successfully
}

#[test]
fn test_action_stage_to_execution_stage_conversion() {
    // Test conversion from owned ActionStage values
    assert_eq!(
        ExecutionStage::from(ActionStage::Unknown),
        ExecutionStage::Unknown
    );
    assert_eq!(
        ExecutionStage::from(ActionStage::CacheCheck),
        ExecutionStage::CacheCheck
    );
    assert_eq!(
        ExecutionStage::from(ActionStage::Queued),
        ExecutionStage::Queued
    );
    assert_eq!(
        ExecutionStage::from(ActionStage::Executing),
        ExecutionStage::Executing
    );

    // Test that Completed variants map to ExecutionStage::Completed
    let action_result = ActionResult::default();
    assert_eq!(
        ExecutionStage::from(ActionStage::Completed(action_result.clone())),
        ExecutionStage::Completed
    );

    // Note: We can't easily test CompletedFromCache without creating a ProtoActionResult,
    // but the implementation handles it the same as Completed
}

#[test]
fn test_action_stage_ref_to_execution_stage_conversion() {
    // Test conversion from ActionStage references
    let unknown = ActionStage::Unknown;
    let cache_check = ActionStage::CacheCheck;
    let queued = ActionStage::Queued;
    let executing = ActionStage::Executing;
    let completed = ActionStage::Completed(ActionResult::default());

    assert_eq!(ExecutionStage::from(&unknown), ExecutionStage::Unknown);
    assert_eq!(
        ExecutionStage::from(&cache_check),
        ExecutionStage::CacheCheck
    );
    assert_eq!(ExecutionStage::from(&queued), ExecutionStage::Queued);
    assert_eq!(ExecutionStage::from(&executing), ExecutionStage::Executing);
    assert_eq!(ExecutionStage::from(&completed), ExecutionStage::Completed);
}

#[test]
fn test_action_stage_conversion_avoids_clone() {
    use nativelink_util::action_messages::{FileInfo, NameOrPath};
    use nativelink_util::common::DigestInfo;

    // This test verifies that using a reference doesn't clone the large ActionResult
    let large_file_info = FileInfo {
        name_or_path: NameOrPath::Path("test.txt".to_string()),
        digest: DigestInfo::new([0u8; 32], 100),
        is_executable: false,
    };
    let large_action_result = ActionResult {
        output_files: vec![large_file_info; 1000], // Large vector to make clone expensive
        ..Default::default()
    };
    let completed = ActionStage::Completed(large_action_result);

    // Using a reference should be fast even with large data
    let start = std::time::Instant::now();
    for _ in 0..10000 {
        let _stage = ExecutionStage::from(&completed);
    }
    let elapsed = start.elapsed();

    // This should complete very quickly since we're not cloning
    // In practice, 10000 conversions should take less than 1ms
    assert!(
        elapsed.as_millis() < 100,
        "Reference conversion took too long: {:?}",
        elapsed
    );
}
