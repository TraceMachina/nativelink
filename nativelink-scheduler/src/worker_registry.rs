// Copyright 2024 Trace Machina, Inc. All rights reserved.
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

use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use std::time::SystemTime;

use async_lock::RwLock;
use async_trait::async_trait;
use jiff::Timestamp;
use nativelink_error::Error;
use nativelink_util::action_messages::WorkerId;
use nativelink_util::background_spawn;
use parking_lot::Mutex;
use tracing::{debug, trace, warn};

/// Ceiling for an action whose worker no instance here recognises. Long on
/// purpose: heartbeats never reach the shared state, so a healthy hour-long
/// action and an abandoned one look identical from here, and acting early
/// costs live work.
pub const ORPHANED_ACTION_TIMEOUT: Duration = Duration::from_hours(1);

/// What this scheduler instance knows about a worker's liveness.
///
/// A worker only appears in the registry of the instance holding its
/// `connect_worker` stream, so `Unknown` means "not mine to judge", not
/// "dead".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerLiveness {
    /// Registered here and has heartbeat within the timeout.
    Alive,
    /// Registered here, but has not been heard from within the timeout.
    Stale,
    /// Not registered here at all. Either connected to a different scheduler
    /// instance, or already evicted (in which case its actions were requeued
    /// at eviction time and no longer reference it).
    Unknown,
}

/// How long a read of a peer's worker lease is reused: a sweep asks about
/// the same worker once for every operation it holds.
const LEASE_READ_FRESHNESS: Duration = Duration::from_secs(5);

/// Past this many remembered lease reads, those not repeated within the
/// orphan backstop are forgotten, and all of them if that is not enough.
const MAX_LEASE_READS: usize = 10_000;

/// A lease's key expires in the store. Its renewal time only catches a key
/// left with no expiry, by a replica that died between writing the two, so
/// it allows this many lease periods for clocks that disagree.
const LEASE_TIME_SLACK: u32 = 3;

/// Leases on workers, in a store every scheduler replica shares. A replica
/// renews the lease of each worker connected to it; another replica reads
/// it to tell whether a worker it does not hold is alive.
#[async_trait]
pub trait WorkerLeases: Send + Sync + core::fmt::Debug {
    /// Marks each of `worker_ids` held by `scheduler_id`, renewed at `now`,
    /// for `ttl`.
    async fn renew(
        &self,
        scheduler_id: &str,
        worker_ids: &[WorkerId],
        now: SystemTime,
        ttl: Duration,
    ) -> Result<(), Error>;

    /// When the lease on `worker_id` was last renewed, if a replica holds
    /// one.
    async fn renewed_at(&self, worker_id: &WorkerId) -> Result<Option<SystemTime>, Error>;
}

#[derive(Debug, Clone, Copy)]
struct LeaseRead {
    read_at: SystemTime,
    held: bool,
    /// The first of the reads in a row that found the lease missing.
    missing_since: Option<SystemTime>,
}

#[derive(Debug, Default)]
struct LeaseState {
    last_renewal: Option<SystemTime>,
    renewing: bool,
    /// Recent reads of peers' worker leases.
    reads: HashMap<WorkerId, LeaseRead>,
}

/// In-memory worker registry that tracks worker liveness.
///
/// Per-process: fed by the `connect_worker` streams this instance owns, not a
/// view of every worker in the deployment.
#[derive(Debug)]
pub struct WorkerRegistry {
    workers: RwLock<HashMap<WorkerId, SystemTime>>,
    /// Set when the scheduler's state is shared with other replicas.
    leases: OnceLock<Arc<dyn WorkerLeases>>,
    /// Every live peer replica renews leases, so a missing lease means a
    /// dead worker. False until the peers have been read, and while any of
    /// them is an older replica that does not renew leases.
    peers_renew_leases: AtomicBool,
    lease_state: Arc<Mutex<LeaseState>>,
}

impl Default for WorkerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerRegistry {
    /// Creates a new worker registry.
    pub fn new() -> Self {
        Self {
            workers: RwLock::new(HashMap::new()),
            leases: OnceLock::new(),
            peers_renew_leases: AtomicBool::new(false),
            lease_state: Arc::new(Mutex::new(LeaseState::default())),
        }
    }

    /// Updates the heartbeat timestamp for a worker. A heartbeat never moves
    /// back: a message that waited to be processed carries the time it
    /// started waiting, and the worker has been heard from since.
    pub async fn update_worker_heartbeat(&self, worker_id: &WorkerId, now: SystemTime) {
        let mut workers = self.workers.write().await;
        let last_seen = workers.entry(worker_id.clone()).or_insert(now);
        if *last_seen < now {
            *last_seen = now;
        }
        trace!(?worker_id, now = %Timestamp::try_from(now).unwrap(), "FLOW: Worker heartbeat updated in registry");
    }

    /// Moves a registered worker's heartbeat forward to `now`, as its
    /// messages arrive. A message can arrive after the worker was removed,
    /// and before an earlier one has been processed, so this neither
    /// registers a worker nor moves a heartbeat back.
    pub async fn worker_heard_from(&self, worker_id: &WorkerId, now: SystemTime) {
        let mut workers = self.workers.write().await;
        if let Some(last_seen) = workers.get_mut(worker_id)
            && *last_seen < now
        {
            *last_seen = now;
        }
    }

    pub async fn register_worker(&self, worker_id: &WorkerId, now: SystemTime) {
        let mut workers = self.workers.write().await;
        workers.insert(worker_id.clone(), now);
        debug!(?worker_id, "FLOW: Worker registered in registry");
    }

    pub async fn remove_worker(&self, worker_id: &WorkerId) {
        let mut workers = self.workers.write().await;
        workers.remove(worker_id);
        debug!(?worker_id, "FLOW: Worker removed from registry");
    }

    /// Liveness as far as THIS instance can tell. Callers acting on "dead"
    /// must handle `Unknown` separately.
    pub async fn check_liveness(
        &self,
        worker_id: &WorkerId,
        timeout: Duration,
        now: SystemTime,
    ) -> WorkerLiveness {
        let workers = self.workers.read().await;

        let Some(last_seen) = workers.get(worker_id) else {
            trace!(?worker_id, "FLOW: Worker not registered on this instance");
            return WorkerLiveness::Unknown;
        };

        // An overflowing deadline can only be clock skew; treat as alive.
        let liveness = match last_seen.checked_add(timeout) {
            Some(deadline) if deadline > now => WorkerLiveness::Alive,
            Some(_) => WorkerLiveness::Stale,
            None => WorkerLiveness::Alive,
        };
        trace!(
            ?worker_id,
            last_seen = %Timestamp::try_from(*last_seen).unwrap_or_default(),
            ?timeout,
            ?liveness,
            "FLOW: Worker liveness check"
        );
        liveness
    }

    /// Gives the registry the worker leases it shares with peer replicas.
    pub fn set_leases(&self, leases: Arc<dyn WorkerLeases>) {
        if self.leases.set(leases).is_err() {
            debug!("Worker leases were already set");
        }
    }

    /// Whether every live peer replica renews worker leases, as the peers'
    /// fleet records say.
    pub fn set_peers_renew_leases(&self, all: bool) {
        self.peers_renew_leases.store(all, Ordering::Release);
    }

    /// Liveness of a worker connected here or to a peer replica. One
    /// connected here is judged on its heartbeat; another on the lease its
    /// replica renews every third of `timeout`. Held is `Alive`. Missing
    /// for a whole lease period is `Stale` once every live peer renews
    /// leases: a lease also lapses while the store refuses writes, and is
    /// renewed once they are taken again. `Unknown` otherwise, and without
    /// shared leases or when the lease cannot be read.
    pub async fn fleet_liveness(
        &self,
        worker_id: &WorkerId,
        timeout: Duration,
        now: SystemTime,
    ) -> WorkerLiveness {
        let local = self.check_liveness(worker_id, timeout, now).await;
        if local != WorkerLiveness::Unknown {
            return local;
        }
        let Some(leases) = self.leases.get() else {
            return WorkerLiveness::Unknown;
        };
        let previous = self.lease_state.lock().reads.get(worker_id).copied();
        let read = match previous {
            Some(read)
                if now
                    .duration_since(read.read_at)
                    .is_ok_and(|age| age < LEASE_READ_FRESHNESS) =>
            {
                read
            }
            _ => match leases.renewed_at(worker_id).await {
                Ok(renewed_at) => {
                    let held = renewed_at.is_some_and(|renewed_at| {
                        renewed_at
                            .checked_add(timeout.saturating_mul(LEASE_TIME_SLACK))
                            .is_none_or(|until| until > now)
                    });
                    let read = LeaseRead {
                        read_at: now,
                        held,
                        missing_since: if held {
                            None
                        } else {
                            previous.and_then(|read| read.missing_since).or(Some(now))
                        },
                    };
                    let mut state = self.lease_state.lock();
                    if state.reads.len() >= MAX_LEASE_READS {
                        state.reads.retain(|_, read| {
                            now.duration_since(read.read_at)
                                .is_ok_and(|age| age < ORPHANED_ACTION_TIMEOUT)
                        });
                        if state.reads.len() >= MAX_LEASE_READS {
                            state.reads.clear();
                        }
                    }
                    state.reads.insert(worker_id.clone(), read);
                    read
                }
                Err(err) => {
                    debug!(?worker_id, ?err, "Could not read a worker's lease");
                    return WorkerLiveness::Unknown;
                }
            },
        };
        if read.held {
            return WorkerLiveness::Alive;
        }
        let missing_for_a_period = read
            .missing_since
            .is_some_and(|since| now.duration_since(since).is_ok_and(|gap| gap >= timeout));
        if missing_for_a_period && self.peers_renew_leases.load(Ordering::Acquire) {
            WorkerLiveness::Stale
        } else {
            WorkerLiveness::Unknown
        }
    }

    /// Renews, for `timeout`, the lease of every worker connected here and
    /// heard from within `timeout`. Called every second; renews every third
    /// of `timeout`, so a live worker's lease does not lapse. The writes run
    /// in the background, so a slow store does not hold up the caller, and
    /// one renewal finishes before the next starts.
    pub async fn renew_leases(&self, scheduler_id: &str, timeout: Duration, now: SystemTime) {
        let Some(leases) = self.leases.get().cloned() else {
            return;
        };
        {
            let mut state = self.lease_state.lock();
            if state.renewing
                || state.last_renewal.is_some_and(|last| {
                    now.duration_since(last)
                        .is_ok_and(|since| since < timeout / 3)
                })
            {
                return;
            }
            state.last_renewal = Some(now);
        }
        let alive: Vec<WorkerId> = self
            .workers
            .read()
            .await
            .iter()
            .filter(|(_, last_seen)| {
                last_seen
                    .checked_add(timeout)
                    .is_none_or(|deadline| deadline > now)
            })
            .map(|(worker_id, _)| worker_id.clone())
            .collect();
        if alive.is_empty() {
            return;
        }
        self.lease_state.lock().renewing = true;
        let lease_state = self.lease_state.clone();
        let scheduler_id = scheduler_id.to_string();
        background_spawn!("worker_lease_renewal", async move {
            let result = leases.renew(&scheduler_id, &alive, now, timeout).await;
            let mut state = lease_state.lock();
            state.renewing = false;
            if let Err(err) = result {
                warn!(
                    ?err,
                    workers = alive.len(),
                    "Could not renew worker leases; peers cannot tell whether these workers are alive"
                );
                // Tried again on the next call rather than a third of the
                // timeout later.
                state.last_renewal = None;
            }
        });
    }

    pub async fn is_worker_alive(
        &self,
        worker_id: &WorkerId,
        timeout: Duration,
        now: SystemTime,
    ) -> bool {
        self.check_liveness(worker_id, timeout, now).await == WorkerLiveness::Alive
    }

    pub async fn get_worker_last_seen(&self, worker_id: &WorkerId) -> Option<SystemTime> {
        let workers = self.workers.read().await;
        workers.get(worker_id).copied()
    }
}

pub type SharedWorkerRegistry = Arc<WorkerRegistry>;

#[cfg(test)]
mod tests {
    use nativelink_macro::nativelink_test;

    use super::*;

    #[nativelink_test]
    async fn test_worker_heartbeat() {
        let registry = WorkerRegistry::new();
        let worker_id = WorkerId::from(String::from("test"));
        let now = SystemTime::now();

        // Worker not registered yet
        assert!(
            !registry
                .is_worker_alive(&worker_id, Duration::from_secs(5), now)
                .await
        );

        // Register worker
        registry.register_worker(&worker_id, now).await;
        assert!(
            registry
                .is_worker_alive(&worker_id, Duration::from_secs(5), now)
                .await
        );

        // Check with expired timeout
        let future = now.checked_add(Duration::from_secs(10)).unwrap();
        assert!(
            !registry
                .is_worker_alive(&worker_id, Duration::from_secs(5), future)
                .await
        );

        // Update heartbeat
        registry.update_worker_heartbeat(&worker_id, future).await;
        assert!(
            registry
                .is_worker_alive(&worker_id, Duration::from_secs(5), future)
                .await
        );
    }

    #[nativelink_test]
    async fn test_check_liveness_distinguishes_stale_from_unknown() {
        let registry = WorkerRegistry::new();
        let mine = WorkerId::from(String::from("mine"));
        let theirs = WorkerId::from(String::from("belongs-to-another-instance"));
        let now = SystemTime::now();
        let timeout = Duration::from_secs(5);

        // Never registered here. This is the case that must NOT read as dead:
        // with several schedulers on shared state it is a peer's worker.
        assert_eq!(
            registry.check_liveness(&theirs, timeout, now).await,
            WorkerLiveness::Unknown
        );

        registry.register_worker(&mine, now).await;
        assert_eq!(
            registry.check_liveness(&mine, timeout, now).await,
            WorkerLiveness::Alive
        );

        // Registered here but gone quiet: genuinely ours and genuinely dead.
        let later = now.checked_add(Duration::from_secs(10)).unwrap();
        assert_eq!(
            registry.check_liveness(&mine, timeout, later).await,
            WorkerLiveness::Stale
        );

        // Eviction takes it back to Unknown, not Stale.
        registry.remove_worker(&mine).await;
        assert_eq!(
            registry.check_liveness(&mine, timeout, later).await,
            WorkerLiveness::Unknown
        );
    }

    #[nativelink_test]
    async fn test_remove_worker() {
        let registry = WorkerRegistry::new();
        let worker_id = WorkerId::from(String::from("test-worker"));
        let now = SystemTime::now();

        registry.register_worker(&worker_id, now).await;
        assert!(
            registry
                .is_worker_alive(&worker_id, Duration::from_secs(5), now)
                .await
        );

        registry.remove_worker(&worker_id).await;
        assert!(
            !registry
                .is_worker_alive(&worker_id, Duration::from_secs(5), now)
                .await
        );
    }
}
