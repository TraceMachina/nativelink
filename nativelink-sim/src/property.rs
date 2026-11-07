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

//! Property mode: scenarios generated from a seed, and a shrinker that
//! cuts a failing one down to the steps that still fail and prints it as
//! builder code to paste into the catalogue.

use core::fmt::Write as _;
use core::time::Duration;

use nativelink_error::Error;

use crate::actions::{Action, Run};
use crate::checker::{Invariant, verdict};
use crate::net::Link;
use crate::scenario::{Scenario, Step};
use crate::world::{Backend, WorkerSpec};

/// `SplitMix64`, so a seed gives the same scenario on every machine.
#[derive(Debug, Clone, Copy)]
pub struct Rng(u64);

impl Rng {
    pub const fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Uniform in `low..=high`.
    pub const fn range(&mut self, low: u64, high: u64) -> u64 {
        low + self.next_u64() % (high - low + 1)
    }

    /// Uniform in `0..len`.
    pub fn index(&mut self, len: usize) -> usize {
        usize::try_from(self.next_u64() % len as u64).unwrap_or(0)
    }

    pub const fn chance(&mut self, percent: u64) -> bool {
        self.range(1, 100) <= percent
    }
}

/// What a generated scenario may do to its workers and clients.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools, reason = "one switch per fault kind")]
pub struct Faults {
    pub kill: bool,
    pub cut: bool,
    pub silence: bool,
    pub client_gone: bool,
}

impl Faults {
    pub const ALL: Self = Self {
        kill: true,
        cut: true,
        silence: true,
        client_gone: true,
    };
}

/// One to eight workers, one to forty actions over twenty seconds, and up
/// to four faults from `faults`. A killed worker is replaced two seconds
/// later; a fleet that only shrinks proves nothing.
pub fn generate(seed: u64, backends: &[Backend], faults: Faults) -> Scenario {
    let mut rng = Rng::new(seed);
    let workers = 1 + rng.index(8);
    let spec = WorkerSpec::default().max_inflight(rng.range(1, 3));
    let mut scenario = Scenario::new(&format!("property-{seed}"), seed)
        .backends(backends)
        .settle(Duration::from_mins(10))
        .workers(workers, spec.clone());
    let actions = u32::try_from(rng.range(1, 40)).unwrap_or(1);
    for i in 0..actions {
        let mut action = Action::new(i)
            .run_ms(rng.range(100, 5_000))
            .priority(i32::try_from(rng.range(0, 3)).unwrap_or(0));
        if rng.chance(20) {
            action = action.uncacheable();
        }
        if rng.chance(15) {
            action = action.attempts(&[Run::Error(nativelink_error::Code::Internal), Run::Succeed]);
        }
        scenario = scenario.at(
            rng.range(0, 20_000),
            Step::execute(&format!("c{i}"), action),
        );
    }
    let mut menu = Vec::new();
    if faults.kill {
        menu.push(0);
    }
    if faults.cut {
        menu.push(1);
    }
    if faults.silence {
        menu.push(2);
    }
    if faults.client_gone {
        menu.push(3);
    }
    if menu.is_empty() {
        return scenario;
    }
    for _ in 0..rng.range(0, 4) {
        let at = rng.range(0, 25_000);
        let worker = rng.index(workers);
        scenario = match menu[rng.index(menu.len())] {
            // A killed worker's pod comes back, as it would in a cluster.
            0 => scenario
                .at(at, Step::KillWorker { worker })
                .at(at + 2_000, Step::add_workers(1, spec.clone())),
            1 => scenario.at(at, Step::CutWorker { worker }),
            2 => scenario.silence(worker, at, if rng.chance(50) { 1_000 } else { 15_000 }),
            _ => scenario.at(
                at,
                Step::client_gone(&format!("c{}", rng.range(0, u64::from(actions) - 1))),
            ),
        };
    }
    scenario
}

/// The invariants `scenario` breaks on `backend`.
pub fn failures(scenario: &Scenario, backend: Backend) -> Result<Vec<Invariant>, Error> {
    let report = scenario.run_on(backend)?;
    Ok(verdict(&report.violations).into_iter().collect())
}

/// Removes steps one at a time while the scenario still breaks `target`
/// on `backend`, until no single removal keeps it broken. Worker additions
/// at time zero stay, so the fleet does not vanish.
pub fn shrink(scenario: &Scenario, backend: Backend, target: Invariant) -> Result<Scenario, Error> {
    let mut current = scenario.clone();
    loop {
        let steps = current.steps().to_vec();
        let mut smaller = None;
        for i in 0..steps.len() {
            if matches!(steps[i], (0, Step::AddWorkers { .. })) {
                continue;
            }
            let mut candidate_steps = steps.clone();
            candidate_steps.remove(i);
            // A silence and its end go together.
            if let Step::SetLink {
                worker,
                link: Link::Silent,
            } = steps[i].1
                && let Some(j) = candidate_steps.iter().position(|(at, s)| {
                    *at > steps[i].0
                        && matches!(s, Step::SetLink { worker: w, link: Link::Up } if *w == worker)
                })
            {
                candidate_steps.remove(j);
            }
            let candidate = current.clone().with_steps(candidate_steps);
            if failures(&candidate, backend)?.contains(&target) {
                smaller = Some(candidate);
                break;
            }
        }
        match smaller {
            Some(next) => current = next,
            None => return Ok(current),
        }
    }
}

/// `scenario` as the builder code that makes it.
pub fn to_code(scenario: &Scenario) -> String {
    let mut code = format!("Scenario::new({:?}, 0)\n", scenario.name());
    for (at, step) in scenario.steps() {
        writeln!(code, "    .at({at}, {})", step.to_code()).unwrap();
    }
    code.push_str("    .run()");
    code
}
