// Copyright 2026 The NativeLink Authors. All rights reserved.
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

use core::time::Duration;
use std::borrow::Cow;
use std::sync::Arc;

use axum::http::Request;
use hyper::StatusCode;
use nativelink_config::cas_server::HealthConfig;
use nativelink_macro::nativelink_test;
use nativelink_service::health_server::{HealthServer, health_paths};
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

    fn struct_name(&self) -> &'static str {
        "TestIndicator"
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
        "struct_name": "TestIndicator",
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

    fn struct_name(&self) -> &'static str {
        "TestSleepIndicator"
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
        "struct_name": "TestSleepIndicator"
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
        "Timeout during health check struct_name=\"TestSleepIndicator\""
    ));
    Ok(())
}

struct InitializingIndicator {}

#[async_trait]
impl HealthStatusIndicator for InitializingIndicator {
    fn get_name(&self) -> &'static str {
        "initializing_indicator"
    }

    async fn check_health(&self, _namespace: Cow<'static, str>) -> HealthStatus {
        HealthStatus::Initializing {
            struct_name: "InitializingIndicator",
            message: "not yet".into(),
        }
    }

    fn struct_name(&self) -> &'static str {
        "InitializingIndicator"
    }
}

async fn status_of(server: HealthServer) -> Result<StatusCode, Box<dyn core::error::Error>> {
    let tonic_services = Routes::builder().routes();
    let mut svc = tonic_services
        .into_axum_router()
        .route_service("/probe", server);
    let request = Request::builder()
        .method("GET")
        .uri("/probe")
        .body(Body::empty())?;
    let response: hyper::Response<axum::body::Body> =
        svc.as_service().ready().await?.call(request).await?;
    Ok(response.status())
}

/// A component still initializing is healthy enough for `/status` and not
/// ready enough for `/ready`.
#[nativelink_test]
async fn readiness_is_unavailable_while_initializing() -> Result<(), Box<dyn core::error::Error>> {
    let mut health_registry_builder = HealthRegistryBuilder::new("foo");
    health_registry_builder.register_indicator(Arc::new(InitializingIndicator {}));
    let health_registry = health_registry_builder.build();
    let config = HealthConfig::default();

    assert_eq!(
        status_of(HealthServer::new(health_registry.clone(), &config)).await?,
        StatusCode::OK
    );
    assert_eq!(
        status_of(HealthServer::readiness(health_registry, &config)).await?,
        StatusCode::SERVICE_UNAVAILABLE
    );
    Ok(())
}

/// Defaults fill the two paths; a configuration that gives both the same
/// path is refused with a message rather than left to panic the router.
#[nativelink_test]
async fn health_paths_are_distinct_or_refused() -> Result<(), Box<dyn core::error::Error>> {
    assert_eq!(
        health_paths(&HealthConfig::default())?,
        ("/status".to_string(), "/ready".to_string())
    );
    let custom = HealthConfig {
        path: "/healthz".to_string(),
        readiness_path: "/readyz".to_string(),
        ..HealthConfig::default()
    };
    assert_eq!(
        health_paths(&custom)?,
        ("/healthz".to_string(), "/readyz".to_string())
    );
    let clash = HealthConfig {
        path: "/ready".to_string(),
        ..HealthConfig::default()
    };
    let err = health_paths(&clash).expect_err("the same path for both must be refused");
    assert!(err.to_string().contains("both /ready"), "{err}");
    Ok(())
}
