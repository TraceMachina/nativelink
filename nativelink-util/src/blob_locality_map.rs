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
use std::sync::Arc;
use std::time::SystemTime;

use crate::common::DigestInfo;
use parking_lot::RwLock;

/// Tracks which worker endpoints have which blobs, enabling peer-to-peer
/// blob fetching between workers.
///
/// The map is bidirectional:
/// - `blobs`: digest → { endpoint → last_registered_timestamp }
/// - `endpoint_blobs`: endpoint → set of digests (for fast cleanup on disconnect)
///
/// Cleanup relies entirely on explicit eviction notifications and worker
/// disconnect (no TTL — EvictingMap's `max_seconds_since_last_access` defaults
/// to unlimited).
#[derive(Debug)]
pub struct BlobLocalityMap {
    /// digest → { endpoint → timestamp }
    blobs: HashMap<DigestInfo, HashMap<Arc<str>, SystemTime>>,
    /// endpoint → set of digests (for fast cleanup on disconnect)
    endpoint_blobs: HashMap<Arc<str>, HashSet<DigestInfo>>,
}

impl BlobLocalityMap {
    pub fn new() -> Self {
        Self {
            blobs: HashMap::new(),
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
    pub fn register_blobs_with_timestamps(
        &mut self,
        endpoint: &str,
        digests_with_ts: &[(DigestInfo, SystemTime)],
    ) {
        // Allocate the endpoint Arc<str> once; clones are O(1) atomic increments
        // instead of O(N) String allocations per digest.
        let ep: Arc<str> = endpoint.into();
        let digest_set = self
            .endpoint_blobs
            .entry(ep.clone())
            .or_default();

        for (digest, ts) in digests_with_ts {
            digest_set.insert(*digest);
            self.blobs
                .entry(*digest)
                .or_default()
                .insert(ep.clone(), *ts);
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
    pub fn blobs_map(&self) -> &HashMap<DigestInfo, HashMap<Arc<str>, SystemTime>> {
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
