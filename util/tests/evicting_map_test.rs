// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use mock_instant::{Instant as MockInstant, MockClock};

use common::DigestInfo;
use config::stores::EvictionPolicy;
use error::Error;
use evicting_map::{EvictingMap, InstantWrapper, LenEntry};

#[derive(Clone, PartialEq, Debug)]
pub struct BytesWrapper(Bytes);

impl LenEntry for BytesWrapper {
    #[inline]
    fn len(&self) -> usize {
        Bytes::len(&self.0)
    }

    #[inline]
    fn is_empty(&self) -> bool {
        Bytes::is_empty(&self.0)
    }
}

impl From<Bytes> for BytesWrapper {
    #[inline]
    fn from(bytes: Bytes) -> BytesWrapper {
        BytesWrapper(bytes)
    }
}

/// Our mocked out instant that we can pass to our EvictionMap.
struct MockInstantWrapped(MockInstant);

impl InstantWrapper for MockInstantWrapped {
    fn from_secs(_secs: u64) -> Self {
        MockInstantWrapped(MockInstant::now())
    }

    fn unix_timestamp(&self) -> u64 {
        100
    }

    fn elapsed(&self) -> Duration {
        self.0.elapsed()
    }
}

#[cfg(test)]
mod evicting_map_tests {
    use super::*;

    const HASH1: &str = "0123456789abcdef000000000000000000000000000000000123456789abcdef";
    const HASH2: &str = "123456789abcdef000000000000000000000000000000000123456789abcdef1";
    const HASH3: &str = "23456789abcdef000000000000000000000000000000000123456789abcdef12";
    const HASH4: &str = "3456789abcdef000000000000000000000000000000000123456789abcdef012";

    #[tokio::test]
    async fn insert_purges_at_max_count() -> Result<(), Error> {
        let evicting_map = EvictingMap::<BytesWrapper, MockInstantWrapped>::new(
            &EvictionPolicy {
                max_count: 3,
                max_seconds: 0,
                max_bytes: 0,
            },
            MockInstantWrapped(MockInstant::now()),
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
            evicting_map.size_for_key(&DigestInfo::try_new(HASH1, 0)?).await,
            None,
            "Expected map to not have item 1"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH2, 0)?).await,
            Some(0),
            "Expected map to have item 2"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH3, 0)?).await,
            Some(0),
            "Expected map to have item 3"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH4, 0)?).await,
            Some(0),
            "Expected map to have item 4"
        );

        Ok(())
    }

    #[tokio::test]
    async fn insert_purges_at_max_bytes() -> Result<(), Error> {
        let evicting_map = EvictingMap::<BytesWrapper, MockInstantWrapped>::new(
            &EvictionPolicy {
                max_count: 0,
                max_seconds: 0,
                max_bytes: 17,
            },
            MockInstantWrapped(MockInstant::now()),
        );
        const DATA: &str = "12345678";
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
            evicting_map.size_for_key(&DigestInfo::try_new(HASH1, 0)?).await,
            None,
            "Expected map to not have item 1"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH2, 0)?).await,
            None,
            "Expected map to not have item 2"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH3, 0)?).await,
            Some(DATA.len()),
            "Expected map to have item 3"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH4, 0)?).await,
            Some(DATA.len()),
            "Expected map to have item 4"
        );

        Ok(())
    }

    #[tokio::test]
    async fn insert_purges_at_max_seconds() -> Result<(), Error> {
        let evicting_map = EvictingMap::<BytesWrapper, MockInstantWrapped>::new(
            &EvictionPolicy {
                max_count: 0,
                max_seconds: 5,
                max_bytes: 0,
            },
            MockInstantWrapped(MockInstant::now()),
        );

        const DATA: &str = "12345678";
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
            evicting_map.size_for_key(&DigestInfo::try_new(HASH1, 0)?).await,
            None,
            "Expected map to not have item 1"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH2, 0)?).await,
            Some(DATA.len()),
            "Expected map to have item 2"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH3, 0)?).await,
            Some(DATA.len()),
            "Expected map to have item 3"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH4, 0)?).await,
            Some(DATA.len()),
            "Expected map to have item 4"
        );

        Ok(())
    }

    #[tokio::test]
    async fn get_refreshes_time() -> Result<(), Error> {
        let evicting_map = EvictingMap::<BytesWrapper, MockInstantWrapped>::new(
            &EvictionPolicy {
                max_count: 0,
                max_seconds: 3,
                max_bytes: 0,
            },
            MockInstantWrapped(MockInstant::now()),
        );

        const DATA: &str = "12345678";
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
            evicting_map.size_for_key(&DigestInfo::try_new(HASH1, 0)?).await,
            Some(DATA.len()),
            "Expected map to have item 1"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH2, 0)?).await,
            None,
            "Expected map to not have item 2"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH3, 0)?).await,
            Some(DATA.len()),
            "Expected map to have item 3"
        );

        Ok(())
    }

    #[tokio::test]
    async fn unref_called_on_replace() -> Result<(), Error> {
        #[derive(Debug)]
        struct MockEntry {
            data: Bytes,
            unref_called: AtomicBool,
        }

        #[async_trait]
        impl LenEntry for MockEntry {
            fn len(&self) -> usize {
                // Note: We are not testing this functionality.
                0
            }

            fn is_empty(&self) -> bool {
                // Note: We are not testing this functionality.
                true
            }

            async fn touch(&self) {
                // Do nothing. We are not testing this functionality.
            }

            async fn unref(&self) {
                self.unref_called.store(true, Ordering::Relaxed);
            }
        }

        let evicting_map = EvictingMap::<Arc<MockEntry>, MockInstantWrapped>::new(
            &EvictionPolicy {
                max_count: 1,
                max_seconds: 0,
                max_bytes: 0,
            },
            MockInstantWrapped(MockInstant::now()),
        );

        const DATA1: &str = "12345678";
        const DATA2: &str = "87654321";

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

        let existing_entry = evicting_map.get(&DigestInfo::try_new(HASH1, 0)?).await.unwrap();
        assert_eq!(existing_entry.data, DATA2);

        assert!(entry1.unref_called.load(Ordering::Relaxed));
        assert!(!entry2.unref_called.load(Ordering::Relaxed));

        Ok(())
    }

    #[tokio::test]
    async fn contains_key_refreshes_time() -> Result<(), Error> {
        let evicting_map = EvictingMap::<BytesWrapper, MockInstantWrapped>::new(
            &EvictionPolicy {
                max_count: 0,
                max_seconds: 3,
                max_bytes: 0,
            },
            MockInstantWrapped(MockInstant::now()),
        );

        const DATA: &str = "12345678";
        evicting_map
            .insert(DigestInfo::try_new(HASH1, 0)?, Bytes::from(DATA).into())
            .await;
        MockClock::advance(Duration::from_secs(2));
        evicting_map
            .insert(DigestInfo::try_new(HASH2, 0)?, Bytes::from(DATA).into())
            .await;
        MockClock::advance(Duration::from_secs(2));
        evicting_map.size_for_key(&DigestInfo::try_new(HASH1, 0)?).await; // HASH1 should now be last to be evicted.
        MockClock::advance(Duration::from_secs(2));
        evicting_map
            .insert(DigestInfo::try_new(HASH3, 0)?, Bytes::from(DATA).into())
            .await; // This will trigger an eviction.

        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH1, 0)?).await,
            Some(8),
            "Expected map to have item 1"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH2, 0)?).await,
            None,
            "Expected map to not have item 2"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH3, 0)?).await,
            Some(8),
            "Expected map to have item 3"
        );

        Ok(())
    }

    #[tokio::test]
    async fn hashes_equal_sizes_different_doesnt_override() -> Result<(), Error> {
        let evicting_map = EvictingMap::<BytesWrapper, MockInstantWrapped>::new(
            &EvictionPolicy {
                max_count: 0,
                max_seconds: 0,
                max_bytes: 0,
            },
            MockInstantWrapped(MockInstant::now()),
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
            evicting_map.size_for_key(&DigestInfo::try_new(HASH1, 0)?).await,
            Some(value1.len()),
            "HASH1/0 should exist"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH1, 1)?).await,
            Some(value2.len()),
            "HASH1/1 should exist"
        );

        assert_eq!(evicting_map.get(&DigestInfo::try_new(HASH1, 0)?).await.unwrap(), value1);
        assert_eq!(evicting_map.get(&DigestInfo::try_new(HASH1, 1)?).await.unwrap(), value2);

        Ok(())
    }

    #[tokio::test]
    async fn build_lru_index_and_reload() -> Result<(), Error> {
        let mut evicting_map = EvictingMap::<BytesWrapper, MockInstantWrapped>::new(
            &EvictionPolicy {
                max_count: 0,
                max_seconds: 0,
                max_bytes: 0,
            },
            MockInstantWrapped(MockInstant::now()),
        );

        let value1 = BytesWrapper(Bytes::new());
        let value2 = BytesWrapper(Bytes::new());
        evicting_map
            .insert(DigestInfo::try_new(HASH1, 0)?, value1.clone())
            .await;
        evicting_map
            .insert(DigestInfo::try_new(HASH2, 1)?, value2.clone())
            .await;

        let serialized_index = evicting_map.build_lru_index().await;

        {
            // Now insert another entry.
            let value3 = BytesWrapper(Bytes::new());
            evicting_map
                .insert(DigestInfo::try_new(HASH3, 3)?, value3.clone())
                .await;

            assert_eq!(
                evicting_map.size_for_key(&DigestInfo::try_new(HASH1, 0)?).await,
                Some(value1.len()),
                "HASH1/0 should exist"
            );
            assert_eq!(
                evicting_map.size_for_key(&DigestInfo::try_new(HASH2, 1)?).await,
                Some(value2.len()),
                "HASH2/1 should exist"
            );
            assert_eq!(
                evicting_map.size_for_key(&DigestInfo::try_new(HASH3, 3)?).await,
                Some(value3.len()),
                "HASH3/1 should exist"
            );
        }

        // Now reload from the serialized version.
        evicting_map
            .restore_lru(serialized_index, move |_digest| BytesWrapper(Bytes::new()))
            .await;

        // Data should now have the previously inserted data, but not the newly inserted data
        // that was inserted after the `build_lru_index` call.
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH1, 0)?).await,
            Some(value1.len()),
            "HASH1/0 should exist"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH2, 1)?).await,
            Some(value2.len()),
            "HASH2/1 should exist"
        );
        assert_eq!(
            evicting_map.size_for_key(&DigestInfo::try_new(HASH3, 3)?).await,
            None,
            "HASH3/2 should be empty"
        );

        Ok(())
    }
}
