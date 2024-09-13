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

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::panicking;

use bytes::{Bytes, BytesMut};
use fred::bytes_utils::string::Str;
use fred::error::RedisError;
use fred::mocks::{MockCommand, Mocks};
use fred::prelude::Builder;
use fred::types::{RedisConfig, RedisValue};
use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_store::cas_utils::ZERO_BYTE_DIGESTS;
use nativelink_store::redis_store::{RedisStore, READ_CHUNK_SIZE};
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::{StoreLike, UploadSizeInfo};
use pretty_assertions::assert_eq;
use tokio::sync::watch;

const VALID_HASH1: &str = "3031323334353637383961626364656630303030303030303030303030303030";
const TEMP_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

fn mock_uuid_generator() -> String {
    uuid::Uuid::parse_str(TEMP_UUID).unwrap().to_string()
}

fn make_temp_key(final_name: &str) -> String {
    format!("temp-{TEMP_UUID}-{{{final_name}}}")
}

#[derive(Debug)]
struct MockRedisBackend {
    /// Commands we expect to encounter, and results we to return to the client.
    // Commands are pushed from the back and popped from the front.
    expected: Mutex<VecDeque<(MockCommand, Result<RedisValue, RedisError>)>>,

    tx: watch::Sender<MockCommand>,
    rx: watch::Receiver<MockCommand>,

    failing: AtomicBool,
}

impl Default for MockRedisBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl MockRedisBackend {
    fn new() -> Self {
        let (tx, rx) = watch::channel(MockCommand {
            cmd: "".into(),
            subcommand: None,
            args: vec![],
        });
        Self {
            expected: Mutex::default(),
            tx,
            rx,
            failing: AtomicBool::new(false),
        }
    }

    fn expect(&self, command: MockCommand, result: Result<RedisValue, RedisError>) -> &Self {
        self.expected.lock().unwrap().push_back((command, result));
        self
    }

    async fn wait_for(&self, command: MockCommand) {
        self.rx
            .clone()
            .wait_for(|cmd| *cmd == command)
            .await
            .expect("the channel isn't closed while the struct exists");
    }
}

impl Mocks for MockRedisBackend {
    fn process_command(&self, actual: MockCommand) -> Result<RedisValue, RedisError> {
        self.tx
            .send(actual.clone())
            .expect("the channel isn't closed while the struct exists");

        let Some((expected, result)) = self.expected.lock().unwrap().pop_front() else {
            // panic here -- this isn't a redis error, it's a test failure
            self.failing.store(true, Ordering::Relaxed);
            panic!("Didn't expect any more commands, but received {actual:?}");
        };

        if actual != expected {
            self.failing.store(true, Ordering::Relaxed);
            assert_eq!(
                actual, expected,
                "mismatched command, received (left) but expected (right)"
            );
        };

        result
    }

    fn process_transaction(&self, commands: Vec<MockCommand>) -> Result<RedisValue, RedisError> {
        static MULTI: MockCommand = MockCommand {
            cmd: Str::from_static("MULTI"),
            subcommand: None,
            args: Vec::new(),
        };
        static EXEC: MockCommand = MockCommand {
            cmd: Str::from_static("EXEC"),
            subcommand: None,
            args: Vec::new(),
        };

        let results = std::iter::once(MULTI.clone())
            .chain(commands)
            .chain([EXEC.clone()])
            .map(|command| self.process_command(command))
            .collect::<Result<Vec<_>, RedisError>>()?;

        Ok(RedisValue::Array(results))
    }
}

impl Drop for MockRedisBackend {
    fn drop(&mut self) {
        if panicking() || self.failing.load(Ordering::Relaxed) {
            // We're already failing, let's make debugging easier and let future devs solve problems one at a time.
            return;
        }

        let expected = self.expected.get_mut().unwrap();

        if expected.is_empty() {
            return;
        }

        assert_eq!(
            *expected,
            VecDeque::new(),
            "Didn't receive all expected commands, expected (left)"
        );

        // Panicking isn't enough inside a tokio task, we need to `exit(1)`
        std::process::exit(1)
    }
}

#[nativelink_test]
async fn upload_and_get_data() -> Result<(), Error> {
    // Construct the data we want to send. Since it's small, we expect it to be sent in a single chunk.
    let data = Bytes::from_static(b"14");
    let chunk_data = RedisValue::Bytes(data.clone());

    // Construct a digest for our data and create a key based on that digest.
    let digest = DigestInfo::try_new(VALID_HASH1, 2)?;
    let packed_hash_hex = format!("{digest}");

    // Construct our Redis store with a mocked out backend.
    let temp_key = RedisValue::Bytes(make_temp_key(&packed_hash_hex).into());
    let real_key = RedisValue::Bytes(packed_hash_hex.into());

    let mocks = Arc::new(MockRedisBackend::new());

    // The first set of commands are for setting the data.
    mocks
        // Append the real value to the temp key.
        .expect(
            MockCommand {
                cmd: Str::from_static("APPEND"),
                subcommand: None,
                args: vec![temp_key.clone(), chunk_data],
            },
            Ok(RedisValue::Array(vec![RedisValue::Null])),
        )
        // Move the data from the fake key to the real key.
        .expect(
            MockCommand {
                cmd: Str::from_static("RENAME"),
                subcommand: None,
                args: vec![temp_key, real_key.clone()],
            },
            Ok(RedisValue::Array(vec![RedisValue::Null])),
        );

    // The second set of commands are for retrieving the data from the key.
    mocks
        // Check that the key exists.
        .expect(
            MockCommand {
                cmd: Str::from_static("STRLEN"),
                subcommand: None,
                args: vec![real_key.clone()],
            },
            Ok(RedisValue::Integer(2)),
        )
        // Retrieve the data from the real key.
        .expect(
            MockCommand {
                cmd: Str::from_static("GETRANGE"),
                subcommand: None,
                args: vec![real_key, RedisValue::Integer(0), RedisValue::Integer(1)],
            },
            Ok(RedisValue::String(Str::from_static("14"))),
        );

    let store = {
        let mut builder = Builder::default_centralized();
        builder.set_config(RedisConfig {
            mocks: Some(Arc::clone(&mocks) as Arc<dyn Mocks>),
            ..Default::default()
        });

        RedisStore::new_from_builder_and_parts(builder, None, mock_uuid_generator, String::new())?
    };

    store.update_oneshot(digest, data.clone()).await.unwrap();

    let result = store.has(digest).await.unwrap();
    assert!(
        result.is_some(),
        "Expected redis store to have hash: {VALID_HASH1}",
    );

    let result = store
        .get_part_unchunked(digest, 0, Some(data.len() as u64))
        .await
        .unwrap();

    assert_eq!(result, data, "Expected redis store to have updated value",);

    Ok(())
}

#[nativelink_test]
async fn upload_and_get_data_with_prefix() -> Result<(), Error> {
    let data = Bytes::from_static(b"14");
    let chunk_data = RedisValue::Bytes(data.clone());

    let prefix = "TEST_PREFIX-";

    let digest = DigestInfo::try_new(VALID_HASH1, 2)?;
    let packed_hash_hex = format!("{prefix}{digest}");

    let temp_key = RedisValue::Bytes(make_temp_key(&packed_hash_hex).into());
    let real_key = RedisValue::Bytes(packed_hash_hex.into());

    let mocks = Arc::new(MockRedisBackend::new());
    mocks
        .expect(
            MockCommand {
                cmd: Str::from_static("APPEND"),
                subcommand: None,
                args: vec![temp_key.clone(), chunk_data],
            },
            Ok(RedisValue::Array(vec![RedisValue::Null])),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("RENAME"),
                subcommand: None,
                args: vec![temp_key, real_key.clone()],
            },
            Ok(RedisValue::Array(vec![RedisValue::Null])),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("STRLEN"),
                subcommand: None,
                args: vec![real_key.clone()],
            },
            Ok(RedisValue::Integer(2)),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("GETRANGE"),
                subcommand: None,
                args: vec![real_key, RedisValue::Integer(0), RedisValue::Integer(1)],
            },
            Ok(RedisValue::String(Str::from_static("14"))),
        );

    let store = {
        let mut builder = Builder::default_centralized();
        builder.set_config(RedisConfig {
            mocks: Some(Arc::clone(&mocks) as Arc<dyn Mocks>),
            ..Default::default()
        });

        RedisStore::new_from_builder_and_parts(
            builder,
            None,
            mock_uuid_generator,
            prefix.to_string(),
        )?
    };

    store.update_oneshot(digest, data.clone()).await.unwrap();

    let result = store.has(digest).await.unwrap();
    assert!(
        result.is_some(),
        "Expected redis store to have hash: {VALID_HASH1}",
    );

    let result = store
        .get_part_unchunked(digest, 0, Some(data.len() as u64))
        .await
        .unwrap();

    assert_eq!(result, data, "Expected redis store to have updated value",);

    Ok(())
}

#[nativelink_test]
async fn upload_empty_data() -> Result<(), Error> {
    let data = Bytes::from_static(b"");
    let digest = ZERO_BYTE_DIGESTS[0];

    // We expect to skip both uploading and downloading when the digest is known zero.
    let store = RedisStore::new_from_builder_and_parts(
        Builder::default_centralized(),
        None,
        mock_uuid_generator,
        String::new(),
    )
    .unwrap();

    store.update_oneshot(digest, data).await.unwrap();

    let result = store.has(digest).await.unwrap();
    assert!(
        result.is_some(),
        "Expected redis store to have hash: {VALID_HASH1}",
    );

    Ok(())
}

#[nativelink_test]
async fn upload_empty_data_with_prefix() -> Result<(), Error> {
    let data = Bytes::from_static(b"");
    let digest = ZERO_BYTE_DIGESTS[0];
    let prefix = "TEST_PREFIX-";

    let store = RedisStore::new_from_builder_and_parts(
        Builder::default_centralized(),
        None,
        mock_uuid_generator,
        prefix.to_string(),
    )
    .unwrap();

    store.update_oneshot(digest, data).await.unwrap();

    let result = store.has(digest).await.unwrap();
    assert!(
        result.is_some(),
        "Expected redis store to have hash: {VALID_HASH1}",
    );

    Ok(())
}

#[nativelink_test]
async fn test_large_downloads_are_chunked() -> Result<(), Error> {
    // Requires multiple chunks as data is larger than 64K.
    let data = Bytes::from(vec![0u8; READ_CHUNK_SIZE + 128]);

    let digest = DigestInfo::try_new(VALID_HASH1, 1)?;
    let packed_hash_hex = format!("{digest}");

    let temp_key = RedisValue::Bytes(make_temp_key(&packed_hash_hex).into());
    let real_key = RedisValue::Bytes(packed_hash_hex.into());

    let mocks = Arc::new(MockRedisBackend::new());

    mocks
        .expect(
            MockCommand {
                cmd: Str::from_static("APPEND"),
                subcommand: None,
                args: vec![temp_key.clone(), data.clone().into()],
            },
            Ok(RedisValue::Array(vec![RedisValue::Null])),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("RENAME"),
                subcommand: None,
                args: vec![temp_key, real_key.clone()],
            },
            Ok(RedisValue::Array(vec![RedisValue::Null])),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("STRLEN"),
                subcommand: None,
                args: vec![real_key.clone()],
            },
            Ok(RedisValue::Integer(data.len().try_into().unwrap())),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("GETRANGE"),
                subcommand: None,
                args: vec![
                    real_key.clone(),
                    RedisValue::Integer(0),
                    // We expect to be asked for data from `0..READ_CHUNK_SIZE`, but since GETRANGE is inclusive
                    // the actual call should be from `0..=(READ_CHUNK_SIZE - 1)`.
                    RedisValue::Integer(READ_CHUNK_SIZE as i64 - 1),
                ],
            },
            Ok(RedisValue::Bytes(data.slice(..READ_CHUNK_SIZE))),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("GETRANGE"),
                subcommand: None,
                args: vec![
                    real_key,
                    RedisValue::Integer(READ_CHUNK_SIZE as i64),
                    // Similar GETRANCE index shenanigans here.
                    RedisValue::Integer(data.len() as i64 - 1),
                ],
            },
            Ok(RedisValue::Bytes(data.slice(READ_CHUNK_SIZE..))),
        );

    let store = {
        let mut builder = Builder::default_centralized();
        builder.set_config(RedisConfig {
            mocks: Some(Arc::clone(&mocks) as Arc<dyn Mocks>),
            ..Default::default()
        });

        RedisStore::new_from_builder_and_parts(builder, None, mock_uuid_generator, String::new())?
    };

    store.update_oneshot(digest, data.clone()).await.unwrap();

    let result = store.has(digest).await.unwrap();
    assert!(
        result.is_some(),
        "Expected redis store to have hash: {VALID_HASH1}",
    );

    let get_result: Bytes = store
        .get_part_unchunked(digest, 0, Some(data.clone().len() as u64))
        .await
        .unwrap();

    assert_eq!(
        get_result,
        data.clone(),
        "Expected redis store to have updated value",
    );

    Ok(())
}

#[nativelink_test]
async fn yield_between_sending_packets_in_update() -> Result<(), Error> {
    let data_p1 = Bytes::from(vec![b'A'; 6 * 1024]);
    let data_p2 = Bytes::from(vec![b'B'; 4 * 1024]);

    let mut data = BytesMut::new();
    data.extend_from_slice(&data_p1);
    data.extend_from_slice(&data_p2);
    let data = data.freeze();

    let digest = DigestInfo::try_new(VALID_HASH1, 2)?;
    let packed_hash_hex = format!("{digest}");

    let temp_key = RedisValue::Bytes(make_temp_key(&packed_hash_hex).into());
    let real_key = RedisValue::Bytes(packed_hash_hex.into());

    let mocks = Arc::new(MockRedisBackend::new());
    let first_append = MockCommand {
        cmd: Str::from_static("APPEND"),
        subcommand: None,
        args: vec![temp_key.clone(), data_p1.clone().into()],
    };

    mocks
        // We expect multiple `"APPEND"`s as we send data in multiple chunks
        .expect(
            first_append.clone(),
            Ok(RedisValue::Array(vec![RedisValue::Null])),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("APPEND"),
                subcommand: None,
                args: vec![temp_key.clone(), data_p2.clone().into()],
            },
            Ok(RedisValue::Array(vec![RedisValue::Null])),
        )
        // The rest of the process looks the same.
        .expect(
            MockCommand {
                cmd: Str::from_static("RENAME"),
                subcommand: None,
                args: vec![temp_key, real_key.clone()],
            },
            Ok(RedisValue::Array(vec![RedisValue::Null])),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("STRLEN"),
                subcommand: None,
                args: vec![real_key.clone()],
            },
            Ok(RedisValue::Integer(2)),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("GETRANGE"),
                subcommand: None,
                args: vec![real_key, RedisValue::Integer(0), RedisValue::Integer(10239)],
            },
            Ok(RedisValue::Bytes(data.clone())),
        );

    let store = {
        let mut builder = Builder::default_centralized();
        builder.set_config(RedisConfig {
            mocks: Some(Arc::clone(&mocks) as Arc<dyn Mocks>),
            ..Default::default()
        });

        RedisStore::new_from_builder_and_parts(builder, None, mock_uuid_generator, String::new())?
    };

    let (mut tx, rx) = make_buf_channel_pair();

    tokio::try_join!(
        async {
            store
                .update(digest, rx, UploadSizeInfo::ExactSize(data.len() as u64))
                .await
                .unwrap();

            Ok::<_, Error>(())
        },
        async {
            tx.send(data_p1).await.unwrap();
            mocks.wait_for(first_append).await;
            tx.send(data_p2).await.unwrap();
            tx.send_eof().unwrap();
            Ok::<_, Error>(())
        },
    )
    .unwrap();

    let result = store.has(digest).await.unwrap();
    assert!(
        result.is_some(),
        "Expected redis store to have hash: {VALID_HASH1}",
    );

    let result = store
        .get_part_unchunked(digest, 0, Some(data.clone().len() as u64))
        .await
        .unwrap();

    assert_eq!(result, data, "Expected redis store to have updated value",);

    Ok(())
}

// Regression test for: https://github.com/TraceMachina/nativelink/issues/1286
#[nativelink_test]
async fn zero_len_items_exist_check() -> Result<(), Error> {
    let mocks = Arc::new(MockRedisBackend::new());

    let digest = DigestInfo::try_new(VALID_HASH1, 0)?;
    let packed_hash_hex = format!("{digest}");
    let real_key = RedisValue::Bytes(packed_hash_hex.into());

    mocks
        .expect(
            MockCommand {
                cmd: Str::from_static("GETRANGE"),
                subcommand: None,
                args: vec![
                    real_key.clone(),
                    RedisValue::Integer(0),
                    // We expect to be asked for data from `0..READ_CHUNK_SIZE`, but since GETRANGE is inclusive
                    // the actual call should be from `0..=(READ_CHUNK_SIZE - 1)`.
                    RedisValue::Integer(READ_CHUNK_SIZE as i64 - 1),
                ],
            },
            Ok(RedisValue::String(Str::from_static(""))),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("EXISTS"),
                subcommand: None,
                args: vec![real_key],
            },
            Ok(RedisValue::Integer(0)),
        );

    let store = {
        let mut builder = Builder::default_centralized();
        builder.set_config(RedisConfig {
            mocks: Some(Arc::clone(&mocks) as Arc<dyn Mocks>),
            ..Default::default()
        });

        RedisStore::new_from_builder_and_parts(builder, None, mock_uuid_generator, String::new())?
    };

    let result = store.get_part_unchunked(digest, 0, None).await;
    assert_eq!(result.unwrap_err().code, Code::NotFound);

    Ok(())
}

// Prevent regressions to https://reviewable.io/reviews/TraceMachina/nativelink/1188#-O2pu9LV5ux4ILuT6MND
#[nativelink_test]
async fn dont_loop_forever_on_empty() -> Result<(), Error> {
    let store = {
        let mut builder = Builder::default_centralized();
        builder.set_config(RedisConfig {
            mocks: Some(Arc::new(MockRedisBackend::new()) as Arc<dyn Mocks>),
            ..Default::default()
        });

        RedisStore::new_from_builder_and_parts(builder, None, mock_uuid_generator, String::new())?
    };

    let digest = DigestInfo::try_new(VALID_HASH1, 2).unwrap();
    let (tx, rx) = make_buf_channel_pair();

    tokio::join!(
        async {
            store
                .update(digest, rx, UploadSizeInfo::MaxSize(0))
                .await
                .unwrap_err();
        },
        async {
            drop(tx);
        },
    );

    Ok(())
}

#[nativelink_test]
fn test_fingerprint_create_index() -> Result<(), Error> {
    let store = {
        let mut builder = Builder::default_centralized();
        builder.set_config(RedisConfig {
            ..Default::default()
        });

        RedisStore::new_from_builder_and_parts(builder, None, mock_uuid_generator, String::new())?
    };

    let expected_fingerprint_create_index =
        calculate_expected_fingerprint_create_index_hardcoded_test();

    let fingerprint = store.get_fingerprint_create_index();
    assert_eq!(expected_fingerprint_create_index, fingerprint);

    Ok(())
}

fn calculate_expected_fingerprint_create_index_hardcoded_test() -> u32 {
    const POLY: u32 = 0xEDB88320;
    const DATA: &[u8] = b"FT.CREATE {} ON HASH PREFIX 1 {} NOOFFSETS NOHL NOFIELDS NOFREQS SCHEMA {} TAG CASESENSITIVE SORTABLE";
    let mut crc = 0xFFFFFFFF;
    let mut i = 0;
    while i < DATA.len() {
        let byte = DATA[i];
        crc ^= byte as u32;

        let mut j = 0;
        while j < 8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ POLY
            } else {
                crc >> 1
            };
            j += 1;
        }
        i += 1;
    }
    crc
}
