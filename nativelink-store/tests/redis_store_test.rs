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

use bytes::Bytes;
use nativelink_error::{Code, Error};
use nativelink_store::redis_store::RedisStore;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::Store;
use redis_test::{MockCmd, MockRedisConnection};

const VALID_HASH1: &str = "3031323334353637383961626364656630303030303030303030303030303030";
const TEMP_UUID: &str = "temp-550e8400-e29b-41d4-a716-446655440000";
const TEMP_UUID1: &str = "temp-550e8400-e29b-41d4-a716-446655440001";

fn mock_uuid_generator() -> uuid::Uuid {
    uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
}
fn mock_uuid_generator1() -> uuid::Uuid {
    uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440001").unwrap()
}

#[cfg(test)]
mod redis_store_tests {
    use super::*;

    pub async fn new_ext(
        _config: nativelink_config::stores::RedisStore,
        mock_commands: Vec<String>,
        mock_data: Vec<String>,
        mock_name_generator: fn() -> uuid::Uuid,
    ) -> Result<RedisStore<MockRedisConnection>, Error> {
        let mock_commands = mock_commands.clone();
        let mock_data = mock_data.clone();
        let mut mock_cmds = Vec::new();
        let mut data_iter = mock_data.iter();
        for cmd in &mock_commands {
            let mock_cmd = match cmd.as_str() {
                "APPEND" => {
                    let digest = data_iter.next().ok_or(Error::new(
                        Code::NotFound,
                        "Missing digest for APPEND".to_string(),
                    ))?;
                    let data = data_iter.next().ok_or(Error::new(
                        Code::NotFound,
                        "Missing data for APPEND".to_string(),
                    ))?;

                    MockCmd::new(redis::cmd(cmd).arg(digest).arg(data), Ok(1))
                }
                "RENAME" => {
                    let old_key = data_iter.next().ok_or(Error::new(
                        Code::NotFound,
                        "Missing digest for RENAME".to_string(),
                    ))?;
                    let new_key = data_iter.next().ok_or(Error::new(
                        Code::NotFound,
                        "Missing data for RENAME".to_string(),
                    ))?;

                    MockCmd::new(redis::cmd(cmd).arg(old_key).arg(new_key), Ok(1))
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
                        format!("Unsupported command: {cmd}"),
                    ))
                }
            };
            mock_cmds.push(mock_cmd);
        }

        let connection = MockRedisConnection::new(mock_cmds);
        let redis_store = RedisStore {
            conn: connection,
            temp_name_generator: mock_name_generator,
        };

        Ok(redis_store)
    }

    #[tokio::test]
    async fn verify_redis_connection() -> Result<(), Error> {
        let data = Bytes::from_static(b"13");
        //let data_vec = data.to_vec();

        let chunk_data = "13".to_string();
        let end_chunk_data = "".to_string();

        let mock_commands = Some(vec![
            "APPEND".to_string(),
            "APPEND".to_string(),
            "RENAME".to_string(),
            "EXISTS".to_string(),
        ]);
        let mock_data = Some(vec![
            TEMP_UUID.to_string(),
            chunk_data,
            TEMP_UUID.to_string(),
            end_chunk_data,
            TEMP_UUID.to_string(),
            VALID_HASH1.to_string(),
            VALID_HASH1.to_string(),
        ]);

        let config = nativelink_config::stores::RedisStore {
            address: vec!["".to_string()],
            response_timeout: 5,
            connection_timeout: 5,
        };

        let store: RedisStore<MockRedisConnection>;

        if let (Some(commands), Some(data)) = (mock_commands, mock_data) {
            store = new_ext(config, commands, data, mock_uuid_generator).await?;
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
            "Expected redis store to have hash: {VALID_HASH1}",
        );

        Ok(())
    }

    #[tokio::test]
    async fn multiple_clients_can_upload_same_data_in_tandem() -> Result<(), Error> {
        let data1 = Bytes::from_static(b"13");
        let data2 = Bytes::from_static(b"14");
        //let data3 = Bytes::from_static(b"15");
        //let data_vec1 = data1.to_vec();
        //let data_vec2 = data2.to_vec();

        let chunk_data = "13".to_string();
        let update_chunk_data = "14".to_string();
        let end_chunk_data = "".to_string();

        let mock_commands = Some(vec![
            "APPEND".to_string(),
            "APPEND".to_string(),
            "RENAME".to_string(),
            "EXISTS".to_string(),
            "APPEND".to_string(),
            "APPEND".to_string(),
            "RENAME".to_string(),
            "GET".to_string(),
        ]);
        let mock_data = Some(vec![
            TEMP_UUID.to_string(),
            chunk_data.clone(),
            TEMP_UUID.to_string(),
            end_chunk_data.clone(),
            TEMP_UUID.to_string(),
            VALID_HASH1.to_string(),
            VALID_HASH1.to_string(),
            TEMP_UUID.to_string(),
            update_chunk_data.clone(),
            TEMP_UUID.to_string(),
            end_chunk_data.clone(),
            TEMP_UUID.to_string(),
            VALID_HASH1.to_string(),
            VALID_HASH1.to_string(),
        ]);

        let mock_data1 = Some(vec![
            TEMP_UUID1.to_string(),
            chunk_data,
            TEMP_UUID1.to_string(),
            end_chunk_data.clone(),
            TEMP_UUID1.to_string(),
            VALID_HASH1.to_string(),
            VALID_HASH1.to_string(),
            TEMP_UUID1.to_string(),
            update_chunk_data,
            TEMP_UUID1.to_string(),
            end_chunk_data.clone(),
            TEMP_UUID1.to_string(),
            VALID_HASH1.to_string(),
            VALID_HASH1.to_string(),
        ]);

        let config = nativelink_config::stores::RedisStore {
            address: vec!["".to_string()],
            response_timeout: 5,
            connection_timeout: 5,
        };
        let store: RedisStore<MockRedisConnection>;
        let store1: RedisStore<MockRedisConnection>;

        if let (Some(commands), Some(data)) = (mock_commands.clone(), mock_data.clone()) {
            store = new_ext(config.clone(), commands, data, mock_uuid_generator).await?;
        } else {
            panic!("Commands did not match")
        }

        if let (Some(commands), Some(data)) = (mock_commands, mock_data1) {
            store1 = new_ext(config, commands, data, mock_uuid_generator1).await?;
        } else {
            panic!("Commands did not match")
        }

        let pinned_store: Pin<&RedisStore<MockRedisConnection>> = Pin::new(&store);
        let pinned_store1: Pin<&RedisStore<MockRedisConnection>> = Pin::new(&store1);

        let digest = DigestInfo::try_new(VALID_HASH1, data2.len() as i64)?;
        pinned_store.update_oneshot(digest, data1.clone()).await?;
        pinned_store1.update_oneshot(digest, data1).await?;

        let result = pinned_store.has(digest).await?;
        assert!(
            result.is_some(),
            "Expected redis store to have hash: {VALID_HASH1}"
        );

        let result1 = pinned_store1.has(digest).await?;
        assert!(
            result1.is_some(),
            "Expected redis store to have hash: {VALID_HASH1}"
        );

        pinned_store.update_oneshot(digest, data2.clone()).await?;
        pinned_store1.update_oneshot(digest, data2.clone()).await?;

        let result = pinned_store
            .get_part_unchunked(digest, 0, Some(data2.len()), Some(data2.len()))
            .await?;

        println!("Result: {result:?}");
        assert_eq!(
            result, data2,
            "Expected redis store to have updated value: {VALID_HASH1}",
        );

        let result1 = pinned_store1
            .get_part_unchunked(digest, 0, Some(data2.len()), Some(data2.len()))
            .await?;
        assert_eq!(
            result1, data2,
            "Expected redis store to have updated value: {VALID_HASH1}",
        );

        Ok(())
    }
}
