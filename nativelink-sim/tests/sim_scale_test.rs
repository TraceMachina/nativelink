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

//! Scale in-process (RFC-0060 §3.6): a burst at zero workers, then the
//! fleet arrives. I11 counts the store commands between passes. The full
//! size runs nightly (`--ignored`); every push runs a tenth of it.

use core::time::Duration;

use nativelink_error::Error;
use nativelink_sim::Invariant::I11;
use nativelink_sim::{Action, Backend, Scenario, Step, WorkerSpec, verdict};

/// `burst` actions at zero workers, `workers` arriving at five seconds, and
/// `rest` more actions over the following minute.
fn burst_then_fleet(name: &str, burst: u32, workers: usize, rest: u32) -> Scenario {
    let mut scenario = Scenario::new(name, 30)
        .command_bound(1_000)
        .settle(Duration::from_mins(30))
        .at(
            5_000,
            Step::add_workers(workers, WorkerSpec::default().max_inflight(2)),
        );
    for i in 0..burst {
        scenario = scenario.at(
            u64::from(i % 1_000),
            Step::execute(&format!("b{i}"), Action::new(i).run_ms(2_000)),
        );
    }
    for i in 0..rest {
        scenario = scenario.at(
            6_000 + u64::from(i) * 60_000 / u64::from(rest.max(1)),
            Step::execute(&format!("r{i}"), Action::new(burst + i).run_ms(2_000)),
        );
    }
    scenario
}

#[test]
fn burst_600_then_80_workers() -> Result<(), Error> {
    burst_then_fleet("burst-600-then-80", 600, 80, 4_400)
        .backends(&[Backend::Memory])
        .run()
        .map(drop)
}

/// The Redis backend reads the queue every pass (I11, S3). A lead, not a
/// finding: in this burst one to five queued actions are never dispatched
/// in thirty minutes while workers sit idle, a different number each run.
/// The fake's `FT.AGGREGATE` is a simplification, so the bed confirms it
/// (T3) before anyone fixes it; until then this asserts only I11.
#[test]
#[ignore = "nightly: a lead to confirm on the bed"]
fn burst_600_then_80_workers_on_redis() -> Result<(), Error> {
    let report =
        burst_then_fleet("burst-600-then-80", 600, 80, 4_400).run_on(Backend::FakeRedis)?;
    let failed = verdict(&report.violations);
    assert!(failed.contains(&I11), "{:?}", report.violations);
    Ok(())
}

#[test]
#[ignore = "nightly: 800 workers and 50,000 actions"]
fn fleet_800() -> Result<(), Error> {
    let reports = burst_then_fleet("fleet-800", 6_000, 800, 44_000)
        .backends(&[Backend::Memory])
        .run()?;
    drop(reports);
    let redis = burst_then_fleet("fleet-800", 6_000, 800, 44_000).run_on(Backend::FakeRedis)?;
    assert!(
        verdict(&redis.violations).contains(&I11),
        "{:?}",
        redis.violations
    );
    Ok(())
}
