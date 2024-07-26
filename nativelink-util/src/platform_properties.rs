// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use std::borrow::Cow;
use std::collections::HashMap;

use nativelink_metric::{
    publish, MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
};
use nativelink_proto::build::bazel::remote::execution::v2::Platform as ProtoPlatform;
use serde::{Deserialize, Serialize};

/// `PlatformProperties` helps manage the configuration of platform properties to
/// keys and types. The scheduler uses these properties to decide what jobs
/// can be assigned to different workers. For example, if a job states it needs
/// a specific key, it will never be run on a worker that does not have at least
/// all the platform property keys configured on the worker.
///
/// Additional rules may be applied based on `PlatfromPropertyValue`.
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
    pub fn is_satisfied_by(&self, worker_properties: &Self) -> bool {
        for (property, check_value) in &self.properties {
            if let Some(worker_value) = worker_properties.properties.get(property) {
                if !check_value.is_satisfied_by(worker_value) {
                    return false;
                }
            } else {
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

/// Holds the associated value of the key and type.
///
/// Exact    - Means the worker must have this exact value.
/// Minimum  - Means that workers must have at least this number available. When
///            a worker executes a task that has this value, the worker will have
///            this value subtracted from the available resources of the worker.
/// Priority - Means the worker is given this information, but does not restrict
///            what workers can take this value. However, the worker must have the
///            associated key present to be matched.
///            TODO(allada) In the future this will be used by the scheduler and
///            worker to cause the scheduler to prefer certain workers over others,
///            but not restrict them based on these values.
#[derive(Eq, PartialEq, Hash, Clone, Ord, PartialOrd, Debug, Serialize, Deserialize)]
pub enum PlatformPropertyValue {
    Exact(String),
    Minimum(u64),
    Priority(String),
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
            Self::Priority(_) => true,
            // Success exact case is handled above.
            Self::Exact(_) | Self::Unknown(_) => false,
        }
    }

    pub fn as_str(&self) -> Cow<str> {
        match self {
            Self::Exact(value) => Cow::Borrowed(value),
            Self::Minimum(value) => Cow::Owned(value.to_string()),
            Self::Priority(value) => Cow::Borrowed(value),
            Self::Unknown(value) => Cow::Borrowed(value),
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
            Self::Unknown(v) => publish!(name, v, kind, help, "unknown"),
        }

        Ok(MetricPublishKnownKindData::Component)
    }
}
