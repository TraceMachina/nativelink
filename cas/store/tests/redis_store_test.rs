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

use error::Error;
use std::pin::Pin;

use bytes::Bytes;
use common::DigestInfo;
use redis_store::RedisStore;
use redis_test::MockRedisConnection;
use traits::StoreTrait;

const VALID_HASH1: &str = "3031323334353637383961626364656630303030303030303030303030303030";

#[cfg(test)]
mod redis_store_tests {
    use super::*;

    #[tokio::test]
    async fn verify_redis_connection() -> Result<(), Error> {
        let data = Bytes::from_static(b"13");
        let data_vec = data.to_vec();

        let config = config::stores::RedisStore {
            url: "".to_string(),
            use_mock: Some(true),
            mock_commands: Some(vec!["SET".to_string(), "EXISTS".to_string()]),
            mock_data: Some(vec![
                VALID_HASH1.to_string(),
                String::from_utf8(data_vec).unwrap(),
                VALID_HASH1.to_string(),
            ]),
        };
        let store: RedisStore<MockRedisConnection> = RedisStore::new(config).await?;
        let pinned_store: Pin<&RedisStore<MockRedisConnection>> = Pin::new(&store);

        let digest = DigestInfo::try_new(VALID_HASH1, 1)?;
        pinned_store.update_oneshot(digest, data).await?;

        let result = pinned_store.has(digest).await?;
        assert!(result.is_some(), "Expected redis store to have hash: {}", VALID_HASH1);

        Ok(())
    }

    #[tokio::test]
    async fn insert_one_item_then_update_and_get() -> Result<(), Error> {
        let data1 = Bytes::from_static(b"13");
        let data2 = Bytes::from_static(b"14");
        let data_vec1 = data1.to_vec();
        let data_vec2 = data2.to_vec();

        let config = config::stores::RedisStore {
            url: "".to_string(),
            use_mock: Some(true),
            mock_commands: Some(vec![
                "SET".to_string(),
                "EXISTS".to_string(),
                "SET".to_string(),
                "GET".to_string(),
            ]),
            mock_data: Some(vec![
                VALID_HASH1.to_string(),
                String::from_utf8(data_vec1).unwrap(),
                VALID_HASH1.to_string(),
                VALID_HASH1.to_string(),
                String::from_utf8(data_vec2).unwrap(),
                VALID_HASH1.to_string(),
            ]),
        };
        let store: RedisStore<MockRedisConnection> = RedisStore::new(config).await?;
        let pinned_store: Pin<&RedisStore<MockRedisConnection>> = Pin::new(&store);

        let digest = DigestInfo::try_new(VALID_HASH1, data2.len() as i64)?;
        pinned_store.update_oneshot(digest, data1).await?;

        let result = pinned_store.has(digest).await?;
        assert!(result.is_some(), "Expected redis store to have hash: {}", VALID_HASH1);

        pinned_store.update_oneshot(digest, data2.clone()).await?;

        let result = pinned_store
            .get_part_unchunked(digest, 0, Some(data2.len()), Some(data2.len()))
            .await?;

        assert_eq!(
            result, data2,
            "Expected redis store to have updated value: {}",
            VALID_HASH1
        );

        Ok(())
    }
}
