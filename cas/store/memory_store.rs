// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use hex::FromHex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, Error};

use traits::StoreTrait;

#[derive(Debug)]
pub struct MemoryStore {
    map: HashMap<[u8; 32], Arc<Vec<u8>>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        MemoryStore {
            map: HashMap::new(),
        }
    }
}

#[async_trait]
impl StoreTrait for MemoryStore {
    async fn has(&self, hash: &str, _expected_size: usize) -> Result<bool, Error> {
        let raw_key = <[u8; 32]>::from_hex(&hash).expect("Hex length is not 64 hex characters");
        Ok(self.map.contains_key(&raw_key))
    }

    async fn update<'a>(
        &'a mut self,
        hash: &'a str,
        expected_size: usize,
        mut reader: Box<dyn AsyncRead + Send + Unpin>,
    ) -> Result<(), Error> {
        let raw_key = <[u8; 32]>::from_hex(&hash).expect("Hex length is not 64 hex characters");
        let mut buffer = Vec::new();
        let read_size = reader.read_to_end(&mut buffer).await?;
        assert_eq!(
            read_size, expected_size,
            "Expected size {} but got size {} for hash {} CAS insert",
            expected_size, read_size, hash
        );
        self.map.insert(raw_key, Arc::new(buffer));
        Ok(())
    }

    async fn get(
        &self,
        hash: &str,
        _expected_size: usize,
        writer: &mut (dyn AsyncWrite + Send + Unpin),
    ) -> Result<(), Error> {
        let raw_key = <[u8; 32]>::from_hex(&hash).expect("Hex length is not 64 hex characters");
        let value = self
            .map
            .get(&raw_key)
            .unwrap_or_else(|| panic!("Trying to get object but could not find hash: {}", hash));
        writer.write_all(value).await?;
        Ok(())
    }
}
