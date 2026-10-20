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

use std::sync::Arc;

use nativelink_config::stores::{CacheMetricsSpec, MemorySpec, RefSpec, StoreSpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_store::cache_metrics_store::CacheMetricsStore;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::ref_store::RefStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::metrics::{CACHE_OPERATION, CACHE_RESULT, CACHE_TYPE};
use nativelink_util::store_trait::{Store, StoreKey, StoreLike};
use opentelemetry::metrics::{
    Counter, Histogram, HistogramBuilder, InstrumentBuilder, InstrumentProvider, Meter,
    MeterProvider, SyncInstrument,
};
use opentelemetry::{InstrumentationScope, KeyValue, global};
use parking_lot::Mutex;

#[derive(Debug, Default)]
struct Recordings<T> {
    values: Mutex<Vec<(T, Vec<KeyValue>)>>,
}

impl<T: Send + Sync> SyncInstrument<T> for Recordings<T> {
    fn measure(&self, value: T, attrs: &[KeyValue]) {
        self.values.lock().push((value, attrs.to_vec()));
    }
}

#[derive(Clone, Debug, Default)]
struct RecordingProvider {
    operations: Arc<Recordings<u64>>,
    durations: Arc<Recordings<f64>>,
}

impl MeterProvider for RecordingProvider {
    fn meter_with_scope(&self, _scope: InstrumentationScope) -> Meter {
        Meter::new(Arc::new(self.clone()))
    }
}

impl InstrumentProvider for RecordingProvider {
    fn u64_counter(&self, builder: InstrumentBuilder<'_, Counter<u64>>) -> Counter<u64> {
        Counter::new(if builder.name == "cache.operations" {
            self.operations.clone()
        } else {
            Arc::new(Recordings::default())
        })
    }

    fn f64_histogram(&self, builder: HistogramBuilder<'_, Histogram<f64>>) -> Histogram<f64> {
        Histogram::new(if builder.name == "cache.operation.duration" {
            self.durations.clone()
        } else {
            Arc::new(Recordings::default())
        })
    }
}

fn wrap(backend: Store, allow_remove: bool) -> Store {
    Store::new(CacheMetricsStore::new(
        &CacheMetricsSpec {
            cache_type: "cas".into(),
            allow_remove,
            backend: StoreSpec::Memory(MemorySpec::default()),
        },
        backend,
    ))
}

// Keep this in its own test binary: CACHE_METRICS initializes its instruments
// once, using the global meter provider installed before constructing stores.
#[nativelink_test]
async fn removal_records_each_outcome_and_duration() -> Result<(), Error> {
    let provider = RecordingProvider::default();
    global::set_meter_provider(provider.clone());
    let manager = Arc::new(StoreManager::new());
    let memory = store_factory(&StoreSpec::Memory(MemorySpec::default()), &manager, None).await?;
    let key = StoreKey::from("repair-entry");
    memory
        .update_oneshot(key.borrow(), "payload".into())
        .await?;
    let enabled = wrap(memory.clone(), true);
    let disabled = wrap(memory.clone(), false);
    provider.operations.values.lock().clear();
    provider.durations.values.lock().clear();

    // Disabled removal must preserve even an existing entry and report a no-op.
    assert!(!disabled.as_store_driver_pin().remove(key.borrow()).await?);
    assert_eq!(memory.has(key.borrow()).await?, Some(7));
    assert!(enabled.as_store_driver_pin().remove(key.borrow()).await?);
    assert!(!enabled.as_store_driver_pin().remove(key.borrow()).await?);

    // An uninitialized reference supplies a real backend error. The wrapper
    // must preserve it while recording the failed removal attempt.
    let failing_backend = Store::new(RefStore::new(
        &RefSpec {
            name: "missing".into(),
        },
        Arc::downgrade(&manager),
    ));
    let expected = failing_backend
        .as_store_driver_pin()
        .remove(key.borrow())
        .await
        .unwrap_err();
    let failing = wrap(failing_backend, true);
    let actual = failing.as_store_driver_pin().remove(key).await.unwrap_err();
    assert_eq!(actual.code, expected.code);
    assert_eq!(actual.messages, expected.messages);

    let operations = provider.operations.values.lock();
    let durations = provider.durations.values.lock();
    let outcomes = ["miss", "success", "miss", "error"];
    assert_eq!(operations.len(), outcomes.len());
    assert_eq!(durations.len(), outcomes.len());
    for (((count, attrs), (duration, duration_attrs)), outcome) in
        operations.iter().zip(durations.iter()).zip(outcomes)
    {
        let expected_attrs = vec![
            KeyValue::new(CACHE_TYPE, "cas"),
            KeyValue::new(CACHE_OPERATION, "delete"),
            KeyValue::new(CACHE_RESULT, outcome),
        ];
        assert_eq!(*count, 1);
        assert_eq!(attrs, &expected_attrs);
        assert_eq!(duration_attrs, &expected_attrs);
        assert!(duration.is_finite() && *duration >= 0.0);
    }
    Ok(())
}
