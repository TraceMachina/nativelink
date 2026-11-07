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

//! One virtual clock for every component: tokio's, paused. The scheduler,
//! its db and the servers read it through `SimInstant` and `SimClock::now`;
//! the workers and the simulated actions sleep on it directly. A jump moves
//! what `now` reads, the way an NTP step does, and no timer sees it.
//!
//! On the memory backend the clock moves only when every task is waiting on
//! a timer, so work costs no virtual time and a timeout fires exactly when
//! its duration has passed. The fake Redis is reached over a socket, and a
//! paused runtime that is idle while a reply is on its way jumps straight
//! to the next timer, a connect timeout included; there the runner keeps
//! the runtime busy and steps time itself, holding each step until the
//! store stops answering.

use core::cell::Cell;
use core::future::Future;
use core::time::Duration;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use nativelink_util::instant_wrapper::InstantWrapper;
use tokio::time::Instant;

/// Where virtual time starts. Not zero: a zero timestamp reads as unset in
/// places.
pub const EPOCH_OFFSET: Duration = Duration::from_secs(1_000_000);

thread_local! {
    static START: Cell<Option<Instant>> = const { Cell::new(None) };
    static JUMPED: Cell<Duration> = const { Cell::new(Duration::ZERO) };
}

/// How far one manual step moves the clock.
pub const STEP: Duration = Duration::from_millis(10);

/// The most rounds of yields a step waits for a busy store to go quiet.
const MAX_QUIET_ROUNDS: usize = 1_000;

/// A counter of work outside the runtime's view, such as the commands a
/// store has answered; a manual step waits while it moves.
pub type ActivityProbe = Arc<dyn Fn() -> u64 + Send + Sync>;

#[derive(Clone, Default)]
pub struct SimClock {
    /// Set when time is stepped by hand.
    activity: Option<ActivityProbe>,
}

impl core::fmt::Debug for SimClock {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SimClock")
            .field("stepped", &self.activity.is_some())
            .finish()
    }
}

impl SimClock {
    /// Starts virtual time on this thread. Call once per run, inside the
    /// paused runtime, before anything reads the time.
    pub fn start() -> Self {
        START.set(Some(Instant::now()));
        JUMPED.set(Duration::ZERO);
        Self { activity: None }
    }

    /// Steps time by hand, waiting at each step until `activity` stops
    /// moving. The caller keeps the runtime from going idle.
    #[must_use]
    pub fn stepped(mut self, activity: ActivityProbe) -> Self {
        self.activity = Some(activity);
        self
    }

    fn elapsed() -> Duration {
        START.get().map_or(Duration::ZERO, |start| start.elapsed()) + JUMPED.get()
    }

    /// Milliseconds since the run started, jumps included.
    pub fn now_ms() -> u64 {
        u64::try_from(Self::elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// What the scheduler and the servers read as now.
    pub fn now() -> SystemTime {
        UNIX_EPOCH + EPOCH_OFFSET + Self::elapsed()
    }

    /// Lets every runnable task run without moving time.
    pub async fn yield_all() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    /// Lets `by` of virtual time pass: timer by timer, each as soon as
    /// nothing is left to run, or in manual steps.
    pub async fn advance(&self, by: Duration) {
        let Some(activity) = &self.activity else {
            tokio::time::sleep(by).await;
            return;
        };
        let mut left = by;
        while !left.is_zero() {
            let step = left.min(STEP);
            tokio::time::advance(step).await;
            Self::yield_all().await;
            for _ in 0..MAX_QUIET_ROUNDS {
                let before = activity();
                Self::yield_all().await;
                if activity() == before {
                    break;
                }
            }
            left -= step;
        }
    }

    /// Moves what `now` reads forward by `by`; timers do not see it. Forward
    /// only, as a clock that steps back breaks `Instant` arithmetic.
    pub fn jump(by: Duration) {
        JUMPED.set(JUMPED.get() + by);
    }
}

/// `InstantWrapper` over the virtual clock, for the scheduler and its db:
/// production's `SystemTime` wrapper with `SimClock::now` in place of the
/// wall clock, method for method. `from_secs(0)` is the epoch, so a fresh
/// subscriber's keepalive is long overdue, as in production, and `elapsed`
/// sees a jump, as `SystemTime` sees an NTP step.
#[derive(Debug, Clone, Copy)]
pub struct SimInstant(SystemTime);

impl SimInstant {
    pub fn now() -> Self {
        Self(SimClock::now())
    }
}

impl InstantWrapper for SimInstant {
    fn from_secs(secs: u64) -> Self {
        Self(UNIX_EPOCH + Duration::from_secs(secs))
    }

    fn unix_timestamp(&self) -> u64 {
        self.0.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
    }

    fn now(&self) -> SystemTime {
        SimClock::now()
    }

    fn elapsed(&self) -> Duration {
        SimClock::now().duration_since(self.0).unwrap_or_default()
    }

    fn sleep(self, duration: Duration) -> impl Future<Output = ()> + Send + Sync + 'static {
        tokio::time::sleep(duration)
    }
}
