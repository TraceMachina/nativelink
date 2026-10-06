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

//! The scheduler's wall clock can step backwards (NTP, VM time sync)
//! between two messages from the same worker. The worker is no less
//! alive for it: a liveness refresh with an older timestamp keeps the
//! newer one and succeeds, and the action the worker finishes afterwards
//! completes normally.

use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use mock_instant::thread_local::MockClock;
use nativelink_config::schedulers::{PropertyType, SimpleSpec};
use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    UpdateForWorker, update_for_worker,
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
use tokio::sync::{Notify, mpsc};
use utils::scheduler_utils::make_base_action_info;

mod utils {
    pub(crate) mod scheduler_utils;
}

const NOW_TIME: u64 = 10000;
const WORKER: &str = "the_worker";

fn make_scheduler() -> (Arc<SimpleScheduler>, Arc<dyn WorkerScheduler>) {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let task_change_notify = Arc::new(Notify::new());
    let spec = SimpleSpec {
        supported_platform_properties: Some(HashMap::from([(
            "memory_kb".to_string(),
            PropertyType::Minimum,
        )])),
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

async fn add_worker(scheduler: &SimpleScheduler) -> Result<mpsc::Receiver<UpdateForWorker>, Error> {
    let (tx, mut rx) = mpsc::channel(64);
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
        "expected a ConnectionResult, got {connected:?}"
    );
    Ok(rx)
}

async fn wait_for_completion(listener: &mut Box<dyn ActionStateResult>) -> ActionResult {
    loop {
        let (state, _) = listener.changed().await.expect("state stream open");
        if let ActionStage::Completed(result) = &state.stage {
            return result.clone();
        }
    }
}

/// A liveness refresh carrying a timestamp older than the worker's last
/// one succeeds, keeps the newer timestamp, and nothing downstream of it
/// is lost: the result the worker reports afterwards completes the
/// action instead of stranding it in `Executing`.
#[nativelink_test]
async fn backwards_clock_step_does_not_fail_liveness_or_lose_results()
-> Result<(), Box<dyn core::error::Error>> {
    let (scheduler, worker_scheduler) = make_scheduler();
    let mut rx = add_worker(&scheduler).await?;

    let base = make_base_action_info(
        UNIX_EPOCH + MockClock::time(),
        DigestInfo::new([7u8; 32], 512),
    );
    let action_info = Arc::new(ActionInfo {
        platform_properties: HashMap::from([("memory_kb".to_string(), "2000".to_string())]),
        ..(*base).clone()
    });
    let mut listener = scheduler
        .add_action(OperationId::default(), action_info)
        .await?;
    tokio::task::yield_now().await;

    let dispatched = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .expect("a dispatch within two seconds")
        .expect("the channel is open");
    let Some(update_for_worker::Update::StartAction(start_execute)) = dispatched.update else {
        panic!("expected a StartAction, got {dispatched:?}");
    };

    // The worker's last message was at NOW_TIME + 1 ...
    worker_scheduler
        .worker_liveness_refreshed(&worker_id(), NOW_TIME + 1)
        .await
        .err_tip(|| "Liveness refresh at NOW_TIME + 1 failed")?;

    // ... and NTP then steps the scheduler's clock back two seconds. The
    // refresh must succeed anyway; failing here used to propagate out of
    // the worker API server and discard the message that carried it.
    worker_scheduler
        .worker_liveness_refreshed(&worker_id(), NOW_TIME - 1)
        .await
        .err_tip(|| "Liveness refresh after a backwards clock step failed")?;

    // The result the worker produced still completes the action.
    worker_scheduler
        .update_action(
            &worker_id(),
            &OperationId::from(start_execute.operation_id.as_str()),
            UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(
                ActionResult::default(),
            )),
        )
        .await
        .err_tip(|| "Failed to complete the action after the clock step")?;
    let result = wait_for_completion(&mut listener).await;
    assert_eq!(result, ActionResult::default());
    Ok(())
}
