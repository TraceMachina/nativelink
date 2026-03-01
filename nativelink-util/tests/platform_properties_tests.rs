use std::collections::HashMap;

use nativelink_macro::nativelink_test;
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};

#[test]
fn ignore_property_value_match_all() {
    let ignore_property = PlatformPropertyValue::Ignore("foo".to_string());
    let other_property = PlatformPropertyValue::Exact("bar".to_string());
    assert!(ignore_property.is_satisfied_by(&ignore_property));
    assert!(ignore_property.is_satisfied_by(&other_property));
}

#[test]
fn ignore_property_match_all() {
    let ignore_property = PlatformPropertyValue::Ignore("foo".to_string());
    let mut ignore_property_map = HashMap::new();
    ignore_property_map.insert("foo".into(), ignore_property);
    let ignore_properties = PlatformProperties::new(ignore_property_map);

    assert!(ignore_properties.is_satisfied_by(&PlatformProperties::new(HashMap::new()), true));
}

#[nativelink_test]
fn minimum_property_logs_error() {
    let minimum_property = PlatformPropertyValue::Minimum(1);
    let mut minimum_property_map = HashMap::new();
    minimum_property_map.insert("foo".into(), minimum_property);
    let minimum_properties = PlatformProperties::new(minimum_property_map);

    let worker_minimum_property = PlatformPropertyValue::Minimum(0);
    let mut worker_minimum_property_map = HashMap::new();
    worker_minimum_property_map.insert("foo".into(), worker_minimum_property);
    let worker_minimum_properties = PlatformProperties::new(worker_minimum_property_map);

    assert!(!minimum_properties.is_satisfied_by(&worker_minimum_properties, true));

    assert!(logs_contain(
        "Property mismatch on worker property foo. Minimum(0) < Minimum(1)"
    ));
}
