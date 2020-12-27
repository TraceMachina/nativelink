// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use hex::FromHex;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, Error, ErrorKind};

use macros::{error_if, make_err, make_input_err};
use traits::StoreTrait;

use async_mutex::Mutex;

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
        let raw_key = <[u8; 32]>::from_hex(&hash).or_else(|_| {
            println!("Foobar");
            Err(make_input_err!("Hex length is not 64 hex characters"))
        })?;
        let map = self.map.lock().await;
        Ok(map.contains_key(&raw_key))
    }

    async fn update<'a, 'b>(
        &'a self,
        hash: &'a str,
        expected_size: usize,
        mut reader: Box<dyn AsyncRead + Send + Unpin + 'b>,
    ) -> Result<(), Error> {
        let raw_key = <[u8; 32]>::from_hex(&hash)
            .or(Err(make_input_err!("Hex length is not 64 hex characters")))?;
        let mut buffer = Vec::new();
        let read_size = reader.read_to_end(&mut buffer).await?;
        error_if!(
            read_size != expected_size,
            make_input_err!(
                "Expected size {} but got size {} for hash {} CAS insert",
                expected_size,
                read_size,
                hash
            )
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
        let raw_key = <[u8; 32]>::from_hex(&hash)
            .or(Err(make_input_err!("Hex length is not 64 hex characters")))?;
        let map = self.map.lock().await;
        let value = map.get(&raw_key).ok_or(make_err!(
            ErrorKind::NotFound,
            "Trying to get object but could not find hash: {}",
            hash
        ))?;
        writer.write_all(value).await?;
        Ok(())
    }
}
