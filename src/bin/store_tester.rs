use std::borrow::Cow;

use bytes::Bytes;
use nativelink_config::stores::RedisSpec;
use nativelink_store::redis_store::RedisStore;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::store_trait::{StoreKey, StoreLike, UploadSizeInfo};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
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
    Ok(())
}
