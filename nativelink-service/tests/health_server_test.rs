use core::time::Duration;
use std::borrow::Cow;
use std::sync::Arc;

use axum::http::Request;
use hyper::StatusCode;
use nativelink_config::cas_server::HealthConfig;
use nativelink_macro::nativelink_test;
use nativelink_service::health_server::HealthServer;
use nativelink_util::health_utils::{
    HealthRegistry, HealthRegistryBuilder, HealthStatus, HealthStatusIndicator,
};
use pretty_assertions::assert_eq;
use serde_json::Value;
use tonic::async_trait;
use tonic::body::Body;
use tonic::service::Routes;
use tower::{Service, ServiceExt};

async fn health_tester(
    health_registry: HealthRegistry,
    expected_status_code: StatusCode,
    expected_result: &str,
    config: HealthConfig,
) -> Result<(), Box<dyn core::error::Error>> {
    let health_server = HealthServer::new(health_registry, &config);

    let tonic_services = Routes::builder().routes();

    let mut svc = tonic_services
        .into_axum_router()
        .route_service("/status", health_server);

    let request = Request::builder()
        .method("GET")
        .uri("/status")
        .body(Body::empty())?;
    let response: hyper::Response<axum::body::Body> =
        svc.as_service().ready().await?.call(request).await?;
    assert_eq!(response.status(), expected_status_code);

    let raw_json = String::from_utf8(
        axum::body::to_bytes(response.into_body(), usize::MAX)
            .await?
            .to_vec(),
    )?;
    let parsed_json: Value = serde_json::from_str(&raw_json)?;
    assert_eq!(
        serde_json::to_string_pretty(&parsed_json)?,
        String::from(expected_result)
    );
    Ok(())
}

#[nativelink_test]
async fn basic_health_test() -> Result<(), Box<dyn core::error::Error>> {
    let health_registry = HealthRegistryBuilder::new("foo").build();
    health_tester(
        health_registry,
        StatusCode::OK,
        "[]",
        HealthConfig::default(),
    )
    .await
}

struct TestIndicator {}

#[async_trait]
impl HealthStatusIndicator for TestIndicator {
    fn get_name(&self) -> &'static str {
        "test_indicator"
    }

    async fn check_health(&self, _namespace: Cow<'static, str>) -> HealthStatus {
        HealthStatus::Ok {
            struct_name: self.struct_name(),
            message: Cow::Borrowed("all good"),
        }
    }
}

#[nativelink_test]
async fn health_test_with_item() -> Result<(), Box<dyn core::error::Error>> {
    let mut health_registry_builder = HealthRegistryBuilder::new("foo");
    health_registry_builder.register_indicator(Arc::new(TestIndicator {}));
    let health_registry = health_registry_builder.build();
    health_tester(
        health_registry,
        StatusCode::OK,
        r#"[
  {
    "namespace": "/foo/test_indicator",
    "status": {
      "Ok": {
        "struct_name": "integration_tests_health_server_test_test::TestIndicator",
        "message": "all good"
      }
    }
  }
]"#,
        HealthConfig::default(),
    )
    .await
}

struct TestSleepIndicator {}

#[async_trait]
impl HealthStatusIndicator for TestSleepIndicator {
    fn get_name(&self) -> &'static str {
        "test_sleep_indicator"
    }

    async fn check_health(&self, _namespace: Cow<'static, str>) -> HealthStatus {
        tokio::time::sleep(Duration::MAX).await;
        unreachable!("Because we sleep forever");
    }
}

#[nativelink_test]
async fn health_test_with_sleep() -> Result<(), Box<dyn core::error::Error>> {
    let mut health_registry_builder = HealthRegistryBuilder::new("foo");
    health_registry_builder.register_indicator(Arc::new(TestSleepIndicator {}));
    let health_registry = health_registry_builder.build();
    health_tester(
        health_registry,
        StatusCode::SERVICE_UNAVAILABLE,
        r#"[
  {
    "namespace": "/foo/test_sleep_indicator",
    "status": {
      "Timeout": {
        "struct_name": "integration_tests_health_server_test_test::TestSleepIndicator"
      }
    }
  }
]"#,
        HealthConfig {
            timeout_seconds: 1,
            ..Default::default()
        },
    )
    .await?;
    assert!(logs_contain(
        "Timeout during health check struct_name=\"integration_tests_health_server_test_test::TestSleepIndicator\""
    ));
    Ok(())
}
