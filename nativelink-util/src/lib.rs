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

pub mod action_messages;
pub mod buf_channel;
pub mod channel_body_for_tests;
pub mod chunked_stream;
pub mod common;
pub mod connection_manager;
pub mod digest_hasher;
pub mod evicting_map;
pub mod fastcdc;
pub mod fs;
pub mod health_utils;
pub mod instant_wrapper;
pub mod known_platform_property_provider;
pub mod metrics_utils;
pub mod operation_state_manager;
pub mod origin_context;
pub mod origin_event;
pub mod origin_event_middleware;
pub mod origin_event_publisher;
pub mod platform_properties;
pub mod proto_stream_utils;
pub mod resource_info;
pub mod retry;
pub mod shutdown_guard;
pub mod store_trait;
pub mod task;
pub mod tls_utils;
pub mod write_counter;

// Re-export tracing mostly for use in macros.
pub use tracing as __tracing;

/// Initialize tracing.
pub fn init_tracing() -> Result<(), nativelink_error::Error> {
    use std::sync::OnceLock;

    use nativelink_error::{Code, ResultExt, make_err};
    use opentelemetry::trace::TracerProvider;
    use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
    use opentelemetry_otlp::{Protocol, WithExportConfig};
    use opentelemetry_sdk::Resource;
    use opentelemetry_sdk::logs::SdkLoggerProvider;
    use opentelemetry_sdk::metrics::SdkMeterProvider;
    use tracing_subscriber::prelude::*;

    static INITIALIZED: OnceLock<()> = OnceLock::new();

    // Filter non-nativelink information.
    // See: https://github.com/open-telemetry/opentelemetry-rust/issues/2877
    // Note that EnvFilter doesn't implement `clone`, so we create a new one for
    // each telemetry kind.
    fn otlp_filter() -> tracing_subscriber::EnvFilter {
        tracing_subscriber::EnvFilter::builder()
            .with_default_directive(tracing::metadata::LevelFilter::WARN.into())
            .from_env_lossy()
            .add_directive("hyper=off".parse().unwrap())
            .add_directive("tonic=off".parse().unwrap())
            .add_directive("h2=off".parse().unwrap())
            .add_directive("reqwest=off".parse().unwrap())
    }

    if INITIALIZED.get().is_some() {
        return Err(make_err!(Code::Internal, "Logging already initialized"));
    }

    let stdout_filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing::metadata::LevelFilter::WARN.into())
        .from_env_lossy();

    // Setup tracing logger for multiple format types, compact, json, and pretty
    // as a single layer. Configuration for log format comes from environment
    // variable NL_LOG_FMT due to subscribers being configured before config
    // parsing.
    let nl_log_fmt = std::env::var("NL_LOG").unwrap_or_else(|_| "pretty".to_string());

    let fmt_layer = match nl_log_fmt.as_str() {
        "compact" => tracing_subscriber::fmt::layer()
            .compact()
            .with_timer(tracing_subscriber::fmt::time::time())
            .with_filter(stdout_filter)
            .boxed(),
        "json" => tracing_subscriber::fmt::layer()
            .json()
            .with_timer(tracing_subscriber::fmt::time::time())
            .with_filter(stdout_filter)
            .boxed(),
        _ => tracing_subscriber::fmt::layer()
            .pretty()
            .with_timer(tracing_subscriber::fmt::time::time())
            .with_filter(stdout_filter)
            .boxed(),
    };

    // TODO(aaronmondal): Consider making the resource name configurable.
    let resource = Resource::builder().with_service_name("nativelink").build();

    // Logs
    let log_provider = SdkLoggerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(
            opentelemetry_otlp::LogExporter::builder()
                .with_tonic()
                .with_protocol(Protocol::Grpc)
                .build()
                .map_err(|e| make_err!(Code::Internal, "{e}"))
                .err_tip(|| "While creating OpenTelemetry OTLP Log exporter")?,
        )
        .build();

    let otlp_log_layer = OpenTelemetryTracingBridge::new(&log_provider).with_filter(otlp_filter());

    // Traces
    let tracer_provider = opentelemetry_sdk::trace::SdkTracerProvider::builder()
        .with_resource(resource.clone())
        .with_batch_exporter(
            opentelemetry_otlp::SpanExporter::builder()
                .with_tonic()
                .with_protocol(Protocol::Grpc)
                .build()
                .map_err(|e| make_err!(Code::Internal, "{e}"))
                .err_tip(|| "While creating OpenTelemetry OTLP Trace exporter")?,
        )
        .build();

    let otlp_trace_layer = tracing_opentelemetry::layer()
        .with_tracer(tracer_provider.tracer("nativelink"))
        .with_filter(otlp_filter());

    // Metrics
    let metrics_provider = SdkMeterProvider::builder()
        .with_resource(resource)
        .with_periodic_exporter(
            opentelemetry_otlp::MetricExporter::builder()
                .with_tonic()
                .with_protocol(Protocol::Grpc)
                .build()
                .map_err(|e| make_err!(Code::Internal, "{e}"))
                .err_tip(|| "While creating OpenTelemetry OTLP Metric exporter")?,
        )
        .build();

    opentelemetry::global::set_meter_provider(metrics_provider.clone());

    let otlp_metrics_layer =
        tracing_opentelemetry::MetricsLayer::new(metrics_provider).with_filter(otlp_filter());

    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(otlp_log_layer)
        .with(otlp_trace_layer)
        .with(otlp_metrics_layer)
        .init();

    INITIALIZED.set(()).unwrap_or(());

    Ok(())
}

/// The OTLP logic in the main tracing loop causes issues with the tokio runtime
/// in tests, so we use a more naive logger for those.
pub fn init_tracing_for_tests() -> Result<(), nativelink_error::Error> {
    use std::sync::OnceLock;

    static INITIALIZED: OnceLock<()> = OnceLock::new();

    INITIALIZED.get_or_init(|| {
        let filter = tracing_subscriber::EnvFilter::builder()
            // TODO(aaronmondal): During this implementation we observed
            // deadlock issues when the log level is set to INFO. That should
            // not happen.
            .with_default_directive(tracing::metadata::LevelFilter::WARN.into())
            .from_env_lossy();
        tracing_subscriber::fmt().with_env_filter(filter).init();
    });

    Ok(())
}
