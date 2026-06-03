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

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use std::sync::Arc;

use bytes::Bytes;
use mock_instant::thread_local::MockClock;
use nativelink_config::stores::EvictionPolicy;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_util::common::DigestInfo;
use nativelink_util::evicting_map::{EvictingMap, LenEntry};
use nativelink_util::instant_wrapper::MockInstantWrapped;
use pretty_assertions::assert_eq;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BytesWrapper(Bytes);

impl LenEntry for BytesWrapper {
    #[inline]
    fn len(&self) -> u64 {
        Bytes::len(&self.0) as u64
    }

    #[inline]
    fn is_empty(&self) -> bool {
        Bytes::is_empty(&self.0)
    }
}

impl From<Bytes> for BytesWrapper {
    #[inline]
    fn from(bytes: Bytes) -> Self {
        Self(bytes)
    }
}

const HASH1: &str = "0123456789abcdef000000000000000000000000000000000123456789abcdef";
const HASH2: &str = "123456789abcdef000000000000000000000000000000000123456789abcdef1";
const HASH3: &str = "23456789abcdef000000000000000000000000000000000123456789abcdef12";
const HASH4: &str = "3456789abcdef000000000000000000000000000000000123456789abcdef012";

#[nativelink_test]
async fn insert_purges_at_max_count() -> Result<(), Error> {
    let evicting_map = EvictingMap::<DigestInfo, DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 3,
            max_seconds: 0,
            max_bytes: 0,
            evict_bytes: 0,
        },
        MockInstantWrapped::default(),
    );
    evicting_map
        .insert(DigestInfo::try_new(HASH1, 0)?, Bytes::new().into())
        .await;
    evicting_map
        .insert(DigestInfo::try_new(HASH2, 0)?, Bytes::new().into())
        .await;
    evicting_map
        .insert(DigestInfo::try_new(HASH3, 0)?, Bytes::new().into())
        .await;
    evicting_map
        .insert(DigestInfo::try_new(HASH4, 0)?, Bytes::new().into())
        .await;

    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH1, 0)?)
            .await,
        None,
        "Expected map to not have item 1"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH2, 0)?)
            .await,
        Some(0),
        "Expected map to have item 2"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH3, 0)?)
            .await,
        Some(0),
        "Expected map to have item 3"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH4, 0)?)
            .await,
        Some(0),
        "Expected map to have item 4"
    );

    Ok(())
}

#[nativelink_test]
async fn insert_purges_at_max_bytes() -> Result<(), Error> {
    const DATA: &str = "12345678";
    let evicting_map = EvictingMap::<DigestInfo, DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 0,
            max_bytes: 17,
            evict_bytes: 0,
        },
        MockInstantWrapped::default(),
    );
    evicting_map
        .insert(DigestInfo::try_new(HASH1, 0)?, Bytes::from(DATA).into())
        .await;
    evicting_map
        .insert(DigestInfo::try_new(HASH2, 0)?, Bytes::from(DATA).into())
        .await;
    evicting_map
        .insert(DigestInfo::try_new(HASH3, 0)?, Bytes::from(DATA).into())
        .await;
    evicting_map
        .insert(DigestInfo::try_new(HASH4, 0)?, Bytes::from(DATA).into())
        .await;

    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH1, 0)?)
            .await,
        None,
        "Expected map to not have item 1"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH2, 0)?)
            .await,
        None,
        "Expected map to not have item 2"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH3, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Expected map to have item 3"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH4, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Expected map to have item 4"
    );

    Ok(())
}

#[nativelink_test]
async fn insert_purges_to_low_watermark_at_max_bytes() -> Result<(), Error> {
    const DATA: &str = "12345678";
    let evicting_map = EvictingMap::<DigestInfo, DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 0,
            max_bytes: 17,
            evict_bytes: 9,
        },
        MockInstantWrapped::default(),
    );
    evicting_map
        .insert(DigestInfo::try_new(HASH1, 0)?, Bytes::from(DATA).into())
        .await;
    evicting_map
        .insert(DigestInfo::try_new(HASH2, 0)?, Bytes::from(DATA).into())
        .await;
    evicting_map
        .insert(DigestInfo::try_new(HASH3, 0)?, Bytes::from(DATA).into())
        .await;
    evicting_map
        .insert(DigestInfo::try_new(HASH4, 0)?, Bytes::from(DATA).into())
        .await;

    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH1, 0)?)
            .await,
        None,
        "Expected map to not have item 1"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH2, 0)?)
            .await,
        None,
        "Expected map to not have item 2"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH3, 0)?)
            .await,
        None,
        "Expected map to not have item 3"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH4, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Expected map to have item 4"
    );

    Ok(())
}

#[nativelink_test]
async fn insert_purges_at_max_seconds() -> Result<(), Error> {
    const DATA: &str = "12345678";

    let evicting_map = EvictingMap::<DigestInfo, DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 5,
            max_bytes: 0,
            evict_bytes: 0,
        },
        MockInstantWrapped::default(),
    );

    evicting_map
        .insert(DigestInfo::try_new(HASH1, 0)?, Bytes::from(DATA).into())
        .await;
    MockClock::advance(Duration::from_secs(2));
    evicting_map
        .insert(DigestInfo::try_new(HASH2, 0)?, Bytes::from(DATA).into())
        .await;
    MockClock::advance(Duration::from_secs(2));
    evicting_map
        .insert(DigestInfo::try_new(HASH3, 0)?, Bytes::from(DATA).into())
        .await;
    MockClock::advance(Duration::from_secs(2));
    evicting_map
        .insert(DigestInfo::try_new(HASH4, 0)?, Bytes::from(DATA).into())
        .await;

    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH1, 0)?)
            .await,
        None,
        "Expected map to not have item 1"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH2, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Expected map to have item 2"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH3, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Expected map to have item 3"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH4, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Expected map to have item 4"
    );

    Ok(())
}

#[nativelink_test]
async fn get_refreshes_time() -> Result<(), Error> {
    const DATA: &str = "12345678";

    let evicting_map = EvictingMap::<DigestInfo, DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 3,
            max_bytes: 0,
            evict_bytes: 0,
        },
        MockInstantWrapped::default(),
    );

    evicting_map
        .insert(DigestInfo::try_new(HASH1, 0)?, Bytes::from(DATA).into())
        .await;
    MockClock::advance(Duration::from_secs(2));
    evicting_map
        .insert(DigestInfo::try_new(HASH2, 0)?, Bytes::from(DATA).into())
        .await;
    MockClock::advance(Duration::from_secs(2));
    evicting_map.get(&DigestInfo::try_new(HASH1, 0)?).await; // HASH1 should now be last to be evicted.
    MockClock::advance(Duration::from_secs(2));
    evicting_map
        .insert(DigestInfo::try_new(HASH3, 0)?, Bytes::from(DATA).into())
        .await; // This will trigger an eviction.

    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH1, 0)?)
            .await,
        None,
        "Expected map to not have item 1"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH2, 0)?)
            .await,
        None,
        "Expected map to not have item 2"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH3, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Expected map to have item 3"
    );

    Ok(())
}

#[nativelink_test]
async fn unref_called_on_replace() -> Result<(), Error> {
    #[derive(Debug)]
    struct MockEntry {
        data: Bytes,
        unref_called: AtomicBool,
    }

    impl LenEntry for MockEntry {
        fn len(&self) -> u64 {
            // Note: We are not testing this functionality.
            0
        }

        fn is_empty(&self) -> bool {
            unreachable!("We are not testing this functionality");
        }

        async fn unref(&self) {
            self.unref_called.store(true, Ordering::Relaxed);
        }
    }

    const DATA1: &str = "12345678";
    const DATA2: &str = "87654321";

    let evicting_map =
        EvictingMap::<DigestInfo, DigestInfo, Arc<MockEntry>, MockInstantWrapped>::new(
            &EvictionPolicy {
                max_count: 1,
                max_seconds: 0,
                max_bytes: 0,
                evict_bytes: 0,
            },
            MockInstantWrapped::default(),
        );

    let (entry1, entry2) = {
        let entry1 = Arc::new(MockEntry {
            data: Bytes::from(DATA1),
            unref_called: AtomicBool::new(false),
        });
        evicting_map
            .insert(DigestInfo::try_new(HASH1, 0)?, entry1.clone())
            .await;

        let entry2 = Arc::new(MockEntry {
            data: Bytes::from(DATA2),
            unref_called: AtomicBool::new(false),
        });
        evicting_map
            .insert(DigestInfo::try_new(HASH1, 0)?, entry2.clone())
            .await;
        (entry1, entry2)
    };

    let existing_entry = evicting_map
        .get(&DigestInfo::try_new(HASH1, 0)?)
        .await
        .unwrap();
    assert_eq!(existing_entry.data, DATA2);

    assert!(entry1.unref_called.load(Ordering::Relaxed));
    assert!(!entry2.unref_called.load(Ordering::Relaxed));

    Ok(())
}

#[nativelink_test]
async fn contains_key_refreshes_time() -> Result<(), Error> {
    const DATA: &str = "12345678";

    let evicting_map = EvictingMap::<DigestInfo, DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 3,
            max_bytes: 0,
            evict_bytes: 0,
        },
        MockInstantWrapped::default(),
    );

    evicting_map
        .insert(DigestInfo::try_new(HASH1, 0)?, Bytes::from(DATA).into())
        .await;
    MockClock::advance(Duration::from_secs(2));
    evicting_map
        .insert(DigestInfo::try_new(HASH2, 0)?, Bytes::from(DATA).into())
        .await;
    MockClock::advance(Duration::from_secs(2));
    evicting_map
        .size_for_key(&DigestInfo::try_new(HASH1, 0)?)
        .await; // HASH1 should now be last to be evicted.
    MockClock::advance(Duration::from_secs(2));
    evicting_map
        .insert(DigestInfo::try_new(HASH3, 0)?, Bytes::from(DATA).into())
        .await; // This will trigger an eviction.

    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH1, 0)?)
            .await,
        None,
        "Expected map to not have item 1"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH2, 0)?)
            .await,
        None,
        "Expected map to not have item 2"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH3, 0)?)
            .await,
        Some(8),
        "Expected map to have item 3"
    );

    Ok(())
}

#[nativelink_test]
async fn hashes_equal_sizes_different_doesnt_override() -> Result<(), Error> {
    let evicting_map = EvictingMap::<DigestInfo, DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 0,
            max_bytes: 0,
            evict_bytes: 0,
        },
        MockInstantWrapped::default(),
    );

    let value1 = BytesWrapper(Bytes::from_static(b"12345678"));
    let value2 = BytesWrapper(Bytes::from_static(b"87654321"));
    evicting_map
        .insert(DigestInfo::try_new(HASH1, 0)?, value1.clone())
        .await;
    evicting_map
        .insert(DigestInfo::try_new(HASH1, 1)?, value2.clone())
        .await;
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH1, 0)?)
            .await,
        Some(value1.len()),
        "HASH1/0 should exist"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH1, 1)?)
            .await,
        Some(value2.len()),
        "HASH1/1 should exist"
    );

    assert_eq!(
        evicting_map
            .get(&DigestInfo::try_new(HASH1, 0)?)
            .await
            .unwrap(),
        value1
    );
    assert_eq!(
        evicting_map
            .get(&DigestInfo::try_new(HASH1, 1)?)
            .await
            .unwrap(),
        value2
    );

    Ok(())
}

#[nativelink_test]
async fn get_evicts_on_time() -> Result<(), Error> {
    const DATA: &str = "12345678";

    let evicting_map = EvictingMap::<DigestInfo, DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 5,
            max_bytes: 0,
            evict_bytes: 0,
        },
        MockInstantWrapped::default(),
    );

    let digest_info1: DigestInfo = DigestInfo::try_new(HASH1, 0)?;
    evicting_map
        .insert(digest_info1, Bytes::from(DATA).into())
        .await;

    // Getting from map before time has expired should return the value.
    assert_eq!(
        evicting_map.get(&digest_info1).await,
        Some(Bytes::from(DATA).into())
    );

    MockClock::advance(Duration::from_secs(10));

    // Getting from map after time has expired should return None.
    assert_eq!(evicting_map.get(&digest_info1).await, None);

    Ok(())
}

#[nativelink_test]
async fn remove_evicts_on_time() -> Result<(), Error> {
    const DATA: &str = "12345678";

    let evicting_map = EvictingMap::<DigestInfo, DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 5,
            max_bytes: 0,
            evict_bytes: 0,
        },
        MockInstantWrapped::default(),
    );

    let digest_info1: DigestInfo = DigestInfo::try_new(HASH1, 0)?;
    evicting_map
        .insert(digest_info1, Bytes::from(DATA).into())
        .await;

    let digest_info2: DigestInfo = DigestInfo::try_new(HASH2, 0)?;
    evicting_map
        .insert(digest_info2, Bytes::from(DATA).into())
        .await;

    // Removing digest before time has expired should return true.
    assert!(evicting_map.remove(&digest_info2).await);

    MockClock::advance(Duration::from_secs(10));

    // Removing digest after time has expired should return false.
    assert!(!evicting_map.remove(&digest_info1).await);

    Ok(())
}

#[nativelink_test]
async fn range_multiple_items_test() -> Result<(), Error> {
    async fn get_map_range(
        evicting_map: &EvictingMap<String, String, BytesWrapper, MockInstantWrapped>,
        range: impl core::ops::RangeBounds<String> + Send,
    ) -> Vec<(String, Bytes)> {
        let mut found_values = Vec::new();
        evicting_map.range(range, |k, v: &BytesWrapper| {
            found_values.push((k.clone(), v.0.clone()));
            true
        });
        found_values
    }

    const KEY1: &str = "key-123";
    const DATA1: &str = "123";

    const KEY2: &str = "key-234";
    const DATA2: &str = "234";

    const KEY3: &str = "key-345";
    const DATA3: &str = "345";

    let evicting_map = EvictingMap::<String, String, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 0,
            max_bytes: 0,
            evict_bytes: 0,
        },
        MockInstantWrapped::default(),
    );

    evicting_map
        .insert(KEY1.into(), Bytes::from(DATA1).into())
        .await;

    evicting_map
        .insert(KEY2.into(), Bytes::from(DATA2).into())
        .await;

    evicting_map
        .insert(KEY3.into(), Bytes::from(DATA3).into())
        .await;

    {
        // Ensure all range works.
        let expected_values = vec![
            (KEY1.to_string(), Bytes::from(DATA1)),
            (KEY2.to_string(), Bytes::from(DATA2)),
            (KEY3.to_string(), Bytes::from(DATA3)),
        ];
        let found_values = get_map_range(&evicting_map, ..).await;
        assert_eq!(expected_values, found_values);
    }
    {
        // Ensure prefix but everything range works.
        let expected_values = vec![
            (KEY1.to_string(), Bytes::from(DATA1)),
            (KEY2.to_string(), Bytes::from(DATA2)),
            (KEY3.to_string(), Bytes::from(DATA3)),
        ];
        let found_values = get_map_range(&evicting_map, "key-".to_string()..).await;
        assert_eq!(expected_values, found_values);
    }
    {
        // Ensure prefix range with everything after "key-2" works.
        let expected_values = vec![
            (KEY2.to_string(), Bytes::from(DATA2)),
            (KEY3.to_string(), Bytes::from(DATA3)),
        ];
        let found_values = get_map_range(&evicting_map, "key-2".to_string()..).await;
        assert_eq!(expected_values, found_values);
    }
    {
        // Ensure prefix range with only KEY2.
        let expected_values = vec![(KEY2.to_string(), Bytes::from(DATA2))];
        let found_values = get_map_range(&evicting_map, KEY2.to_string()..KEY3.to_string()).await;
        assert_eq!(expected_values, found_values);
    }

    Ok(())
}

// `LenEntry` impl that records every `unref()` invocation so tests can
// observe whether reads or writes call into eviction paths.
#[derive(Clone, Debug)]
struct CountedUnref {
    size: u64,
    unref_count: Arc<AtomicU64>,
}

impl LenEntry for CountedUnref {
    #[inline]
    fn len(&self) -> u64 {
        self.size
    }

    #[inline]
    fn is_empty(&self) -> bool {
        self.size == 0
    }

    async fn unref(&self) {
        self.unref_count.fetch_add(1, Ordering::SeqCst);
    }
}

// Contract: a read of a fresh, present key must not call `unref()` on
// any other entry. Regression guard against an earlier implementation
// that ran the full eviction loop inside `get()` when `should_evict`
// fired at read time, cascading through expired LRU neighbors and
// billing the reader for their cleanup.
#[nativelink_test]
async fn get_does_not_cascade_evict_expired_neighbors() -> Result<(), Error> {
    let unref_count = Arc::new(AtomicU64::new(0));
    let entry = || CountedUnref {
        size: 1,
        unref_count: unref_count.clone(),
    };

    let evicting_map = EvictingMap::<DigestInfo, DigestInfo, CountedUnref, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 10,
            max_bytes: 0,
            evict_bytes: 0,
        },
        MockInstantWrapped::default(),
    );

    let key_fresh = DigestInfo::try_new(HASH1, 0)?;
    let key_old1 = DigestInfo::try_new(HASH2, 0)?;
    let key_old2 = DigestInfo::try_new(HASH3, 0)?;
    let key_old3 = DigestInfo::try_new(HASH4, 0)?;

    // T=0: insert K_fresh first so it's the LRU position, then three more.
    evicting_map.insert(key_fresh, entry()).await;
    evicting_map.insert(key_old1, entry()).await;
    evicting_map.insert(key_old2, entry()).await;
    evicting_map.insert(key_old3, entry()).await;

    // T=5: touch K_fresh — its atime becomes 5 and it moves to MRU.
    // K_old1..K_old3 stay at atime=0 and shift to the LRU side.
    MockClock::advance(Duration::from_secs(5));
    assert!(evicting_map.get(&key_fresh).await.is_some());

    // T=15: evict_older_than = 15 - 10 = 5.
    //   K_old*.atime = 0 < 5 → expired-eligible.
    //   K_fresh.atime = 5; 5 < 5 is false → NOT expired-eligible.
    MockClock::advance(Duration::from_secs(10));

    let unrefs_before = unref_count.load(Ordering::SeqCst);
    let result = evicting_map.get(&key_fresh).await;
    let unrefs_after = unref_count.load(Ordering::SeqCst);

    assert!(result.is_some(), "K_fresh should still be present");
    assert_eq!(
        unrefs_after - unrefs_before,
        0,
        "get(K_fresh) should not call unref() on any item; got {} unrefs \
         (cascading eviction of expired neighbors during a read)",
        unrefs_after - unrefs_before,
    );

    Ok(())
}

// Contract: a read of a TTL-expired key reaps exactly that one entry
// and returns None — no neighbors are touched, even if they are also
// expired. Pairs with `get_does_not_cascade_evict_expired_neighbors`.
#[nativelink_test]
async fn get_of_expired_key_reaps_only_that_key() -> Result<(), Error> {
    let unref_count = Arc::new(AtomicU64::new(0));
    let entry = || CountedUnref {
        size: 1,
        unref_count: unref_count.clone(),
    };

    let evicting_map = EvictingMap::<DigestInfo, DigestInfo, CountedUnref, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 10,
            max_bytes: 0,
            evict_bytes: 0,
        },
        MockInstantWrapped::default(),
    );

    let key_target = DigestInfo::try_new(HASH1, 0)?;
    let key_neighbor1 = DigestInfo::try_new(HASH2, 0)?;
    let key_neighbor2 = DigestInfo::try_new(HASH3, 0)?;

    // T=0: insert all three. atime=0 for each.
    evicting_map.insert(key_target, entry()).await;
    evicting_map.insert(key_neighbor1, entry()).await;
    evicting_map.insert(key_neighbor2, entry()).await;

    // T=5: refresh both neighbors so K_target is the only one expired at T=15.
    MockClock::advance(Duration::from_secs(5));
    assert!(evicting_map.get(&key_neighbor1).await.is_some());
    assert!(evicting_map.get(&key_neighbor2).await.is_some());

    // T=15: evict_older_than = 5. K_target.atime=0 < 5 → expired.
    //       K_neighbor*.atime=5; 5 < 5 is false → fresh.
    MockClock::advance(Duration::from_secs(10));

    let unrefs_before = unref_count.load(Ordering::SeqCst);
    let result = evicting_map.get(&key_target).await;
    let unrefs_after = unref_count.load(Ordering::SeqCst);

    assert!(result.is_none(), "K_target should be reaped as expired");
    assert_eq!(
        unrefs_after - unrefs_before,
        1,
        "get of an expired key should reap exactly that one entry; got {} unrefs",
        unrefs_after - unrefs_before,
    );

    // The fresh neighbors must still be present.
    assert!(evicting_map.get(&key_neighbor1).await.is_some());
    assert!(evicting_map.get(&key_neighbor2).await.is_some());

    Ok(())
}
