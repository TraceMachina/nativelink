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

use core::convert::Into;
use core::pin::{Pin, pin};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use std::collections::{HashMap, VecDeque};

use bytes::Bytes;
use fastcdc::v2020::{AsyncStreamCDC, Normalization};
use futures::stream::{FuturesUnordered, Stream};
use futures::{StreamExt, TryStreamExt};
use nativelink_config::cas_server::{CasStoreConfig, WithInstanceName};
use nativelink_error::{Code, Error, ResultExt, error_if, make_err, make_input_err};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent, group, publish,
};
use nativelink_proto::build::bazel::remote::execution::v2::content_addressable_storage_server::{
    ContentAddressableStorage, ContentAddressableStorageServer as Server,
};
use nativelink_proto::build::bazel::remote::execution::v2::{
    BatchReadBlobsRequest, BatchReadBlobsResponse, BatchUpdateBlobsRequest,
    BatchUpdateBlobsResponse, Digest, Directory, FindMissingBlobsRequest, FindMissingBlobsResponse,
    GetTreeRequest, GetTreeResponse, SpliceBlobRequest, SpliceBlobResponse, SplitBlobRequest,
    SplitBlobResponse, batch_read_blobs_response, batch_update_blobs_response, chunking_function,
    compressor,
};
use nativelink_proto::google::rpc::Status as GrpcStatus;
use nativelink_store::ac_utils::get_and_decode_digest;
use nativelink_store::grpc_store::GrpcStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{
    DigestHasher, digest_hasher_func_from_context, make_ctx_for_hash_func,
};
use nativelink_util::store_trait::{Store, StoreLike, UploadSizeInfo};
use opentelemetry::context::FutureExt;
use prost::Message;
use tokio_util::io::StreamReader;
use tonic::{Request, Response, Status};
use tracing::{Instrument, Level, debug, error_span, instrument};

/// Metrics for the experimental `SplitBlob`/`SpliceBlob` chunking RPCs.
/// The split hit rate (`split_hits` / `split_requests_total`) indicates how
/// often chunked downloads could be served; the spliced/split byte totals
/// bound the transfer volume flowing through the chunked paths.
#[derive(Debug, Default)]
pub struct ChunkingMetrics {
    /// Total `SpliceBlob` requests received on chunking-enabled instances.
    pub splice_requests_total: AtomicU64,
    /// `SpliceBlob` requests that were no-ops because the blob and its chunk
    /// layout were already registered.
    pub splice_already_exists: AtomicU64,
    /// `SpliceBlob` requests rejected because the re-assembled blob did not
    /// match the expected digest or size.
    pub splice_verification_failures: AtomicU64,
    /// Total bytes of blobs successfully re-assembled by `SpliceBlob`.
    pub splice_bytes_total: AtomicU64,
    /// Total `SplitBlob` requests received on chunking-enabled instances.
    pub split_requests_total: AtomicU64,
    /// `SplitBlob` requests served from a stored chunk layout.
    pub split_hits: AtomicU64,
    /// `SplitBlob` requests that could not be served because the blob was
    /// not present in the CAS.
    pub split_misses: AtomicU64,
    /// `SplitBlob` requests served by chunking the blob on demand because
    /// no stored layout was available (or its chunks were evicted).
    pub split_chunked_on_demand: AtomicU64,
    /// Total bytes of blobs served as chunk layouts by `SplitBlob`.
    pub split_bytes_total: AtomicU64,
}

impl MetricsComponent for ChunkingMetrics {
    fn publish(
        &self,
        _kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        let _enter = group!(field_metadata.name).entered();

        publish!(
            "splice_requests_total",
            &self.splice_requests_total,
            MetricKind::Counter,
            "Total SpliceBlob requests received"
        );
        publish!(
            "splice_already_exists",
            &self.splice_already_exists,
            MetricKind::Counter,
            "SpliceBlob requests that were no-ops because blob and layout already existed"
        );
        publish!(
            "splice_verification_failures",
            &self.splice_verification_failures,
            MetricKind::Counter,
            "SpliceBlob requests rejected due to digest or size mismatch"
        );
        publish!(
            "splice_bytes_total",
            &self.splice_bytes_total,
            MetricKind::Counter,
            "Total bytes of blobs re-assembled by SpliceBlob"
        );
        publish!(
            "split_requests_total",
            &self.split_requests_total,
            MetricKind::Counter,
            "Total SplitBlob requests received"
        );
        publish!(
            "split_hits",
            &self.split_hits,
            MetricKind::Counter,
            "SplitBlob requests served from a stored chunk layout"
        );
        publish!(
            "split_misses",
            &self.split_misses,
            MetricKind::Counter,
            "SplitBlob requests where the blob was not present"
        );
        publish!(
            "split_chunked_on_demand",
            &self.split_chunked_on_demand,
            MetricKind::Counter,
            "SplitBlob requests served by chunking the blob on demand"
        );
        publish!(
            "split_bytes_total",
            &self.split_bytes_total,
            MetricKind::Counter,
            "Total bytes of blobs served as chunk layouts by SplitBlob"
        );

        Ok(MetricPublishKnownKindData::Component)
    }
}

/// Per-instance state for the experimental chunking RPCs.
#[derive(Debug, Clone)]
struct ChunkingInstance {
    /// Store holding blob-digest -> chunk-layout mappings.
    index_store: Store,
    /// Average chunk size used for server-side `FastCDC` 2020 chunking.
    avg_chunk_size_bytes: u32,
}

#[derive(Debug)]
pub struct CasServer {
    stores: HashMap<String, Store>,
    chunking_instances: HashMap<String, ChunkingInstance>,
    chunking_metrics: ChunkingMetrics,
}

type GetTreeStream = Pin<Box<dyn Stream<Item = Result<GetTreeResponse, Status>> + Send + 'static>>;

/// Per-blob deadline applied inside `BatchReadBlobs` / `BatchUpdateBlobs`.
const BATCH_PER_BLOB_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum size of a single chunk accepted in a `SpliceBlob` request.
/// Deliberately looser than the largest chunk the server ever advertises
/// (4x the maximum allowed average = 4 MiB) so clients using their own
/// chunking function are still accepted. Together with `CHUNK_CONCURRENCY`
/// this bounds the memory a single splice request can pin.
const MAX_SPLICE_CHUNK_SIZE: u64 = 16 * 1024 * 1024;

/// Maximum number of chunks accepted in a `SpliceBlob` request or produced
/// by on-demand chunking in `SplitBlob`. Bounds the size of stored chunk
/// layouts and `SplitBlobResponse` messages (roughly 80 bytes per chunk,
/// ~4 MiB at the cap) independently of how small a client's chunks are.
const MAX_CHUNK_COUNT: usize = 50_000;

/// Maximum serialized chunk layout size read back from the index store.
/// Layouts written by this server are bounded by `MAX_CHUNK_COUNT`, so a
/// larger entry is corrupt. A truncated read is detected (and treated as no
/// layout) by the size consistency check in `read_chunk_layout`.
const MAX_CHUNK_LAYOUT_SIZE: u64 = 16 * 1024 * 1024;

/// Number of chunk reads/writes kept in flight while re-assembling or
/// chunking a blob. Matches the `DedupStore` concurrency default.
const CHUNK_CONCURRENCY: usize = 10;

impl CasServer {
    pub fn new(
        configs: &[WithInstanceName<CasStoreConfig>],
        store_manager: &StoreManager,
    ) -> Result<Self, Error> {
        let mut stores = HashMap::with_capacity(configs.len());
        let mut chunking_instances = HashMap::new();
        for config in configs {
            let store = store_manager.get_store(&config.cas_store).ok_or_else(|| {
                make_input_err!("'cas_store': '{}' does not exist", config.cas_store)
            })?;
            if let Some(chunking_config) = &config.experimental_chunking {
                // Chunk layouts are stored under the digests of the blobs
                // they describe but do not hash to them, so writing them
                // into the CAS itself would overwrite blob content.
                error_if!(
                    chunking_config.index_store == config.cas_store,
                    "'experimental_chunking.index_store' of instance '{}' must not be the same store as 'cas_store'",
                    config.instance_name
                );
                // Chunking against a grpc proxy store would download and
                // re-upload entire blobs through the proxy instead of
                // forwarding the RPCs; reject it until native forwarding is
                // implemented.
                error_if!(
                    store.downcast_ref::<GrpcStore>(None).is_some(),
                    "'experimental_chunking' of instance '{}' is not supported when 'cas_store' is a grpc store",
                    config.instance_name
                );
                let index_store = store_manager
                    .get_store(&chunking_config.index_store)
                    .ok_or_else(|| {
                        make_input_err!(
                            "'experimental_chunking.index_store': '{}' does not exist",
                            chunking_config.index_store
                        )
                    })?;
                let avg_chunk_size_bytes = chunking_config
                    .validated_avg_chunk_size_bytes()
                    .err_tip(|| {
                        format!(
                            "In 'experimental_chunking' of instance '{}'",
                            config.instance_name
                        )
                    })?;
                let avg_chunk_size_bytes = u32::try_from(avg_chunk_size_bytes)
                    .err_tip(|| "avg_chunk_size_bytes did not fit in u32")?;
                chunking_instances.insert(
                    config.instance_name.clone(),
                    ChunkingInstance {
                        index_store,
                        avg_chunk_size_bytes,
                    },
                );
            }
            stores.insert(config.instance_name.clone(), store);
        }
        Ok(Self {
            stores,
            chunking_instances,
            chunking_metrics: ChunkingMetrics::default(),
        })
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
    }

    /// Metrics for the experimental `SplitBlob`/`SpliceBlob` RPCs.
    pub const fn chunking_metrics(&self) -> &ChunkingMetrics {
        &self.chunking_metrics
    }

    async fn inner_find_missing_blobs(
        &self,
        request: FindMissingBlobsRequest,
    ) -> Result<Response<FindMissingBlobsResponse>, Error> {
        let instance_name = &request.instance_name;
        let store = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?
            .clone();

        let mut requested_blobs = Vec::with_capacity(request.blob_digests.len());
        for digest in &request.blob_digests {
            requested_blobs.push(DigestInfo::try_from(digest.clone())?.into());
        }
        let sizes = store
            .has_many(&requested_blobs)
            .await
            .err_tip(|| "In find_missing_blobs")?;
        let missing_blob_digests = sizes
            .into_iter()
            .zip(request.blob_digests)
            .filter_map(|(maybe_size, digest)| maybe_size.map_or_else(|| Some(digest), |_| None))
            .collect();

        Ok(Response::new(FindMissingBlobsResponse {
            missing_blob_digests,
        }))
    }

    async fn inner_batch_update_blobs(
        &self,
        request: BatchUpdateBlobsRequest,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Error> {
        let instance_name = &request.instance_name;

        let store = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?
            .clone();

        // If we are a GrpcStore we shortcut here, as this is a special store.
        // Note: We don't know the digests here, so we try perform a very shallow
        // check to see if it's a grpc store.
        if let Some(grpc_store) = store.downcast_ref::<GrpcStore>(None) {
            return grpc_store.batch_update_blobs(Request::new(request)).await;
        }

        let store_ref = &store;
        let update_futures: FuturesUnordered<_> = request
            .requests
            .into_iter()
            .map(|request| async move {
                let digest = request
                    .digest
                    .clone()
                    .err_tip(|| "Digest not found in request")?;
                let request_data = request.data;
                let digest_info = DigestInfo::try_from(digest.clone())?;
                let size_bytes = usize::try_from(digest_info.size_bytes())
                    .err_tip(|| "Digest size_bytes was not convertible to usize")?;
                error_if!(
                    size_bytes != request_data.len(),
                    "Digest for upload had mismatching sizes, digest said {} data  said {}",
                    size_bytes,
                    request_data.len()
                );
                // Apply a per-blob deadline so one slow upload does not
                // make the whole batch hit the client's overall deadline.
                let result = match tokio::time::timeout(
                    BATCH_PER_BLOB_TIMEOUT,
                    store_ref.update_oneshot(digest_info, request_data),
                )
                .await
                {
                    Ok(r) => r.err_tip(|| "Error writing to store"),
                    Err(_elapsed) => Err(make_err!(
                        Code::DeadlineExceeded,
                        "BatchUpdateBlobs per-blob timeout ({} s) elapsed for digest {}",
                        BATCH_PER_BLOB_TIMEOUT.as_secs(),
                        digest_info,
                    )),
                };
                Ok::<_, Error>(batch_update_blobs_response::Response {
                    digest: Some(digest),
                    status: Some(result.map_or_else(Into::into, |()| GrpcStatus::default())),
                })
            })
            .collect();
        let responses = update_futures
            .try_collect::<Vec<batch_update_blobs_response::Response>>()
            .await?;

        Ok(Response::new(BatchUpdateBlobsResponse { responses }))
    }

    async fn inner_batch_read_blobs(
        &self,
        request: BatchReadBlobsRequest,
    ) -> Result<Response<BatchReadBlobsResponse>, Error> {
        let instance_name = &request.instance_name;

        let store = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?
            .clone();

        // If we are a GrpcStore we shortcut here, as this is a special store.
        // Note: We don't know the digests here, so we try perform a very shallow
        // check to see if it's a grpc store.
        if let Some(grpc_store) = store.downcast_ref::<GrpcStore>(None) {
            return grpc_store.batch_read_blobs(Request::new(request)).await;
        }

        let store_ref = &store;
        let read_futures: FuturesUnordered<_> = request
            .digests
            .into_iter()
            .map(|digest| async move {
                let digest_copy = DigestInfo::try_from(digest.clone())?;
                // TODO(palfrey) There is a security risk here of someone taking all the memory on the instance.
                // Apply a per-blob deadline so one slow read does not
                // make the whole batch hit the client's overall deadline.
                let result = match tokio::time::timeout(
                    BATCH_PER_BLOB_TIMEOUT,
                    store_ref.get_part_unchunked(digest_copy, 0, None),
                )
                .await
                {
                    Ok(r) => r.err_tip(|| "Error reading from store"),
                    Err(_elapsed) => Err(make_err!(
                        Code::DeadlineExceeded,
                        "BatchReadBlobs per-blob timeout ({} s) elapsed for digest {}",
                        BATCH_PER_BLOB_TIMEOUT.as_secs(),
                        digest_copy,
                    )),
                };
                let (status, data) = result.map_or_else(
                    |mut e| {
                        if e.code == Code::NotFound {
                            // Trim the error code. Not Found is quite common and we don't want to send a large
                            // error (debug) message for something that is common. We resize to just the last
                            // message as it will be the most relevant.
                            e.messages.resize_with(1, String::new);
                        }
                        (e.into(), Bytes::new())
                    },
                    |v| (GrpcStatus::default(), v),
                );
                Ok::<_, Error>(batch_read_blobs_response::Response {
                    status: Some(status),
                    digest: Some(digest),
                    compressor: compressor::Value::Identity.into(),
                    data,
                })
            })
            .collect();
        let responses = read_futures
            .try_collect::<Vec<batch_read_blobs_response::Response>>()
            .await?;

        Ok(Response::new(BatchReadBlobsResponse { responses }))
    }

    async fn inner_get_tree(
        &self,
        request: GetTreeRequest,
    ) -> Result<impl Stream<Item = Result<GetTreeResponse, Status>> + Send + use<>, Error> {
        let instance_name = &request.instance_name;

        let store = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?
            .clone();

        // If we are a GrpcStore we shortcut here, as this is a special store.
        // Note: We don't know the digests here, so we try perform a very shallow
        // check to see if it's a grpc store.
        if let Some(grpc_store) = store.downcast_ref::<GrpcStore>(None) {
            let stream = grpc_store
                .get_tree(Request::new(request))
                .await?
                .into_inner();
            return Ok(stream.left_stream());
        }
        let root_digest: DigestInfo = request
            .root_digest
            .err_tip(|| "Expected root_digest to exist in GetTreeRequest")?
            .try_into()
            .err_tip(|| "In GetTreeRequest::root_digest")?;

        let mut deque: VecDeque<DigestInfo> = VecDeque::new();
        let mut directories: Vec<Directory> = Vec::new();
        // `page_token` will return the `{hash_str}-{size_bytes}` of the current request's first directory digest.
        let page_token_digest = if request.page_token.is_empty() {
            root_digest
        } else {
            let mut page_token_parts = request.page_token.split('-');
            DigestInfo::try_new(
                page_token_parts
                    .next()
                    .err_tip(|| "Failed to parse `hash_str` in `page_token`")?,
                page_token_parts
                    .next()
                    .err_tip(|| "Failed to parse `size_bytes` in `page_token`")?
                    .parse::<i64>()
                    .err_tip(|| "Failed to parse `size_bytes` as i64")?,
            )
            .err_tip(|| "Failed to parse `page_token` as `Digest` in `GetTreeRequest`")?
        };
        let page_size = request.page_size;
        // If `page_size` is 0, paging is not necessary.
        let mut page_token_matched = page_size == 0;
        deque.push_back(root_digest);

        while !deque.is_empty() {
            let digest: DigestInfo = deque.pop_front().err_tip(|| "In VecDeque::pop_front")?;
            let directory = get_and_decode_digest::<Directory>(&store, digest.into())
                .await
                .err_tip(|| "Converting digest to Directory")?;
            if digest == page_token_digest {
                page_token_matched = true;
            }
            for directory in &directory.directories {
                let digest: DigestInfo = directory
                    .digest
                    .clone()
                    .err_tip(|| "Expected Digest to exist in Directory::directories::digest")?
                    .try_into()
                    .err_tip(|| "In Directory::file::digest")?;
                deque.push_back(digest);
            }

            let page_size_usize = usize::try_from(page_size).unwrap_or(usize::MAX);

            if page_token_matched {
                directories.push(directory);
                if directories.len() == page_size_usize {
                    break;
                }
            }
        }
        // `next_page_token` will return the `{hash_str}:{size_bytes}` of the next request's first directory digest.
        // It will be an empty string when it reached the end of the directory tree.
        let next_page_token: String = deque
            .front()
            .map_or_else(String::new, |value| format!("{value}"));

        Ok(futures::stream::once(async {
            Ok(GetTreeResponse {
                directories,
                next_page_token,
            })
        })
        .right_stream())
    }

    /// Returns the CAS store and chunking state for an instance, or
    /// `Unimplemented` when chunking is not enabled for it.
    fn chunking_instance(&self, instance_name: &str) -> Result<(Store, ChunkingInstance), Error> {
        let store = self
            .stores
            .get(instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?
            .clone();
        let chunking_instance = self
            .chunking_instances
            .get(instance_name)
            .ok_or_else(|| {
                make_err!(
                    Code::Unimplemented,
                    "Blob chunking is not enabled for instance '{instance_name}'"
                )
            })?
            .clone();
        Ok((store, chunking_instance))
    }

    /// Returns the display names of the chunks missing from the CAS. The
    /// existence check also touches present chunks, which extends their
    /// lifetimes on a best-effort basis (stores that answer existence from a
    /// cache may not promote the underlying entries).
    async fn missing_chunks(store: &Store, chunk_digests: &[Digest]) -> Result<Vec<String>, Error> {
        let mut digest_infos = Vec::with_capacity(chunk_digests.len());
        for digest in chunk_digests {
            digest_infos
                .push(DigestInfo::try_from(digest.clone()).err_tip(|| "Invalid chunk digest")?);
        }
        let chunk_keys: Vec<_> = digest_infos.iter().map(|digest| (*digest).into()).collect();
        let sizes = store
            .has_many(&chunk_keys)
            .await
            .err_tip(|| "In missing_chunks")?;
        Ok(sizes
            .iter()
            .zip(&digest_infos)
            .filter(|(maybe_size, _)| maybe_size.is_none())
            .map(|(_, digest)| digest.to_string())
            .collect())
    }

    /// Reads the chunk layout registered for a blob. Returns `None` when no
    /// usable layout exists: not registered, undecodable, or inconsistent
    /// with the blob size (which also rejects entries truncated by the read
    /// cap below).
    async fn read_chunk_layout(
        index_store: &Store,
        blob_digest: DigestInfo,
    ) -> Option<SplitBlobResponse> {
        let layout_bytes = index_store
            .get_part_unchunked(blob_digest, 0, Some(MAX_CHUNK_LAYOUT_SIZE))
            .await
            .ok()?;
        let layout = SplitBlobResponse::decode(layout_bytes).ok()?;
        // A usable layout must reproduce the blob exactly, so the chunk
        // sizes have to add up to the blob size.
        let mut total_size: u64 = 0;
        for digest in &layout.chunk_digests {
            total_size = total_size.checked_add(u64::try_from(digest.size_bytes).ok()?)?;
        }
        (total_size == blob_digest.size_bytes()).then_some(layout)
    }

    /// Writes the chunk layout for a blob to the index store. This is the
    /// write side of the format `read_chunk_layout` expects.
    async fn write_chunk_layout(
        index_store: &Store,
        blob_digest: DigestInfo,
        layout: &SplitBlobResponse,
    ) -> Result<(), Error> {
        index_store
            .update_oneshot(blob_digest, layout.encode_to_vec().into())
            .await
            .err_tip(|| "Failed to write chunk layout to index store")
    }

    async fn inner_split_blob(
        &self,
        request: SplitBlobRequest,
    ) -> Result<Response<SplitBlobResponse>, Error> {
        let (store, chunking_instance) = self.chunking_instance(&request.instance_name)?;
        self.chunking_metrics
            .split_requests_total
            .fetch_add(1, Ordering::Relaxed);

        let blob_digest: DigestInfo = request
            .blob_digest
            .err_tip(|| "Expected blob_digest to exist in SplitBlobRequest")?
            .try_into()
            .err_tip(|| "In SplitBlobRequest::blob_digest")?;

        // The existence check also touches the blob, extending its lifetime
        // (best effort) as suggested by the REAPI spec for SplitBlob.
        let (blob_exists, maybe_layout) = futures::join!(
            store.has(blob_digest),
            Self::read_chunk_layout(&chunking_instance.index_store, blob_digest),
        );
        if blob_exists.err_tip(|| "In split_blob")?.is_none() {
            self.chunking_metrics
                .split_misses
                .fetch_add(1, Ordering::Relaxed);
            return Err(make_err!(
                Code::NotFound,
                "Blob {blob_digest} not present in the CAS in split_blob"
            ));
        }

        // Serve the registered layout if it is still fully backed by chunks
        // in the CAS. Any problem with it (missing, corrupt, evicted chunks,
        // or a transient chunk existence-check failure) falls back to
        // re-chunking the blob below.
        if let Some(layout) = maybe_layout
            && matches!(
                Self::missing_chunks(&store, &layout.chunk_digests).await,
                Ok(missing) if missing.is_empty()
            )
        {
            self.chunking_metrics
                .split_hits
                .fetch_add(1, Ordering::Relaxed);
            self.chunking_metrics
                .split_bytes_total
                .fetch_add(blob_digest.size_bytes(), Ordering::Relaxed);
            return Ok(Response::new(layout));
        }

        // No usable layout: chunk the blob on demand with FastCDC 2020,
        // store the chunks and the layout, and serve the result. This is the
        // path taken for blobs that were uploaded whole (e.g. outputs
        // produced by remote execution workers).
        let split_response = self
            .chunk_blob_on_demand(&store, &chunking_instance, blob_digest)
            .await?;
        self.chunking_metrics
            .split_chunked_on_demand
            .fetch_add(1, Ordering::Relaxed);
        self.chunking_metrics
            .split_bytes_total
            .fetch_add(blob_digest.size_bytes(), Ordering::Relaxed);
        Ok(Response::new(split_response))
    }

    /// Chunks the blob with `FastCDC` 2020 (normalization level 2, parameters
    /// derived from the configured average chunk size per the REAPI spec),
    /// uploads any missing chunks to the CAS, registers the layout in the
    /// index store, and returns it.
    async fn chunk_blob_on_demand(
        &self,
        store: &Store,
        chunking_instance: &ChunkingInstance,
        blob_digest: DigestInfo,
    ) -> Result<SplitBlobResponse, Error> {
        let avg_size = chunking_instance.avg_chunk_size_bytes;
        let (min_size, max_size) = (avg_size / 4, avg_size * 4);
        let hasher_func = digest_hasher_func_from_context();

        let (tx, rx) = make_buf_channel_pair();
        let read_store = store.clone();
        // `tx` is moved into the future so that when the read finishes or
        // fails it is dropped, which terminates the chunking stream.
        let read_fut = async move {
            let mut tx = tx;
            read_store
                .get_part(blob_digest, &mut tx, 0, None)
                .await
                .err_tip(|| format!("Failed to read blob {blob_digest} in chunk_blob_on_demand"))
        };
        // `rx` is owned by this future so an early error return drops it,
        // which aborts the in-flight read instead of leaving it blocked.
        let chunk_fut = async move {
            let mut bytes_reader = StreamReader::new(rx);
            let mut cdc = AsyncStreamCDC::with_level(
                &mut bytes_reader,
                min_size,
                avg_size,
                max_size,
                Normalization::Level2,
            );
            // Chunks are hashed and stored CHUNK_CONCURRENCY at a time while
            // the blob keeps streaming; `buffered` preserves chunk order.
            let chunk_digests: Vec<Digest> = pin!(cdc.as_stream())
                .map(|chunk_result| async {
                    let chunk = chunk_result
                        .map_err(|e| make_err!(Code::Internal, "Failed to chunk blob: {e:?}"))
                        .err_tip(|| "In chunk_blob_on_demand")?;
                    let mut hasher = hasher_func.hasher();
                    hasher.update(&chunk.data);
                    let chunk_digest = hasher.finalize_digest();
                    // The existence check also touches pre-existing chunks,
                    // extending their lifetimes (best effort). FastCDC is
                    // deterministic, so repeated splits of similar blobs
                    // mostly find their chunks present.
                    if store
                        .has(chunk_digest)
                        .await
                        .err_tip(|| "In chunk_blob_on_demand")?
                        .is_none()
                    {
                        store
                            .update_oneshot(chunk_digest, chunk.data.into())
                            .await
                            .err_tip(|| {
                                format!(
                                    "Failed to store chunk {chunk_digest} in chunk_blob_on_demand"
                                )
                            })?;
                    }
                    Ok::<Digest, Error>(chunk_digest.into())
                })
                .buffered(CHUNK_CONCURRENCY)
                .try_collect()
                .await?;
            Ok::<Vec<Digest>, Error>(chunk_digests)
        };
        let (read_res, chunk_res) = futures::join!(read_fut, chunk_fut);
        // Prefer the read error (the chunker error is usually a consequence
        // of it); merge keeps both messages when both fail.
        let chunk_digests = read_res
            .merge(chunk_res)
            .err_tip(|| "Failed to chunk blob in chunk_blob_on_demand")?;
        if chunk_digests.len() > MAX_CHUNK_COUNT {
            return Err(make_err!(
                Code::NotFound,
                "Blob {blob_digest} produced {} chunks, exceeding the supported maximum of {MAX_CHUNK_COUNT}; no split information available",
                chunk_digests.len()
            ));
        }

        let split_response = SplitBlobResponse {
            chunk_digests,
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        };
        Self::write_chunk_layout(&chunking_instance.index_store, blob_digest, &split_response)
            .await?;
        Ok(split_response)
    }

    async fn inner_splice_blob(
        &self,
        request: SpliceBlobRequest,
    ) -> Result<Response<SpliceBlobResponse>, Error> {
        let (store, chunking_instance) = self.chunking_instance(&request.instance_name)?;
        let index_store = chunking_instance.index_store;
        self.chunking_metrics
            .splice_requests_total
            .fetch_add(1, Ordering::Relaxed);

        let blob_digest: DigestInfo = request
            .blob_digest
            .err_tip(|| "Expected blob_digest to exist in SpliceBlobRequest")?
            .try_into()
            .err_tip(|| "In SpliceBlobRequest::blob_digest")?;

        error_if!(
            request.chunk_digests.is_empty(),
            "chunk_digests must not be empty in splice_blob"
        );
        error_if!(
            request.chunk_digests.len() > MAX_CHUNK_COUNT,
            "Request has {} chunk_digests, expected at most {MAX_CHUNK_COUNT} in splice_blob",
            request.chunk_digests.len()
        );
        let mut chunk_digests = Vec::with_capacity(request.chunk_digests.len());
        let mut total_size: u64 = 0;
        for digest in &request.chunk_digests {
            let digest_info = DigestInfo::try_from(digest.clone())
                .err_tip(|| "In SpliceBlobRequest::chunk_digests")?;
            error_if!(
                digest_info.size_bytes() == 0 || digest_info.size_bytes() > MAX_SPLICE_CHUNK_SIZE,
                "Chunk {digest_info} has invalid size, expected to be in range (0, {MAX_SPLICE_CHUNK_SIZE}] in splice_blob"
            );
            total_size += digest_info.size_bytes();
            chunk_digests.push(digest_info);
        }
        if total_size != blob_digest.size_bytes() {
            self.chunking_metrics
                .splice_verification_failures
                .fetch_add(1, Ordering::Relaxed);
            return Err(make_err!(
                Code::InvalidArgument,
                "Sum of chunk sizes ({total_size}) does not match the expected blob size ({}) in splice_blob",
                blob_digest.size_bytes()
            ));
        }

        // One round of existence checks: the chunks (which also touches
        // them, best-effort extending their lifetimes), the blob, and the
        // registered layout.
        let (missing_chunks, blob_exists, layout_exists) = futures::join!(
            Self::missing_chunks(&store, &request.chunk_digests),
            store.has(blob_digest),
            index_store.has(blob_digest),
        );
        let missing_chunks = missing_chunks.err_tip(|| "In splice_blob")?;
        if !missing_chunks.is_empty() {
            return Err(make_err!(
                Code::NotFound,
                "Chunk(s) [{}] not present in the CAS in splice_blob",
                missing_chunks.join(", ")
            ));
        }
        // Fast path: if the blob and its chunk layout are already registered
        // this request is a no-op.
        if blob_exists.err_tip(|| "In splice_blob")?.is_some()
            && layout_exists.err_tip(|| "In splice_blob")?.is_some()
        {
            self.chunking_metrics
                .splice_already_exists
                .fetch_add(1, Ordering::Relaxed);
            return Ok(Response::new(SpliceBlobResponse {
                blob_digest: Some(blob_digest.into()),
            }));
        }

        // Re-assemble the blob into the store: chunk reads are pipelined
        // CHUNK_CONCURRENCY at a time while hashing and channel writes stay
        // in chunk order. The digest is verified before the final EOF is
        // sent, so a digest mismatch aborts the upload before the store
        // commits it.
        let hasher_func = digest_hasher_func_from_context();
        let verification_failed = AtomicBool::new(false);
        let verification_failed_ref = &verification_failed;
        let (tx, rx) = make_buf_channel_pair();
        let send_store = store.clone();
        // `tx` is moved into the future so that an early error return drops
        // it without an EOF, which aborts the in-flight store update instead
        // of leaving it waiting for more data.
        let send_fut = async move {
            let mut tx = tx;
            let mut hasher = hasher_func.hasher();
            let mut fetch_stream = futures::stream::iter(chunk_digests.into_iter().map(
                move |chunk_digest| {
                    let store = send_store.clone();
                    async move {
                        let data = store
                            .get_part_unchunked(chunk_digest, 0, None)
                            .await
                            .err_tip(|| {
                                format!("Failed to read chunk {chunk_digest} in splice_blob")
                            })?;
                        if u64::try_from(data.len()).unwrap_or(0) != chunk_digest.size_bytes() {
                            return Err(make_err!(
                                Code::Internal,
                                "Chunk {chunk_digest} content has length {}, expected {}, in splice_blob",
                                data.len(),
                                chunk_digest.size_bytes()
                            ));
                        }
                        Ok::<Bytes, Error>(data)
                    }
                },
            ))
            .buffered(CHUNK_CONCURRENCY);
            while let Some(data) = fetch_stream.next().await {
                let data = data?;
                hasher.update(&data);
                tx.send(data)
                    .await
                    .err_tip(|| "Failed to send chunk data in splice_blob")?;
            }
            drop(fetch_stream);
            let computed_digest = hasher.finalize_digest();
            if computed_digest != blob_digest {
                verification_failed_ref.store(true, Ordering::Relaxed);
                return Err(make_err!(
                    Code::InvalidArgument,
                    "Digest of spliced blob ({computed_digest}) does not match the expected digest ({blob_digest}) in splice_blob"
                ));
            }
            tx.send_eof()
                .err_tip(|| "Failed to send EOF in splice_blob")?;
            Ok::<(), Error>(())
        };
        let update_fut = store.update(
            blob_digest,
            rx,
            UploadSizeInfo::ExactSize(blob_digest.size_bytes()),
        );
        let (send_res, update_res) = futures::join!(send_fut, update_fut);
        if verification_failed.load(Ordering::Relaxed) {
            self.chunking_metrics
                .splice_verification_failures
                .fetch_add(1, Ordering::Relaxed);
        }
        // Prefer the sender error: it carries the reason the upload was
        // aborted (e.g. the digest mismatch), the store error is usually a
        // consequence; merge keeps both messages when both fail.
        send_res
            .merge(update_res)
            .err_tip(|| "Failed to write spliced blob to store in splice_blob")?;

        // Persist the chunk layout so SplitBlob can serve it later.
        let split_response = SplitBlobResponse {
            chunk_digests: request.chunk_digests,
            chunking_function: request.chunking_function,
        };
        Self::write_chunk_layout(&index_store, blob_digest, &split_response).await?;

        self.chunking_metrics
            .splice_bytes_total
            .fetch_add(blob_digest.size_bytes(), Ordering::Relaxed);
        Ok(Response::new(SpliceBlobResponse {
            blob_digest: Some(blob_digest.into()),
        }))
    }
}

#[tonic::async_trait]
impl ContentAddressableStorage for CasServer {
    type GetTreeStream = GetTreeStream;

    #[instrument(
        err,
        ret(level = Level::DEBUG),
        level = Level::ERROR,
        skip_all,
        fields(
            // Mostly to skip request.blob_digests which is sometimes enormous
            request.instance_name = ?grpc_request.get_ref().instance_name,
            request.digest_function = ?grpc_request.get_ref().digest_function
        )
    )]
    async fn find_missing_blobs(
        &self,
        grpc_request: Request<FindMissingBlobsRequest>,
    ) -> Result<Response<FindMissingBlobsResponse>, Status> {
        let request = grpc_request.into_inner();
        let digest_function = request.digest_function;
        self.inner_find_missing_blobs(request)
            .instrument(error_span!("cas_server_find_missing_blobs"))
            .with_context(
                make_ctx_for_hash_func(digest_function)
                    .err_tip(|| "In CasServer::find_missing_blobs")?,
            )
            .await
            .err_tip(|| "Failed on find_missing_blobs() command")
            .map_err(Into::into)
    }

    #[instrument(
        err,
        ret(level = Level::DEBUG),
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn batch_update_blobs(
        &self,
        grpc_request: Request<BatchUpdateBlobsRequest>,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Status> {
        let request = grpc_request.into_inner();
        let digest_function = request.digest_function;

        self.inner_batch_update_blobs(request)
            .instrument(error_span!("cas_server_batch_update_blobs"))
            .with_context(
                make_ctx_for_hash_func(digest_function)
                    .err_tip(|| "In CasServer::batch_update_blobs")?,
            )
            .await
            .err_tip(|| "Failed on batch_update_blobs() command")
            .map_err(Into::into)
    }

    #[instrument(
        err,
        ret(level = Level::INFO),
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn batch_read_blobs(
        &self,
        grpc_request: Request<BatchReadBlobsRequest>,
    ) -> Result<Response<BatchReadBlobsResponse>, Status> {
        let request = grpc_request.into_inner();
        let digest_function = request.digest_function;

        self.inner_batch_read_blobs(request)
            .instrument(error_span!("cas_server_batch_read_blobs"))
            .with_context(
                make_ctx_for_hash_func(digest_function)
                    .err_tip(|| "In CasServer::batch_read_blobs")?,
            )
            .await
            .err_tip(|| "Failed on batch_read_blobs() command")
            .map_err(Into::into)
    }

    #[instrument(
        err,
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn get_tree(
        &self,
        grpc_request: Request<GetTreeRequest>,
    ) -> Result<Response<Self::GetTreeStream>, Status> {
        let request = grpc_request.into_inner();
        let digest_function = request.digest_function;

        let resp = self
            .inner_get_tree(request)
            .instrument(error_span!("cas_server_get_tree"))
            .with_context(
                make_ctx_for_hash_func(digest_function).err_tip(|| "In CasServer::get_tree")?,
            )
            .await
            .err_tip(|| "Failed on get_tree() command")
            .map(|stream| -> Response<Self::GetTreeStream> { Response::new(Box::pin(stream)) })
            .map_err(Into::into);

        if resp.is_ok() {
            debug!(return = "Ok(<stream>)");
        }
        resp
    }

    #[instrument(
        err,
        ret(level = Level::DEBUG),
        level = Level::ERROR,
        skip_all,
        fields(
            request.instance_name = ?grpc_request.get_ref().instance_name,
            request.blob_digest = ?grpc_request.get_ref().blob_digest,
            request.digest_function = ?grpc_request.get_ref().digest_function,
        )
    )]
    async fn split_blob(
        &self,
        grpc_request: Request<SplitBlobRequest>,
    ) -> Result<Response<SplitBlobResponse>, Status> {
        let request = grpc_request.into_inner();
        let digest_function = request.digest_function;
        self.inner_split_blob(request)
            .instrument(error_span!("cas_server_split_blob"))
            .with_context(
                make_ctx_for_hash_func(digest_function).err_tip(|| "In CasServer::split_blob")?,
            )
            .await
            .err_tip(|| "Failed on split_blob() command")
            .map_err(Into::into)
    }

    #[instrument(
        err,
        ret(level = Level::DEBUG),
        level = Level::ERROR,
        skip_all,
        fields(
            // Skip request.chunk_digests which is sometimes enormous.
            request.instance_name = ?grpc_request.get_ref().instance_name,
            request.blob_digest = ?grpc_request.get_ref().blob_digest,
            request.digest_function = ?grpc_request.get_ref().digest_function,
        )
    )]
    async fn splice_blob(
        &self,
        grpc_request: Request<SpliceBlobRequest>,
    ) -> Result<Response<SpliceBlobResponse>, Status> {
        let request = grpc_request.into_inner();
        let digest_function = request.digest_function;
        self.inner_splice_blob(request)
            .instrument(error_span!("cas_server_splice_blob"))
            .with_context(
                make_ctx_for_hash_func(digest_function).err_tip(|| "In CasServer::splice_blob")?,
            )
            .await
            .err_tip(|| "Failed on splice_blob() command")
            .map_err(Into::into)
    }
}
