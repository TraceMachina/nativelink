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

use error::{Error, ResultExt};
use native_link_config::stores::{ExistenceStore as ExistenceStoreConfig, StoreConfig};
use native_link_store::existence_store::ExistenceStore;
use native_link_store::memory_store::MemoryStore;
use native_link_util::common::DigestInfo;
use native_link_util::store_trait::Store;

#[cfg(test)]
mod verify_store_tests {
    use super::*;

    const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

    #[tokio::test]
    async fn simple_exist_cache_test() -> Result<(), Error> {
        const VALUE: &str = "123";
        let config = ExistenceStoreConfig {
            inner: StoreConfig::noop, // Note: Not used.
            eviction_policy: Default::default(),
        };
        let inner_store = Arc::new(MemoryStore::new(&native_link_config::stores::MemoryStore::default()));
        let store_owned = ExistenceStore::new(&config, inner_store.clone());
        let store = Pin::new(&store_owned);

        let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
        store
            .update_oneshot(digest, VALUE.into())
            .await
            .err_tip(|| "Failed to update store")?;
        store.remove_from_cache(&digest).await;

        assert!(
            !store.exists_in_cache(&digest).await,
            "Expected digest to not exist in cache"
        );

        assert_eq!(
            store.has(digest).await.err_tip(|| "Failed to check store")?,
            Some(VALUE.len()),
            "Expected digest to exist in store"
        );

        assert!(
            store.exists_in_cache(&digest).await,
            "Expected digest to exist in cache in direct check"
        );
        Ok(())
    }

    #[tokio::test]
    async fn update_flags_existance_cache_test() -> Result<(), Error> {
        const VALUE: &str = "123";
        let config = ExistenceStoreConfig {
            inner: StoreConfig::noop,
            eviction_policy: Default::default(),
        };
        let inner_store = Arc::new(MemoryStore::new(&native_link_config::stores::MemoryStore::default()));
        let store = ExistenceStore::new(&config, inner_store.clone());

        let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
        Pin::new(&store)
            .update_oneshot(digest, VALUE.into())
            .await
            .err_tip(|| "Failed to update store")?;

        assert!(
            store.exists_in_cache(&digest).await,
            "Expected digest to exist in cache"
        );
        Ok(())
    }

    #[tokio::test]
    async fn get_part_caches_if_exact_size_set() -> Result<(), Error> {
        const VALUE: &str = "123";
        let config = ExistenceStoreConfig {
            inner: StoreConfig::noop,
            eviction_policy: Default::default(),
        };
        let inner_store = Arc::new(MemoryStore::new(&native_link_config::stores::MemoryStore::default()));
        let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
        Pin::new(inner_store.as_ref())
            .update_oneshot(digest, VALUE.into())
            .await
            .err_tip(|| "Failed to update store")?;
        let store = ExistenceStore::new(&config, inner_store.clone());

        let _ = Pin::new(&store)
            .get_part_unchunked(digest, 0, None, None)
            .await
            .err_tip(|| "Expected get_part to succeed")?;

        assert!(
            store.exists_in_cache(&digest).await,
            "Expected digest to exist in cache"
        );
        Ok(())
    }
}
