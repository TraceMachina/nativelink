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

use core::cmp;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use core::time::Duration;
use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use aws_config::default_provider::credentials;
use aws_config::provider_config::ProviderConfig;
use aws_config::retry::ErrorKind::TransientError;
use aws_config::{AppName, BehaviorVersion};
use aws_sdk_s3::Client;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::operation::create_multipart_upload::CreateMultipartUploadOutput;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::primitives::ByteStream; // SdkBody
use aws_sdk_s3::types::builders::{CompletedMultipartUploadBuilder, CompletedPartBuilder};
use aws_smithy_runtime_api::client::http::{
    HttpClient as SmithyHttpClient, HttpConnector as SmithyHttpConnector, HttpConnectorFuture,
    HttpConnectorSettings, SharedHttpConnector,
};
use aws_smithy_runtime_api::client::orchestrator::HttpRequest;
use aws_smithy_runtime_api::client::result::ConnectorError;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_runtime_api::http::Response;
use aws_smithy_types::body::SdkBody;
use bytes::{Bytes, BytesMut};
use futures::future::FusedFuture;
use futures::stream::{FuturesUnordered, unfold};
use futures::{FutureExt, Stream, StreamExt, TryFutureExt, TryStreamExt};
use http_body::{Frame, SizeHint};
use http_body_util::BodyExt;
use hyper::{Method, Request};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client as LegacyClient;
use hyper_util::client::legacy::connect::HttpConnector as LegacyHttpConnector;
use hyper_util::rt::TokioExecutor;
use nativelink_config::stores::ExperimentalAwsSpec;
// Note: S3 store should be very careful about the error codes it returns
// when in a retryable wrapper. Always prefer Code::Aborted or another
// retryable code over Code::InvalidArgument or make_input_err!().
// ie: Don't import make_input_err!() to help prevent this.
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair,
};
use nativelink_util::fs;
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::retry::{Retrier, RetryResult};
use nativelink_util::store_trait::{StoreDriver, StoreKey, UploadSizeInfo};
use rand::Rng;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{error, info};

use crate::cas_utils::is_zero_digest;

// S3 parts cannot be smaller than this number. See:
// https://docs.aws.amazon.com/AmazonS3/latest/userguide/qfacts.html
const MIN_MULTIPART_SIZE: u64 = 5 * 1024 * 1024; // 5MB.

// S3 parts cannot be larger than this number. See:
// https://docs.aws.amazon.com/AmazonS3/latest/userguide/qfacts.html
const MAX_MULTIPART_SIZE: u64 = 5 * 1024 * 1024 * 1024; // 5GB.

// S3 parts cannot be more than this number. See:
// https://docs.aws.amazon.com/AmazonS3/latest/userguide/qfacts.html
const MAX_UPLOAD_PARTS: usize = 10_000;

// Default max buffer size for retrying upload requests.
// Note: If you change this, adjust the docs in the config.
const DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST: usize = 5 * 1024 * 1024; // 5MB.

// Default limit for concurrent part uploads per multipart upload.
// Note: If you change this, adjust the docs in the config.
const DEFAULT_MULTIPART_MAX_CONCURRENT_UPLOADS: usize = 10;

#[derive(Clone)]
pub struct TlsClient {
    client: LegacyClient<HttpsConnector<LegacyHttpConnector>, SdkBody>,
    retrier: Retrier,
}

impl TlsClient {
    #[must_use]
    pub fn new(
        spec: &ExperimentalAwsSpec,
        jitter_fn: Arc<dyn Fn(Duration) -> Duration + Send + Sync>,
    ) -> Self {
        let connector_with_roots = HttpsConnectorBuilder::new().with_webpki_roots();

        let connector_with_schemes = if spec.common.insecure_allow_http {
            connector_with_roots.https_or_http()
        } else {
            connector_with_roots.https_only()
        };

        let connector = if spec.common.disable_http2 {
            connector_with_schemes.enable_http1().build()
        } else {
            connector_with_schemes.enable_http1().enable_http2().build()
        };

        let client = LegacyClient::builder(TokioExecutor::new()).build(connector);

        Self {
            client,
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                jitter_fn,
                spec.common.retry.clone(),
            ),
        }
    }
}

impl core::fmt::Debug for TlsClient {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result<(), core::fmt::Error> {
        f.debug_struct("TlsClient").finish_non_exhaustive()
    }
}

impl SmithyHttpClient for TlsClient {
    fn http_connector(
        &self,
        _settings: &HttpConnectorSettings,
        _components: &RuntimeComponents,
    ) -> SharedHttpConnector {
        SharedHttpConnector::new(self.clone())
    }
}

enum BufferedBodyState {
    Cloneable(SdkBody),
    Buffered(Bytes),
    Empty,
}

mod body_processing {
    use super::{BodyExt, BufferedBodyState, BytesMut, ConnectorError, SdkBody, TransientError};

    /// Buffer a request body fully into memory.
    ///
    /// TODO(aaronmondal): This could lead to OOMs in extremely constrained
    ///                    environments. Probably better to implement something
    ///                    like a rewindable stream logic.
    #[inline]
    pub(crate) async fn buffer_body(body: SdkBody) -> Result<BufferedBodyState, ConnectorError> {
        let mut bytes = BytesMut::new();
        let mut body_stream = body;
        while let Some(frame) = body_stream.frame().await {
            match frame {
                Ok(frame) => {
                    if let Some(data) = frame.data_ref() {
                        bytes.extend_from_slice(data);
                    }
                }
                Err(e) => {
                    return Err(ConnectorError::other(
                        format!("Failed to read request body: {e}").into(),
                        Some(TransientError),
                    ));
                }
            }
        }

        Ok(BufferedBodyState::Buffered(bytes.freeze()))
    }
}

struct RequestComponents {
    method: Method,
    uri: hyper::Uri,
    version: hyper::Version,
    headers: hyper::HeaderMap,
    body_data: BufferedBodyState,
}

mod conversions {
    use super::{
        BufferedBodyState, ConnectorError, Future, HttpRequest, Method, RequestComponents,
        Response, SdkBody, TransientError, body_processing,
    };

    pub(crate) trait RequestExt {
        fn into_components(self)
        -> impl Future<Output = Result<RequestComponents, ConnectorError>>;
    }

    impl RequestExt for HttpRequest {
        async fn into_components(self) -> Result<RequestComponents, ConnectorError> {
            // Note: This does *not* refer the the HTTP protocol, but to the
            //       version of the http crate.
            let hyper_req = self.try_into_http1x().map_err(|e| {
                ConnectorError::other(
                    format!("Failed to convert to HTTP request: {e}").into(),
                    Some(TransientError),
                )
            })?;

            let method = hyper_req.method().clone();
            let uri = hyper_req.uri().clone();
            let version = hyper_req.version();
            let headers = hyper_req.headers().clone();

            let body = hyper_req.into_body();

            // Only buffer bodies for methods likely to have payloads.
            let needs_buffering = matches!(method, Method::POST | Method::PUT);

            // Preserve the body in case we need to retry.
            let body_data = if needs_buffering {
                if let Some(cloneable_body) = body.try_clone() {
                    BufferedBodyState::Cloneable(cloneable_body)
                } else {
                    body_processing::buffer_body(body).await?
                }
            } else {
                BufferedBodyState::Empty
            };

            Ok(RequestComponents {
                method,
                uri,
                version,
                headers,
                body_data,
            })
        }
    }

    pub(crate) trait ResponseExt {
        fn into_smithy_response(self) -> Response<SdkBody>;
    }

    impl ResponseExt for hyper::Response<hyper::body::Incoming> {
        fn into_smithy_response(self) -> Response<SdkBody> {
            let (parts, body) = self.into_parts();
            let sdk_body = SdkBody::from_body_1_x(body);
            let mut smithy_resp = Response::new(parts.status.into(), sdk_body);
            let header_pairs: Vec<(String, String)> = parts
                .headers
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value_str| (name.as_str().to_owned(), value_str.to_owned()))
                })
                .collect();

            for (name, value) in header_pairs {
                smithy_resp.headers_mut().insert(name, value);
            }

            smithy_resp
        }
    }
}

struct RequestBuilder<'a> {
    components: &'a RequestComponents,
}

impl<'a> RequestBuilder<'a> {
    #[inline]
    const fn new(components: &'a RequestComponents) -> Self {
        Self { components }
    }

    #[inline]
    #[allow(unused_qualifications, reason = "false positive on hyper::http::Error")]
    fn build(&self) -> Result<Request<SdkBody>, hyper::http::Error> {
        let mut req_builder = Request::builder()
            .method(self.components.method.clone())
            .uri(self.components.uri.clone())
            .version(self.components.version);

        let headers_map = req_builder.headers_mut().unwrap();
        for (name, value) in &self.components.headers {
            headers_map.insert(name, value.clone());
        }

        match &self.components.body_data {
            BufferedBodyState::Cloneable(body) => {
                let cloned_body = body.try_clone().expect("Body should be cloneable");
                req_builder.body(cloned_body)
            }
            BufferedBodyState::Buffered(bytes) => req_builder.body(SdkBody::from(bytes.clone())),
            BufferedBodyState::Empty => req_builder.body(SdkBody::empty()),
        }
    }
}

mod execution {
    use super::conversions::ResponseExt;
    use super::{
        Code, HttpsConnector, LegacyClient, LegacyHttpConnector, RequestBuilder, RequestComponents,
        Response, RetryResult, SdkBody, fs, make_err,
    };

    #[inline]
    pub(crate) async fn execute_request(
        client: LegacyClient<HttpsConnector<LegacyHttpConnector>, SdkBody>,
        components: &RequestComponents,
    ) -> RetryResult<Response<SdkBody>> {
        let _permit = match fs::get_permit().await {
            Ok(permit) => permit,
            Err(e) => {
                return RetryResult::Retry(make_err!(
                    Code::Unavailable,
                    "Failed to acquire permit: {e}"
                ));
            }
        };

        let request = match RequestBuilder::new(components).build() {
            Ok(req) => req,
            Err(e) => {
                return RetryResult::Err(make_err!(
                    Code::Internal,
                    "Failed to create request: {e}",
                ));
            }
        };

        match client.request(request).await {
            Ok(resp) => RetryResult::Ok(resp.into_smithy_response()),
            Err(e) => RetryResult::Retry(make_err!(
                Code::Unavailable,
                "Failed request in S3Store: {e}"
            )),
        }
    }

    #[inline]
    pub(crate) fn create_retry_stream(
        client: LegacyClient<HttpsConnector<LegacyHttpConnector>, SdkBody>,
        components: RequestComponents,
    ) -> impl futures::Stream<Item = RetryResult<Response<SdkBody>>> {
        futures::stream::unfold(components, move |components| {
            let client_clone = client.clone();
            async move {
                let result = execute_request(client_clone, &components).await;

                Some((result, components))
            }
        })
    }
}

impl SmithyHttpConnector for TlsClient {
    fn call(&self, req: HttpRequest) -> HttpConnectorFuture {
        use conversions::RequestExt;

        let client = self.client.clone();
        let retrier = self.retrier.clone();

        HttpConnectorFuture::new(Box::pin(async move {
            let components = req.into_components().await?;

            let retry_stream = execution::create_retry_stream(client, components);

            match retrier.retry(retry_stream).await {
                Ok(response) => Ok(response),
                Err(e) => Err(ConnectorError::other(
                    format!("Connection failed after retries: {e}").into(),
                    Some(TransientError),
                )),
            }
        }))
    }
}

#[derive(Debug)]
pub struct BodyWrapper {
    reader: DropCloserReadHalf,
    size: u64,
}

impl http_body::Body for BodyWrapper {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let reader = Pin::new(&mut Pin::get_mut(self).reader);
        reader
            .poll_next(cx)
            .map(|maybe_bytes_res| maybe_bytes_res.map(|res| res.map(Frame::data)))
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.size)
    }
}

#[derive(Debug, MetricsComponent)]
pub struct S3Store<NowFn> {
    s3_client: Arc<Client>,
    now_fn: NowFn,
    #[metric(help = "The bucket name for the S3 store")]
    bucket: String,
    #[metric(help = "The key prefix for the S3 store")]
    key_prefix: String,
    retrier: Retrier,
    #[metric(help = "The number of seconds to consider an object expired")]
    consider_expired_after_s: i64,
    #[metric(help = "The number of bytes to buffer for retrying requests")]
    max_retry_buffer_per_request: usize,
    #[metric(help = "The number of concurrent uploads allowed for multipart uploads")]
    multipart_max_concurrent_uploads: usize,
}

impl<I, NowFn> S3Store<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    pub async fn new(spec: &ExperimentalAwsSpec, now_fn: NowFn) -> Result<Arc<Self>, Error> {
        let jitter_amt = spec.common.retry.jitter;
        let jitter_fn = Arc::new(move |delay: Duration| {
            if jitter_amt == 0. {
                return delay;
            }
            delay.mul_f32(jitter_amt.mul_add(rand::rng().random::<f32>() - 0.5, 1.))
        });
        let s3_client = {
            let http_client = TlsClient::new(&spec.clone(), jitter_fn.clone());

            let credential_provider = credentials::DefaultCredentialsChain::builder()
                .configure(
                    ProviderConfig::without_region()
                        .with_region(Some(Region::new(Cow::Owned(spec.region.clone()))))
                        .with_http_client(http_client.clone()),
                )
                .build()
                .await;

            let config = aws_config::defaults(BehaviorVersion::v2025_01_17())
                .credentials_provider(credential_provider)
                .app_name(AppName::new("nativelink").expect("valid app name"))
                .timeout_config(
                    aws_config::timeout::TimeoutConfig::builder()
                        .connect_timeout(Duration::from_secs(15))
                        .build(),
                )
                .region(Region::new(Cow::Owned(spec.region.clone())))
                .http_client(http_client)
                .load()
                .await;

            Client::new(&config)
        };
        Self::new_with_client_and_jitter(spec, s3_client, jitter_fn, now_fn)
    }

    pub fn new_with_client_and_jitter(
        spec: &ExperimentalAwsSpec,
        s3_client: Client,
        jitter_fn: Arc<dyn Fn(Duration) -> Duration + Send + Sync>,
        now_fn: NowFn,
    ) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self {
            s3_client: Arc::new(s3_client),
            now_fn,
            bucket: spec.bucket.to_string(),
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
            max_retry_buffer_per_request: spec
                .common
                .max_retry_buffer_per_request
                .unwrap_or(DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST),
            multipart_max_concurrent_uploads: spec
                .common
                .multipart_max_concurrent_uploads
                .map_or(DEFAULT_MULTIPART_MAX_CONCURRENT_UPLOADS, |v| v),
        }))
    }

    fn make_s3_path(&self, key: &StoreKey<'_>) -> String {
        format!("{}{}", self.key_prefix, key.as_str(),)
    }

    async fn has(self: Pin<&Self>, digest: &StoreKey<'_>) -> Result<Option<u64>, Error> {
        self.retrier
            .retry(unfold((), move |state| async move {
                let result = self
                    .s3_client
                    .head_object()
                    .bucket(&self.bucket)
                    .key(self.make_s3_path(&digest.borrow()))
                    .send()
                    .await;

                match result {
                    Ok(head_object_output) => {
                        if self.consider_expired_after_s != 0 {
                            if let Some(last_modified) = head_object_output.last_modified {
                                let now_s = (self.now_fn)().unix_timestamp() as i64;
                                if last_modified.secs() + self.consider_expired_after_s <= now_s {
                                    return Some((RetryResult::Ok(None), state));
                                }
                            }
                        }
                        let Some(length) = head_object_output.content_length else {
                            return Some((RetryResult::Ok(None), state));
                        };
                        if length >= 0 {
                            return Some((RetryResult::Ok(Some(length as u64)), state));
                        }
                        Some((
                            RetryResult::Err(make_err!(
                                Code::InvalidArgument,
                                "Negative content length in S3: {length:?}",
                            )),
                            state,
                        ))
                    }
                    Err(sdk_error) => match sdk_error.into_service_error() {
                        HeadObjectError::NotFound(_) => Some((RetryResult::Ok(None), state)),
                        other => Some((
                            RetryResult::Retry(make_err!(
                                Code::Unavailable,
                                "Unhandled HeadObjectError in S3: {other:?}"
                            )),
                            state,
                        )),
                    },
                }
            }))
            .await
    }
}

#[async_trait]
impl<I, NowFn> StoreDriver for S3Store<NowFn>
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
                // We need to do a special pass to ensure our zero key exist.
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
        let s3_path = &self.make_s3_path(&digest.borrow());

        let max_size = match upload_size {
            UploadSizeInfo::ExactSize(sz) | UploadSizeInfo::MaxSize(sz) => sz,
        };

        // Note(aaronmondal) It might be more optimal to use a different
        // heuristic here, but for simplicity we use a hard coded value.
        // Anything going down this if-statement will have the advantage of only
        // 1 network request for the upload instead of minimum of 3 required for
        // multipart upload requests.
        //
        // Note(aaronmondal) If the upload size is not known, we go down the multipart upload path.
        // This is not very efficient, but it greatly reduces the complexity of the code.
        if max_size < MIN_MULTIPART_SIZE && matches!(upload_size, UploadSizeInfo::ExactSize(_)) {
            let UploadSizeInfo::ExactSize(sz) = upload_size else {
                unreachable!("upload_size must be UploadSizeInfo::ExactSize here");
            };
            reader.set_max_recent_data_size(
                u64::try_from(self.max_retry_buffer_per_request)
                    .err_tip(|| "Could not convert max_retry_buffer_per_request to u64")?,
            );
            return self
                .retrier
                .retry(unfold(reader, move |mut reader| async move {
                    // We need to make a new pair here because the aws sdk does not give us
                    // back the body after we send it in order to retry.
                    let (mut tx, rx) = make_buf_channel_pair();

                    // Upload the data to the S3 backend.
                    let result = {
                        let reader_ref = &mut reader;
                        let (upload_res, bind_res) = tokio::join!(
                            self.s3_client
                                .put_object()
                                .bucket(&self.bucket)
                                .key(s3_path.clone())
                                .content_length(sz as i64)
                                .body(ByteStream::from_body_1_x(BodyWrapper {
                                    reader: rx,
                                    size: sz,
                                }))
                                .send()
                                .map_ok_or_else(|e| Err(make_err!(Code::Aborted, "{e:?}")), |_| Ok(())),
                            // Stream all data from the reader channel to the writer channel.
                            tx.bind_buffered(reader_ref)
                        );
                        upload_res
                            .merge(bind_res)
                            .err_tip(|| "Failed to upload file to s3 in single chunk")
                    };

                    // If we failed to upload the file, check to see if we can retry.
                    let retry_result = result.map_or_else(|mut err| {
                        // Ensure our code is Code::Aborted, so the client can retry if possible.
                        err.code = Code::Aborted;
                        let bytes_received = reader.get_bytes_received();
                        if let Err(try_reset_err) = reader.try_reset_stream() {
                            error!(
                                ?bytes_received,
                                err = ?try_reset_err,
                                "Unable to reset stream after failed upload in S3Store::update"
                            );
                            return RetryResult::Err(err
                                .merge(try_reset_err)
                                .append(format!("Failed to retry upload with {bytes_received} bytes received in S3Store::update")));
                        }
                        let err = err.append(format!("Retry on upload happened with {bytes_received} bytes received in S3Store::update"));
                        info!(
                            ?err,
                            ?bytes_received,
                            "Retryable S3 error"
                        );
                        RetryResult::Retry(err)
                    }, |()| RetryResult::Ok(()));
                    Some((retry_result, reader))
                }))
                .await;
        }

        let upload_id = &self
            .retrier
            .retry(unfold((), move |()| async move {
                let retry_result = self
                    .s3_client
                    .create_multipart_upload()
                    .bucket(&self.bucket)
                    .key(s3_path)
                    .send()
                    .await
                    .map_or_else(
                        |e| {
                            RetryResult::Retry(make_err!(
                                Code::Aborted,
                                "Failed to create multipart upload to s3: {e:?}"
                            ))
                        },
                        |CreateMultipartUploadOutput { upload_id, .. }| {
                            upload_id.map_or_else(
                                || {
                                    RetryResult::Err(make_err!(
                                        Code::Internal,
                                        "Expected upload_id to be set by s3 response"
                                    ))
                                },
                                RetryResult::Ok,
                            )
                        },
                    );
                Some((retry_result, ()))
            }))
            .await?;

        // S3 requires us to upload in parts if the size is greater than 5GB. The part size must be at least
        // 5mb (except last part) and can have up to 10,000 parts.
        let bytes_per_upload_part =
            (max_size / (MIN_MULTIPART_SIZE - 1)).clamp(MIN_MULTIPART_SIZE, MAX_MULTIPART_SIZE);

        let upload_parts = move || async move {
            // This will ensure we only have `multipart_max_concurrent_uploads` * `bytes_per_upload_part`
            // bytes in memory at any given time waiting to be uploaded.
            let (tx, mut rx) = mpsc::channel(self.multipart_max_concurrent_uploads);

            let read_stream_fut = async move {
                let retrier = &Pin::get_ref(self).retrier;
                // Note: Our break condition is when we reach EOF.
                for part_number in 1..i32::MAX {
                    let write_buf = reader
                        .consume(Some(usize::try_from(bytes_per_upload_part).err_tip(
                            || "Could not convert bytes_per_upload_part to usize",
                        )?))
                        .await
                        .err_tip(|| "Failed to read chunk in s3_store")?;
                    if write_buf.is_empty() {
                        break; // Reached EOF.
                    }

                    tx.send(retrier.retry(unfold(write_buf, move |write_buf| {
                        async move {
                            let retry_result = self
                                .s3_client
                                .upload_part()
                                .bucket(&self.bucket)
                                .key(s3_path)
                                .upload_id(upload_id)
                                .body(ByteStream::new(SdkBody::from(write_buf.clone())))
                                .part_number(part_number)
                                .send()
                                .await
                                .map_or_else(
                                    |e| {
                                        RetryResult::Retry(make_err!(
                                            Code::Aborted,
                                            "Failed to upload part {part_number} in S3 store: {e:?}"
                                        ))
                                    },
                                    |mut response| {
                                        RetryResult::Ok(
                                            CompletedPartBuilder::default()
                                                // Only set an entity tag if it exists. This saves
                                                // 13 bytes per part on the final request if it can
                                                // omit the `<ETAG><ETAG/>` string.
                                                .set_e_tag(response.e_tag.take())
                                                .part_number(part_number)
                                                .build(),
                                        )
                                    },
                                );
                            Some((retry_result, write_buf))
                        }
                    })))
                    .await
                    .map_err(|_| {
                        make_err!(Code::Internal, "Failed to send part to channel in s3_store")
                    })?;
                }
                Result::<_, Error>::Ok(())
            }
            .fuse();

            let mut upload_futures = FuturesUnordered::new();

            let mut completed_parts = Vec::with_capacity(
                usize::try_from(cmp::min(
                    MAX_UPLOAD_PARTS as u64,
                    (max_size / bytes_per_upload_part) + 1,
                ))
                .err_tip(|| "Could not convert u64 to usize")?,
            );
            tokio::pin!(read_stream_fut);
            loop {
                if read_stream_fut.is_terminated() && rx.is_empty() && upload_futures.is_empty() {
                    break; // No more data to process.
                }
                tokio::select! {
                    result = &mut read_stream_fut => result?, // Return error or wait for other futures.
                    Some(upload_result) = upload_futures.next() => completed_parts.push(upload_result?),
                    Some(fut) = rx.recv() => upload_futures.push(fut),
                }
            }

            // Even though the spec does not require parts to be sorted by number, we do it just in case
            // there's an S3 implementation that requires it.
            completed_parts.sort_unstable_by_key(|part| part.part_number);

            self.retrier
                .retry(unfold(completed_parts, move |completed_parts| async move {
                    Some((
                        self.s3_client
                            .complete_multipart_upload()
                            .bucket(&self.bucket)
                            .key(s3_path)
                            .multipart_upload(
                                CompletedMultipartUploadBuilder::default()
                                    .set_parts(Some(completed_parts.clone()))
                                    .build(),
                            )
                            .upload_id(upload_id)
                            .send()
                            .await
                            .map_or_else(
                                |e| {
                                    RetryResult::Retry(make_err!(
                                        Code::Aborted,
                                        "Failed to complete multipart upload in S3 store: {e:?}"
                                    ))
                                },
                                |_| RetryResult::Ok(()),
                            ),
                        completed_parts,
                    ))
                }))
                .await
        };
        // Upload our parts and complete the multipart upload.
        // If we fail attempt to abort the multipart upload (cleanup).
        upload_parts()
            .or_else(move |e| async move {
                Result::<(), _>::Err(e).merge(
                    // Note: We don't retry here because this is just a best attempt.
                    self.s3_client
                        .abort_multipart_upload()
                        .bucket(&self.bucket)
                        .key(s3_path)
                        .upload_id(upload_id)
                        .send()
                        .await
                        .map_or_else(
                            |e| {
                                let err = make_err!(
                                    Code::Aborted,
                                    "Failed to abort multipart upload in S3 store : {e:?}"
                                );
                                info!(?err, "Multipart upload error");
                                Err(err)
                            },
                            |_| Ok(()),
                        ),
                )
            })
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
                .err_tip(|| "Failed to send zero EOF in filesystem store get_part")?;
            return Ok(());
        }

        let s3_path = &self.make_s3_path(&key);
        let end_read_byte = length
            .map_or(Some(None), |length| Some(offset.checked_add(length)))
            .err_tip(|| "Integer overflow protection triggered")?;

        self.retrier
            .retry(unfold(writer, move |writer| async move {
                let result = self
                    .s3_client
                    .get_object()
                    .bucket(&self.bucket)
                    .key(s3_path)
                    .range(format!(
                        "bytes={}-{}",
                        offset + writer.get_bytes_written(),
                        end_read_byte.map_or_else(String::new, |v| v.to_string())
                    ))
                    .send()
                    .await;

                let mut s3_in_stream = match result {
                    Ok(head_object_output) => head_object_output.body,
                    Err(sdk_error) => match sdk_error.into_service_error() {
                        GetObjectError::NoSuchKey(e) => {
                            return Some((
                                RetryResult::Err(make_err!(
                                    Code::NotFound,
                                    "No such key in S3: {e}"
                                )),
                                writer,
                            ));
                        }
                        other => {
                            return Some((
                                RetryResult::Retry(make_err!(
                                    Code::Unavailable,
                                    "Unhandled GetObjectError in S3: {other:?}",
                                )),
                                writer,
                            ));
                        }
                    },
                };

                // Copy data from s3 input stream to the writer stream.
                while let Some(maybe_bytes) = s3_in_stream.next().await {
                    match maybe_bytes {
                        Ok(bytes) => {
                            if bytes.is_empty() {
                                // Ignore possible EOF. Different implementations of S3 may or may not
                                // send EOF this way.
                                continue;
                            }
                            if let Err(e) = writer.send(bytes).await {
                                return Some((
                                    RetryResult::Err(make_err!(
                                        Code::Aborted,
                                        "Error sending bytes to consumer in S3: {e}"
                                    )),
                                    writer,
                                ));
                            }
                        }
                        Err(e) => {
                            return Some((
                                RetryResult::Retry(make_err!(
                                    Code::Aborted,
                                    "Bad bytestream element in S3: {e}"
                                )),
                                writer,
                            ));
                        }
                    }
                }
                if let Err(e) = writer.send_eof() {
                    return Some((
                        RetryResult::Err(make_err!(
                            Code::Aborted,
                            "Failed to send EOF to consumer in S3: {e}"
                        )),
                        writer,
                    ));
                }
                Some((RetryResult::Ok(()), writer))
            }))
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
}

#[async_trait]
impl<I, NowFn> HealthStatusIndicator for S3Store<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    fn get_name(&self) -> &'static str {
        "S3Store"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}
