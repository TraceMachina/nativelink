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
use bytes::Bytes;
use futures::stream::{FuturesUnordered, unfold};
use futures::{StreamExt, TryStreamExt};
use nativelink_config::stores::ExperimentalGcsSpec;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::retry::{Retrier, RetryResult};
use nativelink_util::store_trait::{
    RemoveItemCallback, StoreDriver, StoreKey, StoreOptimizations, UploadSizeInfo,
};
use rand::Rng;
use tokio::time::sleep;

use crate::cas_utils::is_zero_digest;
use crate::gcs_client::client::{GcsClient, GcsOperations};
use crate::gcs_client::types::{
    CHUNK_SIZE, DEFAULT_CONCURRENT_UPLOADS, DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST,
    MIN_MULTIPART_SIZE, ObjectPath,
};

#[derive(MetricsComponent, Debug)]
pub struct GcsStore<Client: GcsOperations, NowFn> {
    client: Arc<Client>,
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
        // Chunks must be a multiple of 256kb according to the documentation.
        const CHUNK_MULTIPLE: usize = 256 * 1024;

        let max_connections = spec
            .common
            .multipart_max_concurrent_uploads
            .unwrap_or(DEFAULT_CONCURRENT_UPLOADS);

        let jitter_amt = spec.common.retry.jitter;
        let jitter_fn = Arc::new(move |delay: tokio::time::Duration| {
            if jitter_amt == 0.0 {
                return delay;
            }
            delay.mul_f32(jitter_amt.mul_add(rand::rng().random::<f32>() - 0.5, 1.))
        });

        let max_chunk_size =
            core::cmp::min(spec.resumable_chunk_size.unwrap_or(CHUNK_SIZE), CHUNK_SIZE);

        let max_chunk_size = if max_chunk_size.is_multiple_of(CHUNK_MULTIPLE) {
            max_chunk_size
        } else {
            ((max_chunk_size + CHUNK_MULTIPLE / 2) / CHUNK_MULTIPLE) * CHUNK_MULTIPLE
        };

        let max_retry_buffer_size = spec
            .common
            .max_retry_buffer_per_request
            .unwrap_or(DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST);

        // The retry buffer should be at least as big as the chunk size.
        let max_retry_buffer_size = if max_retry_buffer_size < max_chunk_size {
            max_chunk_size
        } else {
            max_retry_buffer_size
        };

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
                jitter_fn,
                spec.common.retry.clone(),
            ),
            consider_expired_after_s: i64::from(spec.common.consider_expired_after_s),
            max_retry_buffer_size,
            max_chunk_size,
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

    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        matches!(optimization, StoreOptimizations::LazyExistenceOnSync)
    }

    async fn update(
        self: Pin<&Self>,
        digest: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
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

        reader.set_max_recent_data_size(
            u64::try_from(self.max_retry_buffer_size)
                .err_tip(|| "Could not convert max_retry_buffer_size to u64")?,
        );

        // For small files with exact size, we'll use simple upload
        if let UploadSizeInfo::ExactSize(size) = upload_size {
            if size < MIN_MULTIPART_SIZE {
                let content = reader.consume(Some(usize::try_from(size)?)).await?;
                let client = &self.client;

                return self
                    .retrier
                    .retry(unfold(content, |content| async {
                        match client.write_object(&object_path, content.to_vec()).await {
                            Ok(()) => Some((RetryResult::Ok(()), content)),
                            Err(e) => Some((RetryResult::Retry(e), content)),
                        }
                    }))
                    .await;
            }
        }

        // For larger files, we'll use resumable upload
        // Stream and upload data in chunks
        let mut offset = 0u64;
        let mut total_size = if let UploadSizeInfo::ExactSize(size) = upload_size {
            Some(size)
        } else {
            None
        };
        let mut upload_id: Option<String> = None;
        let client = &self.client;

        loop {
            let chunk = reader.consume(Some(self.max_chunk_size)).await?;
            if chunk.is_empty() {
                break;
            }
            // If a full chunk wasn't read, then this is the full length.
            if chunk.len() < self.max_chunk_size {
                total_size = Some(offset + chunk.len() as u64);
            }

            let upload_id_ref = if let Some(upload_id_ref) = &upload_id {
                upload_id_ref
            } else {
                // Initiate the upload session on the first non-empty chunk.
                upload_id = Some(
                    self.retrier
                        .retry(unfold((), |()| async {
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
                        }))
                        .await?,
                );
                upload_id.as_deref().unwrap()
            };

            let current_offset = offset;
            offset += chunk.len() as u64;

            // Uploading the chunk with a retry
            let object_path_ref = &object_path;
            self.retrier
                .retry(unfold(chunk, |chunk| async move {
                    match client
                        .upload_chunk(
                            upload_id_ref,
                            object_path_ref,
                            chunk.clone(),
                            current_offset,
                            offset,
                            total_size,
                        )
                        .await
                    {
                        Ok(()) => Some((RetryResult::Ok(()), chunk)),
                        Err(e) => Some((RetryResult::Retry(e), chunk)),
                    }
                }))
                .await?;
        }

        // Handle the case that the stream was of unknown length and
        // happened to be an exact multiple of chunk size.
        if let Some(upload_id_ref) = &upload_id {
            if total_size.is_none() {
                let object_path_ref = &object_path;
                self.retrier
                    .retry(unfold((), |()| async move {
                        match client
                            .upload_chunk(
                                upload_id_ref,
                                object_path_ref,
                                Bytes::new(),
                                offset,
                                offset,
                                Some(offset),
                            )
                            .await
                        {
                            Ok(()) => Some((RetryResult::Ok(()), ())),
                            Err(e) => Some((RetryResult::Retry(e), ())),
                        }
                    }))
                    .await?;
            }
        } else {
            // Handle streamed empty file.
            return self
                .retrier
                .retry(unfold((), |()| async {
                    match client.write_object(&object_path, Vec::new()).await {
                        Ok(()) => Some((RetryResult::Ok(()), ())),
                        Err(e) => Some((RetryResult::Retry(e), ())),
                    }
                }))
                .await;
        }

        // Verifying if the upload was successful
        self.retrier
            .retry(unfold((), |()| async {
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

        let object_path_ref = &object_path;
        self.retrier
            .retry(unfold(
                (offset, writer),
                |(mut offset, writer)| async move {
                    let mut stream = match client
                        .read_object_content(object_path_ref, offset, end_offset)
                        .await
                    {
                        Ok(stream) => stream,
                        Err(e) if e.code == Code::NotFound => {
                            return Some((RetryResult::Err(e), (offset, writer)));
                        }
                        Err(e) => return Some((RetryResult::Retry(e), (offset, writer))),
                    };

                    while let Some(next_chunk) = stream.next().await {
                        match next_chunk {
                            Ok(bytes) => {
                                offset += bytes.len() as u64;
                                if let Err(err) = writer.send(bytes).await {
                                    return Some((RetryResult::Err(err), (offset, writer)));
                                }
                            }
                            Err(err) => return Some((RetryResult::Retry(err), (offset, writer))),
                        }
                    }

                    if let Err(err) = writer.send_eof() {
                        return Some((RetryResult::Err(err), (offset, writer)));
                    }

                    Some((RetryResult::Ok(()), (offset, writer)))
                },
            ))
            .await
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
