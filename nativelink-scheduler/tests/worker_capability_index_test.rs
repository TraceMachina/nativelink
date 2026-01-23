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

//! Tests for the worker capability index.

use std::collections::HashMap;

use nativelink_scheduler::worker_capability_index::WorkerCapabilityIndex;
use nativelink_util::action_messages::WorkerId;
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};

fn make_worker_id(name: &str) -> WorkerId {
    WorkerId(name.to_string())
}

fn make_properties(props: &[(&str, PlatformPropertyValue)]) -> PlatformProperties {
    let mut map = HashMap::new();
    for (name, value) in props {
        map.insert((*name).to_string(), value.clone());
    }
    PlatformProperties::new(map)
}

#[test]
fn test_empty_index() {
    let index = WorkerCapabilityIndex::new();
    let props = make_properties(&[]);
    let result = index.find_matching_workers(&props, true);
    assert!(result.is_empty());
}

#[test]
fn test_exact_property_match() {
    let mut index = WorkerCapabilityIndex::new();

    let worker1 = make_worker_id("worker1");
    let worker2 = make_worker_id("worker2");

    index.add_worker(
        &worker1,
        &make_properties(&[("os", PlatformPropertyValue::Exact("linux".to_string()))]),
    );
    index.add_worker(
        &worker2,
        &make_properties(&[("os", PlatformPropertyValue::Exact("windows".to_string()))]),
    );

    // Match linux
    let linux_props = make_properties(&[("os", PlatformPropertyValue::Exact("linux".to_string()))]);
    let result = index.find_matching_workers(&linux_props, true);
    assert_eq!(result.len(), 1);
    assert!(result.contains(&worker1));

    // Match windows
    let windows_props =
        make_properties(&[("os", PlatformPropertyValue::Exact("windows".to_string()))]);
    let result = index.find_matching_workers(&windows_props, true);
    assert_eq!(result.len(), 1);
    assert!(result.contains(&worker2));
}

#[test]
fn test_minimum_property_presence_only() {
    // The index only checks PRESENCE of Minimum properties, not their values.
    // Actual value checking is done at runtime by the caller since Minimum
    // values are dynamic (change as jobs are assigned to workers).
    let mut index = WorkerCapabilityIndex::new();

    let worker1 = make_worker_id("worker1");
    let worker2 = make_worker_id("worker2");
    let worker3 = make_worker_id("worker3");

    index.add_worker(
        &worker1,
        &make_properties(&[("cpu_count", PlatformPropertyValue::Minimum(4))]),
    );
    index.add_worker(
        &worker2,
        &make_properties(&[("cpu_count", PlatformPropertyValue::Minimum(8))]),
    );
    // Worker3 has no cpu_count property
    index.add_worker(
        &worker3,
        &make_properties(&[("os", PlatformPropertyValue::Exact("linux".to_string()))]),
    );

    // Any request for cpu_count returns workers that HAVE the property (regardless of value)
    let props = make_properties(&[("cpu_count", PlatformPropertyValue::Minimum(2))]);
    let result = index.find_matching_workers(&props, true);
    assert_eq!(result.len(), 2);
    assert!(result.contains(&worker1));
    assert!(result.contains(&worker2));
    assert!(!result.contains(&worker3)); // Doesn't have cpu_count

    // Even a high value returns the same workers - actual value check is done at runtime
    let props = make_properties(&[("cpu_count", PlatformPropertyValue::Minimum(100))]);
    let result = index.find_matching_workers(&props, true);
    assert_eq!(result.len(), 2);
}

#[test]
fn test_mixed_properties() {
    let mut index = WorkerCapabilityIndex::new();

    let worker1 = make_worker_id("worker1");
    let worker2 = make_worker_id("worker2");
    let worker3 = make_worker_id("worker3");

    index.add_worker(
        &worker1,
        &make_properties(&[
            ("os", PlatformPropertyValue::Exact("linux".to_string())),
            ("cpu_count", PlatformPropertyValue::Minimum(4)),
        ]),
    );
    index.add_worker(
        &worker2,
        &make_properties(&[
            ("os", PlatformPropertyValue::Exact("linux".to_string())),
            ("cpu_count", PlatformPropertyValue::Minimum(8)),
        ]),
    );
    // Worker3 has different OS
    index.add_worker(
        &worker3,
        &make_properties(&[
            ("os", PlatformPropertyValue::Exact("windows".to_string())),
            ("cpu_count", PlatformPropertyValue::Minimum(16)),
        ]),
    );

    // Match linux with cpu_count - both linux workers match (Minimum is presence-only)
    let props = make_properties(&[
        ("os", PlatformPropertyValue::Exact("linux".to_string())),
        ("cpu_count", PlatformPropertyValue::Minimum(6)),
    ]);
    let result = index.find_matching_workers(&props, true);
    // Both worker1 and worker2 have linux OS and cpu_count property
    assert_eq!(result.len(), 2);
    assert!(result.contains(&worker1));
    assert!(result.contains(&worker2));
    assert!(!result.contains(&worker3)); // Different OS
}

#[test]
fn test_remove_worker() {
    let mut index = WorkerCapabilityIndex::new();

    let worker1 = make_worker_id("worker1");
    index.add_worker(
        &worker1,
        &make_properties(&[("os", PlatformPropertyValue::Exact("linux".to_string()))]),
    );

    assert_eq!(index.worker_count(), 1);

    index.remove_worker(&worker1);

    assert_eq!(index.worker_count(), 0);

    let props = make_properties(&[("os", PlatformPropertyValue::Exact("linux".to_string()))]);
    let result = index.find_matching_workers(&props, true);
    assert!(result.is_empty());
}

#[test]
fn test_no_properties_matches_all() {
    let mut index = WorkerCapabilityIndex::new();

    let worker1 = make_worker_id("worker1");
    let worker2 = make_worker_id("worker2");

    index.add_worker(
        &worker1,
        &make_properties(&[("os", PlatformPropertyValue::Exact("linux".to_string()))]),
    );
    index.add_worker(&worker2, &make_properties(&[]));

    // No properties required - all workers match
    let props = make_properties(&[]);
    let result = index.find_matching_workers(&props, true);
    assert_eq!(result.len(), 2);
}

#[test]
fn test_priority_property() {
    let mut index = WorkerCapabilityIndex::new();

    let worker1 = make_worker_id("worker1");
    let worker2 = make_worker_id("worker2");

    index.add_worker(
        &worker1,
        &make_properties(&[("pool", PlatformPropertyValue::Priority("high".to_string()))]),
    );
    index.add_worker(
        &worker2,
        &make_properties(&[("pool", PlatformPropertyValue::Priority("low".to_string()))]),
    );

    // Priority just checks presence, so any pool value matches workers with pool
    let props = make_properties(&[("pool", PlatformPropertyValue::Priority("any".to_string()))]);
    let result = index.find_matching_workers(&props, true);
    assert_eq!(result.len(), 2);
}
