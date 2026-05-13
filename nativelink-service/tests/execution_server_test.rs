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

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use futures::stream;
use nativelink_config::cas_server::{ExecutionConfig, WithInstanceName};
use nativelink_config::stores::{MemorySpec, StoreSpec};
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::execution_server::Execution;
use nativelink_proto::build::bazel::remote::execution::v2::{
    Action, ExecuteRequest, digest_function,
};
use nativelink_proto::google::longrunning::operations_server::Operations;
use nativelink_proto::google::longrunning::{
    CancelOperationRequest, DeleteOperationRequest, GetOperationRequest, ListOperationsRequest,
    WaitOperationRequest,
};
use nativelink_proto::google::rpc::Status as GrpcStatusProto;
use nativelink_scheduler::mock_scheduler::MockActionScheduler;
use nativelink_service::execution_server::ExecutionServer;
use nativelink_store::ac_utils::serialize_and_upload_message;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::action_messages::{
    ActionInfo, ActionResult, ActionStage, ActionState, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager,
};
use nativelink_util::origin_event::OriginMetadata;
use nativelink_util::precondition_failure;
use nativelink_util::store_trait::StoreLike;
use prost::Message as _;
use tonic::{Code as TonicCode, Request};

const INSTANCE_NAME: &str = "instance_name";

async fn make_store_manager() -> Result<Arc<StoreManager>, Error> {
    let store_manager = Arc::new(StoreManager::new());
    store_manager.add_store(
        "main_cas",
        store_factory(
            &StoreSpec::Memory(MemorySpec::default()),
            &store_manager,
            None,
        )
        .await?,
    );
    Ok(store_manager)
}

fn make_execution_server(
    store_manager: &StoreManager,
) -> Result<(ExecutionServer, Arc<MockActionScheduler>), Error> {
    let mock_scheduler = Arc::new(MockActionScheduler::new());
    let mut action_schedulers: HashMap<String, Arc<dyn ClientStateManager>> = HashMap::new();
    action_schedulers.insert("main_scheduler".to_string(), mock_scheduler.clone());
    let server = ExecutionServer::new(
        &[WithInstanceName {
            instance_name: INSTANCE_NAME.to_string(),
            config: ExecutionConfig {
                cas_store: "main_cas".to_string(),
                scheduler: "main_scheduler".to_string(),
            },
        }],
        &action_schedulers,
        store_manager,
    )?;
    Ok((server, mock_scheduler))
}

#[nativelink_test]
async fn instance_name_fail() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let (execution_server, _) = make_execution_server(&store_manager)?;

    let raw_response = execution_server
        .execute(Request::new(ExecuteRequest {
            instance_name: "foo".to_string(),
            digest_function: digest_function::Value::Sha256.into(),
            skip_cache_lookup: false,
            action_digest: None,
            execution_policy: None,
            results_cache_policy: None,
        }))
        .await;

    match raw_response {
        Err(response_err) => {
            assert_eq!(
                response_err.message(),
                "'instance_name' not configured for 'foo' : Failed on execute() command"
            );
        }
        Ok(_) => {
            panic!("Not expecting ok!");
        }
    }
    Ok(())
}

#[nativelink_test]
async fn operations_list_operations_unimplemented() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let (execution_server, _) = make_execution_server(&store_manager)?;

    let err = execution_server
        .list_operations(Request::new(ListOperationsRequest::default()))
        .await
        .unwrap_err();

    assert_eq!(err.code(), Code::Unimplemented);
    assert_eq!(err.message(), "list_operations not implemented");
    Ok(())
}

#[nativelink_test]
async fn operations_delete_operation_unimplemented() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let (execution_server, _) = make_execution_server(&store_manager)?;

    let err = execution_server
        .delete_operation(Request::new(DeleteOperationRequest::default()))
        .await
        .unwrap_err();

    assert_eq!(err.code(), Code::Unimplemented);
    assert_eq!(err.message(), "delete_operation not implemented");
    Ok(())
}

#[nativelink_test]
async fn operations_cancel_operation_unimplemented() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let (execution_server, _) = make_execution_server(&store_manager)?;

    let err = execution_server
        .cancel_operation(Request::new(CancelOperationRequest::default()))
        .await
        .unwrap_err();

    assert_eq!(err.code(), Code::Unimplemented);
    assert_eq!(err.message(), "cancel_operation not implemented");
    Ok(())
}

struct MockActionStateResult {
    states: Vec<Arc<ActionState>>,
}

#[async_trait]
impl ActionStateResult for MockActionStateResult {
    async fn as_state(&self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        Ok((self.states.first().unwrap().clone(), None))
    }

    async fn changed(&mut self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        if self.states.is_empty() {
            return Err(make_err!(Code::Internal, "No more states"));
        }
        let state = self.states.remove(0);
        Ok((state, None))
    }

    async fn as_action_info(&self) -> Result<(Arc<ActionInfo>, Option<OriginMetadata>), Error> {
        Err(make_err!(
            Code::Unimplemented,
            "as_action_info not implemented"
        ))
    }
}

#[nativelink_test]
async fn operations_get_operation() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let (execution_server, mock_scheduler) = make_execution_server(&store_manager)?;

    let operation_name = format!("{INSTANCE_NAME}/some_operation_id");

    let action_state = Arc::new(ActionState {
        client_operation_id: OperationId::from("some_operation_id"),
        stage: ActionStage::Queued,
        action_digest: DigestInfo::new([0u8; 32], 0),
        last_transition_timestamp: SystemTime::UNIX_EPOCH,
    });

    let mock_action_state_result = MockActionStateResult {
        states: vec![action_state.clone()],
    };

    let stream: ActionStateResultStream = Box::pin(stream::once(async move {
        let result: Box<dyn ActionStateResult> = Box::new(mock_action_state_result);
        result
    }));

    let request_fut = execution_server.get_operation(Request::new(GetOperationRequest {
        name: operation_name.clone(),
    }));

    let (request_res, filter) = tokio::join!(
        request_fut,
        mock_scheduler.expect_filter_operations(Ok(stream)),
    );

    assert_eq!(
        filter.client_operation_id,
        Some(OperationId::from("some_operation_id"))
    );

    let operation = request_res?.into_inner();
    assert_eq!(operation.name, operation_name);
    assert!(!operation.done);

    Ok(())
}

#[nativelink_test]
async fn operations_get_operation_not_found() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let (execution_server, mock_scheduler) = make_execution_server(&store_manager)?;

    let operation_name = format!("{INSTANCE_NAME}/some_operation_id");

    let stream: ActionStateResultStream = Box::pin(stream::empty());

    let request_fut = execution_server.get_operation(Request::new(GetOperationRequest {
        name: operation_name.clone(),
    }));

    let (request_res, filter) = tokio::join!(
        request_fut,
        mock_scheduler.expect_filter_operations(Ok(stream)),
    );

    assert_eq!(
        filter.client_operation_id,
        Some(OperationId::from("some_operation_id"))
    );

    let err = request_res.unwrap_err();
    assert_eq!(err.code(), Code::NotFound);
    assert_eq!(err.message(), "Failed to find existing task");

    Ok(())
}

#[nativelink_test]
async fn operations_wait_operation_finishes() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let (execution_server, mock_scheduler) = make_execution_server(&store_manager)?;

    let operation_name = format!("{INSTANCE_NAME}/some_operation_id");

    let state1 = Arc::new(ActionState {
        client_operation_id: OperationId::from("some_operation_id"),
        stage: ActionStage::Queued,
        action_digest: DigestInfo::new([0u8; 32], 0),
        last_transition_timestamp: SystemTime::UNIX_EPOCH,
    });

    let state2 = Arc::new(ActionState {
        client_operation_id: OperationId::from("some_operation_id"),
        stage: ActionStage::Completed(ActionResult::default()),
        action_digest: DigestInfo::new([0u8; 32], 0),
        last_transition_timestamp: SystemTime::UNIX_EPOCH,
    });

    let mock_action_state_result = MockActionStateResult {
        states: vec![state1.clone(), state2.clone()],
    };

    let stream: ActionStateResultStream = Box::pin(stream::once(async move {
        let result: Box<dyn ActionStateResult> = Box::new(mock_action_state_result);
        result
    }));

    let request_fut = execution_server.wait_operation(Request::new(WaitOperationRequest {
        name: operation_name.clone(),
        timeout: None,
    }));

    let (request_res, _) = tokio::join!(
        request_fut,
        mock_scheduler.expect_filter_operations(Ok(stream)),
    );

    let operation = request_res?.into_inner();
    assert_eq!(operation.name, operation_name);
    assert!(operation.done);

    Ok(())
}

struct TimeoutActionStateResult {
    state: Arc<ActionState>,
    first_called: bool,
}

#[async_trait]
impl ActionStateResult for TimeoutActionStateResult {
    async fn as_state(&self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        Ok((self.state.clone(), None))
    }

    async fn changed(&mut self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        if !self.first_called {
            self.first_called = true;
            return Ok((self.state.clone(), None));
        }
        tokio::time::sleep(core::time::Duration::from_secs(1)).await;
        Ok((self.state.clone(), None))
    }

    async fn as_action_info(&self) -> Result<(Arc<ActionInfo>, Option<OriginMetadata>), Error> {
        Err(make_err!(
            Code::Unimplemented,
            "as_action_info not implemented"
        ))
    }
}

/// Encodes a digest into the `REv2` missing-blob subject format.
fn blob_subject(d: &DigestInfo) -> String {
    format!("blobs/{}/{}", d.packed_hash(), d.size_bytes())
}

/// Decode the `FAILED_PRECONDITION` detail bytes that the server placed
/// in `grpc-status-details-bin`. Returns the inner `PreconditionFailure`.
fn decode_precondition_failure(
    status: &tonic::Status,
) -> Result<precondition_failure::PreconditionFailure, Box<dyn core::error::Error>> {
    let outer = GrpcStatusProto::decode(status.details())?;
    assert_eq!(
        outer.code,
        TonicCode::FailedPrecondition as i32,
        "inner google.rpc.Status code should match the tonic FAILED_PRECONDITION",
    );
    assert_eq!(outer.details.len(), 1, "expected exactly one detail");
    assert_eq!(
        outer.details[0].type_url,
        precondition_failure::TYPE_URL,
        "detail type_url must match PreconditionFailure",
    );
    Ok(precondition_failure::PreconditionFailure::decode(
        &*outer.details[0].value,
    )?)
}

async fn upload_action(
    cas_store: &nativelink_util::store_trait::Store,
    action: &Action,
) -> Result<DigestInfo, Error> {
    serialize_and_upload_message(
        action,
        cas_store.as_pin(),
        &mut DigestHasherFunc::Sha256.hasher(),
    )
    .await
}

const fn make_fake_digest(byte: u8, size: u64) -> DigestInfo {
    DigestInfo::new([byte; 32], size)
}

fn make_execute_request(action_digest: DigestInfo) -> ExecuteRequest {
    ExecuteRequest {
        instance_name: INSTANCE_NAME.to_string(),
        digest_function: digest_function::Value::Sha256.into(),
        skip_cache_lookup: false,
        action_digest: Some(action_digest.into()),
        execution_policy: None,
        results_cache_policy: None,
    }
}

#[nativelink_test]
async fn execute_missing_action_returns_precondition_failure()
-> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let (execution_server, _) = make_execution_server(&store_manager)?;

    let action_digest = make_fake_digest(0xaa, 16);

    let Err(status) = execution_server
        .execute(Request::new(make_execute_request(action_digest)))
        .await
    else {
        panic!("execute should fail when the Action is missing");
    };

    assert_eq!(status.code(), TonicCode::FailedPrecondition);
    let pf = decode_precondition_failure(&status)?;
    assert_eq!(pf.violations.len(), 1);
    let v = &pf.violations[0];
    assert_eq!(v.r#type, "MISSING");
    assert_eq!(v.subject, blob_subject(&action_digest));
    assert_eq!(v.description, "Action");
    Ok(())
}

#[nativelink_test]
async fn execute_missing_command_returns_precondition_failure()
-> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let (execution_server, _) = make_execution_server(&store_manager)?;
    let cas_store = store_manager.get_store("main_cas").unwrap();

    let command_digest = make_fake_digest(0xc1, 8);
    let input_root = Action::default();
    let input_root_digest = upload_action(&cas_store, &input_root).await?;

    let action = Action {
        command_digest: Some(command_digest.into()),
        input_root_digest: Some(input_root_digest.into()),
        ..Default::default()
    };
    let action_digest = upload_action(&cas_store, &action).await?;

    let Err(status) = execution_server
        .execute(Request::new(make_execute_request(action_digest)))
        .await
    else {
        panic!("execute should fail when command_digest is missing");
    };

    assert_eq!(status.code(), TonicCode::FailedPrecondition);
    let pf = decode_precondition_failure(&status)?;
    assert_eq!(pf.violations.len(), 1);
    assert_eq!(pf.violations[0].r#type, "MISSING");
    assert_eq!(pf.violations[0].subject, blob_subject(&command_digest));
    assert_eq!(pf.violations[0].description, "Action.command_digest");
    Ok(())
}

#[nativelink_test]
async fn execute_missing_input_root_returns_precondition_failure()
-> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let (execution_server, _) = make_execution_server(&store_manager)?;
    let cas_store = store_manager.get_store("main_cas").unwrap();

    // Upload command, omit input_root.
    let command_proto = nativelink_proto::build::bazel::remote::execution::v2::Command::default();
    let command_digest = serialize_and_upload_message(
        &command_proto,
        cas_store.as_pin(),
        &mut DigestHasherFunc::Sha256.hasher(),
    )
    .await?;
    let input_root_digest = make_fake_digest(0xd2, 16);

    let action = Action {
        command_digest: Some(command_digest.into()),
        input_root_digest: Some(input_root_digest.into()),
        ..Default::default()
    };
    let action_digest = upload_action(&cas_store, &action).await?;

    let Err(status) = execution_server
        .execute(Request::new(make_execute_request(action_digest)))
        .await
    else {
        panic!("execute should fail when input_root_digest is missing");
    };

    assert_eq!(status.code(), TonicCode::FailedPrecondition);
    let pf = decode_precondition_failure(&status)?;
    assert_eq!(pf.violations.len(), 1);
    assert_eq!(pf.violations[0].r#type, "MISSING");
    assert_eq!(pf.violations[0].subject, blob_subject(&input_root_digest));
    assert_eq!(pf.violations[0].description, "Action.input_root_digest");
    Ok(())
}

#[nativelink_test]
async fn execute_missing_command_and_input_root_returns_both_violations()
-> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let (execution_server, _) = make_execution_server(&store_manager)?;
    let cas_store = store_manager.get_store("main_cas").unwrap();

    let command_digest = make_fake_digest(0xe3, 8);
    let input_root_digest = make_fake_digest(0xf4, 16);

    let action = Action {
        command_digest: Some(command_digest.into()),
        input_root_digest: Some(input_root_digest.into()),
        ..Default::default()
    };
    let action_digest = upload_action(&cas_store, &action).await?;

    let Err(status) = execution_server
        .execute(Request::new(make_execute_request(action_digest)))
        .await
    else {
        panic!("execute should fail when both blobs are missing");
    };

    assert_eq!(status.code(), TonicCode::FailedPrecondition);
    let pf = decode_precondition_failure(&status)?;
    assert_eq!(pf.violations.len(), 2);
    assert_eq!(pf.violations[0].subject, blob_subject(&command_digest));
    assert_eq!(pf.violations[0].description, "Action.command_digest");
    assert_eq!(pf.violations[1].subject, blob_subject(&input_root_digest));
    assert_eq!(pf.violations[1].description, "Action.input_root_digest");
    Ok(())
}

#[nativelink_test]
async fn operations_wait_operation_timeout() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let (execution_server, mock_scheduler) = make_execution_server(&store_manager)?;

    let operation_name = format!("{INSTANCE_NAME}/some_operation_id");

    let state1 = Arc::new(ActionState {
        client_operation_id: OperationId::from("some_operation_id"),
        stage: ActionStage::Queued,
        action_digest: DigestInfo::new([0u8; 32], 0),
        last_transition_timestamp: SystemTime::UNIX_EPOCH,
    });

    let mock_action_state_result = TimeoutActionStateResult {
        state: state1.clone(),
        first_called: false,
    };

    let stream: ActionStateResultStream = Box::pin(stream::once(async move {
        let result: Box<dyn ActionStateResult> = Box::new(mock_action_state_result);
        result
    }));

    let request_fut = execution_server.wait_operation(Request::new(WaitOperationRequest {
        name: operation_name.clone(),
        timeout: Some(prost_types::Duration {
            seconds: 0,
            nanos: 10_000_000, // 10ms
        }),
    }));

    let (request_res, _) = tokio::join!(
        request_fut,
        mock_scheduler.expect_filter_operations(Ok(stream)),
    );

    let operation = request_res?.into_inner();
    assert_eq!(operation.name, operation_name);
    assert!(!operation.done);

    Ok(())
}
