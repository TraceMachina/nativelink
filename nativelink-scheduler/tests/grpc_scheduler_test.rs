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

use core::pin::Pin;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;
use std::sync::Arc;
use std::time::SystemTime;

use futures::{Stream, StreamExt, stream};
use nativelink_config::schedulers::GrpcSpec;
use nativelink_config::stores::{GrpcEndpoint, Retry};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::execution_server::{
    Execution, ExecutionServer,
};
use nativelink_proto::build::bazel::remote::execution::v2::{ExecuteRequest, WaitExecutionRequest};
use nativelink_proto::google::longrunning::Operation;
use nativelink_scheduler::grpc_scheduler::GrpcScheduler;
use nativelink_util::action_messages::{ActionStage, ActionState, OperationId};
use nativelink_util::background_spawn;
use nativelink_util::common::DigestInfo;
use nativelink_util::operation_state_manager::{ClientStateManager, OperationFilter};
use tonic::transport::Server;
use tonic::transport::server::TcpIncoming;
use tonic::{Request, Response, Status};

type OperationStream = Pin<Box<dyn Stream<Item = Result<Operation, Status>> + Send>>;

#[derive(Clone)]
struct TransientNotFoundExecutionServer {
    wait_execution_calls: Arc<AtomicUsize>,
    not_found_responses: usize,
}

#[tonic::async_trait]
impl Execution for TransientNotFoundExecutionServer {
    type ExecuteStream = OperationStream;
    type WaitExecutionStream = OperationStream;

    async fn execute(
        &self,
        _request: Request<ExecuteRequest>,
    ) -> Result<Response<Self::ExecuteStream>, Status> {
        Err(Status::unimplemented("execute is not used in this test"))
    }

    async fn wait_execution(
        &self,
        request: Request<WaitExecutionRequest>,
    ) -> Result<Response<Self::WaitExecutionStream>, Status> {
        if self.wait_execution_calls.fetch_add(1, Ordering::Relaxed) < self.not_found_responses {
            return Err(Status::not_found("Failed to find existing task"));
        }

        let operation_id = OperationId::from(request.into_inner().name);
        let state = ActionState {
            client_operation_id: operation_id.clone(),
            stage: ActionStage::Queued,
            action_digest: DigestInfo::new([0u8; 32], 0),
            last_transition_timestamp: SystemTime::UNIX_EPOCH,
        };
        Ok(Response::new(Box::pin(stream::once(async move {
            Ok(state.as_operation(operation_id))
        }))))
    }
}

async fn make_transient_not_found_server(not_found_responses: usize) -> (Arc<AtomicUsize>, u16) {
    let wait_execution_calls = Arc::new(AtomicUsize::new(0));
    let service = ExecutionServer::new(TransientNotFoundExecutionServer {
        wait_execution_calls: wait_execution_calls.clone(),
        not_found_responses,
    });
    let listener = TcpIncoming::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = listener.local_addr().unwrap().port();

    background_spawn!("transient_not_found_execution_server", async move {
        Server::builder()
            .add_service(service)
            .serve_with_incoming(listener)
            .await
            .unwrap();
    });

    (wait_execution_calls, port)
}

fn make_scheduler(port: u16, max_retries: usize) -> Result<GrpcScheduler, Error> {
    GrpcScheduler::new_with_jitter(
        &GrpcSpec {
            endpoint: GrpcEndpoint {
                address: format!("http://127.0.0.1:{port}"),
                tls_config: None,
                concurrency_limit: None,
                connect_timeout_s: 0,
                tcp_keepalive_s: 0,
                http2_keepalive_interval_s: 0,
                http2_keepalive_timeout_s: 0,
            },
            retry: Retry {
                max_retries,
                delay: 0.0,
                ..Default::default()
            },
            max_concurrent_requests: 0,
            connections_per_endpoint: 1,
        },
        Arc::new(|_| Duration::ZERO),
    )
}

#[nativelink_test]
async fn wait_execution_retries_transient_not_found() -> Result<(), Error> {
    let (wait_execution_calls, port) = make_transient_not_found_server(1).await;
    let scheduler = make_scheduler(port, 1)?;
    let operation_id = OperationId::from("operation-id");

    let mut results = scheduler
        .filter_operations(OperationFilter {
            client_operation_id: Some(operation_id.clone()),
            ..Default::default()
        })
        .await?;
    let result = results
        .next()
        .await
        .expect("WaitExecution should recover after a transient NotFound");
    let (state, _) = result.as_state().await?;

    assert_eq!(state.client_operation_id, operation_id);
    assert_eq!(wait_execution_calls.load(Ordering::Relaxed), 2);
    Ok(())
}

#[nativelink_test]
async fn wait_execution_not_found_retry_is_bounded() -> Result<(), Error> {
    let (wait_execution_calls, port) = make_transient_not_found_server(usize::MAX).await;
    let scheduler = make_scheduler(port, 1)?;

    let mut results = scheduler
        .filter_operations(OperationFilter {
            client_operation_id: Some(OperationId::from("missing-operation-id")),
            ..Default::default()
        })
        .await?;

    assert!(results.next().await.is_none());
    assert_eq!(wait_execution_calls.load(Ordering::Relaxed), 2);
    Ok(())
}
