use std::{ops::{Bound, RangeBounds}, pin::Pin};

use nativelink_util::{buf_channel::DropCloserReadHalf, store_trait::{StoreDriver, StoreKey, StoreLike}};
use nativelink_error::Error;
use serde::{Deserialize, Serialize};
use tonic::async_trait;

#[async_trait]
pub trait SchedulerStore: StoreDriver + StoreLike {
    async fn remove(
        &self,
        _key: StoreKey<'_>,
    ) -> Result<(), Error>;

    async fn update_if(
        &self,
        key: StoreKey<'_>,
        data: DropCloserReadHalf,
        old_version: u64,
        new_version: u64
    ) -> Result<(), Error>;

    async fn list_prefix(
        self: Pin<&Self>,
        _prefix: StoreKey<'_>,
        _range: (Bound<StoreKey<'_>>, Bound<StoreKey<'_>>),
        _handler: &mut (dyn for<'a> FnMut(&'a StoreKey) -> bool + Send + Sync + '_),
    ) -> Result<usize, Error>;

    async fn subscribe<'de, T: Deserialize<'de>>(
        &self,
        _key: StoreKey<'_>,
    ) -> Result<tokio::sync::broadcast::Receiver<T>, Error>;

    async fn publish_channel<T: Serialize + Send>(
        &self,
        key: StoreKey<'_>,
        message: T
    ) -> Result<(), Error>;

}
