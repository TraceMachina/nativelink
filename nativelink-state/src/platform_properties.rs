use std::collections::HashMap;
use nativelink_proto::build::bazel::remote::execution::v2::Platform;
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};
use serde::{Deserialize, Serialize};
// use serde::ser::{SerializeMap, Serializer};

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub enum PropertyType {
    Exact,
    Minimum,
    Priority,
    Unknown,
}

#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
struct RedisPlatformPropertyValue {
    value: String,
    property_type: PropertyType
}
impl From<RedisPlatformPropertyValue> for PlatformPropertyValue {
    fn from(prop: RedisPlatformPropertyValue) -> Self {
        match prop.property_type {
            PropertyType::Unknown => Self::Unknown(prop.value),
            PropertyType::Exact => Self::Exact(prop.value),
            PropertyType::Priority=> Self::Priority(prop.value),
            PropertyType::Minimum => Self::Minimum(prop.value.parse::<u64>().unwrap()),
        }
    }
}

impl From<PlatformPropertyValue> for RedisPlatformPropertyValue {
    fn from(prop: PlatformPropertyValue) -> Self {
        match prop {
            PlatformPropertyValue::Unknown(v) => Self {
                value: v, property_type: PropertyType::Unknown
            },
            PlatformPropertyValue::Exact(v) => Self {
                value: v, property_type: PropertyType::Exact
            },
            PlatformPropertyValue::Priority(v) => Self {
                value: v, property_type: PropertyType::Priority
            },
            PlatformPropertyValue::Minimum(v) => Self {
                value: v.to_string(), property_type: PropertyType::Minimum
            },
        }
    }
}
#[derive(Serialize, Deserialize, Eq, PartialEq, Clone, Debug)]
pub struct RedisPlatformProperties {
    pub properties: HashMap<String, RedisPlatformPropertyValue>,
}

impl From<PlatformProperties> for RedisPlatformProperties {
    fn from(value: PlatformProperties) -> Self {
        let properties: HashMap<String, RedisPlatformPropertyValue> = HashMap::from_iter(
            value.properties.iter().map(|(k,v)| {
                (k.to_owned(), RedisPlatformPropertyValue::from(v.to_owned()))
            }));
        Self {
            properties
        }
    }
}

impl From<RedisPlatformProperties> for PlatformProperties {
    fn from(value: RedisPlatformProperties) -> Self {
        let properties: HashMap<String, PlatformPropertyValue> = HashMap::from_iter(
            value.properties.iter().map(|(k,v)| {
                (k.to_owned(), PlatformPropertyValue::from(v.to_owned()))
            }));
        Self {
            properties
        }
    }
}


impl From<Platform> for PlatformProperties {
    fn from(value: RedisPlatformProperties) -> Self {
        let properties: HashMap<String, PlatformPropertyValue> = HashMap::from_iter(
            value.properties.iter().map(|(k,v)| {
                (k.to_owned(), PlatformPropertyValue::from(v.to_owned()))
            }));
        Self {
            properties
        }
    }
}
