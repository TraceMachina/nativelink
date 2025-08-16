use nativelink_config::stores::RedisSpec;
use nativelink_store::redis_store::RedisStore;
use nativelink_util::store_trait::StoreLike;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let spec = RedisSpec {
        addresses: vec!["redis://127.0.0.1:6379/".to_string()],
        ..Default::default()
    };
    let store = RedisStore::new(spec)?;
    store.has("1234").await?;
    Ok(())
}
