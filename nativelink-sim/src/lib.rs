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

//! A deterministic in-process simulation of the scheduler with its real
//! servers and real workers (RFC-0060). Clients go through the production
//! `ExecutionServer`, workers are the production `LocalWorker` talking to
//! the production `WorkerApiServer`, and only execution is simulated. A
//! scenario runs on the memory and the Redis backend in virtual time, and
//! the checker holds the trace to the invariants.

// The harness reports to test output, where `Debug` is the format a reader
// expects for traces, verdicts and expectations.
#![allow(clippy::use_debug, reason = "the harness's output is test diagnostics")]

pub mod actions;
pub mod cas_load;
pub mod checker;
pub mod clock;
pub mod fleet;
pub mod net;
pub mod property;
pub mod scenario;
pub mod trace;
pub mod world;

pub use actions::{Action, Run};
pub use checker::{Facts, Invariant, Violation, check, verdict};
pub use clock::SimClock;
pub use nativelink_redis_tester::Fault;
pub use net::Link;
pub use scenario::{Expect, Report, Scenario, Step};
pub use trace::{Event, Outcome, Record, Stage};
pub use world::{Backend, WorkerSpec};
