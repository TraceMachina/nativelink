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

use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use async_lock::Mutex;
use futures::{StreamExt, future};
use lru::LruCache;
use nativelink_config::schedulers::{MemoryEscalationSpec, WorkerAllocationStrategy};
use nativelink_error::{Code, Error, ResultExt, error_if, make_err, make_input_err};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
    RootMetricsComponent, group,
};
use nativelink_proto::com::github::trace_machina::nativelink::events::{
    Event, OriginEvent, ResponseEvent, event, response_event,
};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    ActionResourceUsage, ResourceOutcome, WorkerLoad,
};
use nativelink_util::action_messages::{ActionStage, OperationId, WorkerId};
use nativelink_util::metrics::{
    WorkerDisconnectReason, record_dispatch_requeue, record_execution_cpu_time,
    record_execution_peak_memory, record_worker_connected, record_worker_disconnected,
    record_worker_keepalive, record_worker_keepalive_gap, record_worker_state,
};
use nativelink_util::operation_state_manager::{
    Decline, Escalation, UpdateOperationType, WorkerStateManager,
};
use nativelink_util::origin_event::get_node_id;
use nativelink_util::platform_properties::{PlatformProperties, PlatformPropertyValue};
use nativelink_util::shutdown_guard::ShutdownGuard;
use tokio::sync::{Notify, mpsc};
use tonic::async_trait;
use tracing::{debug, error, info, trace, warn};

/// How many state-manager lookups `kill_revoked_operations` has in flight
/// at once while checking which running operations were revoked.
const MAX_CONCURRENT_REVOKED_CHECKS: usize = 32;

/// How many of the action's `priority` properties the worker carries with
/// the same value. A priority property never restricts placement; it says
/// where the action would rather be.
fn priority_matches(action: &PlatformProperties, worker: &Worker) -> usize {
    action
        .properties
        .iter()
        .filter(|(name, value)| {
            matches!(value, PlatformPropertyValue::Priority(_))
                && worker.platform_properties.properties.get(*name) == Some(*value)
        })
        .count()
}

/// How much of the worker's advertised capacity would be left after this
/// action, summed over the action's `minimum` properties, in thousandths.
/// Lower is a tighter fit. An action with no numbers fits every worker
/// equally, and a dimension the worker does not advertise is ignored.
fn fit_leftover_permille(action: &PlatformProperties, worker: &Worker) -> u64 {
    let mut leftover = 0u64;
    for (name, value) in &action.properties {
        let PlatformPropertyValue::Minimum(needed) = value else {
            continue;
        };
        let (
            Some(PlatformPropertyValue::Minimum(available)),
            Some(PlatformPropertyValue::Minimum(total)),
        ) = (
            worker.platform_properties.properties.get(name),
            worker.total_platform_properties.properties.get(name),
        )
        else {
            continue;
        };
        if *total == 0 {
            continue;
        }
        let remaining = available.saturating_sub(*needed);
        leftover += remaining.saturating_mul(1000) / total;
    }
    leftover
}

/// How many property shapes `static_verdicts` holds before it is emptied.
const MAX_STATIC_VERDICTS: usize = 4096;

use uuid::Uuid;

/// Metrics for tracking scheduler performance.
#[derive(Debug, Default)]
pub struct SchedulerMetrics {
    /// Total number of worker additions.
    pub workers_added: AtomicU64,
    /// Total number of worker removals.
    pub workers_removed: AtomicU64,
    /// Total number of `find_worker_for_action` calls.
    pub find_worker_calls: AtomicU64,
    /// Total number of successful worker matches.
    pub find_worker_hits: AtomicU64,
    /// Total number of failed worker matches (no worker found).
    pub find_worker_misses: AtomicU64,
    /// Total time spent in `find_worker_for_action` (nanoseconds).
    pub find_worker_time_ns: AtomicU64,
    /// Total number of workers iterated during find operations.
    pub workers_iterated: AtomicU64,
    /// Total number of action dispatches.
    pub actions_dispatched: AtomicU64,
    /// Total number of keep-alive updates.
    pub keep_alive_updates: AtomicU64,
    /// Total number of worker timeouts.
    pub worker_timeouts: AtomicU64,
    /// Dispatches the worker declined, requeued untried.
    pub dispatches_declined: AtomicU64,
    /// Dispatches that found the worker's channel full, requeued untried.
    pub dispatch_channel_full: AtomicU64,
    /// Dispatches requeued because the worker never acknowledged them.
    pub dispatches_unacknowledged: AtomicU64,
}

use crate::match_outcome::{
    MatchOutcome, PropertyShape, UnsatisfiableReason, explain_unsatisfiable,
};
use crate::platform_property_manager::PlatformPropertyManager;
use crate::worker::{ActionInfoWithProps, Worker, WorkerTimestamp, WorkerUpdate};
use crate::worker_capability_index::WorkerCapabilityIndex;
use crate::worker_registry::SharedWorkerRegistry;
use crate::worker_scheduler::{WorkerScheduler, WorkerSummary};

#[derive(Debug)]
struct Workers(LruCache<WorkerId, Worker>);

impl Deref for Workers {
    type Target = LruCache<WorkerId, Worker>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for Workers {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

// Note: This could not be a derive macro because this derive-macro
// does not support LruCache and nameless field structs.
impl MetricsComponent for Workers {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        let _enter = group!("workers").entered();
        for (worker_id, worker) in self.iter() {
            let _enter = group!(worker_id).entered();
            worker.publish(MetricKind::Component, MetricFieldData::default())?;
        }
        Ok(MetricPublishKnownKindData::Component)
    }
}

/// A collection of workers that are available to run tasks.
#[derive(MetricsComponent)]
struct ApiWorkerSchedulerImpl {
    /// A `LruCache` of workers available based on `allocation_strategy`.
    #[metric(group = "workers")]
    workers: Workers,

    /// The worker state manager.
    #[metric(group = "worker_state_manager")]
    worker_state_manager: Arc<dyn WorkerStateManager>,
    /// The allocation strategy for workers.
    allocation_strategy: WorkerAllocationStrategy,
    /// The `minimum` property compared against each worker's reported free
    /// memory before placement; unset means no veto.
    live_memory_veto: Option<String>,
    /// How a memory kill is turned into a retry with a larger reservation;
    /// unset means the kill fails the action.
    memory_escalation: Option<MemoryEscalationSpec>,
    /// A channel to notify the matching engine that the worker pool has changed.
    worker_change_notify: Arc<Notify>,
    /// Bumped with every notification, under the same lock as placement, so
    /// a matching pass can tell that room opened while it was walking the
    /// queue and offer that room to the oldest waiting action first.
    capacity_generation: Arc<AtomicU64>,
    /// Worker registry for tracking worker liveness.
    worker_registry: SharedWorkerRegistry,

    /// Whether the worker scheduler is shutting down.
    shutting_down: bool,

    /// Index for fast worker capability lookup.
    /// Used to accelerate `find_worker_for_action` by filtering candidates
    /// based on properties before doing linear scan.
    capability_index: WorkerCapabilityIndex,

    /// Incremented when the set of distinct worker shapes changes, here or
    /// on a peer. A worker joining or leaving with a shape the fleet already
    /// has does not count: the unsatisfiable tracker keys its clock on this,
    /// and a pool that scales up and down with identical workers used to
    /// reset that clock every time, so an action no shape could ever run
    /// waited forever behind a fleet that never stopped moving.
    fleet_generation: u64,
    /// The distinct shapes behind `fleet_generation`, local and peer.
    fleet_shape_set: BTreeSet<PropertyShape>,

    /// What the workers connected to other schedulers on the same state
    /// registered with, one entry per distinct shape, sorted by shape so
    /// that an unchanged fleet compares equal. `None` until the peers have
    /// been read, and whenever reading them fails: an action is then never
    /// judged unsatisfiable, since a peer might have a worker for it.
    peer_fleet: Option<Vec<PlatformProperties>>,

    /// Whether this scheduler shares its state with peers. A non-shared db is
    /// the whole fleet, so peer-absence is authoritative at once; a shared one
    /// applies the census warm-up / staleness checks below before it trusts
    /// peer-absence to fail an action.
    shared: bool,

    /// One peer-record TTL, used for both the census warm-up and staleness
    /// bounds below.
    record_ttl: Duration,

    /// When the peer census last became known (a `None` -> `Some` exchange).
    /// Peer-absence is only trusted to fail an action once it has been known
    /// for `record_ttl`, so a peer publishing on its own cadence has had time
    /// to appear. Cleared whenever the census goes unknown.
    peer_fleet_known_since: Option<SystemTime>,

    /// When the peer census was last refreshed by a successful exchange. If the
    /// exchange task dies or stalls this stops advancing, and after `record_ttl`
    /// the census is treated as stale and peer-absence is no longer trusted
    /// (fail open). Cleared whenever the census goes unknown.
    peer_fleet_last_refresh: Option<SystemTime>,

    /// Woken when a worker joins or leaves this scheduler, so peers hear
    /// about it without waiting for the next periodic exchange.
    local_fleet_change_notify: Arc<Notify>,

    /// Whether an action with a given property shape could run on some
    /// connected worker when that worker is idle. `None` means it could. This
    /// depends only on what workers registered with, so it holds until the
    /// fleet changes, at which point the map is emptied.
    static_verdicts: HashMap<PropertyShape, Option<Arc<UnsatisfiableReason>>>,
}

/// Whether peer-absence can be trusted to fail an action, given the census
/// state and the current time. A free function so its edge cases (warm-up,
/// staleness, clock skew, db kind) can be unit-tested without a scheduler.
fn peer_census_trusted(
    shared: bool,
    peer_fleet_known: bool,
    peer_fleet_known_since: Option<SystemTime>,
    peer_fleet_last_refresh: Option<SystemTime>,
    record_ttl: Duration,
    now: SystemTime,
) -> bool {
    // Unknown (a shared db before its first exchange or after a failed one): a
    // peer we cannot see might run the action. Checked before the non-shared
    // short-circuit so a deliberately-unknown census is never trusted.
    if !peer_fleet_known {
        return false;
    }
    // A non-shared db is the whole fleet; peer-absence is authoritative at
    // once, with no warm-up or staleness delay.
    if !shared {
        return true;
    }
    let (Some(known_since), Some(last_refresh)) = (peer_fleet_known_since, peer_fleet_last_refresh)
    else {
        return false;
    };
    // Warm: the census must have been known for a full record TTL, so a peer
    // publishing on its own ~interval cadence has had time to appear. A clock
    // going backwards (Err) is treated as not-yet-warm: don't fail.
    let warm = now
        .duration_since(known_since)
        .is_ok_and(|elapsed| elapsed >= record_ttl);
    // Fresh: a successful exchange within the last record TTL. If the exchange
    // task died or stalled this goes stale and we stop trusting peer-absence
    // (fail open). A future-dated refresh (clock skew) counts as fresh.
    let fresh = now
        .duration_since(last_refresh)
        .map_or(true, |elapsed| elapsed <= record_ttl);
    warm && fresh
}

impl core::fmt::Debug for ApiWorkerSchedulerImpl {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ApiWorkerSchedulerImpl")
            .field("workers", &self.workers)
            .field("allocation_strategy", &self.allocation_strategy)
            .field("worker_change_notify", &self.worker_change_notify)
            .field(
                "capability_index_size",
                &self.capability_index.worker_count(),
            )
            .field("worker_registry", &self.worker_registry)
            .finish_non_exhaustive()
    }
}

impl ApiWorkerSchedulerImpl {
    /// Refreshes the lifetime of the worker with the given timestamp.
    ///
    /// Instead of sending N keepalive messages (one per operation),
    /// we now send a single worker heartbeat. The worker registry tracks worker liveness,
    /// and timeout detection checks the worker's `last_seen` instead of per-operation timestamps.
    ///
    /// Note: This only updates the local worker state. The worker registry is updated
    /// separately after releasing the inner lock to reduce contention.
    fn refresh_lifetime(
        &mut self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
        load: Option<WorkerLoad>,
        keepalive: bool,
    ) -> Result<(), Error> {
        let worker = self.workers.0.peek_mut(worker_id).ok_or_else(|| {
            make_input_err!(
                "Worker not found in worker map in refresh_lifetime() {}",
                worker_id
            )
        })?;
        error_if!(
            worker.last_update_timestamp > timestamp,
            "Worker already had a timestamp of {}, but tried to update it with {}",
            worker.last_update_timestamp,
            timestamp
        );
        record_worker_keepalive_gap(timestamp.saturating_sub(worker.last_update_timestamp));
        worker.last_update_timestamp = timestamp;
        // Any other message (an acknowledgement, a decline, an execute
        // result) proves the worker is alive and nothing more. A decline in
        // particular is not the worker saying it is ready to be asked
        // again, so the pause it took stands until a real keepalive.
        if !keepalive {
            return Ok(());
        }
        // A keepalive without a load (an older worker) leaves the last
        // report in place. A report with
        // more room than the last one can make a vetoed action eligible
        // again; less room never does.
        let more_room = load.is_some_and(|load| {
            let more = worker
                .last_load
                .as_ref()
                .is_none_or(|last| load.free_memory_kb > last.free_memory_kb);
            worker.last_load = Some(load);
            more
        });
        // A keepalive is the worker saying it is ready to be asked again,
        // so a pause from backpressure lasts one keepalive interval. A
        // pause taken for a decline for load is the exception: it waits
        // for a keepalive whose free memory, read above, covers what the
        // declined action wanted. A worker that reports no load is taken
        // at its word.
        let load_fits = match (worker.pause_needs_kb, worker.last_load) {
            (Some(needs_kb), Some(load)) => load.free_memory_kb >= needs_kb,
            _ => true,
        };
        let resumed = worker.is_paused && load_fits;
        if resumed {
            worker.is_paused = false;
            worker.pause_needs_kb = None;
            record_worker_state("paused", false);
        }
        // Either is room the fleet did not have a moment ago, so the
        // capacity generation moves and the matcher is woken, together:
        // a pass in flight offers the room to its parked actions first,
        // and a pass not in flight starts. `capacity_changed` by hand,
        // since `worker` still borrows the map.
        if resumed || more_room {
            self.capacity_generation.fetch_add(1, Ordering::Relaxed);
            self.worker_change_notify.notify_one();
        }

        trace!(
            ?worker_id,
            running_operations = worker.running_action_infos.len(),
            "Worker keepalive received"
        );
        record_worker_keepalive();

        Ok(())
    }

    /// Adds a worker to the pool.
    /// Note: This function will not do any task matching.
    fn add_worker(&mut self, worker: Worker) -> Result<(), Error> {
        let worker_id = worker.id.clone();
        let platform_properties = worker.platform_properties.clone();
        // A replacement is one out and one in, so the gauge only moves for a
        // genuinely new worker.
        let replaced = self.workers.put(worker_id.clone(), worker);

        // Add to capability index for fast matching
        self.capability_index
            .add_worker(&worker_id, &platform_properties);
        self.fleet_changed();
        self.local_fleet_change_notify.notify_one();

        // Worker is not cloneable, and we do not want to send the initial connection results until
        // we have added it to the map, or we might get some strange race conditions due to the way
        // the multi-threaded runtime works.
        let worker = self.workers.peek_mut(&worker_id).unwrap();
        let res = worker
            .send_initial_connection_result(self.live_memory_veto.as_deref())
            .err_tip(|| "Failed to send initial connection result to worker");
        if let Err(err) = &res {
            error!(
                ?worker_id,
                ?err,
                "Worker connection appears to have been closed while adding to pool"
            );
        }
        if let Some(replaced) = &replaced {
            if replaced.is_draining {
                record_worker_state("draining", false);
            }
            if replaced.is_paused {
                record_worker_state("paused", false);
            }
        } else {
            record_worker_connected();
        }
        self.capacity_changed();
        res
    }

    /// Removes worker from pool.
    /// Note: The caller is responsible for any rescheduling of any tasks that might be
    /// running.
    fn remove_worker(&mut self, worker_id: &WorkerId) -> Option<Worker> {
        // Remove from capability index
        self.capability_index.remove_worker(worker_id);
        // Pop before recomputing the shapes, or the departing worker still
        // counts and a cached verdict that only it satisfied survives it.
        let result = self.workers.pop(worker_id);
        self.fleet_changed();
        self.local_fleet_change_notify.notify_one();

        self.capacity_changed();
        result
    }

    fn fleet_changed(&mut self) {
        let mut shapes: BTreeSet<PropertyShape> = self
            .workers
            .iter()
            .map(|(_, w)| PropertyShape::from(&w.total_platform_properties))
            .collect();
        if let Some(peer_fleet) = &self.peer_fleet {
            shapes.extend(peer_fleet.iter().map(PropertyShape::from));
        }
        if shapes == self.fleet_shape_set {
            return;
        }
        self.fleet_shape_set = shapes;
        self.fleet_generation += 1;
        self.static_verdicts.clear();
    }

    /// Something about the fleet's room to run actions changed: a worker
    /// joined, left, paused, resumed, drained or finished an action.
    fn capacity_changed(&self) {
        self.capacity_generation.fetch_add(1, Ordering::Relaxed);
        self.worker_change_notify.notify_one();
    }

    /// The distinct properties this scheduler's workers registered with.
    fn fleet_shapes(&self) -> Vec<PlatformProperties> {
        let mut seen = HashSet::new();
        self.workers
            .iter()
            .map(|(_, w)| &w.total_platform_properties)
            .filter(|totals| seen.insert(PropertyShape::from(*totals)))
            .cloned()
            .collect()
    }

    fn set_peer_fleet(&mut self, peer_fleet: Option<Vec<PlatformProperties>>, now: SystemTime) {
        // Peers list their workers in no particular order and may share
        // shapes, so sort and dedup before comparing, or an unchanged fleet
        // would look changed on every exchange and empty `static_verdicts`.
        let peer_fleet = peer_fleet.map(|peer_fleet| {
            let mut keyed: Vec<_> = peer_fleet
                .into_iter()
                .map(|totals| (PropertyShape::from(&totals), totals))
                .collect();
            keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0));
            keyed.dedup_by(|a, b| a.0 == b.0);
            keyed.into_iter().map(|(_, totals)| totals).collect()
        });
        // Freshness bookkeeping: update the warm-up /
        // staleness clocks BEFORE the unchanged-shapes early return, so a
        // successful exchange that happens to return the same shapes still
        // refreshes the staleness clock (otherwise a steady fleet would look
        // like a stalled exchange task).
        if peer_fleet.is_some() {
            self.peer_fleet_last_refresh = Some(now);
            // Start the warm-up only on the unknown -> known transition.
            if self.peer_fleet.is_none() {
                self.peer_fleet_known_since = Some(now);
            }
        } else {
            self.peer_fleet_known_since = None;
            self.peer_fleet_last_refresh = None;
        }
        if self.peer_fleet == peer_fleet {
            return;
        }
        // Unknown peers make every verdict "maybe satisfiable elsewhere",
        // and that verdict is cached. Peers becoming known (or unknown
        // again) changes what a verdict means even when the union of shapes
        // does not move, so the cache goes, though the generation stays:
        // clocks already running were right and keep their start.
        if self.peer_fleet.is_some() != peer_fleet.is_some() {
            self.static_verdicts.clear();
        }
        self.peer_fleet = peer_fleet;
        self.fleet_changed();
    }

    /// Sets if the worker is draining or not.
    fn set_drain_worker(&mut self, worker_id: &WorkerId, is_draining: bool) -> Result<(), Error> {
        let worker = self
            .workers
            .get_mut(worker_id)
            .err_tip(|| format!("Worker {worker_id} doesn't exist in the pool"))?;
        if worker.is_draining != is_draining {
            record_worker_state("draining", is_draining);
        }
        worker.is_draining = is_draining;
        self.capacity_changed();
        Ok(())
    }

    /// Whether peer-absence can be trusted to fail an action right now — a
    /// shared census must be known, warm and fresh; a non-shared db is trusted
    /// as soon as it is known. See the free `peer_census_trusted`.
    fn peer_census_trusted(&self, now: SystemTime) -> bool {
        peer_census_trusted(
            self.shared,
            self.peer_fleet.is_some(),
            self.peer_fleet_known_since,
            self.peer_fleet_last_refresh,
            self.record_ttl,
            now,
        )
    }

    /// Works out whether any of the `candidates` the capability index picked
    /// could run an action with these properties when idle, and if not, why.
    fn static_verdict(
        &self,
        platform_properties: &PlatformProperties,
        candidates: &HashSet<WorkerId>,
        full_worker_logging: bool,
    ) -> Option<Arc<UnsatisfiableReason>> {
        // The index only checks that Minimum keys are present, so the values
        // are checked here against what each candidate registered with.
        let satisfiable = candidates.iter().any(|worker_id| {
            self.workers.peek(worker_id).is_some_and(|w| {
                platform_properties
                    .is_satisfied_by(&w.total_platform_properties, full_worker_logging)
            })
        });
        if satisfiable {
            return None;
        }
        // A peer's worker counts too: the action would run there once that
        // worker has room, and this scheduler cannot see how busy it is.
        // While the peers are unknown, any of them might have one.
        let peer_fleet = self.peer_fleet.as_deref()?;
        if peer_fleet
            .iter()
            .any(|totals| platform_properties.is_satisfied_by(totals, false))
        {
            return None;
        }
        let fleet: Vec<_> = self
            .workers
            .iter()
            .map(|(_, w)| &w.total_platform_properties)
            .chain(peer_fleet.iter())
            .collect();
        Some(Arc::new(explain_unsatisfiable(platform_properties, &fleet)))
    }

    /// `static_verdict`, cached per property shape until the fleet changes.
    fn cached_static_verdict(
        &mut self,
        platform_properties: &PlatformProperties,
        candidates: Option<&HashSet<WorkerId>>,
        full_worker_logging: bool,
    ) -> Option<Arc<UnsatisfiableReason>> {
        let shape = PropertyShape::from(platform_properties);
        // A cached verdict skips the per-property logging, so recompute when
        // that logging was asked for.
        if !full_worker_logging && let Some(verdict) = self.static_verdicts.get(&shape) {
            return verdict.clone();
        }
        let verdict = if let Some(candidates) = candidates {
            self.static_verdict(platform_properties, candidates, full_worker_logging)
        } else {
            let candidates = self
                .capability_index
                .find_matching_workers(platform_properties, full_worker_logging);
            self.static_verdict(platform_properties, &candidates, full_worker_logging)
        };
        if self.static_verdicts.len() >= MAX_STATIC_VERDICTS {
            self.static_verdicts.clear();
        }
        self.static_verdicts.insert(shape, verdict.clone());
        verdict
    }

    /// Finds an idle worker among `candidates` that can run the action now.
    fn find_available_worker(
        &self,
        platform_properties: &PlatformProperties,
        candidates: &HashSet<WorkerId>,
        full_worker_logging: bool,
    ) -> Option<WorkerId> {
        // Check function for availability AND dynamic Minimum property verification.
        // The index only does presence checks for Minimum properties since their
        // values change dynamically as jobs are assigned to workers.
        let worker_matches = |(worker_id, w): &(&WorkerId, &Worker)| -> bool {
            if !w.can_accept_work() {
                if full_worker_logging {
                    info!(
                        "Worker {worker_id} cannot accept work: is_paused={}, is_draining={}, inflight={}/{}",
                        w.is_paused,
                        w.is_draining,
                        w.running_action_infos.len(),
                        w.max_inflight_tasks
                    );
                }
                return false;
            }

            // The ledger only knows what actions declared; the worker's own
            // report of what it has left catches the ones that declared too
            // little. A worker that reports nothing is never vetoed.
            // An action that asks for the worker's whole advertised memory
            // (the last escalation step) is not vetoed: no worker ever
            // reports that much free, since its own binary, the kernel and
            // the page cache hold some, and the ledger already keeps it
            // alone there.
            if let (Some(property), Some(load)) = (self.live_memory_veto.as_deref(), w.last_load)
                && let Some(PlatformPropertyValue::Minimum(needed_kb)) =
                    platform_properties.properties.get(property)
                && *needed_kb > load.free_memory_kb
                && matches!(
                    w.total_platform_properties.properties.get(property),
                    Some(PlatformPropertyValue::Minimum(advertised_kb)) if *needed_kb < *advertised_kb
                )
            {
                if full_worker_logging {
                    info!(
                        "Worker {worker_id} vetoed for this action: it reports {} KiB free, the action asks {needed_kb} KiB ({property})",
                        load.free_memory_kb
                    );
                }
                return false;
            }

            // Verify Minimum properties at runtime (their values are dynamic)
            platform_properties.is_satisfied_by(&w.platform_properties, full_worker_logging)
        };

        // Now check constraints on filtered candidates.
        // Iterate in LRU order based on allocation strategy.
        let workers_iter = self.workers.iter();

        match self.allocation_strategy {
            // Use rfind to get the least recently used that satisfies the properties.
            WorkerAllocationStrategy::LeastRecentlyUsed => workers_iter
                .rev()
                .filter(|(worker_id, _)| candidates.contains(worker_id))
                .find(&worker_matches)
                .map(|(_, w)| w.id.clone()),

            // Use find to get the most recently used that satisfies the properties.
            WorkerAllocationStrategy::MostRecentlyUsed => workers_iter
                .filter(|(worker_id, _)| candidates.contains(worker_id))
                .find(&worker_matches)
                .map(|(_, w)| w.id.clone()),

            // Fewest running actions wins; the scan runs from the least
            // recently used end so `min_by_key` keeps that one on a tie.
            WorkerAllocationStrategy::LeastLoaded => workers_iter
                .rev()
                .filter(|(worker_id, _)| candidates.contains(worker_id))
                .filter(&worker_matches)
                .min_by_key(|(_, w)| w.running_action_infos.len())
                .map(|(_, w)| w.id.clone()),

            WorkerAllocationStrategy::BestFit => workers_iter
                .rev()
                .filter(|(worker_id, _)| candidates.contains(worker_id))
                .filter(&worker_matches)
                .min_by_key(|(_, w)| {
                    (
                        // More matching priority properties first.
                        usize::MAX - priority_matches(platform_properties, w),
                        fit_leftover_permille(platform_properties, w),
                        w.running_action_infos.len(),
                    )
                })
                .map(|(_, w)| w.id.clone()),
        }
    }

    fn inner_find_worker_for_action(
        &mut self,
        platform_properties: &PlatformProperties,
        full_worker_logging: bool,
        now: SystemTime,
    ) -> MatchOutcome {
        if self.workers.is_empty() {
            if full_worker_logging {
                info!("No workers available to match!");
            }
            return MatchOutcome::NoWorkersConnected;
        }

        // Do a fast check to see if any workers are available at all for work allocation
        let candidates = if self.workers.iter().any(|(_, w)| w.can_accept_work()) {
            // Use capability index to get candidate workers that match STATIC properties
            // (Exact, Unknown) and have the required property keys (Priority, Minimum).
            // This reduces complexity from O(W × P) to O(P × log(W)) for exact properties.
            let candidates = self
                .capability_index
                .find_matching_workers(platform_properties, full_worker_logging);
            if let Some(worker_id) =
                self.find_available_worker(platform_properties, &candidates, full_worker_logging)
            {
                return MatchOutcome::Matched(worker_id);
            }
            Some(candidates)
        } else {
            if full_worker_logging {
                info!("All workers are fully allocated");
            }
            None
        };

        // Nothing can take the action now. Only then is it worth working out
        // whether anything ever could, so a match pays nothing for this.
        if let Some(reason) = self.cached_static_verdict(
            platform_properties,
            candidates.as_ref(),
            full_worker_logging,
        ) {
            // The intrinsic verdict (nobody could run it) is cached by shape;
            // the census freshness check is applied live here, OUTSIDE the
            // cache, so a verdict computed while the census was trusted can
            // never outlive a staleness transition (nor a warm-up one). On a
            // shared db, trust peer-absence to fail only once the census is
            // warm and fresh; otherwise a peer we cannot fully account for
            // might run it, so wait rather than fail.
            if !self.peer_census_trusted(now) {
                if full_worker_logging {
                    info!("No idle worker matched; peer census not yet trusted, waiting");
                }
                return MatchOutcome::WaitingForCapacity;
            }
            if full_worker_logging {
                info!(%reason, "No connected worker can ever run this action");
            }
            return MatchOutcome::Unsatisfiable(reason);
        }
        if full_worker_logging && candidates.is_some() {
            warn!("No workers matched!");
        }
        MatchOutcome::WaitingForCapacity
    }

    /// The largest memory reservation any connected worker could hold.
    /// The largest memory any connected worker advertises under `property`,
    /// and that worker's whole CPU under `cpu_property` (`None` when the
    /// property is empty or the worker does not advertise it). Reserving
    /// both is what runs an action alone there.
    fn fleet_largest_worker(&self, property: &str, cpu_property: &str) -> (u64, Option<u64>) {
        let minimum = |worker: &Worker, name: &str| match worker
            .total_platform_properties
            .properties
            .get(name)
        {
            Some(PlatformPropertyValue::Minimum(value)) => Some(*value),
            _ => None,
        };
        // A draining worker is leaving: escalating to it would strand the
        // action at a size the remaining fleet cannot serve.
        self.workers
            .iter()
            .filter(|(_, worker)| !worker.is_draining)
            .filter_map(|(_, worker)| {
                minimum(worker, property).map(|memory_kb| {
                    let cpu = if cpu_property.is_empty() {
                        None
                    } else {
                        minimum(worker, cpu_property)
                    };
                    (memory_kb, cpu)
                })
            })
            .max_by_key(|(memory_kb, _)| *memory_kb)
            .unwrap_or((0, None))
    }

    /// Turns a result that the worker's last usage report says was a memory
    /// kill into a requeue with a larger reservation, when escalation is
    /// configured and there is room to grow. Returns the update to apply
    /// and whether it was escalated.
    fn maybe_escalate(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        update: UpdateOperationType,
    ) -> (UpdateOperationType, bool) {
        let Some(policy) = &self.memory_escalation else {
            return (update, false);
        };
        let UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(result)) = &update
        else {
            return (update, false);
        };
        let Some(pending) = self
            .workers
            .peek(worker_id)
            .and_then(|worker| worker.running_action_infos.get(operation_id))
        else {
            return (update, false);
        };
        let Some(usage) = pending.last_usage.as_ref() else {
            return (update, false);
        };
        if usage.outcome != ResourceOutcome::KilledMemory as i32 {
            return (update, false);
        }
        // What it ran under: the worker's echo, else what the scheduler sent.
        let reserved_kb = usage
            .reserved
            .map(|r| r.memory_kb)
            .filter(|kb| *kb > 0)
            .or_else(|| {
                match pending
                    .action_info
                    .platform_properties
                    .properties
                    .get(&policy.property)
                {
                    Some(PlatformPropertyValue::Minimum(kb)) => Some(*kb),
                    _ => None,
                }
            })
            .unwrap_or(0);
        let (fleet_memory_kb, fleet_cpu) =
            self.fleet_largest_worker(&policy.property, &policy.cpu_property);
        // `max_kb` caps the fleet's ceiling; it cannot raise it past what a
        // worker declares, or the escalation lands where nothing can run.
        let ceiling_kb = if policy.max_kb > 0 {
            fleet_memory_kb.min(policy.max_kb)
        } else {
            fleet_memory_kb
        };
        if ceiling_kb == 0 {
            return (update, false);
        }
        // The last step reserves the largest worker whole: its memory and,
        // when the fleet advertises one, its CPU, so nothing shares the
        // worker with the action. A kill there is the end: no worker in
        // the fleet can run the action, and the client hears exactly that.
        let cpu_held = match pending
            .action_info
            .platform_properties
            .properties
            .get(&policy.cpu_property)
        {
            Some(PlatformPropertyValue::Minimum(cores)) => *cores,
            _ => 0,
        };
        let whole_worker_cpu = fleet_cpu.filter(|cores| *cores > 0);
        let already_whole =
            reserved_kb >= ceiling_kb && whole_worker_cpu.is_none_or(|cores| cpu_held >= cores);
        if already_whole {
            warn!(
                ?worker_id,
                ?operation_id,
                reserved_kb,
                ceiling_kb,
                "Action killed for memory with the largest worker reserved whole; no worker in the fleet can run it"
            );
            let mut failed = result.clone();
            let no_worker = make_err!(
                Code::FailedPrecondition,
                "Action was killed for memory at {reserved_kb} KiB with the largest worker in the fleet ({ceiling_kb} KiB) reserved for it alone; no worker can run it. It needs a bigger worker or less memory"
            );
            failed.error = Some(match result.error.clone() {
                Some(worker_error) => no_worker.merge(worker_error),
                None => no_worker,
            });
            return (
                UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(failed)),
                false,
            );
        }
        let base_kb = if reserved_kb > 0 {
            reserved_kb
        } else {
            usage.peak_memory_kb
        };
        if base_kb == 0 {
            // Nothing to grow from: no reservation and an unsampled kill.
            // Scaling zero would hand out the smallest steps the ladder or
            // the percent allow and burn the budget on them.
            warn!(
                ?worker_id,
                ?operation_id,
                "Action killed for memory with no reservation and no sampled peak; not escalating"
            );
            return (update, false);
        }
        // With a ladder the next class up; without one, scale. Past the
        // top class or the ceiling, the whole worker.
        let next = if policy.ladder_kb.is_empty() {
            Some(
                base_kb
                    .saturating_mul(policy.percent)
                    .checked_div(100)
                    .unwrap_or(base_kb)
                    .max(base_kb.saturating_add(1)),
            )
        } else {
            policy
                .ladder_kb
                .iter()
                .copied()
                .filter(|kb| *kb > base_kb)
                .min()
        };
        let value = next.map_or(ceiling_kb, |kb| kb.min(ceiling_kb));
        let cpu = if value >= ceiling_kb {
            whole_worker_cpu.map(|cores| (policy.cpu_property.clone(), cores))
        } else {
            None
        };
        let reason = result.error.clone().unwrap_or_else(|| {
            make_err!(
                Code::FailedPrecondition,
                "Action was killed for memory at {} KiB",
                usage.peak_memory_kb
            )
        });
        warn!(
            ?worker_id,
            ?operation_id,
            reserved_kb,
            peak_memory_kb = usage.peak_memory_kb,
            escalated_kb = value,
            ceiling_kb,
            property = %policy.property,
            "Action killed for memory; requeuing with a larger reservation"
        );
        (
            UpdateOperationType::UpdateWithEscalation(Escalation {
                property: policy.property.clone(),
                value,
                reason,
                cpu,
            }),
            true,
        )
    }

    async fn update_action(
        &mut self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        let worker = self.workers.get_mut(worker_id).err_tip(|| {
            format!("Worker {worker_id} does not exist in SimpleScheduler::update_action")
        })?;

        // Ensure the worker is supposed to be running the operation. A
        // result for something this worker no longer holds is stale, not
        // rogue: the operation was requeued after a timeout or a kill and
        // the worker finished it anyway. Refuse it and keep the worker;
        // evicting it here requeued every other action it held for one
        // late message.
        if !worker.running_action_infos.contains_key(operation_id) {
            warn!(
                %operation_id,
                ?worker_id,
                running_actions = worker.running_action_infos.len(),
                "Dropping update for an operation the worker is not running"
            );
            return Err(make_err!(
                Code::Internal,
                "Operation {operation_id} should not be running on worker {worker_id} in SimpleScheduler::update_action"
            ));
        }

        let (is_finished, due_to_backpressure) = match &update {
            UpdateOperationType::UpdateWithActionStage(action_stage) => {
                (action_stage.is_finished(), false)
            }
            UpdateOperationType::KeepAlive => (false, false),
            UpdateOperationType::UpdateWithError(err) => {
                (true, err.code == Code::ResourceExhausted)
            }
            // A decline pauses like backpressure: the worker said it has no
            // room, so it gets nothing more until it says otherwise.
            UpdateOperationType::UpdateWithDecline(_) => (true, true),
            UpdateOperationType::UpdateWithDisconnect
            | UpdateOperationType::UpdateWithEscalation(_) => (true, false),
            UpdateOperationType::ExecutionComplete => {
                // The process has exited but the action is still resident on
                // the worker until its result arrives, so nothing is released
                // here; `complete_action` returns the budget and the slot
                // together.
                return Ok(());
            }
        };

        // Update the operation in the worker state manager.
        let update_operation_res = if worker.is_kill_requested(operation_id) {
            debug!(
                %operation_id,
                ?worker_id,
                "Ignoring update for operation the worker was told to kill"
            );
            Ok(())
        } else {
            let update_operation_res = self
                .worker_state_manager
                .update_operation(operation_id, worker_id, update)
                .await
                .err_tip(|| "in update_operation on SimpleScheduler::update_action");
            if let Err(err) = &update_operation_res {
                error!(
                    %operation_id,
                    ?worker_id,
                    ?err,
                    "Failed to update_operation on update_action"
                );
            }
            update_operation_res
        };

        if !is_finished {
            return update_operation_res;
        }
        // The worker is done with this action even if the state-manager update
        // failed (e.g. the operation was already torn down after its clients
        // timed out). The worker bookkeeping below must still run, or the
        // worker's platform properties leak until it can never match again.

        // Clear this action from the current worker if finished.
        let complete_action_res = {
            // Note: We need to run this before dealing with backpressure logic.
            let was_paused = worker.is_paused;
            let complete_action_res = worker.complete_action(operation_id);

            // Backpressure pauses the worker even when it holds nothing
            // else: an idle worker that refuses work and is not paused is
            // offered the same action again at once, and the pair spin
            // until something changes. The next keepalive clears it.
            if due_to_backpressure || (!worker.can_accept_work() && worker.has_actions()) {
                worker.is_paused = true;
            }
            // complete_action clears is_paused on its way through, so compare
            // the state either side of it rather than testing the flag after.
            // Testing afterwards counts a re-pause every time and never
            // unwinds, which leaves the gauge climbing forever.
            if was_paused != worker.is_paused {
                record_worker_state("paused", worker.is_paused);
            }
            complete_action_res
        };

        self.capacity_changed();

        update_operation_res.merge(complete_action_res)
    }

    /// Notifies the specified worker to run the given action and handles errors by evicting
    /// the worker if the notification fails.
    async fn worker_notify_run_action(
        &mut self,
        worker_id: WorkerId,
        operation_id: OperationId,
        action_info: ActionInfoWithProps,
        dispatched_at: WorkerTimestamp,
    ) -> Result<(), Error> {
        if let Some(worker) = self.workers.get_mut(&worker_id) {
            let notify_worker_result = worker
                .notify_update(WorkerUpdate::RunAction(Box::new((
                    operation_id.clone(),
                    action_info.clone(),
                    dispatched_at,
                ))))
                .await;

            if let Err(notify_worker_result) = notify_worker_result {
                if notify_worker_result.code == Code::ResourceExhausted {
                    // The worker's queue is full: it has stopped reading, or
                    // it is far behind. Nothing was charged; the action goes
                    // back untried and the worker waits for its next
                    // keepalive before it is offered anything else.
                    warn!(
                        ?worker_id,
                        %operation_id,
                        "Worker channel full, requeuing dispatch and pausing the worker"
                    );
                    record_dispatch_requeue("channel_full");
                    if !worker.is_paused {
                        worker.is_paused = true;
                        record_worker_state("paused", true);
                    }
                    self.worker_change_notify.notify_one();
                    return self
                        .worker_state_manager
                        .update_operation(
                            &operation_id,
                            &worker_id,
                            UpdateOperationType::UpdateWithDecline(Decline {
                                reason: "channel_full".to_string(),
                            }),
                        )
                        .await
                        .err_tip(|| "requeuing a dispatch the worker's channel refused");
                }
                warn!(
                    ?worker_id,
                    ?action_info,
                    ?notify_worker_result,
                    "Worker command failed, removing worker",
                );

                // A slightly nasty way of figuring out that the worker disconnected
                // from send_msg_to_worker without introducing complexity to the
                // code path from here to there.
                let is_disconnect = notify_worker_result.code == Code::Internal
                    && notify_worker_result.messages.len() == 1
                    && notify_worker_result.messages[0] == "Worker Disconnected";

                let err = make_err!(
                    Code::Internal,
                    "Worker command failed, removing worker {worker_id} -- {notify_worker_result:?}",
                );

                let reason = if is_disconnect {
                    WorkerDisconnectReason::Disconnected
                } else {
                    WorkerDisconnectReason::Error
                };
                let evicted = self
                    .immediate_evict_worker(&worker_id, err.clone(), is_disconnect, reason)
                    .await;
                // The dispatch was never charged to the worker (the send
                // comes first), so the eviction did not see this operation;
                // send it back to the queue here, untried: nothing ran, so
                // no attempt is counted against it.
                let requeued = self
                    .worker_state_manager
                    .update_operation(
                        &operation_id,
                        &worker_id,
                        UpdateOperationType::UpdateWithDecline(Decline {
                            reason: "worker_disconnected".to_string(),
                        }),
                    )
                    .await;
                return Result::<(), _>::Err(err).merge(evicted).merge(requeued);
            }
            Ok(())
        } else {
            warn!(
                ?worker_id,
                %operation_id,
                ?action_info,
                "Worker not found in worker map in worker_notify_run_action"
            );
            // Ensure the operation is put back to queued state.
            self.worker_state_manager
                .update_operation(
                    &operation_id,
                    &worker_id,
                    UpdateOperationType::UpdateWithDisconnect,
                )
                .await
        }
    }

    /// Tells the worker to kill an operation it is still running but the
    /// state manager no longer has executing on it. A worker that cannot be
    /// reached is evicted, the same as for a failed run request.
    /// The worker acknowledged a dispatch. A late acknowledgement for an
    /// operation no longer on the worker is nothing to act on.
    fn dispatch_accepted(&mut self, worker_id: &WorkerId, operation_id: &OperationId) {
        let Some(worker) = self.workers.get_mut(worker_id) else {
            return;
        };
        if worker.mark_accepted(operation_id) {
            return;
        }
        // Messages from a worker arrive in order and the acknowledgement
        // comes before the run, so an acknowledgement for an operation not
        // on this worker means the sweep took the dispatch back already: the
        // worker is about to run an action that now belongs elsewhere. Tell
        // it to stop now, not at the next revoked-operation sweep.
        info!(
            ?worker_id,
            %operation_id,
            "Acknowledgement for a dispatch already taken back; telling the worker to stop it"
        );
        if let Err(err) = worker.kill_unknown_operation(operation_id) {
            warn!(?worker_id, %operation_id, ?err, "Could not tell the worker to stop the operation");
        }
    }

    /// The worker declined a dispatch: the ledger is restored, the action
    /// requeued untried, and the worker paused; for a decline for load the
    /// pause holds until the worker reports that much free memory.
    /// Returns whether there was a dispatch to take back. A decline for an
    /// operation the worker no longer holds (it finished, or the sweep
    /// already requeued it) is nothing to act on, like a late
    /// acknowledgement, and must not pause the worker.
    async fn dispatch_declined(
        &mut self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        reason: String,
        needs_kb: Option<u64>,
    ) -> Result<bool, Error> {
        let holds_it = self
            .workers
            .peek(worker_id)
            .is_some_and(|worker| worker.running_action_infos.contains_key(operation_id));
        if !holds_it {
            debug!(?worker_id, %operation_id, %reason, "Decline for an operation not on this worker");
            return Ok(false);
        }
        info!(?worker_id, %operation_id, %reason, ?needs_kb, "Worker declined dispatch");
        self.update_action(
            worker_id,
            operation_id,
            UpdateOperationType::UpdateWithDecline(Decline { reason }),
        )
        .await?;
        if let Some(worker) = self.workers.get_mut(worker_id) {
            if !worker.is_paused {
                worker.is_paused = true;
                record_worker_state("paused", true);
            }
            worker.pause_needs_kb = needs_kb;
        }
        Ok(true)
    }

    /// Dispatches this worker has not acknowledged within the timeout, as
    /// of its latest keepalive.
    fn unacknowledged_dispatches(
        &self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
        timeout_s: u64,
    ) -> Vec<OperationId> {
        self.workers
            .peek(worker_id)
            .map(|worker| worker.unacknowledged(timestamp.saturating_sub(timeout_s)))
            .unwrap_or_default()
    }

    async fn worker_notify_kill_operation(
        &mut self,
        worker_id: &WorkerId,
        operation_id: OperationId,
    ) -> Result<(), Error> {
        let Some(worker) = self.workers.get_mut(worker_id) else {
            // Gone between the snapshot and now; its actions were requeued.
            return Ok(());
        };
        // Already told, or finished in the meantime; nothing more to send.
        if !worker.running_action_infos.contains_key(&operation_id)
            || worker.is_kill_requested(&operation_id)
        {
            return Ok(());
        }
        info!(
            ?worker_id,
            %operation_id,
            "Killing operation the state manager no longer has executing on this worker"
        );
        if let Err(err) = worker
            .notify_update(WorkerUpdate::KillOperation(operation_id.clone()))
            .await
        {
            // A full channel is a worker that is behind, not one that is
            // gone. It keeps its place; the request is forgotten so the
            // next sweep sends the kill again.
            if err.code == Code::ResourceExhausted {
                worker.clear_kill_request(&operation_id);
                warn!(
                    ?worker_id,
                    %operation_id,
                    "Worker channel full, kill deferred to the next sweep"
                );
                return Ok(());
            }
            warn!(
                ?worker_id,
                %operation_id,
                ?err,
                "Worker command failed, removing worker"
            );
            let err = make_err!(
                Code::Internal,
                "Worker command failed, removing worker {worker_id} -- {err:?}",
            );
            return Result::<(), _>::Err(err.clone()).merge(
                self.immediate_evict_worker(
                    worker_id,
                    err,
                    true,
                    WorkerDisconnectReason::Disconnected,
                )
                .await,
            );
        }
        Ok(())
    }

    /// Evicts the worker from the pool and puts items back into the queue if anything was being executed on it.
    async fn immediate_evict_worker(
        &mut self,
        worker_id: &WorkerId,
        err: Error,
        is_disconnect: bool,
        reason: WorkerDisconnectReason,
    ) -> Result<(), Error> {
        let mut result = Ok(());
        if let Some(mut worker) = self.remove_worker(worker_id) {
            // Log every eviction here rather than in each caller, so a worker
            // can never leave the pool unexplained. Without this, a worker that
            // vanished mid-build left nothing on the scheduler side to
            // attribute it to, and the only visible symptom was the worker
            // reconnecting with a fresh id.
            info!(
                ?worker_id,
                is_disconnect,
                running_actions = worker.running_action_infos.len(),
                reason = %err.message_string(),
                "Evicting worker from pool"
            );
            record_worker_disconnected(reason, worker.is_draining, worker.is_paused);
            // We don't care if we fail to send message to worker, this is only a best attempt.
            drop(worker.notify_update(WorkerUpdate::Disconnect).await);
            // Whatever took the worker away (its stream ended, it timed
            // out, a command to it failed, an operator removed it), the
            // action running on it did nothing wrong: it is a worker loss
            // with its own budget, not an attempt the action spent.
            let update = UpdateOperationType::UpdateWithDisconnect;
            for (operation_id, _) in worker.running_action_infos.drain() {
                result = result.merge(
                    self.worker_state_manager
                        .update_operation(&operation_id, worker_id, update.clone())
                        .await,
                );
            }
        }
        // Note: Calling this many time is very cheap, it'll only trigger `do_try_match` once.
        // TODO(palfrey) This should be moved to inside the Workers struct.
        self.capacity_changed();
        result
    }
}

#[derive(Debug, MetricsComponent)]
pub struct ApiWorkerScheduler {
    #[metric]
    inner: Mutex<ApiWorkerSchedulerImpl>,
    /// See `ApiWorkerSchedulerImpl::capacity_generation`; read here without
    /// the lock.
    capacity_generation: Arc<AtomicU64>,
    #[metric(group = "platform_property_manager")]
    platform_property_manager: Arc<PlatformPropertyManager>,

    #[metric(
        help = "Timeout of how long to evict workers if no response in this given amount of time in seconds."
    )]
    worker_timeout_s: u64,
    #[metric(
        help = "How long a sent kill may go unacknowledged before the worker is evicted, in seconds."
    )]
    unacknowledged_kill_timeout_s: u64,
    #[metric(
        help = "How long a dispatch may go unacknowledged before it is requeued untried, in seconds; 0 is off."
    )]
    dispatch_ack_timeout_s: u64,
    /// Shared worker registry for checking worker liveness.
    worker_registry: SharedWorkerRegistry,

    /// Performance metrics for observability.
    metrics: Arc<SchedulerMetrics>,

    /// Channel for publishing origin events such as worker-observed action
    /// resource usage. `None` when origin events are disabled.
    maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,

    /// Woken when a worker joins or leaves this scheduler.
    local_fleet_change_notify: Arc<Notify>,
}

impl ApiWorkerScheduler {
    #[expect(clippy::too_many_arguments)]
    pub fn new(
        worker_state_manager: Arc<dyn WorkerStateManager>,
        platform_property_manager: Arc<PlatformPropertyManager>,
        allocation_strategy: WorkerAllocationStrategy,
        live_memory_veto: Option<String>,
        memory_escalation: Option<MemoryEscalationSpec>,
        worker_change_notify: Arc<Notify>,
        worker_timeout_s: u64,
        unacknowledged_kill_timeout_s: u64,
        dispatch_ack_timeout_s: u64,
        worker_registry: SharedWorkerRegistry,
        maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,
        has_peers: bool,
        record_ttl: Duration,
    ) -> Arc<Self> {
        debug_assert!(
            !record_ttl.is_zero(),
            "record_ttl must be non-zero; a zero TTL degenerates the census warm-up/staleness checks"
        );
        let local_fleet_change_notify = Arc::new(Notify::new());
        let capacity_generation = Arc::new(AtomicU64::new(0));
        Arc::new(Self {
            inner: Mutex::new(ApiWorkerSchedulerImpl {
                workers: Workers(LruCache::unbounded()),
                worker_state_manager,
                allocation_strategy,
                live_memory_veto,
                memory_escalation,
                worker_change_notify,
                capacity_generation: capacity_generation.clone(),
                worker_registry: worker_registry.clone(),
                shutting_down: false,
                capability_index: WorkerCapabilityIndex::new(),
                fleet_generation: 0,
                fleet_shape_set: BTreeSet::new(),
                // Without peers there is nothing to wait for.
                peer_fleet: (!has_peers).then(Vec::new),
                shared: has_peers,
                record_ttl,
                peer_fleet_known_since: None,
                peer_fleet_last_refresh: None,
                local_fleet_change_notify: local_fleet_change_notify.clone(),
                static_verdicts: HashMap::new(),
            }),
            platform_property_manager,
            worker_timeout_s,
            unacknowledged_kill_timeout_s,
            dispatch_ack_timeout_s,
            worker_registry,
            metrics: Arc::new(SchedulerMetrics::default()),
            maybe_origin_event_tx,
            local_fleet_change_notify,
            capacity_generation,
        })
    }

    /// Whether no worker is connected here and, as far as a trusted census
    /// says, to any peer either. False while the census cannot be trusted,
    /// so a peer we cannot see is assumed to have workers.
    pub async fn no_worker_anywhere(&self, now: SystemTime) -> bool {
        let inner = self.inner.lock().await;
        inner.workers.is_empty()
            && inner.peer_census_trusted(now)
            && inner.peer_fleet.as_ref().is_some_and(Vec::is_empty)
    }

    /// The capacity generation as of now; see `find_worker_for_action_observed`.
    pub fn capacity_generation(&self) -> u64 {
        self.capacity_generation.load(Ordering::Relaxed)
    }

    /// Returns a reference to the worker registry.
    pub const fn worker_registry(&self) -> &SharedWorkerRegistry {
        &self.worker_registry
    }

    /// `dispatched_at` is the scheduler's clock at the send; the
    /// acknowledgement timeout counts from it.
    pub async fn worker_notify_run_action(
        &self,
        worker_id: WorkerId,
        operation_id: OperationId,
        action_info: ActionInfoWithProps,
        dispatched_at: WorkerTimestamp,
    ) -> Result<(), Error> {
        self.metrics
            .actions_dispatched
            .fetch_add(1, Ordering::Relaxed);
        let mut inner = self.inner.lock().await;
        inner
            .worker_notify_run_action(worker_id, operation_id, action_info, dispatched_at)
            .await
    }

    /// A keepalive, or another message standing in for one. Only a
    /// keepalive carries a load and lifts a pause; anything else refreshes
    /// the timestamp and runs the acknowledgement sweep.
    async fn refresh_worker(
        &self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
        load: Option<WorkerLoad>,
        keepalive: bool,
    ) -> Result<(), Error> {
        {
            let mut inner = self.inner.lock().await;
            inner
                .refresh_lifetime(worker_id, timestamp, load, keepalive)
                .err_tip(|| "Error refreshing lifetime in worker_keep_alive_received()")?;
            if self.dispatch_ack_timeout_s > 0 {
                let stale = inner.unacknowledged_dispatches(
                    worker_id,
                    timestamp,
                    self.dispatch_ack_timeout_s,
                );
                for operation_id in stale {
                    warn!(
                        ?worker_id,
                        %operation_id,
                        timeout_s = self.dispatch_ack_timeout_s,
                        "Dispatch never acknowledged, requeuing untried"
                    );
                    match inner
                        .dispatch_declined(
                            worker_id,
                            &operation_id,
                            "unacknowledged".to_string(),
                            None,
                        )
                        .await
                    {
                        Ok(true) => {
                            self.metrics
                                .dispatches_unacknowledged
                                .fetch_add(1, Ordering::Relaxed);
                            record_dispatch_requeue("unacknowledged");
                        }
                        Ok(false) => {}
                        Err(err) => {
                            warn!(?worker_id, %operation_id, ?err, "Could not requeue unacknowledged dispatch");
                        }
                    }
                }
            }
        }
        let now = UNIX_EPOCH + Duration::from_secs(timestamp);
        self.worker_registry
            .update_worker_heartbeat(worker_id, now)
            .await;
        Ok(())
    }

    pub async fn running_action_info(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
    ) -> Option<ActionInfoWithProps> {
        let inner = self.inner.lock().await;
        inner
            .workers
            .peek(worker_id)
            .and_then(|worker| worker.running_action_infos.get(operation_id))
            .map(|pending_action_info| pending_action_info.action_info.clone())
    }

    /// Returns the scheduler metrics for observability.
    #[must_use]
    pub const fn get_metrics(&self) -> &Arc<SchedulerMetrics> {
        &self.metrics
    }

    /// A counter that changes every time a worker joins or leaves.
    pub async fn fleet_generation(&self) -> u64 {
        self.inner.lock().await.fleet_generation
    }

    /// The distinct properties this scheduler's workers registered with.
    pub async fn fleet_shapes(&self) -> Vec<PlatformProperties> {
        self.inner.lock().await.fleet_shapes()
    }

    /// Records what the workers connected to peer schedulers can run, or
    /// `None` when that is not known.
    pub async fn set_peer_fleet(
        &self,
        peer_fleet: Option<Vec<PlatformProperties>>,
        now: SystemTime,
    ) {
        self.inner.lock().await.set_peer_fleet(peer_fleet, now);
    }

    /// Woken when a worker joins or leaves this scheduler.
    #[must_use]
    pub const fn local_fleet_change_notify(&self) -> &Arc<Notify> {
        &self.local_fleet_change_notify
    }

    /// Attempts to find a worker that is capable of running this action.
    // TODO(palfrey) This algorithm is not very efficient. Simple testing using a tree-like
    // structure showed worse performance on a 10_000 worker * 7 properties * 1000 queued tasks
    // simulation of worst cases in a single threaded environment.
    pub async fn find_worker_for_action(
        &self,
        platform_properties: &PlatformProperties,
        full_worker_logging: bool,
        now: SystemTime,
    ) -> MatchOutcome {
        self.find_worker_for_action_observed(platform_properties, full_worker_logging, now)
            .await
            .0
    }

    /// `find_worker_for_action`, also returning the capacity generation read
    /// under the same lock as the placement. A pass compares it with the one
    /// it saw last: a change means a worker gained room since, and an older
    /// action that found none earlier in the pass should be offered that room
    /// before this one takes it.
    pub async fn find_worker_for_action_observed(
        &self,
        platform_properties: &PlatformProperties,
        full_worker_logging: bool,
        now: SystemTime,
    ) -> (MatchOutcome, u64) {
        let start = Instant::now();
        self.metrics
            .find_worker_calls
            .fetch_add(1, Ordering::Relaxed);

        let mut inner = self.inner.lock().await;
        let worker_count = inner.workers.len() as u64;
        let result =
            inner.inner_find_worker_for_action(platform_properties, full_worker_logging, now);
        let generation = inner.capacity_generation.load(Ordering::Relaxed);

        // Track workers iterated (worst case is all workers)
        self.metrics
            .workers_iterated
            .fetch_add(worker_count, Ordering::Relaxed);

        if matches!(result, MatchOutcome::Matched(_)) {
            self.metrics
                .find_worker_hits
                .fetch_add(1, Ordering::Relaxed);
        } else {
            self.metrics
                .find_worker_misses
                .fetch_add(1, Ordering::Relaxed);
        }

        #[allow(clippy::cast_possible_truncation)]
        self.metrics
            .find_worker_time_ns
            .fetch_add(start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        (result, generation)
    }

    /// Checks to see if the worker exists in the worker pool. Should only be used in unit tests.
    #[must_use]
    pub async fn contains_worker_for_test(&self, worker_id: &WorkerId) -> bool {
        let inner = self.inner.lock().await;
        inner.workers.contains(worker_id)
    }

    /// A unit test function used to send the keep alive message to the worker from the server.
    pub async fn send_keep_alive_to_worker_for_test(
        &self,
        worker_id: &WorkerId,
    ) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        let worker = inner.workers.get_mut(worker_id).ok_or_else(|| {
            make_input_err!("WorkerId '{}' does not exist in workers map", worker_id)
        })?;
        worker.keep_alive()
    }
}

#[async_trait]
impl WorkerScheduler for ApiWorkerScheduler {
    fn get_platform_property_manager(&self) -> &PlatformPropertyManager {
        self.platform_property_manager.as_ref()
    }

    async fn record_action_resource_usage(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        mut resource_usage: ActionResourceUsage,
    ) -> Result<(), Error> {
        // The worker API talks to this `ApiWorkerScheduler` (it is the
        // `WorkerScheduler` returned by `SimpleScheduler::new`), so the
        // resource-usage origin event must be published here. Previously the
        // only override lived on `SimpleScheduler`, which this path never
        // reaches, so the event was silently dropped by the trait's no-op
        // default and `observed_worker_peak_memory_mib` was never recorded.
        // Sampling is optional, so only record when the worker actually took a
        // reading. A zero here means "not sampled", not "used no memory".
        {
            // The result follows this report; what it says decides whether
            // that result is a kill to escalate.
            let mut inner = self.inner.lock().await;
            if let Some(pending) = inner
                .workers
                .get_mut(worker_id)
                .and_then(|worker| worker.running_action_infos.get_mut(operation_id))
            {
                pending.last_usage = Some(resource_usage.clone());
            }
        }
        let maybe_action_info = self.running_action_info(worker_id, operation_id).await;
        let action_mnemonic = maybe_action_info
            .as_ref()
            .and_then(|action_info| action_info.origin_metadata.bazel_metadata.as_ref())
            .map_or_else(String::new, |bazel_metadata| {
                bazel_metadata.action_mnemonic.clone()
            });

        let outcome =
            ResourceOutcome::try_from(resource_usage.outcome).unwrap_or(ResourceOutcome::Unknown);
        if !matches!(
            outcome,
            ResourceOutcome::Unknown | ResourceOutcome::Completed
        ) {
            warn!(
                ?worker_id,
                ?operation_id,
                outcome = outcome.as_str_name(),
                enforced = resource_usage.enforced,
                action_mnemonic,
                peak_memory_kb = resource_usage.peak_memory_kb,
                wall_time_ms = resource_usage.wall_time_ms,
                reserved = ?resource_usage.reserved,
                "Action ended by a kill"
            );
        }

        if resource_usage.sampled {
            if resource_usage.peak_memory_kb > 0 {
                record_execution_peak_memory(
                    resource_usage.peak_memory_kb,
                    "",
                    &action_mnemonic,
                    outcome.as_str_name(),
                );
            }
            if resource_usage.cpu_time_ms > 0 {
                record_execution_cpu_time(
                    resource_usage.cpu_time_ms,
                    "",
                    &action_mnemonic,
                    outcome.as_str_name(),
                );
            }
        }

        let Some(origin_event_tx) = self.maybe_origin_event_tx.as_ref() else {
            return Ok(());
        };
        let Some(action_info) = maybe_action_info else {
            return Ok(());
        };

        if resource_usage.operation_id.is_empty() {
            resource_usage.operation_id = operation_id.to_string();
        }
        if resource_usage.worker_id.is_empty() {
            resource_usage.worker_id = worker_id.to_string();
        }

        let event = Event {
            event: Some(event::Event::Response(ResponseEvent {
                event: Some(response_event::Event::ActionResourceUsage(resource_usage)),
            })),
        };
        let origin_event = OriginEvent {
            version: 0,
            event_id: Uuid::now_v6(&get_node_id(Some(&event)))
                .hyphenated()
                .to_string(),
            parent_event_id: action_info
                .scheduler_start_execute_event_id
                .clone()
                .unwrap_or_default(),
            bazel_request_metadata: action_info.origin_metadata.bazel_metadata.clone(),
            identity: action_info.origin_metadata.identity,
            event: Some(event),
        };
        // Awaited send (not try_send): apply backpressure when the publisher
        // queue is full instead of silently dropping the resource-usage event,
        // which is what drives action-level resource sizing in the UI.
        if let Err(err) = origin_event_tx.send(origin_event).await {
            warn!(?err, "Failed to publish action resource usage origin event");
        }
        Ok(())
    }

    async fn add_worker(&self, worker: Worker) -> Result<(), Error> {
        let worker_id = worker.id.clone();
        let worker_timestamp = worker.last_update_timestamp;
        let mut inner = self.inner.lock().await;
        if inner.shutting_down {
            warn!("Rejected worker add during shutdown: {}", worker_id);
            return Err(make_err!(
                Code::Unavailable,
                "Received request to add worker while shutting down"
            ));
        }
        let result = inner
            .add_worker(worker)
            .err_tip(|| "Error while adding worker, removing from pool");
        if let Err(err) = result {
            return Result::<(), _>::Err(err.clone()).merge(
                inner
                    .immediate_evict_worker(&worker_id, err, false, WorkerDisconnectReason::Error)
                    .await,
            );
        }

        let now = UNIX_EPOCH + Duration::from_secs(worker_timestamp);
        self.worker_registry.register_worker(&worker_id, now).await;

        self.metrics.workers_added.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    async fn update_action(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        let (update, escalated) = inner.maybe_escalate(worker_id, operation_id, update);
        if escalated {
            record_dispatch_requeue("memory_escalation");
        }
        inner.update_action(worker_id, operation_id, update).await
    }

    async fn worker_dispatch_accepted(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
    ) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        inner.dispatch_accepted(worker_id, operation_id);
        Ok(())
    }

    async fn worker_dispatch_declined(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        reason: String,
        needs_kb: Option<u64>,
    ) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        // Counted once the action is back in the queue, not per message.
        if inner
            .dispatch_declined(worker_id, operation_id, reason, needs_kb)
            .await?
        {
            self.metrics
                .dispatches_declined
                .fetch_add(1, Ordering::Relaxed);
            record_dispatch_requeue("declined");
        }
        Ok(())
    }

    async fn worker_keep_alive_received(
        &self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
        load: Option<WorkerLoad>,
    ) -> Result<(), Error> {
        self.refresh_worker(worker_id, timestamp, load, true).await
    }

    async fn worker_liveness_refreshed(
        &self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
    ) -> Result<(), Error> {
        self.refresh_worker(worker_id, timestamp, None, false).await
    }

    async fn remove_worker(&self, worker_id: &WorkerId) -> Result<(), Error> {
        self.worker_registry.remove_worker(worker_id).await;

        let mut inner = self.inner.lock().await;
        inner
            .immediate_evict_worker(
                worker_id,
                make_err!(Code::Internal, "Received request to remove worker"),
                false,
                WorkerDisconnectReason::Removed,
            )
            .await
    }

    async fn worker_disconnected(&self, worker_id: &WorkerId) -> Result<(), Error> {
        self.worker_registry.remove_worker(worker_id).await;

        let mut inner = self.inner.lock().await;
        inner
            .immediate_evict_worker(
                worker_id,
                make_err!(Code::Unavailable, "Worker stream ended without going away"),
                true,
                WorkerDisconnectReason::Disconnected,
            )
            .await
    }

    async fn shutdown(&self, shutdown_guard: ShutdownGuard) {
        let mut inner = self.inner.lock().await;
        inner.shutting_down = true; // should reject further worker registration
        while let Some(worker_id) = inner
            .workers
            .peek_lru()
            .map(|(worker_id, _worker)| worker_id.clone())
        {
            if let Err(err) = inner
                .immediate_evict_worker(
                    &worker_id,
                    make_err!(Code::Internal, "Scheduler shutdown"),
                    true,
                    WorkerDisconnectReason::Shutdown,
                )
                .await
            {
                error!(?err, "Error evicting worker on shutdown.");
            }
        }
        drop(shutdown_guard);
    }

    async fn remove_timedout_workers(&self, now_timestamp: WorkerTimestamp) -> Result<(), Error> {
        // Check worker liveness using both the local timestamp (from LRU)
        // and the worker registry. A worker is alive if either source says it's alive.
        let timeout = Duration::from_secs(self.worker_timeout_s);
        let now = UNIX_EPOCH + Duration::from_secs(now_timestamp);
        let timeout_threshold = now_timestamp.saturating_sub(self.worker_timeout_s);

        let workers_to_check: Vec<(WorkerId, bool, bool)> = {
            let inner = self.inner.lock().await;
            inner
                .workers
                .iter()
                .map(|(worker_id, worker)| {
                    let local_alive = worker.last_update_timestamp > timeout_threshold;
                    let kill_overdue = worker.running_action_infos.values().any(|info| {
                        info.kill_requested_at.is_some_and(|at| {
                            now_timestamp.saturating_sub(at) > self.unacknowledged_kill_timeout_s
                        })
                    });
                    (worker_id.clone(), local_alive, kill_overdue)
                })
                .collect()
        };

        let mut worker_ids_to_remove = Vec::new();
        for (worker_id, local_alive, kill_overdue) in workers_to_check {
            // A healthy worker acknowledges a kill in moments; one that
            // cannot is wedged, and its keepalives keep the liveness checks
            // below from ever firing while the dead operation holds its
            // slot (the nativelink#2672 symptom). Evicting requeues its
            // other operations.
            if kill_overdue {
                warn!(
                    ?worker_id,
                    unacknowledged_kill_timeout_s = self.unacknowledged_kill_timeout_s,
                    "Worker did not acknowledge a kill in time, removing from pool"
                );
                worker_ids_to_remove.push((worker_id, true));
                continue;
            }

            if local_alive {
                continue;
            }

            let registry_alive = self
                .worker_registry
                .is_worker_alive(&worker_id, timeout, now)
                .await;

            if !registry_alive {
                trace!(
                    ?worker_id,
                    local_alive,
                    registry_alive,
                    timeout_threshold,
                    "Worker timed out - neither local nor registry shows alive"
                );
                worker_ids_to_remove.push((worker_id, false));
            }
        }

        if worker_ids_to_remove.is_empty() {
            return Ok(());
        }

        let mut inner = self.inner.lock().await;
        let mut result = Ok(());

        for (worker_id, kill_overdue) in &worker_ids_to_remove {
            let err = if *kill_overdue {
                make_err!(
                    Code::Internal,
                    "Worker {worker_id} did not acknowledge a kill within {}s, removing from pool",
                    self.unacknowledged_kill_timeout_s
                )
            } else {
                warn!(?worker_id, "Worker timed out, removing from pool");
                make_err!(
                    Code::Internal,
                    "Worker {worker_id} timed out, removing from pool"
                )
            };
            let reason = if *kill_overdue {
                WorkerDisconnectReason::KillUnacknowledged
            } else {
                WorkerDisconnectReason::Timeout
            };
            result = result.merge(
                inner
                    .immediate_evict_worker(worker_id, err, false, reason)
                    .await,
            );
        }

        result
    }

    async fn worker_snapshot(&self) -> Vec<WorkerSummary> {
        fn as_strings(properties: &PlatformProperties) -> HashMap<String, String> {
            properties
                .properties
                .iter()
                .map(|(name, value)| {
                    let value = match value {
                        PlatformPropertyValue::Minimum(v) => v.to_string(),
                        PlatformPropertyValue::Exact(v)
                        | PlatformPropertyValue::Priority(v)
                        | PlatformPropertyValue::Ignore(v)
                        | PlatformPropertyValue::Unknown(v) => v.clone(),
                    };
                    (name.clone(), value)
                })
                .collect()
        }
        let inner = self.inner.lock().await;
        inner
            .workers
            .iter()
            .map(|(_, worker)| WorkerSummary {
                id: worker.id.to_string(),
                running_actions: u32::try_from(worker.running_action_infos.len())
                    .unwrap_or(u32::MAX),
                max_inflight_tasks: worker.max_inflight_tasks,
                is_paused: worker.is_paused,
                is_draining: worker.is_draining,
                last_update_timestamp: worker.last_update_timestamp,
                platform_properties: as_strings(&worker.total_platform_properties),
                available_platform_properties: as_strings(&worker.platform_properties),
                free_memory_kb: worker.last_load.map(|load| load.free_memory_kb),
            })
            .collect()
    }

    async fn set_drain_worker(&self, worker_id: &WorkerId, is_draining: bool) -> Result<(), Error> {
        let mut inner = self.inner.lock().await;
        inner.set_drain_worker(worker_id, is_draining)
    }

    async fn kill_revoked_operations(&self) -> Result<(), Error> {
        let (worker_state_manager, running) = {
            let inner = self.inner.lock().await;
            let running: Vec<(WorkerId, OperationId)> = inner
                .workers
                .iter()
                .flat_map(|(worker_id, worker)| {
                    worker
                        .running_action_infos
                        .iter()
                        // An operation already told to die is never swept
                        // again; the kill itself is not re-sent. A worker
                        // that received the kill but never reports back is
                        // evicted by remove_timedout_workers once the kill
                        // has gone unacknowledged longer than
                        // `unacknowledged_kill_timeout_s`; a dead worker by
                        // the ordinary keepalive timeout.
                        .filter(|(_, pending_action_info)| {
                            pending_action_info.kill_requested_at.is_none()
                        })
                        .map(|(operation_id, _)| (worker_id.clone(), operation_id.clone()))
                })
                .collect();
            (inner.worker_state_manager.clone(), running)
        };

        // On store-backed deployments each check is a network round-trip,
        // so run them lock-free with bounded concurrency.
        let revoked: Vec<(WorkerId, OperationId)> = futures::stream::iter(running)
            .map(|(worker_id, operation_id)| {
                let worker_state_manager = worker_state_manager.clone();
                async move {
                    match worker_state_manager
                        .is_executing_on_worker(&operation_id, &worker_id)
                        .await
                    {
                        Ok(true) => None,
                        Ok(false) => Some((worker_id, operation_id)),
                        // Only kill on positive evidence; try again next pass.
                        Err(err) => {
                            warn!(
                                ?worker_id,
                                %operation_id,
                                ?err,
                                "Could not check whether operation is still executing on worker"
                            );
                            None
                        }
                    }
                }
            })
            .buffer_unordered(MAX_CONCURRENT_REVOKED_CHECKS)
            .filter_map(future::ready)
            .collect()
            .await;

        if revoked.is_empty() {
            return Ok(());
        }

        // Re-check lock-free so the scheduler mutex is never held across
        // store I/O; the checks above may be stale by the time we get here.
        // The remaining TOCTOU window is fine: worker_notify_kill_operation
        // re-guards with contains_key + is_kill_requested under the lock.
        let mut confirmed = Vec::new();
        for (worker_id, operation_id) in revoked {
            // A result of Ok(true) or Err(_) means the operation was
            // reassigned to this worker after the first check, or the state
            // manager went quiet: nothing to kill on this pass.
            let revoked = worker_state_manager
                .is_executing_on_worker(&operation_id, &worker_id)
                .await
                .is_ok_and(|executing| !executing);
            if revoked {
                confirmed.push((worker_id, operation_id));
            }
        }

        if confirmed.is_empty() {
            return Ok(());
        }

        let mut inner = self.inner.lock().await;
        let mut result = Ok(());
        for (worker_id, operation_id) in confirmed {
            result = result.merge(
                inner
                    .worker_notify_kill_operation(&worker_id, operation_id)
                    .await,
            );
        }
        result
    }
}

impl RootMetricsComponent for ApiWorkerScheduler {}

#[cfg(test)]
mod peer_census_trusted_tests {
    use super::{Duration, SystemTime, UNIX_EPOCH, peer_census_trusted};

    const TTL: Duration = Duration::from_secs(15);
    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(1000 + secs)
    }

    #[test]
    fn unknown_census_is_never_trusted() {
        // Even a non-shared db, if explicitly unknown, is not trusted.
        assert!(!peer_census_trusted(false, false, None, None, TTL, at(0)));
        assert!(!peer_census_trusted(
            true,
            false,
            Some(at(0)),
            Some(at(0)),
            TTL,
            at(100)
        ));
    }

    #[test]
    fn non_shared_known_is_trusted_immediately() {
        // The whole fleet is local; no warm-up or staleness applies.
        assert!(peer_census_trusted(false, true, None, None, TTL, at(0)));
    }

    #[test]
    fn shared_known_needs_both_timestamps() {
        assert!(!peer_census_trusted(true, true, None, None, TTL, at(100)));
        assert!(!peer_census_trusted(
            true,
            true,
            Some(at(0)),
            None,
            TTL,
            at(100)
        ));
    }

    #[test]
    fn shared_trusts_only_when_warm_and_fresh() {
        // Known at 0, refreshed at 100, evaluated at 100: warm and fresh.
        assert!(peer_census_trusted(
            true,
            true,
            Some(at(0)),
            Some(at(100)),
            TTL,
            at(100)
        ));
        // Exactly one TTL since known counts as warm.
        assert!(peer_census_trusted(
            true,
            true,
            Some(at(0)),
            Some(at(15)),
            TTL,
            at(15)
        ));
    }

    #[test]
    fn shared_not_warm_is_not_trusted() {
        // Known only half a TTL ago: a peer may not have published yet.
        assert!(!peer_census_trusted(
            true,
            true,
            Some(at(0)),
            Some(at(7)),
            TTL,
            at(7)
        ));
    }

    #[test]
    fn shared_stale_refresh_is_not_trusted() {
        // Warm, but the last successful exchange was more than a TTL ago (the
        // exchange task died/stalled): fail open.
        assert!(!peer_census_trusted(
            true,
            true,
            Some(at(0)),
            Some(at(50)),
            TTL,
            at(100)
        ));
    }

    #[test]
    fn future_dated_refresh_counts_as_fresh() {
        // Clock skew: a refresh timestamped ahead of `now` is treated as fresh
        // (the safe direction). Still requires warm.
        assert!(peer_census_trusted(
            true,
            true,
            Some(at(0)),
            Some(at(120)),
            TTL,
            at(100)
        ));
    }
}
