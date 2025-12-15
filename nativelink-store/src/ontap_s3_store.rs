// Copyright 2025 The NativeLink Authors. All rights reserved.
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

use core::cmp;
use core::pin::Pin;
use core::time::Duration;
use std::borrow::Cow;
use std::fs::File;
use std::io::BufReader;
use std::sync::Arc;

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_config::default_provider::credentials::DefaultCredentialsChain;
use aws_config::provider_config::ProviderConfig;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::Region;
use aws_sdk_s3::operation::create_multipart_upload::CreateMultipartUploadOutput;
use aws_sdk_s3::operation::get_object::GetObjectError;
use aws_sdk_s3::operation::head_object::HeadObjectError;
use aws_sdk_s3::primitives::{ByteStream, SdkBody};
use aws_sdk_s3::types::builders::{CompletedMultipartUploadBuilder, CompletedPartBuilder};
use base64::Engine;
use base64::prelude::BASE64_STANDARD_NO_PAD;
use bytes::BytesMut;
use futures::future::{Either, FusedFuture};
use futures::stream::{FuturesUnordered, unfold};
use futures::{FutureExt, StreamExt, TryFutureExt, TryStreamExt};
use hyper_rustls::ConfigBuilderExt;
use nativelink_config::stores::ExperimentalOntapS3Spec;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair,
};
use nativelink_util::health_utils::{HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::retry::{Retrier, RetryResult};
use nativelink_util::store_trait::{RemoveItemCallback, StoreDriver, StoreKey, UploadSizeInfo};
use parking_lot::Mutex;
use rustls::{ClientConfig, RootCertStore};
use rustls_pki_types::CertificateDer;
use rustls_pki_types::pem::PemObject;
use sha2::{Digest, Sha256};
use tokio::time::sleep;
use tracing::{Level, event, warn};

use crate::cas_utils::is_zero_digest;
use crate::common_s3_utils::TlsClient;

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

type RemoveCallback = Arc<dyn RemoveItemCallback>;

#[derive(Debug, MetricsComponent)]
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

    remove_callbacks: Mutex<Vec<RemoveCallback>>,
}

pub fn load_custom_certs(cert_path: &str) -> Result<Arc<ClientConfig>, Error> {
    let mut root_store = RootCertStore::empty();

    // Create a BufReader from the cert file
    let mut cert_reader = BufReader::new(
        File::open(cert_path)
            .err_tip(|| format!("Failed to open CA certificate file {cert_path}"))?,
    );

    // Parse certificates
    let certs = CertificateDer::pem_reader_iter(&mut cert_reader).collect::<Result<Vec<_>, _>>()?;

    // Add each certificate to the root store
    for cert in certs {
        root_store.add(cert).map_err(|e| {
            make_err!(
                Code::Internal,
                "Failed to add certificate to root store: {e:?}"
            )
        })?;
    }

    // Build the client config with the root store
    let config = ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    Ok(Arc::new(config))
}

impl<I, NowFn> OntapS3Store<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    pub async fn new(spec: &ExperimentalOntapS3Spec, now_fn: NowFn) -> Result<Arc<Self>, Error> {
        // Load custom CA config
        let ca_config = if let Some(cert_path) = &spec.root_certificates {
            load_custom_certs(cert_path)?
        } else {
            Arc::new(
                ClientConfig::builder()
                    .with_native_roots()?
                    .with_no_client_auth(),
            )
        };

        let https_connector = hyper_rustls::HttpsConnectorBuilder::new()
            .with_tls_config((*ca_config).clone())
            .https_only()
            .enable_http1()
            .enable_http2()
            .build();

        let http_client = TlsClient::with_https_connector(&spec.common, https_connector);

        let credentials_provider = DefaultCredentialsChain::builder()
            .configure(
                ProviderConfig::without_region()
                    .with_region(Some(Region::new(Cow::Owned(spec.vserver_name.clone()))))
                    .with_http_client(http_client.clone()),
            )
            .build()
            .await;

        let config = aws_sdk_s3::Config::builder()
            .credentials_provider(credentials_provider)
            .endpoint_url(&spec.endpoint)
            .region(Region::new(spec.vserver_name.clone()))
            .app_name(aws_config::AppName::new("nativelink").expect("valid app name"))
            .http_client(http_client)
            .force_path_style(true)
            .behavior_version(BehaviorVersion::v2025_08_07())
            .timeout_config(
                aws_config::timeout::TimeoutConfig::builder()
                    .connect_timeout(Duration::from_secs(30))
                    .operation_timeout(Duration::from_secs(120))
                    .build(),
            )
            .build();

        let s3_client = Client::from_conf(config);

        Self::new_with_client_and_jitter(
            spec,
            s3_client,
            spec.common.retry.make_jitter_fn(),
            now_fn,
        )
    }

    pub fn new_with_client_and_jitter(
        spec: &ExperimentalOntapS3Spec,
        s3_client: Client,
        jitter_fn: Arc<dyn (Fn(Duration) -> Duration) + Send + Sync>,
        now_fn: NowFn,
    ) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self {
            s3_client: Arc::new(s3_client),
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
            max_retry_buffer_per_request: spec
                .common
                .max_retry_buffer_per_request
                .unwrap_or(DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST),
            multipart_max_concurrent_uploads: spec
                .common
                .multipart_max_concurrent_uploads
                .unwrap_or(DEFAULT_MULTIPART_MAX_CONCURRENT_UPLOADS),
            remove_callbacks: Mutex::new(vec![]),
        }))
    }

    fn make_s3_path(&self, key: &StoreKey<'_>) -> String {
        format!("{}{}", self.key_prefix, key.as_str())
    }

    async fn has(self: Pin<&Self>, digest: StoreKey<'_>) -> Result<Option<u64>, Error> {
        let digest_clone = digest.into_owned();
        self.retrier
            .retry(unfold((), move |state| {
                let local_digest = digest_clone.clone();
                async move {
                    let result = self
                        .s3_client
                        .head_object()
                        .bucket(&self.bucket)
                        .key(self.make_s3_path(&local_digest))
                        .send()
                        .await;

                    match result {
                        Ok(head_object_output) => {
                            if self.consider_expired_after_s != 0 {
                                if let Some(last_modified) = head_object_output.last_modified {
                                    let now_s = (self.now_fn)().unix_timestamp() as i64;
                                    if last_modified.secs() + self.consider_expired_after_s <= now_s
                                    {
                                        let remove_callbacks = self.remove_callbacks.lock().clone();
                                        let mut callbacks: FuturesUnordered<_> = remove_callbacks
                                            .into_iter()
                                            .map(|callback| {
                                                let store_key = local_digest.borrow();
                                                async move { callback.callback(store_key).await }
                                            })
                                            .collect();
                                        while callbacks.next().await.is_some() {}
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

                match self.has(key.borrow()).await {
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
                    let (mut tx, mut rx) = make_buf_channel_pair();

                    let result = {
                        let reader_ref = &mut reader;
                        let (upload_res, bind_res): (Result<(), Error>, Result<(), Error>) = tokio::join!(async move {
                            let raw_body_bytes = {
                                let mut raw_body_chunks = BytesMut::new();
                                loop {
                                    match rx.recv().await {
                                        Ok(chunk) => {
                                            if chunk.is_empty() {
                                                break Ok(raw_body_chunks.freeze());
                                            }
                                            raw_body_chunks.extend_from_slice(&chunk);
                                        }
                                        Err(err) => {
                                            break Err(err);
                                        }
                                    }
                                }
                            };
                            let internal_res = match raw_body_bytes {
                                Ok(body_bytes) => {
                                    let hash = Sha256::digest(&body_bytes);
                                    let send_res = self.s3_client
                                        .put_object()
                                        .bucket(&self.bucket)
                                        .key(s3_path.clone())
                                        .content_length(sz as i64)
                                        .body(
                                            ByteStream::from(body_bytes)
                                        )
                                        .set_checksum_algorithm(Some(aws_sdk_s3::types::ChecksumAlgorithm::Sha256))
                                        .set_checksum_sha256(Some(BASE64_STANDARD_NO_PAD.encode(hash)))
                                        .customize()
                                        .mutate_request(|req| {req.headers_mut().insert("x-amz-content-sha256", "UNSIGNED-PAYLOAD");})
                                        .send();
                                    Either::Left(send_res.map_ok_or_else(|e| Err(make_err!(Code::Aborted, "{e:?}")), |_| Ok(())))
                                    }
                                Err(collect_err) => {
                                    async fn make_collect_err(collect_err: Error) -> Result<(), Error> {
                                        Err(collect_err)
                                    }

                                    warn!(
                                        ?collect_err,
                                        "Failed to get body");
                                    let future_err = make_collect_err(collect_err);
                                    Either::Right(future_err)
                                }
                            };
                            internal_res.await
                        },
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

    fn as_any<'a>(&'a self) -> &'a (dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_remove_callback(
        self: Arc<Self>,
        callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        self.remove_callbacks.lock().push(callback);
        Ok(())
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
