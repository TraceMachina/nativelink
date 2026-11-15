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

//! Simulated execution. An action is a duration per phase and an outcome
//! per attempt; the worker around it is the production `LocalWorker`.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;

use nativelink_error::{Code, Error, make_err};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    ActionResourceUsage, StartExecute,
};
use nativelink_util::action_messages::{ActionResult, OperationId};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_worker::running_actions_manager::{Metrics, RunningAction, RunningActionsManager};
use parking_lot::Mutex;
use tokio::sync::Notify;

use crate::trace::Event;

/// What one attempt at an action does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(variant_size_differences, reason = "a few bytes")]
pub enum Run {
    /// Exits 0.
    Succeed,
    /// Exits with this code.
    Exit(i32),
    /// The worker fails it with this code, as a lost input or a crash
    /// in the runner would.
    Error(Code),
    /// Never finishes until killed.
    Hang,
    /// The execution task panics.
    Panic,
}

/// One action of a scenario. Its digest comes from `seed`, so two clients
/// that name the same seed ask for the same action.
#[derive(Clone, Debug)]
pub struct Action {
    pub(crate) seed: u32,
    pub(crate) priority: i32,
    pub(crate) cacheable: bool,
    pub(crate) prepare: Duration,
    pub(crate) run: Duration,
    pub(crate) upload: Duration,
    /// One per attempt; the last repeats.
    pub(crate) attempts: Vec<Run>,
    pub(crate) properties: Vec<(String, String)>,
    pub(crate) timeout: Option<Duration>,
}

impl Action {
    /// A cacheable action that runs for one second and succeeds.
    pub fn new(seed: u32) -> Self {
        Self {
            seed,
            priority: 0,
            cacheable: true,
            prepare: Duration::ZERO,
            run: Duration::from_secs(1),
            upload: Duration::ZERO,
            attempts: vec![Run::Succeed],
            properties: Vec::new(),
            timeout: None,
        }
    }

    #[must_use]
    pub const fn priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Sent with `skip_cache_lookup`, so it never coalesces with another.
    #[must_use]
    pub const fn uncacheable(mut self) -> Self {
        self.cacheable = false;
        self
    }

    #[must_use]
    pub const fn run_ms(mut self, ms: u64) -> Self {
        self.run = Duration::from_millis(ms);
        self
    }

    #[must_use]
    pub const fn prepare_ms(mut self, ms: u64) -> Self {
        self.prepare = Duration::from_millis(ms);
        self
    }

    #[must_use]
    pub const fn upload_ms(mut self, ms: u64) -> Self {
        self.upload = Duration::from_millis(ms);
        self
    }

    /// What each attempt does, in order; the last repeats.
    #[must_use]
    pub fn attempts(mut self, attempts: &[Run]) -> Self {
        assert!(!attempts.is_empty(), "an action needs at least one attempt");
        self.attempts = attempts.to_vec();
        self
    }

    #[must_use]
    pub fn property(mut self, name: &str, value: &str) -> Self {
        self.properties.push((name.to_string(), value.to_string()));
        self
    }

    /// `Action.timeout`.
    #[must_use]
    pub const fn timeout_s(mut self, seconds: u64) -> Self {
        self.timeout = Some(Duration::from_secs(seconds));
        self
    }

    pub fn label(&self) -> String {
        format!("a{}", self.seed)
    }

    pub const fn seed(&self) -> u32 {
        self.seed
    }
}

#[derive(Debug)]
pub(crate) struct Entry {
    pub(crate) action: Action,
    pub(crate) attempts: AtomicU32,
}

/// Every action a scenario submitted, by its digest, so the worker side can
/// find what a `StartExecute` should do.
#[derive(Debug, Default)]
pub struct Registry {
    entries: Mutex<HashMap<DigestInfo, Arc<Entry>>>,
}

impl Registry {
    pub(crate) fn insert(&self, digest: DigestInfo, action: Action) {
        self.entries.lock().entry(digest).or_insert_with(|| {
            Arc::new(Entry {
                action,
                attempts: AtomicU32::new(0),
            })
        });
    }

    pub(crate) fn label(&self, digest: &DigestInfo) -> Option<String> {
        self.entries.lock().get(digest).map(|e| e.action.label())
    }

    pub(crate) fn lookup(&self, digest: &DigestInfo) -> Option<Arc<Entry>> {
        self.entries.lock().get(digest).cloned()
    }
}

pub(crate) fn start_execute_digest(start_execute: &StartExecute) -> Option<DigestInfo> {
    start_execute
        .execute_request
        .as_ref()?
        .action_digest
        .as_ref()
        .and_then(|digest| DigestInfo::try_from(digest).ok())
}

/// What one attempt at an action does on the worker that got it.
#[derive(Clone, Debug)]
pub struct Plan {
    /// The action's name in the trace.
    pub action: String,
    /// A stable name for the scheduler's operation.
    pub key: String,
    pub prepare: Duration,
    pub run_for: Duration,
    pub upload: Duration,
    pub run: Run,
}

/// Where a simulated worker learns what an action does, and where it
/// reports what it did: the in-process scenario (`Net`), or a seeded
/// distribution in a worker of its own process (the fleet simulator).
pub trait ActionSource: Send + Sync + core::fmt::Debug + 'static {
    fn plan(&self, start_execute: &StartExecute) -> Result<Plan, Error>;

    fn record(&self, event: Event);

    /// The scheduler asked for a kill. The in-process network already
    /// records it.
    fn killed(&self, _operation_id: &OperationId) {}
}

/// The `RunningActionsManager` of one simulated worker.
#[derive(Debug)]
pub struct SimActionsManager {
    worker: String,
    source: Arc<dyn ActionSource>,
    running: Arc<Mutex<HashMap<OperationId, Arc<KillSwitch>>>>,
    metrics: Arc<Metrics>,
}

impl SimActionsManager {
    pub fn new(worker: &str, source: Arc<dyn ActionSource>) -> Self {
        Self {
            worker: worker.to_string(),
            source,
            running: Arc::new(Mutex::new(HashMap::new())),
            metrics: Arc::new(Metrics::default()),
        }
    }

    /// Executions this worker has not finished.
    pub fn running(&self) -> usize {
        self.running.lock().len()
    }
}

#[derive(Debug, Default)]
struct KillSwitch {
    killed: AtomicBool,
    notify: Notify,
}

impl KillSwitch {
    fn kill(&self) {
        self.killed.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            if self.killed.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
}

impl RunningActionsManager for SimActionsManager {
    type RunningAction = SimRunningAction;

    async fn create_and_add_action(
        self: &Arc<Self>,
        _worker_id: String,
        start_execute: StartExecute,
    ) -> Result<Arc<SimRunningAction>, Error> {
        let plan = self.source.plan(&start_execute)?;
        let operation_id = OperationId::from(start_execute.operation_id);
        let kill = Arc::new(KillSwitch::default());
        self.running
            .lock()
            .insert(operation_id.clone(), kill.clone());
        Ok(Arc::new(SimRunningAction {
            worker: self.worker.clone(),
            action: plan.action,
            key: plan.key,
            operation_id,
            prepare: plan.prepare,
            run_for: plan.run_for,
            upload: plan.upload,
            run: plan.run,
            kill,
            running: self.running.clone(),
            source: self.source.clone(),
            work_directory: String::new(),
            started: AtomicBool::new(false),
            ended: AtomicBool::new(false),
        }))
    }

    async fn cache_action_result(
        &self,
        _action_digest: DigestInfo,
        _action_result: &mut ActionResult,
        _hasher: DigestHasherFunc,
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn kill_all(&self) {
        self.source.record(Event::KillAll {
            worker: self.worker.clone(),
        });
        let switches: Vec<_> = self.running.lock().values().cloned().collect();
        for switch in switches {
            switch.kill();
        }
    }

    async fn kill_operation(&self, operation_id: &OperationId) -> Result<(), Error> {
        self.source.killed(operation_id);
        let switch = self.running.lock().get(operation_id).cloned();
        if let Some(switch) = switch {
            switch.kill();
        }
        Ok(())
    }

    fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
    }
}

#[derive(Debug)]
pub struct SimRunningAction {
    worker: String,
    action: String,
    key: String,
    operation_id: OperationId,
    prepare: Duration,
    run_for: Duration,
    upload: Duration,
    run: Run,
    kill: Arc<KillSwitch>,
    running: Arc<Mutex<HashMap<OperationId, Arc<KillSwitch>>>>,
    source: Arc<dyn ActionSource>,
    work_directory: String,
    started: AtomicBool,
    ended: AtomicBool,
}

/// An execution whose task was aborted (the worker dropped it on a
/// disconnect) still ends in the trace and leaves the running set.
impl Drop for SimRunningAction {
    fn drop(&mut self) {
        if self.started.load(Ordering::Acquire) && !self.ended.load(Ordering::Acquire) {
            self.end(true);
        }
        self.running.lock().remove(&self.operation_id);
    }
}

impl SimRunningAction {
    /// Sleeps `duration` unless killed first; `Err` when killed.
    async fn phase(&self, duration: Duration) -> Result<(), Error> {
        if duration.is_zero() {
            return Ok(());
        }
        tokio::select! {
            () = tokio::time::sleep(duration) => Ok(()),
            () = self.kill.wait() => Err(make_err!(Code::Aborted, "Killed")),
        }
    }

    fn end(&self, killed: bool) {
        if self.ended.swap(true, Ordering::AcqRel) {
            return;
        }
        self.source.record(Event::ExecEnd {
            worker: self.worker.clone(),
            action: self.action.clone(),
            key: self.key.clone(),
            killed,
        });
    }
}

impl RunningAction for SimRunningAction {
    fn get_operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    async fn prepare_action(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        self.phase(self.prepare).await?;
        Ok(self)
    }

    async fn execute(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        self.started.store(true, Ordering::Release);
        self.source.record(Event::ExecStart {
            worker: self.worker.clone(),
            action: self.action.clone(),
            key: self.key.clone(),
        });
        let ran = match self.run {
            Run::Hang => {
                self.kill.wait().await;
                Err(make_err!(Code::Aborted, "Killed"))
            }
            _ => self.phase(self.run_for).await,
        };
        if let Err(err) = ran {
            self.end(true);
            return Err(err);
        }
        self.end(false);
        match self.run {
            Run::Panic => panic!("simulated action {} panicked", self.action),
            Run::Error(code) => Err(make_err!(code, "Simulated failure of {}", self.action)),
            Run::Succeed | Run::Exit(_) | Run::Hang => Ok(self),
        }
    }

    async fn upload_results(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        self.phase(self.upload).await?;
        Ok(self)
    }

    async fn cleanup(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        self.running.lock().remove(&self.operation_id);
        Ok(self)
    }

    async fn get_finished_result(self: Arc<Self>) -> Result<ActionResult, Error> {
        let exit_code = match self.run {
            Run::Exit(code) => code,
            _ => 0,
        };
        Ok(ActionResult {
            exit_code,
            ..ActionResult::default()
        })
    }

    fn resource_usage(&self) -> Option<ActionResourceUsage> {
        None
    }

    fn get_work_directory(&self) -> &String {
        &self.work_directory
    }
}
