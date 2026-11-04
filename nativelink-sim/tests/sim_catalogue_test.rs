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

//! The day-one catalogue (RFC-0060 §7): one test per scenario, each with
//! an expectation per backend. A test fails when the verdict and the
//! expectation differ in either direction, so a fix flips its expectation
//! in the same change. Each expectation started from the review (the LLD's
//! §7 table); where the first run disagreed, the comment on the scenario
//! says which was wrong.

use core::time::Duration;

use nativelink_config::schedulers::PropertyType;
use nativelink_error::Error;
use nativelink_sim::Invariant::{I1, I8, I11};
use nativelink_sim::{
    Action, Backend, Event, Expect, Fault, Link, Outcome, Report, Run, Scenario, Step, WorkerSpec,
};

const STRANDED: &str = "S1 and S4: an action executing at the restart waits out the hour ceiling";

fn report(reports: &[Report], backend: Backend) -> &Report {
    reports
        .iter()
        .find(|r| r.backend == backend)
        .expect("the backend ran")
}

fn executions(report: &Report, action: &str) -> usize {
    report.count(|e| matches!(e, Event::ExecStart { action: a, .. } if a == action))
}

fn property_spec(
    name: &'static str,
    kind: PropertyType,
) -> impl Fn(&mut nativelink_config::schedulers::SimpleSpec) + Send + Sync {
    move |spec| {
        let mut properties = spec
            .supported_platform_properties
            .take()
            .unwrap_or_default();
        properties.insert(name.to_string(), kind);
        spec.supported_platform_properties = Some(properties);
    }
}

// Rows of the LLD's §7 table.

#[test]
fn empty_queue() -> Result<(), Error> {
    Scenario::new("empty-queue", 1)
        .workers(4, WorkerSpec::default())
        .settle(Duration::from_secs(30))
        .run()
        .map(drop)
}

/// The LLD's one million queued actions, at a size a push can afford,
/// behind one worker that a long action holds. A pass with nothing it can
/// dispatch should cost a bounded number of store commands; the Redis
/// backend reads the whole queue every pass. With no worker at all neither
/// backend reads the queue, which is why the worker is here.
#[test]
fn queue_2k_behind_a_busy_worker() -> Result<(), Error> {
    let mut scenario = Scenario::new("queue-2k", 2)
        .workers(1, WorkerSpec::default())
        .at(0, Step::execute("busy", Action::new(9_999).run_ms(30_000)))
        .command_bound(500)
        .settle(Duration::from_mins(2))
        .expect(
            Backend::FakeRedis,
            Expect::known_failure(&[I11], "S3: the queue read is O(queue) per pass"),
        );
    for i in 0..2_000u32 {
        let op = format!("c{i}");
        scenario = scenario
            .at(100 + u64::from(i), Step::execute(&op, Action::new(i)))
            .at(10_000, Step::client_gone(&op));
    }
    scenario.run().map(drop)
}

/// The review expected the Redis backend to hold the action for the
/// one-hour executing ceiling. With one scheduler the worker's eviction
/// requeues it; the ceiling bites when the worker belongs to another
/// scheduler replica, which T1 does not run yet.
fn no_action_timeout() -> Scenario {
    Scenario::new("no-action-timeout", 3)
        .workers(2, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(120_000)))
        .silence(0, 1_000, 600_000)
        .settle(Duration::from_mins(15))
}

#[test]
fn no_action_timeout_and_the_worker_goes_silent() -> Result<(), Error> {
    no_action_timeout()
        .backends(&[Backend::Memory])
        .run()
        .map(drop)
}

/// L3 in RFC-0060 §11: about one run in three on Redis, the completed write
/// succeeds and the client never hears of it. The cause is not found; a
/// dropped `changed()` is ruled out (`subscriber_cancel_test`).
#[test]
#[ignore = "nightly: L3, an intermittent lost completion on Redis"]
fn no_action_timeout_on_redis() -> Result<(), Error> {
    no_action_timeout()
        .backends(&[Backend::FakeRedis])
        .run()
        .map(drop)
}

#[test]
fn unsatisfiable_reservation_waits() -> Result<(), Error> {
    Scenario::new("unsatisfiable", 4)
        .spec(property_spec("pool", PropertyType::Exact))
        .workers(2, WorkerSpec::default().property("pool", "cpu"))
        .at(
            0,
            Step::execute("c1", Action::new(1).property("pool", "gpu")),
        )
        .settle(Duration::from_mins(2))
        .expect(
            Backend::Memory,
            Expect::known_failure(&[I8], "S4: no default unsatisfiable timeout"),
        )
        .expect(
            Backend::FakeRedis,
            Expect::known_failure(&[I8], "S4: no default unsatisfiable timeout"),
        )
        .run()
        .map(drop)
}

#[test]
fn zero_dimension_worker() -> Result<(), Error> {
    Scenario::new("zero-dimension", 5)
        .spec(property_spec("memory_kb", PropertyType::Minimum))
        .workers(1, WorkerSpec::default().property("memory_kb", "0"))
        .at(0, Step::execute("c1", Action::new(1)))
        .run()
        .map(drop)
}

/// Back right as the eviction tick takes it: the worker is evicted, its
/// reconnect waits out the rest of the silence, and the action runs on the
/// new connection.
#[test]
fn reconnect_during_eviction() -> Result<(), Error> {
    Scenario::new("reconnect-during-eviction", 6)
        .workers(1, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(30_000)))
        .silence(0, 1_000, 11_500)
        .settle(Duration::from_mins(3))
        .run()
        .map(drop)
}

/// The review expected the old result to reach the new scheduler and be
/// dropped as unknown (I1). The production worker kills what it runs when
/// its stream closes, so no old result arrives. On the memory backend the
/// state goes with the process and the client executes again. On Redis the
/// record survives as Executing on a worker id the new scheduler has never
/// seen; an unknown worker reads as a peer's, so nothing requeues it and
/// the client waits for the executing ceiling (I8 within the settle).
#[test]
fn result_after_scheduler_restart() -> Result<(), Error> {
    let reports = Scenario::new("result-after-restart", 7)
        .workers(4, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(30_000)))
        .at(5_000, Step::SchedulerRestart)
        .settle(Duration::from_mins(3))
        .expect(Backend::FakeRedis, Expect::known_failure(&[I8], STRANDED))
        .run()?;
    assert_eq!(executions(report(&reports, Backend::Memory), "a1"), 2);
    Ok(())
}

#[test]
fn result_arrives_twice() -> Result<(), Error> {
    Scenario::new("result-twice", 8)
        .workers(1, WorkerSpec::default())
        .at(
            0,
            Step::SetLink {
                worker: 0,
                link: Link::DuplicateResults,
            },
        )
        .at(100, Step::execute("c1", Action::new(1)))
        .at(200, Step::execute("c2", Action::new(2)))
        .run()
        .map(drop)
}

#[test]
fn result_after_client_gone() -> Result<(), Error> {
    let reports = Scenario::new("result-after-client-gone", 9)
        .workers(1, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(5_000)))
        .at(1_000, Step::client_gone("c1"))
        .run()?;
    for r in &reports {
        assert_eq!(executions(r, "a1"), 1, "{:?}", r.backend);
    }
    Ok(())
}

/// The re-subscription works: the client finds its operation on the new
/// scheduler. What it finds is the stranded record of
/// result-after-restart, which the review's pass did not account for.
#[test]
fn wait_execution_after_failover() -> Result<(), Error> {
    let reports = Scenario::new("wait-execution-after-failover", 10)
        .workers(2, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(10_000)))
        .at(2_000, Step::SchedulerDown)
        .at(5_000, Step::SchedulerUp)
        .settle(Duration::from_mins(3))
        .expect(Backend::Memory, Expect::NotApplicable)
        .expect(Backend::FakeRedis, Expect::known_failure(&[I8], STRANDED))
        .run()?;
    let redis = report(&reports, Backend::FakeRedis);
    assert!(
        redis.count(|e| matches!(e, Event::ClientResubmit { .. })) == 0,
        "the operation was found again, not executed anew"
    );
    Ok(())
}

/// The LLD's restart with ten thousand executing, at two hundred.
#[test]
fn restart_with_200_executing() -> Result<(), Error> {
    let mut scenario = Scenario::new("restart-200-executing", 11)
        .workers(200, WorkerSpec::default())
        .at(10_000, Step::SchedulerRestart)
        .settle(Duration::from_mins(10));
    for i in 0..200u32 {
        scenario = scenario.at(
            100,
            Step::execute(&format!("c{i}"), Action::new(i).run_ms(60_000)),
        );
    }
    // As the review expected on Redis: the executing records wait out the
    // hour (result-after-restart). The memory backend loses them with the
    // process, so its clients execute again and finish.
    scenario
        .expect(Backend::FakeRedis, Expect::known_failure(&[I8], STRANDED))
        .run()
        .map(drop)
}

/// The write lands and its reply is lost, as the results of this run land.
/// The review expected the version retry to recover. Found by the harness
/// on 2026-11-04: the write lands, `update_operation` takes the lost reply
/// as a non-retryable failure, and nothing tells the client.
#[test]
fn journal_write_times_out_after_applying() -> Result<(), Error> {
    Scenario::new("journal-write-timeout", 12)
        .workers(1, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(3_000)))
        .at(
            2_990,
            Step::redis_fail("EVALSHA", Fault::AppliedThenError("timeout".into()), 1),
        )
        .expect(
            Backend::FakeRedis,
            Expect::known_failure(
                &[I1, I8],
                "unassigned: a refused result write is not retried",
            ),
        )
        .run()
        .map(drop)
}

/// Found by the harness on 2026-11-04: a result whose store write is
/// refused once is dropped. `update_operation` logs the error as
/// non-retryable, the worker has already reported it, and the client waits
/// until the executing ceiling.
#[test]
fn result_write_refused() -> Result<(), Error> {
    Scenario::new("result-write-refused", 27)
        .workers(1, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(3_000)))
        .at(
            2_990,
            Step::redis_fail("EVALSHA", Fault::Error("refused".into()), 2),
        )
        .settle(Duration::from_mins(2))
        .expect(
            Backend::FakeRedis,
            Expect::known_failure(
                &[I1, I8],
                "unassigned: a refused result write is not retried",
            ),
        )
        .run()
        .map(drop)
}

/// Found by the harness on 2026-11-04: an hour's forward step evicts both
/// live workers, the requeued action's client keepalive now reads an hour
/// old, and the matching pass retires it as abandoned while its client is
/// attached; on the memory backend the build fails with
/// `DEADLINE_EXCEEDED`. The Redis backend runs it again and completes.
#[test]
fn clock_jumps_an_hour() -> Result<(), Error> {
    let reports = Scenario::new("clock-jump", 13)
        .workers(2, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(10_000)))
        .at(2_000, Step::ClockJump { ms: 3_600_000 })
        .at(3_000, Step::execute("c2", Action::new(2).run_ms(2_000)))
        .settle(Duration::from_mins(2))
        .backends_disagree("unassigned: a clock step retires an attached client's requeued action")
        .run()?;
    let memory = report(&reports, Backend::Memory);
    assert_eq!(memory.outcomes()["c1"], Some(Outcome::Failed(4)));
    Ok(())
}

/// The review expected the refused result to spend an attempt (I7). It
/// spends nothing: the result is dropped and the client waits, the
/// result-write-refused defect.
#[test]
fn redis_oom_during_results() -> Result<(), Error> {
    Scenario::new("redis-oom", 14)
        .workers(1, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(3_000)))
        .at(2_990, Step::redis_fail("EVALSHA", Fault::Oom, 3))
        .settle(Duration::from_mins(2))
        .expect(
            Backend::FakeRedis,
            Expect::known_failure(
                &[I1, I8],
                "unassigned: a refused result write is not retried",
            ),
        )
        .run()
        .map(drop)
}

/// Five writes answer `READONLY` after the failover; the last of them meet
/// c2's result, the result-write-refused defect again.
#[test]
fn redis_readonly_failover() -> Result<(), Error> {
    Scenario::new("redis-readonly-failover", 15)
        .workers(2, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(3_000)))
        .at(100, Step::execute("c2", Action::new(2).run_ms(3_000)))
        .at(1_000, Step::RedisFailover { readonly_writes: 5 })
        .settle(Duration::from_mins(2))
        .expect(
            Backend::FakeRedis,
            Expect::known_failure(
                &[I1, I8],
                "unassigned: a refused result write is not retried",
            ),
        )
        .run()
        .map(drop)
}

/// The failover lands as the results do: the same drop as a refused write.
#[test]
fn redis_failover_as_results_land() -> Result<(), Error> {
    Scenario::new("redis-failover-at-results", 28)
        .workers(1, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(3_000)))
        .at(2_990, Step::RedisFailover { readonly_writes: 5 })
        .settle(Duration::from_mins(2))
        .expect(
            Backend::FakeRedis,
            Expect::known_failure(
                &[I1, I8],
                "unassigned: a refused result write is not retried",
            ),
        )
        .run()
        .map(drop)
}

/// The memory backend runs all 2,000. On Redis 1,111 end in
/// `DEADLINE_EXCEEDED` with their clients attached. A lead, not a finding:
/// the Redis runs step time by hand and may overstate its latency, so the
/// bed confirms it before anyone fixes it.
#[test]
fn pubsub_storm() -> Result<(), Error> {
    let mut scenario = Scenario::new("pubsub-storm", 16)
        .workers(50, WorkerSpec::default())
        .settle(Duration::from_mins(5))
        .backends_disagree("lead: confirm the Redis burst failures on the bed (T3)");
    for i in 0..2_000u32 {
        scenario = scenario.at(
            u64::from(i / 2),
            Step::execute(&format!("c{i}"), Action::new(i).run_ms(200)),
        );
    }
    scenario.run().map(drop)
}

#[test]
fn channel_full_at_one_worker() -> Result<(), Error> {
    let mut scenario = Scenario::new("channel-full", 17)
        .workers(1, WorkerSpec::default().max_inflight(300))
        .settle(Duration::from_mins(5));
    for i in 0..300u32 {
        scenario = scenario.at(
            0,
            Step::execute(&format!("c{i}"), Action::new(i).run_ms(1_000)),
        );
    }
    scenario.run().map(drop)
}

#[test]
fn ack_never_arrives() -> Result<(), Error> {
    Scenario::new("ack-never", 18)
        .spec(|spec| spec.dispatch_ack_timeout_s = 5)
        .workers(2, WorkerSpec::default())
        .at(
            0,
            Step::SetLink {
                worker: 0,
                link: Link::DropAccepts,
            },
        )
        .at(100, Step::execute("c1", Action::new(1).run_ms(2_000)))
        .at(100, Step::execute("c2", Action::new(2).run_ms(2_000)))
        .settle(Duration::from_mins(2))
        .run()
        .map(drop)
}

#[test]
fn evict_queued_record() -> Result<(), Error> {
    let mut scenario = Scenario::new("evict-queued-record", 19)
        .at(2_000, Step::redis_evict("aa_"))
        .at(4_000, Step::add_workers(2, WorkerSpec::default()))
        .settle(Duration::from_mins(2));
    for i in 0..5u32 {
        scenario = scenario.at(0, Step::execute(&format!("c{i}"), Action::new(i)));
    }
    // The review expected the clients to wait on records that are gone. They
    // do not wait for long: the next keepalive reads NotFound, the clients
    // execute again as Bazel would, and every action runs once.
    let reports = scenario.run()?;
    let redis = report(&reports, Backend::FakeRedis);
    assert_eq!(
        redis.count(|e| matches!(e, Event::ClientResubmit { .. })),
        5,
        "{}",
        redis.trace()
    );
    Ok(())
}

// The worker rows the real worker makes possible: the stream cut and the
// kill in each phase of an action.

/// Two seconds fetching inputs, three running, two uploading.
fn phased() -> Action {
    Action::new(1)
        .prepare_ms(2_000)
        .run_ms(3_000)
        .upload_ms(2_000)
}

const PHASES: [(&str, u64); 3] = [("fetch", 1_000), ("run", 3_500), ("upload", 6_000)];

#[test]
fn stream_cut_in_each_phase() -> Result<(), Error> {
    for (phase, at) in PHASES {
        Scenario::new(&format!("cut-during-{phase}"), 20)
            .workers(2, WorkerSpec::default())
            .at(0, Step::execute("c1", phased()))
            .at(at, Step::CutWorker { worker: 0 })
            .settle(Duration::from_mins(2))
            .run()?;
    }
    Ok(())
}

#[test]
fn stream_silent_one_second_thirty_seconds_ten_minutes() -> Result<(), Error> {
    for (label, for_ms) in [("1s", 1_000), ("30s", 30_000), ("10m", 600_000)] {
        for (phase, at) in PHASES {
            Scenario::new(&format!("silent-{label}-during-{phase}"), 21)
                .workers(2, WorkerSpec::default())
                .at(0, Step::execute("c1", phased()))
                .silence(0, at, for_ms)
                .settle(Duration::from_mins(15))
                .run()?;
        }
    }
    Ok(())
}

#[test]
fn worker_killed_in_each_phase() -> Result<(), Error> {
    for (phase, at) in PHASES {
        Scenario::new(&format!("kill-during-{phase}"), 22)
            .workers(2, WorkerSpec::default())
            .at(0, Step::execute("c1", phased()))
            .at(at, Step::KillWorker { worker: 0 })
            .settle(Duration::from_mins(2))
            .run()?;
    }
    Ok(())
}

// Rows the harness adds: retries, order, abandonment, coalescing.

#[test]
fn failed_attempts_are_retried() -> Result<(), Error> {
    let reports = Scenario::new("retries", 23)
        .workers(1, WorkerSpec::default())
        .at(
            0,
            Step::execute(
                "c1",
                Action::new(1).attempts(&[
                    Run::Error(nativelink_error::Code::Internal),
                    Run::Error(nativelink_error::Code::Internal),
                    Run::Succeed,
                ]),
            ),
        )
        .settle(Duration::from_mins(2))
        .run()?;
    for r in &reports {
        assert_eq!(
            r.outcomes()["c1"],
            Some(Outcome::Exited(0)),
            "{:?}",
            r.backend
        );
        assert_eq!(executions(r, "a1"), 3, "{:?}", r.backend);
    }
    Ok(())
}

#[test]
fn priority_then_age_at_one_worker() -> Result<(), Error> {
    let mut scenario = Scenario::new("order", 24)
        .ordered()
        .workers(1, WorkerSpec::default())
        .at(0, Step::execute("busy", Action::new(100).run_ms(5_000)))
        .settle(Duration::from_mins(2));
    for (i, priority) in [(1u32, 0), (2, 5), (3, 0), (4, 9), (5, 5), (6, 0)] {
        scenario = scenario.at(
            1_000 + u64::from(i) * 100,
            Step::execute(
                &format!("c{i}"),
                Action::new(i).priority(priority).run_ms(500),
            ),
        );
    }
    let reports = scenario.run()?;
    for r in &reports {
        assert_eq!(
            r.dispatch_order(),
            ["a100", "a4", "a2", "a5", "a1", "a3", "a6"],
            "{:?}",
            r.backend
        );
    }
    Ok(())
}

#[test]
fn abandoned_queued_actions_are_not_run() -> Result<(), Error> {
    let reports = Scenario::new("abandoned-queued", 25)
        .at(0, Step::execute("c1", Action::new(1)))
        .at(0, Step::execute("c2", Action::new(2)))
        .at(1_000, Step::client_gone("c1"))
        .at(1_000, Step::client_gone("c2"))
        .at(120_000, Step::add_workers(1, WorkerSpec::default()))
        .settle(Duration::from_mins(1))
        .run()?;
    for r in &reports {
        assert_eq!(
            r.count(|e| matches!(e, Event::Dispatch { .. })),
            0,
            "{:?}",
            r.backend
        );
    }
    Ok(())
}

#[test]
fn two_clients_one_execution() -> Result<(), Error> {
    let reports = Scenario::new("coalesce", 26)
        .workers(2, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(3_000)))
        .at(500, Step::execute_same("c2", "c1"))
        .run()?;
    for r in &reports {
        assert_eq!(executions(r, "a1"), 1, "{:?}", r.backend);
        assert_eq!(
            r.outcomes()["c2"],
            Some(Outcome::Exited(0)),
            "{:?}",
            r.backend
        );
    }
    Ok(())
}
