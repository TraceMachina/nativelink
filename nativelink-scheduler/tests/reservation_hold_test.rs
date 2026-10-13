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
use nativelink_config::schedulers::{PropertyType, ReservationHoldSpec, SimpleSpec};
use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    UpdateForWorker, update_for_worker,
};
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::{ActionResult, ActionStage, OperationId, WorkerId};
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
const HOLD_AFTER_S: u64 = 30;
const MAX_HOLD_S: u64 = 300;
/// Each worker advertises this much memory and runs up to four actions.
const WORKER_KB: u64 = 100_000;
const SMALL_KB: u64 = 10_000;
/// Fits a worker only when nothing else runs on it.
const LARGE_KB: u64 = 95_000;

/// The client listeners of the queued actions, kept so that dropping one
/// does not drop its action.
type Listeners = Vec<Box<dyn ActionStateResult>>;

fn make_scheduler(max_held_workers: usize) -> Arc<SimpleScheduler> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let spec = SimpleSpec {
        supported_platform_properties: Some(HashMap::from([(
            "memory_kb".to_string(),
            PropertyType::Minimum,
        )])),
        reservation_hold: Some(ReservationHoldSpec {
            hold_after_s: HOLD_AFTER_S,
            max_hold_s: MAX_HOLD_S,
            max_held_workers,
        }),
        // The clock jumps past a hold here, and no client polls meanwhile;
        // neither the client timeout nor the backend's retention window
        // (below) may retire a queued action over the jump.
        client_action_timeout_s: 100_000,
        ..SimpleSpec::default()
    };
    let task_change_notify = Arc::new(Notify::new());
    let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
        &spec,
        memory_awaited_action_db_factory(100_000, &task_change_notify, MockInstantWrapped::default),
        || async move {},
        task_change_notify,
        MockInstantWrapped::default,
        None,
    );
    scheduler
}

async fn add_worker(
    scheduler: &SimpleScheduler,
    name: &str,
) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    let (tx, mut rx) = mpsc::channel(64);
    let worker = Worker::new(
        WorkerId(name.to_string()),
        PlatformProperties::new(HashMap::from([(
            "memory_kb".to_string(),
            PlatformPropertyValue::Minimum(WORKER_KB),
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
    let first = rx.try_recv().expect("worker got no connection result");
    assert!(matches!(
        first.update,
        Some(update_for_worker::Update::ConnectionResult(_))
    ));
    Ok(rx)
}

async fn add_action(
    scheduler: &SimpleScheduler,
    listeners: &mut Listeners,
    digest_byte: u8,
    kb: u64,
) -> Result<(), Error> {
    let mut action_info = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([digest_byte; 32], 512),
    );
    Arc::make_mut(&mut action_info).platform_properties =
        HashMap::from([("memory_kb".to_string(), kb.to_string())]);
    let listener = scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    listeners.push(listener);
    tokio::task::yield_now().await;
    Ok(())
}

/// The dispatches a worker received since last asked: operation id and the
/// memory each reserved.
fn dispatches(rx: &mut mpsc::Receiver<UpdateForWorker>) -> Vec<(String, u64)> {
    let mut seen = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let Some(update_for_worker::Update::StartAction(start)) = msg.update {
            let memory_kb = start
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
            seen.push((start.operation_id, memory_kb));
        }
    }
    seen
}

async fn complete(
    scheduler: &SimpleScheduler,
    worker: &str,
    operation_id: &str,
) -> Result<(), Error> {
    scheduler
        .update_action(
            &WorkerId(worker.to_string()),
            &OperationId::from(operation_id),
            UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(
                ActionResult::default(),
            )),
        )
        .await?;
    tokio::task::yield_now().await;
    Ok(())
}

/// Lets the clock run and runs a pass, as the matcher would.
async fn pass_after(scheduler: &SimpleScheduler, seconds: u64) -> Result<(), Error> {
    MockClock::advance(Duration::from_secs(seconds));
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    Ok(())
}

/// Two workers each running one small action, and a large action that fits
/// neither beside it. Returns the small actions' operation ids on a and b.
async fn two_busy_workers_and_a_large_action(
    scheduler: &SimpleScheduler,
    listeners: &mut Listeners,
    a: &mut mpsc::Receiver<UpdateForWorker>,
    b: &mut mpsc::Receiver<UpdateForWorker>,
) -> Result<(String, String), Error> {
    add_action(scheduler, listeners, 1, SMALL_KB).await?;
    add_action(scheduler, listeners, 2, SMALL_KB).await?;
    let (on_a, on_b) = (dispatches(a), dispatches(b));
    assert_eq!(
        (on_a.len(), on_b.len()),
        (1, 1),
        "one small action per worker"
    );
    add_action(scheduler, listeners, 3, LARGE_KB).await?;
    assert_eq!(
        (dispatches(a).len(), dispatches(b).len()),
        (0, 0),
        "the large action fits neither worker beside a small one"
    );
    Ok((on_a[0].0.clone(), on_b[0].0.clone()))
}

/// Once a worker is held, two small actions both land on the other one.
/// Returns the names of the held and the free worker.
async fn held_and_free(
    scheduler: &SimpleScheduler,
    listeners: &mut Listeners,
    a: &mut mpsc::Receiver<UpdateForWorker>,
    b: &mut mpsc::Receiver<UpdateForWorker>,
) -> Result<(&'static str, &'static str), Error> {
    add_action(scheduler, listeners, 4, SMALL_KB).await?;
    add_action(scheduler, listeners, 5, SMALL_KB).await?;
    let (on_a, on_b) = (dispatches(a).len(), dispatches(b).len());
    match (on_a, on_b) {
        (0, 2) => Ok(("a", "b")),
        (2, 0) => Ok(("b", "a")),
        other => panic!("one worker should be held and take nothing, got {other:?}"),
    }
}

/// A large action waits behind small ones on every worker. Once it has
/// waited `hold_after_s`, one worker is held: new small actions all go to
/// the other, and when the held worker's own action finishes the large one
/// runs there. The hold then ends and the worker takes small actions again.
#[nativelink_test]
async fn a_starving_reservation_gets_a_worker_held_then_runs_there() -> Result<(), Error> {
    let scheduler = make_scheduler(0);
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    let (small_on_a, small_on_b) =
        two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;

    // Not long enough yet: nothing is held, a small action goes anywhere.
    pass_after(&scheduler, HOLD_AFTER_S - 1).await?;
    add_action(&scheduler, &mut listeners, 9, SMALL_KB).await?;
    let (on_a, on_b) = (dispatches(&mut a), dispatches(&mut b));
    assert_eq!(on_a.len() + on_b.len(), 1, "nothing is held yet");
    // Put the fleet back to one small action per worker.
    let (third_worker, third) = if on_a.is_empty() {
        ("b", on_b[0].0.clone())
    } else {
        ("a", on_a[0].0.clone())
    };
    complete(&scheduler, third_worker, &third).await?;
    scheduler.do_try_match_for_test().await?;
    assert_eq!(
        (dispatches(&mut a).len(), dispatches(&mut b).len()),
        (0, 0),
        "the room that opened is still too small for the large action"
    );

    pass_after(&scheduler, 1).await?;
    let (held, _free) = held_and_free(&scheduler, &mut listeners, &mut a, &mut b).await?;
    let (held_rx, free_rx) = if held == "a" {
        (&mut a, &mut b)
    } else {
        (&mut b, &mut a)
    };
    let small_on_held = if held == "a" { small_on_a } else { small_on_b };

    // The held worker's own action finishes: the large action runs there.
    complete(&scheduler, held, &small_on_held).await?;
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    let on_held = dispatches(held_rx);
    assert_eq!(
        on_held.iter().map(|(_, kb)| *kb).collect::<Vec<_>>(),
        vec![LARGE_KB],
        "the held worker runs the action it was held for"
    );
    assert_eq!(dispatches(free_rx).len(), 0);

    // The hold is over. Once the large action is done, the worker is idle
    // and the free one has a slot left, so two small actions both run only
    // if the formerly held worker takes one.
    complete(&scheduler, held, &on_held[0].0).await?;
    add_action(&scheduler, &mut listeners, 6, SMALL_KB).await?;
    add_action(&scheduler, &mut listeners, 7, SMALL_KB).await?;
    assert_eq!(
        dispatches(held_rx).len() + dispatches(free_rx).len(),
        2,
        "the worker is offered everything again once its action ran"
    );
    Ok(())
}

/// A hold that never fits is given up after `max_hold_s`, and the action is
/// not held for again straight away: the worker takes other work.
#[nativelink_test]
async fn a_hold_expires_and_the_worker_takes_other_work() -> Result<(), Error> {
    let scheduler = make_scheduler(0);
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    let (held, free) = held_and_free(&scheduler, &mut listeners, &mut a, &mut b).await?;
    let (held_rx, free_rx) = if held == "a" {
        (&mut a, &mut b)
    } else {
        (&mut b, &mut a)
    };

    // Fill the free worker (three running) to its four slots.
    add_action(&scheduler, &mut listeners, 6, SMALL_KB).await?;
    assert_eq!(
        (dispatches(held_rx).len(), dispatches(free_rx).len()),
        (0, 1)
    );
    // Now a small action has nowhere to go: the free worker is full and
    // the held worker takes nothing else.
    add_action(&scheduler, &mut listeners, 7, SMALL_KB).await?;
    assert_eq!(
        (dispatches(held_rx).len(), dispatches(free_rx).len()),
        (0, 0)
    );

    // The hold runs out; the pass that notices wakes the matcher, and the
    // next pass places the small action on the worker, which is not held
    // for the large action again while its cooldown lasts.
    pass_after(&scheduler, MAX_HOLD_S).await?;
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    assert_eq!(
        dispatches(held_rx)
            .iter()
            .map(|(_, kb)| *kb)
            .collect::<Vec<_>>(),
        vec![SMALL_KB],
        "the expired hold's worker takes the small action, not the large one"
    );
    assert_eq!(dispatches(free_rx).len(), 0, "{free} was full");

    // Cooldown over, and the large action still waits: a worker is held
    // for it again, and a small action finds no room on it.
    pass_after(&scheduler, MAX_HOLD_S).await?;
    add_action(&scheduler, &mut listeners, 8, SMALL_KB).await?;
    assert_eq!(
        dispatches(held_rx).len() + dispatches(free_rx).len(),
        0,
        "one worker full, the other held again"
    );
    Ok(())
}

/// The large action finds room elsewhere first: an idle worker joins and
/// takes it at once, the hold ends as placed, and the held worker takes
/// small actions again.
#[nativelink_test]
async fn a_hold_ends_when_its_action_runs_elsewhere() -> Result<(), Error> {
    let scheduler = make_scheduler(0);
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    let (held, _free) = held_and_free(&scheduler, &mut listeners, &mut a, &mut b).await?;
    let (held_rx, free_rx) = if held == "a" {
        (&mut a, &mut b)
    } else {
        (&mut b, &mut a)
    };

    let mut c = add_worker(&scheduler, "c").await?;
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    assert_eq!(
        dispatches(&mut c)
            .iter()
            .map(|(_, kb)| *kb)
            .collect::<Vec<_>>(),
        vec![LARGE_KB],
        "an idle worker that fits the action takes it at once"
    );
    assert_eq!(dispatches(held_rx).len(), 0);

    // The hold ended as placed. The new worker has no memory left beside
    // the large action, the free worker one slot, and the formerly held
    // worker three: five small actions run four deep only if it takes
    // part again, and one deep if it were still held.
    for digest in 20..25 {
        add_action(&scheduler, &mut listeners, digest, SMALL_KB).await?;
    }
    let total = dispatches(held_rx).len() + dispatches(free_rx).len() + dispatches(&mut c).len();
    assert_eq!(
        total, 4,
        "the formerly held worker takes small actions again"
    );
    Ok(())
}

/// With the cap at one hold, a second starving action waits unheld.
#[nativelink_test]
async fn at_most_the_cap_is_held() -> Result<(), Error> {
    let scheduler = make_scheduler(1);
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;
    add_action(&scheduler, &mut listeners, 10, LARGE_KB).await?;
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    // One worker held, the other still takes small actions.
    add_action(&scheduler, &mut listeners, 4, SMALL_KB).await?;
    add_action(&scheduler, &mut listeners, 5, SMALL_KB).await?;
    assert_eq!(dispatches(&mut a).len() + dispatches(&mut b).len(), 2);
    Ok(())
}
