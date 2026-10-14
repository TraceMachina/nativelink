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
    UpdateForWorker, WorkerLoad, update_for_worker,
};
use nativelink_scheduler::api_worker_scheduler::hold_owner;
use nativelink_scheduler::awaited_action_db::PeerScheduler;
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::{
    ActionResult, ActionStage, ActionUniqueQualifier, OperationId, WorkerId,
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
const HOLD_AFTER_S: u64 = 30;
const MAX_HOLD_S: u64 = 300;
/// Each worker advertises this much memory and runs up to four actions.
const WORKER_KB: u64 = 100_000;
const SMALL_KB: u64 = 10_000;
/// Fits a worker only when nothing else runs on it: beside one small
/// action there is room for it once that one finishes, beside two there
/// is not until both have.
const LARGE_KB: u64 = 95_000;

/// The client listeners of the queued actions, kept so that dropping one
/// does not drop its action.
type Listeners = Vec<Box<dyn ActionStateResult>>;

#[derive(Clone, Copy)]
struct Options {
    max_held_workers: usize,
    live_memory_veto: bool,
    client_action_timeout_s: u64,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_held_workers: 0,
            live_memory_veto: false,
            // The clock jumps past a hold here, and no client polls
            // meanwhile; neither the client timeout nor the backend's
            // retention window may retire a queued action over the jump.
            client_action_timeout_s: 100_000,
        }
    }
}

fn make_scheduler(options: Options) -> Arc<SimpleScheduler> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let spec = SimpleSpec {
        supported_platform_properties: Some(HashMap::from([(
            "memory_kb".to_string(),
            PropertyType::Minimum,
        )])),
        reservation_hold: Some(ReservationHoldSpec {
            hold_after_s: HOLD_AFTER_S,
            max_hold_s: MAX_HOLD_S,
            max_held_workers: options.max_held_workers,
        }),
        live_memory_veto: options.live_memory_veto.then(|| "memory_kb".to_string()),
        client_action_timeout_s: options.client_action_timeout_s,
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

fn worker_id(name: &str) -> WorkerId {
    WorkerId(name.to_string())
}

async fn add_worker_with(
    scheduler: &SimpleScheduler,
    name: &str,
    admits_when_idle: bool,
) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    let (tx, mut rx) = mpsc::channel(64);
    let mut worker = Worker::new(
        worker_id(name),
        PlatformProperties::new(HashMap::from([(
            "memory_kb".to_string(),
            PlatformPropertyValue::Minimum(WORKER_KB),
        )])),
        tx,
        NOW_TIME,
        /* max_inflight_tasks */ 4,
    );
    worker.admits_when_idle = admits_when_idle;
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

async fn add_worker(
    scheduler: &SimpleScheduler,
    name: &str,
) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    add_worker_with(scheduler, name, false).await
}

async fn add_action(
    scheduler: &SimpleScheduler,
    listeners: &mut Listeners,
    digest_byte: u8,
    kb: u64,
) -> Result<ActionUniqueQualifier, Error> {
    let mut action_info = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([digest_byte; 32], 512),
    );
    Arc::make_mut(&mut action_info).platform_properties =
        HashMap::from([("memory_kb".to_string(), kb.to_string())]);
    let unique_qualifier = action_info.unique_qualifier.clone();
    let listener = scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    listeners.push(listener);
    tokio::task::yield_now().await;
    Ok(unique_qualifier)
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

fn memory_of(dispatched: &[(String, u64)]) -> Vec<u64> {
    dispatched.iter().map(|(_, kb)| *kb).collect()
}

async fn complete(
    scheduler: &SimpleScheduler,
    worker: &str,
    operation_id: &str,
) -> Result<(), Error> {
    scheduler
        .update_action(
            &worker_id(worker),
            &OperationId::from(operation_id),
            UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(
                ActionResult::default(),
            )),
        )
        .await?;
    tokio::task::yield_now().await;
    Ok(())
}

/// The worker acknowledges the dispatch.
async fn acknowledge(
    scheduler: &SimpleScheduler,
    worker: &str,
    operation_id: &str,
) -> Result<(), Error> {
    scheduler
        .worker_dispatch_accepted(&worker_id(worker), &OperationId::from(operation_id))
        .await?;
    tokio::task::yield_now().await;
    Ok(())
}

/// A keepalive from the worker reporting this much memory free.
async fn report_free(scheduler: &SimpleScheduler, worker: &str, free_kb: u64) -> Result<(), Error> {
    scheduler
        .worker_keep_alive_received(
            &worker_id(worker),
            MockClock::time().as_secs(),
            Some(WorkerLoad {
                free_memory_kb: free_kb,
            }),
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

async fn held(scheduler: &SimpleScheduler) -> Vec<String> {
    let mut held: Vec<String> = scheduler
        .held_workers_for_test()
        .await
        .into_iter()
        .map(|id| id.0)
        .collect();
    held.sort();
    held
}

/// Two workers each running two small actions, and a large action that
/// fits neither until both of its small actions finish. Returns the small
/// actions' operation ids on a and on b, and the large action.
async fn two_busy_workers_and_a_large_action(
    scheduler: &SimpleScheduler,
    listeners: &mut Listeners,
    a: &mut mpsc::Receiver<UpdateForWorker>,
    b: &mut mpsc::Receiver<UpdateForWorker>,
) -> Result<(Vec<String>, Vec<String>, ActionUniqueQualifier), Error> {
    for digest in 1..5 {
        add_action(scheduler, listeners, digest, SMALL_KB).await?;
    }
    let (on_a, on_b) = (dispatches(a), dispatches(b));
    assert_eq!(
        (on_a.len(), on_b.len()),
        (2, 2),
        "two small actions per worker"
    );
    let large = add_action(scheduler, listeners, 9, LARGE_KB).await?;
    assert_eq!(
        (dispatches(a).len(), dispatches(b).len()),
        (0, 0),
        "the large action fits neither worker beside two small ones"
    );
    Ok((
        on_a.into_iter().map(|(id, _)| id).collect(),
        on_b.into_iter().map(|(id, _)| id).collect(),
        large,
    ))
}

/// Once a worker is held, two small actions both land on the other one.
/// Returns the names of the held and the free worker.
async fn held_and_free(
    scheduler: &SimpleScheduler,
    listeners: &mut Listeners,
    a: &mut mpsc::Receiver<UpdateForWorker>,
    b: &mut mpsc::Receiver<UpdateForWorker>,
) -> Result<(&'static str, &'static str), Error> {
    add_action(scheduler, listeners, 10, SMALL_KB).await?;
    add_action(scheduler, listeners, 11, SMALL_KB).await?;
    let (on_a, on_b) = (dispatches(a).len(), dispatches(b).len());
    let (held_worker, free) = match (on_a, on_b) {
        (0, 2) => ("a", "b"),
        (2, 0) => ("b", "a"),
        other => panic!("one worker should be held and take nothing, got {other:?}"),
    };
    assert_eq!(held(scheduler).await, vec![held_worker.to_string()]);
    Ok((held_worker, free))
}

/// A large action waits behind small ones on every worker. Once it has
/// waited `hold_after_s`, one worker is held: new small actions all go to
/// the other, and when the held worker's own actions finish the large one
/// runs there. The hold ends once the worker takes it, and the worker
/// takes small actions again.
#[nativelink_test]
async fn a_starving_reservation_gets_a_worker_held_then_runs_there() -> Result<(), Error> {
    let scheduler = make_scheduler(Options::default());
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    let (small_on_a, small_on_b, _) =
        two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;

    // Not long enough yet: nothing is held, a small action goes anywhere.
    pass_after(&scheduler, HOLD_AFTER_S - 1).await?;
    assert_eq!(held(&scheduler).await, Vec::<String>::new());
    add_action(&scheduler, &mut listeners, 12, SMALL_KB).await?;
    let (on_a, on_b) = (dispatches(&mut a), dispatches(&mut b));
    assert_eq!(on_a.len() + on_b.len(), 1, "nothing is held yet");
    // Put the fleet back to two small actions per worker.
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
    let (held_worker, _free) = held_and_free(&scheduler, &mut listeners, &mut a, &mut b).await?;
    let (held_rx, free_rx) = if held_worker == "a" {
        (&mut a, &mut b)
    } else {
        (&mut b, &mut a)
    };
    let small_on_held = if held_worker == "a" {
        small_on_a
    } else {
        small_on_b
    };

    // One of the held worker's actions finishes: not enough room yet, and
    // nothing else is offered the worker either.
    complete(&scheduler, held_worker, &small_on_held[0]).await?;
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    assert_eq!(dispatches(held_rx).len(), 0);
    assert_eq!(held(&scheduler).await, vec![held_worker.to_string()]);

    // The other finishes: the large action runs there. The hold lasts
    // until the worker says it took it.
    complete(&scheduler, held_worker, &small_on_held[1]).await?;
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    let on_held = dispatches(held_rx);
    assert_eq!(
        memory_of(&on_held),
        vec![LARGE_KB],
        "the held worker runs the action it was held for"
    );
    assert_eq!(dispatches(free_rx).len(), 0);
    assert_eq!(
        held(&scheduler).await,
        vec![held_worker.to_string()],
        "the hold outlives the dispatch"
    );
    acknowledge(&scheduler, held_worker, &on_held[0].0).await?;
    assert_eq!(held(&scheduler).await, Vec::<String>::new());

    // Once the large action is done the worker is idle and the free one
    // is full, so two small actions both run only if it takes them.
    complete(&scheduler, held_worker, &on_held[0].0).await?;
    add_action(&scheduler, &mut listeners, 13, SMALL_KB).await?;
    add_action(&scheduler, &mut listeners, 14, SMALL_KB).await?;
    assert_eq!(
        (dispatches(held_rx).len(), dispatches(free_rx).len()),
        (2, 0),
        "the worker is offered everything again once its action ran"
    );
    Ok(())
}

/// Small actions queued behind a full fleet are only waiting their turn:
/// one action finishing anywhere makes room for one of them. However long
/// they wait, no worker is held.
#[nativelink_test]
async fn an_action_one_completion_makes_room_for_gets_no_hold() -> Result<(), Error> {
    let scheduler = make_scheduler(Options::default());
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    for digest in 1..9 {
        add_action(&scheduler, &mut listeners, digest, SMALL_KB).await?;
    }
    assert_eq!((dispatches(&mut a).len(), dispatches(&mut b).len()), (4, 4));
    for digest in 9..12 {
        add_action(&scheduler, &mut listeners, digest, SMALL_KB).await?;
    }
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    assert_eq!(held(&scheduler).await, Vec::<String>::new());
    // Nor for a reservation that fits beside three small actions but not
    // beside four: one finishing makes room for it.
    add_action(&scheduler, &mut listeners, 12, WORKER_KB - 3 * SMALL_KB).await?;
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    assert_eq!(held(&scheduler).await, Vec::<String>::new());
    Ok(())
}

/// A hold that never fits is given up after `max_hold_s`, and the action is
/// not held for again straight away: the worker takes other work.
#[nativelink_test]
async fn a_hold_expires_and_the_worker_takes_other_work() -> Result<(), Error> {
    let scheduler = make_scheduler(Options::default());
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    let (held_worker, free) = held_and_free(&scheduler, &mut listeners, &mut a, &mut b).await?;
    let (held_rx, free_rx) = if held_worker == "a" {
        (&mut a, &mut b)
    } else {
        (&mut b, &mut a)
    };

    // The free worker is full now; a small action has nowhere to go, since
    // the held worker takes nothing else.
    add_action(&scheduler, &mut listeners, 12, SMALL_KB).await?;
    assert_eq!(
        (dispatches(held_rx).len(), dispatches(free_rx).len()),
        (0, 0)
    );

    // The hold runs out; the pass that notices wakes the matcher, and the
    // next pass places the small action on the worker, which is not held
    // for the large action again while its cooldown lasts.
    pass_after(&scheduler, MAX_HOLD_S).await?;
    assert_eq!(held(&scheduler).await, Vec::<String>::new());
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    assert_eq!(
        memory_of(&dispatches(held_rx)),
        vec![SMALL_KB],
        "the expired hold's worker takes the small action, not the large one"
    );
    assert_eq!(dispatches(free_rx).len(), 0, "{free} was full");

    // Cooldown over (as long as the hold, since the worker was busy when
    // it expired), and the large action still waits: a worker is held for
    // it again.
    pass_after(&scheduler, MAX_HOLD_S).await?;
    assert_eq!(held(&scheduler).await.len(), 1);
    Ok(())
}

/// With the live veto on, a held worker that drains but never reports room
/// for the action keeps the hold to its expiry while idle. Each such expiry
/// in a row doubles how long the action is left unheld.
#[nativelink_test]
async fn an_expiry_on_an_idle_worker_doubles_the_cooldown() -> Result<(), Error> {
    let scheduler = make_scheduler(Options {
        live_memory_veto: true,
        ..Options::default()
    });
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    let (small_on_a, small_on_b, _) =
        two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    let (held_worker, _free) = held_and_free(&scheduler, &mut listeners, &mut a, &mut b).await?;
    let held_rx = if held_worker == "a" { &mut a } else { &mut b };
    let small_on_held = if held_worker == "a" {
        small_on_a
    } else {
        small_on_b
    };

    // The held worker reports less free than the action asks and then
    // drains: the ledger would place it, the veto does not.
    report_free(&scheduler, held_worker, LARGE_KB / 2).await?;
    for operation_id in &small_on_held {
        complete(&scheduler, held_worker, operation_id).await?;
    }
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    assert_eq!(dispatches(held_rx).len(), 0, "vetoed, not placed");
    assert_eq!(held(&scheduler).await, vec![held_worker.to_string()]);

    // Expiry on an idle worker: the cooldown is twice the hold.
    pass_after(&scheduler, MAX_HOLD_S).await?;
    assert_eq!(held(&scheduler).await, Vec::<String>::new());
    pass_after(&scheduler, MAX_HOLD_S).await?;
    assert_eq!(
        held(&scheduler).await,
        Vec::<String>::new(),
        "still cooling down after one hold's worth"
    );
    pass_after(&scheduler, MAX_HOLD_S).await?;
    assert_eq!(held(&scheduler).await.len(), 1, "held again after two");
    Ok(())
}

/// The held worker declines the action for load once it is sent: the hold
/// stays, and when the worker reports room the action is offered it again.
#[nativelink_test]
async fn a_declined_dispatch_keeps_the_hold() -> Result<(), Error> {
    let scheduler = make_scheduler(Options::default());
    let mut listeners = Listeners::new();
    // Workers that admit when idle: a decline for load from one is about
    // what else it runs, not a verdict on the action.
    let mut a = add_worker_with(&scheduler, "a", true).await?;
    let mut b = add_worker_with(&scheduler, "b", true).await?;
    let (small_on_a, small_on_b, _) =
        two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    let (held_worker, _free) = held_and_free(&scheduler, &mut listeners, &mut a, &mut b).await?;
    let held_rx = if held_worker == "a" { &mut a } else { &mut b };
    let small_on_held = if held_worker == "a" {
        small_on_a
    } else {
        small_on_b
    };
    for operation_id in &small_on_held {
        complete(&scheduler, held_worker, operation_id).await?;
    }
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    let sent = dispatches(held_rx);
    assert_eq!(memory_of(&sent), vec![LARGE_KB]);

    scheduler
        .worker_dispatch_declined(
            &worker_id(held_worker),
            &OperationId::from(sent[0].0.as_str()),
            "load".to_string(),
            Some(LARGE_KB),
        )
        .await?;
    tokio::task::yield_now().await;
    assert_eq!(
        held(&scheduler).await,
        vec![held_worker.to_string()],
        "the hold outlives the decline"
    );
    // Paused until it reports room; meanwhile the matcher offers it
    // nothing, and a pass does not release the hold either.
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    assert_eq!(dispatches(held_rx).len(), 0);
    assert_eq!(held(&scheduler).await, vec![held_worker.to_string()]);

    report_free(&scheduler, held_worker, WORKER_KB).await?;
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    let sent = dispatches(held_rx);
    assert_eq!(memory_of(&sent), vec![LARGE_KB], "offered again");
    acknowledge(&scheduler, held_worker, &sent[0].0).await?;
    assert_eq!(held(&scheduler).await, Vec::<String>::new());
    Ok(())
}

/// A worker that declined that much while idle is not offered the action
/// again until it reports room, so it is never the one held; and a held
/// worker that declines the action idle loses the hold to another.
#[nativelink_test]
async fn a_worker_that_declined_idle_is_passed_over() -> Result<(), Error> {
    // A decline for load, and the verdict it leaves on an idle worker,
    // exist with the live veto on.
    let scheduler = make_scheduler(Options {
        live_memory_veto: true,
        ..Options::default()
    });
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    for digest in 1..5 {
        add_action(&scheduler, &mut listeners, digest, SMALL_KB).await?;
    }
    let (on_a, on_b) = (dispatches(&mut a), dispatches(&mut b));
    assert_eq!((on_a.len(), on_b.len()), (2, 2));

    // An idle worker joins; the large action goes to it at once, and it
    // declines it. Paused by the decline, it is offered nothing until its
    // next keepalive, and that much never again until it reports room.
    let mut c = add_worker(&scheduler, "c").await?;
    let large = add_action(&scheduler, &mut listeners, 9, LARGE_KB).await?;
    let sent = dispatches(&mut c);
    assert_eq!(memory_of(&sent), vec![LARGE_KB]);
    scheduler
        .worker_dispatch_declined(
            &worker_id("c"),
            &OperationId::from(sent[0].0.as_str()),
            "load".to_string(),
            Some(LARGE_KB),
        )
        .await?;
    tokio::task::yield_now().await;
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    assert_eq!(dispatches(&mut c).len(), 0, "not offered the action again");
    let held_now = held(&scheduler).await;
    assert_eq!(held_now.len(), 1);
    assert_ne!(
        held_now[0], "c",
        "the idle worker that declined is not held"
    );
    drop(large);

    // The held worker drains and declines the action idle: the hold moves.
    let held_worker = held_now[0].clone();
    let held_rx = if held_worker == "a" { &mut a } else { &mut b };
    let other = if held_worker == "a" { "b" } else { "a" };
    let running = if held_worker == "a" { on_a } else { on_b };
    for (operation_id, _) in &running {
        complete(&scheduler, &held_worker, operation_id).await?;
    }
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    let sent = dispatches(held_rx);
    assert_eq!(memory_of(&sent), vec![LARGE_KB]);
    scheduler
        .worker_dispatch_declined(
            &worker_id(&held_worker),
            &OperationId::from(sent[0].0.as_str()),
            "load".to_string(),
            Some(LARGE_KB),
        )
        .await?;
    tokio::task::yield_now().await;
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    assert_eq!(
        held(&scheduler).await,
        vec![other.to_string()],
        "the hold moved to the other busy worker"
    );
    Ok(())
}

/// Of two equally busy workers, the one with more memory free as last
/// reported is held.
#[nativelink_test]
async fn the_worker_with_more_memory_free_is_held() -> Result<(), Error> {
    let scheduler = make_scheduler(Options::default());
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;
    report_free(&scheduler, "a", 30_000).await?;
    report_free(&scheduler, "b", 60_000).await?;
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    assert_eq!(held(&scheduler).await, vec!["b".to_string()]);
    Ok(())
}

/// A hold ends when its worker drains or leaves, and the next pass holds
/// another.
#[nativelink_test]
async fn a_hold_ends_when_its_worker_drains_or_leaves() -> Result<(), Error> {
    let scheduler = make_scheduler(Options::default());
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    let held_now = held(&scheduler).await;
    assert_eq!(held_now.len(), 1);
    let (held_worker, other) = if held_now[0] == "a" {
        ("a", "b")
    } else {
        ("b", "a")
    };

    scheduler
        .set_drain_worker(&worker_id(held_worker), true)
        .await?;
    assert_eq!(held(&scheduler).await, Vec::<String>::new());
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    assert_eq!(held(&scheduler).await, vec![other.to_string()]);

    scheduler.remove_worker(&worker_id(other)).await?;
    assert_eq!(held(&scheduler).await, Vec::<String>::new());
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    assert_eq!(
        held(&scheduler).await,
        Vec::<String>::new(),
        "a draining worker is not held"
    );
    Ok(())
}

/// A hold ends when its action leaves the queue: here the client gives up
/// on it and the client timeout retires it.
#[nativelink_test]
async fn a_hold_ends_when_its_action_leaves_the_queue() -> Result<(), Error> {
    let scheduler = make_scheduler(Options {
        client_action_timeout_s: 100,
        ..Options::default()
    });
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;
    // The large action's client goes away.
    drop(listeners.pop());
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    assert_eq!(held(&scheduler).await.len(), 1);
    pass_after(&scheduler, 100).await?;
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    assert_eq!(
        held(&scheduler).await,
        Vec::<String>::new(),
        "released once no pass meets the action"
    );
    Ok(())
}

/// The large action is placed elsewhere first: an idle worker joins and
/// takes it at once; the hold ends once that worker takes it, and the held
/// worker takes small actions again.
#[nativelink_test]
async fn a_hold_ends_when_its_action_runs_elsewhere() -> Result<(), Error> {
    let scheduler = make_scheduler(Options::default());
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    let (held_worker, _free) = held_and_free(&scheduler, &mut listeners, &mut a, &mut b).await?;
    let (held_rx, free_rx) = if held_worker == "a" {
        (&mut a, &mut b)
    } else {
        (&mut b, &mut a)
    };

    let mut c = add_worker(&scheduler, "c").await?;
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    let on_c = dispatches(&mut c);
    assert_eq!(
        memory_of(&on_c),
        vec![LARGE_KB],
        "an idle worker that fits the action takes it at once"
    );
    assert_eq!(dispatches(held_rx).len(), 0);
    assert_eq!(
        held(&scheduler).await,
        vec![held_worker.to_string()],
        "held until the new worker takes it"
    );
    acknowledge(&scheduler, "c", &on_c[0].0).await?;
    assert_eq!(held(&scheduler).await, Vec::<String>::new());

    // The new worker has no memory left beside the large action, the free
    // worker is full, and the formerly held worker has two slots: two
    // small actions run only if it takes them.
    add_action(&scheduler, &mut listeners, 20, SMALL_KB).await?;
    add_action(&scheduler, &mut listeners, 21, SMALL_KB).await?;
    assert_eq!(
        (
            dispatches(held_rx).len(),
            dispatches(free_rx).len(),
            dispatches(&mut c).len()
        ),
        (2, 0, 0),
        "the formerly held worker takes small actions again"
    );
    Ok(())
}

/// At most the cap is held: one worker with the cap at one, and one on a
/// two-worker fleet with the cap at a tenth of the fleet.
#[nativelink_test]
async fn at_most_the_cap_is_held() -> Result<(), Error> {
    for max_held_workers in [1, 0] {
        let scheduler = make_scheduler(Options {
            max_held_workers,
            ..Options::default()
        });
        let mut listeners = Listeners::new();
        let mut a = add_worker(&scheduler, "a").await?;
        let mut b = add_worker(&scheduler, "b").await?;
        two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;
        add_action(&scheduler, &mut listeners, 10, LARGE_KB).await?;
        pass_after(&scheduler, HOLD_AFTER_S).await?;
        assert_eq!(
            held(&scheduler).await.len(),
            1,
            "cap {max_held_workers}: one worker held, the other still takes work"
        );
        add_action(&scheduler, &mut listeners, 11, SMALL_KB).await?;
        add_action(&scheduler, &mut listeners, 12, SMALL_KB).await?;
        assert_eq!(dispatches(&mut a).len() + dispatches(&mut b).len(), 2);
    }
    Ok(())
}

/// Scheduler replicas sharing a queue take turns holding, by a hash of the
/// action over the replicas whose workers would fit it: a peer whose
/// workers would not fit it is never the owner, and when the peer would,
/// this scheduler holds only for the actions that fall to it.
#[nativelink_test]
async fn replicas_take_turns_holding() -> Result<(), Error> {
    let scheduler = make_scheduler(Options::default());
    let mut listeners = Listeners::new();
    let mut a = add_worker(&scheduler, "a").await?;
    let mut b = add_worker(&scheduler, "b").await?;
    let shape = |memory_kb: u64| {
        PlatformProperties::new(HashMap::from([(
            "memory_kb".to_string(),
            PlatformPropertyValue::Minimum(memory_kb),
        )]))
    };
    // A peer with workers too small for the large action: this scheduler
    // holds for it whatever the hash says.
    scheduler
        .set_peer_schedulers_for_test(vec![PeerScheduler {
            scheduler_id: "peer".to_string(),
            workers: vec![shape(LARGE_KB - 1)],
        }])
        .await;
    let (_, _, large) =
        two_busy_workers_and_a_large_action(&scheduler, &mut listeners, &mut a, &mut b).await?;
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    assert_eq!(held(&scheduler).await.len(), 1);

    // A peer whose workers would fit it: whichever of the two the hash
    // names holds, and the other does not.
    let local = scheduler.scheduler_id_for_test().to_string();
    let mut owners = vec![local.as_str(), "peer"];
    owners.sort_unstable();
    let local_owns = hold_owner(&large, &owners) == local;
    scheduler
        .set_peer_schedulers_for_test(vec![PeerScheduler {
            scheduler_id: "peer".to_string(),
            workers: vec![shape(WORKER_KB)],
        }])
        .await;
    // The existing hold stands; a fresh one goes by the owner. Expire it
    // and let the cooldown pass, then see whether it comes back.
    pass_after(&scheduler, MAX_HOLD_S).await?;
    assert_eq!(held(&scheduler).await, Vec::<String>::new());
    pass_after(&scheduler, MAX_HOLD_S).await?;
    assert_eq!(
        held(&scheduler).await.len(),
        usize::from(local_owns),
        "held here only when the hash names this scheduler (owner: {})",
        hold_owner(&large, &owners)
    );
    // Another action, which the hash sends to the other owner, shows the
    // turn-taking rather than a fixed verdict.
    let other = loop {
        let digest = u8::try_from(30 + listeners.len()).unwrap_or(250);
        let action = add_action(&scheduler, &mut listeners, digest, LARGE_KB).await?;
        if (hold_owner(&action, &owners) == local) != local_owns {
            break action;
        }
    };
    pass_after(&scheduler, HOLD_AFTER_S).await?;
    assert_eq!(
        held(&scheduler).await.len(),
        usize::from(!local_owns),
        "the other action falls to the other owner ({})",
        hold_owner(&other, &owners)
    );
    Ok(())
}
