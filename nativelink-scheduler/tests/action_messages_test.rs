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

use std::collections::HashMap;
use std::time::SystemTime;

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::ExecuteResponse;
use nativelink_proto::google::longrunning::{operation, Operation};
use nativelink_proto::google::rpc::Status;
use nativelink_util::action_messages::{
    ActionResult, ActionStage, ActionState, ActionUniqueKey, ActionUniqueQualifier,
    ClientOperationId, ExecutionMetadata, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use pretty_assertions::assert_eq;

#[nativelink_test]
async fn action_state_any_url_test() -> Result<(), Error> {
    let unique_qualifier = ActionUniqueQualifier::Cachable(ActionUniqueKey {
        instance_name: "foo_instance".to_string(),
        digest_function: DigestHasherFunc::Sha256,
        digest: DigestInfo::new([1u8; 32], 5),
    });
    let client_id = ClientOperationId::new(unique_qualifier.clone());
    let operation_id = OperationId::new(unique_qualifier);
    let action_state = ActionState {
        id: operation_id.clone(),
        // Result is only populated if has_action_result.
        stage: ActionStage::Completed(ActionResult::default()),
    };
    let operation: Operation = action_state.as_operation(client_id);

    match &operation.result {
        Some(operation::Result::Response(any)) => assert_eq!(
            any.type_url,
            "type.googleapis.com/build.bazel.remote.execution.v2.ExecuteResponse"
        ),
        other => panic!("Expected Some(Result(Any)), got: {other:?}"),
    }

    let action_state_round_trip = ActionState::try_from_operation(operation, operation_id)?;
    assert_eq!(action_state, action_state_round_trip);

    Ok(())
}

#[nativelink_test]
async fn execute_response_status_message_is_some_on_success_test() -> Result<(), Error> {
    let execute_response: ExecuteResponse = ActionStage::Completed(ActionResult {
        output_files: vec![],
        output_folders: vec![],
        output_file_symlinks: vec![],
        output_directory_symlinks: vec![],
        exit_code: 0,
        stdout_digest: DigestInfo::new([2u8; 32], 5),
        stderr_digest: DigestInfo::new([3u8; 32], 5),
        execution_metadata: ExecutionMetadata {
            worker: "foo_worker_id".to_string(),
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
    })
    .into();

    // This was once discovered to be None, which is why this test exists.
    assert_eq!(execute_response.status, Some(Status::default()));

    Ok(())
}
