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

//! `cache.size` and `cache.entries` are read back through a real exporter.
//!
//! This file is its own test binary, so the meter provider installed here is
//! in place before `CACHE_METRICS` is first touched.

use core::convert::Infallible;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context as TaskContext, Poll};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use hyper::{Request, Response};
use nativelink_config::stores::EvictionPolicy;
use nativelink_macro::nativelink_test;
use nativelink_util::common::DigestInfo;
use nativelink_util::evicting_map::{EvictingMap, LenEntry};
use nativelink_util::instant_wrapper::MockInstantWrapped;
use opentelemetry::{KeyValue, global};
use opentelemetry_otlp::{MetricExporter, Protocol, WithExportConfig};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use opentelemetry_proto::tonic::common::v1::any_value;
use opentelemetry_proto::tonic::metrics::v1::{metric, number_data_point};
use opentelemetry_sdk::metrics::SdkMeterProvider;
use pretty_assertions::assert_eq;
use tokio_stream::wrappers::TcpListenerStream;
use tonic::body::Body;
use tower::Service;

#[derive(Clone, PartialEq, Eq, Debug)]
struct BytesWrapper(Bytes);

impl LenEntry for BytesWrapper {
    fn len(&self) -> u64 {
        Bytes::len(&self.0) as u64
    }

    fn is_empty(&self) -> bool {
        Bytes::is_empty(&self.0)
    }
}

fn blob(len: usize) -> BytesWrapper {
    BytesWrapper(Bytes::from(vec![0u8; len]))
}

const HASH1: &str = "0123456789abcdef000000000000000000000000000000000123456789abcdef";
const HASH2: &str = "123456789abcdef000000000000000000000000000000000123456789abcdef1";
const HASH3: &str = "23456789abcdef000000000000000000000000000000000123456789abcdef12";
const HASH4: &str = "3456789abcdef000000000000000000000000000000000123456789abcdef012";

const METRICS_EXPORT_PATH: &str = "/opentelemetry.proto.collector.metrics.v1.MetricsService/Export";

/// A minimal in-process OTLP metrics collector, as in `telemetry_test.rs`.
#[derive(Clone)]
struct TestMetricsService {
    received: Arc<Mutex<Vec<ExportMetricsServiceRequest>>>,
}

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
            if req.uri().path() != METRICS_EXPORT_PATH {
                let mut resp = Response::new(Body::empty());
                resp.headers_mut().insert(
                    tonic::Status::GRPC_STATUS,
                    (tonic::Code::Unimplemented as i32).into(),
                );
                return Ok(resp);
            }
            let export_svc = tower::service_fn(
                move |request: tonic::Request<ExportMetricsServiceRequest>| {
                    let received = received.clone();
                    async move {
                        received.lock().unwrap().push(request.into_inner());
                        Ok::<_, tonic::Status>(tonic::Response::new(
                            ExportMetricsServiceResponse::default(),
                        ))
                    }
                },
            );
            let mut grpc = tonic::server::Grpc::new(tonic_prost::ProstCodec::<
                ExportMetricsServiceResponse,
                ExportMetricsServiceRequest,
            >::default());
            Ok(grpc.unary(export_svc, req).await)
        })
    }
}

impl tonic::server::NamedService for TestMetricsService {
    const NAME: &'static str = "opentelemetry.proto.collector.metrics.v1.MetricsService";
}

async fn spawn_test_collector()
-> Result<(u16, Arc<Mutex<Vec<ExportMetricsServiceRequest>>>), Box<dyn core::error::Error>> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let received = Arc::new(Mutex::new(vec![]));
    let svc = TestMetricsService {
        received: received.clone(),
    };
    nativelink_util::background_spawn!("otlp_test_collector", async move {
        tonic::transport::Server::builder()
            .add_service(svc)
            .serve_with_incoming(TcpListenerStream::new(listener))
            .await
            .ok();
    });
    Ok((port, received))
}

/// The last exported value of the sum `name` for `cache.type = cache_type`.
fn last_sum_value(
    requests: &[ExportMetricsServiceRequest],
    name: &str,
    cache_type: &str,
) -> Option<i64> {
    let mut last = None;
    for request in requests {
        for resource_metrics in &request.resource_metrics {
            for scope_metrics in &resource_metrics.scope_metrics {
                for m in scope_metrics.metrics.iter().filter(|m| m.name == name) {
                    let Some(metric::Data::Sum(sum)) = &m.data else {
                        continue;
                    };
                    for point in &sum.data_points {
                        let matches = point.attributes.iter().any(|kv| {
                            kv.key == "cache.type"
                                && kv.value.as_ref().and_then(|v| v.value.as_ref())
                                    == Some(&any_value::Value::StringValue(cache_type.to_string()))
                        });
                        if let (true, Some(number_data_point::Value::AsInt(v))) =
                            (matches, point.value)
                        {
                            last = Some(v);
                        }
                    }
                }
            }
        }
    }
    last
}

#[nativelink_test(flavor = "multi_thread", worker_threads = 2)]
async fn cache_size_metrics_follow_inserts_replacements_evictions_and_removals()
-> Result<(), Box<dyn core::error::Error>> {
    let (port, received) = spawn_test_collector().await?;
    let exporter = MetricExporter::builder()
        .with_tonic()
        .with_endpoint(format!("http://127.0.0.1:{port}"))
        .with_protocol(Protocol::Grpc)
        .build()?;
    let meter_provider = SdkMeterProvider::builder()
        .with_periodic_exporter(exporter)
        .build();
    global::set_meter_provider(meter_provider.clone());

    let evicting_map = EvictingMap::<DigestInfo, DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 3,
            max_seconds: 0,
            max_bytes: 0,
            evict_bytes: 0,
        },
        MockInstantWrapped::default(),
    );
    let key1 = DigestInfo::try_new(HASH1, 0)?;
    let key2 = DigestInfo::try_new(HASH2, 0)?;
    let key3 = DigestInfo::try_new(HASH3, 0)?;
    let key4 = DigestInfo::try_new(HASH4, 0)?;

    // Stored before metrics are enabled, so reported by the enable itself.
    evicting_map.insert(key1, blob(10)).await;
    evicting_map.insert(key2, blob(20)).await;
    let attrs = vec![KeyValue::new("cache.type", "test_cache")];
    evicting_map.enable_cache_size_metrics(attrs.clone());
    // A second enable must not count the same entries again.
    evicting_map.enable_cache_size_metrics(attrs);

    evicting_map.insert(key3, blob(5)).await; // 35 bytes, 3 entries
    evicting_map.insert(key2, blob(7)).await; // replace 20 with 7: 22 bytes, 3 entries
    evicting_map.insert(key4, blob(1)).await; // over max_count, evicts key1: 13 bytes, 3 entries
    assert!(evicting_map.remove(&key3).await); // 8 bytes, 2 entries

    meter_provider.force_flush()?;
    let requests = received.lock().unwrap().split_off(0);
    meter_provider.shutdown()?;

    assert_eq!(
        last_sum_value(&requests, "cache.size", "test_cache"),
        Some(8)
    );
    assert_eq!(
        last_sum_value(&requests, "cache.entries", "test_cache"),
        Some(2)
    );
    Ok(())
}
