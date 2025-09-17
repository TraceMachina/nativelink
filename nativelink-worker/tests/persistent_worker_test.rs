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

#[test]
fn test_persistent_worker_key_empty_platform() {
    let platform = Platform { properties: vec![] };

    assert!(PersistentWorkerKey::from_platform(&platform).is_none());
}

#[test]
fn test_persistent_worker_key_only_tool_missing_key() {
    let platform = Platform {
        properties: vec![Property {
            name: "persistentWorkerTool".to_string(),
            value: "rustc".to_string(),
        }],
    };

    assert!(PersistentWorkerKey::from_platform(&platform).is_none());
}

#[test]
fn test_persistent_worker_key_only_key_missing_tool() {
    let platform = Platform {
        properties: vec![Property {
            name: "persistentWorkerKey".to_string(),
            value: "worker-789".to_string(),
        }],
    };

    assert!(PersistentWorkerKey::from_platform(&platform).is_none());
}

#[test]
fn test_persistent_worker_key_with_many_platform_properties() {
    let platform = Platform {
        properties: vec![
            Property {
                name: "persistentWorkerKey".to_string(),
                value: "complex-worker".to_string(),
            },
            Property {
                name: "persistentWorkerTool".to_string(),
                value: "gcc".to_string(),
            },
            Property {
                name: "arch".to_string(),
                value: "x86_64".to_string(),
            },
            Property {
                name: "os".to_string(),
                value: "darwin".to_string(),
            },
            Property {
                name: "compiler_version".to_string(),
                value: "11.0.0".to_string(),
            },
            Property {
                name: "optimization".to_string(),
                value: "-O2".to_string(),
            },
        ],
    };

    let key = PersistentWorkerKey::from_platform(&platform).unwrap();
    assert_eq!(key.tool, "gcc");
    assert_eq!(key.key, "complex-worker");
    assert_eq!(
        key.platform_properties.get("arch"),
        Some(&"x86_64".to_string())
    );
    assert_eq!(
        key.platform_properties.get("os"),
        Some(&"darwin".to_string())
    );
    assert_eq!(
        key.platform_properties.get("compiler_version"),
        Some(&"11.0.0".to_string())
    );
    assert_eq!(
        key.platform_properties.get("optimization"),
        Some(&"-O2".to_string())
    );
}

#[test]
fn test_persistent_worker_key_duplicate_properties() {
    let platform = Platform {
        properties: vec![
            Property {
                name: "persistentWorkerKey".to_string(),
                value: "first-key".to_string(),
            },
            Property {
                name: "persistentWorkerTool".to_string(),
                value: "clang".to_string(),
            },
            Property {
                name: "persistentWorkerKey".to_string(),
                value: "second-key".to_string(),
            },
            Property {
                name: "flag".to_string(),
                value: "value1".to_string(),
            },
            Property {
                name: "flag".to_string(),
                value: "value2".to_string(),
            },
        ],
    };

    let key = PersistentWorkerKey::from_platform(&platform).unwrap();
    assert_eq!(key.tool, "clang");
    // Last value should win for duplicates
    assert_eq!(key.key, "second-key");
    assert_eq!(
        key.platform_properties.get("flag"),
        Some(&"value2".to_string())
    );
}

#[test]
fn test_persistent_worker_key_empty_values() {
    let platform = Platform {
        properties: vec![
            Property {
                name: "persistentWorkerKey".to_string(),
                value: String::new(),
            },
            Property {
                name: "persistentWorkerTool".to_string(),
                value: String::new(),
            },
        ],
    };

    // Empty values should still create a valid key
    let key = PersistentWorkerKey::from_platform(&platform).unwrap();
    assert_eq!(key.tool, "");
    assert_eq!(key.key, "");
}

#[test]
fn test_persistent_worker_key_to_platform_properties_empty() {
    let key = PersistentWorkerKey {
        tool: "tsc".to_string(),
        key: "typescript-worker".to_string(),
        platform_properties: BTreeMap::new(),
    };

    let props = key.to_platform_properties();
    assert_eq!(props.len(), 2);
    assert!(props.contains(&(
        "persistentWorkerKey".to_string(),
        "typescript-worker".to_string()
    )));
    assert!(props.contains(&("persistentWorkerTool".to_string(), "tsc".to_string())));
}

#[test]
fn test_persistent_worker_key_case_sensitive() {
    let platform = Platform {
        properties: vec![
            Property {
                name: "PersistentWorkerKey".to_string(),
                value: "wrong-case".to_string(),
            },
            Property {
                name: "persistentWorkerTool".to_string(),
                value: "tool".to_string(),
            },
        ],
    };

    // Should not find the key with wrong case
    assert!(PersistentWorkerKey::from_platform(&platform).is_none());
}

#[test]
fn test_persistent_worker_key_special_characters() {
    let platform = Platform {
        properties: vec![
            Property {
                name: "persistentWorkerKey".to_string(),
                value: "worker-@#$%^&*()_+-=[]{}|;:',.<>?/~`".to_string(),
            },
            Property {
                name: "persistentWorkerTool".to_string(),
                value: "tool/with/path".to_string(),
            },
            Property {
                name: "path".to_string(),
                value: "/usr/local/bin/tool".to_string(),
            },
        ],
    };

    let key = PersistentWorkerKey::from_platform(&platform).unwrap();
    assert_eq!(key.tool, "tool/with/path");
    assert_eq!(key.key, "worker-@#$%^&*()_+-=[]{}|;:',.<>?/~`");
    assert_eq!(
        key.platform_properties.get("path"),
        Some(&"/usr/local/bin/tool".to_string())
    );
}

#[test]
fn test_persistent_worker_key_unicode() {
    let platform = Platform {
        properties: vec![
            Property {
                name: "persistentWorkerKey".to_string(),
                value: "worker-日本語-🎯".to_string(),
            },
            Property {
                name: "persistentWorkerTool".to_string(),
                value: "компилятор".to_string(),
            },
            Property {
                name: "locale".to_string(),
                value: "中文".to_string(),
            },
        ],
    };

    let key = PersistentWorkerKey::from_platform(&platform).unwrap();
    assert_eq!(key.tool, "компилятор");
    assert_eq!(key.key, "worker-日本語-🎯");
    assert_eq!(
        key.platform_properties.get("locale"),
        Some(&"中文".to_string())
    );
}
