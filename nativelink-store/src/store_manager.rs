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

use core::any::type_name_of_val;
use std::collections::HashMap;

use nativelink_util::store_trait::Store;
use parking_lot::RwLock;
use tracing::{info, instrument};

#[derive(Debug, Default)]
pub struct StoreManager {
    stores: RwLock<HashMap<String, Store>>,
}

impl StoreManager {
    pub fn new() -> StoreManager {
        StoreManager {
            stores: RwLock::new(HashMap::new()),
        }
    }

    #[instrument(skip(self, store))]
    pub fn add_store(&self, name: &str, store: Store) {
        // TODO(aaronmondal): This currently always prints "Store". Make the
        //                    store trait implementation properly advertise its
        //                    type.
        let store_type = type_name_of_val(&store);

        let mut stores = self.stores.write();
        stores.insert(name.to_string(), store);
        info!(
            store_name = name,
            store_type = store_type,
            "Store added to manager",
        );
    }

    #[instrument(skip(self))]
    pub fn get_store(&self, name: &str) -> Option<Store> {
        let stores = self.stores.read();
        if let Some(store) = stores.get(name) {
            return Some(store.clone());
        }
        None
    }
}
