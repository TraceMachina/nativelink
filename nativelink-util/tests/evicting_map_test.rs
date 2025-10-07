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

use core::sync::atomic::{AtomicBool, Ordering};
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
    let evicting_map = EvictingMap::<DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 3,
            max_seconds: 0,
            max_bytes: 0,
            evict_bytes: 0,
            eviction_grace_period_seconds: 0,
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
    let evicting_map = EvictingMap::<DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 0,
            max_bytes: 17,
            evict_bytes: 0,
            eviction_grace_period_seconds: 0,
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
    let evicting_map = EvictingMap::<DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 0,
            max_bytes: 17,
            evict_bytes: 9,
            eviction_grace_period_seconds: 0,
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

    let evicting_map = EvictingMap::<DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 5,
            max_bytes: 0,
            evict_bytes: 0,
            eviction_grace_period_seconds: 0,
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

    let evicting_map = EvictingMap::<DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 3,
            max_bytes: 0,
            evict_bytes: 0,
            eviction_grace_period_seconds: 0,
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

    let evicting_map = EvictingMap::<DigestInfo, Arc<MockEntry>, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 1,
            max_seconds: 0,
            max_bytes: 0,
            evict_bytes: 0,
            eviction_grace_period_seconds: 0,
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

    let evicting_map = EvictingMap::<DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 3,
            max_bytes: 0,
            evict_bytes: 0,
            eviction_grace_period_seconds: 0,
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
    let evicting_map = EvictingMap::<DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 0,
            max_bytes: 0,
            evict_bytes: 0,
            eviction_grace_period_seconds: 0,
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

    let evicting_map = EvictingMap::<DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 5,
            max_bytes: 0,
            evict_bytes: 0,
            eviction_grace_period_seconds: 0,
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

    let evicting_map = EvictingMap::<DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 5,
            max_bytes: 0,
            evict_bytes: 0,
            eviction_grace_period_seconds: 0,
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
        evicting_map: &EvictingMap<String, BytesWrapper, MockInstantWrapped>,
        range: impl core::ops::RangeBounds<String> + Send,
    ) -> Vec<(String, Bytes)> {
        let mut found_values = Vec::new();
        evicting_map
            .range(range, |k, v: &BytesWrapper| {
                found_values.push((k.clone(), v.0.clone()));
                true
            })
            .await;
        found_values
    }

    const KEY1: &str = "key-123";
    const DATA1: &str = "123";

    const KEY2: &str = "key-234";
    const DATA2: &str = "234";

    const KEY3: &str = "key-345";
    const DATA3: &str = "345";

    let evicting_map = EvictingMap::<String, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 0,
            max_bytes: 0,
            evict_bytes: 0,
            eviction_grace_period_seconds: 0,
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

// Here is an explanation of the test that follows in a table:
// Item,    Age (after 4th insert),Expected Reason
// 1       70s                    Exists   LRU (3) protected
// 2       70s                    Exists   LRU (3) protected
// 3       40s                    Exists   Within grace
// 4       0s.                    Exists   Just inserted
#[nativelink_test]
async fn grace_period_prevents_eviction() -> Result<(), Error> {
    const DATA: &str = "12345678";

    // Create map with small max_bytes and 60 second grace period
    let evicting_map = EvictingMap::<DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 0,
            max_bytes: 17, // Only fits 2 items of DATA
            evict_bytes: 0,
            eviction_grace_period_seconds: 60, // 60 second grace period
        },
        MockInstantWrapped::default(),
    );

    // Insert first two items
    evicting_map
        .insert(DigestInfo::try_new(HASH1, 0)?, Bytes::from(DATA).into())
        .await;
    evicting_map
        .insert(DigestInfo::try_new(HASH2, 0)?, Bytes::from(DATA).into())
        .await;

    // Advance time by 30 seconds (within grace period)
    MockClock::advance(Duration::from_secs(30));

    // Insert third item - should NOT evict items 1 or 2 because they're within grace period
    evicting_map
        .insert(DigestInfo::try_new(HASH3, 0)?, Bytes::from(DATA).into())
        .await;

    // All three items should still exist (grace period blocked eviction)
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH1, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Expected item 1 to be protected by grace period"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH2, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Expected item 2 to be protected by grace period"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH3, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Expected item 3 to exist"
    );

    // Advance time beyond grace period for items 1 and 2 (total 70 seconds)
    // Items 1 and 2 are now 70 seconds old (outside grace period)
    // Item 3 is now 40 seconds old (still within grace period)
    MockClock::advance(Duration::from_secs(40));

    // Insert fourth item - this will trigger eviction
    // Important: Grace period prevents eviction of the LRU item (item 3, age 40s)
    // But items 1 and 2 (age 70s) are outside grace period and can be evicted
    // However, the current LRU implementation checks from least-recently-used first
    // If the LRU item is protected, we stop eviction to prevent thrashing
    evicting_map
        .insert(DigestInfo::try_new(HASH4, 0)?, Bytes::from(DATA).into())
        .await;

    // All items should still exist because the LRU item (item 3) is within grace period
    // This prevents cache thrashing when items are still being actively used
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH1, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Item 1 protected because LRU item (item 3) is within grace period"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH2, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Item 2 protected because LRU item (item 3) is within grace period"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH3, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Item 3 within grace period"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH4, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Item 4 was just inserted"
    );

    Ok(())
}

#[nativelink_test]
async fn grace_period_zero_means_no_protection() -> Result<(), Error> {
    const DATA: &str = "12345678";

    // Create map with grace period of 0 (disabled)
    let evicting_map = EvictingMap::<DigestInfo, BytesWrapper, MockInstantWrapped>::new(
        &EvictionPolicy {
            max_count: 0,
            max_seconds: 0,
            max_bytes: 17, // Only fits 2 items
            evict_bytes: 0,
            eviction_grace_period_seconds: 0,
        },
        MockInstantWrapped::default(),
    );

    // Insert three items
    evicting_map
        .insert(DigestInfo::try_new(HASH1, 0)?, Bytes::from(DATA).into())
        .await;
    evicting_map
        .insert(DigestInfo::try_new(HASH2, 0)?, Bytes::from(DATA).into())
        .await;
    evicting_map
        .insert(DigestInfo::try_new(HASH3, 0)?, Bytes::from(DATA).into())
        .await;

    // Item 1 should be evicted immediately (no grace period protection)
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH1, 0)?)
            .await,
        None,
        "Expected item 1 to be evicted without grace period"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH2, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Expected item 2 to exist"
    );
    assert_eq!(
        evicting_map
            .size_for_key(&DigestInfo::try_new(HASH3, 0)?)
            .await,
        Some(DATA.len() as u64),
        "Expected item 3 to exist"
    );

    Ok(())
}
