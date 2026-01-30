use std::collections::HashMap;

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
