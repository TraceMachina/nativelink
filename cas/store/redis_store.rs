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

use async_trait::async_trait;
use buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use common::DigestInfo;
use error::{Error, ResultExt, Code};
use redis::{AsyncCommands, Client as RedisClient};
use traits::{StoreTrait, UploadSizeInfo};
use futures::future::try_join_all;

pub struct RedisStore {
    client: RedisClient,
}

impl RedisStore {
    pub async fn new(config: &config::stores::RedisStore) -> Result<Self, Error> {
        let url = config.url.as_ref().ok_or_else(|| Error::new(Code::InvalidArgument, "URL is not set".to_string()))?;
        let client = RedisClient::open(url.clone())?;
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
        let client = self.client.clone(); // Clone the client to use it inside the async block

        let futures: Vec<_> = digests.iter().enumerate().map(|(index, digest)| {
            let client = client.clone(); // Clone the client for each future
            async move {
                let mut conn = client.get_multiplexed_async_connection().await?;
                let exists = conn.exists(&digest.packed_hash).await?;
                Ok::<(usize, _), Error>((index, exists))
            }
        }).collect();

        let results_vec: Vec<_> = try_join_all(futures).await?;

        for (index, exists) in results_vec {
            results[index] = exists;
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;

        let size = match size_info {
            UploadSizeInfo::ExactSize(size) => size,
            // handle other variants as needed
            UploadSizeInfo::MaxSize(size) => size,
        };

        let buffer = reader
            .collect_all_with_size_hint(size)
            .await
            .err_tip(|| "Failed to collect all bytes from reader in redis_store::update")?;

        let _: () = conn.set(&digest.packed_hash, &buffer[..]).await?;
        Ok(())
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        let mut conn = self.client.get_multiplexed_async_connection().await?;
        let value: Vec<u8> = conn.get::<_, Vec<u8>>(&digest.packed_hash).await?;
        let data = &value[offset..length.unwrap_or(value.len())];
        writer.send(Bytes::copy_from_slice(data)).await?;
        Ok(())
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
