use std::sync::{Arc, Mutex};

use axum::extract::{FromRequestParts, State};
use axum::http::request::Parts;
use axum::middleware::Next;
use axum::{Router, middleware};
use hyper::{Request, Response, StatusCode, Uri};
use nativelink_macro::nativelink_test;
use nativelink_util::telemetry::ClientHeaders;
use opentelemetry::baggage::BaggageExt;
use opentelemetry::{Context, KeyValue, global};
use opentelemetry_http::HeaderExtractor as OTELHeaderExtractor;
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
