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

use std::collections::HashMap;

use nativelink_metric::{MetricsComponent, RootMetricsComponent};
use nativelink_util::store_trait::Store;
use parking_lot::RwLock;
use tracing::{info, warn};

#[derive(Debug, Default, MetricsComponent)]
pub struct StoreManager {
    #[metric]
    stores: RwLock<HashMap<String, Store>>,
}

impl StoreManager {
    pub fn new() -> Self {
        Self {
            stores: RwLock::new(HashMap::new()),
        }
    }

    pub fn add_store(&self, name: &str, store: Store) {
        let mut stores = self.stores.write();
        stores.insert(name.to_string(), store);
    }

    pub fn get_store(&self, name: &str) -> Option<Store> {
        let stores = self.stores.read();
        if let Some(store) = stores.get(name) {
            return Some(store.clone());
        }
        None
    }

    /// Flush all in-flight background slow writes across all FastSlowStores.
    /// Called during graceful shutdown to ensure blobs are persisted before exit.
    /// Walks the wrapper chain (ExistenceCacheStore → VerifyStore → etc.)
    /// to find nested FastSlowStores.
    pub async fn flush_slow_writes(&self, timeout: core::time::Duration) {
        use crate::existence_cache_store::ExistenceCacheStore;
        use crate::fast_slow_store::FastSlowStore;
        use crate::verify_store::VerifyStore;
        use nativelink_util::store_trait::StoreDriver;

        /// Walk the store wrapper chain to find a FastSlowStore.
        /// ExistenceCacheStore and VerifyStore return `self` from
        /// `inner_store()` (trait method), so we use `as_any()` to
        /// downcast to known wrapper types and access their typed
        /// inner_store() methods instead.
        fn find_fast_slow<'a>(store: &'a dyn StoreDriver) -> Option<&'a FastSlowStore> {
            if let Some(fss) = store.as_any().downcast_ref::<FastSlowStore>() {
                return Some(fss);
            }
            if let Some(ecs) = store.as_any().downcast_ref::<ExistenceCacheStore<std::time::SystemTime>>() {
                return find_fast_slow(
                    ecs.inner_store().inner_store(
                        Option::<nativelink_util::store_trait::StoreKey<'_>>::None,
                    ),
                );
            }
            if let Some(vs) = store.as_any().downcast_ref::<VerifyStore>() {
                return find_fast_slow(
                    vs.inner_store().inner_store(
                        Option::<nativelink_util::store_trait::StoreKey<'_>>::None,
                    ),
                );
            }
            // Unknown wrapper — try the trait inner_store as fallback.
            let inner = store.inner_store(None);
            if core::ptr::eq(
                inner as *const dyn StoreDriver,
                store as *const dyn StoreDriver,
            ) {
                return None;
            }
            find_fast_slow(inner)
        }

        let stores: Vec<(String, Store)> = {
            let guard = self.stores.read();
            guard.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };

        for (name, store) in &stores {
            let driver: &dyn StoreDriver = store.inner_store(Option::<nativelink_util::store_trait::StoreKey<'_>>::None);
            let Some(fss) = find_fast_slow(driver) else {
                continue;
            };
            let count = fss.in_flight_slow_write_count();
            if count > 0 {
                info!(
                    store = %name,
                    count,
                    "flushing in-flight slow writes before shutdown"
                );
                let remaining = fss.flush_slow_writes(timeout).await;
                if remaining > 0 {
                    warn!(
                        store = %name,
                        remaining,
                        "some slow writes did not complete before shutdown timeout"
                    );
                } else {
                    info!(store = %name, "all slow writes flushed");
                }
            }
        }
    }
}

impl RootMetricsComponent for StoreManager {}
