// Copyright 2023 The Native Link Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::pin::Pin;
use std::sync::Arc;

use error::Error;
use native_link_store::fast_slow_store::FastSlowStore;
use native_link_store::memory_store::MemoryStore;
use native_link_util::common::DigestInfo;
use native_link_util::store_trait::Store;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};

const MEGABYTE_SZ: usize = 1024 * 1024;

fn make_stores() -> (Arc<impl Store>, Arc<impl Store>, Arc<impl Store>) {
    let fast_store = Arc::new(MemoryStore::new(&native_link_config::stores::MemoryStore::default()));
    let slow_store = Arc::new(MemoryStore::new(&native_link_config::stores::MemoryStore::default()));
    let fast_slow_store = Arc::new(FastSlowStore::new(
        &native_link_config::stores::FastSlowStore {
            fast: native_link_config::stores::StoreConfig::memory(native_link_config::stores::MemoryStore::default()),
            slow: native_link_config::stores::StoreConfig::memory(native_link_config::stores::MemoryStore::default()),
        },
        fast_store.clone(),
        slow_store.clone(),
    ));
    (fast_slow_store, fast_store, slow_store)
}

fn make_random_data(sz: usize) -> Vec<u8> {
    let mut value = vec![0u8; sz];
    let mut rng = SmallRng::seed_from_u64(1);
    rng.fill(&mut value[..]);
    value
}

async fn check_data<S: Store>(
    check_store: Pin<&S>,
    digest: DigestInfo,
    original_data: &Vec<u8>,
    debug_name: &str,
) -> Result<(), Error> {
    assert!(
        check_store.has(digest).await?.is_some(),
        "Expected data to exist in {} store",
        debug_name
    );

    let store_data = check_store.get_part_unchunked(digest, 0, None, None).await?;
    assert_eq!(
        store_data, original_data,
        "Expected data to match in {} store",
        debug_name
    );
    Ok(())
}

#[cfg(test)]
mod fast_slow_store_tests {
    use pretty_assertions::assert_eq;

    use super::*; // Must be declared in every module.

    const VALID_HASH: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

    #[tokio::test]
    async fn write_large_amount_to_both_stores_test() -> Result<(), Error> {
        let (store, fast_store, slow_store) = make_stores();
        let store = Pin::new(store.as_ref());

        let original_data = make_random_data(20 * MEGABYTE_SZ);
        let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
        store.update_oneshot(digest, original_data.clone().into()).await?;

        check_data(store, digest, &original_data, "fast_slow").await?;
        check_data(Pin::new(fast_store.as_ref()), digest, &original_data, "fast").await?;
        check_data(Pin::new(slow_store.as_ref()), digest, &original_data, "slow").await?;

        Ok(())
    }

    #[tokio::test]
    async fn fetch_slow_store_puts_in_fast_store_test() -> Result<(), Error> {
        let (fast_slow_store, fast_store, slow_store) = make_stores();
        let fast_slow_store = Pin::new(fast_slow_store.as_ref());
        let fast_store = Pin::new(fast_store.as_ref());
        let slow_store = Pin::new(slow_store.as_ref());

        let original_data = make_random_data(MEGABYTE_SZ);
        let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
        slow_store.update_oneshot(digest, original_data.clone().into()).await?;

        assert_eq!(fast_slow_store.has(digest).await, Ok(Some(original_data.len())));
        assert_eq!(fast_store.has(digest).await, Ok(None));
        assert_eq!(slow_store.has(digest).await, Ok(Some(original_data.len())));

        // This get() request should place the data in fast_store too.
        fast_slow_store.get_part_unchunked(digest, 0, None, None).await?;

        // Now the data should exist in all the stores.
        check_data(fast_store, digest, &original_data, "fast_store").await?;
        check_data(slow_store, digest, &original_data, "slow_store").await?;

        Ok(())
    }

    #[tokio::test]
    async fn partial_reads_copy_full_to_fast_store_test() -> Result<(), Error> {
        let (fast_slow_store, fast_store, slow_store) = make_stores();
        let fast_slow_store = Pin::new(fast_slow_store.as_ref());
        let fast_store = Pin::new(fast_store.as_ref());
        let slow_store = Pin::new(slow_store.as_ref());

        let original_data = make_random_data(MEGABYTE_SZ);
        let digest = DigestInfo::try_new(VALID_HASH, 100).unwrap();
        slow_store.update_oneshot(digest, original_data.clone().into()).await?;

        // This get() request should place the data in fast_store too.
        assert_eq!(
            original_data[10..60],
            fast_slow_store.get_part_unchunked(digest, 10, Some(50), None).await?
        );

        // Full data should exist in the fast store even though only partially
        // read.
        check_data(slow_store, digest, &original_data, "slow_store").await?;
        check_data(fast_store, digest, &original_data, "fast_store").await?;

        Ok(())
    }

    #[test]
    fn calculate_range_test() {
        let test = |start_range, end_range| FastSlowStore::calculate_range(&start_range, &end_range);
        {
            // Exact match.
            let received_range = 0..1;
            let send_range = 0..1;
            let expected_results = Some(0..1);
            assert_eq!(test(received_range, send_range), expected_results);
        }
        {
            // Minus one on received_range.
            let received_range = 1..4;
            let send_range = 1..5;
            let expected_results = Some(0..3);
            assert_eq!(test(received_range, send_range), expected_results);
        }
        {
            // Minus one on send_range.
            let received_range = 1..5;
            let send_range = 1..4;
            let expected_results = Some(0..3);
            assert_eq!(test(received_range, send_range), expected_results);
        }
        {
            // Should have already sent all data (start fence post).
            let received_range = 1..2;
            let send_range = 0..1;
            let expected_results = None;
            assert_eq!(test(received_range, send_range), expected_results);
        }
        {
            // Definiltly already sent data.
            let received_range = 2..3;
            let send_range = 0..1;
            let expected_results = None;
            assert_eq!(test(received_range, send_range), expected_results);
        }
        {
            // All data should be sent (inside range).
            let received_range = 3..4;
            let send_range = 0..100;
            let expected_results = Some(0..1); // Note: This is relative received_range.start.
            assert_eq!(test(received_range, send_range), expected_results);
        }
        {
            // Subset of received data should be sent.
            let received_range = 1..100;
            let send_range = 3..4;
            let expected_results = Some(2..3); // Note: This is relative received_range.start.
            assert_eq!(test(received_range, send_range), expected_results);
        }
        {
            // We are clearly not at the offset yet.
            let received_range = 0..1;
            let send_range = 3..4;
            let expected_results = None;
            assert_eq!(test(received_range, send_range), expected_results);
        }
        {
            // Not at offset yet (fence post).
            let received_range = 0..1;
            let send_range = 1..2;
            let expected_results = None;
            assert_eq!(test(received_range, send_range), expected_results);
        }
        {
            // Head part of the received data should be sent.
            let received_range = 1..3;
            let send_range = 2..5;
            let expected_results = Some(1..2);
            assert_eq!(test(received_range, send_range), expected_results);
        }
    }
}
