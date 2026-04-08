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

use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use parking_lot::RwLock;
use tokio::sync::{Notify, Semaphore};
use tokio::task::JoinHandle;
use tracing::{debug, info, trace, warn};

use nativelink_config::stores::{ClientTlsConfig, GrpcEndpoint, GrpcSpec, Retry, StoreType};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::blob_locality_map::SharedBlobLocalityMap;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthStatus, HealthStatusIndicator};
use nativelink_util::store_trait::{
    IS_MIRROR_REQUEST, IS_WORKER_REQUEST, ItemCallback, REDIRECT_PREFIX, Store, StoreDriver,
    StoreKey, StoreLike, StoreOptimizations, UploadSizeInfo,
};

use crate::grpc_store::GrpcStore;

/// A store wrapper that transparently proxies CAS reads from workers when
/// the inner store returns NotFound. This enables worker-to-worker blob sharing.
///
/// Behavior:
/// - `get_part()`: Try inner store first. If NotFound, consult the locality map
///   for workers that have the digest, try reading from a worker.
/// - `has()` / `has_with_results()`: Check inner store first. For any digests
///   still missing, consult the locality map — if a worker has the blob, report
///   it as present. This is safe because workers pin blobs until they are
///   uploaded to the server CAS, so a locality entry implies the blob is
///   retrievable (either from the worker or already in the server CAS).
/// - `update()`: Pass through to inner store.
#[derive(MetricsComponent)]
pub struct WorkerProxyStore {
    #[metric(group = "inner_store")]
    inner: Store,
    /// Blob locality map — digest → worker endpoints.
    locality_map: SharedBlobLocalityMap,
    /// Cached GrpcStore connections to worker endpoints.
    worker_connections: RwLock<HashMap<Arc<str>, Store>>,
    /// When true, race peer fetches against server fetches in get_part.
    /// Only workers should enable this — servers should use the sequential
    /// path which generates redirects for workers.
    race_peers: bool,
    /// Optional TLS config for connecting to worker CAS endpoints.
    /// When set, connections use `grpcs://` with this TLS config.
    worker_tls_config: Option<ClientTlsConfig>,
}

impl core::fmt::Debug for WorkerProxyStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WorkerProxyStore")
            .field("inner", &self.inner)
            .field("worker_connections", &self.worker_connections.read().len())
            .finish()
    }
}

/// Returns true if the error code indicates a connection-level failure,
/// meaning the cached connection should be removed.
fn is_connection_error(e: &Error) -> bool {
    matches!(e.code, Code::Unavailable | Code::Unknown)
}

impl WorkerProxyStore {
    pub fn new(inner: Store, locality_map: SharedBlobLocalityMap) -> Arc<Self> {
        Arc::new(Self {
            inner,
            locality_map,
            worker_connections: RwLock::new(HashMap::new()),
            race_peers: false,
            worker_tls_config: None,
        })
    }

    /// Create a new WorkerProxyStore with TLS configuration for
    /// connecting to worker CAS endpoints.
    pub fn new_with_tls(
        inner: Store,
        locality_map: SharedBlobLocalityMap,
        tls_config: ClientTlsConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner,
            locality_map,
            worker_connections: RwLock::new(HashMap::new()),
            race_peers: false,
            worker_tls_config: Some(tls_config),
        })
    }

    /// Enable racing peer fetches against server fetches.
    /// Only workers should call this — servers should leave it disabled.
    pub fn enable_race_peers(&mut self) {
        self.race_peers = true;
    }

    /// Add a worker endpoint to the connection pool.
    pub async fn add_worker_endpoint(&self, endpoint: &str) {
        if self.get_worker_connection(endpoint).is_some() {
            return;
        }
        self.get_or_create_connection(endpoint).await;
    }

    /// Returns the inner (server) store.
    pub fn inner_store(&self) -> &Store {
        &self.inner
    }

    /// Returns the locality map for looking up which peers have which digests.
    pub fn locality_map(&self) -> &SharedBlobLocalityMap {
        &self.locality_map
    }

    /// Returns all currently-connected peer stores.
    pub fn peer_stores(&self) -> HashMap<Arc<str>, Store> {
        self.worker_connections.read().clone()
    }

    /// Remove a worker endpoint from the connection pool.
    pub fn remove_worker_endpoint(&self, endpoint: &str) {
        let mut conns = self.worker_connections.write();
        if conns.remove(endpoint).is_some() {
            info!(endpoint, "WorkerProxyStore: removed worker connection");
        }
    }

    /// Inject a pre-built Store as a worker connection for the given endpoint.
    /// This is primarily useful for testing, where you want to use a MemoryStore
    /// instead of a real GrpcStore.
    pub fn inject_worker_connection(&self, endpoint: &str, store: Store) {
        self.worker_connections
            .write()
            .insert(Arc::from(endpoint), store);
    }

    /// Get a cached connection to a worker endpoint, or None.
    fn get_worker_connection(&self, endpoint: &str) -> Option<Store> {
        self.worker_connections.read().get(endpoint).cloned()
    }

    /// Get or create a connection to a worker endpoint.
    /// Returns None if the connection could not be created.
    async fn get_or_create_connection(&self, endpoint: &str) -> Option<Store> {
        if let Some(store) = self.get_worker_connection(endpoint) {
            return Some(store);
        }
        match self.create_worker_connection(endpoint).await {
            Ok(store) => {
                self.worker_connections
                    .write()
                    .entry(Arc::from(endpoint))
                    .or_insert_with(|| store.clone());
                Some(store)
            }
            Err(e) => {
                trace!(endpoint, ?e, "WorkerProxyStore: failed to connect to peer");
                None
            }
        }
    }

    /// Create a minimal GrpcStore connection to a worker endpoint.
    async fn create_worker_connection(&self, endpoint: &str) -> Result<Store, Error> {
        let spec = GrpcSpec {
            instance_name: String::new(),
            endpoints: vec![GrpcEndpoint {
                address: endpoint.to_string(),
                tls_config: self.worker_tls_config.clone(),
                concurrency_limit: None,
                connect_timeout_s: 5,
                tcp_keepalive_s: 30,
                http2_keepalive_interval_s: 30,
                http2_keepalive_timeout_s: 20,
                tcp_nodelay: true,
                // Use TCP (h2) for worker connections. QUIC was previously
                // used but dominated server CPU (~50%).
                use_http3: false,
            }],
            store_type: StoreType::Cas,
            retry: Retry::default(),
            max_concurrent_requests: 0,
            connections_per_endpoint: 64,
            rpc_timeout_s: 120,
            batch_update_threshold_bytes: 1_048_576, // 1MB: small blobs use BatchUpdateBlobs
            max_concurrent_batch_rpcs: 32,
            parallel_chunk_read_threshold: 8 * 1024 * 1024,
            parallel_chunk_count: 8,
            dual_transport: false,
        };
        let store = GrpcStore::new(&spec)
            .await
            .err_tip(|| format!("Creating worker proxy connection to {endpoint}"))?;
        Ok(Store::new(store))
    }

    /// Try to read a blob from a specific list of peer endpoints (e.g. from
    /// a redirect response). Same logic as `try_read_from_worker` but uses
    /// the caller-provided endpoints instead of consulting the locality map.
    async fn try_read_from_endpoints(
        &self,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
        endpoints: &[String],
    ) -> Result<bool, Error> {
        let digest = key.borrow().into_digest();
        debug!(
            ?digest,
            endpoint_count = endpoints.len(),
            "WorkerProxyStore: following redirect to peer endpoints"
        );

        for endpoint in endpoints {
            let Some(store) = self.get_or_create_connection(endpoint).await else {
                continue;
            };

            match self
                .get_part_and_cache(&store, key.borrow(), &mut *writer, offset, length)
                .await
            {
                Ok(()) => {
                    debug!(
                        ?digest,
                        endpoint = endpoint.as_str(),
                        "WorkerProxyStore: successfully read blob from redirected peer"
                    );
                    return Ok(true);
                }
                Err(e) => {
                    if is_connection_error(&e) {
                        self.remove_worker_endpoint(endpoint);
                    }
                    warn!(
                        ?digest,
                        endpoint = endpoint.as_str(),
                        ?e,
                        "WorkerProxyStore: read from redirected peer failed, trying next"
                    );
                    continue;
                }
            }
        }

        Ok(false)
    }

    /// Try to read a blob from a worker that has it, according to the locality map.
    ///
    /// Streams from the peer to the caller's writer via `get_part_and_cache()`,
    /// which tees the data to both the caller and the inner store for caching
    /// (for full-blob reads within the size limit). If a peer fails mid-stream,
    /// we resume from the next peer at the byte offset where the previous one
    /// left off (content-addressed blobs are identical across peers).
    async fn try_read_from_worker(
        &self,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<bool, Error> {
        let digest = key.borrow().into_digest();
        let workers = self.locality_map.read().lookup_workers(&digest);

        if workers.is_empty() {
            return Ok(false);
        }

        debug!(
            ?digest,
            worker_count = workers.len(),
            "WorkerProxyStore: attempting to proxy blob from workers"
        );

        // Track how many bytes have been written so we can resume from the
        // correct offset if a streaming peer fails mid-transfer.
        let bytes_before_proxy = writer.get_bytes_written();
        let mut current_offset = offset;
        let mut remaining_length = length;

        for endpoint in &workers {
            let Some(store) = self.get_or_create_connection(endpoint).await else {
                continue;
            };

            // Stream from the peer, caching in the inner store when possible.
            // On failure, compute how many bytes were written and resume
            // from the next peer at the correct offset.
            match self
                .get_part_and_cache(&store, key.borrow(), &mut *writer, current_offset, remaining_length)
                .await
            {
                Ok(()) => {
                    info!(
                        ?digest,
                        endpoint = %endpoint,
                        "WorkerProxyStore: successfully proxied blob from worker"
                    );
                    return Ok(true);
                }
                Err(e) => {
                    if is_connection_error(&e) {
                        self.remove_worker_endpoint(endpoint);
                    }
                    let bytes_written_total =
                        writer.get_bytes_written() - bytes_before_proxy;
                    warn!(
                        ?digest,
                        endpoint = %endpoint,
                        bytes_written_total,
                        ?e,
                        "WorkerProxyStore: streaming get_part from peer failed, \
                         will resume from next peer at offset {}",
                        offset + bytes_written_total,
                    );
                    // Advance offset so the next peer picks up where this one left off.
                    current_offset = offset + bytes_written_total;
                    if let Some(len) = remaining_length {
                        remaining_length =
                            Some(len.saturating_sub(bytes_written_total));
                    }
                    continue;
                }
            }
        }

        Ok(false)
    }

    /// Maximum blob size to buffer and cache in the inner store after a
    /// successful proxy read. Blobs larger than this are streamed directly
    /// without caching, to avoid excessive memory usage.
    const MAX_CACHE_BLOB_SIZE: u64 = 64 * 1024 * 1024; // 64 MiB

    /// Wrapper around a peer's `get_part` that tees the data to both the
    /// caller's writer and a background write to the inner store.
    ///
    /// For full-blob reads (offset=0, length=None) of blobs within the
    /// size limit, the data is collected during streaming and written to
    /// `self.inner` in a background task after success. For partial reads
    /// or oversized blobs, streams directly without caching.
    async fn get_part_and_cache(
        &self,
        peer_store: &Store,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        let digest = key.borrow().into_digest();

        // Only cache full-blob reads for blobs within the size limit.
        let should_cache = offset == 0
            && length.is_none()
            && digest.size_bytes() <= Self::MAX_CACHE_BLOB_SIZE;

        if !should_cache {
            return peer_store
                .get_part(key, &mut *writer, offset, length)
                .await;
        }

        // Create an intermediate channel so we can tee the data to both the
        // caller's writer and a concurrent inner store write.
        let (mut proxy_tx, mut proxy_rx) = make_buf_channel_pair();
        let (mut cache_tx, cache_rx) = make_buf_channel_pair();

        // Run the peer's get_part concurrently with forwarding, because the
        // buf_channel has limited capacity and the producer will block if
        // we don't consume data as it arrives.
        let owned_key = key.borrow().into_owned();
        let peer = peer_store.clone();
        let get_part_fut = async move {
            peer.get_part(owned_key.borrow(), &mut proxy_tx, offset, length)
                .await
        };

        // Start the inner store write concurrently. If the blob size is known
        // from the digest, use ExactSize; otherwise MaxSize.
        let inner = self.inner.clone();
        let cache_size = UploadSizeInfo::ExactSize(digest.size_bytes());
        let cache_key: StoreKey<'static> = digest.into();
        let cache_write_fut = async move {
            inner.update(cache_key, cache_rx, cache_size).await
        };

        let mut total_bytes: u64 = 0;
        let forward_fut = async {
            loop {
                match proxy_rx.recv().await {
                    Ok(chunk) if chunk.is_empty() => {
                        writer
                            .send_eof()
                            .err_tip(|| "get_part_and_cache: forwarding EOF")?;
                        cache_tx
                            .send_eof()
                            .err_tip(|| "get_part_and_cache: cache EOF")?;
                        break;
                    }
                    Ok(chunk) => {
                        total_bytes += chunk.len() as u64;
                        // Send to inner store write (clone is O(1) refcount bump).
                        if let Err(e) = cache_tx.send(chunk.clone()).await {
                            // Cache write failed; log but continue serving the caller.
                            warn!(
                                %digest,
                                ?e,
                                "get_part_and_cache: cache channel send failed, \
                                 skipping cache"
                            );
                            // Drop the cache writer so the cache_write_fut finishes.
                            drop(cache_tx);
                            // Forward remaining data without caching.
                            writer
                                .send(chunk)
                                .await
                                .err_tip(|| "get_part_and_cache: forwarding chunk")?;
                            loop {
                                match proxy_rx.recv().await {
                                    Ok(c) if c.is_empty() => {
                                        writer.send_eof().err_tip(
                                            || "get_part_and_cache: forwarding EOF (no cache)",
                                        )?;
                                        return Ok::<(), Error>(());
                                    }
                                    Ok(c) => {
                                        writer.send(c).await.err_tip(
                                            || "get_part_and_cache: forwarding chunk (no cache)",
                                        )?;
                                    }
                                    Err(e) => {
                                        return Err(e).err_tip(
                                            || "get_part_and_cache: proxy channel (no cache)",
                                        );
                                    }
                                }
                            }
                        }
                        writer
                            .send(chunk)
                            .await
                            .err_tip(|| "get_part_and_cache: forwarding chunk")?;
                    }
                    Err(e) => {
                        return Err(e)
                            .err_tip(|| "get_part_and_cache: reading from proxy channel");
                    }
                }
            }
            Ok::<(), Error>(())
        };

        let (get_part_result, forward_result, cache_result) =
            tokio::join!(get_part_fut, forward_fut, cache_write_fut);

        // If forwarding failed, propagate that error.
        forward_result?;
        // If the peer's get_part failed, propagate that error.
        get_part_result?;

        // Log cache write result (non-fatal).
        match cache_result {
            Ok(()) => {
                debug!(
                    %digest,
                    size_bytes = total_bytes,
                    "proxy_cache: cached proxied blob in inner store"
                );
            }
            Err(e) => {
                warn!(
                    %digest,
                    size_bytes = total_bytes,
                    ?e,
                    "proxy_cache: failed to cache proxied blob in inner store"
                );
            }
        }

        Ok(())
    }

    /// The original sequential get_part logic: try inner store, then parse
    /// redirects, then fall back to locality map / peer proxying.
    /// This is used as the fallback when no peers are known for racing.
    async fn get_part_sequential(
        &self,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        let mut redirect_endpoints: Option<Vec<String>> = None;
        match IS_WORKER_REQUEST
            .scope(
                true,
                self.inner.get_part(key.borrow(), &mut *writer, offset, length),
            )
            .await
        {
            Ok(()) => return Ok(()),
            Err(e) if e.code == Code::NotFound => {
                trace!(
                    key = ?key.borrow().into_digest(),
                    "WorkerProxyStore: inner store miss (NotFound), consulting locality map"
                );
            }
            Err(e) if e.code == Code::FailedPrecondition => {
                let msg = e.message_string();
                if let Some(start) = msg.find(REDIRECT_PREFIX) {
                    let endpoints_str = &msg[start + REDIRECT_PREFIX.len()..];
                    let endpoints_str = endpoints_str
                        .split('|')
                        .next()
                        .unwrap_or(endpoints_str);
                    let endpoints: Vec<String> = endpoints_str
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .map(String::from)
                        .collect();
                    if !endpoints.is_empty() {
                        debug!(
                            key = ?key.borrow().into_digest(),
                            ?endpoints,
                            "WorkerProxyStore: received redirect from inner store"
                        );
                        redirect_endpoints = Some(endpoints);
                    }
                }
                if redirect_endpoints.is_none() {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }

        let is_worker = IS_WORKER_REQUEST.try_with(|v| *v).unwrap_or(false);

        if let Some(endpoints) = redirect_endpoints {
            // For worker requests, pass the redirect through instead of
            // following it — workers should fetch from peers directly.
            if is_worker {
                let digest = key.borrow().into_digest();
                let ep_str = endpoints.join(",");
                debug!(
                    ?digest,
                    endpoints = ep_str.as_str(),
                    "WorkerProxyStore: passing redirect through to worker"
                );
                return Err(make_err!(
                    Code::FailedPrecondition,
                    "{REDIRECT_PREFIX}{ep_str}|"
                ));
            }
            if self
                .try_read_from_endpoints(key.borrow(), writer, offset, length, &endpoints)
                .await?
            {
                return Ok(());
            }
        }

        if is_worker {
            // When a worker asks the server for a blob that the server doesn't
            // have, return NotFound directly. Do NOT generate a redirect to
            // other workers — that creates a loop: worker → server → redirect
            // to workers → workers ask server → redirect → ...
            // Workers handle their own peer fetching via WorkerProxyStore on
            // the worker side with race_peers enabled.
            let digest = key.borrow().into_digest();
            return Err(make_err!(
                Code::NotFound,
                "Blob {digest:?} not found in inner store (worker request, no redirect)"
            ));
        }

        let bytes_before_workers = writer.get_bytes_written();
        if self
            .try_read_from_worker(key.borrow(), writer, offset, length)
            .await?
        {
            return Ok(());
        }

        // All workers failed. The blob may have arrived in the inner store
        // while we were trying workers (e.g. another client uploaded it, or
        // a backfill completed). Re-check before giving up.
        //
        // Only safe to retry if no bytes were written to the writer by any
        // worker — otherwise the consumer would receive overlapping data.
        let bytes_written_by_workers = writer.get_bytes_written() - bytes_before_workers;
        if bytes_written_by_workers > 0 {
            return Err(make_err!(
                Code::Internal,
                "Blob {:?} worker transfer wrote {} bytes then failed, \
                 cannot retry inner store without data corruption",
                key.borrow().into_digest(),
                bytes_written_by_workers
            ));
        }
        match self
            .inner
            .get_part(key.borrow(), writer, offset, length)
            .await
        {
            Ok(()) => {
                info!(
                    digest = ?key.borrow().into_digest(),
                    "WorkerProxyStore: inner store retry succeeded after all workers failed"
                );
                return Ok(());
            }
            Err(e) if e.code == Code::NotFound => {
                // Still not found — fall through to the final error.
            }
            Err(e) => return Err(e),
        }

        Err(make_err!(
            Code::NotFound,
            "Blob {:?} not found in inner store or any worker",
            key.borrow().into_digest()
        ))
    }

    /// Forward remaining data from a racer's read half to the caller's writer,
    /// then wait for the spawned task to complete.
    async fn forward_racer(
        winner_name: &str,
        writer: &mut DropCloserWriteHalf,
        rx: &mut DropCloserReadHalf,
        handle: JoinHandle<Result<(), Error>>,
    ) -> Result<(), Error> {
        // Forward all remaining chunks from the racer's channel to the
        // caller's writer. bind_buffered handles EOF propagation.
        writer
            .bind_buffered(rx)
            .await
            .err_tip(|| format!("WorkerProxyStore: {winner_name} racer bind_buffered"))?;

        // Wait for the spawned get_part to confirm it finished successfully.
        // If the task was already done (sent EOF), this returns immediately.
        handle
            .await
            .map_err(|e| make_err!(Code::Internal, "WorkerProxyStore: {winner_name} task join error: {e}"))?
            .err_tip(|| format!("WorkerProxyStore: {winner_name} get_part failed after winning race"))
    }

    /// Mirror a blob to a random connected worker for OOM redundancy.
    /// Fire-and-forget: errors are logged but do not propagate.
    /// The blob data is passed as `Bytes` to avoid re-reading from the store.
    /// Threshold above which mirror uses streaming `update()` instead of
    /// `update_oneshot()`. 4 MiB is well under the 64 MiB gRPC max message
    /// size, giving headroom for framing overhead.
    const MIRROR_CHUNK_THRESHOLD: usize = 4 * 1024 * 1024;

    /// Chunk size for the streaming mirror path. 3 MiB matches the
    /// `max_bytes_per_stream` default used by ByteStream configs.
    const MIRROR_CHUNK_SIZE: usize = 3 * 1024 * 1024;

    pub async fn mirror_blob_to_random_worker(
        &self,
        digest: DigestInfo,
        data: Bytes,
    ) {
        // Limit concurrent mirror operations so a burst of hundreds of
        // blobs doesn't spawn unbounded tasks against the GrpcStore.
        // 64 permits keeps the network busy without resource exhaustion.
        static MIRROR_SEMAPHORE: Semaphore = Semaphore::const_new(64);

        let _permit = match MIRROR_SEMAPHORE.acquire().await {
            Ok(p) => p,
            Err(_) => return, // semaphore closed, should not happen
        };

        let endpoints = self.locality_map.read().all_endpoints();
        if endpoints.is_empty() {
            return;
        }

        // Pick a random endpoint using the atomic counter to avoid
        // pulling in the `rand` crate. Simple round-robin is fine
        // since the goal is distribution, not cryptographic randomness.
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let idx = COUNTER.fetch_add(1, Ordering::Relaxed) as usize % endpoints.len();
        let endpoint = &endpoints[idx];

        let Some(store) = self.get_or_create_connection(endpoint).await else {
            warn!(
                %digest,
                endpoint = endpoint.as_ref(),
                "mirror: failed to connect to worker"
            );
            return;
        };

        let size_bytes = data.len();
        let result = IS_MIRROR_REQUEST.scope(true, async {
            if size_bytes > Self::MIRROR_CHUNK_THRESHOLD {
                // Large blob: stream in chunks to stay under gRPC max message size.
                let (mut tx, rx) = make_buf_channel_pair();
                let chunk_size = Self::MIRROR_CHUNK_SIZE;
                let data_for_sender = data;
                tokio::spawn(async move {
                    let mut offset = 0;
                    while offset < data_for_sender.len() {
                        let end = (offset + chunk_size).min(data_for_sender.len());
                        let chunk = data_for_sender.slice(offset..end);
                        if tx.send(chunk).await.is_err() {
                            return;
                        }
                        offset = end;
                    }
                    drop(tx.send_eof());
                });
                let key: StoreKey<'_> = digest.into();
                store
                    .update(key, rx, UploadSizeInfo::ExactSize(size_bytes as u64))
                    .await
            } else {
                // Small blob: single-message oneshot is more efficient.
                store.update_oneshot(digest, data).await
            }
        }).await;

        match result {
            Ok(()) => {
                info!(
                    %digest,
                    size_bytes,
                    endpoint = endpoint.as_ref(),
                    "mirror: blob sent to worker"
                );
            }
            Err(e) => {
                warn!(
                    %digest,
                    size_bytes,
                    endpoint = endpoint.as_ref(),
                    ?e,
                    "mirror: failed to send blob to worker"
                );
            }
        }
    }

    /// Mirror a blob to a random connected worker via a streaming channel.
    /// The caller provides a `DropCloserReadHalf` that produces the blob data.
    /// Fire-and-forget semantics: errors are logged but do not propagate.
    pub async fn mirror_blob_via_stream(
        &self,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
    ) {
        static MIRROR_SEMAPHORE: Semaphore = Semaphore::const_new(64);

        let _permit = match MIRROR_SEMAPHORE.acquire().await {
            Ok(p) => p,
            Err(_) => {
                drop(reader);
                return;
            }
        };

        let endpoints = self.locality_map.read().all_endpoints();
        if endpoints.is_empty() {
            // No workers — drain the reader so the sender doesn't block.
            drop(reader);
            return;
        }

        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let idx = COUNTER.fetch_add(1, Ordering::Relaxed) as usize % endpoints.len();
        let endpoint = &endpoints[idx];

        let Some(store) = self.get_or_create_connection(endpoint).await else {
            warn!(
                %digest,
                endpoint = endpoint.as_ref(),
                "mirror_stream: failed to connect to worker"
            );
            drop(reader);
            return;
        };

        let size_bytes = digest.size_bytes();
        let key: StoreKey<'_> = digest.into();
        let result = IS_MIRROR_REQUEST.scope(true, async {
            store
                .update(key, reader, UploadSizeInfo::ExactSize(size_bytes))
                .await
        }).await;

        match &result {
            Ok(()) => {
                info!(
                    %digest,
                    size_bytes,
                    endpoint = endpoint.as_ref(),
                    "mirror_stream: blob streamed to worker"
                );
            }
            Err(e) => {
                warn!(
                    %digest,
                    size_bytes,
                    endpoint = endpoint.as_ref(),
                    ?e,
                    "mirror_stream: failed to stream blob to worker"
                );
            }
        }
    }
}

#[async_trait]
impl StoreDriver for WorkerProxyStore {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        // Only check the inner store — do NOT consult the locality map.
        //
        // The locality map tracks blobs that workers reported via
        // BlobsAvailable, but those blobs may be evicted from the
        // worker at any time. Reporting them as "present" here causes
        // FindMissingBlobs to tell Bazel the blob exists, so Bazel
        // skips uploading it. When the blob is later needed (GetTree,
        // BatchReadBlobs, resolve_tree_from_cas), neither the server's
        // CAS nor the worker has it — causing NotFound errors and
        // 13-19s fallback to recursive directory fetch.
        //
        // The locality map is still used in get_part() for read
        // optimization: if a blob is missing from the inner store but
        // a worker has it, get_part() can proxy the read. This is safe
        // because get_part() handles NotFound gracefully, whereas
        // has_with_results() drives upload decisions that cannot be
        // retried.
        self.inner.has_with_results(digests, results).await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        // Pass through to inner store.
        self.inner.update(key, reader, upload_size).await
    }

    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        // Report LazyExistenceOnSync so that FastSlowStore skips the has()
        // check before get_part(). get_part() handles redirect/proxy logic
        // via the locality map that has_with_results() intentionally skips.
        if optimization == StoreOptimizations::LazyExistenceOnSync {
            return true;
        }
        self.inner
            .inner_store(None::<StoreKey<'_>>)
            .optimized_for(optimization)
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        // Only race when explicitly enabled (worker side). Server-side
        // WorkerProxyStore uses the sequential path which generates
        // redirects for workers and proxies for non-worker callers.
        let digest = key.borrow().into_digest();
        let peers = if self.race_peers {
            self.locality_map.read().lookup_workers(&digest)
        } else {
            Vec::new()
        };

        if peers.is_empty() {
            // No peers known (or server side) — use the sequential path.
            return self
                .get_part_sequential(key, writer, offset, length)
                .await;
        }

        // Try to get a connection to the first peer.
        let peer_store = match self.get_or_create_connection(&peers[0]).await {
            Some(store) => store,
            None => {
                return self
                    .get_part_sequential(key, writer, offset, length)
                    .await;
            }
        };
        let peer_endpoint: Arc<str> = peers[0].clone();

        // Create buf_channel pairs for each racer. Each spawned task writes
        // into its own tx; we read from the rx to see who produces data first.
        let (mut server_tx, mut server_rx) = make_buf_channel_pair();
        let (mut peer_tx, mut peer_rx) = make_buf_channel_pair();

        // We need owned keys for the spawned tasks.
        let server_key = key.borrow().into_owned();
        let peer_key = key.borrow().into_owned();

        // Clone inner store for the server task.
        let inner = self.inner.clone();

        // Spawn server fetch. Do NOT set IS_WORKER_REQUEST — we want the
        // server to actually serve the blob data, not return a redirect.
        let server_handle: JoinHandle<Result<(), Error>> = tokio::spawn(async move {
            inner
                .get_part(server_key.borrow(), &mut server_tx, offset, length)
                .await
        });

        // Spawn peer fetch.
        let peer_handle: JoinHandle<Result<(), Error>> = tokio::spawn(async move {
            peer_store
                .get_part(peer_key.borrow(), &mut peer_tx, offset, length)
                .await
        });

        // Race: wait for the first racer to produce a data chunk (or error).
        tokio::select! {
            server_result = server_rx.recv() => {
                match server_result {
                    Ok(chunk) if !chunk.is_empty() => {
                        // Server produced data first — it wins.
                        peer_handle.abort();
                        debug!(
                            ?digest,
                            "WorkerProxyStore: server won race against peer"
                        );
                        writer.send(chunk).await
                            .err_tip(|| "WorkerProxyStore: sending server winner chunk")?;
                        Self::forward_racer("server", writer, &mut server_rx, server_handle).await
                    }
                    Ok(_empty) => {
                        // Server returned EOF immediately (zero-length blob).
                        peer_handle.abort();
                        debug!(
                            ?digest,
                            "WorkerProxyStore: server won race (empty blob)"
                        );
                        writer.send_eof()
                            .err_tip(|| "WorkerProxyStore: sending EOF for empty blob")?;
                        server_handle.await
                            .map_err(|e| make_err!(Code::Internal, "server task join: {e}"))?
                    }
                    Err(_server_err) => {
                        // Server racer failed — wait for peer.
                        warn!(
                            ?digest,
                            "WorkerProxyStore: server racer failed, waiting for peer"
                        );
                        let peer_chunk = peer_rx.recv().await
                            .err_tip(|| "WorkerProxyStore: peer recv after server failure")?;
                        if peer_chunk.is_empty() {
                            writer.send_eof()
                                .err_tip(|| "WorkerProxyStore: peer EOF after server failure")?;
                            return peer_handle.await
                                .map_err(|e| make_err!(Code::Internal, "peer task join: {e}"))?;
                        }
                        debug!(
                            ?digest,
                            endpoint = %peer_endpoint,
                            "WorkerProxyStore: peer won race (server failed)"
                        );
                        writer.send(peer_chunk).await
                            .err_tip(|| "WorkerProxyStore: sending peer fallback chunk")?;
                        Self::forward_racer("peer", writer, &mut peer_rx, peer_handle).await
                    }
                }
            }
            peer_result = peer_rx.recv() => {
                match peer_result {
                    Ok(chunk) if !chunk.is_empty() => {
                        // Peer produced data first — it wins.
                        server_handle.abort();
                        debug!(
                            ?digest,
                            endpoint = %peer_endpoint,
                            "WorkerProxyStore: peer won race against server"
                        );
                        writer.send(chunk).await
                            .err_tip(|| "WorkerProxyStore: sending peer winner chunk")?;
                        Self::forward_racer("peer", writer, &mut peer_rx, peer_handle).await
                    }
                    Ok(_empty) => {
                        // Peer returned EOF immediately (zero-length blob).
                        server_handle.abort();
                        debug!(
                            ?digest,
                            endpoint = %peer_endpoint,
                            "WorkerProxyStore: peer won race (empty blob)"
                        );
                        writer.send_eof()
                            .err_tip(|| "WorkerProxyStore: sending EOF for empty blob from peer")?;
                        peer_handle.await
                            .map_err(|e| make_err!(Code::Internal, "peer task join: {e}"))?
                    }
                    Err(_peer_err) => {
                        // Peer racer failed — wait for server.
                        warn!(
                            ?digest,
                            endpoint = %peer_endpoint,
                            "WorkerProxyStore: peer racer failed, waiting for server"
                        );
                        let server_chunk = server_rx.recv().await
                            .err_tip(|| "WorkerProxyStore: server recv after peer failure")?;
                        if server_chunk.is_empty() {
                            writer.send_eof()
                                .err_tip(|| "WorkerProxyStore: server EOF after peer failure")?;
                            return server_handle.await
                                .map_err(|e| make_err!(Code::Internal, "server task join: {e}"))?;
                        }
                        debug!(
                            ?digest,
                            "WorkerProxyStore: server won race (peer failed)"
                        );
                        writer.send(server_chunk).await
                            .err_tip(|| "WorkerProxyStore: sending server fallback chunk")?;
                        Self::forward_racer("server", writer, &mut server_rx, server_handle).await
                    }
                }
            }
        }
    }

    fn inner_store(&self, key: Option<StoreKey>) -> &dyn StoreDriver {
        // Delegate to inner store so that callers can downcast through
        // the chain (e.g. worker finding FastSlowStore via downcast_ref).
        // WorkerProxyStore's optimized_for override is independent of this.
        self.inner.inner_store(key)
    }

    fn as_any<'a>(&'a self) -> &'a (dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_item_callback(
        self: Arc<Self>,
        callback: Arc<dyn ItemCallback>,
    ) -> Result<(), Error> {
        self.inner.register_item_callback(callback)
    }

    fn drain_stable_digests(&self) -> Vec<DigestInfo> {
        self.inner.drain_stable_digests()
    }

    fn stable_notify(&self) -> Arc<Notify> {
        self.inner.stable_notify()
    }

    fn pin_digests(&self, digests: &[DigestInfo]) {
        self.inner.pin_digests(digests);
    }

    fn drain_failed_digests(&self) -> Vec<DigestInfo> {
        self.inner.drain_failed_digests()
    }
}

#[async_trait]
impl HealthStatusIndicator for WorkerProxyStore {
    fn get_name(&self) -> &'static str {
        "WorkerProxyStore"
    }

    async fn check_health(
        &self,
        namespace: Cow<'static, str>,
    ) -> HealthStatus {
        self.inner.check_health(namespace).await
    }
}

#[cfg(test)]
mod tests {
    use bytes::Bytes;
    use nativelink_config::stores::MemorySpec;
    use nativelink_error::{Code, Error, make_err};
    use nativelink_macro::nativelink_test;
    use nativelink_util::blob_locality_map::new_shared_blob_locality_map;
    use nativelink_util::common::DigestInfo;
    use nativelink_util::store_trait::{
        IS_WORKER_REQUEST, REDIRECT_PREFIX, StoreLike, StoreKey, StoreOptimizations,
    };
    use pretty_assertions::assert_eq;

    use super::*;
    use crate::memory_store::MemoryStore;

    const VALID_HASH1: &str =
        "0123456789abcdef000000000000000000010000000000000123456789abcdef";
    const VALID_HASH2: &str =
        "0123456789abcdef000000000000000000020000000000000123456789abcdef";

    /// Helper: create a WorkerProxyStore backed by a fresh MemoryStore.
    fn make_proxy_store() -> (Store, SharedBlobLocalityMap) {
        let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
        let locality_map = new_shared_blob_locality_map();
        let proxy = WorkerProxyStore::new(inner, locality_map.clone());
        (Store::new(proxy), locality_map)
    }

    // ---------------------------------------------------------------
    // 1. Inner store hit returns data without consulting locality map.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_inner_store_hit_skips_locality() -> Result<(), Error> {
        let (store, locality_map) = make_proxy_store();

        let value = b"hello world";
        let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

        // Write the blob into the inner store via the proxy.
        store
            .update_oneshot(digest, Bytes::from_static(value))
            .await?;

        // Register a fake worker in the locality map so we can verify
        // it is NOT contacted when the inner store already has the blob.
        locality_map
            .write()
            .register_blobs("fake-worker:50081", &[digest]);

        // Read the blob back — should succeed from the inner store.
        let result = store
            .get_part_unchunked(digest, 0, None)
            .await?;
        assert_eq!(result.as_ref(), value);

        Ok(())
    }

    // ---------------------------------------------------------------
    // 2. Inner store miss + empty locality map => NotFound.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_inner_store_miss_no_peers_returns_not_found() -> Result<(), Error> {
        let (store, _locality_map) = make_proxy_store();

        let digest = DigestInfo::try_new(VALID_HASH1, 100)?;

        // The inner store is empty and the locality map has no entries.
        let result = store.get_part_unchunked(digest, 0, None).await;

        assert!(result.is_err(), "Expected NotFound error");
        let err = result.unwrap_err();
        assert_eq!(
            err.code,
            Code::NotFound,
            "Expected NotFound code, got: {err:?}"
        );

        Ok(())
    }

    // ---------------------------------------------------------------
    // 3. Inner store miss + locality has peers but no gRPC connections
    //    => falls through gracefully and returns NotFound.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_inner_store_miss_locality_has_peers_but_no_connections()
        -> Result<(), Error>
    {
        let (store, locality_map) = make_proxy_store();

        let digest = DigestInfo::try_new(VALID_HASH1, 100)?;

        // Use an invalid URI that fails during GrpcStore::new(). The
        // space character is illegal in URIs, so Uri::try_from() fails
        // and create_worker_connection returns Err. try_read_from_worker
        // will `continue` past this endpoint and return Ok(false),
        // resulting in the final NotFound error.
        locality_map
            .write()
            .register_blobs("not a valid uri", &[digest]);

        let result = store.get_part_unchunked(digest, 0, None).await;

        assert!(result.is_err(), "Expected NotFound error");
        let err = result.unwrap_err();
        assert_eq!(
            err.code,
            Code::NotFound,
            "Expected NotFound, got: {err:?}"
        );

        Ok(())
    }

    // ---------------------------------------------------------------
    // 4. has_with_results: inner store only, no locality map.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_has_with_results_does_not_use_locality_map() -> Result<(), Error> {
        let (store, locality_map) = make_proxy_store();

        let value = b"test data";
        let d1 = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;
        let d2 = DigestInfo::try_new(VALID_HASH2, 999)?;

        // Only d1 is in the inner store.
        store
            .update_oneshot(d1, Bytes::from_static(value))
            .await?;

        // Register d2 on a worker — has() must NOT report it as present.
        // The locality map is only for read optimization (get_part), not
        // for existence checks that drive upload decisions. Reporting
        // worker-only blobs as "present" in has_with_results causes
        // FindMissingBlobs to tell clients the blob exists, so they
        // skip uploading it. When the blob is later needed, neither
        // the server's CAS nor the worker may have it.
        locality_map
            .write()
            .register_blobs("worker-a:50081", &[d2]);

        let keys: Vec<StoreKey<'_>> = vec![d1.into(), d2.into()];
        let mut results = vec![None; 2];
        store.has_with_results(&keys, &mut results).await?;

        // d1 should be found with correct size from inner store.
        assert_eq!(
            results[0],
            Some(value.len() as u64),
            "d1 should be present in inner store"
        );
        // d2 should NOT be found — locality map is not consulted.
        assert_eq!(
            results[1],
            None,
            "d2 should not be found (locality map not used in has_with_results)"
        );

        Ok(())
    }

    // ---------------------------------------------------------------
    // 4b. has_with_results: no locality entry => still None.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_has_with_results_no_locality_returns_none() -> Result<(), Error> {
        let (store, _locality_map) = make_proxy_store();

        let d1 = DigestInfo::try_new(VALID_HASH1, 100)?;

        // Neither inner store nor locality map has d1.
        let keys: Vec<StoreKey<'_>> = vec![d1.into()];
        let mut results = vec![None; 1];
        store.has_with_results(&keys, &mut results).await?;

        assert_eq!(
            results[0], None,
            "d1 should not be found when absent from both inner store and locality map"
        );

        Ok(())
    }

    // ---------------------------------------------------------------
    // 5. update() passes through to inner store.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_update_passes_through() -> Result<(), Error> {
        let (store, _locality_map) = make_proxy_store();

        let value = b"upload me";
        let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

        // Upload via the proxy store.
        store
            .update_oneshot(digest, Bytes::from_static(value))
            .await?;

        // Verify the blob is retrievable (proving it went into the inner store).
        let data = store.get_part_unchunked(digest, 0, None).await?;
        assert_eq!(data.as_ref(), value);

        // Also verify via has().
        let size = store.has(digest).await?;
        assert_eq!(size, Some(value.len() as u64));

        Ok(())
    }

    // ---------------------------------------------------------------
    // 6. get_part with offset and length returns correct subset.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_get_part_with_offset_and_length() -> Result<(), Error> {
        let (store, _locality_map) = make_proxy_store();

        let value = b"0123456789abcdefghij"; // 20 bytes
        let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

        store
            .update_oneshot(digest, Bytes::from_static(value))
            .await?;

        // Read bytes [5..15) — 10 bytes starting at offset 5.
        let data = store
            .get_part_unchunked(digest, 5, Some(10))
            .await?;
        assert_eq!(
            data.as_ref(),
            b"56789abcde",
            "Expected subset at offset=5, length=10"
        );

        // Read from offset 15 to end (no length limit).
        let data = store.get_part_unchunked(digest, 15, None).await?;
        assert_eq!(
            data.as_ref(),
            b"fghij",
            "Expected tail from offset=15"
        );

        // Read 0 bytes from offset 0 with length 0.
        let data = store
            .get_part_unchunked(digest, 0, Some(0))
            .await?;
        assert_eq!(data.as_ref(), b"", "Expected empty result for length=0");

        Ok(())
    }

    // ---------------------------------------------------------------
    // 7. Redirect parsing: well-formed redirect error.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_redirect_well_formed() -> Result<(), Error> {
        let err = make_err!(
            Code::FailedPrecondition,
            "{REDIRECT_PREFIX}grpc://w1:50071,grpc://w2:50071|"
        );
        let msg = err.message_string();
        let start = msg.find(REDIRECT_PREFIX).expect("prefix missing");
        let endpoints_str = &msg[start + REDIRECT_PREFIX.len()..];
        let endpoints_str = endpoints_str.split('|').next().unwrap_or(endpoints_str);
        let endpoints: Vec<String> = endpoints_str
            .split(',')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        assert_eq!(endpoints.len(), 2);
        assert_eq!(endpoints[0], "grpc://w1:50071");
        assert_eq!(endpoints[1], "grpc://w2:50071");
        Ok(())
    }

    // ---------------------------------------------------------------
    // 8. Redirect parsing: trailing noise after pipe is ignored.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_redirect_trailing_noise_after_pipe() -> Result<(), Error> {
        let err = make_err!(
            Code::FailedPrecondition,
            "{REDIRECT_PREFIX}grpc://w1:50071|some extra noise"
        );
        let msg = err.message_string();
        let start = msg.find(REDIRECT_PREFIX).expect("prefix missing");
        let endpoints_str = &msg[start + REDIRECT_PREFIX.len()..];
        let endpoints_str = endpoints_str.split('|').next().unwrap_or(endpoints_str);
        let endpoints: Vec<String> = endpoints_str
            .split(',')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        assert_eq!(endpoints.len(), 1);
        assert_eq!(endpoints[0], "grpc://w1:50071");
        Ok(())
    }

    // ---------------------------------------------------------------
    // 9. Redirect parsing: empty segments filtered out.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_redirect_empty_segments_filtered() -> Result<(), Error> {
        let err = make_err!(
            Code::FailedPrecondition,
            "{REDIRECT_PREFIX}a,,b,|"
        );
        let msg = err.message_string();
        let start = msg.find(REDIRECT_PREFIX).expect("prefix missing");
        let endpoints_str = &msg[start + REDIRECT_PREFIX.len()..];
        let endpoints_str = endpoints_str.split('|').next().unwrap_or(endpoints_str);
        let endpoints: Vec<String> = endpoints_str
            .split(',')
            .filter(|s| !s.is_empty())
            .map(String::from)
            .collect();
        assert_eq!(endpoints, vec!["a", "b"]);
        Ok(())
    }

    // ---------------------------------------------------------------
    // 10. IS_WORKER_REQUEST=true gets NotFound (no redirect to avoid loops).
    //     Workers handle peer fetching via their own WorkerProxyStore with
    //     race_peers enabled. Generating redirects from the server to other
    //     workers creates a loop: worker → server → redirect → workers → ...
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_worker_request_gets_not_found_no_redirect() -> Result<(), Error> {
        let (store, locality_map) = make_proxy_store();

        let digest = DigestInfo::try_new(VALID_HASH1, 100)?;
        let peer_endpoint = "grpc://peer-worker:50071";

        locality_map
            .write()
            .register_blobs(peer_endpoint, &[digest]);

        let result = IS_WORKER_REQUEST
            .scope(true, store.get_part_unchunked(digest, 0, None))
            .await;

        assert!(result.is_err(), "Expected NotFound error");
        let err = result.unwrap_err();
        assert_eq!(
            err.code,
            Code::NotFound,
            "Worker request should get NotFound (not redirect), got: {err:?}"
        );
        let msg = err.message_string();
        assert!(
            !msg.contains(REDIRECT_PREFIX),
            "Worker request should NOT contain redirect prefix: {msg}"
        );

        Ok(())
    }

    // ---------------------------------------------------------------
    // 11. IS_WORKER_REQUEST=false gets NotFound (no proxy to invalid peer).
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_non_worker_request_gets_not_found() -> Result<(), Error> {
        let (store, locality_map) = make_proxy_store();

        let digest = DigestInfo::try_new(VALID_HASH1, 100)?;

        // Use an invalid URI so the proxy attempt fails gracefully.
        locality_map
            .write()
            .register_blobs("not a valid uri", &[digest]);

        let result = IS_WORKER_REQUEST
            .scope(false, store.get_part_unchunked(digest, 0, None))
            .await;

        assert!(result.is_err(), "Expected NotFound error");
        let err = result.unwrap_err();
        assert_eq!(
            err.code,
            Code::NotFound,
            "Non-worker should get NotFound, got: {err:?}"
        );

        Ok(())
    }

    // ---------------------------------------------------------------
    // 12. optimized_for(LazyExistenceOnSync) returns true.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_optimized_for_lazy_existence() -> Result<(), Error> {
        let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
        let locality_map = new_shared_blob_locality_map();
        let proxy = WorkerProxyStore::new(inner, locality_map);

        assert!(
            StoreDriver::optimized_for(&*proxy, StoreOptimizations::LazyExistenceOnSync),
            "WorkerProxyStore should report LazyExistenceOnSync"
        );

        Ok(())
    }

    // ---------------------------------------------------------------
    // 13. optimized_for(other) delegates to inner store.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_optimized_for_other_delegates_to_inner() -> Result<(), Error> {
        let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
        let locality_map = new_shared_blob_locality_map();
        let proxy = WorkerProxyStore::new(inner, locality_map);

        assert!(
            !StoreDriver::optimized_for(&*proxy, StoreOptimizations::NoopUpdates),
            "Should delegate non-LazyExistence optimizations to inner store"
        );

        Ok(())
    }

    // ---------------------------------------------------------------
    // 14. Race: inner store has blob, peer registered — server wins race.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_race_server_wins_when_inner_has_blob() -> Result<(), Error> {
        let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
        let locality_map = new_shared_blob_locality_map();
        let mut proxy = WorkerProxyStore::new(inner.clone(), locality_map.clone());
        Arc::get_mut(&mut proxy).unwrap().enable_race_peers();
        let store = Store::new(proxy.clone());

        let value = b"race test data";
        let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

        // Put blob in inner store.
        inner
            .update_oneshot(digest, Bytes::from_static(value))
            .await?;

        // Inject a peer that also has the blob (MemoryStore with same data).
        let peer_store = Store::new(MemoryStore::new(&MemorySpec::default()));
        peer_store
            .update_oneshot(digest, Bytes::from_static(value))
            .await?;
        proxy.inject_worker_connection("grpc://peer:50071", peer_store);

        locality_map
            .write()
            .register_blobs("grpc://peer:50071", &[digest]);

        // NOT in IS_WORKER_REQUEST scope, so racing path is taken.
        let result = store.get_part_unchunked(digest, 0, None).await?;
        assert_eq!(result.as_ref(), value);

        Ok(())
    }

    // ---------------------------------------------------------------
    // 15. Race: inner store miss, peer has blob — peer wins race.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_race_peer_wins_when_inner_misses() -> Result<(), Error> {
        let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
        let locality_map = new_shared_blob_locality_map();
        let mut proxy = WorkerProxyStore::new(inner, locality_map.clone());
        Arc::get_mut(&mut proxy).unwrap().enable_race_peers();
        let store = Store::new(proxy.clone());

        let value = b"peer only data";
        let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

        // Inner store is empty. Peer has the blob.
        let peer_store = Store::new(MemoryStore::new(&MemorySpec::default()));
        peer_store
            .update_oneshot(digest, Bytes::from_static(value))
            .await?;
        proxy.inject_worker_connection("grpc://peer:50071", peer_store);

        locality_map
            .write()
            .register_blobs("grpc://peer:50071", &[digest]);

        let result = store.get_part_unchunked(digest, 0, None).await?;
        assert_eq!(result.as_ref(), value);

        Ok(())
    }

    // ---------------------------------------------------------------
    // 16. Race: both inner and peer miss — returns error.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_race_both_miss_returns_error() -> Result<(), Error> {
        let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
        let locality_map = new_shared_blob_locality_map();
        let mut proxy = WorkerProxyStore::new(inner, locality_map.clone());
        Arc::get_mut(&mut proxy).unwrap().enable_race_peers();
        let store = Store::new(proxy.clone());

        let digest = DigestInfo::try_new(VALID_HASH1, 100)?;

        // Both inner and peer are empty.
        let peer_store = Store::new(MemoryStore::new(&MemorySpec::default()));
        proxy.inject_worker_connection("grpc://peer:50071", peer_store);

        locality_map
            .write()
            .register_blobs("grpc://peer:50071", &[digest]);

        let result = store.get_part_unchunked(digest, 0, None).await;
        assert!(result.is_err(), "Expected error when both miss");

        Ok(())
    }
}
