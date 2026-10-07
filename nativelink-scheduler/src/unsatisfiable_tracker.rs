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
//! all queue together waits once rather than once per action. An action is
//! only failed once its shape is due *and* the action has itself been queued
//! for the timeout, so one that arrives while its shape is already due is not
//! failed on sight. The tracker therefore also knows when the youngest such
//! action becomes eligible, so the matching loop can wake for it, and it only
//! backs off for a shape whose eligible actions could not be retired.

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
    /// Passes in which the shape was due and an eligible action of it was
    /// seen but could not be failed (the per-pass cap, or a version
    /// conflict). Passes where every action was merely too young do not
    /// count: nothing was stuck, it just was not time yet.
    due_passes: u32,
    /// Whether the shape was due in the pass being recorded.
    due_this_pass: bool,
    /// Actions of this shape seen this pass that had waited the timeout.
    eligible_this_pass: u32,
    /// Actions of this shape failed this pass (see `note_failed`).
    failed_this_pass: u32,
    /// When the youngest action of this shape seen this pass will have
    /// waited the timeout, so the loop can wake for it. Kept until the
    /// shape is next seen, because `next_deadline` runs after `end_pass`.
    earliest_eligible: Option<SystemTime>,
}

/// What the matching pass should do with one unsatisfiable action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation {
    /// How long the shape has been unsatisfiable.
    pub waited: Duration,
    /// The shape has been unsatisfiable for the configured timeout.
    pub is_due: bool,
    /// The action has itself been queued for the configured timeout. It is
    /// failed only when this and `is_due` both hold.
    pub action_eligible: bool,
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

    /// Records that an action with this shape, first submitted at
    /// `insert_timestamp`, was found unsatisfiable.
    pub fn observe(
        &mut self,
        shape: PropertyShape,
        now: SystemTime,
        fleet_generation: u64,
        insert_timestamp: SystemTime,
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
            due_this_pass: false,
            eligible_this_pass: 0,
            failed_this_pass: 0,
            earliest_eligible: None,
        });
        let first_this_pass = !state.seen_this_pass;
        if first_this_pass {
            // A shape kept from an earlier build only carries over the time
            // it was actually queued, not the idle gap since.
            if !state.seen_last_pass {
                let gap = now.duration_since(state.last_seen).unwrap_or_default();
                state.first_unsatisfiable += gap;
            }
            state.due_this_pass = false;
            state.eligible_this_pass = 0;
            state.failed_this_pass = 0;
            state.earliest_eligible = None;
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
        state.due_this_pass |= is_due;

        // A clock that went backwards reads as not yet waited, the safe
        // direction; `now` never being reached reads the same way.
        let action_eligible = timeout.is_some_and(|timeout| {
            now.duration_since(insert_timestamp)
                .is_ok_and(|action_waited| action_waited >= timeout)
        });
        if action_eligible {
            state.eligible_this_pass += 1;
        } else if let Some(eligible_at) =
            timeout.and_then(|timeout| insert_timestamp.checked_add(timeout))
        {
            state.earliest_eligible = Some(
                state
                    .earliest_eligible
                    .map_or(eligible_at, |earliest| earliest.min(eligible_at)),
            );
        }
        Observation {
            waited,
            is_due,
            action_eligible,
            should_warn,
        }
    }

    /// Records that an action with this shape was failed this pass, so a
    /// pass that retired every eligible action does not read as stuck.
    pub fn note_failed(&mut self, shape: &PropertyShape) {
        if let Some(state) = self.shapes.get_mut(shape) {
            state.failed_this_pass += 1;
        }
    }

    /// Whether a shape that stays unsatisfiable is ever failed.
    #[must_use]
    pub const fn fails_actions(&self) -> bool {
        self.timeout.is_some()
    }

    /// Closes a matching pass and returns how many unsatisfiable actions it
    /// saw.
    ///
    /// A shape that was not seen is kept for one timeout period so its clock
    /// keeps running and it stays due for actions that arrive one after
    /// another; each such action still waits its own minimum before it is
    /// failed. The shape is dropped at once if the fleet changed, because a
    /// new worker may be able to run it.
    ///
    /// A due shape backs off (see `next_deadline`) only when an eligible
    /// action of it was seen but not failed this pass.
    pub fn end_pass(&mut self, now: SystemTime, fleet_generation: u64) -> u64 {
        let linger = self.timeout.unwrap_or_default();
        self.shapes.retain(|_, state| {
            if state.seen_this_pass
                && state.due_this_pass
                && state.eligible_this_pass > state.failed_this_pass
            {
                state.due_passes = state.due_passes.saturating_add(1);
            }
            state.seen_last_pass = state.seen_this_pass;
            state.seen_this_pass = false;
            state.seen_last_pass
                || (state.fleet_generation == fleet_generation
                    && now.duration_since(state.last_seen).unwrap_or_default() < linger)
        });
        core::mem::take(&mut self.seen_this_pass)
    }

    /// How long until the loop should run again for a shape seen in the
    /// last pass, if any: when the shape comes due, when its youngest action
    /// becomes eligible, or, for a due shape whose eligible actions could
    /// not be retired, a backoff that doubles up to 32 seconds.
    #[must_use]
    pub fn next_deadline(&self, now: SystemTime) -> Option<Duration> {
        let timeout = self.timeout?;
        self.shapes
            .values()
            .filter(|state| state.seen_last_pass)
            .filter_map(|state| {
                let waited = now
                    .duration_since(state.first_unsatisfiable)
                    .unwrap_or_default();
                let shape_term = if waited < timeout {
                    Some(timeout.saturating_sub(waited))
                } else if state.eligible_this_pass > state.failed_this_pass {
                    // Still queued though eligible, so wake less and less
                    // often for it.
                    Some(MIN_RECHECK * 2u32.pow(state.due_passes.min(MAX_DUE_BACKOFF_DOUBLINGS)))
                } else {
                    // Due, but nothing of it is stuck: only a young action can
                    // change anything, and that is covered below.
                    None
                };
                let eligible_term = state
                    .earliest_eligible
                    .map(|eligible_at| eligible_at.duration_since(now).unwrap_or_default());
                match (shape_term, eligible_term) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                }
                .map(|deadline| deadline.max(MIN_RECHECK))
            })
            .min()
    }
}
