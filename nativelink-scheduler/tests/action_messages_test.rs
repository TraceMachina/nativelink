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

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::ExecuteResponse;
use nativelink_proto::google::longrunning::{operation, Operation};
use nativelink_proto::google::rpc::Status;
use nativelink_util::action_messages::{
    ActionInfo, ActionInfoHashKey, ActionResult, ActionStage, ActionState, ClientOperationId,
    ExecutionMetadata, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::platform_properties::PlatformProperties;
use pretty_assertions::assert_eq;

const NOW_TIME: u64 = 10000;

fn make_system_time(add_time: u64) -> SystemTime {
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(NOW_TIME + add_time))
        .unwrap()
}

#[nativelink_test]
async fn action_state_any_url_test() -> Result<(), Error> {
    let unique_qualifier = ActionInfoHashKey {
        instance_name: "foo_instance".to_string(),
        digest_function: DigestHasherFunc::Sha256,
        digest: DigestInfo::new([1u8; 32], 5),
        salt: 0,
    };
    let client_id = ClientOperationId::new(unique_qualifier.clone());
    let action_state = ActionState {
        id: OperationId::new(unique_qualifier),
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

    let action_state_round_trip: ActionState = operation.try_into()?;
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

#[nativelink_test]
async fn highest_priority_action_first() -> Result<(), Error> {
    const INSTANCE_NAME: &str = "foobar_instance_name";

    let high_priority_action = Arc::new(ActionInfo {
        command_digest: DigestInfo::new([0u8; 32], 0),
        input_root_digest: DigestInfo::new([0u8; 32], 0),
        timeout: Duration::MAX,
        platform_properties: PlatformProperties {
            properties: HashMap::new(),
        },
        priority: 1000,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionInfoHashKey {
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: DigestInfo::new([0u8; 32], 0),
            salt: 0,
        },
        skip_cache_lookup: true,
    });
    let lowest_priority_action = Arc::new(ActionInfo {
        command_digest: DigestInfo::new([0u8; 32], 0),
        input_root_digest: DigestInfo::new([0u8; 32], 0),
        timeout: Duration::MAX,
        platform_properties: PlatformProperties {
            properties: HashMap::new(),
        },
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionInfoHashKey {
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: DigestInfo::new([1u8; 32], 0),
            salt: 0,
        },
        skip_cache_lookup: true,
    });
    let mut action_set = BTreeSet::<Arc<ActionInfo>>::new();
    action_set.insert(lowest_priority_action.clone());
    action_set.insert(high_priority_action.clone());

    assert_eq!(
        vec![high_priority_action, lowest_priority_action],
        action_set
            .iter()
            .rev()
            .cloned()
            .collect::<Vec<Arc<ActionInfo>>>()
    );

    Ok(())
}

#[nativelink_test]
async fn equal_priority_earliest_first() -> Result<(), Error> {
    const INSTANCE_NAME: &str = "foobar_instance_name";

    let first_action = Arc::new(ActionInfo {
        command_digest: DigestInfo::new([0u8; 32], 0),
        input_root_digest: DigestInfo::new([0u8; 32], 0),
        timeout: Duration::MAX,
        platform_properties: PlatformProperties {
            properties: HashMap::new(),
        },
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionInfoHashKey {
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: DigestInfo::new([0u8; 32], 0),
            salt: 0,
        },
        skip_cache_lookup: true,
    });
    let current_action = Arc::new(ActionInfo {
        command_digest: DigestInfo::new([0u8; 32], 0),
        input_root_digest: DigestInfo::new([0u8; 32], 0),
        timeout: Duration::MAX,
        platform_properties: PlatformProperties {
            properties: HashMap::new(),
        },
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: make_system_time(0),
        unique_qualifier: ActionInfoHashKey {
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: DigestInfo::new([1u8; 32], 0),
            salt: 0,
        },
        skip_cache_lookup: true,
    });
    let mut action_set = BTreeSet::<Arc<ActionInfo>>::new();
    action_set.insert(current_action.clone());
    action_set.insert(first_action.clone());

    assert_eq!(
        vec![first_action, current_action],
        action_set
            .iter()
            .rev()
            .cloned()
            .collect::<Vec<Arc<ActionInfo>>>()
    );

    Ok(())
}
