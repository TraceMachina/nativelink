use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::unfold;
use futures::{Stream, StreamExt, TryStreamExt};
use googleapis_tonic_google_storage_v2::google::storage::v2::storage_client::StorageClient;
use googleapis_tonic_google_storage_v2::google::storage::v2::{
    bidi_write_object_request, write_object_request, BidiWriteObjectRequest,
    BidiWriteObjectResponse, ChecksummedData, Object, ReadObjectRequest, ReadObjectResponse,
    StartResumableWriteRequest, StartResumableWriteResponse, WriteObjectRequest,
    WriteObjectResponse, WriteObjectSpec,
};
use nativelink_config::stores::GcsSpec;
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{
    make_buf_channel_pair, DropCloserReadHalf, DropCloserWriteHalf,
};
use nativelink_util::health_utils::{HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::retry::{Retrier, RetryResult};
use nativelink_util::store_trait::{StoreDriver, StoreKey, UploadSizeInfo};
use rand::rngs::OsRng;
use rand::Rng;
use tokio::sync::RwLock;
use tokio::time::{sleep, Instant};
use tonic::metadata::{MetadataMap, MetadataValue};
use tonic::transport::Channel;
use tonic::{Request, Response, Status, Streaming};
use tracing::{error, info};

use crate::cas_utils::is_zero_digest;

// Constants for GCS operations
// Unlike what is specified in the docs, there is a slight discrepancy between
// the chunk and multipart sizes limit. The chunk size is recommended to be 8MB
// but the gRPC connection can't handle the size greater than 4MB.
// Also, we are keeping it slightly below the 4MB limit to make sure we don't
// exceed the limit when some metadata is added to the request. It was observed
// that an additional 15 bytes were added to the request size.
const MIN_MULTIPART_SIZE: u64 = 4 * 1024 * 1000; // < 4MB
const MAX_CHUNK_SIZE: usize = 4 * 1024 * 1000; // < 4 MiB
const DEFAULT_MAX_CONCURRENT_UPLOADS: usize = 10;
const DEFAULT_MAX_RETRY_BUFFER_SIZE: usize = 4 * 1024 * 1000; // < MiB

#[derive(Clone)]
pub struct ObjectPath {
    bucket: String,
    path: String,
}

impl ObjectPath {
    fn new(bucket: String, path: &str) -> Self {
        let normalized_path = path.replace('\\', "/").trim_start_matches('/').to_string();
        Self {
            bucket,
            path: normalized_path,
        }
    }

    fn get_formatted_bucket(&self) -> String {
        format!("projects/_/buckets/{}", self.bucket)
    }
}

pub struct GcsAuth {
    token: RwLock<(String, Instant)>,
}

impl GcsAuth {
    async fn new() -> Result<Self, Error> {
        let token = Self::fetch_token().await?;
        Ok(Self {
            token: RwLock::new((token, Instant::now() + Duration::from_secs(3600))),
        })
    }

    async fn fetch_token() -> Result<String, Error> {
        std::env::var("GOOGLE_AUTH_TOKEN").map_err(|_| {
            make_err!(
                Code::Unauthenticated,
                "GOOGLE_AUTH_TOKEN environment variable not found"
            )
        })
    }

    async fn get_valid_token(&self) -> Result<String, Error> {
        let token_guard = self.token.read().await;
        if Instant::now() < token_guard.1 {
            return Ok(token_guard.0.clone());
        }
        drop(token_guard);

        let mut token_guard = self.token.write().await;
        if Instant::now() >= token_guard.1 {
            token_guard.0 = Self::fetch_token().await?;
            token_guard.1 = Instant::now() + Duration::from_secs(3600);
        }
        Ok(token_guard.0.clone())
    }
}

// Define concrete stream types
pub type WriteObjectStream = Pin<Box<dyn Stream<Item = WriteObjectRequest> + Send + 'static>>;
pub type BidiWriteObjectStream =
    Pin<Box<dyn Stream<Item = BidiWriteObjectRequest> + Send + 'static>>;

struct WriteObjectFuture<'a> {
    client: &'a mut StorageClient<Channel>,
    request: Option<Request<WriteObjectStream>>,
}

impl Future for WriteObjectFuture<'_> {
    type Output = Result<Response<WriteObjectResponse>, Status>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Safety: we're not moving any pinned fields
        let this = unsafe { self.get_unchecked_mut() };

        if let Some(request) = this.request.take() {
            let fut = StorageClient::write_object(this.client, request);
            futures::pin_mut!(fut);
            fut.poll(cx)
        } else {
            Poll::Ready(Err(Status::internal("Request already taken")))
        }
    }
}

struct BidiWriteObjectFuture<'a> {
    client: &'a mut StorageClient<Channel>,
    request: Option<Request<BidiWriteObjectStream>>,
}

impl Future for BidiWriteObjectFuture<'_> {
    type Output = Result<Response<Streaming<BidiWriteObjectResponse>>, Status>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        // Safety: we're not moving any pinned fields
        let this = unsafe { self.get_unchecked_mut() };

        if let Some(request) = this.request.take() {
            let fut = StorageClient::bidi_write_object(this.client, request);
            futures::pin_mut!(fut);
            fut.poll(cx)
        } else {
            Poll::Ready(Err(Status::internal("Request already taken")))
        }
    }
}

// First, let's modify the trait definition to be explicit about lifetimes
#[async_trait::async_trait]
pub trait StorageOperations: Send + Sync + 'static {
    async fn read_object(
        &mut self,
        request: Request<ReadObjectRequest>,
    ) -> Result<Response<Streaming<ReadObjectResponse>>, Status>;

    async fn write_object<'a>(
        &'a mut self,
        request: Request<WriteObjectStream>,
    ) -> Result<Response<WriteObjectResponse>, Status>;

    async fn start_resumable_write(
        &mut self,
        request: Request<StartResumableWriteRequest>,
    ) -> Result<Response<StartResumableWriteResponse>, Status>;

    async fn bidi_write_object<'a>(
        &'a mut self,
        request: Request<BidiWriteObjectStream>,
    ) -> Result<Response<Streaming<BidiWriteObjectResponse>>, Status>;
}

// Then implement it with the same lifetime bounds
#[async_trait::async_trait]
impl StorageOperations for StorageClient<Channel> {
    async fn read_object(
        &mut self,
        request: Request<ReadObjectRequest>,
    ) -> Result<Response<Streaming<ReadObjectResponse>>, Status> {
        StorageClient::read_object(self, request).await
    }

    async fn write_object(
        &mut self,
        request: Request<WriteObjectStream>,
    ) -> Result<Response<WriteObjectResponse>, Status> {
        WriteObjectFuture {
            client: self,
            request: Some(request),
        }
        .await
    }

    async fn start_resumable_write(
        &mut self,
        request: Request<StartResumableWriteRequest>,
    ) -> Result<Response<StartResumableWriteResponse>, Status> {
        StorageClient::start_resumable_write(self, request).await
    }

    async fn bidi_write_object(
        &mut self,
        request: Request<BidiWriteObjectStream>,
    ) -> Result<Response<Streaming<BidiWriteObjectResponse>>, Status> {
        BidiWriteObjectFuture {
            client: self,
            request: Some(request),
        }
        .await
    }
}

/// Wrapper for GCS API client operations
#[derive(Clone)]
pub struct GcsClientWrapper {
    inner: Arc<tokio::sync::Mutex<dyn StorageOperations>>,
}

impl GcsClientWrapper {
    fn new<T: StorageOperations + 'static>(client: T) -> Self {
        Self {
            inner: Arc::new(tokio::sync::Mutex::new(client)),
        }
    }

    // Modify handle_request to work with the mutex
    async fn handle_request<T>(
        &mut self,
        operation: impl FnOnce(
            &mut dyn StorageOperations,
        )
            -> Pin<Box<dyn Future<Output = Result<Response<T>, Status>> + Send + '_>>,
    ) -> Result<Response<T>, Error> {
        let mut guard = self.inner.lock().await;
        operation(&mut *guard).await.map_err(|e| match e.code() {
            tonic::Code::NotFound => make_err!(Code::NotFound, "Resource not found: {}", e),
            _ => make_err!(Code::Unavailable, "Operation failed: {}", e),
        })
    }
}

/// Client for Google Cloud Storage operations
#[derive(Clone)]
pub struct GcsClient {
    pub client: Arc<RwLock<GcsClientWrapper>>,
    pub auth: Arc<GcsAuth>,
    pub retrier: Arc<Retrier>,
}

impl GcsClient {
    pub async fn new_with_client(
        client: impl StorageOperations + 'static,
        spec: &GcsSpec,
        jitter_fn: Arc<dyn Fn(Duration) -> Duration + Send + Sync>,
    ) -> Result<Self, Error> {
        Ok(Self {
            client: Arc::new(RwLock::new(GcsClientWrapper::new(client))),
            auth: Arc::new(GcsAuth::new().await?),
            retrier: Arc::new(Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                jitter_fn,
                spec.retry.clone(),
            )),
        })
    }

    async fn new(
        spec: &GcsSpec,
        jitter_fn: Arc<dyn Fn(Duration) -> Duration + Send + Sync>,
    ) -> Result<Self, Error> {
        let endpoint = std::env::var("GOOGLE_STORAGE_ENDPOINT")
            .unwrap_or_else(|_| "https://storage.googleapis.com".to_string());

        let channel = Channel::from_shared(endpoint)
            .map_err(|e| make_err!(Code::InvalidArgument, "Invalid GCS endpoint: {e:?}"))?;

        // Configure channel...
        let channel = channel
            .connect()
            .await
            .map_err(|e| make_err!(Code::Unavailable, "Failed to connect to GCS: {e:?}"))?;

        let storage_client = StorageClient::new(channel);

        Ok(Self {
            client: Arc::new(RwLock::new(GcsClientWrapper::new(storage_client))),
            auth: Arc::new(GcsAuth::new().await?),
            retrier: Arc::new(Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                jitter_fn,
                spec.retry.clone(),
            )),
        })
    }

    async fn add_auth_and_common_headers(
        &self,
        metadata: &mut MetadataMap,
        object: ObjectPath,
    ) -> Result<(), Error> {
        // Add authorization header
        let token = self.auth.get_valid_token().await?;
        metadata.insert(
            "authorization",
            MetadataValue::try_from(&format!("Bearer {token}")).unwrap(),
        );

        // Add bucket parameter. This is required for all requests
        let bucket = object.get_formatted_bucket();
        let encoded_bucket = urlencoding::encode(&bucket);
        let params = format!("bucket={encoded_bucket}");

        metadata.insert(
            "x-goog-request-params",
            MetadataValue::try_from(&params).unwrap(),
        );

        Ok(())
    }

    async fn prepare_request<T>(&self, request: T, object: ObjectPath) -> Request<T> {
        let mut request = Request::new(request);
        self.add_auth_and_common_headers(request.metadata_mut(), object)
            .await
            .expect("Failed to add headers");
        request
    }

    fn create_write_spec(&self, object: &ObjectPath, size: i64) -> WriteObjectSpec {
        WriteObjectSpec {
            resource: Some(Object {
                name: object.path.clone(),
                bucket: object.get_formatted_bucket(),
                size,
                content_type: "application/octet-stream".to_string(),
                ..Default::default()
            }),
            object_size: Some(size),
            ..Default::default()
        }
    }

    async fn simple_upload(
        &self,
        object: ObjectPath,
        reader: DropCloserReadHalf,
        size: i64,
    ) -> Result<(), Error> {
        let retrier = self.retrier.clone();
        let client = self.client.clone();
        let object_clone = object.clone();
        let self_clone = self.clone();
        let write_spec = self.create_write_spec(&object, size);

        // Create a stream that will yield our operation result
        let operation_stream = futures::stream::unfold(
            (client, object_clone, self_clone, write_spec, reader),
            move |(client, object, self_ref, write_spec, mut reader)| {
                async move {
                    let (mut tx, mut rx) = make_buf_channel_pair();

                    let attempt_result = async {
                        let (upload_res, bind_res) = tokio::join!(
                            async {
                                let mut client_guard = client.write().await;
                                let mut buffer = Vec::with_capacity(size as usize);
                                while let Ok(Some(chunk)) = rx.try_next().await {
                                    buffer.extend_from_slice(&chunk);
                                }
                                let crc32c = crc32c::crc32c(&buffer);

                                let init_request = WriteObjectRequest {
                                    first_message: Some(
                                        write_object_request::FirstMessage::WriteObjectSpec(
                                            write_spec.clone(),
                                        ),
                                    ),
                                    write_offset: 0,
                                    data: None,
                                    finish_write: false,
                                    ..Default::default()
                                };

                                let data_request = WriteObjectRequest {
                                    first_message: None,
                                    write_offset: 0,
                                    data: Some(write_object_request::Data::ChecksummedData(
                                        ChecksummedData {
                                            content: buffer,
                                            crc32c: Some(crc32c),
                                        },
                                    )),
                                    finish_write: true,
                                    ..Default::default()
                                };

                                let request_stream = Box::pin(futures::stream::iter(vec![
                                    init_request,
                                    data_request,
                                ]))
                                    as WriteObjectStream;
                                let mut request = Request::new(request_stream);

                                self_ref
                                    .add_auth_and_common_headers(
                                        request.metadata_mut(),
                                        object.clone(),
                                    )
                                    .await?;

                                client_guard
                                    .handle_request(|client| Box::pin(client.write_object(request)))
                                    .await
                            },
                            async { tx.bind_buffered(&mut reader).await }
                        );

                        match (upload_res, bind_res) {
                            (Ok(_), Ok(())) => Ok(()),
                            (Err(e), _) | (_, Err(e)) => Err(e),
                        }
                    }
                    .await;

                    // Return both the result and the state for potential next retry
                    Some((
                        RetryResult::Ok(attempt_result),
                        (client, object, self_ref, write_spec, reader),
                    ))
                }
            },
        );

        retrier.retry(operation_stream).await?
    }

    async fn resumable_upload(
        &self,
        object: ObjectPath,
        reader: DropCloserReadHalf,
        size: i64,
    ) -> Result<(), Error> {
        let retrier = self.retrier.clone();
        let client = self.client.clone();
        let object_clone = object.clone();
        let self_clone = self.clone();
        let write_spec = self.create_write_spec(&object, size);

        let operation_stream = futures::stream::unfold(
            (client, object_clone, self_clone, write_spec, reader),
            move |(client, object, self_ref, write_spec, mut reader)| async move {
                let attempt_result = async {
                    let mut client_guard = client.write().await;
                    let start_request = StartResumableWriteRequest {
                        write_object_spec: Some(write_spec.clone()),
                        common_object_request_params: None,
                        object_checksums: None,
                    };

                    let request = self_ref
                        .prepare_request(start_request, object.clone())
                        .await;
                    let response = client_guard
                        .handle_request(|client| Box::pin(client.start_resumable_write(request)))
                        .await?;

                    let upload_id = response.into_inner().upload_id;

                    let mut requests = Vec::new();
                    requests.push(BidiWriteObjectRequest {
                        first_message: Some(bidi_write_object_request::FirstMessage::UploadId(
                            upload_id,
                        )),
                        write_offset: 0,
                        finish_write: false,
                        data: None,
                        ..Default::default()
                    });

                    let mut offset = 0;
                    while offset < size {
                        let chunk_size = std::cmp::min(MAX_CHUNK_SIZE, (size - offset) as usize);

                        let chunk = reader
                            .consume(Some(chunk_size))
                            .await
                            .err_tip(|| "Failed to read chunk")?;

                        if chunk.is_empty() {
                            break;
                        }

                        let chunk_len = chunk.len();
                        let crc32c = crc32c::crc32c(&chunk);
                        let is_last = offset + (chunk_len as i64) >= size;

                        requests.push(BidiWriteObjectRequest {
                            first_message: None,
                            write_offset: offset,
                            data: Some(bidi_write_object_request::Data::ChecksummedData(
                                ChecksummedData {
                                    content: chunk.to_vec(),
                                    crc32c: Some(crc32c),
                                },
                            )),
                            finish_write: is_last,
                            ..Default::default()
                        });

                        offset += chunk_len as i64;
                    }

                    let request_stream =
                        Box::pin(futures::stream::iter(requests)) as BidiWriteObjectStream;
                    let mut request = Request::new(request_stream);

                    self_ref
                        .add_auth_and_common_headers(request.metadata_mut(), object.clone())
                        .await?;

                    client_guard
                        .handle_request(|client| Box::pin(client.bidi_write_object(request)))
                        .await?;

                    Ok(())
                }
                .await;

                Some((
                    RetryResult::Ok(attempt_result),
                    (client, object, self_ref, write_spec, reader),
                ))
            },
        );

        retrier.retry(operation_stream).await?
    }

    async fn read_object(
        &self,
        object: ObjectPath,
        read_offset: Option<i64>,
        read_limit: Option<i64>,
        metadata_only: bool,
    ) -> Result<Option<(Option<Object>, Vec<u8>)>, Error> {
        let retrier = self.retrier.clone();
        let client = self.client.clone();
        let object_clone = object.clone();
        let self_clone = self.clone();

        let operation_stream = futures::stream::unfold(
            (client, object_clone, self_clone),
            move |(client, object, self_ref)| {
                let read_offset = read_offset;
                let read_limit = read_limit;
                let metadata_only = metadata_only;

                async move {
                    let attempt_result = async {
                        let mut client_guard = client.write().await;
                        let request = ReadObjectRequest {
                            bucket: object.get_formatted_bucket(),
                            object: object.path.clone(),
                            read_offset: read_offset.unwrap_or(0),
                            read_limit: read_limit.unwrap_or(0),
                            ..Default::default()
                        };

                        let auth_request = self_ref.prepare_request(request, object.clone()).await;
                        let response = client_guard
                            .handle_request(|client| {
                                let future = client.read_object(auth_request);
                                Box::pin(future)
                            })
                            .await;

                        match response {
                            Ok(response) => {
                                let mut content = Vec::new();
                                let mut metadata = None;
                                let mut stream = response.into_inner();

                                if let Some(Ok(first_message)) = stream.next().await {
                                    metadata = first_message.metadata;
                                    if metadata_only {
                                        return Ok(Some((metadata, content)));
                                    }
                                }

                                while let Some(chunk) = stream.next().await {
                                    match chunk {
                                        Ok(data) => {
                                            if let Some(checksummed_data) = data.checksummed_data {
                                                content.extend(checksummed_data.content);
                                            }
                                        }
                                        Err(e) => {
                                            return Err(make_err!(
                                                Code::Unavailable,
                                                "Error reading object data: {e:?}"
                                            ));
                                        }
                                    }
                                }

                                Ok(Some((metadata, content)))
                            }
                            Err(e) => match e.code {
                                Code::NotFound => Ok(None),
                                _ => Err(make_err!(
                                    Code::Unavailable,
                                    "Failed to read object: {e:?}"
                                )),
                            },
                        }
                    }
                    .await;

                    Some((RetryResult::Ok(attempt_result), (client, object, self_ref)))
                }
            },
        );

        retrier.retry(operation_stream).await?
    }

    async fn read_object_metadata(&self, object: ObjectPath) -> Result<Option<Object>, Error> {
        Ok(self
            .read_object(object, None, None, true)
            .await?
            .and_then(|(metadata, _)| metadata))
    }

    async fn read_object_content(
        &self,
        object: ObjectPath,
        start: i64,
        end: Option<i64>,
    ) -> Result<Vec<u8>, Error> {
        match self
            .read_object(object.clone(), Some(start), end.map(|e| e - start), false)
            .await?
        {
            Some((_, content)) => Ok(content),
            None => Err(make_err!(
                Code::NotFound,
                "Object not found: {}",
                object.path
            )),
        }
    }
}

/// The main Google Cloud Storage implementation.
#[derive(MetricsComponent)]
pub struct GcsStore<NowFn> {
    client: Arc<GcsClient>,
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
    chunk_size: usize,
    #[metric(help = "The number of concurrent uploads allowed")]
    max_concurrent_uploads: usize,
}

impl<I, NowFn> GcsStore<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    pub async fn new(spec: &GcsSpec, now_fn: NowFn) -> Result<Arc<Self>, Error> {
        let jitter_amt = spec.retry.jitter;
        let jitter_fn = Arc::new(move |delay: Duration| {
            if jitter_amt == 0. {
                return delay;
            }
            let min = 1. - (jitter_amt / 2.);
            let max = 1. + (jitter_amt / 2.);
            delay.mul_f32(OsRng.gen_range(min..max))
        });

        let client = GcsClient::new(spec, jitter_fn.clone()).await?;

        Self::new_with_client_and_jitter(spec, client, jitter_fn, now_fn)
    }

    pub fn new_with_client_and_jitter(
        spec: &GcsSpec,
        gcs_client: GcsClient,
        jitter_fn: Arc<dyn Fn(Duration) -> Duration + Send + Sync>,
        now_fn: NowFn,
    ) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self {
            client: Arc::new(gcs_client),
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
                .unwrap_or(DEFAULT_MAX_RETRY_BUFFER_SIZE),
            chunk_size: spec
                .resumable_chunk_size
                .unwrap_or(MAX_CHUNK_SIZE)
                .min(MAX_CHUNK_SIZE),
            max_concurrent_uploads: spec
                .max_concurrent_uploads
                .unwrap_or(DEFAULT_MAX_CONCURRENT_UPLOADS),
        }))
    }

    fn make_object_path(&self, key: &StoreKey<'_>) -> String {
        format!("{}{}", self.key_prefix, key.as_str())
    }

    pub async fn has(self: Pin<&Self>, digest: &StoreKey<'_>) -> Result<Option<u64>, Error> {
        let object = ObjectPath::new(self.bucket.clone(), &self.make_object_path(digest));

        let client = self.client.clone();
        let consider_expired_after_s = self.consider_expired_after_s;
        let now_fn = &self.now_fn;

        self.retrier
            .retry(futures::stream::unfold(
                object.clone(),
                move |object_path| {
                    let client = client.clone();

                    async move {
                        let result = client.read_object_metadata(object_path.clone()).await;

                        match result {
                            Ok(Some(object)) => {
                                if consider_expired_after_s > 0 {
                                    if let Some(update_time) = object.update_time {
                                        let now_s = now_fn().unix_timestamp() as i64;
                                        if update_time.seconds + consider_expired_after_s <= now_s {
                                            return Some((RetryResult::Ok(None), object_path));
                                        }
                                    }
                                }
                                Some((RetryResult::Ok(Some(object.size as u64)), object_path))
                            }
                            Ok(None) => Some((RetryResult::Ok(None), object_path)),
                            Err(e) => Some((RetryResult::Retry(e), object_path)),
                        }
                    }
                },
            ))
            .await
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
        use futures::stream::FuturesUnordered;

        keys.iter()
            .zip(results.iter_mut())
            .map(|(key, result)| async move {
                if is_zero_digest(key.borrow()) {
                    *result = Some(0);
                    return Ok::<_, Error>(());
                }
                *result = self.has(key).await?;
                Ok::<_, Error>(())
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
        let object = ObjectPath::new(self.bucket.clone(), &self.make_object_path(&digest));

        if let UploadSizeInfo::ExactSize(0) = upload_size {
            return Ok(());
        }

        let max_size = match upload_size {
            UploadSizeInfo::ExactSize(sz) | UploadSizeInfo::MaxSize(sz) => sz,
        };

        // For small files (< 4MB) use simple upload
        if max_size < MIN_MULTIPART_SIZE && matches!(upload_size, UploadSizeInfo::ExactSize(_)) {
            let UploadSizeInfo::ExactSize(sz) = upload_size else {
                unreachable!("upload_size must be ExactSize here");
            };

            reader.set_max_recent_data_size(
                u64::try_from(self.max_retry_buffer_size)
                    .err_tip(|| "Could not convert max_retry_buffer_size to u64")?,
            );

            return self.retrier
                .retry(unfold(reader, move |mut reader| {
                    let client = Arc::clone(&self.client);
                    let object = object.clone();

                    async move {
                        let (mut tx, rx) = make_buf_channel_pair();

                        let result = {
                            let reader_ref = &mut reader;
                            let (upload_res, bind_res) = tokio::join!(
                            async {
                                client
                                    .simple_upload(object.clone(), rx, sz as i64)
                                    .await
                                    .map_err(|e| make_err!(Code::Aborted, "{:?}", e))
                            },
                            async {
                                tx.bind_buffered(reader_ref).await
                            }
                        );

                            upload_res
                                .and(bind_res)
                                .err_tip(|| "Failed to upload object in single chunk")
                        };

                        match result {
                            Ok(()) => Some((RetryResult::Ok(()), reader)),
                            Err(mut err) => {
                                err.code = Code::Aborted;
                                let bytes_received = reader.get_bytes_received();

                                if let Err(try_reset_err) = reader.try_reset_stream() {
                                    error!(
                                    ?bytes_received,
                                    err = ?try_reset_err,
                                    "Unable to reset stream after failed upload"
                                );
                                    Some((
                                        RetryResult::Err(err
                                            .merge(try_reset_err)
                                            .append(format!(
                                                "Failed to retry upload with {bytes_received} bytes received"
                                            ))),
                                        reader,
                                    ))
                                } else {
                                    let err = err.append(format!(
                                        "Retry on upload happened with {bytes_received} bytes received"
                                    ));
                                    info!(?err, ?bytes_received, "Retryable GCS error");
                                    Some((RetryResult::Retry(err), reader))
                                }
                            }
                        }
                    }
                }))
                .await;
        }

        // For larger files, use resumable upload with streaming
        reader.set_max_recent_data_size(
            u64::try_from(self.max_retry_buffer_size)
                .err_tip(|| "Could not convert max_retry_buffer_size to u64")?,
        );

        self.retrier
            .retry(unfold(reader, move |mut reader| {
                let client = Arc::clone(&self.client);
                let object = object.clone();

                async move {
                    let (mut tx, rx) = make_buf_channel_pair();

                    let result = {
                        let reader_ref = &mut reader;
                        let (upload_res, bind_res) = tokio::join!(
                        async {
                            client
                                .resumable_upload(object.clone(), rx, max_size as i64)
                                .await
                                .map_err(|e| make_err!(Code::Aborted, "{:?}", e))
                        },
                        async {
                            tx.bind_buffered(reader_ref).await
                        }
                    );

                        upload_res
                            .and(bind_res)
                            .err_tip(|| "Failed to upload object using resumable upload")
                    };

                    match result {
                        Ok(()) => Some((RetryResult::Ok(()), reader)),
                        Err(mut err) => {
                            err.code = Code::Aborted;
                            let bytes_received = reader.get_bytes_received();

                            if let Err(try_reset_err) = reader.try_reset_stream() {
                                error!(
                                ?bytes_received,
                                err = ?try_reset_err,
                                "Unable to reset stream after failed upload"
                            );
                                Some((
                                    RetryResult::Err(err
                                        .merge(try_reset_err)
                                        .append(format!(
                                            "Failed to retry upload with {bytes_received} bytes received"
                                        ))),
                                    reader,
                                ))
                            } else {
                                let err = err.append(format!(
                                    "Retry on upload happened with {bytes_received} bytes received"
                                ));
                                info!(?err, ?bytes_received, "Retryable GCS error");
                                Some((RetryResult::Retry(err), reader))
                            }
                        }
                    }
                }
            }))
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
            writer
                .send_eof()
                .err_tip(|| "Failed to send zero EOF in GCS store get_part")?;
            return Ok(());
        }

        let object = ObjectPath::new(self.bucket.clone(), &self.make_object_path(&key));

        let client = Arc::clone(&self.client);

        self.retrier
            .retry(unfold(writer, move |writer| {
                let client = client.clone();
                let object = object.clone();
                async move {
                    let result: Result<(), Error> = async {
                        let data = client
                            .read_object_content(
                                object,
                                offset as i64,
                                length.map(|len| offset as i64 + len as i64),
                            )
                            .await?;

                        if !data.is_empty() {
                            writer.send(Bytes::from(data)).await.map_err(|e| {
                                make_err!(Code::Aborted, "Failed to send data to writer: {:?}", e)
                            })?;
                        }

                        writer.send_eof().map_err(|e| {
                            make_err!(Code::Aborted, "Failed to send EOF to writer: {:?}", e)
                        })?;
                        Ok(())
                    }
                    .await;

                    match result {
                        Ok(()) => Some((RetryResult::Ok(()), writer)),
                        Err(e) => {
                            if e.code == Code::NotFound {
                                Some((RetryResult::Err(e), writer))
                            } else {
                                Some((RetryResult::Retry(e), writer))
                            }
                        }
                    }
                }
            }))
            .await
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &'_ dyn StoreDriver {
        self
    }

    fn as_any<'a>(&'a self) -> &'a (dyn std::any::Any + Sync + Send + 'static) {
        self
    }

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
