use core::convert::Infallible;
use core::future::Future;
use core::net::SocketAddr;
use core::pin::Pin;
use core::task::{Context as TaskContext, Poll};
use std::collections::HashSet;
use std::env;
use std::sync::{Arc, Mutex};

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::{Router, middleware};
use ginepro::{LoadBalancedChannel, LookupService, ServiceDefinition};
use hyper::{Request, Response, StatusCode, Uri};
use nativelink_macro::nativelink_test;
use nativelink_util::telemetry::{ClientHeaders, NL_OTEL_ENDPOINT, maybe_load_balanced_channel};
use opentelemetry::baggage::BaggageExt;
use opentelemetry::metrics::MeterProvider as _;
use opentelemetry::{Context, KeyValue, global};
use opentelemetry_http::HeaderExtractor as OTELHeaderExtractor;
use opentelemetry_otlp::{MetricExporter, Protocol, WithExportConfig, WithTonicConfig};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use opentelemetry_proto::tonic::metrics::v1::metric;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use serial_test::serial;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::body::Body;
use tower::{Service, ServiceBuilder, ServiceExt};
use tracing::{debug, error, warn};

struct ExtractClientHeaders(ClientHeaders);

impl<S> FromRequestParts<S> for ExtractClientHeaders
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let context = global::get_text_map_propagator(|propagator| {
            propagator.extract(&OTELHeaderExtractor(&parts.headers))
        });
        if let Some(client_headers) = context.get::<ClientHeaders>() {
            Ok(Self(client_headers.clone()))
        } else {
            error!("Missing OTEL headers");
            Err((StatusCode::BAD_REQUEST, "OTEL headers are missing"))
        }
    }
}

#[derive(Clone)]
struct AppState {
    client_headers: Arc<Mutex<Vec<ClientHeaders>>>,
}

impl AppState {
    fn new() -> Self {
        Self {
            client_headers: Arc::new(Mutex::new(vec![])),
        }
    }
}

async fn header_extract(
    ExtractClientHeaders(client_headers): ExtractClientHeaders,
    State(state): State<Arc<AppState>>,
    request: axum::extract::Request,
    next: Next,
) -> axum::response::Response {
    debug!(?client_headers, "Client headers");
    state
        .client_headers
        .lock()
        .unwrap()
        .push(client_headers.clone());
    next.run(request).await
}

fn demo_service(app_state: Arc<AppState>) -> Router {
    Router::default()
        .with_state(app_state.clone())
        .fallback(|uri: Uri| async move {
            warn!("No route for {uri}");
            (StatusCode::NOT_FOUND, format!("No route for {uri}"))
        })
        .layer(
            ServiceBuilder::new()
                .layer(nativelink_util::telemetry::OtlpLayer::new(false))
                .layer(middleware::from_fn_with_state(app_state, header_extract)),
        )
}

async fn run_request(
    svc: &mut Router,
    request: Request<Body>,
) -> Result<(), Box<dyn core::error::Error>> {
    let response: Response<axum::body::Body> =
        svc.as_service().ready().await?.call(request).await?;
    let status = response.status();
    let body = String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await?
            .to_vec(),
    )?;
    assert_eq!(status, 404, "{body}");
    assert_eq!(body, String::from("No route for /demo"));
    Ok(())
}

#[nativelink_test]
async fn oltp_logs_no_baggage() -> Result<(), Box<dyn core::error::Error>> {
    let mut svc = demo_service(Arc::new(AppState::new()));

    let request: Request<Body> = Request::builder()
        .method("GET")
        .uri("/demo")
        .body(Body::empty())?;
    run_request(&mut svc, request).await?;

    assert!(!logs_contain("Baggage enduser.id:"));

    Ok(())
}

#[nativelink_test]
async fn oltp_logs_with_baggage() -> Result<(), Box<dyn core::error::Error>> {
    let mut svc = demo_service(Arc::new(AppState::new()));

    let mut request: Request<Body> = Request::builder()
        .method("GET")
        .uri("/demo")
        .body(Body::empty())?;

    let cx_guard =
        Context::map_current(|cx| cx.with_baggage([KeyValue::new("enduser.id", "foobar")]))
            .attach();

    request
        .headers_mut()
        .insert("baggage", "enduser.id=foobar".parse().unwrap());

    run_request(&mut svc, request).await?;

    assert!(logs_contain("Baggage enduser.id: foobar"));
    drop(cx_guard);

    Ok(())
}

#[nativelink_test]
async fn oltp_logs_with_headers() -> Result<(), Box<dyn core::error::Error>> {
    let app_state = Arc::new(AppState::new());
    let mut svc = demo_service(app_state.clone());

    let request: Request<Body> = Request::builder()
        .method("GET")
        .header("Foo", "bar")
        .uri("/demo")
        .body(Body::empty())?;
    run_request(&mut svc, request).await?;

    let client_headers = app_state.client_headers.lock().unwrap();
    assert_eq!(client_headers.len(), 1, "{client_headers:#?}");
    let client_header = client_headers.first().unwrap();
    assert_eq!(client_header.0.get("foo"), Some(&"bar".to_string()));

    Ok(())
}

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

// Resolves a host:port pair by parsing the host as a literal IP address,
// bypassing hickory-dns and /etc/resolv.conf.  Used by tests that run in
// sandboxed environments (e.g. Nix builds) where DNS is unavailable.
struct DirectIpResolver;

#[async_trait::async_trait]
impl LookupService for DirectIpResolver {
    async fn resolve_service_endpoints(
        &self,
        definition: &ServiceDefinition,
    ) -> Result<HashSet<SocketAddr>, anyhow::Error> {
        let addr: SocketAddr =
            format!("{}:{}", definition.hostname(), definition.port()).parse()?;
        Ok(HashSet::from([addr]))
    }
}

// A minimal in-process OTLP metrics collector backed by tonic.
//
// We implement the gRPC service manually rather than using the generated
// `MetricsServiceServer` from `opentelemetry-proto` so the test stays
// self-contained and does not pull in an additional server dependency.
#[derive(Clone)]
struct TestMetricsService {
    received: Arc<Mutex<Vec<ExportMetricsServiceRequest>>>,
}

const METRICS_EXPORT_PATH: &str = "/opentelemetry.proto.collector.metrics.v1.MetricsService/Export";

impl Service<Request<Body>> for TestMetricsService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, _cx: &mut TaskContext<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Request<Body>) -> Self::Future {
        let received = self.received.clone();
        Box::pin(async move {
            if req.uri().path() == METRICS_EXPORT_PATH {
                let export_svc = tower::service_fn(
                    move |request: tonic::Request<ExportMetricsServiceRequest>| {
                        let received = received.clone();
                        async move {
                            received.lock().unwrap().push(request.into_inner());
                            Ok::<tonic::Response<ExportMetricsServiceResponse>, tonic::Status>(
                                tonic::Response::new(ExportMetricsServiceResponse::default()),
                            )
                        }
                    },
                );
                // ProstCodec<T, U>: Encode = T (sent to client),
                //                   Decode = U (received from client).
                let mut grpc = tonic::server::Grpc::new(tonic_prost::ProstCodec::<
                    ExportMetricsServiceResponse,
                    ExportMetricsServiceRequest,
                >::default());
                Ok(grpc.unary(export_svc, req).await)
            } else {
                let mut resp = Response::new(Body::empty());
                resp.headers_mut().insert(
                    tonic::Status::GRPC_STATUS,
                    (tonic::Code::Unimplemented as i32).into(),
                );
                Ok(resp)
            }
        })
    }
}

impl tonic::server::NamedService for TestMetricsService {
    const NAME: &'static str = "opentelemetry.proto.collector.metrics.v1.MetricsService";
}

// Spawns the in-process OTLP collector on an ephemeral port and returns the
// port together with the requests it receives.
async fn spawn_test_collector()
-> Result<(u16, Arc<Mutex<Vec<ExportMetricsServiceRequest>>>), Box<dyn core::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let incoming = TcpListenerStream::new(listener);

    let received: Arc<Mutex<Vec<ExportMetricsServiceRequest>>> = Arc::new(Mutex::new(vec![]));
    let svc = TestMetricsService {
        received: received.clone(),
    };

    nativelink_util::background_spawn!("otlp_test_collector", async move {
        tonic::transport::Server::builder()
            .add_service(svc)
            .serve_with_incoming(incoming)
            .await
            .ok();
    });

    Ok((port, received))
}

// Exports two counter increments through `exporter` and returns the requests
// the in-process collector received for them.
async fn export_counters_and_drain(
    exporter: MetricExporter,
    received: &Arc<Mutex<Vec<ExportMetricsServiceRequest>>>,
) -> Result<Vec<ExportMetricsServiceRequest>, Box<dyn core::error::Error>> {
    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter)
        .build();

    let meter = meter_provider.meter("nativelink_test");
    let counter = meter
        .u64_counter("test.operations")
        .with_description("Total test operations")
        .build();
    counter.add(3, &[KeyValue::new("operation", "cache_read")]);
    counter.add(7, &[KeyValue::new("operation", "cache_write")]);

    meter_provider.force_flush()?;
    meter_provider.shutdown()?;

    Ok(received.lock().unwrap().split_off(0))
}

// Reduces collected OTLP requests to a comparable, order- and time-independent
// shape: (metric name, description, data point attributes, data point value).
fn extract_sum_data_points(
    requests: &[ExportMetricsServiceRequest],
) -> Vec<(String, String, Vec<String>, String)> {
    let mut points = vec![];
    for request in requests {
        for resource_metrics in &request.resource_metrics {
            for scope_metrics in &resource_metrics.scope_metrics {
                for m in &scope_metrics.metrics {
                    let Some(metric::Data::Sum(sum)) = &m.data else {
                        continue;
                    };
                    for point in &sum.data_points {
                        let mut attributes: Vec<String> = point
                            .attributes
                            .iter()
                            .map(|kv| format!("{kv:?}"))
                            .collect();
                        attributes.sort();
                        points.push((
                            m.name.clone(),
                            m.description.clone(),
                            attributes,
                            format!("{:?}", point.value),
                        ));
                    }
                }
            }
        }
    }
    points.sort();
    points.dedup();
    points
}

#[nativelink_test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_pushed_via_load_balanced_channel() -> Result<(), Box<dyn core::error::Error>> {
    let (port, received) = spawn_test_collector().await?;

    // DirectIpResolver bypasses hickory-dns / /etc/resolv.conf so this test
    // also covers sandboxed builds (e.g. Nix) where DNS is unavailable.
    let channel = LoadBalancedChannel::builder(("127.0.0.1", port))
        .lookup_service(DirectIpResolver)
        .channel()
        .await
        .expect("LoadBalancedChannel with DirectIpResolver should succeed");

    let exporter = MetricExporter::builder()
        .with_tonic()
        .with_channel(channel.into())
        .with_protocol(Protocol::Grpc)
        .build()?;

    let points = extract_sum_data_points(&export_counters_and_drain(exporter, &received).await?);
    assert!(
        points.iter().any(|(name, ..)| name == "test.operations"),
        "Counter 'test.operations' should have been delivered via gRPC, got: {points:?}"
    );
    Ok(())
}

// The data points `export_counters_and_drain` is expected to deliver,
// reduced with `extract_sum_data_points`.  Both the with- and without-
// `NL_OTEL_ENDPOINT` tests assert against this same value, which checks
// that the two export paths produce identical OTLP payloads.
fn expected_sum_data_points() -> Vec<(String, String, Vec<String>, String)> {
    use opentelemetry_proto::tonic::common::v1::{AnyValue, KeyValue as ProtoKeyValue, any_value};
    use opentelemetry_proto::tonic::metrics::v1::number_data_point;

    let attribute = |key: &str, value: &str| {
        format!(
            "{:?}",
            ProtoKeyValue {
                key: key.to_string(),
                value: Some(AnyValue {
                    value: Some(any_value::Value::StringValue(value.to_string())),
                }),
                key_strindex: 0,
            }
        )
    };
    let value = |v: i64| format!("{:?}", Some(number_data_point::Value::AsInt(v)));

    let mut points = vec![
        (
            "test.operations".to_string(),
            "Total test operations".to_string(),
            vec![attribute("operation", "cache_read")],
            value(3),
        ),
        (
            "test.operations".to_string(),
            "Total test operations".to_string(),
            vec![attribute("operation", "cache_write")],
            value(7),
        ),
    ];
    points.sort();
    points
}

// Without `NL_OTEL_ENDPOINT` the exporter creates its own channel.
#[serial(env)]
#[nativelink_test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_are_tracked_without_otel_endpoint() -> Result<(), Box<dyn core::error::Error>> {
    let (port, received) = spawn_test_collector().await?;

    // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
    unsafe { env::remove_var(NL_OTEL_ENDPOINT) };
    assert!(
        maybe_load_balanced_channel().await.is_none(),
        "Expected no channel when {NL_OTEL_ENDPOINT} is unset"
    );
    let exporter = MetricExporter::builder()
        .with_tonic()
        .with_endpoint(format!("http://127.0.0.1:{port}"))
        .with_protocol(Protocol::Grpc)
        .build()?;
    let points = extract_sum_data_points(&export_counters_and_drain(exporter, &received).await?);

    assert_eq!(
        points,
        expected_sum_data_points(),
        "Metric export without {NL_OTEL_ENDPOINT} must deliver the expected OTLP payload"
    );
    Ok(())
}

// With `NL_OTEL_ENDPOINT` set the exporter goes through the ginepro
// load-balanced channel.
#[serial(env)]
#[nativelink_test(flavor = "multi_thread", worker_threads = 2)]
async fn metrics_are_tracked_with_otel_endpoint() -> Result<(), Box<dyn core::error::Error>> {
    // The load-balanced channel needs ginepro's default DNS resolver.
    if !dns_configured() {
        eprintln!(
            "Skipping metrics_are_tracked_with_otel_endpoint: no DNS configuration \
             available (e.g. sandboxed Nix build)"
        );
        return Ok(());
    }

    let (port, received) = spawn_test_collector().await?;

    // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
    unsafe { env::set_var(NL_OTEL_ENDPOINT, format!("http://127.0.0.1:{port}")) };
    let maybe_channel = maybe_load_balanced_channel().await;
    // SAFETY: `#[serial(env)]` serializes all env-var writes across tests.
    unsafe { env::remove_var(NL_OTEL_ENDPOINT) };
    let channel = maybe_channel.expect("Expected a channel when NL_OTEL_ENDPOINT is set");
    let exporter = MetricExporter::builder()
        .with_tonic()
        .with_channel(channel.into())
        .with_protocol(Protocol::Grpc)
        .build()?;
    let points = extract_sum_data_points(&export_counters_and_drain(exporter, &received).await?);

    assert_eq!(
        points,
        expected_sum_data_points(),
        "Metric export with {NL_OTEL_ENDPOINT} must deliver the expected OTLP payload"
    );
    Ok(())
}
