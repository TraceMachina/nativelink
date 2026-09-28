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
) -> Result<mpsc::UnboundedReceiver<UpdateForWorker>, Error> {
    add_worker_with_properties(scheduler, name, HashMap::new()).await
}

async fn add_worker_with_properties(
    scheduler: &SimpleScheduler,
    name: &str,
    properties: HashMap<String, PlatformPropertyValue>,
) -> Result<mpsc::UnboundedReceiver<UpdateForWorker>, Error> {
    let (tx, mut rx) = mpsc::unbounded_channel();
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

fn dispatched(rx: &mut mpsc::UnboundedReceiver<UpdateForWorker>) -> usize {
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
