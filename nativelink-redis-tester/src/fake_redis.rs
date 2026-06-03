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
use core::hash::BuildHasher;
use std::collections::HashMap;

use nativelink_util::background_spawn;
use redis::Value;
use redis_test::IntoRedisValue;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{error, info, warn};

fn cmd_as_string(cmd: &redis::Cmd) -> String {
    let raw = cmd.get_packed_command();
    String::from_utf8(raw).unwrap()
}

pub(crate) fn arg_as_string(output: &mut String, arg: Value) {
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
        Value::Nil => {
            write!(output, "_\r\n").unwrap();
        }
        Value::Boolean(value) => {
            if value {
                write!(output, "#t\r\n")
            } else {
                write!(output, "#f\r\n")
            }
            .unwrap();
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

pub fn add_to_response<B: BuildHasher>(
    response: &mut HashMap<String, String, B>,
    cmd: &redis::Cmd,
    args: Vec<Value>,
) {
    add_to_response_raw(response, cmd, args_as_string(args));
}

pub fn add_to_response_raw<B: BuildHasher>(
    response: &mut HashMap<String, String, B>,
    cmd: &redis::Cmd,
    args: String,
) {
    response.insert(cmd_as_string(cmd), args);
}

fn setinfo(responses: &mut HashMap<String, String>) {
    // We do raw inserts of command here, because the library sends 3/4 commands in one go
    // They always start with HELLO, then optionally SELECT, so we use this to differentiate
    let hello = cmd_as_string(redis::cmd("HELLO").arg("3"));
    let setinfo = cmd_as_string(
        redis::cmd("CLIENT")
            .arg("SETINFO")
            .arg("LIB-NAME")
            .arg("redis-rs"),
    );
    responses.insert(
        [hello.clone(), setinfo.clone()].join(""),
        args_as_string(vec![
            Value::Map(vec![(
                Value::SimpleString("server".into()),
                Value::SimpleString("redis".into()),
            )]),
            Value::Okay,
            Value::Okay,
        ]),
    );
    responses.insert(
        [hello, cmd_as_string(redis::cmd("SELECT").arg(3)), setinfo].join(""),
        args_as_string(vec![
            Value::Map(vec![(
                Value::SimpleString("server".into()),
                Value::SimpleString("redis".into()),
            )]),
            Value::Okay,
            Value::Okay,
            Value::Okay,
        ]),
    );
}

pub fn add_lua_script<B: BuildHasher>(
    responses: &mut HashMap<String, String, B>,
    lua_script: &str,
    hash: &str,
) {
    add_to_response(
        responses,
        redis::cmd("SCRIPT").arg("LOAD").arg(lua_script),
        vec![hash.into_redis_value()],
    );
}

pub fn fake_redis_stream() -> HashMap<String, String> {
    let mut responses = HashMap::new();
    setinfo(&mut responses);
    // Does setinfo as well, so need to respond to all 3
    add_to_response(
        &mut responses,
        redis::cmd("SELECT").arg("3"),
        vec![Value::Okay, Value::Okay, Value::Okay],
    );
    responses
}

pub fn fake_redis_sentinel_master_stream() -> HashMap<String, String> {
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

pub fn fake_redis_sentinel_stream(master_name: &str, redis_port: u16) -> HashMap<String, String> {
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

pub(crate) async fn fake_redis_internal<H>(listener: TcpListener, handlers: Vec<H>)
where
    H: Fn(&[u8]) -> String + Send + Clone + 'static + Sync,
{
    let mut handler_iter = handlers.iter().cloned().cycle();
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
        let local_handler = handler_iter.next().unwrap();
        background_spawn!("thread", async move {
            loop {
                let mut buf = vec![0; 8192];
                let res = stream.read(&mut buf).await.unwrap();
                if res != 0 {
                    let output = local_handler(&buf[..res]);
                    if !output.is_empty() {
                        stream.write_all(output.as_bytes()).await.unwrap();
                    }
                }
            }
        });
    }
}

async fn fake_redis<B>(listener: TcpListener, all_responses: Vec<HashMap<String, String, B>>)
where
    B: BuildHasher + Clone + Send + 'static + Sync,
{
    let funcs = all_responses
        .iter()
        .map(|responses| {
            info!("Responses are: {:?}", responses);
            let values = responses.clone();
            move |buf: &[u8]| -> String {
                let str_buf = String::from_utf8_lossy(buf).into_owned();
                for (key, value) in &values {
                    if str_buf.starts_with(key) {
                        info!("Responding to {}", str_buf.replace("\r\n", "\\r\\n"));
                        return value.clone();
                    }
                }
                warn!(
                    "Unknown command: {}",
                    str_buf.chars().take(1000).collect::<String>()
                );
                String::new()
            }
        })
        .collect();
    fake_redis_internal(listener, funcs).await;
}

async fn make_fake_redis_with_multiple_responses<B: BuildHasher + Clone + Send + 'static + Sync>(
    responses: Vec<HashMap<String, String, B>>,
) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    info!(port, "Fake redis booted");

    background_spawn!("listener", async move {
        fake_redis(listener, responses).await;
    });

    port
}

pub async fn make_fake_redis_with_responses<B: BuildHasher + Clone + Send + 'static + Sync>(
    responses: HashMap<String, String, B>,
) -> u16 {
    make_fake_redis_with_multiple_responses(vec![responses]).await
}
