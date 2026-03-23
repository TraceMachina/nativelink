use core::pin::Pin;
use std::sync::Arc;

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::default_health_status_indicator;
use nativelink_util::health_utils::HealthStatusIndicator;
use nativelink_util::store_trait::{
    ItemCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use tonic::async_trait;

#[derive(Debug, MetricsComponent)]
struct FakeStore {}

#[async_trait]
#[allow(clippy::todo)]
impl StoreDriver for FakeStore {
    async fn has_with_results(
        self: Pin<&Self>,
        _keys: &[StoreKey<'_>],
        _results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        todo!();
    }

    async fn update(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        _reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        todo!();
    }

    async fn get_part(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        _writer: &mut DropCloserWriteHalf,
        _offset: u64,
        _length: Option<u64>,
    ) -> Result<(), Error> {
        todo!();
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_item_callback(
        self: Arc<Self>,
        _callback: Arc<dyn ItemCallback>,
    ) -> Result<(), Error> {
        todo!();
    }
}

default_health_status_indicator!(FakeStore);

#[nativelink_test]
async fn fast_has_with_results() -> Result<(), Error> {
    let store = Store::new(Arc::new(FakeStore {}));
    let mut results: [Option<u64>; 0] = [];
    store.has_with_results(&[], &mut results).await?;

    Ok(())
}

#[nativelink_test]
async fn fast_has_many() -> Result<(), Error> {
    let store = Store::new(Arc::new(FakeStore {}));
    let res = store.has_many(&[]).await?;
    assert!(res.is_empty());

    Ok(())
}
