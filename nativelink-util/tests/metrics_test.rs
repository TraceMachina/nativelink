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

use nativelink_util::metrics::{
    CACHE_METRICS, CacheMetricAttrs, EXECUTION_METRICS, ExecutionMetricAttrs,
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
