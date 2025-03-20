// Copyright 2025 The NativeLink Authors. All rights reserved.
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
use std::cmp;
use std::fs::File;
use std::future::Future;
use std::io::BufReader;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use aws_config::default_provider::credentials::DefaultCredentialsChain;
use aws_config::BehaviorVersion;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::operation::create_multipart_upload::CreateMultipartUploadOutput;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::primitives::{ByteStream, SdkBody};
use aws_sdk_s3::types::builders::{CompletedMultipartUploadBuilder, CompletedPartBuilder};
use aws_sdk_s3::Client;
use aws_smithy_runtime::client::http::hyper_014::HyperClientBuilder;
use bytes::Bytes;
use futures::future::FusedFuture;
use futures::stream::{unfold, FuturesUnordered};
use futures::{FutureExt, Stream, StreamExt, TryFutureExt, TryStreamExt};
use http_body::{Frame, SizeHint};
use hyper::client::connect::{Connected, Connection, HttpConnector};
use hyper::service::Service;
use hyper::Uri;
use hyper_rustls::{ConfigBuilderExt, HttpsConnector, MaybeHttpsStream};
use nativelink_config::stores::OntapS3Spec;
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{
    make_buf_channel_pair, DropCloserReadHalf, DropCloserWriteHalf,
};
use nativelink_util::fs;
use nativelink_util::health_utils::{HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::retry::{Retrier, RetryResult};
use nativelink_util::store_trait::{StoreDriver, StoreKey, UploadSizeInfo};
use rand::Rng;
use rustls::{Certificate, ClientConfig, RootCertStore};
use rustls_pemfile::certs;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::SemaphorePermit;
use tokio::time::sleep;
use tracing::{event, Level};

use crate::cas_utils::is_zero_digest;

// S3 parts cannot be smaller than this number
const MIN_MULTIPART_SIZE: u64 = 5 * 1024 * 1024; // 5MB

// S3 parts cannot be larger than this number
const MAX_MULTIPART_SIZE: u64 = 5 * 1024 * 1024 * 1024; // 5GB

// S3 parts cannot be more than this number
const MAX_UPLOAD_PARTS: usize = 10_000;

// Default max buffer size for retrying upload requests
const DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST: usize = 20 * 1024 * 1024; // 20MB

// Default limit for concurrent part uploads per multipart upload
const DEFAULT_MULTIPART_MAX_CONCURRENT_UPLOADS: usize = 10;

pub struct ConnectionWithPermit<T: Connection + AsyncRead + AsyncWrite + Unpin> {
    connection: T,
    _permit: SemaphorePermit<'static>,
}

impl<T: Connection + AsyncRead + AsyncWrite + Unpin> Connection for ConnectionWithPermit<T> {
    fn connected(&self) -> Connected {
        self.connection.connected()
    }
}

impl<T: Connection + AsyncRead + AsyncWrite + Unpin> AsyncRead for ConnectionWithPermit<T> {
    #[inline]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut Pin::get_mut(self).connection).poll_read(cx, buf)
    }
}

impl<T: Connection + AsyncWrite + AsyncRead + Unpin> AsyncWrite for ConnectionWithPermit<T> {
    #[inline]
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, tokio::io::Error>> {
        Pin::new(&mut Pin::get_mut(self).connection).poll_write(cx, buf)
    }

    #[inline]
    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut Pin::get_mut(self).connection).poll_flush(cx)
    }

    #[inline]
    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut Pin::get_mut(self).connection).poll_shutdown(cx)
    }
}

#[derive(Clone)]
pub struct TlsConnector {
    connector: HttpsConnector<HttpConnector>,
    retrier: Retrier,
}

impl TlsConnector {
    #[must_use]
    pub fn new(
        spec: &OntapS3Spec,
        jitter_fn: Arc<dyn (Fn(Duration) -> Duration) + Send + Sync>,
    ) -> Self {
        let connector_with_roots = hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots();

        let connector_with_schemes = if spec.insecure_allow_http {
            connector_with_roots.https_or_http()
        } else {
            connector_with_roots.https_only()
        };

        let connector = if spec.disable_http2 {
            connector_with_schemes.enable_http1().build()
        } else {
            connector_with_schemes.enable_http1().enable_http2().build()
        };

        Self {
            connector,
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                jitter_fn,
                spec.retry.clone(),
            ),
        }
    }

    async fn call_with_retry(
        &self,
        req: &Uri,
    ) -> Result<ConnectionWithPermit<MaybeHttpsStream<TcpStream>>, Error> {
        let retry_stream_fn = unfold(self.connector.clone(), move |mut connector| async move {
            let _permit = fs::get_permit().await.unwrap();
            match connector.call(req.clone()).await {
                Ok(connection) => Some((
                    RetryResult::Ok(ConnectionWithPermit {
                        connection,
                        _permit,
                    }),
                    connector,
                )),
                Err(e) => Some((
                    RetryResult::Retry(make_err!(
                        Code::Unavailable,
                        "Failed to call ONTAP S3 connector: {e:?}"
                    )),
                    connector,
                )),
            }
        });
        self.retrier.retry(retry_stream_fn).await
    }
}

impl Service<Uri> for TlsConnector {
    type Response = ConnectionWithPermit<MaybeHttpsStream<TcpStream>>;
    type Error = Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.connector
            .poll_ready(cx)
            .map_err(|e| make_err!(Code::Unavailable, "Failed poll in ONTAP S3: {e}"))
    }

    fn call(&mut self, req: Uri) -> Self::Future {
        let connector_clone = self.clone();
        Box::pin(async move { connector_clone.call_with_retry(&req).await })
    }
}

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

#[derive(MetricsComponent)]
pub struct OntapS3Store<NowFn> {
    s3_client: Arc<Client>,
    now_fn: NowFn,
    #[metric(help = "The bucket name for the ONTAP S3 store")]
    bucket: String,
    #[metric(help = "The key prefix for the ONTAP S3 store")]
    key_prefix: String,
    retrier: Retrier,
    #[metric(help = "The number of seconds to consider an object expired")]
    consider_expired_after_s: i64,
    #[metric(help = "The number of bytes to buffer for retrying requests")]
    max_retry_buffer_per_request: usize,
    #[metric(help = "The number of concurrent uploads allowed for multipart uploads")]
    multipart_max_concurrent_uploads: usize,
}

pub fn load_custom_certs(cert_path: &str) -> Result<Arc<ClientConfig>, Error> {
    let mut root_store = RootCertStore::empty();

    // Create a BufReader from the cert file
    let mut cert_reader = BufReader::new(
        File::open(cert_path)
            .map_err(|e| make_err!(Code::Internal, "Failed to open CA certificate file: {e:?}"))?,
    );

    // Parse certificates
    let certs = certs(&mut cert_reader)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| make_err!(Code::Internal, "Failed to parse certificates: {e:?}"))?;

    // Add each certificate to the root store
    for cert in certs {
        root_store.add(&Certificate(cert.to_vec())).map_err(|e| {
            make_err!(
                Code::Internal,
                "Failed to add certificate to root store: {e:?}"
            )
        })?;
    }

    // Build the client config with the root store
    let config = ClientConfig::builder()
        .with_safe_defaults()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    Ok(Arc::new(config))
}

impl<I, NowFn> OntapS3Store<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    pub async fn new(spec: &OntapS3Spec, now_fn: NowFn) -> Result<Arc<Self>, Error> {
        let jitter_amt = spec.retry.jitter;
        let jitter_fn = Arc::new(move |delay: Duration| {
            if jitter_amt == 0.0 {
                return delay;
            }
            let min = 1.0 - jitter_amt / 2.0;
            let max = 1.0 + jitter_amt / 2.0;
            delay.mul_f32(rand::rng().random_range(min..max))
        });

        // Load custom CA config
        let ca_config = if let Some(cert_path) = &spec.root_certificates {
            load_custom_certs(cert_path)?
        } else {
            Arc::new(
                ClientConfig::builder()
                    .with_safe_defaults()
                    .with_native_roots()
                    .with_no_client_auth(),
            )
        };

        let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config((*ca_config).clone())
            .https_only()
            .enable_http1()
            .enable_http2()
            .build();

        let http_client = HyperClientBuilder::new().build(https_connector);

        let credentials_provider = DefaultCredentialsChain::builder().build().await;

        let config = aws_sdk_s3::Config::builder()
            .credentials_provider(credentials_provider)
            .endpoint_url(&spec.endpoint)
            .region(Region::new(spec.vserver_name.clone()))
            .app_name(aws_config::AppName::new("nativelink").expect("valid app name"))
            .http_client(http_client)
            .force_path_style(true)
            .behavior_version(BehaviorVersion::v2024_03_28())
            .timeout_config(
                aws_config::timeout::TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(30))
                    .operation_timeout(Duration::from_secs(120))
                    .build(),
            )
            .build();

        let s3_client = aws_sdk_s3::Client::from_conf(config);

        Ok(Arc::new(Self {
            s3_client: Arc::new(s3_client),
            now_fn,
            bucket: spec.bucket.clone(),
            key_prefix: spec.key_prefix.as_ref().unwrap_or(&String::new()).clone(),
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                jitter_fn,
                spec.retry.clone(),
            ),
            consider_expired_after_s: i64::from(spec.consider_expired_after_s),
            max_retry_buffer_per_request: spec
                .max_retry_buffer_per_request
                .unwrap_or(DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST),
            multipart_max_concurrent_uploads: spec
                .multipart_max_concurrent_uploads
                .unwrap_or(DEFAULT_MULTIPART_MAX_CONCURRENT_UPLOADS),
        }))
    }
    pub fn new_with_client_and_jitter(
        spec: &OntapS3Spec,
        s3_client: Client,
        jitter_fn: Arc<dyn (Fn(Duration) -> Duration) + Send + Sync>,
        now_fn: NowFn,
    ) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self {
            s3_client: Arc::new(s3_client),
            now_fn,
            bucket: spec.bucket.clone(),
            key_prefix: spec.key_prefix.as_ref().unwrap_or(&String::new()).clone(),
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                jitter_fn,
                spec.retry.clone(),
            ),
            consider_expired_after_s: i64::from(spec.consider_expired_after_s),
            max_retry_buffer_per_request: spec
                .max_retry_buffer_per_request
                .unwrap_or(DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST),
            multipart_max_concurrent_uploads: spec
                .multipart_max_concurrent_uploads
                .unwrap_or(DEFAULT_MULTIPART_MAX_CONCURRENT_UPLOADS),
        }))
    }

    fn make_s3_path(&self, key: &StoreKey<'_>) -> String {
        format!("{}{}", self.key_prefix, key.as_str())
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
                                "Negative content length in ONTAP S3: {length:?}"
                            )),
                            state,
                        ))
                    }
                    Err(sdk_error) => match sdk_error.into_service_error() {
                        HeadObjectError::NotFound(_) => Some((RetryResult::Ok(None), state)),
                        other => Some((
                            RetryResult::Retry(make_err!(
                                Code::Unavailable,
                                "Unhandled HeadObjectError in ONTAP S3: {other:?}"
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
impl<I, NowFn> StoreDriver for OntapS3Store<NowFn>
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
                    return Ok::<_, Error>(());
                }

                match self.has(key).await {
                    Ok(size) => {
                        *result = size;
                        if size.is_none() {
                            event!(
                                Level::INFO,
                                key = %key.as_str(),
                                "Object not found in ONTAP S3"
                            );
                        }
                        Ok(())
                    }
                    Err(err) => {
                        event!(
                            Level::ERROR,
                            key = %key.as_str(),
                            error = ?err,
                            "Error checking object existence"
                        );
                        Err(err)
                    }
                }
            })
            .collect::<FuturesUnordered<_>>()
            .try_collect()
            .await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        let s3_path = &self.make_s3_path(&key);

        let max_size = match size_info {
            UploadSizeInfo::ExactSize(sz) | UploadSizeInfo::MaxSize(sz) => sz,
        };

        // For small files, use simple upload. For larger files or unknown size, use multipart
        if max_size < MIN_MULTIPART_SIZE && matches!(size_info, UploadSizeInfo::ExactSize(_)) {
            let UploadSizeInfo::ExactSize(sz) = size_info else {
                unreachable!("upload_size must be UploadSizeInfo::ExactSize here");
            };
            reader.set_max_recent_data_size(
                u64::try_from(self.max_retry_buffer_per_request)
                    .err_tip(|| "Could not convert max_retry_buffer_per_request to u64")?,
            );
            return self.retrier.retry(
                unfold(reader, move |mut reader| async move {
                    let (mut tx, rx) = make_buf_channel_pair();

                    let result = {
                        let reader_ref = &mut reader;
                        let (upload_res, bind_res) = tokio::join!(
                            self.s3_client
                                .put_object()
                                .bucket(&self.bucket)
                                .key(s3_path.clone())
                                .content_length(sz as i64)
                                .body(
                                    ByteStream::from_body_1_x(BodyWrapper {
                                        reader: rx,
                                        size: sz,
                                    })
                                )
                                .send()
                                .map_ok_or_else(
                                    |e| Err(make_err!(Code::Aborted, "{e:?}")),
                                    |_| Ok(())
                                ),
                            tx.bind_buffered(reader_ref)
                        );
                        upload_res
                            .merge(bind_res)
                            .err_tip(|| "Failed to upload file to ONTAP S3 in single chunk")
                    };

                    let retry_result = result.map_or_else(
                        |mut err| {
                            err.code = Code::Aborted;
                            let bytes_received = reader.get_bytes_received();
                            if let Err(try_reset_err) = reader.try_reset_stream() {
                                event!(
                                    Level::ERROR,
                                    ?bytes_received,
                                    err = ?try_reset_err,
                                    "Unable to reset stream after failed upload in OntapS3Store::update"
                                );
                                return RetryResult::Err(
                                    err
                                        .merge(try_reset_err)
                                        .append(
                                            format!(
                                                "Failed to retry upload with {bytes_received} bytes received in OntapS3Store::update"
                                            )
                                        )
                                );
                            }
                            let err = err.append(
                                format!(
                                    "Retry on upload happened with {bytes_received} bytes received in OntapS3Store::update"
                                )
                            );
                            event!(Level::INFO, ?err, ?bytes_received, "Retryable ONTAP S3 error");
                            RetryResult::Retry(err)
                        },
                        |()| RetryResult::Ok(())
                    );
                    Some((retry_result, reader))
                })
            ).await;
        }

        // Handle multipart upload for large files
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
                                "Failed to create multipart upload to ONTAP S3: {e:?}"
                            ))
                        },
                        |CreateMultipartUploadOutput { upload_id, .. }| {
                            upload_id.map_or_else(
                                || {
                                    RetryResult::Err(make_err!(
                                        Code::Internal,
                                        "Expected upload_id to be set by ONTAP S3 response"
                                    ))
                                },
                                RetryResult::Ok,
                            )
                        },
                    );
                Some((retry_result, ()))
            }))
            .await?;

        let bytes_per_upload_part =
            (max_size / (MIN_MULTIPART_SIZE - 1)).clamp(MIN_MULTIPART_SIZE, MAX_MULTIPART_SIZE);

        let upload_parts = move || async move {
            let (tx, mut rx) = tokio::sync::mpsc::channel(self.multipart_max_concurrent_uploads);

            let read_stream_fut = (
                async move {
                    let retrier = &Pin::get_ref(self).retrier;
                    for part_number in 1..i32::MAX {
                        let write_buf = reader
                            .consume(
                                Some(
                                    usize
                                        ::try_from(bytes_per_upload_part)
                                        .err_tip(
                                            || "Could not convert bytes_per_upload_part to usize"
                                        )?
                                )
                            ).await
                            .err_tip(|| "Failed to read chunk in ontap_s3_store")?;
                        if write_buf.is_empty() {
                            break;
                        }

                        tx
                            .send(
                                retrier.retry(
                                    unfold(write_buf, move |write_buf| async move {
                                        let retry_result = self.s3_client
                                            .upload_part()
                                            .bucket(&self.bucket)
                                            .key(s3_path)
                                            .upload_id(upload_id)
                                            .body(ByteStream::new(SdkBody::from(write_buf.clone())))
                                            .part_number(part_number)
                                            .send().await
                                            .map_or_else(
                                                |e| {
                                                    RetryResult::Retry(
                                                        make_err!(
                                                            Code::Aborted,
                                                            "Failed to upload part {part_number} in ONTAP S3 store: {e:?}"
                                                        )
                                                    )
                                                },
                                                |mut response| {
                                                    RetryResult::Ok(
                                                        CompletedPartBuilder::default()
                                                            .set_e_tag(response.e_tag.take())
                                                            .part_number(part_number)
                                                            .build()
                                                    )
                                                }
                                            );
                                        Some((retry_result, write_buf))
                                    })
                                )
                            ).await
                            .map_err(|_| {
                                make_err!(
                                    Code::Internal,
                                    "Failed to send part to channel in ontap_s3_store"
                                )
                            })?;
                    }
                    Result::<_, Error>::Ok(())
                }
            ).fuse();

            let mut upload_futures = FuturesUnordered::new();
            let mut completed_parts = Vec::with_capacity(
                usize::try_from(cmp::min(
                    MAX_UPLOAD_PARTS as u64,
                    max_size / bytes_per_upload_part + 1,
                ))
                .err_tip(|| "Could not convert u64 to usize")?,
            );

            tokio::pin!(read_stream_fut);
            loop {
                if read_stream_fut.is_terminated() && rx.is_empty() && upload_futures.is_empty() {
                    break;
                }
                tokio::select! {
                    result = &mut read_stream_fut => result?,
                    Some(upload_result) = upload_futures.next() => completed_parts.push(upload_result?),
                    Some(fut) = rx.recv() => upload_futures.push(fut),
                }
            }

            completed_parts.sort_unstable_by_key(|part| part.part_number);

            self.retrier.retry(
                unfold(completed_parts, move |completed_parts| async move {
                    Some((
                        self.s3_client
                            .complete_multipart_upload()
                            .bucket(&self.bucket)
                            .key(s3_path)
                            .multipart_upload(
                                CompletedMultipartUploadBuilder::default()
                                    .set_parts(Some(completed_parts.clone()))
                                    .build()
                            )
                            .upload_id(upload_id)
                            .send().await
                            .map_or_else(
                                |e| {
                                    RetryResult::Retry(
                                        make_err!(
                                            Code::Aborted,
                                            "Failed to complete multipart upload in ONTAP S3 store: {e:?}"
                                        )
                                    )
                                },
                                |_| RetryResult::Ok(())
                            ),
                        completed_parts,
                    ))
                })
            ).await
        };

        upload_parts()
            .or_else(move |e| async move {
                Result::<(), _>::Err(e).merge(
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
                                    "Failed to abort multipart upload in ONTAP S3 store : {e:?}"
                                );
                                event!(Level::INFO, ?err, "Multipart upload error");
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
                .err_tip(|| "Failed to send zero EOF in ONTAP S3 store get_part")?;
            return Ok(());
        }

        let s3_path = &self.make_s3_path(&key);
        let end_read_byte = length
            .map_or(Some(None), |length| Some(offset.checked_add(length)))
            .err_tip(|| "Integer overflow protection triggered")?;

        self.retrier
            .retry(unfold(writer, move |writer| async move {
                let result = self.s3_client
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

                match result {
                    Ok(head_object_output) => {
                        let mut s3_in_stream = head_object_output.body;
                        let _bytes_sent = 0;

                        while let Some(maybe_bytes) = s3_in_stream.next().await {
                            match maybe_bytes {
                                Ok(bytes) => {
                                    if bytes.is_empty() {
                                        continue;
                                    }

                                    // Clone bytes before sending
                                    let bytes_clone = bytes.clone();

                                    // More robust sending mechanism
                                    match writer.send(bytes).await {
                                        Ok(()) => {
                                            let _ = bytes_clone.len();
                                        }
                                        Err(e) => {
                                            return Some((
                                                RetryResult::Err(make_err!(
                                                    Code::Aborted,
                                                    "Error sending bytes to consumer in ONTAP S3: {e}"
                                                )),
                                                writer,
                                            ));
                                        }
                                    }
                                }
                                Err(e) => {
                                    return Some((
                                        RetryResult::Retry(make_err!(
                                            Code::Aborted,
                                            "Bad bytestream element in ONTAP S3: {e}"
                                        )),
                                        writer,
                                    ));
                                }
                            }
                        }

                        // EOF handling
                        if let Err(e) = writer.send_eof() {
                            return Some((
                                RetryResult::Err(make_err!(
                                    Code::Aborted,
                                    "Failed to send EOF to consumer in ONTAP S3: {e}"
                                )),
                                writer,
                            ));
                        }

                        Some((RetryResult::Ok(()), writer))
                    }
                    Err(sdk_error) => {
                        // Clone sdk_error before moving
                        let error_description = format!("{sdk_error:?}");
                        match sdk_error.into_service_error() {
                            GetObjectError::NoSuchKey(e) => {
                                Some((
                                    RetryResult::Err(make_err!(
                                        Code::NotFound,
                                        "No such key in ONTAP S3: {e}"
                                    )),
                                    writer,
                                ))
                            }
                            _ => Some((
                                RetryResult::Retry(make_err!(
                                    Code::Unavailable,
                                    "Unhandled GetObjectError in ONTAP S3: {error_description}"
                                )),
                                writer,
                            )),
                        }
                    },
                }
            }))
            .await
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &dyn StoreDriver {
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
impl<I, NowFn> HealthStatusIndicator for OntapS3Store<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    fn get_name(&self) -> &'static str {
        "OntapS3Store"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}
