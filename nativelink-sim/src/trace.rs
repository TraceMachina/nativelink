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

//! Everything that crosses a boundary the harness owns, in the order it
//! happened: the client streams, the worker connections, the simulated
//! executions, the faults and the clock. Names are the scenario's own
//! (`op`, `action`, `worker`), never the ids the servers mint, so two runs
//! of one scenario can be compared line by line.

use core::fmt;
use std::sync::Arc;

use parking_lot::Mutex;

use crate::clock::SimClock;

/// The stage a client saw on its `Execute` or `WaitExecution` stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stage {
    Unknown,
    CacheCheck,
    Queued,
    Executing,
    Completed,
}

impl Stage {
    pub(crate) const fn from_proto(value: i32) -> Self {
        match value {
            1 => Self::CacheCheck,
            2 => Self::Queued,
            3 => Self::Executing,
            4 => Self::Completed,
            _ => Self::Unknown,
        }
    }
}

/// How a client's operation ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// The action ran and exited with this code.
    Exited(i32),
    /// The operation failed with this gRPC code.
    Failed(i32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// A client called `Execute` for `action`.
    Submit {
        op: String,
        action: String,
        priority: i32,
    },
    ClientStage {
        op: String,
        stage: Stage,
    },
    ClientDone {
        op: String,
        outcome: Outcome,
    },
    /// The client dropped its stream for good.
    ClientGone {
        op: String,
    },
    /// The stream ended or failed before the operation was done; the
    /// client re-subscribes with `WaitExecution`, as Bazel does.
    ClientReconnect {
        op: String,
        code: i32,
    },
    /// `WaitExecution` did not know the operation; the client calls
    /// `Execute` again.
    ClientResubmit {
        op: String,
    },
    /// The scheduler sent `StartExecute`. `key` is the scheduler's
    /// operation id, which a requeue keeps and a new execution does not.
    Dispatch {
        worker: String,
        action: String,
        key: String,
    },
    Accept {
        worker: String,
        action: String,
    },
    Decline {
        worker: String,
        action: String,
    },
    /// The worker released the slot (`ExecuteComplete`).
    Complete {
        worker: String,
        action: String,
    },
    /// The worker reported a result: `ok` is a completed stage, otherwise
    /// an internal error.
    Result {
        worker: String,
        action: String,
        ok: bool,
    },
    /// The scheduler asked the worker to kill the operation.
    Kill {
        worker: String,
        action: String,
    },
    ExecStart {
        worker: String,
        action: String,
        key: String,
    },
    ExecEnd {
        worker: String,
        action: String,
        key: String,
        killed: bool,
    },
    /// The worker's `kill_all`, after it lost its connection.
    KillAll {
        worker: String,
    },
    WorkerStarted {
        worker: String,
    },
    /// The scheduler answered the connection; `dispatch_ack` says the worker
    /// acknowledges dispatches.
    WorkerConnected {
        worker: String,
        dispatch_ack: bool,
    },
    /// The scheduler's stream to the worker ended (eviction, restart,
    /// a cut link).
    WorkerDisconnected {
        worker: String,
    },
    /// The worker process is gone.
    WorkerStopped {
        worker: String,
    },
    Fault {
        what: String,
    },
    SchedulerUp,
    SchedulerDown,
    /// What the clock reads jumped forward; timers did not move.
    ClockJump {
        ms: u64,
    },
    /// What the scheduler and the workers held at the end: operations the
    /// scheduler has executing, and executions the workers are running.
    Ledger {
        scheduler_executing: usize,
        workers_running: usize,
    },
    Settled {
        quiet: bool,
    },
}

impl fmt::Display for Event {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Submit {
                op,
                action,
                priority,
            } => write!(f, "submit {op} {action} p{priority}"),
            Self::ClientStage { op, stage } => write!(f, "stage {op} {stage:?}"),
            Self::ClientDone { op, outcome } => write!(f, "done {op} {outcome:?}"),
            Self::ClientGone { op } => write!(f, "gone {op}"),
            Self::ClientReconnect { op, code } => write!(f, "reconnect {op} code {code}"),
            Self::ClientResubmit { op } => write!(f, "resubmit {op}"),
            Self::Dispatch {
                worker,
                action,
                key,
            } => write!(f, "dispatch {worker} {action} {key}"),
            Self::Accept { worker, action } => write!(f, "accept {worker} {action}"),
            Self::Decline { worker, action } => write!(f, "decline {worker} {action}"),
            Self::Complete { worker, action } => write!(f, "complete {worker} {action}"),
            Self::Result { worker, action, ok } => write!(f, "result {worker} {action} ok={ok}"),
            Self::Kill { worker, action } => write!(f, "kill {worker} {action}"),
            Self::ExecStart {
                worker,
                action,
                key,
            } => write!(f, "exec-start {worker} {action} {key}"),
            Self::ExecEnd {
                worker,
                action,
                key,
                killed,
            } => write!(f, "exec-end {worker} {action} {key} killed={killed}"),
            Self::KillAll { worker } => write!(f, "kill-all {worker}"),
            Self::WorkerStarted { worker } => write!(f, "worker-started {worker}"),
            Self::WorkerConnected {
                worker,
                dispatch_ack,
            } => write!(f, "worker-connected {worker} ack={dispatch_ack}"),
            Self::WorkerDisconnected { worker } => write!(f, "worker-disconnected {worker}"),
            Self::WorkerStopped { worker } => write!(f, "worker-stopped {worker}"),
            Self::Fault { what } => write!(f, "fault {what}"),
            Self::SchedulerUp => write!(f, "scheduler-up"),
            Self::SchedulerDown => write!(f, "scheduler-down"),
            Self::ClockJump { ms } => write!(f, "clock-jump +{ms}ms"),
            Self::Ledger {
                scheduler_executing,
                workers_running,
            } => write!(
                f,
                "ledger scheduler={scheduler_executing} workers={workers_running}"
            ),
            Self::Settled { quiet } => write!(f, "settled quiet={quiet}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub at_ms: u64,
    pub event: Event,
}

impl fmt::Display for Record {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:>9} {}", self.at_ms, self.event)
    }
}

/// The shared trace. Cheap to clone; every component holds one.
#[derive(Clone, Debug, Default)]
pub struct Trace(Arc<Mutex<Vec<Record>>>);

impl Trace {
    pub fn record(&self, event: Event) {
        self.0.lock().push(Record {
            at_ms: SimClock::now_ms(),
            event,
        });
    }

    pub fn records(&self) -> Vec<Record> {
        self.0.lock().clone()
    }

    pub fn len(&self) -> usize {
        self.0.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.lock().is_empty()
    }
}

/// The trace as text, one record per line.
pub fn render(records: &[Record]) -> String {
    let mut out = String::new();
    for record in records {
        out.push_str(&record.to_string());
        out.push('\n');
    }
    out
}
