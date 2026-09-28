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
use std::time::UNIX_EPOCH;

use futures::StreamExt;
use nativelink_error::{Error, ResultExt};
use nativelink_util::operation_state_manager::{
    ClientStateManager, OperationFilter, OperationStageFlags,
};
use serde::{Deserialize, Serialize};

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
}

/// Every queued action of the scheduler, oldest first as the store yields
/// them.
pub async fn queued_demand(scheduler: &dyn ClientStateManager) -> Result<Vec<QueuedDemand>, Error> {
    let mut stream = scheduler
        .filter_operations(OperationFilter {
            stages: OperationStageFlags::Queued,
            ..Default::default()
        })
        .await
        .err_tip(|| "Listing queued operations for the admin API")?;
    let mut out = Vec::new();
    while let Some(result) = stream.next().await {
        let (state, _) = result.as_state().await?;
        let (action_info, _) = result.as_action_info().await?;
        out.push(QueuedDemand {
            operation_id: state.client_operation_id.to_string(),
            queued_since_ms: u64::try_from(
                action_info
                    .insert_timestamp
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis(),
            )
            .unwrap_or(u64::MAX),
            priority: action_info.priority,
            platform_properties: action_info.platform_properties.clone(),
        });
    }
    Ok(out)
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
