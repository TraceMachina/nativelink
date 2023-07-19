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
use std::sync::{Arc, Weak};

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

type NameString = String;
type HelpString = String;
type Metric = (NameString, HelpString, MaybeOwned<'static, Box<dyn LocalMetric>>);

#[derive(Default)]
pub struct CollectorState {
    module_name: Option<NameString>,
    metrics: Vec<Metric>,
    text: Vec<(NameString, HelpString, String)>,
    children: Vec<CollectorState>,
}

impl CollectorState {
    /// Publish a numerical metric.
    pub fn publish<N, T>(&mut self, name: impl Into<String>, value: T, help: impl Into<String>)
    where
        N: Debug + 'static,
        T: Into<NumericalMetric<N>>,
        NumericalMetric<N>: EncodeMetric,
    {
        let gague: Box<dyn LocalMetric> = Box::new(value.into());
        self.metrics.push((name.into(), help.into(), MaybeOwned::Owned(gague)));
    }

    /// Publish a static text metric. Generally these are used for labels and don't
    /// change during runtime.
    pub fn publish_text(&mut self, name: impl Into<String>, value: impl Into<String>, help: impl Into<String>) {
        self.text.push((name.into(), help.into(), value.into()));
    }

    /// Publish a child module. The child module must implement the `MetricsComponent`.
    /// The child module will have all of its metrics published prefixed with the
    /// parent's name.
    pub fn publish_child(&mut self, module_name: Option<impl Into<String>>, module: &impl MetricsComponent) {
        let mut state = CollectorState {
            module_name: match (&self.module_name, module_name) {
                (Some(parent), None) => Some(parent.clone()),
                (Some(parent), Some(child)) => Some(format!("{parent}_{}", child.into())),
                (None, Some(child)) => Some(child.into()),
                (None, None) => None,
            },
            metrics: Vec::default(),
            text: Vec::default(),
            children: Vec::default(),
        };
        module.gather_metrics(&mut state);
        self.children.push(state);
    }

    /// Publish a histogram metric. Be careful not to have the iterator take too
    /// much data or this will consume a lot of memory because we need to collect
    /// all the data and sort them to calculate the percentiles.
    pub fn publish_stats<N, T>(
        &mut self,
        name: impl std::fmt::Display,
        data: impl Iterator<Item = T>,
        help: impl std::fmt::Display,
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
        for i in &[0.01, 0.03, 0.05, 0.10, 0.30, 0.50, 0.70, 0.90, 0.95, 0.97, 0.99] {
            let index = (i * data_len) as usize;
            let value = data.get(index).unwrap();
            let p = i * 100.0;
            self.publish(format!("{}_p{:02}", name, p), *value, format!("{} p{:02}", help, p));
        }
    }

    fn into_metrics<'a>(self) -> CollectorResult<'a> {
        let module_name1 = self.module_name.clone();
        let module_name2 = self.module_name.clone();
        Box::new(
            self.metrics
                .into_iter()
                .map(move |(name, help, metric)| {
                    let mut prefix: Option<Prefix> = None;
                    if let Some(parent_prefix) = &module_name1 {
                        prefix = Some(Prefix::from(parent_prefix.clone()));
                    }
                    (
                        Cow::Owned(Descriptor::new(name, help, None, prefix.as_ref(), vec![])),
                        metric,
                    )
                })
                .chain(self.text.into_iter().map(move |(name, help, value)| {
                    let info: Box<dyn LocalMetric> = Box::new(Info::new(vec![(name, value)]));
                    let mut prefix: Option<Prefix> = None;
                    if let Some(parent_prefix) = &module_name2 {
                        prefix = Some(Prefix::from(parent_prefix.clone()));
                    }
                    (
                        Cow::Owned(Descriptor::new("labels", help, None, prefix.as_ref(), vec![])),
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

#[derive(Debug)]
pub struct NumericalMetric<T>(T);

macro_rules! impl_numerical {
    ($($t:ty),*) => {
        $(
            impl From<$t> for NumericalMetric<$t> {
                fn from(t: $t) -> Self {
                    NumericalMetric(t)
                }
            }
        )*
    };
}

// Regsiter all the numerical types to be converted into Numerical.
impl_numerical!(u8, bool, u16, u32, u64, usize, i8, i16, i32, i64, isize);

macro_rules! impl_numerical_metric {
    ($u:ty,$($t:ty),*) => {
        $(
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
