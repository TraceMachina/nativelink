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

//! An action lives as long as a client is waiting on it (RFC-0064,
//! `action-retention-tied-to-real-listeners/06`). A client that steps away
//! and comes back within the client timeout, by `WaitExecution` or by a new
//! `Execute` that joins the running action, gets its result; so does one
//! that waits passively by name from the start.
//!
//! An attached client refreshes its keepalive every ten seconds, so when it
//! drops, its last refresh can be ten seconds old already: within the
//! default 60 s timeout, a rejoin at 49 s is the latest that is safe.

use core::time::Duration;

use nativelink_error::{Code, Error};
use nativelink_sim::{Action, Event, Outcome, Rejoin, Report, Scenario, Step, WorkerSpec};

/// A two-minute action, so every rejoin lands while it runs.
const RUN_MS: u64 = 120_000;

fn executions(report: &Report, action: &str) -> usize {
    report.count(|e| matches!(e, Event::ExecStart { action: a, .. } if a == action))
}

fn resubmits(report: &Report, op: &str) -> usize {
    report.count(|e| matches!(e, Event::ClientResubmit { op: o } if o == op))
}

/// The client gets the result: no timeout, no second run, no resubmit.
fn assert_finished_once(reports: &[Report]) {
    for r in reports {
        assert_eq!(
            r.outcomes()["c1"],
            Some(Outcome::Exited(0)),
            "{:?}",
            r.backend
        );
        assert_ne!(
            r.outcomes()["c1"],
            Some(Outcome::Failed(Code::DeadlineExceeded as i32)),
            "{:?}",
            r.backend
        );
        assert_eq!(executions(r, "a1"), 1, "{:?}", r.backend);
        assert_eq!(resubmits(r, "c1"), 0, "{:?}", r.backend);
    }
}

fn step_away(seed: u64, away_ms: u64, via: Rejoin) -> Result<Vec<Report>, Error> {
    Scenario::new(&format!("away-{away_ms}-{via:?}"), seed)
        .workers(1, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(RUN_MS)))
        .at(10_000, Step::client_away("c1", away_ms, via))
        .settle(Duration::from_mins(5))
        .run()
}

#[test]
fn a_client_back_by_wait_execution_within_the_timeout_gets_its_result() -> Result<(), Error> {
    for (seed, away_ms) in [(41, 1_000), (42, 49_000)] {
        assert_finished_once(&step_away(seed, away_ms, Rejoin::WaitExecution)?);
    }
    Ok(())
}

#[test]
fn a_client_back_by_execute_within_the_timeout_joins_the_running_action() -> Result<(), Error> {
    for (seed, away_ms) in [(43, 1_000), (44, 49_000)] {
        assert_finished_once(&step_away(seed, away_ms, Rejoin::Execute)?);
    }
    Ok(())
}

/// The client gives up its `Execute` stream at once and waits by name for
/// the whole two minutes, as a client that only polls `WaitExecution` does.
#[test]
fn a_passive_wait_execution_from_the_start_keeps_the_action() -> Result<(), Error> {
    let reports = Scenario::new("passive-wait-execution", 45)
        .workers(1, WorkerSpec::default())
        .at(0, Step::execute("c1", Action::new(1).run_ms(RUN_MS)))
        .at(100, Step::client_away("c1", 0, Rejoin::WaitExecution))
        .settle(Duration::from_mins(5))
        .run()?;
    assert_finished_once(&reports);
    Ok(())
}
