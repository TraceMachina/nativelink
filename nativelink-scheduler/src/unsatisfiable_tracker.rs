// Copyright 2026 The NativeLink Authors. All rights reserved.
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

//! Tracks how long each action property shape has gone without any worker
//! able to run it.
//!
//! Time is kept per shape rather than per operation, so a build whose actions
//! all queue together waits once rather than once per action. The scheduler
//! additionally requires each action to have been queued for the timeout
//! itself before failing it, so an action that arrives while its shape is
//! already due is not failed on sight.

use core::time::Duration;
use std::collections::HashMap;
use std::time::SystemTime;

use crate::match_outcome::PropertyShape;

/// How often the same shape is logged while it stays unsatisfiable.
const WARN_INTERVAL: Duration = Duration::from_mins(1);

/// Shortest wait before re-running a matching pass on behalf of a shape.
const MIN_RECHECK: Duration = Duration::from_secs(1);

/// Most doublings of `MIN_RECHECK` while a due shape keeps failing to be
/// retired, so a stuck action wakes the loop at most every 32 seconds.
const MAX_DUE_BACKOFF_DOUBLINGS: u32 = 5;

#[derive(Debug)]
struct ShapeState {
    first_unsatisfiable: SystemTime,
    last_seen: SystemTime,
    /// Fleet generation when the shape was last seen unsatisfiable.
    fleet_generation: u64,
    seen_this_pass: bool,
    seen_last_pass: bool,
    last_warned: Option<SystemTime>,
    /// Passes in which the shape was due but still seen queued.
    due_passes: u32,
}

/// What the matching pass should do with one unsatisfiable action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation {
    /// How long the shape has been unsatisfiable.
    pub waited: Duration,
    /// The shape has been unsatisfiable for the configured timeout, so the
    /// action should be failed.
    pub is_due: bool,
    /// The shape has not been logged recently.
    pub should_warn: bool,
}

#[derive(Debug)]
pub struct UnsatisfiableTracker {
    /// `None` never fails an action.
    timeout: Option<Duration>,
    shapes: HashMap<PropertyShape, ShapeState>,
    seen_this_pass: u64,
}

impl UnsatisfiableTracker {
    #[must_use]
    pub fn new(timeout: Option<Duration>) -> Self {
        Self {
            timeout,
            shapes: HashMap::new(),
            seen_this_pass: 0,
        }
    }

    /// Records that an action with this shape was found unsatisfiable.
    pub fn observe(
        &mut self,
        shape: PropertyShape,
        now: SystemTime,
        fleet_generation: u64,
    ) -> Observation {
        self.seen_this_pass += 1;
        let timeout = self.timeout;
        let state = self.shapes.entry(shape).or_insert(ShapeState {
            first_unsatisfiable: now,
            last_seen: now,
            fleet_generation,
            seen_this_pass: false,
            seen_last_pass: false,
            last_warned: None,
            due_passes: 0,
        });
        let first_this_pass = !state.seen_this_pass;
        // A shape kept from an earlier build only carries over the time it
        // was actually queued, not the idle gap since.
        if !state.seen_last_pass && first_this_pass {
            let gap = now.duration_since(state.last_seen).unwrap_or_default();
            state.first_unsatisfiable += gap;
        }
        state.last_seen = now;
        state.fleet_generation = fleet_generation;
        state.seen_this_pass = true;

        let should_warn = state.last_warned.is_none_or(|last_warned| {
            now.duration_since(last_warned).unwrap_or_default() >= WARN_INTERVAL
        });
        if should_warn {
            state.last_warned = Some(now);
        }

        let waited = now
            .duration_since(state.first_unsatisfiable)
            .unwrap_or_default();
        let is_due = timeout.is_some_and(|timeout| waited >= timeout);
        // Counted once per pass, however many actions share the shape.
        if is_due && first_this_pass {
            state.due_passes = state.due_passes.saturating_add(1);
        }
        Observation {
            waited,
            is_due,
            should_warn,
        }
    }

    /// Whether a shape that stays unsatisfiable is ever failed.
    #[must_use]
    pub const fn fails_actions(&self) -> bool {
        self.timeout.is_some()
    }

    /// The configured timeout; `None` never fails an action.
    #[must_use]
    pub const fn timeout(&self) -> Option<Duration> {
        self.timeout
    }

    /// Closes a matching pass and returns how many unsatisfiable actions it
    /// saw.
    ///
    /// A shape that was not seen is kept for one timeout period so its clock
    /// keeps running and it stays due for actions that arrive one after
    /// another; each such action still waits its own minimum before it is
    /// failed. The shape is dropped at once if the fleet changed, because a
    /// new worker may be able to run it.
    pub fn end_pass(&mut self, now: SystemTime, fleet_generation: u64) -> u64 {
        let linger = self.timeout.unwrap_or_default();
        self.shapes.retain(|_, state| {
            state.seen_last_pass = state.seen_this_pass;
            state.seen_this_pass = false;
            state.seen_last_pass
                || (state.fleet_generation == fleet_generation
                    && now.duration_since(state.last_seen).unwrap_or_default() < linger)
        });
        core::mem::take(&mut self.seen_this_pass)
    }

    /// How long until a shape seen in the last pass comes due, if any.
    #[must_use]
    pub fn next_deadline(&self, now: SystemTime) -> Option<Duration> {
        let timeout = self.timeout?;
        self.shapes
            .values()
            .filter(|state| state.seen_last_pass)
            .map(|state| {
                let waited = now
                    .duration_since(state.first_unsatisfiable)
                    .unwrap_or_default();
                // A due shape that is still queued could not be retired last
                // pass, so wake less and less often for it.
                let recheck =
                    MIN_RECHECK * 2u32.pow(state.due_passes.min(MAX_DUE_BACKOFF_DOUBLINGS));
                timeout.saturating_sub(waited).max(recheck)
            })
            .min()
    }
}
