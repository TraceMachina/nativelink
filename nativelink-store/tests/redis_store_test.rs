// Copyright 2024-2025 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::fmt::Write;
use core::ops::RangeBounds;
use std::collections::HashMap;

use bytes::{Bytes, BytesMut};
use futures::TryStreamExt;
use nativelink_config::stores::{RedisMode, RedisSpec};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_store::cas_utils::ZERO_BYTE_DIGESTS;
use nativelink_store::redis_store::{
    DEFAULT_MAX_CHUNK_UPLOADS_PER_UPDATE, DEFAULT_MAX_COUNT_PER_CURSOR, LUA_VERSION_SET_SCRIPT,
    RedisStore,
};
use nativelink_util::background_spawn;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::HealthStatus;
use nativelink_util::store_trait::{
    SchedulerIndexProvider, SchedulerStore, SchedulerStoreDecodeTo, SchedulerStoreKeyProvider,
    StoreKey, StoreLike, TrueValue, UploadSizeInfo,
};
use pretty_assertions::assert_eq;
use redis::{RedisError, Value};
use redis_test::{IntoRedisValue, MockCmd, MockRedisConnection};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc::unbounded_channel;
use tracing::{Instrument, error, info, info_span, warn};

const VALID_HASH1: &str = "3031323334353637383961626364656630303030303030303030303030303030";
const TEMP_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";

const DEFAULT_READ_CHUNK_SIZE: usize = 1024;
const DEFAULT_SCAN_COUNT: usize = 10_000;
const DEFAULT_MAX_PERMITS: usize = 100;

fn mock_uuid_generator() -> String {
    uuid::Uuid::parse_str(TEMP_UUID).unwrap().to_string()
}

fn make_temp_key(final_name: &str) -> String {
    format!("temp-{TEMP_UUID}-{{{final_name}}}")
}

async fn make_mock_store(commands: Vec<MockCmd>) -> RedisStore<MockRedisConnection> {
    make_mock_store_with_prefix(commands, String::new()).await
}

async fn make_mock_store_with_prefix(
    mut commands: Vec<MockCmd>,
    key_prefix: String,
) -> RedisStore<MockRedisConnection> {
    let (_tx, subscriber_channel) = unbounded_channel();
    commands.insert(
        0,
        MockCmd::new(
            redis::cmd("SCRIPT").arg("LOAD").arg(LUA_VERSION_SET_SCRIPT),
            Ok("b22b9926cbce9dd9ba97fa7ba3626f89feea1ed5"),
        ),
    );
    let mock_connection = MockRedisConnection::new(commands);
    RedisStore::new_from_builder_and_parts(
        mock_connection,
        None,
        mock_uuid_generator,
        key_prefix,
        DEFAULT_READ_CHUNK_SIZE,
        DEFAULT_MAX_CHUNK_UPLOADS_PER_UPDATE,
        DEFAULT_SCAN_COUNT,
        DEFAULT_MAX_PERMITS,
        DEFAULT_MAX_COUNT_PER_CURSOR,
        Some(subscriber_channel),
    )
    .await
    .unwrap()
}

#[nativelink_test]
async fn upload_and_get_data() -> Result<(), Error> {
    // Construct the data we want to send. Since it's small, we expect it to be sent in a single chunk.
    let data = Bytes::from_static(b"14");

    // Construct a digest for our data and create a key based on that digest.
    let digest = DigestInfo::try_new(VALID_HASH1, 2)?;
    let packed_hash_hex = format!("{digest}");

    // Construct our Redis store with a mocked out backend.
    let temp_key = make_temp_key(&packed_hash_hex);
    let real_key = packed_hash_hex;

    let commands = vec![
        // The first set of commands are for setting the data.
        // Append the real value to the temp key.
        MockCmd::new(
            redis::cmd("SETRANGE")
                .arg(temp_key.clone())
                .arg(0)
                .arg(data.to_vec()),
            Ok(Value::Int(0)),
        ),
        MockCmd::new(
            redis::cmd("STRLEN").arg(temp_key.clone()),
            Ok(Value::Int(data.len() as i64)),
        ),
        // Move the data from the fake key to the real key.
        MockCmd::new(
            redis::cmd("RENAME")
                .arg(temp_key.clone())
                .arg(real_key.clone()),
            Ok(Value::Nil),
        ),
        // The second set of commands are for retrieving the data from the key.
        // Check that the key exists.
        MockCmd::with_values(
            redis::pipe()
                .cmd("STRLEN")
                .arg(real_key.clone())
                .cmd("EXISTS")
                .arg(real_key.clone()),
            Ok(vec![Value::Int(2), Value::Boolean(true)]),
        ),
        // Retrieve the data from the real key.
        MockCmd::new(
            redis::cmd("GETRANGE").arg(real_key).arg(0).arg(1),
            Ok(Value::BulkString(b"14".to_vec())),
        ),
    ];

    let store = make_mock_store(commands).await;

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

    let prefix = "TEST_PREFIX-";

    let digest = DigestInfo::try_new(VALID_HASH1, 2)?;
    let packed_hash_hex = format!("{prefix}{digest}");

    let temp_key = make_temp_key(&packed_hash_hex);
    let real_key = packed_hash_hex;

    let commands = vec![
        MockCmd::new(
            redis::cmd("SETRANGE")
                .arg(temp_key.clone())
                .arg(0)
                .arg(data.clone().to_vec()),
            Ok(Value::Int(0)),
        ),
        MockCmd::new(
            redis::cmd("STRLEN").arg(temp_key.clone()),
            Ok(Value::Int(data.len() as i64)),
        ),
        MockCmd::new(
            redis::cmd("RENAME").arg(temp_key).arg(real_key.clone()),
            Ok(Value::Nil),
        ),
        MockCmd::with_values(
            redis::pipe()
                .cmd("STRLEN")
                .arg(real_key.clone())
                .cmd("EXISTS")
                .arg(real_key.clone()),
            Ok(vec![Value::Int(2), Value::Boolean(true)]),
        ),
        MockCmd::new(
            redis::cmd("GETRANGE").arg(real_key).arg(0).arg(1),
            Ok(Value::BulkString(b"14".to_vec())),
        ),
    ];

    let store = make_mock_store_with_prefix(commands, prefix.to_string()).await;

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

    let commands = vec![];
    let store = make_mock_store(commands).await;
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

    let commands = vec![];
    let store = make_mock_store_with_prefix(commands, prefix.to_string()).await;
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
    const READ_CHUNK_SIZE: usize = 1024;
    let data = Bytes::from(vec![0u8; READ_CHUNK_SIZE + 128]);

    let digest = DigestInfo::try_new(VALID_HASH1, 1)?;
    let packed_hash_hex = format!("{digest}");

    let temp_key = make_temp_key(&packed_hash_hex);
    let real_key = packed_hash_hex;

    let commands = vec![
        MockCmd::new(
            redis::cmd("SETRANGE")
                .arg(temp_key.clone())
                .arg(0)
                .arg(data.clone().to_vec()),
            Ok(Value::Int(0)),
        ),
        MockCmd::new(
            redis::cmd("STRLEN").arg(temp_key.clone()),
            Ok(Value::Int(data.len() as i64)),
        ),
        MockCmd::new(
            redis::cmd("RENAME").arg(temp_key).arg(real_key.clone()),
            Ok(Value::Nil),
        ),
        MockCmd::with_values(
            redis::pipe()
                .cmd("STRLEN")
                .arg(real_key.clone())
                .cmd("EXISTS")
                .arg(real_key.clone()),
            Ok(vec![
                Value::Int(data.len().try_into().unwrap()),
                Value::Int(1),
            ]),
        ),
        MockCmd::new(
            // We expect to be asked for data from `0..READ_CHUNK_SIZE`, but since GETRANGE is inclusive
            // the actual call should be from `0..=(READ_CHUNK_SIZE - 1)`.
            redis::cmd("GETRANGE")
                .arg(real_key.clone())
                .arg(0)
                .arg(READ_CHUNK_SIZE as i64 - 1),
            Ok(Value::BulkString(data.slice(..READ_CHUNK_SIZE).into())),
        ),
        MockCmd::new(
            // Similar GETRANGE index shenanigans here.
            redis::cmd("GETRANGE")
                .arg(real_key)
                .arg(READ_CHUNK_SIZE as i64)
                .arg(data.len() as i64 - 1),
            Ok(Value::BulkString(data.slice(READ_CHUNK_SIZE..).into())),
        ),
    ];

    let store = make_mock_store(commands).await;

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
        get_result, data,
        "Expected redis store to have updated value",
    );

    Ok(())
}

#[nativelink_test]
async fn yield_between_sending_packets_in_update() -> Result<(), Error> {
    let data_p1 = Bytes::from(vec![b'A'; DEFAULT_READ_CHUNK_SIZE + 512]);
    let data_p2 = Bytes::from(vec![b'B'; DEFAULT_READ_CHUNK_SIZE]);

    let mut data = BytesMut::new();
    data.extend_from_slice(&data_p1);
    data.extend_from_slice(&data_p2);
    let data = data.freeze();

    let digest = DigestInfo::try_new(VALID_HASH1, 2)?;
    let packed_hash_hex = format!("{digest}");

    let temp_key = make_temp_key(&packed_hash_hex);
    let real_key = packed_hash_hex;

    let commands = vec![
        // We expect multiple `"SETRANGE"`s as we send data in multiple chunks
        MockCmd::new(
            redis::cmd("SETRANGE")
                .arg(temp_key.clone())
                .arg(0)
                .arg(data_p1.clone().to_vec()),
            Ok(Value::Int(0)),
        ),
        MockCmd::new(
            redis::cmd("SETRANGE")
                .arg(temp_key.clone())
                .arg(data_p1.len())
                .arg(data_p2.clone().to_vec()),
            Ok(Value::Int(0)),
        ),
        MockCmd::new(
            redis::cmd("STRLEN").arg(temp_key.clone()),
            Ok(Value::Int(data.len() as i64)),
        ),
        MockCmd::new(
            redis::cmd("RENAME").arg(temp_key).arg(real_key.clone()),
            Ok(Value::Nil),
        ),
        MockCmd::with_values(
            redis::pipe()
                .cmd("STRLEN")
                .arg(real_key.clone())
                .cmd("EXISTS")
                .arg(real_key.clone()),
            Ok(vec![Value::Int(2), Value::Int(1)]),
        ),
        MockCmd::new(
            redis::cmd("GETRANGE")
                .arg(real_key.clone())
                .arg(0)
                .arg((DEFAULT_READ_CHUNK_SIZE - 1) as i64),
            Ok(Value::BulkString(data.clone().to_vec())),
        ),
        MockCmd::new(
            redis::cmd("GETRANGE")
                .arg(real_key.clone())
                .arg(DEFAULT_READ_CHUNK_SIZE as i64)
                .arg((DEFAULT_READ_CHUNK_SIZE * 2 - 1) as i64),
            Ok(Value::BulkString(data.clone().to_vec())),
        ),
        MockCmd::new(
            redis::cmd("GETRANGE")
                .arg(real_key)
                .arg((DEFAULT_READ_CHUNK_SIZE * 2) as i64)
                .arg((data_p1.len() + data_p2.len() - 1) as i64),
            Ok(Value::BulkString(data.clone().to_vec())),
        ),
    ];

    let store = make_mock_store(commands).await;

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
    let digest = DigestInfo::try_new(VALID_HASH1, 0)?;
    let packed_hash_hex = format!("{digest}");
    let real_key = packed_hash_hex;

    let commands = vec![
        MockCmd::new(
            redis::cmd("GETRANGE")
                .arg(real_key.clone())
                .arg(0)
                .arg(DEFAULT_READ_CHUNK_SIZE as i64 - 1),
            Ok(Value::BulkString(vec![])),
        ),
        MockCmd::new(redis::cmd("EXISTS").arg(real_key), Ok(Value::Int(0))),
    ];

    let store = make_mock_store(commands).await;

    let result = store.get_part_unchunked(digest, 0, None).await;
    assert_eq!(
        result.as_ref().unwrap_err().code,
        Code::NotFound,
        "{result:?}"
    );

    Ok(())
}

#[nativelink_test]
async fn list_test() -> Result<(), Error> {
    async fn get_list(
        store: &RedisStore<MockRedisConnection>,
        range: impl RangeBounds<StoreKey<'static>> + Send + Sync + 'static,
    ) -> Vec<StoreKey<'static>> {
        let mut found_keys = vec![];
        store
            .list(range, |key| {
                found_keys.push(key.borrow().into_owned());
                true
            })
            .await
            .unwrap();
        found_keys
    }

    const KEY1: StoreKey = StoreKey::new_str("key1");
    const KEY2: StoreKey = StoreKey::new_str("key2");
    const KEY3: StoreKey = StoreKey::new_str("key3");

    #[allow(clippy::unnecessary_wraps)] // because that's what MockCmd wants
    fn result() -> Result<Value, RedisError> {
        Ok(Value::Array(vec![
            Value::BulkString(b"key1".to_vec()),
            Value::BulkString(b"key2".to_vec()),
            Value::BulkString(b"key3".to_vec()),
        ]))
    }

    fn command() -> MockCmd {
        MockCmd::new(
            redis::cmd("SCAN")
                .arg("0")
                .arg("MATCH")
                .arg("key*")
                .arg("COUNT")
                .arg(10000),
            result(),
        )
    }
    fn command_open() -> MockCmd {
        MockCmd::new(
            redis::cmd("SCAN")
                .arg("0")
                .arg("MATCH")
                .arg("*")
                .arg("COUNT")
                .arg(10000),
            result(),
        )
    }

    let commands = vec![
        command_open(),
        command_open(),
        command(),
        command(),
        command(),
        command_open(),
        command(),
        command(),
    ];

    let store = make_mock_store(commands).await;

    info!("Test listing all keys");
    let keys = get_list(&store, ..).await;
    assert_eq!(keys, vec![KEY1, KEY2, KEY3]);

    info!("Test listing from key1 to all");
    let keys = get_list(&store, KEY1..).await;
    assert_eq!(keys, vec![KEY1, KEY2, KEY3]);

    info!("Test listing from key1 to key2");
    let keys = get_list(&store, KEY1..KEY2).await;
    assert_eq!(keys, vec![KEY1]);

    info!("Test listing from key1 including key2");
    let keys = get_list(&store, KEY1..=KEY2).await;
    assert_eq!(keys, vec![KEY1, KEY2]);

    info!("Test listing from key1 to key3");
    let keys = get_list(&store, KEY1..KEY3).await;
    assert_eq!(keys, vec![KEY1, KEY2]);

    info!("Test listing from all to key2");
    let keys = get_list(&store, ..KEY2).await;
    assert_eq!(keys, vec![KEY1]);

    info!("Test listing from key2 to key3");
    let keys = get_list(&store, KEY2..KEY3).await;
    assert_eq!(keys, vec![KEY2]);

    info!("Test listing with reversed bounds");
    let keys = get_list(&store, KEY3..=KEY1).await;
    assert_eq!(keys, vec![]);

    Ok(())
}

// Prevent regressions to https://reviewable.io/reviews/TraceMachina/nativelink/1188#-O2pu9LV5ux4ILuT6MND
#[nativelink_test]
async fn dont_loop_forever_on_empty() -> Result<(), Error> {
    let commands = vec![];
    let store = make_mock_store(commands).await;
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
fn test_connection_errors() {
    // name is resolvable, but not connectable
    let spec = RedisSpec {
        addresses: vec!["redis://nativelink.com:6379/".to_string()],
        connection_timeout_ms: 1000,
        ..Default::default()
    };
    let err = RedisStore::new_standard(spec)
        .await
        .expect_err("Shouldn't have connected");
    assert_eq!(
        Error {
            code: Code::DeadlineExceeded,
            messages: vec![
                "Io: timed out".into(),
                format!("While connecting to redis with url: redis://nativelink.com:6379/")
            ]
        },
        err
    );
}

fn cmd_as_string(cmd: &redis::Cmd) -> String {
    let raw = cmd.get_packed_command();
    String::from_utf8(raw).unwrap()
}

fn arg_as_string(output: &mut String, arg: Value) {
    match arg {
        Value::SimpleString(s) => {
            write!(output, "+{s}\r\n").unwrap();
        }
        Value::Okay => {
            write!(output, "+OK\r\n").unwrap();
        }
        Value::BulkString(s) => {
            write!(
                output,
                "${}\r\n{}\r\n",
                s.len(),
                str::from_utf8(&s).unwrap()
            )
            .unwrap();
        }
        Value::Int(v) => {
            write!(output, ":{v}\r\n").unwrap();
        }
        Value::Array(values) => {
            write!(output, "*{}\r\n", values.len()).unwrap();
            for value in values {
                arg_as_string(output, value);
            }
        }
        Value::Map(values) => {
            write!(output, "%{}\r\n", values.len()).unwrap();
            for (key, value) in values {
                arg_as_string(output, key);
                arg_as_string(output, value);
            }
        }
        _ => {
            panic!("No support for {arg:?}")
        }
    }
}

fn args_as_string(args: Vec<Value>) -> String {
    let mut output = String::new();
    for arg in args {
        arg_as_string(&mut output, arg);
    }
    output
}

fn add_to_response(response: &mut HashMap<String, String>, cmd: &redis::Cmd, args: Vec<Value>) {
    response.insert(cmd_as_string(cmd), args_as_string(args));
}

fn setinfo(responses: &mut HashMap<String, String>) {
    // Library sends both lib-name and lib-ver in one go, so we respond to both
    add_to_response(
        responses,
        redis::cmd("CLIENT")
            .arg("SETINFO")
            .arg("LIB-NAME")
            .arg("redis-rs"),
        vec![Value::Okay, Value::Okay],
    );
}

fn fake_redis_stream() -> HashMap<String, String> {
    let mut responses = HashMap::new();
    setinfo(&mut responses);
    add_to_response(
        &mut responses,
        redis::cmd("SCRIPT").arg("LOAD").arg(LUA_VERSION_SET_SCRIPT),
        vec!["b22b9926cbce9dd9ba97fa7ba3626f89feea1ed5".into_redis_value()],
    );
    // Does setinfo as well, so need to respond to all 3
    add_to_response(
        &mut responses,
        redis::cmd("SELECT").arg("3"),
        vec![Value::Okay, Value::Okay, Value::Okay],
    );
    responses
}

fn fake_redis_sentinel_master_stream() -> HashMap<String, String> {
    let mut response = fake_redis_stream();
    add_to_response(
        &mut response,
        &redis::cmd("ROLE"),
        vec![Value::Array(vec![
            "master".into_redis_value(),
            0.into_redis_value(),
            Value::Array(vec![]),
        ])],
    );
    response
}

fn fake_redis_sentinel_stream(master_name: &str, redis_port: u16) -> HashMap<String, String> {
    let mut response = HashMap::new();
    setinfo(&mut response);

    // Not a full "sentinel masters" response, but enough for redis-rs
    let resp: Vec<(Value, Value)> = vec![
        ("name".into_redis_value(), master_name.into_redis_value()),
        ("ip".into_redis_value(), "127.0.0.1".into_redis_value()),
        (
            "port".into_redis_value(),
            i64::from(redis_port).into_redis_value(),
        ),
        ("flags".into_redis_value(), "master".into_redis_value()),
    ];

    add_to_response(
        &mut response,
        redis::cmd("SENTINEL").arg("MASTERS"),
        vec![Value::Array(vec![Value::Map(resp)])],
    );
    response
}

async fn fake_redis(listener: TcpListener, responses: HashMap<String, String>) {
    info!("Responses are: {:?}", responses);
    loop {
        info!(
            "Waiting for connection on {}",
            listener.local_addr().unwrap()
        );
        let Ok((mut stream, _)) = listener.accept().await else {
            error!("accept error");
            panic!("error");
        };
        info!("Accepted new connection");
        let values = responses.clone();
        background_spawn!("thread", async move {
            loop {
                let mut buf = vec![0; 8192];
                let res = stream.read(&mut buf).await.unwrap();
                if res != 0 {
                    let str_buf = str::from_utf8(&buf[..res]);
                    if let Ok(s) = str_buf {
                        let mut matched = false;
                        for (key, value) in &values {
                            if s.starts_with(key) {
                                info!("Responding to {}", s.replace("\r\n", "\\r\\n"));
                                stream.write_all(value.as_bytes()).await.unwrap();
                                matched = true;
                                break;
                            }
                        }
                        if !matched {
                            warn!("Unknown command: {s}");
                        }
                    } else {
                        warn!("Bytes buffer: {buf:?}");
                    }
                }
            }
        });
    }
}

async fn make_fake_redis_with_responses(responses: HashMap<String, String>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    info!("Using port {port}");

    background_spawn!("listener", async move {
        fake_redis(listener, responses).await;
    });

    port
}

async fn make_fake_redis() -> u16 {
    make_fake_redis_with_responses(fake_redis_stream()).await
}

#[nativelink_test]
async fn test_health() {
    let port = make_fake_redis().await;
    let spec = RedisSpec {
        addresses: vec![format!("redis://127.0.0.1:{port}/")],
        command_timeout_ms: 1000,
        ..Default::default()
    };
    let store = RedisStore::new_standard(spec).await.expect("Working spec");
    match store.check_health(std::borrow::Cow::Borrowed("foo")).await {
        HealthStatus::Ok {
            struct_name: _,
            message: _,
        } => {
            panic!("Expected failure");
        }
        HealthStatus::Failed {
            struct_name,
            message,
        } => {
            assert_eq!(
                struct_name,
                "nativelink_store::redis_store::RedisStore<redis::aio::connection_manager::ConnectionManager>"
            );
            assert!(
                message.starts_with("Store.update_oneshot() failed: Error { code: DeadlineExceeded, messages: [\"Io: timed out\", \"While appending to temp key ("),
                "message: '{message}'"
            );
            logs_assert(|logs| {
                for log in logs {
                    if log.contains("check_health Store.update_oneshot() failed e=Error { code: DeadlineExceeded, messages: [\"Io: timed out\", \"While appending to temp key (") {
                        return Ok(())
                    }
                }
                Err(format!("No check_health log! {logs:?}"))
            });
        }
        health_result => {
            panic!("Other result: {health_result:?}");
        }
    }
}

#[nativelink_test]
async fn test_deprecated_broadcast_channel_capacity() {
    let port = make_fake_redis().await;
    let spec = RedisSpec {
        addresses: vec![format!("redis://127.0.0.1:{port}/")],
        broadcast_channel_capacity: 1,
        ..Default::default()
    };
    RedisStore::new_standard(spec).await.expect("Working spec");

    assert!(logs_contain(
        "broadcast_channel_capacity in Redis spec is deprecated and ignored"
    ));
}

#[nativelink_test]
async fn test_sentinel_connect() {
    let redis_span = info_span!("redis");
    let redis_port = make_fake_redis_with_responses(fake_redis_sentinel_master_stream())
        .instrument(redis_span)
        .await;
    let sentinel_span = info_span!("sentinel");
    let sentinel_port =
        make_fake_redis_with_responses(fake_redis_sentinel_stream("master", redis_port))
            .instrument(sentinel_span)
            .await;
    let spec = RedisSpec {
        addresses: vec![format!("redis+sentinel://127.0.0.1:{sentinel_port}/")],
        mode: RedisMode::Sentinel,
        ..Default::default()
    };
    RedisStore::new_standard(spec).await.expect("Working spec");
}

#[nativelink_test]
async fn test_sentinel_connect_with_bad_master() {
    // Note this is a fake redis port, which is fine because the sentinel code never connects to it
    let port = make_fake_redis_with_responses(fake_redis_sentinel_stream("other_name", 1234)).await;
    let spec = RedisSpec {
        addresses: vec![format!("redis+sentinel://127.0.0.1:{port}/")],
        mode: RedisMode::Sentinel,
        connection_timeout_ms: 100,
        ..Default::default()
    };
    assert_eq!(
        Error {
            code: Code::InvalidArgument,
            messages: vec![
                "MasterNameNotFoundBySentinel: Master with given name not found in sentinel - MasterNameNotFoundBySentinel".into(),
                format!("While connecting to redis with url: redis+sentinel://127.0.0.1:{port}/")
            ]
        },
        RedisStore::new_standard(spec).await.unwrap_err()
    );
}

#[nativelink_test]
async fn test_redis_connect_timeout() {
    let port = make_fake_redis_with_responses(HashMap::new()).await;
    let spec = RedisSpec {
        addresses: vec![format!("redis://127.0.0.1:{port}/")],
        connection_timeout_ms: 1,
        ..Default::default()
    };
    assert_eq!(
        Error {
            code: Code::DeadlineExceeded,
            messages: vec![
                "Io: timed out".into(),
                format!("While connecting to redis with url: redis://127.0.0.1:{port}/")
            ]
        },
        RedisStore::new_standard(spec).await.unwrap_err()
    );
}

#[nativelink_test]
async fn test_connect_other_db() {
    let redis_port = make_fake_redis_with_responses(fake_redis_stream()).await;
    let spec = RedisSpec {
        addresses: vec![format!("redis://127.0.0.1:{redis_port}/3")],
        ..Default::default()
    };
    RedisStore::new_standard(spec).await.expect("Working spec");
}

#[nativelink_test]
async fn test_sentinel_connect_other_db() {
    let redis_span = info_span!("redis");
    let redis_port = make_fake_redis_with_responses(fake_redis_sentinel_master_stream())
        .instrument(redis_span)
        .await;
    let sentinel_span = info_span!("sentinel");
    let sentinel_port =
        make_fake_redis_with_responses(fake_redis_sentinel_stream("master", redis_port))
            .instrument(sentinel_span)
            .await;
    let spec = RedisSpec {
        addresses: vec![format!("redis+sentinel://127.0.0.1:{sentinel_port}/3")],
        mode: RedisMode::Sentinel,
        connection_timeout_ms: 5_000,
        command_timeout_ms: 5_000,
        ..Default::default()
    };
    RedisStore::new_standard(spec).await.expect("Working spec");
}

struct SearchByContentPrefix {
    prefix: String,
}

// Define test structures that implement the scheduler traits
#[derive(Debug, Clone, PartialEq)]
struct TestSchedulerData {
    key: String,
    content: String,
    version: i64,
}

impl SchedulerStoreDecodeTo for TestSchedulerData {
    type DecodeOutput = Self;

    fn decode(version: i64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
        let content = String::from_utf8(data.to_vec())
            .map_err(|e| make_err!(Code::InvalidArgument, "Invalid UTF-8 data: {e}"))?;
        // We don't have the key in the data, so we'll use a placeholder
        Ok(Self {
            key: "decoded".to_string(),
            content,
            version,
        })
    }
}

struct TestSchedulerKey;

impl SchedulerStoreDecodeTo for TestSchedulerKey {
    type DecodeOutput = TestSchedulerData;

    fn decode(version: i64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
        TestSchedulerData::decode(version, data)
    }
}

impl SchedulerIndexProvider for SearchByContentPrefix {
    const KEY_PREFIX: &'static str = "test:";
    const INDEX_NAME: &'static str = "content_prefix";
    type Versioned = TrueValue;

    fn index_value(&self) -> std::borrow::Cow<'_, str> {
        std::borrow::Cow::Borrowed(&self.prefix)
    }
}

impl SchedulerStoreKeyProvider for SearchByContentPrefix {
    type Versioned = TrueValue;

    fn get_key(&self) -> StoreKey<'static> {
        StoreKey::Str(std::borrow::Cow::Owned("dummy_key".to_string()))
    }
}

impl SchedulerStoreDecodeTo for SearchByContentPrefix {
    type DecodeOutput = TestSchedulerData;

    fn decode(version: i64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
        TestSchedulerKey::decode(version, data)
    }
}

#[nativelink_test]
fn test_search_by_index() -> Result<(), Error> {
    fn make_ft_aggregate() -> MockCmd {
        MockCmd::new(
            redis::cmd("FT.AGGREGATE")
                .arg("test:_content_prefix__3e762c15")
                .arg("@content_prefix:{ Searchable }")
                .arg("LOAD")
                .arg(2)
                .arg("data")
                .arg("version")
                .arg("WITHCURSOR")
                .arg("COUNT")
                .arg(1500)
                .arg("MAXIDLE")
                .arg(30000)
                .arg("SORTBY")
                .arg(0),
            Ok(Value::Array(vec![
                Value::Array(vec![
                    Value::Int(1),
                    Value::Array(vec![
                        Value::BulkString(b"data".to_vec()),
                        Value::BulkString(b"1234".to_vec()),
                        Value::BulkString(b"version".to_vec()),
                        Value::BulkString(b"1".to_vec()),
                    ]),
                ]),
                Value::Int(0),
            ])),
        )
    }

    let commands = vec![
        make_ft_aggregate(),
        MockCmd::new(
            redis::cmd("FT.CREATE")
                .arg("test:_content_prefix__3e762c15")
                .arg("ON")
                .arg("HASH")
                .arg("NOHL")
                .arg("NOFIELDS")
                .arg("NOFREQS")
                .arg("NOOFFSETS")
                .arg("TEMPORARY")
                .arg(86400)
                .arg("PREFIX")
                .arg(1)
                .arg("test:")
                .arg("SCHEMA")
                .arg("content_prefix")
                .arg("TAG"),
            Ok(Value::Nil),
        ),
        make_ft_aggregate(),
    ];
    let store = make_mock_store(commands).await;
    let search_provider = SearchByContentPrefix {
        prefix: "Searchable".to_string(),
    };

    let search_results: Vec<TestSchedulerData> = store
        .search_by_index_prefix(search_provider)
        .await
        .err_tip(|| "Failed to search by index")?
        .try_collect()
        .await?;

    assert!(search_results.len() == 1, "Should find 1 matching entry");

    assert_eq!(
        search_results[0].content, "1234",
        "Content should match search pattern: '{}'",
        search_results[0].content
    );

    Ok(())
}
