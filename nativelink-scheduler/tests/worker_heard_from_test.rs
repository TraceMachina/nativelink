// Copyright 2026 Trace Machina, Inc. All rights reserved.
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

//! A worker is timed out for its own silence, never for the scheduler's
//! delay. Its messages count from the moment they arrive, and a message
//! that waited to be processed does not make the worker look older than it
//! is.

use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use nativelink_config::schedulers::WorkerAllocationStrategy;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::UpdateForWorker;
use nativelink_scheduler::api_worker_scheduler::ApiWorkerScheduler;
use nativelink_scheduler::platform_property_manager::PlatformPropertyManager;
use nativelink_scheduler::worker::{ActionInfoWithProps, Worker};
use nativelink_scheduler::worker_registry::WorkerRegistry;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::{ActionResult, ActionStage, OperationId, WorkerId};
use nativelink_util::common::DigestInfo;
use nativelink_util::operation_state_manager::{UpdateOperationType, WorkerStateManager};
use nativelink_util::origin_event::OriginMetadata;
use nativelink_util::platform_properties::PlatformProperties;
use tokio::sync::{Notify, Semaphore, mpsc};
use utils::scheduler_utils::make_base_action_info;

mod utils {
    pub(crate) mod scheduler_utils;
}

const NOW: u64 = 10_000;
const WORKER_TIMEOUT_S: u64 = 10;

/// Holds every result until `answer` has a permit, as a store that has
/// stopped answering does. The worker scheduler writes a result with its
/// lock held, so nothing else of it moves meanwhile.
#[derive(MetricsComponent)]
struct StalledStateManager {
    writing: Notify,
    answer: Semaphore,
}

#[async_trait]
impl WorkerStateManager for StalledStateManager {
    async fn update_operation(
        &self,
        _operation_id: &OperationId,
        _worker_id: &WorkerId,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        if matches!(
            update,
            UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(_))
        ) {
            self.writing.notify_one();
            self.answer.acquire().await.expect("never closed").forget();
        }
        Ok(())
    }

    async fn is_executing_on_worker(
        &self,
        _operation_id: &OperationId,
        _worker_id: &WorkerId,
    ) -> Result<bool, Error> {
        Ok(true)
    }
}

struct Fixture {
    state_manager: Arc<StalledStateManager>,
    scheduler: Arc<ApiWorkerScheduler>,
    busy: WorkerId,
    idle: WorkerId,
    silent: WorkerId,
    operation_id: OperationId,
    /// The workers' ends of their command streams; dropping one disconnects
    /// its worker.
    _streams: Vec<mpsc::Receiver<UpdateForWorker>>,
}

impl Fixture {
    /// Whether the worker is still in the pool.
    async fn has(&self, worker_id: &WorkerId) -> bool {
        self.scheduler
            .running_on_worker(worker_id, &self.operation_id)
            .await
            .is_some()
    }
}

/// Three workers connected at `NOW`; `busy` holds one operation.
async fn three_workers() -> Result<Fixture, Error> {
    let state_manager = Arc::new(StalledStateManager {
        writing: Notify::new(),
        answer: Semaphore::new(0),
    });
    let scheduler = ApiWorkerScheduler::new(
        state_manager.clone(),
        Arc::new(PlatformPropertyManager::new(HashMap::new())),
        WorkerAllocationStrategy::default(),
        None,
        None,
        None,
        None, // worker_outcome_tripwire
        Arc::new(Notify::new()),
        WORKER_TIMEOUT_S,
        60,
        0,
        Arc::new(WorkerRegistry::new()),
        None,
        None,
        false,
        Duration::from_secs(15),
        "scheduler".to_string(),
    );
    let [busy, idle, silent] =
        ["busy", "idle", "silent"].map(|name| WorkerId::from(name.to_string()));
    let mut streams = Vec::new();
    for worker_id in [&busy, &idle, &silent] {
        let (tx, rx) = mpsc::channel(8);
        scheduler
            .add_worker(Worker::new(
                worker_id.clone(),
                PlatformProperties::default(),
                tx,
                NOW,
                0,
            ))
            .await?;
        streams.push(rx);
    }
    let operation_id = OperationId::default();
    scheduler
        .worker_notify_run_action(
            busy.clone(),
            operation_id.clone(),
            ActionInfoWithProps {
                inner: make_base_action_info(SystemTime::UNIX_EPOCH, DigestInfo::new([1u8; 32], 1)),
                platform_properties: PlatformProperties::default(),
                origin_metadata: OriginMetadata::default(),
                scheduler_start_execute_event_id: None,
            },
            NOW,
        )
        .await?;
    Ok(Fixture {
        state_manager,
        scheduler,
        busy,
        idle,
        silent,
        operation_id,
        _streams: streams,
    })
}

/// A result's store write stalls with the scheduler's lock held, for three
/// worker timeouts. The workers whose messages kept arriving are kept; the
/// one that sent nothing is timed out.
#[nativelink_test]
async fn a_worker_heard_from_while_the_scheduler_is_stalled_is_not_timed_out() -> Result<(), Error>
{
    let f = three_workers().await?;
    let stalled_write = {
        let scheduler = f.scheduler.clone();
        let (busy, operation_id) = (f.busy.clone(), f.operation_id.clone());
        tokio::spawn(async move {
            scheduler
                .update_action(
                    &busy,
                    &operation_id,
                    UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(
                        ActionResult::default(),
                    )),
                )
                .await
        })
    };
    f.state_manager.writing.notified().await;

    // Keepalives arrive throughout and wait to be processed.
    let stall_end = NOW + 3 * WORKER_TIMEOUT_S;
    for worker_id in [&f.busy, &f.idle] {
        f.scheduler
            .worker_message_received(worker_id, stall_end)
            .await;
    }

    f.state_manager.answer.add_permits(1);
    stalled_write.await.expect("the write task ran")?;
    drop(f.scheduler.remove_timedout_workers(stall_end + 1).await);

    assert!(f.has(&f.busy).await, "the busy worker was heard from");
    assert!(f.has(&f.idle).await, "the idle worker was heard from");
    assert!(!f.has(&f.silent).await, "the silent worker timed out");
    Ok(())
}

/// A message that waited to be processed carries the time it started
/// waiting. Processing it must not make the worker look that old: the worker
/// has been heard from since.
#[nativelink_test]
async fn a_message_processed_late_does_not_move_last_seen_back() -> Result<(), Error> {
    let f = three_workers().await?;
    let recent = NOW + 3 * WORKER_TIMEOUT_S;
    // Heard from a moment ago; none of it has been processed.
    f.scheduler.worker_message_received(&f.idle, recent).await;
    // An old message of the same worker gets its turn only now.
    f.scheduler
        .worker_liveness_refreshed(&f.idle, NOW + 1)
        .await?;
    drop(f.scheduler.remove_timedout_workers(recent + 1).await);

    assert!(f.has(&f.idle).await, "heard from a second ago");
    Ok(())
}
