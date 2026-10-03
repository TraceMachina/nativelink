// Copyright 2024 The NativeLink Authors. All rights reserved.
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

//! Dispatch is a proposal until the worker answers it: a decline sends the
//! action back untried and pauses the worker, a full channel does the same
//! without an eviction, and an acknowledgement that never comes is a
//! decline after the timeout.

use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use mock_instant::thread_local::MockClock;
use nativelink_config::schedulers::{PropertyType, SimpleSpec};
use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    StartExecute, UpdateForWorker, WorkerLoad, update_for_worker,
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

fn make_scheduler(
    max_job_retries: usize,
    dispatch_ack_timeout_s: u64,
) -> (Arc<SimpleScheduler>, Arc<dyn WorkerScheduler>) {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let task_change_notify = Arc::new(Notify::new());
    let spec = SimpleSpec {
        supported_platform_properties: Some(HashMap::from([(
            "memory_kb".to_string(),
            PropertyType::Minimum,
        )])),
        max_job_retries,
        dispatch_ack_timeout_s,
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

fn worker_id() -> WorkerId {
    WorkerId(WORKER.to_string())
}

fn worker_with_channel(capacity: usize) -> (Worker, mpsc::Receiver<UpdateForWorker>) {
    let (tx, rx) = mpsc::channel(capacity);
    let worker = Worker::new(
        worker_id(),
        PlatformProperties::new(HashMap::from([(
            "memory_kb".to_string(),
            PlatformPropertyValue::Minimum(5_000),
        )])),
        tx,
        NOW_TIME,
        /* max_inflight_tasks */ 4,
    );
    (worker, rx)
}

async fn add_worker(scheduler: &SimpleScheduler) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    let (worker, mut rx) = worker_with_channel(64);
    scheduler
        .add_worker(worker)
        .await
        .err_tip(|| "Failed to add worker")?;
    tokio::task::yield_now().await;
    let connected = rx.recv().await.unwrap();
    let Some(update_for_worker::Update::ConnectionResult(result)) = connected.update else {
        panic!("expected a ConnectionResult, got {connected:?}");
    };
    assert!(
        result.dispatch_ack,
        "the scheduler announces the acknowledgement"
    );
    assert!(
        result.memory_property.is_empty(),
        "no live memory veto, so the worker is told nothing to decline for load on"
    );
    Ok(rx)
}

async fn add_action(
    scheduler: &SimpleScheduler,
    seed: u8,
) -> Result<Box<dyn ActionStateResult>, Error> {
    let base = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([seed; 32], 512),
    );
    let action_info = Arc::new(ActionInfo {
        platform_properties: HashMap::from([("memory_kb".to_string(), "2000".to_string())]),
        ..(*base).clone()
    });
    let listener = scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    tokio::task::yield_now().await;
    Ok(listener)
}

async fn next_dispatch(rx: &mut mpsc::Receiver<UpdateForWorker>) -> StartExecute {
    let msg = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("a dispatch within two seconds")
        .expect("the channel is open");
    let Some(update_for_worker::Update::StartAction(start_execute)) = msg.update else {
        panic!("expected a StartAction, got {msg:?}");
    };
    start_execute
}

async fn no_dispatch(rx: &mut mpsc::Receiver<UpdateForWorker>) {
    let quiet = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
    assert!(
        quiet.is_err(),
        "expected no dispatch, got {:?}",
        quiet.expect("timeout branch handled above")
    );
}

async fn keepalive(
    worker_scheduler: &dyn WorkerScheduler,
    at: u64,
    free_memory_kb: Option<u64>,
) -> Result<(), Error> {
    worker_scheduler
        .worker_keep_alive_received(
            &worker_id(),
            at,
            free_memory_kb.map(|free_memory_kb| WorkerLoad { free_memory_kb }),
        )
        .await?;
    tokio::task::yield_now().await;
    Ok(())
}

async fn complete(worker_scheduler: &dyn WorkerScheduler, operation_id: &str) -> Result<(), Error> {
    worker_scheduler
        .update_action(
            &worker_id(),
            &OperationId::from(operation_id),
            UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(
                ActionResult::default(),
            )),
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

/// A decline sends the action back untried: with a retry cap of one, two
/// declines and a completion still succeed. The worker is paused by the
/// decline; a decline for load from a busy worker lifts only on a keepalive
/// whose free memory covers the need, any other on the next keepalive.
#[nativelink_test]
async fn a_decline_requeues_untried_and_pauses_until_the_load_fits() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler(1, 0);
    let mut rx = add_worker(&scheduler).await?;
    // Something already running, so the declines below come from a busy
    // worker: one that will have more room once it finishes.
    let _resident = add_action(&scheduler, 9).await?;
    next_dispatch(&mut rx).await;
    let mut listener = add_action(&scheduler, 1).await?;

    let first = next_dispatch(&mut rx).await;
    worker_scheduler
        .worker_dispatch_declined(
            &worker_id(),
            &OperationId::from(first.operation_id.as_str()),
            "load".to_string(),
            Some(2_000),
        )
        .await?;
    tokio::task::yield_now().await;
    no_dispatch(&mut rx).await;

    // Too little free memory: still paused.
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 1, Some(1_000)).await?;
    no_dispatch(&mut rx).await;

    // Enough: the same operation comes back.
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 2, Some(3_000)).await?;
    let second = next_dispatch(&mut rx).await;
    assert_eq!(second.operation_id, first.operation_id);

    // A decline for capacity lifts on any keepalive.
    worker_scheduler
        .worker_dispatch_declined(
            &worker_id(),
            &OperationId::from(second.operation_id.as_str()),
            "at_capacity".to_string(),
            None,
        )
        .await?;
    tokio::task::yield_now().await;
    no_dispatch(&mut rx).await;
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 3, None).await?;
    let third = next_dispatch(&mut rx).await;
    assert_eq!(third.operation_id, first.operation_id);

    complete(worker_scheduler.as_ref(), &third.operation_id).await?;
    let result = wait_for_completion(&mut listener).await;
    assert!(
        result.error.is_none(),
        "two declines did not count against a retry cap of one: {:?}",
        result.error
    );
    Ok(())
}

/// A worker that declines for load while holding nothing else will never
/// have more room, so the pause lifts on its next keepalive instead of
/// waiting for free memory it never reports. Such a worker predates idle
/// admission and would decline the same reservation again, so it is not
/// offered that much or more; smaller actions still reach it.
#[nativelink_test]
async fn a_decline_from_an_idle_worker_lifts_and_is_not_offered_again() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let task_change_notify = Arc::new(Notify::new());
    let spec = SimpleSpec {
        supported_platform_properties: Some(HashMap::from([(
            "memory_kb".to_string(),
            PropertyType::Minimum,
        )])),
        live_memory_veto: Some("memory_kb".to_string()),
        ..SimpleSpec::default()
    };
    let (scheduler, worker_scheduler) = SimpleScheduler::new_with_callback(
        &spec,
        memory_awaited_action_db_factory(0, &task_change_notify, MockInstantWrapped::default),
        || async move {},
        task_change_notify,
        MockInstantWrapped::default,
        None,
    );
    let (worker, mut rx) = worker_with_channel(64);
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

    let add = |seed: u8, memory_kb: &str| {
        let base = make_base_action_info(
            UNIX_EPOCH + MockClock::time(),
            DigestInfo::new([seed; 32], 512),
        );
        let action_info = Arc::new(ActionInfo {
            platform_properties: HashMap::from([("memory_kb".to_string(), memory_kb.to_string())]),
            ..(*base).clone()
        });
        let scheduler = scheduler.clone();
        async move {
            scheduler
                .add_action(OperationId::default(), action_info)
                .await?;
            tokio::task::yield_now().await;
            Ok::<_, Error>(())
        }
    };

    // The whole worker, as `worker_with_channel` advertises it.
    add(7, "5000").await?;

    let first = next_dispatch(&mut rx).await;
    worker_scheduler
        .worker_dispatch_declined(
            &worker_id(),
            &OperationId::from(first.operation_id.as_str()),
            "load".to_string(),
            Some(5_000),
        )
        .await?;
    tokio::task::yield_now().await;
    no_dispatch(&mut rx).await;

    // Less than the whole worker free, as always: the pause lifts all the
    // same, but the declined reservation is not offered to it again.
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 1, Some(4_000)).await?;
    no_dispatch(&mut rx).await;

    // A smaller action that fits what it reports free still reaches it.
    add(8, "3000").await?;
    let second = next_dispatch(&mut rx).await;
    assert_ne!(second.operation_id, first.operation_id);
    no_dispatch(&mut rx).await;

    // Once it finishes and the worker reports room for the declined size,
    // that size is offered to it again.
    complete(worker_scheduler.as_ref(), &second.operation_id).await?;
    no_dispatch(&mut rx).await;
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 2, Some(5_000)).await?;
    let third = next_dispatch(&mut rx).await;
    assert_eq!(third.operation_id, first.operation_id);
    Ok(())
}

/// A worker that said on connection it admits any action while idle only
/// declines for load while it holds other work, even if this scheduler has
/// taken that work back and thinks it idle. Its decline is treated as a
/// busy worker's: the pause waits for a keepalive with room, and the size
/// is not marked as one it declines while idle.
#[nativelink_test]
async fn a_decline_from_a_worker_that_admits_when_idle_waits_for_room() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler(1, 0);
    let (mut worker, mut rx) = worker_with_channel(64);
    worker.admits_when_idle = true;
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
    let _listener = add_action(&scheduler, 1).await?;

    // Nothing else is running as far as the scheduler knows.
    let first = next_dispatch(&mut rx).await;
    worker_scheduler
        .worker_dispatch_declined(
            &worker_id(),
            &OperationId::from(first.operation_id.as_str()),
            "load".to_string(),
            Some(2_000),
        )
        .await?;
    tokio::task::yield_now().await;
    no_dispatch(&mut rx).await;

    // Too little free memory: still paused, as for a busy worker.
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 1, Some(1_000)).await?;
    no_dispatch(&mut rx).await;

    // Enough: the same operation comes back.
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 2, Some(3_000)).await?;
    let second = next_dispatch(&mut rx).await;
    assert_eq!(second.operation_id, first.operation_id);
    Ok(())
}

/// A worker whose channel will not take the dispatch keeps its place: the
/// action goes back untried, the worker is paused, and once it reads and
/// sends a keepalive the same action comes to it.
#[nativelink_test]
async fn a_full_channel_requeues_and_pauses_instead_of_evicting() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler(1, 0);
    // Room for the ConnectionResult and nothing else.
    let (worker, mut rx) = worker_with_channel(1);
    scheduler.add_worker(worker).await?;
    tokio::task::yield_now().await;

    let mut listener = add_action(&scheduler, 2).await?;
    tokio::task::yield_now().await;

    // The worker was not evicted: draining it would fail otherwise.
    worker_scheduler
        .set_drain_worker(&worker_id(), false)
        .await
        .err_tip(|| "the worker is still in the pool")?;

    // The worker catches up: reads its connection result, sends a
    // keepalive, and the action arrives.
    let connected = rx.recv().await.unwrap();
    assert!(matches!(
        connected.update,
        Some(update_for_worker::Update::ConnectionResult(_))
    ));
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 1, None).await?;
    let dispatched = next_dispatch(&mut rx).await;
    complete(worker_scheduler.as_ref(), &dispatched.operation_id).await?;
    let result = wait_for_completion(&mut listener).await;
    assert!(result.error.is_none(), "{:?}", result.error);
    Ok(())
}

/// With the sweep on, a dispatch the worker never acknowledged goes back
/// untried after the timeout; an acknowledged one stays.
#[nativelink_test]
async fn an_unacknowledged_dispatch_is_requeued_after_the_timeout() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler(1, 5);
    let mut rx = add_worker(&scheduler).await?;
    let mut listener = add_action(&scheduler, 3).await?;

    let first = next_dispatch(&mut rx).await;
    // Within the timeout: nothing happens.
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 3, None).await?;
    no_dispatch(&mut rx).await;
    // Past it: requeued and the worker paused; the next keepalive brings
    // the same operation back.
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 6, None).await?;
    no_dispatch(&mut rx).await;
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 7, None).await?;
    let second = next_dispatch(&mut rx).await;
    assert_eq!(second.operation_id, first.operation_id);

    // Acknowledged this time: the sweep leaves it alone.
    worker_scheduler
        .worker_dispatch_accepted(
            &worker_id(),
            &OperationId::from(second.operation_id.as_str()),
        )
        .await?;
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 20, None).await?;
    no_dispatch(&mut rx).await;

    complete(worker_scheduler.as_ref(), &second.operation_id).await?;
    let result = wait_for_completion(&mut listener).await;
    assert!(result.error.is_none(), "{:?}", result.error);
    Ok(())
}

/// A decline for an operation the worker no longer holds, because it
/// finished or the sweep already took it back, is nothing to act on: the
/// worker keeps taking work.
#[nativelink_test]
async fn a_late_decline_does_not_pause_the_worker() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler(1, 0);
    let mut rx = add_worker(&scheduler).await?;
    let mut listener = add_action(&scheduler, 1).await?;
    let first = next_dispatch(&mut rx).await;
    complete(worker_scheduler.as_ref(), &first.operation_id).await?;
    let result = wait_for_completion(&mut listener).await;
    assert!(result.error.is_none(), "{:?}", result.error);

    // The decline arrives after the completion.
    worker_scheduler
        .worker_dispatch_declined(
            &worker_id(),
            &OperationId::from(first.operation_id.as_str()),
            "load".to_string(),
            Some(5_000),
        )
        .await?;

    let _listener = add_action(&scheduler, 2).await?;
    next_dispatch(&mut rx).await;
    Ok(())
}

async fn next_kill(rx: &mut mpsc::Receiver<UpdateForWorker>) -> String {
    let msg = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("a kill within two seconds")
        .expect("the channel is open");
    let Some(update_for_worker::Update::KillOperationRequest(kill)) = msg.update else {
        panic!("expected a KillOperationRequest, got {msg:?}");
    };
    kill.operation_id
}

/// The acknowledgement clock starts at the dispatch, not at the worker's
/// last message before it: a worker that was quiet for a while still gets
/// the whole timeout for a dispatch made later.
#[nativelink_test]
async fn the_acknowledgement_clock_starts_at_the_dispatch() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler(1, 5);
    let mut rx = add_worker(&scheduler).await?;
    // The worker's last message was at NOW_TIME; the dispatch is four
    // seconds later.
    MockClock::advance(Duration::from_secs(4));
    let _listener = add_action(&scheduler, 4).await?;
    let first = next_dispatch(&mut rx).await;

    // Four seconds after the dispatch, eight after the worker's last
    // message: still within the timeout, so the acknowledgement counts.
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 8, None).await?;
    worker_scheduler
        .worker_dispatch_accepted(
            &worker_id(),
            &OperationId::from(first.operation_id.as_str()),
        )
        .await?;
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 20, None).await?;
    no_dispatch(&mut rx).await;
    Ok(())
}

/// An acknowledgement for a dispatch the sweep already took back means the
/// worker is about to run an action that now belongs elsewhere: it is told
/// to stop at once, not at the next revoked-operation sweep.
#[nativelink_test]
async fn a_late_acknowledgement_is_answered_with_a_kill() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler(1, 5);
    let mut rx = add_worker(&scheduler).await?;
    let _listener = add_action(&scheduler, 5).await?;
    let first = next_dispatch(&mut rx).await;

    // Past the timeout with no acknowledgement: taken back, worker paused.
    keepalive(worker_scheduler.as_ref(), NOW_TIME + 6, None).await?;
    no_dispatch(&mut rx).await;

    worker_scheduler
        .worker_dispatch_accepted(
            &worker_id(),
            &OperationId::from(first.operation_id.as_str()),
        )
        .await?;
    assert_eq!(next_kill(&mut rx).await, first.operation_id);
    Ok(())
}
