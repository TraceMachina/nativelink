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

//! A result the store refused is held for the executing sweep to write. It
//! is the outcome of one attempt, on the worker that sent it: asked about
//! another worker, or once the operation has been dispatched again, the
//! worker scheduler does not offer it.

use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_trait::async_trait;
use nativelink_config::schedulers::WorkerAllocationStrategy;
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::UpdateForWorker;
use nativelink_scheduler::api_worker_scheduler::ApiWorkerScheduler;
use nativelink_scheduler::platform_property_manager::PlatformPropertyManager;
use nativelink_scheduler::simple_scheduler_state_manager::OnWorker;
use nativelink_scheduler::worker::{ActionInfoWithProps, Worker};
use nativelink_scheduler::worker_registry::WorkerRegistry;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::{ActionResult, ActionStage, OperationId, WorkerId};
use nativelink_util::common::DigestInfo;
use nativelink_util::operation_state_manager::{UpdateOperationType, WorkerStateManager};
use nativelink_util::origin_event::OriginMetadata;
use nativelink_util::platform_properties::PlatformProperties;
use tokio::sync::{Notify, mpsc};
use utils::scheduler_utils::make_base_action_info;

mod utils {
    pub(crate) mod scheduler_utils;
}

const NOW: u64 = 10_000;

/// Refuses every result while `refusing` is set, as a store at its memory
/// ceiling does.
#[derive(Default, MetricsComponent)]
struct RefusingStateManager {
    refusing: AtomicBool,
}

#[async_trait]
impl WorkerStateManager for RefusingStateManager {
    async fn update_operation(
        &self,
        _operation_id: &OperationId,
        _worker_id: &WorkerId,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        let is_result = matches!(
            update,
            UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(_))
        );
        if is_result && self.refusing.load(Ordering::Acquire) {
            return Err(make_err!(Code::Unavailable, "The store refused the write"));
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
    scheduler: Arc<ApiWorkerScheduler>,
    first: WorkerId,
    second: WorkerId,
    operation_id: OperationId,
    /// The workers' ends of their command streams; dropping one disconnects
    /// its worker.
    _streams: Vec<mpsc::Receiver<UpdateForWorker>>,
}

impl Fixture {
    async fn dispatch_to(&self, worker_id: &WorkerId) -> Result<(), Error> {
        self.scheduler
            .worker_notify_run_action(
                worker_id.clone(),
                self.operation_id.clone(),
                ActionInfoWithProps {
                    inner: make_base_action_info(
                        SystemTime::UNIX_EPOCH,
                        DigestInfo::new([1u8; 32], 1),
                    ),
                    platform_properties: PlatformProperties::default(),
                    origin_metadata: OriginMetadata::default(),
                    scheduler_start_execute_event_id: None,
                },
                NOW,
            )
            .await
    }

    async fn on_worker(&self, worker_id: &WorkerId) -> Option<OnWorker> {
        self.scheduler
            .running_on_worker(worker_id, &self.operation_id)
            .await
    }
}

/// Two connected workers; the first ran the operation and the store refused
/// its result.
async fn first_workers_result_refused() -> Result<Fixture, Error> {
    let state_manager = Arc::new(RefusingStateManager::default());
    let scheduler = ApiWorkerScheduler::new(
        state_manager.clone(),
        Arc::new(PlatformPropertyManager::new(HashMap::new())),
        WorkerAllocationStrategy::default(),
        None,
        None,
        None,
        None, // worker_outcome_tripwire
        Arc::new(Notify::new()),
        10,
        60,
        0,
        Arc::new(WorkerRegistry::new()),
        None,
        None,
        false,
        Duration::from_secs(15),
        "scheduler".to_string(),
    );
    let first = WorkerId::from(String::from("first"));
    let second = WorkerId::from(String::from("second"));
    let mut streams = Vec::new();
    for worker_id in [&first, &second] {
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
    let fixture = Fixture {
        scheduler,
        first,
        second,
        operation_id: OperationId::default(),
        _streams: streams,
    };
    fixture.dispatch_to(&fixture.first).await?;

    state_manager.refusing.store(true, Ordering::Release);
    let refused = fixture
        .scheduler
        .update_action(
            &fixture.first,
            &fixture.operation_id,
            UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(
                ActionResult::default(),
            )),
        )
        .await;
    assert!(refused.is_err(), "the store should refuse the result");
    Ok(fixture)
}

#[nativelink_test]
async fn refused_result_is_held_for_the_worker_that_sent_it() -> Result<(), Error> {
    let f = first_workers_result_refused().await?;

    let on_first = f.on_worker(&f.first).await;
    assert!(
        matches!(on_first, Some(OnWorker::ResultHeld(_))),
        "the sweep should be handed the first worker's result, got {on_first:?}"
    );
    Ok(())
}

/// The record was requeued and is now another worker's: that attempt is not
/// completed with the first worker's result.
#[nativelink_test]
async fn held_result_is_not_offered_for_another_workers_attempt() -> Result<(), Error> {
    let f = first_workers_result_refused().await?;

    // A worker of this scheduler that is not running it.
    let on_second = f.on_worker(&f.second).await;
    assert!(
        matches!(on_second, Some(OnWorker::NotRunning)),
        "the first worker's result is not the second's, got {on_second:?}"
    );
    // A worker of another scheduler.
    let elsewhere = WorkerId::from(String::from("elsewhere"));
    let on_elsewhere = f.on_worker(&elsewhere).await;
    assert!(
        on_elsewhere.is_none(),
        "a worker not connected here is not known, got {on_elsewhere:?}"
    );

    // Settling it for the wrong worker leaves it for the right one.
    f.scheduler
        .settle_held_result(&f.second, &f.operation_id, false)
        .await;
    let on_first = f.on_worker(&f.first).await;
    assert!(
        matches!(on_first, Some(OnWorker::ResultHeld(_))),
        "the result is still the first worker's, got {on_first:?}"
    );
    Ok(())
}

/// Dispatched again, the operation is on a new attempt, and the result of
/// the one before is not written into it, whichever worker now runs it.
#[nativelink_test]
async fn held_result_is_dropped_when_the_operation_is_dispatched_again() -> Result<(), Error> {
    let f = first_workers_result_refused().await?;
    f.dispatch_to(&f.first).await?;
    let on_first = f.on_worker(&f.first).await;
    assert!(
        matches!(on_first, Some(OnWorker::Running)),
        "the first worker is running a new attempt, got {on_first:?}"
    );

    let f = first_workers_result_refused().await?;
    f.dispatch_to(&f.second).await?;
    let on_second = f.on_worker(&f.second).await;
    assert!(
        matches!(on_second, Some(OnWorker::Running)),
        "the second worker is running a new attempt, got {on_second:?}"
    );
    let on_first = f.on_worker(&f.first).await;
    assert!(
        matches!(on_first, Some(OnWorker::NotRunning)),
        "the first worker's result went with its attempt, got {on_first:?}"
    );
    Ok(())
}
