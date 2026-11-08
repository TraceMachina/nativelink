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

//! What the scheduler does with a worker's outcomes: a command timeout is
//! queued again for another worker once, a retry avoids the worker that
//! failed it while another can take it, and the outcome tripwire drains a
//! worker whose recent attempts keep failing.

use core::time::Duration;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use mock_instant::thread_local::MockClock;
use nativelink_config::schedulers::{SimpleSpec, WorkerOutcomeTripwireSpec};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    ConnectionResult, UpdateForWorker, update_for_worker,
};
use nativelink_scheduler::api_worker_scheduler::AVOID_FALLBACK_AFTER;
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::{
    ActionResult, ActionStage, LOST_RESULT_ERROR_TIP, OperationId, WorkerId,
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

fn make_system_time(add_time: u64) -> SystemTime {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(NOW_TIME + add_time))
        .unwrap()
}

fn make_scheduler(spec: &SimpleSpec) -> Arc<SimpleScheduler> {
    make_scheduler_with_notify(spec).0
}

/// The scheduler and the notify that wakes its matching pass, for a test
/// that moves the clock and wants the pass to look again.
fn make_scheduler_with_notify(spec: &SimpleSpec) -> (Arc<SimpleScheduler>, Arc<Notify>) {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let task_change_notify = Arc::new(Notify::new());
    let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
        spec,
        memory_awaited_action_db_factory(
            0,
            &task_change_notify.clone(),
            MockInstantWrapped::default,
        ),
        || async move {},
        task_change_notify.clone(),
        MockInstantWrapped::default,
        None,
    );
    (scheduler, task_change_notify)
}

fn tripwire(min_failures: usize, failure_percent: usize, max_drained_percent: usize) -> SimpleSpec {
    SimpleSpec {
        // A timeout is final here, so each one is one judged attempt.
        max_timeout_retries: Some(0),
        worker_outcome_tripwire: Some(WorkerOutcomeTripwireSpec {
            window: 10,
            min_failures,
            failure_percent,
            max_drained_percent,
        }),
        ..Default::default()
    }
}

async fn setup_new_worker(
    scheduler: &SimpleScheduler,
    worker_id: &WorkerId,
) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    setup_new_worker_with(scheduler, worker_id, PlatformProperties::default()).await
}

/// A worker advertising the node it is on, as the chart's
/// `advertiseNodeName` has it do.
fn on_node(node: &str) -> PlatformProperties {
    let mut props = PlatformProperties::default();
    props.properties.insert(
        "node_name".to_string(),
        PlatformPropertyValue::Priority(node.to_string()),
    );
    props
}

async fn setup_new_worker_with(
    scheduler: &SimpleScheduler,
    worker_id: &WorkerId,
    props: PlatformProperties,
) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    let (tx, mut rx) = mpsc::channel(64);
    let worker = Worker::new(worker_id.clone(), props, tx, NOW_TIME, 0);
    scheduler
        .add_worker(worker)
        .await
        .err_tip(|| "Failed to add worker")?;
    tokio::task::yield_now().await;
    let Some(update_for_worker::Update::ConnectionResult(ConnectionResult {
        worker_id: sent, ..
    })) = rx.recv().await.unwrap().update
    else {
        panic!("expected a ConnectionResult first");
    };
    assert_eq!(sent, String::from(worker_id.clone()));
    Ok(rx)
}

async fn setup_action(
    scheduler: &SimpleScheduler,
    digest_byte: u8,
) -> Result<Box<dyn ActionStateResult>, Error> {
    let action_info = make_base_action_info(
        make_system_time(u64::from(digest_byte)),
        DigestInfo::new([digest_byte; 32], 512),
    );
    let result = scheduler
        .add_action(OperationId::default(), action_info)
        .await;
    tokio::task::yield_now().await;
    result
}

fn start_action_id(update: Option<UpdateForWorker>) -> OperationId {
    match update.and_then(|u| u.update) {
        Some(update_for_worker::Update::StartAction(exec)) => {
            OperationId::from(exec.operation_id.as_str())
        }
        other => panic!("expected a StartAction, got {other:?}"),
    }
}

/// The next dispatch to this worker, within a bound so a test that expects
/// one and gets none fails instead of hanging.
async fn next_start(rx: &mut mpsc::Receiver<UpdateForWorker>) -> OperationId {
    let update = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("no dispatch to the worker within 5 s");
    start_action_id(update)
}

/// Which of two workers the next dispatch went to, and the operation.
async fn next_start_on_either(
    rx1: &mut mpsc::Receiver<UpdateForWorker>,
    rx2: &mut mpsc::Receiver<UpdateForWorker>,
) -> (usize, OperationId) {
    tokio::select! {
        update = rx1.recv() => (1, start_action_id(update)),
        update = rx2.recv() => (2, start_action_id(update)),
    }
}

/// The same over three workers.
async fn next_start_on_three(
    rx1: &mut mpsc::Receiver<UpdateForWorker>,
    rx2: &mut mpsc::Receiver<UpdateForWorker>,
    rx3: &mut mpsc::Receiver<UpdateForWorker>,
) -> (usize, OperationId) {
    tokio::select! {
        update = rx1.recv() => (1, start_action_id(update)),
        update = rx2.recv() => (2, start_action_id(update)),
        update = rx3.recv() => (3, start_action_id(update)),
    }
}

async fn expect_stage(listener: &mut Box<dyn ActionStateResult>, expected: &str) -> ActionStage {
    let (state, _) = listener
        .changed()
        .await
        .expect("the action listener closed");
    let name = match &state.stage {
        ActionStage::Queued => "queued",
        ActionStage::Executing => "executing",
        ActionStage::Completed(_) => "completed",
        other => panic!("unexpected stage {other:?}"),
    };
    assert_eq!(name, expected, "stage {:?}", state.stage);
    state.stage.clone()
}

/// The worker's report of a command that ran past its timeout: a result
/// carrying `DeadlineExceeded`, as `running_actions_manager` sends it.
fn timed_out() -> UpdateOperationType {
    UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(ActionResult {
        exit_code: 9,
        error: Some(make_err!(
            Code::DeadlineExceeded,
            "Command 'sleep' timed out after 60 seconds"
        )),
        ..ActionResult::default()
    }))
}

fn finished(exit_code: i32) -> UpdateOperationType {
    UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(ActionResult {
        exit_code,
        ..ActionResult::default()
    }))
}

fn is_timeout(stage: &ActionStage) -> bool {
    matches!(stage, ActionStage::Completed(result)
        if result.error.as_ref().is_some_and(|err| err.code == Code::DeadlineExceeded))
}

async fn is_draining(scheduler: &SimpleScheduler, worker_id: &WorkerId) -> bool {
    scheduler
        .worker_snapshot()
        .await
        .into_iter()
        .find(|summary| summary.id == worker_id.0)
        .map_or_else(
            || panic!("worker {worker_id} is not in the snapshot"),
            |summary| summary.is_draining,
        )
}

#[nativelink_test]
async fn command_timeout_is_queued_again_for_another_worker_test() -> Result<(), Error> {
    let scheduler = make_scheduler(&SimpleSpec::default());
    let w1 = WorkerId("w1".to_string());
    let w2 = WorkerId("w2".to_string());
    let mut rx1 = setup_new_worker(&scheduler, &w1).await?;
    let mut rx2 = setup_new_worker(&scheduler, &w2).await?;
    let mut listener = setup_action(&scheduler, 1).await?;

    let (first, operation_id) = next_start_on_either(&mut rx1, &mut rx2).await;
    expect_stage(&mut listener, "executing").await;
    let (first_id, other_rx, other_id) = if first == 1 {
        (&w1, &mut rx2, &w2)
    } else {
        (&w2, &mut rx1, &w1)
    };

    scheduler
        .update_action(first_id, &operation_id, timed_out())
        .await?;
    // Queued again, and the second worker gets the same operation.
    expect_stage(&mut listener, "queued").await;
    let retried = next_start(other_rx).await;
    assert_eq!(retried, operation_id);
    expect_stage(&mut listener, "executing").await;

    scheduler
        .update_action(other_id, &operation_id, finished(0))
        .await?;
    let stage = expect_stage(&mut listener, "completed").await;
    let ActionStage::Completed(result) = stage else {
        unreachable!()
    };
    assert_eq!(result.exit_code, 0);
    assert!(result.error.is_none(), "{:?}", result.error);
    Ok(())
}

#[nativelink_test]
async fn second_command_timeout_is_final_test() -> Result<(), Error> {
    let scheduler = make_scheduler(&SimpleSpec::default());
    let w1 = WorkerId("w1".to_string());
    let w2 = WorkerId("w2".to_string());
    let mut rx1 = setup_new_worker(&scheduler, &w1).await?;
    let mut rx2 = setup_new_worker(&scheduler, &w2).await?;
    let mut listener = setup_action(&scheduler, 2).await?;

    let (first, operation_id) = next_start_on_either(&mut rx1, &mut rx2).await;
    expect_stage(&mut listener, "executing").await;
    let (first_id, other_rx, other_id) = if first == 1 {
        (&w1, &mut rx2, &w2)
    } else {
        (&w2, &mut rx1, &w1)
    };
    scheduler
        .update_action(first_id, &operation_id, timed_out())
        .await?;
    expect_stage(&mut listener, "queued").await;
    assert_eq!(next_start(other_rx).await, operation_id);
    expect_stage(&mut listener, "executing").await;

    // The second worker times out too: the action is slow by itself.
    scheduler
        .update_action(other_id, &operation_id, timed_out())
        .await?;
    let stage = expect_stage(&mut listener, "completed").await;
    assert!(is_timeout(&stage), "{stage:?}");
    Ok(())
}

#[nativelink_test]
async fn command_timeout_retry_returns_to_a_lone_worker_test() -> Result<(), Error> {
    let (scheduler, notify) = make_scheduler_with_notify(&SimpleSpec::default());
    let w1 = WorkerId("w1".to_string());
    let mut rx1 = setup_new_worker(&scheduler, &w1).await?;
    let mut listener = setup_action(&scheduler, 3).await?;

    let operation_id = next_start(&mut rx1).await;
    expect_stage(&mut listener, "executing").await;
    scheduler
        .update_action(&w1, &operation_id, timed_out())
        .await?;
    expect_stage(&mut listener, "queued").await;
    // Inside the grace the lone worker is not offered it: another replica
    // might have a worker that did not fail it.
    notify.notify_one();
    tokio::task::yield_now().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(200), rx1.recv())
            .await
            .is_err(),
        "the worker that timed the action out got it back inside the grace"
    );
    // Past the grace, with no other taker, the same worker does.
    MockClock::advance(AVOID_FALLBACK_AFTER + Duration::from_secs(1));
    notify.notify_one();
    assert_eq!(next_start(&mut rx1).await, operation_id);
    expect_stage(&mut listener, "executing").await;
    Ok(())
}

#[nativelink_test]
async fn a_lost_result_does_not_avoid_the_worker_test() -> Result<(), Error> {
    // The sweep requeues an operation whose result never reached the
    // scheduler through the error path. The worker ran it; the retry may go
    // straight back to that worker, with no wait for another.
    let scheduler = make_scheduler(&SimpleSpec {
        max_job_retries: 3,
        ..Default::default()
    });
    let w1 = WorkerId("w1".to_string());
    let mut rx1 = setup_new_worker(&scheduler, &w1).await?;
    let mut listener = setup_action(&scheduler, 6).await?;

    let operation_id = next_start(&mut rx1).await;
    expect_stage(&mut listener, "executing").await;
    scheduler
        .update_action(
            &w1,
            &operation_id,
            UpdateOperationType::UpdateWithError(make_err!(
                Code::Internal,
                "{LOST_RESULT_ERROR_TIP}"
            )),
        )
        .await?;
    expect_stage(&mut listener, "queued").await;
    assert_eq!(next_start(&mut rx1).await, operation_id);
    expect_stage(&mut listener, "executing").await;
    Ok(())
}

#[nativelink_test]
async fn a_lone_worker_retry_wakes_the_matcher_for_the_fallback_test() -> Result<(), Error> {
    // With the only worker that fits avoided, the retry waits for a peer
    // until AVOID_FALLBACK_AFTER. There is no periodic pass here (the
    // spec's default interval is off), so the pass that left it waiting
    // must arrange the wake-up for that moment itself; the simulation's
    // property test found a retry left waiting until its budget ran out.
    let (scheduler, notify) = make_scheduler_with_notify(&SimpleSpec {
        max_job_retries: 3,
        ..Default::default()
    });
    let w1 = WorkerId("w1".to_string());
    let mut rx1 = setup_new_worker(&scheduler, &w1).await?;
    let mut listener = setup_action(&scheduler, 7).await?;

    let operation_id = next_start(&mut rx1).await;
    expect_stage(&mut listener, "executing").await;
    scheduler
        .update_action(
            &w1,
            &operation_id,
            UpdateOperationType::UpdateWithError(make_err!(Code::Internal, "disk full")),
        )
        .await?;
    expect_stage(&mut listener, "queued").await;
    scheduler.do_try_match_for_test().await?;
    assert_eq!(
        scheduler.next_wake_for_test(),
        Some(AVOID_FALLBACK_AFTER),
        "the pass that left the retry waiting arms the wake-up for its fallback"
    );
    MockClock::advance(AVOID_FALLBACK_AFTER + Duration::from_secs(1));
    assert_eq!(scheduler.next_wake_for_test(), Some(Duration::ZERO));
    notify.notify_one();
    assert_eq!(next_start(&mut rx1).await, operation_id);
    expect_stage(&mut listener, "executing").await;
    Ok(())
}

#[nativelink_test]
async fn max_timeout_retries_zero_keeps_a_timeout_final_test() -> Result<(), Error> {
    let scheduler = make_scheduler(&SimpleSpec {
        max_timeout_retries: Some(0),
        ..Default::default()
    });
    let w1 = WorkerId("w1".to_string());
    let w2 = WorkerId("w2".to_string());
    let mut rx1 = setup_new_worker(&scheduler, &w1).await?;
    let mut rx2 = setup_new_worker(&scheduler, &w2).await?;
    let mut listener = setup_action(&scheduler, 4).await?;

    let (first, operation_id) = next_start_on_either(&mut rx1, &mut rx2).await;
    expect_stage(&mut listener, "executing").await;
    let first_id = if first == 1 { &w1 } else { &w2 };
    scheduler
        .update_action(first_id, &operation_id, timed_out())
        .await?;
    let stage = expect_stage(&mut listener, "completed").await;
    assert!(is_timeout(&stage), "{stage:?}");
    Ok(())
}

#[nativelink_test]
async fn retry_after_a_worker_error_prefers_another_worker_test() -> Result<(), Error> {
    let scheduler = make_scheduler(&SimpleSpec {
        max_job_retries: 3,
        ..Default::default()
    });
    let w1 = WorkerId("w1".to_string());
    let w2 = WorkerId("w2".to_string());
    let mut rx1 = setup_new_worker(&scheduler, &w1).await?;
    let mut rx2 = setup_new_worker(&scheduler, &w2).await?;
    let mut listener = setup_action(&scheduler, 5).await?;

    let (first, operation_id) = next_start_on_either(&mut rx1, &mut rx2).await;
    expect_stage(&mut listener, "executing").await;
    let (first_id, other_rx) = if first == 1 {
        (&w1, &mut rx2)
    } else {
        (&w2, &mut rx1)
    };
    scheduler
        .update_action(
            first_id,
            &operation_id,
            UpdateOperationType::UpdateWithError(make_err!(Code::Internal, "disk full")),
        )
        .await?;
    expect_stage(&mut listener, "queued").await;
    assert_eq!(next_start(other_rx).await, operation_id);
    Ok(())
}

#[nativelink_test]
async fn tripwire_drains_a_worker_whose_attempts_keep_failing_test() -> Result<(), Error> {
    let scheduler = make_scheduler(&tripwire(2, 50, 100));
    let w1 = WorkerId("w1".to_string());
    let mut rx1 = setup_new_worker(&scheduler, &w1).await?;

    for digest in [10u8, 11] {
        let mut listener = setup_action(&scheduler, digest).await?;
        let operation_id = next_start(&mut rx1).await;
        expect_stage(&mut listener, "executing").await;
        scheduler
            .update_action(&w1, &operation_id, timed_out())
            .await?;
        expect_stage(&mut listener, "completed").await;
    }
    assert!(
        is_draining(&scheduler, &w1).await,
        "two timeouts drain the worker"
    );

    // A worker that joins now takes the next action; the drained one gets
    // nothing more.
    let w2 = WorkerId("w2".to_string());
    let mut rx2 = setup_new_worker(&scheduler, &w2).await?;
    let mut listener = setup_action(&scheduler, 12).await?;
    let operation_id = next_start(&mut rx2).await;
    expect_stage(&mut listener, "executing").await;
    assert!(
        rx1.try_recv().is_err(),
        "the drained worker was offered work"
    );
    scheduler
        .update_action(&w2, &operation_id, finished(0))
        .await?;
    assert!(!is_draining(&scheduler, &w2).await);
    Ok(())
}

#[nativelink_test]
async fn tripwire_holds_when_its_share_is_drained_test() -> Result<(), Error> {
    // Four workers, a quarter of them drainable: the floor of two applies,
    // and the third worker that would trip is held.
    let scheduler = make_scheduler(&tripwire(2, 50, 25));
    let ids: Vec<WorkerId> = (1..=4).map(|i| WorkerId(format!("w{i}"))).collect();
    let mut rx1 = setup_new_worker(&scheduler, &ids[0]).await?;
    let mut rx2 = setup_new_worker(&scheduler, &ids[1]).await?;
    let mut rx3 = setup_new_worker(&scheduler, &ids[2]).await?;
    let mut rx4 = setup_new_worker(&scheduler, &ids[3]).await?;
    // Enough failures to trip every worker twice over, whoever gets them.
    for digest in 20u8..32 {
        let mut listener = setup_action(&scheduler, digest).await?;
        let (which, operation_id) = tokio::select! {
            u = rx1.recv() => (0, start_action_id(u)),
            u = rx2.recv() => (1, start_action_id(u)),
            u = rx3.recv() => (2, start_action_id(u)),
            u = rx4.recv() => (3, start_action_id(u)),
        };
        expect_stage(&mut listener, "executing").await;
        scheduler
            .update_action(&ids[which], &operation_id, timed_out())
            .await?;
        expect_stage(&mut listener, "completed").await;
    }
    let mut drained = Vec::new();
    for id in &ids {
        drained.push(is_draining(&scheduler, id).await);
    }
    assert_eq!(
        drained.iter().filter(|d| **d).count(),
        2,
        "exactly two workers drained (the floor): {drained:?}"
    );
    Ok(())
}

#[nativelink_test]
async fn tripwire_counts_a_result_with_a_nonzero_exit_as_success_test() -> Result<(), Error> {
    let scheduler = make_scheduler(&tripwire(2, 50, 100));
    let w1 = WorkerId("w1".to_string());
    let mut rx1 = setup_new_worker(&scheduler, &w1).await?;

    // A failing command, a timeout, another failing command: one failure
    // of three, under both thresholds.
    for (digest, update) in [(30u8, finished(1)), (31, timed_out()), (32, finished(1))] {
        let mut listener = setup_action(&scheduler, digest).await?;
        let operation_id = next_start(&mut rx1).await;
        expect_stage(&mut listener, "executing").await;
        scheduler.update_action(&w1, &operation_id, update).await?;
        expect_stage(&mut listener, "completed").await;
    }
    assert!(
        !is_draining(&scheduler, &w1).await,
        "exit codes are not failures"
    );

    // A second timeout: two of four, half of them.
    let mut listener = setup_action(&scheduler, 33).await?;
    let operation_id = next_start(&mut rx1).await;
    expect_stage(&mut listener, "executing").await;
    scheduler
        .update_action(&w1, &operation_id, timed_out())
        .await?;
    expect_stage(&mut listener, "completed").await;
    assert!(is_draining(&scheduler, &w1).await);
    Ok(())
}

#[nativelink_test]
async fn undraining_a_tripped_worker_lets_it_be_judged_afresh_test() -> Result<(), Error> {
    let scheduler = make_scheduler(&tripwire(2, 50, 100));
    let w1 = WorkerId("w1".to_string());
    let mut rx1 = setup_new_worker(&scheduler, &w1).await?;
    for digest in [40u8, 41] {
        let mut listener = setup_action(&scheduler, digest).await?;
        let operation_id = next_start(&mut rx1).await;
        expect_stage(&mut listener, "executing").await;
        scheduler
            .update_action(&w1, &operation_id, timed_out())
            .await?;
        expect_stage(&mut listener, "completed").await;
    }
    assert!(is_draining(&scheduler, &w1).await);

    scheduler.set_drain_worker(&w1, false).await?;
    assert!(!is_draining(&scheduler, &w1).await);
    // One more timeout is one of one judged since: not enough to trip.
    let mut listener = setup_action(&scheduler, 42).await?;
    let operation_id = next_start(&mut rx1).await;
    expect_stage(&mut listener, "executing").await;
    scheduler
        .update_action(&w1, &operation_id, timed_out())
        .await?;
    expect_stage(&mut listener, "completed").await;
    assert!(!is_draining(&scheduler, &w1).await);
    Ok(())
}

#[nativelink_test]
async fn command_timeout_retry_keeps_off_the_whole_node_test() -> Result<(), Error> {
    let scheduler = make_scheduler(&SimpleSpec::default());
    // w1 and w2 share node a; w3 is on node b.
    let (w1, w2, w3) = (
        WorkerId("w1".to_string()),
        WorkerId("w2".to_string()),
        WorkerId("w3".to_string()),
    );
    let mut rx1 = setup_new_worker_with(&scheduler, &w1, on_node("a")).await?;
    let mut rx2 = setup_new_worker_with(&scheduler, &w2, on_node("a")).await?;
    let mut rx3 = setup_new_worker_with(&scheduler, &w3, on_node("b")).await?;
    let node = |which: usize| if which == 3 { "b" } else { "a" };
    let ids = [&w1, &w2, &w3];
    let mut listener = setup_action(&scheduler, 7).await?;

    let (first, operation_id) = next_start_on_three(&mut rx1, &mut rx2, &mut rx3).await;
    expect_stage(&mut listener, "executing").await;
    scheduler
        .update_action(ids[first - 1], &operation_id, timed_out())
        .await?;
    expect_stage(&mut listener, "queued").await;
    let (second, retried) = next_start_on_three(&mut rx1, &mut rx2, &mut rx3).await;
    assert_eq!(retried, operation_id);
    assert_ne!(
        node(second),
        node(first),
        "the retry went to w{second}, on the node w{first} timed it out on"
    );
    Ok(())
}

#[nativelink_test]
async fn the_node_is_remembered_after_the_worker_that_failed_leaves_test() -> Result<(), Error> {
    // Another scheduler replica would not know w1 at all; here w1 is gone
    // by the time the retry is matched, which is the same for the matcher.
    let scheduler = make_scheduler(&SimpleSpec::default());
    let (w1, w2, w3) = (
        WorkerId("w1".to_string()),
        WorkerId("w2".to_string()),
        WorkerId("w3".to_string()),
    );
    let mut rx1 = setup_new_worker_with(&scheduler, &w1, on_node("a")).await?;
    let mut listener = setup_action(&scheduler, 8).await?;
    let operation_id = next_start(&mut rx1).await;
    expect_stage(&mut listener, "executing").await;
    scheduler
        .update_action(&w1, &operation_id, timed_out())
        .await?;
    expect_stage(&mut listener, "queued").await;
    scheduler.remove_worker(&w1).await?;

    // w2 shares w1's node; w3 does not. The record names the node.
    let mut rx2 = setup_new_worker_with(&scheduler, &w2, on_node("a")).await?;
    let mut rx3 = setup_new_worker_with(&scheduler, &w3, on_node("b")).await?;
    let (which, retried) = next_start_on_either(&mut rx2, &mut rx3).await;
    assert_eq!(retried, operation_id);
    assert_eq!(
        which, 2,
        "the retry went to w2, on the node w1 timed it out on"
    );
    Ok(())
}

#[nativelink_test]
async fn tripwire_trips_on_a_run_of_failures_after_a_healthy_history_test() -> Result<(), Error> {
    // Eight good results, then two timeouts in a row: a fifth of the
    // window failed, under the ratio, but the run alone drains the worker.
    let scheduler = make_scheduler(&tripwire(2, 50, 100));
    let w1 = WorkerId("w1".to_string());
    let mut rx1 = setup_new_worker(&scheduler, &w1).await?;
    for digest in 50u8..58 {
        let mut listener = setup_action(&scheduler, digest).await?;
        let operation_id = next_start(&mut rx1).await;
        expect_stage(&mut listener, "executing").await;
        scheduler
            .update_action(&w1, &operation_id, finished(0))
            .await?;
        expect_stage(&mut listener, "completed").await;
    }
    for digest in [58u8, 59] {
        assert!(!is_draining(&scheduler, &w1).await);
        let mut listener = setup_action(&scheduler, digest).await?;
        let operation_id = next_start(&mut rx1).await;
        expect_stage(&mut listener, "executing").await;
        scheduler
            .update_action(&w1, &operation_id, timed_out())
            .await?;
        expect_stage(&mut listener, "completed").await;
    }
    assert!(
        is_draining(&scheduler, &w1).await,
        "two failures in a row drain the worker"
    );
    Ok(())
}

#[nativelink_test]
async fn tripwire_spec_defaults_test() -> Result<(), Error> {
    let tripwire = WorkerOutcomeTripwireSpec::default();
    assert_eq!(
        (
            tripwire.window,
            tripwire.min_failures,
            tripwire.failure_percent,
            tripwire.max_drained_percent
        ),
        (20, 3, 50, 20)
    );
    Ok(())
}
