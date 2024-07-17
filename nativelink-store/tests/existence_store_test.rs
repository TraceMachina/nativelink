// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use nativelink_config::stores::{ExistenceCacheStore as ExistenceCacheStoreConfig, StoreConfig};
use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_store::existence_cache_store::ExistenceCacheStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::{Store, StoreLike};
use pretty_assertions::assert_eq;

const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

#[nativelink_test]
async fn simple_exist_cache_test() -> Result<(), Error> {
    const VALUE: &str = "123";
    let config = ExistenceCacheStoreConfig {
        backend: StoreConfig::noop, // Note: Not used.
        eviction_policy: Default::default(),
    };
    let inner_store = Store::new(MemoryStore::new(
        &nativelink_config::stores::MemoryStore::default(),
    ));
    let store = ExistenceCacheStore::new(&config, inner_store.clone());

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
        store
            .has(digest)
            .await
            .err_tip(|| "Failed to check store")?,
        Some(VALUE.len()),
        "Expected digest to exist in store"
    );

    assert!(
        store.exists_in_cache(&digest).await,
        "Expected digest to exist in cache in direct check"
    );
    Ok(())
}

#[nativelink_test]
async fn update_flags_existance_cache_test() -> Result<(), Error> {
    const VALUE: &str = "123";
    let config = ExistenceCacheStoreConfig {
        backend: StoreConfig::noop,
        eviction_policy: Default::default(),
    };
    let inner_store = Store::new(MemoryStore::new(
        &nativelink_config::stores::MemoryStore::default(),
    ));
    let store = ExistenceCacheStore::new(&config, inner_store.clone());

    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    store
        .update_oneshot(digest, VALUE.into())
        .await
        .err_tip(|| "Failed to update store")?;

    assert!(
        store.exists_in_cache(&digest).await,
        "Expected digest to exist in cache"
    );
    Ok(())
}

#[nativelink_test]
async fn get_part_caches_if_exact_size_set() -> Result<(), Error> {
    const VALUE: &str = "123";
    let config = ExistenceCacheStoreConfig {
        backend: StoreConfig::noop,
        eviction_policy: Default::default(),
    };
    let inner_store = Store::new(MemoryStore::new(
        &nativelink_config::stores::MemoryStore::default(),
    ));
    let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
    inner_store
        .update_oneshot(digest, VALUE.into())
        .await
        .err_tip(|| "Failed to update store")?;
    let store = ExistenceCacheStore::new(&config, inner_store.clone());

    let _ = store
        .get_part_unchunked(digest, 0, None)
        .await
        .err_tip(|| "Expected get_part to succeed")?;

    assert!(
        store.exists_in_cache(&digest).await,
        "Expected digest to exist in cache"
    );
    Ok(())
}
