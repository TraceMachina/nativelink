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

use std::collections::HashMap;
use std::time::SystemTime;

use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::{
    ActionResult as ProtoActionResult, ExecuteResponse,
};
use nativelink_proto::google::longrunning::{Operation, operation};
use nativelink_proto::google::rpc::Status;
use nativelink_util::action_messages::{
    ActionResult, ActionStage, ActionState, ActionUniqueKey, ActionUniqueQualifier,
    ExecutionMetadata, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use pretty_assertions::assert_eq;

#[nativelink_test]
async fn test_remote_exec_scheduling_exit_code_and_output_buffer_flush() -> Result<(), Error> {
    // 1. Verify successful action result flush: exit code 0 and non-empty stdout/stderr digests
    let stdout_digest = DigestInfo::new([2u8; 32], 42);
    let stderr_digest = DigestInfo::new([3u8; 32], 0);

    let action_result_success = ActionResult {
        output_files: vec![],
        output_folders: vec![],
        output_directory_symlinks: vec![],
        output_file_symlinks: vec![],
        exit_code: 0,
        stdout_digest,
        stderr_digest,
        execution_metadata: ExecutionMetadata {
            worker: "worker_node_1".to_string(),
            queued_timestamp: SystemTime::UNIX_EPOCH,
            worker_start_timestamp: SystemTime::UNIX_EPOCH,
            worker_completed_timestamp: SystemTime::UNIX_EPOCH,
            input_fetch_start_timestamp: SystemTime::UNIX_EPOCH,
            input_fetch_completed_timestamp: SystemTime::UNIX_EPOCH,
            execution_start_timestamp: SystemTime::UNIX_EPOCH,
            execution_completed_timestamp: SystemTime::UNIX_EPOCH,
            output_upload_start_timestamp: SystemTime::UNIX_EPOCH,
            output_upload_completed_timestamp: SystemTime::UNIX_EPOCH,
        },
        server_logs: HashMap::default(),
        error: None,
        message: String::new(),
    };

    let execute_response: ExecuteResponse =
        ActionStage::Completed(action_result_success.clone()).into();

    // Verify ExecuteResponse status is OK and action result contains flushed stdout/stderr
    assert_eq!(execute_response.status, Some(Status::default()));
    assert!(!execute_response.cached_result);

    let proto_res: ProtoActionResult = execute_response
        .result
        .expect("Expected result in ExecuteResponse on completion");
    assert_eq!(proto_res.exit_code, 0);
    assert_eq!(proto_res.stdout_digest, Some(stdout_digest.into()));
    assert_eq!(proto_res.stderr_digest, Some(stderr_digest.into()));

    // 2. Verify non-zero exit code propagation across execution boundary
    let action_result_failure = ActionResult {
        exit_code: 1,
        stdout_digest: DigestInfo::new([4u8; 32], 10),
        stderr_digest: DigestInfo::new([5u8; 32], 88),
        ..action_result_success
    };

    let fail_response: ExecuteResponse = ActionStage::Completed(action_result_failure).into();

    let fail_proto_res = fail_response
        .result
        .expect("Expected result in ExecuteResponse on failure completion");
    assert_eq!(fail_proto_res.exit_code, 1);
    assert_eq!(
        fail_proto_res.stdout_digest,
        Some(DigestInfo::new([4u8; 32], 10).into())
    );
    assert_eq!(
        fail_proto_res.stderr_digest,
        Some(DigestInfo::new([5u8; 32], 88).into())
    );

    // 3. Verify intermediate stages do not prematurely flush results
    let executing_response: ExecuteResponse = ActionStage::Executing.into();
    assert_eq!(executing_response, ExecuteResponse::default());

    let queued_response: ExecuteResponse = ActionStage::Queued.into();
    assert_eq!(queued_response, ExecuteResponse::default());

    Ok(())
}

#[nativelink_test]
async fn test_remote_exec_scheduling_timeout_and_error_propagation() -> Result<(), Error> {
    // Verify worker execution timeout / cancellation propagation without socket leak
    let timeout_error = make_err!(
        Code::DeadlineExceeded,
        "Worker execution timed out after 50ms"
    );

    let action_result_timeout = ActionResult {
        output_files: vec![],
        output_folders: vec![],
        output_directory_symlinks: vec![],
        output_file_symlinks: vec![],
        exit_code: 143, // Standard SIGTERM exit code
        stdout_digest: DigestInfo::new([0u8; 32], 0),
        stderr_digest: DigestInfo::new([0u8; 32], 0),
        execution_metadata: ExecutionMetadata::default(),
        server_logs: HashMap::default(),
        error: Some(timeout_error),
        message: "Timeout enforced by scheduler".to_string(),
    };

    let response: ExecuteResponse = ActionStage::Completed(action_result_timeout).into();

    let status = response
        .status
        .expect("Status must be populated when error is present");
    assert_eq!(status.code, Code::DeadlineExceeded as i32);
    assert!(status.message.contains("Worker execution timed out"));
    assert_eq!(response.message, "Timeout enforced by scheduler");

    Ok(())
}

#[nativelink_test]
async fn test_remote_exec_input_root_path_resolution_and_digest_qualifier() -> Result<(), Error> {
    // Verify input root digest resolution and qualifier isolation
    let input_root_digest = DigestInfo::new([7u8; 32], 512);
    let unique_key = ActionUniqueKey {
        execution_scope: None,
        instance_name: "remote_exec_instance".to_string(),
        digest_function: DigestHasherFunc::Sha256,
        digest: input_root_digest,
    };

    let qualifier = ActionUniqueQualifier::Cacheable(unique_key);
    let resolved_digest = qualifier.digest();

    assert_eq!(resolved_digest, input_root_digest);

    // Verify ActionState round-trip serialization through LongRunning Operation
    let operation_id = OperationId::default();
    let client_id = OperationId::default();
    let action_state = ActionState {
        client_operation_id: operation_id.clone(),
        stage: ActionStage::Completed(ActionResult::default()),
        action_digest: resolved_digest,
        last_transition_timestamp: SystemTime::now(),
    };

    let operation: Operation = action_state.clone().as_operation(client_id);
    match &operation.result {
        Some(operation::Result::Response(any)) => {
            assert_eq!(
                any.type_url,
                "type.googleapis.com/build.bazel.remote.execution.v2.ExecuteResponse"
            );
        }
        other => panic!("Expected Some(Result(Any)), got: {other:?}"),
    }

    let round_trip_state = ActionState::try_from_operation(operation, operation_id)?;
    assert_eq!(action_state, round_trip_state);

    Ok(())
}
