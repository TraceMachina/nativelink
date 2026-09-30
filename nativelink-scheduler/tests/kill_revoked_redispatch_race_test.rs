// Copyright 2024 The NativeLink Authors. All rights reserved.
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

//! Deterministic reproduction of the requeue-then-redispatch TOCTOU in
//! `kill_revoked_operations` (`api_worker_scheduler.rs`): the sweep drops the
//! inner mutex between its lock-free `is_executing_on_worker` confirmation
//! and the final locked `worker_notify_kill_operation` loop. If the matcher
//! re-dispatches the same operation to the same worker inside that gap, the
//! re-guard (`contains_key` + `is_kill_requested`) passes on the fresh
//! entry (`kill_requested_at: None`) and the kill lands on live work.

mod utils {
    pub(crate) mod scheduler_utils;
}

use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nativelink_config::schedulers::WorkerAllocationStrategy;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::update_for_worker;
use nativelink_scheduler::api_worker_scheduler::ApiWorkerScheduler;
use nativelink_scheduler::platform_property_manager::PlatformPropertyManager;
use nativelink_scheduler::worker::{ActionInfoWithProps, Worker};
use nativelink_scheduler::worker_registry::WorkerRegistry;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::{OperationId, WorkerId};
use nativelink_util::common::DigestInfo;
use nativelink_util::operation_state_manager::{Decline, UpdateOperationType, WorkerStateManager};
use nativelink_util::origin_event::OriginMetadata;
use nativelink_util::platform_properties::PlatformProperties;
use nativelink_util::spawn;
use tokio::sync::{Notify, mpsc};
use utils::scheduler_utils::make_base_action_info;

/// A state manager scripted to answer `is_executing_on_worker` truthfully
/// for the interleaving under test, with rendezvous points so the test can
/// act inside the sweep's lock-free windows.
struct ScriptedStateManager {
    op_x: OperationId,
    x_calls: AtomicUsize,
    y_calls: AtomicUsize,
    /// Sweep reached X's first (snapshot-verification) check; the snapshot
    /// of running operations has been taken by now.
    x1_reached: Notify,
    /// Test releases X's first check (after the decline requeued X).
    x1_gate: Notify,
    /// Test releases Y's first check.
    y1_gate: Notify,
    /// X's confirmation check has returned false; `confirmed` now contains
    /// X and the sweep still holds no lock.
    x2_done: Notify,
    /// Test releases Y's confirmation check (the await point that models
    /// the gap before the sweep re-acquires the inner mutex).
    y2_gate: Notify,
}

impl MetricsComponent for ScriptedStateManager {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        Ok(MetricPublishKnownKindData::Component)
    }
}

#[async_trait]
impl WorkerStateManager for ScriptedStateManager {
    async fn update_operation(
        &self,
        _operation_id: &OperationId,
        _worker_id: &WorkerId,
        _update: UpdateOperationType,
    ) -> Result<(), Error> {
        // The decline requeues the operation in the real state manager;
        // nothing to record here.
        Ok(())
    }

    async fn is_executing_on_worker(
        &self,
        operation_id: &OperationId,
        _worker_id: &WorkerId,
    ) -> Result<bool, Error> {
        if *operation_id == self.op_x {
            if self.x_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.x1_reached.notify_one();
                self.x1_gate.notified().await;
                // Truthful: X was declined and requeued.
            } else {
                // Confirmation check. Truthful: X is still queued (the
                // re-dispatch has not happened yet -- it happens right
                // after this returns, in the gap before the sweep
                // takes the inner lock).
                self.x2_done.notify_one();
            }
            Ok(false)
        } else if self.y_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            self.y1_gate.notified().await;
            // Truthful: Y was declined and requeued.
            Ok(false)
        } else {
            // Confirmation check for Y: the sweep awaits here after
            // X's confirmation returned. This is the window in
            // which the matcher re-dispatches X.
            self.y2_gate.notified().await;
            // Y got reassigned elsewhere: skip it this pass.
            Ok(true)
        }
    }
}

fn make_action(digest_byte: u8) -> ActionInfoWithProps {
    ActionInfoWithProps {
        inner: make_base_action_info(
            std::time::UNIX_EPOCH,
            DigestInfo::new([digest_byte; 32], 512),
        ),
        platform_properties: PlatformProperties::default(),
        origin_metadata: OriginMetadata::default(),
        scheduler_start_execute_event_id: None,
    }
}

#[nativelink_test]
async fn kill_revoked_operations_must_not_kill_a_redispatched_operation() -> Result<(), Error> {
    let worker_id = WorkerId("worker1".to_string());
    let op_x = OperationId::default();
    let op_y = OperationId::default();

    let scripted = Arc::new(ScriptedStateManager {
        op_x: op_x.clone(),
        x_calls: AtomicUsize::new(0),
        y_calls: AtomicUsize::new(0),
        x1_reached: Notify::new(),
        x1_gate: Notify::new(),
        y1_gate: Notify::new(),
        x2_done: Notify::new(),
        y2_gate: Notify::new(),
    });

    let scheduler = ApiWorkerScheduler::new(
        scripted.clone(),
        Arc::new(PlatformPropertyManager::new(HashMap::new())),
        WorkerAllocationStrategy::default(),
        None,
        None,
        Arc::new(Notify::new()),
        100, // worker_timeout_s
        100, // unacknowledged_kill_timeout_s
        100, // dispatch_ack_timeout_s
        Arc::new(WorkerRegistry::new()),
        None,
        false,
        Duration::from_mins(1),
    );

    let (tx, mut rx) = mpsc::channel(16);
    scheduler
        .add_worker(Worker::new(
            worker_id.clone(),
            PlatformProperties::default(),
            tx,
            0,
            4,
        ))
        .await?;
    // Drain the initial connection message.
    drop(rx.recv().await);

    // Dispatch X and Y to the worker.
    scheduler
        .worker_notify_run_action(worker_id.clone(), op_x.clone(), make_action(1), 0)
        .await?;
    scheduler
        .worker_notify_run_action(worker_id.clone(), op_y.clone(), make_action(2), 0)
        .await?;
    // Drain the two StartAction messages.
    drop(rx.recv().await);
    drop(rx.recv().await);

    // The sweep runs concurrently, exactly as the worker_api_server
    // interval task does.
    let sweep = spawn!("kill_revoked_sweep", {
        let scheduler = scheduler.clone();
        async move { scheduler.kill_revoked_operations().await }
    });

    // The sweep has snapshotted (W, X) and (W, Y) with
    // kill_requested_at: None and reached its lock-free checks.
    scripted.x1_reached.notified().await;

    // The worker declines both operations (e.g. the unacknowledged sweep or
    // load backpressure): complete_action removes them from
    // running_action_infos and the state manager requeues them.
    scheduler
        .update_action(
            &worker_id,
            &op_x,
            UpdateOperationType::UpdateWithDecline(Decline {
                reason: "test".to_string(),
            }),
        )
        .await?;
    scheduler
        .update_action(
            &worker_id,
            &op_y,
            UpdateOperationType::UpdateWithDecline(Decline {
                reason: "test".to_string(),
            }),
        )
        .await?;

    // Release the first-round checks, X strictly before Y so the sweep's
    // `revoked` list is ordered [X, Y].
    scripted.x1_gate.notify_one();
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }
    scripted.y1_gate.notify_one();

    // Wait until X's confirmation check has returned false (X is in
    // `confirmed`) while the sweep is parked on Y's confirmation check,
    // holding no lock. This is the TOCTOU window.
    scripted.x2_done.notified().await;

    // The matching engine re-dispatches X to the same worker: a fresh,
    // legitimate dispatch with kill_requested_at: None. The wall-clock
    // timestamp is the SAME second as the original dispatch: `dispatched_at`
    // has second resolution, so a requeue-and-redispatch inside one second
    // is indistinguishable by timestamp. Only a per-dispatch generation
    // tells the two dispatches apart.
    scheduler
        .worker_notify_run_action(worker_id.clone(), op_x.clone(), make_action(1), 0)
        .await?;
    match rx.recv().await.unwrap().update {
        Some(update_for_worker::Update::StartAction(_)) => {}
        v => panic!("Expected StartAction for the re-dispatch, got: {v:?}"),
    }

    // Let the sweep finish: it re-acquires the inner lock and runs
    // worker_notify_kill_operation for X.
    scripted.y2_gate.notify_one();
    sweep.await.unwrap()?;

    // The fresh dispatch must not be killed: the state manager considers X
    // legitimately executing on this worker again. On buggy code the
    // re-guard passes on the NEW entry and a KillOperationRequest lands on
    // live work (and the worker's subsequent report is dropped by
    // update_action's is_kill_requested check).
    match rx.try_recv() {
        Err(mpsc::error::TryRecvError::Empty) => {}
        Ok(update) => panic!(
            "The freshly re-dispatched operation was killed by the stale sweep: {:?}",
            update.update
        ),
        Err(err) => panic!("Worker channel unexpectedly closed: {err:?}"),
    }

    Ok(())
}
