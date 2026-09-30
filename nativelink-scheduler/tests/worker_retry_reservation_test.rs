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

use std::collections::HashMap;
use std::time::{Duration, UNIX_EPOCH};

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::worker::{ActionInfoWithProps, Worker, WorkerUpdate};
use nativelink_util::action_messages::{OperationId, WorkerId};
use nativelink_util::common::DigestInfo;
use nativelink_util::origin_event::OriginMetadata;
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};
use pretty_assertions::assert_eq;
use tokio::sync::mpsc;

mod utils {
    pub(crate) mod scheduler_utils;
}
use utils::scheduler_utils::make_base_action_info;

const NOW_TIME: u64 = 10_000;

fn minimum(mem: u64) -> PlatformProperties {
    PlatformProperties::new(HashMap::from([(
        "mem".to_string(),
        PlatformPropertyValue::Minimum(mem),
    )]))
}

fn action_needing(mem: u64) -> ActionInfoWithProps {
    ActionInfoWithProps {
        inner: make_base_action_info(
            UNIX_EPOCH + Duration::from_secs(NOW_TIME),
            DigestInfo::new([7; 32], 512),
        ),
        platform_properties: minimum(mem),
        origin_metadata: OriginMetadata::default(),
        scheduler_start_execute_event_id: None,
    }
}

fn mem_of(props: &PlatformProperties) -> u64 {
    match props.properties.get("mem") {
        Some(PlatformPropertyValue::Minimum(v)) => *v,
        _ => panic!("expected a Minimum 'mem' property"),
    }
}

/// N2 regression: a same-worker retry (the earlier attempt of the same op is
/// still resident on the worker) must not double-deduct or leak the worker's
/// resource budget.
///
/// WITHOUT THE FIX: `Worker::run_action` unconditionally re-runs
/// `reduce_platform_properties` and overwrites the op-keyed entry, so the
/// worker's `mem` budget is deducted a SECOND time (10 -> 6 -> 2) even though
/// the op already holds one reservation. Since completion (`complete_action`)
/// only refunds the single surviving entry, those extra 4 units are leaked
/// forever. This test asserts that re-dispatching an op the worker already
/// holds does not further reduce the budget — it FAILS on the unfixed code
/// (asserts 6, finds 2).
#[nativelink_test]
async fn same_worker_retry_conserves_resource_budget() -> Result<(), Error> {
    let (tx, _rx) = mpsc::channel(64);
    let mut worker = Worker::new(
        WorkerId("w".to_string()),
        minimum(10),
        tx,
        NOW_TIME,
        // Two inflight slots so a retry has spare capacity while the first
        // attempt is still resident.
        2,
    );

    let op = OperationId::default();

    // First dispatch of op reserves 4 of the worker's 10 mem.
    worker
        .notify_update(WorkerUpdate::RunAction(Box::new((
            op.clone(),
            action_needing(4),
            NOW_TIME,
        ))))
        .await?;
    assert_eq!(
        mem_of(&worker.platform_properties),
        6,
        "first dispatch should reserve 4 of 10"
    );

    // Same op dispatched again to the SAME worker while the earlier attempt is
    // still resident (same-worker retry). The worker must not create a second
    // live reservation for an op it already holds.
    let retry_result = worker
        .notify_update(WorkerUpdate::RunAction(Box::new((
            op.clone(),
            action_needing(4),
            NOW_TIME,
        ))))
        .await;
    // The worker may accept or reject the redundant dispatch; either way the
    // invariant below (no double reservation) must hold.
    drop(retry_result);

    // Exactly one live reservation should exist for the op.
    assert_eq!(
        worker.running_action_infos.len(),
        1,
        "op must have exactly one live reservation on the worker"
    );

    // The op still holds exactly one reservation, so the budget must be
    // unchanged from the first dispatch. A second deduction here is a leak:
    // completion only refunds the single surviving entry.
    assert_eq!(
        mem_of(&worker.platform_properties),
        6,
        "re-dispatching an already-held op must not deduct the budget again"
    );
    Ok(())
}
