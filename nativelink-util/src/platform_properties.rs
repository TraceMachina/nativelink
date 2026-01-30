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

use std::borrow::Cow;
use std::collections::HashMap;

use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent, publish,
};
use nativelink_proto::build::bazel::remote::execution::v2::Platform as ProtoPlatform;
use nativelink_proto::build::bazel::remote::execution::v2::platform::Property as ProtoProperty;
use serde::{Deserialize, Serialize};
use tracing::info;

/// `PlatformProperties` helps manage the configuration of platform properties to
/// keys and types. The scheduler uses these properties to decide what jobs
/// can be assigned to different workers. For example, if a job states it needs
/// a specific key, it will never be run on a worker that does not have at least
/// all the platform property keys configured on the worker.
///
/// Additional rules may be applied based on `PlatformPropertyValue`.
#[derive(Eq, PartialEq, Clone, Debug, Default, Serialize, Deserialize, MetricsComponent)]
pub struct PlatformProperties {
    #[metric]
    pub properties: HashMap<String, PlatformPropertyValue>,
}

impl PlatformProperties {
    #[must_use]
    pub const fn new(map: HashMap<String, PlatformPropertyValue>) -> Self {
        Self { properties: map }
    }

    /// Determines if the worker's `PlatformProperties` is satisfied by this struct.
    #[must_use]
    pub fn is_satisfied_by(&self, worker_properties: &Self, full_worker_logging: bool) -> bool {
        for (property, check_value) in &self.properties {
            if let PlatformPropertyValue::Ignore(_) = check_value {
                continue; // always matches
            }
            if let Some(worker_value) = worker_properties.properties.get(property) {
                if !check_value.is_satisfied_by(worker_value) {
                    if full_worker_logging {
                        info!(
                            "Property mismatch on worker property {property}. {worker_value:?} != {check_value:?}"
                        );
                    }
                    return false;
                }
            } else {
                if full_worker_logging {
                    info!("Property missing on worker property {property}");
                }
                return false;
            }
        }
        true
    }
}

impl From<ProtoPlatform> for PlatformProperties {
    fn from(platform: ProtoPlatform) -> Self {
        let mut properties = HashMap::with_capacity(platform.properties.len());
        for property in platform.properties {
            properties.insert(
                property.name,
                PlatformPropertyValue::Unknown(property.value),
            );
        }
        Self { properties }
    }
}

impl From<&PlatformProperties> for ProtoPlatform {
    fn from(val: &PlatformProperties) -> Self {
        Self {
            properties: val
                .properties
                .iter()
                .map(|(name, value)| ProtoProperty {
                    name: name.clone(),
                    value: value.as_str().to_string(),
                })
                .collect(),
        }
    }
}

/// Holds the associated value of the key and type.
///
/// Exact    - Means the worker must have this exact value.
/// Minimum  - Means that workers must have at least this number available. When
///            a worker executes a task that has this value, the worker will have
///            this value subtracted from the available resources of the worker.
/// Priority - Means the worker is given this information, but does not restrict
///            what workers can take this value. However, the worker must have the
///            associated key present to be matched.
///            TODO(palfrey) In the future this will be used by the scheduler and
///            worker to cause the scheduler to prefer certain workers over others,
///            but not restrict them based on these values.
/// Ignore   - Jobs can request this key, but workers do not have to have it. This allows
///            for example the `InputRootAbsolutePath` case for chromium builds, where we can safely
///            ignore it without having to change the worker configs.
#[derive(Eq, PartialEq, Hash, Clone, Ord, PartialOrd, Debug, Serialize, Deserialize)]
pub enum PlatformPropertyValue {
    Exact(String),
    Minimum(u64),
    Priority(String),
    Ignore(String),
    Unknown(String),
}

impl PlatformPropertyValue {
    /// Same as `PlatformProperties::is_satisfied_by`, but on an individual value.
    #[must_use]
    pub fn is_satisfied_by(&self, worker_value: &Self) -> bool {
        if self == worker_value {
            return true;
        }
        match self {
            Self::Minimum(v) => {
                if let Self::Minimum(worker_v) = worker_value {
                    return worker_v >= v;
                }
                false
            }
            // Priority is used to pass info to the worker and not restrict which
            // workers can be selected, but might be used to prefer certain workers
            // over others.
            Self::Priority(_) | Self::Ignore(_) => true,
            // Success exact case is handled above.
            Self::Exact(_) | Self::Unknown(_) => false,
        }
    }

    pub fn as_str(&self) -> Cow<'_, str> {
        match self {
            Self::Exact(value)
            | Self::Priority(value)
            | Self::Unknown(value)
            | Self::Ignore(value) => Cow::Borrowed(value),
            Self::Minimum(value) => Cow::Owned(value.to_string()),
        }
    }
}

impl MetricsComponent for PlatformPropertyValue {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        let name = field_metadata.name.into_owned();
        let help = field_metadata.help.as_ref();
        match self {
            Self::Exact(v) => publish!(name, v, kind, help, "exact"),
            Self::Minimum(v) => publish!(name, v, kind, help, "minimum"),
            Self::Priority(v) => publish!(name, v, kind, help, "priority"),
            Self::Ignore(v) => publish!(name, v, kind, help, "ignore"),
            Self::Unknown(v) => publish!(name, v, kind, help, "unknown"),
        }

        Ok(MetricPublishKnownKindData::Component)
    }
}
