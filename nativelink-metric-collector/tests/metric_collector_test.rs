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
use std::marker::PhantomData;

use nativelink_metric::{MetricFieldData, MetricKind, MetricsComponent};
use nativelink_metric_collector::MetricsCollectorLayer;
use serde_json::Value;
use tracing_subscriber::layer::SubscriberExt;

#[derive(Debug, MetricsComponent)]
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

#[derive(Debug, MetricsComponent)]
struct Foo<#[expect(single_use_lifetimes, reason = "false positive")] 'a, T: Debug + Send + Sync> {
    #[metric(help = "help str1", handler = ToString::to_string)]
    custom_handler_num_str: u64,

    #[metric(help = "help str2", handler = ToString::to_string, kind = "counter")]
    custom_handler_num_counter: u64,

    _bar: &'a PhantomData<T>,
}

// Note: Special case to not use nativelink-test macro. We want this test
// to be very lightweight and not depend on other crates.
#[test]
fn test_metric_collector() {
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
}
