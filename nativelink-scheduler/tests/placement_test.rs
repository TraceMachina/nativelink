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
use nativelink_config::schedulers::{SimpleSpec, WorkerAllocationStrategy};
use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    UpdateForWorker, update_for_worker,
};
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::{OperationId, WorkerId};
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::operation_state_manager::ClientStateManager;
use nativelink_util::platform_properties::PlatformProperties;
use pretty_assertions::assert_eq;
use tokio::sync::{Notify, mpsc};
use utils::scheduler_utils::make_base_action_info;

mod utils {
    pub(crate) mod scheduler_utils;
}

const NOW_TIME: u64 = 10000;

fn make_scheduler(strategy: WorkerAllocationStrategy) -> Arc<SimpleScheduler> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let task_change_notify = Arc::new(Notify::new());
    let spec = SimpleSpec {
        allocation_strategy: strategy,
        ..SimpleSpec::default()
    };
    let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
        &spec,
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
    let (tx, mut rx) = mpsc::unbounded_channel();
    let worker = Worker::new(
        WorkerId(name.to_string()),
        PlatformProperties::new(HashMap::new()),
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
    let action_info = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([digest_byte; 32], 512),
    );
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
