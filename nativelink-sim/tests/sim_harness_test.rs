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

//! The harness's own tests: virtual time, determinism, and a checker that
//! catches what it claims (each invariant fires on a trace altered to break
//! it).

use core::time::Duration;

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_sim::{
    Action, Backend, Event, Expect, Invariant, Record, Report, Scenario, SimClock, Stage, Step,
    WorkerSpec, check, verdict,
};

#[nativelink_test(start_paused = true)]
async fn advance_moves_the_clock_by_exactly_that() -> Result<(), Error> {
    let clock = SimClock::start();
    let tokio_start = tokio::time::Instant::now();
    assert_eq!(SimClock::now_ms(), 0);
    clock.advance(Duration::from_millis(1_234)).await;
    assert_eq!(SimClock::now_ms(), 1_234);
    assert_eq!(tokio_start.elapsed(), Duration::from_millis(1_234));
    Ok(())
}

#[nativelink_test(start_paused = true)]
async fn jump_moves_now_but_not_timers() -> Result<(), Error> {
    let clock = SimClock::start();
    clock.advance(Duration::from_millis(100)).await;
    let tokio_before = tokio::time::Instant::now();
    let before = SimClock::now();
    SimClock::jump(Duration::from_hours(1));
    assert_eq!(SimClock::now_ms(), 3_600_100);
    assert_eq!(
        SimClock::now().duration_since(before).unwrap(),
        Duration::from_hours(1)
    );
    assert_eq!(tokio::time::Instant::now(), tokio_before);
    Ok(())
}

/// Work costs no virtual time: a task that yields a million times finishes
/// at the instant it started.
#[nativelink_test(start_paused = true)]
async fn busy_tasks_do_not_move_the_clock() -> Result<(), Error> {
    SimClock::start();
    for _ in 0..1_000_000 {
        tokio::task::yield_now().await;
    }
    assert_eq!(SimClock::now_ms(), 0);
    Ok(())
}

/// A scenario with retries, priorities and a worker loss, used by the tests
/// below.
fn busy() -> Scenario {
    Scenario::new("busy", 7)
        .workers(3, WorkerSpec::default().max_inflight(2))
        .at(100, Step::execute("c1", Action::new(1).run_ms(2_000)))
        .at(
            110,
            Step::execute("c2", Action::new(2).run_ms(1_000).priority(3)),
        )
        .at(150, Step::execute("c3", Action::new(3).run_ms(3_000)))
        .at(160, Step::execute("c4", Action::new(4).run_ms(500)))
        .at(200, Step::execute("c5", Action::new(5).run_ms(800)))
        .at(210, Step::execute("c6", Action::new(6).run_ms(800)))
        .at(250, Step::execute("c7", Action::new(7).run_ms(800)))
        .at(500, Step::KillWorker { worker: 0 })
        .at(900, Step::CutWorker { worker: 1 })
}

#[test]
fn memory_trace_is_identical_in_ten_runs() -> Result<(), Error> {
    let scenario = busy();
    let first = scenario.run_on(Backend::Memory)?.canonical_trace();
    for run in 1..10 {
        let again = scenario.run_on(Backend::Memory)?.canonical_trace();
        assert_eq!(first, again, "run {run} differs");
    }
    Ok(())
}

/// The fake Redis is reached over a real socket, so its replies can land a
/// tick apart from one run to the next; what must repeat is the verdict and
/// how every operation ends.
#[test]
fn fake_redis_verdict_and_outcomes_repeat_in_ten_runs() -> Result<(), Error> {
    let scenario = busy();
    let first = scenario.run_on(Backend::FakeRedis)?;
    for run in 1..10 {
        let again = scenario.run_on(Backend::FakeRedis)?;
        assert_eq!(
            verdict(&first.violations),
            verdict(&again.violations),
            "run {run}"
        );
        assert_eq!(first.outcomes(), again.outcomes(), "run {run}");
    }
    Ok(())
}

#[test]
fn a_wrong_expectation_fails_the_run() {
    let result = Scenario::new("wrong-expectation", 1)
        .backends(&[Backend::Memory])
        .workers(1, WorkerSpec::default())
        .at(100, Step::execute("c1", Action::new(1)))
        .expect(
            Backend::Memory,
            Expect::known_failure(&[Invariant::I1], "nothing"),
        )
        .run();
    let err = result.expect_err("a pass expected to fail must fail the run");
    assert!(
        err.message_string().contains("expected I1 but got pass"),
        "{err:?}"
    );
}

fn passing_report() -> Result<Report, Error> {
    let report = busy().run_on(Backend::Memory)?;
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    Ok(report)
}

fn fires(records: &[Record], facts: &nativelink_sim::Facts, invariant: Invariant) -> bool {
    verdict(&check(records, facts)).contains(&invariant)
}

fn position(records: &[Record], matches: impl Fn(&Event) -> bool) -> usize {
    records
        .iter()
        .position(|r| matches(&r.event))
        .expect("the trace has the event")
}

#[test]
fn each_invariant_fires_on_its_mutation() -> Result<(), Error> {
    let report = passing_report()?;
    let base = report.records;
    let facts = report.facts;

    // I1: a client that got a result never hears it.
    let mut records = base.clone();
    let i = position(
        &records,
        |e| matches!(e, Event::ClientDone { op, .. } if op == "c2"),
    );
    records.remove(i);
    assert!(fires(&records, &facts, Invariant::I1), "I1");

    // I2: a second dispatch while the first worker still holds the action.
    let mut records = base.clone();
    let i = position(
        &records,
        |e| matches!(e, Event::Dispatch { action, .. } if action == "a2"),
    );
    let mut second = records[i].clone();
    if let Event::Dispatch { worker, .. } = &mut second.event {
        *worker = if worker == "w2" {
            "w1".into()
        } else {
            "w2".into()
        };
    }
    records.insert(i + 1, second);
    assert!(fires(&records, &facts, Invariant::I2), "I2");

    // I3: an accepted dispatch that never ends.
    let mut records = base.clone();
    let i = records
        .iter()
        .rposition(|r| matches!(&r.event, Event::Result { action, .. } if action == "a2"))
        .expect("a2 has a result");
    records.remove(i);
    assert!(fires(&records, &facts, Invariant::I3), "I3");

    // I4: the scheduler still has something executing at quiescence.
    let mut records = base.clone();
    for r in &mut records {
        if let Event::Ledger {
            scheduler_executing,
            ..
        } = &mut r.event
        {
            *scheduler_executing += 1;
        }
    }
    assert!(fires(&records, &facts, Invariant::I4), "I4");

    // I5: an action dispatched while one submitted earlier still waited,
    // with the waiting one given the higher priority. Needs a queue: one
    // worker, four actions.
    let queue = Scenario::new("queue", 3)
        .workers(1, WorkerSpec::default())
        .at(0, Step::execute("c0", Action::new(10).run_ms(1_000)))
        .at(100, Step::execute("c1", Action::new(11).run_ms(1_000)))
        .at(200, Step::execute("c2", Action::new(12).run_ms(1_000)))
        .at(300, Step::execute("c3", Action::new(13).run_ms(1_000)))
        .run_on(Backend::Memory)?;
    assert!(queue.violations.is_empty(), "{:?}", queue.violations);
    let mut records = queue.records.clone();
    let mut first_dispatch = std::collections::HashMap::new();
    let mut submitted = std::collections::HashMap::new();
    for r in &records {
        match &r.event {
            Event::Dispatch { action, .. } => {
                first_dispatch.entry(action.clone()).or_insert(r.at_ms);
            }
            Event::Submit { action, .. } => {
                submitted.entry(action.clone()).or_insert(r.at_ms);
            }
            _ => {}
        }
    }
    let (passed, waiting) = first_dispatch
        .iter()
        .find_map(|(x, tx)| {
            first_dispatch
                .iter()
                .find(|(y, ty)| *ty > tx && submitted[*y] < *tx)
                .map(|(y, _)| (x.clone(), y.clone()))
        })
        .expect("the queue has an action that waits while another is dispatched");
    for r in &mut records {
        if let Event::Submit {
            action, priority, ..
        } = &mut r.event
        {
            *priority = if *action == waiting {
                10
            } else if *action == passed {
                -10
            } else {
                0
            };
        }
    }
    let mut ordered = queue.facts.clone();
    ordered.ordered = true;
    assert!(
        !fires(&queue.records, &ordered, Invariant::I5),
        "I5 on the real order"
    );
    assert!(fires(&records, &ordered, Invariant::I5), "I5");

    // I6: an action dispatched long after its only client left.
    let mut records = base.clone();
    records.insert(
        1,
        Record {
            at_ms: 0,
            event: Event::ClientGone { op: "c7".into() },
        },
    );
    let mut tight = facts.clone();
    tight.abandon_bound_ms = 0;
    assert!(fires(&records, &tight, Invariant::I6), "I6");

    // I7: a budget the worker loss already spends.
    let mut tight = facts.clone();
    tight.dispatch_budget = 1;
    assert!(fires(&base, &tight, Invariant::I7), "I7");

    // I8: the settle ran out.
    let mut records = base.clone();
    for r in &mut records {
        if let Event::Settled { quiet } = &mut r.event {
            *quiet = false;
        }
    }
    assert!(fires(&records, &facts, Invariant::I8), "I8");

    // I9: a worker over its limit.
    let mut tight = facts.clone();
    tight.max_inflight.insert("w1".into(), 0);
    tight.max_inflight.insert("w2".into(), 0);
    assert!(fires(&base, &tight, Invariant::I9), "I9");

    // I10: a stage after the operation was done.
    let mut records = base.clone();
    let i = position(
        &records,
        |e| matches!(e, Event::ClientDone { op, .. } if op == "c1"),
    );
    records.insert(
        i + 1,
        Record {
            at_ms: records[i].at_ms,
            event: Event::ClientStage {
                op: "c1".into(),
                stage: Stage::Queued,
            },
        },
    );
    assert!(fires(&records, &facts, Invariant::I10), "I10");

    // I11: more commands between two passes than the bound.
    let mut tight = facts;
    tight.command_bound = Some(1);
    tight.max_commands_per_pass = 2;
    assert!(fires(&base, &tight, Invariant::I11), "I11");
    Ok(())
}
