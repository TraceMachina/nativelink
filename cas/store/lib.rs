// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::sync::Arc;

pub use traits::{StoreConfig, StoreTrait as Store, StoreType};

use memory_store::MemoryStore;

pub fn create_store(config: &StoreConfig) -> Arc<dyn Store> {
    match config.store_type {
        StoreType::Memory => Arc::new(MemoryStore::new(&config)),
    }
}
