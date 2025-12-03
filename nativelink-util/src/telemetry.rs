// Copyright 2025 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::default::Default;
use std::env;
use std::sync::OnceLock;

use base64::Engine;
use base64::prelude::BASE64_STANDARD_NO_PAD;
use hyper::http::Response;
use nativelink_error::{Code, ResultExt, make_err};
use nativelink_proto::build::bazel::remote::execution::v2::RequestMetadata;
use opentelemetry::propagation::TextMapCompositePropagator;
use opentelemetry::trace::{TraceContextExt, Tracer, TracerProvider};
use opentelemetry::{KeyValue, global};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_http::HeaderExtractor;
use opentelemetry_otlp::{LogExporter, MetricExporter, Protocol, SpanExporter, WithExportConfig};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::propagation::{BaggagePropagator, TraceContextPropagator};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_semantic_conventions::attribute::ENDUSER_ID;
use prost::Message;
use tracing::debug;
use tracing::metadata::LevelFilter;
use tracing_opentelemetry::{MetricsLayer, layer};
use tracing_subscriber::filter::Directive;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry, fmt, registry};
use uuid::Uuid;

/// The OTLP "service.name" field for all nativelink services.
const NATIVELINK_SERVICE_NAME: &str = "nativelink";

// An `EnvFilter` to filter out non-nativelink information.
//
// See: https://github.com/open-telemetry/opentelemetry-rust/issues/2877
//
// Note that `EnvFilter` doesn't implement `clone`, so create a new one for
// each telemetry kind.
fn otlp_filter() -> EnvFilter {
    fn expect_parse(directive: &str) -> Directive {
        directive
            .parse()
            .unwrap_or_else(|_| panic!("Static directive '{directive}' failed to parse"))
    }

    EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy()
        .add_directive(expect_parse("hyper=off"))
        .add_directive(expect_parse("tonic=off"))
        .add_directive(expect_parse("h2=off"))
        .add_directive(expect_parse("reqwest=off"))
        .add_directive(expect_parse("tower=off"))
        .add_directive(expect_parse("fred=off"))
}

// Create a tracing layer intended for stdout printing.
//
// The output of this layer is configurable via the `NL_LOG` environment
// variable.
fn tracing_stdout_layer() -> impl Layer<Registry> {
    let nl_log_fmt = env::var("NL_LOG").unwrap_or_else(|_| "pretty".to_string());

    let stdout_filter = otlp_filter();

    match nl_log_fmt.as_str() {
        "compact" => fmt::layer()
            .compact()
            .with_timer(fmt::time::time())
            .with_filter(stdout_filter)
            .boxed(),
        "json" => fmt::layer()
            .json()
            .with_timer(fmt::time::time())
            .with_filter(stdout_filter)
            .boxed(),
        _ => fmt::layer()
            .pretty()
            .with_timer(fmt::time::time())
            .with_filter(stdout_filter)
            .boxed(),
    }
}

/// Initialize tracing with OpenTelemetry support.
///
/// # Errors
///
/// Returns `Err` if logging was already initialized or if the exporters can't
/// be initialized.
pub fn init_tracing() -> Result<(), nativelink_error::Error> {
    static INITIALIZED: OnceLock<()> = OnceLock::new();

    if INITIALIZED.get().is_some() {
        return Err(make_err!(Code::Internal, "Logging already initialized"));
    }

    // We currently use a UUIDv4 for "service.instance.id" as per:
    // https://opentelemetry.io/docs/specs/semconv/attributes-registry/service/
    // This might change as we get a better understanding of its usecases in the
    // context of broader observability infrastructure.
    let resource = Resource::builder()
        .with_service_name(NATIVELINK_SERVICE_NAME)
        .with_attribute(KeyValue::new(
            "service.instance.id",
            Uuid::new_v4().to_string(),
        ))
        .build();

    let propagator = TextMapCompositePropagator::new(vec![
        Box::new(BaggagePropagator::new()),
        Box::new(TraceContextPropagator::new()),
    ]);
    global::set_text_map_propagator(propagator);

    // Logs
    let otlp_log_layer = OpenTelemetryTracingBridge::new(
        &SdkLoggerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(
                LogExporter::builder()
                    .with_tonic()
                    .with_protocol(Protocol::Grpc)
                    .build()
                    .map_err(|e| make_err!(Code::Internal, "{e}"))
                    .err_tip(|| "While creating OpenTelemetry OTLP Log exporter")?,
            )
            .build(),
    )
    .with_filter(otlp_filter());

    // Traces
    let otlp_trace_layer = layer()
        .with_tracer(
            SdkTracerProvider::builder()
                .with_resource(resource.clone())
                .with_batch_exporter(
                    SpanExporter::builder()
                        .with_tonic()
                        .with_protocol(Protocol::Grpc)
                        .build()
                        .map_err(|e| make_err!(Code::Internal, "{e}"))
                        .err_tip(|| "While creating OpenTelemetry OTLP Span exporter")?,
                )
                .build()
                .tracer(NATIVELINK_SERVICE_NAME),
        )
        .with_filter(otlp_filter());

    // Metrics
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource)
        .with_periodic_exporter(
            MetricExporter::builder()
                .with_tonic()
                .with_protocol(Protocol::Grpc)
                .build()
                .map_err(|e| make_err!(Code::Internal, "{e}"))
                .err_tip(|| "While creating OpenTelemetry OTLP Metric exporter")?,
        )
        .build();

    global::set_meter_provider(meter_provider.clone());

    let otlp_metrics_layer = MetricsLayer::new(meter_provider).with_filter(otlp_filter());

    registry()
        .with(tracing_stdout_layer())
        .with(otlp_log_layer)
        .with(otlp_trace_layer)
        .with(otlp_metrics_layer)
        .init();

    INITIALIZED.set(()).unwrap_or(());

    Ok(())
}

/// Custom metadata key field for Bazel metadata.
const BAZEL_METADATA_KEY: &str = "bazel.metadata";

/// This is the header that bazel sends when using the `--remote_header` flag.
/// TODO(palfrey): There are various other headers that bazel supports.
///                    Optimize their usage.
const BAZEL_REQUESTMETADATA_HEADER: &str = "build.bazel.remote.execution.v2.requestmetadata-bin";

use opentelemetry::baggage::BaggageExt;
use opentelemetry::context::FutureExt;

#[derive(Debug, Clone)]
pub struct OtlpMiddleware<S> {
    inner: S,
    identity_required: bool,
}

impl<S> OtlpMiddleware<S> {
    const fn new(inner: S, identity_required: bool) -> Self {
        Self {
            inner,
            identity_required,
        }
    }
}

impl<S, ReqBody, ResBody> tower::Service<hyper::http::Request<ReqBody>> for OtlpMiddleware<S>
where
    S: tower::Service<hyper::http::Request<ReqBody>, Response = Response<ResBody>>
        + Clone
        + Send
        + 'static,
    S::Future: Send + 'static,
    ReqBody: core::fmt::Debug + Send + 'static,
    ResBody: From<String> + Send + 'static + Default,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = futures::future::BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(
        &mut self,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: hyper::http::Request<ReqBody>) -> Self::Future {
        // We must take the current `inner` and not the clone.
        // See: https://docs.rs/tower/latest/tower/trait.Service.html#be-careful-when-cloning-inner-services
        let clone = self.inner.clone();
        let mut inner = core::mem::replace(&mut self.inner, clone);

        let parent_cx = global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(req.headers()))
        });

        let identity = parent_cx
            .baggage()
            .get(ENDUSER_ID)
            .map(|value| value.as_str().to_string())
            .unwrap_or_default();

        if identity.is_empty() && self.identity_required {
            return Box::pin(async move {
                Ok(tonic::Status::failed_precondition(
                    r"

NativeLink instance configured to require this OpenTelemetry Baggage header:

    `Baggage: enduser.id=YOUR_IDENTITY`

",
                )
                .into_http())
            });
        }

        debug!("Baggage enduser.id: {identity}");

        let tracer = global::tracer("origin_middleware");
        let span = tracer
            .span_builder("origin_request")
            .with_kind(opentelemetry::trace::SpanKind::Server)
            .start_with_context(&tracer, &parent_cx);

        let mut cx = parent_cx.with_span(span);

        if let Some(bazel_header) = req.headers().get(BAZEL_REQUESTMETADATA_HEADER) {
            if let Ok(decoded) = BASE64_STANDARD_NO_PAD.decode(bazel_header.as_bytes()) {
                if let Ok(metadata) = RequestMetadata::decode(decoded.as_slice()) {
                    let metadata_str = format!("{metadata:?}");
                    debug!("Baggage Bazel request metadata: {metadata_str}");
                    cx = cx.with_baggage(vec![
                        KeyValue::new(BAZEL_METADATA_KEY, metadata_str),
                        KeyValue::new(ENDUSER_ID, identity),
                    ]);
                }
            }
        }

        Box::pin(async move { inner.call(req).with_context(cx).await })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct OtlpLayer {
    identity_required: bool,
}

impl OtlpLayer {
    pub const fn new(identity_required: bool) -> Self {
        Self { identity_required }
    }
}

impl<S> tower::Layer<S> for OtlpLayer {
    type Service = OtlpMiddleware<S>;

    fn layer(&self, service: S) -> Self::Service {
        OtlpMiddleware::new(service, self.identity_required)
    }
}
