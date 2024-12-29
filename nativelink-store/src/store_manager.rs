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

use std::collections::HashMap;

use nativelink_error::{make_err, Code, Error};
use nativelink_metric::{MetricsComponent, RootMetricsComponent};
use nativelink_util::store_trait::Store;
use parking_lot::RwLock;

#[derive(MetricsComponent)]
pub struct StoreManager {
    #[metric]
    stores: RwLock<HashMap<String, Store>>,
    store_config_anti_collision_digests: RwLock<Vec<String>>,
}

impl StoreManager {
    pub fn new() -> StoreManager {
        StoreManager {
            stores: RwLock::new(HashMap::new()),
            store_config_anti_collision_digests: RwLock::new(vec![]),
        }
    }

    pub fn add_store(&self, name: &str, store: Store) -> Result<(), Error> {
        let mut stores = self.stores.write();
        if stores.contains_key(name) {
            return Err(make_err!(
                Code::AlreadyExists,
                "A store with the name '{}' already exists",
                name
            ));
        }
        stores.insert(name.to_string(), store);
        Ok(())
    }

    pub fn digest_not_already_present(&self, digest: &str) -> Result<(), Error> {
        let digests = self.store_config_anti_collision_digests.read();
        match digests.contains(&String::from(digest)) {
            true => Err(make_err!(
                Code::AlreadyExists,
                "the provided config is already being used by another store"
            )),
            _ => Ok(()),
        }
    }

    pub fn config_digest_add(&self, digest: String) {
        let mut digests = self.store_config_anti_collision_digests.write();
        digests.push(digest);
    }

    pub fn get_store(&self, name: &str) -> Option<Store> {
        let stores = self.stores.read();
        if let Some(store) = stores.get(name) {
            return Some(store.clone());
        }
        None
    }
}

impl RootMetricsComponent for StoreManager {}

impl Default for StoreManager {
    fn default() -> Self {
        Self::new()
    }
}
