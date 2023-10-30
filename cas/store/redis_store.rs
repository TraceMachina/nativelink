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

use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::join;

use buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use common::DigestInfo;
use error::{Error, ResultExt};
use redis::AsyncCommands;
use traits::{StoreTrait, UploadSizeInfo};

pub struct RedisStore {
    client: redis::Client,
}

impl RedisStore {
    pub async fn new(host: &str, port: u16) -> Result<Self, redis::RedisError> {
        let client = redis::Client::open(format!("redis://{}:{}", host, port))?;
        Ok(RedisStore { client })
    }
}

#[async_trait]
impl StoreTrait for RedisStore {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        let mut conn = self.client.get_async_connection().await?;
        for (digest, result) in digests.iter().zip(results.iter_mut()) {
            *result = conn.exists(digest.hash()).await?;
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        let mut conn = self.client.get_async_connection().await?;
        let _: () = conn.set(digest.hash(), reader).await?;
        Ok(())
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        let mut conn = self.client.get_async_connection().await?;
        let value: Vec<u8> = conn.get(digest.hash()).await?;
        writer
            .write_all(&value[offset..length.unwrap_or_else(|| value.len())])
            .await?;
        Ok(())
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
