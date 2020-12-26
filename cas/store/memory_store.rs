// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use async_trait::async_trait;

use tokio::io::{AsyncRead, Error};

use traits::StoreTrait;

#[derive(Debug)]
pub struct MemoryStore {}

impl MemoryStore {
    pub fn new() -> Self {
        MemoryStore {}
    }
}

#[async_trait]
impl StoreTrait for MemoryStore {
    fn has(&self, _hash: &str) -> bool {
        false
    }

    async fn update<'a>(
        &'a self,
        _hash: &'a str,
        _size: i64,
        _reader: Box<dyn AsyncRead + Send>,
    ) -> Result<(), Error> {
        Ok(())
    }
}
