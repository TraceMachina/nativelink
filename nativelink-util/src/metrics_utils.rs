// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::mem::forget;
use core::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use futures::Future;
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent, group, publish,
};

#[derive(Debug, Default)]
pub struct FuncCounterWrapper {
    pub successes: AtomicU64,
    pub failures: AtomicU64,
}

impl FuncCounterWrapper {
    #[inline]
    pub fn wrap<T, E>(&self, func: impl FnOnce() -> Result<T, E>) -> Result<T, E> {
        let result = (func)();
        if result.is_ok() {
            self.successes.fetch_add(1, Ordering::Acquire);
        } else {
            self.failures.fetch_add(1, Ordering::Acquire);
        }
        result
    }
}

// Derive-macros have no way to tell the collector that the parent
// is now a group with the name of the group as the field so we
// can attach multiple values on the same group, so we need to
// manually implement the `MetricsComponent` trait to do so.
impl MetricsComponent for FuncCounterWrapper {
    fn publish(
        &self,
        _kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        let _enter = group!(field_metadata.name).entered();

        publish!(
            "successes",
            &self.successes,
            MetricKind::Counter,
            format!(
                "The number of times {} was successful.",
                field_metadata.name
            )
        );
        publish!(
            "failures",
            &self.failures,
            MetricKind::Counter,
            format!("The number of times {} failed.", field_metadata.name)
        );

        Ok(MetricPublishKnownKindData::Component)
    }
}

/// This is a utility that will only increment the referenced counter when it is dropped.
/// This struct is zero cost and has a runtime cost only when it is dropped.
/// This struct is very useful for tracking when futures are dropped.
#[derive(Debug)]
struct DropCounter<'a> {
    counter: &'a AtomicU64,
}

impl<'a> DropCounter<'a> {
    #[inline]
    pub(crate) const fn new(counter: &'a AtomicU64) -> Self {
        Self { counter }
    }
}

impl Drop for DropCounter<'_> {
    #[inline]
    fn drop(&mut self) {
        self.counter.fetch_add(1, Ordering::Acquire);
    }
}

#[derive(Debug)]
pub struct AsyncTimer<'a> {
    start: Instant,
    drop_counter: DropCounter<'a>,
    counter: &'a AsyncCounterWrapper,
}

impl AsyncTimer<'_> {
    #[inline]
    pub fn measure(self) {
        self.counter
            .sum_func_duration_ns
            .fetch_add(self.start.elapsed().as_nanos() as u64, Ordering::Acquire);
        self.counter.calls.fetch_add(1, Ordering::Acquire);
        self.counter.successes.fetch_add(1, Ordering::Acquire);
        // This causes DropCounter's drop to never be called.
        forget(self.drop_counter);
    }
}

/// Tracks the number of calls, successes, failures, and drops of an async function.
/// call `.wrap(future)` to wrap a future and stats about the future are automatically
/// tracked and can be published to a `CollectorState`.
#[derive(Debug, Default)]
pub struct AsyncCounterWrapper {
    pub calls: AtomicU64,
    pub successes: AtomicU64,
    pub failures: AtomicU64,
    pub drops: AtomicU64,
    // Time spent in nanoseconds in the future.
    // 64 bit address space gives ~584 years of nanoseconds.
    pub sum_func_duration_ns: AtomicU64,
}

// Derive-macros have no way to tell the collector that the parent
// is now a group with the name of the group as the field so we
// can attach multiple values on the same group, so we need to
// manually implement the `MetricsComponent` trait to do so.
#[expect(
    clippy::cognitive_complexity,
    reason = "complexity arises from macro expansion"
)]
impl MetricsComponent for AsyncCounterWrapper {
    fn publish(
        &self,
        _kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        let _enter = group!(field_metadata.name).entered();

        publish!(
            "calls",
            &self.calls,
            MetricKind::Counter,
            format!("The number of times {} was called.", field_metadata.name)
        );
        publish!(
            "successes",
            &self.successes,
            MetricKind::Counter,
            format!(
                "The number of times {} was successful.",
                field_metadata.name
            )
        );
        publish!(
            "failures",
            &self.failures,
            MetricKind::Counter,
            format!("The number of times {} failed.", field_metadata.name)
        );
        publish!(
            "drops",
            &self.drops,
            MetricKind::Counter,
            format!("The number of times {} was dropped.", field_metadata.name)
        );
        publish!(
            "sum_func_duration_ns",
            &self.sum_func_duration_ns,
            MetricKind::Counter,
            format!(
                "The sum of the time spent in nanoseconds in {}.",
                field_metadata.name
            )
        );

        Ok(MetricPublishKnownKindData::Component)
    }
}

impl AsyncCounterWrapper {
    #[inline]
    pub fn wrap_fn<'a, T: 'a, E>(
        &'a self,
        func: impl FnOnce() -> Result<T, E> + 'a,
    ) -> Result<T, E> {
        self.calls.fetch_add(1, Ordering::Acquire);
        let result = (func)();
        if result.is_ok() {
            self.successes.fetch_add(1, Ordering::Acquire);
        } else {
            self.failures.fetch_add(1, Ordering::Acquire);
        }
        result
    }

    #[inline]
    pub async fn wrap<'a, T, E, F: Future<Output = Result<T, E>> + 'a>(
        &'a self,
        future: F,
    ) -> Result<T, E> {
        let result = self.wrap_no_capture_result(future).await;
        if result.is_ok() {
            self.successes.fetch_add(1, Ordering::Acquire);
        } else {
            self.failures.fetch_add(1, Ordering::Acquire);
        }
        result
    }

    #[inline]
    pub async fn wrap_no_capture_result<'a, T, F: Future<Output = T> + 'a>(
        &'a self,
        future: F,
    ) -> T {
        self.calls.fetch_add(1, Ordering::Acquire);
        let drop_counter = DropCounter::new(&self.drops);
        let instant = Instant::now();
        let result = future.await;
        // By default `drop_counter` will increment the drop counter when it goes out of scope.
        // This will ensure we don't increment the counter if we make it here with a zero cost.
        forget(drop_counter);
        self.sum_func_duration_ns
            .fetch_add(instant.elapsed().as_nanos() as u64, Ordering::Acquire);
        result
    }

    #[inline]
    pub fn begin_timer(&self) -> AsyncTimer<'_> {
        AsyncTimer {
            start: Instant::now(),
            drop_counter: DropCounter::new(&self.drops),
            counter: self,
        }
    }
}

/// Tracks a number.
#[derive(Debug, Default)]
pub struct Counter(AtomicU64);

impl Counter {
    #[inline]
    pub fn inc(&self) {
        self.add(1);
    }

    #[inline]
    pub fn add(&self, value: u64) {
        self.0.fetch_add(value, Ordering::Acquire);
    }

    #[inline]
    pub fn sub(&self, value: u64) {
        self.0.fetch_sub(value, Ordering::Acquire);
    }
}

impl MetricsComponent for Counter {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        self.0.publish(kind, field_metadata)
    }
}

/// Tracks an counter through time and the last time the counter was changed.
#[derive(Debug, Default)]
pub struct CounterWithTime {
    pub counter: AtomicU64,
    pub last_time: AtomicU64,
}

impl CounterWithTime {
    #[inline]
    pub fn inc(&self) {
        self.counter.fetch_add(1, Ordering::Acquire);
        self.last_time.store(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            Ordering::Release,
        );
    }
}

// Derive-macros have no way to tell the collector that the parent
// is now a group with the name of the group as the field so we
// can attach multiple values on the same group, so we need to
// manually implement the `MetricsComponent` trait to do so.
impl MetricsComponent for CounterWithTime {
    fn publish(
        &self,
        _kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        let _enter = group!(field_metadata.name).entered();

        publish!(
            "counter",
            &self.counter,
            MetricKind::Counter,
            format!("Current count of {}.", field_metadata.name)
        );
        publish!(
            "last_time",
            &self.last_time,
            MetricKind::Counter,
            format!("Last timestamp {} was published.", field_metadata.name)
        );

        Ok(MetricPublishKnownKindData::Component)
    }
}
