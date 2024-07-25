use std::{borrow::Cow, fmt::Debug};

use nativelink_metric::MetricKind;
use serde::Serialize;
use tracing::field::{Field, Visit};

use crate::metrics_collection::{CollectedMetricPrimitive, CollectedMetricPrimitiveValue};

#[derive(Default, Debug, Serialize)]
pub enum CollectionKind {
    #[default]
    Counter = 0,
    String = 1,
}

impl From<MetricKind> for CollectionKind {
    fn from(kind: MetricKind) -> Self {
        match kind {
            MetricKind::Counter => CollectionKind::Counter,
            MetricKind::String => CollectionKind::String,
            _ => CollectionKind::String,
        }
    }
}

#[derive(Debug)]
enum ValueWithPrimitiveType {
    String(String),
    U64(u64),
}

impl Default for ValueWithPrimitiveType {
    fn default() -> Self {
        ValueWithPrimitiveType::U64(0)
    }
}

#[derive(Default, Debug)]
pub struct MetricDataVisitor {
    pub name: String,
    value: ValueWithPrimitiveType,
    help: String,
    value_type: Option<CollectionKind>,
}

impl From<MetricDataVisitor> for CollectedMetricPrimitive {
    fn from(visitor: MetricDataVisitor) -> Self {
        let (value, derived_type) = match visitor.value {
            ValueWithPrimitiveType::String(s) => (
                CollectedMetricPrimitiveValue::String(Cow::Owned(s)),
                CollectionKind::String,
            ),
            ValueWithPrimitiveType::U64(u) => (
                CollectedMetricPrimitiveValue::Counter(u),
                CollectionKind::Counter,
            ),
        };
        CollectedMetricPrimitive {
            value: Some(value),
            help: visitor.help,
            value_type: visitor.value_type.unwrap_or(derived_type),
        }
    }
}

impl Visit for MetricDataVisitor {
    // Required method
    fn record_debug(&mut self, _field: &Field, _value: &dyn Debug) {}

    // Provided methods
    fn record_f64(&mut self, field: &Field, value: f64) {
        if field.name() == "__value" {
            self.value = ValueWithPrimitiveType::String(value.to_string())
        }
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        if field.name() == "__value" {
            match u64::try_from(value) {
                Ok(v) => self.value = ValueWithPrimitiveType::U64(v),
                Err(_) => self.value = ValueWithPrimitiveType::String(value.to_string()),
            }
        }
    }
    fn record_u64(&mut self, field: &Field, value: u64) {
        match field.name() {
            "__value" => self.value = ValueWithPrimitiveType::U64(value),
            "__type" => self.value_type = Some(MetricKind::from(value).into()),
            "__help" => self.help = value.to_string(),
            "__name" => self.name = value.to_string(),
            field => panic!("UNKNOWN FIELD {field}"),
        }
    }
    fn record_i128(&mut self, field: &Field, value: i128) {
        if field.name() == "__value" {
            match u64::try_from(value) {
                Ok(v) => self.value = ValueWithPrimitiveType::U64(v),
                Err(_) => self.value = ValueWithPrimitiveType::String(value.to_string()),
            }
        }
    }
    fn record_u128(&mut self, field: &Field, value: u128) {
        if field.name() == "__value" {
            match u64::try_from(value) {
                Ok(v) => self.value = ValueWithPrimitiveType::U64(v),
                Err(_) => self.value = ValueWithPrimitiveType::String(value.to_string()),
            }
        }
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        if field.name() == "__value" {
            self.value = ValueWithPrimitiveType::U64(u64::from(value));
        }
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        match field.name() {
            "__value" => self.value = ValueWithPrimitiveType::String(value.to_string()),
            "__help" => self.help = value.to_string(),
            "__name" => self.name = value.to_string(),
            field => panic!("UNKNOWN FIELD {field}"),
        }
    }
    fn record_error(&mut self, _field: &Field, _value: &(dyn std::error::Error + 'static)) {}
}

pub struct SpanFields {
    pub name: Cow<'static, str>,
}

impl Visit for SpanFields {
    // Required method
    fn record_debug(&mut self, _field: &Field, _value: &dyn Debug) {}

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "__name" {
            self.name = Cow::Owned(value.to_string());
        }
    }
}
