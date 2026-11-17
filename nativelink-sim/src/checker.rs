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

//! The invariants, checked over a finished trace. Each is a pure function
//! of the records and the facts of the run, so a test can hand one a trace
//! it altered and see it fire.

use core::fmt;
use std::collections::{BTreeSet, HashMap, HashSet};

use crate::trace::{Event, Record, Stage};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Invariant {
    /// No result lost: a result a worker reported reaches every client still
    /// attached to the action.
    I1,
    /// At most one dispatch of an action at a time: a second needs the first
    /// declined, finished, killed or its worker gone in between.
    I2,
    /// Every dispatch is answered (accepted or declined, when the worker
    /// acknowledges), and an accepted one ends in exactly one result, a kill
    /// or the worker's loss.
    I3,
    /// What the scheduler has executing equals what the workers run, at
    /// quiescence.
    I4,
    /// Order: a dispatch never passes over a waiting action of higher
    /// priority, or of equal priority submitted earlier.
    I5,
    /// Liveness bounds: abandoned actions stop being dispatched, and a
    /// silent worker is evicted, within their bounds.
    I6,
    /// Retry budgets: an action is executed no more often than its budgets
    /// allow.
    I7,
    /// No stall: the final settle reaches quiescence inside its budget.
    I8,
    /// A worker never runs more than its `max_inflight_tasks`.
    I9,
    /// A client sees stages in order; the only back-edge is Executing to
    /// Queued.
    I10,
    /// Store commands between two matching passes stay inside the declared
    /// bound.
    I11,
}

impl fmt::Display for Invariant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Violation {
    pub invariant: Invariant,
    pub at_ms: u64,
    pub detail: String,
}

impl fmt::Display for Violation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} at {} ms: {}",
            self.invariant, self.at_ms, self.detail
        )
    }
}

/// What the checker needs to know about a run beyond its trace.
#[derive(Clone, Debug, Default)]
pub struct Facts {
    /// `max_inflight_tasks` by worker name.
    pub max_inflight: HashMap<String, u64>,
    /// Whether each action label is cacheable.
    pub cacheable: HashMap<String, bool>,
    /// The most executions one action may get; 0 does not check.
    pub dispatch_budget: u64,
    /// After every client of an action left, how long it may still be
    /// dispatched.
    pub abandon_bound_ms: u64,
    /// How long a silent worker may stay before it is evicted.
    pub evict_bound_ms: u64,
    /// Silences: worker, start and end.
    pub silences: Vec<(String, u64, u64)>,
    /// Check I5.
    pub ordered: bool,
    /// The bound I11 checks, and what the run measured.
    pub command_bound: Option<u64>,
    pub max_commands_per_pass: u64,
}

/// Every violation in `records`, in invariant order.
pub fn check(records: &[Record], facts: &Facts) -> Vec<Violation> {
    let quiet = records
        .iter()
        .rev()
        .find_map(|r| match r.event {
            Event::Settled { quiet } => Some(quiet),
            _ => None,
        })
        .unwrap_or(false);
    let end_ms = records.last().map_or(0, |r| r.at_ms);
    let mut out = Vec::new();
    check_i1(records, &mut out);
    check_i2(records, facts, &mut out);
    check_i3(records, quiet, &mut out);
    check_i4(records, quiet, &mut out);
    if facts.ordered {
        check_i5(records, &mut out);
    }
    check_i6(records, facts, end_ms, &mut out);
    check_i7(records, facts, &mut out);
    check_i8(records, quiet, end_ms, &mut out);
    check_i9(records, facts, &mut out);
    check_i10(records, &mut out);
    if let Some(bound) = facts.command_bound
        && facts.max_commands_per_pass > bound
    {
        out.push(Violation {
            invariant: Invariant::I11,
            at_ms: end_ms,
            detail: format!(
                "{} store commands between two passes, bound {bound}",
                facts.max_commands_per_pass
            ),
        });
    }
    out.sort_by_key(|v| (v.invariant, v.at_ms));
    out
}

/// The invariants `violations` name, once each.
pub fn verdict(violations: &[Violation]) -> BTreeSet<Invariant> {
    violations.iter().map(|v| v.invariant).collect()
}

struct Ops {
    /// op to action.
    action: HashMap<String, String>,
    /// op to submit time.
    submitted: HashMap<String, u64>,
    gone: HashMap<String, u64>,
    done: HashSet<String>,
}

fn ops(records: &[Record]) -> Ops {
    let mut ops = Ops {
        action: HashMap::new(),
        submitted: HashMap::new(),
        gone: HashMap::new(),
        done: HashSet::new(),
    };
    for r in records {
        match &r.event {
            Event::Submit { op, action, .. } => {
                ops.action.insert(op.clone(), action.clone());
                ops.submitted.entry(op.clone()).or_insert(r.at_ms);
            }
            Event::ClientGone { op } => {
                ops.gone.entry(op.clone()).or_insert(r.at_ms);
            }
            Event::ClientDone { op, .. } => {
                ops.done.insert(op.clone());
            }
            _ => {}
        }
    }
    ops
}

fn check_i1(records: &[Record], out: &mut Vec<Violation>) {
    let ops = ops(records);
    let mut first_result: HashMap<&str, u64> = HashMap::new();
    for r in records {
        if let Event::Result {
            action, ok: true, ..
        } = &r.event
        {
            first_result.entry(action.as_str()).or_insert(r.at_ms);
        }
    }
    let mut lost: Vec<_> = ops
        .action
        .iter()
        .filter(|(op, action)| {
            first_result
                .get(action.as_str())
                .is_some_and(|at| ops.submitted.get(*op).is_some_and(|s| s <= at))
                && !ops.gone.contains_key(*op)
                && !ops.done.contains(*op)
        })
        .map(|(op, action)| (first_result[action.as_str()], op.clone(), action.clone()))
        .collect();
    lost.sort();
    for (at_ms, op, action) in lost {
        out.push(Violation {
            invariant: Invariant::I1,
            at_ms,
            detail: format!(
                "a worker reported {action}'s result but attached client {op} never got an answer"
            ),
        });
    }
}

fn dispatch_key(facts: &Facts, action: &str, key: &str) -> String {
    if facts.cacheable.get(action).copied().unwrap_or(true) {
        action.to_string()
    } else {
        key.to_string()
    }
}

fn check_i2(records: &[Record], facts: &Facts, out: &mut Vec<Violation>) {
    // Dispatch key to the worker holding it.
    let mut holder: HashMap<String, String> = HashMap::new();
    for r in records {
        match &r.event {
            Event::Dispatch {
                worker,
                action,
                key,
            } => {
                let dkey = dispatch_key(facts, action, key);
                if let Some(first) = holder.get(&dkey) {
                    out.push(Violation {
                        invariant: Invariant::I2,
                        at_ms: r.at_ms,
                        detail: format!(
                            "{action} dispatched to {worker} while {first} still held it"
                        ),
                    });
                }
                holder.insert(dkey, worker.clone());
            }
            Event::Decline { worker, action }
            | Event::Result { worker, action, .. }
            | Event::Kill { worker, action } => {
                holder.retain(|k, w| !(w == worker && key_is_action(facts, k, action, records)));
            }
            Event::WorkerDisconnected { worker } | Event::WorkerStopped { worker } => {
                holder.retain(|_, w| w != worker);
            }
            _ => {}
        }
    }
}

fn check_i7(records: &[Record], facts: &Facts, out: &mut Vec<Violation>) {
    if facts.dispatch_budget == 0 {
        return;
    }
    let mut executions: HashMap<String, u64> = HashMap::new();
    for r in records {
        if let Event::ExecStart { action, key, .. } = &r.event {
            let count = executions
                .entry(dispatch_key(facts, action, key))
                .or_default();
            *count += 1;
            if *count == facts.dispatch_budget + 1 {
                out.push(Violation {
                    invariant: Invariant::I7,
                    at_ms: r.at_ms,
                    detail: format!(
                        "{action} executed {count} times, budget {}",
                        facts.dispatch_budget
                    ),
                });
            }
        }
    }
}

/// Whether dispatch key `key` belongs to `action`.
fn key_is_action(facts: &Facts, key: &str, action: &str, records: &[Record]) -> bool {
    if facts.cacheable.get(action).copied().unwrap_or(true) {
        return key == action;
    }
    records.iter().any(|r| {
        matches!(&r.event, Event::Dispatch { action: a, key: k, .. } if a == action && k == key)
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DispatchState {
    Sent,
    Accepted,
    /// Ended by a kill; the worker may still report the killed attempt.
    Killed,
}

fn check_i3(records: &[Record], quiet: bool, out: &mut Vec<Violation>) {
    let mut open: HashMap<(&str, &str), (DispatchState, u64)> = HashMap::new();
    for r in records {
        match &r.event {
            Event::Dispatch { worker, action, .. } => {
                open.insert((worker, action), (DispatchState::Sent, r.at_ms));
            }
            Event::Accept { worker, action } => {
                match open.get_mut(&(worker.as_str(), action.as_str())) {
                    Some((state @ DispatchState::Sent, _)) => *state = DispatchState::Accepted,
                    _ => out.push(Violation {
                        invariant: Invariant::I3,
                        at_ms: r.at_ms,
                        detail: format!("{worker} accepted {action}, which it was not sent"),
                    }),
                }
            }
            Event::Decline { worker, action } => {
                if open.remove(&(worker.as_str(), action.as_str())).is_none() {
                    out.push(Violation {
                        invariant: Invariant::I3,
                        at_ms: r.at_ms,
                        detail: format!("{worker} declined {action}, which it was not sent"),
                    });
                }
            }
            Event::Result { worker, action, .. } => {
                // A result answers the dispatch too: an accept the network
                // lost is the ack timeout's business, not this one's.
                match open.remove(&(worker.as_str(), action.as_str())) {
                    Some(_) => {}
                    None => out.push(Violation {
                        invariant: Invariant::I3,
                        at_ms: r.at_ms,
                        detail: format!(
                            "{worker} reported a result for {action} with no open dispatch"
                        ),
                    }),
                }
            }
            Event::Kill { worker, action } => {
                if let Some((state, _)) = open.get_mut(&(worker.as_str(), action.as_str())) {
                    *state = DispatchState::Killed;
                }
            }
            Event::WorkerDisconnected { worker } | Event::WorkerStopped { worker } => {
                open.retain(|(w, _), _| w != worker);
            }
            _ => {}
        }
    }
    if quiet {
        let mut left: Vec<_> = open
            .into_iter()
            .filter(|(_, (state, _))| *state != DispatchState::Killed)
            .collect();
        left.sort_by_key(|((w, a), (_, at))| (*at, *w, *a));
        for ((worker, action), (_, at_ms)) in left {
            out.push(Violation {
                invariant: Invariant::I3,
                at_ms,
                detail: format!("dispatch of {action} to {worker} never ended"),
            });
        }
    }
}

fn check_i4(records: &[Record], quiet: bool, out: &mut Vec<Violation>) {
    if !quiet {
        return;
    }
    for r in records {
        if let Event::Ledger {
            scheduler_executing,
            workers_running,
        } = r.event
            && scheduler_executing != workers_running
        {
            out.push(Violation {
                invariant: Invariant::I4,
                at_ms: r.at_ms,
                detail: format!(
                    "the scheduler has {scheduler_executing} executing, the workers run {workers_running}"
                ),
            });
        }
    }
}

fn check_i5(records: &[Record], out: &mut Vec<Violation>) {
    let ops = ops(records);
    // Per action: priority, first submit, ops.
    let mut priority: HashMap<&str, i32> = HashMap::new();
    let mut submitted: HashMap<&str, u64> = HashMap::new();
    for r in records {
        if let Event::Submit {
            action,
            priority: p,
            ..
        } = &r.event
        {
            priority.insert(action, *p);
            submitted.entry(action).or_insert(r.at_ms);
        }
    }
    let mut dispatched: HashSet<&str> = HashSet::new();
    for r in records {
        let Event::Dispatch { action, .. } = &r.event else {
            continue;
        };
        if !dispatched.insert(action.as_str()) {
            continue;
        }
        let (Some(&p), Some(&s)) = (
            priority.get(action.as_str()),
            submitted.get(action.as_str()),
        ) else {
            continue;
        };
        for (&other, &op_priority) in &priority {
            if other == action || dispatched.contains(other) {
                continue;
            }
            let other_submit = submitted[other];
            if other_submit > r.at_ms {
                continue;
            }
            let attached = ops
                .action
                .iter()
                .any(|(op, a)| a == other && ops.gone.get(op).is_none_or(|g| *g > r.at_ms));
            if attached && (op_priority > p || (op_priority == p && other_submit < s)) {
                out.push(Violation {
                    invariant: Invariant::I5,
                    at_ms: r.at_ms,
                    detail: format!(
                        "{action} (p{p}, at {s} ms) dispatched ahead of waiting {other} (p{op_priority}, at {other_submit} ms)"
                    ),
                });
            }
        }
    }
}

fn check_i6(records: &[Record], facts: &Facts, end_ms: u64, out: &mut Vec<Violation>) {
    let ops = ops(records);
    let mut abandoned: HashMap<&str, u64> = HashMap::new();
    let mut by_action: HashMap<&str, Vec<&str>> = HashMap::new();
    for (op, action) in &ops.action {
        by_action.entry(action).or_default().push(op);
    }
    for (action, action_ops) in by_action {
        let all_gone = action_ops.iter().all(|op| ops.gone.contains_key(*op));
        if all_gone && let Some(last) = action_ops.iter().filter_map(|op| ops.gone.get(*op)).max() {
            abandoned.insert(action, *last);
        }
    }
    for r in records {
        if let Event::Dispatch { worker, action, .. } = &r.event
            && let Some(&gone) = abandoned.get(action.as_str())
            && r.at_ms > gone + facts.abandon_bound_ms
        {
            out.push(Violation {
                invariant: Invariant::I6,
                at_ms: r.at_ms,
                detail: format!(
                    "{action} dispatched to {worker} {} ms after its last client left",
                    r.at_ms - gone
                ),
            });
        }
    }
    for (worker, start, end) in &facts.silences {
        let deadline = start + facts.evict_bound_ms;
        if end.min(&end_ms) < &deadline {
            continue;
        }
        // Only a worker with a connection when the silence began has
        // anything to evict.
        let connected = records
            .iter()
            .take_while(|r| r.at_ms <= *start)
            .filter_map(|r| match &r.event {
                Event::WorkerConnected { worker: w, .. } if w == worker => Some(true),
                Event::WorkerDisconnected { worker: w } | Event::WorkerStopped { worker: w }
                    if w == worker =>
                {
                    Some(false)
                }
                _ => None,
            })
            .last()
            .unwrap_or(false);
        if !connected {
            continue;
        }
        let evicted = records.iter().any(|r| {
            r.at_ms >= *start
                && r.at_ms <= deadline
                && matches!(&r.event, Event::WorkerDisconnected { worker: w } if w == worker)
        });
        if !evicted {
            out.push(Violation {
                invariant: Invariant::I6,
                at_ms: deadline,
                detail: format!(
                    "{worker} went silent at {start} ms and was not evicted within {} ms",
                    facts.evict_bound_ms
                ),
            });
        }
    }
}

fn check_i8(records: &[Record], quiet: bool, end_ms: u64, out: &mut Vec<Violation>) {
    if quiet {
        return;
    }
    let ops = ops(records);
    let mut waiting: Vec<_> = ops
        .action
        .keys()
        .filter(|op| !ops.gone.contains_key(*op) && !ops.done.contains(*op))
        .cloned()
        .collect();
    waiting.sort();
    out.push(Violation {
        invariant: Invariant::I8,
        at_ms: end_ms,
        detail: if waiting.is_empty() {
            "the settle budget ran out with executions still running".to_string()
        } else {
            let shown: Vec<_> = waiting.iter().take(10).cloned().collect();
            format!(
                "the settle budget ran out with {} waiting ({}{})",
                waiting.len(),
                shown.join(", "),
                if waiting.len() > shown.len() {
                    ", ..."
                } else {
                    ""
                }
            )
        },
    });
}

fn check_i9(records: &[Record], facts: &Facts, out: &mut Vec<Violation>) {
    let mut running: HashMap<&str, HashSet<&str>> = HashMap::new();
    for r in records {
        match &r.event {
            Event::ExecStart { worker, key, .. } => {
                let set = running.entry(worker).or_default();
                set.insert(key);
                let limit = facts
                    .max_inflight
                    .get(worker.as_str())
                    .copied()
                    .unwrap_or(1);
                if set.len() as u64 > limit {
                    out.push(Violation {
                        invariant: Invariant::I9,
                        at_ms: r.at_ms,
                        detail: format!("{worker} runs {} actions, limit {limit}", set.len()),
                    });
                }
            }
            Event::ExecEnd { worker, key, .. } => {
                if let Some(set) = running.get_mut(worker.as_str()) {
                    set.remove(key.as_str());
                }
            }
            Event::WorkerStopped { worker } => {
                running.remove(worker.as_str());
            }
            _ => {}
        }
    }
}

const fn rank(stage: Stage) -> Option<u8> {
    match stage {
        Stage::Unknown => None,
        Stage::CacheCheck => Some(0),
        Stage::Queued => Some(1),
        Stage::Executing => Some(2),
        Stage::Completed => Some(3),
    }
}

fn check_i10(records: &[Record], out: &mut Vec<Violation>) {
    let mut last: HashMap<&str, Stage> = HashMap::new();
    let mut done: HashSet<&str> = HashSet::new();
    for r in records {
        match &r.event {
            Event::ClientStage { op, stage } => {
                if done.contains(op.as_str()) {
                    out.push(Violation {
                        invariant: Invariant::I10,
                        at_ms: r.at_ms,
                        detail: format!("{op} saw {stage:?} after it was done"),
                    });
                    continue;
                }
                if let (Some(prev), Some(now)) =
                    (last.get(op.as_str()).and_then(|s| rank(*s)), rank(*stage))
                    && now < prev
                    && !(prev == 2 && now == 1)
                {
                    out.push(Violation {
                        invariant: Invariant::I10,
                        at_ms: r.at_ms,
                        detail: format!("{op} went from {:?} back to {stage:?}", last[op.as_str()]),
                    });
                }
                if rank(*stage).is_some() {
                    last.insert(op, *stage);
                }
            }
            Event::ClientReconnect { op, .. } | Event::ClientResubmit { op } => {
                last.remove(op.as_str());
            }
            Event::ClientDone { op, .. } => {
                done.insert(op);
            }
            _ => {}
        }
    }
}
