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

use core::fmt::Debug;
use core::pin::Pin;
use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use futures::TryStreamExt;
use futures::stream::FuturesUnordered;
use nativelink_config::stores::ExperimentalGcsSpec;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::store_trait::{RemoveItemCallback, StoreDriver, StoreKey, UploadSizeInfo};

use crate::cas_utils::is_zero_digest;
use crate::gcs_client::client::{GcsClient, GcsOperations};
use crate::gcs_client::types::ObjectPath;

#[derive(MetricsComponent, Debug)]
pub struct GcsStore<Client: GcsOperations, NowFn> {
    client: Arc<Client>,
    now_fn: NowFn,
    #[metric(help = "The bucket name for the GCS store")]
    bucket: String,
    #[metric(help = "The key prefix for the GCS store")]
    key_prefix: String,
    #[metric(help = "The number of seconds to consider an object expired")]
    consider_expired_after_s: i64,
}

impl<I, NowFn> GcsStore<GcsClient, NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    pub async fn new(spec: &ExperimentalGcsSpec, now_fn: NowFn) -> Result<Arc<Self>, Error> {
        let client = Arc::new(GcsClient::new(spec).await?);
        Self::new_with_ops(spec, client, now_fn)
    }
}

impl<I, Client, NowFn> GcsStore<Client, NowFn>
where
    I: InstantWrapper,
    Client: GcsOperations + Send + Sync,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    // Primarily used for injecting a mock or real operations implementation
    pub fn new_with_ops(
        spec: &ExperimentalGcsSpec,
        client: Arc<Client>,
        now_fn: NowFn,
    ) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self {
            client,
            now_fn,
            bucket: spec.bucket.clone(),
            key_prefix: spec
                .common
                .key_prefix
                .as_ref()
                .unwrap_or(&String::new())
                .clone(),
            consider_expired_after_s: i64::from(spec.common.consider_expired_after_s),
        }))
    }

    async fn has(self: Pin<&Self>, key: &StoreKey<'_>) -> Result<Option<u64>, Error> {
        let object_path = self.make_object_path(key);
        let client = &self.client;
        let consider_expired_after_s = self.consider_expired_after_s;
        let now_fn = &self.now_fn;

        match client.read_object_metadata(&object_path).await.err_tip(|| {
            format!(
                "Error while trying to read - bucket: {} path: {}",
                object_path.bucket, object_path.path
            )
        }) {
            Ok(Some(metadata)) => {
                if consider_expired_after_s != 0 {
                    if let Some(update_time) = &metadata.update_time {
                        let now_s = now_fn().unix_timestamp() as i64;
                        if update_time.seconds + consider_expired_after_s <= now_s {
                            return Ok(None);
                        }
                    }
                }

                if metadata.size >= 0 {
                    Ok(Some(metadata.size as u64))
                } else {
                    Err(make_err!(
                        Code::InvalidArgument,
                        "Invalid metadata size in GCS: {}",
                        metadata.size
                    ))
                }
            }
            Ok(None) => Ok(None),
            Err(err) if err.code == Code::NotFound => Ok(None),
            Err(err) => Err(err),
        }
    }

    fn make_object_path(&self, key: &StoreKey) -> ObjectPath {
        ObjectPath::new(
            self.bucket.clone(),
            &format!("{}{}", self.key_prefix, key.as_str()),
        )
    }
}

#[async_trait]
impl<I, Client, NowFn> StoreDriver for GcsStore<Client, NowFn>
where
    I: InstantWrapper,
    Client: GcsOperations + 'static,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        keys.iter()
            .zip(results.iter_mut())
            .map(|(key, result)| async move {
                if is_zero_digest(key.borrow()) {
                    *result = Some(0);
                    return Ok(());
                }
                *result = self.has(key).await?;
                Ok(())
            })
            .collect::<FuturesUnordered<_>>()
            .try_collect()
            .await
    }

    async fn update(
        self: Pin<&Self>,
        digest: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        _upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        if is_zero_digest(digest.borrow()) {
            return reader.recv().await.and_then(|should_be_empty| {
                should_be_empty
                    .is_empty()
                    .then_some(())
                    .ok_or_else(|| make_err!(Code::Internal, "Zero byte hash not empty"))
            });
        }

        let object_path = self.make_object_path(&digest);
        self.client.upload_from_reader(&object_path, reader).await
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        if is_zero_digest(key.borrow()) {
            writer.send_eof()?;
            return Ok(());
        }

        let object_path = self.make_object_path(&key);
        let end_offset = length.map(|len| offset + len);
        let mut stream = self
            .client
            .read_object_content(&object_path, offset, end_offset)
            .await?;
        while let Some(chunk) = stream.try_next().await? {
            writer.send(chunk).await?;
        }
        writer.send_eof()
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &'_ dyn StoreDriver {
        self
    }

    fn as_any<'a>(&'a self) -> &'a (dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_health(self: Arc<Self>, registry: &mut HealthRegistryBuilder) {
        registry.register_indicator(self);
    }

    fn register_remove_callback(
        self: Arc<Self>,
        _callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        // As we're backed by GCS, this store doesn't actually drop stuff
        // so we can actually just ignore this
        Ok(())
    }
}

#[async_trait]
impl<I, Client, NowFn> HealthStatusIndicator for GcsStore<Client, NowFn>
where
    I: InstantWrapper,
    Client: GcsOperations + 'static,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    fn get_name(&self) -> &'static str {
        "GcsStore"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}
