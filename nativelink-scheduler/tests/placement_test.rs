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
use nativelink_config::schedulers::{PropertyType, SimpleSpec, WorkerAllocationStrategy};
use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    UpdateForWorker, update_for_worker,
};
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::{ActionInfo, OperationId, WorkerId};
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::operation_state_manager::ClientStateManager;
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};
use pretty_assertions::assert_eq;
use tokio::sync::{Notify, mpsc};
use utils::scheduler_utils::make_base_action_info;

mod utils {
    pub(crate) mod scheduler_utils;
}

const NOW_TIME: u64 = 10000;

fn make_scheduler(strategy: WorkerAllocationStrategy) -> Arc<SimpleScheduler> {
    make_scheduler_with_spec(&SimpleSpec {
        allocation_strategy: strategy,
        ..SimpleSpec::default()
    })
}

fn make_scheduler_with_spec(spec: &SimpleSpec) -> Arc<SimpleScheduler> {
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

async fn add_worker(
    scheduler: &SimpleScheduler,
    name: &str,
) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    add_worker_with_properties(scheduler, name, HashMap::new()).await
}

async fn add_worker_with_properties(
    scheduler: &SimpleScheduler,
    name: &str,
    properties: HashMap<String, PlatformPropertyValue>,
) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    let (tx, mut rx) = mpsc::channel(64);
    let worker = Worker::new(
        WorkerId(name.to_string()),
        PlatformProperties::new(properties),
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
    assert!(
        matches!(
            connected.update,
            Some(update_for_worker::Update::ConnectionResult(_))
        ),
        "first message should be the connection result: {connected:?}"
    );
    Ok(rx)
}

async fn add_action(scheduler: &SimpleScheduler, digest_byte: u8) -> Result<(), Error> {
    add_action_with_properties(scheduler, digest_byte, HashMap::new()).await
}

async fn add_action_with_properties(
    scheduler: &SimpleScheduler,
    digest_byte: u8,
    platform_properties: HashMap<String, String>,
) -> Result<(), Error> {
    let base = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([digest_byte; 32], 512),
    );
    let action_info = Arc::new(ActionInfo {
        platform_properties,
        ..(*base).clone()
    });
    scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    tokio::task::yield_now().await;
    Ok(())
}

fn dispatched(rx: &mut mpsc::Receiver<UpdateForWorker>) -> usize {
    let mut count = 0;
    while let Ok(msg) = rx.try_recv() {
        if matches!(msg.update, Some(update_for_worker::Update::StartAction(_))) {
            count += 1;
        }
    }
    count
}

/// One worker takes the first actions; a second worker joins; the next action
/// goes to the idle newcomer under `least_loaded`, and to the busy veteran
/// under the LRU default, which only knows who was used least recently.
async fn run(strategy: WorkerAllocationStrategy) -> Result<(usize, usize), Error> {
    let scheduler = make_scheduler(strategy);
    let mut veteran = add_worker(&scheduler, "veteran").await?;
    add_action(&scheduler, 1).await?;
    add_action(&scheduler, 2).await?;
    assert_eq!(
        dispatched(&mut veteran),
        2,
        "the only worker takes everything"
    );

    let mut newcomer = add_worker(&scheduler, "newcomer").await?;
    add_action(&scheduler, 3).await?;
    Ok((dispatched(&mut veteran), dispatched(&mut newcomer)))
}

#[nativelink_test]
async fn least_loaded_gives_the_next_action_to_the_idle_newcomer() -> Result<(), Error> {
    assert_eq!(run(WorkerAllocationStrategy::LeastLoaded).await?, (0, 1));
    Ok(())
}

#[nativelink_test]
async fn least_recently_used_gives_it_to_the_busy_veteran() -> Result<(), Error> {
    assert_eq!(
        run(WorkerAllocationStrategy::LeastRecentlyUsed).await?,
        (1, 0)
    );
    Ok(())
}

fn memory_worker(kb: u64) -> HashMap<String, PlatformPropertyValue> {
    HashMap::from([("memory_kb".to_string(), PlatformPropertyValue::Minimum(kb))])
}

fn memory_action(kb: u64) -> HashMap<String, String> {
    HashMap::from([("memory_kb".to_string(), kb.to_string())])
}

fn worker_props(memory_kb: u64, zone: Option<&str>) -> HashMap<String, PlatformPropertyValue> {
    let mut props = memory_worker(memory_kb);
    if let Some(zone) = zone {
        props.insert(
            "zone".to_string(),
            PlatformPropertyValue::Priority(zone.to_string()),
        );
    }
    props
}

fn best_fit_scheduler() -> Arc<SimpleScheduler> {
    make_scheduler_with_spec(&SimpleSpec {
        supported_platform_properties: Some(HashMap::from([
            ("memory_kb".to_string(), PropertyType::Minimum),
            ("zone".to_string(), PropertyType::Priority),
        ])),
        allocation_strategy: WorkerAllocationStrategy::BestFit,
        ..SimpleSpec::default()
    })
}

/// A 4 GiB action goes to the worker it fills best: the one with 6 GiB
/// left, not the one with 30 GiB left, whatever the order they joined in.
#[nativelink_test]
async fn best_fit_keeps_the_big_room_for_big_actions() -> Result<(), Error> {
    let scheduler = best_fit_scheduler();
    let mut roomy =
        add_worker_with_properties(&scheduler, "roomy", worker_props(30_000, None)).await?;
    let mut snug =
        add_worker_with_properties(&scheduler, "snug", worker_props(6_000, None)).await?;
    add_action_with_properties(&scheduler, 1, memory_action(4_000)).await?;
    assert_eq!((dispatched(&mut roomy), dispatched(&mut snug)), (0, 1));

    // The next 4 GiB action no longer fits the snug worker (2 GiB left) and
    // takes the roomy one.
    add_action_with_properties(&scheduler, 2, memory_action(4_000)).await?;
    assert_eq!((dispatched(&mut roomy), dispatched(&mut snug)), (1, 0));
    Ok(())
}

/// A priority property is a preference: the worker carrying the action's
/// zone is chosen first, even when another fits more tightly; an action
/// naming a zone no worker has still runs, on the tightest fit.
#[nativelink_test]
async fn best_fit_prefers_the_worker_with_the_actions_priority_value() -> Result<(), Error> {
    let scheduler = best_fit_scheduler();
    let mut east =
        add_worker_with_properties(&scheduler, "east", worker_props(30_000, Some("east"))).await?;
    let mut west =
        add_worker_with_properties(&scheduler, "west", worker_props(6_000, Some("west"))).await?;

    let mut wants_east = memory_action(4_000);
    wants_east.insert("zone".to_string(), "east".to_string());
    add_action_with_properties(&scheduler, 3, wants_east).await?;
    assert_eq!((dispatched(&mut east), dispatched(&mut west)), (1, 0));

    let mut wants_north = memory_action(4_000);
    wants_north.insert("zone".to_string(), "north".to_string());
    add_action_with_properties(&scheduler, 4, wants_north).await?;
    assert_eq!(
        (dispatched(&mut east), dispatched(&mut west)),
        (0, 1),
        "no worker is north; the tightest fit takes it"
    );
    Ok(())
}

fn cache_scheduler(wait_s: u64) -> Arc<SimpleScheduler> {
    make_scheduler_with_spec(&SimpleSpec {
        allocation_strategy: WorkerAllocationStrategy::CacheAware,
        cache_affinity_wait_s: wait_s,
        worker_timeout_s: 1_000,
        client_action_timeout_s: 1_000,
        supported_platform_properties: Some(HashMap::from([(
            "memory_kb".to_string(),
            PropertyType::Minimum,
        )])),
        ..SimpleSpec::default()
    })
}

/// Different actions with the same command keep a three-worker working set.
/// An idle fourth worker must not draw input copies until the wait expires.
#[nativelink_test]
async fn cache_affinity_is_bounded_and_expires_without_new_capacity() -> Result<(), Error> {
    let scheduler = cache_scheduler(5);
    let mut workers = Vec::new();
    for i in 0..8 {
        workers.push(add_worker(&scheduler, &format!("worker-{i}")).await?);
    }
    for digest in 1..=15 {
        add_action(&scheduler, digest).await?;
    }
    let before: Vec<_> = workers.iter_mut().map(dispatched).collect();
    assert_eq!(before.iter().sum::<usize>(), 12);
    assert_eq!(before.iter().filter(|count| **count > 0).count(), 3);
    MockClock::advance(Duration::from_secs(4));
    scheduler.do_try_match_for_test().await?;
    assert_eq!(workers.iter_mut().map(dispatched).sum::<usize>(), 0);
    MockClock::advance(Duration::from_secs(1));
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    let after: Vec<_> = workers.iter_mut().map(dispatched).collect();
    assert_eq!(after.iter().sum::<usize>(), 3);
    assert!(
        before
            .iter()
            .zip(after.iter())
            .all(|(before, after)| *before == 0 || *after == 0)
    );
    Ok(())
}

#[nativelink_test]
async fn cache_aware_defaults_to_immediate_fallback() -> Result<(), Error> {
    let spec = serde_json::from_str(r#"{"allocation_strategy":"cache_aware"}"#).unwrap();
    let scheduler = make_scheduler_with_spec(&spec);
    let mut workers = Vec::new();
    for i in 0..5 {
        workers.push(add_worker(&scheduler, &format!("worker-{i}")).await?);
    }
    for digest in 1..=16 {
        add_action(&scheduler, digest).await?;
    }
    assert_eq!(workers.iter_mut().map(dispatched).sum::<usize>(), 16);
    Ok(())
}

/// The shortlist uses registered capacity, not changing free capacity: small
/// actions do not move to the large pool just because a small worker is busy.
#[nativelink_test]
async fn cache_affinity_preserves_the_large_pool_for_large_actions() -> Result<(), Error> {
    let scheduler = cache_scheduler(5);
    let mut small = Vec::new();
    for i in 0..4 {
        small.push(
            add_worker_with_properties(&scheduler, &format!("small-{i}"), memory_worker(4_000))
                .await?,
        );
    }
    let mut large = add_worker_with_properties(&scheduler, "large", memory_worker(40_000)).await?;
    for digest in 1..=12 {
        add_action_with_properties(&scheduler, digest, memory_action(1_000)).await?;
    }
    assert_eq!(small.iter_mut().map(dispatched).sum::<usize>(), 12);
    assert_eq!(dispatched(&mut large), 0);
    add_action_with_properties(&scheduler, 13, memory_action(30_000)).await?;
    assert_eq!(
        dispatched(&mut large),
        1,
        "a large worker is eligible immediately for a large action"
    );
    assert_eq!(small.iter_mut().map(dispatched).sum::<usize>(), 0);
    Ok(())
}

async fn priority_order(age_weight_s: u64) -> Result<Vec<String>, Error> {
    let scheduler = make_scheduler_with_spec(&SimpleSpec {
        queue_age_priority_s: age_weight_s,
        client_action_timeout_s: 1_000,
        ..SimpleSpec::default()
    });
    add_action(&scheduler, 1).await?;
    MockClock::advance(Duration::from_mins(1));
    let base = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([2; 32], 512),
    );
    let fresh = Arc::new(ActionInfo {
        priority: 5,
        ..(*base).clone()
    });
    scheduler.add_action(OperationId::default(), fresh).await?;
    let mut rx = add_worker(&scheduler, "worker").await?;
    scheduler.do_try_match_for_test().await?;
    tokio::task::yield_now().await;
    let mut order = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let Some(update_for_worker::Update::StartAction(start)) = msg.update {
            order.push(start.execute_request.unwrap().action_digest.unwrap().hash);
        }
    }
    Ok(order)
}

#[nativelink_test]
async fn queue_age_outweighs_a_fresh_actions_priority_advantage() -> Result<(), Error> {
    assert_eq!(
        priority_order(10).await?,
        vec!["01".repeat(32), "02".repeat(32)]
    );
    Ok(())
}

#[nativelink_test]
async fn disabled_queue_aging_preserves_strict_priority() -> Result<(), Error> {
    assert_eq!(
        priority_order(0).await?,
        vec!["02".repeat(32), "01".repeat(32)]
    );
    Ok(())
}

/// Keep a worker occupied while one higher-priority action arrives per second.
/// Each completion offers exactly one slot: the older action must eventually
/// take it even though fresh work continues to arrive.
async fn old_action_dispatch_round(age_weight_s: u64) -> Result<Option<u8>, Error> {
    use nativelink_util::action_messages::{ActionResult, ActionStage};
    use nativelink_util::operation_state_manager::UpdateOperationType;

    let scheduler = make_scheduler_with_spec(&SimpleSpec {
        queue_age_priority_s: age_weight_s,
        client_action_timeout_s: 1_000,
        worker_timeout_s: 1_000,
        supported_platform_properties: Some(HashMap::from([(
            "memory_kb".to_string(),
            PropertyType::Minimum,
        )])),
        ..SimpleSpec::default()
    });
    let worker_id = WorkerId("worker".to_string());
    let mut rx = add_worker_with_properties(&scheduler, "worker", memory_worker(1_000)).await?;
    add_action_with_properties(&scheduler, 1, memory_action(1_000)).await?;
    let Some(update_for_worker::Update::StartAction(mut running)) = rx.recv().await.unwrap().update
    else {
        panic!("the initial action must occupy the worker");
    };
    add_action_with_properties(&scheduler, 2, memory_action(1_000)).await?;
    let old_digest = DigestInfo::new([2; 32], 512);
    let mut dispatched_round = None;
    for round in 1..=12 {
        MockClock::advance(Duration::from_secs(1));
        let base = make_base_action_info(
            UNIX_EPOCH + MockClock::time(),
            DigestInfo::new([100 + round; 32], 512),
        );
        scheduler
            .add_action(
                OperationId::default(),
                Arc::new(ActionInfo {
                    priority: 5,
                    platform_properties: memory_action(1_000),
                    ..(*base).clone()
                }),
            )
            .await?;
        scheduler
            .update_action(
                &worker_id,
                &OperationId::from(running.operation_id),
                UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(
                    ActionResult::default(),
                )),
            )
            .await?;
        scheduler.do_try_match_for_test().await?;
        let update = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("a runnable action must take the released slot")
            .unwrap();
        let Some(update_for_worker::Update::StartAction(next)) = update.update else {
            panic!("expected the next action, got {update:?}");
        };
        if next.execute_request.as_ref().unwrap().action_digest == Some(old_digest.into()) {
            assert!(
                dispatched_round.replace(round).is_none(),
                "duplicate assignment"
            );
        }
        assert_eq!(rx.try_recv(), Err(mpsc::error::TryRecvError::Empty));
        running = next;
    }
    Ok(dispatched_round)
}

#[nativelink_test]
async fn queue_aging_dispatches_old_work_during_continuing_priority_arrivals() -> Result<(), Error>
{
    // Negative control: strict priority bypasses the old action at every slot.
    assert_eq!(old_action_dispatch_round(0).await?, None);
    // At five seconds the scores tie; at six the old action strictly wins.
    // Continue arrivals after it runs to check it was assigned only once.
    assert_eq!(old_action_dispatch_round(1).await?, Some(6));
    Ok(())
}

#[nativelink_test]
async fn repeated_commands_reuse_the_same_workers_after_completion() -> Result<(), Error> {
    use nativelink_util::action_messages::{ActionResult, ActionStage};
    use nativelink_util::operation_state_manager::UpdateOperationType;

    let scheduler = cache_scheduler(5);
    let mut workers = Vec::new();
    for i in 0..8 {
        workers.push(add_worker(&scheduler, &format!("worker-{i}")).await?);
    }
    let mut first = Vec::new();
    for round in 0..4 {
        for digest in 1..=12 {
            add_action(&scheduler, round * 12 + digest).await?;
        }
        let mut chosen = Vec::new();
        for (i, rx) in workers.iter_mut().enumerate() {
            let mut operations = Vec::new();
            while let Ok(msg) = rx.try_recv() {
                if let Some(update_for_worker::Update::StartAction(start)) = msg.update {
                    operations.push(start.operation_id);
                }
            }
            if !operations.is_empty() {
                chosen.push(i);
            }
            for operation in operations {
                scheduler
                    .update_action(
                        &WorkerId(format!("worker-{i}")),
                        &OperationId::from(operation),
                        UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(
                            ActionResult::default(),
                        )),
                    )
                    .await?;
            }
        }
        assert_eq!(chosen.len(), 3);
        if round == 0 {
            first = chosen;
        } else {
            assert_eq!(chosen, first);
        }
    }
    Ok(())
}

#[nativelink_test]
async fn draining_preferred_workers_are_replaced_without_a_locality_wait() -> Result<(), Error> {
    let scheduler = cache_scheduler(5);
    let mut workers = Vec::new();
    for i in 0..6 {
        workers.push(add_worker(&scheduler, &format!("worker-{i}")).await?);
    }
    for digest in 1..=12 {
        add_action(&scheduler, digest).await?;
    }
    for (i, rx) in workers.iter_mut().enumerate() {
        if dispatched(rx) > 0 {
            scheduler
                .set_drain_worker(&WorkerId(format!("worker-{i}")), true)
                .await?;
        }
    }
    add_action(&scheduler, 13).await?;
    assert_eq!(workers.iter_mut().map(dispatched).sum::<usize>(), 1);
    Ok(())
}

#[nativelink_test]
async fn locality_wait_requires_a_periodic_wakeup() -> Result<(), Error> {
    use nativelink_config::schedulers::SchedulerSpec;
    use nativelink_scheduler::default_scheduler_factory::scheduler_factory;
    use nativelink_store::store_manager::StoreManager;

    let mut spec: SimpleSpec =
        serde_json::from_str(r#"{"allocation_strategy":"cache_aware"}"#).unwrap();
    assert_eq!(spec.cache_affinity_wait_s, 0);
    assert_eq!(spec.queue_age_priority_s, 0);
    assert!(spec.fallback_match_interval_s > 0);
    spec.cache_affinity_wait_s = 5;
    spec.fallback_match_interval_s = 0;
    let stores = StoreManager::new();
    let rejected = scheduler_factory(&SchedulerSpec::Simple(spec), &stores, None).await;
    assert!(
        matches!(rejected, Err(ref err) if err.message_string().contains("requires fallback_match_interval_s"))
    );
    let no_wait: SimpleSpec = serde_json::from_str(
        r#"{
        "allocation_strategy":"cache_aware", "cache_affinity_wait_s":0,
        "fallback_match_interval_s":0
    }"#,
    )
    .unwrap();
    scheduler_factory(&SchedulerSpec::Simple(no_wait), &stores, None).await?;
    Ok(())
}
