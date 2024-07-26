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

use opentelemetry::metrics::Meter;
use tracing::info;

use crate::metrics_collection::{
    CollectedMetricChildren, CollectedMetricPrimitive, CollectedMetricPrimitiveValue,
    CollectedMetrics, RootMetricCollectedMetrics,
};

/// The maximum length of a metric name that otel supports.
/// Going beyond this limit causes otel to complain.
const MAX_METRIC_NAME_LENGTH: usize = 256;

/// Export the collected metrics to the OpenTelemetry meter.
pub fn otel_export(
    mut root_prefix: String,
    meter: &Meter,
    root_collected_metrics: &RootMetricCollectedMetrics,
) {
    if !root_prefix.is_empty() {
        root_prefix.push('_');
    }
    process_children(&mut root_prefix, meter, root_collected_metrics);
}

fn process_children(prefix: &mut String, meter: &Meter, children: &CollectedMetricChildren) {
    for (name, child) in children {
        prefix.push_str(name);
        let mut added_prefix_len = name.len();
        match child {
            CollectedMetrics::Primitive(primitive) => {
                process_primitive(prefix, meter, primitive);
            }
            CollectedMetrics::Component(component) => {
                prefix.push('_');
                added_prefix_len += 1;
                process_children(prefix, meter, component);
            }
        }
        prefix.truncate(prefix.len() - added_prefix_len);
    }
}

fn process_primitive(prefix: &mut String, meter: &Meter, primitive: &CollectedMetricPrimitive) {
    match &primitive.value {
        Some(CollectedMetricPrimitiveValue::Counter(value)) => {
            if prefix.len() > MAX_METRIC_NAME_LENGTH {
                info!("Metric name longer than 256 characters: {}", prefix);
                return;
            }
            let counter = meter
                .u64_counter(prefix.clone())
                .with_description(primitive.help.clone())
                .init();
            counter.add(*value, &[]);
        }
        Some(CollectedMetricPrimitiveValue::String(_value)) => {
            // We don't publish strings in metrics.
        }
        None => {}
    }
}
