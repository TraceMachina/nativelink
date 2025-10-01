// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use core::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use nativelink_config::stores::SizePartitioningSpec;
use nativelink_error::{Error, ResultExt, make_input_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    RemoveItemCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use tokio::join;

#[derive(Debug, MetricsComponent)]
pub struct SizePartitioningStore {
    #[metric(help = "Size to partition our data")]
    partition_size: u64,
    #[metric(group = "lower_store")]
    lower_store: Store,
    #[metric(group = "upper_store")]
    upper_store: Store,
}

impl SizePartitioningStore {
    pub fn new(spec: &SizePartitioningSpec, lower_store: Store, upper_store: Store) -> Arc<Self> {
        Arc::new(Self {
            partition_size: spec.size,
            lower_store,
            upper_store,
        })
    }
}

#[async_trait]
impl StoreDriver for SizePartitioningStore {
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'static>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        let mut non_digest_sample = None;
        let (lower_digests, upper_digests): (Vec<_>, Vec<_>) =
            keys.iter().cloned().partition(|k| {
                let StoreKey::Digest(digest) = k else {
                    non_digest_sample = Some(k.borrow().into_owned());
                    return false;
                };
                digest.size_bytes() < self.partition_size
            });
        if let Some(non_digest) = non_digest_sample {
            return Err(make_input_err!(
                "SizePartitioningStore only supports Digest keys, got {non_digest:?}"
            ));
        }
        let (lower_results, upper_results) = join!(
            self.lower_store.has_many(&lower_digests),
            self.upper_store.has_many(&upper_digests),
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
        for (digest, result) in keys.iter().zip(results.iter_mut()) {
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
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        let digest = match key {
            StoreKey::Digest(digest) => digest,
            other @ StoreKey::Str(_) => {
                return Err(make_input_err!(
                    "SizePartitioningStore only supports Digest keys, got {other:?}"
                ));
            }
        };
        if digest.size_bytes() < self.partition_size {
            return self.lower_store.update(digest, reader, size_info).await;
        }
        self.upper_store.update(digest, reader, size_info).await
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'static>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        let digest = match key {
            StoreKey::Digest(digest) => digest,
            other @ StoreKey::Str(_) => {
                return Err(make_input_err!(
                    "SizePartitioningStore only supports Digest keys, got {other:?}"
                ));
            }
        };
        if digest.size_bytes() < self.partition_size {
            return self
                .lower_store
                .get_part(digest, writer, offset, length)
                .await;
        }
        self.upper_store
            .get_part(digest, writer, offset, length)
            .await
    }

    fn inner_store(&self, key: Option<StoreKey>) -> &'_ dyn StoreDriver {
        let Some(key) = key else {
            return self;
        };
        let StoreKey::Digest(digest) = key else {
            return self;
        };
        if digest.size_bytes() < self.partition_size {
            return self.lower_store.inner_store(Some(digest));
        }
        self.upper_store.inner_store(Some(digest))
    }

    fn as_any<'a>(&'a self) -> &'a (dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_remove_callback(self: Arc<Self>, callback: &Arc<Box<dyn RemoveItemCallback>>) {
        self.lower_store.register_remove_callback(callback);
        self.upper_store.register_remove_callback(callback);
    }
}

default_health_status_indicator!(SizePartitioningStore);
