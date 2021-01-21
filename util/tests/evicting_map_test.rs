// Copyright 2021 Nathan (Blaise) Bruer.  All rights reserved.

use std::sync::Arc;
use std::time::{Duration, Instant};

use hex::FromHex;
use mock_instant::{Instant as MockInstant, MockClock};

use common::DigestInfo;
use config::backends::EvictionPolicy;
use evicting_map::{EvictingMap, InstantWrapper};

use error::{Error, ResultExt};

/// Our mocked out instant that we can pass to our EvictionMap.
struct MockInstantWrapped(MockInstant);

impl InstantWrapper for MockInstantWrapped {
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
        let mut evicting_map = EvictingMap::new(
            &EvictionPolicy {
                max_count: 3,
                max_seconds: 0,
                max_bytes: 0,
            },
            Instant::now(),
        );
        evicting_map.insert(DigestInfo::try_new(HASH1, 0)?, Arc::new(vec![]));
        evicting_map.insert(DigestInfo::try_new(HASH2, 0)?, Arc::new(vec![]));
        evicting_map.insert(DigestInfo::try_new(HASH3, 0)?, Arc::new(vec![]));
        evicting_map.insert(DigestInfo::try_new(HASH4, 0)?, Arc::new(vec![]));

        assert!(
            !evicting_map.contains_key(&DigestInfo::try_new(HASH1, 0)?),
            "Expected map to not have item 1"
        );
        assert!(
            evicting_map.contains_key(&DigestInfo::try_new(HASH2, 0)?),
            "Expected map to have item 2"
        );
        assert!(
            evicting_map.contains_key(&DigestInfo::try_new(HASH3, 0)?),
            "Expected map to have item 3"
        );
        assert!(
            evicting_map.contains_key(&DigestInfo::try_new(HASH4, 0)?),
            "Expected map to have item 4"
        );

        Ok(())
    }

    #[tokio::test]
    async fn insert_purges_at_max_bytes() -> Result<(), Error> {
        let mut evicting_map = EvictingMap::new(
            &EvictionPolicy {
                max_count: 0,
                max_seconds: 0,
                max_bytes: 17,
            },
            Instant::now(),
        );
        evicting_map.insert(DigestInfo::try_new(HASH1, 0)?, Arc::new("12345678".into()));
        evicting_map.insert(DigestInfo::try_new(HASH2, 0)?, Arc::new("12345678".into()));
        evicting_map.insert(DigestInfo::try_new(HASH3, 0)?, Arc::new("12345678".into()));
        evicting_map.insert(DigestInfo::try_new(HASH4, 0)?, Arc::new("12345678".into()));

        assert!(
            !evicting_map.contains_key(&DigestInfo::try_new(HASH1, 0)?),
            "Expected map to not have item 1"
        );
        assert!(
            !evicting_map.contains_key(&DigestInfo::try_new(HASH2, 0)?),
            "Expected map to not have item 2"
        );
        assert!(
            evicting_map.contains_key(&DigestInfo::try_new(HASH3, 0)?),
            "Expected map to have item 3"
        );
        assert!(
            evicting_map.contains_key(&DigestInfo::try_new(HASH4, 0)?),
            "Expected map to have item 4"
        );

        Ok(())
    }

    #[tokio::test]
    async fn insert_purges_at_max_seconds() -> Result<(), Error> {
        let mut evicting_map = EvictingMap::new(
            &EvictionPolicy {
                max_count: 0,
                max_seconds: 5,
                max_bytes: 0,
            },
            MockInstantWrapped(MockInstant::now()),
        );

        evicting_map.insert(DigestInfo::try_new(HASH1, 0)?, Arc::new("12345678".into()));
        MockClock::advance(Duration::from_secs(2));
        evicting_map.insert(DigestInfo::try_new(HASH2, 0)?, Arc::new("12345678".into()));
        MockClock::advance(Duration::from_secs(2));
        evicting_map.insert(DigestInfo::try_new(HASH3, 0)?, Arc::new("12345678".into()));
        MockClock::advance(Duration::from_secs(2));
        evicting_map.insert(DigestInfo::try_new(HASH4, 0)?, Arc::new("12345678".into()));

        assert!(
            !evicting_map.contains_key(&DigestInfo::try_new(HASH1, 0)?),
            "Expected map to not have item 1"
        );
        assert!(
            evicting_map.contains_key(&DigestInfo::try_new(HASH2, 0)?),
            "Expected map to have item 2"
        );
        assert!(
            evicting_map.contains_key(&DigestInfo::try_new(HASH3, 0)?),
            "Expected map to have item 3"
        );
        assert!(
            evicting_map.contains_key(&DigestInfo::try_new(HASH4, 0)?),
            "Expected map to have item 4"
        );

        Ok(())
    }

    #[tokio::test]
    async fn get_refreshes_time() -> Result<(), Error> {
        let mut evicting_map = EvictingMap::new(
            &EvictionPolicy {
                max_count: 0,
                max_seconds: 3,
                max_bytes: 0,
            },
            MockInstantWrapped(MockInstant::now()),
        );

        evicting_map.insert(DigestInfo::try_new(HASH1, 0)?, Arc::new("12345678".into()));
        MockClock::advance(Duration::from_secs(2));
        evicting_map.insert(DigestInfo::try_new(HASH2, 0)?, Arc::new("12345678".into()));
        MockClock::advance(Duration::from_secs(2));
        evicting_map.get(&DigestInfo::try_new(HASH1, 0)?); // HASH1 should now be last to be evicted.
        MockClock::advance(Duration::from_secs(2));
        evicting_map.insert(DigestInfo::try_new(HASH3, 0)?, Arc::new("12345678".into()));

        assert!(
            evicting_map.contains_key(&DigestInfo::try_new(HASH1, 0)?),
            "Expected map to have item 1"
        );
        assert!(
            !evicting_map.contains_key(&DigestInfo::try_new(HASH2, 0)?),
            "Expected map to not have item 2"
        );
        assert!(
            evicting_map.contains_key(&DigestInfo::try_new(HASH3, 0)?),
            "Expected map to have item 3"
        );

        Ok(())
    }

    #[tokio::test]
    async fn contains_key_refreshes_time() -> Result<(), Error> {
        let mut evicting_map = EvictingMap::new(
            &EvictionPolicy {
                max_count: 0,
                max_seconds: 3,
                max_bytes: 0,
            },
            MockInstantWrapped(MockInstant::now()),
        );

        evicting_map.insert(DigestInfo::try_new(HASH1, 0)?, Arc::new("12345678".into()));
        MockClock::advance(Duration::from_secs(2));
        evicting_map.insert(DigestInfo::try_new(HASH2, 0)?, Arc::new("12345678".into()));
        MockClock::advance(Duration::from_secs(2));
        evicting_map.contains_key(&DigestInfo::try_new(HASH1, 0)?); // HASH1 should now be last to be evicted.
        MockClock::advance(Duration::from_secs(2));
        evicting_map.insert(DigestInfo::try_new(HASH3, 0)?, Arc::new("12345678".into()));

        assert!(
            evicting_map.contains_key(&DigestInfo::try_new(HASH1, 0)?),
            "Expected map to have item 1"
        );
        assert!(
            !evicting_map.contains_key(&DigestInfo::try_new(HASH2, 0)?),
            "Expected map to not have item 2"
        );
        assert!(
            evicting_map.contains_key(&DigestInfo::try_new(HASH3, 0)?),
            "Expected map to have item 3"
        );

        Ok(())
    }
}
