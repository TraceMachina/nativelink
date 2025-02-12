use std::borrow::Cow;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{unfold, FuturesUnordered};
use futures::TryStreamExt;
use nativelink_config::stores::GcsSpec;
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::retry::{Retrier, RetryResult};
use nativelink_util::store_trait::{StoreDriver, StoreKey, UploadSizeInfo};
use tokio::time::sleep;

use crate::cas_utils::is_zero_digest;
use crate::gcs_client::client::{GcsClient, ObjectPath};

const MIN_MULTIPART_SIZE: u64 = 5 * 1024 * 1024; // 5MB
const DEFAULT_CONCURRENT_UPLOADS: usize = 10;
const DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST: usize = 2 * 1024 * 1000; // ~2MB.
const MAX_CHUNK_SIZE: usize = 2 * 1024 * 1000; // ~2MB.

struct ConnectionPool {
    semaphore: Arc<tokio::sync::Semaphore>,
    client: Arc<GcsClient>,
}

impl ConnectionPool {
    fn new(max_connections: usize, client: Arc<GcsClient>) -> Self {
        Self {
            semaphore: Arc::new(tokio::sync::Semaphore::new(max_connections)),
            client,
        }
    }

    async fn acquire(&self) -> Result<PooledConnection, Error> {
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
    client: Arc<GcsClient>,
    _permit: tokio::sync::SemaphorePermit<'a>,
}

#[derive(MetricsComponent)]
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
        let jitter_fn =
            Arc::new(move |delay: Duration| delay.mul_f32(1.0 + rand::random::<f32>() * 0.1));

        let client = GcsClient::new(spec, jitter_fn).await?;
        let client = Arc::new(client);

        let max_connections = spec
            .max_concurrent_uploads
            .unwrap_or(DEFAULT_CONCURRENT_UPLOADS);
        let connection_pool = Arc::new(ConnectionPool::new(max_connections, client));

        Ok(Arc::new(Self {
            connection_pool,
            now_fn,
            bucket: spec.bucket.clone(),
            key_prefix: spec.key_prefix.as_ref().unwrap_or(&String::new()).clone(),
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                Arc::new(move |delay| delay.mul_f32(1.0 + rand::random::<f32>() * 0.1)),
                spec.retry.clone(),
            ),
            consider_expired_after_s: i64::from(spec.consider_expired_after_s),
            max_retry_buffer_size: spec
                .max_retry_buffer_per_request
                .unwrap_or(DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST),
            max_chunk_size: spec
                .resumable_chunk_size
                .unwrap_or(MAX_CHUNK_SIZE)
                .min(MAX_CHUNK_SIZE),
            max_concurrent_uploads: max_connections,
        }))
    }

    async fn has(self: Pin<&Self>, key: &StoreKey<'_>) -> Result<Option<u64>, Error> {
        let object = self.make_object_path(key);

        self.retrier
            .retry(unfold(object, move |object| async move {
                match self.connection_pool.acquire().await {
                    Ok(conn) => match conn.client.read_object_metadata(object.clone()).await {
                        Ok(Some(metadata)) => {
                            // Check if the object should be considered expired
                            if self.consider_expired_after_s != 0 {
                                if let Some(update_time) = metadata.update_time {
                                    let now_s = (self.now_fn)().unix_timestamp() as i64;
                                    // update_time.seconds is the number of seconds since the Unix epoch
                                    if update_time.seconds + self.consider_expired_after_s <= now_s
                                    {
                                        return Some((RetryResult::Ok(None), object));
                                    }
                                }
                            }

                            let size = metadata.size;
                            if size >= 0 {
                                Some((RetryResult::Ok(Some(size as u64)), object))
                            } else {
                                Some((
                                    RetryResult::Err(make_err!(
                                        Code::InvalidArgument,
                                        "Invalid metadata size in GCS: {:?}",
                                        metadata
                                    )),
                                    object,
                                ))
                            }
                        }
                        Ok(None) => Some((RetryResult::Ok(None), object)),
                        Err(e) if e.code == Code::NotFound => Some((RetryResult::Ok(None), object)),
                        Err(e) => Some((RetryResult::Retry(e), object)),
                    },
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

        // For small files, use simple upload
        if max_size < MIN_MULTIPART_SIZE && matches!(upload_size, UploadSizeInfo::ExactSize(_)) {
            let UploadSizeInfo::ExactSize(sz) = upload_size else {
                unreachable!("upload_size must be ExactSize here");
            };

            reader.set_max_recent_data_size(
                u64::try_from(self.max_retry_buffer_size)
                    .err_tip(|| "Could not convert max_retry_buffer_size to u64")?,
            );

            return self
                .retrier
                .retry(unfold(
                    (reader, object.clone()),
                    move |(mut reader, object)| async move {
                        let content = match reader.consume(Some(sz as usize)).await {
                            Ok(content) => content,
                            Err(e) => return Some((RetryResult::Err(e), (reader, object))),
                        };

                        let conn = match self.connection_pool.acquire().await {
                            Ok(conn) => conn,
                            Err(e) => return Some((RetryResult::Retry(e), (reader, object))),
                        };

                        match conn
                            .client
                            .simple_upload(&object, content.to_vec(), sz as i64)
                            .await
                        {
                            Ok(()) => Some((RetryResult::Ok(()), (reader, object))),
                            Err(e) => {
                                if let Err(reset_err) = reader.try_reset_stream() {
                                    return Some((
                                        RetryResult::Err(e.merge(reset_err)),
                                        (reader, object),
                                    ));
                                }
                                Some((RetryResult::Retry(e), (reader, object)))
                            }
                        }
                    },
                ))
                .await;
        }

        // For larger files, we'll use resumable upload
        // Resumable uploads are not optimal, we should find a way to upload smaller files
        // with unknown sizes directly with simple upload itself.
        self.retrier
            .retry(unfold(
                (reader, object.clone()),
                move |(mut reader, object)| async move {
                    // Start resumable upload
                    let conn = match self.connection_pool.acquire().await {
                        Ok(conn) => conn,
                        Err(e) => return Some((RetryResult::Retry(e), (reader, object))),
                    };

                    let upload_id = match conn.client.start_resumable_write(&object).await {
                        Ok(id) => id,
                        Err(e) => {
                            return Some((
                                RetryResult::Retry(make_err!(
                                    Code::Aborted,
                                    "Failed to start resumable upload: {:?}",
                                    e
                                )),
                                (reader, object),
                            ))
                        }
                    };

                    // Perform resumable upload
                    match conn
                        .client
                        .resumable_upload(
                            &object,
                            &mut reader,
                            &upload_id,
                            max_size as i64,
                            self.max_concurrent_uploads,
                        )
                        .await
                    {
                        Ok(()) => Some((RetryResult::Ok(()), (reader, object))),
                        Err(e) => {
                            if let Err(reset_err) = reader.try_reset_stream() {
                                return Some((
                                    RetryResult::Err(e.merge(reset_err)),
                                    (reader, object),
                                ));
                            }
                            Some((RetryResult::Retry(e), (reader, object)))
                        }
                    }
                },
            ))
            .await
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
        let conn = self.connection_pool.acquire().await?;

        self.retrier
            .retry(unfold(
                (writer, conn, object),
                move |(writer, conn, object)| async move {
                    match conn
                        .client
                        .read_object_content(
                            object.clone(),
                            offset as i64,
                            length.map(|len| offset as i64 + len as i64),
                        )
                        .await
                    {
                        Ok(data) => {
                            if !data.is_empty() {
                                if let Err(e) = writer.send(Bytes::from(data)).await {
                                    return Some((
                                        RetryResult::Err(make_err!(
                                            Code::Internal,
                                            "Failed to send data: {}",
                                            e
                                        )),
                                        (writer, conn, object),
                                    ));
                                }
                            }

                            if let Err(e) = writer.send_eof() {
                                return Some((
                                    RetryResult::Err(make_err!(
                                        Code::Internal,
                                        "Failed to send EOF: {}",
                                        e
                                    )),
                                    (writer, conn, object),
                                ));
                            }

                            Some((RetryResult::Ok(()), (writer, conn, object)))
                        }
                        Err(e) if e.code == Code::NotFound => {
                            Some((RetryResult::Err(e), (writer, conn, object)))
                        }
                        Err(e) => Some((RetryResult::Retry(e), (writer, conn, object))),
                    }
                },
            ))
            .await?;

        Ok(())
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
