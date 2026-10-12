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

//! A worker's keepalive refreshes the store's worker-update timestamp on the
//! operations it runs, once per interval, so "silent for the executing
//! ceiling" means the worker went quiet and not merely that the action was
//! assigned that long ago.

use core::time::Duration;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use async_trait::async_trait;
use nativelink_config::schedulers::WorkerAllocationStrategy;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_scheduler::api_worker_scheduler::ApiWorkerScheduler;
use nativelink_scheduler::platform_property_manager::PlatformPropertyManager;
use nativelink_scheduler::worker::{ActionInfoWithProps, Worker};
use nativelink_scheduler::worker_registry::WorkerRegistry;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::{OperationId, WorkerId};
use nativelink_util::common::DigestInfo;
use nativelink_util::operation_state_manager::{UpdateOperationType, WorkerStateManager};
use nativelink_util::origin_event::OriginMetadata;
use nativelink_util::platform_properties::PlatformProperties;
use pretty_assertions::assert_eq;
use tokio::sync::{Notify, mpsc};
use utils::scheduler_utils::make_base_action_info;

mod utils {
    pub(crate) mod scheduler_utils;
}

const NOW: u64 = 10_000;
const WORKER_TIMEOUT_S: u64 = 10;
const TOUCH_INTERVAL_S: u64 = 100;

/// Records every keepalive update the worker scheduler asks for.
#[derive(Default, MetricsComponent)]
struct RecordingStateManager {
    keepalives: Mutex<Vec<(OperationId, WorkerId)>>,
}

impl RecordingStateManager {
    fn keepalives(&self) -> Vec<(OperationId, WorkerId)> {
        self.keepalives.lock().unwrap().clone()
    }
}

#[async_trait]
impl WorkerStateManager for RecordingStateManager {
    async fn update_operation(
        &self,
        operation_id: &OperationId,
        worker_id: &WorkerId,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        if matches!(update, UpdateOperationType::KeepAlive) {
            self.keepalives
                .lock()
                .unwrap()
                .push((operation_id.clone(), worker_id.clone()));
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

#[nativelink_test]
async fn keepalive_refreshes_running_operations_once_per_interval() -> Result<(), Error> {
    let state_manager = Arc::new(RecordingStateManager::default());
    let scheduler = ApiWorkerScheduler::new(
        state_manager.clone(),
        Arc::new(PlatformPropertyManager::new(HashMap::new())),
        WorkerAllocationStrategy::default(),
        None,
        None,
        Arc::new(Notify::new()),
        WORKER_TIMEOUT_S,
        60,
        0,
        TOUCH_INTERVAL_S,
        Arc::new(WorkerRegistry::new()),
        None,
        false,
        Duration::from_secs(15),
    );

    let worker_id = WorkerId::from(String::from("worker"));
    let (tx, mut rx) = mpsc::channel(8);
    scheduler
        .add_worker(Worker::new(
            worker_id.clone(),
            PlatformProperties::default(),
            tx,
            NOW,
            0,
        ))
        .await?;

    let operation_id = OperationId::default();
    scheduler
        .worker_notify_run_action(
            worker_id.clone(),
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
    // The dispatch itself.
    rx.recv()
        .await
        .expect("the worker should be told to run the action");

    // Inside the interval the keepalive refreshes nothing.
    scheduler
        .worker_keep_alive_received(&worker_id, NOW + TOUCH_INTERVAL_S / 2, None)
        .await?;
    assert_eq!(state_manager.keepalives(), vec![]);

    // Past it, every running operation is refreshed once.
    scheduler
        .worker_keep_alive_received(&worker_id, NOW + TOUCH_INTERVAL_S + 1, None)
        .await?;
    assert_eq!(
        state_manager.keepalives(),
        vec![(operation_id.clone(), worker_id.clone())]
    );

    // The clock restarts from that refresh, not from the keepalive before.
    scheduler
        .worker_keep_alive_received(&worker_id, NOW + TOUCH_INTERVAL_S + 50, None)
        .await?;
    assert_eq!(state_manager.keepalives().len(), 1);
    scheduler
        .worker_keep_alive_received(&worker_id, NOW + 2 * TOUCH_INTERVAL_S + 2, None)
        .await?;
    assert_eq!(state_manager.keepalives().len(), 2);
    Ok(())
}
