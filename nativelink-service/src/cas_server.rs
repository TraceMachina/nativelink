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
use core::pin::Pin;
use std::collections::{HashMap, HashSet, VecDeque};

use bytes::Bytes;
use futures::stream::{FuturesUnordered, Stream};
use futures::{StreamExt, TryStreamExt};
use nativelink_config::cas_server::{CasStoreConfig, WithInstanceName};
use nativelink_error::{Code, Error, ResultExt, error_if, make_input_err};
use nativelink_proto::build::bazel::remote::execution::v2::content_addressable_storage_server::{
    ContentAddressableStorage, ContentAddressableStorageServer as Server,
};
use nativelink_proto::build::bazel::remote::execution::v2::{
    BatchReadBlobsRequest, BatchReadBlobsResponse, BatchUpdateBlobsRequest,
    BatchUpdateBlobsResponse, Directory, FindMissingBlobsRequest, FindMissingBlobsResponse,
    GetTreeRequest, GetTreeResponse, batch_read_blobs_response, batch_update_blobs_response,
    compressor,
};
use nativelink_proto::google::rpc::Status as GrpcStatus;
use nativelink_store::ac_utils::get_and_decode_digest;
use nativelink_store::grpc_store::GrpcStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::make_ctx_for_hash_func;
use nativelink_util::log_utils::throughput_mbps;
use nativelink_util::stall_detector::StallGuard;
use nativelink_util::store_trait::{IS_WORKER_REQUEST, Store, StoreLike};
use opentelemetry::context::FutureExt;
use prost::Message;
use tonic::{Request, Response, Status};
use tracing::{Instrument, Level, debug, error, error_span, info, instrument, warn};

#[derive(Debug)]
pub struct CasServer {
    stores: HashMap<String, Store>,
}

type GetTreeStream = Pin<Box<dyn Stream<Item = Result<GetTreeResponse, Status>> + Send + 'static>>;

impl CasServer {
    pub fn new(
        configs: &[WithInstanceName<CasStoreConfig>],
        store_manager: &StoreManager,
    ) -> Result<Self, Error> {
        let mut stores = HashMap::with_capacity(configs.len());
        for config in configs {
            let store = store_manager.get_store(&config.cas_store).ok_or_else(|| {
                make_input_err!("'cas_store': '{}' does not exist", config.cas_store)
            })?;
            stores.insert(config.instance_name.to_string(), store);
        }
        Ok(Self { stores })
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
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
        let missing_blob_digests: Vec<_> = sizes
            .into_iter()
            .zip(request.blob_digests)
            .filter_map(|(maybe_size, digest)| maybe_size.map_or_else(|| Some(digest), |_| None))
            .collect();

        info!(
            requested = requested_blobs.len(),
            missing = missing_blob_digests.len(),
            "FindMissingBlobs",
        );
        if !missing_blob_digests.is_empty() {
            debug!(
                digests = ?missing_blob_digests.iter().map(|d| format!("{}-{}", d.hash, d.size_bytes)).collect::<Vec<_>>(),
                "FindMissingBlobs: missing digests",
            );
        }

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
        let blob_count = request.requests.len();
        let batch_start = std::time::Instant::now();
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
                info!(
                    %digest_info,
                    size_bytes,
                    "BatchUpdateBlobs: blob received",
                );
                let upload_start = std::time::Instant::now();
                let result = store_ref
                    .update_oneshot(digest_info, request_data)
                    .await
                    .err_tip(|| "Error writing to store");
                match &result {
                    Ok(()) => {
                        let elapsed = upload_start.elapsed();
                        info!(
                            %digest_info,
                            size_bytes,
                            elapsed_ms = elapsed.as_millis() as u64,
                            throughput_mbps = format!("{:.1}", throughput_mbps(size_bytes as u64, elapsed)),
                            "BatchUpdateBlobs: CAS write completed",
                        );
                    }
                    Err(e) => {
                        let elapsed = upload_start.elapsed();
                        warn!(
                            %digest_info,
                            size_bytes,
                            elapsed_ms = elapsed.as_millis() as u64,
                            ?e,
                            "BatchUpdateBlobs: blob upload failed",
                        );
                    }
                }
                Ok::<_, Error>(batch_update_blobs_response::Response {
                    digest: Some(digest),
                    status: Some(result.map_or_else(Into::into, |()| GrpcStatus::default())),
                })
            })
            .collect();
        let responses = update_futures
            .try_collect::<Vec<batch_update_blobs_response::Response>>()
            .await?;

        let batch_elapsed = batch_start.elapsed();
        let total_bytes: usize = responses
            .iter()
            .filter_map(|r| r.digest.as_ref())
            .map(|d| d.size_bytes as usize)
            .sum();
        info!(
            blob_count,
            total_bytes,
            elapsed_ms = batch_elapsed.as_millis() as u64,
            "BatchUpdateBlobs: batch completed",
        );

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
                let read_start = std::time::Instant::now();
                let result = store_ref
                    .get_part_unchunked(digest_copy, 0, None)
                    .await
                    .err_tip(|| "Error reading from store");
                let (status, data) = result.map_or_else(
                    |mut e| {
                        let elapsed = read_start.elapsed();
                        if e.code != Code::NotFound {
                            error!(
                                %digest_copy,
                                elapsed_ms = elapsed.as_millis() as u64,
                                ?e,
                                "BatchReadBlobs: CAS read failed",
                            );
                        }
                        if e.code == Code::NotFound {
                            // Trim the error code. Not Found is quite common and we don't want to send a large
                            // error (debug) message for something that is common. We resize to just the last
                            // message as it will be the most relevant.
                            e.messages.resize_with(1, String::new);
                        }
                        (e.into(), Bytes::new())
                    },
                    |v| {
                        let elapsed = read_start.elapsed();
                        let size_bytes = v.len() as u64;
                        info!(
                            %digest_copy,
                            size_bytes,
                            elapsed_ms = elapsed.as_millis() as u64,
                            throughput_mbps = format!("{:.1}", throughput_mbps(size_bytes, elapsed)),
                            "BatchReadBlobs: CAS read completed",
                        );
                        (GrpcStatus::default(), v)
                    },
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
        let tree_start = std::time::Instant::now();
        let root_digest: DigestInfo = request
            .root_digest
            .err_tip(|| "Expected root_digest to exist in GetTreeRequest")?
            .try_into()
            .err_tip(|| "In GetTreeRequest::root_digest")?;

        let mut deque: VecDeque<DigestInfo> = VecDeque::new();
        // Track all digests we have ever enqueued to avoid fetching/processing
        // the same directory twice. In a Merkle tree, identical subdirectory
        // structures share the same digest, so multiple parents at the same BFS
        // level can reference the same child digest. Without deduplication:
        //   1. We fetch the same blob N times concurrently (wasteful).
        //   2. `level_results.remove()` succeeds for the first occurrence but
        //      returns None for duplicates, causing a spurious
        //      "Directory missing from level results" error.
        let mut seen: HashSet<DigestInfo> = HashSet::new();
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
        // If `page_size` is 0, paging is not necessary — return all directories.
        let page_size_limit = if page_size == 0 {
            usize::MAX
        } else {
            usize::try_from(page_size).unwrap_or(usize::MAX)
        };
        let mut page_token_matched = page_size == 0;
        seen.insert(root_digest);
        deque.push_back(root_digest);
        let mut page_filled = false;

        // Per-level timing and dedup tracking for diagnostics.
        let mut bfs_level: u32 = 0;
        let mut total_duplicates_skipped: u64 = 0;
        let mut level_timings: Vec<(u32, usize, u64, u64)> = Vec::new(); // (level, dirs_fetched, children_discovered, elapsed_ms)

        while !deque.is_empty() && !page_filled {
            let level_start = std::time::Instant::now();
            let level: Vec<DigestInfo> = deque.drain(..).collect();
            // Fetch all directories in this BFS level concurrently.
            let mut futs = FuturesUnordered::new();
            for digest in &level {
                let store = store.clone();
                let digest = *digest;
                futs.push(async move {
                    let dir = get_and_decode_digest::<Directory>(&store, digest.into())
                        .await
                        .err_tip(|| {
                            format!(
                                "Converting digest to Directory (digest: {})",
                                digest,
                            )
                        })?;
                    Ok::<_, Error>((digest, dir))
                });
            }
            // Collect results into a map so we can iterate in deterministic (discovery) order.
            let mut level_results: HashMap<DigestInfo, Directory> =
                HashMap::with_capacity(level.len());
            while let Some(result) = futs.next().await {
                let (digest, directory) = result?;
                level_results.insert(digest, directory);
            }
            // Process directories in the order they appeared in the deque (BFS discovery order).
            let mut level_new_children: u64 = 0;
            let mut level_duplicates: u64 = 0;
            for (i, digest) in level.iter().enumerate() {
                let directory = level_results
                    .get(digest)
                    .cloned()
                    .err_tip(|| {
                        format!(
                            "Directory missing from level results (digest: {}, level_size: {}, results_size: {})",
                            digest,
                            level.len(),
                            level_results.len(),
                        )
                    })?;
                if *digest == page_token_digest {
                    page_token_matched = true;
                }
                // Always enqueue children so BFS traversal finds the page token
                // even when it's deeper in the tree.
                for child in &directory.directories {
                    let child_digest: DigestInfo = child
                        .digest
                        .clone()
                        .err_tip(|| {
                            "Expected Digest to exist in Directory::directories::digest"
                        })?
                        .try_into()
                        .err_tip(|| "In Directory::file::digest")?;
                    // Only enqueue children we haven't seen before to avoid
                    // duplicate fetches and processing.
                    if seen.insert(child_digest) {
                        deque.push_back(child_digest);
                        level_new_children += 1;
                    } else {
                        level_duplicates += 1;
                    }
                }
                if page_token_matched {
                    directories.push(directory);
                    if directories.len() >= page_size_limit {
                        // Put remaining unprocessed items from this level back
                        // into the front of the deque for the next page token.
                        let remaining: Vec<DigestInfo> =
                            level[i + 1..].iter().copied().collect();
                        // Prepend remaining items before any children already in deque.
                        for (j, rem) in remaining.into_iter().enumerate() {
                            deque.insert(j, rem);
                        }
                        page_filled = true;
                        break;
                    }
                }
            }

            let level_elapsed_ms = level_start.elapsed().as_millis() as u64;
            total_duplicates_skipped += level_duplicates;

            if level_duplicates > 0 {
                debug!(
                    ?root_digest,
                    bfs_level,
                    duplicates_skipped = level_duplicates,
                    "GetTree: deduplication skipped children at this level",
                );
            }

            debug!(
                ?root_digest,
                bfs_level,
                dirs_fetched = level.len(),
                new_children = level_new_children,
                duplicates_skipped = level_duplicates,
                elapsed_ms = level_elapsed_ms,
                "GetTree: BFS level completed",
            );

            if level_elapsed_ms > 100 {
                warn!(
                    ?root_digest,
                    bfs_level,
                    dirs_fetched = level.len(),
                    new_children = level_new_children,
                    elapsed_ms = level_elapsed_ms,
                    "GetTree: slow BFS level (>100ms)",
                );
            }

            level_timings.push((bfs_level, level.len(), level_new_children, level_elapsed_ms));
            bfs_level += 1;
        }
        // `next_page_token` will return the `{hash_str}-{size_bytes}` of the next request's first directory digest.
        // It will be an empty string when it reached the end of the directory tree.
        let next_page_token: String = deque
            .front()
            .map_or_else(String::new, |value| format!("{value}"));

        let elapsed = tree_start.elapsed();
        let total_bytes: u64 = directories.iter().map(|d| d.encoded_len() as u64).sum();

        // Build per-level timing breakdown string for the summary log.
        let level_breakdown: String = level_timings
            .iter()
            .map(|(lvl, dirs, children, ms)| {
                format!("L{lvl}:{dirs}dirs/{children}children/{ms}ms")
            })
            .collect::<Vec<_>>()
            .join(", ");

        info!(
            ?root_digest,
            dir_count = directories.len(),
            total_bytes,
            total_duplicates_skipped,
            bfs_levels = bfs_level,
            elapsed_ms = elapsed.as_millis() as u64,
            level_breakdown = %level_breakdown,
            "GetTree: resolved directory tree",
        );

        Ok(futures::stream::once(async {
            Ok(GetTreeResponse {
                directories,
                next_page_token,
            })
        })
        .right_stream())
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

        let _stall_guard = StallGuard::new(
            nativelink_util::stall_detector::DEFAULT_STALL_THRESHOLD,
            "BatchUpdateBlobs",
        );
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
        let is_worker = grpc_request
            .metadata()
            .contains_key("x-nativelink-worker");
        let request = grpc_request.into_inner();
        let digest_function = request.digest_function;

        let _stall_guard = StallGuard::new(
            nativelink_util::stall_detector::DEFAULT_STALL_THRESHOLD,
            "BatchReadBlobs",
        );
        IS_WORKER_REQUEST
            .scope(
                is_worker,
                self.inner_batch_read_blobs(request)
                    .instrument(error_span!("cas_server_batch_read_blobs"))
                    .with_context(
                        make_ctx_for_hash_func(digest_function)
                            .err_tip(|| "In CasServer::batch_read_blobs")?,
                    ),
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
}
