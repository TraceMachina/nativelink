// use nativelink_error::Error;
// // use nativelink_macro::nativelink_test;
// use nativelink_metric::{MetricFieldData, MetricKind, MetricsComponent};
// use nativelink_metric_collector::MetricsCollectorLayer;
// use std::{fmt::Debug, marker::PhantomData};
// use tracing_subscriber::layer::SubscriberExt;

// #[derive(MetricsComponent)]
// pub struct MultiStruct {
//     #[metric(help = "Number of active drop spawns")]
//     pub pub_u64: u64,

//     #[metric(help = "Path to the configured temp path")]
//     str: String,

//     _no_metric_str: String,
//     _no_metric_u64: u64,

//     #[metric(group = "foo")]
//     sub_struct_group: Foo<'static, String>,

//     #[metric]
//     sub_struct: Foo<'static, String>,
// }

// #[derive(MetricsComponent)]
// struct Foo<'a, T: Debug + Send + Sync> {
//     #[metric(help = "help str", handler = ToString::to_string)]
//     custom_handler_num_str: u64,

//     #[metric(help = "help str", handler = ToString::to_string, kind = "counter")]
//     custom_handler_num_counter: u64,

//     _bar: &'a PhantomData<T>,
// }

// #[test]
// fn test_metric_collector() -> Result<(), Error> {
//     let multi_struct = MultiStruct {
//         pub_u64: 1,
//         str: "str_data".to_string(),
//         _no_metric_str: "no_metric_str".to_string(),
//         _no_metric_u64: 2,
//         sub_struct_group: Foo {
//             custom_handler_num_str: 3,
//             custom_handler_num_counter: 4,
//             _bar: &PhantomData,
//         },
//         sub_struct: Foo {
//             custom_handler_num_str: 5,
//             custom_handler_num_counter: 6,
//             _bar: &PhantomData,
//         },
//     };
//     let (layer, output_metrics) = MetricsCollectorLayer::new();
//     let subscriber = tracing_subscriber::registry().with(layer);

//     tracing::subscriber::with_default(subscriber, || {
//         MetricsComponent::publish(&multi_struct, MetricKind::Component, MetricFieldData::default()).unwrap();
//         let output_metrics = output_metrics.lock();

//         println!("{output_metrics:#?}");
//         println!("{}", serde_json::to_string_pretty(&*output_metrics).unwrap());
//     });

//     Ok(())
// }

// TODO(FINISH TEST)
