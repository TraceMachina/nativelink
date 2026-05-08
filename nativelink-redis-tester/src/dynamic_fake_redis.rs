// Copyright 2026 The NativeLink Authors. All rights reserved.
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

use core::fmt;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{Arc, Mutex};

use nativelink_util::background_spawn;
use redis::Value;
use redis_protocol::resp2::decode::decode;
use redis_protocol::resp2::types::{OwnedFrame, Resp2Frame};
use tokio::net::TcpListener;
use tracing::{debug, info, trace};

use crate::fake_redis::{arg_as_string, fake_redis_internal};

pub trait SubscriptionManagerNotify {
    fn notify_for_test(&self, value: String);
}

#[derive(Clone)]
pub struct FakeRedisBackend<S: SubscriptionManagerNotify> {
    /// Contains a list of all of the Redis keys -> fields.
    pub table: Arc<Mutex<HashMap<String, HashMap<String, Value>>>>,
    subscription_manager: Arc<Mutex<Option<Arc<S>>>>,
}

impl<S: SubscriptionManagerNotify + Send + 'static + Sync> Default for FakeRedisBackend<S> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: SubscriptionManagerNotify> fmt::Debug for FakeRedisBackend<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeRedisBackend").finish()
    }
}

const FAKE_SCRIPT_SHA: &str = "5148c724ce419ea27d1971dcb61c111dbbc6b63e";

impl<S: SubscriptionManagerNotify + Send + 'static + Sync> FakeRedisBackend<S> {
    pub fn new() -> Self {
        Self {
            table: Arc::new(Mutex::new(HashMap::new())),
            subscription_manager: Arc::new(Mutex::new(None)),
        }
    }

    pub fn set_subscription_manager(&self, subscription_manager: Arc<S>) {
        self.subscription_manager
            .lock()
            .unwrap()
            .replace(subscription_manager);
    }

    async fn dynamic_fake_redis(self, listener: TcpListener) {
        let inner = move |buf: &[u8]| -> String {
            let mut output = String::new();
            let mut buf_index = 0;
            loop {
                let frame = match decode(&buf[buf_index..]).unwrap() {
                    Some((frame, amt)) => {
                        buf_index += amt;
                        frame
                    }
                    None => {
                        panic!("No frame!");
                    }
                };
                let (cmd, args) = {
                    if let OwnedFrame::Array(a) = frame {
                        if let OwnedFrame::BulkString(s) = a.first().unwrap() {
                            let args: Vec<_> = a[1..].to_vec();
                            (str::from_utf8(s).unwrap().to_string(), args)
                        } else {
                            panic!("Array not starting with cmd: {a:?}");
                        }
                    } else {
                        panic!("Non array cmd: {frame:?}");
                    }
                };

                let ret: Value = match cmd.as_str() {
                    "HELLO" => Value::Map(vec![(
                        Value::SimpleString("server".into()),
                        Value::SimpleString("redis".into()),
                    )]),
                    "CLIENT" => {
                        // We can safely ignore these, as it's just setting the library name/version
                        Value::Int(0)
                    }
                    "SCRIPT" => {
                        assert_eq!(args[0], OwnedFrame::BulkString(b"LOAD".to_vec()));

                        let OwnedFrame::BulkString(ref _script) = args[1] else {
                            panic!("Script should be a bulkstring: {args:?}");
                        };
                        Value::SimpleString(FAKE_SCRIPT_SHA.to_string())
                    }

                    "PSUBSCRIBE" => {
                        // This does nothing at the moment, maybe we need to implement it later.
                        Value::Int(0)
                    }

                    "PUBLISH" => {
                        if let Some(subscription_manager) =
                            self.subscription_manager.lock().unwrap().as_ref()
                        {
                            subscription_manager.notify_for_test(
                                str::from_utf8(args[1].as_bytes().expect("Notification not bytes"))
                                    .expect("Notification not UTF-8")
                                    .into(),
                            );
                            Value::Int(1)
                        } else {
                            Value::Int(0)
                        }
                    }

                    "FT.AGGREGATE" => {
                        // The query is either "*" (match all) or @field:{ value }.
                        let OwnedFrame::BulkString(ref raw_query) = args[1] else {
                            panic!("Aggregate query should be a string: {args:?}");
                        };
                        let query = str::from_utf8(raw_query).unwrap();
                        // Lazy implementation making assumptions.
                        assert_eq!(
                            args[2..6],
                            vec![
                                OwnedFrame::BulkString(b"LOAD".to_vec()),
                                OwnedFrame::BulkString(b"2".to_vec()),
                                OwnedFrame::BulkString(b"data".to_vec()),
                                OwnedFrame::BulkString(b"version".to_vec())
                            ]
                        );
                        let mut results = vec![Value::Int(0)];

                        if query == "*" {
                            // Wildcard query - return all records that have both data and version fields.
                            // Some entries (e.g., from HSET) may not have version field.
                            for fields in self.table.lock().unwrap().values() {
                                if let (Some(data), Some(version)) =
                                    (fields.get("data"), fields.get("version"))
                                {
                                    results.push(Value::Array(vec![
                                        Value::BulkString(b"data".to_vec()),
                                        data.clone(),
                                        Value::BulkString(b"version".to_vec()),
                                        version.clone(),
                                    ]));
                                }
                            }
                        } else {
                            // Field-specific query: @field:{ value }
                            assert_eq!(&query[..1], "@");
                            let mut parts = query[1..].split(':');
                            let field = parts.next().expect("No field name");
                            let value = parts.next().expect("No value");
                            let value = value
                                .strip_prefix("{ ")
                                .and_then(|s| s.strip_suffix(" }"))
                                .unwrap_or(value);
                            for fields in self.table.lock().unwrap().values() {
                                if let Some(key_value) = fields.get(field)
                                    && *key_value == Value::BulkString(value.as_bytes().to_vec())
                                {
                                    results.push(Value::Array(vec![
                                        Value::BulkString(b"data".to_vec()),
                                        fields.get("data").expect("No data field").clone(),
                                        Value::BulkString(b"version".to_vec()),
                                        fields.get("version").expect("No version field").clone(),
                                    ]));
                                }
                            }
                        }

                        results[0] =
                            Value::Int(i64::try_from(results.len() - 1).unwrap_or(i64::MAX));
                        Value::Array(vec![
                            Value::Array(results),
                            Value::Int(0), // Means no more items in cursor.
                        ])
                    }

                    "EVALSHA" => {
                        assert_eq!(
                            args[0],
                            OwnedFrame::BulkString(FAKE_SCRIPT_SHA.as_bytes().to_vec())
                        );
                        assert_eq!(args[1], OwnedFrame::BulkString(b"1".to_vec()));
                        let mut value: HashMap<_, Value> = HashMap::new();
                        value.insert(
                            "data".into(),
                            Value::BulkString(args[5].as_bytes().unwrap().to_vec()),
                        );
                        for pair in args[6..].chunks(2) {
                            value.insert(
                                str::from_utf8(pair[0].as_bytes().expect("Field name not bytes"))
                                    .expect("Unable to parse field name as string")
                                    .into(),
                                Value::BulkString(pair[1].as_bytes().unwrap().to_vec()),
                            );
                        }
                        let mut ret: Option<Value> = None;
                        let key: String =
                            str::from_utf8(args[2].as_bytes().expect("Key not bytes"))
                                .expect("Key cannot be parsed as string")
                                .into();
                        let expected_existing_version: i64 =
                            str::from_utf8(args[3].as_bytes().unwrap())
                                .unwrap()
                                .parse()
                                .expect("Unable to parse existing version field");
                        let expiry: i64 = str::from_utf8(args[4].as_bytes().unwrap())
                            .unwrap()
                            .parse()
                            .expect("Unable to parse expiry field");
                        trace!(
                            key,
                            expected_existing_version,
                            expiry,
                            ?value,
                            "Want to insert with EVALSHA"
                        );
                        let version = match self.table.lock().unwrap().entry(key.clone()) {
                            Entry::Occupied(mut occupied_entry) => {
                                let version = occupied_entry
                                    .get()
                                    .get("version")
                                    .expect("No version field");
                                let Value::BulkString(version_bytes) = version else {
                                    panic!("Non-bulkstring version: {version:?}");
                                };
                                let version_int: i64 = str::from_utf8(version_bytes)
                                    .expect("Version field not valid string")
                                    .parse()
                                    .expect("Unable to parse version field");
                                if version_int == expected_existing_version {
                                    let new_version = version_int + 1;
                                    debug!(%key, %new_version, "Version update");
                                    value.insert(
                                        "version".into(),
                                        Value::BulkString(
                                            format!("{new_version}").as_bytes().to_vec(),
                                        ),
                                    );
                                    occupied_entry.insert(value);
                                    new_version
                                } else {
                                    // Version mismatch.
                                    debug!(%key, %version_int, %expected_existing_version, "Version mismatch");
                                    ret = Some(Value::Array(vec![
                                        Value::Int(0),
                                        Value::Int(version_int),
                                    ]));
                                    -1
                                }
                            }
                            Entry::Vacant(vacant_entry) => {
                                if expected_existing_version != 0 {
                                    // Version mismatch.
                                    debug!(%key, %expected_existing_version, "Version mismatch, expected zero");
                                    ret = Some(Value::Array(vec![Value::Int(0), Value::Int(0)]));
                                    -1
                                } else {
                                    debug!(%key, "Version insert");
                                    value
                                        .insert("version".into(), Value::BulkString(b"1".to_vec()));
                                    vacant_entry.insert_entry(value);
                                    1
                                }
                            }
                        };
                        if let Some(r) = ret {
                            r
                        } else {
                            Value::Array(vec![Value::Int(1), Value::Int(version)])
                        }
                    }

                    "HMSET" => {
                        let mut values = HashMap::new();
                        assert_eq!(
                            (args.len() - 1).rem_euclid(2),
                            0,
                            "Non-even args for hmset: {args:?}"
                        );
                        let chunks = args[1..].chunks_exact(2);
                        for chunk in chunks {
                            let [key, value] = chunk else {
                                panic!("Uneven hmset args");
                            };
                            let key_name: String =
                                str::from_utf8(key.as_bytes().expect("Key argument is not bytes"))
                                    .expect("Unable to parse key as string")
                                    .into();
                            values.insert(
                                key_name,
                                Value::BulkString(value.as_bytes().unwrap().to_vec()),
                            );
                        }
                        let key =
                            str::from_utf8(args[0].as_bytes().expect("Key argument is not bytes"))
                                .expect("Unable to parse key as string")
                                .into();
                        debug!(%key, ?values, "Inserting with HMSET");
                        self.table.lock().unwrap().insert(key, values);
                        Value::Okay
                    }

                    "HMGET" => {
                        let key_name =
                            str::from_utf8(args[0].as_bytes().expect("Key argument is not bytes"))
                                .expect("Unable to parse key name");

                        if let Some(fields) = self.table.lock().unwrap().get(key_name) {
                            trace!(%key_name, keys = ?fields.keys(), "Getting keys with HMGET, some keys");
                            let mut result = vec![];
                            for key in &args[1..] {
                                let field_name = str::from_utf8(
                                    key.as_bytes().expect("Field argument is not bytes"),
                                )
                                .expect("Unable to parse requested field");
                                if let Some(value) = fields.get(field_name) {
                                    result.push(value.clone());
                                } else {
                                    debug!(%key_name, %field_name, "Missing field");
                                    result.push(Value::Nil);
                                }
                            }
                            Value::Array(result)
                        } else {
                            trace!(%key_name, "Getting keys with HMGET, empty");
                            let null_count = i64::try_from(args.len() - 1).unwrap();
                            Value::Array(vec![Value::Nil, Value::Int(null_count)])
                        }
                    }
                    actual => {
                        panic!("Mock command not implemented! {actual:?}");
                    }
                };

                arg_as_string(&mut output, ret);
                if buf_index == buf.len() {
                    break;
                }
            }
            output
        };
        fake_redis_internal(listener, vec![inner]).await;
    }

    pub async fn run(self) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        info!("Using port {port}");

        background_spawn!("listener", async move {
            self.dynamic_fake_redis(listener).await;
        });

        port
    }
}
