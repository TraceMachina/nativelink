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

//! Deterministic simulation fuzzing of `SimpleScheduler`.
//!
//! The fuzz input decodes into a schedule of events — client submissions,
//! worker connect/disconnect, keepalives, dispatch acks, completions, and
//! virtual-clock advances — that is replayed against a real scheduler on a
//! mock clock inside a current-thread runtime. Coverage guidance then
//! searches the interleaving space that hand-written tests cannot cover.
//!
//! Two classes of finding:
//! - any panic inside the scheduler (libfuzzer catches it), and
//! - invariant violations checked by this harness, which panic explicitly:
//!   * REVOKE-KILL: an operation `StartAction`ed on a second worker while
//!     the first assignment is still live must resolve — the superseded
//!     worker must receive a kill/disconnect by the end of the drain,
//!     otherwise it is silent double execution (revocation is asynchronous
//!     by design, so this is checked eventually, not instantaneously), and
//!   * the drain epilogue: after all fuzz events, with one healthy worker
//!     connected and the clock advanced past every configured ceiling, every
//!     operation must reach a terminal stage — anything still `Executing` or
//!     `Queued` is a wedge (the "dead peer / preempted timeout / lost
//!     requeue" bug class).
//!
//! Determinism: single-threaded runtime, `MockClock` for time, no real I/O.
//! A crashing input replays exactly.

use core::time::Duration;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use arbitrary::{Arbitrary, Unstructured};
use mock_instant::thread_local::MockClock;
use nativelink_config::schedulers::SimpleSpec;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    UpdateForWorker, update_for_worker,
};
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionUniqueKey, ActionUniqueQualifier, OperationId, WorkerId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::operation_state_manager::{
    ClientStateManager, OperationFilter,
};
use nativelink_util::platform_properties::PlatformProperties;
use tokio::sync::{Notify, mpsc};

/// Base virtual time; far from zero so `duration_since` never underflows for
/// timestamps the harness fabricates. This is the ONE logical epoch: `run`
/// seeds `MockClock` itself with this base, so every observer — the worker
/// registry (fed `UNIX_EPOCH + worker_timestamp`), the state manager's
/// timeout checker (`MockInstantWrapped::now()` = `UNIX_EPOCH +
/// MockClock::time()`), and the harness's own `now()`/`now_ts()` — reads the
/// same instant. Do NOT add this constant to timestamps anywhere else.
const NOW_TIME_S: u64 = 100_000;

/// Scheduler timing knobs. Small values keep the drain epilogue's virtual
/// advance cheap; nonzero `max_action_executing_timeout_s` gives the drain a
/// ceiling to converge on even for pathological states.
const WORKER_TIMEOUT_S: u64 = 30;
/// Must be crossed (twice over) by the drain's total virtual advance: an
/// operation whose clients are all gone and whose worker is dead has ONLY
/// this ceiling left (the poll-driven timeout needs a live subscriber; the
/// abandoned-op backstop in `apply_filter_predicate` completes it with
/// `DeadlineExceeded` after `client_action_timeout`). The drain advances
/// 48 * (WORKER_TIMEOUT_S/2 + 1) = 768 virtual seconds, so keep this well
/// under half of that or the drain cannot certify liveness.
const CLIENT_ACTION_TIMEOUT_S: u64 = 120;
const MAX_EXECUTING_TIMEOUT_S: u64 = 120;
const MAX_JOB_RETRIES: u32 = 2;

/// The event vocabulary the fuzzer schedules. Worker/action indices are
/// small so schedules revisit the same entities often — races need contact.
#[derive(Debug, Arbitrary)]
pub enum Op {
    AddAction {
        key: u8,
        timeout_s: u8,
        skip_cache: bool,
    },
    ConnectWorker {
        w: u8,
        slots: u8,
    },
    KeepAlive {
        w: u8,
    },
    DispatchAccept {
        w: u8,
    },
    DispatchDecline {
        w: u8,
    },
    CompleteOk {
        w: u8,
    },
    CompleteRetryableErr {
        w: u8,
    },
    Disconnect {
        w: u8,
    },
    RemoveWorker {
        w: u8,
    },
    SetDrain {
        w: u8,
        draining: bool,
    },
    RemoveTimedoutWorkers,
    TryMatch,
    Advance {
        ms: u16,
    },
    DropClient {
        key: u8,
    },
}

fn now() -> SystemTime {
    UNIX_EPOCH + MockClock::time()
}

fn now_ts() -> u64 {
    MockClock::time().as_secs()
}

/// H1 self-check: the two production time readers must agree on "now".
/// The registry sweep path reconstructs `UNIX_EPOCH + now_ts()` from worker
/// timestamps; the operation-timeout checker asks
/// `MockInstantWrapped::now()`. If these drift, the same worker can be alive
/// to one observer and dead to the other at the same instant.
fn assert_clock_readers_agree() {
    use nativelink_util::instant_wrapper::InstantWrapper;
    let state_manager_now = MockInstantWrapped::default().now();
    let registry_now = UNIX_EPOCH + Duration::from_secs(now_ts());
    assert_eq!(
        now(),
        state_manager_now,
        "harness now() diverged from MockInstantWrapped::now()"
    );
    let drift = state_manager_now
        .duration_since(registry_now)
        .expect("registry epoch ran ahead of state-manager epoch");
    assert!(
        drift < Duration::from_secs(1),
        "registry epoch and state-manager epoch drifted by {drift:?} (>= 1s)"
    );
}

/// Advances every virtual clock in lockstep: the mock clock (state manager,
/// registry timestamps, pollers) AND tokio's paused clock. Tokio's
/// `start_paused` clock is NOT coupled to `MockClock::advance` — the
/// background abandoned-action sweep sleeps on tokio time — so any invariant
/// that depends on that sweep firing needs the explicit tokio advance here.
async fn advance_clocks(duration: Duration) {
    assert_clock_readers_agree();
    MockClock::advance(duration);
    tokio::time::advance(duration).await;
    assert_clock_readers_agree();
}

fn action_info(key: u8, timeout_s: u8, skip_cache: bool) -> ActionInfo {
    // Distinct digests per key so cache-key merging is itself schedulable.
    let digest = DigestInfo::new([key; 32], u64::from(key) + 1);
    let unique_key = ActionUniqueKey {
        execution_scope: None,
        instance_name: "main".to_string(),
        digest_function: DigestHasherFunc::Sha256,
        digest,
    };
    ActionInfo {
        command_digest: digest,
        input_root_digest: digest,
        timeout: Duration::from_secs(u64::from(timeout_s)),
        platform_properties: HashMap::default(),
        priority: 0,
        load_timestamp: now(),
        insert_timestamp: now(),
        unique_qualifier: if skip_cache {
            ActionUniqueQualifier::Uncacheable(unique_key)
        } else {
            ActionUniqueQualifier::Cacheable(unique_key)
        },
    }
}

struct SimWorker {
    id: WorkerId,
    rx: mpsc::Receiver<UpdateForWorker>,
    /// Operations this worker believes it is running (assigned, not yet
    /// completed/killed from this harness's perspective).
    running: Vec<OperationId>,
    connected: bool,
    /// Virtual timestamp at which the harness learned this connection died
    /// (disconnect, removal, or channel close). Ghost assignments on a dead
    /// worker are legal transiently — the scheduler heals them via the
    /// poll-driven timeout or the abandoned-client backstop — so the
    /// NO-GHOST check only fires once every healing ceiling has elapsed
    /// since this time.
    died_at: Option<u64>,
}

struct Sim {
    scheduler: Arc<SimpleScheduler>,
    worker_scheduler: Arc<dyn WorkerScheduler>,
    workers: Vec<SimWorker>,
    /// Live client pollers: a spawned task long-polling `changed()` per
    /// listener, mirroring the execution server. The timeout machinery is
    /// client-driven (ClientActionStateResult wraps the matching-engine
    /// subscriber), so an undriven listener would model a client that never
    /// polls — and mask nothing OR everything. Abort = client disconnect.
    listeners: HashMap<u8, tokio::task::JoinHandle<()>>,
    /// ActionInfo per key so the drain can re-attach a client to a zombie
    /// operation via cache-key merge, modeling "the next client shows up".
    /// `BTreeMap` so the drain's adopter pass iterates in key order —
    /// `HashMap` iteration order is randomly seeded per process and made
    /// replays nondeterministic.
    infos: BTreeMap<u8, Arc<ActionInfo>>,
    /// operation -> worker currently believed to be executing it; the
    /// double-execution invariant checks insertions against this.
    executing_on: HashMap<OperationId, usize>,
    next_client_id: u64,
    /// Client operation ids observed in a terminal stage. RESULT-ONCE: once
    /// here, an op must never be observed non-terminal again.
    terminal: HashSet<OperationId>,
    /// StartAction pumps per scheduler operation id. RETRY-BUDGET: this must
    /// not exceed `max_job_retries + 1` plus the requeue-untried allowance.
    dispatches: HashMap<OperationId, u32>,
    /// Extra dispatch allowance per op for events the scheduler requeues
    /// without consuming an attempt (declines, kills).
    extra_budget: HashMap<OperationId, u32>,
    /// Normalized dispatch/stage trace of the replay. Two runs of the same
    /// input bytes must produce identical traces (determinism check).
    trace: Vec<String>,
    /// Assignments the scheduler revoked by handing the operation to a
    /// different worker (timeout-driven requeue) while the first assignee
    /// still ran it. Revocation is asynchronous by design — the kill
    /// arrives on a later pass — so this is checked for EVENTUAL
    /// resolution at the end of the drain, not instantaneously.
    revoked: Vec<(usize, OperationId)>,
}

impl Sim {
    fn new() -> Self {
        let task_change_notify = Arc::new(Notify::new());
        let db = memory_awaited_action_db_factory(
            0,
            &task_change_notify.clone(),
            MockInstantWrapped::default,
        );
        let spec = SimpleSpec {
            worker_timeout_s: WORKER_TIMEOUT_S,
            client_action_timeout_s: CLIENT_ACTION_TIMEOUT_S,
            max_action_executing_timeout_s: MAX_EXECUTING_TIMEOUT_S,
            max_job_retries: MAX_JOB_RETRIES as usize,
            ..Default::default()
        };
        let (scheduler, worker_scheduler) = SimpleScheduler::new_with_callback(
            &spec,
            db,
            || std::future::ready(()),
            task_change_notify,
            MockInstantWrapped::default,
            None,
        );
        Self {
            scheduler,
            worker_scheduler,
            workers: Vec::new(),
            listeners: HashMap::new(),
            infos: BTreeMap::new(),
            executing_on: HashMap::new(),
            next_client_id: 0,
            terminal: HashSet::new(),
            dispatches: HashMap::new(),
            extra_budget: HashMap::new(),
            trace: Vec::new(),
            revoked: Vec::new(),
        }
    }

    /// Drain every connected worker's update channel, tracking assignments
    /// and checking the double-execution invariant as updates surface.
    fn pump_workers(&mut self) {
        // Double-execution candidates are checked AFTER the full pass: the
        // prior assignee's own pending Disconnect / kill / channel-close may
        // sit later in this same pump, and asserting mid-pass would report a
        // race the scheduler had already resolved.
        let mut double_exec_candidates: Vec<(OperationId, usize, usize)> = Vec::new();
        for idx in 0..self.workers.len() {
            loop {
                let update = match self.workers[idx].rx.try_recv() {
                    Ok(update) => update,
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        // The scheduler dropped this worker's channel — e.g.
                        // a later ConnectWorker reused the same worker id and
                        // `workers.put` replaced (and dropped) this entry.
                        // Treat as a disconnect: keeping it "connected" with
                        // stale `running` state fabricates double-execution
                        // reports for the superseding connection.
                        if self.workers[idx].connected {
                            self.trace.push(format!("channel-closed w{idx}"));
                        }
                        self.workers[idx].connected = false;
                        let died = now_ts();
                        self.workers[idx].died_at.get_or_insert(died);
                        for operation_id in self.workers[idx].running.drain(..) {
                            self.executing_on.remove(&operation_id);
                        }
                        break;
                    }
                };
                match update.update {
                    Some(update_for_worker::Update::StartAction(start)) => {
                        let operation_id = OperationId::from(start.operation_id.as_str());
                        self.trace.push(format!("start w{idx} {operation_id}"));
                        if std::env::var("FUZZ_DEBUG").is_ok() {
                            eprintln!("  pump: worker[{idx}] StartAction {operation_id}");
                        }
                        if let Some(&other) = self.executing_on.get(&operation_id) {
                            if other != idx {
                                double_exec_candidates.push((operation_id.clone(), other, idx));
                            }
                        }
                        // RETRY-BUDGET: an op may be handed to a worker at
                        // most `max_job_retries + 1` times, plus one per
                        // requeue-untried event (decline/kill) the scheduler
                        // explicitly does not charge an attempt for.
                        let count = self.dispatches.entry(operation_id.clone()).or_insert(0);
                        *count += 1;
                        let budget = MAX_JOB_RETRIES
                            + 1
                            + self.extra_budget.get(&operation_id).copied().unwrap_or(0);
                        assert!(
                            *count <= budget,
                            "INVARIANT: RETRY-BUDGET: operation {operation_id} dispatched \
                             {count} times (budget {budget})"
                        );
                        self.executing_on.insert(operation_id.clone(), idx);
                        self.workers[idx].running.push(operation_id);
                    }
                    Some(update_for_worker::Update::Disconnect(())) => {
                        self.trace.push(format!("disconnect w{idx}"));
                        self.workers[idx].connected = false;
                        let died = now_ts();
                        self.workers[idx].died_at.get_or_insert(died);
                        for operation_id in self.workers[idx].running.drain(..) {
                            self.executing_on.remove(&operation_id);
                        }
                    }
                    Some(update_for_worker::Update::KillOperationRequest(kill)) => {
                        let operation_id = OperationId::from(kill.operation_id.as_str());
                        self.trace.push(format!("kill w{idx} {operation_id}"));
                        self.workers[idx].running.retain(|id| *id != operation_id);
                        self.executing_on.remove(&operation_id);
                        *self.extra_budget.entry(operation_id).or_insert(0) += 1;
                    }
                    _ => {}
                }
            }
        }
        for (operation_id, other, idx) in double_exec_candidates {
            let both_live = self.workers[other].connected
                && self.workers[other].running.contains(&operation_id)
                && self.workers[idx].connected
                && self.workers[idx].running.contains(&operation_id);
            if both_live {
                // Timeout-driven requeue revokes the old assignment but the
                // KillOperation for it is sent asynchronously on a later
                // revoked-check pass (see ApiWorkerScheduler). Not an
                // instantaneous violation; record it and require the kill /
                // disconnect to have landed by the end of the drain.
                self.trace
                    .push(format!("revoked w{other} {operation_id} (now w{idx})"));
                self.revoked.push((other, operation_id));
            }
        }
    }

    /// REVOKE-KILL: every assignment the scheduler superseded must, by the
    /// time the drain has run every timeout past its ceiling, have been
    /// resolved at the old assignee — via KillOperation, Disconnect,
    /// channel close, or the harness completing/declining it there. A
    /// worker still believing it runs a reassigned operation is silent
    /// double execution.
    fn check_revoked_assignments_resolved(&self) {
        for (idx, operation_id) in &self.revoked {
            let still_running = self.workers[*idx].connected
                && self.workers[*idx].running.contains(operation_id);
            assert!(
                !still_running,
                "INVARIANT: REVOKE-KILL: operation {operation_id} was reassigned away from \
                 worker {idx}, but that worker never received a kill/disconnect and still \
                 believes it is executing it (double execution)"
            );
        }
    }

    fn live_worker(&mut self, w: u8) -> Option<usize> {
        if self.workers.is_empty() {
            return None;
        }
        let idx = usize::from(w) % self.workers.len();
        self.workers[idx].connected.then_some(idx)
    }

    /// Sampled state invariants, checked through the same
    /// `filter_operations` surface a real client/admin would use.
    ///
    /// RESULT-ONCE: a client operation observed terminal
    /// (`Completed`/`CompletedFromCache`) must never be observed in a
    /// non-terminal stage afterwards.
    ///
    /// NO-GHOST-ASSIGNMENT: no operation may be `Executing` with its
    /// scheduler-side worker assignment naming a worker the harness knows is
    /// disconnected/removed (and not since reconnected under the same id).
    async fn check_state_invariants(&mut self) {
        use futures::StreamExt;
        use nativelink_util::operation_state_manager::OperationStageFlags;
        if let Ok(mut stream) = self
            .scheduler
            .filter_operations(OperationFilter::default())
            .await
        {
            while let Some(entry) = stream.next().await {
                if let Ok((state, _)) = entry.as_state().await {
                    let is_terminal = matches!(
                        state.stage,
                        ActionStage::Completed(_) | ActionStage::CompletedFromCache(_)
                    );
                    if is_terminal {
                        self.terminal.insert(state.client_operation_id.clone());
                    } else {
                        assert!(
                            !self.terminal.contains(&state.client_operation_id),
                            "INVARIANT: RESULT-ONCE: operation {} observed in {:?} \
                             after being observed terminal",
                            state.client_operation_id,
                            state.stage,
                        );
                    }
                }
            }
        }
        let live: HashSet<&str> = self
            .workers
            .iter()
            .filter(|worker| worker.connected)
            .map(|worker| worker.id.0.as_str())
            .collect();
        // A dead worker's assignments heal asynchronously: the poll-driven
        // timeout (needs a live subscriber) fires within
        // WORKER_TIMEOUT_S + MAX_EXECUTING_TIMEOUT_S, and a fully abandoned
        // operation only falls to the client backstop at roughly
        // 2 * CLIENT_ACTION_TIMEOUT_S. A ghost is only a violation once
        // every one of those ceilings has elapsed since the death.
        let ghost_grace_s = WORKER_TIMEOUT_S + MAX_EXECUTING_TIMEOUT_S + 2 * CLIENT_ACTION_TIMEOUT_S;
        let dead: HashSet<WorkerId> = self
            .workers
            .iter()
            .filter(|worker| {
                !worker.connected
                    && !live.contains(worker.id.0.as_str())
                    && worker
                        .died_at
                        .is_some_and(|died| now_ts().saturating_sub(died) > ghost_grace_s)
            })
            .map(|worker| worker.id.clone())
            .collect();
        for worker_id in dead {
            let filter = OperationFilter {
                stages: OperationStageFlags::Executing,
                worker_id: Some(worker_id.clone()),
                ..Default::default()
            };
            if let Ok(mut stream) = self.scheduler.filter_operations(filter).await {
                while let Some(entry) = stream.next().await {
                    if let Ok((state, _)) = entry.as_state().await {
                        panic!(
                            "INVARIANT: NO-GHOST-ASSIGNMENT: operation {} Executing on \
                             dead worker {worker_id}",
                            state.client_operation_id,
                        );
                    }
                }
            }
        }
    }

    async fn apply(&mut self, op: Op) {
        let sample = matches!(
            op,
            Op::CompleteOk { .. }
                | Op::CompleteRetryableErr { .. }
                | Op::DispatchDecline { .. }
                | Op::Disconnect { .. }
                | Op::RemoveWorker { .. }
                | Op::RemoveTimedoutWorkers
                | Op::TryMatch
        );
        match op {
            Op::AddAction {
                key,
                timeout_s,
                skip_cache,
            } => {
                self.next_client_id += 1;
                let client_id = OperationId::from(format!("client-{}", self.next_client_id));
                let info = Arc::new(action_info(key % 8, timeout_s, skip_cache));
                self.infos.insert(key, info.clone());
                if let Ok(mut listener) = self.scheduler.add_action(client_id, info).await {
                    if let Some(old) = self.listeners.insert(
                        key,
                        tokio::task::spawn(async move {
                            loop {
                                match listener.changed().await {
                                    Ok((state, _)) if state.stage.is_finished() => break,
                                    Ok(_) => {}
                                    Err(_) => break,
                                }
                            }
                        }),
                    ) {
                        old.abort();
                    }
                }
            }
            Op::ConnectWorker { w, slots } => {
                let worker_id = WorkerId(format!("worker-{w}"));
                let (tx, rx) = mpsc::channel(256);
                let worker = Worker::new(
                    worker_id.clone(),
                    PlatformProperties::default(),
                    tx,
                    now_ts(),
                    u64::from(slots % 4) + 1,
                );
                if self.worker_scheduler.add_worker(worker).await.is_ok() {
                    self.workers.push(SimWorker {
                        id: worker_id,
                        rx,
                        running: Vec::new(),
                        connected: true,
                        died_at: None,
                    });
                }
            }
            Op::KeepAlive { w } => {
                if let Some(idx) = self.live_worker(w) {
                    let id = self.workers[idx].id.clone();
                    drop(
                        self.worker_scheduler
                            .worker_keep_alive_received(&id, now_ts(), None)
                            .await,
                    );
                }
            }
            Op::DispatchAccept { w } => {
                if let Some(idx) = self.live_worker(w)
                    && let Some(operation_id) = self.workers[idx].running.last().cloned()
                {
                    let id = self.workers[idx].id.clone();
                    let result = self
                        .worker_scheduler
                        .worker_dispatch_accepted(&id, &operation_id)
                        .await;
                    if std::env::var("FUZZ_DEBUG").is_ok()
                        && let Err(err) = &result
                    {
                        eprintln!("dispatch_accepted({id}, {operation_id}) -> {err:?}");
                    }
                }
            }
            Op::DispatchDecline { w } => {
                if let Some(idx) = self.live_worker(w)
                    && let Some(operation_id) = self.workers[idx].running.pop()
                {
                    let id = self.workers[idx].id.clone();
                    self.executing_on.remove(&operation_id);
                    // Declines requeue "untried" (no attempt charged), so
                    // they extend the RETRY-BUDGET allowance.
                    *self.extra_budget.entry(operation_id.clone()).or_insert(0) += 1;
                    drop(
                        self.worker_scheduler
                            .worker_dispatch_declined(
                                &id,
                                &operation_id,
                                "fuzz-declined".to_string(),
                                None,
                            )
                            .await,
                    );
                }
            }
            Op::CompleteOk { w } => self.complete(w, None).await,
            Op::CompleteRetryableErr { w } => {
                self.complete(w, Some(nativelink_error::Code::Aborted))
                    .await;
            }
            Op::Disconnect { w } => {
                if let Some(idx) = self.live_worker(w) {
                    let id = self.workers[idx].id.clone();
                    drop(self.worker_scheduler.worker_disconnected(&id).await);
                    self.workers[idx].connected = false;
                        let died = now_ts();
                        self.workers[idx].died_at.get_or_insert(died);
                    for operation_id in self.workers[idx].running.drain(..) {
                        self.executing_on.remove(&operation_id);
                    }
                }
            }
            Op::RemoveWorker { w } => {
                if let Some(idx) = self.live_worker(w) {
                    let id = self.workers[idx].id.clone();
                    drop(self.worker_scheduler.remove_worker(&id).await);
                    self.workers[idx].connected = false;
                        let died = now_ts();
                        self.workers[idx].died_at.get_or_insert(died);
                    for operation_id in self.workers[idx].running.drain(..) {
                        self.executing_on.remove(&operation_id);
                    }
                }
            }
            Op::SetDrain { w, draining } => {
                if let Some(idx) = self.live_worker(w) {
                    let id = self.workers[idx].id.clone();
                    drop(self.worker_scheduler.set_drain_worker(&id, draining).await);
                }
            }
            Op::RemoveTimedoutWorkers => {
                drop(
                    self.worker_scheduler
                        .remove_timedout_workers(now_ts())
                        .await,
                );
            }
            Op::TryMatch => {
                drop(self.scheduler.do_try_match_for_test().await);
            }
            Op::Advance { ms } => {
                advance_clocks(Duration::from_millis(u64::from(ms))).await;
            }
            Op::DropClient { key } => {
                if let Some(poller) = self.listeners.remove(&key) {
                    poller.abort();
                }
            }
        }
        self.pump_workers();
        if sample {
            self.check_state_invariants().await;
        }
    }

    async fn complete(&mut self, w: u8, err: Option<nativelink_error::Code>) {
        if let Some(idx) = self.live_worker(w) {
            self.complete_at(idx, err).await;
        }
    }

    async fn complete_at(&mut self, idx: usize, err: Option<nativelink_error::Code>) {
        use nativelink_util::operation_state_manager::UpdateOperationType;
        if let Some(operation_id) = self.workers[idx].running.pop() {
            let id = self.workers[idx].id.clone();
            self.executing_on.remove(&operation_id);
            let update = match err {
                None => UpdateOperationType::UpdateWithActionStage(ActionStage::Completed(
                    nativelink_util::action_messages::ActionResult::default(),
                )),
                Some(code) => UpdateOperationType::UpdateWithError(nativelink_error::make_err!(
                    code,
                    "fuzz-injected worker failure"
                )),
            };
            let result = self
                .worker_scheduler
                .update_action(&id, &operation_id, update)
                .await;
            if std::env::var("FUZZ_DEBUG").is_ok()
                && let Err(err) = &result
            {
                eprintln!("update_action({id}, {operation_id}) -> {err:?}");
            }
        }
    }

    /// The liveness engine: with one healthy worker attached, actively
    /// polled clients, and every configured ceiling elapsed, every
    /// operation must reach a terminal stage. Anything else is a wedge.
    async fn drain(&mut self) {
        let dbg = std::env::var("FUZZ_DEBUG").is_ok();
        // A fresh worker, in an id namespace fuzz schedules cannot collide
        // with, with generous capacity to absorb requeues.
        let worker_id = WorkerId("drain-worker".to_string());
        let (tx, rx) = mpsc::channel(256);
        let worker = Worker::new(
            worker_id.clone(),
            PlatformProperties::default(),
            tx,
            now_ts(),
            8,
        );
        self.worker_scheduler
            .add_worker(worker)
            .await
            .expect("drain worker connects");
        self.workers.push(SimWorker {
            id: worker_id,
            rx,
            running: Vec::new(),
            connected: true,
            died_at: None,
        });
        let drain_idx = self.workers.len() - 1;
        self.pump_workers();

        let step = Duration::from_secs(WORKER_TIMEOUT_S / 2 + 1);
        for phase in 0..2u8 {
            if phase == 1 {
                // Model "the next client shows up": re-attach an actively
                // polling client to every action via cache-key merge —
                // recovery of an abandoned operation is client-driven by
                // design, so the contract is "heals once any client polls".
                for info in self.infos.values() {
                    self.next_client_id += 1;
                    let client_id = OperationId::from(format!("adopter-{}", self.next_client_id));
                    if let Ok(mut listener) =
                        self.scheduler.add_action(client_id, info.clone()).await
                    {
                        if dbg {
                            eprintln!("  adopter attached");
                        }
                        self.listeners.insert(
                            200 + (self.next_client_id % 50) as u8,
                            tokio::task::spawn(async move {
                                loop {
                                    match listener.changed().await {
                                        Ok((state, _)) if state.stage.is_finished() => break,
                                        Ok(_) => {}
                                        Err(_) => break,
                                    }
                                }
                            }),
                        );
                    }
                }
            }
            for iter in 0..24 {
                advance_clocks(step).await;
                // Let poller mock-clock sleeps observe the advance
                // (MockInstantWrapped::sleep spins on yield_now).
                for _ in 0..512 {
                    tokio::task::yield_now().await;
                }
                drop(
                    self.worker_scheduler
                        .remove_timedout_workers(now_ts())
                        .await,
                );
                drop(self.scheduler.do_try_match_for_test().await);
                self.pump_workers();
                // The drain worker follows the full dispatch protocol:
                // acknowledge, then complete, everything it is handed.
                while self.workers[drain_idx].connected
                    && !self.workers[drain_idx].running.is_empty()
                {
                    let id = self.workers[drain_idx].id.clone();
                    let operation_id = self.workers[drain_idx]
                        .running
                        .last()
                        .cloned()
                        .expect("non-empty");
                    if dbg {
                        eprintln!("  drain[{phase}.{iter}]: accept+complete {operation_id}");
                    }
                    drop(
                        self.worker_scheduler
                            .worker_dispatch_accepted(&id, &operation_id)
                            .await,
                    );
                    self.complete_at(drain_idx, None).await;
                    // Let completion notifications propagate before deciding
                    // whether more work arrived.
                    for _ in 0..64 {
                        tokio::task::yield_now().await;
                    }
                    self.pump_workers();
                }
                {
                    let id = self.workers[drain_idx].id.clone();
                    drop(
                        self.worker_scheduler
                            .worker_keep_alive_received(&id, now_ts(), None)
                            .await,
                    );
                }
                self.check_state_invariants().await;
                if dbg {
                    use futures::StreamExt;
                    if let Ok(mut stream) = self
                        .scheduler
                        .filter_operations(OperationFilter::default())
                        .await
                    {
                        while let Some(entry) = stream.next().await {
                            if let Ok((state, _)) = entry.as_state().await {
                                eprintln!(
                                    "  [{phase}.{iter}] t={:?} op {} stage={:?}",
                                    MockClock::time(),
                                    state.client_operation_id,
                                    state.stage
                                );
                            }
                        }
                    }
                }
            }
        }
        for (_, poller) in self.listeners.drain() {
            poller.abort();
        }
        self.check_revoked_assignments_resolved();
        // Every operation the scheduler still knows about must be terminal.
        let mut finals = Vec::new();
        let filter = OperationFilter::default();
        if let Ok(mut stream) = self.scheduler.filter_operations(filter).await {
            use futures::StreamExt;
            while let Some(entry) = stream.next().await {
                if let Ok((state, _)) = entry.as_state().await {
                    finals.push(format!(
                        "final {} {}",
                        state.client_operation_id,
                        match &state.stage {
                            ActionStage::Completed(_) => "completed",
                            ActionStage::CompletedFromCache(_) => "completed-from-cache",
                            other => {
                                self.trace.push(format!("wedged {other:?}"));
                                "wedged"
                            }
                        }
                    ));
                    assert!(
                        matches!(
                            state.stage,
                            ActionStage::Completed(_) | ActionStage::CompletedFromCache(_)
                        ),
                        "WEDGED OPERATION after full drain: {} stuck in {:?}",
                        state.client_operation_id,
                        state.stage,
                    );
                }
            }
        }
        // Stream order over the state manager is not contractual; sort so
        // the trace depends only on the final state, not iteration order.
        finals.sort();
        self.trace.append(&mut finals);
    }
}

/// Hard cap on events per schedule so pathological inputs stay cheap.
const MAX_OPS: usize = 256;

/// Decode the fuzz input into a BOUNDED-but-FULL op schedule.
///
/// `Vec::<Op>::arbitrary_take_rest` reads a continuation byte before each
/// element, so a random even byte ended the vector — the expected schedule
/// length was ~1 event and the campaign tested almost-empty schedules.
/// Instead: the first byte fixes a target length (1..=256), then exactly
/// that many `Op`s are pulled in a loop until the buffer runs dry or the cap
/// is hit. A typical random input now yields tens of events. Deterministic:
/// pure function of the input bytes.
pub fn decode(data: &[u8]) -> Option<Vec<Op>> {
    let mut unstructured = Unstructured::new(data);
    let target = usize::from(unstructured.arbitrary::<u8>().ok()?) + 1;
    let target = target.min(MAX_OPS);
    let mut ops = Vec::with_capacity(target);
    while ops.len() < target && !unstructured.is_empty() {
        match Op::arbitrary(&mut unstructured) {
            Ok(op) => ops.push(op),
            Err(_) => break,
        }
    }
    Some(ops)
}

/// Entry point shared by the libfuzzer target and the stable smoke binary.
pub fn run(data: &[u8]) {
    drop(run_trace(data));
}

/// Like [`run`], but returns the normalized dispatch/stage trace so callers
/// can assert that replaying the same bytes yields the same execution.
pub fn run_trace(data: &[u8]) -> Vec<String> {
    let Some(ops) = decode(data) else {
        return Vec::new();
    };
    if ops.is_empty() {
        return Vec::new();
    }
    if std::env::var("FUZZ_TRACE").is_ok() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(if std::env::var("FUZZ_TRACE").as_deref() == Ok("trace") {
                    "nativelink_scheduler=trace"
                } else {
                    "nativelink_scheduler=debug,nativelink_util=warn"
                })
                .with_writer(std::io::stderr)
                .without_time()
                .try_init();
        });
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .start_paused(true)
        .build()
        .expect("current-thread runtime");
    // H2: deterministic replay. The scheduler mints internal operation ids
    // via `OperationId::default()` (a random UUIDv4), and
    // `SortedAwaitedAction` breaks equal priority/time ties by operation id
    // — so identical inputs dispatched in different orders across replays.
    // The mock_operation_id hook swaps in a thread-local counter, reset per
    // run, so tie-breaking is a pure function of the input.
    nativelink_util::action_messages::set_deterministic_operation_ids(Some(0));
    let trace = runtime.block_on(async move {
        MockClock::set_time(Duration::from_secs(NOW_TIME_S));
        assert_clock_readers_agree();
        let mut sim = Sim::new();
        for op in ops {
            sim.apply(op).await;
            // Let scheduler background tasks (matching engine notify loop)
            // run to quiescence between events.
            tokio::task::yield_now().await;
        }
        sim.drain().await;
        sim.trace
    });
    nativelink_util::action_messages::set_deterministic_operation_ids(None);
    trace
}

#[cfg(test)]
mod tests {
    use super::run;

    /// Deterministic xorshift so the smoke corpus is reproducible.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }

    /// H3 regression: a typical random input must decode to a rich schedule
    /// (tens of events, both submissions and workers present) — not the ~1
    /// event `arbitrary_take_rest`'s continuation bytes yielded.
    #[test]
    fn decoded_schedules_are_rich() {
        let mut rng = Rng(0x5EED_CAFE_F00D_D00D);
        let cases = 500u64;
        let mut total_ops = 0usize;
        let mut with_action_and_worker = 0usize;
        for _ in 0..cases {
            let len = (rng.next() % 900 + 40) as usize;
            let mut data = vec![0u8; len];
            for byte in &mut data {
                *byte = (rng.next() & 0xFF) as u8;
            }
            let ops = super::decode(&data).expect("decodes");
            total_ops += ops.len();
            let has_action = ops.iter().any(|op| matches!(op, super::Op::AddAction { .. }));
            let has_worker = ops
                .iter()
                .any(|op| matches!(op, super::Op::ConnectWorker { .. }));
            if has_action && has_worker {
                with_action_and_worker += 1;
            }
        }
        let mean = total_ops as f64 / cases as f64;
        assert!(
            mean >= 20.0,
            "mean decoded schedule length {mean:.1} < 20 — decoder regressed to near-empty schedules"
        );
        assert!(
            with_action_and_worker as f64 >= cases as f64 * 0.5,
            "only {with_action_and_worker}/{cases} schedules contain both an AddAction and a \
             ConnectWorker"
        );
    }

    /// H1 self-check: the registry sweep reader (`UNIX_EPOCH + now_ts()`
    /// reconstructed from worker timestamps) and the operation-timeout
    /// reader (`MockInstantWrapped::now()`) must agree on worker liveness at
    /// the same instant, immediately before and after a deadline crossing.
    /// Also checks that the tokio-clocked background sweep timer fires when
    /// virtual time crosses its interval (tokio `start_paused` is not
    /// coupled to `MockClock::advance`; `advance_clocks` drives both).
    #[test]
    fn clock_readers_agree_across_deadline() {
        use core::time::Duration;
        use std::time::UNIX_EPOCH;

        use mock_instant::thread_local::MockClock;
        use nativelink_util::instant_wrapper::{InstantWrapper, MockInstantWrapped};

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .start_paused(true)
            .build()
            .unwrap();
        runtime.block_on(async {
            MockClock::set_time(Duration::from_secs(super::NOW_TIME_S));
            let worker_last_seen_ts = super::now_ts();
            let timeout = Duration::from_secs(super::WORKER_TIMEOUT_S);
            let verdicts = |ts: u64| {
                // Registry reader: worker alive iff last_seen + timeout > now
                // (worker_registry::check_liveness semantics).
                let registry_alive = (UNIX_EPOCH + Duration::from_secs(ts) + timeout)
                    > MockInstantWrapped::default().now();
                // Sweep reader: alive iff last_update_timestamp > now_ts - timeout
                // (api_worker_scheduler::remove_timedout_workers semantics).
                let sweep_alive = ts > super::now_ts().saturating_sub(super::WORKER_TIMEOUT_S);
                (registry_alive, sweep_alive)
            };

            // Just before the deadline: both must say alive.
            super::advance_clocks(timeout - Duration::from_secs(1)).await;
            let (registry_alive, sweep_alive) = verdicts(worker_last_seen_ts);
            assert!(registry_alive && sweep_alive, "before deadline: registry={registry_alive} sweep={sweep_alive}");

            // Just after: both must say dead.
            super::advance_clocks(Duration::from_secs(2)).await;
            let (registry_alive, sweep_alive) = verdicts(worker_last_seen_ts);
            assert!(!registry_alive && !sweep_alive, "after deadline: registry={registry_alive} sweep={sweep_alive}");

            // Tokio-driven sweep coupling: a timer on tokio's clock fires
            // once advance_clocks pushes virtual time past its interval.
            let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let fired_clone = fired.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(super::CLIENT_ACTION_TIMEOUT_S)).await;
                fired_clone.store(true, std::sync::atomic::Ordering::SeqCst);
            });
            tokio::task::yield_now().await;
            super::advance_clocks(Duration::from_secs(super::CLIENT_ACTION_TIMEOUT_S + 1)).await;
            for _ in 0..8 {
                tokio::task::yield_now().await;
            }
            assert!(
                fired.load(std::sync::atomic::Ordering::SeqCst),
                "tokio-clocked sweep timer did not fire after virtual time crossed its interval"
            );
        });
    }

    /// H2 regression: replaying the same input bytes must yield an identical
    /// normalized dispatch/stage trace.
    #[test]
    fn replay_is_deterministic() {
        let mut rng = Rng(0xD5E7_0000_0000_0001);
        let mut nonempty = 0usize;
        for case in 0..8u64 {
            let len = (rng.next() % 900 + 40) as usize;
            let mut data = vec![0u8; len];
            for byte in &mut data {
                *byte = (rng.next() & 0xFF) as u8;
            }
            let first = super::run_trace(&data);
            for replay in 1..3 {
                let again = super::run_trace(&data);
                assert_eq!(
                    first, again,
                    "case {case}: replay {replay} produced a different trace"
                );
            }
            if !first.is_empty() {
                nonempty += 1;
            }
        }
        assert!(nonempty > 0, "all determinism-check traces were empty");
    }

    #[test]
    fn dump_one_case() {
        let Ok(case_target) = std::env::var("FUZZ_DUMP_CASE").map(|v| v.parse::<u64>().unwrap())
        else {
            return;
        };
        let mut rng = Rng(0x5EED_CAFE_F00D_D00D);
        let mut data = Vec::new();
        for _case in 0..=case_target {
            let len = (rng.next() % 900 + 40) as usize;
            data = vec![0u8; len];
            for byte in &mut data {
                *byte = (rng.next() & 0xFF) as u8;
            }
        }
        if let Some(ops) = super::decode(&data) {
            for (i, op) in ops.iter().enumerate() {
                eprintln!("{i:3}: {op:?}");
            }
        }
        run(&data);
    }

    /// Not a substitute for coverage-guided fuzzing — a harness self-check
    /// that also gives stable toolchains a way to run the simulator.
    #[test]
    fn random_schedules_smoke() {
        let cases: u64 = std::env::var("FUZZ_SMOKE_CASES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(200);
        let seed: u64 = std::env::var("FUZZ_SMOKE_SEED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0x5EED_CAFE_F00D_D00D);
        let start: u64 = std::env::var("FUZZ_SMOKE_START")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let dump: bool = std::env::var("FUZZ_SMOKE_DUMP").is_ok();
        let mut rng = Rng(seed);
        for case in 0..cases {
            let len = (rng.next() % 900 + 40) as usize;
            let mut data = vec![0u8; len];
            for byte in &mut data {
                *byte = (rng.next() & 0xFF) as u8;
            }
            if case < start {
                continue;
            }
            if case % 500 == 0 {
                eprintln!("smoke case {case} len {len}");
            }
            if dump {
                eprintln!("=== case {case}");
                if let Some(ops) = super::decode(&data) {
                    for (i, op) in ops.iter().enumerate() {
                        eprintln!("{i:3}: {op:?}");
                    }
                }
            }
            run(&data);
        }
    }
}
