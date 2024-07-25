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
use std::ops::{Deref, DerefMut};

use serde::Serialize;

use crate::metrics_visitors::CollectionKind;

/// The final-metric primitive value that was collected with type.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum CollectedMetricPrimitiveValue {
    Counter(u64),
    String(Cow<'static, str>),
}

/// The final-metric primitive field that was collected.
#[derive(Default, Debug)]
pub struct CollectedMetricPrimitive {
    pub value: Option<CollectedMetricPrimitiveValue>,
    pub help: String,
    pub value_type: CollectionKind,
}

impl Serialize for CollectedMetricPrimitive {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match &self.value {
            Some(CollectedMetricPrimitiveValue::Counter(value)) => serializer.serialize_u64(*value),
            Some(CollectedMetricPrimitiveValue::String(value)) => serializer.serialize_str(value),
            None => serializer.serialize_none(),
        }
    }
}

/// Key-value represented output.
pub type CollectedMetricChildren = HashMap<String, CollectedMetrics>;

/// The type of the collected metric (eg: nested vs primitive).
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum CollectedMetrics {
    Primitive(CollectedMetricPrimitive),
    Component(Box<CollectedMetricChildren>),
}

impl CollectedMetrics {
    pub fn new_component() -> Self {
        Self::Component(Box::default())
    }
}

/// The root metric component that was collected.
#[derive(Default, Debug, Serialize)]
pub struct RootMetricCollectedMetrics {
    #[serde(flatten)]
    inner: CollectedMetricChildren,
}

impl Deref for RootMetricCollectedMetrics {
    type Target = CollectedMetricChildren;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for RootMetricCollectedMetrics {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}
