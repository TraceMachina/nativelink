// Copyright 2023 The NativeLink Authors. All rights reserved.
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

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use nativelink_error::{Error, ResultExt};
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{default_health_status_indicator, HealthStatusIndicator};
use nativelink_util::metrics_utils::{Collector, CollectorState, MetricsComponent, Registry};
use nativelink_util::store_trait::{Store, UploadSizeInfo};
use tokio::join;

pub struct SizePartitioningStore {
    partition_size: i64,
    lower_store: Arc<dyn Store>,
    upper_store: Arc<dyn Store>,
}

impl SizePartitioningStore {
    pub fn new(
        config: &nativelink_config::stores::SizePartitioningStore,
        lower_store: Arc<dyn Store>,
        upper_store: Arc<dyn Store>,
    ) -> Self {
        SizePartitioningStore {
            partition_size: config.size as i64,
            lower_store,
            upper_store,
        }
    }
}

#[async_trait]
impl Store for SizePartitioningStore {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        let (lower_digests, upper_digests): (Vec<_>, Vec<_>) = digests
            .iter()
            .cloned()
            .partition(|digest| digest.size_bytes < self.partition_size);
        let (lower_results, upper_results) = join!(
            Pin::new(self.lower_store.as_ref()).has_many(&lower_digests),
            Pin::new(self.upper_store.as_ref()).has_many(&upper_digests),
        );
        let mut lower_results = match lower_results {
            Ok(lower_results) => lower_results.into_iter(),
            Err(err) => match upper_results {
                Ok(_) => return Err(err),
                Err(upper_err) => return Err(err.merge(upper_err)),
            },
        };
        let mut upper_digests = upper_digests.into_iter().peekable();
        let mut upper_results = upper_results?.into_iter();
        for (digest, result) in digests.iter().zip(results.iter_mut()) {
            if Some(digest) == upper_digests.peek() {
                upper_digests.next();
                *result = upper_results
                    .next()
                    .err_tip(|| "upper_results out of sync with upper_digests")?;
            } else {
                *result = lower_results
                    .next()
                    .err_tip(|| "lower_results out of sync with lower_digests")?;
            }
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        if digest.size_bytes < self.partition_size {
            return Pin::new(self.lower_store.as_ref())
                .update(digest, reader, size_info)
                .await;
        }
        Pin::new(self.upper_store.as_ref())
            .update(digest, reader, size_info)
            .await
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        if digest.size_bytes < self.partition_size {
            return Pin::new(self.lower_store.as_ref())
                .get_part_ref(digest, writer, offset, length)
                .await;
        }
        Pin::new(self.upper_store.as_ref())
            .get_part_ref(digest, writer, offset, length)
            .await
    }

    fn inner_store(&self, digest: Option<DigestInfo>) -> &'_ dyn Store {
        let Some(digest) = digest else {
            return self;
        };
        if digest.size_bytes < self.partition_size {
            return self.lower_store.inner_store(Some(digest));
        }
        self.upper_store.inner_store(Some(digest))
    }

    fn inner_store_arc(self: Arc<Self>, digest: Option<DigestInfo>) -> Arc<dyn Store> {
        let Some(digest) = digest else {
            return self;
        };
        if digest.size_bytes < self.partition_size {
            return self.lower_store.clone().inner_store_arc(Some(digest));
        }
        self.upper_store.clone().inner_store_arc(Some(digest))
    }

    fn as_any<'a>(&'a self) -> &'a (dyn std::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_metrics(self: Arc<Self>, registry: &mut Registry) {
        let lower_store_registry = registry.sub_registry_with_prefix("lower_store");
        self.lower_store
            .clone()
            .register_metrics(lower_store_registry);
        let upper_store_registry = registry.sub_registry_with_prefix("upper_store");
        self.upper_store
            .clone()
            .register_metrics(upper_store_registry);
        registry.register_collector(Box::new(Collector::new(&self)));
    }
}

impl MetricsComponent for SizePartitioningStore {
    fn gather_metrics(&self, c: &mut CollectorState) {
        c.publish(
            "partition_size",
            &self.partition_size,
            "Size to partition our data",
        );
    }
}

default_health_status_indicator!(SizePartitioningStore);
