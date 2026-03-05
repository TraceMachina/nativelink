use axum::Router;
use hyper::{Request, StatusCode, Uri};
use nativelink_macro::nativelink_test;
use opentelemetry::baggage::BaggageExt;
use opentelemetry::{Context, KeyValue};
use tonic::body::Body;
use tonic::service::Routes;
use tower::{Service, ServiceExt};
use tracing::warn;

fn demo_service() -> Router {
    let tonic_services = Routes::builder().routes();
    tonic_services
        .into_axum_router()
        .fallback(|uri: Uri| async move {
            warn!("No route for {uri}");
            (StatusCode::NOT_FOUND, format!("No route for {uri}"))
        })
        .layer(nativelink_util::telemetry::OtlpLayer::new(false))
}

async fn run_request(
    svc: &mut Router,
    request: Request<Body>,
) -> Result<(), Box<dyn core::error::Error>> {
    let response: hyper::Response<axum::body::Body> =
        svc.as_service().ready().await?.call(request).await?;
    assert_eq!(response.status(), 404);

    let response = String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await?
            .to_vec(),
    )?;
    assert_eq!(response, String::from("No route for /demo"));
    Ok(())
}

#[nativelink_test]
async fn oltp_logs_no_baggage() -> Result<(), Box<dyn core::error::Error>> {
    let mut svc = demo_service();

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
    let mut svc = demo_service();

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
