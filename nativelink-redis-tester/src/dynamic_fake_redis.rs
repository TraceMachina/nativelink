// Copyright 2026 Trace Machina, Inc. All rights reserved.
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
use core::time::Duration;
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{Arc, Mutex};

use nativelink_util::background_spawn;
use redis::Value;
use redis_protocol::resp2::decode::decode;
use redis_protocol::resp2::types::{OwnedFrame, Resp2Frame};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::oneshot::{self, Sender};
use tracing::{debug, info, trace};

use crate::fake_redis::arg_as_string;

/// The error a command answers with instead of running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fault {
    /// `-ERR <message>`.
    Error(String),
    /// `-READONLY`, what a replica answers a write with after a failover.
    ReadOnly,
    /// `-OOM`, what a full server with `noeviction` answers a write with.
    Oom,
    /// The command runs, then the client reads `-ERR <message>`: a reply
    /// lost after the write landed, as a timeout loses it.
    AppliedThenError(String),
}

impl Fault {
    fn reply(&self) -> String {
        match self {
            Self::Error(message) | Self::AppliedThenError(message) => {
                format!("-ERR {message}\r\n")
            }
            Self::ReadOnly => "-READONLY You can't write against a read only replica.\r\n".into(),
            Self::Oom => "-OOM command not allowed when used memory > 'maxmemory'.\r\n".into(),
        }
    }
}

#[derive(Clone, Debug)]
struct FaultRule {
    /// Upper-case command name, or `*` for any.
    command: String,
    /// Matches commands whose first argument starts with this.
    key_prefix: Option<String>,
    fault: Fault,
    /// How many more commands it fails; `None` is until cleared.
    remaining: Option<u64>,
}

/// What the fake does wrong, consulted on every command and connection.
/// The default does nothing wrong.
#[derive(Debug, Default)]
struct FaultPolicy {
    rules: Vec<FaultRule>,
    /// Commands received, by upper-case name, faulted ones included.
    counts: HashMap<String, u64>,
    /// Commands received since the start; never reset.
    total: u64,
    /// Connections are closed and new ones refused while set.
    partitioned: bool,
    /// Bumped to close every open connection once.
    generation: u64,
    /// Held before each reply is written.
    latency: Option<Duration>,
    /// Ports whose listener is gone: its connections close and it accepts
    /// no more, as when the process that held them dies.
    retired: std::collections::HashSet<u16>,
}

impl FaultPolicy {
    /// The reply that replaces the command's, and whether the command
    /// still runs.
    fn on_command(&mut self, command: &str, key: Option<&str>) -> Option<(String, bool)> {
        *self.counts.entry(command.to_string()).or_default() += 1;
        self.total += 1;
        let index = self.rules.iter().position(|rule| {
            (rule.command == "*" || rule.command == command)
                && rule
                    .key_prefix
                    .as_ref()
                    .is_none_or(|prefix| key.is_some_and(|key| key.starts_with(prefix.as_str())))
        })?;
        let rule = &mut self.rules[index];
        let reply = (
            rule.fault.reply(),
            matches!(rule.fault, Fault::AppliedThenError(_)),
        );
        if let Some(remaining) = &mut rule.remaining {
            *remaining -= 1;
            if *remaining == 0 {
                self.rules.remove(index);
            }
        }
        Some(reply)
    }
}

/// How many leading bytes of `buf` are whole RESP frames.
fn complete_frames_len(buf: &[u8]) -> usize {
    let mut index = 0;
    while index < buf.len() {
        match decode(&buf[index..]) {
            Ok(Some((_, amt))) => index += amt,
            _ => break,
        }
    }
    index
}

/// Commands that write, for faults aimed at writes only.
pub const WRITE_COMMANDS: &[&str] = &["EVALSHA", "HMSET", "EXPIRE", "PUBLISH"];

pub trait SubscriptionManagerNotify {
    fn notify_for_test(&self, value: String);
}

#[derive(Clone)]
pub struct FakeRedisBackend<S: SubscriptionManagerNotify> {
    /// Contains a list of all of the Redis keys -> fields.
    pub table: Arc<Mutex<HashMap<String, HashMap<String, Value>>>>,
    /// TTL (seconds) attached to each key via `EXPIRE`, so tests can
    /// assert a key was given a bounded lifetime.
    pub expiries: Arc<Mutex<HashMap<String, i64>>>,
    subscription_manager: Arc<Mutex<Option<Arc<S>>>>,
    faults: Arc<Mutex<FaultPolicy>>,
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

const FAKE_SCRIPT_SHA: &str = "d89e3573a1f9689c22115e8a41bd332d0c7c2643";

impl<S: SubscriptionManagerNotify + Send + 'static + Sync> FakeRedisBackend<S> {
    pub fn new() -> Self {
        Self {
            table: Arc::new(Mutex::new(HashMap::new())),
            expiries: Arc::new(Mutex::new(HashMap::new())),
            subscription_manager: Arc::new(Mutex::new(None)),
            faults: Arc::new(Mutex::new(FaultPolicy::default())),
        }
    }

    /// Answers the next `times` commands named `command` (`*` for any)
    /// whose key starts with `key_prefix` with `fault`; `None` is until
    /// `clear_faults`.
    pub fn fail_commands(
        &self,
        command: &str,
        key_prefix: Option<&str>,
        fault: Fault,
        times: Option<u64>,
    ) {
        if times == Some(0) {
            return;
        }
        self.faults.lock().unwrap().rules.push(FaultRule {
            command: command.to_ascii_uppercase(),
            key_prefix: key_prefix.map(str::to_string),
            fault,
            remaining: times,
        });
    }

    /// A failover as the client sees it: every connection closes once,
    /// then writes answer `READONLY` `readonly_writes` times.
    pub fn failover(&self, readonly_writes: u64) {
        self.drop_connections();
        for command in WRITE_COMMANDS {
            self.fail_commands(command, None, Fault::ReadOnly, Some(readonly_writes));
        }
    }

    /// Removes every fault rule, the partition and the latency.
    pub fn clear_faults(&self) {
        let mut faults = self.faults.lock().unwrap();
        faults.rules.clear();
        faults.partitioned = false;
        faults.latency = None;
    }

    /// Closes every open connection and refuses new ones until called
    /// with `false`.
    pub fn partition(&self, partitioned: bool) {
        let mut faults = self.faults.lock().unwrap();
        faults.partitioned = partitioned;
        if partitioned {
            faults.generation += 1;
        }
    }

    /// Closes every connection made through `port`'s listener and refuses
    /// new ones there; the data and the other listeners stay.
    pub fn retire_listener(&self, port: u16) {
        self.faults.lock().unwrap().retired.insert(port);
    }

    /// Closes every open connection once; new ones are accepted.
    pub fn drop_connections(&self) {
        self.faults.lock().unwrap().generation += 1;
    }

    /// Holds each reply for `latency` (tokio time) before writing it.
    pub fn set_latency(&self, latency: Option<Duration>) {
        self.faults.lock().unwrap().latency = latency;
    }

    /// Deletes every key starting with `prefix`, as an eviction would.
    /// Returns how many went.
    pub fn evict_prefix(&self, prefix: &str) -> usize {
        let mut table = self.table.lock().unwrap();
        let before = table.len();
        table.retain(|key, _| !key.starts_with(prefix));
        before - table.len()
    }

    /// Commands received since the last reset, by upper-case name.
    pub fn command_counts(&self) -> HashMap<String, u64> {
        self.faults.lock().unwrap().counts.clone()
    }

    /// Commands received since the fake started.
    pub fn commands_total(&self) -> u64 {
        self.faults.lock().unwrap().total
    }

    pub fn reset_command_counts(&self) {
        self.faults.lock().unwrap().counts.clear();
    }

    pub fn set_subscription_manager(&self, subscription_manager: Arc<S>) {
        self.subscription_manager
            .lock()
            .unwrap()
            .replace(subscription_manager);
    }

    async fn dynamic_fake_redis(self, listener: TcpListener, listener_ready_tx: Sender<()>) {
        let faults = self.faults.clone();
        let port = listener.local_addr().map_or(0, |addr| addr.port());
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

                let key = args
                    .first()
                    .and_then(|arg| arg.as_bytes())
                    .and_then(|bytes| str::from_utf8(bytes).ok());
                // EVALSHA's key is its third argument, after the sha and the
                // key count.
                let key = if cmd.eq_ignore_ascii_case("EVALSHA") {
                    args.get(2)
                        .and_then(|arg| arg.as_bytes())
                        .and_then(|bytes| str::from_utf8(bytes).ok())
                } else {
                    key
                };
                let fault = self
                    .faults
                    .lock()
                    .unwrap()
                    .on_command(&cmd.to_ascii_uppercase(), key);
                if let Some((reply, false)) = &fault {
                    output.push_str(reply);
                    if buf_index == buf.len() {
                        break;
                    }
                    continue;
                }

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
                        // The real ft_aggregate caller now passes an explicit
                        // `TIMEOUT <ms>` clause before `LOAD`. Tolerate both
                        // shapes here so this fake doesn't break older callers
                        // and the LOAD-args check still validates the bit we
                        // actually care about.
                        let load_offset = if matches!(args.get(2), Some(OwnedFrame::BulkString(b)) if b == b"TIMEOUT")
                        {
                            // Skip "TIMEOUT" and its millisecond argument.
                            4
                        } else {
                            2
                        };
                        // Lazy implementation making assumptions.
                        assert_eq!(
                            args[load_offset..load_offset + 4],
                            vec![
                                OwnedFrame::BulkString(b"LOAD".to_vec()),
                                OwnedFrame::BulkString(b"2".to_vec()),
                                OwnedFrame::BulkString(b"data".to_vec()),
                                OwnedFrame::BulkString(b"version".to_vec())
                            ]
                        );
                        let mut results = vec![Value::Int(0)];

                        // `SORTBY <n> <@field> <ASC|DESC> ...`: honour the first
                        // key so tests can assert the order the scheduler asks
                        // for. Missing sort fields sort first, like RediSearch.
                        let sort_spec = args
                            .iter()
                            .position(|a| matches!(a, OwnedFrame::BulkString(b) if b == b"SORTBY"))
                            .and_then(|i| {
                                let field = args.get(i + 2)?.as_bytes()?;
                                let field = str::from_utf8(field).ok()?.trim_start_matches('@').to_string();
                                let desc = matches!(args.get(i + 3), Some(OwnedFrame::BulkString(d)) if d == b"DESC");
                                Some((field, desc))
                            });
                        // Ties come back in key order rather than the
                        // table's hash order, so a run repeats.
                        let sorted = |mut rows: Vec<(String, HashMap<String, Value>)>| {
                            rows.sort_by(|a, b| a.0.cmp(&b.0));
                            let mut rows: Vec<_> = rows.into_iter().map(|(_, f)| f).collect();
                            if let Some((field, desc)) = &sort_spec {
                                let key = |r: &HashMap<String, Value>| match r.get(field) {
                                    Some(Value::BulkString(b)) => b.clone(),
                                    _ => Vec::new(),
                                };
                                rows.sort_by_key(key);
                                if *desc {
                                    rows.reverse();
                                }
                            }
                            rows
                        };

                        if query == "*" {
                            // Wildcard query - return all records that have both data and version fields.
                            // Some entries (e.g., from HSET) may not have version field.
                            let rows: Vec<_> = self
                                .table
                                .lock()
                                .unwrap()
                                .iter()
                                .filter(|(_, f)| {
                                    f.contains_key("data") && f.contains_key("version")
                                })
                                .map(|(k, f)| (k.clone(), f.clone()))
                                .collect();
                            for fields in sorted(rows) {
                                results.push(Value::Array(vec![
                                    Value::BulkString(b"data".to_vec()),
                                    fields["data"].clone(),
                                    Value::BulkString(b"version".to_vec()),
                                    fields["version"].clone(),
                                ]));
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
                            let rows: Vec<_> = self
                                .table
                                .lock()
                                .unwrap()
                                .iter()
                                .filter(|(_, f)| {
                                    f.get(field)
                                        == Some(&Value::BulkString(value.as_bytes().to_vec()))
                                })
                                .map(|(k, f)| (k.clone(), f.clone()))
                                .collect();
                            for fields in sorted(rows) {
                                {
                                    let mut record = vec![
                                        Value::BulkString(b"data".to_vec()),
                                        fields.get("data").expect("No data field").clone(),
                                    ];
                                    // LOAD leaves out fields the hash does not
                                    // have, so a non-versioned record comes
                                    // back without a version.
                                    if let Some(version) = fields.get("version") {
                                        record.push(Value::BulkString(b"version".to_vec()));
                                        record.push(version.clone());
                                    }
                                    results.push(Value::Array(record));
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
                        let (chunks, rest) = args[1..].as_chunks::<2>();
                        assert_eq!(rest, [], "Uneven hmset args");
                        for [key, value] in chunks {
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

                    "EXPIRE" => {
                        // `EXPIRE key seconds`: record the TTL; return 1
                        // if the key existed, else 0 (as real Redis does).
                        let key_name =
                            str::from_utf8(args[0].as_bytes().expect("Key argument is not bytes"))
                                .expect("Unable to parse key name")
                                .to_string();
                        let seconds = str::from_utf8(
                            args[1].as_bytes().expect("EXPIRE seconds is not bytes"),
                        )
                        .expect("EXPIRE seconds is not utf8")
                        .parse::<i64>()
                        .expect("EXPIRE seconds is not an integer");
                        let exists = self.table.lock().unwrap().contains_key(&key_name);
                        if exists {
                            self.expiries.lock().unwrap().insert(key_name, seconds);
                            Value::Int(1)
                        } else {
                            Value::Int(0)
                        }
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

                match fault {
                    Some((reply, true)) => output.push_str(&reply),
                    _ => arg_as_string(&mut output, ret),
                }
                if buf_index == buf.len() {
                    break;
                }
            }
            output
        };
        listener_ready_tx
            .send(())
            .expect("Expected successful send");
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                panic!("accept error");
            };
            let generation = {
                let faults = faults.lock().unwrap();
                if faults.partitioned || faults.retired.contains(&port) {
                    drop(stream);
                    continue;
                }
                faults.generation
            };
            let handler = inner.clone();
            let faults = faults.clone();
            background_spawn!("dynamic_fake_redis_connection", async move {
                let mut buf = vec![0; 8192];
                // Bytes read but not yet a whole command: a command can span
                // two reads.
                let mut pending: Vec<u8> = Vec::new();
                loop {
                    let Ok(read) = stream.read(&mut buf).await else {
                        return;
                    };
                    {
                        let faults = faults.lock().unwrap();
                        if read == 0
                            || faults.generation != generation
                            || faults.retired.contains(&port)
                        {
                            return;
                        }
                    }
                    pending.extend_from_slice(&buf[..read]);
                    let complete = complete_frames_len(&pending);
                    if complete == 0 {
                        continue;
                    }
                    let commands: Vec<u8> = pending.drain(..complete).collect();
                    let output = handler(&commands);
                    let latency = faults.lock().unwrap().latency;
                    if let Some(latency) = latency {
                        tokio::time::sleep(latency).await;
                    }
                    if faults.lock().unwrap().generation != generation {
                        return;
                    }
                    if !output.is_empty() && stream.write_all(output.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    }

    pub async fn run(self) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        info!("Using port {port}");

        let (listener_ready_tx, listener_ready_rx) = oneshot::channel::<()>();

        background_spawn!("listener", async move {
            self.dynamic_fake_redis(listener, listener_ready_tx).await;
        });

        listener_ready_rx
            .await
            .expect("Expected successful listener boot");

        port
    }
}
