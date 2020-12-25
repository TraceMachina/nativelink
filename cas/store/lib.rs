// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

pub use traits::StoreTrait as Store;

use memory_store::MemoryStore;

pub enum StoreType {
    Memory,
}

pub fn create_store(store_type: &StoreType) -> Box<dyn Store> {
    match store_type {
        StoreType::Memory => Box::new(MemoryStore::new()),
    }
}
