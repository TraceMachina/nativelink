// Copyright 2021 Nathan (Blaise) Bruer.  All rights reserved.

use std::cmp;
use std::io::Cursor;
use std::marker::Send;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::future::{try_join_all, FutureExt};
use futures::stream::unfold;
use http::status::StatusCode;
use lease::{Lease, Pool as ObjectPool};
use rand::{rngs::OsRng, Rng};
use rusoto_core::{region::Region, ByteStream, RusotoError};
use rusoto_s3::{
    AbortMultipartUploadRequest, CompleteMultipartUploadRequest, CompletedMultipartUpload, CompletedPart,
    CreateMultipartUploadRequest, GetObjectError, GetObjectRequest, HeadObjectError, HeadObjectRequest,
    PutObjectRequest, S3Client, UploadPartRequest, S3,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::sleep;
use tokio_util::io::ReaderStream;

use common::{log, DigestInfo, JoinHandleDropGuard};
use config;
use error::{make_err, make_input_err, Code, Error, ResultExt};
use retry::{ExponentialBackoff, Retrier, RetryResult};
use traits::{ResultFuture, StoreTrait, UploadSizeInfo};
use write_counter::WriteCounter;

// S3 parts cannot be smaller than this number. See:
// https://docs.aws.amazon.com/AmazonS3/latest/userguide/qfacts.html
const MIN_MULTIPART_SIZE: usize = 5 * 1024 * 1024; // 5mb.

// Size for the large vector pool if not specified.
const DEFAULT_BUFFER_POOL_SIZE: usize = 50;

type ReaderType = Box<dyn AsyncRead + Send + Unpin + Sync + 'static>;

fn should_retry<T, E>(result: Result<T, RusotoError<E>>) -> RetryResult<T>
where
    RusotoError<E>: std::fmt::Display,
{
    match result {
        // Object found in S3.
        Ok(v) => RetryResult::Ok(v), // Success

        // Timeout-like errors. This can retry.
        Err(RusotoError::HttpDispatch(e)) => RetryResult::Retry(make_err!(Code::Unavailable, "{}", e.to_string())),

        // HTTP-level errors. Sometimes can retry.
        Err(RusotoError::Unknown(e)) => match e.status {
            StatusCode::NOT_FOUND => RetryResult::Err(make_err!(Code::NotFound, "{}", e.status.to_string())),
            StatusCode::INTERNAL_SERVER_ERROR => {
                RetryResult::Retry(make_err!(Code::Unavailable, "{}", e.status.to_string()))
            }
            StatusCode::SERVICE_UNAVAILABLE => {
                RetryResult::Retry(make_err!(Code::Unavailable, "{}", e.status.to_string()))
            }
            StatusCode::CONFLICT => RetryResult::Retry(make_err!(Code::Unavailable, "{}", e.status.to_string())),
            other_err => RetryResult::Err(make_err!(Code::Unavailable, "{}", other_err.to_string())),
        },

        // Other errors (eg. Validation, Parse, Credentials). Never retry.
        Err(other_err) => RetryResult::Err(make_err!(Code::Unavailable, "{}", other_err.to_string())),
    }
}

pub struct S3Store {
    s3_client: Arc<S3Client>,
    bucket: String,
    key_prefix: String,
    jitter_fn: Box<dyn Fn(Duration) -> Duration + Send + Sync>,
    retry: config::backends::Retry,
    retrier: Retrier,
    large_vec_pool: ObjectPool<Vec<u8>>,
}

impl S3Store {
    pub fn new(config: &config::backends::S3Store) -> Result<Self, Error> {
        let jitter_amt = config.retry.jitter;
        S3Store::new_with_client_and_jitter(
            config,
            S3Client::new(
                config
                    .region
                    .parse::<Region>()
                    .map_err(|e| make_input_err!("{}", e.to_string()))?,
            ),
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
        config: &config::backends::S3Store,
        s3_client: S3Client,
        jitter_fn: Box<dyn Fn(Duration) -> Duration + Send + Sync>,
    ) -> Result<Self, Error> {
        let mut buffer_pool_size = config.buffer_pool_size;
        if buffer_pool_size == 0 {
            buffer_pool_size = DEFAULT_BUFFER_POOL_SIZE;
        }
        Ok(S3Store {
            s3_client: Arc::new(s3_client),
            bucket: config.bucket.to_owned(),
            key_prefix: config.key_prefix.as_ref().unwrap_or(&"".to_string()).to_owned(),
            jitter_fn: jitter_fn,
            retry: config.retry.to_owned(),
            retrier: Retrier::new(Box::new(|duration| Box::pin(sleep(duration)))),
            large_vec_pool: ObjectPool::new(buffer_pool_size, || Vec::with_capacity(MIN_MULTIPART_SIZE)),
        })
    }

    async fn get_large_vec(self: Pin<&Self>) -> Lease<Vec<u8>> {
        let mut write_data = self.large_vec_pool.get_async().await;
        write_data.clear();
        write_data.shrink_to(MIN_MULTIPART_SIZE);
        return write_data;
    }

    fn make_s3_path(&self, digest: &DigestInfo) -> String {
        format!("{}{}-{}", self.key_prefix, digest.str(), digest.size_bytes)
    }
}

#[async_trait]
impl StoreTrait for S3Store {
    fn has<'a>(self: Pin<&'a Self>, digest: DigestInfo) -> ResultFuture<'a, bool> {
        Box::pin(async move {
            let retry_config = ExponentialBackoff::new(Duration::from_millis(self.retry.delay as u64))
                .map(|d| (self.jitter_fn)(d))
                .take(self.retry.max_retries); // Remember this is number of retries, so will run max_retries + 1.
            let s3_path = &self.make_s3_path(&digest);
            self.retrier
                .retry(
                    retry_config,
                    unfold((), move |state| async move {
                        let head_req = HeadObjectRequest {
                            bucket: self.bucket.to_owned(),
                            key: s3_path.to_owned(),
                            ..Default::default()
                        };

                        let result = self.s3_client.head_object(head_req).await;
                        if let Err(RusotoError::Service(HeadObjectError::NoSuchKey(_))) = &result {
                            return Some((RetryResult::Ok(false), state));
                        }

                        match should_retry(result) {
                            RetryResult::Ok(_) => Some((RetryResult::Ok(true), state)),
                            RetryResult::Err(err) => {
                                if err.code == Code::NotFound {
                                    return Some((RetryResult::Ok(false), state));
                                }
                                Some((RetryResult::Err(err), state))
                            }
                            RetryResult::Retry(err) => Some((
                                RetryResult::Retry(make_err!(
                                    Code::Unavailable,
                                    "Error attempting to load s3 result, retried {} times. Error: {}",
                                    self.retry.max_retries + 1,
                                    err,
                                )),
                                state,
                            )),
                        }
                    }),
                )
                .await
        })
    }

    fn update<'a>(
        self: Pin<&'a Self>,
        digest: DigestInfo,
        mut reader: ReaderType,
        upload_size: UploadSizeInfo,
    ) -> ResultFuture<'a, ()> {
        Box::pin(async move {
            let s3_path = &self.make_s3_path(&digest);

            let max_size = match upload_size {
                UploadSizeInfo::ExactSize(sz) => sz,
                UploadSizeInfo::MaxSize(sz) => sz,
            };
            // NOTE(blaise.bruer) It might be more optimal to use a different heuristic here, but for
            // simplicity we use a hard codded value. Anything going down this if-statement will have
            // the advantage of only 1 network request for the upload instead of minimum of 3 required
            // for multipart upload requests.
            if max_size < MIN_MULTIPART_SIZE {
                let (reader, content_length) = if let UploadSizeInfo::ExactSize(sz) = upload_size {
                    (reader, Some(sz as i64))
                } else {
                    let mut write_buf = self.get_large_vec().await;
                    reader
                        .take(max_size as u64)
                        .read_to_end(&mut write_buf)
                        .await
                        .err_tip(|| "Failed to read file in upload to s3 in single chunk")?;
                    let content_length = write_buf.len();
                    (
                        Box::new(Cursor::new(write_buf)) as ReaderType,
                        Some(content_length as i64),
                    )
                };

                let put_object_request = PutObjectRequest {
                    bucket: self.bucket.to_owned(),
                    key: s3_path.to_owned(),
                    content_length,
                    body: Some(ByteStream::new(ReaderStream::new(reader))),
                    ..Default::default()
                };
                return self
                    .s3_client
                    .put_object(put_object_request)
                    .await
                    .map_or_else(|e| Err(make_err!(Code::Unknown, "{:?}", e)), |_| Ok(()))
                    .err_tip(|| "Failed to upload file to s3 in single chunk");
            }

            // S3 requires us to upload in parts if the size is greater than 5GB. The part size must be at least
            // 5mb and can have up to 10,000 parts.
            let bytes_per_upload_part = cmp::max(MIN_MULTIPART_SIZE, max_size / (MIN_MULTIPART_SIZE - 1));

            let response = self
                .s3_client
                .create_multipart_upload(CreateMultipartUploadRequest {
                    bucket: self.bucket.to_owned(),
                    key: s3_path.to_owned(),
                    ..Default::default()
                })
                .await
                .map_err(|e| make_err!(Code::Unknown, "Failed to create multipart upload to s3: {:?}", e))?;

            let upload_id = response
                .upload_id
                .err_tip(|| "Expected upload_id to be set by s3 response")?;

            let complete_result = {
                let mut part_number: i64 = 1;

                // We might end up with +1 capacity units than needed, but that is the worst case.
                let mut completed_part_futs = Vec::with_capacity((max_size / bytes_per_upload_part) + 1);
                loop {
                    let mut write_buf = self.get_large_vec().await;
                    let mut take = reader.take(bytes_per_upload_part as u64);
                    take.read_to_end(&mut write_buf)
                        .await
                        .err_tip(|| "Failed to read chunk in s3_store")?;
                    reader = take.into_inner();
                    if write_buf.len() == 0 {
                        break; // Reached EOF.
                    }

                    let content_length = Some(write_buf.len() as i64);
                    let body = Some(ByteStream::new(ReaderStream::new(Cursor::new(write_buf))));
                    let request = UploadPartRequest {
                        bucket: self.bucket.to_owned(),
                        key: s3_path.to_owned(),
                        content_length,
                        body,
                        part_number,
                        upload_id: upload_id.clone(),
                        ..Default::default()
                    };

                    let s3_client = self.s3_client.clone();
                    completed_part_futs.push(
                        JoinHandleDropGuard::new(tokio::spawn(async move {
                            let part_number = request.part_number;
                            let mut response = s3_client
                                .upload_part(request)
                                .await
                                .map_err(|e| make_err!(Code::Unknown, "Failed to upload part: {:?}", e))?;
                            let e_tag = response.e_tag.take();
                            // Double down to ensure our Lease<Vec<u8>> is freed up and returned to pool.
                            drop(response);
                            Result::<CompletedPart, Error>::Ok(CompletedPart {
                                e_tag,
                                part_number: Some(part_number),
                            })
                        }))
                        .map(|r| match r.err_tip(|| "Failed to run s3 upload spawn") {
                            Ok(r2) => r2.err_tip(|| "S3 upload chunk failure"),
                            Err(e) => Err(e),
                        }),
                    );
                    part_number += 1;
                }

                let completed_parts = try_join_all(completed_part_futs).await?;
                self.s3_client
                    .complete_multipart_upload(CompleteMultipartUploadRequest {
                        bucket: self.bucket.to_owned(),
                        key: s3_path.to_owned(),
                        upload_id: upload_id.clone(),
                        multipart_upload: Some(CompletedMultipartUpload {
                            parts: Some(completed_parts),
                        }),
                        ..Default::default()
                    })
                    .await
                    .map_or_else(|e| Err(make_err!(Code::Unknown, "{:?}", e)), |_| Ok(()))
                    .err_tip(|| "Failed to complete multipart to s3")?;
                Ok(())
            };
            if complete_result.is_err() {
                let abort_result = self
                    .s3_client
                    .abort_multipart_upload(AbortMultipartUploadRequest {
                        bucket: self.bucket.to_owned(),
                        key: s3_path.to_owned(),
                        upload_id: upload_id.clone(),
                        ..Default::default()
                    })
                    .await;
                if let Err(err) = abort_result {
                    log::info!("\x1b[0;31ms3_store\x1b[0m: Failed to abort_multipart_upload: {:?}", err);
                }
            }
            complete_result
        })
    }

    fn get_part<'a>(
        self: Pin<&'a Self>,
        digest: DigestInfo,
        writer: &'a mut (dyn AsyncWrite + Send + Unpin + Sync),
        offset: usize,
        length: Option<usize>,
    ) -> ResultFuture<'a, ()> {
        Box::pin(async move {
            let s3_path = &self.make_s3_path(&digest);
            let end_read_byte = length
                .map_or(Some(None), |length| Some(offset.checked_add(length)))
                .err_tip(|| "Integer overflow protection triggered")?;

            let retry_config = ExponentialBackoff::new(Duration::from_millis(self.retry.delay as u64))
                .map(|d| (self.jitter_fn)(d))
                .take(self.retry.max_retries); // Remember this is number of retries, so will run max_retries + 1.

            self.retrier
                .retry(
                    retry_config,
                    unfold(WriteCounter::new(writer), move |mut write_counter| async move {
                        let result = self
                            .s3_client
                            .get_object(GetObjectRequest {
                                bucket: self.bucket.to_owned(),
                                key: s3_path.to_owned(),
                                range: Some(format!(
                                    "bytes={}-{}",
                                    offset + write_counter.get_bytes_written() as usize,
                                    end_read_byte.map_or_else(|| "".to_string(), |v| v.to_string())
                                )),
                                ..Default::default()
                            })
                            .await;

                        if let Err(RusotoError::Service(GetObjectError::NoSuchKey(err))) = &result {
                            return Some((
                                RetryResult::Err(make_err!(Code::NotFound, "File not found in S3: {:?}", err)),
                                write_counter,
                            ));
                        }

                        let s3_in_stream = match should_retry(result) {
                            RetryResult::Ok(get_object_output) => {
                                let body_result = get_object_output
                                    .body
                                    .err_tip(|| "Expected body to be set in s3 get request");
                                match body_result {
                                    Ok(s3_in_stream) => s3_in_stream,
                                    Err(err) => {
                                        return Some((
                                            RetryResult::Err(make_err!(
                                                Code::Unavailable,
                                                "Error attempting to get s3 result. This is not a retryable error: {}",
                                                err
                                            )),
                                            write_counter,
                                        ));
                                    }
                                }
                            }
                            RetryResult::Err(err) => {
                                return Some((RetryResult::Err(err), write_counter));
                            }
                            RetryResult::Retry(err) => {
                                return Some((
                                    RetryResult::Retry(make_err!(
                                        Code::Unavailable,
                                        "Error attempting to get s3 result, retried {} times. Error: {}",
                                        self.retry.max_retries + 1,
                                        err,
                                    )),
                                    write_counter,
                                ));
                            }
                        };

                        // Copy data from s3 input stream to the writer stream.
                        let result = tokio::io::copy(&mut s3_in_stream.into_async_read(), &mut write_counter).await;

                        // We may want to retry, but only if the pipe was broken.
                        if let Err(e) = result {
                            if !write_counter.did_fail() && e.kind() == std::io::ErrorKind::BrokenPipe {
                                return Some((RetryResult::Retry(e.into()), write_counter));
                            }
                            return Some((RetryResult::Err(e.into()), write_counter));
                        }

                        let _ = write_counter.write(&[]).await; // At this point we really only care about if shutdown() fails.
                        let shutdown_result = write_counter
                            .shutdown()
                            .await
                            .err_tip(|| "Failed to shutdown write stream in S3Store::get_part");
                        if let Err(e) = shutdown_result {
                            return Some((RetryResult::Err(make_err!(Code::Unavailable, "{}", e)), write_counter));
                        }

                        Some((RetryResult::Ok(()), write_counter))
                    }),
                )
                .await
        })
    }
}
