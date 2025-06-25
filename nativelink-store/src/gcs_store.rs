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

use core::fmt::Debug;
use core::pin::Pin;
use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use futures::stream::{FuturesUnordered, unfold};
use nativelink_config::stores::ExperimentalGcsSpec;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::retry::{Retrier, RetryResult};
use nativelink_util::store_trait::{StoreDriver, StoreKey, UploadSizeInfo};
use tokio::time::sleep;

use crate::cas_utils::is_zero_digest;
use crate::gcs_client::client::{GcsClient, GcsOperations};
use crate::gcs_client::types::{
    CHUNK_SIZE, DEFAULT_CONCURRENT_UPLOADS, DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST,
    MIN_MULTIPART_SIZE, ObjectPath,
};

#[derive(MetricsComponent, Debug)]
pub struct GcsStore<NowFn> {
    client: Arc<dyn GcsOperations>,
    now_fn: NowFn,
    #[metric(help = "The bucket name for the GCS store")]
    bucket: String,
    #[metric(help = "The key prefix for the GCS store")]
    key_prefix: String,
    retrier: Retrier,
    #[metric(help = "The number of seconds to consider an object expired")]
    consider_expired_after_s: i64,
    #[metric(help = "The number of bytes to buffer for retrying requests")]
    max_retry_buffer_size: usize,
    #[metric(help = "The size of chunks for resumable uploads")]
    max_chunk_size: usize,
    #[metric(help = "The number of concurrent uploads allowed")]
    max_concurrent_uploads: usize,
}

impl<I, NowFn> GcsStore<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    pub async fn new(spec: &ExperimentalGcsSpec, now_fn: NowFn) -> Result<Arc<Self>, Error> {
        let client = GcsClient::new(spec).await?;
        let client: Arc<dyn GcsOperations> = Arc::new(client);

        Self::new_with_ops(spec, client, now_fn)
    }

    // Primarily used for injecting a mock or real operations implementation
    pub fn new_with_ops(
        spec: &ExperimentalGcsSpec,
        client: Arc<dyn GcsOperations>,
        now_fn: NowFn,
    ) -> Result<Arc<Self>, Error> {
        let max_connections = spec
            .common
            .multipart_max_concurrent_uploads
            .unwrap_or(DEFAULT_CONCURRENT_UPLOADS);

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
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                spec.common.retry.make_jitter_fn(),
                spec.common.retry.clone(),
            ),
            consider_expired_after_s: i64::from(spec.common.consider_expired_after_s),
            max_retry_buffer_size: spec
                .common
                .max_retry_buffer_per_request
                .unwrap_or(DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST),
            max_chunk_size: core::cmp::min(
                spec.resumable_chunk_size.unwrap_or(CHUNK_SIZE),
                CHUNK_SIZE,
            ),
            max_concurrent_uploads: max_connections,
        }))
    }

    async fn has(self: Pin<&Self>, key: &StoreKey<'_>) -> Result<Option<u64>, Error> {
        let object_path = self.make_object_path(key);
        let client = &self.client;
        let consider_expired_after_s = self.consider_expired_after_s;
        let now_fn = &self.now_fn;

        self.retrier
            .retry(unfold(object_path, move |object_path| async move {
                match client.read_object_metadata(&object_path).await {
                    Ok(Some(metadata)) => {
                        if consider_expired_after_s != 0 {
                            if let Some(update_time) = &metadata.update_time {
                                let now_s = now_fn().unix_timestamp() as i64;
                                if update_time.seconds + consider_expired_after_s <= now_s {
                                    return Some((RetryResult::Ok(None), object_path));
                                }
                            }
                        }

                        if metadata.size >= 0 {
                            Some((RetryResult::Ok(Some(metadata.size as u64)), object_path))
                        } else {
                            Some((
                                RetryResult::Err(make_err!(
                                    Code::InvalidArgument,
                                    "Invalid metadata size in GCS: {}",
                                    metadata.size
                                )),
                                object_path,
                            ))
                        }
                    }
                    Ok(None) => Some((RetryResult::Ok(None), object_path)),
                    Err(e) if e.code == Code::NotFound => {
                        Some((RetryResult::Ok(None), object_path))
                    }
                    Err(e) => Some((RetryResult::Retry(e), object_path)),
                }
            }))
            .await
    }

    fn make_object_path(&self, key: &StoreKey) -> ObjectPath {
        ObjectPath::new(
            self.bucket.clone(),
            &format!("{}{}", self.key_prefix, key.as_str()),
        )
    }
}

#[async_trait]
impl<I, NowFn> StoreDriver for GcsStore<NowFn>
where
    I: InstantWrapper,
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
        upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        let object_path = self.make_object_path(&digest);
        let max_size = match upload_size {
            UploadSizeInfo::ExactSize(sz) | UploadSizeInfo::MaxSize(sz) => sz,
        };

        reader.set_max_recent_data_size(
            u64::try_from(self.max_retry_buffer_size)
                .err_tip(|| "Could not convert max_retry_buffer_size to u64")?,
        );

        // For small files with exact size, we'll use simple upload
        if max_size < MIN_MULTIPART_SIZE && matches!(upload_size, UploadSizeInfo::ExactSize(_)) {
            let content = reader.consume(Some(max_size as usize)).await?;
            let client = &self.client;
            let object_path_cloned = object_path.clone();

            return self
                .retrier
                .retry(unfold(content, move |content| {
                    let object_path_cloned = object_path_cloned.clone();
                    async move {
                        match client
                            .write_object(&object_path_cloned, content.to_vec())
                            .await
                        {
                            Ok(()) => Some((RetryResult::Ok(()), content)),
                            Err(e) => Some((RetryResult::Retry(e), content)),
                        }
                    }
                }))
                .await;
        }

        // For larger files, we'll use resumable upload
        // First, we'll initiate the upload session
        let client = &self.client;
        let object_path_for_start = object_path.clone();
        let upload_id = self
            .retrier
            .retry(unfold((), move |()| {
                let object_path = object_path_for_start.clone();
                async move {
                    match client.start_resumable_write(&object_path).await {
                        Ok(id) => Some((RetryResult::Ok(id), ())),
                        Err(e) => Some((
                            RetryResult::Retry(make_err!(
                                Code::Aborted,
                                "Failed to start resumable upload: {:?}",
                                e
                            )),
                            (),
                        )),
                    }
                }
            }))
            .await?;

        // Stream and upload data in chunks
        let chunk_size = core::cmp::min(self.max_chunk_size, max_size as usize);
        let mut offset = 0u64;
        let mut total_uploaded = 0u64;

        let upload_id = upload_id.clone();
        let object_path_for_chunks = object_path.clone();

        loop {
            let to_read = core::cmp::min(chunk_size, (max_size - total_uploaded) as usize);
            if to_read == 0 {
                break;
            }

            let chunk = reader.consume(Some(to_read)).await?;
            if chunk.is_empty() {
                break;
            }

            let chunk_size = chunk.len() as u64;
            total_uploaded += chunk_size;
            let is_final = total_uploaded >= max_size || chunk.len() < to_read;
            let current_offset = offset;
            let object_path = object_path_for_chunks.clone();
            let upload_id_clone = upload_id.clone();

            // Uploading the chunk with a retry
            self.retrier
                .retry(unfold(chunk, move |chunk| {
                    let object_path = object_path.clone();
                    let upload_id_clone = upload_id_clone.clone();
                    async move {
                        match client
                            .upload_chunk(
                                &upload_id_clone,
                                &object_path,
                                chunk.to_vec(),
                                current_offset as i64,
                                (current_offset + chunk.len() as u64) as i64,
                                is_final,
                            )
                            .await
                        {
                            Ok(()) => Some((RetryResult::Ok(()), chunk)),
                            Err(e) => Some((RetryResult::Retry(e), chunk)),
                        }
                    }
                }))
                .await?;

            offset += chunk_size;

            if is_final {
                break;
            }
        }

        // Handle the edge case: empty file (nothing uploaded)
        if offset == 0 {
            let object_path = object_path.clone();
            let upload_id_clone = upload_id.clone();

            self.retrier
                .retry(unfold((), move |()| {
                    let object_path = object_path.clone();
                    let upload_id_clone = upload_id_clone.clone();
                    async move {
                        match client
                            .upload_chunk(&upload_id_clone, &object_path, Vec::new(), 0, 0, true)
                            .await
                        {
                            Ok(()) => Some((RetryResult::Ok(()), ())),
                            Err(e) => Some((RetryResult::Retry(e), ())),
                        }
                    }
                }))
                .await?;
        }

        // Verifying if the upload was successful
        let object_path = object_path.clone();

        self.retrier
            .retry(unfold((), move |()| {
                let object_path = object_path.clone();
                async move {
                    match client.object_exists(&object_path).await {
                        Ok(true) => Some((RetryResult::Ok(()), ())),
                        Ok(false) => Some((
                            RetryResult::Retry(make_err!(
                                Code::Internal,
                                "Object not found after upload completion"
                            )),
                            (),
                        )),
                        Err(e) => Some((RetryResult::Retry(e), ())),
                    }
                }
            }))
            .await?;

        Ok(())
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
        let client = &self.client;

        let result = self
            .retrier
            .retry(unfold(
                (offset, end_offset, object_path.clone()),
                move |(start_offset, end_offset, object_path)| async move {
                    match client
                        .read_object_content(
                            &object_path,
                            start_offset as i64,
                            end_offset.map(|e| e as i64),
                        )
                        .await
                    {
                        Ok(data) => Some((
                            RetryResult::Ok(data),
                            (start_offset, end_offset, object_path),
                        )),
                        Err(e) if e.code == Code::NotFound => {
                            Some((RetryResult::Err(e), (start_offset, end_offset, object_path)))
                        }
                        Err(e) => Some((
                            RetryResult::Retry(e),
                            (start_offset, end_offset, object_path),
                        )),
                    }
                },
            ))
            .await;

        match result {
            Ok(data) => {
                if !data.is_empty() {
                    writer.send(Bytes::from(data)).await?;
                }
                writer.send_eof()?;
                Ok(())
            }
            Err(e) => {
                drop(writer.send_eof());
                Err(e)
            }
        }
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
}

#[async_trait]
impl<I, NowFn> HealthStatusIndicator for GcsStore<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    fn get_name(&self) -> &'static str {
        "GcsStore"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}
