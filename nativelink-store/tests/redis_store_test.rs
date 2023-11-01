// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use bytes::Bytes;
use nativelink_error::{Code, Error};
use nativelink_store::redis_store::RedisStore;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::Store;
use redis_test::{MockCmd, MockRedisConnection};
use tokio::sync::Mutex;

const VALID_HASH1: &str = "3031323334353637383961626364656630303030303030303030303030303030";

#[cfg(test)]
mod redis_store_tests {
    use super::*;

    pub async fn new_ext(
        _config: nativelink_config::stores::RedisStore,
        mock_commands: Vec<String>,
        mock_data: Vec<String>,
    ) -> Result<RedisStore<MockRedisConnection>, Error> {
        let mock_commands = mock_commands.clone();
        let mock_data = mock_data.clone();
        let mut mock_cmds = Vec::new();
        let mut data_iter = mock_data.iter();
        for cmd in &mock_commands {
            let mock_cmd = match cmd.as_str() {
                "SET" => {
                    let digest = data_iter.next().ok_or(Error::new(
                        Code::NotFound,
                        "Missing digest for SET".to_string(),
                    ))?;
                    let data = data_iter.next().ok_or(Error::new(
                        Code::NotFound,
                        "Missing data for SET".to_string(),
                    ))?;

                    MockCmd::new(redis::cmd(cmd).arg(digest).arg(data), Ok(1))
                }
                "EXISTS" => {
                    let digest = data_iter.next().ok_or(Error::new(
                        Code::NotFound,
                        "Missing digest for EXISTS".to_string(),
                    ))?;
                    MockCmd::new(redis::cmd(cmd).arg(digest), Ok(1))
                }
                "GET" => {
                    let digest = data_iter.next().ok_or(Error::new(
                        Code::NotFound,
                        "Missing digest for GET".to_string(),
                    ))?;
                    MockCmd::new(redis::cmd(cmd).arg(digest), Ok("14"))
                }
                _ => {
                    return Err(Error::new(
                        Code::NotFound,
                        format!("Unsupported command: {}", cmd),
                    ))
                }
            };
            mock_cmds.push(mock_cmd);
        }

        let connection = Arc::new(Mutex::new(MockRedisConnection::new(mock_cmds)));
        let redis_store = RedisStore { conn: connection };

        Ok(redis_store)
    }

    #[tokio::test]
    async fn verify_redis_connection() -> Result<(), Error> {
        let data = Bytes::from_static(b"13");
        let data_vec = data.to_vec();

        let mock_commands = Some(vec!["SET".to_string(), "EXISTS".to_string()]);
        let mock_data = Some(vec![
            VALID_HASH1.to_string(),
            String::from_utf8(data_vec).unwrap(),
            VALID_HASH1.to_string(),
        ]);

        let config = nativelink_config::stores::RedisStore {
            url: "".to_string(),
        };

        let store: RedisStore<MockRedisConnection>;

        if let (Some(commands), Some(data)) = (mock_commands, mock_data) {
            store = new_ext(config, commands, data).await?;
        } else {
            // Handle the case where either is None
            panic!("Failed test")
        }
        let pinned_store: Pin<&RedisStore<MockRedisConnection>> = Pin::new(&store);

        let digest = DigestInfo::try_new(VALID_HASH1, 1)?;
        pinned_store.update_oneshot(digest, data).await?;

        let result = pinned_store.has(digest).await?;
        assert!(
            result.is_some(),
            "Expected redis store to have hash: {}",
            VALID_HASH1
        );

        Ok(())
    }

    #[tokio::test]
    async fn insert_one_item_then_update_and_get() -> Result<(), Error> {
        let data1 = Bytes::from_static(b"13");
        let data2 = Bytes::from_static(b"14");
        let data_vec1 = data1.to_vec();
        let data_vec2 = data2.to_vec();

        let mock_commands = Some(vec![
            "SET".to_string(),
            "EXISTS".to_string(),
            "SET".to_string(),
            "GET".to_string(),
        ]);
        let mock_data = Some(vec![
            VALID_HASH1.to_string(),
            String::from_utf8(data_vec1).unwrap(),
            VALID_HASH1.to_string(),
            VALID_HASH1.to_string(),
            String::from_utf8(data_vec2).unwrap(),
            VALID_HASH1.to_string(),
        ]);

        let config = nativelink_config::stores::RedisStore {
            url: "".to_string(),
        };
        let store: RedisStore<MockRedisConnection>;

        if let (Some(commands), Some(data)) = (mock_commands, mock_data) {
            store = new_ext(config, commands, data).await?;
        } else {
            panic!("Commands did not match")
        }
        let pinned_store: Pin<&RedisStore<MockRedisConnection>> = Pin::new(&store);

        let digest = DigestInfo::try_new(VALID_HASH1, data2.len() as i64)?;
        pinned_store.update_oneshot(digest, data1).await?;

        let result = pinned_store.has(digest).await?;
        assert!(
            result.is_some(),
            "Expected redis store to have hash: {}",
            VALID_HASH1
        );

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
