use std::borrow::Cow;

use bytes::Bytes;
use nativelink_config::stores::RedisSpec;
use nativelink_error::Error;
use nativelink_store::redis_store::RedisStore;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::store_trait::{
    SchedulerCurrentVersionProvider, SchedulerStore, SchedulerStoreDataProvider,
    SchedulerStoreKeyProvider, StoreKey, StoreLike, TrueValue, UploadSizeInfo,
};
use nativelink_util::telemetry::init_tracing;

// Define test structures that implement the scheduler traits
#[derive(Debug, Clone, PartialEq)]
struct TestSchedulerData {
    key: String,
    content: String,
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

impl SchedulerCurrentVersionProvider for TestSchedulerData {
    fn current_version(&self) -> i64 {
        self.version
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The OTLP exporters need to run in a Tokio context.
    tokio::spawn(async { init_tracing() })
        .await?
        .expect("Init tracing should work");

    let spec = RedisSpec {
        addresses: vec!["redis://127.0.0.1:6379/".to_string()],
        connection_timeout_ms: 1000,
        ..Default::default()
    };
    let store = RedisStore::new(spec)?;
    let res = store.has("1234").await?;
    assert_eq!(res, None);
    let mut results = (0..100).into_iter().map(|_| None).collect::<Vec<_>>();
    let (mut tx, rx) = make_buf_channel_pair();
    tx.send(Bytes::from_static(b"12345")).await?;
    tx.send_eof()?;
    let one_key: StoreKey = "1".to_string().into();
    store
        .update(one_key, rx, UploadSizeInfo::ExactSize(5))
        .await?;
    store
        .has_with_results(
            &(0..100)
                .into_iter()
                .map(|i| StoreKey::Str(Cow::Owned(i.to_string())))
                .collect::<Vec<_>>(),
            &mut results,
        )
        .await?;
    println!("results: {:?}", results);

    let data = TestSchedulerData {
        key: "test:scheduler_key_1".to_string(),
        content: "Test scheduler data #1".to_string(),
        version: 0,
    };

    store.update_data(data.clone()).await?;
    store.update_data(data.clone()).await?;

    Ok(())
}
