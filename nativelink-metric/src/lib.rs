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
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::hash::BuildHasher;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub use nativelink_metric_macro_derive::MetricsComponent;
pub use tracing::{error as __metric_error, info as __metric_event, info_span as __metric_span};

/// Error type for the metrics library.
// Note: We do not use the nativelink-error struct because
// we'd end up in a circular dependency if we did, because
// nativelink-error uses the metrics library.
#[derive(Debug)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for Error {}

/// Holds metadata about the field that is being published.
#[derive(Default, Clone)]
pub struct MetricFieldData<'a> {
    pub name: Cow<'a, str>,
    pub help: Cow<'a, str>,
    pub group: Cow<'a, str>,
}

/// The final primtive data that is being published with the kind.
#[derive(Debug)]
pub enum MetricPublishKnownKindData {
    Counter(u64),
    String(String),
    Component,
}

/// The kind of metric that is being published.
// Note: This enum will be translate in-and-out
// of a u64 when traversing the `tracing::event`
// boundary for efficiency reasons.
#[derive(Clone, Copy, Debug)]
#[repr(u8)]
pub enum MetricKind {
    Default = 0,
    Counter = 1,
    String = 2,
    Component = 3,
}

impl From<u64> for MetricKind {
    fn from(value: u64) -> Self {
        match value {
            0 | 4_u64..=u64::MAX => MetricKind::Default,
            1 => MetricKind::Counter,
            2 => MetricKind::String,
            3 => MetricKind::Component,
        }
    }
}

impl MetricKind {
    pub fn into_known_kind(&self, default_kind: MetricKind) -> MetricPublishKnownKindData {
        let mut this = *self;
        if matches!(self, MetricKind::Default) {
            this = default_kind;
        }
        match this {
            MetricKind::Counter => MetricPublishKnownKindData::Counter(0),
            MetricKind::String => MetricPublishKnownKindData::String(String::new()),
            MetricKind::Component => MetricPublishKnownKindData::Component,
            MetricKind::Default => unreachable!("Default should have been handled"),
        }
    }
}

/// The trait that all components that can be published must implement.
pub trait MetricsComponent {
    fn publish(
        &self,
        kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error>;
}

pub trait RootMetricsComponent: MetricsComponent + Send + Sync {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        MetricsComponent::publish(self, kind, field_metadata)
    }
}

impl<T: MetricsComponent> MetricsComponent for Option<T> {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        match self {
            Some(value) => value.publish(kind, field_metadata),
            None => Ok(MetricPublishKnownKindData::Component),
        }
    }
}

impl<T: MetricsComponent> MetricsComponent for tokio::sync::watch::Sender<T> {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        self.borrow().publish(kind, field_metadata)
    }
}

impl<T: MetricsComponent + ?Sized> MetricsComponent for Arc<T> {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        self.as_ref().publish(kind, field_metadata)
    }
}

impl<T: MetricsComponent, S: BuildHasher> MetricsComponent for HashSet<T, S> {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        for (i, item) in self.iter().enumerate() {
            let guard = group!(i).entered();
            let publish_result = item.publish(kind, field_metadata.clone())?;
            drop(guard);
            match publish_result {
                MetricPublishKnownKindData::Counter(value) => {
                    publish!(
                        i,
                        &value,
                        MetricKind::Counter,
                        field_metadata.help.to_string()
                    );
                }
                MetricPublishKnownKindData::String(value) => {
                    publish!(
                        i,
                        &value,
                        MetricKind::String,
                        field_metadata.help.to_string()
                    );
                }
                MetricPublishKnownKindData::Component => {}
            }
        }
        Ok(MetricPublishKnownKindData::Component)
    }
}

impl<U: ToString, T: MetricsComponent, S: BuildHasher> MetricsComponent for HashMap<U, T, S> {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        for (key, item) in self {
            let guard = group!(key).entered();
            let publish_result = item.publish(kind, field_metadata.clone())?;
            drop(guard);
            match publish_result {
                MetricPublishKnownKindData::Counter(value) => {
                    publish!(
                        key,
                        &value,
                        MetricKind::Counter,
                        field_metadata.help.to_string()
                    );
                }
                MetricPublishKnownKindData::String(value) => {
                    publish!(
                        key,
                        &value,
                        MetricKind::String,
                        field_metadata.help.to_string()
                    );
                }
                MetricPublishKnownKindData::Component => {}
            }
        }
        Ok(MetricPublishKnownKindData::Component)
    }
}

impl<U: ToString, T: MetricsComponent> MetricsComponent for BTreeMap<U, T> {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        for (key, item) in self {
            group!(key).in_scope(|| item.publish(kind, field_metadata.clone()))?;
        }
        Ok(MetricPublishKnownKindData::Component)
    }
}

impl<T: MetricsComponent> MetricsComponent for BTreeSet<T> {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        for (i, item) in self.iter().enumerate() {
            group!(i).in_scope(|| item.publish(kind, field_metadata.clone()))?;
        }
        Ok(MetricPublishKnownKindData::Component)
    }
}

impl<T: MetricsComponent> MetricsComponent for Vec<T> {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        for (i, item) in self.iter().enumerate() {
            group!(i).in_scope(|| item.publish(kind, field_metadata.clone()))?;
        }
        Ok(MetricPublishKnownKindData::Component)
    }
}

impl<T: MetricsComponent + ?Sized> MetricsComponent for Weak<T> {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        let Some(this) = self.upgrade() else {
            return Ok(MetricPublishKnownKindData::Component);
        };
        this.as_ref().publish(kind, field_metadata)
    }
}

impl<T, E> MetricsComponent for Result<T, E>
where
    T: MetricsComponent,
    E: MetricsComponent,
{
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        match self {
            Ok(value) => value.publish(kind, field_metadata),
            Err(value) => value.publish(kind, field_metadata),
        }
    }
}

impl MetricsComponent for Duration {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        self.as_secs_f64().publish(kind, field_metadata)
    }
}

impl MetricsComponent for SystemTime {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(n) => n.as_secs().publish(kind, field_metadata),
            Err(_) => Err(Error("SystemTime before UNIX EPOCH!".to_string())),
        }
    }
}

impl MetricsComponent for f64 {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        Ok(MetricPublishKnownKindData::String(self.to_string()))
    }
}

impl MetricsComponent for bool {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        let value = u64::from(*self);
        value.publish(kind, field_metadata)
    }
}

impl MetricsComponent for i32 {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        let value = u64::try_from(*self)
            .map_err(|_| Error(format!("Could not convert {self} to u64 in metrics lib")))?;
        value.publish(kind, field_metadata)
    }
}

impl MetricsComponent for u64 {
    fn publish(
        &self,
        kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        let mut known_kind_data = kind.into_known_kind(MetricKind::Counter);
        match &mut known_kind_data {
            MetricPublishKnownKindData::Counter(data) => {
                *data = *self;
            }
            MetricPublishKnownKindData::String(data) => {
                *data = self.to_string();
            }
            MetricPublishKnownKindData::Component => {}
        }
        Ok(known_kind_data)
    }
}

impl MetricsComponent for i64 {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        let value = u64::try_from(*self)
            .map_err(|_| Error(format!("Could not convert {self} to u64 in metrics lib")))?;
        value.publish(kind, field_metadata)
    }
}

impl MetricsComponent for u32 {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        u64::from(*self).publish(kind, field_metadata)
    }
}

impl MetricsComponent for usize {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        let value = u64::try_from(*self)
            .map_err(|_| Error(format!("Could not convert {self} to u64 in metrics lib")))?;
        value.publish(kind, field_metadata)
    }
}

impl MetricsComponent for AtomicU64 {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        self.load(Ordering::Acquire).publish(kind, field_metadata)
    }
}

impl MetricsComponent for AtomicI64 {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        self.load(Ordering::Acquire).publish(kind, field_metadata)
    }
}

impl MetricsComponent for String {
    fn publish(
        &self,
        kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        let mut known_kind_data = kind.into_known_kind(MetricKind::String);
        match &mut known_kind_data {
            MetricPublishKnownKindData::Counter(data) => {
                *data = self.parse::<u64>().map_err(|_| {
                    Error(format!(
                        "Could not convert String '{self}' to u64 in metrics lib"
                    ))
                })?;
            }
            MetricPublishKnownKindData::String(data) => {
                data.clone_from(self);
            }
            MetricPublishKnownKindData::Component => {}
        }
        Ok(known_kind_data)
    }
}

impl<T: MetricsComponent> MetricsComponent for async_lock::Mutex<T> {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        // It is safe to block in the publishing thread.
        let lock = self.lock_blocking();
        lock.publish(kind, field_metadata)
    }
}

impl<T: MetricsComponent> MetricsComponent for parking_lot::Mutex<T> {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        // It is safe to block in the publishing thread.
        let lock = self.lock();
        lock.publish(kind, field_metadata)
    }
}

impl<T: MetricsComponent> MetricsComponent for parking_lot::RwLock<T> {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, Error> {
        // It is safe to block in the publishing thread.
        let lock = self.read();
        lock.publish(kind, field_metadata)
    }
}

#[macro_export]
macro_rules! group {
    ($name:expr) => {
        $crate::__metric_span!(target: "nativelink_metric", "", __name = $name.to_string())
    };
}

#[macro_export]
macro_rules! publish {
    ($name:expr, $value:expr, $metric_kind:expr, $help:expr) => {
        $crate::publish!($name, $value, $metric_kind, $help, "")
    };
    ($name:expr, $value:expr, $metric_kind:expr, $help:expr, $group:expr) => {
        {
            let _maybe_entered = if !$group.is_empty() {
                Some($crate::group!($group).entered())
            } else {
                None
            };
            let name = $name.to_string();
            let field_metadata = $crate::MetricFieldData {
                name: ::std::borrow::Cow::Borrowed(&name),
                help: $help.into(),
                group: $group.into(),
            };
            match $crate::MetricsComponent::publish($value, $metric_kind, field_metadata)? {
                $crate::MetricPublishKnownKindData::Counter(value) => {
                    $crate::__metric_event!(
                        target: "nativelink_metric",
                        __value = value,
                        __type = $crate::MetricKind::Counter as u8,
                        __help = $help.to_string(),
                        __name = name
                    );
                }
                $crate::MetricPublishKnownKindData::String(value) => {
                    $crate::__metric_event!(
                        target: "nativelink_metric",
                        __value = value,
                        __type = $crate::MetricKind::String as u8,
                        __help = $help.to_string(),
                        __name = name
                    );
                }
                $crate::MetricPublishKnownKindData::Component => {
                    // Do nothing, data already published.
                }
            }
        }
    };
}
