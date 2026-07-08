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

use nativelink_error::{Code, Error, make_err};
use nativelink_metric::{MetricsComponent, RootMetricsComponent};
use nativelink_util::store_trait::Store;
use parking_lot::RwLock;

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

    pub fn add_store(&self, name: &str, store: Store) -> Result<(), Error> {
        let mut stores = self.stores.write();
        if stores.contains_key(name) {
            return Err(make_err!(
                Code::AlreadyExists,
                "Store name '{name}' already exists in the store manager"
            ));
        }
        stores.insert(name.to_string(), store);
        Ok(())
    }

    pub fn get_store(&self, name: &str) -> Option<Store> {
        let stores = self.stores.read();
        if let Some(store) = stores.get(name) {
            return Some(store.clone());
        }
        None
    }

    pub async fn run_post_init(&self) -> Result<(), Error> {
        let stores = {
            let lock = self.stores.read();
            lock.values().cloned().collect::<Vec<_>>()
        };
        for store in stores {
            store.into_inner().post_init().await?;
        }
        Ok(())
    }
}

impl RootMetricsComponent for StoreManager {}

#[cfg(test)]
mod tests {
    use nativelink_config::stores::MemorySpec;
    use nativelink_error::Code;
    use nativelink_util::store_trait::Store;

    use super::StoreManager;
    use crate::memory_store::MemoryStore;

    #[test]
    fn add_store_rejects_duplicate_name() {
        let store_manager = StoreManager::new();
        store_manager
            .add_store("dup", Store::new(MemoryStore::new(&MemorySpec::default())))
            .expect("first add_store should succeed");
        let err = store_manager
            .add_store("dup", Store::new(MemoryStore::new(&MemorySpec::default())))
            .expect_err("duplicate store name must be rejected");
        assert_eq!(err.code, Code::AlreadyExists);
    }
}
