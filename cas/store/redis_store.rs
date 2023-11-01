// Copyright 2023 The Turbo Cache Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use bytes::Bytes;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;

use async_trait::async_trait;
use buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use common::DigestInfo;
use error::{Code, Error, ResultExt};
use futures::future::try_join_all;
//use futures::stream::FuturesUnordered;
use hex::encode;
use redis::aio::ConnectionLike;
use redis::AsyncCommands;
use redis_test::{MockCmd, MockRedisConnection};
use traits::{StoreTrait, UploadSizeInfo};

#[async_trait]
pub trait RedisConnectionTrait: ConnectionLike + AsyncCommands + Send + Sync {
    fn new_mock_connection(mock_commands: Vec<MockCmd>) -> Self;
    async fn new_connection(url: &str) -> Result<Self, Error>;
}

#[async_trait]
impl RedisConnectionTrait for MockRedisConnection {
    fn new_mock_connection(mock_commands: Vec<MockCmd>) -> Self {
        MockRedisConnection::new(mock_commands)
    }
    async fn new_connection(_url: &str) -> Result<Self, Error> {
        Err(Error::new(
            Code::InvalidArgument,
            "MockRedisConnection cannot create a real connection".to_string(),
        ))
    }
}

#[async_trait]
impl RedisConnectionTrait for redis::aio::Connection {
    fn new_mock_connection(_mock_commands: Vec<MockCmd>) -> Self {
        panic!("Cannot create a mock connection from a real connection")
    }
    async fn new_connection(url: &str) -> Result<Self, Error> {
        let client = redis::Client::open(url)?;
        client.get_async_connection().await.map_err(|e| Error::from(e))
    }
}

pub struct RedisStore<T: RedisConnectionTrait + 'static> {
    conn: Arc<Mutex<T>>,
}

impl<T: RedisConnectionTrait + 'static> RedisStore<T> {
    pub async fn new(config: config::stores::RedisStore) -> Result<Self, Error> {
        let conn = Self::configure_connection(&config).await?;
        Ok(RedisStore { conn })
    }

    async fn configure_connection(config: &config::stores::RedisStore) -> Result<Arc<Mutex<T>>, Error> {
        let conn = if let Some(use_mock) = config.use_mock {
            if use_mock {
                let mock_commands = config.mock_commands.clone().unwrap_or_default();
                let mock_data = config.mock_data.clone().unwrap_or_default();
                let mut mock_cmds = Vec::new();
                let mut data_iter = mock_data.iter();
                for cmd in &mock_commands {
                    let mock_cmd = match cmd.as_str() {
                        "SET" => {
                            let digest = data_iter
                                .next()
                                .ok_or(Error::new(Code::NotFound, "Missing digest for SET".to_string()))?;
                            let data = data_iter
                                .next()
                                .ok_or(Error::new(Code::NotFound, "Missing data for SET".to_string()))?;
                            MockCmd::new(redis::cmd(cmd).arg(digest).arg(data), Ok(1))
                        }
                        "EXISTS" => {
                            let digest = data_iter
                                .next()
                                .ok_or(Error::new(Code::NotFound, "Missing digest for EXISTS".to_string()))?;
                            MockCmd::new(redis::cmd(cmd).arg(digest), Ok(1))
                        }
                        _ => return Err(Error::new(Code::NotFound, format!("Unsupported command: {}", cmd))),
                    };
                    mock_cmds.push(mock_cmd);
                }
                Ok(Arc::new(Mutex::new(T::new_mock_connection(mock_cmds))))
            } else {
                T::new_connection(config.url.as_str())
                    .await
                    .map(|connection| Arc::new(Mutex::new(connection)))
            }
        } else {
            T::new_connection(config.url.as_str())
                .await
                .map(|connection| Arc::new(Mutex::new(connection)))
        };
        conn
    }
}

#[async_trait]
impl<T: RedisConnectionTrait + 'static> StoreTrait for RedisStore<T> {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        // TODO: Do not collect use buffer_unordered(SOME_N_LIMIT)
        let futures: Vec<_> = digests
            .iter()
            .enumerate()
            .map(|(index, digest)| async move {
                let conn = Arc::clone(&self.conn);
                let mut conn = conn.lock().await;
                let packed_hash = encode(digest.packed_hash);
                let exists: bool = conn.exists(&packed_hash).await?;
                Ok::<(usize, _), Error>((index, exists))
            })
            .collect();

        let results_vec: Vec<_> = try_join_all(futures).await?;

        for (index, exists) in results_vec {
            results[index] = Some(exists as usize);
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        let size = match size_info {
            UploadSizeInfo::ExactSize(size) => size,
            UploadSizeInfo::MaxSize(size) => size,
        };

        let buffer = reader
            .collect_all_with_size_hint(size)
            .await
            .err_tip(|| "Failed to collect all bytes from reader in redis_store::update")?;

        let conn = Arc::clone(&self.conn);
        let mut conn = conn.lock().await;
        let packed_hash = encode(digest.packed_hash);
        let _: () = conn.set(&packed_hash, &buffer[..]).await?;
        Ok(())
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        let conn = Arc::clone(&self.conn);
        let mut conn = conn.lock().await;
        let packed_hash = encode(digest.packed_hash);
        let value = conn.get::<_, Vec<u8>>(&packed_hash).await?;
        let default_len = value.len() - offset;
        let bytes_wrapper = Bytes::from(value);
        let length = length.unwrap_or(default_len).min(default_len);
        if length > 0 {
            writer
                .send(bytes_wrapper.slice(offset..(offset + length)))
                .await
                .err_tip(|| "Failed to write data in Redis store")?;
        }
        writer
            .send_eof()
            .await
            .err_tip(|| "Failed to write EOF in memory store get_part")?;
        Ok(())
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
