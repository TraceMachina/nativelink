// Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

use std::borrow::Cow;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::mem::forget;
use std::sync::atomic::{
    AtomicBool, AtomicI16, AtomicI32, AtomicI64, AtomicI8, AtomicIsize, AtomicU16, AtomicU32, AtomicU64, AtomicU8,
    AtomicUsize, Ordering,
};
use std::sync::{Arc, Weak};
use std::thread_local;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use futures::Future;
use prometheus_client::collector::Collector as PrometheusCollector;
use prometheus_client::encoding::{EncodeMetric, MetricEncoder};
use prometheus_client::metrics::info::Info;
use prometheus_client::metrics::MetricType;
pub use prometheus_client::registry::Registry;
use prometheus_client::registry::{Descriptor, LocalMetric, Prefix};
use prometheus_client::MaybeOwned;

/// A component that can be registered with the metrics collector.
pub trait MetricsComponent {
    /// This method will magically be called by the metrics collector to gather
    /// all the metrics from this component if it has been registered.
    /// This function should be extremely fast.
    ///
    /// It is safe to block in this function.
    fn gather_metrics(&self, collector: &mut CollectorState);
}

thread_local! {
    /// This is a thread local variable that will enable or disable metrics for
    /// the current thread. This does not mean that metrics are "disabled"
    /// everywhere. It only means that metrics gathering for this specific thread
    /// will be disabled. Because tokio uses thread pools, if you change this
    /// value you'll need to change it on every thread tokio is using, often using
    /// the `tokio::runtime::Builder::on_thread_start` function. This field also
    /// does not mean that metrics cannot be pulled from the registry. It only
    /// removes the ability for metrics that are collected at runtime (hot path)
    /// from being collected.
    pub static METRICS_ENABLED: AtomicBool = const { AtomicBool::new(true) };
}

#[inline]
pub fn metrics_enabled() -> bool {
    METRICS_ENABLED.with(
        #[inline]
        |v| v.load(Ordering::Relaxed),
    )
}

/// This function will enable or disable metrics for the current thread.
/// WARNING: This will only happen for this thread. Tokio uses thread pools
/// so you'd need to run this function on every thread in the thread pool in
/// order to enable it everywhere.
pub fn set_metrics_enabled_for_this_thread(enabled: bool) {
    METRICS_ENABLED.with(|v| v.store(enabled, Ordering::Relaxed));
}

type NameString = String;
type HelpString = String;
type Metric = (
    NameString,
    HelpString,
    MaybeOwned<'static, Box<dyn LocalMetric>>,
    Vec<(Cow<'static, str>, Cow<'static, str>)>,
);

type TextMetric = (
    NameString,
    HelpString,
    String,
    Vec<(Cow<'static, str>, Cow<'static, str>)>,
);

#[derive(Default)]
pub struct CollectorState {
    module_name: Option<NameString>,
    metrics: Vec<Metric>,
    text: Vec<TextMetric>,
    children: Vec<CollectorState>,
}

impl CollectorState {
    /// Publishes a value. This should be the primary way a metric is published.
    /// Any special types that want metrics published should implement `MetricPublisher`
    /// for that type.
    #[inline]
    pub fn publish(&mut self, name: impl Into<String>, value: impl MetricPublisher, help: impl Into<String>) {
        value.publish(self, name.into(), help.into());
    }

    /// Publish a numerical metric. Usually used by `MetricPublisher` to publish metrics.
    #[inline]
    pub fn publish_number<N, T>(
        &mut self,
        name: impl Into<String>,
        value: T,
        help: impl Into<String>,
        labels: impl Into<Vec<(Cow<'static, str>, Cow<'static, str>)>>,
    ) where
        N: Debug + 'static,
        T: Into<NumericalMetric<N>>,
        NumericalMetric<N>: EncodeMetric,
    {
        let gague: Box<dyn LocalMetric> = Box::new(value.into());
        self.metrics
            .push((name.into(), help.into(), MaybeOwned::Owned(gague), labels.into()));
    }

    /// Publish a static text metric. Generally these are used for labels and don't
    /// change during runtime. Usually used by `MetricPublisher` to publish metrics.
    #[inline]
    pub fn publish_text(
        &mut self,
        name: impl Into<String>,
        value: impl Into<String>,
        help: impl Into<String>,
        labels: impl Into<Vec<(Cow<'static, str>, Cow<'static, str>)>>,
    ) {
        self.text.push((name.into(), help.into(), value.into(), labels.into()));
    }

    /// Publish a histogram metric. Be careful not to have the iterator take too
    /// much data or this will consume a lot of memory because we need to collect
    /// all the data and sort them to calculate the percentiles.
    #[inline]
    pub fn publish_stats<N, T>(
        &mut self,
        name: impl Into<String> + Clone,
        data: impl Iterator<Item = T>,
        help: impl Into<String> + Clone,
    ) where
        N: Debug + 'static,
        T: Into<NumericalMetric<N>> + Ord + Copy + std::fmt::Display,
        NumericalMetric<N>: EncodeMetric,
    {
        let mut data = data.collect::<Vec<T>>();
        if data.is_empty() {
            return;
        }
        data.sort_unstable();
        let data_len = data.len() as f64;
        for i in &[
            0.00, 0.01, 0.03, 0.05, 0.10, 0.30, 0.50, 0.70, 0.90, 0.95, 0.97, 0.99, 1.00,
        ] {
            let index = (i * data_len) as usize;
            let value = data.get(if index < data.len() { index } else { index - 1 }).unwrap();
            let labels = vec![("quantile".into(), format!("{:.2}", i).into())];
            self.publish_number(name.clone(), *value, help.clone(), labels);
        }
    }

    fn into_metrics<'a>(self) -> CollectorResult<'a> {
        let module_name1 = self.module_name.clone();
        let module_name2 = self.module_name.clone();
        Box::new(
            self.metrics
                .into_iter()
                .map(move |(name, help, metric, labels)| {
                    let mut prefix: Option<Prefix> = None;
                    if let Some(parent_prefix) = &module_name1 {
                        prefix = Some(Prefix::from(parent_prefix.clone()));
                    }
                    (
                        Cow::Owned(Descriptor::new(name, help, None, prefix.as_ref(), labels)),
                        metric,
                    )
                })
                .chain(self.text.into_iter().map(move |(name, help, value, labels)| {
                    let info: Box<dyn LocalMetric> = Box::new(Info::new(vec![(name, value)]));
                    let mut prefix: Option<Prefix> = None;
                    if let Some(parent_prefix) = &module_name2 {
                        prefix = Some(Prefix::from(parent_prefix.clone()));
                    }
                    (
                        Cow::Owned(Descriptor::new("labels", help, None, prefix.as_ref(), labels)),
                        MaybeOwned::Owned(info),
                    )
                }))
                .chain(
                    self.children
                        .into_iter()
                        .flat_map(move |child_state| child_state.into_metrics()),
                ),
        )
    }
}

/// This is a utility that will only increment the referenced counter when it is dropped.
/// This struct is zero cost and has a runtime cost only when it is dropped.
/// This struct is very useful for tracking when futures are dropped.
struct DropCounter<'a> {
    counter: &'a AtomicU64,
}

impl<'a> DropCounter<'a> {
    #[inline]
    pub fn new(counter: &'a AtomicU64) -> Self {
        Self { counter }
    }
}

impl<'a> Drop for DropCounter<'a> {
    #[inline]
    fn drop(&mut self) {
        if !metrics_enabled() {
            return;
        }
        self.counter.fetch_add(1, Ordering::Relaxed);
    }
}

pub struct AsyncTimer<'a> {
    start: Instant,
    drop_counter: DropCounter<'a>,
    counter: &'a AsyncCounterWrapper,
}

impl<'a> AsyncTimer<'a> {
    #[inline]
    pub fn measure(self) {
        if !metrics_enabled() {
            return;
        }
        self.counter
            .sum_func_duration_ns
            .fetch_add(self.start.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.counter.calls.fetch_add(1, Ordering::Relaxed);
        self.counter.successes.fetch_add(1, Ordering::Relaxed);
        // This causes DropCounter's drop to never be called.
        forget(self.drop_counter);
    }
}

/// Tracks the number of calls, successes, failures, and drops of an async function.
/// call `.wrap(future)` to wrap a future and stats about the future are automatically
/// tracked and can be published to a `CollectorState`.
#[derive(Default)]
pub struct AsyncCounterWrapper {
    calls: AtomicU64,
    successes: AtomicU64,
    failures: AtomicU64,
    drops: AtomicU64,
    // Time spent in nano seconds in the future.
    // 64 bit address space gives ~584 years of nanoseconds.
    sum_func_duration_ns: AtomicU64,
}

impl AsyncCounterWrapper {
    #[inline]
    pub async fn wrap<'a, T, E, F: Future<Output = Result<T, E>> + 'a>(&'a self, future: F) -> Result<T, E> {
        if !metrics_enabled() {
            return future.await;
        }
        let result = self.wrap_no_capture_result(future).await;
        if result.is_ok() {
            self.successes.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failures.fetch_add(1, Ordering::Relaxed);
        }
        result
    }

    #[inline]
    pub async fn wrap_no_capture_result<'a, T, F: Future<Output = T> + 'a>(&'a self, future: F) -> T {
        if !metrics_enabled() {
            return future.await;
        }
        self.calls.fetch_add(1, Ordering::Relaxed);
        let drop_counter = DropCounter::new(&self.drops);
        let instant = Instant::now();
        let result = future.await;
        // By default `drop_counter` will increment the drop counter when it goes out of scope.
        // This will ensure we don't increment the counter if we make it here with a zero cost.
        forget(drop_counter);
        self.sum_func_duration_ns
            .fetch_add(instant.elapsed().as_nanos() as u64, Ordering::Relaxed);
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

impl MetricPublisher for &AsyncCounterWrapper {
    #[inline]
    fn publish(&self, state: &mut CollectorState, name: String, help: String) {
        let calls = self.calls.load(Ordering::Relaxed);
        let successes = self.successes.load(Ordering::Relaxed);
        let failures = self.failures.load(Ordering::Relaxed);
        let drops = self.drops.load(Ordering::Relaxed);
        let active = calls - successes - failures - drops;
        let non_zero_calls = if calls == 0 { 1 } else { calls };
        let avg_duration_ns = self.sum_func_duration_ns.load(Ordering::Relaxed) / non_zero_calls;
        state.publish_number(
            name.clone(),
            drops,
            format!("{help} The number of dropped futures."),
            vec![("type".into(), "drop".into())],
        );
        state.publish_number(
            name.clone(),
            successes,
            format!("{help} The number of successes."),
            vec![("type".into(), "success".into())],
        );
        state.publish_number(
            name.clone(),
            failures,
            format!("{help} The number of failures."),
            vec![("type".into(), "failure".into())],
        );
        state.publish_number(
            name.clone(),
            active,
            format!("{help} The number of active futures."),
            vec![("type".into(), "active".into())],
        );
        state.publish(
            format!("{name}_avg_duration_ns"),
            &avg_duration_ns,
            format!("{help} The average number of nanos spent in future."),
        );
    }
}

/// Tracks an number.
#[derive(Default)]
pub struct Counter(AtomicU64);

impl Counter {
    #[inline]
    pub fn inc(&self) {
        self.add(1);
    }

    #[inline]
    pub fn add(&self, value: u64) {
        if !metrics_enabled() {
            return;
        }
        self.0.fetch_add(value, Ordering::Relaxed);
    }

    #[inline]
    pub fn sub(&self, value: u64) {
        if !metrics_enabled() {
            return;
        }
        self.0.fetch_sub(value, Ordering::Relaxed);
    }
}

impl MetricPublisher for &Counter {
    #[inline]
    fn publish(&self, state: &mut CollectorState, name: String, help: String) {
        state.publish(name, &self.0, help);
    }
}

/// Tracks an counter through time and the last time the counter was changed.
#[derive(Default)]
pub struct CounterWithTime {
    counter: AtomicU64,
    last_time: AtomicI64,
}

impl CounterWithTime {
    #[inline]
    pub fn inc(&self) {
        if !metrics_enabled() {
            return;
        }
        self.counter.fetch_add(1, Ordering::Relaxed);
        self.last_time.store(
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64,
            Ordering::Relaxed,
        );
    }
}

impl MetricPublisher for &CounterWithTime {
    #[inline]
    fn publish(&self, state: &mut CollectorState, name: String, help: String) {
        state.publish(
            format!("{name}_last_ts"),
            &self.last_time,
            format!("The timestamp of when {name} was last published"),
        );
        state.publish(name, &self.counter, help);
    }
}

pub struct Collector<T>
where
    T: Sync + Send + 'static,
{
    handle: Weak<T>,
    _marker: PhantomData<T>,
}

impl<T> Collector<T>
where
    T: Sync + Send + 'static,
{
    pub fn new(handle: &Arc<T>) -> Self {
        Self {
            handle: Arc::downgrade(handle),
            _marker: PhantomData,
        }
    }
}

impl<T: Sync + Send + 'static> Debug for Collector<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Collector").finish()
    }
}

type CollectorResult<'a> = Box<dyn Iterator<Item = (Cow<'a, Descriptor>, MaybeOwned<'a, Box<dyn LocalMetric>>)> + 'a>;

impl<S: MetricsComponent + Sync + Send + 'static> PrometheusCollector for Collector<S> {
    fn collect(&self) -> CollectorResult {
        let Some(handle) = self.handle.upgrade() else {
            // Don't report any metrics if the component is no longer alive.
            return Box::new(std::iter::empty());
        };

        let mut state = CollectorState::default();
        handle.gather_metrics(&mut state);
        state.into_metrics()
    }
}

pub trait MetricPublisher {
    /// Publish a gague metric.
    fn publish(&self, state: &mut CollectorState, name: String, help: String);
}

/// Implements MetricPublisher for string types.
impl MetricPublisher for &String {
    #[inline]
    fn publish(&self, state: &mut CollectorState, name: String, help: String) {
        state.publish_text(name, *self, help, vec![]);
    }
}

/// Implements MetricPublisher for string types.
impl<T> MetricPublisher for &T
where
    T: MetricsComponent,
{
    #[inline]
    fn publish(&self, parent_state: &mut CollectorState, module_name: String, _help: String) {
        let module_name = if module_name.is_empty() {
            None
        } else {
            Some(module_name)
        };
        let mut state = CollectorState {
            module_name: match (&parent_state.module_name, module_name) {
                (Some(parent), None) => Some(parent.clone()),
                (Some(parent), Some(child)) => Some(format!("{parent}_{}", child)),
                (None, child) => child,
            },
            metrics: Vec::default(),
            text: Vec::default(),
            children: Vec::default(),
        };
        self.gather_metrics(&mut state);
        parent_state.children.push(state);
    }
}

macro_rules! impl_publish_atomic {
    ($($t:ty),*) => {
        $(
            impl MetricPublisher for &$t {
                #[inline]
                fn publish(&self, state: &mut CollectorState, name: String, help: String) {
                    state.publish_number(name, &self.load(Ordering::Relaxed), help, vec![]);
                }
            }
        )*
    };
}

impl_publish_atomic!(
    AtomicU8,
    AtomicU16,
    AtomicU32,
    AtomicU64,
    AtomicUsize,
    AtomicI8,
    AtomicI16,
    AtomicI32,
    AtomicI64,
    AtomicIsize
);

#[derive(Debug)]
pub struct NumericalMetric<T>(T);

macro_rules! impl_numerical {
    ($($t:ty),*) => {
        $(
            impl From<$t> for NumericalMetric<$t> {
                #[inline]
                fn from(t: $t) -> Self {
                    NumericalMetric(t)
                }
            }
            impl From<&$t> for NumericalMetric<$t> {
                #[inline]
                fn from(t: &$t) -> Self {
                    NumericalMetric(*t)
                }
            }
        )*
    };
}

// Regsiter all the numerical types to be converted into Numerical.
impl_numerical!(u8, bool, u16, u32, u64, usize, i8, i16, i32, i64, isize, f32, f64);

macro_rules! impl_numerical_metric {
    ($u:ty,$($t:ty),*) => {
        $(
            impl MetricPublisher for &$t {
                #[inline]
                fn publish(&self, state: &mut CollectorState, name: String, help: String) {
                    state.publish_number(name, *self, help, vec![]);
                }
            }

            impl EncodeMetric for NumericalMetric<$t> {
                fn encode(&self, mut encoder: MetricEncoder) -> Result<(), std::fmt::Error> {
                    encoder.encode_gauge(&TryInto::<$u>::try_into(self.0).map_err(|_| std::fmt::Error::default())?)
                }

                fn metric_type(&self) -> MetricType {
                    MetricType::Gauge
                }
            }
        )*
    };
}
// Implement metrics for all the numerical integer types by trying to cast it to i64.
impl_numerical_metric!(i64, bool, u8, u16, u32, u64, usize, i8, i16, i32, i64, isize);

// Implement metrics for all float types by trying to cast it to f64.
impl_numerical_metric!(f64, f64, f32);
