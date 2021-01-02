// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::collections::HashMap;
use std::sync::Arc;

use async_mutex::Mutex;
use async_trait::async_trait;
use hex::FromHex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use error::{error_if, Code, Error, ResultExt};
use traits::StoreTrait;

#[derive(Debug)]
pub struct MemoryStore {
    map: Mutex<HashMap<[u8; 32], Arc<Vec<u8>>>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        MemoryStore {
            map: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl StoreTrait for MemoryStore {
    async fn has(&self, hash: &str, _expected_size: usize) -> Result<bool, Error> {
        let raw_key =
            <[u8; 32]>::from_hex(&hash).err_tip(|| "Hex length is not 64 hex characters")?;
        let map = self.map.lock().await;
        Ok(map.contains_key(&raw_key))
    }

    async fn update<'a, 'b>(
        &'a self,
        hash: &'a str,
        expected_size: usize,
        mut reader: Box<dyn AsyncRead + Send + Unpin + 'b>,
    ) -> Result<(), Error> {
        let raw_key =
            <[u8; 32]>::from_hex(&hash).err_tip(|| "Hex length is not 64 hex characters")?;
        let mut buffer = Vec::new();
        let read_size = reader.read_to_end(&mut buffer).await?;
        error_if!(
            read_size != expected_size,
            "Expected size {} but got size {} for hash {} CAS insert",
            expected_size,
            read_size,
            hash
        );
        let mut map = self.map.lock().await;
        map.insert(raw_key, Arc::new(buffer));
        Ok(())
    }

    async fn get(
        &self,
        hash: &str,
        _expected_size: usize,
        writer: &mut (dyn AsyncWrite + Send + Unpin),
    ) -> Result<(), Error> {
        let raw_key =
            <[u8; 32]>::from_hex(&hash).err_tip(|| "Hex length is not 64 hex characters")?;
        let map = self.map.lock().await;
        let value = map
            .get(&raw_key)
            .err_tip_with_code(|_| (Code::NotFound, format!("Hash {} not found", hash)))?;
        writer.write_all(value).await?;
        Ok(())
    }
}
