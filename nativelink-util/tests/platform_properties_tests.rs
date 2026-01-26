use nativelink_util::platform_properties::PlatformPropertyValue;

#[test]
fn ignore_properties_match_all() {
    let ignore_property = PlatformPropertyValue::Ignore("foo".to_string());
    let other_property = PlatformPropertyValue::Exact("bar".to_string());
    assert!(ignore_property.is_satisfied_by(&ignore_property));
    assert!(ignore_property.is_satisfied_by(&other_property));
}
