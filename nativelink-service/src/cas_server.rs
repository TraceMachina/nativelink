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
use core::task::{Context, Poll};
use std::collections::{HashMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use futures::stream::{FuturesUnordered, Stream};
use futures::{StreamExt, TryStreamExt};
use nativelink_config::cas_server::{CasStoreConfig, WithInstanceName};
use nativelink_config::stores::EvictionPolicy;
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
use nativelink_store::ac_utils::batch_get_and_decode_digest;
use nativelink_store::grpc_store::GrpcStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_store::worker_proxy_store::WorkerProxyStore;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::make_ctx_for_hash_func;
use nativelink_util::evicting_map::LenEntry;
use nativelink_util::log_utils::throughput_mbps;
use nativelink_util::moka_evicting_map::MokaEvictingMap;
use nativelink_util::stall_detector::StallGuard;
use nativelink_util::store_trait::{IS_MIRROR_REQUEST, IS_WORKER_REQUEST, Store, StoreKey, StoreLike};
use nativelink_util::zero_copy_codec::{
    GrpcUnaryBody, decode_unary_request, encode_grpc_unary_response,
};
use opentelemetry::context::FutureExt;
use prost::Message;
use tokio::sync::watch;
use tonic::{Request, Response, Status};
use tracing::{Instrument, Level, debug, error, error_span, info, instrument, warn};

/// Maximum per-blob size for BatchReadBlobs batch reads (64 MiB).
/// Bounds memory usage per blob when reading through the store chain.
const MAX_BATCH_READ_BLOB_SIZE: u64 = 64 << 20;

/// Maximum total encoded size of cached GetTree results (512 MiB).
const TREE_CACHE_MAX_BYTES: usize = 512 << 20;

/// Maximum number of cached GetTree results.
const TREE_CACHE_MAX_COUNT: u64 = 10_000;

/// TTL for cached GetTree results (5 minutes). CAS trees are immutable
/// (content-addressed), but we expire entries to bound memory usage
/// for trees that aren't re-requested.
const TREE_CACHE_TTL_SECS: u32 = 300;

/// Maximum total encoded size of cached individual directory protos (256 MiB).
/// This cache is populated as a side effect of BFS traversal, so future
/// GetTree calls with overlapping subtrees can skip store fetches for
/// directories already seen.
const SUBTREE_CACHE_MAX_BYTES: usize = 256 << 20;

/// Maximum number of cached individual directory protos.
const SUBTREE_CACHE_MAX_COUNT: u64 = 50_000;

/// TTL for cached individual directory protos (5 minutes).
const SUBTREE_CACHE_TTL_SECS: u32 = 300;

/// A cached GetTree result: the full list of directories for a given
/// root digest. Keyed by `DigestInfo` in the tree cache.
///
/// `directories` is wrapped in `Arc` so cache hits return a cheap
/// reference-count bump instead of deep-cloning every `Directory`.
#[derive(Clone, Debug)]
struct CachedTree {
    directories: Arc<Vec<Directory>>,
    /// Pre-computed total protobuf encoded size for LenEntry.
    encoded_size: u64,
    /// The next_page_token from the full BFS traversal (empty string
    /// when the tree is complete).
    next_page_token: String,
}

impl LenEntry for CachedTree {
    #[inline]
    fn len(&self) -> u64 {
        self.encoded_size
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.directories.is_empty()
    }
}

/// A cached individual `Directory` proto, populated as a side effect of
/// GetTree BFS traversal. When a future BFS encounters a directory
/// digest that's already cached here, it uses the cached proto instead
/// of reading from the store. This avoids redundant fetches for
/// overlapping subtrees across concurrent or sequential GetTree calls
/// (very common in Bazel builds within the same repository).
#[derive(Clone, Debug)]
struct CachedDirectory {
    directory: Directory,
    /// Pre-computed protobuf encoded size for LenEntry.
    encoded_size: u64,
}

impl LenEntry for CachedDirectory {
    #[inline]
    fn len(&self) -> u64 {
        self.encoded_size
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.encoded_size == 0
    }
}

/// Spawn a background task to mirror a blob (with data already in hand)
/// to a random connected worker for OOM redundancy. Fire-and-forget.
fn mirror_blob_to_worker_with_data(store: &Store, digest: DigestInfo, data: Bytes) {
    let Some(_proxy) = store
        .as_store_driver()
        .as_any()
        .downcast_ref::<WorkerProxyStore>()
    else {
        return;
    };

    if digest.size_bytes() == 0 {
        return;
    }

    // Clone the store so the spawned task can access WorkerProxyStore.
    let store = store.clone();
    nativelink_util::background_spawn!("mirror_blob_to_worker", async move {
        let Some(proxy) = store
            .as_store_driver()
            .as_any()
            .downcast_ref::<WorkerProxyStore>()
        else {
            return;
        };
        proxy.mirror_blob_to_random_worker(digest, data).await;
    });
}

#[derive(Debug)]
pub struct CasServer {
    stores: HashMap<String, Store>,
    /// Cache of GetTree results keyed by root digest. CAS trees are
    /// immutable (content-addressed), so a cache hit avoids re-running
    /// the full BFS traversal. Bounded by size and TTL.
    tree_cache: MokaEvictingMap<DigestInfo, DigestInfo, CachedTree, SystemTime>,
    /// Cache of individual directory digests -> their resolved Directory
    /// proto. Populated as a side effect of GetTree BFS. When a future
    /// BFS encounters a directory that's already cached here, it can use
    /// the cached proto instead of reading from the store. This covers
    /// the common case of overlapping subtrees across GetTree calls
    /// (e.g., multiple Bazel targets in the same repo share identical
    /// third_party/ or generated code directories).
    ///
    /// Level 3 optimization (Tree proto lookup) is deferred: GetTree is
    /// keyed by a root Directory digest, but Tree protos are stored
    /// under their own separate digest in the CAS. There is no mapping
    /// from root_directory_digest -> tree_digest in the CAS protocol,
    /// so the server cannot look up a pre-assembled Tree proto given
    /// only the root digest. Supporting this would require either:
    ///   (a) A side index populated from ActionResult output_directories,
    ///       requiring hooks into the AC write path, or
    ///   (b) A separate mapping store (root_digest -> tree_digest).
    /// The subtree cache already covers the main performance win
    /// (avoiding redundant fetches for shared subdirectories), so the
    /// Tree proto lookup is not needed at this time.
    subtree_cache: MokaEvictingMap<DigestInfo, DigestInfo, CachedDirectory, SystemTime>,
    /// In-flight GetTree BFS operations, keyed by root digest. When
    /// multiple concurrent GetTree calls arrive for the same tree,
    /// only the first performs the BFS traversal. Others subscribe to
    /// the watch channel and wait for the result to appear in
    /// `tree_cache`, avoiding thundering-herd redundant traversals.
    tree_inflight: parking_lot::Mutex<HashMap<DigestInfo, watch::Receiver<bool>>>,
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
        let tree_cache_policy = EvictionPolicy {
            max_bytes: TREE_CACHE_MAX_BYTES,
            max_count: TREE_CACHE_MAX_COUNT,
            max_seconds: TREE_CACHE_TTL_SECS,
            ..Default::default()
        };
        let tree_cache = MokaEvictingMap::with_anchor(&tree_cache_policy, SystemTime::now());
        let subtree_cache_policy = EvictionPolicy {
            max_bytes: SUBTREE_CACHE_MAX_BYTES,
            max_count: SUBTREE_CACHE_MAX_COUNT,
            max_seconds: SUBTREE_CACHE_TTL_SECS,
            ..Default::default()
        };
        let subtree_cache =
            MokaEvictingMap::with_anchor(&subtree_cache_policy, SystemTime::now());
        Ok(Self {
            stores,
            tree_cache,
            subtree_cache,
            tree_inflight: parking_lot::Mutex::new(HashMap::new()),
        })
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
    }

    /// Returns the number of entries in the tree cache. Exposed for
    /// integration tests to verify caching behavior.
    #[doc(hidden)]
    pub async fn tree_cache_len(&self) -> usize {
        self.tree_cache.len_for_test().await
    }

    /// Returns the number of entries in the subtree cache. Exposed for
    /// integration tests to verify caching behavior.
    #[doc(hidden)]
    pub async fn subtree_cache_len(&self) -> usize {
        self.subtree_cache.len_for_test().await
    }

    /// Returns the number of in-flight GetTree BFS operations. Exposed
    /// for integration tests to verify coalescing behavior.
    #[doc(hidden)]
    pub fn tree_inflight_len(&self) -> usize {
        self.tree_inflight.lock().len()
    }

    /// Wrap this server in a `ZeroCopyCasService` that intercepts
    /// `BatchUpdateBlobs` RPCs and decodes the request directly from HTTP
    /// body frames, bypassing tonic's `BytesMut` reassembly buffer.
    ///
    /// All other CAS RPCs (FindMissingBlobs, BatchReadBlobs, GetTree)
    /// delegate to the standard tonic path.
    pub fn into_zero_copy_service(
        self,
        max_decoding_message_size: usize,
        max_encoding_message_size: usize,
    ) -> ZeroCopyCasService {
        let inner = Arc::new(self);
        ZeroCopyCasService {
            inner: inner.clone(),
            tonic_service: Server::from_arc(inner)
                .max_decoding_message_size(max_decoding_message_size)
                .max_encoding_message_size(max_encoding_message_size),
        }
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

        debug!(
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
        is_mirror: bool,
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

        // Pre-parse all digests and validate sizes upfront so we can do a
        // single batch has() check instead of N individual checks inside
        // ExistenceCacheStore::update().
        let mut parsed: Vec<(DigestInfo, usize)> = Vec::with_capacity(blob_count);
        for req in &request.requests {
            let digest = req
                .digest
                .clone()
                .err_tip(|| "Digest not found in request")?;
            let digest_info = DigestInfo::try_from(digest)?;
            let size_bytes = usize::try_from(digest_info.size_bytes())
                .err_tip(|| "Digest size_bytes was not convertible to usize")?;
            error_if!(
                size_bytes != req.data.len(),
                "Digest for upload had mismatching sizes, digest said {} data  said {}",
                size_bytes,
                req.data.len()
            );
            parsed.push((digest_info, size_bytes));
        }

        // Batch has() check: skip writes for blobs the store already has.
        let keys: Vec<StoreKey<'_>> = parsed
            .iter()
            .map(|(d, _)| (*d).into())
            .collect();
        let mut has_results = vec![None; keys.len()];
        store_ref
            .has_with_results(&keys, &mut has_results)
            .await
            .err_tip(|| "BatchUpdateBlobs: has_with_results failed")?;
        let skipped = has_results.iter().filter(|r| r.is_some()).count();
        if skipped > 0 {
            info!(
                blob_count,
                skipped,
                "BatchUpdateBlobs: skipping blobs that already exist",
            );
        }

        let update_futures: FuturesUnordered<_> = request
            .requests
            .into_iter()
            .zip(parsed.iter())
            .zip(has_results.iter())
            .map(|((request, &(digest_info, size_bytes)), has_result)| async move {
                // Skip blobs the store already has.
                if has_result.is_some() {
                    return Ok::<batch_update_blobs_response::Response, Error>(
                        batch_update_blobs_response::Response {
                            digest: Some(digest_info.into()),
                            status: Some(GrpcStatus {
                                code: 0, // OK
                                ..Default::default()
                            }),
                        },
                    );
                }
                let request_data = request.data;
                debug!(
                    %digest_info,
                    size_bytes,
                    "BatchUpdateBlobs: blob received",
                );
                // Clone data for mirroring (Bytes clone is O(1) refcount bump).
                let mirror_data = request_data.clone();
                let upload_start = std::time::Instant::now();
                let result = IS_MIRROR_REQUEST.scope(is_mirror, async {
                    store_ref
                        .update_oneshot(digest_info, request_data)
                        .await
                        .err_tip(|| "Error writing to store")
                }).await;
                match &result {
                    Ok(()) => {
                        let elapsed = upload_start.elapsed();
                        debug!(
                            %digest_info,
                            size_bytes,
                            elapsed_ms = elapsed.as_millis() as u64,
                            throughput_mbps = format!("{:.1}", throughput_mbps(size_bytes as u64, elapsed)),
                            "BatchUpdateBlobs: CAS write completed",
                        );
                        // Mirror to a random worker for OOM redundancy.
                        // Skip for mirror writes to avoid feedback loops.
                        if !is_mirror {
                            mirror_blob_to_worker_with_data(store_ref, digest_info, mirror_data);
                        }
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
                    digest: Some(digest_info.into()),
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

    /// Zero-copy BatchUpdateBlobs handler called from `ZeroCopyCasService`.
    ///
    /// The request has already been decoded from the raw HTTP body frames
    /// without copying through tonic's BytesMut reassembly buffer.
    async fn zero_copy_batch_update_blobs(
        &self,
        request: BatchUpdateBlobsRequest,
        is_mirror: bool,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Status> {
        let digest_function = request.digest_function;

        let _stall_guard = StallGuard::new(
            nativelink_util::stall_detector::DEFAULT_STALL_THRESHOLD,
            "BatchUpdateBlobs",
        );
        self.inner_batch_update_blobs(request, is_mirror)
            .instrument(error_span!("cas_server_batch_update_blobs"))
            .with_context(
                make_ctx_for_hash_func(digest_function)
                    .err_tip(|| "In CasServer::batch_update_blobs")?,
            )
            .await
            .err_tip(|| "Failed on batch_update_blobs() command")
            .map_err(Into::into)
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

        // Parse all digests upfront so we can do a single pipelined batch read.
        let mut parsed_digests: Vec<DigestInfo> = Vec::with_capacity(request.digests.len());
        for digest in &request.digests {
            parsed_digests.push(DigestInfo::try_from(digest.clone())?);
        }

        // Use batch_get_part_unchunked which pipelines the underlying I/O
        // (e.g. a single Redis round-trip for all keys instead of N individual ones).
        // Cap per-blob size to bound memory usage across the batch.
        let keys: Vec<_> = parsed_digests.iter().map(|d| StoreKey::Digest(*d)).collect();
        let read_start = std::time::Instant::now();
        let batch_results = store
            .batch_get_part_unchunked(keys, Some(MAX_BATCH_READ_BLOB_SIZE))
            .await;
        let batch_elapsed = read_start.elapsed();

        let mut total_bytes: u64 = 0;
        let responses: Vec<batch_read_blobs_response::Response> = request
            .digests
            .into_iter()
            .zip(parsed_digests.iter())
            .zip(batch_results)
            .map(|((digest, &digest_info), result)| {
                let (status, data) = match result {
                    Err(mut e) => {
                        if e.code != Code::NotFound {
                            error!(
                                %digest_info,
                                elapsed_ms = batch_elapsed.as_millis() as u64,
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
                    }
                    Ok(v) => {
                        total_bytes += v.len() as u64;
                        (GrpcStatus::default(), v)
                    }
                };
                batch_read_blobs_response::Response {
                    status: Some(status),
                    digest: Some(digest),
                    compressor: compressor::Value::Identity.into(),
                    data,
                }
            })
            .collect();

        debug!(
            blob_count = responses.len(),
            total_bytes,
            elapsed_ms = batch_elapsed.as_millis() as u64,
            throughput_mbps = format!("{:.1}", throughput_mbps(total_bytes, batch_elapsed)),
            "BatchReadBlobs: batch completed",
        );

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

        // Cache check: for non-paginated requests (the common case from
        // Bazel), serve from the tree cache to avoid redundant BFS
        // traversals. CAS trees are immutable (content-addressed), so
        // the cached result is always valid.
        let is_unpaginated = request.page_token.is_empty() && request.page_size == 0;

        // For unpaginated requests, coalesce concurrent GetTree calls
        // for the same root digest. Only one request performs the BFS
        // traversal; others wait for it to populate the tree_cache.
        // This prevents thundering-herd when many workers request the
        // same tree simultaneously.
        //
        // `inflight_tx` is Some when we are the "leader" — the first
        // request that registered for this root_digest. On all exit
        // paths (success, error, early return) we must send on it to
        // wake waiters, and remove the entry from `tree_inflight`.
        let mut inflight_tx: Option<watch::Sender<bool>> = None;

        if is_unpaginated {
            if let Some(cached) = self.tree_cache.get(&root_digest).await {
                let elapsed = tree_start.elapsed();
                info!(
                    ?root_digest,
                    dir_count = cached.directories.len(),
                    encoded_size = cached.encoded_size,
                    elapsed_us = elapsed.as_micros() as u64,
                    "GetTree: cache hit",
                );
                return Ok(futures::stream::once(futures::future::ready(
                    Ok(GetTreeResponse {
                        directories: cached.directories.as_ref().clone(),
                        next_page_token: cached.next_page_token,
                    }),
                ))
                .right_stream());
            }

            // Check-and-register in a single lock scope to prevent
            // TOCTOU race where two requests both see no inflight entry
            // and both register as leader.
            let maybe_rx = {
                use std::collections::hash_map::Entry;
                let mut inflight = self.tree_inflight.lock();
                match inflight.entry(root_digest) {
                    Entry::Occupied(entry) => {
                        // Another request is already doing BFS.
                        Some(entry.get().clone())
                    }
                    Entry::Vacant(entry) => {
                        // We are the first — register as leader.
                        let (tx, rx) = watch::channel(false);
                        entry.insert(rx);
                        inflight_tx = Some(tx);
                        None
                    }
                }
            };
            if let Some(mut rx) = maybe_rx {
                // Wait for the leader to complete BFS.
                info!(
                    ?root_digest,
                    "GetTree: coalescing with in-flight BFS traversal",
                );
                // Ignore errors (sender dropped = leader failed/panicked).
                let _ = rx.changed().await;
                // Re-check cache — the leader should have populated it.
                if let Some(cached) = self.tree_cache.get(&root_digest).await {
                    let elapsed = tree_start.elapsed();
                    info!(
                        ?root_digest,
                        dir_count = cached.directories.len(),
                        encoded_size = cached.encoded_size,
                        elapsed_us = elapsed.as_micros() as u64,
                        "GetTree: coalesced cache hit",
                    );
                    return Ok(futures::stream::once(futures::future::ready(
                        Ok(GetTreeResponse {
                            directories: cached.directories.as_ref().clone(),
                            next_page_token: cached.next_page_token,
                        }),
                    ))
                    .right_stream());
                }
                // Leader failed (missing dirs, error, etc.). Fall through
                // and do our own BFS as a non-leader (no inflight_tx).
                warn!(
                    ?root_digest,
                    "GetTree: coalesced request found no cache entry, performing own BFS",
                );
            }
        }

        // BFS traversal. Runs for:
        // - The inflight leader (inflight_tx is Some)
        // - A waiter whose leader failed (inflight_tx is None, is_unpaginated)
        // - Paginated requests (inflight_tx is None, !is_unpaginated)
        let result = self
            .bfs_get_tree(
                &store,
                root_digest,
                &request.page_token,
                request.page_size,
                tree_start,
                is_unpaginated,
            )
            .await;

        // Cleanup: if we are the inflight leader, notify waiters and
        // remove ourselves from the inflight map regardless of outcome.
        if let Some(tx) = inflight_tx {
            // Send wakes all receivers waiting on changed().
            let _ = tx.send(true);
            self.tree_inflight.lock().remove(&root_digest);
        }

        let response = result?;
        Ok(futures::stream::once(futures::future::ready(Ok(response))).right_stream())
    }

    /// Perform the BFS traversal for GetTree. Factored out so the
    /// coalescing logic in `inner_get_tree` can wrap it with inflight
    /// tracking and cleanup.
    async fn bfs_get_tree(
        &self,
        store: &Store,
        root_digest: DigestInfo,
        page_token: &str,
        page_size: i32,
        tree_start: std::time::Instant,
        is_unpaginated: bool,
    ) -> Result<GetTreeResponse, Error> {
        let mut deque: VecDeque<DigestInfo> = VecDeque::with_capacity(64);
        // Track all digests we have ever enqueued to avoid fetching/processing
        // the same directory twice. In a Merkle tree, identical subdirectory
        // structures share the same digest, so multiple parents at the same BFS
        // level can reference the same child digest. Without deduplication:
        //   1. We fetch the same blob N times concurrently (wasteful).
        //   2. `level_results.remove()` succeeds for the first occurrence but
        //      returns None for duplicates, causing a spurious
        //      "Directory missing from level results" error.
        let mut seen: HashSet<DigestInfo> = HashSet::with_capacity(256);
        let mut directories: Vec<Directory> = Vec::with_capacity(256);
        // `page_token` will return the `{hash_str}-{size_bytes}` of the current request's first directory digest.
        let page_token_digest = if page_token.is_empty() {
            root_digest
        } else {
            let mut page_token_parts = page_token.split('-');
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
        let mut total_missing_skipped: u64 = 0;
        let mut total_subtree_cache_hits: u64 = 0;
        let mut level_timings: Vec<(u32, usize, u64, u64, u64)> = Vec::with_capacity(16); // (level, dirs_fetched, children_discovered, elapsed_ms, cache_hits)

        while !deque.is_empty() && !page_filled {
            let level_start = std::time::Instant::now();
            let level: Vec<DigestInfo> = deque.drain(..).collect();

            // Subtree cache lookup: check which directories we already have
            // cached from previous GetTree calls. Only fetch uncached ones
            // from the store (avoids redundant I/O for overlapping subtrees).
            let mut level_results: HashMap<DigestInfo, Directory> =
                HashMap::with_capacity(level.len());
            let mut uncached_digests: Vec<DigestInfo> = Vec::with_capacity(level.len());
            let mut level_cache_hits: u64 = 0;

            for &digest in &level {
                if let Some(cached_dir) = self.subtree_cache.get(&digest).await {
                    level_results.insert(digest, cached_dir.directory);
                    level_cache_hits += 1;
                } else {
                    uncached_digests.push(digest);
                }
            }
            total_subtree_cache_hits += level_cache_hits;

            // Batch-fetch uncached directories using a single pipelined
            // store operation (one Redis round-trip instead of N).
            // Tolerant: missing or corrupt directories are skipped rather
            // than failing the entire GetTree response. The client can
            // fill in gaps via individual directory fetches.
            let mut level_missing: u64 = 0;
            if !uncached_digests.is_empty() {
                let batch_results =
                    batch_get_and_decode_digest::<Directory>(store, &uncached_digests).await;
                for (digest, result) in batch_results {
                    match result {
                        Ok(directory) => {
                            // Populate the subtree cache for future GetTree calls.
                            let encoded_size = directory.encoded_len() as u64;
                            let cached = CachedDirectory {
                                directory: directory.clone(),
                                encoded_size,
                            };
                            drop(self.subtree_cache.insert(digest, cached).await);
                            level_results.insert(digest, directory);
                        }
                        Err(e) => {
                            warn!(
                                ?root_digest,
                                missing_digest = %digest,
                                bfs_level,
                                err = ?e,
                                "GetTree: skipping missing/corrupt directory, client will fetch individually"
                            );
                            level_missing += 1;
                        }
                    }
                }
            }
            total_missing_skipped += level_missing;
            // Process directories in the order they appeared in the deque (BFS discovery order).
            // Missing directories are skipped — the client's parallel BFS fallback
            // will detect gaps and fetch them individually.
            let mut level_new_children: u64 = 0;
            let mut level_duplicates: u64 = 0;
            for (i, digest) in level.iter().enumerate() {
                let Some(directory) = level_results.get(digest).cloned() else {
                    // This directory was missing/corrupt — skip it.
                    // Its children won't be enqueued, but the client will
                    // discover and fetch them via its own tree walk.
                    continue;
                };
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
                dirs_in_level = level.len(),
                subtree_cache_hits = level_cache_hits,
                store_fetched = uncached_digests.len(),
                new_children = level_new_children,
                duplicates_skipped = level_duplicates,
                elapsed_ms = level_elapsed_ms,
                "GetTree: BFS level completed",
            );

            if level_elapsed_ms > 100 {
                warn!(
                    ?root_digest,
                    bfs_level,
                    dirs_in_level = level.len(),
                    subtree_cache_hits = level_cache_hits,
                    store_fetched = uncached_digests.len(),
                    new_children = level_new_children,
                    elapsed_ms = level_elapsed_ms,
                    "GetTree: slow BFS level (>100ms)",
                );
            }

            level_timings.push((bfs_level, level.len(), level_new_children, level_elapsed_ms, level_cache_hits));
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
            .map(|(lvl, dirs, children, ms, cache_hits)| {
                format!("L{lvl}:{dirs}dirs/{cache_hits}cached/{children}children/{ms}ms")
            })
            .collect::<Vec<_>>()
            .join(", ");

        if total_missing_skipped > 0 {
            warn!(
                ?root_digest,
                dir_count = directories.len(),
                total_bytes,
                total_missing_skipped,
                total_duplicates_skipped,
                total_subtree_cache_hits,
                bfs_levels = bfs_level,
                elapsed_ms = elapsed.as_millis() as u64,
                level_breakdown = %level_breakdown,
                "GetTree: resolved directory tree (partial — some directories missing)",
            );
        } else {
            info!(
                ?root_digest,
                dir_count = directories.len(),
                total_bytes,
                total_duplicates_skipped,
                total_subtree_cache_hits,
                bfs_levels = bfs_level,
                elapsed_ms = elapsed.as_millis() as u64,
                level_breakdown = %level_breakdown,
                "GetTree: resolved directory tree",
            );
        }

        // Cache the result for future GetTree calls with the same root
        // digest. Only cache complete, non-paginated results with no
        // missing directories (partial trees could be stale).
        if is_unpaginated && total_missing_skipped == 0 {
            // Move directories into Arc first (zero-copy), give cache a
            // cheap Arc clone, then clone out for the response. Avoids
            // the old Arc::new(directories.clone()) which briefly doubled
            // the directory list in memory.
            let dirs_arc = Arc::new(directories);
            let cached = CachedTree {
                directories: Arc::clone(&dirs_arc),
                encoded_size: total_bytes,
                next_page_token: next_page_token.clone(),
            };
            drop(self.tree_cache.insert(root_digest, cached).await);
            Ok(GetTreeResponse {
                directories: dirs_arc.as_ref().clone(),
                next_page_token,
            })
        } else {
            Ok(GetTreeResponse {
                directories,
                next_page_token,
            })
        }
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
        let is_mirror = grpc_request
            .metadata()
            .contains_key("x-nativelink-mirror");
        let request = grpc_request.into_inner();
        let digest_function = request.digest_function;

        let _stall_guard = StallGuard::new(
            nativelink_util::stall_detector::DEFAULT_STALL_THRESHOLD,
            "BatchUpdateBlobs",
        );
        self.inner_batch_update_blobs(request, is_mirror)
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

/// A tower `Service` wrapper around `CasServer` that intercepts
/// `BatchUpdateBlobs` RPCs and decodes the `BatchUpdateBlobsRequest`
/// directly from raw HTTP body frames, bypassing tonic's `BytesMut`
/// reassembly buffer.
///
/// This preserves zero-copy semantics for `Bytes` fields in the request
/// (specifically `BatchUpdateBlobsRequest.requests[].data`), eliminating
/// one full copy of every blob byte on the inbound path.
///
/// All other CAS RPCs pass through to the inner tonic service unchanged.
#[derive(Clone, Debug)]
pub struct ZeroCopyCasService {
    inner: Arc<CasServer>,
    tonic_service: Server<CasServer>,
}

impl ZeroCopyCasService {
    /// Apply compression settings to the inner tonic service
    /// (for non-BatchUpdateBlobs RPCs).
    pub fn accept_compressed(mut self, encoding: tonic::codec::CompressionEncoding) -> Self {
        self.tonic_service = self.tonic_service.accept_compressed(encoding);
        self
    }

    /// Apply compression settings to the inner tonic service
    /// (for non-BatchUpdateBlobs RPCs).
    pub fn send_compressed(mut self, encoding: tonic::codec::CompressionEncoding) -> Self {
        self.tonic_service = self.tonic_service.send_compressed(encoding);
        self
    }
}

impl tonic::server::NamedService for ZeroCopyCasService {
    const NAME: &'static str =
        "build.bazel.remote.execution.v2.ContentAddressableStorage";
}

impl tower::Service<http::Request<tonic::body::Body>> for ZeroCopyCasService {
    type Response = http::Response<tonic::body::Body>;
    type Error = core::convert::Infallible;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: http::Request<tonic::body::Body>) -> Self::Future {
        let path = req.uri().path();
        if path
            == "/build.bazel.remote.execution.v2.ContentAddressableStorage/BatchUpdateBlobs"
        {
            let inner = self.inner.clone();
            Box::pin(async move {
                let (parts, body) = req.into_parts();
                let is_mirror = parts.headers.contains_key("x-nativelink-mirror");

                // Decode the unary request directly from body frames.
                let request: BatchUpdateBlobsRequest =
                    match decode_unary_request(body).await {
                        Ok(req) => req,
                        Err(status) => return Ok(status.into_http()),
                    };

                let result = inner.zero_copy_batch_update_blobs(request, is_mirror).await;

                match result {
                    Ok(response) => {
                        let (resp_metadata, update_response, _extensions) =
                            response.into_parts();
                        let body_bytes =
                            encode_grpc_unary_response(&update_response);
                        let body = GrpcUnaryBody::new(body_bytes);
                        let mut http_response = http::Response::new(
                            tonic::body::Body::new(body),
                        );
                        *http_response.headers_mut() =
                            resp_metadata.into_headers();
                        http_response.headers_mut().insert(
                            http::header::CONTENT_TYPE,
                            tonic::metadata::GRPC_CONTENT_TYPE,
                        );
                        Ok(http_response)
                    }
                    Err(status) => Ok(status.into_http()),
                }
            })
        } else {
            // Delegate all other RPCs to the standard tonic path.
            self.tonic_service.call(req)
        }
    }
}
