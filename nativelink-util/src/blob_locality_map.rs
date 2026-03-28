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

use std::collections::{HashMap, HashSet};
use std::hash::{BuildHasher, Hasher};
use std::sync::Arc;
use std::time::SystemTime;

use crate::common::DigestInfo;
use parking_lot::RwLock;

/// A hasher that uses the first 8 bytes of a DigestInfo's packed SHA-256 hash
/// directly as the hash value. Since SHA-256 output is uniformly distributed,
/// this is a perfect hash input — no need for SipHash to re-mix it.
///
/// This saves ~20ns per HashMap operation on 40-byte DigestInfo keys, which
/// adds up to significant CPU savings when processing 500K+ digests/second
/// from worker BlobsAvailable notifications.
#[derive(Default, Clone, Copy, Debug)]
pub struct DigestHasher(u64);

impl Hasher for DigestHasher {
    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }

    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        // Derived Hash for DigestInfo calls:
        //   1. [u8; 32]::hash → write_usize(32) then write(32_bytes)
        //   2. u64::hash → write_u64(size_bytes)
        // We capture the first 8 bytes of the SHA-256 hash (already uniformly
        // distributed) and mix in the size via write_u64 below.
        // write_usize is a no-op so the length prefix is harmlessly discarded.
        if bytes.len() >= 8 {
            self.0 = u64::from_ne_bytes([
                bytes[0], bytes[1], bytes[2], bytes[3],
                bytes[4], bytes[5], bytes[6], bytes[7],
            ]);
        } else {
            // Fallback for smaller writes.
            for &b in bytes {
                self.0 = self.0.wrapping_mul(31).wrapping_add(b as u64);
            }
        }
    }

    #[inline]
    fn write_usize(&mut self, _: usize) {
        // Ignore length prefixes from [u8; N]::hash — we only care about
        // the actual hash bytes (from write) and size_bytes (from write_u64).
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        // Mix in size_bytes to differentiate digests with same hash prefix
        // but different sizes (extremely rare for SHA-256 but correct).
        self.0 = self.0.wrapping_add(i);
    }
}

#[derive(Default, Clone, Copy, Debug)]
pub struct DigestBuildHasher;

impl BuildHasher for DigestBuildHasher {
    type Hasher = DigestHasher;

    #[inline]
    fn build_hasher(&self) -> DigestHasher {
        DigestHasher(0)
    }
}

/// Compact per-digest endpoint list. With only ~10 workers, a Vec with linear
/// scan is faster than HashMap due to:
/// - No hashing overhead for Arc<str> keys
/// - Cache-friendly sequential memory access
/// - No bucket array overhead (HashMap has 50%+ empty slots)
/// - Fewer allocations (one Vec vs HashMap's bucket array + entries)
#[derive(Debug, Clone, Default)]
pub struct EndpointList {
    entries: Vec<(Arc<str>, SystemTime)>,
}

impl EndpointList {
    /// Insert or update an endpoint's timestamp. Returns true if the endpoint
    /// was newly inserted (not just updated).
    #[inline]
    fn upsert(&mut self, endpoint: &Arc<str>, ts: SystemTime) -> bool {
        for entry in &mut self.entries {
            if Arc::ptr_eq(&entry.0, endpoint) || *entry.0 == **endpoint {
                entry.1 = ts;
                return false;
            }
        }
        self.entries.push((endpoint.clone(), ts));
        true
    }

    /// Remove an endpoint. Returns true if it was present.
    #[inline]
    fn remove(&mut self, endpoint: &str) -> bool {
        if let Some(pos) = self.entries.iter().position(|(e, _)| &**e == endpoint) {
            self.entries.swap_remove(pos);
            true
        } else {
            false
        }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    #[inline]
    pub fn keys(&self) -> impl Iterator<Item = &Arc<str>> {
        self.entries.iter().map(|(e, _)| e)
    }

    #[inline]
    pub fn iter(&self) -> impl Iterator<Item = (&Arc<str>, &SystemTime)> {
        self.entries.iter().map(|(e, ts)| (e, ts))
    }

    #[inline]
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(e, _)| &**e == key)
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Get the timestamp for a specific endpoint.
    #[inline]
    pub fn get(&self, key: &str) -> Option<&SystemTime> {
        self.entries.iter().find(|(e, _)| &**e == key).map(|(_, ts)| ts)
    }
}

impl<'a> IntoIterator for &'a EndpointList {
    type Item = (&'a Arc<str>, &'a SystemTime);
    type IntoIter = std::iter::Map<
        std::slice::Iter<'a, (Arc<str>, SystemTime)>,
        fn(&'a (Arc<str>, SystemTime)) -> (&'a Arc<str>, &'a SystemTime),
    >;

    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().map(|(e, ts)| (e, ts))
    }
}

pub type DigestMap<V> = HashMap<DigestInfo, V, DigestBuildHasher>;
type DigestSet = HashSet<DigestInfo, DigestBuildHasher>;

/// Tracks which worker endpoints have which blobs, enabling peer-to-peer
/// blob fetching between workers.
///
/// The map is bidirectional:
/// - `blobs`: digest → { endpoint → last_registered_timestamp }
/// - `endpoint_blobs`: endpoint → set of digests (for fast cleanup on disconnect)
///
/// Performance notes:
/// - DigestInfo keys use a passthrough hasher (first 8 bytes of SHA-256 are
///   already uniformly distributed, so SipHash re-mixing is pure waste).
/// - Per-digest endpoint lists use Vec with linear scan instead of HashMap
///   (only ~10 workers, so cache-friendly linear scan beats hashing).
///
/// Cleanup relies entirely on explicit eviction notifications and worker
/// disconnect (no TTL — EvictingMap's `max_seconds_since_last_access` defaults
/// to unlimited).
#[derive(Debug)]
pub struct BlobLocalityMap {
    /// digest → endpoint list with timestamps
    blobs: DigestMap<EndpointList>,
    /// endpoint → set of digests (for fast cleanup on disconnect)
    endpoint_blobs: HashMap<Arc<str>, DigestSet>,
}

impl BlobLocalityMap {
    pub fn new() -> Self {
        Self {
            blobs: HashMap::with_hasher(DigestBuildHasher),
            endpoint_blobs: HashMap::new(),
        }
    }

    /// Register that the given digests are available on the given endpoint.
    pub fn register_blobs(&mut self, endpoint: &str, digests: &[DigestInfo]) {
        let now = SystemTime::now();
        self.register_blobs_with_timestamps(
            endpoint,
            &digests.iter().map(|d| (*d, now)).collect::<Vec<_>>(),
        );
    }

    /// Register digests with explicit timestamps (e.g. from BlobDigestInfo).
    ///
    /// Performance: Each digest requires one lookup in `blobs` (passthrough hash
    /// of first 8 SHA-256 bytes) plus a linear scan of <=10 endpoint entries.
    /// The `endpoint_blobs` reverse index also uses the passthrough hasher.
    /// Arc<str> cloning is avoided for existing endpoints (only atomic refcount
    /// on first insert per endpoint).
    pub fn register_blobs_with_timestamps(
        &mut self,
        endpoint: &str,
        digests_with_ts: &[(DigestInfo, SystemTime)],
    ) {
        // Allocate the endpoint Arc<str> once; the EndpointList.upsert() only
        // clones it when the endpoint is genuinely new for that digest.
        let ep: Arc<str> = endpoint.into();
        let digest_set = self
            .endpoint_blobs
            .entry(ep.clone())
            .or_insert_with(|| HashSet::with_hasher(DigestBuildHasher));

        for &(digest, ts) in digests_with_ts {
            digest_set.insert(digest);
            self.blobs
                .entry(digest)
                .or_default()
                .upsert(&ep, ts);
        }
    }

    /// Remove specific digests from the given endpoint (eviction notification).
    pub fn evict_blobs(&mut self, endpoint: &str, digests: &[DigestInfo]) {
        if let Some(digest_set) = self.endpoint_blobs.get_mut(endpoint) {
            for digest in digests {
                digest_set.remove(digest);
                if let Some(endpoints) = self.blobs.get_mut(digest) {
                    endpoints.remove(endpoint);
                    if endpoints.is_empty() {
                        self.blobs.remove(digest);
                    }
                }
            }
            if digest_set.is_empty() {
                self.endpoint_blobs.remove(endpoint);
            }
        }
    }

    /// Remove ALL entries for an endpoint (worker disconnect).
    pub fn remove_endpoint(&mut self, endpoint: &str) {
        if let Some(digests) = self.endpoint_blobs.remove(endpoint) {
            for digest in &digests {
                if let Some(endpoints) = self.blobs.get_mut(digest) {
                    endpoints.remove(endpoint);
                    if endpoints.is_empty() {
                        self.blobs.remove(digest);
                    }
                }
            }
        }
    }

    /// Returns true if any worker endpoint has the given digest.
    /// This is cheaper than `lookup_workers` because it avoids allocating.
    pub fn has_digest(&self, digest: &DigestInfo) -> bool {
        self.blobs
            .get(digest)
            .is_some_and(|eps| !eps.is_empty())
    }

    /// Look up which worker endpoints have the given digest.
    /// Returns all endpoints that have registered this digest.
    ///
    /// Workers refresh their timestamps on every BlobsAvailable update
    /// (typically every ~500ms), so stale entries are only possible if
    /// a worker disconnects without cleanup. Disconnects are handled
    /// via `remove_endpoint`, so we can simply return all endpoints.
    pub fn lookup_workers(&self, digest: &DigestInfo) -> Vec<Arc<str>> {
        let Some(endpoints) = self.blobs.get(digest) else {
            return Vec::new();
        };

        endpoints.keys().cloned().collect()
    }

    /// Look up which worker endpoints have the given digest, including the
    /// timestamp of when the blob was last registered/refreshed on each endpoint.
    /// Useful for preferring workers with more recently-refreshed locality data.
    pub fn lookup_workers_with_timestamps(&self, digest: &DigestInfo) -> Vec<(Arc<str>, SystemTime)> {
        let Some(endpoints) = self.blobs.get(digest) else {
            return Vec::new();
        };

        endpoints
            .iter()
            .map(|(endpoint, ts)| (endpoint.clone(), *ts))
            .collect()
    }

    /// Returns the set of all known endpoints.
    pub fn all_endpoints(&self) -> Vec<Arc<str>> {
        self.endpoint_blobs.keys().cloned().collect()
    }

    /// Returns the number of tracked digests.
    pub fn digest_count(&self) -> usize {
        self.blobs.len()
    }

    /// Returns the number of tracked endpoints.
    pub fn endpoint_count(&self) -> usize {
        self.endpoint_blobs.len()
    }

    /// Raw access to the blobs map for bulk scoring.
    /// Caller must hold the read lock.
    pub fn blobs_map(&self) -> &DigestMap<EndpointList> {
        &self.blobs
    }
}

impl Default for BlobLocalityMap {
    fn default() -> Self {
        Self::new()
    }
}

/// Thread-safe shared handle to a `BlobLocalityMap`.
pub type SharedBlobLocalityMap = Arc<RwLock<BlobLocalityMap>>;

/// Create a new shared blob locality map.
pub fn new_shared_blob_locality_map() -> SharedBlobLocalityMap {
    Arc::new(RwLock::new(BlobLocalityMap::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_lookup() {
        let mut map = BlobLocalityMap::new();
        let d1 = DigestInfo::new([1u8; 32], 100);
        let d2 = DigestInfo::new([2u8; 32], 200);

        map.register_blobs("worker-a:50081", &[d1, d2]);
        map.register_blobs("worker-b:50081", &[d1]);

        let workers = map.lookup_workers(&d1);
        assert_eq!(workers.len(), 2);
        assert!(workers.contains(&Arc::from("worker-a:50081")));
        assert!(workers.contains(&Arc::from("worker-b:50081")));

        let workers = map.lookup_workers(&d2);
        assert_eq!(workers.len(), 1);
        assert!(workers.contains(&Arc::from("worker-a:50081")));
    }

    #[test]
    fn test_evict_blobs() {
        let mut map = BlobLocalityMap::new();
        let d1 = DigestInfo::new([1u8; 32], 100);
        let d2 = DigestInfo::new([2u8; 32], 200);

        map.register_blobs("worker-a:50081", &[d1, d2]);
        map.evict_blobs("worker-a:50081", &[d1]);

        assert!(map.lookup_workers(&d1).is_empty());
        assert_eq!(map.lookup_workers(&d2).len(), 1);
    }

    #[test]
    fn test_remove_endpoint() {
        let mut map = BlobLocalityMap::new();
        let d1 = DigestInfo::new([1u8; 32], 100);
        let d2 = DigestInfo::new([2u8; 32], 200);

        map.register_blobs("worker-a:50081", &[d1, d2]);
        map.register_blobs("worker-b:50081", &[d1]);

        map.remove_endpoint("worker-a:50081");

        // d1 still available on worker-b
        let workers = map.lookup_workers(&d1);
        assert_eq!(workers.len(), 1);
        assert!(workers.contains(&Arc::from("worker-b:50081")));

        // d2 no longer available anywhere
        assert!(map.lookup_workers(&d2).is_empty());
    }

    #[test]
    fn test_lookup_unknown_digest() {
        let map = BlobLocalityMap::new();
        let d1 = DigestInfo::new([1u8; 32], 100);
        assert!(map.lookup_workers(&d1).is_empty());
    }

    #[test]
    fn test_blobs_map_accessor() {
        let mut map = BlobLocalityMap::new();
        let d1 = DigestInfo::new([1u8; 32], 100);
        let d2 = DigestInfo::new([2u8; 32], 200);

        map.register_blobs("worker-a:50081", &[d1, d2]);
        map.register_blobs("worker-b:50081", &[d1]);

        let blobs = map.blobs_map();
        assert_eq!(blobs.len(), 2);

        // d1 has two endpoints
        let d1_endpoints = blobs.get(&d1).unwrap();
        assert_eq!(d1_endpoints.len(), 2);
        assert!(d1_endpoints.contains_key("worker-a:50081"));
        assert!(d1_endpoints.contains_key("worker-b:50081"));

        // d2 has one endpoint
        let d2_endpoints = blobs.get(&d2).unwrap();
        assert_eq!(d2_endpoints.len(), 1);
        assert!(d2_endpoints.contains_key("worker-a:50081"));
    }

    #[test]
    fn test_re_registration_updates_timestamp() {
        let mut map = BlobLocalityMap::new();
        let d1 = DigestInfo::new([1u8; 32], 100);

        map.register_blobs("worker-a", &[d1]);
        let ts1 = *map
            .blobs_map()
            .get(&d1)
            .unwrap()
            .get("worker-a")
            .unwrap();

        // Spin until the clock advances (SystemTime resolution varies by OS).
        loop {
            if SystemTime::now() > ts1 {
                break;
            }
        }

        map.register_blobs("worker-a", &[d1]);
        let ts2 = *map
            .blobs_map()
            .get(&d1)
            .unwrap()
            .get("worker-a")
            .unwrap();

        assert!(
            ts2 > ts1,
            "Expected re-registration to update timestamp: ts1={ts1:?}, ts2={ts2:?}"
        );
    }

    #[test]
    fn test_evict_all_blobs_removes_endpoint() {
        let mut map = BlobLocalityMap::new();
        let d1 = DigestInfo::new([1u8; 32], 100);
        let d2 = DigestInfo::new([2u8; 32], 200);

        map.register_blobs("worker-a", &[d1, d2]);
        assert_eq!(map.endpoint_count(), 1);

        map.evict_blobs("worker-a", &[d1, d2]);

        assert_eq!(map.endpoint_count(), 0);
        assert_eq!(map.digest_count(), 0);
        assert!(map.lookup_workers(&d1).is_empty());
        assert!(map.lookup_workers(&d2).is_empty());
        // endpoint_blobs should be fully cleaned up
        assert!(map.all_endpoints().is_empty());
    }

    #[test]
    fn test_partial_eviction_preserves_remaining() {
        let mut map = BlobLocalityMap::new();
        let d1 = DigestInfo::new([1u8; 32], 100);
        let d2 = DigestInfo::new([2u8; 32], 200);
        let d3 = DigestInfo::new([3u8; 32], 300);

        map.register_blobs("worker-a", &[d1, d2, d3]);
        assert_eq!(map.digest_count(), 3);
        assert_eq!(map.endpoint_count(), 1);

        map.evict_blobs("worker-a", &[d1]);

        assert!(map.lookup_workers(&d1).is_empty());
        assert_eq!(map.lookup_workers(&d2), vec![Arc::from("worker-a")]);
        assert_eq!(map.lookup_workers(&d3), vec![Arc::from("worker-a")]);
        assert_eq!(map.digest_count(), 2);
        assert_eq!(map.endpoint_count(), 1);
    }

    #[test]
    fn test_evict_unknown_digest_is_noop() {
        let mut map = BlobLocalityMap::new();
        let d1 = DigestInfo::new([1u8; 32], 100);
        let d2 = DigestInfo::new([2u8; 32], 200);

        map.register_blobs("worker-a", &[d1]);

        // Evict a digest that was never registered — should not panic.
        map.evict_blobs("worker-a", &[d2]);

        assert_eq!(map.lookup_workers(&d1), vec![Arc::from("worker-a")]);
        assert_eq!(map.endpoint_count(), 1);
        assert_eq!(map.digest_count(), 1);
    }

    #[test]
    fn test_complex_multi_endpoint_topology() {
        let mut map = BlobLocalityMap::new();
        let d1 = DigestInfo::new([1u8; 32], 100);
        let d2 = DigestInfo::new([2u8; 32], 200);
        let d3 = DigestInfo::new([3u8; 32], 300);
        let d4 = DigestInfo::new([4u8; 32], 400);
        let d5 = DigestInfo::new([5u8; 32], 500);

        map.register_blobs("worker-a", &[d1, d2, d3]);
        map.register_blobs("worker-b", &[d2, d3, d4]);
        map.register_blobs("worker-c", &[d4, d5]);

        assert_eq!(map.digest_count(), 5);
        assert_eq!(map.endpoint_count(), 3);

        // D2 on both worker-a and worker-b
        let d2_workers = map.lookup_workers(&d2);
        assert_eq!(d2_workers.len(), 2);
        assert!(d2_workers.contains(&Arc::from("worker-a")));
        assert!(d2_workers.contains(&Arc::from("worker-b")));

        // Remove worker-b
        map.remove_endpoint("worker-b");

        assert_eq!(map.endpoint_count(), 2);

        // D2 still on worker-a
        let d2_workers = map.lookup_workers(&d2);
        assert_eq!(d2_workers.len(), 1);
        assert!(d2_workers.contains(&Arc::from("worker-a")));

        // D4 still on worker-c
        let d4_workers = map.lookup_workers(&d4);
        assert_eq!(d4_workers.len(), 1);
        assert!(d4_workers.contains(&Arc::from("worker-c")));

        // D3 only on worker-a now
        let d3_workers = map.lookup_workers(&d3);
        assert_eq!(d3_workers.len(), 1);
        assert!(d3_workers.contains(&Arc::from("worker-a")));

        // D1 still on worker-a, D5 still on worker-c
        assert_eq!(map.lookup_workers(&d1).len(), 1);
        assert_eq!(map.lookup_workers(&d5).len(), 1);
        assert_eq!(map.digest_count(), 5);
    }

    #[test]
    fn test_digest_count_and_endpoint_count_consistency() {
        let mut map = BlobLocalityMap::new();
        let d1 = DigestInfo::new([1u8; 32], 100);
        let d2 = DigestInfo::new([2u8; 32], 200);
        let d3 = DigestInfo::new([3u8; 32], 300);

        // Step 1: Empty map.
        assert_eq!(map.digest_count(), 0);
        assert_eq!(map.endpoint_count(), 0);

        // Step 2: Register d1, d2 on worker-a.
        map.register_blobs("worker-a", &[d1, d2]);
        assert_eq!(map.digest_count(), 2);
        assert_eq!(map.endpoint_count(), 1);

        // Step 3: Register d2, d3 on worker-b (d2 shared).
        map.register_blobs("worker-b", &[d2, d3]);
        assert_eq!(map.digest_count(), 3);
        assert_eq!(map.endpoint_count(), 2);

        // Step 4: Evict d1 from worker-a (d1 disappears entirely).
        map.evict_blobs("worker-a", &[d1]);
        assert_eq!(map.digest_count(), 2);
        assert_eq!(map.endpoint_count(), 2);

        // Step 5: Evict d2 from worker-a (d2 still on worker-b).
        map.evict_blobs("worker-a", &[d2]);
        assert_eq!(map.digest_count(), 2); // d2 and d3 remain
        assert_eq!(map.endpoint_count(), 1); // worker-a removed (empty)

        // Step 6: Remove worker-b entirely.
        map.remove_endpoint("worker-b");
        assert_eq!(map.digest_count(), 0);
        assert_eq!(map.endpoint_count(), 0);
    }

    #[test]
    fn test_lookup_workers_with_timestamps() {
        let mut map = BlobLocalityMap::new();
        let d1 = DigestInfo::new([1u8; 32], 100);

        map.register_blobs("worker-a:50081", &[d1]);
        map.register_blobs("worker-b:50081", &[d1]);

        let workers_with_ts = map.lookup_workers_with_timestamps(&d1);
        assert_eq!(
            workers_with_ts.len(),
            2,
            "Expected 2 endpoints with timestamps"
        );

        // Both timestamps should be non-UNIX_EPOCH (i.e., set to SystemTime::now()).
        for (endpoint, ts) in &workers_with_ts {
            assert!(
                *ts > std::time::UNIX_EPOCH,
                "Expected valid timestamp for {endpoint}, got {ts:?}"
            );
        }

        // Verify endpoint names match.
        let endpoints: Vec<&str> = workers_with_ts.iter().map(|(e, _)| &**e).collect();
        assert!(
            endpoints.contains(&"worker-a:50081"),
            "Expected worker-a:50081 in results"
        );
        assert!(
            endpoints.contains(&"worker-b:50081"),
            "Expected worker-b:50081 in results"
        );
    }

    #[test]
    fn test_lookup_workers_with_timestamps_unknown_digest() {
        let map = BlobLocalityMap::new();
        let d1 = DigestInfo::new([1u8; 32], 100);
        let result = map.lookup_workers_with_timestamps(&d1);
        assert!(
            result.is_empty(),
            "Expected empty result for unknown digest"
        );
    }
}
