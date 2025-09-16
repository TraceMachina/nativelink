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

fn create_test_platform(properties: Vec<(&str, &str)>) -> Platform {
    Platform {
        properties: properties
            .into_iter()
            .map(|(name, value)| Property {
                name: name.to_string(),
                value: value.to_string(),
            })
            .collect(),
    }
}

#[test]
fn test_persistent_worker_key_extraction() {
    // Test successful extraction with all required fields
    let platform = create_test_platform(vec![
        ("persistentWorkerKey", "worker-123"),
        ("persistentWorkerTool", "javac"),
        ("os", "linux"),
        ("arch", "x86_64"),
    ]);

    let key = PersistentWorkerKey::from_platform(&platform).unwrap();
    assert_eq!(key.key, "worker-123");
    assert_eq!(key.tool, "javac");
    assert_eq!(
        key.platform_properties.get("os"),
        Some(&"linux".to_string())
    );
    assert_eq!(
        key.platform_properties.get("arch"),
        Some(&"x86_64".to_string())
    );

    // Test missing key field
    let platform = create_test_platform(vec![("persistentWorkerTool", "javac"), ("os", "linux")]);
    assert!(PersistentWorkerKey::from_platform(&platform).is_none());

    // Test missing tool field
    let platform =
        create_test_platform(vec![("persistentWorkerKey", "worker-123"), ("os", "linux")]);
    assert!(PersistentWorkerKey::from_platform(&platform).is_none());

    // Test empty platform
    let platform = create_test_platform(vec![]);
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
fn test_persistent_worker_key_with_different_tools() {
    // Test with TypeScript compiler
    let platform = create_test_platform(vec![
        ("persistentWorkerKey", "tsc-worker"),
        ("persistentWorkerTool", "tsc"),
        ("node_version", "18"),
    ]);

    let key = PersistentWorkerKey::from_platform(&platform).unwrap();
    assert_eq!(key.tool, "tsc");
    assert_eq!(key.key, "tsc-worker");
    assert_eq!(
        key.platform_properties.get("node_version"),
        Some(&"18".to_string())
    );

    // Test with Kotlin compiler
    let platform = create_test_platform(vec![
        ("persistentWorkerKey", "kotlinc-worker"),
        ("persistentWorkerTool", "kotlinc"),
        ("jvm_version", "17"),
    ]);

    let key = PersistentWorkerKey::from_platform(&platform).unwrap();
    assert_eq!(key.tool, "kotlinc");
    assert_eq!(key.key, "kotlinc-worker");
    assert_eq!(
        key.platform_properties.get("jvm_version"),
        Some(&"17".to_string())
    );
}

#[test]
fn test_persistent_worker_key_empty_values() {
    let platform = create_test_platform(vec![
        ("persistentWorkerKey", ""),
        ("persistentWorkerTool", ""),
    ]);

    // Empty values should still create a valid key
    let key = PersistentWorkerKey::from_platform(&platform).unwrap();
    assert_eq!(key.tool, "");
    assert_eq!(key.key, "");
    assert!(key.platform_properties.is_empty());
}

#[test]
fn test_persistent_worker_key_special_characters() {
    let platform = create_test_platform(vec![
        (
            "persistentWorkerKey",
            "worker-@#$%^&*()_+-=[]{}|;:',.<>?/~`",
        ),
        ("persistentWorkerTool", "tool/with/path"),
        ("path", "/usr/local/bin/tool"),
    ]);

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
    let platform = create_test_platform(vec![
        ("persistentWorkerKey", "worker-日本語-🎯"),
        ("persistentWorkerTool", "компилятор"),
        ("locale", "中文"),
    ]);

    let key = PersistentWorkerKey::from_platform(&platform).unwrap();
    assert_eq!(key.tool, "компилятор");
    assert_eq!(key.key, "worker-日本語-🎯");
    assert_eq!(
        key.platform_properties.get("locale"),
        Some(&"中文".to_string())
    );
}

#[test]
fn test_persistent_worker_key_case_sensitive() {
    // Test that the fields are case-sensitive
    let platform = create_test_platform(vec![
        ("PersistentWorkerKey", "wrong-case"),
        ("persistentWorkerTool", "tool"),
    ]);

    // Should not find the key with wrong case
    assert!(PersistentWorkerKey::from_platform(&platform).is_none());

    let platform = create_test_platform(vec![
        ("persistentWorkerKey", "correct-case"),
        ("PersistentWorkerTool", "wrong-case-tool"),
    ]);

    // Should not find the tool with wrong case
    assert!(PersistentWorkerKey::from_platform(&platform).is_none());
}

#[test]
fn test_persistent_worker_key_duplicate_properties() {
    let platform = create_test_platform(vec![
        ("persistentWorkerKey", "first-key"),
        ("persistentWorkerTool", "clang"),
        ("persistentWorkerKey", "second-key"),
        ("flag", "value1"),
        ("flag", "value2"),
    ]);

    let key = PersistentWorkerKey::from_platform(&platform).unwrap();
    assert_eq!(key.tool, "clang");
    // Last value should win for duplicates
    assert_eq!(key.key, "second-key");
    assert_eq!(
        key.platform_properties.get("flag"),
        Some(&"value2".to_string())
    );
}
