// Copyright 2024 The NativeLink Authorsr All rights reserved.
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

use core::time::Duration;
use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::Arc;
use std::thread::panicking;
use std::time::SystemTime;

use bytes::Bytes;
use fred::bytes_utils::string::Str;
use fred::clients::SubscriberClient;
use fred::error::{Error as RedisError, ErrorKind as RedisErrorKind};
use fred::mocks::{MockCommand, Mocks};
use fred::prelude::{Builder, Pool as RedisPool};
use fred::types::Value as RedisValue;
use fred::types::config::{Config as RedisConfig, PerformanceConfig};
use futures::StreamExt;
use mock_instant::global::SystemTime as MockSystemTime;
use nativelink_config::schedulers::SimpleSpec;
use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::{
    ExecuteRequest, Platform, digest_function,
};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    ConnectionResult, StartExecute, UpdateForWorker, update_for_worker,
};
use nativelink_scheduler::awaited_action_db::{
    AwaitedAction, AwaitedActionDb, AwaitedActionSubscriber,
};
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::store_awaited_action_db::StoreAwaitedActionDb;
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_store::redis_store::{RedisStore, RedisSubscriptionManager};
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionUniqueKey, ActionUniqueQualifier, OperationId, WorkerId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::operation_state_manager::{ClientStateManager, OperationFilter};
use nativelink_util::platform_properties::PlatformProperties;
use nativelink_util::store_trait::{SchedulerStore, SchedulerSubscriptionManager};
use parking_lot::Mutex;
use pretty_assertions::assert_eq;
use tokio::sync::{Notify, mpsc};
use tonic::Code;
use utils::scheduler_utils::update_eq;

mod utils {
    pub(crate) mod scheduler_utils;
}

const INSTANCE_NAME: &str = "instance_name";
const TEMP_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";
const SCRIPT_VERSION: &str = "3e762c15";
const VERSION_SCRIPT_HASH: &str = "b22b9926cbce9dd9ba97fa7ba3626f89feea1ed5";
const MAX_CHUNK_UPLOADS_PER_UPDATE: usize = 10;
const SCAN_COUNT: u32 = 10_000;

fn mock_uuid_generator() -> String {
    uuid::Uuid::parse_str(TEMP_UUID).unwrap().to_string()
}

type CommandandCallbackTuple = (MockCommand, Option<Box<dyn FnOnce() + Send>>);
#[derive(Default)]
struct MockRedisBackend {
    /// Commands we expect to encounter, and results we to return to the client.
    // Commands are pushed from the back and popped from the front.
    expected: Mutex<VecDeque<(CommandandCallbackTuple, Result<RedisValue, RedisError>)>>,
}

impl fmt::Debug for MockRedisBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MockRedisBackend").finish()
    }
}

impl MockRedisBackend {
    fn new() -> Self {
        Self::default()
    }

    fn expect(
        &self,
        command: MockCommand,
        result: Result<RedisValue, RedisError>,
        cb: Option<Box<dyn FnOnce() + Send>>,
    ) -> &Self {
        self.expected.lock().push_back(((command, cb), result));
        self
    }
}

impl Mocks for MockRedisBackend {
    fn process_command(&self, actual: MockCommand) -> Result<RedisValue, RedisError> {
        let Some(((expected, maybe_cb), result)) = self.expected.lock().pop_front() else {
            // panic here -- this isn't a redis error, it's a test failure
            panic!("Didn't expect any more commands, but received {actual:?}");
        };

        assert_eq!(actual, expected);
        if let Some(cb) = maybe_cb {
            (cb)();
        }

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

        let results = core::iter::once(MULTI.clone())
            .chain(commands)
            .chain([EXEC.clone()])
            .map(|command| self.process_command(command))
            .collect::<Result<Vec<_>, RedisError>>()?;

        Ok(RedisValue::Array(results))
    }
}

impl Drop for MockRedisBackend {
    fn drop(&mut self) {
        if panicking() {
            // We're already panicking, let's make debugging easier and let future devs solve problems one at a time.
            return;
        }

        let expected = self.expected.get_mut();

        if expected.is_empty() {
            return;
        }

        assert_eq!(
            expected
                .drain(..)
                .map(|((cmd, _), res)| (cmd, res))
                .collect::<VecDeque<_>>(),
            VecDeque::new(),
            "Didn't receive all expected commands."
        );

        // Panicking isn't enough inside a tokio task, we need to `exit(1)`
        std::process::exit(1)
    }
}

struct FakeRedisBackend {
    /// Contains a list of all of the Redis keys -> fields.
    table: Mutex<HashMap<String, HashMap<String, RedisValue>>>,
    /// The subscription manager (maybe).
    subscription_manager: Mutex<Option<Arc<RedisSubscriptionManager>>>,
}

impl fmt::Debug for FakeRedisBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FakeRedisBackend").finish()
    }
}

impl FakeRedisBackend {
    fn new() -> Self {
        Self {
            table: Mutex::new(HashMap::new()),
            subscription_manager: Mutex::new(None),
        }
    }

    fn set_subscription_manager(&self, subscription_manager: Arc<RedisSubscriptionManager>) {
        *self.subscription_manager.lock() = Some(subscription_manager);
    }
}

impl Mocks for FakeRedisBackend {
    fn process_command(&self, actual: MockCommand) -> Result<RedisValue, RedisError> {
        if actual.cmd == Str::from_static("SUBSCRIBE") {
            // This does nothing at the moment, maybe we need to implement it later.
            return Ok(RedisValue::Integer(0));
        }

        if actual.cmd == Str::from_static("PUBLISH") {
            if let Some(subscription_manager) = self.subscription_manager.lock().as_ref() {
                subscription_manager.notify_for_test(
                    str::from_utf8(actual.args[1].as_bytes().expect("Notification not bytes"))
                        .expect("Notification not UTF-8")
                        .into(),
                );
            }
            return Ok(RedisValue::Integer(0));
        }

        if actual.cmd == Str::from_static("FT.AGGREGATE") {
            // The query is @field:value where value might be wrapped in braces.
            let query = actual.args[1]
                .clone()
                .into_string()
                .expect("Aggregate query should be a string");
            assert_eq!(&query[..1], "@");
            let mut parts = query[1..].split(':');
            let field = parts.next().expect("No field name");
            let value = parts.next().expect("No value");
            let value = value
                .strip_prefix("{ ")
                .and_then(|s| s.strip_suffix(" }"))
                .unwrap_or(value);
            // Lazy implementation making assumptions.
            assert_eq!(
                actual.args[2..6],
                vec!["LOAD".into(), 2.into(), "data".into(), "version".into()]
            );
            let mut results = vec![RedisValue::Integer(0)];
            for fields in self.table.lock().values() {
                if let Some(key_value) = fields.get(field) {
                    if *key_value == RedisValue::Bytes(Bytes::from(value.to_owned())) {
                        results.push(RedisValue::Array(vec![
                            RedisValue::Bytes(Bytes::from("data")),
                            fields.get("data").expect("No data field").clone(),
                            RedisValue::Bytes(Bytes::from("version")),
                            fields.get("version").expect("No version field").clone(),
                        ]));
                    }
                }
            }
            results[0] = ((results.len() - 1) as u32).into();
            return Ok(RedisValue::Array(vec![
                RedisValue::Array(results),
                RedisValue::Integer(0), // Means no more items in cursor.
            ]));
        }

        if actual.cmd == Str::from_static("EVALSHA") {
            assert_eq!(actual.args[0], VERSION_SCRIPT_HASH.into());
            let mut value = HashMap::new();
            value.insert("data".into(), actual.args[4].clone());
            for pair in actual.args[5..].chunks(2) {
                value.insert(
                    str::from_utf8(pair[0].as_bytes().expect("Field name not bytes"))
                        .expect("Unable to parse field name as string")
                        .into(),
                    pair[1].clone(),
                );
            }
            let version = match self.table.lock().entry(
                str::from_utf8(actual.args[2].as_bytes().expect("Key not bytes"))
                    .expect("Key cannot be parsed as string")
                    .into(),
            ) {
                Entry::Occupied(mut occupied_entry) => {
                    let version = occupied_entry
                        .get()
                        .get("version")
                        .expect("No version field");
                    let version_int: i64 =
                        str::from_utf8(version.as_bytes().expect("Version field not bytes"))
                            .expect("Version field not valid string")
                            .parse()
                            .expect("Unable to parse version field");
                    if *version != actual.args[3] {
                        // Version mismatch.
                        return Ok(RedisValue::Array(vec![
                            RedisValue::Integer(0),
                            RedisValue::Integer(version_int),
                        ]));
                    }
                    value.insert(
                        "version".into(),
                        RedisValue::Bytes(
                            format!("{}", version_int + 1).as_bytes().to_owned().into(),
                        ),
                    );
                    occupied_entry.insert(value);
                    version_int + 1
                }
                Entry::Vacant(vacant_entry) => {
                    if actual.args[3] != RedisValue::Bytes(Bytes::from_static(b"0")) {
                        // Version mismatch.
                        return Ok(RedisValue::Array(vec![
                            RedisValue::Integer(0),
                            RedisValue::Integer(0),
                        ]));
                    }
                    value.insert("version".into(), RedisValue::Bytes("1".into()));
                    vacant_entry.insert_entry(value);
                    1
                }
            };
            return Ok(RedisValue::Array(vec![
                RedisValue::Integer(1),
                RedisValue::Integer(version),
            ]));
        }

        if actual.cmd == Str::from_static("HSET") {
            assert_eq!(
                RedisValue::Bytes(Bytes::from_static(b"data")),
                actual.args[1]
            );
            let mut values = HashMap::new();
            values.insert("data".into(), actual.args[2].clone());
            self.table.lock().insert(
                str::from_utf8(
                    actual.args[0]
                        .as_bytes()
                        .expect("Key argument is not bytes"),
                )
                .expect("Unable to parse key as string")
                .into(),
                values,
            );
            return Ok(RedisValue::new_ok());
        }

        if actual.cmd == Str::from_static("HMGET") {
            if let Some(fields) = self.table.lock().get(
                str::from_utf8(
                    actual.args[0]
                        .as_bytes()
                        .expect("Key argument is not bytes"),
                )
                .expect("Unable to parse key name"),
            ) {
                let mut result = vec![];
                for key in &actual.args[1..] {
                    if let Some(value) = fields.get(
                        str::from_utf8(key.as_bytes().expect("Field argument is not bytes"))
                            .expect("Unable to parse requested field"),
                    ) {
                        result.push(value.clone());
                    } else {
                        result.push(RedisValue::Null);
                    }
                }
                return Ok(RedisValue::Array(result));
            }
            return Err(RedisError::new(RedisErrorKind::NotFound, String::new()));
        }

        panic!("Mock command not implemented! {actual:?}");
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

        let results = core::iter::once(MULTI.clone())
            .chain(commands)
            .chain([EXEC.clone()])
            .map(|command| self.process_command(command))
            .collect::<Result<Vec<_>, RedisError>>()?;

        Ok(RedisValue::Array(results))
    }
}

fn make_redis_store(sub_channel: &str, mocks: Arc<impl Mocks>) -> Arc<RedisStore> {
    let mut builder = Builder::default_centralized();
    builder.set_config(RedisConfig {
        mocks: Some(mocks),
        ..Default::default()
    });
    let (client_pool, subscriber_client) = make_clients(builder);
    Arc::new(
        RedisStore::new_from_builder_and_parts(
            client_pool,
            subscriber_client,
            Some(sub_channel.into()),
            mock_uuid_generator,
            String::new(),
            4064,
            MAX_CHUNK_UPLOADS_PER_UPDATE,
            SCAN_COUNT,
        )
        .unwrap(),
    )
}

fn make_clients(mut builder: Builder) -> (RedisPool, SubscriberClient) {
    const CONNECTION_POOL_SIZE: usize = 1;
    let client_pool = builder
        .set_performance_config(PerformanceConfig {
            broadcast_channel_capacity: 4096,
            ..Default::default()
        })
        .build_pool(CONNECTION_POOL_SIZE)
        .unwrap();

    let subscriber_client = builder.build_subscriber_client().unwrap();
    (client_pool, subscriber_client)
}

async fn verify_initial_connection_message(
    worker_id: WorkerId,
    rx: &mut mpsc::UnboundedReceiver<UpdateForWorker>,
) {
    // Worker should have been sent an execute command.
    let expected_msg_for_worker = UpdateForWorker {
        update: Some(update_for_worker::Update::ConnectionResult(
            ConnectionResult {
                worker_id: worker_id.into(),
            },
        )),
    };
    let msg_for_worker = rx.recv().await.unwrap();
    assert_eq!(msg_for_worker, expected_msg_for_worker);
}

const NOW_TIME: u64 = 10000;

async fn setup_new_worker(
    scheduler: &SimpleScheduler,
    worker_id: WorkerId,
    props: PlatformProperties,
) -> Result<mpsc::UnboundedReceiver<UpdateForWorker>, Error> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let worker = Worker::new(worker_id.clone(), props, tx, NOW_TIME);
    scheduler
        .add_worker(worker)
        .await
        .err_tip(|| "Failed to add worker")?;
    tokio::task::yield_now().await; // Allow task<->worker matcher to run.
    verify_initial_connection_message(worker_id, &mut rx).await;
    Ok(rx)
}

fn make_awaited_action(operation_id: &str) -> AwaitedAction {
    AwaitedAction::new(
        operation_id.into(),
        Arc::new(ActionInfo {
            command_digest: DigestInfo::zero_digest(),
            input_root_digest: DigestInfo::zero_digest(),
            timeout: Duration::from_secs(1),
            platform_properties: HashMap::new(),
            priority: 0,
            load_timestamp: SystemTime::UNIX_EPOCH,
            insert_timestamp: SystemTime::UNIX_EPOCH,
            unique_qualifier: ActionUniqueQualifier::Cacheable(ActionUniqueKey {
                instance_name: INSTANCE_NAME.to_string(),
                digest_function: DigestHasherFunc::Sha256,
                digest: DigestInfo::zero_digest(),
            }),
        }),
        MockSystemTime::now().into(),
    )
}

#[nativelink_test]
async fn add_action_smoke_test() -> Result<(), Error> {
    const CLIENT_OPERATION_ID: &str = "my_client_operation_id";
    const WORKER_OPERATION_ID: &str = "my_worker_operation_id";
    static SUBSCRIPTION_MANAGER: Mutex<Option<Arc<RedisSubscriptionManager>>> = Mutex::new(None);
    const SUB_CHANNEL: &str = "sub_channel";

    let worker_awaited_action = make_awaited_action(WORKER_OPERATION_ID);
    let new_awaited_action = {
        let mut new_awaited_action = worker_awaited_action.clone();
        let mut new_state = new_awaited_action.state().as_ref().clone();
        new_state.stage = ActionStage::Executing;
        new_awaited_action.worker_set_state(Arc::new(new_state), MockSystemTime::now().into());
        new_awaited_action
    };

    let worker_operation_id = OperationId::from(WORKER_OPERATION_ID);

    let ft_aggregate_args = vec![
        format!("aa__unique_qualifier__{SCRIPT_VERSION}").into(),
        format!("@unique_qualifier:{{ {INSTANCE_NAME}_SHA256_0000000000000000000000000000000000000000000000000000000000000000_0_c }}").into(),
        "LOAD".into(),
        2.into(),
        "data".into(),
        "version".into(),
        "SORTBY".into(),
        0.into(),
        "WITHCURSOR".into(),
        "COUNT".into(),
        256.into(),
        "MAXIDLE".into(),
        2000.into(),
    ];
    let mocks = Arc::new(MockRedisBackend::new());
    #[expect(
        clippy::string_lit_as_bytes,
        reason = r#"avoids `b"foo".as_slice()`, which is hardly better"#
    )]
    mocks
        .expect(
            MockCommand {
                cmd: Str::from_static("FT.AGGREGATE"),
                subcommand: None,
                args: ft_aggregate_args.clone(),
            },
            Err(RedisError::new(
                RedisErrorKind::NotFound,
                String::new(),
            )),
            None,
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("SUBSCRIBE"),
                subcommand: None,
                args: vec![SUB_CHANNEL.as_bytes().into()],
            },
            Ok(RedisValue::Integer(0)),
            None,
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("FT.CREATE"),
                subcommand: None,
                args: vec![
                    format!("aa__unique_qualifier__{SCRIPT_VERSION}").into(),
                    "ON".into(),
                    "HASH".into(),
                    "PREFIX".into(),
                    1.into(),
                    "aa_".into(),
                    "TEMPORARY".into(),
                    86400.into(),
                    "NOOFFSETS".into(),
                    "NOHL".into(),
                    "NOFIELDS".into(),
                    "NOFREQS".into(),
                    "SCHEMA".into(),
                    "unique_qualifier".into(),
                    "TAG".into(),
                ],
            },
            Ok(RedisValue::Bytes(Bytes::from("data"))),
            None,
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("FT.AGGREGATE"),
                subcommand: None,
                args: ft_aggregate_args.clone(),
            },
            Ok(RedisValue::Array(vec![
                RedisValue::Array(vec![
                    RedisValue::Integer(0),
                ]),
                RedisValue::Integer(0), // Means no more items in cursor.
            ])),
            None,
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("EVALSHA"),
                subcommand: None,
                args: vec![
                    VERSION_SCRIPT_HASH.into(),
                    1.into(),
                    format!("aa_{WORKER_OPERATION_ID}").as_bytes().into(),
                    "0".as_bytes().into(),
                    RedisValue::Bytes(Bytes::from(serde_json::to_string(&worker_awaited_action).unwrap())),
                    "unique_qualifier".as_bytes().into(),
                    format!("{INSTANCE_NAME}_SHA256_0000000000000000000000000000000000000000000000000000000000000000_0_c").as_bytes().into(),
                    "state".as_bytes().into(),
                    "queued".as_bytes().into(),
                    "sort_key".as_bytes().into(),
                    "80000000ffffffff".as_bytes().into(),
                ],
            },
            Ok(RedisValue::Array(vec![RedisValue::Integer(1), RedisValue::Integer(1)])),
            None,
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("PUBLISH"),
                subcommand: None,
                args: vec![
                    SUB_CHANNEL.into(),
                    format!("aa_{WORKER_OPERATION_ID}").into(),
                ],
            },
            Ok(0.into() /* unused */),
            Some(Box::new(|| SUBSCRIPTION_MANAGER.lock().as_ref().unwrap().notify_for_test(format!("aa_{WORKER_OPERATION_ID}")))),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("HSET"),
                subcommand: None,
                args: vec![
                    format!("cid_{CLIENT_OPERATION_ID}").as_bytes().into(),
                    "data".as_bytes().into(),
                    format!("{{\"String\":\"{WORKER_OPERATION_ID}\"}}").as_bytes().into(),
                ],
            },
            Ok(RedisValue::new_ok()),
            None,
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("PUBLISH"),
                subcommand: None,
                args: vec![
                    SUB_CHANNEL.into(),
                    format!("cid_{CLIENT_OPERATION_ID}").into(),
                ],
            },
            Ok(0.into() /* unused */),
            Some(Box::new(|| SUBSCRIPTION_MANAGER.lock().as_ref().unwrap().notify_for_test(format!("aa_{CLIENT_OPERATION_ID}")))),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("HMGET"),
                subcommand: None,
                args: vec![
                    format!("aa_{WORKER_OPERATION_ID}").as_bytes().into(),
                    "version".as_bytes().into(),
                    "data".as_bytes().into(),
                ],
            },
            Ok(RedisValue::Array(vec![
                // Version.
                "1".into(),
                // Data.
                RedisValue::Bytes(Bytes::from(serde_json::to_string(&worker_awaited_action).unwrap())),
            ])),
            None,
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("HMGET"),
                subcommand: None,
                args: vec![
                    format!("cid_{CLIENT_OPERATION_ID}").as_bytes().into(),
                    "version".as_bytes().into(),
                    "data".as_bytes().into(),
                ],
            },
            Ok(RedisValue::Array(vec![
                // Version.
                RedisValue::Null,
                // Data.
                RedisValue::Bytes(Bytes::from(serde_json::to_string(&worker_operation_id).unwrap())),
            ])),
            None,
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("HMGET"),
                subcommand: None,
                args: vec![
                    format!("aa_{WORKER_OPERATION_ID}").as_bytes().into(),
                    "version".as_bytes().into(),
                    "data".as_bytes().into(),
                ],
            },
            Ok(RedisValue::Array(vec![
                // Version.
                "2".into(),
                // Data.
                RedisValue::Bytes(Bytes::from(serde_json::to_string(&new_awaited_action).unwrap())),
            ])),
            None,
        )

        .expect(
            MockCommand {
                cmd: Str::from_static("EVALSHA"),
                subcommand: None,
                args: vec![
                    VERSION_SCRIPT_HASH.into(),
                    1.into(),
                    format!("aa_{WORKER_OPERATION_ID}").as_bytes().into(),
                    "0".as_bytes().into(),
                    RedisValue::Bytes(Bytes::from(serde_json::to_string(&new_awaited_action).unwrap())),
                    "unique_qualifier".as_bytes().into(),
                    format!("{INSTANCE_NAME}_SHA256_0000000000000000000000000000000000000000000000000000000000000000_0_c").as_bytes().into(),
                    "state".as_bytes().into(),
                    "executing".as_bytes().into(),
                    "sort_key".as_bytes().into(),
                    "80000000ffffffff".as_bytes().into(),
                ],
            },
            Ok(RedisValue::Array(vec![RedisValue::Integer(1), RedisValue::Integer(2)])),
            None,
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("PUBLISH"),
                subcommand: None,
                args: vec![
                    SUB_CHANNEL.into(),
                    format!("aa_{WORKER_OPERATION_ID}").into(),
                ],
            },
            Ok(0.into() /* unused */),
            Some(Box::new(|| SUBSCRIPTION_MANAGER.lock().as_ref().unwrap().notify_for_test(format!("aa_{WORKER_OPERATION_ID}")))),
        )
        .expect(
            MockCommand {
                cmd: Str::from_static("HMGET"),
                subcommand: None,
                args: vec![
                    format!("aa_{WORKER_OPERATION_ID}").as_bytes().into(),
                    "version".as_bytes().into(),
                    "data".as_bytes().into(),
                ],
            },
            Ok(RedisValue::Array(vec![
                // Version.
                "2".into(),
                // Data.
                RedisValue::Bytes(Bytes::from(serde_json::to_string(&new_awaited_action).unwrap())),
            ])),
            None,
        )
        ;

    let store = make_redis_store(SUB_CHANNEL, mocks);
    SUBSCRIPTION_MANAGER
        .lock()
        .replace(store.subscription_manager().unwrap());

    let notifier = Arc::new(Notify::new());
    let awaited_action_db = StoreAwaitedActionDb::new(
        store.clone(),
        notifier.clone(),
        MockInstantWrapped::default,
        move || WORKER_OPERATION_ID.into(),
    )
    .unwrap();

    let mut subscription = awaited_action_db
        .add_action(
            CLIENT_OPERATION_ID.into(),
            worker_awaited_action.action_info().clone(),
            Duration::from_secs(60),
        )
        .await
        .unwrap();

    {
        // Check initial change state.
        let changed_awaited_action_res = subscription.changed().await;

        assert_eq!(
            changed_awaited_action_res.unwrap().state().stage,
            ActionStage::Queued
        );
    }

    {
        let get_subscription = awaited_action_db
            .get_awaited_action_by_id(&OperationId::from(CLIENT_OPERATION_ID))
            .await
            .unwrap()
            .unwrap();

        let get_res = get_subscription.borrow().await;

        assert_eq!(get_res.unwrap().state().stage, ActionStage::Executing);
    }

    {
        // Update the action and check the new state.
        let (changed_awaited_action_res, update_res) = tokio::join!(
            subscription.changed(),
            awaited_action_db.update_awaited_action(new_awaited_action.clone())
        );
        assert_eq!(update_res, Ok(()));

        assert_eq!(
            changed_awaited_action_res.unwrap().state().stage,
            ActionStage::Executing
        );
    }

    Ok(())
}

#[nativelink_test]
async fn test_multiple_clients_subscribe_to_same_action() -> Result<(), Error> {
    const CLIENT_OPERATION_ID_1: &str = "client_operation_id_1";
    const CLIENT_OPERATION_ID_2: &str = "client_operation_id_2";
    const CLIENT_OPERATION_ID_3: &str = "client_operation_id_3";
    const WORKER_OPERATION_ID_1: &str = "worker_operation_id_1";
    const WORKER_OPERATION_ID_2: &str = "worker_operation_id_2";
    const SUB_CHANNEL: &str = "sub_channel";

    let action_info = Arc::new(ActionInfo {
        command_digest: DigestInfo::zero_digest(),
        input_root_digest: DigestInfo::zero_digest(),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Cacheable(ActionUniqueKey {
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: DigestInfo::zero_digest(),
        }),
    });

    let mocks = Arc::new(FakeRedisBackend::new());
    let store = make_redis_store(SUB_CHANNEL, mocks.clone());
    mocks.set_subscription_manager(store.subscription_manager().unwrap());

    let notifier = Arc::new(Notify::new());
    let worker_operation_id = Arc::new(Mutex::new(WORKER_OPERATION_ID_1));
    let worker_operation_id_clone = worker_operation_id.clone();
    let awaited_action_db = StoreAwaitedActionDb::new(
        store.clone(),
        notifier.clone(),
        MockInstantWrapped::default,
        move || worker_operation_id_clone.lock().clone().into(),
    )
    .unwrap();

    let task_change_notify = Arc::new(Notify::new());
    let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
        &SimpleSpec::default(),
        awaited_action_db,
        || async move {},
        task_change_notify,
        MockInstantWrapped::default,
        None,
    );

    // First client adds the action
    let mut subscription1 = scheduler
        .add_action(CLIENT_OPERATION_ID_1.into(), action_info.clone())
        .await
        .unwrap();

    // Second client tries to add the same action (should subscribe to existing one)
    let _subscription2 = scheduler
        .add_action(CLIENT_OPERATION_ID_2.into(), action_info.clone())
        .await
        .unwrap();

    // Second client should be able to get the action by its client_operation_id
    let get_subscription = scheduler
        .filter_operations(OperationFilter {
            client_operation_id: Some(OperationId::from(CLIENT_OPERATION_ID_2)),
            ..Default::default()
        })
        .await
        .expect("Second client should be able to get action by its client_operation_id")
        .next()
        .await
        .expect("Second client should be able to get action by its client_operation_id");

    let (state, _metadata) = get_subscription
        .as_state()
        .await
        .expect("Unable to get state of operation");
    assert_eq!(state.stage, ActionStage::Queued);

    // Now create a worker and check that it is only allocated the job once.
    let worker_id = WorkerId("worker_id".to_string());
    let mut rx_from_worker =
        setup_new_worker(&scheduler, worker_id.clone(), PlatformProperties::default()).await?;

    // Try to ensure we schedule to the worker.
    scheduler.do_try_match_for_test().await?;

    {
        // Worker should have been sent an execute command.
        let expected_msg_for_worker = UpdateForWorker {
            update: Some(update_for_worker::Update::StartAction(StartExecute {
                execute_request: Some(ExecuteRequest {
                    instance_name: INSTANCE_NAME.to_string(),
                    action_digest: Some(DigestInfo::zero_digest().into()),
                    digest_function: digest_function::Value::Sha256.into(),
                    ..Default::default()
                }),
                operation_id: "Unknown Generated internally".to_string(),
                queued_timestamp: Some(SystemTime::UNIX_EPOCH.into()),
                platform: Some(Platform::default()),
                worker_id: worker_id.clone().into(),
            })),
        };
        let msg_for_worker = rx_from_worker.recv().await.unwrap();
        // Operation ID is random so we ignore it.
        assert!(update_eq(expected_msg_for_worker, msg_for_worker, true));
    }

    let (state, _metadata) = subscription1
        .changed()
        .await
        .expect("No update to subscription");
    assert_eq!(state.stage, ActionStage::Executing);
    let (state, _metadata) = get_subscription
        .as_state()
        .await
        .expect("Unable to get second operation");
    assert_eq!(state.stage, ActionStage::Executing);

    // Immediately try to schedule again to check we don't schedule the job
    // again now that it's executing.
    scheduler.do_try_match_for_test().await?;

    // The worker shouldn't be allocated the job again.
    tokio::select! {
        () = tokio::time::sleep(Duration::from_secs(1)) => {}
        _ = rx_from_worker.recv() => {
            panic!("Worker was allocated another job");
        }
    }

    // The worker goes away without completing the task, so the action goes back
    // to queued.
    drop(rx_from_worker);
    scheduler.remove_worker(&worker_id).await?;

    let (state, _metadata) = subscription1
        .changed()
        .await
        .expect("No update to subscription");
    assert_eq!(state.stage, ActionStage::Queued);

    // Create and drop the worker three times to cause the job to complete with
    // a failure.
    for _ in 0..3 {
        let rx_from_worker =
            setup_new_worker(&scheduler, worker_id.clone(), PlatformProperties::default()).await?;
        scheduler.do_try_match_for_test().await?;
        drop(rx_from_worker);
        scheduler.remove_worker(&worker_id).await?;
    }

    // Update the operation ID for the new subscription.
    *worker_operation_id.lock() = WORKER_OPERATION_ID_2;

    // Subscribe with a new operation ID after all that and we should be queued.
    let subscription3 = scheduler
        .add_action(CLIENT_OPERATION_ID_3.into(), action_info.clone())
        .await
        .unwrap();

    let (state, _metadata) = subscription3
        .as_state()
        .await
        .expect("Unable to get state of operation");
    assert_eq!(state.stage, ActionStage::Queued);

    Ok(())
}

#[nativelink_test]
async fn test_outdated_version() -> Result<(), Error> {
    const CLIENT_OPERATION_ID: &str = "outdated_operation_id";
    let worker_operation_id = Arc::new(Mutex::new(CLIENT_OPERATION_ID));
    let worker_operation_id_clone = worker_operation_id.clone();

    let mocks = Arc::new(FakeRedisBackend::new());

    let store = make_redis_store("sub_channel", mocks);
    let notifier = Arc::new(Notify::new());

    let awaited_action_db = StoreAwaitedActionDb::new(
        store.clone(),
        notifier.clone(),
        MockInstantWrapped::default,
        move || worker_operation_id_clone.lock().clone().into(),
    )
    .unwrap();

    let worker_awaited_action = make_awaited_action("WORKER_OPERATION_ID");

    let update_res = awaited_action_db
        .update_awaited_action(worker_awaited_action.clone())
        .await;
    assert_eq!(update_res, Ok(()));

    let update_res2 = awaited_action_db
        .update_awaited_action(worker_awaited_action.clone())
        .await;
    assert!(update_res2.is_err());
    assert_eq!(
        update_res2.unwrap_err(),
        Error::new(Code::Aborted, "Could not update AwaitedAction because the version did not match for WORKER_OPERATION_ID".into())
    );

    Ok(())
}
