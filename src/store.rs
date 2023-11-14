// Copyright 2022 The Native Link Authors. All rights reserved.
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

pub mod ac_utils;
pub mod compression_store;
pub mod dedup_store;
pub mod default_store_factory;
pub mod existence_store;
pub mod fast_slow_store;
pub mod filesystem_store;
pub mod grpc_store;
pub mod memory_store;
pub mod noop_store;
pub mod ref_store;
pub mod s3_store;
pub mod shard_store;
pub mod size_partitioning_store;
pub mod traits;
pub mod verify_store;

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

pub use traits::{StoreTrait as Store, UploadSizeInfo};

pub struct StoreManager {
    stores: RwLock<HashMap<String, Arc<dyn Store>>>,
}

impl StoreManager {
    pub fn new() -> StoreManager {
        StoreManager {
            stores: RwLock::new(HashMap::new()),
        }
    }

    pub fn add_store(&self, name: &str, store: Arc<dyn Store>) {
        let mut stores = self.stores.write().expect("Failed to lock mutex in add_store()");
        stores.insert(name.to_string(), store);
    }

    pub fn get_store(&self, name: &str) -> Option<Arc<dyn Store>> {
        let stores = self.stores.read().expect("Failed to lock read mutex in get_store()");
        if let Some(store) = stores.get(name) {
            return Some(store.clone());
        }
        None
    }
}

impl Default for StoreManager {
    fn default() -> Self {
        Self::new()
    }
}
