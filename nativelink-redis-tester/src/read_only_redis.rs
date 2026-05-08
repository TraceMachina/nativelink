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

use core::fmt::Write;
use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use either::Either;
use nativelink_util::background_spawn;
use redis::Value;
use redis_protocol::resp2::decode::decode;
use redis_protocol::resp2::types::OwnedFrame;
use tokio::net::TcpListener;
use tracing::info;

use crate::fake_redis::{arg_as_string, fake_redis_internal};

const FAKE_SCRIPT_SHA: &str = "5148c724ce419ea27d1971dcb61c111dbbc6b63e";

#[derive(Clone, Debug)]
pub struct ReadOnlyRedis {
    // The first time we hit SETRANGE/HMSET, we output a ReadOnly. Next time, we assume we're reconnected and do correct values
    readonly_triggered: Arc<AtomicBool>,
}

impl Default for ReadOnlyRedis {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadOnlyRedis {
    pub fn new() -> Self {
        Self {
            readonly_triggered: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn dynamic_fake_redis(self, listener: TcpListener) {
        let readonly_err_str = "READONLY You can't write against a read only replica.";
        let readonly_err = format!("!{}\r\n{readonly_err_str}\r\n", readonly_err_str.len());

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

                let ret: Either<Value, String> = match cmd.as_str() {
                    "HELLO" => Either::Left(Value::Map(vec![(
                        Value::SimpleString("server".into()),
                        Value::SimpleString("redis".into()),
                    )])),
                    "CLIENT" => {
                        // We can safely ignore these, as it's just setting the library name/version
                        Either::Left(Value::Int(0))
                    }
                    "SCRIPT" => {
                        assert_eq!(args[0], OwnedFrame::BulkString(b"LOAD".to_vec()));

                        let OwnedFrame::BulkString(ref _script) = args[1] else {
                            panic!("Script should be a bulkstring: {args:?}");
                        };
                        Either::Left(Value::SimpleString(FAKE_SCRIPT_SHA.to_string()))
                    }
                    "ROLE" => Either::Left(Value::Array(vec![
                        Value::BulkString(b"master".to_vec()),
                        Value::Int(0),
                        Value::Array(vec![]),
                    ])),
                    "SETRANGE" => {
                        let value = self.readonly_triggered.load(Ordering::Relaxed);
                        if value {
                            Either::Left(Value::Int(5))
                        } else {
                            self.readonly_triggered.store(true, Ordering::Relaxed);
                            Either::Right(readonly_err.clone())
                        }
                    }
                    "STRLEN" => Either::Left(Value::Int(5)),
                    "RENAME" | "HMSET" => {
                        let value = self.readonly_triggered.load(Ordering::Relaxed);
                        if value {
                            Either::Left(Value::Okay)
                        } else {
                            self.readonly_triggered.store(true, Ordering::Relaxed);
                            Either::Right(readonly_err.clone())
                        }
                    }
                    "EVALSHA" => Either::Left(Value::Array(vec![Value::Int(1), Value::Int(0)])),
                    "EXPIRE" => {
                        assert_eq!(args[1], OwnedFrame::BulkString(b"60".to_vec()));
                        let value = self.readonly_triggered.load(Ordering::Relaxed);
                        if value {
                            Either::Left(Value::Int(1))
                        } else {
                            self.readonly_triggered.store(true, Ordering::Relaxed);
                            Either::Right(readonly_err.clone())
                        }
                    }
                    actual => {
                        panic!("Mock command not implemented! {actual:?}");
                    }
                };

                match ret {
                    Either::Left(v) => {
                        arg_as_string(&mut output, v);
                    }
                    Either::Right(s) => {
                        write!(&mut output, "{s}").unwrap();
                    }
                }

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
