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

use std::collections::HashMap;
use std::fmt::Debug;
use std::io::{BufRead, Cursor};
use std::marker::PhantomData;
use std::str::from_utf8;

use nativelink_error::Error;
use nativelink_metric::{MetricFieldData, MetricKind, MetricsComponent};
use nativelink_metric_collector::{otel_export, MetricsCollectorLayer};
use opentelemetry::metrics::MeterProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use prometheus::{Encoder, TextEncoder};
use serde_json::Value;
use tracing_subscriber::layer::SubscriberExt;

#[derive(MetricsComponent)]
pub struct MultiStruct {
    #[metric(help = "dummy help pub_u64")]
    pub pub_u64: u64,

    #[metric(help = "Dummy help str")]
    str: String,

    _no_metric_str: String,
    _no_metric_u64: u64,

    #[metric(group = "foo")]
    sub_struct_group: Foo<'static, String>,

    #[metric]
    sub_struct: Foo<'static, String>,
}

#[derive(MetricsComponent)]
struct Foo<'a, T: Debug + Send + Sync> {
    #[metric(help = "help str1", handler = ToString::to_string)]
    custom_handler_num_str: u64,

    #[metric(help = "help str2", handler = ToString::to_string, kind = "counter")]
    custom_handler_num_counter: u64,

    _bar: &'a PhantomData<T>,
}

// Note: Special case to not use nativelink-test macro. We want this test
// to be very lightweight and not depend on other crates.
#[test]
fn test_metric_collector() -> Result<(), Error> {
    let multi_struct = MultiStruct {
        pub_u64: 1,
        str: "str_data".to_string(),
        _no_metric_str: "no_metric_str".to_string(),
        _no_metric_u64: 2,
        sub_struct_group: Foo {
            custom_handler_num_str: 3,
            custom_handler_num_counter: 4,
            _bar: &PhantomData,
        },
        sub_struct: Foo {
            custom_handler_num_str: 5,
            custom_handler_num_counter: 6,
            _bar: &PhantomData,
        },
    };
    let (layer, output_metrics) = MetricsCollectorLayer::new();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        MetricsComponent::publish(
            &multi_struct,
            MetricKind::Component,
            MetricFieldData::default(),
        )
        .unwrap();
    });

    let output_json_data = serde_json::to_string(&*output_metrics.lock()).unwrap();
    let final_output_metrics: HashMap<String, Value> =
        serde_json::from_str(&output_json_data).unwrap();
    let expected_json_data = r#"{"custom_handler_num_str":"5","str":"str_data","foo":{"custom_handler_num_counter":4,"custom_handler_num_str":"3"},"pub_u64":1,"custom_handler_num_counter":6}"#;
    let expected_value: HashMap<String, Value> = serde_json::from_str(expected_json_data).unwrap();

    // We cannot compare the strings directly as the order
    // of the keys in the JSON string can be different.
    // instead we go to string then back to anonymous hashmaps
    // then validate the values.
    assert_eq!(final_output_metrics, expected_value);
    // To ensure the round trip is correct, we compare the length of the
    // output JSON string and the expected JSON string.
    assert_eq!(output_json_data.len(), expected_json_data.len());
    // Ensure the double round trip is also correct and not an
    // encoding issue.
    assert_eq!(
        serde_json::to_string(&final_output_metrics).unwrap().len(),
        expected_json_data.len()
    );

    Ok(())
}

// Note: Special case to not use nativelink-test macro. We want this test
// to be very lightweight and not depend on other crates.
#[test]
fn test_prometheus_exporter() -> Result<(), Error> {
    let multi_struct = MultiStruct {
        pub_u64: 1,
        str: "str_data".to_string(),
        _no_metric_str: "no_metric_str".to_string(),
        _no_metric_u64: 2,
        sub_struct_group: Foo {
            custom_handler_num_str: 3,
            custom_handler_num_counter: 4,
            _bar: &PhantomData,
        },
        sub_struct: Foo {
            custom_handler_num_str: 5,
            custom_handler_num_counter: 6,
            _bar: &PhantomData,
        },
    };
    let (layer, output_metrics) = MetricsCollectorLayer::new();
    let subscriber = tracing_subscriber::registry().with(layer);

    tracing::subscriber::with_default(subscriber, || {
        MetricsComponent::publish(
            &multi_struct,
            MetricKind::Component,
            MetricFieldData::default(),
        )
        .unwrap();
    });

    let registry = prometheus::Registry::new();
    let exporter = opentelemetry_prometheus::exporter()
        .with_registry(registry.clone())
        .without_counter_suffixes()
        .without_scope_info()
        .build()
        .unwrap();

    // Prepare our OpenTelemetry collector/exporter.
    let provider = SdkMeterProvider::builder().with_reader(exporter).build();
    let meter = provider.meter("nativelink");

    // Export the metrics to OpenTelemetry.
    otel_export("nativelink".to_string(), &meter, &output_metrics.lock());

    // Translate the OpenTelemetry metrics to Prometheus format and encode
    // them into a hyper::Response.
    let mut result = vec![];
    TextEncoder::new()
        .encode(&registry.gather(), &mut result)
        .unwrap();

    let mut output: Vec<String> = Cursor::new(from_utf8(&result).unwrap())
        .lines()
        .map(|v| v.unwrap())
        .collect();
    let mut expected_output: Vec<String> = Cursor::new(r#"
# HELP nativelink_custom_handler_num_counter help str2
# HELP nativelink_foo_custom_handler_num_counter help str2
# HELP nativelink_pub_u64 dummy help pub_u64
# HELP target_info Target metadata
# TYPE nativelink_custom_handler_num_counter counter
# TYPE nativelink_foo_custom_handler_num_counter counter
# TYPE nativelink_pub_u64 counter
# TYPE target_info gauge
nativelink_custom_handler_num_counter 6
nativelink_foo_custom_handler_num_counter 4
nativelink_pub_u64 1
target_info{service_name="unknown_service",telemetry_sdk_language="rust",telemetry_sdk_name="opentelemetry",telemetry_sdk_version="0.23.0"} 1
"#.trim()).lines().map(|v| v.unwrap()).collect();

    // We need to sort because the output order is non-deterministic.
    output.sort();
    expected_output.sort();

    assert_eq!(output, expected_output);
    Ok(())
}
