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
//!   * an operation `StartAction`ed on a second worker while the first
//!     assignment is still live (double execution), and
//!   * the drain epilogue: after all fuzz events, with one healthy worker
//!     connected and the clock advanced past every configured ceiling, every
//!     operation must reach a terminal stage — anything still `Executing` or
//!     `Queued` is a wedge (the "dead peer / pre-empted timeout / lost
//!     requeue" bug class).
//!
//! Determinism: single-threaded runtime, `MockClock` for time, no real I/O.
//! A crashing input replays exactly.

use core::time::Duration;
use std::collections::HashMap;
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
    ActionStateResult, ClientStateManager, OperationFilter,
};
use nativelink_util::platform_properties::PlatformProperties;
use tokio::sync::{Notify, mpsc};

/// Base virtual time; far from zero so `duration_since` never underflows for
/// timestamps the harness fabricates.
const NOW_TIME_S: u64 = 100_000;

/// Scheduler timing knobs. Small values keep the drain epilogue's virtual
/// advance cheap; nonzero `max_action_executing_timeout_s` gives the drain a
/// ceiling to converge on even for pathological states.
const WORKER_TIMEOUT_S: u64 = 30;
const CLIENT_ACTION_TIMEOUT_S: u64 = 3_600;
const MAX_EXECUTING_TIMEOUT_S: u64 = 120;
const MAX_JOB_RETRIES: u32 = 2;

/// The event vocabulary the fuzzer schedules. Worker/action indices are
/// small so schedules revisit the same entities often — races need contact.
#[derive(Debug, Arbitrary)]
pub enum Op {
    AddAction { key: u8, timeout_s: u8, skip_cache: bool },
    ConnectWorker { w: u8, slots: u8 },
    KeepAlive { w: u8 },
    DispatchAccept { w: u8 },
    DispatchDecline { w: u8 },
    CompleteOk { w: u8 },
    CompleteRetryableErr { w: u8 },
    Disconnect { w: u8 },
    RemoveWorker { w: u8 },
    SetDrain { w: u8, draining: bool },
    RemoveTimedoutWorkers,
    TryMatch,
    Advance { ms: u16 },
    DropClient { key: u8 },
}

fn now() -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(NOW_TIME_S) + MockClock::time()
}

fn now_ts() -> u64 {
    NOW_TIME_S + MockClock::time().as_secs()
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
    infos: HashMap<u8, Arc<ActionInfo>>,
    /// operation -> worker currently believed to be executing it; the
    /// double-execution invariant checks insertions against this.
    executing_on: HashMap<OperationId, usize>,
    next_client_id: u64,
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
            infos: HashMap::new(),
            executing_on: HashMap::new(),
            next_client_id: 0,
        }
    }

    /// Drain every connected worker's update channel, tracking assignments
    /// and checking the double-execution invariant as updates surface.
    fn pump_workers(&mut self) {
        for idx in 0..self.workers.len() {
            loop {
                let update = match self.workers[idx].rx.try_recv() {
                    Ok(update) => update,
                    Err(_) => break,
                };
                match update.update {
                    Some(update_for_worker::Update::StartAction(start)) => {
                        let operation_id = OperationId::from(start.operation_id.as_str());
                        if std::env::var("FUZZ_DEBUG").is_ok() {
                            eprintln!("  pump: worker[{idx}] StartAction {operation_id}");
                        }
                        if let Some(&other) = self.executing_on.get(&operation_id) {
                            let other_live = self.workers[other].connected
                                && self.workers[other].running.contains(&operation_id);
                            assert!(
                                !other_live || other == idx,
                                "DOUBLE EXECUTION: operation {operation_id} assigned to \
                                 worker {idx} while worker {other} still runs it"
                            );
                        }
                        self.executing_on.insert(operation_id.clone(), idx);
                        self.workers[idx].running.push(operation_id);
                    }
                    Some(update_for_worker::Update::Disconnect(())) => {
                        self.workers[idx].connected = false;
                        for operation_id in self.workers[idx].running.drain(..) {
                            self.executing_on.remove(&operation_id);
                        }
                    }
                    Some(update_for_worker::Update::KillOperationRequest(kill)) => {
                        let operation_id = OperationId::from(kill.operation_id.as_str());
                        self.workers[idx].running.retain(|id| *id != operation_id);
                        self.executing_on.remove(&operation_id);
                    }
                    _ => {}
                }
            }
        }
    }

    fn live_worker(&mut self, w: u8) -> Option<usize> {
        if self.workers.is_empty() {
            return None;
        }
        let idx = usize::from(w) % self.workers.len();
        self.workers[idx].connected.then_some(idx)
    }

    async fn apply(&mut self, op: Op) {
        match op {
            Op::AddAction { key, timeout_s, skip_cache } => {
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
                    });
                }
            }
            Op::KeepAlive { w } => {
                if let Some(idx) = self.live_worker(w) {
                    let id = self.workers[idx].id.clone();
                    drop(self.worker_scheduler.worker_keep_alive_received(&id, now_ts(), None).await);
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
                self.complete(w, Some(nativelink_error::Code::Aborted)).await;
            }
            Op::Disconnect { w } => {
                if let Some(idx) = self.live_worker(w) {
                    let id = self.workers[idx].id.clone();
                    drop(self.worker_scheduler.worker_disconnected(&id).await);
                    self.workers[idx].connected = false;
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
                drop(self.worker_scheduler.remove_timedout_workers(now_ts()).await);
            }
            Op::TryMatch => {
                drop(self.scheduler.do_try_match_for_test().await);
            }
            Op::Advance { ms } => {
                MockClock::advance(Duration::from_millis(u64::from(ms)));
            }
            Op::DropClient { key } => {
                if let Some(poller) = self.listeners.remove(&key) {
                    poller.abort();
                }
            }
        }
        self.pump_workers();
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
                    let client_id =
                        OperationId::from(format!("adopter-{}", self.next_client_id));
                    if let Ok(mut listener) =
                        self.scheduler.add_action(client_id, info.clone()).await
                    {
                        if dbg { eprintln!("  adopter attached"); }
                        self.listeners.insert(200 + (self.next_client_id % 50) as u8,
                            tokio::task::spawn(async move {
                                loop {
                                    match listener.changed().await {
                                        Ok((state, _)) if state.stage.is_finished() => break,
                                        Ok(_) => {}
                                        Err(_) => break,
                                    }
                                }
                            }));
                    }
                }
            }
            for iter in 0..24 {
                MockClock::advance(step);
                // Let poller mock-clock sleeps observe the advance
                // (MockInstantWrapped::sleep spins on yield_now).
                for _ in 0..512 {
                    tokio::task::yield_now().await;
                }
                drop(self.worker_scheduler.remove_timedout_workers(now_ts()).await);
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
        // Every operation the scheduler still knows about must be terminal.
        let filter = OperationFilter::default();
        if let Ok(mut stream) = self.scheduler.filter_operations(filter).await {
            use futures::StreamExt;
            while let Some(entry) = stream.next().await {
                if let Ok((state, _)) = entry.as_state().await {
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
    }
}

/// Decode the fuzz input into its op schedule (for repro diagnostics).
pub fn decode(data: &[u8]) -> Option<Vec<Op>> {
    let unstructured = Unstructured::new(data);
    Vec::<Op>::arbitrary_take_rest(unstructured).ok()
}

/// Entry point shared by the libfuzzer target and the stable smoke binary.
pub fn run(data: &[u8]) {
    let unstructured = Unstructured::new(data);
    let Ok(ops) = Vec::<Op>::arbitrary_take_rest(unstructured) else {
        return;
    };
    if ops.is_empty() || ops.len() > 512 {
        return;
    }
    if std::env::var("FUZZ_TRACE").is_ok() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = tracing_subscriber::fmt()
                .with_env_filter(
                    if std::env::var("FUZZ_TRACE").as_deref() == Ok("trace") {
                        "nativelink_scheduler=trace"
                    } else {
                        "nativelink_scheduler=debug,nativelink_util=warn"
                    },
                )
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
    runtime.block_on(async move {
        MockClock::set_time(Duration::ZERO);
        let mut sim = Sim::new();
        for op in ops {
            sim.apply(op).await;
            // Let scheduler background tasks (matching engine notify loop)
            // run to quiescence between events.
            tokio::task::yield_now().await;
        }
        sim.drain().await;
    });
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

    #[test]
    fn dump_one_case() {
        let Ok(case_target) = std::env::var("FUZZ_DUMP_CASE").map(|v| v.parse::<u64>().unwrap()) else { return; };
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
