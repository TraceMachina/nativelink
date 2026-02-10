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

fn add_to_response<B: BuildHasher>(
    response: &mut HashMap<String, String, B>,
    cmd: &redis::Cmd,
    args: Vec<Value>,
) {
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

async fn fake_redis<B: BuildHasher + Clone + Send + 'static>(
    listener: TcpListener,
    responses: HashMap<String, String, B>,
) {
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
                        warn!("Bytes buffer: {:?}", &buf[..res]);
                    }
                }
            }
        });
    }
}

pub async fn make_fake_redis_with_responses<B: BuildHasher + Clone + Send + 'static>(
    responses: HashMap<String, String, B>,
) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    info!("Using port {port}");

    background_spawn!("listener", async move {
        fake_redis(listener, responses).await;
    });

    port
}
