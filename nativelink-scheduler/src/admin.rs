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

//! Read-only views for the admin API: what the scheduler has queued and what
//! it has connected. A provisioner sizes pods from the first and drains
//! through the second, instead of reading the scheduler's store behind its
//! back.

use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::StreamExt;
use nativelink_error::{Error, ResultExt};
use nativelink_util::action_messages::ActionStage;
use nativelink_util::metrics::record_awaited_action_orphan;
use nativelink_util::operation_state_manager::{
    ClientStateManager, OperationFilter, OperationStageFlags, OrderDirection,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::simple_scheduler_state_manager::is_lost_record;
use crate::worker_scheduler::WorkerSummary;

/// A queued action as the admin API lists it: enough to size a pod for it
/// and to tell how long it has waited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedDemand {
    pub operation_id: String,
    /// Milliseconds since the epoch when the action was queued.
    pub queued_since_ms: u64,
    pub priority: i32,
    /// The action's properties as the scheduler matches them, reservations
    /// included.
    pub platform_properties: HashMap<String, String>,
    /// Milliseconds since the epoch when a client last checked in on the
    /// operation. A queue entry no client has touched for longer than the
    /// client timeout is one the scheduler will not dispatch.
    pub client_last_seen_ms: Option<u64>,
}

/// Every queued action of the scheduler, oldest first as the store yields
/// them.
pub async fn queued_demand(scheduler: &dyn ClientStateManager) -> Result<Vec<QueuedDemand>, Error> {
    let mut stream = scheduler
        .filter_operations(OperationFilter {
            stages: OperationStageFlags::Queued,
            // The Redis backend serves the queue in one direction only, the
            // matcher's: highest priority first, then oldest.
            order_by_priority_direction: Some(OrderDirection::Desc),
            ..Default::default()
        })
        .await
        .err_tip(|| "Listing queued operations for the admin API")?;
    let mut out = Vec::new();
    while let Some(result) = stream.next().await {
        // The filter read the record once; each read below is a fresh one,
        // and the record can have moved on or gone in between. One that is
        // gone or cannot be decoded is left out and counted, one that has
        // since been dispatched is no longer demand, and a store failure is
        // the listing's error.
        let Some((state, _)) = still_listed(result.as_state().await)? else {
            continue;
        };
        if !matches!(state.stage, ActionStage::Queued) {
            continue;
        }
        let Some((action_info, _)) = still_listed(result.as_action_info().await)? else {
            continue;
        };
        let Some(client_last_seen) = still_listed(result.client_last_seen().await)? else {
            continue;
        };
        let client_last_seen_ms = client_last_seen.map(epoch_ms);
        out.push(QueuedDemand {
            operation_id: state.client_operation_id.to_string(),
            queued_since_ms: epoch_ms(action_info.insert_timestamp),
            priority: action_info.priority,
            platform_properties: action_info.platform_properties.clone(),
            client_last_seen_ms,
        });
    }
    Ok(out)
}

/// A read of a listed record: the value, `None` for a record that is gone
/// or cannot be decoded (skipped and counted), or the store's error.
fn still_listed<T>(read: Result<T, Error>) -> Result<Option<T>, Error> {
    match read {
        Ok(value) => Ok(Some(value)),
        Err(err) if is_lost_record(&err) => {
            warn!(
                ?err,
                "Queued operation listed but its record cannot be read; left out of the demand"
            );
            record_awaited_action_orphan("admin");
            Ok(None)
        }
        Err(err) => Err(err).err_tip(|| "Reading a queued operation for the admin API"),
    }
}

fn epoch_ms(at: SystemTime) -> u64 {
    u64::try_from(
        at.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or(u64::MAX)
}

/// The demand listing as the JSON body the admin API serves.
pub async fn queued_demand_json(scheduler: &dyn ClientStateManager) -> Result<String, Error> {
    let demand = queued_demand(scheduler).await?;
    serde_json::to_string(&demand).map_err(|e| {
        Error::new(
            nativelink_error::Code::Internal,
            format!("Serializing the demand listing: {e}"),
        )
    })
}

/// The worker listing as the JSON body the admin API serves.
pub fn workers_json(workers: &[WorkerSummary]) -> Result<String, Error> {
    serde_json::to_string(workers).map_err(|e| {
        Error::new(
            nativelink_error::Code::Internal,
            format!("Serializing the worker listing: {e}"),
        )
    })
}
