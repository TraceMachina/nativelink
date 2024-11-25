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

use std::sync::Arc;
use std::time::Duration;

use nativelink_error::{make_err, Code, Error};
use nativelink_metric::{MetricFieldData, MetricKind, MetricsComponent, RootMetricsComponent};
use nativelink_metric_collector::{otel_export, MetricsCollectorLayer};
use nativelink_util::spawn_blocking;
use opentelemetry::metrics::MeterProvider;
use opentelemetry_otlp::WithExportConfig;
use opentelemetry_sdk::metrics::SdkMeterProvider;
use opentelemetry_sdk::runtime;
use parking_lot::RwLock;
use tracing::error;
use tracing_subscriber::prelude::__tracing_subscriber_SubscriberExt;

pub struct OtlpServer {
    root_metrics: Arc<RwLock<dyn RootMetricsComponent>>,
    provider: SdkMeterProvider,
}

impl OtlpServer {
    pub fn new(
        root_metrics: Arc<RwLock<dyn RootMetricsComponent>>,
        endpoint: &str,
    ) -> Result<Self, Error> {
        let pipeline = opentelemetry_otlp::new_pipeline()
            .metrics(runtime::Tokio)
            .with_exporter(
                opentelemetry_otlp::new_exporter()
                    .tonic()
                    .with_endpoint(endpoint)
                    .with_timeout(Duration::from_secs(10)),
            );

        let provider = pipeline
            .build()
            .map_err(|e| make_err!(Code::Internal, "{e}"))?;

        Ok(Self {
            root_metrics,
            provider,
        })
    }

    pub async fn collect_and_export(&self) -> Result<(), Error> {
        let meter = self.provider.meter("nativelink");
        let metrics = self.root_metrics.clone();

        spawn_blocking!("otlp_metrics", move || {
            let (layer, output_metrics) = MetricsCollectorLayer::new();
            tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
                let metrics_component = metrics.read();
                MetricsComponent::publish(
                    &*metrics_component,
                    MetricKind::Component,
                    MetricFieldData::default(),
                )
            })
            .map_err(|e| make_err!(Code::Internal, "{e}"))?;

            otel_export("nativelink".to_string(), &meter, &output_metrics.lock());

            Ok::<(), Error>(())
        })
        .await
        .map_err(|e| make_err!(Code::Internal, "Join error: {}", e))?
    }

    pub async fn run_export_loop(self) {
        loop {
            tokio::time::sleep(Duration::from_secs(15)).await;
            if let Err(e) = self.collect_and_export().await {
                error!("Failed to export OTLP metrics: {e}");
            }
        }
    }
}
