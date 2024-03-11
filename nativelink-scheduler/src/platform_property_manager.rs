// Copyright 2023 The NativeLink Authors. All rights reserved.
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

use std::collections::HashMap;

use nativelink_config::schedulers::PropertyType;
use nativelink_error::{make_input_err, Code, Error, ResultExt};
use nativelink_util::platform_properties::PlatformPropertyValue;

/// Helps manage known properties and conversion into `PlatformPropertyValue`.
pub struct PlatformPropertyManager {
    known_properties: HashMap<String, PropertyType>,
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

    /// Given a specific key and value, returns the translated `PlatformPropertyValue`. This will
    /// automatically convert any strings to the type-value pairs of `PlatformPropertyValue` based
    /// on the configuration passed into the `PlatformPropertyManager` constructor.
    pub fn make_prop_value(&self, key: &str, value: &str) -> Result<PlatformPropertyValue, Error> {
        if let Some(prop_type) = self.known_properties.get(key) {
            return match prop_type {
                PropertyType::minimum => Ok(PlatformPropertyValue::Minimum(
                    value.parse::<u64>().err_tip_with_code(|e| {
                        (
                            Code::InvalidArgument,
                            format!("Cannot convert to platform property to u64: {value} - {e}"),
                        )
                    })?,
                )),
                PropertyType::exact => Ok(PlatformPropertyValue::Exact(value.to_string())),
                PropertyType::priority => Ok(PlatformPropertyValue::Priority(value.to_string())),
            };
        }
        Err(make_input_err!("Unknown platform property '{}'", key))
    }
}
