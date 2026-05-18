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

use core::borrow::BorrowMut;
use core::ops::Bound;
use core::pin::Pin;
use std::ffi::OsString;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use nativelink_config::stores::CacheMetricsSpec;
use nativelink_error::{Code, Error};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent, group, publish,
};
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::fs;
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatusIndicator};
use nativelink_util::metrics::{CACHE_METRICS, CACHE_TYPE, CacheMetricAttrs};
use nativelink_util::store_trait::{
    RemoveItemCallback, Store, StoreDriver, StoreKey, StoreLike, StoreOptimizations, UploadSizeInfo,
};
use opentelemetry::KeyValue;

#[derive(Debug)]
pub struct CacheMetricsStore {
    backend: Store,
    cache_type: String,
    attrs: CacheMetricAttrs,
}

impl CacheMetricsStore {
    pub fn new(spec: &CacheMetricsSpec, backend: Store) -> Arc<Self> {
        Arc::new(Self {
            backend,
            cache_type: spec.cache_type.clone(),
            attrs: CacheMetricAttrs::new(&[KeyValue::new(CACHE_TYPE, spec.cache_type.clone())]),
        })
    }

    fn duration_ms(start: Instant) -> f64 {
        start.elapsed().as_secs_f64() * 1000.0
    }

    const fn size_info_bytes(size_info: UploadSizeInfo) -> Option<u64> {
        match size_info {
            UploadSizeInfo::ExactSize(size) => Some(size),
            UploadSizeInfo::MaxSize(_) => None,
        }
    }

    fn record_duration(&self, start: Instant, attrs: &[KeyValue]) {
        CACHE_METRICS
            .cache_operation_duration
            .record(Self::duration_ms(start), attrs);
    }

    fn record_write_io(&self, bytes: Option<u64>) {
        if let Some(bytes) = bytes {
            CACHE_METRICS
                .cache_io
                .add(bytes, self.attrs.write_success());
            CACHE_METRICS
                .cache_entry_size
                .record(bytes, self.attrs.write_success());
        }
    }
}

impl MetricsComponent for CacheMetricsStore {
    fn publish(
        &self,
        _kind: MetricKind,
        _field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        publish!(
            "cache_type",
            &self.cache_type,
            MetricKind::String,
            "Low-cardinality cache type label emitted on cache metrics"
        );
        let _enter = group!("backend").entered();
        self.backend
            .publish(MetricKind::Component, MetricFieldData::default())?;
        Ok(MetricPublishKnownKindData::Component)
    }
}

#[async_trait]
impl StoreDriver for CacheMetricsStore {
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        let start = Instant::now();
        let result = self.backend.has_with_results(keys, results).await;
        match &result {
            Ok(()) => {
                let hits = results.iter().filter(|result| result.is_some()).count();
                let misses = results.len().saturating_sub(hits);
                if hits > 0 {
                    CACHE_METRICS
                        .cache_operations
                        .add(hits as u64, self.attrs.read_hit());
                }
                if misses > 0 {
                    CACHE_METRICS
                        .cache_operations
                        .add(misses as u64, self.attrs.read_miss());
                }
                let duration_attrs = if hits > 0 {
                    self.attrs.read_hit()
                } else {
                    self.attrs.read_miss()
                };
                self.record_duration(start, duration_attrs);
            }
            Err(_) => {
                CACHE_METRICS
                    .cache_operations
                    .add(keys.len() as u64, self.attrs.read_error());
                self.record_duration(start, self.attrs.read_error());
            }
        }
        result
    }

    async fn list(
        self: Pin<&Self>,
        range: (Bound<StoreKey<'_>>, Bound<StoreKey<'_>>),
        handler: &mut (dyn for<'a> FnMut(&'a StoreKey) -> bool + Send + Sync + '_),
    ) -> Result<u64, Error> {
        self.backend
            .list(
                (
                    range.0.map(StoreKey::into_owned),
                    range.1.map(StoreKey::into_owned),
                ),
                handler,
            )
            .await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        let start = Instant::now();
        let bytes = Self::size_info_bytes(upload_size);
        let result = self.backend.update(key, reader, upload_size).await;
        match &result {
            Ok(()) => {
                CACHE_METRICS
                    .cache_operations
                    .add(1, self.attrs.write_success());
                self.record_write_io(bytes);
                self.record_duration(start, self.attrs.write_success());
            }
            Err(_) => {
                CACHE_METRICS
                    .cache_operations
                    .add(1, self.attrs.write_error());
                self.record_duration(start, self.attrs.write_error());
            }
        }
        result
    }

    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        self.backend.optimized_for(optimization)
    }

    async fn update_with_whole_file(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        path: OsString,
        file: fs::FileSlot,
        upload_size: UploadSizeInfo,
    ) -> Result<Option<fs::FileSlot>, Error> {
        let start = Instant::now();
        let bytes = Self::size_info_bytes(upload_size);
        let result = self
            .backend
            .update_with_whole_file(key, path, file, upload_size)
            .await;
        match &result {
            Ok(_) => {
                CACHE_METRICS
                    .cache_operations
                    .add(1, self.attrs.write_success());
                self.record_write_io(bytes);
                self.record_duration(start, self.attrs.write_success());
            }
            Err(_) => {
                CACHE_METRICS
                    .cache_operations
                    .add(1, self.attrs.write_error());
                self.record_duration(start, self.attrs.write_error());
            }
        }
        result
    }

    async fn update_oneshot(self: Pin<&Self>, key: StoreKey<'_>, data: Bytes) -> Result<(), Error> {
        let start = Instant::now();
        let bytes = data.len() as u64;
        let result = self.backend.update_oneshot(key, data).await;
        match &result {
            Ok(()) => {
                CACHE_METRICS
                    .cache_operations
                    .add(1, self.attrs.write_success());
                self.record_write_io(Some(bytes));
                self.record_duration(start, self.attrs.write_success());
            }
            Err(_) => {
                CACHE_METRICS
                    .cache_operations
                    .add(1, self.attrs.write_error());
                self.record_duration(start, self.attrs.write_error());
            }
        }
        result
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        let start = Instant::now();
        let result = self
            .backend
            .get_part(key, writer.borrow_mut(), offset, length)
            .await;
        match &result {
            Ok(()) => {
                CACHE_METRICS.cache_operations.add(1, self.attrs.read_hit());
                CACHE_METRICS
                    .cache_io
                    .add(writer.get_bytes_written(), self.attrs.read_hit());
                self.record_duration(start, self.attrs.read_hit());
            }
            Err(err) if err.code == Code::NotFound => {
                CACHE_METRICS
                    .cache_operations
                    .add(1, self.attrs.read_miss());
                self.record_duration(start, self.attrs.read_miss());
            }
            Err(_) => {
                CACHE_METRICS
                    .cache_operations
                    .add(1, self.attrs.read_error());
                self.record_duration(start, self.attrs.read_error());
            }
        }
        result
    }

    fn inner_store(&self, _key: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_health(self: Arc<Self>, registry: &mut HealthRegistryBuilder) {
        self.backend.clone().register_health(registry);
    }

    fn register_remove_callback(
        self: Arc<Self>,
        callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        self.backend.register_remove_callback(callback)
    }
}

#[async_trait]
impl HealthStatusIndicator for CacheMetricsStore {
    fn get_name(&self) -> &'static str {
        "CacheMetricsStore"
    }

    async fn check_health(
        &self,
        namespace: std::borrow::Cow<'static, str>,
    ) -> nativelink_util::health_utils::HealthStatus {
        self.backend
            .as_store_driver_pin()
            .check_health(namespace)
            .await
    }
}
