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
use std::fmt::Debug;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::TryStreamExt;
use futures::stream::{FuturesUnordered, unfold};
use nativelink_config::stores::GcsSpec;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::retry::{Retrier, RetryResult};
use nativelink_util::store_trait::{StoreDriver, StoreKey, UploadSizeInfo};
use rand::Rng;
use tokio::time::sleep;

use crate::cas_utils::is_zero_digest;
use crate::gcs_client::client::GcsClient;
use crate::gcs_client::operations::GcsOperations;
use crate::gcs_client::types::{
    CHUNK_SIZE, DEFAULT_CONCURRENT_UPLOADS, DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST,
    MIN_MULTIPART_SIZE, ObjectPath,
};

struct ConnectionPool {
    semaphore: Arc<tokio::sync::Semaphore>,
    client: Arc<dyn GcsOperations>,
}

impl Debug for ConnectionPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConnectionPool")
            .field("semaphore", &self.semaphore)
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

impl ConnectionPool {
    fn new(max_connections: usize, client: Arc<dyn GcsOperations>) -> Self {
        Self {
            semaphore: Arc::new(tokio::sync::Semaphore::new(max_connections)),
            client,
        }
    }

    async fn acquire(&self) -> Result<PooledConnection<'_>, Error> {
        let permit =
            self.semaphore.acquire().await.map_err(|e| {
                make_err!(Code::Internal, "Failed to acquire connection permit: {}", e)
            })?;
        Ok(PooledConnection {
            client: self.client.clone(),
            _permit: permit,
        })
    }
}

struct PooledConnection<'a> {
    client: Arc<dyn GcsOperations>,
    _permit: tokio::sync::SemaphorePermit<'a>,
}

#[derive(MetricsComponent, Debug)]
pub struct GcsStore<NowFn> {
    connection_pool: Arc<ConnectionPool>,
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
    pub async fn new(spec: &GcsSpec, now_fn: NowFn) -> Result<Arc<Self>, Error> {
        let client = GcsClient::new(spec).await?;
        let client: Arc<dyn GcsOperations> = Arc::new(client);

        Self::new_with_ops(spec, client, now_fn)
    }

    // Primarily used for injecting a mock or real operations implementation
    pub fn new_with_ops(
        spec: &GcsSpec,
        ops: Arc<dyn GcsOperations>,
        now_fn: NowFn,
    ) -> Result<Arc<Self>, Error> {
        let max_connections = spec
            .max_concurrent_uploads
            .unwrap_or(DEFAULT_CONCURRENT_UPLOADS);
        let connection_pool = Arc::new(ConnectionPool::new(max_connections, ops));

        let jitter_amt = spec.retry.jitter;
        let jitter_fn = Arc::new(move |delay: tokio::time::Duration| {
            if jitter_amt == 0.0 {
                return delay;
            }
            let min = 1.0 - (jitter_amt / 2.0);
            let max = 1.0 + (jitter_amt / 2.0);
            let mut rng = rand::rng();
            let factor = min + (max - min) * rng.random::<f32>();
            delay.mul_f32(factor)
        });

        Ok(Arc::new(Self {
            connection_pool,
            now_fn,
            bucket: spec.bucket.clone(),
            key_prefix: spec.key_prefix.as_ref().unwrap_or(&String::new()).clone(),
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                jitter_fn,
                spec.retry.clone(),
            ),
            consider_expired_after_s: i64::from(spec.consider_expired_after_s),
            max_retry_buffer_size: spec
                .max_retry_buffer_per_request
                .unwrap_or(DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST),
            max_chunk_size: spec
                .resumable_chunk_size
                .unwrap_or(CHUNK_SIZE)
                .min(CHUNK_SIZE),
            max_concurrent_uploads: max_connections,
        }))
    }

    async fn has(self: Pin<&Self>, key: &StoreKey<'_>) -> Result<Option<u64>, Error> {
        let object = self.make_object_path(key);

        self.retrier
            .retry(unfold(object.clone(), move |object| async move {
                let conn = match self.connection_pool.acquire().await {
                    Ok(conn) => conn,
                    Err(e) => return Some((RetryResult::Retry(e), object.clone())),
                };

                match conn.client.read_object_metadata(object.clone()).await {
                    Ok(Some(metadata)) => {
                        if self.consider_expired_after_s != 0 {
                            if let Some(update_time) = &metadata.update_time {
                                let now_s = (self.now_fn)().unix_timestamp() as i64;
                                if update_time.seconds + self.consider_expired_after_s <= now_s {
                                    return Some((RetryResult::Ok(None), object));
                                }
                            }
                        }

                        if metadata.size >= 0 {
                            Some((RetryResult::Ok(Some(metadata.size as u64)), object))
                        } else {
                            Some((
                                RetryResult::Err(make_err!(
                                    Code::InvalidArgument,
                                    "Invalid metadata size in GCS: {}",
                                    metadata.size
                                )),
                                object,
                            ))
                        }
                    }
                    Ok(None) => Some((RetryResult::Ok(None), object)),
                    Err(e) if e.code == Code::NotFound => Some((RetryResult::Ok(None), object)),
                    Err(e) => Some((RetryResult::Retry(e), object)),
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
        let object = self.make_object_path(&digest);
        let max_size = match upload_size {
            UploadSizeInfo::ExactSize(sz) | UploadSizeInfo::MaxSize(sz) => sz,
        };

        reader.set_max_recent_data_size(
            u64::try_from(self.max_retry_buffer_size)
                .err_tip(|| "Could not convert max_retry_buffer_size to u64")?,
        );

        // For small files with exact size, use simple upload
        if max_size < MIN_MULTIPART_SIZE && matches!(upload_size, UploadSizeInfo::ExactSize(_)) {
            let content = reader.consume(Some(max_size as usize)).await?;

            return self
                .retrier
                .retry(unfold(
                    (content.clone(), object.clone()),
                    move |(content, object)| async move {
                        let conn = match self.connection_pool.acquire().await {
                            Ok(conn) => conn,
                            Err(e) => {
                                return Some((
                                    RetryResult::Retry(e),
                                    (content.clone(), object.clone()),
                                ));
                            }
                        };

                        match conn.client.write_object(&object, content.to_vec()).await {
                            Ok(()) => Some((RetryResult::Ok(()), (content, object))),
                            Err(e) => {
                                Some((RetryResult::Retry(e), (content.clone(), object.clone())))
                            }
                        }
                    },
                ))
                .await;
        }

        // For larger files, use resumable upload
        // First, initiate the upload session
        let upload_id = self
            .retrier
            .retry(unfold(object.clone(), move |object| async move {
                let conn = match self.connection_pool.acquire().await {
                    Ok(conn) => conn,
                    Err(e) => return Some((RetryResult::Retry(e), object.clone())),
                };

                match conn.client.start_resumable_write(&object).await {
                    Ok(id) => Some((RetryResult::Ok(id), object)),
                    Err(e) => Some((
                        RetryResult::Retry(make_err!(
                            Code::Aborted,
                            "Failed to start resumable upload: {:?}",
                            e
                        )),
                        object.clone(),
                    )),
                }
            }))
            .await?;

        // Stream and upload data in chunks
        let chunk_size = std::cmp::min(self.max_chunk_size, max_size as usize);
        let mut offset = 0u64;
        let mut total_uploaded = 0u64;

        loop {
            let to_read = std::cmp::min(chunk_size, (max_size - total_uploaded) as usize);
            if to_read == 0 {
                break;
            }

            let chunk = reader.consume(Some(to_read)).await?;
            if chunk.is_empty() {
                break;
            }

            let chunk_size_u64 = chunk.len() as u64;
            total_uploaded += chunk_size_u64;
            let is_final = total_uploaded >= max_size || chunk.len() < to_read;

            // Upload the chunk with retry
            self.retrier
                .retry(unfold(
                    (
                        chunk.clone(),
                        object.clone(),
                        upload_id.clone(),
                        offset,
                        is_final,
                    ),
                    move |(chunk, obj, url, offs, final_chunk)| async move {
                        let conn = match self.connection_pool.acquire().await {
                            Ok(conn) => conn,
                            Err(e) => {
                                return Some((
                                    RetryResult::Retry(e),
                                    (chunk.clone(), obj.clone(), url.clone(), offs, final_chunk),
                                ));
                            }
                        };

                        match conn
                            .client
                            .upload_chunk(
                                &url,
                                &obj,
                                chunk.to_vec(),
                                offs as i64,
                                (offs + chunk.len() as u64) as i64,
                                final_chunk,
                            )
                            .await
                        {
                            Ok(()) => Some((
                                RetryResult::Ok(()),
                                (chunk, obj.clone(), url.clone(), offs, final_chunk),
                            )),
                            Err(e) => Some((
                                RetryResult::Retry(e),
                                (chunk.clone(), obj.clone(), url.clone(), offs, final_chunk),
                            )),
                        }
                    },
                ))
                .await?;

            offset += chunk_size_u64;

            if is_final {
                break;
            }
        }

        // Handle edge case: empty file (nothing uploaded)
        if offset == 0 {
            self.retrier
                .retry(unfold(
                    (object.clone(), upload_id.clone()),
                    move |(obj, url)| async move {
                        let conn = match self.connection_pool.acquire().await {
                            Ok(conn) => conn,
                            Err(e) => {
                                return Some((RetryResult::Retry(e), (obj.clone(), url.clone())));
                            }
                        };

                        match conn
                            .client
                            .upload_chunk(&url, &obj, Vec::new(), 0, 0, true)
                            .await
                        {
                            Ok(()) => Some((RetryResult::Ok(()), (obj.clone(), url.clone()))),
                            Err(e) => Some((RetryResult::Retry(e), (obj.clone(), url.clone()))),
                        }
                    },
                ))
                .await?;
        }

        // Verify the upload was successful
        self.retrier
            .retry(unfold(object.clone(), move |obj| async move {
                let conn = match self.connection_pool.acquire().await {
                    Ok(conn) => conn,
                    Err(e) => return Some((RetryResult::Retry(e), obj.clone())),
                };

                match conn.client.object_exists(&obj).await {
                    Ok(true) => Some((RetryResult::Ok(()), obj)),
                    Ok(false) => Some((
                        RetryResult::Retry(make_err!(
                            Code::Internal,
                            "Object not found after upload completion"
                        )),
                        obj,
                    )),
                    Err(e) => Some((RetryResult::Retry(e), obj)),
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

        let object = self.make_object_path(&key);
        let end_offset = length.map(|len| offset + len);

        let result = self
            .retrier
            .retry(unfold(
                (object.clone(), offset, end_offset),
                move |(object, start_offset, end_offset)| {
                    let connection_pool = self.connection_pool.clone();
                    async move {
                        let conn = match connection_pool.acquire().await {
                            Ok(conn) => conn,
                            Err(e) => {
                                return Some((
                                    RetryResult::Retry(e),
                                    (object.clone(), start_offset, end_offset),
                                ));
                            }
                        };

                        match conn
                            .client
                            .read_object_content(
                                object.clone(),
                                start_offset as i64,
                                end_offset.map(|e| e as i64),
                            )
                            .await
                        {
                            Ok(data) => Some((
                                RetryResult::Ok(data),
                                (object.clone(), start_offset, end_offset),
                            )),
                            Err(e) if e.code == Code::NotFound => Some((
                                RetryResult::Err(e),
                                (object.clone(), start_offset, end_offset),
                            )),
                            Err(e) => Some((
                                RetryResult::Retry(e),
                                (object.clone(), start_offset, end_offset),
                            )),
                        }
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

    #[inline]
    fn inner_store(&self, _digest: Option<StoreKey>) -> &'_ dyn StoreDriver {
        self
    }

    #[inline]
    fn as_any<'a>(&'a self) -> &'a (dyn std::any::Any + Sync + Send + 'static) {
        self
    }

    #[inline]
    fn as_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Sync + Send + 'static> {
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
