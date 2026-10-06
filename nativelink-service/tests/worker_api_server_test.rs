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

use core::time::Duration;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use async_lock::Mutex as AsyncMutex;
use async_trait::async_trait;
use bytes::Bytes;
use nativelink_config::cas_server::WorkerApiConfig;
use nativelink_config::schedulers::WorkerAllocationStrategy;
use nativelink_error::{Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_proto::build::bazel::remote::execution::v2::{
    ActionResult as ProtoActionResult, ExecuteResponse, ExecutedActionMetadata, LogFile,
    OutputDirectory, OutputFile, OutputSymlink,
};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::update_for_scheduler::Update;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    execute_declined, execute_result, update_for_worker, ConnectWorkerRequest, ExecuteAccepted, ExecuteComplete, ExecuteDeclined, ExecuteResult, GoingAwayRequest, KeepAliveRequest, UpdateForScheduler, WorkerLoad
};
use nativelink_proto::google::rpc::Status as ProtoStatus;
use nativelink_scheduler::api_worker_scheduler::ApiWorkerScheduler;
use nativelink_scheduler::match_outcome::MatchOutcome;
use nativelink_scheduler::platform_property_manager::PlatformPropertyManager;
use nativelink_scheduler::worker::{ActionInfoWithProps, channel_capacity};
use nativelink_scheduler::worker_scheduler::{WorkerScheduler, WorkerSummary};
use nativelink_service::worker_api_server::{ConnectWorkerStream, NowFn, WorkerApiServer};
use nativelink_util::action_messages::{
    ActionInfo, ActionUniqueKey, ActionUniqueQualifier, OperationId, WorkerId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::operation_state_manager::{UpdateOperationType, WorkerStateManager};
use nativelink_util::origin_event::OriginMetadata;
use nativelink_util::platform_properties::PlatformProperties;
use pretty_assertions::assert_eq;
use tokio::join;
use tokio::sync::{Notify, mpsc};
use tokio_stream::StreamExt;
use nativelink_scheduler::worker_registry::WorkerRegistry;

const BASE_NOW_S: u64 = 10;
const BASE_WORKER_TIMEOUT_S: u64 = 100;

#[derive(Debug)]
enum WorkerStateManagerCalls {
    UpdateOperation((OperationId, WorkerId, UpdateOperationType)),
}

#[derive(Debug)]
enum WorkerStateManagerReturns {
    UpdateOperation(Result<(), Error>),
}

#[derive(MetricsComponent)]
struct MockWorkerStateManager {
    rx_call: Arc<AsyncMutex<mpsc::UnboundedReceiver<WorkerStateManagerCalls>>>,
    tx_call: mpsc::UnboundedSender<WorkerStateManagerCalls>,
    rx_resp: Arc<AsyncMutex<mpsc::UnboundedReceiver<WorkerStateManagerReturns>>>,
    tx_resp: mpsc::UnboundedSender<WorkerStateManagerReturns>,
    /// Operations the state manager no longer has executing on any worker.
    revoked: Mutex<HashSet<OperationId>>,
}

impl MockWorkerStateManager {
    pub(crate) fn new() -> Self {
        let (tx_call, rx_call) = mpsc::unbounded_channel();
        let (tx_resp, rx_resp) = mpsc::unbounded_channel();
        Self {
            rx_call: Arc::new(AsyncMutex::new(rx_call)),
            tx_call,
            rx_resp: Arc::new(AsyncMutex::new(rx_resp)),
            tx_resp,
            revoked: Mutex::new(HashSet::new()),
        }
    }

    /// From now on the operation is not executing on any worker.
    pub(crate) fn revoke(&self, operation_id: &OperationId) {
        self.revoked.lock().unwrap().insert(operation_id.clone());
    }

    pub(crate) async fn expect_update_operation(
        &self,
        result: Result<(), Error>,
    ) -> (OperationId, WorkerId, UpdateOperationType) {
        let mut rx_call_lock = self.rx_call.lock().await;
        let recv = rx_call_lock.recv();
        let WorkerStateManagerCalls::UpdateOperation(req) =
            recv.await.expect("Could not receive msg in mpsc");
        self.tx_resp
            .send(WorkerStateManagerReturns::UpdateOperation(result))
            .expect("Could not send request to mpsc");
        req
    }
}

#[async_trait]
impl WorkerStateManager for MockWorkerStateManager {
    async fn update_operation(
        &self,
        operation_id: &OperationId,
        worker_id: &WorkerId,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        self.tx_call
            .send(WorkerStateManagerCalls::UpdateOperation((
                operation_id.clone(),
                worker_id.clone(),
                update,
            )))
            .expect("Could not send request to mpsc");
        let mut rx_resp_lock = self.rx_resp.lock().await;
        match rx_resp_lock
            .recv()
            .await
            .expect("Could not receive msg in mpsc")
        {
            WorkerStateManagerReturns::UpdateOperation(result) => result,
        }
    }

    async fn is_executing_on_worker(
        &self,
        operation_id: &OperationId,
        _worker_id: &WorkerId,
    ) -> Result<bool, Error> {
        Ok(!self.revoked.lock().unwrap().contains(operation_id))
    }
}

struct TestContext {
    scheduler: Arc<ApiWorkerScheduler>,
    state_manager: Arc<MockWorkerStateManager>,
    _worker_api_server: WorkerApiServer,
    connection_worker_stream: ConnectWorkerStream,
    worker_id: WorkerId,
    worker_stream: mpsc::Sender<Update>,
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "`setup_api_server` requires a method that returns a `Result`"
)]
const fn static_now_fn() -> Result<Duration, Error> {
    Ok(Duration::from_secs(BASE_NOW_S))
}

async fn setup_api_server(worker_timeout: u64, now_fn: NowFn) -> Result<TestContext, Error> {
    setup_api_server_with_task_limit(worker_timeout, now_fn, 0).await
}

async fn setup_api_server_with_task_limit(
    worker_timeout: u64,
    now_fn: NowFn,
    max_worker_tasks: u64,
) -> Result<TestContext, Error> {
    setup_api_server_with(worker_timeout, now_fn, max_worker_tasks, 0).await
}

async fn setup_api_server_with(
    worker_timeout: u64,
    now_fn: NowFn,
    max_worker_tasks: u64,
    dispatch_ack_timeout_s: u64,
) -> Result<TestContext, Error> {
    const SCHEDULER_NAME: &str = "DUMMY_SCHEDULE_NAME";

    const UUID_SIZE: usize = 36;

    let platform_property_manager = Arc::new(PlatformPropertyManager::new(HashMap::new()));
    let tasks_or_worker_change_notify = Arc::new(Notify::new());
    let state_manager = Arc::new(MockWorkerStateManager::new());
    let worker_registry = Arc::new(WorkerRegistry::new());
    let scheduler = ApiWorkerScheduler::new(
        state_manager.clone(),
        platform_property_manager,
        WorkerAllocationStrategy::default(),
        None,
        None,
        tasks_or_worker_change_notify,
        worker_timeout,
        60, // unacknowledged_kill_timeout_s
        dispatch_ack_timeout_s,
        worker_registry,
        None,
        false,                   // has_peers
        Duration::from_secs(15), // record_ttl (unused while has_peers = false)
    );

    let mut schedulers: HashMap<String, Arc<dyn WorkerScheduler>> = HashMap::new();
    schedulers.insert(SCHEDULER_NAME.to_string(), scheduler.clone());
    let worker_api_server = WorkerApiServer::new_with_now_fn(
        &WorkerApiConfig {
            scheduler: SCHEDULER_NAME.to_string(),
            disable_kill_revoked_operations: false,
            kill_revoked_operations_interval_s: 0,
        },
        &schedulers,
        now_fn,
        [1u8; 6],
    )
    .err_tip(|| "Error creating WorkerApiServer")?;

    let connect_worker_request = ConnectWorkerRequest {
        max_inflight_tasks: max_worker_tasks,
        ..Default::default()
    };
    let (tx, rx) = mpsc::channel(1);
    tx.send(Update::ConnectWorkerRequest(connect_worker_request))
        .await
        .unwrap();
    let update_stream = Box::pin(futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|update| {
            let update = Ok(UpdateForScheduler {
                update: Some(update),
            });
            (update, rx)
        })
    }));
    let mut connection_worker_stream = worker_api_server
        .inner_connect_worker_for_testing(update_stream)
        .await?
        .into_inner();

    let maybe_first_message = connection_worker_stream.next().await;
    assert!(
        maybe_first_message.is_some(),
        "Expected first message from stream"
    );
    let first_update = maybe_first_message
        .unwrap()
        .err_tip(|| "Expected success result")?
        .update
        .err_tip(|| "Expected update field to be populated")?;
    let worker_id = match first_update {
        update_for_worker::Update::ConnectionResult(connection_result) => {
            connection_result.worker_id
        }
        other => unreachable!("Expected ConnectionResult, got {:?}", other),
    };

    assert_eq!(
        worker_id.len(),
        UUID_SIZE,
        "Worker ID should be 36 characters"
    );

    Ok(TestContext {
        scheduler,
        state_manager,
        _worker_api_server: worker_api_server,
        connection_worker_stream,
        worker_id: worker_id.into(),
        worker_stream: tx,
    })
}

#[nativelink_test]
pub async fn connect_worker_adds_worker_to_scheduler_test()
-> Result<(), Box<dyn core::error::Error>> {
    let test_context = setup_api_server(BASE_WORKER_TIMEOUT_S, Box::new(static_now_fn)).await?;

    let worker_exists = test_context
        .scheduler
        .contains_worker_for_test(&test_context.worker_id)
        .await;
    assert!(worker_exists, "Expected worker to exist in worker map");

    Ok(())
}

#[nativelink_test]
pub async fn server_times_out_workers_test() -> Result<(), Box<dyn core::error::Error>> {
    let test_context = setup_api_server(BASE_WORKER_TIMEOUT_S, Box::new(static_now_fn)).await?;

    let mut now_timestamp = BASE_NOW_S;
    {
        // Now change time to 1 second before timeout and ensure the worker is still in the pool.
        now_timestamp += BASE_WORKER_TIMEOUT_S - 1;
        test_context
            .scheduler
            .remove_timedout_workers(now_timestamp)
            .await?;
        let worker_exists = test_context
            .scheduler
            .contains_worker_for_test(&test_context.worker_id)
            .await;
        assert!(worker_exists, "Expected worker to exist in worker map");
    }
    {
        // Now add 1 second and our worker should have been evicted due to timeout.
        now_timestamp += 1;
        test_context
            .scheduler
            .remove_timedout_workers(now_timestamp)
            .await?;
        let worker_exists = test_context
            .scheduler
            .contains_worker_for_test(&test_context.worker_id)
            .await;
        assert!(!worker_exists, "Expected worker to not exist in map");
    }

    Ok(())
}

/// A result from the worker is proof of life. Before this, only a keepalive
/// refreshed the liveness timestamp, so a worker that spent the window
/// finishing actions was evicted with all of them requeued.
#[nativelink_test]
pub async fn server_does_not_timeout_if_execute_complete_test()
-> Result<(), Box<dyn core::error::Error>> {
    let now_timestamp = Arc::new(Mutex::new(BASE_NOW_S));
    let now_timestamp_clone = now_timestamp.clone();
    let add_and_return_timestamp = move |add_amount: u64| -> u64 {
        let mut locked_now_timestamp = now_timestamp.lock().unwrap();
        *locked_now_timestamp += add_amount;
        *locked_now_timestamp
    };

    let test_context = setup_api_server(
        BASE_WORKER_TIMEOUT_S,
        Box::new(move || Ok(Duration::from_secs(*now_timestamp_clone.lock().unwrap()))),
    )
    .await?;

    // Give the worker an action so it has something to report on.
    let action_digest = DigestInfo::new([7u8; 32], 123);
    let action_info = Arc::new(ActionInfo {
        command_digest: DigestInfo::new([0u8; 32], 0),
        input_root_digest: DigestInfo::new([0u8; 32], 0),
        timeout: Duration::MAX,
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: make_system_time(0),
        insert_timestamp: make_system_time(0),
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: "instance_name".to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: action_digest,
        }),
    });
    let operation_id = OperationId::default();
    let platform_properties = test_context
        .scheduler
        .get_platform_property_manager()
        .make_platform_properties(action_info.platform_properties.clone())?;
    test_context
        .scheduler
        .worker_notify_run_action(
            test_context.worker_id.clone(),
            operation_id.clone(),
            ActionInfoWithProps {
                inner: action_info,
                platform_properties,
                origin_metadata: OriginMetadata::default(),
                scheduler_start_execute_event_id: None,
            },
            BASE_NOW_S,
        )
        .await
        .unwrap();

    // One second before the timeout, with no keepalive sent, the worker
    // reports that the action's process has exited. `ExecuteResult` goes
    // through the same `touch_liveness` first.
    let _ = add_and_return_timestamp(BASE_WORKER_TIMEOUT_S - 1);
    test_context
        .worker_stream
        .send(Update::ExecuteComplete(ExecuteComplete {
            operation_id: operation_id.to_string(),
        }))
        .await
        .map_err(|e| make_err!(tonic::Code::Internal, "Error sending completion {e}"))?;
    tokio::time::sleep(Duration::from_millis(10)).await;

    // Past the original deadline the worker must still be in the pool,
    // because the result refreshed its liveness.
    // An eviction here would fail the running action through the state
    // manager and then wait for the mock's reply, so race the sweep
    // against that call to turn a regression into a failure instead of
    // a hang.
    let timestamp = add_and_return_timestamp(2);
    tokio::select! {
        sweep_result = test_context.scheduler.remove_timedout_workers(timestamp) => sweep_result?,
        _ = test_context.state_manager.expect_update_operation(Ok(())) => {
            panic!("worker was evicted although it had just reported a result");
        }
    }
    assert!(
        test_context
            .scheduler
            .contains_worker_for_test(&test_context.worker_id)
            .await,
        "worker was evicted although it had just reported a result"
    );

    // And it is still evicted once it goes quiet for a full window. The
    // eviction hands the running action back to the queue as a lost worker
    // through the state manager, so that call has to be serviced alongside
    // it.
    let timestamp = add_and_return_timestamp(BASE_WORKER_TIMEOUT_S);
    let (remove_result, (evicted_operation_id, evicted_worker_id, evicted_update)) = join!(
        test_context.scheduler.remove_timedout_workers(timestamp),
        test_context.state_manager.expect_update_operation(Ok(())),
    );
    remove_result?;
    assert_eq!(evicted_operation_id, operation_id);
    assert_eq!(evicted_worker_id, test_context.worker_id);
    assert!(
        matches!(evicted_update, UpdateOperationType::UpdateWithDisconnect),
        "expected the running action to be requeued as a lost worker, got {evicted_update:?}"
    );
    assert!(
        !test_context
            .scheduler
            .contains_worker_for_test(&test_context.worker_id)
            .await,
        "worker should be evicted after a silent window"
    );
    Ok(())
}

#[nativelink_test]
pub async fn stream_end_requeues_running_actions_as_disconnect_test()
-> Result<(), Box<dyn core::error::Error>> {
    let test_context = setup_api_server(BASE_WORKER_TIMEOUT_S, Box::new(static_now_fn)).await?;

    let action_digest = DigestInfo::new([8u8; 32], 123);
    let action_info = Arc::new(ActionInfo {
        command_digest: DigestInfo::new([0u8; 32], 0),
        input_root_digest: DigestInfo::new([0u8; 32], 0),
        timeout: Duration::MAX,
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: make_system_time(0),
        insert_timestamp: make_system_time(0),
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: "instance_name".to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: action_digest,
        }),
    });
    let operation_id = OperationId::default();
    let platform_properties = test_context
        .scheduler
        .get_platform_property_manager()
        .make_platform_properties(action_info.platform_properties.clone())?;
    test_context
        .scheduler
        .worker_notify_run_action(
            test_context.worker_id.clone(),
            operation_id.clone(),
            ActionInfoWithProps {
                inner: action_info,
                platform_properties,
                origin_metadata: OriginMetadata::default(),
                scheduler_start_execute_event_id: None,
            },
            BASE_NOW_S,
        )
        .await
        .unwrap();

    // The worker dies: its update stream ends with no GoingAway. The action
    // it held must come back as a disconnect, which is what the retry cap
    // reports as a dying worker rather than a failing job.
    drop(test_context.worker_stream);
    let (evicted_operation_id, evicted_worker_id, evicted_update) = test_context
        .state_manager
        .expect_update_operation(Ok(()))
        .await;
    assert_eq!(evicted_operation_id, operation_id);
    assert_eq!(evicted_worker_id, test_context.worker_id);
    assert_eq!(evicted_update, UpdateOperationType::UpdateWithDisconnect);
    assert!(
        !test_context
            .scheduler
            .contains_worker_for_test(&test_context.worker_id)
            .await,
        "worker should be gone once its stream ended"
    );
    Ok(())
}

#[nativelink_test]
pub async fn going_away_with_drain_stops_dispatch_and_keeps_the_worker_until_the_stream_ends()
-> Result<(), Box<dyn core::error::Error>> {
    let test_context = setup_api_server(BASE_WORKER_TIMEOUT_S, Box::new(static_now_fn)).await?;
    test_context
        .worker_stream
        .send(Update::GoingAwayRequest(GoingAwayRequest { drain: true }))
        .await
        .map_err(|e| make_err!(tonic::Code::Internal, "Error sending going away {e}"))?;
    tokio::time::sleep(Duration::from_millis(10)).await;

    assert!(
        test_context
            .scheduler
            .contains_worker_for_test(&test_context.worker_id)
            .await,
        "a draining worker stays in the pool while its stream is open"
    );
    let outcome = test_context
        .scheduler
        .find_worker_for_action(
            &PlatformProperties::new(HashMap::new()),
            false,
            make_system_time(BASE_NOW_S),
        )
        .await;
    assert!(
        !matches!(outcome, MatchOutcome::Matched(_)),
        "a draining worker must not be offered work: {outcome:?}"
    );

    drop(test_context.worker_stream);
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(
        !test_context
            .scheduler
            .contains_worker_for_test(&test_context.worker_id)
            .await,
        "the drained worker leaves the pool when its stream closes"
    );
    Ok(())
}

#[nativelink_test]
pub async fn server_does_not_timeout_if_keep_alive_test() -> Result<(), Box<dyn core::error::Error>>
{
    let now_timestamp = Arc::new(Mutex::new(BASE_NOW_S));
    let now_timestamp_clone = now_timestamp.clone();
    let add_and_return_timestamp = move |add_amount: u64| -> u64 {
        let mut locked_now_timestamp = now_timestamp.lock().unwrap();
        *locked_now_timestamp += add_amount;
        *locked_now_timestamp
    };

    let test_context = setup_api_server(
        BASE_WORKER_TIMEOUT_S,
        Box::new(move || Ok(Duration::from_secs(*now_timestamp_clone.lock().unwrap()))),
    )
    .await?;
    {
        // Now change time to 1 second before timeout and ensure the worker is still in the pool.
        let timestamp = add_and_return_timestamp(BASE_WORKER_TIMEOUT_S - 1);
        test_context
            .scheduler
            .remove_timedout_workers(timestamp)
            .await?;
        let worker_exists = test_context
            .scheduler
            .contains_worker_for_test(&test_context.worker_id)
            .await;
        assert!(worker_exists, "Expected worker to exist in worker map");
    }
    {
        // Now send keep alive.
        test_context
            .worker_stream
            .send(Update::KeepAliveRequest(KeepAliveRequest::default()))
            .await
            .map_err(|e| make_err!(tonic::Code::Internal, "Error sending keep alive {e}"))?;
        // Wait for a moment to allow it to be processed.
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    {
        // Now add 1 second and our worker should still exist in our map.
        let timestamp = add_and_return_timestamp(1);
        test_context
            .scheduler
            .remove_timedout_workers(timestamp)
            .await?;
        let worker_exists = test_context
            .scheduler
            .contains_worker_for_test(&test_context.worker_id)
            .await;
        assert!(worker_exists, "Expected worker to exist in map");
    }

    Ok(())
}

#[nativelink_test]
pub async fn worker_receives_keep_alive_request_test() -> Result<(), Box<dyn core::error::Error>> {
    let mut test_context = setup_api_server(BASE_WORKER_TIMEOUT_S, Box::new(static_now_fn)).await?;

    // Send keep alive to client.
    test_context
        .scheduler
        .send_keep_alive_to_worker_for_test(&test_context.worker_id)
        .await
        .err_tip(|| "Could not send keep alive to worker")?;

    {
        // Read stream and ensure it was a keep alive message.
        let maybe_message = test_context.connection_worker_stream.next().await;
        assert!(
            maybe_message.is_some(),
            "Expected next message in stream to exist"
        );
        let update_message = maybe_message
            .unwrap()
            .err_tip(|| "Expected success result")?
            .update
            .err_tip(|| "Expected update field to be populated")?;
        assert_eq!(
            update_message,
            update_for_worker::Update::KeepAlive(()),
            "Expected KeepAlive message"
        );
    }

    Ok(())
}

#[nativelink_test]
pub async fn going_away_removes_worker_test() -> Result<(), Box<dyn core::error::Error>> {
    let test_context = setup_api_server(BASE_WORKER_TIMEOUT_S, Box::new(static_now_fn)).await?;

    let worker_exists = test_context
        .scheduler
        .contains_worker_for_test(&test_context.worker_id)
        .await;
    assert!(worker_exists, "Expected worker to exist in worker map");

    test_context
        .scheduler
        .remove_worker(&test_context.worker_id)
        .await
        .unwrap();

    let worker_exists = test_context
        .scheduler
        .contains_worker_for_test(&test_context.worker_id)
        .await;
    assert!(
        !worker_exists,
        "Expected worker to be removed from worker map"
    );

    Ok(())
}

fn make_system_time(time: u64) -> SystemTime {
    UNIX_EPOCH.checked_add(Duration::from_secs(time)).unwrap()
}

#[nativelink_test]
pub async fn execution_response_success_test() -> Result<(), Box<dyn core::error::Error>> {
    let mut test_context = setup_api_server(BASE_WORKER_TIMEOUT_S, Box::new(static_now_fn)).await?;

    let action_digest = DigestInfo::new([7u8; 32], 123);
    let instance_name = "instance_name".to_string();

    let unique_qualifier = ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
        execution_scope: None,
        instance_name: instance_name.clone(),
        digest_function: DigestHasherFunc::Sha256,
        digest: action_digest,
    });
    let action_info = Arc::new(ActionInfo {
        command_digest: DigestInfo::new([0u8; 32], 0),
        input_root_digest: DigestInfo::new([0u8; 32], 0),
        timeout: Duration::MAX,
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: make_system_time(0),
        insert_timestamp: make_system_time(0),
        unique_qualifier,
    });
    let expected_operation_id = OperationId::default();

    let platform_properties = test_context
        .scheduler
        .get_platform_property_manager()
        .make_platform_properties(action_info.platform_properties.clone())
        .err_tip(|| "Failed to make platform properties in SimpleScheduler::do_try_match")?;

    test_context
        .scheduler
        .worker_notify_run_action(
            test_context.worker_id.clone(),
            expected_operation_id.clone(),
            ActionInfoWithProps {
                inner: action_info,
                platform_properties,
                origin_metadata: OriginMetadata::default(),
                scheduler_start_execute_event_id: None,
            },
            BASE_NOW_S,
        )
        .await
        .unwrap();

    let mut server_logs = HashMap::new();
    server_logs.insert(
        "log_name".to_string(),
        LogFile {
            digest: Some(DigestInfo::new([9u8; 32], 124).into()),
            human_readable: false, // We only support non-human readable.
        },
    );
    let execute_response = ExecuteResponse {
        result: Some(ProtoActionResult {
            output_files: vec![OutputFile {
                path: "some path1".to_string(),
                digest: Some(DigestInfo::new([8u8; 32], 124).into()),
                is_executable: true,
                contents: Bytes::default(), // We don't implement this.
                node_properties: None,
            }],
            output_file_symlinks: vec![OutputSymlink {
                path: "some path3".to_string(),
                target: "some target3".to_string(),
                node_properties: None,
            }],
            output_symlinks: vec![OutputSymlink {
                path: "some path3".to_string(),
                target: "some target3".to_string(),
                node_properties: None,
            }],
            output_directories: vec![OutputDirectory {
                path: "some path4".to_string(),
                tree_digest: Some(DigestInfo::new([12u8; 32], 124).into()),
                is_topologically_sorted: false,
            }],
            output_directory_symlinks: Vec::default(), // Bazel deprecated this.
            exit_code: 5,
            stdout_raw: Bytes::default(), // We don't implement this.
            stdout_digest: Some(DigestInfo::new([10u8; 32], 124).into()),
            stderr_raw: Bytes::default(), // We don't implement this.
            stderr_digest: Some(DigestInfo::new([11u8; 32], 124).into()),
            execution_metadata: Some(ExecutedActionMetadata {
                worker: test_context.worker_id.to_string(),
                queued_timestamp: Some(make_system_time(1).into()),
                worker_start_timestamp: Some(make_system_time(2).into()),
                worker_completed_timestamp: Some(make_system_time(3).into()),
                input_fetch_start_timestamp: Some(make_system_time(4).into()),
                input_fetch_completed_timestamp: Some(make_system_time(5).into()),
                execution_start_timestamp: Some(make_system_time(6).into()),
                execution_completed_timestamp: Some(make_system_time(7).into()),
                output_upload_start_timestamp: Some(make_system_time(8).into()),
                output_upload_completed_timestamp: Some(make_system_time(9).into()),
                virtual_execution_duration: Some(prost_types::Duration {
                    seconds: 1,
                    nanos: 0,
                }),
                auxiliary_metadata: vec![],
            }),
        }),
        cached_result: false,
        status: Some(ProtoStatus {
            code: 9,
            message: "foo".to_string(),
            details: Vec::default(),
        }),
        server_logs,
        message: "TODO(palfrey) We should put a reference something like bb_browser".to_string(),
    };
    let result = ExecuteResult {
        instance_name,
        operation_id: expected_operation_id.to_string(),
        result: Some(execute_result::Result::ExecuteResponse(
            execute_response.clone(),
        )),
        resource_usage: None,
    };

    let update_for_worker = test_context
        .connection_worker_stream
        .next()
        .await
        .expect("Worker stream ended early")?
        .update
        .expect("Expected update field to be populated");
    let update_for_worker::Update::StartAction(start_execute) = update_for_worker else {
        panic!("Expected StartAction message");
    };
    assert_eq!(result.operation_id, start_execute.operation_id);

    {
        // Ensure our state manager got the same result as the server.
        let (execution_response_result, (operation_id, worker_id, client_given_update)) = join!(
            test_context
                .worker_stream
                .send(Update::ExecuteResult(result.clone())),
            test_context.state_manager.expect_update_operation(Ok(())),
        );
        execution_response_result?;

        assert_eq!(operation_id, expected_operation_id);
        assert_eq!(worker_id, test_context.worker_id);
        assert_eq!(
            client_given_update,
            UpdateOperationType::UpdateWithActionStage(execute_response.clone().try_into()?)
        );
        let UpdateOperationType::UpdateWithActionStage(client_given_state) = client_given_update
        else {
            unreachable!()
        };
        assert_eq!(execute_response, client_given_state.into());
    }
    Ok(())
}

/// A finished result still lands when the scheduler's wall clock has
/// stepped backwards (NTP, VM time sync) since the worker's last
/// message. The liveness refresh used to fail on the older timestamp and
/// abort `inner_execution_response`, discarding a result the worker had
/// already uploaded and stranding the operation in `Executing`.
#[nativelink_test]
pub async fn execution_response_lands_when_clock_steps_backwards_test()
-> Result<(), Box<dyn core::error::Error>> {
    let now_timestamp = Arc::new(Mutex::new(BASE_NOW_S));
    let now_timestamp_clone = now_timestamp.clone();
    let test_context = setup_api_server(
        BASE_WORKER_TIMEOUT_S,
        Box::new(move || Ok(Duration::from_secs(*now_timestamp_clone.lock().unwrap()))),
    )
    .await?;
    let operation_id = dispatch(&test_context, 7).await?;

    // The worker's liveness was last refreshed at `BASE_NOW_S`; the
    // scheduler's clock steps back before the result arrives.
    *now_timestamp.lock().unwrap() = BASE_NOW_S - 2;

    let result = ExecuteResult {
        instance_name: "instance_name".to_string(),
        operation_id: operation_id.to_string(),
        result: Some(execute_result::Result::InternalError(ProtoStatus {
            code: 13,
            message: "worker hit an internal error".to_string(),
            details: Vec::default(),
        })),
        resource_usage: None,
    };
    let (sent, (updated_operation_id, updated_worker_id, update)) = join!(
        test_context
            .worker_stream
            .send(Update::ExecuteResult(result)),
        test_context.state_manager.expect_update_operation(Ok(())),
    );
    sent.map_err(|e| make_err!(tonic::Code::Internal, "Error sending result {e}"))?;
    assert_eq!(updated_operation_id, operation_id);
    assert_eq!(updated_worker_id, test_context.worker_id);
    assert!(
        matches!(update, UpdateOperationType::UpdateWithError(_)),
        "{update:?}"
    );
    assert!(
        test_context
            .scheduler
            .contains_worker_for_test(&test_context.worker_id)
            .await,
        "a backwards clock step must not cost the worker its registration"
    );
    Ok(())
}

#[nativelink_test]
pub async fn workers_only_allow_max_tasks() -> Result<(), Box<dyn core::error::Error>> {
    let test_context =
        setup_api_server_with_task_limit(BASE_WORKER_TIMEOUT_S, Box::new(static_now_fn), 1).await?;

    let selected_worker = test_context
        .scheduler
        .find_worker_for_action(
            &PlatformProperties::new(HashMap::new()),
            true,
            make_system_time(BASE_NOW_S),
        )
        .await;
    assert_eq!(
        selected_worker,
        MatchOutcome::Matched(test_context.worker_id.clone()),
        "Expected worker to permit tasks to begin with"
    );

    let action_digest = DigestInfo::new([7u8; 32], 123);
    let instance_name = "instance_name".to_string();

    let unique_qualifier = ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
        execution_scope: None,
        instance_name: instance_name.clone(),
        digest_function: DigestHasherFunc::Sha256,
        digest: action_digest,
    });

    let action_info = Arc::new(ActionInfo {
        command_digest: DigestInfo::new([0u8; 32], 0),
        input_root_digest: DigestInfo::new([0u8; 32], 0),
        timeout: Duration::MAX,
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: make_system_time(0),
        insert_timestamp: make_system_time(0),
        unique_qualifier,
    });

    let platform_properties = test_context
        .scheduler
        .get_platform_property_manager()
        .make_platform_properties(action_info.platform_properties.clone())
        .err_tip(|| "Failed to make platform properties in SimpleScheduler::do_try_match")?;

    let expected_operation_id = OperationId::default();

    test_context
        .scheduler
        .worker_notify_run_action(
            test_context.worker_id.clone(),
            expected_operation_id,
            ActionInfoWithProps {
                inner: action_info,
                platform_properties,
                origin_metadata: OriginMetadata::default(),
                scheduler_start_execute_event_id: None,
            },
            BASE_NOW_S,
        )
        .await
        .unwrap();

    let selected_worker = test_context
        .scheduler
        .find_worker_for_action(
            &PlatformProperties::new(HashMap::new()),
            true,
            make_system_time(BASE_NOW_S),
        )
        .await;
    assert_eq!(
        selected_worker,
        MatchOutcome::WaitingForCapacity,
        "Expected not to be able to give worker a second task"
    );

    assert!(logs_contain("All workers are fully allocated"));

    Ok(())
}

/// An acknowledgement is recorded before the liveness refresh it carries,
/// because that refresh runs the unacknowledged sweep: one arriving right
/// at the timeout must confirm the dispatch, not be requeued by it.
#[nativelink_test]
pub async fn acknowledgement_is_recorded_before_the_sweep_it_carries_test()
-> Result<(), Box<dyn core::error::Error>> {
    const ACK_TIMEOUT_S: u64 = 5;
    let now_timestamp = Arc::new(Mutex::new(BASE_NOW_S));
    let now_timestamp_clone = now_timestamp.clone();
    let test_context = setup_api_server_with(
        BASE_WORKER_TIMEOUT_S,
        Box::new(move || Ok(Duration::from_secs(*now_timestamp_clone.lock().unwrap()))),
        0,
        ACK_TIMEOUT_S,
    )
    .await?;

    let action_digest = DigestInfo::new([8u8; 32], 123);
    let action_info = Arc::new(ActionInfo {
        command_digest: DigestInfo::new([0u8; 32], 0),
        input_root_digest: DigestInfo::new([0u8; 32], 0),
        timeout: Duration::MAX,
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: make_system_time(0),
        insert_timestamp: make_system_time(0),
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: "instance_name".to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: action_digest,
        }),
    });
    let operation_id = OperationId::default();
    let platform_properties = test_context
        .scheduler
        .get_platform_property_manager()
        .make_platform_properties(action_info.platform_properties.clone())?;
    test_context
        .scheduler
        .worker_notify_run_action(
            test_context.worker_id.clone(),
            operation_id.clone(),
            ActionInfoWithProps {
                inner: action_info,
                platform_properties,
                origin_metadata: OriginMetadata::default(),
                scheduler_start_execute_event_id: None,
            },
            BASE_NOW_S,
        )
        .await
        .unwrap();

    // The acknowledgement arrives past the timeout, with no message from
    // the worker in between.
    *now_timestamp.lock().unwrap() += ACK_TIMEOUT_S + 1;
    test_context
        .worker_stream
        .send(Update::ExecuteAccepted(ExecuteAccepted {
            operation_id: operation_id.to_string(),
        }))
        .await
        .map_err(|e| make_err!(tonic::Code::Internal, "Error sending acceptance {e}"))?;

    // A requeue would go through the state manager; nothing may.
    tokio::select! {
        () = tokio::time::sleep(Duration::from_millis(50)) => {}
        _ = test_context.state_manager.expect_update_operation(Ok(())) => {
            panic!("the acknowledgement was requeued by the sweep it arrived with");
        }
    }
    assert!(
        test_context
            .scheduler
            .running_action_info(&test_context.worker_id, &operation_id)
            .await
            .is_some(),
        "the acknowledged dispatch stays on the worker"
    );
    Ok(())
}

fn uncacheable_action(seed: u8) -> Arc<ActionInfo> {
    Arc::new(ActionInfo {
        command_digest: DigestInfo::new([0u8; 32], 0),
        input_root_digest: DigestInfo::new([0u8; 32], 0),
        timeout: Duration::MAX,
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: make_system_time(0),
        insert_timestamp: make_system_time(0),
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: "instance_name".to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: DigestInfo::new([seed; 32], 123),
        }),
    })
}

/// Sends the action to the test worker as the matcher would, dated now.
async fn dispatch(test_context: &TestContext, seed: u8) -> Result<OperationId, Error> {
    let action_info = uncacheable_action(seed);
    let platform_properties = test_context
        .scheduler
        .get_platform_property_manager()
        .make_platform_properties(action_info.platform_properties.clone())?;
    let operation_id = OperationId::default();
    test_context
        .scheduler
        .worker_notify_run_action(
            test_context.worker_id.clone(),
            operation_id.clone(),
            ActionInfoWithProps {
                inner: action_info,
                platform_properties,
                origin_metadata: OriginMetadata::default(),
                scheduler_start_execute_event_id: None,
            },
            BASE_NOW_S,
        )
        .await?;
    Ok(operation_id)
}

/// The test worker's summary once the scheduler has taken in a keepalive
/// reporting `free_kb`: the worker's messages are handled on another task,
/// so the report is the sign that everything sent before it has been too.
async fn summary_after_report(test_context: &TestContext, free_kb: u64) -> WorkerSummary {
    let worker_id = test_context.worker_id.to_string();
    for _ in 0..500 {
        if let Some(summary) = test_context
            .scheduler
            .worker_snapshot()
            .await
            .into_iter()
            .find(|summary| summary.id == worker_id && summary.free_memory_kb == Some(free_kb))
        {
            return summary;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    panic!("the keepalive reporting {free_kb} KiB was never taken in");
}

/// A decline pauses the worker. The liveness refresh the decline carries
/// must not lift that pause, or the next matching pass hands the worker
/// the same action straight back; only a keepalive reporting enough room
/// resumes it. The worker holds another action, so the room can come.
#[nativelink_test]
pub async fn a_decline_keeps_the_worker_paused_until_a_keepalive_test()
-> Result<(), Box<dyn core::error::Error>> {
    let test_context = setup_api_server(BASE_WORKER_TIMEOUT_S, Box::new(static_now_fn)).await?;
    let _resident = dispatch(&test_context, 8).await?;
    let operation_id = dispatch(&test_context, 9).await?;

    // The decline sends the action back through the state manager.
    let decline = test_context
        .worker_stream
        .send(Update::ExecuteDeclined(ExecuteDeclined {
            operation_id: operation_id.to_string(),
            reason: execute_declined::Reason::Load as i32,
            detail: String::new(),
            needed_kb: 4_000,
            free_kb: 1_000,
        }));
    let (sent, (requeued_id, _, update)) = join!(
        decline,
        test_context.state_manager.expect_update_operation(Ok(()))
    );
    sent.map_err(|e| make_err!(tonic::Code::Internal, "Error sending decline {e}"))?;
    assert_eq!(requeued_id, operation_id);
    assert!(
        matches!(update, UpdateOperationType::UpdateWithDecline(_)),
        "{update:?}"
    );

    // A keepalive without the room asked for leaves the pause in place.
    test_context
        .worker_stream
        .send(Update::KeepAliveRequest(KeepAliveRequest {
            load: Some(WorkerLoad {
                free_memory_kb: 1_000,
            }),
        }))
        .await
        .map_err(|e| make_err!(tonic::Code::Internal, "Error sending keepalive {e}"))?;
    let summary = summary_after_report(&test_context, 1_000).await;
    assert!(
        summary.is_paused,
        "the decline's own refresh lifted the pause"
    );

    // One with the room lifts it.
    test_context
        .worker_stream
        .send(Update::KeepAliveRequest(KeepAliveRequest {
            load: Some(WorkerLoad {
                free_memory_kb: 5_000,
            }),
        }))
        .await
        .map_err(|e| make_err!(tonic::Code::Internal, "Error sending keepalive {e}"))?;
    let summary = summary_after_report(&test_context, 5_000).await;
    assert!(
        !summary.is_paused,
        "a keepalive with the room resumes the worker"
    );
    Ok(())
}

/// A kill that does not fit the worker's channel is a worker that is
/// behind, not one that is gone: it keeps its place and its actions, and
/// the next sweep sends the kill once the worker has read its backlog.
#[nativelink_test]
pub async fn a_full_channel_defers_the_kill_instead_of_evicting_test()
-> Result<(), Box<dyn core::error::Error>> {
    let mut test_context =
        setup_api_server_with_task_limit(BASE_WORKER_TIMEOUT_S, Box::new(static_now_fn), 1).await?;
    // Dispatches the worker never reads fill its channel to the brim.
    let capacity = channel_capacity(1);
    let mut operation_ids = Vec::new();
    for seed in 0..capacity {
        operation_ids.push(dispatch(&test_context, u8::try_from(seed).unwrap()).await?);
    }
    let revoked = operation_ids[0].clone();
    test_context.state_manager.revoke(&revoked);

    // The kill has nowhere to go; the worker is neither evicted nor
    // stripped of the operation.
    test_context.scheduler.kill_revoked_operations().await?;
    assert!(
        test_context
            .scheduler
            .running_action_info(&test_context.worker_id, &revoked)
            .await
            .is_some(),
        "a full channel evicted the worker"
    );

    // The worker reads one message; the next sweep gets the kill through.
    let first = test_context
        .connection_worker_stream
        .next()
        .await
        .unwrap()?;
    assert!(matches!(
        first.update,
        Some(update_for_worker::Update::StartAction(_))
    ));
    test_context.scheduler.kill_revoked_operations().await?;
    let mut deferred_kill = None;
    for _ in 0..capacity {
        let msg = test_context
            .connection_worker_stream
            .next()
            .await
            .unwrap()?;
        if let Some(update_for_worker::Update::KillOperationRequest(kill)) = msg.update {
            deferred_kill = Some(kill.operation_id);
            break;
        }
    }
    assert_eq!(
        deferred_kill,
        Some(revoked.to_string()),
        "the deferred kill was never sent"
    );
    Ok(())
}

/// A message the scheduler is slow to process must not stop it reading the
/// worker's stream: the keepalives behind it still come off the wire (here,
/// a one-slot channel the test could not fill otherwise), and are taken in,
/// in order, once the slow one is done. Reading inline, the stream's
/// flow-control window filled behind a slow result and the worker's
/// keepalives never left it, so this scheduler evicted a worker that was
/// only waiting for it.
#[nativelink_test]
pub async fn a_slow_result_does_not_stop_the_stream_reads_test()
-> Result<(), Box<dyn core::error::Error>> {
    let mut test_context = setup_api_server(BASE_WORKER_TIMEOUT_S, Box::new(static_now_fn)).await?;
    let operation_id = dispatch(&test_context, 1).await?;
    let update_for_worker = test_context
        .connection_worker_stream
        .next()
        .await
        .expect("Worker stream ended early")?
        .update
        .expect("Expected update field to be populated");
    let update_for_worker::Update::StartAction(start_execute) = update_for_worker else {
        panic!("Expected StartAction message");
    };
    assert_eq!(operation_id.to_string(), start_execute.operation_id);

    // The result's store write is not answered yet: processing it blocks.
    test_context
        .worker_stream
        .send(Update::ExecuteResult(ExecuteResult {
            instance_name: "instance_name".to_string(),
            operation_id: operation_id.to_string(),
            result: Some(execute_result::Result::InternalError(
                make_err!(tonic::Code::Internal, "the action failed").into(),
            )),
            resource_usage: None,
        }))
        .await?;

    // The keepalives behind it are still read off the stream.
    for free_memory_kb in 1..=8u64 {
        tokio::time::timeout(
            Duration::from_secs(5),
            test_context
                .worker_stream
                .send(Update::KeepAliveRequest(KeepAliveRequest {
                    load: Some(WorkerLoad { free_memory_kb }),
                })),
        )
        .await
        .map_err(|_| {
            make_err!(
                tonic::Code::DeadlineExceeded,
                "the stream read stalled behind the result"
            )
        })??;
    }

    // Once the result is through, the keepalives are taken in after it.
    let (operation_id_seen, worker_id, _) = test_context
        .state_manager
        .expect_update_operation(Ok(()))
        .await;
    assert_eq!(operation_id_seen, operation_id);
    assert_eq!(worker_id, test_context.worker_id);
    summary_after_report(&test_context, 8).await;
    Ok(())
}
