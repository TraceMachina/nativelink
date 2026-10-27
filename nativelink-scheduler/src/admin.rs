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
use nativelink_error::{Error, ResultExt, make_input_err};
use nativelink_util::action_messages::ActionStage;
use nativelink_util::metrics::record_awaited_action_orphan;
use nativelink_util::operation_state_manager::{
    ClientStateManager, OperationFilter, OperationStageFlags, OrderDirection,
};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::resource_sketch::{Family, ResourceSketch, SAMPLER_GENERATION, SketchKey};
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

/// Every queued action of the scheduler, highest priority first and then
/// oldest, as the store yields them.
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
        // On the Redis backend the reads below return the record as the
        // listing read it; on the memory one they are live, and the record
        // can have moved on or gone in between. One that is gone or cannot
        // be decoded is left out and counted, one no longer queued is no
        // longer demand, and a store failure is the listing's error.
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

/// Counts per sketch family.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct FamilyCounts {
    pub target: u64,
    pub mnemonic: u64,
    pub command: u64,
}

impl FamilyCounts {
    fn from_slice<T: Copy + Into<u64>>(values: [T; 3]) -> Self {
        Self {
            target: values[0].into(),
            mnemonic: values[1].into(),
            command: values[2].into(),
        }
    }
}

/// One hint of the listing, in the hints-file shape plus what the sketch
/// knows about it.
#[derive(Debug, Serialize)]
pub struct HintListing {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action_mnemonic: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
    #[serde(skip_serializing_if = "is_zero")]
    pub cpu_count: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub memory_kb: u64,
    #[serde(skip_serializing_if = "is_zero")]
    pub disk_kb: u64,
    pub samples: u32,
    pub last_seen_s: u64,
    /// `measured`, or `seed` for a hint from the file no attempt has
    /// replaced yet.
    pub source: &'static str,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if takes a reference"
)]
const fn is_zero(value: &u64) -> bool {
    *value == 0
}

/// The hints listing: a header with the sketch's size and the shaped hints,
/// most recently used first.
#[derive(Debug, Serialize)]
pub struct HintsListing {
    pub sampler_generation: u32,
    pub window_days: u64,
    pub headroom_percent: u64,
    pub entries: FamilyCounts,
    pub capacity: FamilyCounts,
    pub evictions: FamilyCounts,
    pub samples: FamilyCounts,
    pub probation: u64,
    pub hints: Vec<HintListing>,
}

/// The hints listing as the JSON body the admin API serves. Its `hints`
/// are a valid `hints_file`, so a new fleet can be seeded from an old one.
/// `family` narrows to `target`, `mnemonic` or `command`; `limit` caps the
/// rows, 100,000 by default.
pub fn hints_json(
    sketch: &ResourceSketch,
    family: Option<&str>,
    limit: Option<usize>,
) -> Result<String, Error> {
    let family = match family {
        None | Some("") => None,
        Some("target") => Some(Family::Target),
        Some("mnemonic") => Some(Family::Mnemonic),
        Some("command") => Some(Family::Command),
        Some(other) => {
            return Err(make_input_err!(
                "family must be target, mnemonic or command, not {other:?}"
            ));
        }
    };
    let stats = sketch.stats();
    let hints = sketch
        .hints(family, limit.unwrap_or(100_000))
        .into_iter()
        .map(|row| {
            let (target_id, action_mnemonic, command_digest) = match row.key {
                SketchKey::Target { target, mnemonic } => (
                    Some(target.to_string()),
                    mnemonic.map(|m| m.to_string()),
                    None,
                ),
                SketchKey::Mnemonic { mnemonic } => (None, Some(mnemonic.to_string()), None),
                SketchKey::Command { digest } => (None, None, Some(digest.to_string())),
            };
            HintListing {
                target_id,
                action_mnemonic,
                command_digest,
                class: row.shaped.class,
                cpu_count: row.shaped.cpu_count,
                memory_kb: row.shaped.memory_kb,
                disk_kb: row.shaped.disk_kb,
                samples: row.samples,
                last_seen_s: row.last_seen_s,
                source: if row.seeded { "seed" } else { "measured" },
            }
        })
        .collect();
    let listing = HintsListing {
        sampler_generation: SAMPLER_GENERATION,
        window_days: sketch.window().as_secs() / 86_400,
        headroom_percent: sketch.policy().headroom_percent,
        entries: FamilyCounts::from_slice(
            stats.entries.map(|n| u64::try_from(n).unwrap_or(u64::MAX)),
        ),
        capacity: FamilyCounts::from_slice(
            stats.capacity.map(|n| u64::try_from(n).unwrap_or(u64::MAX)),
        ),
        evictions: FamilyCounts::from_slice(stats.evictions),
        samples: FamilyCounts::from_slice(stats.samples),
        probation: u64::try_from(stats.probation).unwrap_or(u64::MAX),
        hints,
    };
    serde_json::to_string(&listing).map_err(|e| {
        Error::new(
            nativelink_error::Code::Internal,
            format!("Serializing the hints listing: {e}"),
        )
    })
}
