// Copyright 2023 The NativeLink Authors. All rights reserved.
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

use std::sync::Arc;

use async_lock::{Mutex as AsyncMutex, MutexGuard};
use futures::StreamExt;
use hyper::{Response, StatusCode};
use nativelink_error::Error;
use nativelink_util::health_utils::{
    HealthRegistryBuilder, HealthStatus, HealthStatusDescription, HealthStatusReporter,
};

#[derive(Clone)]
pub struct HealthServer {
    health_registry_builder: Arc<AsyncMutex<HealthRegistryBuilder>>,
}

impl HealthServer {
    pub async fn new(namespace: &str) -> Result<Self, Error> {
        let namespace = namespace.to_string();
        let health_registry_builder = Arc::new(AsyncMutex::new(HealthRegistryBuilder::new(
            namespace.into(),
        )));

        Ok(HealthServer {
            health_registry_builder,
        })
    }

    pub async fn get_health_registry(&self) -> Result<MutexGuard<HealthRegistryBuilder>, Error> {
        let health_registry_lock = self.health_registry_builder.lock().await;
        Ok(health_registry_lock)
    }

    pub async fn check_health_status(&self, json_content_type: &'static str) -> Response<String> {
        let health_registry_status = self.get_health_registry().await.unwrap().build();
        let health_status_descriptions: Vec<HealthStatusDescription> = health_registry_status
            .health_status_report()
            .collect()
            .await;

        match serde_json5::to_string(&health_status_descriptions) {
            Ok(body) => {
                let contains_failed_report = health_status_descriptions
                    .iter()
                    .any(|description| matches!(description.status, HealthStatus::Failed { .. }));
                let status_code = if contains_failed_report {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::OK
                };

                Response::builder()
                    .status(status_code)
                    .header(
                        hyper::header::CONTENT_TYPE,
                        hyper::header::HeaderValue::from_static(json_content_type),
                    )
                    .body(body)
                    .unwrap()
            }

            Err(e) => Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header(
                    hyper::header::CONTENT_TYPE,
                    hyper::header::HeaderValue::from_static(json_content_type),
                )
                .body(format!("Internal Failure: {e:?}"))
                .unwrap(),
        }
    }
}
