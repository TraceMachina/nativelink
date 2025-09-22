// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use core::convert::Infallible;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};

use axum::body::Body;
use bytes::Bytes;
use futures::StreamExt;
use http_body_util::Full;
use hyper::header::{CONTENT_TYPE, HeaderValue};
use hyper::{Request, Response, StatusCode};
use nativelink_util::health_utils::{
    HealthRegistry, HealthStatus, HealthStatusDescription, HealthStatusReporter,
};
use tower::Service;
use tracing::error_span;

/// Content type header value for JSON.
const JSON_CONTENT_TYPE: &str = "application/json; charset=utf-8";

#[derive(Debug, Clone)]
pub struct HealthServer {
    health_registry: HealthRegistry,
}

impl HealthServer {
    pub const fn new(health_registry: HealthRegistry) -> Self {
        Self { health_registry }
    }
}

impl Service<Request<Body>> for HealthServer {
    type Response = Response<Full<Bytes>>;
    type Error = Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _req: Request<Body>) -> Self::Future {
        let health_registry = self.health_registry.clone();
        Box::pin(error_span!("health_server_call").in_scope(|| async move {
            let health_status_descriptions: Vec<HealthStatusDescription> =
                health_registry.health_status_report().collect().await;

            match serde_json5::to_string(&health_status_descriptions) {
                Ok(body) => {
                    let contains_failed_report =
                        health_status_descriptions.iter().any(|description| {
                            matches!(description.status, HealthStatus::Failed { .. })
                        });
                    let status_code = if contains_failed_report {
                        StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        StatusCode::OK
                    };

                    Ok(Response::builder()
                        .status(status_code)
                        .header(CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE))
                        .body(Full::new(Bytes::from(body)))
                        .unwrap())
                }

                Err(e) => Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header(CONTENT_TYPE, HeaderValue::from_static(JSON_CONTENT_TYPE))
                    .body(Full::new(Bytes::from(format!("Internal Failure: {e:?}"))))
                    .unwrap()),
            }
        }))
    }
}
