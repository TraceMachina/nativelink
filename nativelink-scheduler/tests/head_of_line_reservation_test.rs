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

//! Large-action starvation and the head-of-line hold that ends it.
//! A fleet of twelve-core workers takes a stream of one-core actions; an
//! eight-core action at the head of the queue never sees eight free cores
//! at once, because every core that frees is taken by the next small
//! action before the pass reaches the big one again.
//!
//! A hold is made at the end of a pass for the first action still waiting,
//! once it has waited `after_s` since it was last queued and the pass placed
//! something queued after it: so every test that wants a hold frees a core
//! and queues a small action that takes it. Each queued action is a second
//! later than the one before, since two actions queued in the same instant
//! tie on age and the listing breaks the tie by operation id.

use core::time::Duration;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use mock_instant::thread_local::MockClock;
use nativelink_config::schedulers::{HeadOfLineReservationSpec, PropertyType, SimpleSpec};
use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    UpdateForWorker, WorkerLoad, update_for_worker,
};
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::match_outcome::MatchOutcome;
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

const NOW_TIME: u64 = 10_000;
const CORES: u64 = 12;
const BIG: u64 = 8;
/// A queued action whose client has not been heard from for this long is
/// retired, so a test keeps the clock short of it, or crosses it on
/// purpose to stand in for a client that went away.
const CLIENT_GONE_AFTER_S: u64 = 60;

/// What a fleet budgets besides `cpu_count`.
#[derive(Clone, Copy)]
enum Fleet {
    Cpu,
    /// `memory_kb` too, and the scheduler vetoes on the free memory workers
    /// report.
    CpuAndMemory,
    /// A `zone` each worker is in, matched exactly when an action names one.
    CpuAndZone,
}

fn make_scheduler(
    reservation: Option<HeadOfLineReservationSpec>,
) -> (Arc<SimpleScheduler>, Arc<dyn WorkerScheduler>) {
    make_scheduler_with(reservation, Fleet::Cpu)
}

fn make_scheduler_with(
    reservation: Option<HeadOfLineReservationSpec>,
    fleet: Fleet,
) -> (Arc<SimpleScheduler>, Arc<dyn WorkerScheduler>) {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let task_change_notify = Arc::new(Notify::new());
    let mut supported = HashMap::from([("cpu_count".to_string(), PropertyType::Minimum)]);
    match fleet {
        Fleet::Cpu => {}
        Fleet::CpuAndMemory => {
            supported.insert("memory_kb".to_string(), PropertyType::Minimum);
        }
        Fleet::CpuAndZone => {
            supported.insert("zone".to_string(), PropertyType::Exact);
        }
    }
    SimpleScheduler::new_with_callback(
        &SimpleSpec {
            supported_platform_properties: Some(supported),
            head_of_line_reservation: reservation,
            live_memory_veto: matches!(fleet, Fleet::CpuAndMemory).then(|| "memory_kb".to_string()),
            ..SimpleSpec::default()
        },
        memory_awaited_action_db_factory(0, &task_change_notify, MockInstantWrapped::default),
        || async move {},
        task_change_notify,
        MockInstantWrapped::default,
        None,
    )
}

/// Lets the matching task and the state manager's event loop run.
async fn settle() {
    for _ in 0..8 {
        tokio::task::yield_now().await;
    }
}

/// A worker advertising `cores`, with no concurrency cap: the ledger alone
/// decides what fits.
async fn add_worker(
    scheduler: &SimpleScheduler,
    name: &str,
    cores: u64,
) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    add_worker_with(scheduler, name, cores, HashMap::new()).await
}

/// With more properties, as the fleet budgets them.
async fn add_worker_with(
    scheduler: &SimpleScheduler,
    name: &str,
    cores: u64,
    mut properties: HashMap<String, PlatformPropertyValue>,
) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    let (tx, mut rx) = mpsc::channel(64);
    properties.insert(
        "cpu_count".to_string(),
        PlatformPropertyValue::Minimum(cores),
    );
    let worker = Worker::new(
        WorkerId(name.to_string()),
        PlatformProperties::new(properties),
        tx,
        NOW_TIME,
        /* max_inflight_tasks */ 0,
    );
    scheduler
        .add_worker(worker)
        .await
        .err_tip(|| "Failed to add worker")?;
    settle().await;
    let first = rx.try_recv().expect("worker got no connection result");
    assert!(matches!(
        first.update,
        Some(update_for_worker::Update::ConnectionResult(_))
    ));
    Ok(rx)
}

fn memory(kb: u64) -> HashMap<String, PlatformPropertyValue> {
    HashMap::from([("memory_kb".to_string(), PlatformPropertyValue::Minimum(kb))])
}

fn zone(name: &str) -> HashMap<String, PlatformPropertyValue> {
    HashMap::from([(
        "zone".to_string(),
        PlatformPropertyValue::Exact(name.to_string()),
    )])
}

/// A unique action asking for `cores`, queued a second from now.
async fn add_action(
    scheduler: &SimpleScheduler,
    serial: u32,
    cores: u64,
    priority: i32,
) -> Result<Box<dyn ActionStateResult>, Error> {
    add_action_with(scheduler, serial, cores, HashMap::new(), priority).await
}

/// With more properties, as strings, the way a client sends them.
async fn add_action_with(
    scheduler: &SimpleScheduler,
    serial: u32,
    cores: u64,
    mut platform_properties: HashMap<String, String>,
    priority: i32,
) -> Result<Box<dyn ActionStateResult>, Error> {
    MockClock::advance(Duration::from_secs(1));
    let byte = u8::try_from(serial % 251).expect("a residue mod 251 is a byte");
    let digest = DigestInfo::new([byte; 32], 512 + u64::from(serial));
    let base = make_base_action_info(UNIX_EPOCH + MockClock::time(), digest);
    platform_properties.insert("cpu_count".to_string(), cores.to_string());
    let action_info = Arc::new(ActionInfo {
        platform_properties,
        priority,
        ..(*base).clone()
    });
    let listener = scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    settle().await;
    Ok(listener)
}

fn with_memory(kb: u64) -> HashMap<String, String> {
    HashMap::from([("memory_kb".to_string(), kb.to_string())])
}

/// The operations the worker was told to start since last asked.
fn started(rx: &mut mpsc::Receiver<UpdateForWorker>) -> Vec<OperationId> {
    let mut ops = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let Some(update_for_worker::Update::StartAction(start)) = msg.update {
            ops.push(OperationId::from(start.operation_id));
        }
    }
    ops
}

/// The worker reports the operation done, which frees its cores.
async fn finish(
    worker_scheduler: &dyn WorkerScheduler,
    worker: &str,
    operation_id: &OperationId,
) -> Result<(), Error> {
    worker_scheduler
        .update_action(
            &WorkerId(worker.to_string()),
            operation_id,
            UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(
                ActionResult::default(),
            )),
        )
        .await?;
    settle().await;
    Ok(())
}

/// The worker reports its free memory on a keepalive at `at` seconds.
async fn report(
    scheduler: &SimpleScheduler,
    worker: &str,
    free_memory_kb: u64,
    at: u64,
) -> Result<(), Error> {
    scheduler
        .worker_keep_alive_received(
            &WorkerId(worker.to_string()),
            NOW_TIME + at,
            Some(WorkerLoad { free_memory_kb }),
        )
        .await
}

async fn stage(listener: &dyn ActionStateResult) -> ActionStage {
    listener
        .as_state()
        .await
        .expect("state readable")
        .0
        .stage
        .clone()
}

/// `(worker, operation)` of the hold the admin snapshot shows, if any.
async fn reservation(worker_scheduler: &dyn WorkerScheduler) -> Option<(String, String)> {
    let reserved: Vec<_> = worker_scheduler
        .worker_snapshot()
        .await
        .into_iter()
        .filter_map(|worker| {
            worker
                .reserved_for_operation
                .map(|operation| (worker.id, operation))
        })
        .collect();
    assert!(reserved.len() <= 1, "one hold at a time: {reserved:?}");
    reserved.into_iter().next()
}

/// Every worker's running one-core operations, after filling the fleet.
type Running = HashMap<String, Vec<OperationId>>;

/// Fills every worker with one-core actions carrying `extra`.
async fn fill_with(
    scheduler: &SimpleScheduler,
    workers: &mut [(&str, &mut mpsc::Receiver<UpdateForWorker>)],
    serial: &mut u32,
    extra: HashMap<String, String>,
) -> Result<Running, Error> {
    let total = CORES * workers.len() as u64;
    for _ in 0..total {
        *serial += 1;
        add_action_with(scheduler, *serial, 1, extra.clone(), 0).await?;
    }
    let mut running = HashMap::new();
    for (name, rx) in workers.iter_mut() {
        let ops = started(rx);
        assert_eq!(ops.len() as u64, CORES, "worker {name} is full");
        running.insert((*name).to_string(), ops);
    }
    Ok(running)
}

async fn fill(
    scheduler: &SimpleScheduler,
    workers: &mut [(&str, &mut mpsc::Receiver<UpdateForWorker>)],
    serial: &mut u32,
) -> Result<Running, Error> {
    fill_with(scheduler, workers, serial, HashMap::new()).await
}

/// The starvation step: a core frees on `worker` and a new one-core action
/// (with `extra`) takes it, overtaking whatever waits. Returns the worker
/// the small action landed on.
async fn overtake_with(
    scheduler: &SimpleScheduler,
    worker_scheduler: &dyn WorkerScheduler,
    running: &mut Running,
    rxs: &mut [(&str, &mut mpsc::Receiver<UpdateForWorker>)],
    worker: &str,
    serial: &mut u32,
    extra: HashMap<String, String>,
) -> Result<String, Error> {
    let done = running.get_mut(worker).unwrap().remove(0);
    finish(worker_scheduler, worker, &done).await?;
    *serial += 1;
    add_action_with(scheduler, *serial, 1, extra, 0).await?;
    let mut landed = None;
    for (name, rx) in rxs.iter_mut() {
        let ops = started(rx);
        if !ops.is_empty() {
            assert_eq!(ops.len(), 1, "one small action lands");
            assert!(landed.is_none(), "it lands once");
            running.get_mut(*name).unwrap().extend(ops);
            landed = Some((*name).to_string());
        }
    }
    Ok(landed.expect("the small action landed somewhere"))
}

async fn overtake(
    scheduler: &SimpleScheduler,
    worker_scheduler: &dyn WorkerScheduler,
    running: &mut Running,
    rxs: &mut [(&str, &mut mpsc::Receiver<UpdateForWorker>)],
    worker: &str,
    serial: &mut u32,
) -> Result<String, Error> {
    overtake_with(
        scheduler,
        worker_scheduler,
        running,
        rxs,
        worker,
        serial,
        HashMap::new(),
    )
    .await
}

fn other(worker: &str) -> &'static str {
    if worker == "a" { "b" } else { "a" }
}

/// The defect: with the option off, an eight-core action waits forever on
/// a twelve-core worker whose cores free one at a time, because each
/// freed core goes to the next one-core action before eight are free.
#[nativelink_test]
async fn without_the_option_a_stream_of_small_actions_starves_a_large_one() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler(None);
    let mut w = add_worker(&scheduler, "w", CORES).await?;
    let mut serial = 0;
    let mut running = fill(&scheduler, &mut [("w", &mut w)], &mut serial).await?;

    let big = add_action(&scheduler, 1000, BIG, 0).await?;
    assert_eq!(stage(big.as_ref()).await, ActionStage::Queued);

    // Twenty rounds of a core freeing and a new small action taking it.
    for _ in 0..20 {
        MockClock::advance(Duration::from_secs(1));
        overtake(
            &scheduler,
            worker_scheduler.as_ref(),
            &mut running,
            &mut [("w", &mut w)],
            "w",
            &mut serial,
        )
        .await?;
        assert_eq!(stage(big.as_ref()).await, ActionStage::Queued);
    }
    assert_eq!(reservation(worker_scheduler.as_ref()).await, None);
    Ok(())
}

/// With the option on, once the big action has waited `after_s` and a
/// small action overtakes it, a worker is held for it: that worker takes
/// no more small actions, the other worker keeps taking them, and the big
/// action runs once the held worker has drained enough. The hold then
/// lifts and the worker's cores take small actions again.
#[nativelink_test]
async fn after_the_wait_a_worker_is_held_and_drains_to_the_large_action() -> Result<(), Error> {
    let (scheduler, worker_scheduler) =
        make_scheduler(Some(HeadOfLineReservationSpec { after_s: 20 }));
    let mut a = add_worker(&scheduler, "a", CORES).await?;
    let mut b = add_worker(&scheduler, "b", CORES).await?;
    let mut serial = 0;
    let mut running = fill(&scheduler, &mut [("a", &mut a), ("b", &mut b)], &mut serial).await?;
    let small_ops: HashSet<OperationId> = running.values().flatten().cloned().collect();

    let big = add_action(&scheduler, 1000, BIG, 0).await?;
    assert_eq!(stage(big.as_ref()).await, ActionStage::Queued);

    // Ten seconds in: cores free on both workers and the small actions
    // keep taking them; it has not waited long enough, so nothing is held.
    MockClock::advance(Duration::from_secs(10));
    for worker in ["a", "b"] {
        overtake(
            &scheduler,
            worker_scheduler.as_ref(),
            &mut running,
            &mut [("a", &mut a), ("b", &mut b)],
            worker,
            &mut serial,
        )
        .await?;
    }
    assert_eq!(reservation(worker_scheduler.as_ref()).await, None);
    assert_eq!(stage(big.as_ref()).await, ActionStage::Queued);

    // Twenty seconds in, a small action overtakes it again: the pass that
    // placed it finds the big action has waited `after_s` and holds a
    // worker for it.
    MockClock::advance(Duration::from_secs(10));
    overtake(
        &scheduler,
        worker_scheduler.as_ref(),
        &mut running,
        &mut [("a", &mut a), ("b", &mut b)],
        "a",
        &mut serial,
    )
    .await?;
    let (held, reserved_op) = reservation(worker_scheduler.as_ref())
        .await
        .expect("a worker is held after the wait");
    assert!(
        !small_ops.contains(&OperationId::from(reserved_op.as_str())),
        "the hold is for the big action, not a small one"
    );
    let other = other(&held);
    let (mut held_rx, mut other_rx) = if held == "a" { (a, b) } else { (b, a) };

    // A core frees on the held worker; a new small action does not take
    // it, and the other worker is full, so it waits.
    let done = running.get_mut(&held).unwrap().remove(0);
    finish(worker_scheduler.as_ref(), &held, &done).await?;
    serial += 1;
    add_action(&scheduler, serial, 1, 0).await?;
    assert_eq!(
        started(&mut held_rx),
        vec![],
        "the held worker takes nothing else"
    );
    assert_eq!(started(&mut other_rx), vec![]);

    // A core freeing on the other worker goes to that small action: the
    // rest of the fleet keeps backfilling.
    let done = running.get_mut(other).unwrap().remove(0);
    finish(worker_scheduler.as_ref(), other, &done).await?;
    assert_eq!(
        started(&mut other_rx).len(),
        1,
        "the unheld worker backfills"
    );
    assert_eq!(started(&mut held_rx), vec![]);

    // The held worker drains: six more cores free and the big action still
    // does not fit (seven free); nothing else lands there either.
    for _ in 0..6 {
        let done = running.get_mut(&held).unwrap().remove(0);
        finish(worker_scheduler.as_ref(), &held, &done).await?;
        assert_eq!(started(&mut held_rx), vec![]);
        assert_eq!(stage(big.as_ref()).await, ActionStage::Queued);
    }
    // The eighth free core places the big action there and lifts the hold.
    let done = running.get_mut(&held).unwrap().remove(0);
    finish(worker_scheduler.as_ref(), &held, &done).await?;
    let placed = started(&mut held_rx);
    assert_eq!(placed.len(), 1);
    assert_eq!(placed[0].to_string(), reserved_op);
    assert_eq!(stage(big.as_ref()).await, ActionStage::Executing);
    assert_eq!(reservation(worker_scheduler.as_ref()).await, None);

    // The worker is full again (four small actions and the big one). With
    // the hold off, the next core it frees goes to a small action as before.
    let done = running.get_mut(&held).unwrap().remove(0);
    finish(worker_scheduler.as_ref(), &held, &done).await?;
    serial += 1;
    add_action(&scheduler, serial, 1, 0).await?;
    assert_eq!(started(&mut held_rx).len(), 1, "the worker backfills again");
    Ok(())
}

/// The hold is released when the action leaves the queue unplaced: here
/// its only client goes away and the state manager drops it. The worker
/// is open to everything again on the next pass.
#[nativelink_test]
async fn the_hold_is_released_when_the_action_leaves_the_queue() -> Result<(), Error> {
    let (scheduler, worker_scheduler) =
        make_scheduler(Some(HeadOfLineReservationSpec { after_s: 0 }));
    let mut w = add_worker(&scheduler, "w", CORES).await?;
    let mut serial = 0;
    let mut running = fill(&scheduler, &mut [("w", &mut w)], &mut serial).await?;

    // Queued, then overtaken once: with `after_s: 0` that is enough.
    let big = add_action(&scheduler, 1000, BIG, 0).await?;
    assert_eq!(reservation(worker_scheduler.as_ref()).await, None);
    overtake(
        &scheduler,
        worker_scheduler.as_ref(),
        &mut running,
        &mut [("w", &mut w)],
        "w",
        &mut serial,
    )
    .await?;
    let (held, _) = reservation(worker_scheduler.as_ref())
        .await
        .expect("held once overtaken");
    assert_eq!(held, "w");

    // A freed core is not offered to a small action while the hold is on.
    let done = running.get_mut("w").unwrap().remove(0);
    finish(worker_scheduler.as_ref(), "w", &done).await?;
    serial += 1;
    add_action(&scheduler, serial, 1, 0).await?;
    assert_eq!(started(&mut w), vec![]);

    // The client is not heard from again and the state manager retires
    // the action: it leaves the queue, and the hold goes with it on the
    // next pass. (The submission below is what makes the state manager
    // look at the clock.)
    MockClock::advance(Duration::from_secs(CLIENT_GONE_AFTER_S + 1));
    serial += 1;
    add_action(&scheduler, serial, 1, 0).await?;
    scheduler.do_try_match_for_test().await?;
    settle().await;
    assert_eq!(reservation(worker_scheduler.as_ref()).await, None);
    assert_eq!(
        started(&mut w).len(),
        1,
        "a waiting small action takes the free core once the hold is off"
    );
    drop(big);
    Ok(())
}

/// A held worker that disconnects takes its running actions to the queue
/// and the hold moves to another worker for the same action at once. The
/// requeued small actions were queued before the big one and may use the
/// held worker as it frees, in queue order; the hold stays with the big
/// action throughout and it lands once the worker has drained past them.
#[nativelink_test]
async fn a_worker_disconnect_moves_the_hold() -> Result<(), Error> {
    let (scheduler, worker_scheduler) =
        make_scheduler(Some(HeadOfLineReservationSpec { after_s: 0 }));
    let mut a = add_worker(&scheduler, "a", CORES).await?;
    let mut b = add_worker(&scheduler, "b", CORES).await?;
    let mut serial = 0;
    let mut running = fill(&scheduler, &mut [("a", &mut a), ("b", &mut b)], &mut serial).await?;

    let big = add_action(&scheduler, 1000, BIG, 0).await?;
    overtake(
        &scheduler,
        worker_scheduler.as_ref(),
        &mut running,
        &mut [("a", &mut a), ("b", &mut b)],
        "a",
        &mut serial,
    )
    .await?;
    let (lost, reserved_op) = reservation(worker_scheduler.as_ref())
        .await
        .expect("held once overtaken");
    let survivor = other(&lost);

    scheduler
        .worker_disconnected(&WorkerId(lost.clone()))
        .await?;
    settle().await;
    assert_eq!(
        reservation(worker_scheduler.as_ref()).await,
        Some((survivor.to_string(), reserved_op.clone())),
        "the hold moved to the surviving worker for the same action"
    );
    assert_eq!(stage(big.as_ref()).await, ActionStage::Queued);

    // The lost worker's twelve small actions are queued again, ahead of
    // the big one. As the survivor's cores free they take them, one each,
    // in queue order; the hold is unmoved by any of it.
    let mut survivor_rx = if survivor == "a" { a } else { b };
    let on_survivor = running.get_mut(survivor).unwrap();
    for _ in 0..CORES {
        let done = on_survivor.remove(0);
        finish(worker_scheduler.as_ref(), survivor, &done).await?;
        let placed = started(&mut survivor_rx);
        assert_eq!(
            placed.len(),
            1,
            "a requeued small action, which sorts first"
        );
        on_survivor.extend(placed);
        assert_eq!(
            reservation(worker_scheduler.as_ref()).await,
            Some((survivor.to_string(), reserved_op.clone()))
        );
    }
    assert_eq!(stage(big.as_ref()).await, ActionStage::Queued);

    // Nothing is queued ahead of the big action any more: the survivor
    // drains to it.
    for _ in 0..(BIG - 1) {
        let done = on_survivor.remove(0);
        finish(worker_scheduler.as_ref(), survivor, &done).await?;
        assert_eq!(started(&mut survivor_rx), vec![]);
    }
    let done = on_survivor.remove(0);
    finish(worker_scheduler.as_ref(), survivor, &done).await?;
    assert_eq!(
        started(&mut survivor_rx).first().map(ToString::to_string),
        Some(reserved_op)
    );
    assert_eq!(stage(big.as_ref()).await, ActionStage::Executing);
    assert_eq!(reservation(worker_scheduler.as_ref()).await, None);
    Ok(())
}

/// Draining the held worker moves the hold at once: a draining worker is
/// leaving, so waiting for it to free up would wait forever while it sat
/// idle. The other worker is held instead.
#[nativelink_test]
async fn a_drain_of_the_held_worker_moves_the_hold() -> Result<(), Error> {
    let (scheduler, worker_scheduler) =
        make_scheduler(Some(HeadOfLineReservationSpec { after_s: 0 }));
    let mut a = add_worker(&scheduler, "a", CORES).await?;
    let mut b = add_worker(&scheduler, "b", CORES).await?;
    let mut serial = 0;
    let mut running = fill(&scheduler, &mut [("a", &mut a), ("b", &mut b)], &mut serial).await?;

    let big = add_action(&scheduler, 1000, BIG, 0).await?;
    overtake(
        &scheduler,
        worker_scheduler.as_ref(),
        &mut running,
        &mut [("a", &mut a), ("b", &mut b)],
        "a",
        &mut serial,
    )
    .await?;
    let (held, reserved_op) = reservation(worker_scheduler.as_ref())
        .await
        .expect("held once overtaken");

    scheduler
        .set_drain_worker(&WorkerId(held.clone()), true)
        .await?;
    settle().await;
    let survivor = other(&held);
    assert_eq!(
        reservation(worker_scheduler.as_ref()).await,
        Some((survivor.to_string(), reserved_op.clone())),
        "the hold left the draining worker for the other one"
    );
    assert_eq!(stage(big.as_ref()).await, ActionStage::Queued);

    // The drained worker's cores free one by one and nothing lands on it
    // (it is draining), while the survivor drains to the big action.
    let (mut held_rx, mut survivor_rx) = if held == "a" { (a, b) } else { (b, a) };
    for done in &running[&held] {
        finish(worker_scheduler.as_ref(), &held, done).await?;
        assert_eq!(started(&mut held_rx), vec![]);
    }
    for done in running[survivor]
        .iter()
        .take(usize::try_from(BIG).expect("a small count"))
    {
        finish(worker_scheduler.as_ref(), survivor, done).await?;
    }
    let placed = started(&mut survivor_rx);
    assert_eq!(placed.first().map(ToString::to_string), Some(reserved_op));
    assert_eq!(stage(big.as_ref()).await, ActionStage::Executing);
    Ok(())
}

/// The held worker drains, takes the action, and declines it for load
/// while holding nothing else. It is now idle, paused and on record as
/// having refused this much, so it is never held for the action again:
/// the next hold goes to the other worker.
#[nativelink_test]
async fn a_worker_that_declined_the_action_while_idle_is_not_held_for_it_again() -> Result<(), Error>
{
    let (scheduler, worker_scheduler) = make_scheduler_with(
        Some(HeadOfLineReservationSpec { after_s: 0 }),
        Fleet::CpuAndMemory,
    );
    let mut a = add_worker_with(&scheduler, "a", CORES, memory(100_000)).await?;
    let mut b = add_worker_with(&scheduler, "b", CORES, memory(100_000)).await?;
    let mut serial = 0;
    let mut running = fill_with(
        &scheduler,
        &mut [("a", &mut a), ("b", &mut b)],
        &mut serial,
        with_memory(1_000),
    )
    .await?;

    let big = add_action_with(&scheduler, 1000, BIG, with_memory(8_000), 0).await?;
    overtake_with(
        &scheduler,
        worker_scheduler.as_ref(),
        &mut running,
        &mut [("a", &mut a), ("b", &mut b)],
        "a",
        &mut serial,
        with_memory(1_000),
    )
    .await?;
    let (held, reserved_op) = reservation(worker_scheduler.as_ref())
        .await
        .expect("held once overtaken");
    let other = other(&held);
    let (mut held_rx, mut other_rx) = if held == "a" { (a, b) } else { (b, a) };

    // The held worker drains completely and takes the big action.
    for done in running.remove(&held).unwrap() {
        finish(worker_scheduler.as_ref(), &held, &done).await?;
    }
    assert_eq!(
        started(&mut held_rx).first().map(ToString::to_string),
        Some(reserved_op.clone())
    );
    assert_eq!(reservation(worker_scheduler.as_ref()).await, None);

    // It declines for load with nothing else running: the action is queued
    // again and the worker is idle, paused and marked as having declined
    // 8 000 KiB while idle.
    scheduler
        .worker_dispatch_declined(
            &WorkerId(held.clone()),
            &OperationId::from(reserved_op.as_str()),
            "load".to_string(),
            Some(8_000),
        )
        .await?;
    settle().await;
    assert_eq!(stage(big.as_ref()).await, ActionStage::Queued);

    // Overtaken again, by a small action landing on the other worker: the
    // hold goes to that busy worker, not back to the one that refused.
    overtake_with(
        &scheduler,
        worker_scheduler.as_ref(),
        &mut running,
        &mut [(other, &mut other_rx)],
        other,
        &mut serial,
        with_memory(1_000),
    )
    .await?;
    assert_eq!(
        reservation(worker_scheduler.as_ref()).await,
        Some((other.to_string(), reserved_op.clone()))
    );
    assert_eq!(
        started(&mut held_rx),
        vec![],
        "the action is not offered to it again"
    );

    // A keepalive lifts the pause; the record of the decline, and the hold
    // on the other worker, stay.
    scheduler
        .worker_keep_alive_received(&WorkerId(held.clone()), NOW_TIME + 100, None)
        .await?;
    settle().await;
    assert_eq!(
        reservation(worker_scheduler.as_ref()).await,
        Some((other.to_string(), reserved_op))
    );
    assert_eq!(started(&mut held_rx), vec![]);
    Ok(())
}

/// A requeue can raise an action's properties (a memory escalation does)
/// past what the held worker registered. The hold is checked against the
/// action's current properties every time the pass meets it, so a worker
/// that can no longer ever fit the action is released rather than waited
/// on. Driven through the worker scheduler directly, since the memory
/// state manager has no second scheduler to run the escalation on.
#[nativelink_test]
async fn a_requeue_that_outgrows_the_held_worker_releases_the_hold() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with(
        Some(HeadOfLineReservationSpec { after_s: 0 }),
        Fleet::CpuAndMemory,
    );
    let mut a = add_worker_with(&scheduler, "a", CORES, memory(100_000)).await?;
    let mut serial = 0;
    let mut running = fill_with(
        &scheduler,
        &mut [("a", &mut a)],
        &mut serial,
        with_memory(1_000),
    )
    .await?;

    add_action_with(&scheduler, 1000, BIG, with_memory(8_000), 0).await?;
    overtake_with(
        &scheduler,
        worker_scheduler.as_ref(),
        &mut running,
        &mut [("a", &mut a)],
        "a",
        &mut serial,
        with_memory(1_000),
    )
    .await?;
    let (held, reserved_op) = reservation(worker_scheduler.as_ref())
        .await
        .expect("held once overtaken");
    assert_eq!(held, "a");

    // The same action, now asking for more memory than `a` registered.
    let outgrown = PlatformProperties::new(HashMap::from([
        ("cpu_count".to_string(), PlatformPropertyValue::Minimum(BIG)),
        (
            "memory_kb".to_string(),
            PlatformPropertyValue::Minimum(200_000),
        ),
    ]));
    let (outcome, _) = scheduler
        .worker_scheduler_for_test()
        .find_worker_for_action_observed(
            &outgrown,
            Some(&OperationId::from(reserved_op.as_str())),
            false,
            false,
            UNIX_EPOCH + MockClock::time(),
        )
        .await;
    assert!(
        !matches!(outcome, MatchOutcome::Matched(_)),
        "nothing fits it: {outcome:?}"
    );
    assert_eq!(
        reservation(worker_scheduler.as_ref()).await,
        None,
        "a hold on a worker the action outgrew is released"
    );
    Ok(())
}

/// An action that sorts before the held one (a higher priority here) may
/// use the held worker: it takes the worker's room first, in queue order,
/// and the hold stays with the action it was made for.
#[nativelink_test]
async fn an_action_ahead_of_the_hold_takes_the_held_workers_room_first() -> Result<(), Error> {
    let (scheduler, worker_scheduler) =
        make_scheduler(Some(HeadOfLineReservationSpec { after_s: 20 }));
    let mut a = add_worker(&scheduler, "a", CORES).await?;
    let mut b = add_worker(&scheduler, "b", CORES).await?;
    let mut serial = 0;
    let mut running = fill(&scheduler, &mut [("a", &mut a), ("b", &mut b)], &mut serial).await?;

    let low = add_action(&scheduler, 1000, BIG, 0).await?;
    MockClock::advance(Duration::from_secs(20));
    overtake(
        &scheduler,
        worker_scheduler.as_ref(),
        &mut running,
        &mut [("a", &mut a), ("b", &mut b)],
        "a",
        &mut serial,
    )
    .await?;
    let (held, low_op) = reservation(worker_scheduler.as_ref())
        .await
        .expect("the low-priority action is held for after its wait");
    let mut held_rx = if held == "a" { a } else { b };

    // A higher-priority action of the same size arrives: it is the head of
    // the queue now, but the held worker could run it, so the drain in
    // progress serves it first and the hold is left where it was.
    let high = add_action(&scheduler, 1001, BIG, 1).await?;
    assert_eq!(
        reservation(worker_scheduler.as_ref()).await,
        Some((held.clone(), low_op.clone()))
    );
    let on_held = running.get_mut(&held).unwrap();
    for _ in 0..(BIG - 1) {
        let done = on_held.remove(0);
        finish(worker_scheduler.as_ref(), &held, &done).await?;
        assert_eq!(started(&mut held_rx), vec![]);
    }
    let done = on_held.remove(0);
    finish(worker_scheduler.as_ref(), &held, &done).await?;
    assert_eq!(started(&mut held_rx).len(), 1);
    assert_eq!(stage(high.as_ref()).await, ActionStage::Executing);
    assert_eq!(stage(low.as_ref()).await, ActionStage::Queued);
    assert_eq!(
        reservation(worker_scheduler.as_ref()).await,
        Some((held, low_op)),
        "the hold is still the low-priority action's"
    );
    Ok(())
}

/// An action ahead of the held one that the held worker could never run
/// takes the hold instead, once it has waited and been overtaken itself:
/// the drain in progress is no use to it.
#[nativelink_test]
async fn an_action_ahead_that_the_held_worker_cannot_run_takes_the_hold() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with(
        Some(HeadOfLineReservationSpec { after_s: 0 }),
        Fleet::CpuAndZone,
    );
    let mut a = add_worker_with(&scheduler, "a", CORES, zone("east")).await?;
    let mut b = add_worker_with(&scheduler, "b", CORES, zone("west")).await?;
    let mut serial = 0;
    let mut running = fill(&scheduler, &mut [("a", &mut a), ("b", &mut b)], &mut serial).await?;

    add_action(&scheduler, 1000, BIG, 0).await?;
    overtake(
        &scheduler,
        worker_scheduler.as_ref(),
        &mut running,
        &mut [("a", &mut a), ("b", &mut b)],
        "a",
        &mut serial,
    )
    .await?;
    let (held, low_op) = reservation(worker_scheduler.as_ref())
        .await
        .expect("held once overtaken");
    let other = other(&held);
    let other_zone = if other == "a" { "east" } else { "west" };

    // A higher-priority action that must run in the other worker's zone,
    // overtaken by a small action landing there: the hold moves to the
    // other worker, for it.
    add_action_with(
        &scheduler,
        1001,
        BIG,
        HashMap::from([("zone".to_string(), other_zone.to_string())]),
        1,
    )
    .await?;
    overtake(
        &scheduler,
        worker_scheduler.as_ref(),
        &mut running,
        &mut [("a", &mut a), ("b", &mut b)],
        other,
        &mut serial,
    )
    .await?;
    let (now_held, held_op) = reservation(worker_scheduler.as_ref())
        .await
        .expect("still one hold");
    assert_eq!(now_held, other);
    assert_ne!(held_op, low_op, "the hold is the higher-priority action's");
    Ok(())
}

/// Two workers report, busy, less free memory than the action asks, but
/// what their running actions borrowed would, given back, cover it, so a
/// busy one is held and drained; drained and still refusing, it is
/// released, and what it reported free idle is remembered as the most a
/// drain can reach. The other is held and drained to the same end. Refilled
/// past the shortfall, so the lent-back projection alone would clear the
/// bar again, neither is held again, with no new report or with a busy
/// report of the same figure; an idle report with room lets the action land
/// with no hold at all.
#[nativelink_test]
async fn workers_that_refuse_the_action_idle_are_not_drained_in_turn() -> Result<(), Error> {
    let (scheduler, worker_scheduler) = make_scheduler_with(
        Some(HeadOfLineReservationSpec { after_s: 0 }),
        Fleet::CpuAndMemory,
    );
    let mut a = add_worker_with(&scheduler, "a", CORES, memory(100_000)).await?;
    let mut b = add_worker_with(&scheduler, "b", CORES, memory(100_000)).await?;
    let mut serial = 0;
    let mut running = fill_with(
        &scheduler,
        &mut [("a", &mut a), ("b", &mut b)],
        &mut serial,
        with_memory(1_000),
    )
    .await?;
    // Busy reports: the 12 000 KiB their running actions borrowed would
    // bring either to 62 000 once given back.
    report(&scheduler, "a", 50_000, 1).await?;
    report(&scheduler, "b", 50_000, 1).await?;

    let big = add_action_with(&scheduler, 1000, BIG, with_memory(60_000), 0).await?;
    overtake_with(
        &scheduler,
        worker_scheduler.as_ref(),
        &mut running,
        &mut [("a", &mut a), ("b", &mut b)],
        "a",
        &mut serial,
        with_memory(1_000),
    )
    .await?;
    let (first, reserved_op) = reservation(worker_scheduler.as_ref())
        .await
        .expect("a busy worker's borrowed memory would cover it once given back");
    let second = other(&first);

    // The first held worker drains and, idle, still reports 50 000 KiB:
    // the hold is dropped.
    for done in core::mem::take(running.get_mut(&first).unwrap()) {
        finish(worker_scheduler.as_ref(), &first, &done).await?;
    }
    assert_eq!(stage(big.as_ref()).await, ActionStage::Queued);
    assert_eq!(reservation(worker_scheduler.as_ref()).await, None);

    // Overtaken again: the other, still busy, worker is held and drained
    // the same way, to the same end.
    overtake_with(
        &scheduler,
        worker_scheduler.as_ref(),
        &mut running,
        &mut [("a", &mut a), ("b", &mut b)],
        second,
        &mut serial,
        with_memory(1_000),
    )
    .await?;
    assert_eq!(
        reservation(worker_scheduler.as_ref())
            .await
            .map(|(worker, _)| worker),
        Some(second.to_string())
    );
    for done in core::mem::take(running.get_mut(second).unwrap()) {
        finish(worker_scheduler.as_ref(), second, &done).await?;
    }
    assert_eq!(reservation(worker_scheduler.as_ref()).await, None);

    // Both refilled to twelve one-core actions: borrowed memory alone
    // would again project 62 000 KiB for either, but 50 000 reported idle
    // caps it. Overtaken with no new report, and then with a busy report
    // of the same figure, nothing is held.
    let need = 2 * CORES - running.values().map(Vec::len).sum::<usize>() as u64;
    for _ in 0..need {
        serial += 1;
        add_action_with(&scheduler, serial, 1, with_memory(1_000), 0).await?;
    }
    for (name, rx) in [("a", &mut a), ("b", &mut b)] {
        let on_worker = running.get_mut(name).unwrap();
        on_worker.extend(started(rx));
        assert_eq!(on_worker.len() as u64, CORES, "worker {name} is full again");
    }
    assert_eq!(reservation(worker_scheduler.as_ref()).await, None);
    for (round, worker) in ["a", "b", "a", "b"].into_iter().enumerate() {
        overtake_with(
            &scheduler,
            worker_scheduler.as_ref(),
            &mut running,
            &mut [("a", &mut a), ("b", &mut b)],
            worker,
            &mut serial,
            with_memory(1_000),
        )
        .await?;
        assert_eq!(
            reservation(worker_scheduler.as_ref()).await,
            None,
            "no hold on the strength of borrowed memory, round {round}"
        );
        if round == 1 {
            report(&scheduler, "a", 50_000, 100).await?;
            report(&scheduler, "b", 50_000, 100).await?;
            settle().await;
        }
    }
    assert_eq!(stage(big.as_ref()).await, ActionStage::Queued);

    // A worker drains and reports room for it idle: it lands there, no
    // hold needed.
    for done in core::mem::take(running.get_mut("a").unwrap()) {
        finish(worker_scheduler.as_ref(), "a", &done).await?;
    }
    assert_eq!(reservation(worker_scheduler.as_ref()).await, None);
    report(&scheduler, "a", 70_000, 200).await?;
    settle().await;
    assert_eq!(stage(big.as_ref()).await, ActionStage::Executing);
    assert_eq!(
        started(&mut a).first().map(ToString::to_string),
        Some(reserved_op)
    );
    Ok(())
}

/// A batch of one-core actions queued again after their worker is lost
/// sorts ahead of a big action already waiting. As the surviving worker's
/// cores free they take them one at a time, in order; none of them is
/// overtaken, so none is ever held for, whatever `after_s`, and the big
/// action lands once they are through.
#[nativelink_test]
async fn a_batch_requeued_ahead_is_placed_in_order_without_a_hold() -> Result<(), Error> {
    let (scheduler, worker_scheduler) =
        make_scheduler(Some(HeadOfLineReservationSpec { after_s: 0 }));
    let mut a = add_worker(&scheduler, "a", CORES).await?;
    let mut b = add_worker(&scheduler, "b", CORES).await?;
    let mut serial = 0;
    let mut running = fill(&scheduler, &mut [("a", &mut a), ("b", &mut b)], &mut serial).await?;

    let big = add_action(&scheduler, 1000, BIG, 0).await?;
    scheduler
        .worker_disconnected(&WorkerId("a".to_string()))
        .await?;
    settle().await;
    assert_eq!(reservation(worker_scheduler.as_ref()).await, None);

    let on_b = running.get_mut("b").unwrap();
    for _ in 0..CORES {
        let done = on_b.remove(0);
        finish(worker_scheduler.as_ref(), "b", &done).await?;
        let placed = started(&mut b);
        assert_eq!(placed.len(), 1, "the next requeued small action");
        on_b.extend(placed);
        assert_eq!(reservation(worker_scheduler.as_ref()).await, None);
    }
    assert_eq!(stage(big.as_ref()).await, ActionStage::Queued);
    for _ in 0..BIG {
        let done = on_b.remove(0);
        finish(worker_scheduler.as_ref(), "b", &done).await?;
        assert_eq!(reservation(worker_scheduler.as_ref()).await, None);
    }
    assert_eq!(stage(big.as_ref()).await, ActionStage::Executing);
    drop(a);
    Ok(())
}
