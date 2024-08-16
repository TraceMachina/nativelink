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

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt::Debug;
use std::marker::PhantomData;
use std::ops::DerefMut;
use std::sync::Arc;

use parking_lot::Mutex;
use tracing::span::Attributes;
use tracing::subscriber::Interest;
use tracing::{Event, Id, Metadata, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::registry::SpanRef;
use tracing_subscriber::Layer;

use crate::metrics_collection::{
    CollectedMetricChildren, CollectedMetricPrimitive, CollectedMetrics, RootMetricCollectedMetrics,
};
use crate::metrics_visitors::{MetricDataVisitor, SpanFields};

/// The layer that is given to `tracing` to collect metrics.
/// The output of the metrics will be populated in the `root_collected_metrics`
/// field.
pub struct MetricsCollectorLayer<S> {
    spans: Mutex<HashMap<Id, SpanFields>>,
    root_collected_metrics: Arc<Mutex<RootMetricCollectedMetrics>>,
    _subscriber: PhantomData<S>,
}

impl<S> MetricsCollectorLayer<S> {
    /// Creates a new `MetricsCollectorLayer` and returns it along with the
    /// `root_collected_metrics` that will be populated with the collected metrics.
    pub fn new() -> (Self, Arc<Mutex<RootMetricCollectedMetrics>>) {
        let root_collected_metrics = Arc::new(Mutex::new(RootMetricCollectedMetrics::default()));
        (
            MetricsCollectorLayer {
                spans: Mutex::new(HashMap::new()),
                root_collected_metrics: root_collected_metrics.clone(),
                _subscriber: PhantomData,
            },
            root_collected_metrics,
        )
    }
}

impl<S> Layer<S> for MetricsCollectorLayer<S>
where
    S: Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a> + Debug,
{
    fn enabled(&self, metadata: &Metadata<'_>, _ctx: Context<'_, S>) -> bool {
        metadata.target() == "nativelink_metric"
    }

    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, _ctx: Context<'_, S>) {
        let mut span_fields = SpanFields {
            name: Cow::Borrowed(attrs.metadata().name()),
        };
        // Store the current metadata values map representing the current span.
        // We need to 'snapshot' the current span, because when a more recent
        // span (such as the one being initialized) updates, these values will
        // be overwritten.
        attrs.values().record(&mut span_fields);

        self.spans.lock().insert(id.clone(), span_fields);
    }

    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        let mut event_visitor = MetricDataVisitor::default();
        // First, we populate the MetricDataVisitor we are interested
        // in from the event.
        event.record(&mut event_visitor);
        // This represents the field we are concerned with updating or
        // initializing.
        let name = event_visitor.name.clone();

        let mut root_collected_metrics = self.root_collected_metrics.lock();
        let collected_component = root_collected_metrics.deref_mut().deref_mut();

        // Find out which span we are currently in and retrieve its metadata.
        // It is possible to not be in a span in the tracing library.
        // If we are not in a span, we assume you want your metrics published
        // in the root of the collected metrics.
        if let Some(current_span) = ctx.lookup_current() {
            let mut known_spans = self.spans.lock();
            // By default tracing starts you at the bottom of the span tree,
            // but we want to start at the root of the tree and walk down,
            // so invert it.
            let span_iter = current_span.scope().from_root();
            // Find the layer in our output struct we are going to populate
            // the data into.
            let collected_component =
                find_component(span_iter, known_spans.deref_mut(), collected_component);

            // Get the new value from the event and update it in the component.
            let primitive = CollectedMetricPrimitive::from(event_visitor);
            collected_component.insert(name, CollectedMetrics::Primitive(primitive));
        } else {
            let primitive = CollectedMetricPrimitive::from(event_visitor);
            collected_component.insert(name, CollectedMetrics::Primitive(primitive));
        }
    }

    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::always()
    }
}

fn find_component<'a, 'b, S, I>(
    mut iter: I,
    known_spans: &'a mut HashMap<Id, SpanFields>,
    mut collected_component: &'a mut CollectedMetricChildren,
) -> &'a mut CollectedMetricChildren
where
    S: Subscriber + for<'c> tracing_subscriber::registry::LookupSpan<'c> + Debug,
    I: Iterator<Item = SpanRef<'b, S>>,
{
    let Some(span) = iter.next() else {
        // Once there are no more nested spans, we have reached a leaf field.
        return collected_component;
    };
    let span_fields = known_spans.get(&span.id()).expect("Span not found");
    // LayerMap<Name, Either<LayerMap, Primitive>>
    // This is a hashmap of the existing data for the layer
    let collected_metric = collected_component
        .entry(span_fields.name.to_string())
        .or_insert_with(CollectedMetrics::new_component);

    collected_component = match collected_metric {
        CollectedMetrics::Component(component) => component.deref_mut(),
        _ => panic!("Expected to be component"),
    };
    // DFS the iterator of keys and return the first leaf found matching the name query.
    find_component(iter, known_spans, collected_component)
}
