use std::{
    borrow::Cow,
    collections::HashMap,
    ops::{Deref, DerefMut},
};

use serde::Serialize;

use crate::metrics_visitors::CollectionKind;

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum CollectedMetricPrimitiveValue {
    Counter(u64),
    String(Cow<'static, str>),
}

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

pub type CollectedMetricChildren = HashMap<String, CollectedMetrics>;

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum CollectedMetrics {
    Primitive(CollectedMetricPrimitive),
    Component(Box<CollectedMetricChildren>),
}

impl CollectedMetrics {
    pub fn new_component() -> Self {
        Self::Component(Box::new(CollectedMetricChildren::default()))
    }
}

#[derive(Default, Debug, Serialize)]
pub struct RootMetricCollectedMetrics {
    #[serde(flatten)]
    inner: CollectedMetricChildren,
}

impl RootMetricCollectedMetrics {
    pub fn to_json5(&self) -> Result<std::string::String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
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
