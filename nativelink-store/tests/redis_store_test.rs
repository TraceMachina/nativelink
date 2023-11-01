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

fn mock_uuid_generator() -> String {
    uuid::Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000")
        .unwrap()
        .to_string()
}

#[cfg(test)]
mod redis_store_tests {
    use super::*;

    pub async fn new_ext(
        _config: nativelink_config::stores::RedisStore,
        mock_commands: Vec<String>,
        mock_data: Vec<String>,
        mock_name_generator: fn() -> String,
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
                "STRLEN" => {
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
                    let expected_output = data_iter.next().ok_or(Error::new(
                        Code::NotFound,
                        "Missing output for GET".to_string(),
                    ))?;
                    MockCmd::new(redis::cmd(cmd).arg(digest), Ok(expected_output.clone()))
                }
                "GETRANGE" => {
                    let digest = data_iter.next().ok_or(Error::new(
                        Code::NotFound,
                        "Missing digest for GETRANGE".to_string(),
                    ))?;
                    let start = data_iter.next().ok_or(Error::new(
                        Code::NotFound,
                        "Missing start range for GETRANGE".to_string(),
                    ))?;
                    let end = data_iter.next().ok_or(Error::new(
                        Code::NotFound,
                        "Missing end range for GETRANGE".to_string(),
                    ))?;
                    let expected_output = data_iter.next().ok_or(Error::new(
                        Code::NotFound,
                        "Missing output for GETRANGE".to_string(),
                    ))?;
                    MockCmd::new(
                        redis::cmd(cmd).arg(digest).arg(start).arg(end),
                        Ok(expected_output.clone()),
                    )
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
    async fn upload_and_get_data() -> Result<(), Error> {
        let data = Bytes::from_static(b"14");

        let digest = DigestInfo::try_new(VALID_HASH1, 1)?;
        let packed_hash_ref = &digest.packed_hash;
        let packed_hash_hex = hex::encode(packed_hash_ref);

        let chunk_data = "14".to_string();

        let mock_commands = Some(vec![
            "APPEND".to_string(),
            "RENAME".to_string(),
            "EXISTS".to_string(),
            "STRLEN".to_string(),
            "GETRANGE".to_string(),
        ]);
        let mock_data = Some(vec![
            TEMP_UUID.to_string(),
            chunk_data.clone(),
            TEMP_UUID.to_string(),
            packed_hash_hex.clone(),
            packed_hash_hex.clone(),
            packed_hash_hex.clone(),
            VALID_HASH1.to_string(),
            0.to_string(),
            2.to_string(),
            chunk_data.clone(),
        ]);

        let config = nativelink_config::stores::RedisStore {
            addresses: vec!["".to_string()],
            response_timeout: 5,
            connection_timeout: 5,
        };

        let store: RedisStore<MockRedisConnection>;

        if let (Some(commands), Some(data)) = (mock_commands, mock_data) {
            store = new_ext(config, commands, data, mock_uuid_generator).await?;
        } else {
            panic!("Failed test")
        }
        let pinned_store: Pin<&RedisStore<MockRedisConnection>> = Pin::new(&store);

        pinned_store.update_oneshot(digest, data.clone()).await?;

        let result = pinned_store.has(digest).await?;
        assert!(
            result.is_some(),
            "Expected redis store to have hash: {VALID_HASH1}",
        );

        let result = pinned_store
            .get_part_unchunked(
                digest,
                0,
                Some(data.clone().len()),
                Some(data.clone().len()),
            )
            .await?;

        assert_eq!(result, data, "Expected redis store to have updated value",);

        Ok(())
    }

    #[tokio::test]
    async fn upload_empty_data() -> Result<(), Error> {
        let data = Bytes::from_static(b"");

        let digest = DigestInfo::try_new(VALID_HASH1, 1)?;
        let packed_hash_ref = &digest.packed_hash;
        let packed_hash_hex = hex::encode(packed_hash_ref);

        let mock_commands = Some(vec![
            "RENAME".to_string(),
            "EXISTS".to_string(),
            "STRLEN".to_string(),
        ]);
        let mock_data = Some(vec![
            TEMP_UUID.to_string(),
            packed_hash_hex.clone(),
            packed_hash_hex.clone(),
            packed_hash_hex,
        ]);

        let config = nativelink_config::stores::RedisStore {
            addresses: vec!["".to_string()],
            response_timeout: 5,
            connection_timeout: 5,
        };

        let store: RedisStore<MockRedisConnection>;

        if let (Some(commands), Some(data)) = (mock_commands, mock_data) {
            store = new_ext(config, commands, data, mock_uuid_generator).await?;
        } else {
            panic!("Failed test")
        }
        let pinned_store: Pin<&RedisStore<MockRedisConnection>> = Pin::new(&store);

        pinned_store.update_oneshot(digest, data).await?;

        let result = pinned_store.has(digest).await?;
        assert!(
            result.is_some(),
            "Expected redis store to have hash: {VALID_HASH1}",
        );

        Ok(())
    }

    // Test uploading large data that requires multiple chunks
    #[tokio::test]
    async fn test_uploading_large_data() -> Result<(), Error> {
        let data: Bytes = Bytes::from(vec![0u8; 65 * 1024]);

        let digest = DigestInfo::try_new(VALID_HASH1, 1)?;
        let packed_hash_ref = &digest.packed_hash;
        let packed_hash_hex = hex::encode(packed_hash_ref);

        let chunk_data = std::str::from_utf8(&data).unwrap().to_string();

        let mock_commands = Some(vec![
            "APPEND".to_string(),
            "RENAME".to_string(),
            "EXISTS".to_string(),
            "STRLEN".to_string(),
            "GETRANGE".to_string(),
            "GETRANGE".to_string(),
        ]);
        let mock_data = Some(vec![
            TEMP_UUID.to_string(),
            chunk_data.clone(),
            TEMP_UUID.to_string(),
            packed_hash_hex.clone(),
            packed_hash_hex.clone(),
            packed_hash_hex.clone(),
            VALID_HASH1.to_string(),
            0.to_string(),
            65535.to_string(),
            hex::encode(&data[..]),
            VALID_HASH1.to_string(),
            65535.to_string(),
            66560.to_string(),
            hex::encode(&data[..]),
        ]);

        let config = nativelink_config::stores::RedisStore {
            addresses: vec!["".to_string()],
            response_timeout: 5,
            connection_timeout: 5,
        };

        let store: RedisStore<MockRedisConnection>;

        if let (Some(commands), Some(data)) = (mock_commands, mock_data) {
            store = new_ext(config, commands, data.clone(), mock_uuid_generator).await?;
        } else {
            panic!("Failed test")
        }
        let pinned_store: Pin<&RedisStore<MockRedisConnection>> = Pin::new(&store);

        pinned_store.update_oneshot(digest, data.clone()).await?;

        let result = pinned_store.has(digest).await?;
        assert!(
            result.is_some(),
            "Expected redis store to have hash: {VALID_HASH1}",
        );

        let get_result: Bytes = pinned_store
            .get_part_unchunked(
                digest,
                0,
                Some(data.clone().len()),
                Some(data.clone().len()),
            )
            .await?;

        assert_eq!(
            hex::encode(get_result).len(),
            hex::encode(data.clone()).len(),
            "Expected redis store to have updated value",
        );

        Ok(())
    }
}
