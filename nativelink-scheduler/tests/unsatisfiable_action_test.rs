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
use std::time::{SystemTime, UNIX_EPOCH};

use futures::poll;
use futures::task::Poll;
use mock_instant::thread_local::MockClock;
use nativelink_config::schedulers::{PropertyType, SimpleSpec};
use nativelink_error::{Code, Error, ErrorContext, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::UpdateForWorker;
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::match_outcome::{PropertyShape, explain_unsatisfiable};
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::unsatisfiable_tracker::UnsatisfiableTracker;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::{ActionStage, OperationId, WorkerId, to_execute_response};
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::operation_state_manager::{ActionStateResult, ClientStateManager};
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};
use pretty_assertions::assert_eq;
use tokio::sync::{Notify, mpsc};
use utils::scheduler_utils::make_base_action_info;

mod utils {
    pub(crate) mod scheduler_utils;
}

const NOW_TIME: u64 = 10000;
const TIMEOUT_S: u64 = 60;

fn make_scheduler(spec: &SimpleSpec) -> Arc<SimpleScheduler> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let task_change_notify = Arc::new(Notify::new());
    let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
        spec,
        memory_awaited_action_db_factory(0, &task_change_notify, MockInstantWrapped::default),
        || async move {},
        task_change_notify,
        MockInstantWrapped::default,
        None,
    );
    scheduler
}

fn make_spec(unsatisfiable_action_timeout_s: u64) -> SimpleSpec {
    SimpleSpec {
        supported_platform_properties: Some(HashMap::from([
            ("gpu_count".to_string(), PropertyType::Minimum),
            ("memory_kb".to_string(), PropertyType::Minimum),
            ("cpu_count".to_string(), PropertyType::Minimum),
            ("OSFamily".to_string(), PropertyType::Priority),
            ("ISA".to_string(), PropertyType::Exact),
        ])),
        // The mock clock moves past the default while nothing polls the
        // client, which would otherwise retire the action first.
        client_action_timeout_s: 1_000_000,
        unsatisfiable_action_timeout_s,
        ..Default::default()
    }
}

fn cpu_worker_properties() -> PlatformProperties {
    PlatformProperties::new(HashMap::from([
        (
            "OSFamily".to_string(),
            PlatformPropertyValue::Priority("Linux".to_string()),
        ),
        ("gpu_count".to_string(), PlatformPropertyValue::Minimum(0)),
        (
            "ISA".to_string(),
            PlatformPropertyValue::Exact("x86-64".to_string()),
        ),
        (
            "cpu_count".to_string(),
            PlatformPropertyValue::Minimum(7000),
        ),
        (
            "memory_kb".to_string(),
            PlatformPropertyValue::Minimum(31_457_280),
        ),
    ]))
}

fn gpu_worker_properties() -> PlatformProperties {
    let mut properties = cpu_worker_properties();
    properties
        .properties
        .insert("gpu_count".to_string(), PlatformPropertyValue::Minimum(1));
    properties.properties.insert(
        "memory_kb".to_string(),
        PlatformPropertyValue::Minimum(41_943_040),
    );
    properties
}

fn cpu_action_properties() -> HashMap<String, String> {
    HashMap::from([
        ("OSFamily".to_string(), "Linux".to_string()),
        ("ISA".to_string(), "x86-64".to_string()),
        ("cpu_count".to_string(), "7000".to_string()),
    ])
}

fn gpu_action_properties() -> HashMap<String, String> {
    HashMap::from([
        ("OSFamily".to_string(), "Linux_t4".to_string()),
        ("gpu_count".to_string(), "1".to_string()),
        ("ISA".to_string(), "x86-64".to_string()),
        ("cpu_count".to_string(), "7000".to_string()),
        ("memory_kb".to_string(), "37748736".to_string()),
    ])
}

async fn add_worker(
    scheduler: &SimpleScheduler,
    name: &str,
    properties: PlatformProperties,
    max_inflight_tasks: u64,
) -> Result<mpsc::UnboundedReceiver<UpdateForWorker>, Error> {
    let (tx, rx) = mpsc::unbounded_channel();
    let worker = Worker::new(
        WorkerId(name.to_string()),
        properties,
        tx,
        NOW_TIME,
        max_inflight_tasks,
    );
    scheduler
        .add_worker(worker)
        .await
        .err_tip(|| "Failed to add worker")?;
    tokio::task::yield_now().await;
    Ok(rx)
}

async fn add_action(
    scheduler: &SimpleScheduler,
    digest_byte: u8,
    platform_properties: HashMap<String, String>,
) -> Result<Box<dyn ActionStateResult>, Error> {
    // Stamp from the mock clock the scheduler reads, so an action added after
    // the clock advanced is as old as it would be in production.
    let insert_timestamp = UNIX_EPOCH + MockClock::time();
    let mut action_info =
        make_base_action_info(insert_timestamp, DigestInfo::new([digest_byte; 32], 512));
    Arc::make_mut(&mut action_info).platform_properties = platform_properties;
    let result = scheduler
        .add_action(OperationId::default(), action_info)
        .await;
    tokio::task::yield_now().await;
    result
}

async fn stage_of(action: &dyn ActionStateResult) -> Result<ActionStage, Error> {
    Ok(action.as_state().await?.0.stage.clone())
}

fn assert_failed_as_unsatisfiable(stage: &ActionStage) -> String {
    let ActionStage::Completed(action_result) = stage else {
        panic!("Expected the action to be completed, got {stage:?}");
    };
    let err = action_result
        .error
        .as_ref()
        .expect("Expected the action to carry an error");
    assert_eq!(err.code, Code::FailedPrecondition);
    assert_eq!(err.context, ErrorContext::None);

    // Bazel only falls back to local execution when the status is not
    // DEADLINE_EXCEEDED, and only skips its retry loop when there are no
    // details attached.
    let status = to_execute_response(action_result.clone())
        .status
        .expect("Expected a status");
    assert_eq!(status.code, Code::FailedPrecondition as i32);
    assert_eq!(status.details, Vec::new());
    err.message_string()
}

#[nativelink_test]
async fn gpu_action_on_cpu_fleet_fails_after_timeout() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let mut worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;
    // Drain the connection result.
    worker_rx.recv().await.unwrap();

    let action = add_action(&scheduler, 1, gpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Queued);

    MockClock::advance(Duration::from_secs(TIMEOUT_S - 1));
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Queued);

    MockClock::advance(Duration::from_secs(1));
    scheduler.do_try_match_for_test().await?;
    let message = assert_failed_as_unsatisfiable(&stage_of(action.as_ref()).await?);
    assert!(
        message.contains(
            "no worker can satisfy: 'gpu_count' requested 1, largest worker total 0; \
             'memory_kb' requested 37748736, largest worker total 31457280"
        ),
        "Unexpected message: {message}"
    );
    assert!(!message.contains("OSFamily"), "{message}");

    // The failure is final, and the worker was never sent the action.
    scheduler.do_try_match_for_test().await?;
    assert!(matches!(
        stage_of(action.as_ref()).await?,
        ActionStage::Completed(_)
    ));
    assert!(worker_rx.try_recv().is_err());
    Ok(())
}

#[nativelink_test]
async fn later_action_of_a_due_shape_waits_its_own_timeout() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    let first = add_action(&scheduler, 1, gpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S));
    scheduler.do_try_match_for_test().await?;
    assert_failed_as_unsatisfiable(&stage_of(first.as_ref()).await?);

    // Differs only in a property that does not restrict matching, so it
    // shares the (now due) shape. It still waits its own timeout rather than
    // failing on sight, so a pool outage does not become an immediate burst.
    let mut properties = gpu_action_properties();
    properties.insert("OSFamily".to_string(), "Linux_h100".to_string());
    let second = add_action(&scheduler, 2, properties).await?;
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(second.as_ref()).await?, ActionStage::Queued);

    MockClock::advance(Duration::from_secs(TIMEOUT_S - 1));
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(second.as_ref()).await?, ActionStage::Queued);

    MockClock::advance(Duration::from_secs(1));
    scheduler.do_try_match_for_test().await?;
    assert_failed_as_unsatisfiable(&stage_of(second.as_ref()).await?);
    Ok(())
}

#[nativelink_test]
async fn actions_queued_together_fail_within_one_timeout() -> Result<(), Error> {
    // A build whose GPU actions all queue at once waits once, not once per
    // action: every one of them has waited the timeout at the same moment.
    const TOTAL: u8 = 5;
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    let mut actions = Vec::new();
    for i in 1..=TOTAL {
        actions.push(add_action(&scheduler, i, gpu_action_properties()).await?);
    }
    scheduler.do_try_match_for_test().await?;
    for action in &actions {
        assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Queued);
    }

    MockClock::advance(Duration::from_secs(TIMEOUT_S));
    scheduler.do_try_match_for_test().await?;
    for action in &actions {
        assert_failed_as_unsatisfiable(&stage_of(action.as_ref()).await?);
    }
    Ok(())
}

#[nativelink_test]
async fn later_action_waits_from_its_own_submission() -> Result<(), Error> {
    // The shape being due is necessary but not sufficient: an action queued
    // while its shape was already waiting fails a full timeout after its own
    // submission, not when the shape comes due and not on a restarted clock.
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    let first = add_action(&scheduler, 1, gpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;

    MockClock::advance(Duration::from_secs(30));
    let second = add_action(&scheduler, 2, gpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;

    // The shape comes due: the first action has waited 60, the second 30.
    MockClock::advance(Duration::from_secs(TIMEOUT_S - 30));
    scheduler.do_try_match_for_test().await?;
    assert_failed_as_unsatisfiable(&stage_of(first.as_ref()).await?);
    assert_eq!(stage_of(second.as_ref()).await?, ActionStage::Queued);

    MockClock::advance(Duration::from_secs(29));
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(second.as_ref()).await?, ActionStage::Queued);

    MockClock::advance(Duration::from_secs(1));
    scheduler.do_try_match_for_test().await?;
    assert_failed_as_unsatisfiable(&stage_of(second.as_ref()).await?);
    Ok(())
}

#[nativelink_test]
async fn young_actions_do_not_back_off_the_wake_for_their_shape() -> Result<(), Error> {
    // While a due shape only has actions that are still too young, nothing is
    // stuck, so the loop must not back off for it: it wakes when the youngest
    // action becomes eligible and fails it on that pass.
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    let first = add_action(&scheduler, 1, gpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S));
    scheduler.do_try_match_for_test().await?;
    assert_failed_as_unsatisfiable(&stage_of(first.as_ref()).await?);

    let second = add_action(&scheduler, 2, gpu_action_properties()).await?;
    // Six passes with the second action still young would have saturated the
    // recheck backoff at 32 seconds.
    for _ in 0..6 {
        MockClock::advance(Duration::from_secs(1));
        scheduler.do_try_match_for_test().await?;
        assert_eq!(stage_of(second.as_ref()).await?, ActionStage::Queued);
    }
    assert_eq!(
        scheduler.unsatisfiable_deadline_for_test(),
        Some(Duration::from_secs(TIMEOUT_S - 6)),
        "the loop must wake when the action becomes eligible, not after a backoff"
    );

    MockClock::advance(Duration::from_secs(TIMEOUT_S - 6));
    scheduler.do_try_match_for_test().await?;
    assert_failed_as_unsatisfiable(&stage_of(second.as_ref()).await?);
    Ok(())
}

#[nativelink_test]
async fn capable_worker_joining_runs_the_action() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _cpu_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    let action = add_action(&scheduler, 1, gpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S - 1));

    let _gpu_rx = add_worker(&scheduler, "gpu", gpu_worker_properties(), 0).await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S));
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Executing);
    Ok(())
}

#[nativelink_test]
async fn unsatisfiable_clock_restarts_after_a_capable_worker_leaves() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _cpu_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    let first = add_action(&scheduler, 1, gpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S - 1));

    // The GPU worker takes the first action and then goes away.
    let _gpu_rx = add_worker(&scheduler, "gpu", gpu_worker_properties(), 0).await?;
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(first.as_ref()).await?, ActionStage::Executing);
    scheduler
        .remove_worker(&WorkerId("gpu".to_string()))
        .await?;

    let second = add_action(&scheduler, 2, gpu_action_properties()).await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S - 1));
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(second.as_ref()).await?, ActionStage::Queued);

    // The verdict from when the GPU worker was connected must not linger.
    // Two seconds rather than one, so the second action's own wait is past
    // the timeout instead of sitting exactly on it.
    MockClock::advance(Duration::from_secs(2));
    scheduler.do_try_match_for_test().await?;
    assert_failed_as_unsatisfiable(&stage_of(second.as_ref()).await?);
    Ok(())
}

#[nativelink_test]
async fn waiting_client_is_woken_by_the_failure() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    let mut action = add_action(&scheduler, 1, gpu_action_properties()).await?;
    // The client's stream reports the queued state first.
    assert_eq!(action.changed().await?.0.stage, ActionStage::Queued);
    scheduler.do_try_match_for_test().await?;

    let changed = action.changed();
    tokio::pin!(changed);
    assert_eq!(poll!(&mut changed), Poll::Pending);

    MockClock::advance(Duration::from_secs(TIMEOUT_S));
    scheduler.do_try_match_for_test().await?;
    let (state, _origin_metadata) = changed.await?;
    assert_failed_as_unsatisfiable(&state.stage);
    Ok(())
}

#[nativelink_test]
async fn zero_timeout_never_fails_but_logs() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(0));
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    let action = add_action(&scheduler, 1, gpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;
    MockClock::advance(Duration::from_hours(24));
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Queued);
    assert!(logs_contain(
        "Queued action cannot run on any connected worker"
    ));
    Ok(())
}

#[nativelink_test]
async fn busy_fleet_is_not_unsatisfiable() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    // Each action asks for every CPU the worker has.
    let running = add_action(&scheduler, 1, cpu_action_properties()).await?;
    let waiting = add_action(&scheduler, 2, cpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(running.as_ref()).await?, ActionStage::Executing);

    MockClock::advance(Duration::from_secs(TIMEOUT_S * 10));
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(waiting.as_ref()).await?, ActionStage::Queued);
    Ok(())
}

#[nativelink_test]
async fn saturated_fleet_does_not_hide_an_unsatisfiable_action() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 1).await?;

    let running = add_action(&scheduler, 1, cpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(running.as_ref()).await?, ActionStage::Executing);

    let impossible = add_action(&scheduler, 2, gpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S));
    scheduler.do_try_match_for_test().await?;
    assert_failed_as_unsatisfiable(&stage_of(impossible.as_ref()).await?);
    Ok(())
}

#[nativelink_test]
async fn empty_fleet_is_never_failed() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));

    let action = add_action(&scheduler, 1, gpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S * 10));
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Queued);
    Ok(())
}

#[nativelink_test]
async fn a_peer_schedulers_capable_worker_prevents_the_failure() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    // Another scheduler on the same state has a GPU worker connected.
    scheduler
        .set_peer_fleet_for_test(vec![gpu_worker_properties()])
        .await;

    let action = add_action(&scheduler, 1, gpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S * 10));
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Queued);

    // Once that worker is gone, the clock starts from there.
    scheduler.set_peer_fleet_for_test(Vec::new()).await;
    scheduler.do_try_match_for_test().await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S - 1));
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Queued);
    MockClock::advance(Duration::from_secs(1));
    scheduler.do_try_match_for_test().await?;
    assert_failed_as_unsatisfiable(&stage_of(action.as_ref()).await?);
    Ok(())
}

#[nativelink_test]
async fn unknown_peers_never_fail_an_action() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    // The exchange with peer schedulers failed, so one of them might have a
    // GPU worker.
    scheduler.set_peers_unknown_for_test().await;

    let action = add_action(&scheduler, 1, gpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S * 10));
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Queued);

    // Once the peers are known to have none, the clock starts from there.
    scheduler.set_peer_fleet_for_test(Vec::new()).await;
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Queued);
    MockClock::advance(Duration::from_secs(TIMEOUT_S));
    scheduler.do_try_match_for_test().await?;
    assert_failed_as_unsatisfiable(&stage_of(action.as_ref()).await?);
    Ok(())
}

#[nativelink_test]
async fn peer_fleet_in_another_order_is_not_a_fleet_change() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let cpu = cpu_worker_properties();
    let gpu = gpu_worker_properties();

    scheduler
        .set_peer_fleet_for_test(vec![cpu.clone(), gpu.clone()])
        .await;
    let generation = scheduler.fleet_generation_for_test().await;

    // Peers list their workers in no particular order, and two peers may
    // have the same kind of worker.
    scheduler
        .set_peer_fleet_for_test(vec![gpu.clone(), cpu.clone(), gpu])
        .await;
    assert_eq!(scheduler.fleet_generation_for_test().await, generation);

    scheduler.set_peer_fleet_for_test(vec![cpu]).await;
    assert_ne!(scheduler.fleet_generation_for_test().await, generation);
    Ok(())
}

#[nativelink_test]
async fn priority_value_mismatch_still_matches() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    let mut properties = cpu_action_properties();
    properties.insert("OSFamily".to_string(), "Linux_t4".to_string());
    let action = add_action(&scheduler, 1, properties).await?;
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Executing);
    Ok(())
}

#[nativelink_test]
async fn undeclared_property_is_not_unsatisfiable_when_a_worker_lacks_the_key() -> Result<(), Error>
{
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    let mut properties = cpu_action_properties();
    properties.insert("gpu_model".to_string(), "h100".to_string());
    let action = add_action(&scheduler, 1, properties).await?;
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Executing);
    Ok(())
}

#[nativelink_test]
async fn exact_mismatch_names_what_workers_offer_only_in_the_log() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    let mut properties = cpu_action_properties();
    properties.insert("ISA".to_string(), "aarch64".to_string());
    let action = add_action(&scheduler, 1, properties).await?;
    scheduler.do_try_match_for_test().await?;
    MockClock::advance(Duration::from_secs(TIMEOUT_S));
    scheduler.do_try_match_for_test().await?;
    let message = assert_failed_as_unsatisfiable(&stage_of(action.as_ref()).await?);
    // Worker values can name internal images or pools, so the client only
    // hears what it asked for.
    assert!(
        message.contains("'ISA' requested aarch64, no worker offers it"),
        "Unexpected message: {message}"
    );
    assert!(!message.contains("x86-64"), "Unexpected message: {message}");
    assert!(logs_contain(
        "'ISA' requested aarch64, workers offer [x86-64]"
    ));
    Ok(())
}

#[nativelink_test]
async fn deadline_is_reached_without_the_fallback_match_interval() -> Result<(), Error> {
    const SHORT_TIMEOUT_S: u64 = 2;
    let mut spec = make_spec(SHORT_TIMEOUT_S);
    spec.fallback_match_interval_s = 0;
    let scheduler = make_scheduler(&spec);
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    // Only the background matching loop runs from here on.
    let action = add_action(&scheduler, 1, gpu_action_properties()).await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Queued);

    MockClock::advance(Duration::from_secs(SHORT_TIMEOUT_S));
    tokio::time::sleep(Duration::from_secs(SHORT_TIMEOUT_S + 1)).await;
    assert_failed_as_unsatisfiable(&stage_of(action.as_ref()).await?);
    Ok(())
}

#[nativelink_test]
async fn deadline_wakes_the_loop_before_the_fallback_match_interval() -> Result<(), Error> {
    const SHORT_TIMEOUT_S: u64 = 2;
    let mut spec = make_spec(SHORT_TIMEOUT_S);
    spec.fallback_match_interval_s = 3600;
    let scheduler = make_scheduler(&spec);
    let _worker_rx = add_worker(&scheduler, "cpu", cpu_worker_properties(), 0).await?;

    let action = add_action(&scheduler, 1, gpu_action_properties()).await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Queued);

    MockClock::advance(Duration::from_secs(SHORT_TIMEOUT_S));
    tokio::time::sleep(Duration::from_secs(SHORT_TIMEOUT_S + 1)).await;
    assert_failed_as_unsatisfiable(&stage_of(action.as_ref()).await?);
    Ok(())
}

#[nativelink_test]
async fn fleet_shapes_are_the_distinct_worker_totals() -> Result<(), Error> {
    let scheduler = make_scheduler(&make_spec(TIMEOUT_S));
    let _a = add_worker(&scheduler, "cpu-a", cpu_worker_properties(), 0).await?;
    let _b = add_worker(&scheduler, "cpu-b", cpu_worker_properties(), 0).await?;
    let _c = add_worker(&scheduler, "gpu", gpu_worker_properties(), 0).await?;

    // Running an action reduces what is free, not what was registered.
    let action = add_action(&scheduler, 1, cpu_action_properties()).await?;
    scheduler.do_try_match_for_test().await?;
    assert_eq!(stage_of(action.as_ref()).await?, ActionStage::Executing);

    let mut shapes = scheduler.fleet_shapes_for_test().await;
    shapes.sort_by_key(|shape| shape.properties["gpu_count"].as_str().into_owned());
    assert_eq!(
        shapes,
        vec![cpu_worker_properties(), gpu_worker_properties()]
    );
    Ok(())
}

fn minimum_properties(values: &[(&str, u64)]) -> PlatformProperties {
    PlatformProperties::new(
        values
            .iter()
            .map(|(name, value)| ((*name).to_string(), PlatformPropertyValue::Minimum(*value)))
            .collect(),
    )
}

#[nativelink_test]
async fn explain_reports_every_failing_property_sorted() -> Result<(), Error> {
    let action = minimum_properties(&[("memory_kb", 100), ("gpu_count", 1), ("cpu_count", 1)]);
    let worker = minimum_properties(&[("memory_kb", 50), ("gpu_count", 0), ("cpu_count", 8)]);

    let reason = explain_unsatisfiable(&action, &[&worker]);
    assert!(!reason.combination_only);
    let names: Vec<_> = reason.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["gpu_count", "memory_kb"]);
    assert_eq!(
        reason.to_string(),
        "no worker can satisfy: 'gpu_count' requested 1, largest worker total 0; \
         'memory_kb' requested 100, largest worker total 50"
    );
    Ok(())
}

#[nativelink_test]
async fn explain_reports_a_combination_no_single_worker_has() -> Result<(), Error> {
    let action = minimum_properties(&[("memory_kb", 100), ("gpu_count", 1)]);
    let gpu_worker = minimum_properties(&[("memory_kb", 50), ("gpu_count", 1)]);
    let big_worker = minimum_properties(&[("memory_kb", 200), ("gpu_count", 0)]);

    let reason = explain_unsatisfiable(&action, &[&gpu_worker, &big_worker]);
    assert!(reason.combination_only);
    let names: Vec<_> = reason.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["gpu_count", "memory_kb"]);
    assert_eq!(
        reason.to_string(),
        "no single worker satisfies the combination of: 'gpu_count' requested 1, \
         largest worker total 1; 'memory_kb' requested 100, largest worker total 200"
    );
    assert_eq!(reason.property_names(), "gpu_count,memory_kb");
    Ok(())
}

#[nativelink_test]
async fn explain_truncates_a_long_list_of_offered_values() -> Result<(), Error> {
    let action = PlatformProperties::new(HashMap::from([(
        "ISA".to_string(),
        PlatformPropertyValue::Exact("riscv".to_string()),
    )]));
    let workers: Vec<_> = (0..10)
        .map(|i| {
            PlatformProperties::new(HashMap::from([(
                "ISA".to_string(),
                PlatformPropertyValue::Exact(format!("arch{i}")),
            )]))
        })
        .collect();
    let fleet: Vec<_> = workers.iter().collect();

    let reason = explain_unsatisfiable(&action, &fleet);
    assert_eq!(
        reason.to_string(),
        "no worker can satisfy: 'ISA' requested riscv, workers offer [arch0, arch1, arch2, \
         arch3, arch4, arch5, arch6, arch7] and 2 more"
    );
    Ok(())
}

#[nativelink_test]
async fn explain_leaves_out_properties_every_worker_satisfies() -> Result<(), Error> {
    let action = PlatformProperties::new(HashMap::from([
        ("memory_kb".to_string(), PlatformPropertyValue::Minimum(100)),
        ("gpu_count".to_string(), PlatformPropertyValue::Minimum(1)),
        (
            "ISA".to_string(),
            PlatformPropertyValue::Exact("x86-64".to_string()),
        ),
        (
            "OSFamily".to_string(),
            PlatformPropertyValue::Priority("Linux_t4".to_string()),
        ),
        (
            "toolchain".to_string(),
            PlatformPropertyValue::Unknown("clang".to_string()),
        ),
    ]));
    let common = |memory_kb, gpu_count| {
        PlatformProperties::new(HashMap::from([
            (
                "memory_kb".to_string(),
                PlatformPropertyValue::Minimum(memory_kb),
            ),
            (
                "gpu_count".to_string(),
                PlatformPropertyValue::Minimum(gpu_count),
            ),
            (
                "ISA".to_string(),
                PlatformPropertyValue::Exact("x86-64".to_string()),
            ),
            (
                "OSFamily".to_string(),
                PlatformPropertyValue::Priority("Linux".to_string()),
            ),
        ]))
    };

    let reason = explain_unsatisfiable(&action, &[&common(50, 1), &common(200, 0)]);
    assert!(reason.combination_only);
    let names: Vec<_> = reason.properties.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, vec!["gpu_count", "memory_kb"]);
    Ok(())
}

#[nativelink_test]
async fn explain_names_a_missing_priority_key_without_its_value() -> Result<(), Error> {
    let action = PlatformProperties::new(HashMap::from([(
        "OSFamily".to_string(),
        PlatformPropertyValue::Priority("Linux_t4".to_string()),
    )]));
    let worker = minimum_properties(&[("cpu_count", 8)]);

    let reason = explain_unsatisfiable(&action, &[&worker]);
    assert_eq!(
        reason.to_string(),
        "no worker can satisfy: 'OSFamily' is required, no worker declares this property"
    );
    Ok(())
}

#[nativelink_test]
async fn explain_reports_a_key_no_worker_declares() -> Result<(), Error> {
    let action = minimum_properties(&[("gpu_count", 1)]);
    let worker = minimum_properties(&[("cpu_count", 8)]);

    let reason = explain_unsatisfiable(&action, &[&worker]);
    assert_eq!(
        reason.to_string(),
        "no worker can satisfy: 'gpu_count' requested 1, no worker declares this property"
    );
    Ok(())
}

fn shape(gpu_count: u64) -> PropertyShape {
    PropertyShape::from(&minimum_properties(&[("gpu_count", gpu_count)]))
}

fn at(secs: u64) -> SystemTime {
    UNIX_EPOCH.checked_add(Duration::from_secs(secs)).unwrap()
}

#[nativelink_test]
async fn property_shape_ignores_what_does_not_restrict_matching() -> Result<(), Error> {
    let with = |os_family: &str, ignored: &str| {
        PropertyShape::from(&PlatformProperties::new(HashMap::from([
            ("gpu_count".to_string(), PlatformPropertyValue::Minimum(1)),
            (
                "OSFamily".to_string(),
                PlatformPropertyValue::Priority(os_family.to_string()),
            ),
            (
                "note".to_string(),
                PlatformPropertyValue::Ignore(ignored.to_string()),
            ),
        ])))
    };
    assert_eq!(with("Linux_t4", "a"), with("Linux_h100", "b"));
    assert_ne!(shape(1), shape(2));
    Ok(())
}

#[nativelink_test]
async fn tracker_comes_due_after_the_timeout() -> Result<(), Error> {
    let mut tracker = UnsatisfiableTracker::new(Some(Duration::from_secs(TIMEOUT_S)));

    let observation = tracker.observe(shape(1), at(0), 1, at(0));
    assert!(!observation.is_due);
    assert!(observation.should_warn);
    assert_eq!(tracker.end_pass(at(0), 1), 1);
    assert_eq!(
        tracker.next_deadline(at(10)),
        Some(Duration::from_secs(TIMEOUT_S - 10))
    );

    let observation = tracker.observe(shape(1), at(TIMEOUT_S), 1, at(0));
    assert!(observation.is_due);
    assert!(observation.should_warn);
    assert_eq!(observation.waited, Duration::from_secs(TIMEOUT_S));
    Ok(())
}

#[nativelink_test]
async fn tracker_warns_once_a_minute_per_shape() -> Result<(), Error> {
    let mut tracker = UnsatisfiableTracker::new(None);
    assert!(tracker.observe(shape(1), at(0), 1, at(0)).should_warn);
    assert!(!tracker.observe(shape(1), at(1), 1, at(0)).should_warn);
    assert!(tracker.observe(shape(2), at(1), 1, at(0)).should_warn);
    assert!(tracker.observe(shape(1), at(60), 1, at(0)).should_warn);
    Ok(())
}

#[nativelink_test]
async fn tracker_only_counts_time_a_shape_was_queued() -> Result<(), Error> {
    let mut tracker = UnsatisfiableTracker::new(Some(Duration::from_secs(TIMEOUT_S)));
    // Queued for 30 seconds, then the build is cancelled.
    for secs in [0, 10, 20, 30] {
        assert!(!tracker.observe(shape(1), at(secs), 1, at(0)).is_due);
        tracker.end_pass(at(secs), 1);
    }
    // Not seen, but within the timeout and the fleet is unchanged.
    tracker.end_pass(at(40), 1);
    assert_eq!(tracker.next_deadline(at(40)), None);

    // A new build queues the same shape: the 30 seconds carry over, the idle
    // gap does not.
    let observation = tracker.observe(shape(1), at(100), 1, at(0));
    assert!(!observation.is_due);
    assert_eq!(observation.waited, Duration::from_secs(30));
    tracker.end_pass(at(100), 1);
    assert!(!tracker.observe(shape(1), at(129), 1, at(0)).is_due);
    tracker.end_pass(at(129), 1);
    assert!(tracker.observe(shape(1), at(130), 1, at(0)).is_due);
    tracker.end_pass(at(130), 1);

    // Unseen for a whole timeout, so the shape is forgotten.
    tracker.end_pass(at(130 + TIMEOUT_S), 1);
    let observation = tracker.observe(shape(1), at(130 + TIMEOUT_S), 1, at(0));
    assert!(!observation.is_due);
    assert_eq!(observation.waited, Duration::ZERO);
    Ok(())
}

#[nativelink_test]
async fn tracker_backs_off_while_a_due_shape_stays_queued() -> Result<(), Error> {
    let mut tracker = UnsatisfiableTracker::new(Some(Duration::from_secs(TIMEOUT_S)));
    tracker.observe(shape(1), at(0), 1, at(0));
    tracker.end_pass(at(0), 1);
    assert_eq!(
        tracker.next_deadline(at(0)),
        Some(Duration::from_secs(TIMEOUT_S))
    );

    for want in [2, 4, 8, 16, 32, 32] {
        assert!(tracker.observe(shape(1), at(TIMEOUT_S), 1, at(0)).is_due);
        tracker.end_pass(at(TIMEOUT_S), 1);
        assert_eq!(
            tracker.next_deadline(at(TIMEOUT_S)),
            Some(Duration::from_secs(want))
        );
    }
    Ok(())
}

#[nativelink_test]
async fn tracker_wakes_for_a_young_action_without_backing_off() -> Result<(), Error> {
    let mut tracker = UnsatisfiableTracker::new(Some(Duration::from_secs(TIMEOUT_S)));
    // An old action brings the shape due and is failed.
    tracker.observe(shape(1), at(0), 1, at(0));
    tracker.end_pass(at(0), 1);
    let observation = tracker.observe(shape(1), at(TIMEOUT_S), 1, at(0));
    assert!(observation.is_due && observation.action_eligible);
    tracker.note_failed(&shape(1));
    // Every eligible action was retired, so nothing is stuck: no backoff, and
    // nothing to wake for.
    assert_eq!(tracker.end_pass(at(TIMEOUT_S), 1), 1);
    assert_eq!(tracker.next_deadline(at(TIMEOUT_S)), None);

    // A young action of the due shape is not eligible yet. The loop wakes
    // when it will be, and repeated passes do not back that off.
    for pass_at in [70, 80, 90] {
        let observation = tracker.observe(shape(1), at(pass_at), 1, at(70));
        assert!(observation.is_due && !observation.action_eligible);
        tracker.end_pass(at(pass_at), 1);
        assert_eq!(
            tracker.next_deadline(at(pass_at)),
            Some(Duration::from_secs(70 + TIMEOUT_S - pass_at))
        );
    }

    // Once eligible but still not failed, the shape is stuck and backs off.
    let observation = tracker.observe(shape(1), at(130), 1, at(70));
    assert!(observation.action_eligible);
    tracker.end_pass(at(130), 1);
    assert_eq!(tracker.next_deadline(at(130)), Some(Duration::from_secs(2)));
    Ok(())
}

#[nativelink_test]
async fn tracker_drops_an_unseen_shape_when_the_fleet_changes() -> Result<(), Error> {
    let mut tracker = UnsatisfiableTracker::new(Some(Duration::from_secs(TIMEOUT_S)));
    tracker.observe(shape(1), at(0), 1, at(0));
    tracker.end_pass(at(0), 1);

    tracker.end_pass(at(1), 2);
    assert!(!tracker.observe(shape(1), at(TIMEOUT_S), 2, at(0)).is_due);
    Ok(())
}

#[nativelink_test]
async fn tracker_without_a_timeout_is_never_due() -> Result<(), Error> {
    let mut tracker = UnsatisfiableTracker::new(None);
    tracker.observe(shape(1), at(0), 1, at(0));
    tracker.end_pass(at(0), 1);
    assert!(!tracker.observe(shape(1), at(1_000_000), 1, at(0)).is_due);
    assert_eq!(tracker.next_deadline(at(1_000_000)), None);
    Ok(())
}
