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

use core::cmp;
use core::pin::Pin;
use core::time::Duration;
use std::borrow::Cow;
use std::sync::Arc;

use async_trait::async_trait;
use azure_core::credentials::TokenCredential;
use azure_core::error::ErrorKind;
use azure_core::http::{RequestContent, RetryOptions, StatusCode, Transport, Url};
use azure_identity::WorkloadIdentityCredential;
use azure_storage_blob::clients::{BlobContainerClient, BlobContainerClientOptions};
use azure_storage_blob::models::{
    BlobClientDownloadOptions, BlobClientGetPropertiesResultHeaders, BlockLookupList, HttpRange,
    StorageErrorCode,
};
use futures::future::FusedFuture;
use futures::stream::{FuturesUnordered, unfold};
use futures::{FutureExt, StreamExt, TryStreamExt};
use nativelink_config::stores::ExperimentalAzureSpec;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair,
};
use nativelink_util::fs;
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::retry::{Retrier, RetryResult};
use nativelink_util::store_trait::{
    RemoveCallback, StoreDriver, StoreKey, StoreOptimizations, UploadSizeInfo,
};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{Level, event};

use crate::cas_utils::is_zero_digest;
use crate::common_s3_utils::install_default_rustls_crypto_provider;

// Check the below doc for the limits specific to Azure.
// https://learn.microsoft.com/en-us/azure/storage/blobs/scalability-targets#scale-targets-for-blob-storage

// Maximum number of blocks in a block blob or append blob
const MAX_BLOCKS: usize = 50_000;

// Maximum size of a block in a block blob (4,000 MiB)
const MAX_BLOCK_SIZE: u64 = 4_000 * 1024 * 1024; // 4,000 MiB = 4 GiB

// Default block size for uploads (5 MiB)
const DEFAULT_BLOCK_SIZE: u64 = 5 * 1024 * 1024; // 5 MiB

// Default maximum retry buffer per request
const DEFAULT_MAX_RETRY_BUFFER_PER_REQUEST: usize = 5 * 1024 * 1024; // 5 MiB

// Default maximum number of concurrent uploads
const DEFAULT_MAX_CONCURRENT_UPLOADS: usize = 10;

// Default public Azure Blob Storage endpoint suffix.
const DEFAULT_BLOB_ENDPOINT_SUFFIX: &str = "blob.core.windows.net";

#[derive(MetricsComponent)]
pub struct AzureBlobStore<NowFn> {
    client: Arc<BlobContainerClient>,
    now_fn: NowFn,
    #[metric(help = "The container name for the Azure store")]
    container: String,
    #[metric(help = "The blob prefix for the Azure store")]
    blob_prefix: String,
    retrier: Retrier,
    #[metric(help = "The number of seconds to consider an object expired")]
    consider_expired_after_s: i64,
    #[metric(help = "The number of bytes to buffer for retrying requests")]
    max_retry_buffer_per_request: usize,
    #[metric(help = "The number of concurrent uploads allowed")]
    max_concurrent_uploads: usize,
}

impl<NowFn> core::fmt::Debug for AzureBlobStore<NowFn> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("AzureBlobStore")
            .field("container", &self.container)
            .field("blob_prefix", &self.blob_prefix)
            .field("consider_expired_after_s", &self.consider_expired_after_s)
            .finish_non_exhaustive()
    }
}

impl<I, NowFn> AzureBlobStore<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    pub async fn new(spec: &ExperimentalAzureSpec, now_fn: NowFn) -> Result<Arc<Self>, Error> {
        let jitter_fn = spec.common.retry.make_jitter_fn();
        let client = Self::build_container_client(spec)?;
        Self::new_with_client_and_jitter(spec, client, jitter_fn, now_fn)
    }

    /// Builds the container URL and selects the auth strategy:
    ///   * `sas_url` set    -> use it verbatim as the container URL with no credential.
    ///   * otherwise        -> `https://{account}.{endpoint}/{container}` authenticated with
    ///     Entra ID via Workload Identity (keyless).
    fn build_container_client(spec: &ExperimentalAzureSpec) -> Result<BlobContainerClient, Error> {
        let mut options = BlobContainerClientOptions::default();
        options.client_options.retry = RetryOptions::none();
        // Hand the SDK an HTTP client with an explicit rustls (ring) config.
        options.client_options.transport = Some(Self::build_http_transport()?);

        let (container_url, credential): (Url, Option<Arc<dyn TokenCredential>>) =
            if let Some(sas_url) = spec.sas_url.as_ref() {
                let url = Url::parse(sas_url)
                    .map_err(|e| make_err!(Code::InvalidArgument, "Invalid Azure sas_url: {e}"))?;
                (url, None)
            } else {
                let endpoint = spec.endpoint.clone().unwrap_or_else(|| {
                    format!(
                        "https://{}.{DEFAULT_BLOB_ENDPOINT_SUFFIX}",
                        spec.account_name
                    )
                });
                let mut url = Url::parse(&endpoint)
                    .map_err(|e| make_err!(Code::InvalidArgument, "Invalid Azure endpoint: {e}"))?;
                url.path_segments_mut()
                    .map_err(|()| {
                        make_err!(
                            Code::InvalidArgument,
                            "Azure endpoint is not a valid base URL: {endpoint}"
                        )
                    })?
                    .pop_if_empty()
                    .push(&spec.container);
                let credential: Arc<dyn TokenCredential> = WorkloadIdentityCredential::new(None)
                    .map_err(|e| {
                        make_err!(
                            Code::FailedPrecondition,
                            "Failed to create Azure Workload Identity credential: {e}"
                        )
                    })?;
                (url, Some(credential))
            };

        BlobContainerClient::new(container_url, credential, Some(options))
            .map_err(|e| make_err!(Code::Unavailable, "Failed to create Azure client: {e}"))
    }

    /// Builds an HTTP transport for the Azure SDK backed by a reqwest client with
    /// an explicit rustls config using `NativeLink`'s ring crypto provider, so the
    /// SDK never falls back to guessing a provider (which breaks HTTPS here).
    fn build_http_transport() -> Result<Transport, Error> {
        install_default_rustls_crypto_provider();

        let mut roots = rustls::RootCertStore::empty();
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();

        let client = reqwest::Client::builder()
            .use_preconfigured_tls(tls_config)
            .build()
            .map_err(|e| make_err!(Code::Unavailable, "Failed to build Azure HTTP client: {e}"))?;

        Ok(Transport::new(Arc::new(client)))
    }

    pub fn new_with_client_and_jitter(
        spec: &ExperimentalAzureSpec,
        client: BlobContainerClient,
        jitter_fn: Arc<dyn Fn(Duration) -> Duration + Send + Sync>,
        now_fn: NowFn,
    ) -> Result<Arc<Self>, Error> {
        Ok(Arc::new(Self {
            client: Arc::new(client),
            now_fn,
            container: spec.container.clone(),
            blob_prefix: spec
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
            max_concurrent_uploads: spec
                .common
                .multipart_max_concurrent_uploads
                .map_or(DEFAULT_MAX_CONCURRENT_UPLOADS, |v| v),
        }))
    }

    fn make_blob_path(&self, key: &StoreKey<'_>) -> String {
        format!("{}{}", self.blob_prefix, key.as_str())
    }

    async fn has(self: Pin<&Self>, digest: &StoreKey<'_>) -> Result<Option<u64>, Error> {
        let blob_path = self.make_blob_path(digest);

        self.retrier
            .retry(unfold((), move |state| {
                let blob_path = blob_path.clone();
                let client = Arc::clone(&self.client);
                async move {
                    let _permit = match fs::get_permit().await {
                        Ok(permit) => permit,
                        Err(e) => {
                            return Some((
                                RetryResult::Retry(make_err!(
                                    Code::Unavailable,
                                    "Failed to acquire permit: {e}"
                                )),
                                state,
                            ));
                        }
                    };

                    let result = client.blob_client(&blob_path).get_properties(None).await;

                    match result {
                        Ok(props) => {
                            if self.consider_expired_after_s > 0
                                && let Some(last_modified) = props.last_modified().ok().flatten()
                            {
                                let now = (self.now_fn)()
                                    .unix_timestamp()
                                    .try_into()
                                    .unwrap_or(i64::MAX);
                                if last_modified.unix_timestamp() + self.consider_expired_after_s
                                    <= now
                                {
                                    return Some((RetryResult::Ok(None), state));
                                }
                            }
                            let blob_size = props.content_length().ok().flatten().unwrap_or(0);
                            Some((RetryResult::Ok(Some(blob_size)), state))
                        }
                        Err(err) => {
                            if err.http_status() == Some(StatusCode::NotFound) {
                                // Distinguish a missing container (a config error) from a
                                // missing blob (a normal cache miss).
                                if let ErrorKind::HttpResponse {
                                    error_code: Some(error_code),
                                    ..
                                } = err.kind()
                                    && error_code == StorageErrorCode::ContainerNotFound.as_ref()
                                {
                                    return Some((
                                        RetryResult::Err(make_err!(
                                            Code::InvalidArgument,
                                            "Container not found: {err}"
                                        )),
                                        state,
                                    ));
                                }
                                Some((RetryResult::Ok(None), state))
                            } else {
                                Some((
                                    RetryResult::Retry(make_err!(
                                        Code::Unavailable,
                                        "Failed to get blob properties: {err:?}"
                                    )),
                                    state,
                                ))
                            }
                        }
                    }
                }
            }))
            .await
    }
}

#[async_trait]
impl<I, NowFn> StoreDriver for AzureBlobStore<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

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
                *result = self.has(key).await?;
                Ok::<_, Error>(())
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
    ) -> Result<u64, Error> {
        let blob_path = self.make_blob_path(&digest);
        // Handling zero-sized content check
        if upload_size == UploadSizeInfo::ExactSize(0) {
            return Ok(0);
        }

        let max_size = match upload_size {
            UploadSizeInfo::ExactSize(sz) | UploadSizeInfo::MaxSize(sz) => sz,
        };

        // For small files of a known size we buffer to `Bytes` and upload in a single request.
        if max_size < DEFAULT_BLOCK_SIZE && matches!(upload_size, UploadSizeInfo::ExactSize(_)) {
            let UploadSizeInfo::ExactSize(sz) = upload_size else {
                unreachable!("upload_size must be UploadSizeInfo::ExactSize here");
            };

            reader.set_max_recent_data_size(
                u64::try_from(self.max_retry_buffer_per_request)
                    .err_tip(|| "Could not convert max_retry_buffer_per_request to u64")?,
            );

            return self
                .retrier
                .retry(unfold(reader, move |mut reader| {
                    let client = Arc::clone(&self.client);
                    let blob_path = blob_path.clone();
                    async move {
                        let _permit = match fs::get_permit().await {
                            Ok(permit) => permit,
                            Err(e) => {
                                return Some((
                                    RetryResult::Retry(make_err!(
                                        Code::Unavailable,
                                        "Failed to acquire permit: {e}"
                                    )),
                                    reader,
                                ));
                            }
                        };

                        let (mut tx, mut rx) = make_buf_channel_pair();

                        let result = {
                            let reader_ref = &mut reader;
                            let (upload_res, bind_res) = tokio::join!(
                                async {
                                    let mut buffer = Vec::with_capacity(
                                        usize::try_from(sz).expect(
                                            "size must be non-negative and fit in usize",
                                        ),
                                    );
                                    while let Ok(Some(chunk)) = rx.try_next().await {
                                        buffer.extend_from_slice(&chunk);
                                    }

                                    client
                                        .blob_client(&blob_path)
                                        .block_blob_client()
                                        .upload(RequestContent::from(buffer), None)
                                        .await
                                        .map(|_| ())
                                        .map_err(|e| make_err!(Code::Aborted, "{e:?}"))
                                },
                                async { tx.bind_buffered(reader_ref).await }
                            );

                            match (upload_res, bind_res) {
                                (Ok(()), Ok(())) => Ok(()),
                                (Err(e), _) | (_, Err(e)) => Err(e),
                            }
                            .err_tip(|| "Failed to upload blob in single chunk")
                        };

                        match result {
                            Ok(()) => {
                                Some((RetryResult::Ok(reader.get_bytes_received()), reader))
                            }
                            Err(mut err) => {
                                err.code = Code::Aborted;
                                let bytes_received = reader.get_bytes_received();

                                if let Err(try_reset_err) = reader.try_reset_stream() {
                                    event!(
                                        Level::ERROR,
                                        ?bytes_received,
                                        err = ?try_reset_err,
                                        "Unable to reset stream after failed upload in AzureStore::update"
                                    );
                                    Some((
                                        RetryResult::Err(err.merge(try_reset_err).append(format!(
                                            "Failed to retry upload with {bytes_received} bytes received in AzureStore::update"
                                        ))),
                                        reader,
                                    ))
                                } else {
                                    let err = err.append(format!(
                                        "Retry on upload happened with {bytes_received} bytes received in AzureStore::update"
                                    ));
                                    event!(
                                        Level::INFO,
                                        ?err,
                                        ?bytes_received,
                                        "Retryable Azure error"
                                    );
                                    Some((RetryResult::Retry(err), reader))
                                }
                            }
                        }
                    }
                }))
                .await;
        }

        // For larger files we stream the content as staged blocks and commit a block list.
        let block_size =
            cmp::min(max_size / (MAX_BLOCKS as u64 - 1), MAX_BLOCK_SIZE).max(DEFAULT_BLOCK_SIZE);

        let (tx, mut rx) = mpsc::channel(self.max_concurrent_uploads);
        let mut block_ids: Vec<Vec<u8>> = Vec::with_capacity(MAX_BLOCKS);
        let retrier = self.retrier.clone();

        let read_stream_fut = {
            let tx = tx.clone();
            let blob_path = blob_path.clone();
            async move {
                let mut total_uploaded = 0;
                for block_id in 0..MAX_BLOCKS {
                    let write_buf = reader
                        .consume(Some(
                            usize::try_from(block_size)
                                .err_tip(|| "Could not convert block_size to usize")?,
                        ))
                        .await
                        .err_tip(|| "Failed to read chunk in azure_store")?;

                    if write_buf.is_empty() {
                        break;
                    }

                    total_uploaded += write_buf.len() as u64;

                    // Fixed-width, zero-padded ids keep the committed block list ordered
                    // after a lexicographic sort.
                    let block_id = format!("{block_id:032}").into_bytes();
                    let blob_path = blob_path.clone();

                    tx.send(async move {
                        self.retrier
                            .retry(unfold(
                                (write_buf, block_id),
                                move |(write_buf, block_id)| {
                                    let client = Arc::clone(&self.client);
                                    let blob_path = blob_path.clone();
                                    async move {
                                        let _permit = match fs::get_permit().await {
                                            Ok(permit) => permit,
                                            Err(e) => {
                                                return Some((
                                                    RetryResult::Retry(make_err!(
                                                        Code::Unavailable,
                                                        "Failed to acquire permit: {e}"
                                                    )),
                                                    (write_buf, block_id),
                                                ));
                                            }
                                        };
                                        let content_length = write_buf.len() as u64;
                                        let retry_result = client
                                            .blob_client(&blob_path)
                                            .block_blob_client()
                                            .stage_block(
                                                &block_id,
                                                content_length,
                                                RequestContent::from(write_buf.to_vec()),
                                                None,
                                            )
                                            .await
                                            .map_or_else(
                                                |e| {
                                                    RetryResult::Retry(make_err!(
                                                        Code::Aborted,
                                                        "Failed to upload block in Azure store: {e:?}"
                                                    ))
                                                },
                                                |_| RetryResult::Ok(block_id.clone()),
                                            );
                                        Some((retry_result, (write_buf, block_id)))
                                    }
                                },
                            ))
                            .await
                    })
                    .await
                    .map_err(|err| {
                        Error::from_std_err(Code::Internal, &err)
                            .append("Failed to send block to channel")
                    })?;
                }
                Ok::<_, Error>(total_uploaded)
            }
            .fuse()
        };

        let mut upload_futures = FuturesUnordered::new();
        let mut total_uploaded = 0;

        tokio::pin!(read_stream_fut);

        loop {
            if read_stream_fut.is_terminated() && rx.is_empty() && upload_futures.is_empty() {
                break;
            }
            tokio::select! {
                result = &mut read_stream_fut => {
                    total_uploaded = result?;
                },
                Some(block_id) = upload_futures.next() => block_ids.push(block_id?),
                Some(fut) = rx.recv() => upload_futures.push(fut),
            }
        }

        // Sorting block IDs to ensure consistent ordering of the committed blob.
        block_ids.sort_unstable();

        let block_list = BlockLookupList {
            latest: Some(block_ids),
            ..Default::default()
        };

        retrier
            .retry(unfold(block_list, move |block_list| {
                let client = Arc::clone(&self.client);
                let blob_path = blob_path.clone();

                async move {
                    let _permit = match fs::get_permit().await {
                        Ok(permit) => permit,
                        Err(e) => {
                            return Some((
                                RetryResult::Retry(make_err!(
                                    Code::Unavailable,
                                    "Failed to acquire permit: {e}"
                                )),
                                block_list,
                            ));
                        }
                    };

                    let blocks = match RequestContent::try_from(block_list.clone()) {
                        Ok(blocks) => blocks,
                        Err(e) => {
                            return Some((
                                RetryResult::Err(make_err!(
                                    Code::Internal,
                                    "Failed to serialize block list in Azure store: {e:?}"
                                )),
                                block_list,
                            ));
                        }
                    };

                    let retry_result = client
                        .blob_client(&blob_path)
                        .block_blob_client()
                        .commit_block_list(blocks, None)
                        .await
                        .map_or_else(
                            |e| {
                                RetryResult::Retry(
                                    Error::from_std_err(Code::Aborted, &e)
                                        .append("Failed to commit block list in Azure store:"),
                                )
                            },
                            |_| RetryResult::Ok(total_uploaded),
                        );
                    Some((retry_result, block_list))
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
                .err_tip(|| "Failed to send zero EOF in azure store get_part")?;
            return Ok(());
        }

        let blob_path = self.make_blob_path(&key);

        let range = match length {
            Some(len) => Some(HttpRange::new(offset, len)),
            None if offset == 0 => None,
            None => Some(HttpRange::from_offset(offset)),
        };

        self.retrier
            .retry(unfold(writer, move |writer| {
                let range = range.clone();
                let client = Arc::clone(&self.client);
                let blob_path = blob_path.clone();
                async move {
                    let _permit = match fs::get_permit().await {
                        Ok(permit) => permit,
                        Err(e) => {
                            return Some((
                                RetryResult::Retry(make_err!(
                                    Code::Unavailable,
                                    "Failed to acquire permit: {e}"
                                )),
                                writer,
                            ));
                        }
                    };

                    let result: Result<(), Error> = async {
                        let options = BlobClientDownloadOptions {
                            range,
                            ..Default::default()
                        };
                        let response = client
                            .blob_client(&blob_path)
                            .download(Some(options))
                            .await
                            .map_err(|e| {
                                if e.http_status() == Some(StatusCode::NotFound) {
                                    make_err!(Code::NotFound, "Blob not found in Azure: {e:?}")
                                } else {
                                    make_err!(
                                        Code::Aborted,
                                        "Failed to start download from Azure: {e:?}"
                                    )
                                }
                            })?;

                        let mut body = response.body;
                        while let Some(chunk) = body.try_next().await.map_err(|e| {
                            make_err!(Code::Aborted, "Error reading from Azure stream: {e:?}")
                        })? {
                            if chunk.is_empty() {
                                continue;
                            }
                            writer.send(chunk).await.map_err(|e| {
                                make_err!(Code::Aborted, "Failed to send data to writer: {e:?}")
                            })?;
                        }

                        writer.send_eof().map_err(|e| {
                            make_err!(Code::Aborted, "Failed to send EOF to writer: {e:?}")
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

    fn as_any<'a>(&'a self) -> &'a (dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_health(self: Arc<Self>, registry: &mut HealthRegistryBuilder) {
        registry.register_indicator(self);
    }

    fn register_remove_callback(self: Arc<Self>, _callback: RemoveCallback) -> Result<(), Error> {
        // Azure Blob Storage manages object lifecycle externally,
        // so we can safely ignore remove callbacks.
        Ok(())
    }
}

#[async_trait]
impl<I, NowFn> HealthStatusIndicator for AzureBlobStore<NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + 'static,
{
    fn get_name(&self) -> &'static str {
        "AzureBlobStore"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}
