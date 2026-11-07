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

//! Scenarios as Rust builders, and the runner. Each backend runs on a
//! thread of its own with a fresh current-thread runtime in paused time, so
//! a run's clock and tasks end with it.

use core::fmt::Write as _;
use core::time::Duration;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use nativelink_config::schedulers::SimpleSpec;
use nativelink_error::{Code, Error, make_err};
use nativelink_redis_tester::Fault;

use crate::actions::Action;
use crate::checker::{Facts, Invariant, Violation, check, verdict};
use crate::clock::SimClock;
use crate::net::Link;
use crate::trace::{Event, Outcome, Record, Trace, render};
use crate::world::{Backend, WorkerSpec, World};

/// One thing that happens at a point of the timeline.
#[derive(Clone, Debug)]
pub enum Step {
    /// Client `op` calls `Execute` for `action`.
    Execute {
        op: String,
        action: Action,
    },
    /// Client `op` calls `Execute` for the action client `of` asked for.
    ExecuteSame {
        op: String,
        of: String,
    },
    /// Client `op` drops its stream and does not come back.
    ClientGone {
        op: String,
    },
    AddWorkers {
        count: usize,
        spec: WorkerSpec,
    },
    /// The worker process dies.
    KillWorker {
        worker: usize,
    },
    /// The worker's connection closes; the worker reconnects.
    CutWorker {
        worker: usize,
    },
    /// The worker's link carries nothing (`Silent`) or works again (`Up`).
    SetLink {
        worker: usize,
        link: Link,
    },
    /// The scheduler process dies and a new one starts at once.
    SchedulerRestart,
    SchedulerDown,
    SchedulerUp,
    /// Every Redis connection closes, then writes answer `READONLY`.
    RedisFailover {
        readonly_writes: u64,
    },
    /// Redis connections close and are refused (`true`) or accepted again.
    RedisPartition {
        partitioned: bool,
    },
    /// `command` (`*` for any) answers `fault` `times` times.
    RedisFail {
        command: String,
        fault: Fault,
        times: u64,
    },
    /// Every Redis key starting with `prefix` is deleted.
    RedisEvict {
        prefix: String,
    },
    /// Every Redis reply waits this long (`None` clears it).
    RedisLatency {
        ms: Option<u64>,
    },
    /// What the clock reads jumps forward; timers do not move.
    ClockJump {
        ms: u64,
    },
}

impl Step {
    pub fn execute(op: &str, action: Action) -> Self {
        Self::Execute {
            op: op.to_string(),
            action,
        }
    }

    pub fn execute_same(op: &str, of: &str) -> Self {
        Self::ExecuteSame {
            op: op.to_string(),
            of: of.to_string(),
        }
    }

    pub fn client_gone(op: &str) -> Self {
        Self::ClientGone { op: op.to_string() }
    }

    pub const fn add_workers(count: usize, spec: WorkerSpec) -> Self {
        Self::AddWorkers { count, spec }
    }

    pub fn redis_fail(command: &str, fault: Fault, times: u64) -> Self {
        Self::RedisFail {
            command: command.to_string(),
            fault,
            times,
        }
    }

    pub fn redis_evict(prefix: &str) -> Self {
        Self::RedisEvict {
            prefix: prefix.to_string(),
        }
    }

    const fn needs_redis(&self) -> bool {
        matches!(
            self,
            Self::RedisFailover { .. }
                | Self::RedisPartition { .. }
                | Self::RedisFail { .. }
                | Self::RedisEvict { .. }
                | Self::RedisLatency { .. }
        )
    }

    /// The step as the builder call that makes it, for printing a shrunk
    /// property case.
    pub fn to_code(&self) -> String {
        match self {
            Self::Execute { op, action } => {
                let mut code = format!("Step::execute({op:?}, Action::new({})", action.seed);
                if action.priority != 0 {
                    write!(code, ".priority({})", action.priority).unwrap();
                }
                if !action.cacheable {
                    code.push_str(".uncacheable()");
                }
                write!(code, ".run_ms({})", action.run.as_millis()).unwrap();
                if action.attempts.len() > 1 || action.attempts[0] != crate::actions::Run::Succeed {
                    write!(code, ".attempts(&{:?})", action.attempts).unwrap();
                }
                code.push(')');
                code
            }
            Self::ExecuteSame { op, of } => format!("Step::execute_same({op:?}, {of:?})"),
            Self::ClientGone { op } => format!("Step::client_gone({op:?})"),
            Self::AddWorkers { count, spec } => format!(
                "Step::add_workers({count}, WorkerSpec::default().max_inflight({}))",
                spec.max_inflight
            ),
            Self::KillWorker { worker } => format!("Step::KillWorker {{ worker: {worker} }}"),
            Self::CutWorker { worker } => format!("Step::CutWorker {{ worker: {worker} }}"),
            Self::SetLink { worker, link } => {
                format!("Step::SetLink {{ worker: {worker}, link: Link::{link:?} }}")
            }
            Self::SchedulerRestart => "Step::SchedulerRestart".to_string(),
            Self::SchedulerDown => "Step::SchedulerDown".to_string(),
            Self::SchedulerUp => "Step::SchedulerUp".to_string(),
            Self::RedisFailover { readonly_writes } => {
                format!("Step::RedisFailover {{ readonly_writes: {readonly_writes} }}")
            }
            Self::RedisPartition { partitioned } => {
                format!("Step::RedisPartition {{ partitioned: {partitioned} }}")
            }
            Self::RedisFail {
                command,
                fault,
                times,
            } => format!("Step::redis_fail({command:?}, Fault::{fault:?}, {times})"),
            Self::RedisEvict { prefix } => format!("Step::redis_evict({prefix:?})"),
            Self::RedisLatency { ms } => format!("Step::RedisLatency {{ ms: {ms:?} }}"),
            Self::ClockJump { ms } => format!("Step::ClockJump {{ ms: {ms} }}"),
        }
    }
}

/// What a scenario should do on one backend.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Expect {
    Pass,
    /// Fails exactly these invariants until `closed_by` lands.
    KnownFailure {
        invariants: BTreeSet<Invariant>,
        closed_by: &'static str,
    },
    /// Not run on this backend.
    NotApplicable,
}

impl Expect {
    pub fn known_failure(invariants: &[Invariant], closed_by: &'static str) -> Self {
        Self::KnownFailure {
            invariants: invariants.iter().copied().collect(),
            closed_by,
        }
    }

    fn invariants(&self) -> BTreeSet<Invariant> {
        match self {
            Self::KnownFailure { invariants, .. } => invariants.clone(),
            Self::Pass | Self::NotApplicable => BTreeSet::new(),
        }
    }
}

type SpecEdit = Arc<dyn Fn(&mut SimpleSpec) + Send + Sync>;

#[derive(Clone)]
pub struct Scenario {
    name: String,
    seed: u64,
    backends: Vec<Backend>,
    /// Applied in order to the default `SimpleSpec`; the spec itself is not
    /// `Clone`.
    spec_edits: Vec<SpecEdit>,
    steps: Vec<(u64, Step)>,
    settle: Duration,
    expect: BTreeMap<Backend, Expect>,
    ordered: bool,
    command_bound: Option<u64>,
    /// The backends are known to end operations differently until this
    /// item closes.
    disagree: Option<&'static str>,
}

impl core::fmt::Debug for Scenario {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Scenario")
            .field("name", &self.name)
            .field("seed", &self.seed)
            .field("steps", &self.steps)
            .finish_non_exhaustive()
    }
}

impl Scenario {
    pub fn new(name: &str, seed: u64) -> Self {
        Self {
            name: name.to_string(),
            seed,
            backends: vec![Backend::Memory, Backend::FakeRedis],
            spec_edits: Vec::new(),
            steps: Vec::new(),
            settle: Duration::from_mins(1),
            expect: BTreeMap::new(),
            ordered: false,
            command_bound: None,
            disagree: None,
        }
    }

    /// The backends are known to end operations differently until
    /// `closed_by` lands; the run fails if they agree.
    #[must_use]
    pub const fn backends_disagree(mut self, closed_by: &'static str) -> Self {
        self.disagree = Some(closed_by);
        self
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn backends(mut self, backends: &[Backend]) -> Self {
        self.backends = backends.to_vec();
        self
    }

    /// Changes the scheduler's config.
    #[must_use]
    pub fn spec(mut self, edit: impl Fn(&mut SimpleSpec) + Send + Sync + 'static) -> Self {
        self.spec_edits.push(Arc::new(edit));
        self
    }

    fn make_spec(&self) -> SimpleSpec {
        let mut spec = SimpleSpec::default();
        for edit in &self.spec_edits {
            edit(&mut spec);
        }
        spec
    }

    /// `count` workers at time zero.
    #[must_use]
    pub fn workers(self, count: usize, spec: WorkerSpec) -> Self {
        self.at(0, Step::add_workers(count, spec))
    }

    #[must_use]
    pub fn at(mut self, ms: u64, step: Step) -> Self {
        self.steps.push((ms, step));
        self
    }

    /// The worker's link is silent from `ms` for `for_ms`.
    #[must_use]
    pub fn silence(self, worker: usize, ms: u64, for_ms: u64) -> Self {
        self.at(
            ms,
            Step::SetLink {
                worker,
                link: Link::Silent,
            },
        )
        .at(
            ms + for_ms,
            Step::SetLink {
                worker,
                link: Link::Up,
            },
        )
    }

    /// Redis is unreachable from `ms` for `for_ms`.
    #[must_use]
    pub fn redis_partition(self, ms: u64, for_ms: u64) -> Self {
        self.at(ms, Step::RedisPartition { partitioned: true })
            .at(ms + for_ms, Step::RedisPartition { partitioned: false })
    }

    /// How long the final settle may take.
    #[must_use]
    pub const fn settle(mut self, budget: Duration) -> Self {
        self.settle = budget;
        self
    }

    /// Check dispatch order (I5).
    #[must_use]
    pub const fn ordered(mut self) -> Self {
        self.ordered = true;
        self
    }

    /// The I11 bound on store commands between two matching passes,
    /// measured from one second after the last `Execute` step, so it reads
    /// the steady cost rather than the submissions.
    #[must_use]
    pub const fn command_bound(mut self, bound: u64) -> Self {
        self.command_bound = Some(bound);
        self
    }

    #[must_use]
    pub fn expect(mut self, backend: Backend, expect: Expect) -> Self {
        self.expect.insert(backend, expect);
        self
    }

    pub fn steps(&self) -> &[(u64, Step)] {
        &self.steps
    }

    #[must_use]
    pub fn with_steps(mut self, steps: Vec<(u64, Step)>) -> Self {
        self.steps = steps;
        self
    }

    fn expectation(&self, backend: Backend) -> Expect {
        if backend == Backend::Memory && self.steps.iter().any(|(_, s)| s.needs_redis()) {
            return Expect::NotApplicable;
        }
        self.expect.get(&backend).cloned().unwrap_or(Expect::Pass)
    }

    /// Runs on every backend and checks each verdict against its
    /// expectation, and the backends against each other.
    pub fn run(&self) -> Result<Vec<Report>, Error> {
        let mut reports = Vec::new();
        for &backend in &self.backends {
            if self.expectation(backend) == Expect::NotApplicable {
                continue;
            }
            let report = self.run_on(backend)?;
            // Full traces for a reader, when asked for.
            if let Some(dir) = std::env::var_os("SIM_DUMP_DIR") {
                let path =
                    std::path::Path::new(&dir).join(format!("{}-{backend:?}.trace", self.name));
                drop(std::fs::write(path, report.trace()));
            }
            reports.push(report);
        }
        let mut problems = String::new();
        for report in &reports {
            let expected = self.expectation(report.backend).invariants();
            let actual = verdict(&report.violations);
            if expected != actual {
                writeln!(
                    problems,
                    "{} on {:?}: expected {} but got {}",
                    self.name,
                    report.backend,
                    names(&expected),
                    names(&actual)
                )
                .unwrap();
                for violation in &report.violations {
                    writeln!(problems, "  {violation}").unwrap();
                }
                writeln!(problems, "  trace:\n{}", report.tail(40)).unwrap();
            }
        }
        if problems.is_empty() {
            self.check_agreement(&reports, &mut problems);
        }
        if problems.is_empty() {
            self.record_known_failures(&reports);
            Ok(reports)
        } else {
            Err(make_err!(Code::Internal, "{problems}"))
        }
    }

    /// Appends each known failure this run confirmed, with the item that
    /// closes it, to the file `SIM_KNOWN_FAILURES` names. The nightly prints
    /// that file as the SIM column of STATUS.md.
    fn record_known_failures(&self, reports: &[Report]) {
        let Some(path) = std::env::var_os("SIM_KNOWN_FAILURES") else {
            return;
        };
        let mut lines = String::new();
        for report in reports {
            if let Expect::KnownFailure {
                invariants,
                closed_by,
            } = self.expectation(report.backend)
            {
                writeln!(
                    lines,
                    "{}\t{:?}\t{}\t{closed_by}",
                    self.name,
                    report.backend,
                    names(&invariants)
                )
                .unwrap();
            }
        }
        if let Some(closed_by) = self.disagree {
            writeln!(lines, "{}\tbackends disagree\t\t{closed_by}", self.name).unwrap();
        }
        if lines.is_empty() {
            return;
        }
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path);
        if let Ok(mut file) = file {
            drop(std::io::Write::write_all(&mut file, lines.as_bytes()));
        }
    }

    /// Where the backends must agree: when their expectations are the same,
    /// every operation ends the same way, and an ordered scenario dispatches
    /// in the same order.
    fn check_agreement(&self, reports: &[Report], problems: &mut String) {
        let Some((first, rest)) = reports.split_first() else {
            return;
        };
        for other in rest {
            if self.expectation(first.backend) != self.expectation(other.backend) {
                continue;
            }
            if let Some(closed_by) = self.disagree {
                if first.outcomes() == other.outcomes() {
                    writeln!(
                        problems,
                        "{}: {:?} and {:?} were expected to disagree until {closed_by}, but agree",
                        self.name, first.backend, other.backend
                    )
                    .unwrap();
                }
            } else if first.outcomes() != other.outcomes() {
                writeln!(
                    problems,
                    "{}: {:?} and {:?} disagree on outcomes:\n  {:?}\n  {:?}",
                    self.name,
                    first.backend,
                    other.backend,
                    first.outcomes(),
                    other.outcomes()
                )
                .unwrap();
            }
            if self.ordered && first.dispatch_order() != other.dispatch_order() {
                writeln!(
                    problems,
                    "{}: {:?} and {:?} dispatch in different orders:\n  {:?}\n  {:?}",
                    self.name,
                    first.backend,
                    other.backend,
                    first.dispatch_order(),
                    other.dispatch_order()
                )
                .unwrap();
            }
        }
    }

    /// Runs on one backend without checking expectations.
    pub fn run_on(&self, backend: Backend) -> Result<Report, Error> {
        let scenario = self.clone();
        #[expect(
            clippy::disallowed_methods,
            reason = "each run needs its own thread for its thread-local clock and its own runtime"
        )]
        let handle = std::thread::Builder::new()
            .name(format!("sim-{}-{backend:?}", self.name))
            .stack_size(16 * 1024 * 1024)
            .spawn(move || {
                #[expect(
                    clippy::disallowed_methods,
                    reason = "a paused current-thread runtime per run"
                )]
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .start_paused(true)
                    // Reads the fake Redis's socket after every task, so its
                    // replies land in the tick they were sent in.
                    .event_interval(1)
                    .build()
                    .map_err(|e| make_err!(Code::Internal, "Could not build a runtime: {e}"))?;
                #[expect(
                    clippy::disallowed_methods,
                    reason = "the run owns this runtime and nothing else blocks on it"
                )]
                runtime.block_on(scenario.run_async(backend))
            })
            .map_err(|e| make_err!(Code::Internal, "Could not start the run's thread: {e}"))?;
        handle
            .join()
            .map_err(|_| make_err!(Code::Internal, "{} on {backend:?} panicked", self.name))?
    }

    async fn run_async(self, backend: Backend) -> Result<Report, Error> {
        let clock = SimClock::start();
        // See `clock`: on the fake Redis the runtime must never idle, and
        // time steps by hand.
        let _awake = (backend == Backend::FakeRedis).then(|| {
            nativelink_util::spawn!("sim_keep_awake", async {
                loop {
                    tokio::task::yield_now().await;
                }
            })
        });
        let trace = Trace::default();
        let mut world = World::new(backend, self.make_spec(), self.seed, trace.clone()).await?;
        let clock = match world.redis() {
            Some(redis) => {
                let redis = redis.clone();
                clock.stepped(Arc::new(move || redis.commands_total()))
            }
            None => clock,
        };
        let mut steps = self.steps.clone();
        steps.sort_by_key(|(at, _)| *at);
        if let Some(last_submit) = steps
            .iter()
            .filter(|(_, s)| matches!(s, Step::Execute { .. } | Step::ExecuteSame { .. }))
            .map(|(at, _)| *at)
            .max()
        {
            world.measure_from(last_submit + 1_000);
        }
        let mut silences: Vec<(String, u64, u64)> = Vec::new();
        for (at, step) in steps {
            let now = SimClock::now_ms();
            if at > now {
                clock.advance(Duration::from_millis(at - now)).await;
            }
            // Silences are the periods a link is silent: a second silence
            // while silent changes nothing, and any other setting ends it.
            if let Step::SetLink { worker, link } = &step {
                let name = crate::world::worker_name(*worker);
                let open = silences
                    .iter_mut()
                    .find(|(w, _, end)| *w == name && *end == u64::MAX);
                match (open, *link == Link::Silent) {
                    (None, true) => silences.push((name, at, u64::MAX)),
                    (Some(open), false) => open.2 = at,
                    _ => {}
                }
            }
            apply(&mut world, &step).await?;
            // What the step woke runs at the step's time, not the next tick's.
            SimClock::yield_all().await;
        }
        let quiet = world.settle(&clock, self.settle).await;
        trace.record(Event::Settled { quiet });
        world.record_ledger().await?;

        let spec = world.spec();
        let facts = Facts {
            max_inflight: world
                .worker_specs()
                .into_iter()
                .map(|(name, spec)| (name, spec.max_inflight))
                .collect(),
            cacheable: trace
                .records()
                .iter()
                .filter_map(|r| match &r.event {
                    Event::Submit { op, action, .. } => {
                        world.action_of(op).map(|a| (action.clone(), a.cacheable))
                    }
                    _ => None,
                })
                .collect(),
            dispatch_budget: dispatch_budget(spec),
            abandon_bound_ms: abandon_bound_ms(spec),
            evict_bound_ms: evict_bound_ms(spec),
            silences,
            ordered: self.ordered,
            command_bound: self.command_bound,
            max_commands_per_pass: world.max_commands_per_pass(),
        };
        let passes = world.passes();
        world.shutdown();
        let records = trace.records();
        let violations = check(&records, &facts);
        Ok(Report {
            backend,
            records,
            facts,
            violations,
            passes,
        })
    }
}

fn names(invariants: &BTreeSet<Invariant>) -> String {
    if invariants.is_empty() {
        return "pass".to_string();
    }
    invariants
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// Executions an action may get: the first, the job retries and the
/// worker-loss retries, at the scheduler's defaults when unset.
const fn dispatch_budget(spec: &SimpleSpec) -> u64 {
    let job = if spec.max_job_retries == 0 {
        3
    } else {
        spec.max_job_retries
    };
    let loss = if spec.max_worker_loss_retries == 0 {
        nativelink_scheduler::simple_scheduler_state_manager::RetryLimits::DEFAULT_WORKER_LOSS_RETRIES
    } else {
        spec.max_worker_loss_retries
    };
    1 + job as u64 + loss as u64
}

/// The client keepalive (10 s) after the client action timeout, plus the
/// 5 s retirement pass.
const fn abandon_bound_ms(spec: &SimpleSpec) -> u64 {
    let timeout_s = if spec.client_action_timeout_s == 0 {
        60
    } else {
        spec.client_action_timeout_s
    };
    (timeout_s + 10 + 5) * 1000
}

/// The worker timeout, plus the 1 s eviction tick and one keepalive.
const fn evict_bound_ms(spec: &SimpleSpec) -> u64 {
    let timeout_s = if spec.worker_timeout_s == 0 {
        10
    } else {
        spec.worker_timeout_s
    };
    (timeout_s + 2) * 1000
}

async fn apply(world: &mut World, step: &Step) -> Result<(), Error> {
    let fault = |what: String| Event::Fault { what };
    match step {
        Step::Execute { op, action } => world.execute(op, action).await?,
        Step::ExecuteSame { op, of } => {
            let action = world
                .action_of(of)
                .ok_or_else(|| make_err!(Code::InvalidArgument, "No client named {of}"))?;
            world.execute(op, &action).await?;
        }
        Step::ClientGone { op } => world.client_gone(op),
        Step::AddWorkers { count, spec } => world.add_workers(*count, spec),
        Step::KillWorker { worker } => world.kill_worker(*worker),
        Step::CutWorker { worker } => {
            world.trace().record(fault(format!("cut w{worker}")));
            world.cut_worker(*worker);
        }
        Step::SetLink { worker, link } => {
            world
                .trace()
                .record(fault(format!("link w{worker} {link:?}")));
            world.set_link(*worker, *link);
        }
        Step::SchedulerRestart => {
            world.scheduler_down();
            world.scheduler_up().await?;
        }
        Step::SchedulerDown => world.scheduler_down(),
        Step::SchedulerUp => world.scheduler_up().await?,
        Step::RedisFailover { readonly_writes } => {
            world
                .trace()
                .record(fault(format!("redis failover readonly={readonly_writes}")));
            redis(world)?.failover(*readonly_writes);
        }
        Step::RedisPartition { partitioned } => {
            world
                .trace()
                .record(fault(format!("redis partition={partitioned}")));
            redis(world)?.partition(*partitioned);
        }
        Step::RedisFail {
            command,
            fault: kind,
            times,
        } => {
            world
                .trace()
                .record(fault(format!("redis {command} {kind:?} x{times}")));
            redis(world)?.fail_commands(command, None, kind.clone(), Some(*times));
        }
        Step::RedisEvict { prefix } => {
            let evicted = redis(world)?.evict_prefix(prefix);
            world
                .trace()
                .record(fault(format!("redis evict {prefix:?} ({evicted} keys)")));
        }
        Step::RedisLatency { ms } => {
            world
                .trace()
                .record(fault(format!("redis latency {ms:?} ms")));
            redis(world)?.set_latency(ms.map(Duration::from_millis));
        }
        Step::ClockJump { ms } => {
            SimClock::jump(Duration::from_millis(*ms));
            world.trace().record(Event::ClockJump { ms: *ms });
        }
    }
    Ok(())
}

fn redis(
    world: &World,
) -> Result<
    &nativelink_redis_tester::FakeRedisBackend<
        nativelink_store::redis_store::RedisSubscriptionManager,
    >,
    Error,
> {
    world.redis().ok_or_else(|| {
        make_err!(
            Code::InvalidArgument,
            "A Redis step on {:?}",
            world.backend()
        )
    })
}

/// The result of one scenario on one backend.
#[derive(Clone, Debug)]
pub struct Report {
    pub backend: Backend,
    pub records: Vec<Record>,
    pub facts: Facts,
    pub violations: Vec<Violation>,
    /// Matching passes the scheduler ran.
    pub passes: u64,
}

impl Report {
    /// How each client operation ended, by op name; `None` for one that
    /// left or never ended.
    pub fn outcomes(&self) -> BTreeMap<String, Option<Outcome>> {
        let mut out = BTreeMap::new();
        for r in &self.records {
            match &r.event {
                Event::Submit { op, .. } => {
                    out.entry(op.clone()).or_insert(None);
                }
                Event::ClientDone { op, outcome } => {
                    out.insert(op.clone(), Some(*outcome));
                }
                _ => {}
            }
        }
        out
    }

    /// Actions in the order of their first dispatch.
    pub fn dispatch_order(&self) -> Vec<String> {
        let mut seen = BTreeSet::new();
        self.records
            .iter()
            .filter_map(|r| match &r.event {
                Event::Dispatch { action, .. } if seen.insert(action.clone()) => {
                    Some(action.clone())
                }
                _ => None,
            })
            .collect()
    }

    pub fn trace(&self) -> String {
        render(&self.records)
    }

    /// The trace with the records of each millisecond sorted, so two runs
    /// that interleave tasks differently inside one tick compare equal.
    pub fn canonical_trace(&self) -> String {
        let mut records = self.records.clone();
        records.sort_by_cached_key(|r| (r.at_ms, r.event.to_string()));
        render(&records)
    }

    /// The last `lines` records as text.
    pub fn tail(&self, lines: usize) -> String {
        let start = self.records.len().saturating_sub(lines);
        render(&self.records[start..])
    }

    /// Records whose event matches.
    pub fn count(&self, matches: impl Fn(&Event) -> bool) -> usize {
        self.records.iter().filter(|r| matches(&r.event)).count()
    }
}
