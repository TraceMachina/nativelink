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

use std::borrow::Cow;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use std::{cmp, env};

use async_trait::async_trait;
use aws_config::default_provider::credentials;
use aws_config::BehaviorVersion;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::operation::create_multipart_upload::CreateMultipartUploadOutput;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::primitives::{ByteStream, SdkBody};
use aws_sdk_s3::types::builders::{CompletedMultipartUploadBuilder, CompletedPartBuilder};
use aws_sdk_s3::Client;
use aws_smithy_runtime::client::http::hyper_014::HyperClientBuilder;
use futures::stream::{unfold, FuturesUnordered};
use futures::{try_join, FutureExt, StreamExt, TryStreamExt};
use hyper::client::connect::HttpConnector;
use hyper::service::Service;
use hyper::Uri;
use hyper_rustls::{HttpsConnector, MaybeHttpsStream};
use nativelink_error::{error_if, make_err, make_input_err, Code, Error, ResultExt};
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{default_health_status_indicator, HealthStatusIndicator};
use nativelink_util::retry::{Retrier, RetryResult};
use nativelink_util::store_trait::{Store, UploadSizeInfo};
use rand::rngs::OsRng;
use rand::Rng;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_stream::wrappers::ReceiverStream;
use tracing::info;

use crate::cas_utils::is_zero_digest;

// S3 parts cannot be smaller than this number. See:
// https://docs.aws.amazon.com/AmazonS3/latest/userguide/qfacts.html
const MIN_MULTIPART_SIZE: usize = 5 * 1024 * 1024; // 5mb.

// Default limit for concurrent part uploads per multipart upload.
// Note: If you change this, adjust the docs in the config.
const DEFAULT_MULTIPART_MAX_CONCURRENT_UPLOADS: usize = 10;

#[derive(Clone)]
pub struct TlsConnector {
    connector: HttpsConnector<HttpConnector>,
    retrier: Retrier,
}

impl TlsConnector {
    #[must_use]
    pub fn new(
        config: &nativelink_config::stores::S3Store,
        jitter_fn: Arc<dyn Fn(Duration) -> Duration + Send + Sync>,
    ) -> Self {
        let connector_with_roots = hyper_rustls::HttpsConnectorBuilder::new().with_webpki_roots();

        let connector_with_schemes = if config.insecure_allow_http {
            connector_with_roots.https_or_http()
        } else {
            connector_with_roots.https_only()
        };

        let connector = if config.disable_http2 {
            connector_with_schemes.enable_http1().build()
        } else {
            connector_with_schemes.enable_http1().enable_http2().build()
        };

        Self {
            connector,
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                jitter_fn,
                config.retry.to_owned(),
            ),
        }
    }

    async fn call_with_retry(&self, req: &Uri) -> Result<MaybeHttpsStream<TcpStream>, Error> {
        let retry_stream_fn = unfold(self.connector.clone(), move |mut connector| async move {
            match connector.call(req.clone()).await {
                Ok(stream) => Some((RetryResult::Ok(stream), connector)),
                Err(e) => Some((
                    RetryResult::Retry(make_err!(
                        Code::Unavailable,
                        "Failed to call S3 connector: {e:?}"
                    )),
                    connector,
                )),
            }
        });
        self.retrier.retry(retry_stream_fn).await
    }
}

impl Service<Uri> for TlsConnector {
    type Response = MaybeHttpsStream<TcpStream>;
    type Error = Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.connector
            .poll_ready(cx)
            .map_err(|e| make_err!(Code::Unavailable, "Failed poll in S3: {e}"))
    }

    fn call(&mut self, req: Uri) -> Self::Future {
        let connector_clone = self.clone();
        Box::pin(async move { connector_clone.call_with_retry(&req).await })
    }
}

pub struct S3Store {
    s3_client: Arc<Client>,
    bucket: String,
    key_prefix: String,
    retrier: Retrier,
    multipart_max_concurrent_uploads: usize,
}

impl S3Store {
    pub async fn new(config: &nativelink_config::stores::S3Store) -> Result<Self, Error> {
        let jitter_amt = config.retry.jitter;
        let jitter_fn = Arc::new(move |delay: Duration| {
            if jitter_amt == 0. {
                return delay;
            }
            let min = 1. - (jitter_amt / 2.);
            let max = 1. + (jitter_amt / 2.);
            delay.mul_f32(OsRng.gen_range(min..max))
        });
        let s3_client = {
            let http_client =
                HyperClientBuilder::new().build(TlsConnector::new(config, jitter_fn.clone()));
            let credential_provider = credentials::default_provider().await;
            let mut config_builder = aws_config::defaults(BehaviorVersion::v2023_11_09())
                .credentials_provider(credential_provider)
                .region(Region::new(Cow::Owned(config.region.clone())))
                .http_client(http_client);
            // TODO(allada) When aws-sdk supports this env variable we should be able
            // to remove this.
            // See: https://github.com/awslabs/aws-sdk-rust/issues/932
            if let Ok(endpoint_url) = env::var("AWS_ENDPOINT_URL") {
                config_builder = config_builder.endpoint_url(endpoint_url);
            }
            aws_sdk_s3::Client::new(&config_builder.load().await)
        };
        Self::new_with_client_and_jitter(config, s3_client, jitter_fn)
    }

    pub fn new_with_client_and_jitter(
        config: &nativelink_config::stores::S3Store,
        s3_client: Client,
        jitter_fn: Arc<dyn Fn(Duration) -> Duration + Send + Sync>,
    ) -> Result<Self, Error> {
        Ok(Self {
            s3_client: Arc::new(s3_client),
            bucket: config.bucket.to_string(),
            key_prefix: config.key_prefix.as_ref().unwrap_or(&String::new()).clone(),
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                jitter_fn,
                config.retry.to_owned(),
            ),
            multipart_max_concurrent_uploads: config
                .multipart_max_concurrent_uploads
                .map_or(DEFAULT_MULTIPART_MAX_CONCURRENT_UPLOADS, |v| v),
        })
    }

    fn make_s3_path(&self, digest: &DigestInfo) -> String {
        format!(
            "{}{}-{}",
            self.key_prefix,
            digest.hash_str(),
            digest.size_bytes
        )
    }

    async fn has(self: Pin<&Self>, digest: &DigestInfo) -> Result<Option<usize>, Error> {
        self.retrier
            .retry(unfold((), move |state| async move {
                let result = self
                    .s3_client
                    .head_object()
                    .bucket(&self.bucket)
                    .key(&self.make_s3_path(digest))
                    .send()
                    .await;

                match result {
                    Ok(head_object_output) => {
                        let Some(length) = head_object_output.content_length else {
                            return Some((RetryResult::Ok(None), state));
                        };
                        if length >= 0 {
                            return Some((RetryResult::Ok(Some(length as usize)), state));
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
impl Store for S3Store {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        digests
            .iter()
            .zip(results.iter_mut())
            .map(|(digest, result)| async move {
                // We need to do a special pass to ensure our zero digest exist.
                if is_zero_digest(digest) {
                    *result = Some(0);
                    return Ok::<_, Error>(());
                }
                *result = self.has(digest).await?;
                Ok::<_, Error>(())
            })
            .collect::<FuturesUnordered<_>>()
            .try_collect()
            .await?;
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        mut reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        let s3_path = &self.make_s3_path(&digest);

        let max_size = match upload_size {
            UploadSizeInfo::ExactSize(sz) | UploadSizeInfo::MaxSize(sz) => sz,
        };
        // NOTE(blaise.bruer) It might be more optimal to use a different
        // heuristic here, but for simplicity we use a hard coded value.
        // Anything going down this if-statement will have the advantage of only
        // 1 network request for the upload instead of minimum of 3 required for
        // multipart upload requests.
        if max_size < MIN_MULTIPART_SIZE {
            let (body, content_length) = if let UploadSizeInfo::ExactSize(sz) = upload_size {
                (
                    ByteStream::new(SdkBody::from(
                        reader
                            .consume(Some(sz))
                            .await
                            .err_tip(|| "Failed to take {sz} bytes from reader in S3")?,
                    )),
                    sz as i64,
                )
            } else {
                // Just in case, we want to capture the EOF, so +1.
                let write_buf = reader
                    .consume(Some(max_size + 1))
                    .await
                    .err_tip(|| "Failed to read file in upload to s3 in single chunk")?;
                error_if!(
                    write_buf.len() > max_size,
                    "More data than provided max_size in s3_store {}",
                    digest.hash_str()
                );
                let content_length = write_buf.len();
                (
                    ByteStream::new(SdkBody::from(write_buf)),
                    content_length as i64,
                )
            };

            return self
                .s3_client
                .put_object()
                .bucket(&self.bucket)
                .key(s3_path.clone())
                .content_length(content_length)
                .body(body)
                .send()
                .await
                .map_or_else(|e| Err(make_err!(Code::Internal, "{e:?}")), |_| Ok(()))
                .err_tip(|| "Failed to upload file to s3 in single chunk");
        }

        // S3 requires us to upload in parts if the size is greater than 5GB. The part size must be at least
        // 5mb and can have up to 10,000 parts.
        let bytes_per_upload_part =
            cmp::max(MIN_MULTIPART_SIZE, max_size / (MIN_MULTIPART_SIZE - 1));

        let response: CreateMultipartUploadOutput = self
            .s3_client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(s3_path)
            .send()
            .await
            .map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Failed to create multipart upload to s3: {e:?}"
                )
            })?;

        let upload_id = response
            .upload_id
            .err_tip(|| "Expected upload_id to be set by s3 response")?;

        let complete_result = {
            let mut part_number: i32 = 1;

            // This will ensure we only have `multipart_max_concurrent_uploads` * `bytes_per_upload_part`
            // bytes in memory at any given time waiting to be uploaded.
            let (tx, rx) = mpsc::channel(self.multipart_max_concurrent_uploads);
            let upload_id_clone = upload_id.clone();

            let read_stream_fut = async move {
                loop {
                    let write_buf = reader
                        .consume(Some(bytes_per_upload_part))
                        .await
                        .err_tip(|| "Failed to read chunk in s3_store")?;
                    if write_buf.is_empty() {
                        break; // Reached EOF.
                    }

                    let upload_fut = self
                        .s3_client
                        .upload_part()
                        .bucket(self.bucket.clone())
                        .key(s3_path)
                        .upload_id(upload_id_clone.clone())
                        .body(ByteStream::new(SdkBody::from(write_buf)))
                        .part_number(part_number)
                        .send()
                        .map(move |result| {
                            result.map_or_else(
                                |e| {
                                    Err(make_err!(
                                        Code::Internal,
                                        "Failed to upload part {part_number} in S3 store: {e:?}"
                                    ))
                                },
                                |mut response| {
                                    Ok(CompletedPartBuilder::default()
                                        // Only set an entity tag if it exists. This saves
                                        // 13 bytes per part on the final request if it can
                                        // omit the `<ETAG><ETAG/>` string.
                                        .set_e_tag(response.e_tag.take())
                                        .part_number(part_number)
                                        .build())
                                },
                            )
                        });
                    tx.send(upload_fut).await.map_err(|e| {
                        make_err!(
                            Code::Internal,
                            "Could not send across mpsc for {part_number} in S3 store: {e:?}"
                        )
                    })?;
                    part_number += 1;
                }
                Ok(())
            };

            // This will ensure we only have `multipart_max_concurrent_uploads` requests in flight
            // at any given time.
            let completed_parts_fut = ReceiverStream::new(rx)
                .buffer_unordered(self.multipart_max_concurrent_uploads)
                .try_collect::<Vec<_>>();

            // Wait for the entire stream to be read and all parts to be uploaded.
            let ((), mut completed_parts) =
                try_join!(read_stream_fut, completed_parts_fut).err_tip(|| "In s3 store")?;

            // Even though the spec does not require parts to be sorted by number, we do it just in case
            // there's an S3 implementation that requires it.
            completed_parts.sort_unstable_by_key(|part| part.part_number);

            self.s3_client
                .complete_multipart_upload()
                .bucket(&self.bucket)
                .key(s3_path.clone())
                .multipart_upload(
                    CompletedMultipartUploadBuilder::default()
                        .set_parts(Some(completed_parts))
                        .build(),
                )
                .upload_id(upload_id.clone())
                .send()
                .await
                .map_or_else(|e| Err(make_err!(Code::Internal, "{e:?}")), |_| Ok(()))
                .err_tip(|| "Failed to complete multipart to s3")?;
            Ok(())
        };
        if complete_result.is_err() {
            let abort_result = self
                .s3_client
                .abort_multipart_upload()
                .bucket(&self.bucket)
                .key(s3_path.clone())
                .upload_id(upload_id.clone())
                .send()
                .await;
            if let Err(err) = abort_result {
                info!(
                    "\x1b[0;31ms3_store\x1b[0m: Failed to abort_multipart_upload: {:?}",
                    err
                );
            }
        }
        complete_result
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        if is_zero_digest(&digest) {
            writer
                .send_eof()
                .err_tip(|| "Failed to send zero EOF in filesystem store get_part_ref")?;
            return Ok(());
        }

        let s3_path = &self.make_s3_path(&digest);
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
                        offset + writer.get_bytes_written() as usize,
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
                                // Ignore possible EOF. Different implimentations of S3 may or may not
                                // send EOF this way.
                                continue;
                            }
                            if let Err(e) = writer.send(bytes).await {
                                return Some((
                                    RetryResult::Err(make_input_err!(
                                        "Error sending bytes to consumer in S3: {e}"
                                    )),
                                    writer,
                                ));
                            }
                        }
                        Err(e) => {
                            return Some((
                                RetryResult::Retry(make_input_err!(
                                    "Bad bytestream element in S3: {e}"
                                )),
                                writer,
                            ));
                        }
                    }
                }
                if let Err(e) = writer.send_eof() {
                    return Some((
                        RetryResult::Err(make_input_err!(
                            "Failed to send EOF to consumer in S3: {e}"
                        )),
                        writer,
                    ));
                }
                Some((RetryResult::Ok(()), writer))
            }))
            .await
    }

    fn inner_store(&self, _digest: Option<DigestInfo>) -> &'_ dyn Store {
        self
    }

    fn inner_store_arc(self: Arc<Self>, _digest: Option<DigestInfo>) -> Arc<dyn Store> {
        self
    }

    fn as_any<'a>(&'a self) -> &'a (dyn std::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Sync + Send + 'static> {
        self
    }
}

default_health_status_indicator!(S3Store);
