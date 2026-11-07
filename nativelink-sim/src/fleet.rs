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

//! The fleet simulator's workers (RFC-0061): production `LocalWorker`s over
//! the production gRPC client, against a real scheduler, with execution
//! drawn from a seeded distribution instead of a scenario.

use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;

use nativelink_config::cas_server::{EndpointConfig, LocalWorkerConfig, WorkerProperty};
use nativelink_error::Error;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::StartExecute;
use nativelink_util::action_messages::OperationId;
use nativelink_util::background_spawn;
use nativelink_util::shutdown_guard::ShutdownGuard;
use nativelink_worker::local_worker::{LocalWorker, connect_to_scheduler};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use crate::actions::{ActionSource, Plan, Run, SimActionsManager};
use crate::property::Rng;
use crate::trace::Event;

/// What the fleet did, counted as it happens.
#[derive(Debug, Default)]
pub struct Counters {
    /// Connection attempts to the scheduler.
    pub connects: AtomicU64,
    /// Actions a worker accepted and started.
    pub started: AtomicU64,
    /// Executions that ran to the end.
    pub finished: AtomicU64,
    /// Executions drawn to exit non-zero.
    pub failed: AtomicU64,
    /// Executions cut short by a kill or a disconnect.
    pub killed: AtomicU64,
    /// Kill requests from the scheduler.
    pub kill_requests: AtomicU64,
    /// `kill_all`, after a worker lost its connection.
    pub kill_all_calls: AtomicU64,
}

impl fmt::Display for Counters {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let get = |c: &AtomicU64| c.load(Ordering::Relaxed);
        write!(
            f,
            "connects={} started={} finished={} failed={} killed={} kill_requests={} kill_all_calls={}",
            get(&self.connects),
            get(&self.started),
            get(&self.finished),
            get(&self.failed),
            get(&self.killed),
            get(&self.kill_requests),
            get(&self.kill_all_calls),
        )
    }
}

/// Execution drawn per operation: a run time uniform in `run_ms` and a
/// non-zero exit for `fail_percent` of operations. The draw is a function
/// of the seed and the operation id, so a requeued operation behaves the
/// same on whichever worker gets it.
#[derive(Debug)]
pub struct Distribution {
    pub seed: u64,
    pub run_ms: (u64, u64),
    pub fail_percent: u64,
    pub counters: Arc<Counters>,
}

/// FNV-1a, for a stable draw per operation.
fn fnv1a(seed: u64, text: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325 ^ seed;
    for byte in text.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

impl Distribution {
    /// The duration and outcome of `operation_id`.
    pub fn draw(&self, operation_id: &str) -> (Duration, Run) {
        let mut rng = Rng::new(fnv1a(self.seed, operation_id));
        let (low, high) = self.run_ms;
        let run_for = Duration::from_millis(rng.range(low, high.max(low)));
        let run = if self.fail_percent > 0 && rng.chance(self.fail_percent) {
            Run::Exit(1)
        } else {
            Run::Succeed
        };
        (run_for, run)
    }
}

impl ActionSource for Distribution {
    fn plan(&self, start_execute: &StartExecute) -> Result<Plan, Error> {
        let (run_for, run) = self.draw(&start_execute.operation_id);
        self.counters.started.fetch_add(1, Ordering::Relaxed);
        if run != Run::Succeed {
            self.counters.failed.fetch_add(1, Ordering::Relaxed);
        }
        Ok(Plan {
            action: start_execute.operation_id.clone(),
            key: start_execute.operation_id.clone(),
            prepare: Duration::ZERO,
            run_for,
            upload: Duration::ZERO,
            run,
        })
    }

    fn record(&self, event: Event) {
        let counter = match event {
            Event::ExecEnd { killed: true, .. } => &self.counters.killed,
            Event::ExecEnd { .. } => &self.counters.finished,
            Event::KillAll { .. } => &self.counters.kill_all_calls,
            _ => return,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn killed(&self, _operation_id: &OperationId) {
        self.counters.kill_requests.fetch_add(1, Ordering::Relaxed);
    }
}

/// One simulated worker's settings.
#[derive(Debug, Clone)]
pub struct FleetWorker {
    pub name: String,
    pub scheduler: String,
    pub max_inflight: u64,
    pub properties: HashMap<String, String>,
}

impl FleetWorker {
    fn config(&self) -> LocalWorkerConfig {
        LocalWorkerConfig {
            name: self.name.clone(),
            max_inflight_tasks: self.max_inflight,
            platform_properties: self
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), WorkerProperty::Values(vec![v.clone()])))
                .collect(),
            worker_api_endpoint: EndpointConfig {
                uri: self.scheduler.clone(),
                ..Default::default()
            },
            ..Default::default()
        }
    }
}

/// A running simulated worker; dropping it does not stop it, `stop` does.
#[derive(Debug)]
pub struct RunningWorker {
    task: JoinHandle<Result<(), Error>>,
    // Held so the worker's shutdown channel stays open.
    _shutdown: broadcast::Sender<ShutdownGuard>,
}

impl RunningWorker {
    /// The process dies: the task is aborted and its connection closes.
    pub fn stop(&self) {
        self.task.abort();
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

/// Starts `worker` with execution from `source`.
pub fn spawn_worker(
    worker: &FleetWorker,
    source: Arc<dyn ActionSource>,
    counters: Arc<Counters>,
) -> RunningWorker {
    let config = Arc::new(worker.config());
    let factory_config = config.clone();
    let local_worker = LocalWorker::new_with_connection_factory_and_actions_manager(
        config,
        Arc::new(SimActionsManager::new(&worker.name, source)),
        Box::new(move || {
            let config = factory_config.clone();
            let counters = counters.clone();
            Box::pin(async move {
                counters.connects.fetch_add(1, Ordering::Relaxed);
                connect_to_scheduler(&config).await
            })
        }),
        Box::new(|duration| Box::pin(tokio::time::sleep(duration))),
    );
    let (shutdown, shutdown_rx) = broadcast::channel(1);
    let task = background_spawn!("fleet_worker", local_worker.run(shutdown_rx));
    RunningWorker {
        task,
        _shutdown: shutdown,
    }
}
