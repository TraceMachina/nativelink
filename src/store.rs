pub mod ac_utils;
pub mod compression_store;
pub mod dedup_store;
pub mod default_store_factory;
pub mod fast_slow_store;
pub mod filesystem_store;
pub mod grpc_store;
pub mod memory_store;
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
