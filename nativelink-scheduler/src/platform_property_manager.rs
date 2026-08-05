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

use std::collections::HashMap;

use nativelink_config::schedulers::PropertyType;
use nativelink_error::{Code, Error, ResultExt, make_input_err};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent, group,
};
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};

/// Helps manage known properties and conversion into `PlatformPropertyValue`.
#[derive(Debug)]
pub struct PlatformPropertyManager {
    known_properties: HashMap<String, PropertyType>,
}

// TODO(palfrey) We cannot use the `MetricsComponent` trait here because
// the `PropertyType` lives in the `nativelink-config` crate which is not
// a dependency of the `nativelink-metric-collector` crate.
impl MetricsComponent for PlatformPropertyManager {
    fn publish(
        &self,
        _kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        let _enter = group!("known_properties").entered();
        for (k, v) in &self.known_properties {
            group!(k).in_scope(|| {
                format!("{v:?}").publish(MetricKind::String, field_metadata.clone())
            })?;
        }
        Ok(MetricPublishKnownKindData::Component)
    }
}

impl PlatformPropertyManager {
    #[must_use]
    pub const fn new(known_properties: HashMap<String, PropertyType>) -> Self {
        Self { known_properties }
    }

    /// Returns the `known_properties` map.
    #[must_use]
    pub const fn get_known_properties(&self) -> &HashMap<String, PropertyType> {
        &self.known_properties
    }

    /// Given a map of key-value pairs, returns a map of `PlatformPropertyValue` based on the
    /// configuration passed into the `PlatformPropertyManager` constructor.
    pub fn make_platform_properties(
        &self,
        properties: HashMap<String, String>,
    ) -> Result<PlatformProperties, Error> {
        let mut platform_properties = HashMap::with_capacity(properties.len());
        for (key, value) in properties {
            let prop_value = self.make_prop_value(&key, &value)?;
            platform_properties.insert(key, prop_value);
        }
        Ok(PlatformProperties::new(platform_properties))
    }

    /// Given a specific key and value, returns the translated `PlatformPropertyValue`. This will
    /// automatically convert any strings to the type-value pairs of `PlatformPropertyValue` based
    /// on the configuration passed into the `PlatformPropertyManager` constructor.
    pub fn make_prop_value(&self, key: &str, value: &str) -> Result<PlatformPropertyValue, Error> {
        if let Some(prop_type) = self.known_properties.get(key) {
            return match prop_type {
                PropertyType::Minimum => Ok(PlatformPropertyValue::Minimum(
                    value.parse::<u64>().err_tip_with_code(|e| {
                        (
                            Code::InvalidArgument,
                            format!("Cannot convert to platform property to u64: {value} - {e}"),
                        )
                    })?,
                )),
                PropertyType::Exact => Ok(PlatformPropertyValue::Exact(value.to_string())),
                PropertyType::Priority => Ok(PlatformPropertyValue::Priority(value.to_string())),
                PropertyType::Ignore => Ok(PlatformPropertyValue::Ignore(value.to_string())),
            };
        }
        Err(make_input_err!("Unknown platform property '{}'", key))
    }
}
