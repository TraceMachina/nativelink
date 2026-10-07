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

//! Tests for the platform property manager.

use std::collections::HashMap;

use nativelink_config::schedulers::PropertyType;
use nativelink_scheduler::platform_property_manager::PlatformPropertyManager;
use nativelink_util::platform_properties::PlatformPropertyValue;

fn make_manager(props: &[(&str, PropertyType)]) -> PlatformPropertyManager {
    PlatformPropertyManager::new(
        props
            .iter()
            .map(|(name, prop_type)| ((*name).to_string(), *prop_type))
            .collect(),
    )
}

#[test]
fn known_properties_are_typed_from_config() {
    let manager = make_manager(&[
        ("cpu_count", PropertyType::Minimum),
        ("cpu_arch", PropertyType::Exact),
        ("priority", PropertyType::Priority),
        ("ignored", PropertyType::Ignore),
    ]);

    assert_eq!(
        manager.make_prop_value("cpu_count", "8").unwrap(),
        PlatformPropertyValue::Minimum(8)
    );
    assert_eq!(
        manager.make_prop_value("cpu_arch", "aarch64").unwrap(),
        PlatformPropertyValue::Exact("aarch64".to_string())
    );
    assert_eq!(
        manager.make_prop_value("priority", "high").unwrap(),
        PlatformPropertyValue::Priority("high".to_string())
    );
    assert_eq!(
        manager.make_prop_value("ignored", "foo").unwrap(),
        PlatformPropertyValue::Ignore("foo".to_string())
    );
}

#[test]
fn minimum_property_requires_u64_value() {
    let manager = make_manager(&[("cpu_count", PropertyType::Minimum)]);
    assert!(
        manager
            .make_prop_value("cpu_count", "not-a-number")
            .is_err()
    );
}

#[test]
fn undeclared_property_becomes_unknown() {
    let manager = make_manager(&[("cpu_arch", PropertyType::Exact)]);

    assert_eq!(
        manager
            .make_prop_value("InputRootAbsolutePath", "/some/path")
            .unwrap(),
        PlatformPropertyValue::Unknown("/some/path".to_string())
    );
}

#[test]
fn undeclared_property_does_not_fail_platform_properties() {
    let manager = make_manager(&[("cpu_count", PropertyType::Minimum)]);

    let mut request = HashMap::new();
    request.insert("cpu_count".to_string(), "4".to_string());
    request.insert("gpu_model".to_string(), "a100".to_string());

    let platform_properties = manager.make_platform_properties(request).unwrap();
    assert_eq!(
        platform_properties.properties.get("cpu_count").unwrap(),
        &PlatformPropertyValue::Minimum(4)
    );
    assert_eq!(
        platform_properties.properties.get("gpu_model").unwrap(),
        &PlatformPropertyValue::Unknown("a100".to_string())
    );
}

#[test]
fn empty_config_accepts_any_property() {
    let manager = make_manager(&[]);

    let mut request = HashMap::new();
    request.insert("OSFamily".to_string(), "linux".to_string());
    request.insert("container-image".to_string(), "docker://foo".to_string());

    let platform_properties = manager.make_platform_properties(request).unwrap();
    assert_eq!(platform_properties.properties.len(), 2);
    assert_eq!(
        platform_properties.properties.get("OSFamily").unwrap(),
        &PlatformPropertyValue::Unknown("linux".to_string())
    );
}
