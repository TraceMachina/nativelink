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

#[test]
fn unknown_property_matches_worker_with_same_value() {
    let unknown_property = PlatformPropertyValue::Unknown("foo".to_string());
    assert!(unknown_property.is_satisfied_by(&PlatformPropertyValue::Unknown("foo".to_string())));
    assert!(unknown_property.is_satisfied_by(&PlatformPropertyValue::Exact("foo".to_string())));
    assert!(!unknown_property.is_satisfied_by(&PlatformPropertyValue::Unknown("bar".to_string())));
    assert!(!unknown_property.is_satisfied_by(&PlatformPropertyValue::Exact("bar".to_string())));
}

#[test]
fn unknown_property_does_not_restrict_worker_without_key() {
    let mut property_map = HashMap::new();
    property_map.insert(
        "foo".into(),
        PlatformPropertyValue::Unknown("bar".to_string()),
    );
    let unknown_properties = PlatformProperties::new(property_map);

    // A worker that does not declare the key is not restricted by it.
    assert!(unknown_properties.is_satisfied_by(&PlatformProperties::new(HashMap::new()), true));

    // A worker that declares the key must match the value.
    let mut mismatched_map = HashMap::new();
    mismatched_map.insert(
        "foo".into(),
        PlatformPropertyValue::Unknown("baz".to_string()),
    );
    assert!(!unknown_properties.is_satisfied_by(&PlatformProperties::new(mismatched_map), true));

    let mut matched_map = HashMap::new();
    matched_map.insert(
        "foo".into(),
        PlatformPropertyValue::Unknown("bar".to_string()),
    );
    assert!(unknown_properties.is_satisfied_by(&PlatformProperties::new(matched_map), true));
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
