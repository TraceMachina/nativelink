// Copyright 2026 Trace Machina, Inc. All rights reserved.
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
use nativelink_proto::build::bazel::remote::execution::v2::Digest;
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
            escalate_after_worker_losses: 2,
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
    add_named_worker(scheduler, WORKER, memory_kb, 4).await
}

async fn add_named_worker(
    scheduler: &SimpleScheduler,
    name: &str,
    memory_kb: u64,
    max_inflight_tasks: u64,
) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    let (tx, mut rx) = mpsc::channel(64);
    let worker = Worker::new(
        WorkerId(name.to_string()),
        PlatformProperties::new(HashMap::from([(
            "memory_kb".to_string(),
            PlatformPropertyValue::Minimum(memory_kb),
        )])),
        tx,
        NOW_TIME,
        max_inflight_tasks,
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
/// reservation: the usage report, then the result carrying the error. The
/// sampled peak is the reservation plus the default 20% headroom.
async fn report_memory_kill(
    worker_scheduler: &dyn WorkerScheduler,
    operation_id: &str,
    reserved_kb: u64,
) -> Result<(), Error> {
    report_memory_kill_at(
        worker_scheduler,
        operation_id,
        reserved_kb,
        reserved_kb + reserved_kb / 5,
    )
    .await
}

/// The memory kill as above, with the peak the sampler reported.
async fn report_memory_kill_at(
    worker_scheduler: &dyn WorkerScheduler,
    operation_id: &str,
    reserved_kb: u64,
    peak_kb: u64,
) -> Result<(), Error> {
    let worker_id = WorkerId(WORKER.to_string());
    let operation_id = OperationId::from(operation_id);
    worker_scheduler
        .record_action_resource_usage(
            &worker_id,
            &operation_id,
            ActionResourceUsage {
                peak_memory_kb: peak_kb,
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

/// A memory kill doubles what the action was seen to use (its sampled
/// peak, 20% over its reservation here) and runs it again; the doubling
/// stops at the largest memory any worker advertises; a kill at that
/// ceiling fails the action with the worker's own error.
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
    assert_eq!(memory, 4_800, "double the 2400 KiB peak after a kill");
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
        // Double the sampled peak, which the kill reports 20% over.
        reserved = (reserved + reserved / 5) * 2;
    }
    // Four kills, four requeues, on a budget of one: still running.
    let (_, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, reserved);
    assert_eq!(reserved, 33_176, "1000, 2400, 5760, 13824, then this");
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
        // Double the sampled peak, which the kill reports 20% over.
        reserved = (reserved + reserved / 5) * 2;
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

// RFC-0018: a worker lost under an action is memory evidence when the
// worker went away on its own. These drive the loss path the way a pod
// OOM-killed as a whole does: the stream ends with no result.

fn make_scheduler_with_policy(
    policy: Option<MemoryEscalationSpec>,
    max_worker_loss_retries: usize,
) -> (Arc<SimpleScheduler>, Arc<dyn WorkerScheduler>) {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let task_change_notify = Arc::new(Notify::new());
    let spec = SimpleSpec {
        supported_platform_properties: Some(HashMap::from([(
            "memory_kb".to_string(),
            PropertyType::Minimum,
        )])),
        memory_escalation: policy,
        max_job_retries: 1,
        max_worker_loss_retries,
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

fn policy(ladder_kb: Vec<u64>, max_steps: u64) -> MemoryEscalationSpec {
    MemoryEscalationSpec {
        property: "memory_kb".to_string(),
        percent: 200,
        max_kb: 0,
        ladder_kb,
        cpu_property: "cpu_count".to_string(),
        max_steps,
        escalate_after_worker_losses: 2,
    }
}

async fn add_action(
    scheduler: &SimpleScheduler,
    seed: u8,
    memory_kb: u64,
) -> Result<Box<dyn ActionStateResult>, Error> {
    let base = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([seed; 32], 512),
    );
    let action_info = Arc::new(ActionInfo {
        platform_properties: HashMap::from([("memory_kb".to_string(), memory_kb.to_string())]),
        ..(*base).clone()
    });
    let listener = scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    tokio::task::yield_now().await;
    Ok(listener)
}

/// The worker's keepalive reported this peak for the operation.
async fn report_peak(
    worker_scheduler: &dyn WorkerScheduler,
    operation_id: &str,
    peak_kb: u64,
) -> Result<(), Error> {
    worker_scheduler
        .worker_operation_memory_reported(
            &WorkerId(WORKER.to_string()),
            vec![(OperationId::from(operation_id), peak_kb)],
        )
        .await
}

/// The worker's stream ends without a result: what an OOM-killed pod looks
/// like to the scheduler. The pool brings up a like worker.
async fn lose_worker(
    scheduler: &SimpleScheduler,
    worker_scheduler: &dyn WorkerScheduler,
    memory_kb: u64,
) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    worker_scheduler
        .worker_disconnected(&WorkerId(WORKER.to_string()))
        .await?;
    tokio::task::yield_now().await;
    add_worker(scheduler, memory_kb).await
}

#[nativelink_test]
async fn loss_without_policy_requeues_unchanged() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with_policy(None, 5);
    let mut rx = add_worker(&scheduler, 1_000_000).await?;
    let _listener = add_action(&scheduler, 10, 1_000).await?;
    for loss in 1..=3 {
        let (dispatch, memory) = next_dispatch(&mut rx).await;
        assert_eq!(memory, 1_000, "loss {loss}: no policy, no escalation");
        report_peak(worker_scheduler.as_ref(), &dispatch.operation_id, 5_000).await?;
        rx = lose_worker(&scheduler, worker_scheduler.as_ref(), 1_000_000).await?;
    }
    let (_, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 1_000);
    Ok(())
}

#[nativelink_test]
async fn a_sampled_loss_escalates_at_once() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with_policy(Some(policy(vec![], 0)), 5);
    let mut rx = add_worker(&scheduler, 1_000_000).await?;
    let _listener = add_action(&scheduler, 11, 1_000).await?;
    let (first, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 1_000);
    report_peak(worker_scheduler.as_ref(), &first.operation_id, 1_300).await?;
    rx = lose_worker(&scheduler, worker_scheduler.as_ref(), 1_000_000).await?;
    let (second, memory) = next_dispatch(&mut rx).await;
    assert_eq!(second.operation_id, first.operation_id);
    assert_eq!(
        memory, 2_600,
        "grown from the reported peak, the larger of peak and reservation"
    );
    Ok(())
}

#[nativelink_test]
async fn a_repeated_loss_escalates_on_the_second() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with_policy(Some(policy(vec![], 0)), 5);
    let mut rx = add_worker(&scheduler, 1_000_000).await?;
    let _listener = add_action(&scheduler, 12, 1_000).await?;
    // No keepalive ever reports this action: an older worker, or a climb
    // between two keepalives.
    let expected = [1_000, 1_000, 2_000, 4_000];
    for (loss, want) in expected.iter().enumerate() {
        let (_, memory) = next_dispatch(&mut rx).await;
        assert_eq!(memory, *want, "dispatch after {loss} losses");
        if loss + 1 < expected.len() {
            rx = lose_worker(&scheduler, worker_scheduler.as_ref(), 1_000_000).await?;
        }
    }
    Ok(())
}

#[nativelink_test]
async fn an_admin_removal_never_escalates() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with_policy(Some(policy(vec![], 0)), 5);
    let mut rx = add_worker(&scheduler, 1_000_000).await?;
    let _listener = add_action(&scheduler, 13, 1_000).await?;
    for loss in 1..=3 {
        let (dispatch, memory) = next_dispatch(&mut rx).await;
        assert_eq!(memory, 1_000, "removal {loss}");
        report_peak(worker_scheduler.as_ref(), &dispatch.operation_id, 5_000).await?;
        worker_scheduler
            .remove_worker(&WorkerId(WORKER.to_string()))
            .await?;
        tokio::task::yield_now().await;
        rx = add_worker(&scheduler, 1_000_000).await?;
    }
    let (_, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 1_000);
    Ok(())
}

#[nativelink_test]
async fn a_draining_worker_loss_never_escalates() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with_policy(Some(policy(vec![], 0)), 5);
    // A second large worker, kept busy, so the fleet has room to escalate
    // into even while the lost worker is draining: only the drain rule
    // keeps the reservation where it is.
    let mut other_rx = add_named_worker(&scheduler, "other", 1_000_000, 1).await?;
    let _filler = add_action(&scheduler, 20, 1).await?;
    let (_filler_dispatch, _) = next_dispatch(&mut other_rx).await;
    let mut rx = add_worker(&scheduler, 1_000_000).await?;
    let _listener = add_action(&scheduler, 14, 1_000).await?;
    for loss in 1..=3 {
        let (dispatch, memory) = next_dispatch(&mut rx).await;
        assert_eq!(memory, 1_000, "drained loss {loss}");
        report_peak(worker_scheduler.as_ref(), &dispatch.operation_id, 5_000).await?;
        worker_scheduler
            .set_drain_worker(&WorkerId(WORKER.to_string()), true)
            .await?;
        rx = lose_worker(&scheduler, worker_scheduler.as_ref(), 1_000_000).await?;
    }
    let (_, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 1_000);
    Ok(())
}

#[nativelink_test]
async fn loss_escalations_spend_max_steps() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with_policy(Some(policy(vec![], 1)), 5);
    let mut rx = add_worker(&scheduler, 1_000_000).await?;
    let _listener = add_action(&scheduler, 15, 1_000).await?;
    // Each report is 20% over: the one escalation grows from that peak.
    for want in [1_000, 2_400, 2_400, 2_400] {
        let (dispatch, memory) = next_dispatch(&mut rx).await;
        assert_eq!(memory, want, "one escalation, then the budget is spent");
        report_peak(
            worker_scheduler.as_ref(),
            &dispatch.operation_id,
            memory + memory / 5,
        )
        .await?;
        rx = lose_worker(&scheduler, worker_scheduler.as_ref(), 1_000_000).await?;
    }
    Ok(())
}

#[nativelink_test]
async fn a_loss_at_the_whole_worker_ends_on_the_loss_budget() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with_policy(Some(policy(vec![], 0)), 2);
    let mut rx = add_worker(&scheduler, 5_000).await?;
    let mut listener = add_action(&scheduler, 16, 5_000).await?;
    for loss in 1..=2 {
        let (dispatch, memory) = next_dispatch(&mut rx).await;
        assert_eq!(memory, 5_000, "loss {loss}: already the whole worker");
        report_peak(worker_scheduler.as_ref(), &dispatch.operation_id, 9_000).await?;
        rx = lose_worker(&scheduler, worker_scheduler.as_ref(), 5_000).await?;
    }
    let (_, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 5_000);
    worker_scheduler
        .worker_disconnected(&WorkerId(WORKER.to_string()))
        .await?;
    let result = wait_for_completion(&mut listener).await;
    let err = result.error.expect("the loss budget ends it");
    assert_eq!(err.code, Code::FailedPrecondition, "{err}");
    assert!(err.to_string().contains("lost 3 times"), "{err}");
    Ok(())
}

#[nativelink_test]
async fn a_sampled_loss_spares_the_co_tenant_under_its_reservation() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with_policy(Some(policy(vec![], 0)), 5);
    let mut rx = add_worker(&scheduler, 1_000_000).await?;
    let _grower = add_action(&scheduler, 17, 1_000).await?;
    let _neighbour = add_action(&scheduler, 18, 1_000).await?;
    let (a, _) = next_dispatch(&mut rx).await;
    let (b, _) = next_dispatch(&mut rx).await;
    let digest_of = |start: &StartExecute| {
        start
            .execute_request
            .as_ref()
            .and_then(|request| request.action_digest.clone())
            .map(|digest| digest.hash)
            .expect("the dispatch names its action")
    };
    let grower_hash = Digest::from(DigestInfo::new([17; 32], 512)).hash;
    let (grower, neighbour) = if digest_of(&a) == grower_hash {
        (a, b)
    } else {
        (b, a)
    };
    report_peak(worker_scheduler.as_ref(), &grower.operation_id, 1_500).await?;
    report_peak(worker_scheduler.as_ref(), &neighbour.operation_id, 800).await?;
    rx = lose_worker(&scheduler, worker_scheduler.as_ref(), 1_000_000).await?;
    let mut memory_by_operation = HashMap::new();
    for _ in 0..2 {
        let (start, memory) = next_dispatch(&mut rx).await;
        memory_by_operation.insert(start.operation_id, memory);
    }
    assert_eq!(memory_by_operation[&grower.operation_id], 3_000);
    assert_eq!(
        memory_by_operation[&neighbour.operation_id], 1_000,
        "a first loss with the co-tenant under its reservation leaves it alone"
    );
    Ok(())
}

/// An action killed cleanly at its first class escalates to the second,
/// then outgrows that faster than the worker's enforcement samples it, so
/// its pod is OOM-killed whole: a lost worker, not a kill. Without a
/// keepalive catching the climb it reaches the third class on the second
/// loss; with one, on the first. Either way it completes there instead of
/// staying at the second until the loss budget fails it.
#[nativelink_test]
async fn pod_oom_losses_climb_to_the_next_class() -> Result<(), Error> {
    const GIB: u64 = 1024 * 1024;
    let ladder = vec![6 * GIB, 12 * GIB, 16 * GIB, 24 * GIB];
    for sampled in [false, true] {
        let (scheduler, worker_scheduler) =
            make_scheduler_with_policy(Some(policy(ladder.clone(), 8)), 10);
        let mut rx = add_worker(&scheduler, 24 * GIB).await?;
        let mut listener = add_action(&scheduler, 19, 6 * GIB).await?;

        let (first, memory) = next_dispatch(&mut rx).await;
        assert_eq!(memory, 6 * GIB);
        report_memory_kill(worker_scheduler.as_ref(), &first.operation_id, 6 * GIB).await?;

        let (second, memory) = next_dispatch(&mut rx).await;
        assert_eq!(memory, 12 * GIB, "the clean kill at 6 GiB escalates");
        if sampled {
            // The keepalive caught it at 15 GiB before the pod died.
            report_peak(worker_scheduler.as_ref(), &second.operation_id, 15 * GIB).await?;
        }
        rx = lose_worker(&scheduler, worker_scheduler.as_ref(), 24 * GIB).await?;
        if !sampled {
            let (_, memory) = next_dispatch(&mut rx).await;
            assert_eq!(memory, 12 * GIB, "one unsampled loss is not yet a pattern");
            rx = lose_worker(&scheduler, worker_scheduler.as_ref(), 24 * GIB).await?;
        }
        let (third, memory) = next_dispatch(&mut rx).await;
        assert_eq!(memory, 16 * GIB, "sampled={sampled}: the next class up");
        worker_scheduler
            .update_action(
                &WorkerId(WORKER.to_string()),
                &OperationId::from(third.operation_id.as_str()),
                UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(
                    ActionResult::default(),
                )),
            )
            .await?;
        let result = wait_for_completion(&mut listener).await;
        assert!(
            result.error.is_none(),
            "sampled={sampled}: {:?}",
            result.error
        );
    }
    Ok(())
}

/// A kill sampled far past its reservation grows from the peak: the next
/// class above the 5000 KiB it was seen using, not the one above the 1500
/// it asked for, which the action was just observed to exceed.
#[nativelink_test]
async fn escalation_steps_past_the_sampled_peak() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with_ladder(5, vec![1_000, 4_000, 9_000]);
    let mut rx = add_worker(&scheduler, 1_000_000).await?;
    let _listener = add_action(&scheduler, 21, 1_500).await?;
    let (first, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 1_500);
    report_memory_kill_at(worker_scheduler.as_ref(), &first.operation_id, 1_500, 5_000).await?;
    let (_, memory) = next_dispatch(&mut rx).await;
    assert_eq!(
        memory, 9_000,
        "the class above the peak, not above the reservation"
    );
    Ok(())
}

/// `escalate_after_worker_losses` sets the loss that escalates: 0 never
/// does, 3 waits for the third.
#[nativelink_test]
async fn the_loss_threshold_is_configurable() -> Result<(), Error> {
    for (threshold, expected) in [
        (0, [1_000, 1_000, 1_000, 1_000]),
        (3, [1_000, 1_000, 1_000, 2_000]),
    ] {
        let spec = MemoryEscalationSpec {
            escalate_after_worker_losses: threshold,
            ..policy(vec![], 0)
        };
        let (scheduler, worker_scheduler) = make_scheduler_with_policy(Some(spec), 5);
        let mut rx = add_worker(&scheduler, 1_000_000).await?;
        let _listener = add_action(&scheduler, 22, 1_000).await?;
        for (losses, want) in expected.iter().enumerate() {
            let (_, memory) = next_dispatch(&mut rx).await;
            assert_eq!(
                memory, *want,
                "threshold {threshold}, after {losses} losses"
            );
            if losses + 1 < expected.len() {
                rx = lose_worker(&scheduler, worker_scheduler.as_ref(), 1_000_000).await?;
            }
        }
    }
    Ok(())
}

/// The keepalive's evidence does not wait for the loss threshold: with
/// `escalate_after_worker_losses` at 0, a loss after a report over the
/// reservation still escalates, and one without a report does not.
#[nativelink_test]
async fn a_sampled_loss_escalates_with_the_threshold_off() -> Result<(), Error> {
    let spec = MemoryEscalationSpec {
        escalate_after_worker_losses: 0,
        ..policy(vec![], 0)
    };
    let (scheduler, worker_scheduler) = make_scheduler_with_policy(Some(spec), 5);
    let mut rx = add_worker(&scheduler, 1_000_000).await?;
    let _listener = add_action(&scheduler, 23, 1_000).await?;
    let (first, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 1_000);
    rx = lose_worker(&scheduler, worker_scheduler.as_ref(), 1_000_000).await?;
    let (second, memory) = next_dispatch(&mut rx).await;
    assert_eq!(second.operation_id, first.operation_id);
    assert_eq!(memory, 1_000, "no report, threshold off: unchanged");
    report_peak(worker_scheduler.as_ref(), &second.operation_id, 1_300).await?;
    rx = lose_worker(&scheduler, worker_scheduler.as_ref(), 1_000_000).await?;
    let (_, memory) = next_dispatch(&mut rx).await;
    assert_eq!(memory, 2_600, "the report alone escalates");
    Ok(())
}
