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
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use nativelink_config::stores::{GrpcEndpoint, GrpcSpec, Retry, StoreType};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::blob_locality_map::SharedBlobLocalityMap;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthStatus, HealthStatusIndicator};
use nativelink_util::store_trait::{
    IS_WORKER_REQUEST, ItemCallback, REDIRECT_PREFIX, Store, StoreDriver, StoreKey, StoreLike,
    StoreOptimizations, UploadSizeInfo,
};
use parking_lot::RwLock;
use tracing::{info, trace, warn};

use crate::grpc_store::GrpcStore;

/// A store wrapper that transparently proxies CAS reads from workers when
/// the inner store returns NotFound. This enables worker-to-worker blob sharing.
///
/// Behavior:
/// - `get_part()`: Try inner store first. If NotFound, consult the locality map
///   for workers that have the digest, try reading from a worker.
/// - `has()` / `has_with_results()`: ONLY check inner store. Never consult the
///   locality map. (Prevents stale-positive issues with FindMissingBlobs.)
/// - `update()`: Pass through to inner store.
#[derive(MetricsComponent)]
pub struct WorkerProxyStore {
    #[metric(group = "inner_store")]
    inner: Store,
    /// Blob locality map — digest → worker endpoints.
    locality_map: SharedBlobLocalityMap,
    /// Cached GrpcStore connections to worker endpoints.
    worker_connections: RwLock<HashMap<Arc<str>, Store>>,
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
        })
    }

    /// Add a worker endpoint to the connection pool.
    pub async fn add_worker_endpoint(&self, endpoint: &str) {
        if self.get_worker_connection(endpoint).is_some() {
            return;
        }
        self.get_or_create_connection(endpoint).await;
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
        match Self::create_worker_connection(endpoint).await {
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
    async fn create_worker_connection(endpoint: &str) -> Result<Store, Error> {
        let spec = GrpcSpec {
            instance_name: String::new(),
            endpoints: vec![GrpcEndpoint {
                address: endpoint.to_string(),
                tls_config: None,
                concurrency_limit: None,
                connect_timeout_s: 5,
                tcp_keepalive_s: 30,
                http2_keepalive_interval_s: 30,
                http2_keepalive_timeout_s: 20,
                tcp_nodelay: true,
            }],
            store_type: StoreType::Cas,
            retry: Retry::default(),
            max_concurrent_requests: 0,
            connections_per_endpoint: 4,
            rpc_timeout_s: 120,
            batch_update_threshold_bytes: 0, // Not uploading via this store
            batch_coalesce_delay_ms: 0,
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
        info!(
            ?digest,
            endpoint_count = endpoints.len(),
            "WorkerProxyStore: following redirect to peer endpoints"
        );

        for endpoint in endpoints {
            let Some(store) = self.get_or_create_connection(endpoint).await else {
                continue;
            };

            match store
                .get_part(key.borrow(), &mut *writer, offset, length)
                .await
            {
                Ok(()) => {
                    info!(
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
    /// Streams directly from the peer to the caller's writer via `get_part()` —
    /// no buffering. If a peer fails mid-stream, we resume from the next peer
    /// at the byte offset where the previous one left off (content-addressed
    /// blobs are identical across peers).
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

        info!(
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

            // Stream directly from the peer — no buffering.
            // On failure, compute how many bytes were written and resume
            // from the next peer at the correct offset.
            match store
                .get_part(key.borrow(), &mut *writer, current_offset, remaining_length)
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
}

#[async_trait]
impl StoreDriver for WorkerProxyStore {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        // ONLY check inner store. Never consult the locality map for has().
        // This prevents stale-positive issues with FindMissingBlobs.
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
        // check before get_part(). Our has() only checks the inner store
        // (to avoid stale-positive FindMissingBlobs), but get_part() also
        // consults the locality map and peer workers. Without this, blobs
        // that exist only on peer workers would never be found by
        // FastSlowStore because has() returns None.
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
        // Try inner store directly — avoids an extra has() round trip.
        // NotFound is returned before any bytes are written, so the
        // writer is still clean and we can retry with peer workers.
        //
        // Always tell the inner store we're a worker so that if it's a
        // GrpcStore → server chain, the server returns a redirect instead
        // of proxying the blob through itself.
        let mut redirect_endpoints: Option<Vec<String>> = None;
        match IS_WORKER_REQUEST
            .scope(true, self.inner.get_part(key.borrow(), &mut *writer, offset, length))
            .await
        {
            Ok(()) => return Ok(()),
            Err(e) if e.code == Code::NotFound => {
                // Inner store doesn't have it — try peer workers below.
                trace!(
                    key = ?key.borrow().into_digest(),
                    "WorkerProxyStore: inner store miss (NotFound), consulting locality map"
                );
            }
            Err(e) if e.code == Code::FailedPrecondition => {
                // Check if the inner store returned a redirect (e.g. from
                // a server-side WorkerProxyStore telling us to fetch from
                // specific peers directly). The prefix may be embedded in
                // a longer error message after gRPC round-tripping.
                let msg = e.message_string();
                if let Some(start) = msg.find(REDIRECT_PREFIX) {
                    let endpoints_str = &msg[start + REDIRECT_PREFIX.len()..];
                    // Endpoints are terminated by '|' (added by the redirect
                    // generator to survive error message wrapping/merging).
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
                        info!(
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

        // If we got redirect endpoints from the inner store, try those
        // specific peers first (they are authoritative).
        if let Some(endpoints) = redirect_endpoints {
            if self
                .try_read_from_endpoints(
                    key.borrow(),
                    writer,
                    offset,
                    length,
                    &endpoints,
                )
                .await?
            {
                return Ok(());
            }
        }

        // Check if the caller is a worker. Workers get a redirect error
        // with peer endpoints so they can fetch directly (data stays on
        // the worker-to-worker plane and never transits through the server).
        let is_worker = IS_WORKER_REQUEST
            .try_with(|v| *v)
            .unwrap_or(false);

        if is_worker {
            let digest = key.borrow().into_digest();
            let workers = self.locality_map.read().lookup_workers(&digest);
            if workers.is_empty() {
                return Err(make_err!(
                    Code::NotFound,
                    "Blob {digest:?} not found in inner store or locality map"
                ));
            }
            let endpoints = workers.join(",");
            info!(
                ?digest,
                endpoints,
                "WorkerProxyStore: redirecting worker to peer endpoints"
            );
            // Terminate the endpoint list with '|' so the receiver can
            // reliably parse it even after error message wrapping/merging.
            return Err(make_err!(
                Code::FailedPrecondition,
                "{REDIRECT_PREFIX}{endpoints}|"
            ));
        }

        // Non-worker caller: proxy the blob from a peer worker.
        if self
            .try_read_from_worker(key.borrow(), writer, offset, length)
            .await?
        {
            return Ok(());
        }

        // No worker had it either.
        Err(make_err!(
            Code::NotFound,
            "Blob {:?} not found in inner store or any worker",
            key.borrow().into_digest()
        ))
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
    // 4. has_with_results passes through to inner store (no proxy).
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_has_with_results_passes_through() -> Result<(), Error> {
        let (store, locality_map) = make_proxy_store();

        let value = b"test data";
        let d1 = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;
        let d2 = DigestInfo::try_new(VALID_HASH2, 999)?;

        // Only d1 is in the inner store.
        store
            .update_oneshot(d1, Bytes::from_static(value))
            .await?;

        // Register d2 on a worker so we can prove has() does NOT
        // consult the locality map.
        locality_map
            .write()
            .register_blobs("worker-a:50081", &[d2]);

        let keys: Vec<StoreKey<'_>> = vec![d1.into(), d2.into()];
        let mut results = vec![None; 2];
        store.has_with_results(&keys, &mut results).await?;

        // d1 should be found with correct size.
        assert_eq!(
            results[0],
            Some(value.len() as u64),
            "d1 should be present in inner store"
        );
        // d2 should NOT be found (locality map is never consulted for has).
        assert_eq!(
            results[1], None,
            "d2 should NOT be found — has() must not consult locality map"
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
    // 10. IS_WORKER_REQUEST=true gets redirect with peer endpoints.
    // ---------------------------------------------------------------
    #[nativelink_test]
    async fn test_worker_request_gets_redirect() -> Result<(), Error> {
        let (store, locality_map) = make_proxy_store();

        let digest = DigestInfo::try_new(VALID_HASH1, 100)?;
        let peer_endpoint = "grpc://peer-worker:50071";

        locality_map
            .write()
            .register_blobs(peer_endpoint, &[digest]);

        let result = IS_WORKER_REQUEST
            .scope(true, store.get_part_unchunked(digest, 0, None))
            .await;

        assert!(result.is_err(), "Expected redirect error");
        let err = result.unwrap_err();
        assert_eq!(
            err.code,
            Code::FailedPrecondition,
            "Redirect should use FailedPrecondition, got: {err:?}"
        );
        let msg = err.message_string();
        assert!(
            msg.contains(REDIRECT_PREFIX),
            "Error should contain redirect prefix: {msg}"
        );
        assert!(
            msg.contains(peer_endpoint),
            "Error should contain peer endpoint: {msg}"
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
}
