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

use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use mock_instant::thread_local::MockClock;
use nativelink_config::schedulers::{MemoryEscalationSpec, PropertyType, SimpleSpec};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    ActionResourceUsage, Reservation, ResourceOutcome, StartExecute, UpdateForWorker,
    update_for_worker,
};
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::{
    ActionInfo, ActionResult, ActionStage, OperationId, WorkerId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ClientStateManager, UpdateOperationType,
};
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};
use pretty_assertions::assert_eq;
use tokio::sync::{Notify, mpsc};
use utils::scheduler_utils::make_base_action_info;

mod utils {
    pub(crate) mod scheduler_utils;
}

const NOW_TIME: u64 = 10000;
const WORKER: &str = "the_worker";

fn make_scheduler(max_job_retries: usize) -> (Arc<SimpleScheduler>, Arc<dyn WorkerScheduler>) {
    make_scheduler_with_ladder(max_job_retries, vec![])
}

fn make_scheduler_with_ladder(
    max_job_retries: usize,
    ladder_kb: Vec<u64>,
) -> (Arc<SimpleScheduler>, Arc<dyn WorkerScheduler>) {
    make_scheduler_with_limits(max_job_retries, ladder_kb, 0)
}

fn make_scheduler_with_limits(
    max_job_retries: usize,
    ladder_kb: Vec<u64>,
    max_steps: u64,
) -> (Arc<SimpleScheduler>, Arc<dyn WorkerScheduler>) {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let task_change_notify = Arc::new(Notify::new());
    let spec = SimpleSpec {
        supported_platform_properties: Some(HashMap::from([(
            "memory_kb".to_string(),
            PropertyType::Minimum,
        )])),
        memory_escalation: Some(MemoryEscalationSpec {
            property: "memory_kb".to_string(),
            percent: 200,
            max_kb: 0,
            ladder_kb,
            cpu_property: "cpu_count".to_string(),
            max_steps,
        }),
        max_job_retries,
        max_worker_loss_retries: 2,
        ..SimpleSpec::default()
    };
    SimpleScheduler::new_with_callback(
        &spec,
        memory_awaited_action_db_factory(0, &task_change_notify, MockInstantWrapped::default),
        || async move {},
        task_change_notify,
        MockInstantWrapped::default,
        None,
    )
}

async fn add_worker(
    scheduler: &SimpleScheduler,
    memory_kb: u64,
) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    let (tx, mut rx) = mpsc::channel(64);
    let worker = Worker::new(
        WorkerId(WORKER.to_string()),
        PlatformProperties::new(HashMap::from([(
            "memory_kb".to_string(),
            PlatformPropertyValue::Minimum(memory_kb),
        )])),
        tx,
        NOW_TIME,
        /* max_inflight_tasks */ 4,
    );
    scheduler
        .add_worker(worker)
        .await
        .err_tip(|| "Failed to add worker")?;
    tokio::task::yield_now().await;
    let connected = rx.recv().await.unwrap();
    assert!(matches!(
        connected.update,
        Some(update_for_worker::Update::ConnectionResult(_))
    ));
    Ok(rx)
}

/// The next dispatch the worker sees, and the memory it was reserved with.
async fn next_dispatch(rx: &mut mpsc::Receiver<UpdateForWorker>) -> (StartExecute, u64) {
    let msg = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("a dispatch within two seconds")
        .expect("the channel is open");
    let Some(update_for_worker::Update::StartAction(start_execute)) = msg.update else {
        panic!("expected a StartAction, got {msg:?}");
    };
    let memory_kb = start_execute
        .platform
        .as_ref()
        .and_then(|platform| {
            platform
                .properties
                .iter()
                .find(|p| p.name == "memory_kb")
                .and_then(|p| p.value.parse().ok())
        })
        .expect("the dispatch carries memory_kb");
    (start_execute, memory_kb)
}

/// What a worker sends when its enforcement killed the action at its
/// reservation: the usage report, then the result carrying the error.
async fn report_memory_kill(
    worker_scheduler: &dyn WorkerScheduler,
    operation_id: &str,
    reserved_kb: u64,
) -> Result<(), Error> {
    let worker_id = WorkerId(WORKER.to_string());
    let operation_id = OperationId::from(operation_id);
    worker_scheduler
        .record_action_resource_usage(
            &worker_id,
            &operation_id,
            ActionResourceUsage {
                peak_memory_kb: reserved_kb + reserved_kb / 5,
                sampled: true,
                outcome: ResourceOutcome::KilledMemory.into(),
                enforced: true,
                reserved: Some(Reservation {
                    memory_kb: reserved_kb,
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await?;
    worker_scheduler
        .update_action(
            &worker_id,
            &operation_id,
            UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(ActionResult {
                exit_code: 9,
                error: Some(make_err!(
                    Code::FailedPrecondition,
                    "Action exceeded its memory reservation: reserved {reserved_kb} KiB"
                )),
                ..ActionResult::default()
            })),
        )
        .await?;
    tokio::task::yield_now().await;
    Ok(())
}

async fn wait_for_completion(listener: &mut Box<dyn ActionStateResult>) -> ActionResult {
    loop {
        let (state, _) = listener.changed().await.expect("state stream open");
        if let ActionStage::Completed(result) = &state.stage {
            return result.clone();
        }
    }
}

/// A memory kill doubles the reservation and runs the action again; the
/// doubling stops at the largest memory any worker advertises; a kill at
/// that ceiling fails the action with the worker's own error.
#[nativelink_test]
async fn a_memory_kill_requeues_the_action_with_double_the_reservation() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler(5);
    let mut rx = add_worker(&scheduler, 5_000).await?;

    let base = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([1; 32], 512),
    );
    let action_info = Arc::new(ActionInfo {
        platform_properties: HashMap::from([("memory_kb".to_string(), "2000".to_string())]),
        ..(*base).clone()
    });
    let mut listener = scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    tokio::task::yield_now().await;

    let (first, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 2_000);
    report_memory_kill(worker_scheduler.as_ref(), &first.operation_id, 2_000).await?;

    let (second, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 4_000, "the reservation doubles after a kill");
    assert_eq!(
        second.operation_id, first.operation_id,
        "same operation, asking for more"
    );
    report_memory_kill(worker_scheduler.as_ref(), &second.operation_id, 4_000).await?;

    let (third, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 5_000, "capped at the largest worker");
    report_memory_kill(worker_scheduler.as_ref(), &third.operation_id, 5_000).await?;

    // Nothing bigger exists: the client gets the worker's error.
    let result = wait_for_completion(&mut listener).await;
    let err = result.error.expect("the action fails");
    assert_eq!(err.code, Code::FailedPrecondition, "{err}");
    assert!(
        err.to_string().contains("memory reservation"),
        "the worker's message reaches the client: {err}"
    );
    Ok(())
}

/// Escalations have their own budget: four kills on a `max_job_retries` of
/// one, and the action is still running, each time with more memory.
#[nativelink_test]
async fn escalation_does_not_spend_the_retry_budget() -> Result<(), Error> {
    // One retry for the action's own failures; escalations are not those.
    let (scheduler, worker_scheduler) = make_scheduler(1);
    let mut rx = add_worker(&scheduler, 1_000_000).await?;

    let base = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([2; 32], 512),
    );
    let action_info = Arc::new(ActionInfo {
        platform_properties: HashMap::from([("memory_kb".to_string(), "1000".to_string())]),
        ..(*base).clone()
    });
    let _listener = scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    tokio::task::yield_now().await;

    let mut reserved = 1_000;
    for _ in 0..4 {
        let (dispatch, memory) = next_dispatch(&mut rx).await;
        assert_eq!(memory, reserved);
        report_memory_kill(worker_scheduler.as_ref(), &dispatch.operation_id, reserved).await?;
        reserved *= 2;
    }
    // Four kills, four requeues, on a budget of one: still running.
    let (_, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 16_000);
    Ok(())
}

/// `max_steps` bounds the escalations: with two, the third kill ends the
/// action, and the client hears that the budget is spent.
#[nativelink_test]
async fn escalation_stops_at_max_steps() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with_limits(5, vec![], 2);
    let mut rx = add_worker(&scheduler, 1_000_000).await?;

    let base = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([4; 32], 512),
    );
    let action_info = Arc::new(ActionInfo {
        platform_properties: HashMap::from([("memory_kb".to_string(), "1000".to_string())]),
        ..(*base).clone()
    });
    let mut listener = scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    tokio::task::yield_now().await;

    let mut reserved = 1_000;
    for _ in 0..3 {
        let (dispatch, memory) = next_dispatch(&mut rx).await;
        assert_eq!(memory, reserved);
        report_memory_kill(worker_scheduler.as_ref(), &dispatch.operation_id, reserved).await?;
        reserved *= 2;
    }

    let result = wait_for_completion(&mut listener).await;
    let err = result.error.expect("the third kill is past the budget");
    assert_eq!(err.code, Code::FailedPrecondition, "{err}");
    assert!(err.to_string().contains("more than max_steps"), "{err}");
    Ok(())
}

/// A worker lost mid-action is not the action's failure: the requeue has
/// its own budget (`max_worker_loss_retries`, two here) and leaves
/// `max_job_retries` alone; past that budget the client hears the worker
/// keeps dying under this action.
#[nativelink_test]
async fn a_lost_worker_does_not_spend_the_retry_budget() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler(1);
    let mut rx = add_worker(&scheduler, 1_000_000).await?;

    let base = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([4; 32], 512),
    );
    let action_info = Arc::new(ActionInfo {
        platform_properties: HashMap::from([("memory_kb".to_string(), "1000".to_string())]),
        ..(*base).clone()
    });
    let mut listener = scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    tokio::task::yield_now().await;

    for loss in 1..=2 {
        let (_, memory) = next_dispatch(&mut rx).await;
        assert_eq!(memory, 1_000, "loss {loss}: the reservation is unchanged");
        worker_scheduler
            .remove_worker(&WorkerId(WORKER.to_string()))
            .await?;
        tokio::task::yield_now().await;
        rx = add_worker(&scheduler, 1_000_000).await?;
    }
    // Two losses on a job budget of one: still queued and dispatched.
    let (_, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 1_000);
    // The third loss is past the loss budget.
    worker_scheduler
        .remove_worker(&WorkerId(WORKER.to_string()))
        .await?;
    let result = wait_for_completion(&mut listener).await;
    let err = result.error.expect("the action fails past the loss budget");
    assert_eq!(err.code, Code::FailedPrecondition, "{err}");
    assert!(err.to_string().contains("lost 3 times"), "{err}");
    Ok(())
}

/// With a size ladder, a kill steps to the next class above the reservation
/// rather than scaling; past the top class the largest worker is reserved
/// whole, and only a kill there fails the action.
#[nativelink_test]
async fn escalation_steps_the_ladder_then_takes_the_whole_worker() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with_ladder(5, vec![1_000, 4_000, 9_000]);
    let mut rx = add_worker(&scheduler, 1_000_000).await?;

    let base = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([3; 32], 512),
    );
    let action_info = Arc::new(ActionInfo {
        platform_properties: HashMap::from([("memory_kb".to_string(), "1500".to_string())]),
        ..(*base).clone()
    });
    let mut listener = scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    tokio::task::yield_now().await;

    let (first, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 1_500);
    report_memory_kill(worker_scheduler.as_ref(), &first.operation_id, 1_500).await?;
    let (second, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 4_000, "the next class above 1500");
    report_memory_kill(worker_scheduler.as_ref(), &second.operation_id, 4_000).await?;
    let (third, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 9_000, "the top class");
    report_memory_kill(worker_scheduler.as_ref(), &third.operation_id, 9_000).await?;
    let (fourth, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 1_000_000, "the whole of the largest worker");
    report_memory_kill(worker_scheduler.as_ref(), &fourth.operation_id, 1_000_000).await?;

    let result = wait_for_completion(&mut listener).await;
    let err = result.error.expect("nothing above the whole worker");
    assert_eq!(err.code, Code::FailedPrecondition, "{err}");
    assert!(err.to_string().contains("no worker can run it"), "{err}");
    Ok(())
}
