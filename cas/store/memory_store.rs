// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::collections::HashMap;
use std::sync::Arc;

use async_mutex::Mutex;
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use common::DigestInfo;
use error::{error_if, Code, Error, ResultExt};
use traits::{StoreConfig, StoreTrait};

#[derive(Debug)]
pub struct MemoryStore {
    map: Mutex<HashMap<[u8; 32], Arc<Vec<u8>>>>,
    verify_size: bool,
}

impl MemoryStore {
    pub fn new(config: &StoreConfig) -> Self {
        MemoryStore {
            map: Mutex::new(HashMap::new()),
            verify_size: config.verify_size,
        }
    }
}

#[async_trait]
impl StoreTrait for MemoryStore {
    async fn has(&self, digest: &DigestInfo) -> Result<bool, Error> {
        let map = self.map.lock().await;
        Ok(map.contains_key(&digest.packed_hash))
    }

    async fn update<'a, 'b>(
        &'a self,
        digest: &'a DigestInfo,
        mut reader: Box<dyn AsyncRead + Send + Unpin + 'b>,
    ) -> Result<(), Error> {
        let mut buffer = Vec::new();
        let read_size = reader.read_to_end(&mut buffer).await? as i64;
        error_if!(
            self.verify_size && read_size != digest.size_bytes,
            "Expected size {} but got size {} for hash {} CAS insert",
            digest.size_bytes,
            read_size,
            digest.str()
        );
        let mut map = self.map.lock().await;
        map.insert(digest.packed_hash, Arc::new(buffer));
        Ok(())
    }

    async fn get(
        &self,
        digest: &DigestInfo,
        writer: &mut (dyn AsyncWrite + Send + Unpin),
    ) -> Result<(), Error> {
        let map = self.map.lock().await;
        let value = map
            .get(&digest.packed_hash)
            .err_tip_with_code(|_| (Code::NotFound, format!("Hash {} not found", digest.str())))?;
        writer.write_all(value).await?;
        Ok(())
    }
}
