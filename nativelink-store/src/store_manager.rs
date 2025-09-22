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
}

impl RootMetricsComponent for StoreManager {}
