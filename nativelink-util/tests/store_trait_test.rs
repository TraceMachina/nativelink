use core::pin::Pin;
use std::sync::Arc;

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::default_health_status_indicator;
use nativelink_util::health_utils::HealthStatusIndicator;
use nativelink_util::store_trait::{
    RemoveCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use tonic::async_trait;

#[derive(Debug, MetricsComponent)]
struct FakeStore {}

#[async_trait]
#[allow(clippy::todo)]
impl StoreDriver for FakeStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

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
    ) -> Result<u64, Error> {
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

    fn register_remove_callback(self: Arc<Self>, _callback: RemoveCallback) -> Result<(), Error> {
        todo!();
    }
}

default_health_status_indicator!(FakeStore);

/// A wrapper `StoreDriver` whose `inner_store()` forwards to its wrapped
/// `Store`, unlike most production wrappers (e.g. `CompressionStore`,
/// `VerifyStore`) which return `self`. Used to exercise the distinction
/// between `Store::downcast_ref` (recursive, follows `inner_store()`) and
/// `Store::downcast_ref_immediate` (checks only the immediate driver).
#[derive(Debug, MetricsComponent)]
struct ForwardingWrapperStore {
    inner: Store,
}

#[async_trait]
#[allow(clippy::todo)]
impl StoreDriver for ForwardingWrapperStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

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
    ) -> Result<u64, Error> {
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

    fn inner_store(&self, digest: Option<StoreKey>) -> &dyn StoreDriver {
        self.inner.inner_store(digest)
    }

    fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_remove_callback(
        self: Arc<Self>,
        _callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        todo!();
    }
}

default_health_status_indicator!(ForwardingWrapperStore);

#[nativelink_test]
async fn downcast_ref_immediate_only_matches_outer_driver() -> Result<(), Error> {
    let store = Store::new(Arc::new(ForwardingWrapperStore {
        inner: Store::new(Arc::new(FakeStore {})),
    }));

    assert!(
        store
            .downcast_ref_immediate::<ForwardingWrapperStore>()
            .is_some()
    );
    assert!(store.downcast_ref_immediate::<FakeStore>().is_none());
    assert!(store.downcast_ref::<FakeStore>(None).is_some());

    Ok(())
}

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
