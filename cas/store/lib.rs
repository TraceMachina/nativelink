// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::sync::Arc;

pub use traits::StoreTrait as Store;

use memory_store::MemoryStore;

pub enum StoreType {
    Memory,
}

pub fn create_store(store_type: &StoreType) -> Arc<dyn Store> {
    match store_type {
        StoreType::Memory => Arc::new(MemoryStore::new()),
    }
}
