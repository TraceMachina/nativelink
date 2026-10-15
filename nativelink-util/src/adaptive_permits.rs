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

use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;
use tracing::{debug, info};

use crate::metrics::{record_object_store_read_limit, record_object_store_read_wait};

/// A throttle within this long of the last cut is the same event seen by
/// another request in flight, not a second signal.
const CUT_WINDOW: Duration = Duration::from_millis(200);

/// A pool of permits in front of a shared resource whose capacity is not
/// known, sized the way TCP sizes its window: a rejection halves the limit,
/// a run of successes raises it by one. Requests wait for a permit inside
/// the process instead of being sent and refused, and many processes
/// behind one limit converge on a share of it.
///
/// Built for reads against a cloud object store that answers 503 or 429
/// when an account is over its limit; the store calls [`record_throttle`]
/// on such a response and [`record_success`] on a completed request.
///
/// [`record_throttle`]: Self::record_throttle
/// [`record_success`]: Self::record_success
#[derive(Debug)]
pub struct AdaptivePermits {
    semaphore: Arc<Semaphore>,
    /// The current limit, between `min` and `max`.
    limit: AtomicUsize,
    min: usize,
    max: usize,
    /// Permits a cut could not take back because they were out; each is
    /// forgotten as it is released instead of returned.
    owed: AtomicUsize,
    /// Successes since the limit last moved.
    streak: AtomicUsize,
    /// Successes in a row needed to add a permit. Zero fixes the limit.
    grow_after: usize,
    last_cut: Mutex<Option<Instant>>,
    /// Who the pool belongs to, the `store.object.provider` label.
    name: &'static str,
}

/// One permit from an [`AdaptivePermits`] pool, held for the length of a
/// request.
#[derive(Debug)]
pub struct AdaptivePermit {
    pool: Arc<AdaptivePermits>,
    inner: Option<OwnedSemaphorePermit>,
}

impl AdaptivePermits {
    /// A pool that starts at `initial` permits and moves between `min` and
    /// `max` as [`record_throttle`] and [`record_success`] are called; with
    /// `grow_after` zero the limit never grows and only falls. `initial` is
    /// clamped into `[min, max]`.
    ///
    /// [`record_throttle`]: Self::record_throttle
    /// [`record_success`]: Self::record_success
    pub fn new(
        name: &'static str,
        initial: usize,
        min: usize,
        max: usize,
        grow_after: usize,
    ) -> Arc<Self> {
        let min = min.max(1);
        let max = max.max(min);
        let initial = initial.clamp(min, max);
        record_object_store_read_limit(name, initial);
        Arc::new(Self {
            semaphore: Arc::new(Semaphore::new(initial)),
            limit: AtomicUsize::new(initial),
            min,
            max,
            owed: AtomicUsize::new(0),
            streak: AtomicUsize::new(0),
            grow_after,
            last_cut: Mutex::new(None),
            name,
        })
    }

    /// Waits for a permit. The permit is returned to the pool when dropped,
    /// unless the limit was cut while it was out.
    pub async fn acquire(self: &Arc<Self>) -> AdaptivePermit {
        if self.semaphore.available_permits() == 0 {
            record_object_store_read_wait(self.name);
        }
        let inner = Arc::clone(&self.semaphore)
            .acquire_owned()
            .await
            .expect("AdaptivePermits semaphore is never closed");
        AdaptivePermit {
            pool: Arc::clone(self),
            inner: Some(inner),
        }
    }

    /// Settles one owed permit if any is owed. True when it did.
    fn take_owed(&self) -> bool {
        let mut owed = self.owed.load(Ordering::Acquire);
        loop {
            if owed == 0 {
                return false;
            }
            match self.owed.compare_exchange_weak(
                owed,
                owed - 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => owed = current,
            }
        }
    }

    /// The limit as it stands.
    pub fn limit(&self) -> usize {
        self.limit.load(Ordering::Acquire)
    }

    /// Permits that can be taken right now.
    pub fn available(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// A request went through. After `grow_after` of these in a row the
    /// limit rises by one, up to `max`.
    pub fn record_success(&self) {
        if self.grow_after == 0 {
            return;
        }
        let streak = self.streak.fetch_add(1, Ordering::AcqRel) + 1;
        if streak < self.grow_after {
            return;
        }
        self.streak.store(0, Ordering::Release);
        let _guard = self.last_cut.lock();
        let limit = self.limit.load(Ordering::Acquire);
        if limit >= self.max {
            return;
        }
        // A permit still owed from a cut is cancelled instead of a new one
        // being added, so the pool never holds more than `limit`.
        if !self.take_owed() {
            self.semaphore.add_permits(1);
        }
        self.limit.store(limit + 1, Ordering::Release);
        record_object_store_read_limit(self.name, limit + 1);
        debug!(pool = self.name, limit = limit + 1, "Read limit raised");
    }

    /// The resource refused a request for being busy. The limit halves, not
    /// below `min`; permits that are out are forgotten as they come back.
    /// A second throttle within `CUT_WINDOW` of the first is the same event
    /// and does not cut again.
    pub fn record_throttle(&self) {
        let mut last_cut = self.last_cut.lock();
        let now = Instant::now();
        if last_cut.is_some_and(|at| now.duration_since(at) < CUT_WINDOW) {
            return;
        }
        *last_cut = Some(now);
        self.streak.store(0, Ordering::Release);
        let limit = self.limit.load(Ordering::Acquire);
        let target = (limit / 2).max(self.min);
        if target >= limit {
            return;
        }
        let cut = limit - target;
        // Take back what is idle now; the rest is owed and leaves with the
        // permits that hold it.
        let idle = self.semaphore.available_permits().min(cut);
        self.semaphore.forget_permits(idle);
        self.owed.fetch_add(cut - idle, Ordering::AcqRel);
        self.limit.store(target, Ordering::Release);
        record_object_store_read_limit(self.name, target);
        info!(
            pool = self.name,
            from = limit,
            to = target,
            "Read limit halved after a throttled response"
        );
    }
}

impl Drop for AdaptivePermit {
    fn drop(&mut self) {
        let Some(permit) = self.inner.take() else {
            return;
        };
        if self.pool.take_owed() {
            permit.forget();
        } else {
            drop(permit);
        }
    }
}
