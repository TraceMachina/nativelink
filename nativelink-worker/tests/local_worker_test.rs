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

use core::sync::atomic::Ordering;
use core::time::Duration;
use std::collections::HashMap;
#[cfg(target_family = "unix")]
use std::env;
use std::ffi::OsString;
use std::io::Write;
#[cfg(target_family = "unix")]
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

mod utils {
    pub(crate) mod local_worker_test_utils;
    pub(crate) mod mock_running_actions_manager;
}

use hyper::body::Frame;
use nativelink_config::cas_server::{EndpointConfig, LocalWorkerConfig, WorkerProperty};
use nativelink_config::stores::{
    FastSlowSpec, FilesystemSpec, MemorySpec, StoreDirection, StoreSpec,
};
use nativelink_error::{Code, Error, ErrorContext, make_err, make_input_err};
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::Platform;
use nativelink_proto::build::bazel::remote::execution::v2::platform::Property;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::update_for_worker::Update;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    ConnectWorkerRequest, ConnectionResult, ExecuteResult, KillOperationRequest, StartExecute,
    UpdateForWorker, execute_declined, execute_result,
};
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::filesystem_store::FilesystemStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::action_messages::{
    ActionInfo, ActionResult, ActionStage, ActionUniqueKey, ActionUniqueQualifier,
    ExecutionMetadata, OperationId,
};
use nativelink_util::common::{DigestInfo, encode_stream_proto, fs, make_temp_path};
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::health_utils::{HealthStatus, HealthStatusIndicator};
use nativelink_util::store_trait::Store;
use nativelink_worker::capacity::{free_memory_kb, memory_is_limited};
use nativelink_worker::local_worker::new_local_worker;
#[cfg(target_family = "unix")]
use nativelink_worker::local_worker::preconditions_met;
use nativelink_worker::running_actions_manager::MISSING_INPUT_ERROR_TIP;
use pretty_assertions::assert_eq;
use prost::Message;
use tokio::io::AsyncWriteExt;
use tokio::time::sleep;
use utils::local_worker_test_utils::{
    TestContext, setup_grpc_stream, setup_local_worker, setup_local_worker_with_config,
};
use utils::mock_running_actions_manager::MockRunningAction;

const INSTANCE_NAME: &str = "foo";

#[nativelink_test]
#[cfg_attr(feature = "nix", ignore)]
async fn platform_properties_smoke_test() -> Result<(), Error> {
    let mut platform_properties = HashMap::new();
    platform_properties.insert(
        "foo".to_string(),
        WorkerProperty::Values(vec!["bar1".to_string(), "bar2".to_string()]),
    );
    platform_properties.insert(
        "baz".to_string(),
        // Note: new lines will result in two entries for same key.
        #[cfg(target_family = "unix")]
        WorkerProperty::QueryCmd("printf 'hello\ngoodbye'".to_string()),
        #[cfg(target_family = "windows")]
        WorkerProperty::QueryCmd("cmd /C \"echo hello && echo goodbye\"".to_string()),
    );
    let mut test_context = setup_local_worker(platform_properties).await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();

    // Now wait for our client to send `.connect_worker()` (which has our platform properties).
    let mut connect_worker_request = test_context
        .client
        .expect_connect_worker(Ok(streaming_response))
        .await;
    // It is undefined which order these will be returned in, so we sort it.
    connect_worker_request
        .properties
        .sort_by_key(Message::encode_to_vec);
    assert_eq!(
        connect_worker_request,
        ConnectWorkerRequest {
            worker_id_prefix: String::new(),
            properties: vec![
                Property {
                    name: "baz".to_string(),
                    value: "hello".to_string(),
                },
                Property {
                    name: "baz".to_string(),
                    value: "goodbye".to_string(),
                },
                Property {
                    name: "foo".to_string(),
                    value: "bar1".to_string(),
                },
                Property {
                    name: "foo".to_string(),
                    value: "bar2".to_string(),
                }
            ],
            max_inflight_tasks: 0,
            admits_when_idle: memory_is_limited(),
        }
    );

    Ok(())
}

#[nativelink_test]
async fn reconnect_on_server_disconnect_test() -> Result<(), Error> {
    let mut test_context = setup_local_worker(HashMap::new()).await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();

    {
        // Ensure our worker connects and properties were sent.
        let props = test_context
            .client
            .expect_connect_worker(Ok(streaming_response))
            .await;
        assert_eq!(
            props,
            ConnectWorkerRequest {
                admits_when_idle: memory_is_limited(),
                ..Default::default()
            }
        );
    }

    // Disconnect our grpc stream.
    drop(test_context.maybe_tx_stream.take().unwrap());

    {
        // Client should try to auto reconnect and check our properties again.
        let (_, streaming_response) = setup_grpc_stream();
        let props = test_context
            .client
            .expect_connect_worker(Ok(streaming_response))
            .await;
        assert_eq!(
            props,
            ConnectWorkerRequest {
                admits_when_idle: memory_is_limited(),
                ..Default::default()
            }
        );
    }

    Ok(())
}

#[nativelink_test]
async fn kill_all_called_on_disconnect() -> Result<(), Error> {
    let mut test_context = setup_local_worker(HashMap::new()).await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();

    {
        // Ensure our worker connects and properties were sent.
        let props = test_context
            .client
            .expect_connect_worker(Ok(streaming_response))
            .await;
        assert_eq!(
            props,
            ConnectWorkerRequest {
                admits_when_idle: memory_is_limited(),
                ..Default::default()
            }
        );
    }

    // Handle registration (kill_all not called unless registered).
    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    {
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::ConnectionResult(ConnectionResult {
                        worker_id: "foobar".to_string(),
                        dispatch_ack: false,
                        memory_property: String::new(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }

    // Disconnect our grpc stream.
    drop(tx_stream);

    // Check that kill_all is called.
    test_context.actions_manager.expect_kill_all().await;

    Ok(())
}

/// A disconnect that catches an action mid-handoff must still reconnect.
/// Returning from the worker's main loop here aborts the process, so a
/// scheduler blip that happened to land while an action was in transit took
/// the whole worker down and every action it held had to run again elsewhere.
#[nativelink_test]
async fn reconnects_when_action_stuck_in_transit_on_disconnect() -> Result<(), Error> {
    let mut test_context = setup_local_worker(HashMap::new()).await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();

    test_context
        .client
        .expect_connect_worker(Ok(streaming_response))
        .await;

    let expected_worker_id = "foobar".to_string();
    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    tx_stream
        .send(Frame::data(
            encode_stream_proto(&UpdateForWorker {
                update: Some(Update::ConnectionResult(ConnectionResult {
                    worker_id: expected_worker_id.clone(),
                    dispatch_ack: false,
                    memory_property: String::new(),
                })),
            })
            .unwrap(),
        ))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;

    let action_info = ActionInfo {
        command_digest: DigestInfo::new([1u8; 32], 10),
        input_root_digest: DigestInfo::new([2u8; 32], 10),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: DigestInfo::new([3u8; 32], 10),
        }),
    };

    // Start an action, but deliberately never answer create_and_add_action.
    // The action stays counted as in transit, so the drain wait on disconnect
    // runs to its limit instead of settling.
    tx_stream
        .send(Frame::data(
            encode_stream_proto(&UpdateForWorker {
                update: Some(Update::StartAction(StartExecute {
                    request_metadata: None,
                    execute_request: Some((&action_info).into()),
                    operation_id: String::new(),
                    queued_timestamp: None,
                    platform: Some(Platform::default()),
                    worker_id: expected_worker_id,
                })),
            })
            .unwrap(),
        ))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;

    // Disconnect while that action is still in transit.
    drop(tx_stream);

    test_context.actions_manager.expect_kill_all().await;

    // The worker must come back rather than exiting.
    let (_, streaming_response) = setup_grpc_stream();
    let props = test_context
        .client
        .expect_connect_worker(Ok(streaming_response))
        .await;
    assert_eq!(
        props,
        ConnectWorkerRequest {
            admits_when_idle: memory_is_limited(),
            ..Default::default()
        }
    );

    Ok(())
}

#[nativelink_test]
async fn blake3_digest_function_registered_properly() -> Result<(), Error> {
    let mut test_context = setup_local_worker(HashMap::new()).await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();

    {
        // Ensure our worker connects and properties were sent.
        let props = test_context
            .client
            .expect_connect_worker(Ok(streaming_response))
            .await;
        assert_eq!(
            props,
            ConnectWorkerRequest {
                admits_when_idle: memory_is_limited(),
                ..Default::default()
            }
        );
    }

    let expected_worker_id = "foobar".to_string();

    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    {
        // First initialize our worker by sending the response to the connection request.
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::ConnectionResult(ConnectionResult {
                        worker_id: expected_worker_id.clone(),
                        dispatch_ack: false,
                        memory_property: String::new(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }

    let action_digest = DigestInfo::new([3u8; 32], 10);
    let action_info = ActionInfo {
        command_digest: DigestInfo::new([1u8; 32], 10),
        input_root_digest: DigestInfo::new([2u8; 32], 10),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Blake3,
            digest: action_digest,
        }),
    };

    {
        // Send execution request.
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::StartAction(StartExecute {
                        request_metadata: None,
                        execute_request: Some((&action_info).into()),
                        operation_id: String::new(),
                        queued_timestamp: None,
                        platform: Some(Platform::default()),
                        worker_id: expected_worker_id.clone(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }
    let running_action = Arc::new(MockRunningAction::new());

    // Send and wait for response from create_and_add_action to RunningActionsManager.
    test_context
        .actions_manager
        .expect_create_and_add_action(Ok(running_action.clone()))
        .await;

    // Now the RunningAction needs to send a series of state updates. This shortcuts them
    // into a single call (shortcut for prepare, execute, upload, collect_results, cleanup).
    running_action
        .simple_expect_get_finished_result(Ok(ActionResult::default()))
        .await?;

    // Expect the action to be updated in the action cache.
    let (_stored_digest, _stored_result, digest_hasher) = test_context
        .actions_manager
        .expect_cache_action_result()
        .await;
    assert_eq!(digest_hasher, DigestHasherFunc::Blake3);

    Ok(())
}

#[nativelink_test]
async fn simple_worker_start_action_test() -> Result<(), Error> {
    start_action_lifecycle_test(false).await
}

#[nativelink_test]
async fn single_use_worker_rejects_second_action_and_waits_for_uploads() -> Result<(), Error> {
    start_action_lifecycle_test(true).await
}

async fn start_action_lifecycle_test(single_use: bool) -> Result<(), Error> {
    let mut test_context = setup_local_worker_with_config(LocalWorkerConfig {
        single_use,
        max_inflight_tasks: if single_use { 9 } else { 0 },
        worker_api_endpoint: EndpointConfig {
            timeout: Some(10000.),
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();

    {
        // Ensure our worker connects and properties were sent.
        let props = test_context
            .client
            .expect_connect_worker(Ok(streaming_response))
            .await;
        assert_eq!(
            props,
            ConnectWorkerRequest {
                max_inflight_tasks: u64::from(single_use),
                admits_when_idle: memory_is_limited(),
                ..Default::default()
            }
        );
    }

    let expected_worker_id = "foobar".to_string();

    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    {
        // First initialize our worker by sending the response to the connection request.
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::ConnectionResult(ConnectionResult {
                        worker_id: expected_worker_id.clone(),
                        dispatch_ack: false,
                        memory_property: String::new(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }

    let action_digest = DigestInfo::new([3u8; 32], 10);
    let action_info = ActionInfo {
        command_digest: DigestInfo::new([1u8; 32], 10),
        input_root_digest: DigestInfo::new([2u8; 32], 10),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: action_digest,
        }),
    };

    {
        // Send execution request.
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::StartAction(StartExecute {
                        request_metadata: None,
                        execute_request: Some((&action_info).into()),
                        operation_id: String::new(),
                        queued_timestamp: None,
                        platform: Some(Platform::default()),
                        worker_id: expected_worker_id.clone(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }
    let action_result = ActionResult {
        output_files: vec![],
        output_folders: vec![],
        output_file_symlinks: vec![],
        output_directory_symlinks: vec![],
        exit_code: 5,
        stdout_digest: DigestInfo::new([21u8; 32], 10),
        stderr_digest: DigestInfo::new([22u8; 32], 10),
        execution_metadata: ExecutionMetadata {
            worker: expected_worker_id.clone(),
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
        server_logs: HashMap::new(),
        error: None,
        message: String::new(),
    };
    let running_action = Arc::new(MockRunningAction::new());

    // Send and wait for response from create_and_add_action to RunningActionsManager.
    test_context
        .actions_manager
        .expect_create_and_add_action(Ok(running_action.clone()))
        .await;

    if single_use {
        // Even a scheduler that sends more than our advertised capacity must
        // not run a second action in this container.
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::StartAction(StartExecute {
                        request_metadata: None,
                        execute_request: Some((&action_info).into()),
                        operation_id: "second-action".to_string(),
                        worker_id: expected_worker_id.clone(),
                        ..Default::default()
                    })),
                })
                .unwrap(),
            ))
            .await
            .unwrap();
        let rejected = test_context.client.expect_execution_response(Ok(())).await;
        assert_eq!(rejected.operation_id, "second-action");
        let Some(execute_result::Result::InternalError(status)) = rejected.result else {
            panic!("Second action must be rejected before execution");
        };
        assert_eq!(status.code, Code::ResourceExhausted as i32);
    }

    // Now the RunningAction needs to send a series of state updates. This shortcuts them
    // into a single call (shortcut for prepare, execute, upload, collect_results, cleanup).
    running_action
        .simple_expect_get_finished_result(Ok(action_result.clone()))
        .await?;

    // Expect the action to be updated in the action cache.
    let (stored_digest, stored_result, digest_hasher) = test_context
        .actions_manager
        .expect_cache_action_result()
        .await;
    assert_eq!(stored_digest, action_digest);
    assert_eq!(stored_result, action_result.clone());
    assert_eq!(digest_hasher, DigestHasherFunc::Sha256);
    assert_eq!(
        test_context.client.going_away_count.load(Ordering::Relaxed),
        0
    );
    assert_eq!(
        test_context
            .client
            .execution_complete_count
            .load(Ordering::Relaxed),
        u64::from(!single_use)
    );

    // Now our client should be notified that our runner finished.
    let execution_response = test_context.client.expect_execution_response(Ok(())).await;

    // Now ensure the final results match our expectations.
    assert_eq!(
        execution_response,
        ExecuteResult {
            instance_name: INSTANCE_NAME.to_string(),
            operation_id: String::new(),
            result: Some(execute_result::Result::ExecuteResponse(
                ActionStage::Completed(action_result).into()
            )),
            resource_usage: None,
        }
    );

    if single_use {
        let going_away = test_context.client.going_away_count.clone();
        tokio::time::timeout(Duration::from_secs(5), test_context.finish())
            .await
            .map_err(|_| make_input_err!("Single-use worker did not exit"))??;
        assert_eq!(going_away.load(Ordering::Relaxed), 1);
    }

    Ok(())
}

#[nativelink_test]
async fn new_local_worker_creates_work_directory_test() -> Result<(), Error> {
    let cas_store = Store::new(FastSlowStore::new(
        &FastSlowSpec {
            // Note: These are not needed for this test, so we put dummy memory stores here.
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
            bypass_dedup_threshold_bytes: 0,
            trust_fast_store_for_has: false,
        },
        Store::new(
            <FilesystemStore>::new(&FilesystemSpec {
                content_path: make_temp_path("content_path"),
                temp_path: make_temp_path("temp_path"),
                ..Default::default()
            })
            .await?,
        ),
        Store::new(MemoryStore::new(&MemorySpec::default())),
    ));
    let ac_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let work_directory = make_temp_path("foo");
    new_local_worker(
        Arc::new(LocalWorkerConfig {
            work_directory: work_directory.clone(),
            ..Default::default()
        }),
        cas_store.clone(),
        Some(ac_store),
        cas_store,
    )
    .await?;

    assert!(
        fs::metadata(work_directory).await.is_ok(),
        "Expected work_directory to be created"
    );

    Ok(())
}

#[nativelink_test]
async fn new_local_worker_removes_work_directory_before_start_test() -> Result<(), Error> {
    let cas_store = Store::new(FastSlowStore::new(
        &FastSlowSpec {
            // Note: These are not needed for this test, so we put dummy memory stores here.
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
            bypass_dedup_threshold_bytes: 0,
            trust_fast_store_for_has: false,
        },
        Store::new(
            <FilesystemStore>::new(&FilesystemSpec {
                content_path: make_temp_path("content_path"),
                temp_path: make_temp_path("temp_path"),
                ..Default::default()
            })
            .await?,
        ),
        Store::new(MemoryStore::new(&MemorySpec::default())),
    ));
    let ac_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let work_directory = make_temp_path("foo");
    fs::create_dir_all(format!("{}/{}", work_directory, "another_dir")).await?;
    let mut file =
        fs::create_file(OsString::from(format!("{}/{}", work_directory, "foo.txt"))).await?;
    file.write_all(b"Hello, world!").await?;
    file.as_mut().sync_all().await?;
    drop(file);
    new_local_worker(
        Arc::new(LocalWorkerConfig {
            work_directory: work_directory.clone(),
            ..Default::default()
        }),
        cas_store.clone(),
        Some(ac_store),
        cas_store,
    )
    .await?;

    let work_directory_path_buf = PathBuf::from(work_directory);

    assert!(
        work_directory_path_buf.read_dir()?.next().is_none(),
        "Expected work_directory to have removed all files and to be empty"
    );

    Ok(())
}

#[nativelink_test]
async fn experimental_precondition_script_fails() -> Result<(), Error> {
    #[cfg(target_family = "unix")]
    const EXPECTED_MSG: &str = "Preconditions script returned status exit status: 1 - ";
    #[cfg(target_family = "windows")]
    const EXPECTED_MSG: &str = "Preconditions script returned status exit code: 1 - ";

    let temp_path = make_temp_path("scripts");
    fs::create_dir_all(temp_path.clone()).await?;
    #[cfg(target_family = "unix")]
    let precondition_script = {
        let precondition_script = format!("{temp_path}/precondition.sh");
        let precondition_script_tmp = format!("{precondition_script}.tmp");

        // We use std::fs::File here because we sometimes get strange bugs here
        // that result in: "Text file busy (os error 26)" if it is an executable.
        // It is likely because somewhere the file descriptor does not get closed
        // in tokio's async context.
        {
            // We write to a temporary file and then rename it to force the kernel
            // to flush all related file descriptors fully before we use it.
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o777)
                .open(OsString::from(&precondition_script_tmp))
                .unwrap();
            file.write_all(b"#!/bin/sh\nexit 1\n").unwrap();
            file.sync_all().unwrap();
            // Note: Github runners appear to use some kind of filesystem driver
            // that does not sync data as expected. This is the easiest solution.
            // See: https://github.com/pantsbuild/pants/issues/10507
            // See: https://github.com/moby/moby/issues/9547
            std::process::Command::new("sync").output().unwrap();
        }
        std::fs::rename(&precondition_script_tmp, &precondition_script).unwrap();
        // Add a small delay to ensure the file system has fully released the file
        // This helps avoid "Text file busy" errors on some Linux environments
        sleep(Duration::from_millis(100)).await;
        precondition_script
    };
    #[cfg(target_family = "windows")]
    let precondition_script = {
        let precondition_script = format!("{}/precondition.bat", temp_path);
        let mut file = std::fs::File::create(OsString::from(&precondition_script))?;
        file.write_all(b"@echo off\r\nexit 1")?;
        file.sync_all().unwrap();
        precondition_script
    };

    let local_worker_config = LocalWorkerConfig {
        experimental_precondition_script: Some(precondition_script),
        ..Default::default()
    };

    let mut test_context = setup_local_worker_with_config(local_worker_config).await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();

    {
        // Ensure our worker connects and properties were sent.
        let props = test_context
            .client
            .expect_connect_worker(Ok(streaming_response))
            .await;
        assert_eq!(
            props,
            ConnectWorkerRequest {
                admits_when_idle: memory_is_limited(),
                ..Default::default()
            }
        );
    }

    let expected_worker_id = "foobar".to_string();

    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    {
        // First initialize our worker by sending the response to the connection request.
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::ConnectionResult(ConnectionResult {
                        worker_id: expected_worker_id.clone(),
                        dispatch_ack: false,
                        memory_property: String::new(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }

    let action_digest = DigestInfo::new([3u8; 32], 10);
    let action_info = ActionInfo {
        command_digest: DigestInfo::new([1u8; 32], 10),
        input_root_digest: DigestInfo::new([2u8; 32], 10),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: action_digest,
        }),
    };

    {
        // Send execution request.
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::StartAction(StartExecute {
                        request_metadata: None,
                        execute_request: Some((&action_info).into()),
                        operation_id: String::new(),
                        queued_timestamp: None,
                        platform: Some(Platform::default()),
                        worker_id: expected_worker_id.clone(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }

    // Now our client should be notified that our runner finished.
    let execution_response = test_context.client.expect_execution_response(Ok(())).await;

    // Now ensure the final results match our expectations.
    assert_eq!(
        execution_response,
        ExecuteResult {
            instance_name: INSTANCE_NAME.to_string(),
            operation_id: String::new(),
            result: Some(execute_result::Result::InternalError(
                make_err!(Code::ResourceExhausted, "{}", EXPECTED_MSG,).into()
            )),
            resource_usage: None,
        }
    );

    Ok(())
}

#[nativelink_test]
async fn kill_action_request_kills_action() -> Result<(), Error> {
    let mut test_context = setup_local_worker(HashMap::new()).await;

    let streaming_response = test_context.maybe_streaming_response.take().unwrap();

    {
        // Ensure our worker connects and properties were sent.
        let props = test_context
            .client
            .expect_connect_worker(Ok(streaming_response))
            .await;
        assert_eq!(
            props,
            ConnectWorkerRequest {
                admits_when_idle: memory_is_limited(),
                ..Default::default()
            }
        );
    }

    let expected_worker_id = "foobar".to_string();

    // Handle registration (kill_all not called unless registered).
    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    {
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::ConnectionResult(ConnectionResult {
                        worker_id: expected_worker_id.clone(),
                        dispatch_ack: false,
                        memory_property: String::new(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }

    let action_digest = DigestInfo::new([3u8; 32], 10);
    let action_info = ActionInfo {
        command_digest: DigestInfo::new([1u8; 32], 10),
        input_root_digest: DigestInfo::new([2u8; 32], 10),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Blake3,
            digest: action_digest,
        }),
    };

    let operation_id = OperationId::default();
    {
        // Send execution request.
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::StartAction(StartExecute {
                        request_metadata: None,
                        execute_request: Some((&action_info).into()),
                        operation_id: operation_id.to_string(),
                        queued_timestamp: None,
                        platform: Some(Platform::default()),
                        worker_id: expected_worker_id.clone(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }
    let running_action = Arc::new(MockRunningAction::new());

    // Send and wait for response from create_and_add_action to RunningActionsManager.
    test_context
        .actions_manager
        .expect_create_and_add_action(Ok(running_action.clone()))
        .await;

    {
        // Send kill request.
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::KillOperationRequest(KillOperationRequest {
                        operation_id: operation_id.to_string(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }

    let killed_operation_id = test_context.actions_manager.expect_kill_operation().await;

    // Make sure that the killed action is the one we intended
    assert_eq!(killed_operation_id, operation_id);

    Ok(())
}

#[nativelink_test]
async fn cas_not_found_returns_failed_precondition_test() -> Result<(), Error> {
    let mut test_context = setup_local_worker(HashMap::new()).await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();

    {
        let props = test_context
            .client
            .expect_connect_worker(Ok(streaming_response))
            .await;
        assert_eq!(
            props,
            ConnectWorkerRequest {
                admits_when_idle: memory_is_limited(),
                ..Default::default()
            }
        );
    }

    let expected_worker_id = "foobar".to_string();

    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    {
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::ConnectionResult(ConnectionResult {
                        worker_id: expected_worker_id.clone(),
                        dispatch_ack: false,
                        memory_property: String::new(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }

    let action_digest = DigestInfo::new([3u8; 32], 10);
    let action_info = ActionInfo {
        command_digest: DigestInfo::new([1u8; 32], 10),
        input_root_digest: DigestInfo::new([2u8; 32], 10),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: action_digest,
        }),
    };

    {
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::StartAction(StartExecute {
                        request_metadata: None,
                        execute_request: Some((&action_info).into()),
                        operation_id: String::new(),
                        queued_timestamp: None,
                        platform: Some(Platform::default()),
                        worker_id: expected_worker_id.clone(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }

    let running_action = Arc::new(MockRunningAction::new());

    // Send and wait for response from create_and_add_action.
    test_context
        .actions_manager
        .expect_create_and_add_action(Ok(running_action.clone()))
        .await;

    // Simulate prepare_action failing the way the manager fails a missing
    // input: NotFound tagged with the missing-input tip, whatever the store
    // itself said.
    let missing_input = make_err!(Code::NotFound, "Hash 0123456789abcdef not found")
        .append(MISSING_INPUT_ERROR_TIP)
        .with_context(ErrorContext::MissingDigest {
            hash: "0123456789abcdef".to_string(),
            size: 42,
        });
    running_action
        .expect_prepare_action(Err(missing_input.clone()))
        .await?;

    // Cleanup is still called even when prepare_action fails.
    running_action.cleanup(Ok(())).await?;

    // The worker should respond with FailedPrecondition wrapped in an ExecuteResponse,
    // NOT an InternalError. This allows Bazel to re-upload the missing artifacts.
    let execution_response = test_context.client.expect_execution_response(Ok(())).await;

    // The digest rides along as context, which the execute response turns
    // into the PreconditionFailure detail Bazel re-uploads on.
    let expected_action_result = ActionResult {
        error: Some(
            make_err!(
                Code::FailedPrecondition,
                "{}",
                missing_input.message_string()
            )
            .with_context(missing_input.context.clone()),
        ),
        ..ActionResult::default()
    };
    assert_eq!(
        execution_response,
        ExecuteResult {
            instance_name: INSTANCE_NAME.to_string(),
            operation_id: String::new(),
            result: Some(execute_result::Result::ExecuteResponse(
                ActionStage::Completed(expected_action_result).into()
            )),
            resource_usage: None,
        }
    );

    Ok(())
}

#[nativelink_test]
async fn non_cas_not_found_returns_internal_error_test() -> Result<(), Error> {
    let mut test_context = setup_local_worker(HashMap::new()).await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();

    {
        let props = test_context
            .client
            .expect_connect_worker(Ok(streaming_response))
            .await;
        assert_eq!(
            props,
            ConnectWorkerRequest {
                admits_when_idle: memory_is_limited(),
                ..Default::default()
            }
        );
    }

    let expected_worker_id = "foobar".to_string();

    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    {
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::ConnectionResult(ConnectionResult {
                        worker_id: expected_worker_id.clone(),
                        dispatch_ack: false,
                        memory_property: String::new(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }

    let action_digest = DigestInfo::new([3u8; 32], 10);
    let action_info = ActionInfo {
        command_digest: DigestInfo::new([1u8; 32], 10),
        input_root_digest: DigestInfo::new([2u8; 32], 10),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: action_digest,
        }),
    };

    {
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::StartAction(StartExecute {
                        request_metadata: None,
                        execute_request: Some((&action_info).into()),
                        operation_id: String::new(),
                        queued_timestamp: None,
                        platform: Some(Platform::default()),
                        worker_id: expected_worker_id.clone(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }

    let running_action = Arc::new(MockRunningAction::new());

    test_context
        .actions_manager
        .expect_create_and_add_action(Ok(running_action.clone()))
        .await;

    // Simulate prepare_action failing with a NotFound error that does NOT contain
    // the CAS-specific message. This should result in an InternalError, not
    // FailedPrecondition.
    let other_not_found_error = make_err!(Code::NotFound, "Some other resource was not found");
    running_action
        .expect_prepare_action(Err(other_not_found_error.clone()))
        .await?;

    // Cleanup is still called even when prepare_action fails.
    running_action.cleanup(Ok(())).await?;

    // The worker should respond with InternalError since this is not a CAS blob miss.
    let execution_response = test_context.client.expect_execution_response(Ok(())).await;

    assert_eq!(
        execution_response,
        ExecuteResult {
            instance_name: INSTANCE_NAME.to_string(),
            operation_id: String::new(),
            result: Some(execute_result::Result::InternalError(
                other_not_found_error.into()
            )),
            resource_usage: None,
        }
    );

    Ok(())
}

#[cfg(target_family = "unix")]
#[nativelink_test]
async fn preconditions_met_extra_envs() -> Result<(), Error> {
    let mut extra_envs = HashMap::new();
    extra_envs.insert("DEMO_ENV".into(), "test_value_for_demo_env".into());

    // So we have bash for nix cases, because the PATH gets reset
    extra_envs.insert("PATH".into(), env::var("PATH").unwrap());

    preconditions_met(
        Some("bash -c \"echo $DEMO_ENV\"".to_string()),
        &extra_envs,
        Duration::from_secs(30),
    )
    .await?;
    assert!(logs_contain("test_value_for_demo_env"));
    Ok(())
}

#[nativelink_test]
async fn keep_alive_fail_logs() -> Result<(), Error> {
    let local_worker_config = LocalWorkerConfig {
        platform_properties: HashMap::new(),
        worker_api_endpoint: EndpointConfig {
            timeout: Some(0.01),
            ..Default::default()
        },
        ..Default::default()
    };

    let mut test_context = setup_local_worker_with_config(local_worker_config).await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();

    // Ensure our worker connects and properties were sent.
    let props = test_context
        .client
        .expect_connect_worker(Ok(streaming_response))
        .await;
    assert_eq!(
        props,
        ConnectWorkerRequest {
            admits_when_idle: memory_is_limited(),
            ..Default::default()
        }
    );

    // handle connection result to scheduler
    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    tx_stream
        .send(Frame::data(
            encode_stream_proto(&UpdateForWorker {
                update: Some(Update::ConnectionResult(ConnectionResult {
                    worker_id: "foobar".to_string(),
                    dispatch_ack: false,
                    memory_property: String::new(),
                })),
            })
            .unwrap(),
        ))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;

    for _ in 0..30 {
        if logs_contain("Started KeepAlive timeout=0.0") // plus some extra digits, because floating point fun
            && logs_contain("nativelink_worker::local_worker: Sent KeepAlive") // first one succeeds
            && logs_contain(
                "Failed to send KeepAlive in LocalWorker e=Error { code: Internal, messages: [\"KeepAlive fail\"] }", // second should fail
            )
        {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }
    Err(make_err!(
        Code::DeadlineExceeded,
        "Timed out looking for KeepAlive logs"
    ))
}

/// A keepalive that never completes does not end the connection by itself:
/// a send waits behind whatever the scheduler is still processing, and the
/// scheduler's liveness timeout is the clock. When the scheduler closes the
/// stream, the loop sees it, the running actions are killed, and the worker
/// registers again.
#[nativelink_test]
async fn a_hung_keepalive_waits_for_the_scheduler_to_close_the_stream() -> Result<(), Error> {
    let local_worker_config = LocalWorkerConfig {
        platform_properties: HashMap::new(),
        worker_api_endpoint: EndpointConfig {
            timeout: Some(0.2),
            ..Default::default()
        },
        ..Default::default()
    };

    let mut test_context = setup_local_worker_with_config(local_worker_config).await;
    test_context
        .client
        .keep_alive_hangs
        .store(true, Ordering::Release);
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();
    test_context
        .client
        .expect_connect_worker(Ok(streaming_response))
        .await;
    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    tx_stream
        .send(Frame::data(
            encode_stream_proto(&UpdateForWorker {
                update: Some(Update::ConnectionResult(ConnectionResult {
                    worker_id: "foobar".to_string(),
                    dispatch_ack: false,
                    memory_property: String::new(),
                })),
            })
            .unwrap(),
        ))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;

    // The first keepalive is held and stays held: no deadline on it ends the
    // connection.
    while test_context
        .client
        .keep_alives_hanging
        .load(Ordering::Acquire)
        == 0
    {
        sleep(Duration::from_millis(10)).await;
    }
    assert!(
        tokio::time::timeout(
            Duration::from_millis(500),
            test_context.actions_manager.expect_kill_all()
        )
        .await
        .is_err(),
        "a hung keepalive must not end the connection"
    );

    // The scheduler closes the stream (it evicted the worker): the loop sees
    // it, the actions are killed, and the worker comes back to register.
    drop(tx_stream);
    tokio::time::timeout(Duration::from_secs(10), async {
        test_context.actions_manager.expect_kill_all().await;
        let (tx_stream2, streaming_response2) = setup_grpc_stream();
        test_context
            .client
            .expect_connect_worker(Ok(streaming_response2))
            .await;
        drop(tx_stream2);
    })
    .await
    .map_err(|_| make_err!(Code::DeadlineExceeded, "the worker did not reconnect"))?;
    assert!(logs_contain(
        "Worker disconnected from scheduler, reconnecting"
    ));
    Ok(())
}

/// The run loop is the only reader of the scheduler's stream, so no call to
/// the scheduler is awaited inside it: a dispatch acknowledgement that hangs
/// leaves the loop reading, and a kill that arrives next is still acted on.
#[nativelink_test]
async fn a_hung_acknowledgement_does_not_block_the_loop() -> Result<(), Error> {
    let mut test_context = setup_local_worker_with_config(LocalWorkerConfig {
        max_inflight_tasks: 2,
        worker_api_endpoint: EndpointConfig {
            timeout: Some(10000.),
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();
    test_context
        .client
        .expect_connect_worker(Ok(streaming_response))
        .await;

    let worker_id = "foobar".to_string();
    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    tx_stream
        .send(Frame::data(
            encode_stream_proto(&UpdateForWorker {
                update: Some(Update::ConnectionResult(ConnectionResult {
                    worker_id: worker_id.clone(),
                    dispatch_ack: true,
                    memory_property: String::new(),
                })),
            })
            .unwrap(),
        ))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;

    let action_info = ActionInfo {
        command_digest: DigestInfo::new([1u8; 32], 10),
        input_root_digest: DigestInfo::new([2u8; 32], 10),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Blake3,
            digest: DigestInfo::new([3u8; 32], 10),
        }),
    };
    // The dispatch's acknowledgement is never answered: the mock holds it.
    tx_stream
        .send(Frame::data(
            encode_stream_proto(&UpdateForWorker {
                update: Some(Update::StartAction(StartExecute {
                    request_metadata: None,
                    execute_request: Some((&action_info).into()),
                    operation_id: "first".to_string(),
                    queued_timestamp: None,
                    platform: Some(Platform::default()),
                    worker_id: worker_id.clone(),
                })),
            })
            .unwrap(),
        ))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;

    // A kill for it arrives while the acknowledgement hangs; the loop reads
    // it and passes it on.
    tx_stream
        .send(Frame::data(
            encode_stream_proto(&UpdateForWorker {
                update: Some(Update::KillOperationRequest(KillOperationRequest {
                    operation_id: "first".to_string(),
                })),
            })
            .unwrap(),
        ))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    let killed = tokio::time::timeout(
        Duration::from_secs(10),
        test_context.actions_manager.expect_kill_operation(),
    )
    .await
    .map_err(|_| make_err!(Code::DeadlineExceeded, "the loop never read the kill"))?;
    assert_eq!(killed, OperationId::from("first"));
    // The action waits on its acknowledgement; it has not started.
    assert!(
        tokio::time::timeout(
            Duration::from_millis(300),
            test_context
                .actions_manager
                .expect_create_and_add_action_no_reply()
        )
        .await
        .is_err(),
        "the action must not start before its acknowledgement is sent"
    );
    Ok(())
}

/// An acknowledgement that fails leaves the action unstarted and a
/// single-use worker unspent: the loop ends on the error, the worker
/// registers again, and the next dispatch runs.
#[nativelink_test]
async fn a_failed_acknowledgement_leaves_a_single_use_worker_unspent() -> Result<(), Error> {
    let mut test_context = setup_local_worker_with_config(LocalWorkerConfig {
        single_use: true,
        max_inflight_tasks: 1,
        worker_api_endpoint: EndpointConfig {
            timeout: Some(10000.),
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();
    test_context
        .client
        .expect_connect_worker(Ok(streaming_response))
        .await;

    let worker_id = "foobar".to_string();
    let connection_result = UpdateForWorker {
        update: Some(Update::ConnectionResult(ConnectionResult {
            worker_id: worker_id.clone(),
            dispatch_ack: true,
            memory_property: String::new(),
        })),
    };
    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    tx_stream
        .send(Frame::data(
            encode_stream_proto(&connection_result).unwrap(),
        ))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;

    let action_info = ActionInfo {
        command_digest: DigestInfo::new([1u8; 32], 10),
        input_root_digest: DigestInfo::new([2u8; 32], 10),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Blake3,
            digest: DigestInfo::new([3u8; 32], 10),
        }),
    };
    let start = |operation_id: &str| UpdateForWorker {
        update: Some(Update::StartAction(StartExecute {
            request_metadata: None,
            execute_request: Some((&action_info).into()),
            operation_id: operation_id.to_string(),
            queued_timestamp: None,
            platform: Some(Platform::default()),
            worker_id: worker_id.clone(),
        })),
    };

    // The acknowledgement of the first dispatch is lost.
    tx_stream
        .send(Frame::data(encode_stream_proto(&start("first")).unwrap()))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    let accepted = test_context
        .client
        .expect_execute_accepted(Err(make_err!(Code::Unavailable, "acknowledgement lost")))
        .await;
    assert_eq!(accepted.operation_id, "first");

    // The loop ends on it and the worker, not spent, registers again.
    let tx_stream2 = tokio::time::timeout(Duration::from_secs(10), async {
        test_context.actions_manager.expect_kill_all().await;
        let (tx_stream2, streaming_response2) = setup_grpc_stream();
        test_context
            .client
            .expect_connect_worker(Ok(streaming_response2))
            .await;
        tx_stream2
    })
    .await
    .map_err(|_| make_err!(Code::DeadlineExceeded, "the worker did not reconnect"))?;
    tx_stream2
        .send(Frame::data(
            encode_stream_proto(&connection_result).unwrap(),
        ))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;

    // The next dispatch is acknowledged and is the first action to start.
    tx_stream2
        .send(Frame::data(encode_stream_proto(&start("second")).unwrap()))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    let accepted = test_context.client.expect_execute_accepted(Ok(())).await;
    assert_eq!(accepted.operation_id, "second");
    let (_, started) = test_context
        .actions_manager
        .expect_create_and_add_action(Ok(Arc::new(MockRunningAction::new())))
        .await;
    assert_eq!(started.operation_id, "second");
    assert!(logs_contain("Could not send ExecuteAccepted"));
    Ok(())
}

/// Regression test: a disconnect from the scheduler while an action is still
/// "in transit" (`StartAction` received, inputs still downloading) must lead to
/// kill-all + reconnect like any other disconnect. It used to strand the
/// `actions_in_transit` counter — the decrement lived inside the action
/// future, which is aborted by the disconnect — so the drain loop always
/// timed out and `LocalWorker::run` returned a fatal error that took down the
/// whole process in colocated deployments.
#[nativelink_test]
async fn disconnect_with_action_in_transit_reconnects_test() -> Result<(), Error> {
    disconnect_with_action_in_transit(false).await
}

#[nativelink_test]
async fn single_use_worker_never_reconnects_after_accepting_an_action() -> Result<(), Error> {
    disconnect_with_action_in_transit(true).await
}

async fn disconnect_with_action_in_transit(single_use: bool) -> Result<(), Error> {
    let mut test_context = setup_local_worker_with_config(LocalWorkerConfig {
        single_use,
        worker_api_endpoint: EndpointConfig {
            timeout: Some(10000.),
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();

    {
        // Ensure our worker connects and properties were sent.
        let props = test_context
            .client
            .expect_connect_worker(Ok(streaming_response))
            .await;
        assert_eq!(
            props,
            ConnectWorkerRequest {
                max_inflight_tasks: u64::from(single_use),
                admits_when_idle: memory_is_limited(),
                ..Default::default()
            }
        );
    }

    let expected_worker_id = "foobar".to_string();

    // Handle registration.
    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    {
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::ConnectionResult(ConnectionResult {
                        worker_id: expected_worker_id.clone(),
                        dispatch_ack: false,
                        memory_property: String::new(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }

    let action_digest = DigestInfo::new([3u8; 32], 10);
    let action_info = ActionInfo {
        command_digest: DigestInfo::new([1u8; 32], 10),
        input_root_digest: DigestInfo::new([2u8; 32], 10),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: action_digest,
        }),
    };

    {
        // Send execution request.
        tx_stream
            .send(Frame::data(
                encode_stream_proto(&UpdateForWorker {
                    update: Some(Update::StartAction(StartExecute {
                        request_metadata: None,
                        execute_request: Some((&action_info).into()),
                        operation_id: String::new(),
                        queued_timestamp: None,
                        platform: Some(Platform::default()),
                        worker_id: expected_worker_id.clone(),
                    })),
                })
                .unwrap(),
            ))
            .await
            .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    }

    // Wait until the action reaches create_and_add_action, but never answer:
    // the action stays in transit, like an input download in progress.
    test_context
        .actions_manager
        .expect_create_and_add_action_no_reply()
        .await;

    // Sever the scheduler connection while the action is still in transit.
    drop(tx_stream);

    tokio::time::timeout(Duration::from_secs(10), async {
        // The worker must clean up...
        test_context.actions_manager.expect_kill_all().await;

        if single_use {
            assert!(test_context.finish().await.is_err());
            return;
        }

        // ...and auto reconnect, checking our properties again.
        let (_, streaming_response) = setup_grpc_stream();
        let props = test_context
            .client
            .expect_connect_worker(Ok(streaming_response))
            .await;
        assert_eq!(
            props,
            ConnectWorkerRequest {
                admits_when_idle: memory_is_limited(),
                ..Default::default()
            }
        );
    })
    .await
    .map_err(|_| {
        make_input_err!(
            "Worker did not kill-all and reconnect after a disconnect with an action in transit"
        )
    })?;

    Ok(())
}

/// With a scheduler that speaks the acknowledgement, a worker at its
/// `max_inflight_tasks` declines the next dispatch instead of running it,
/// and says why; the first dispatch was acknowledged before it ran.
#[nativelink_test]
async fn a_worker_at_capacity_declines_the_next_dispatch() -> Result<(), Error> {
    let mut test_context = setup_local_worker_with_config(LocalWorkerConfig {
        max_inflight_tasks: 1,
        worker_api_endpoint: EndpointConfig {
            timeout: Some(10000.),
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();
    test_context
        .client
        .expect_connect_worker(Ok(streaming_response))
        .await;

    let worker_id = "foobar".to_string();
    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    tx_stream
        .send(Frame::data(
            encode_stream_proto(&UpdateForWorker {
                update: Some(Update::ConnectionResult(ConnectionResult {
                    worker_id: worker_id.clone(),
                    dispatch_ack: true,
                    memory_property: String::new(),
                })),
            })
            .unwrap(),
        ))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;

    let action_info = ActionInfo {
        command_digest: DigestInfo::new([1u8; 32], 10),
        input_root_digest: DigestInfo::new([2u8; 32], 10),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Blake3,
            digest: DigestInfo::new([3u8; 32], 10),
        }),
    };
    let start = |operation_id: &str| UpdateForWorker {
        update: Some(Update::StartAction(StartExecute {
            request_metadata: None,
            execute_request: Some((&action_info).into()),
            operation_id: operation_id.to_string(),
            queued_timestamp: None,
            platform: Some(Platform::default()),
            worker_id: worker_id.clone(),
        })),
    };

    // The first dispatch is acknowledged, then runs (and stays running:
    // nothing answers its prepare).
    tx_stream
        .send(Frame::data(encode_stream_proto(&start("first")).unwrap()))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    let accepted = test_context.client.expect_execute_accepted(Ok(())).await;
    assert_eq!(accepted.operation_id, "first");
    let running_action = Arc::new(MockRunningAction::new());
    test_context
        .actions_manager
        .expect_create_and_add_action(Ok(running_action.clone()))
        .await;

    // The second finds the worker full and is declined with the reason.
    tx_stream
        .send(Frame::data(encode_stream_proto(&start("second")).unwrap()))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    let declined = test_context.client.expect_execute_declined(Ok(())).await;
    assert_eq!(declined.operation_id, "second");
    assert_eq!(declined.reason, execute_declined::Reason::AtCapacity as i32);
    assert_eq!(declined.detail, "1 of 1 in flight");
    Ok(())
}

/// A worker connected to a scheduler that speaks the acknowledgement and
/// reads memory reservations from `memory_kb`, as worker "foobar".
async fn connect_memory_worker(config: LocalWorkerConfig) -> Result<TestContext, Error> {
    let mut test_context = setup_local_worker_with_config(LocalWorkerConfig {
        worker_api_endpoint: EndpointConfig {
            timeout: Some(10000.),
            ..Default::default()
        },
        ..config
    })
    .await;
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();
    test_context
        .client
        .expect_connect_worker(Ok(streaming_response))
        .await;
    send_update(
        &test_context,
        &UpdateForWorker {
            update: Some(Update::ConnectionResult(ConnectionResult {
                worker_id: "foobar".to_string(),
                dispatch_ack: true,
                memory_property: "memory_kb".to_string(),
            })),
        },
    )
    .await?;
    Ok(test_context)
}

async fn send_update(test_context: &TestContext, update: &UpdateForWorker) -> Result<(), Error> {
    test_context
        .maybe_tx_stream
        .as_ref()
        .unwrap()
        .send(Frame::data(encode_stream_proto(update).unwrap()))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))
}

/// A dispatch to worker "foobar" of `operation_id`, reserving `memory_kb`.
fn start_reserving(operation_id: &str, memory_kb: u64) -> UpdateForWorker {
    let action_info = ActionInfo {
        command_digest: DigestInfo::new([1u8; 32], 10),
        input_root_digest: DigestInfo::new([2u8; 32], 10),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Blake3,
            digest: DigestInfo::new([3u8; 32], 10),
        }),
    };
    UpdateForWorker {
        update: Some(Update::StartAction(StartExecute {
            request_metadata: None,
            execute_request: Some((&action_info).into()),
            operation_id: operation_id.to_string(),
            queued_timestamp: None,
            platform: Some(Platform {
                properties: vec![Property {
                    name: "memory_kb".to_string(),
                    value: memory_kb.to_string(),
                }],
            }),
            worker_id: "foobar".to_string(),
        })),
    }
}

/// Under a cgroup memory limit an idle worker admits an action whatever it
/// reads free: with nothing else in flight, waiting frees nothing, and a
/// decline would have the scheduler wait for a figure the worker never
/// reports. Once it holds an action, the same reservation is declined for
/// load. Without a limit, free memory is the host's and can come back, so
/// even an idle worker declines. With no free-memory reading at all (not
/// Linux), nothing is ever refused for load, so there is nothing to see.
#[nativelink_test]
async fn an_idle_worker_under_a_limit_admits_what_a_busy_one_declines() -> Result<(), Error> {
    if free_memory_kb().is_none() {
        return Ok(());
    }
    let test_context = connect_memory_worker(LocalWorkerConfig::default()).await?;

    // More memory than any machine has free.
    send_update(&test_context, &start_reserving("idle", u64::MAX)).await?;
    if !memory_is_limited() {
        let declined = test_context.client.expect_execute_declined(Ok(())).await;
        assert_eq!(declined.operation_id, "idle");
        assert_eq!(declined.reason, execute_declined::Reason::Load as i32);
        return Ok(());
    }
    // Idle: admitted, and it stays running (nothing answers its prepare).
    let accepted = test_context.client.expect_execute_accepted(Ok(())).await;
    assert_eq!(accepted.operation_id, "idle");
    test_context
        .actions_manager
        .expect_create_and_add_action(Ok(Arc::new(MockRunningAction::new())))
        .await;

    // Busy: the same reservation is declined for load.
    send_update(&test_context, &start_reserving("busy", u64::MAX)).await?;
    let declined = test_context.client.expect_execute_declined(Ok(())).await;
    assert_eq!(declined.operation_id, "busy");
    assert_eq!(declined.reason, execute_declined::Reason::Load as i32);
    assert_eq!(declined.needed_kb, u64::MAX);
    Ok(())
}

/// A single-use worker is always idle when its one action arrives, so
/// under a memory limit, or with no free-memory reading, it admits that
/// action whatever it reads free. Only a worker reading the host's memory
/// still declines it, and a decline does not spend it: the next dispatch
/// is still admitted.
#[nativelink_test]
async fn a_single_use_worker_admits_its_action_unless_host_memory_says_no() -> Result<(), Error> {
    let test_context = connect_memory_worker(LocalWorkerConfig {
        single_use: true,
        ..Default::default()
    })
    .await?;

    send_update(&test_context, &start_reserving("greedy", u64::MAX)).await?;
    if free_memory_kb().is_some() && !memory_is_limited() {
        let declined = test_context.client.expect_execute_declined(Ok(())).await;
        assert_eq!(declined.operation_id, "greedy");
        assert_eq!(declined.reason, execute_declined::Reason::Load as i32);
        // Not spent: an action reserving nothing is admitted next.
        send_update(&test_context, &start_reserving("modest", 0)).await?;
        let accepted = test_context.client.expect_execute_accepted(Ok(())).await;
        assert_eq!(accepted.operation_id, "modest");
        test_context
            .actions_manager
            .expect_create_and_add_action(Ok(Arc::new(MockRunningAction::new())))
            .await;
        return Ok(());
    }
    let accepted = test_context.client.expect_execute_accepted(Ok(())).await;
    assert_eq!(accepted.operation_id, "greedy");
    test_context
        .actions_manager
        .expect_create_and_add_action(Ok(Arc::new(MockRunningAction::new())))
        .await;
    Ok(())
}

/// A precondition script that hangs is refused as backpressure after the
/// timeout instead of holding the action forever.
#[cfg(target_family = "unix")]
#[nativelink_test]
async fn precondition_script_that_hangs_times_out() -> Result<(), Error> {
    let extra_envs: HashMap<String, String> = HashMap::new();
    // A busy loop rather than `sleep`: the script runs with a cleared
    // environment, and in a build sandbox only `/bin/sh` is on any path.
    let err = preconditions_met(
        Some("/bin/sh -c 'while :; do :; done'".to_string()),
        &extra_envs,
        Duration::from_millis(200),
    )
    .await
    .expect_err("a hanging script must not pass");
    assert_eq!(err.code, Code::ResourceExhausted, "{err}");
    assert!(err.to_string().contains("did not finish"), "{err}");
    Ok(())
}

/// The readiness flag follows the registration: off until the scheduler's
/// `ConnectionResult`, on after it, off again when the connection is lost.
/// The lost state reads Initializing, not Failed, so only the readiness
/// check drops and a liveness probe on the plain status stays green.
#[nativelink_test]
async fn registration_flag_follows_the_scheduler_connection() -> Result<(), Error> {
    let mut test_context = setup_local_worker(HashMap::new()).await;
    assert!(!test_context.registration.is_registered());
    let streaming_response = test_context.maybe_streaming_response.take().unwrap();
    test_context
        .client
        .expect_connect_worker(Ok(streaming_response))
        .await;
    assert!(!test_context.registration.is_registered());

    let tx_stream = test_context.maybe_tx_stream.take().unwrap();
    tx_stream
        .send(Frame::data(
            encode_stream_proto(&UpdateForWorker {
                update: Some(Update::ConnectionResult(ConnectionResult {
                    worker_id: "foobar".to_string(),
                    dispatch_ack: false,
                    memory_property: String::new(),
                })),
            })
            .unwrap(),
        ))
        .await
        .map_err(|e| make_input_err!("Could not send : {:?}", e))?;
    let mut registered = false;
    for _ in 0..1_000 {
        if test_context.registration.is_registered() {
            registered = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(registered, "registration flag never turned on");

    drop(tx_stream);
    test_context.actions_manager.expect_kill_all().await;
    let mut lost = false;
    for _ in 0..1_000 {
        if !test_context.registration.is_registered() {
            lost = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(
        lost,
        "registration flag never turned off after the disconnect"
    );
    match test_context.registration.check_health("".into()).await {
        HealthStatus::Initializing { message, .. } => {
            assert!(message.contains("reconnecting"), "{message}");
        }
        other => panic!("a lost registration must read Initializing, got {other:?}"),
    }
    Ok(())
}
