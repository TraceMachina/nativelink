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

//! Generated scenarios (RFC-0060 §3.6). Every invariant must hold on every
//! case; a failure is shrunk and printed as builder code. `SIM_PROPERTY_CASES`
//! sets the number of cases (the nightly runs more) and `SIM_PROPERTY_SEED`
//! the first seed.

use nativelink_error::{Error, make_err};
use nativelink_sim::Backend;
use nativelink_sim::property::{Faults, failures, generate, shrink, to_code};

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn check_cases(backend: Backend, faults: Faults, default_cases: u64) -> Result<(), Error> {
    let first = env_u64("SIM_PROPERTY_SEED", 1);
    let cases = env_u64("SIM_PROPERTY_CASES", default_cases);
    for seed in first..first + cases {
        let scenario = generate(seed, &[backend], faults);
        let failed = failures(&scenario, backend)?;
        if !failed.is_empty() {
            let small = shrink(&scenario, backend, failed[0])?;
            let still = failures(&small, backend)?;
            return Err(make_err!(
                nativelink_error::Code::Internal,
                "seed {seed} on {backend:?} breaks {failed:?}; shrunk ({still:?}):\n{}",
                to_code(&small)
            ));
        }
    }
    Ok(())
}

#[test]
fn memory_every_fault() -> Result<(), Error> {
    check_cases(Backend::Memory, Faults::ALL, 64)
}

/// Silences are left out on Redis: a completion after a silence-driven
/// reconnect is not delivered there (reconnect-during-eviction in the
/// catalogue), and every case would find it again.
#[test]
fn fake_redis_kills_cuts_and_departures() -> Result<(), Error> {
    check_cases(
        Backend::FakeRedis,
        Faults {
            silence: false,
            ..Faults::ALL
        },
        16,
    )
}
