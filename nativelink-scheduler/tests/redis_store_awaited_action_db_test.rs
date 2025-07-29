// Copyright 2024 The NativeLink Authorsr All rights reserved.
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

use core::time::Duration;
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
use mock_instant::global::SystemTime as MockSystemTime;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::awaited_action_db::{
    AwaitedAction, AwaitedActionDb, AwaitedActionSubscriber,
};
use nativelink_scheduler::store_awaited_action_db::StoreAwaitedActionDb;
use nativelink_store::redis_store::{RedisStore, RedisSubscriptionManager};
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionUniqueKey, ActionUniqueQualifier, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::store_trait::{SchedulerStore, SchedulerSubscriptionManager};
use parking_lot::Mutex;
use pretty_assertions::assert_eq;
use tokio::sync::Notify;

const INSTANCE_NAME: &str = "instance_name";
const TEMP_UUID: &str = "550e8400-e29b-41d4-a716-446655440000";
const SCRIPT_VERSION: &str = "3e762c15";
const VERSION_SCRIPT_HASH: &str = "fdf1152fd21705c8763752809b86b55c5d4511ce";
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

#[nativelink_test]
async fn add_action_smoke_test() -> Result<(), Error> {
    const CLIENT_OPERATION_ID: &str = "my_client_operation_id";
    const WORKER_OPERATION_ID: &str = "my_worker_operation_id";
    static SUBSCRIPTION_MANAGER: Mutex<Option<Arc<RedisSubscriptionManager>>> = Mutex::new(None);
    const SUB_CHANNEL: &str = "sub_channel";

    let worker_awaited_action = AwaitedAction::new(
        WORKER_OPERATION_ID.into(),
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
    );
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
            Ok(1.into() /* New version */),
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
            Ok(2.into() /* New version */),
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

    let store = {
        let mut builder = Builder::default_centralized();
        let mocks = Arc::clone(&mocks);
        builder.set_config(RedisConfig {
            mocks: Some(mocks),
            ..Default::default()
        });
        let (client_pool, subscriber_client) = make_clients(builder);
        Arc::new(
            RedisStore::new_from_builder_and_parts(
                client_pool,
                subscriber_client,
                Some(SUB_CHANNEL.into()),
                mock_uuid_generator,
                String::new(),
                4064,
                MAX_CHUNK_UPLOADS_PER_UPDATE,
                SCAN_COUNT,
            )
            .unwrap(),
        )
    };
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
    const WORKER_OPERATION_ID: &str = "worker_operation_id";
    static SUBSCRIPTION_MANAGER: Mutex<Option<Arc<RedisSubscriptionManager>>> = Mutex::new(None);
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

    let worker_awaited_action = AwaitedAction::new(
        WORKER_OPERATION_ID.into(),
        action_info.clone(),
        MockSystemTime::now().into(),
    );

    let worker_awaited_action_with_keepalive = {
        let mut action_json: serde_json::Value =
            serde_json::from_str(&serde_json::to_string(&worker_awaited_action).unwrap()).unwrap();
        action_json["version"] = serde_json::json!(1);
        action_json["last_client_keepalive_timestamp"] =
            serde_json::json!({"secs_since_epoch": 0, "nanos_since_epoch": 0});
        serde_json::to_string(&action_json).unwrap()
    };

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
        // First client tries to subscribe - action doesn't exist
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
        // First client creates the action
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
            Ok(1.into() /* New version */),
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
                    format!("cid_{CLIENT_OPERATION_ID_1}").as_bytes().into(),
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
                    format!("cid_{CLIENT_OPERATION_ID_1}").into(),
                ],
            },
            Ok(0.into() /* unused */),
            Some(Box::new(|| SUBSCRIPTION_MANAGER.lock().as_ref().unwrap().notify_for_test(format!("aa_{CLIENT_OPERATION_ID_1}")))),
        )
        // Second client tries to subscribe - action exists
        .expect(
            MockCommand {
                cmd: Str::from_static("FT.AGGREGATE"),
                subcommand: None,
                args: ft_aggregate_args.clone(),
            },
            Ok(RedisValue::Array(vec![
                RedisValue::Array(vec![
                    RedisValue::Integer(1),
                    RedisValue::Array(vec![
                        RedisValue::Bytes(Bytes::from("data")),
                        RedisValue::Bytes(Bytes::from(serde_json::to_string(&worker_awaited_action).unwrap())),
                        RedisValue::Bytes(Bytes::from("version")),
                        RedisValue::Bytes(Bytes::from("1")),
                    ]),
                ]),
                RedisValue::Integer(0), // Means no more items in cursor.
            ])),
            None,
        )
        // Update keep alive for the existing action
        .expect(
            MockCommand {
                cmd: Str::from_static("EVALSHA"),
                subcommand: None,
                args: vec![
                    VERSION_SCRIPT_HASH.into(),
                    1.into(),
                    format!("aa_{WORKER_OPERATION_ID}").as_bytes().into(),
                    "1".as_bytes().into(),
                    RedisValue::Bytes(Bytes::from(worker_awaited_action_with_keepalive.clone())),
                    "unique_qualifier".as_bytes().into(),
                    format!("{INSTANCE_NAME}_SHA256_0000000000000000000000000000000000000000000000000000000000000000_0_c").as_bytes().into(),
                    "state".as_bytes().into(),
                    "queued".as_bytes().into(),
                    "sort_key".as_bytes().into(),
                    "80000000ffffffff".as_bytes().into(),
                ],
            },
            Ok(2.into() /* New version */),
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
        // THE MISSING PIECE: Second client should create mapping from CLIENT_OPERATION_ID_2 to WORKER_OPERATION_ID
        .expect(
            MockCommand {
                cmd: Str::from_static("HSET"),
                subcommand: None,
                args: vec![
                    format!("cid_{CLIENT_OPERATION_ID_2}").as_bytes().into(),
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
                    format!("cid_{CLIENT_OPERATION_ID_2}").into(),
                ],
            },
            Ok(0.into() /* unused */),
            Some(Box::new(|| SUBSCRIPTION_MANAGER.lock().as_ref().unwrap().notify_for_test(format!("aa_{CLIENT_OPERATION_ID_2}")))),
        )
        // Client 2 tries to get by its client_operation_id
        .expect(
            MockCommand {
                cmd: Str::from_static("HMGET"),
                subcommand: None,
                args: vec![
                    format!("cid_{CLIENT_OPERATION_ID_2}").as_bytes().into(),
                    "version".as_bytes().into(),
                    "data".as_bytes().into(),
                ],
            },
            Ok(RedisValue::Array(vec![
                // Version.
                RedisValue::Null,
                // Data.
                RedisValue::Bytes(Bytes::from(format!("{{\"String\":\"{WORKER_OPERATION_ID}\"}}"))),
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
                RedisValue::Bytes(Bytes::from({
                    let mut action_json: serde_json::Value = serde_json::from_str(&serde_json::to_string(&worker_awaited_action).unwrap()).unwrap();
                    action_json["version"] = serde_json::json!(2);
                    action_json["last_client_keepalive_timestamp"] = serde_json::json!({"secs_since_epoch": 0, "nanos_since_epoch": 0});
                    serde_json::to_string(&action_json).unwrap()
                })),
            ])),
            None,
        );

    let store = {
        let mut builder = Builder::default_centralized();
        let mocks = Arc::clone(&mocks);
        builder.set_config(RedisConfig {
            mocks: Some(mocks),
            ..Default::default()
        });
        let (client_pool, subscriber_client) = make_clients(builder);
        Arc::new(
            RedisStore::new_from_builder_and_parts(
                client_pool,
                subscriber_client,
                Some(SUB_CHANNEL.into()),
                mock_uuid_generator,
                String::new(),
                4064,
                MAX_CHUNK_UPLOADS_PER_UPDATE,
                SCAN_COUNT,
            )
            .unwrap(),
        )
    };
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

    // First client adds the action
    let _subscription1 = awaited_action_db
        .add_action(CLIENT_OPERATION_ID_1.into(), action_info.clone())
        .await
        .unwrap();

    // Second client tries to add the same action (should subscribe to existing one)
    let _subscription2 = awaited_action_db
        .add_action(CLIENT_OPERATION_ID_2.into(), action_info.clone())
        .await
        .unwrap();

    // Second client should be able to get the action by its client_operation_id
    let get_subscription = awaited_action_db
        .get_awaited_action_by_id(&OperationId::from(CLIENT_OPERATION_ID_2))
        .await
        .unwrap()
        .expect("Second client should be able to get action by its client_operation_id");

    let get_res = get_subscription.borrow().await?;
    assert_eq!(get_res.state().stage, ActionStage::Queued);
    assert_eq!(
        get_res.operation_id(),
        &OperationId::from(WORKER_OPERATION_ID)
    );

    Ok(())
}
