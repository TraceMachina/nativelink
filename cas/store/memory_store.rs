// Copyright 2020-2021 Nathan (Blaise) Bruer.  All rights reserved.

use std::collections::HashMap;
use std::marker::Send;
use std::sync::Arc;

use async_mutex::Mutex;
use async_trait::async_trait;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use common::DigestInfo;
use config;
use error::{Code, ResultExt};
use traits::{ResultFuture, StoreTrait};

#[derive(Debug)]
pub struct MemoryStore {
    map: Mutex<HashMap<[u8; 32], Arc<Vec<u8>>>>,
}

impl MemoryStore {
    pub fn new(_config: &config::backends::MemoryStore) -> Self {
        MemoryStore {
            map: Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait]
impl StoreTrait for MemoryStore {
    fn has<'a>(self: std::pin::Pin<&'a Self>, digest: DigestInfo) -> ResultFuture<'a, bool> {
        Box::pin(async move {
            let map = self.map.lock().await;
            Ok(map.contains_key(&digest.packed_hash))
        })
    }

    fn update<'a>(
        self: std::pin::Pin<&'a Self>,
        digest: DigestInfo,
        mut reader: Box<dyn AsyncRead + Send + Sync + Unpin + 'a>,
    ) -> ResultFuture<'a, ()> {
        Box::pin(async move {
            let mut buffer = Vec::with_capacity(digest.size_bytes as usize);
            reader.read_to_end(&mut buffer).await?;
            let mut map = self.map.lock().await;
            map.insert(digest.packed_hash, Arc::new(buffer));
            Ok(())
        })
    }

    fn get_part<'a>(
        self: std::pin::Pin<&'a Self>,
        digest: DigestInfo,
        writer: &'a mut (dyn AsyncWrite + Send + Unpin + Sync),
        offset: usize,
        length: Option<usize>,
    ) -> ResultFuture<'a, ()> {
        Box::pin(async move {
            let map = self.map.lock().await;
            let value = map
                .get(&digest.packed_hash)
                .err_tip_with_code(|_| (Code::NotFound, format!("Hash {} not found", digest.str())))?
                .as_ref();
            let default_len = value.len() - offset;
            let length = length.unwrap_or(default_len).min(default_len);
            writer
                .write_all(&value[offset..(offset + length)])
                .await
                .err_tip(|| "Error writing all data to writer")?;
            writer.write(&[]).await.err_tip(|| "Error writing EOF to writer")?;
            writer.shutdown().await.err_tip(|| "Error shutting down writer")?;
            Ok(())
        })
    }
}
