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
use nativelink_config::schedulers::SimpleSpec;
use nativelink_error::{Code, Error, ResultExt, make_err};
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
use nativelink_util::metrics::ActiveCountAttributes;
use nativelink_util::operation_state_manager::{ClientStateManager, UpdateOperationType};
use nativelink_util::platform_properties::PlatformProperties;
use pretty_assertions::assert_eq;
use tokio::sync::{Notify, mpsc};
use utils::scheduler_utils::make_base_action_info;

mod utils {
    pub(crate) mod scheduler_utils;
}

const NOW_TIME: u64 = 10000;

fn make_scheduler() -> Arc<SimpleScheduler> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let task_change_notify = Arc::new(Notify::new());
    let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
        &SimpleSpec::default(),
        memory_awaited_action_db_factory(
            0,
            &task_change_notify,
            MockInstantWrapped::default,
            ActiveCountAttributes::default(),
        ),
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
        PlatformProperties::new(HashMap::new()),
        tx,
        NOW_TIME,
        0,
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

/// The operation ids of every `StartAction` the worker has been sent.
fn started(rx: &mut mpsc::Receiver<UpdateForWorker>) -> Vec<OperationId> {
    let mut ids = Vec::new();
    while let Ok(msg) = rx.try_recv() {
        if let Some(update_for_worker::Update::StartAction(start)) = msg.update {
            ids.push(OperationId::from(start.operation_id));
        }
    }
    ids
}

/// An idle worker that refuses an action with `ResourceExhausted` is not
/// offered it again until its next keepalive. Before, only a worker with
/// other actions was paused, so an idle one refusing (a failing
/// precondition script, a full disk) was re-offered the same action in a
/// tight loop.
#[nativelink_test]
async fn backpressure_pauses_an_idle_worker_until_its_next_keepalive() -> Result<(), Error> {
    let scheduler = make_scheduler();
    let worker_id = WorkerId("worker".to_string());
    let mut rx = add_worker(&scheduler, "worker").await?;
    add_action(&scheduler, 1).await?;
    let first = started(&mut rx);
    assert_eq!(first.len(), 1, "the idle worker takes the action");

    scheduler
        .update_action(
            &worker_id,
            &first[0],
            UpdateOperationType::UpdateWithError(make_err!(
                Code::ResourceExhausted,
                "precondition script failed"
            )),
        )
        .await?;
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    assert_eq!(
        started(&mut rx),
        Vec::<OperationId>::new(),
        "the requeued action must not come straight back to the worker that refused it"
    );

    scheduler
        .worker_keep_alive_received(&worker_id, NOW_TIME + 3, None)
        .await?;
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    assert_eq!(
        started(&mut rx).len(),
        1,
        "after the keepalive the worker is offered the action again"
    );
    Ok(())
}
