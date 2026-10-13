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

//! A `grpc` scheduler in front of an in-process `ExecutionServer`: Execute,
//! `WaitExecution` and `GetCapabilities` reach the upstream with the client's
//! identity and nominated headers, and the operation name Execute hands back
//! resolves again on `WaitExecution`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use futures::{Future, StreamExt, stream};
use hyper_util::rt::TokioIo;
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use nativelink_config::cas_server::{ExecutionConfig, WithInstanceName};
use nativelink_config::schedulers::GrpcSpec;
use nativelink_config::stores::{GrpcEndpoint, MemorySpec, Retry, StoreSpec};
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::{MetricsComponent, RootMetricsComponent};
use nativelink_proto::build::bazel::remote::execution::v2::capabilities_server::{
    Capabilities, CapabilitiesServer,
};
use nativelink_proto::build::bazel::remote::execution::v2::execution_server::Execution;
use nativelink_proto::build::bazel::remote::execution::v2::{
    Action, Command, ExecuteRequest, ExecutionCapabilities, GetCapabilitiesRequest,
    ServerCapabilities, WaitExecutionRequest, digest_function,
};
use nativelink_scheduler::grpc_scheduler::GrpcScheduler;
use nativelink_scheduler::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_service::execution_server::ExecutionServer;
use nativelink_store::ac_utils::serialize_and_upload_message;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::action_messages::{ActionInfo, ActionStage, ActionState, OperationId};
use nativelink_util::background_spawn;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, OperationFilter,
};
use nativelink_util::origin_event::OriginMetadata;
use nativelink_util::store_trait::StoreLike;
use nativelink_util::telemetry::{ClientHeaders, OtlpLayer, init_propagator};
use opentelemetry::KeyValue;
use opentelemetry::baggage::BaggageExt;
use opentelemetry::context::{Context, FutureExt};
use opentelemetry_semantic_conventions::attribute::ENDUSER_ID;
use parking_lot::Mutex;
use tonic::{Request, Response, Status};

const INSTANCE_NAME: &str = "main";
const UPSTREAM_OPERATION_ID: &str = "op-1";
const IDENTITY: &str = "alice";
const TOKEN: &str = "Bearer token";

/// What the upstream saw on one call, read from the context the server's
/// middleware built out of the request headers.
#[derive(Debug)]
struct Seen {
    rpc: &'static str,
    identity: Option<String>,
    authorization: Option<String>,
}

fn observe(rpc: &'static str) -> Seen {
    let context = Context::current();
    Seen {
        rpc,
        identity: context
            .baggage()
            .get(ENDUSER_ID)
            .map(|value| value.as_str().to_string()),
        authorization: context
            .get::<ClientHeaders>()
            .and_then(|headers| headers.0.get("authorization").cloned()),
    }
}

/// Holds an action queued forever: the first `changed` hands out the queued
/// state, later ones never resolve, as a scheduler with no free worker.
struct QueuedForever {
    state: Arc<ActionState>,
    reported: bool,
}

#[async_trait]
impl ActionStateResult for QueuedForever {
    async fn as_state(&self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        Ok((self.state.clone(), None))
    }

    async fn changed(&mut self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        if !self.reported {
            self.reported = true;
            return Ok((self.state.clone(), None));
        }
        futures::future::pending().await
    }

    async fn as_action_info(&self) -> Result<(Arc<ActionInfo>, Option<OriginMetadata>), Error> {
        Err(make_err!(Code::Unimplemented, "not needed here"))
    }
}

/// The upstream's scheduler. It records every call with what the context
/// carried and keeps the one action it is given queued.
#[derive(Debug, Default, MetricsComponent)]
struct RecordingScheduler {
    seen: Mutex<Vec<Seen>>,
    queued: Mutex<Option<Arc<ActionState>>>,
}

impl RootMetricsComponent for RecordingScheduler {}

#[async_trait]
impl ClientStateManager for RecordingScheduler {
    async fn add_action(
        &self,
        _client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        self.seen.lock().push(observe("add_action"));
        let state = Arc::new(ActionState {
            client_operation_id: OperationId::from(UPSTREAM_OPERATION_ID),
            stage: ActionStage::Queued,
            action_digest: action_info.digest(),
            last_transition_timestamp: SystemTime::now(),
        });
        *self.queued.lock() = Some(state.clone());
        Ok(Box::new(QueuedForever {
            state,
            reported: false,
        }))
    }

    async fn filter_operations<'a>(
        &'a self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream<'a>, Error> {
        self.seen.lock().push(observe("filter_operations"));
        let state = self.queued.lock().clone();
        let Some(state) = state.filter(|state| {
            filter.client_operation_id.as_ref() == Some(&state.client_operation_id)
        }) else {
            return Ok(stream::empty().boxed());
        };
        let result: Box<dyn ActionStateResult> = Box::new(QueuedForever {
            state,
            reported: false,
        });
        Ok(stream::once(async move { result }).boxed())
    }
}

#[async_trait]
impl KnownPlatformPropertyProvider for RecordingScheduler {
    async fn get_known_properties(&self, _instance_name: &str) -> Result<Vec<String>, Error> {
        Ok(vec!["cpu_count".to_string()])
    }
}

/// A capabilities service that records the call. The real one asks its
/// scheduler at construction, not per request, so it would see no context.
#[derive(Clone)]
struct RecordingCapabilities(Arc<RecordingScheduler>);

#[tonic::async_trait]
impl Capabilities for RecordingCapabilities {
    async fn get_capabilities(
        &self,
        _request: Request<GetCapabilitiesRequest>,
    ) -> Result<Response<ServerCapabilities>, Status> {
        self.0.seen.lock().push(observe("get_capabilities"));
        Ok(Response::new(ServerCapabilities {
            execution_capabilities: Some(ExecutionCapabilities {
                supported_node_properties: vec!["cpu_count".to_string()],
                ..Default::default()
            }),
            ..Default::default()
        }))
    }
}

#[derive(Clone)]
struct Executor;

impl<F> hyper::rt::Executor<F> for Executor
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn execute(&self, fut: F) {
        background_spawn!("test_executor", fut);
    }
}

fn make_execution_server(
    store_manager: &StoreManager,
    scheduler: Arc<dyn KnownPlatformPropertyProvider>,
) -> Result<ExecutionServer, Error> {
    ExecutionServer::new(
        &[WithInstanceName {
            instance_name: INSTANCE_NAME.to_string(),
            config: ExecutionConfig {
                experimental_buck2_invocation_isolation: false,
                cas_store: "main_cas".to_string(),
                scheduler: "scheduler".to_string(),
            },
        }],
        &HashMap::from([("scheduler".to_string(), scheduler)]),
        store_manager,
    )
}

/// Serves the upstream the way the binary does: behind the OTLP middleware,
/// which turns the request headers into the context the handlers read.
async fn serve_upstream(
    store_manager: &StoreManager,
    scheduler: Arc<RecordingScheduler>,
) -> Result<u16, Error> {
    let execution_server = make_execution_server(store_manager, scheduler.clone())?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let router = tonic::service::Routes::new(execution_server.into_service())
        .add_service(CapabilitiesServer::new(RecordingCapabilities(scheduler)))
        .into_axum_router()
        .layer(OtlpLayer::new(false));
    let service = TowerToHyperService::new(router);
    background_spawn!("upstream_accept", async move {
        while let Ok((stream, _)) = listener.accept().await {
            let service = service.clone();
            background_spawn!("upstream_conn", async move {
                drop(
                    auto::Builder::new(Executor)
                        .serve_connection(TokioIo::new(stream), service)
                        .await,
                );
            });
        }
    });
    Ok(port)
}

#[nativelink_test]
async fn grpc_scheduler_forwards_the_client_and_resolves_its_operation() -> Result<(), Error> {
    init_propagator();
    let store_manager = Arc::new(StoreManager::new());
    store_manager.add_store(
        "main_cas",
        store_factory(
            &StoreSpec::Memory(MemorySpec::default()),
            &store_manager,
            None,
        )
        .await?,
    )?;
    let cas_store = store_manager.get_store("main_cas").unwrap();
    let command_digest = serialize_and_upload_message(
        &Command::default(),
        cas_store.as_pin(),
        &mut DigestHasherFunc::Sha256.hasher(),
    )
    .await?;
    let action_digest = serialize_and_upload_message(
        &Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(command_digest.into()),
            ..Default::default()
        },
        cas_store.as_pin(),
        &mut DigestHasherFunc::Sha256.hasher(),
    )
    .await?;

    let upstream = Arc::new(RecordingScheduler::default());
    let port = serve_upstream(&store_manager, upstream.clone()).await?;
    let scheduler = Arc::new(GrpcScheduler::new(&GrpcSpec {
        endpoint: GrpcEndpoint {
            address: format!("http://127.0.0.1:{port}"),
            tls_config: None,
            concurrency_limit: None,
            connect_timeout_s: 0,
            tcp_keepalive_s: 0,
            http2_keepalive_interval_s: 0,
            http2_keepalive_timeout_s: 0,
        },
        retry: Retry::default(),
        max_concurrent_requests: 0,
        connections_per_endpoint: 0,
        // Mixed case on purpose, header names are case-insensitive.
        forward_headers: vec!["Authorization".to_string()],
    })?);
    let front = make_execution_server(&store_manager, scheduler.clone())?;

    // The context the OTLP middleware would have built for the client's call.
    let client = Context::current_with_baggage(vec![KeyValue::new(ENDUSER_ID, IDENTITY)])
        .with_value(ClientHeaders(Arc::new(HashMap::from([(
            "authorization".to_string(),
            TOKEN.to_string(),
        )]))));

    let known = scheduler
        .get_known_properties(INSTANCE_NAME)
        .with_context(client.clone())
        .await?;
    assert_eq!(known, ["cpu_count"]);

    let mut execute_stream = front
        .execute(Request::new(ExecuteRequest {
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: digest_function::Value::Sha256.into(),
            skip_cache_lookup: false,
            action_digest: Some(action_digest.into()),
            execution_policy: None,
            results_cache_policy: None,
        }))
        .with_context(client.clone())
        .await?
        .into_inner();
    let started = execute_stream
        .next()
        .await
        .expect("Execute streams the queued operation")?;
    assert_eq!(
        started.name,
        format!("{INSTANCE_NAME}/{INSTANCE_NAME}/{UPSTREAM_OPERATION_ID}"),
        "the name carries the upstream's instance inside this server's"
    );
    assert!(!started.done);

    let mut wait_stream = front
        .wait_execution(Request::new(WaitExecutionRequest {
            name: started.name.clone(),
        }))
        .with_context(client)
        .await?
        .into_inner();
    let resumed = wait_stream
        .next()
        .await
        .expect("WaitExecution streams the same operation")?;
    assert_eq!(resumed.name, started.name);
    assert!(!resumed.done);
    assert_eq!(resumed.metadata, started.metadata);

    let seen = upstream.seen.lock();
    assert_eq!(
        seen.iter().map(|call| call.rpc).collect::<Vec<_>>(),
        ["get_capabilities", "add_action", "filter_operations"]
    );
    for call in seen.iter() {
        assert_eq!(call.identity.as_deref(), Some(IDENTITY), "{call:?}");
        assert_eq!(call.authorization.as_deref(), Some(TOKEN), "{call:?}");
    }
    Ok(())
}
