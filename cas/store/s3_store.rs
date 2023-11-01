// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

use std::cmp;
use std::future::Future;
use std::marker::Send;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use aws_sdk_s3::operation::create_multipart_upload::CreateMultipartUploadOutput;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::operation::upload_part::UploadPartOutput;
use aws_sdk_s3::primitives::{ByteStream, SdkBody};
use aws_sdk_s3::types::builders::{CompletedMultipartUploadBuilder, CompletedPartBuilder};
// use aws_sdk_s3::types::CompletedPart;
use aws_sdk_s3::Client;
use aws_smithy_runtime::client::http::hyper_014::HyperClientBuilder;
use buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use common::{log, DigestInfo};
use error::{error_if, make_err, make_input_err, Code, Error, ResultExt};
use futures::stream::{unfold, FuturesUnordered};
use futures::{FutureExt, TryStreamExt};
use hyper::client::connect::HttpConnector;
use hyper::service::Service;
use hyper::Uri;
use hyper_rustls::{HttpsConnector, MaybeHttpsStream};
use rand::rngs::OsRng;
use rand::Rng;
use retry::{ExponentialBackoff, Retrier, RetryResult};
use tokio::net::TcpStream;
use tokio::time::sleep;
use traits::{StoreTrait, UploadSizeInfo};

// S3 parts cannot be smaller than this number. See:
// https://docs.aws.amazon.com/AmazonS3/latest/userguide/qfacts.html
const MIN_MULTIPART_SIZE: usize = 5 * 1024 * 1024; // 5mb.

// Default configuration for the connector retry mechanism.
// Note: If you change this, adjust the docs in the config.
const DEFAULT_CONNECTOR_MAX_RETRIES: usize = 10;
const DEFAULT_CONNECTOR_RETRY_DELAY_SECS: u64 = 1;

// Default limit for concurrent part uploads per multipart upload.
// Note: If you change this, adjust the docs in the config.
const DEFAULT_MULTIPART_MAX_CONCURRENT_UPLOADS: usize = 10;

#[derive(Clone)]
pub struct TlsConnector {
    connector: HttpsConnector<HttpConnector>,
    connector_max_retries: usize,
    connector_retry_delay_secs: Duration,
}

impl TlsConnector {
    #[must_use]
    pub fn new(config: &config::stores::S3Store) -> Self {
        let connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_only()
            .enable_http2()
            .build();

        let connector_max_retries = config
            .connector_max_retries
            .map_or(DEFAULT_CONNECTOR_MAX_RETRIES, |retries| retries);

        let connector_retry_delay_secs = config.connector_retry_delay_secs.map_or_else(
            || Duration::from_secs(DEFAULT_CONNECTOR_RETRY_DELAY_SECS),
            Duration::from_secs,
        );

        Self {
            connector,
            connector_max_retries,
            connector_retry_delay_secs,
        }
    }

    async fn simple_retry(&mut self, req: &Uri) -> Result<MaybeHttpsStream<TcpStream>, Error> {
        let mut retries = 0;
        let max_retries = self.connector_max_retries;
        // This retry logic runs separate from the stream-specific logic and
        // triggers when the entire connection gets interrupted.
        loop {
            match self.connector.call(req.clone()).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    if retries < max_retries {
                        // S3 sometimes randomly drops the connection and
                        // becomes unavailable. This kind of 404 error should be
                        // retried as it usually resolves itself after a few
                        // seconds.
                        log::warn!("S3 connection temporarily dropped. Retry: {retries}/{max_retries}");
                        retries += 1;
                        sleep(self.connector_retry_delay_secs).await;
                    } else {
                        return Err(make_err!(
                            Code::Unavailable,
                            "Future in S3 tls connector failed after {max_retries} retries: {e}"
                        ));
                    }
                }
            }
        }
    }
}

impl Service<Uri> for TlsConnector {
    type Response = MaybeHttpsStream<TcpStream>;
    type Error = Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.connector
            .poll_ready(cx)
            .map_err(|e| make_err!(Code::Unavailable, "Failed poll in S3: {e}"))
    }

    fn call(&mut self, req: Uri) -> Self::Future {
        let mut connector_clone = self.clone();

        Box::pin(async move { connector_clone.simple_retry(&req).await })
    }
}

pub struct S3Store {
    s3_client: Arc<Client>,
    bucket: String,
    key_prefix: String,
    jitter_fn: Box<dyn Fn(Duration) -> Duration + Send + Sync>,
    retry: config::stores::Retry,
    retrier: Retrier,
    multipart_max_concurrent_uploads: usize,
}

impl S3Store {
    pub async fn new(config: &config::stores::S3Store) -> Result<Self, Error> {
        let s3_client = {
            let http_client = HyperClientBuilder::new().build(TlsConnector::new(config));
            let shared_config = aws_config::from_env().http_client(http_client).load().await;
            aws_sdk_s3::Client::new(&shared_config)
        };
        let jitter_amt = config.retry.jitter;
        Self::new_with_client_and_jitter(
            config,
            s3_client,
            Box::new(move |delay: Duration| {
                if jitter_amt == 0. {
                    return delay;
                }
                let min = 1. - (jitter_amt / 2.);
                let max = 1. + (jitter_amt / 2.);
                delay.mul_f32(OsRng.gen_range(min..max))
            }),
        )
    }

    pub fn new_with_client_and_jitter(
        config: &config::stores::S3Store,
        s3_client: Client,
        jitter_fn: Box<dyn Fn(Duration) -> Duration + Send + Sync>,
    ) -> Result<Self, Error> {
        Ok(Self {
            s3_client: Arc::new(s3_client),
            bucket: config.bucket.to_string(),
            key_prefix: config.key_prefix.as_ref().unwrap_or(&String::new()).clone(),
            jitter_fn,
            retry: config.retry.clone(),
            retrier: Retrier::new(Box::new(|duration| Box::pin(sleep(duration)))),
            multipart_max_concurrent_uploads: config
                .multipart_max_concurrent_uploads
                .map_or(DEFAULT_MULTIPART_MAX_CONCURRENT_UPLOADS, |v| v),
        })
    }

    fn make_s3_path(&self, digest: &DigestInfo) -> String {
        format!("{}{}-{}", self.key_prefix, digest.hash_str(), digest.size_bytes)
    }

    async fn has(self: Pin<&Self>, digest: &DigestInfo) -> Result<Option<usize>, Error> {
        let retry_config = ExponentialBackoff::new(Duration::from_millis(self.retry.delay as u64))
            .map(|d| (self.jitter_fn)(d))
            .take(self.retry.max_retries); // Remember this is number of retries, so will run max_retries + 1.
        self.retrier
            .retry(
                retry_config,
                unfold((), move |state| async move {
                    let result = self
                        .s3_client
                        .head_object()
                        .bucket(&self.bucket)
                        .key(&self.make_s3_path(digest))
                        .send()
                        .await;

                    match result {
                        Ok(head_object_output) => {
                            let sz = head_object_output.content_length;
                            Some((RetryResult::Ok(Some(sz as usize)), state))
                        }
                        Err(sdk_error) => match sdk_error.into_service_error() {
                            HeadObjectError::NotFound(_) => Some((RetryResult::Ok(None), state)),
                            HeadObjectError::Unhandled(e) => Some((
                                RetryResult::Retry(make_err!(
                                    Code::Unavailable,
                                    "Unhandled HeadObjectError in S3: {e:?}, retries: {}",
                                    self.retry.max_retries + 1,
                                )),
                                state,
                            )),
                            other => Some((
                                RetryResult::Err(make_err!(
                                    Code::Unavailable,
                                    "Unkown error getting head_object in S3: {other:?}, retries: {}",
                                    self.retry.max_retries + 1,
                                )),
                                state,
                            )),
                        },
                    }
                }),
            )
            .await
    }
}

#[async_trait]
impl StoreTrait for S3Store {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        digests
            .iter()
            .zip(results.iter_mut())
            .map(|(digest, result)| async move {
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
            let (body, content_length) = {
                let write_buf = reader
                    .take(max_size + 1) // Just in case, we want to capture the EOF, so +1.
                    .await
                    .err_tip(|| "Failed to read file in upload to s3 in single chunk")?;
                error_if!(
                    write_buf.len() > max_size,
                    "More data than provided max_size in s3_store {}",
                    digest.hash_str()
                );
                let content_length = write_buf.len();
                (ByteStream::new(SdkBody::from(write_buf)), content_length as i64)
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
                .map_or_else(|e| Err(make_err!(Code::Unknown, "{:?}", e)), |_| Ok(()))
                .err_tip(|| "Failed to upload file to s3 in single chunk");
        }

        // S3 requires us to upload in parts if the size is greater than 5GB. The part size must be at least
        // 5mb and can have up to 10,000 parts.
        let bytes_per_upload_part = cmp::max(MIN_MULTIPART_SIZE, max_size / (MIN_MULTIPART_SIZE - 1));

        let response: CreateMultipartUploadOutput = self
            .s3_client
            .create_multipart_upload()
            .bucket(&self.bucket)
            .key(s3_path.clone())
            .send()
            .await
            .map_err(|e| make_err!(Code::Unknown, "Failed to create multipart upload to s3: {:?}", e))?;

        let upload_id = response
            .upload_id
            .err_tip(|| "Expected upload_id to be set by s3 response")?;

        let complete_result = {
            let mut part_number: i32 = 1;

            // We might end up with +1 capacity units than needed, but that is
            // the worst case.
            let mut completed_parts = Vec::with_capacity((max_size / bytes_per_upload_part) + 1);
            // let mut completed_part_futs = Vec::with_capacity(completed_parts.len());

            let max_concurrent_requests = self.multipart_max_concurrent_uploads;

            let mut futures = futures::stream::FuturesUnordered::new();

            let part_from_response = |result: Result<UploadPartOutput, _>, part_idx: i32| {
                match result {
                    Ok(mut response) => {
                        Ok(CompletedPartBuilder::default()
                            // Only set an entity tag if it exists. This saves
                            // 13 bytes per part on the final request if it can
                            // omit the `<ETAG><ETAG/>` string.
                            .set_e_tag(response.e_tag.take())
                            .part_number(part_idx)
                            .build())
                    }
                    Err(e) => Err(make_err!(Code::Unknown, "Failed to upload part in S3: {e:?}")),
                }
            };

            // Send up to `max_concurrent_requests` parts concurrently in an
            // unordered manner. This way we interleave requests with response
            // handling to process responses as soon as possible. The
            // CompletedMultipartUploadBuilder keeps track of the response
            // order and lets S3 know about it.
            loop {
                if futures.len() < max_concurrent_requests {
                    let write_buf = reader
                        .take(bytes_per_upload_part)
                        .await
                        .err_tip(|| "Failed to read chunk in s3_store")?;
                    if write_buf.is_empty() {
                        break; // Reached EOF.
                    }

                    let current_part_number = part_number;
                    let response_fut = self
                        .s3_client
                        .upload_part()
                        .bucket(self.bucket.clone())
                        .key(s3_path)
                        .upload_id(upload_id.clone())
                        .body(ByteStream::new(SdkBody::from(write_buf)))
                        .part_number(part_number)
                        .send()
                        .map(move |result| (result, current_part_number));
                    futures.push(response_fut);
                    part_number += 1;
                }
                {
                    use futures::StreamExt;
                    if let Some((result, part_num)) = futures.next().await {
                        completed_parts.push(part_from_response(result, part_num)?);
                    }
                }
            }

            // Await remaining results.
            {
                use futures::StreamExt;
                while let Some((result, part_num)) = futures.next().await {
                    completed_parts.push(part_from_response(result, part_num)?);
                }
            }

            let completed_parts = CompletedMultipartUploadBuilder::default()
                .set_parts(Some(completed_parts))
                .build();

            self.s3_client
                .complete_multipart_upload()
                .bucket(&self.bucket)
                .key(s3_path.clone())
                .multipart_upload(completed_parts)
                .upload_id(upload_id.clone())
                .send()
                .await
                .map_or_else(|e| Err(make_err!(Code::Unknown, "{:?}", e)), |_| Ok(()))
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
                log::info!("\x1b[0;31ms3_store\x1b[0m: Failed to abort_multipart_upload: {:?}", err);
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
        let s3_path = &self.make_s3_path(&digest);
        let end_read_byte = length
            .map_or(Some(None), |length| Some(offset.checked_add(length)))
            .err_tip(|| "Integer overflow protection triggered")?;

        let retry_config = ExponentialBackoff::new(Duration::from_millis(self.retry.delay as u64))
            .map(|d| (self.jitter_fn)(d))
            .take(self.retry.max_retries); // Remember this is number of retries, so will run max_retries + 1.

        // S3 drops connections when a stream is done. This means that we can't
        // run the EOF error check. It's safe to disable it since S3 can be
        // trusted to handle incomplete data properly.
        writer.set_ignore_eof();

        self.retrier
            .retry(
                retry_config,
                unfold(writer, move |writer| async move {
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
                                    RetryResult::Err(make_err!(Code::NotFound, "No such key in S3: {e}")),
                                    writer,
                                ));
                            }
                            GetObjectError::Unhandled(e) => {
                                return Some((
                                    RetryResult::Retry(make_err!(
                                        Code::Unavailable,
                                        "Unhandled GetObjectError in S3: {e:?}, retries: {}",
                                        self.retry.max_retries + 1,
                                    )),
                                    writer,
                                ));
                            }
                            other => {
                                return Some((
                                    RetryResult::Err(make_err!(
                                        Code::Unavailable,
                                        "Unkown error getting result in S3: {other:?}, retries: {}",
                                        self.retry.max_retries + 1,
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
                                if let Err(e) = writer.send(bytes).await {
                                    return Some((
                                        RetryResult::Err(make_input_err!("Error sending bytes to consumer in S3: {e}")),
                                        writer,
                                    ));
                                }
                            }
                            Err(e) => {
                                // Handle the error, log it, or break the loop
                                return Some((
                                    RetryResult::Err(make_input_err!("Bad bytestream element in S3: {e}")),
                                    writer,
                                ));
                            }
                        }
                    }
                    if let Err(e) = writer.send_eof().await {
                        return Some((
                            RetryResult::Err(make_input_err!("Failed to send EOF to consumer in S3: {e}")),
                            writer,
                        ));
                    }
                    Some((RetryResult::Ok(()), writer))
                }),
            )
            .await
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
