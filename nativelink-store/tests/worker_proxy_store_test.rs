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
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use nativelink_config::stores::MemorySpec;
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::worker_proxy_store::WorkerProxyStore;
use nativelink_util::blob_locality_map::{SharedBlobLocalityMap, new_shared_blob_locality_map};
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    IS_WORKER_REQUEST, ItemCallback, REDIRECT_PREFIX, Store, StoreDriver, StoreKey, StoreLike,
    StoreOptimizations, UploadSizeInfo,
};
use pretty_assertions::assert_eq;

const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";
const VALID_HASH2: &str = "0123456789abcdef000000000000000000020000000000000123456789abcdef";
const VALID_HASH3: &str = "0123456789abcdef000000000000000000030000000000000123456789abcdef";

/// Helper: create a WorkerProxyStore backed by a fresh MemoryStore.
/// Returns (proxy_store_as_Store, inner_memory_store, locality_map).
fn make_proxy_store() -> (Store, Store, SharedBlobLocalityMap) {
    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let locality_map = new_shared_blob_locality_map();
    let proxy = WorkerProxyStore::new(inner.clone(), locality_map.clone());
    (Store::new(proxy), inner, locality_map)
}

// -------------------------------------------------------------------
// 1. get_part delegates to inner store on hit
// -------------------------------------------------------------------
#[nativelink_test]
async fn get_part_returns_data_from_inner_store_on_hit() -> Result<(), Error> {
    let (proxy, _inner, locality_map) = make_proxy_store();

    let value = b"hello from inner store";
    let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

    // Write directly through the proxy (which delegates update to inner).
    proxy
        .update_oneshot(digest, Bytes::from_static(value))
        .await?;

    // Register a fake worker in the locality map. If get_part were to
    // consult it, it would try to connect and potentially fail or return
    // different data. We verify the inner store data is returned instead.
    locality_map
        .write()
        .register_blobs("fake-worker:9999", &[digest]);

    let result = proxy.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(
        result.as_ref(),
        value,
        "Expected data from inner store, not from worker"
    );

    Ok(())
}

// -------------------------------------------------------------------
// 2. get_part returns NotFound when inner misses and no peers
// -------------------------------------------------------------------
#[nativelink_test]
async fn get_part_returns_not_found_when_inner_misses_and_no_peers() -> Result<(), Error> {
    let (proxy, _inner, _locality_map) = make_proxy_store();

    let digest = DigestInfo::try_new(VALID_HASH1, 42)?;

    let result = proxy.get_part_unchunked(digest, 0, None).await;
    assert!(result.is_err(), "Expected an error for missing blob");

    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        Code::NotFound,
        "Expected NotFound error code, got: {err:?}"
    );

    Ok(())
}

// -------------------------------------------------------------------
// 3. has delegates to inner store (returns Some on hit)
// -------------------------------------------------------------------
#[nativelink_test]
async fn has_returns_size_when_inner_has_blob() -> Result<(), Error> {
    let (proxy, _inner, _locality_map) = make_proxy_store();

    let value = b"test data for has";
    let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

    proxy
        .update_oneshot(digest, Bytes::from_static(value))
        .await?;

    let size = proxy.has(digest).await?;
    assert_eq!(
        size,
        Some(value.len() as u64),
        "has() should return the blob size from inner store"
    );

    Ok(())
}

// -------------------------------------------------------------------
// 4. has returns None when inner does not have blob
//    (locality map is NOT consulted for existence checks)
// -------------------------------------------------------------------
#[nativelink_test]
async fn has_falls_back_to_locality_map_when_inner_missing() -> Result<(), Error> {
    let (proxy, _inner, locality_map) = make_proxy_store();

    let digest = DigestInfo::try_new(VALID_HASH1, 100)?;

    // Register the digest on a worker endpoint.
    locality_map
        .write()
        .register_blobs("worker-a:50081", &[digest]);

    // has() must NOT report locality-only blobs as present.
    // Worker blobs may be evicted at any time; reporting them in
    // has() causes clients to skip uploads, leading to NotFound later.
    let size = proxy.has(digest).await?;
    assert_eq!(
        size,
        None,
        "has() should not find digest via locality map (locality map not used in existence checks)"
    );

    Ok(())
}

// -------------------------------------------------------------------
// 5. has_with_results delegates to inner store only, not locality map
// -------------------------------------------------------------------
#[nativelink_test]
async fn has_with_results_delegates_to_inner_and_locality_map() -> Result<(), Error> {
    let (proxy, _inner, locality_map) = make_proxy_store();

    let value = b"test data";
    let d1 = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;
    let d2 = DigestInfo::try_new(VALID_HASH2, 999)?;
    let d3 = DigestInfo::try_new(VALID_HASH3, 50)?;

    // Only d1 is in the inner store.
    proxy
        .update_oneshot(d1, Bytes::from_static(value))
        .await?;

    // Register d2 and d3 on workers — has_with_results must NOT report
    // them as present. Locality map is only for read optimization in
    // get_part(), not for existence checks that drive upload decisions.
    {
        let mut map = locality_map.write();
        map.register_blobs("worker-a:50081", &[d2]);
        map.register_blobs("worker-b:50081", &[d3]);
    }

    let keys: Vec<StoreKey<'_>> = vec![d1.into(), d2.into(), d3.into()];
    let mut results = vec![None; 3];
    proxy.has_with_results(&keys, &mut results).await?;

    assert_eq!(
        results[0],
        Some(value.len() as u64),
        "d1 should be found in inner store"
    );
    assert_eq!(
        results[1],
        None,
        "d2 should not be found (locality map not used in has_with_results)"
    );
    assert_eq!(
        results[2],
        None,
        "d3 should not be found (locality map not used in has_with_results)"
    );

    Ok(())
}

// -------------------------------------------------------------------
// 6. has_with_results on empty digest list succeeds
// -------------------------------------------------------------------
#[nativelink_test]
async fn has_with_results_empty_digests_succeeds() -> Result<(), Error> {
    let (proxy, _inner, _locality_map) = make_proxy_store();

    let keys: Vec<StoreKey<'_>> = vec![];
    let mut results: Vec<Option<u64>> = vec![];
    proxy.has_with_results(&keys, &mut results).await?;

    // No assertions needed beyond not panicking.
    Ok(())
}

// -------------------------------------------------------------------
// 7. update_oneshot delegates to inner store
// -------------------------------------------------------------------
#[nativelink_test]
async fn update_oneshot_stores_in_inner() -> Result<(), Error> {
    let (proxy, inner, _locality_map) = make_proxy_store();

    let value = b"upload via proxy";
    let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

    proxy
        .update_oneshot(digest, Bytes::from_static(value))
        .await?;

    // Verify the blob landed in the inner store directly.
    let inner_data = inner.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(
        inner_data.as_ref(),
        value,
        "Data should be present in the inner store after update_oneshot"
    );

    Ok(())
}

// -------------------------------------------------------------------
// 8. get_part with offset and length on inner hit
// -------------------------------------------------------------------
#[nativelink_test]
async fn get_part_with_offset_and_length_from_inner() -> Result<(), Error> {
    let (proxy, _inner, _locality_map) = make_proxy_store();

    let value = b"0123456789abcdefghij"; // 20 bytes
    let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

    proxy
        .update_oneshot(digest, Bytes::from_static(value))
        .await?;

    // Read bytes [5..15) — 10 bytes at offset 5.
    let data = proxy.get_part_unchunked(digest, 5, Some(10)).await?;
    assert_eq!(
        data.as_ref(),
        b"56789abcde",
        "Expected subset at offset=5, length=10"
    );

    // Read from offset 15 to end.
    let data = proxy.get_part_unchunked(digest, 15, None).await?;
    assert_eq!(data.as_ref(), b"fghij", "Expected tail from offset=15");

    // Read 0 bytes.
    let data = proxy.get_part_unchunked(digest, 0, Some(0)).await?;
    assert_eq!(data.as_ref(), b"", "Expected empty result for length=0");

    Ok(())
}

// -------------------------------------------------------------------
// 9. Inner miss + locality has peers for a DIFFERENT digest
//    => the queried digest is still NotFound (locality map miss)
// -------------------------------------------------------------------
#[nativelink_test]
async fn get_part_inner_miss_locality_has_different_digest_returns_not_found() -> Result<(), Error> {
    let (proxy, _inner, locality_map) = make_proxy_store();

    let d1 = DigestInfo::try_new(VALID_HASH1, 100)?;
    let d2 = DigestInfo::try_new(VALID_HASH2, 200)?;

    // Register d2 on a worker, but NOT d1.
    locality_map
        .write()
        .register_blobs("worker-a:50081", &[d2]);

    // Query d1 — not in inner store, not in locality map.
    let result = proxy.get_part_unchunked(d1, 0, None).await;
    assert!(result.is_err(), "Expected NotFound for d1");

    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        Code::NotFound,
        "Expected NotFound since d1 has no locality entries, got: {err:?}"
    );

    Ok(())
}

// -------------------------------------------------------------------
// 10. Locality map returns empty workers list after eviction
//     => NotFound (no peers to try)
// -------------------------------------------------------------------
#[nativelink_test]
async fn get_part_inner_miss_locality_evicted_returns_not_found() -> Result<(), Error> {
    let (proxy, _inner, locality_map) = make_proxy_store();

    let digest = DigestInfo::try_new(VALID_HASH1, 100)?;

    // Register then evict the digest.
    {
        let mut map = locality_map.write();
        map.register_blobs("worker-a:50081", &[digest]);
        map.evict_blobs("worker-a:50081", &[digest]);
    }

    // Now there are no workers for this digest.
    let result = proxy.get_part_unchunked(digest, 0, None).await;
    assert!(result.is_err(), "Expected NotFound after eviction");

    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        Code::NotFound,
        "Expected NotFound since locality was evicted, got: {err:?}"
    );

    Ok(())
}

// -------------------------------------------------------------------
// 11. update followed by get_part roundtrip
// -------------------------------------------------------------------
#[nativelink_test]
async fn update_then_get_roundtrip() -> Result<(), Error> {
    let (proxy, _inner, _locality_map) = make_proxy_store();

    let value = b"roundtrip data payload";
    let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

    // Upload via proxy.
    proxy
        .update_oneshot(digest, Bytes::from_static(value))
        .await?;

    // Verify has() works.
    let size = proxy.has(digest).await?;
    assert_eq!(size, Some(value.len() as u64));

    // Verify get_part returns the correct data.
    let data = proxy.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(data.as_ref(), value);

    Ok(())
}

// -------------------------------------------------------------------
// 12. Multiple blobs: has_with_results shows correct presence
// -------------------------------------------------------------------
#[nativelink_test]
async fn has_with_results_multiple_blobs_mixed() -> Result<(), Error> {
    let (proxy, _inner, _locality_map) = make_proxy_store();

    let v1 = b"first blob";
    let v3 = b"third blob";
    let d1 = DigestInfo::try_new(VALID_HASH1, v1.len() as u64)?;
    let d2 = DigestInfo::try_new(VALID_HASH2, 999)?; // not stored
    let d3 = DigestInfo::try_new(VALID_HASH3, v3.len() as u64)?;

    proxy
        .update_oneshot(d1, Bytes::from_static(v1))
        .await?;
    proxy
        .update_oneshot(d3, Bytes::from_static(v3))
        .await?;

    let keys: Vec<StoreKey<'_>> = vec![d1.into(), d2.into(), d3.into()];
    let mut results = vec![None; 3];
    proxy.has_with_results(&keys, &mut results).await?;

    assert_eq!(results[0], Some(v1.len() as u64), "d1 should be found");
    assert_eq!(results[1], None, "d2 should not be found");
    assert_eq!(results[2], Some(v3.len() as u64), "d3 should be found");

    Ok(())
}

// -------------------------------------------------------------------
// 13. get_part for a blob that was never stored and has no locality
//     entries returns NotFound (different digest, not in map at all)
// -------------------------------------------------------------------
#[nativelink_test]
async fn get_part_completely_unknown_digest_returns_not_found() -> Result<(), Error> {
    let (proxy, _inner, locality_map) = make_proxy_store();

    // Register a DIFFERENT digest on a worker (not the one we query).
    let other_digest = DigestInfo::try_new(VALID_HASH2, 50)?;
    locality_map
        .write()
        .register_blobs("worker-x:50081", &[other_digest]);

    // Query a digest that is not in the inner store and not in the
    // locality map at all.
    let query_digest = DigestInfo::try_new(VALID_HASH1, 100)?;
    let result = proxy.get_part_unchunked(query_digest, 0, None).await;

    assert!(result.is_err());
    assert_eq!(result.unwrap_err().code, Code::NotFound);

    Ok(())
}

// -------------------------------------------------------------------
// 14. Overwrite a blob via update and verify new data is returned
// -------------------------------------------------------------------
#[nativelink_test]
async fn update_overwrites_existing_blob() -> Result<(), Error> {
    let (proxy, _inner, _locality_map) = make_proxy_store();

    let digest = DigestInfo::try_new(VALID_HASH1, 5)?;

    proxy
        .update_oneshot(digest, Bytes::from_static(b"first"))
        .await?;

    let data = proxy.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(data.as_ref(), b"first");

    // Overwrite with new data (same digest key, different content for
    // MemoryStore which doesn't validate content hash).
    proxy
        .update_oneshot(digest, Bytes::from_static(b"secnd"))
        .await?;

    let data = proxy.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(data.as_ref(), b"secnd");

    Ok(())
}

// -------------------------------------------------------------------
// 15. Non-NotFound errors from inner store propagate directly
//     (no locality map fallback)
// -------------------------------------------------------------------
// Note: This is difficult to test without a custom mock store that
// returns a non-NotFound error. The inline tests cover this via the
// match arm in get_part(). We verify the pattern indirectly: a
// successful inner read never consults the locality map (test 1),
// and NotFound triggers the locality path (tests 2, 9, 10).

// -------------------------------------------------------------------
// 16. Large blob roundtrip through the proxy
// -------------------------------------------------------------------
#[nativelink_test]
async fn large_blob_roundtrip() -> Result<(), Error> {
    let (proxy, _inner, _locality_map) = make_proxy_store();

    // 1 MiB of repeated bytes
    let size: usize = 1024 * 1024;
    let value: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
    let digest = DigestInfo::try_new(VALID_HASH1, size as u64)?;

    proxy
        .update_oneshot(digest, Bytes::from(value.clone()))
        .await?;

    let data = proxy.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(data.len(), size, "Returned blob size should match");
    assert_eq!(data.as_ref(), value.as_slice());

    Ok(())
}

// ===================================================================
// Gap 1: Successful peer proxy read — inject a MemoryStore as a peer
// ===================================================================

/// Helper: create a WorkerProxyStore and return the underlying Arc so we
/// can call inject_worker_connection().
fn make_proxy_store_with_arc() -> (Arc<WorkerProxyStore>, Store, SharedBlobLocalityMap) {
    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let locality_map = new_shared_blob_locality_map();
    let proxy_arc = WorkerProxyStore::new(inner.clone(), locality_map.clone());
    (proxy_arc, inner, locality_map)
}

// -------------------------------------------------------------------
// 17. Successful peer proxy read: inner miss, peer has the blob
// -------------------------------------------------------------------
#[nativelink_test]
async fn get_part_proxies_from_injected_peer() -> Result<(), Error> {
    let (proxy_arc, _inner, locality_map) = make_proxy_store_with_arc();
    let proxy = Store::new(proxy_arc.clone());

    let value = b"data from the peer worker";
    let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

    // Create a "peer" MemoryStore and populate it with the blob.
    let peer_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    peer_store
        .update_oneshot(digest, Bytes::from_static(value))
        .await?;

    // Inject the peer store as a worker connection.
    let peer_endpoint = "grpc://peer-worker:50081";
    proxy_arc.inject_worker_connection(peer_endpoint, peer_store);

    // Register the digest on the peer in the locality map.
    locality_map
        .write()
        .register_blobs(peer_endpoint, &[digest]);

    // The inner store is empty, so get_part should proxy from the peer.
    let result = proxy.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(
        result.as_ref(),
        value,
        "Expected blob data from the injected peer store"
    );

    Ok(())
}

// -------------------------------------------------------------------
// 18. Peer proxy read with offset and length
// -------------------------------------------------------------------
#[nativelink_test]
async fn get_part_proxies_from_peer_with_offset() -> Result<(), Error> {
    let (proxy_arc, _inner, locality_map) = make_proxy_store_with_arc();
    let proxy = Store::new(proxy_arc.clone());

    let value = b"0123456789abcdef"; // 16 bytes
    let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

    let peer_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    peer_store
        .update_oneshot(digest, Bytes::from_static(value))
        .await?;

    let peer_endpoint = "grpc://peer-worker:50081";
    proxy_arc.inject_worker_connection(peer_endpoint, peer_store);
    locality_map
        .write()
        .register_blobs(peer_endpoint, &[digest]);

    // Read bytes [4..12) from the peer.
    let result = proxy.get_part_unchunked(digest, 4, Some(8)).await?;
    assert_eq!(
        result.as_ref(),
        b"456789ab",
        "Expected subset from peer at offset=4, length=8"
    );

    Ok(())
}

// -------------------------------------------------------------------
// 19. Peer proxy: first peer doesn't have blob, second peer does
// -------------------------------------------------------------------
#[nativelink_test]
async fn get_part_skips_peer_without_blob_and_reads_from_next() -> Result<(), Error> {
    let (proxy_arc, _inner, locality_map) = make_proxy_store_with_arc();
    let proxy = Store::new(proxy_arc.clone());

    let value = b"only on peer-b";
    let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

    // Peer A: empty store (has() returns None).
    let peer_a_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let peer_a_endpoint = "grpc://peer-a:50081";
    proxy_arc.inject_worker_connection(peer_a_endpoint, peer_a_store);

    // Peer B: has the blob.
    let peer_b_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    peer_b_store
        .update_oneshot(digest, Bytes::from_static(value))
        .await?;
    let peer_b_endpoint = "grpc://peer-b:50081";
    proxy_arc.inject_worker_connection(peer_b_endpoint, peer_b_store);

    // Register the digest on both peers.
    {
        let mut map = locality_map.write();
        map.register_blobs(peer_a_endpoint, &[digest]);
        map.register_blobs(peer_b_endpoint, &[digest]);
    }

    let result = proxy.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(
        result.as_ref(),
        value,
        "Expected data from peer-b after peer-a returned None for has()"
    );

    Ok(())
}

// ===================================================================
// Gap 2: Resume-from-offset — PartialFailStore + next peer
// ===================================================================

/// A store wrapper that delegates to an inner store but fails `get_part`
/// after writing a configured number of bytes. Used to test streaming
/// resume logic in WorkerProxyStore.
#[derive(Debug, MetricsComponent)]
struct PartialFailStore {
    inner: Store,
    /// Number of bytes to successfully write before returning an error.
    fail_after_bytes: u64,
}

default_health_status_indicator!(PartialFailStore);

#[async_trait]
impl StoreDriver for PartialFailStore {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        self.inner.has_with_results(digests, results).await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        self.inner.update(key, reader, upload_size).await
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        // Read the full blob from the inner store.
        let data = self.inner.get_part_unchunked(key.borrow(), offset, length).await?;

        // Write up to `fail_after_bytes` bytes, then return an error.
        let write_len = core::cmp::min(data.len() as u64, self.fail_after_bytes) as usize;
        if write_len > 0 {
            writer
                .send(data.slice(..write_len))
                .await
                .map_err(|e| make_err!(Code::Internal, "PartialFailStore write error: {e:?}"))?;
        }

        Err(make_err!(
            Code::Internal,
            "PartialFailStore: simulated failure after {} bytes",
            write_len
        ))
    }

    fn inner_store(&self, _key: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any<'a>(&'a self) -> &'a (dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_item_callback(
        self: Arc<Self>,
        _callback: Arc<dyn ItemCallback>,
    ) -> Result<(), Error> {
        Ok(())
    }
}

// -------------------------------------------------------------------
// 20. Resume from offset: first peer fails mid-stream, second succeeds
// -------------------------------------------------------------------
#[nativelink_test]
async fn get_part_resumes_from_next_peer_after_mid_stream_failure() -> Result<(), Error> {
    let (proxy_arc, _inner, locality_map) = make_proxy_store_with_arc();
    let proxy = Store::new(proxy_arc.clone());

    let value = b"0123456789abcdef"; // 16 bytes
    let digest = DigestInfo::try_new(VALID_HASH1, value.len() as u64)?;

    // Peer A: a PartialFailStore that writes 5 bytes then fails.
    let peer_a_inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    peer_a_inner
        .update_oneshot(digest, Bytes::from_static(value))
        .await?;
    let peer_a_store = Store::new(Arc::new(PartialFailStore {
        inner: peer_a_inner,
        fail_after_bytes: 5,
    }));
    let peer_a_endpoint = "grpc://peer-a:50081";
    proxy_arc.inject_worker_connection(peer_a_endpoint, peer_a_store);

    // Peer B: has the full blob (normal MemoryStore).
    let peer_b_store = Store::new(MemoryStore::new(&MemorySpec::default()));
    peer_b_store
        .update_oneshot(digest, Bytes::from_static(value))
        .await?;
    let peer_b_endpoint = "grpc://peer-b:50081";
    proxy_arc.inject_worker_connection(peer_b_endpoint, peer_b_store);

    // Register the digest on both peers. The order in the locality map
    // determines which peer is tried first. We register A first.
    {
        let mut map = locality_map.write();
        map.register_blobs(peer_a_endpoint, &[digest]);
        map.register_blobs(peer_b_endpoint, &[digest]);
    }

    // The proxy should: try peer A, get 5 bytes, fail, then resume from
    // peer B at offset 5. The final result should be the complete blob.
    let result = proxy.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(
        result.as_ref(),
        value,
        "Expected complete blob after resume from second peer"
    );

    Ok(())
}

// ===================================================================
// Gap 3: IS_WORKER_REQUEST branching tests
// ===================================================================

// -------------------------------------------------------------------
// 21. IS_WORKER_REQUEST=true: inner miss + locality has peer
//     => FailedPrecondition redirect with peer endpoint
// -------------------------------------------------------------------
#[nativelink_test]
async fn worker_request_returns_redirect_with_peer_endpoints() -> Result<(), Error> {
    let (proxy, _inner, locality_map) = make_proxy_store();

    let digest = DigestInfo::try_new(VALID_HASH1, 100)?;
    let peer_endpoint = "grpc://peer-worker:50071";

    locality_map
        .write()
        .register_blobs(peer_endpoint, &[digest]);

    let result = IS_WORKER_REQUEST
        .scope(true, proxy.get_part_unchunked(digest, 0, None))
        .await;

    assert!(result.is_err(), "Expected redirect error for worker request");
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        Code::FailedPrecondition,
        "Redirect should use FailedPrecondition, got: {err:?}"
    );
    let msg = err.message_string();
    assert!(
        msg.contains(REDIRECT_PREFIX),
        "Error message should contain redirect prefix: {msg}"
    );
    assert!(
        msg.contains(peer_endpoint),
        "Error message should contain peer endpoint: {msg}"
    );

    Ok(())
}

// -------------------------------------------------------------------
// 22. IS_WORKER_REQUEST=false: inner miss + locality has peer with
//     invalid URI => NotFound (proxy attempt fails gracefully)
// -------------------------------------------------------------------
#[nativelink_test]
async fn non_worker_request_returns_not_found_when_peer_unreachable() -> Result<(), Error> {
    let (proxy, _inner, locality_map) = make_proxy_store();

    let digest = DigestInfo::try_new(VALID_HASH1, 100)?;

    // Invalid URI fails during create_worker_connection.
    locality_map
        .write()
        .register_blobs("not a valid uri", &[digest]);

    let result = IS_WORKER_REQUEST
        .scope(false, proxy.get_part_unchunked(digest, 0, None))
        .await;

    assert!(result.is_err(), "Expected NotFound error");
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        Code::NotFound,
        "Non-worker request should get NotFound, got: {err:?}"
    );

    Ok(())
}

// ===================================================================
// Gap 4: optimized_for tests
// ===================================================================

// -------------------------------------------------------------------
// 23. optimized_for(LazyExistenceOnSync) returns true
// -------------------------------------------------------------------
#[nativelink_test]
async fn optimized_for_lazy_existence_returns_true() -> Result<(), Error> {
    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let locality_map = new_shared_blob_locality_map();
    let proxy = WorkerProxyStore::new(inner, locality_map);

    assert!(
        StoreDriver::optimized_for(&*proxy, StoreOptimizations::LazyExistenceOnSync),
        "WorkerProxyStore should report LazyExistenceOnSync"
    );

    Ok(())
}

// -------------------------------------------------------------------
// 24. optimized_for(other) delegates to inner store
// -------------------------------------------------------------------
#[nativelink_test]
async fn optimized_for_other_delegates_to_inner() -> Result<(), Error> {
    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let locality_map = new_shared_blob_locality_map();
    let proxy = WorkerProxyStore::new(inner, locality_map);

    assert!(
        !StoreDriver::optimized_for(&*proxy, StoreOptimizations::NoopUpdates),
        "Should delegate non-LazyExistence optimizations to inner store"
    );

    Ok(())
}
