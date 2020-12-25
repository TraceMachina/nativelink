// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use traits::StoreTrait;

#[derive(Debug)]
pub struct MemoryStore {}

impl MemoryStore {
    pub fn new() -> Self {
        MemoryStore {}
    }
}

impl StoreTrait for MemoryStore {
    fn has(&self, _hash: &str) -> bool {
        false
    }
}
