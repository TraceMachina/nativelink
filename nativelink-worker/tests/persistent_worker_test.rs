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

use std::collections::BTreeMap;

use nativelink_proto::build::bazel::remote::execution::v2::Platform;
use nativelink_proto::build::bazel::remote::execution::v2::platform::Property;
use nativelink_worker::persistent_worker::PersistentWorkerKey;

#[test]
fn test_persistent_worker_key_from_platform() {
    let platform = Platform {
        properties: vec![
            Property {
                name: "persistentWorkerKey".to_string(),
                value: "javac-worker-123".to_string(),
            },
            Property {
                name: "persistentWorkerTool".to_string(),
                value: "javac".to_string(),
            },
            Property {
                name: "cpu".to_string(),
                value: "4".to_string(),
            },
        ],
    };

    let key = PersistentWorkerKey::from_platform(&platform).unwrap();
    pretty_assertions::assert_eq!(key.tool, "javac");
    pretty_assertions::assert_eq!(key.key, "javac-worker-123");
    pretty_assertions::assert_eq!(key.platform_properties.get("cpu"), Some(&"4".to_string()));
}

#[test]
fn test_persistent_worker_key_missing_required_fields() {
    let platform = Platform {
        properties: vec![Property {
            name: "cpu".to_string(),
            value: "4".to_string(),
        }],
    };

    assert!(PersistentWorkerKey::from_platform(&platform).is_none());
}

#[test]
fn test_persistent_worker_key_to_platform_properties() {
    let key = PersistentWorkerKey {
        tool: "scalac".to_string(),
        key: "scalac-456".to_string(),
        platform_properties: BTreeMap::from([
            ("memory".to_string(), "8GB".to_string()),
            ("os".to_string(), "linux".to_string()),
        ]),
    };

    let props = key.to_platform_properties();
    assert!(props.contains(&("persistentWorkerKey".to_string(), "scalac-456".to_string())));
    assert!(props.contains(&("persistentWorkerTool".to_string(), "scalac".to_string())));
    assert!(props.contains(&("memory".to_string(), "8GB".to_string())));
    assert!(props.contains(&("os".to_string(), "linux".to_string())));
}
