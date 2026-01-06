use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;
use std::borrow::Cow;
use std::env;
use std::sync::{Arc, RwLock};

use bytes::Bytes;
use clap::{Parser, ValueEnum, command};
use futures::TryStreamExt;
use nativelink_config::stores::{RedisMode, RedisSpec};
use nativelink_error::{Code, Error, ResultExt};
use nativelink_store::redis_store::RedisStore;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::store_trait::{
    SchedulerCurrentVersionProvider, SchedulerIndexProvider, SchedulerStore,
    SchedulerStoreDataProvider, SchedulerStoreDecodeTo, SchedulerStoreKeyProvider, StoreKey,
    StoreLike, TrueValue, UploadSizeInfo,
};
use nativelink_util::telemetry::init_tracing;
use nativelink_util::{background_spawn, spawn};
use rand::Rng;
use redis::aio::ConnectionManager;
use tokio::time::sleep;
use tracing::{error, info};

// Define test structures that implement the scheduler traits
#[derive(Debug, Clone, PartialEq)]
struct TestSchedulerData {
    key: String,
    content: String,
    version: i64,
}

#[derive(Debug)]
struct TestSchedulerReturn {
    version: i64,
}

impl SchedulerStoreKeyProvider for TestSchedulerData {
    type Versioned = TrueValue; // Using versioned storage

    fn get_key(&self) -> StoreKey<'static> {
        StoreKey::Str(Cow::Owned(self.key.clone()))
    }
}

impl SchedulerStoreDataProvider for TestSchedulerData {
    fn try_into_bytes(self) -> Result<Bytes, Error> {
        Ok(Bytes::from(self.content.into_bytes()))
    }

    fn get_indexes(&self) -> Result<Vec<(&'static str, Bytes)>, Error> {
        // Add some test indexes - need to use 'static strings
        Ok(vec![
            ("test_index", Bytes::from("test_value")),
            (
                "content_prefix",
                Bytes::from(self.content.chars().take(10).collect::<String>()),
            ),
        ])
    }
}

impl SchedulerStoreDecodeTo for TestSchedulerData {
    type DecodeOutput = TestSchedulerReturn;

    fn decode(version: i64, _data: Bytes) -> Result<Self::DecodeOutput, Error> {
        Ok(TestSchedulerReturn { version })
    }
}

impl SchedulerCurrentVersionProvider for TestSchedulerData {
    fn current_version(&self) -> i64 {
        self.version
    }
}

struct SearchByContentPrefix {
    prefix: String,
}

impl SchedulerIndexProvider for SearchByContentPrefix {
    const KEY_PREFIX: &'static str = "test:";
    const INDEX_NAME: &'static str = "content_prefix";
    type Versioned = TrueValue;

    fn index_value(&self) -> Cow<'_, str> {
        Cow::Borrowed(&self.prefix)
    }
}

impl SchedulerStoreKeyProvider for SearchByContentPrefix {
    type Versioned = TrueValue;

    fn get_key(&self) -> StoreKey<'static> {
        StoreKey::Str(Cow::Owned("dummy_key".to_string()))
    }
}

impl SchedulerStoreDecodeTo for SearchByContentPrefix {
    type DecodeOutput = TestSchedulerReturn;

    fn decode(version: i64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
        TestSchedulerData::decode(version, data)
    }
}

const MAX_KEY: u16 = 1024;

/// Wrapper type for CLI parsing since we can't implement foreign traits on foreign types.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum RedisModeArg {
    Cluster,
    Sentinel,
    #[default]
    Standard,
}

impl From<RedisModeArg> for RedisMode {
    fn from(arg: RedisModeArg) -> Self {
        match arg {
            RedisModeArg::Standard => RedisMode::Standard,
            RedisModeArg::Sentinel => RedisMode::Sentinel,
            RedisModeArg::Cluster => RedisMode::Cluster,
        }
    }
}

fn random_key() -> StoreKey<'static> {
    let key = rand::rng().random_range(0..MAX_KEY);
    StoreKey::new_str(&key.to_string()).into_owned()
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum TestMode {
    #[default]
    Random,
    Sequential,
}

#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    #[arg(value_enum, short, long, default_value_t)]
    redis_mode: RedisModeArg,

    #[arg(value_enum, short, long, default_value_t)]
    mode: TestMode,
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    let args = Args::parse();
    let redis_mode: RedisMode = args.redis_mode.into();

    let failed = Arc::new(RwLock::new(false));
    let redis_host = env::var("REDIS_HOST").unwrap_or_else(|_| "127.0.0.1".to_string());
    let max_client_permits = env::var("MAX_REDIS_PERMITS")
        .unwrap_or_else(|_| "100".to_string())
        .parse()?;
    let max_loops: usize = env::var("MAX_LOOPS")
        .unwrap_or_else(|_| "2000000".to_string())
        .parse()?;

    #[expect(
        clippy::disallowed_methods,
        reason = "`We need `tokio::runtime::Runtime::block_on` so we can get errors _after_ threads finished"
    )]
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            // The OTLP exporters need to run in a Tokio context.
            spawn!("init tracing", async { init_tracing() })
                .await?
                .expect("Init tracing should work");

            let redis_port = match redis_mode {
                RedisMode::Standard => 6379,
                RedisMode::Sentinel => 26379,
                RedisMode::Cluster => 36379,
            };
            let spec = RedisSpec {
                addresses: vec![format!("redis://{redis_host}:{redis_port}/")],
                connection_timeout_ms: 1000,
                max_client_permits,
                mode: redis_mode,
                ..Default::default()
            };
            let store = match spec.mode {
                RedisMode::Standard | RedisMode::Sentinel => RedisStore::new_standard(spec).await?,
                RedisMode::Cluster => {
                    unimplemented!("Cluster has different return type");
                }
            };

            let mut count = 0;
            let in_flight = Arc::new(AtomicUsize::new(0));

            loop {
                if count % 1000 == 0 {
                    info!(
                        "Loop count {count}. In flight: {}",
                        in_flight.load(Ordering::Relaxed)
                    );
                    if *failed.read().unwrap() {
                        return Err(Error::new(
                            Code::Internal,
                            "Failed in redis_store_tester".to_string(),
                        ));
                    }
                }
                if count == max_loops {
                    loop {
                        let remaining = in_flight.load(Ordering::Relaxed);
                        if remaining == 0 {
                            return Ok(());
                        }
                        info!(remaining, "Remaining");
                        sleep(Duration::from_secs(1)).await;
                    }
                }
                count += 1;
                in_flight.fetch_add(1, Ordering::Relaxed);

                let store_clone = store.clone();
                let local_fail = failed.clone();
                let local_in_flight = in_flight.clone();

                let max_action_value = 7;
                let action_value = match args.mode {
                    TestMode::Random => rand::rng().random_range(0..max_action_value),
                    TestMode::Sequential => count % max_action_value,
                };

                background_spawn!("action", async move {
                    async fn run_action(
                        action_value: usize,
                        store_clone: Arc<RedisStore<ConnectionManager>>,
                    ) -> Result<(), Error> {
                        match action_value {
                            0 => {
                                store_clone.has(random_key()).await?;
                            }
                            1 => {
                                let (mut tx, rx) = make_buf_channel_pair();
                                tx.send(Bytes::from_static(b"12345")).await?;
                                tx.send_eof()?;
                                store_clone
                                    .update(random_key(), rx, UploadSizeInfo::ExactSize(5))
                                    .await?;
                            }
                            2 => {
                                let mut results = (0..MAX_KEY).map(|_| None).collect::<Vec<_>>();

                                store_clone
                                    .has_with_results(
                                        &(0..MAX_KEY)
                                            .map(|i| StoreKey::Str(Cow::Owned(i.to_string())))
                                            .collect::<Vec<_>>(),
                                        &mut results,
                                    )
                                    .await?;
                            }
                            3 => {
                                store_clone
                                    .update_oneshot(random_key(), Bytes::from_static(b"1234"))
                                    .await?;
                            }
                            4 => {
                                let res = store_clone
                                    .list(.., |_key| true)
                                    .await
                                    .err_tip(|| "In list")?;
                                info!(%res, "end list");
                            }
                            5 => {
                                let search_provider = SearchByContentPrefix {
                                    prefix: "Searchable".to_string(),
                                };
                                for i in 0..5 {
                                    let data = TestSchedulerData {
                                        key: format!("test:search_key_{i}"),
                                        content: format!("Searchable content #{i}"),
                                        version: 0,
                                    };

                                    store_clone.update_data(data).await?;
                                }
                                let search_results: Vec<_> = store_clone
                                    .search_by_index_prefix(search_provider)
                                    .await?
                                    .try_collect()
                                    .await?;
                                info!(?search_results, "search results");
                            }
                            _ => {
                                let mut data = TestSchedulerData {
                                    key: "test:scheduler_key_1".to_string(),
                                    content: "Test scheduler data #1".to_string(),
                                    version: 0,
                                };

                                let res = store_clone.get_and_decode(data.clone()).await?;
                                if let Some(existing_data) = res {
                                    data.version = existing_data.version + 1;
                                }

                                store_clone.update_data(data).await?;
                            }
                        }
                        Ok(())
                    }
                    match run_action(action_value, store_clone).await {
                        Ok(()) => {}
                        Err(e) => {
                            error!(?e, "Error!");
                            *local_fail.write().unwrap() = true;
                        }
                    }
                    local_in_flight.fetch_sub(1, Ordering::Relaxed);
                });
            }
        })
        .unwrap();
    if *failed.read().unwrap() {
        return Err(Error::new(Code::Internal, "Failed in redis_store_tester".to_string()).into());
    }
    Ok(())
}
