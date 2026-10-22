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
use std::collections::HashMap;
use std::env;
use std::sync::{Arc, OnceLock};

use base64::Engine;
use base64::prelude::BASE64_STANDARD_NO_PAD;
use ginepro::LoadBalancedChannel;
use hyper::http::Response;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_proto::build::bazel::remote::execution::v2::RequestMetadata;
use opentelemetry::propagation::{Injector, TextMapCompositePropagator, TextMapPropagator};
use opentelemetry::trace::{TraceContextExt, Tracer, TracerProvider};
use opentelemetry::{KeyValue, global};
use opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge;
use opentelemetry_http::HeaderExtractor;
use opentelemetry_otlp::{
    LogExporter, MetricExporter, Protocol, SpanExporter, WithExportConfig, WithTonicConfig,
};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::propagation::{BaggagePropagator, TraceContextPropagator};
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_semantic_conventions::attribute::ENDUSER_ID;
use prost::Message;
use tonic::transport::{Certificate, ClientTlsConfig, Identity};
use tracing::debug;
use tracing::metadata::LevelFilter;
use tracing_opentelemetry::{MetricsLayer, layer};
use tracing_subscriber::filter::Directive;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer, Registry, fmt, registry};
use uuid::Uuid;

use crate::metrics::record_rpc_served;
use crate::origin_event::{BAZEL_METADATA_KEY, request_metadata_to_baggage};

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
pub async fn init_tracing() -> Result<(), Error> {
    static INITIALIZED: OnceLock<()> = OnceLock::new();

    if INITIALIZED.get().is_some() {
        return Err(make_err!(Code::Internal, "Logging already initialized"));
    }

    // We currently use a UUIDv4 for "service.instance.id" as per:
    // https://opentelemetry.io/docs/specs/semconv/attributes-registry/service/
    // This might change as we get a better understanding of its use cases in the
    // context of broader observability infrastructure.
    let resource = Resource::builder()
        .with_service_name(NATIVELINK_SERVICE_NAME)
        .with_attribute(KeyValue::new(
            "service.instance.id",
            Uuid::new_v4().to_string(),
        ))
        .build();

    init_propagator();

    let channels = load_balanced_channels().await?;

    // Logs
    let mut log_exporter_builder = LogExporter::builder().with_tonic();
    if let Some(channel) = channels.logs {
        log_exporter_builder = log_exporter_builder.with_channel(channel.into());
    }
    if let Some(tls_config) = exporter_tls_config("LOGS")? {
        log_exporter_builder = log_exporter_builder.with_tls_config(tls_config);
    }
    let otlp_log_layer = OpenTelemetryTracingBridge::new(
        &SdkLoggerProvider::builder()
            .with_resource(resource.clone())
            .with_batch_exporter(
                log_exporter_builder
                    .with_protocol(Protocol::Grpc)
                    .build()
                    .map_err(|e| make_err!(Code::Internal, "{e}"))
                    .err_tip(|| "While creating OpenTelemetry OTLP Log exporter")?,
            )
            .build(),
    )
    .with_filter(otlp_filter());

    // Traces
    let mut span_exporter_builder = SpanExporter::builder().with_tonic();
    if let Some(channel) = channels.traces {
        span_exporter_builder = span_exporter_builder.with_channel(channel.into());
    }
    if let Some(tls_config) = exporter_tls_config("TRACES")? {
        span_exporter_builder = span_exporter_builder.with_tls_config(tls_config);
    }
    let otlp_trace_layer = layer()
        .with_tracer(
            SdkTracerProvider::builder()
                .with_resource(resource.clone())
                .with_batch_exporter(
                    span_exporter_builder
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
    let mut metric_exporter_builder = MetricExporter::builder().with_tonic();
    if let Some(channel) = channels.metrics {
        metric_exporter_builder = metric_exporter_builder.with_channel(channel.into());
    }
    if let Some(tls_config) = exporter_tls_config("METRICS")? {
        metric_exporter_builder = metric_exporter_builder.with_tls_config(tls_config);
    }
    let meter_provider = SdkMeterProvider::builder()
        .with_resource(resource)
        .with_periodic_exporter(
            metric_exporter_builder
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

/// Environment variable pointing OTLP exporters at a load-balanced endpoint.
pub const NL_OTEL_ENDPOINT: &str = "NL_OTEL_ENDPOINT";

/// The OTLP signals, as the OpenTelemetry specification spells them in the
/// per-signal variables (`OTEL_EXPORTER_OTLP_TRACES_CERTIFICATE`).
const OTLP_SIGNALS: [&str; 3] = ["LOGS", "TRACES", "METRICS"];

/// The load-balanced channel each exporter sends on when
/// [`NL_OTEL_ENDPOINT`] is set, and `None` for each when it is not.
///
/// The common case is one channel shared by all three. A channel carries
/// its TLS configuration, and an exporter given a channel ignores its own
/// `with_tls_config`, so a signal with a certificate variable of its own
/// gets a channel of its own, built with that signal's material.
#[derive(Debug, Default)]
pub struct SignalChannels {
    pub logs: Option<LoadBalancedChannel>,
    pub traces: Option<LoadBalancedChannel>,
    pub metrics: Option<LoadBalancedChannel>,
}

/// Whether any per-signal certificate variable is set and non-empty, which
/// is when the signals need channels of their own.
fn any_per_signal_tls_variable_set() -> bool {
    OTLP_SIGNALS.iter().any(|signal| {
        [OTLP_CERTIFICATE, OTLP_CLIENT_CERTIFICATE, OTLP_CLIENT_KEY]
            .iter()
            .any(|suffix| {
                env::var(format!("OTEL_EXPORTER_OTLP_{signal}_{suffix}"))
                    .is_ok_and(|value| !value.is_empty())
            })
    })
}

/// Builds the channels the exporters send on when [`NL_OTEL_ENDPOINT`] is
/// set: one shared channel with the general certificate variables, or, when
/// any per-signal certificate variable is set, one per signal with that
/// signal's resolution of the variables. Without the endpoint variable
/// every field is `None` and each exporter builds its own connection.
///
/// # Errors
///
/// As [`load_balanced_channel_for`].
pub async fn load_balanced_channels() -> Result<SignalChannels, Error> {
    if env::var(NL_OTEL_ENDPOINT).is_err() {
        return Ok(SignalChannels::default());
    }
    if !any_per_signal_tls_variable_set() {
        let shared = load_balanced_channel_for(None).await?;
        return Ok(SignalChannels {
            logs: shared.clone(),
            traces: shared.clone(),
            metrics: shared,
        });
    }
    Ok(SignalChannels {
        logs: load_balanced_channel_for(Some("LOGS")).await?,
        traces: load_balanced_channel_for(Some("TRACES")).await?,
        metrics: load_balanced_channel_for(Some("METRICS")).await?,
    })
}

/// Creates a load-balanced gRPC channel for the endpoint configured via
/// [`NL_OTEL_ENDPOINT`] with the general certificate variables, or returns
/// `None` if the variable isn't set. This is [`load_balanced_channel_for`]
/// with no signal, kept for the integration tests that verify exporters
/// behave the same with and without the load-balanced channel;
/// [`init_tracing`] goes through [`load_balanced_channels`].
///
/// # Errors
///
/// As [`load_balanced_channel_for`].
pub async fn maybe_load_balanced_channel() -> Result<Option<LoadBalancedChannel>, Error> {
    load_balanced_channel_for(None).await
}

/// Creates a load-balanced gRPC channel for the endpoint configured via
/// [`NL_OTEL_ENDPOINT`], or returns `None` if the variable isn't set. An
/// `https://` endpoint gets TLS, with the material the OpenTelemetry
/// certificate variables name for `signal` (see [`otlp_tls_config`]) or the
/// system trust store without them; a plaintext endpoint gets no TLS
/// configuration at all.
///
/// # Errors
///
/// Returns `Err` if the endpoint is not a URL with a host and an explicit
/// port, if its hostname cannot be resolved, or if the certificate variables
/// name files that cannot be read.
pub async fn load_balanced_channel_for(
    signal: Option<&str>,
) -> Result<Option<LoadBalancedChannel>, Error> {
    let Ok(endpoint) = env::var(NL_OTEL_ENDPOINT) else {
        return Ok(None);
    };
    let url = Url::parse(endpoint.as_str())
        .map_err(|e| {
            make_err!(
                Code::InvalidArgument,
                "Unable to parse endpoint {endpoint}: {e:?}"
            )
        })
        .err_tip(|| format!("In {NL_OTEL_ENDPOINT}"))?;

    let host = url
        .host()
        .err_tip(|| format!("Unable to get host from endpoint {endpoint}"))?;
    let port = url
        .port()
        .err_tip(|| format!("Unable to get port from endpoint {endpoint}"))?;

    let mut builder = LoadBalancedChannel::builder((host.to_string(), port));
    if url.scheme() == "https" {
        let tls_config = otlp_tls_config(signal)?.unwrap_or_else(system_roots_tls_config);
        builder = builder.with_tls(tls_config);
    }
    builder
        .channel()
        .await
        .map_err(|e| make_err!(Code::Internal, "Invalid hostname '{endpoint}': {e}"))
        .map(Some)
}

/// The OpenTelemetry specification's variables for the OTLP exporters' TLS
/// material, as the suffix after `OTEL_EXPORTER_OTLP_`: the CA that signs
/// the collector's certificate, and the certificate and key the exporter
/// presents when the collector asks for one. `opentelemetry-otlp` 0.32 does
/// not read these itself, so they are read here.
const OTLP_CERTIFICATE: &str = "CERTIFICATE";
const OTLP_CLIENT_CERTIFICATE: &str = "CLIENT_CERTIFICATE";
const OTLP_CLIENT_KEY: &str = "CLIENT_KEY";

/// The default OTLP endpoint of the gRPC exporters, as `opentelemetry-otlp`
/// resolves it when no endpoint variable is set.
const OTLP_GRPC_ENDPOINT_DEFAULT: &str = "http://localhost:4317";

/// Looks an OTLP variable up the way the specification orders them: the
/// per-signal form (`OTEL_EXPORTER_OTLP_TRACES_CERTIFICATE`) wins over the
/// general one (`OTEL_EXPORTER_OTLP_CERTIFICATE`), and an empty value counts
/// as unset. Returns the name that was found with its value, for messages.
fn otlp_env_var(signal: Option<&str>, suffix: &str) -> Option<(String, String)> {
    signal
        .map(|signal| format!("OTEL_EXPORTER_OTLP_{signal}_{suffix}"))
        .into_iter()
        .chain(core::iter::once(format!("OTEL_EXPORTER_OTLP_{suffix}")))
        .find_map(|name| {
            env::var(&name)
                .ok()
                .filter(|value| !value.is_empty())
                .map(|value| (name, value))
        })
}

/// Whether the exporter for `signal` will talk to an `https://` endpoint,
/// resolved the way `opentelemetry-otlp` resolves it. The scheme is
/// compared case-insensitively, as the SDK accepts `HTTPS://` too; a value
/// that does not parse as a URL counts as plaintext and is left for the
/// exporter to reject. (The `NL_OTEL_ENDPOINT` path parses its value with
/// `Url` as well, so it judges the scheme the same way.)
fn otlp_endpoint_is_https(signal: &str) -> bool {
    let endpoint = otlp_env_var(Some(signal), "ENDPOINT").map_or_else(
        || OTLP_GRPC_ENDPOINT_DEFAULT.to_string(),
        |(_, value)| value,
    );
    Url::parse(&endpoint).is_ok_and(|url| url.scheme() == "https")
}

/// A TLS configuration that trusts the system's certificate store: what an
/// `https://` collector gets when no certificate variable names a CA.
/// tonic's bare `ClientTlsConfig::new()` trusts nothing at all.
fn system_roots_tls_config() -> ClientTlsConfig {
    ClientTlsConfig::new().with_enabled_roots()
}

fn read_pem((name, path): &(String, String)) -> Result<Vec<u8>, Error> {
    std::fs::read(path).map_err(|e| {
        make_err!(
            Code::InvalidArgument,
            "Could not read {path}, which {name} names: {e}"
        )
    })
}

/// The TLS configuration the OpenTelemetry certificate variables describe
/// for `signal` (`LOGS`, `TRACES` or `METRICS`; `None` reads only the
/// general variables), or `None` when none of them is set.
///
/// `OTEL_EXPORTER_OTLP_CERTIFICATE` names the CA to verify the collector
/// with, and then only that CA is trusted; without it the system trust store
/// is. `OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE` and
/// `OTEL_EXPORTER_OTLP_CLIENT_KEY` name the identity to present for mTLS
/// and have to be set together. Each has a per-signal form that wins over
/// the general one. The files are read here, once, at startup.
///
/// # Errors
///
/// Returns `Err` if a named file cannot be read, or if only one of the
/// client certificate and key is set.
pub fn otlp_tls_config(signal: Option<&str>) -> Result<Option<ClientTlsConfig>, Error> {
    let ca = otlp_env_var(signal, OTLP_CERTIFICATE);
    let client_certificate = otlp_env_var(signal, OTLP_CLIENT_CERTIFICATE);
    let client_key = otlp_env_var(signal, OTLP_CLIENT_KEY);
    if ca.is_none() && client_certificate.is_none() && client_key.is_none() {
        return Ok(None);
    }
    let mut config = match &ca {
        Some(ca) => ClientTlsConfig::new().ca_certificate(Certificate::from_pem(read_pem(ca)?)),
        None => system_roots_tls_config(),
    };
    match (&client_certificate, &client_key) {
        (Some(certificate), Some(key)) => {
            config = config.identity(Identity::from_pem(read_pem(certificate)?, read_pem(key)?));
        }
        (None, None) => {}
        (Some((name, _)), None) | (None, Some((name, _))) => {
            return Err(make_err!(
                Code::InvalidArgument,
                "{name} is set without its pair: a client certificate and its key have to be \
                 set together, as OTEL_EXPORTER_OTLP_{OTLP_CLIENT_CERTIFICATE} and \
                 OTEL_EXPORTER_OTLP_{OTLP_CLIENT_KEY} or their per-signal forms"
            ));
        }
    }
    Ok(Some(config))
}

/// The TLS configuration for the exporter of `signal` when it builds its
/// own channel: nothing for a plaintext endpoint, whatever the certificate
/// variables say, and for an `https://` endpoint the variables' material or
/// the system trust store without them. A plaintext signal must get no
/// configuration at all: the exporter would still hand it to tonic, which
/// loads the trust roots as it builds the connector and fails on a host
/// without a certificate bundle, so a deployment that sends one signal
/// over TLS and another in plaintext would die building the plaintext one.
fn exporter_tls_config(signal: &str) -> Result<Option<ClientTlsConfig>, Error> {
    if !otlp_endpoint_is_https(signal) {
        return Ok(None);
    }
    Ok(Some(
        otlp_tls_config(Some(signal))?.unwrap_or_else(system_roots_tls_config),
    ))
}
/// This is the header that bazel sends when using the `--remote_header` flag.
/// TODO(palfrey): Bazel supports other headers, and we should optimize their usage.
const BAZEL_REQUESTMETADATA_HEADER: &str = "build.bazel.remote.execution.v2.requestmetadata-bin";

use opentelemetry::baggage::BaggageExt;
use opentelemetry::context::FutureExt;
use url::Url;

/// ASCII headers from an inbound client request, stored in the task context
/// so that outgoing upstream calls can forward them (e.g. JWT auth tokens).
#[derive(Clone, Debug, Default)]
pub struct ClientHeaders(pub Arc<HashMap<String, String>>);

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
        // See: <https://docs.rs/tower/latest/tower/trait.Service.html#be-careful-when-cloning-inner-services>
        let clone = self.inner.clone();
        let mut inner = core::mem::replace(&mut self.inner, clone);

        // Capture all ASCII-valued request headers before req is consumed, so
        // they can be forwarded to upstream services (e.g. JWT auth tokens).
        let client_headers = ClientHeaders(Arc::new(
            req.headers()
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|v| (name.as_str().to_lowercase(), v.to_string()))
                })
                .collect(),
        ));

        let parent_cx = global::get_text_map_propagator(|propagator| {
            propagator.extract(&HeaderExtractor(req.headers()))
        });

        let identity = parent_cx
            .baggage()
            .get(ENDUSER_ID)
            .map(|value| value.as_str().to_string())
            .unwrap_or_default();

        if identity.is_empty() {
            if self.identity_required {
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
        } else {
            debug!("Baggage enduser.id: {identity}");
        }

        let tracer = global::tracer("origin_middleware");
        let span = tracer
            .span_builder("origin_request")
            .with_kind(opentelemetry::trace::SpanKind::Server)
            .start_with_context(&tracer, &parent_cx);

        let mut cx = parent_cx.with_span(span);

        if let Some(bazel_header) = req.headers().get(BAZEL_REQUESTMETADATA_HEADER)
            && let Ok(decoded) = BASE64_STANDARD_NO_PAD.decode(bazel_header.as_bytes())
            && let Ok(metadata) = RequestMetadata::decode(decoded.as_slice())
        {
            let metadata_str = format!("{metadata:?}");
            debug!("Baggage Bazel request metadata: {metadata_str}");
            cx = cx.with_baggage(vec![
                KeyValue::new(BAZEL_METADATA_KEY, request_metadata_to_baggage(&metadata)),
                KeyValue::new(ENDUSER_ID, identity),
            ]);
        }

        let cx = cx.with_value(client_headers);
        // Rate, errors and latency for every gRPC service, taken here because
        // this layer already wraps all of them.
        let route = req.uri().path().to_string();
        let started = std::time::Instant::now();
        Box::pin(async move {
            let result = inner.call(req).with_context(cx).await;
            if let Ok(response) = &result {
                record_rpc_served(
                    &route,
                    grpc_status_of(response.headers()),
                    started.elapsed().as_secs_f64(),
                );
            }
            result
        })
    }
}

/// Reads the gRPC status from a response.
///
/// Tonic puts `grpc-status` in the headers when it fails before streaming and
/// in the trailers otherwise. Trailers are not available here without
/// consuming the body, so a call that got far enough to stream is reported as
/// OK. Transport-level failures never reach this point at all.
fn grpc_status_of(headers: &hyper::http::HeaderMap) -> i32 {
    headers
        .get("grpc-status")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0)
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

/// Installs the propagator the servers extract with and the forwarders inject
/// with: W3C baggage and trace context.
pub fn init_propagator() {
    global::set_text_map_propagator(TextMapCompositePropagator::new(vec![
        Box::new(BaggagePropagator::new()),
        Box::new(TraceContextPropagator::new()),
    ]));
}

/// Copies the nominated headers of the inbound client request, as the
/// [`OtlpMiddleware`] stored them in the context, onto an outgoing request.
/// This is how a client's credentials reach the upstream behind a forwarding
/// store or scheduler; names match case-insensitively.
pub fn forward_client_headers<T>(request: &mut tonic::Request<T>, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let Some(client_headers) = opentelemetry::Context::current()
        .get::<ClientHeaders>()
        .cloned()
    else {
        return;
    };
    for name in names {
        if let Some(value) = client_headers.0.get(&name.to_lowercase())
            && let (Ok(k), Ok(v)) = (
                tonic::metadata::MetadataKey::from_bytes(name.as_bytes()),
                tonic::metadata::MetadataValue::try_from(value.as_str()),
            )
        {
            request.metadata_mut().insert(k, v);
        }
    }
}

/// Writes a context's trace and baggage into an outgoing gRPC request, so a
/// service that forwards a call (a grpc store, a grpc scheduler) hands the
/// upstream the identity, the client's build metadata and the trace it was
/// given instead of a bare request.
pub fn inject_context_with<T>(
    propagator: &dyn TextMapPropagator,
    context: &opentelemetry::Context,
    request: &mut tonic::Request<T>,
) {
    struct MetadataInjector<'a>(&'a mut tonic::metadata::MetadataMap);

    impl Injector for MetadataInjector<'_> {
        fn set(&mut self, key: &str, value: String) {
            if let (Ok(k), Ok(v)) = (
                tonic::metadata::MetadataKey::from_bytes(key.as_bytes()),
                tonic::metadata::MetadataValue::try_from(&value),
            ) {
                self.0.insert(k, v);
            }
        }
    }

    propagator.inject_context(context, &mut MetadataInjector(request.metadata_mut()));
}

/// [`inject_context_with`] for the current context and the global propagator.
pub fn inject_context<T>(request: &mut tonic::Request<T>) {
    global::get_text_map_propagator(|propagator| {
        inject_context_with(propagator, &opentelemetry::Context::current(), request);
    });
}

#[cfg(test)]
mod tests {
    use nativelink_macro::nativelink_test;
    use serial_test::serial;

    use super::*;

    // ginepro's default resolver (hickory-dns) reads /etc/resolv.conf, which
    // doesn't exist in sandboxed environments (e.g. Nix builds).
    #[cfg(unix)]
    fn dns_configured() -> bool {
        std::path::Path::new("/etc/resolv.conf").exists()
    }
    #[cfg(not(unix))]
    const fn dns_configured() -> bool {
        true
    }

    // Env vars are process-global, so concurrent writes would be a data race
    // on Unix.  `#[serial(env)]` serializes all env-mutating tests.
    #[serial(env)]
    #[nativelink_test("crate")]
    async fn channel_absent_when_env_not_set() {
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::remove_var(NL_OTEL_ENDPOINT) };
        assert!(
            maybe_load_balanced_channel()
                .await
                .expect("an unset endpoint is not an error")
                .is_none(),
            "Expected None when {NL_OTEL_ENDPOINT} is unset"
        );
    }

    #[serial(env)]
    #[nativelink_test("crate")]
    async fn channel_present_when_valid_endpoint_set() {
        if !dns_configured() {
            eprintln!(
                "Skipping channel_present_when_valid_endpoint_set: no DNS configuration \
                 available (e.g. sandboxed Nix build)"
            );
            return;
        }
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var(NL_OTEL_ENDPOINT, "http://localhost:4317") };
        let result = maybe_load_balanced_channel().await;
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::remove_var(NL_OTEL_ENDPOINT) };
        assert!(
            result.expect("a valid endpoint builds").is_some(),
            "Expected Some(channel) when {NL_OTEL_ENDPOINT} points to a valid URL"
        );
    }

    /// Clears every OTLP variable a TLS test may set, before and after it.
    fn clear_otlp_tls_env() {
        for signal in ["", "LOGS_", "TRACES_", "METRICS_"] {
            for suffix in [
                "ENDPOINT",
                "CERTIFICATE",
                "CLIENT_CERTIFICATE",
                "CLIENT_KEY",
            ] {
                // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
                unsafe { env::remove_var(format!("OTEL_EXPORTER_OTLP_{signal}{suffix}")) };
            }
        }
    }

    /// Writes dummy PEM bytes to a file in `dir`; tonic only stores them
    /// until a connection is made, so the content need not parse.
    fn pem_file(dir: &tempfile::TempDir, name: &str) -> String {
        let path = dir.path().join(name);
        std::fs::write(&path, format!("-----BEGIN {name}-----\n")).unwrap();
        path.to_str().unwrap().to_string()
    }

    // The exporter used to refuse an https endpoint outright ("uses HTTPS
    // but no TLS feature is enabled"); this fails without `tls-ring` on
    // `opentelemetry-otlp`.
    #[serial(env)]
    #[nativelink_test("crate")]
    async fn https_endpoint_builds_an_exporter() {
        clear_otlp_tls_env();
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "https://localhost:4317") };
        let mut builder = SpanExporter::builder().with_tonic();
        if let Some(tls_config) = exporter_tls_config("TRACES").unwrap() {
            builder = builder.with_tls_config(tls_config);
        }
        let result = builder.with_protocol(Protocol::Grpc).build();
        clear_otlp_tls_env();
        assert!(
            result.is_ok(),
            "An https endpoint must build an exporter: {:?}",
            result.err()
        );
    }

    #[serial(env)]
    #[nativelink_test("crate")]
    async fn tls_config_absent_without_variables() {
        clear_otlp_tls_env();
        assert!(otlp_tls_config(Some("TRACES")).unwrap().is_none());
        assert!(otlp_tls_config(None).unwrap().is_none());
        // A plaintext endpoint gets no TLS configuration at all, so a host
        // without a certificate store still exports.
        assert!(exporter_tls_config("TRACES").unwrap().is_none());
        // An empty value is unset, as the specification says.
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_CERTIFICATE", "") };
        let result = otlp_tls_config(Some("TRACES"));
        clear_otlp_tls_env();
        assert!(result.unwrap().is_none());
    }

    #[serial(env)]
    #[nativelink_test("crate")]
    async fn tls_config_present_with_ca_certificate() {
        clear_otlp_tls_env();
        let dir = tempfile::tempdir().unwrap();
        let ca = pem_file(&dir, "ca");
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_CERTIFICATE", &ca) };
        let general = otlp_tls_config(None);
        let per_signal = otlp_tls_config(Some("METRICS"));
        // The endpoint is the plaintext default, so the exporter gets no
        // TLS configuration however the variables are set.
        let exporter = exporter_tls_config("METRICS");
        clear_otlp_tls_env();
        assert!(general.unwrap().is_some());
        assert!(per_signal.unwrap().is_some());
        assert!(exporter.unwrap().is_none());
    }

    // A deployment that sends metrics over TLS with an identity and logs in
    // plaintext: the plaintext signal must get no TLS configuration, or the
    // exporter loads the trust roots for it and a host without a
    // certificate bundle dies building the log exporter.
    #[serial(env)]
    #[nativelink_test("crate")]
    async fn plaintext_signal_gets_no_tls_config_whatever_the_variables() {
        clear_otlp_tls_env();
        let dir = tempfile::tempdir().unwrap();
        let certificate = pem_file(&dir, "client");
        let key = pem_file(&dir, "key");
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE", &certificate) };
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_CLIENT_KEY", &key) };
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe {
            env::set_var(
                "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
                "https://collector:4317",
            );
        };
        let logs = exporter_tls_config("LOGS");
        let metrics = exporter_tls_config("METRICS");
        clear_otlp_tls_env();
        assert!(
            logs.unwrap().is_none(),
            "the plaintext default endpoint gets no TLS configuration"
        );
        assert!(
            metrics.unwrap().is_some(),
            "the https endpoint gets the variables' configuration"
        );
    }

    // The SDK accepts an uppercase scheme; an `HTTPS://` endpoint has to
    // get its trust roots too, or the process starts with an empty store.
    #[serial(env)]
    #[nativelink_test("crate")]
    async fn uppercase_https_scheme_gets_the_default_roots() {
        clear_otlp_tls_env();
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe {
            env::set_var(
                "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT",
                "HTTPS://collector:4317",
            );
        };
        let traces = exporter_tls_config("TRACES");
        // A value that is not a URL is not https; the exporter rejects it.
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "collector:4317:https") };
        let unparsable = exporter_tls_config("TRACES");
        clear_otlp_tls_env();
        assert!(traces.unwrap().is_some(), "HTTPS:// is an https endpoint");
        assert!(unparsable.unwrap().is_none());
    }

    #[serial(env)]
    #[nativelink_test("crate")]
    async fn tls_config_with_client_identity() {
        clear_otlp_tls_env();
        let dir = tempfile::tempdir().unwrap();
        let certificate = pem_file(&dir, "client");
        let key = pem_file(&dir, "key");
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE", &certificate) };
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_CLIENT_KEY", &key) };
        let result = otlp_tls_config(Some("LOGS"));
        clear_otlp_tls_env();
        assert!(result.unwrap().is_some());
    }

    #[serial(env)]
    #[nativelink_test("crate")]
    async fn per_signal_variable_wins_over_the_general_one() {
        clear_otlp_tls_env();
        let dir = tempfile::tempdir().unwrap();
        let ca = pem_file(&dir, "ca");
        let missing = dir.path().join("missing").to_str().unwrap().to_string();
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_CERTIFICATE", &missing) };
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_TRACES_CERTIFICATE", &ca) };
        let traces = otlp_tls_config(Some("TRACES"));
        let metrics = otlp_tls_config(Some("METRICS"));
        let general = otlp_tls_config(None);
        clear_otlp_tls_env();
        assert!(traces.unwrap().is_some(), "TRACES names a readable file");
        let metrics_err = metrics.expect_err("METRICS falls back to the general, missing file");
        assert_eq!(metrics_err.code, Code::InvalidArgument);
        assert!(
            metrics_err
                .to_string()
                .contains("OTEL_EXPORTER_OTLP_CERTIFICATE"),
            "{metrics_err}"
        );
        assert_eq!(
            general.expect_err("the general file is missing").code,
            Code::InvalidArgument
        );
    }

    #[serial(env)]
    #[nativelink_test("crate")]
    async fn client_certificate_without_key_is_rejected() {
        clear_otlp_tls_env();
        let dir = tempfile::tempdir().unwrap();
        let certificate = pem_file(&dir, "client");
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_CLIENT_CERTIFICATE", &certificate) };
        let without_key = otlp_tls_config(Some("LOGS"));
        clear_otlp_tls_env();
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_LOGS_CLIENT_KEY", &certificate) };
        let without_certificate = otlp_tls_config(Some("LOGS"));
        clear_otlp_tls_env();
        for result in [without_key, without_certificate] {
            let err = result.expect_err("half an identity is an error");
            assert_eq!(err.code, Code::InvalidArgument);
            assert!(
                err.to_string().contains("OTEL_EXPORTER_OTLP_CLIENT_KEY"),
                "{err}"
            );
        }
    }

    #[serial(env)]
    #[nativelink_test("crate")]
    async fn missing_certificate_file_is_rejected() {
        clear_otlp_tls_env();
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing.pem").to_str().unwrap().to_string();
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_CERTIFICATE", &missing) };
        let result = otlp_tls_config(Some("METRICS"));
        clear_otlp_tls_env();
        let err = result.expect_err("a missing file is an error");
        assert_eq!(err.code, Code::InvalidArgument);
        assert!(err.to_string().contains("missing.pem"), "{err}");
        assert!(
            err.to_string().contains("OTEL_EXPORTER_OTLP_CERTIFICATE"),
            "{err}"
        );
    }

    // A per-signal certificate variable has to reach the channel the
    // exporter is handed: an exporter given a channel ignores its own TLS
    // configuration, so a shared channel built from the general variables
    // alone failed the handshake with "unknown CA" for a signal whose CA
    // is set per signal.
    #[serial(env)]
    #[nativelink_test("crate")]
    async fn per_signal_certificate_reaches_the_load_balanced_channel() {
        clear_otlp_tls_env();
        let dir = tempfile::tempdir().unwrap();
        let ca = pem_file(&dir, "ca");
        let missing = dir.path().join("missing").to_str().unwrap().to_string();
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var(NL_OTEL_ENDPOINT, "https://localhost:4317") };
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_CERTIFICATE", &missing) };
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_TRACES_CERTIFICATE", &ca) };
        assert!(any_per_signal_tls_variable_set());
        let traces = load_balanced_channel_for(Some("TRACES")).await;
        let metrics = load_balanced_channel_for(Some("METRICS")).await;
        let all = load_balanced_channels().await;
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::remove_var(NL_OTEL_ENDPOINT) };
        clear_otlp_tls_env();

        // METRICS falls back to the general variable, whose file is
        // missing; that error precedes any name resolution, so it holds
        // with or without DNS, and so does the whole-set build that
        // includes METRICS.
        for result in [metrics, all.map(|channels| channels.metrics)] {
            let err = result.expect_err("the general CA file is missing");
            assert_eq!(err.code, Code::InvalidArgument);
            assert!(
                err.to_string().contains("OTEL_EXPORTER_OTLP_CERTIFICATE"),
                "{err}"
            );
        }
        // TRACES reads its own file; building the channel then resolves
        // the host, which needs DNS.
        if dns_configured() {
            assert!(
                traces.expect("TRACES names a readable CA").is_some(),
                "Expected a channel for TRACES when {NL_OTEL_ENDPOINT} is set"
            );
        } else {
            eprintln!(
                "Skipping the TRACES channel assertion: no DNS configuration \
                 available (e.g. sandboxed Nix build)"
            );
        }
    }

    // Without a per-signal variable the three signals share one channel,
    // and without the endpoint variable they get none.
    #[serial(env)]
    #[nativelink_test("crate")]
    async fn signals_share_one_channel_without_per_signal_variables() {
        clear_otlp_tls_env();
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::remove_var(NL_OTEL_ENDPOINT) };
        assert!(!any_per_signal_tls_variable_set());
        let channels = load_balanced_channels()
            .await
            .expect("an unset endpoint is not an error");
        assert!(channels.logs.is_none() && channels.traces.is_none() && channels.metrics.is_none());

        // An empty per-signal value is unset, so it does not split the
        // channel either.
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_METRICS_CLIENT_KEY", "") };
        let split = any_per_signal_tls_variable_set();
        clear_otlp_tls_env();
        assert!(!split);

        if !dns_configured() {
            eprintln!(
                "Skipping the shared channel assertion: no DNS configuration \
                 available (e.g. sandboxed Nix build)"
            );
            return;
        }
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var(NL_OTEL_ENDPOINT, "http://localhost:4317") };
        let channels = load_balanced_channels().await;
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::remove_var(NL_OTEL_ENDPOINT) };
        let channels = channels.expect("a valid endpoint builds");
        assert!(channels.logs.is_some() && channels.traces.is_some() && channels.metrics.is_some());
    }

    #[serial(env)]
    #[nativelink_test("crate")]
    async fn https_endpoint_without_variables_trusts_the_system_store() {
        clear_otlp_tls_env();
        // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
        unsafe { env::set_var("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", "https://collector:4317") };
        let logs = exporter_tls_config("LOGS");
        let traces = exporter_tls_config("TRACES");
        clear_otlp_tls_env();
        assert!(
            logs.unwrap().is_some(),
            "an https endpoint gets a TLS config"
        );
        assert!(
            traces.unwrap().is_none(),
            "the default endpoint is plaintext"
        );
    }

    #[test]
    fn inject_context_writes_baggage_into_request_metadata() {
        use opentelemetry::baggage::BaggageExt;

        let propagator = BaggagePropagator::new();
        let context =
            opentelemetry::Context::new().with_baggage(vec![KeyValue::new(ENDUSER_ID, "alice")]);
        let mut request = tonic::Request::new(());
        inject_context_with(&propagator, &context, &mut request);
        let baggage = request
            .metadata()
            .get("baggage")
            .expect("the baggage header is written")
            .to_str()
            .unwrap();
        assert!(baggage.contains("enduser.id=alice"), "{baggage}");
    }
}
