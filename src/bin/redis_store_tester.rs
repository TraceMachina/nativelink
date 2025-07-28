use std::borrow::Cow;

use bytes::Bytes;
use nativelink_config::stores::RedisSpec;
use nativelink_error::Error;
use nativelink_store::redis_store::RedisStore;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::store_trait::{
    SchedulerCurrentVersionProvider, SchedulerStore, SchedulerStoreDataProvider,
    SchedulerStoreDecodeTo, SchedulerStoreKeyProvider, StoreKey, StoreLike, TrueValue,
    UploadSizeInfo,
};
use nativelink_util::telemetry::init_tracing;
use nativelink_util::{background_spawn, spawn};
use rand::Rng;
use tracing::info;

// Define test structures that implement the scheduler traits
#[derive(Debug, Clone, PartialEq)]
struct TestSchedulerData {
    key: String,
    content: String,
    version: i64,
}

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

const MAX_KEY: u16 = 1024;

fn random_key() -> StoreKey<'static> {
    let key = rand::rng().random_range(0..MAX_KEY);
    StoreKey::new_str(&key.to_string()).into_owned()
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn core::error::Error>> {
    // The OTLP exporters need to run in a Tokio context.
    spawn!("init tracing", async { init_tracing() })
        .await?
        .expect("Init tracing should work");

    let spec = RedisSpec {
        addresses: vec!["redis://127.0.0.1:6379/".to_string()],
        connection_timeout_ms: 1000,
        ..Default::default()
    };
    let store = RedisStore::new(spec)?;
    let mut count = 0;

    loop {
        if count % 1000 == 0 {
            info!("Loop count {count}");
        }
        count += 1;

        let store_clone = store.clone();
        background_spawn!("action", async move {
            let action_value = rand::rng().random_range(0..5);
            match action_value {
                0 => {
                    store_clone.has(random_key()).await.unwrap();
                }
                1 => {
                    let (mut tx, rx) = make_buf_channel_pair();
                    tx.send(Bytes::from_static(b"12345")).await.unwrap();
                    tx.send_eof().unwrap();
                    store_clone
                        .update(random_key(), rx, UploadSizeInfo::ExactSize(5))
                        .await
                        .unwrap();
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
                        .await
                        .unwrap();
                }
                3 => {
                    store_clone
                        .update_oneshot(random_key(), Bytes::from_static(b"1234"))
                        .await
                        .unwrap();
                }
                _ => {
                    let mut data = TestSchedulerData {
                        key: "test:scheduler_key_1".to_string(),
                        content: "Test scheduler data #1".to_string(),
                        version: 0,
                    };

                    let res = store_clone.get_and_decode(data.clone()).await.unwrap();
                    if let Some(existing_data) = res {
                        data.version = existing_data.version + 1;
                    }

                    store_clone.update_data(data).await.unwrap();
                }
            }
        });
    }
}
