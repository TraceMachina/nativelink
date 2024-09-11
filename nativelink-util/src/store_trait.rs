// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use std::borrow::{BorrowMut, Cow};
use std::collections::hash_map::DefaultHasher as StdHasher;
use std::hash::{Hash, Hasher};
use std::ops::{Bound, Deref, RangeBounds};
use std::pin::Pin;
use std::ptr::addr_eq;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::future::{select, Either};
use futures::{join, try_join, Future, FutureExt, Stream};
use nativelink_error::{error_if, make_err, Code, Error, ResultExt};
use nativelink_metric::MetricsComponent;
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncSeekExt;
use tokio::time::timeout;

use crate::buf_channel::{make_buf_channel_pair, DropCloserReadHalf, DropCloserWriteHalf};
use crate::common::DigestInfo;
use crate::default_store_key_subscribe::default_store_key_subscribe;
use crate::digest_hasher::{default_digest_hasher_func, DigestHasher, DigestHasherFunc};
use crate::fs::{self, idle_file_descriptor_timeout};
use crate::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};

static DEFAULT_DIGEST_SIZE_HEALTH_CHECK: OnceLock<usize> = OnceLock::new();
/// Default digest size for health check data. Any change in this value
/// changes the default contract. `GlobalConfig` should be updated to reflect
/// changes in this value.
pub const DEFAULT_DIGEST_SIZE_HEALTH_CHECK_CFG: usize = 1024 * 1024;

// Get the default digest size for health check data, if value is unset a system wide default is used.
pub fn default_digest_size_health_check() -> usize {
    *DEFAULT_DIGEST_SIZE_HEALTH_CHECK.get_or_init(|| DEFAULT_DIGEST_SIZE_HEALTH_CHECK_CFG)
}

/// Set the default digest size for health check data, this should be called once.
pub fn set_default_digest_size_health_check(size: usize) -> Result<(), Error> {
    DEFAULT_DIGEST_SIZE_HEALTH_CHECK.set(size).map_err(|_| {
        make_err!(
            Code::Internal,
            "set_default_digest_size_health_check already set"
        )
    })
}

#[derive(Debug, PartialEq, Copy, Clone, Serialize, Deserialize)]
pub enum UploadSizeInfo {
    /// When the data transfer amount is known to be exact size, this enum should be used.
    /// The receiver store can use this to better optimize the way the data is sent or stored.
    ExactSize(usize),

    /// When the data transfer amount is not known to be exact, the caller should use this enum
    /// to provide the maximum size that could possibly be sent. This will bypass the exact size
    /// checks, but still provide useful information to the underlying store about the data being
    /// sent that it can then use to optimize the upload process.
    MaxSize(usize),
}

/// Utility to send all the data to the store from a file.
// Note: This is not inlined because some code may want to bypass any underlying
// optimizations that may be present in the inner store.
pub async fn slow_update_store_with_file<S: StoreDriver + ?Sized>(
    store: Pin<&S>,
    digest: impl Into<StoreKey<'_>>,
    file: &mut fs::ResumeableFileSlot,
    upload_size: UploadSizeInfo,
) -> Result<(), Error> {
    file.as_writer()
        .await
        .err_tip(|| "Failed to get writer in upload_file_to_store")?
        .rewind()
        .await
        .err_tip(|| "Failed to rewind in upload_file_to_store")?;
    let (tx, rx) = make_buf_channel_pair();

    let mut update_fut = store
        .update(digest.into(), rx, upload_size)
        .map(|r| r.err_tip(|| "Could not upload data to store in upload_file_to_store"));
    let read_result = {
        let read_data_fut = async {
            let (_, mut tx) = file
                .read_buf_cb(
                    (BytesMut::with_capacity(fs::DEFAULT_READ_BUFF_SIZE), tx),
                    move |(chunk, mut tx)| async move {
                        tx.send(chunk.freeze())
                            .await
                            .err_tip(|| "Failed to send in upload_file_to_store")?;
                        Ok((BytesMut::with_capacity(fs::DEFAULT_READ_BUFF_SIZE), tx))
                    },
                )
                .await
                .err_tip(|| "Error in upload_file_to_store::read_buf_cb section")?;
            tx.send_eof()
                .err_tip(|| "Could not send EOF to store in upload_file_to_store")?;
            Ok(())
        };
        tokio::pin!(read_data_fut);
        match select(&mut update_fut, read_data_fut).await {
            Either::Left((update_result, read_data_fut)) => {
                return update_result.merge(read_data_fut.await)
            }
            Either::Right((read_result, _)) => read_result,
        }
    };
    match timeout(idle_file_descriptor_timeout(), &mut update_fut).await {
        Ok(update_result) => update_result.merge(read_result),
        Err(_) => {
            file.close_file()
                .await
                .err_tip(|| "Failed to close file in upload_file_to_store")?;
            update_fut.await.merge(read_result)
        }
    }
}

/// Optimizations that stores may want to expose to the callers.
/// This is useful for specific cases when the store can optimize the processing
/// of the data being processed.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum StoreOptimizations {
    /// The store can optimize the upload process when it knows the data is coming from a file.
    FileUpdates,

    /// If the store will ignore the data uploads.
    NoopUpdates,

    /// If the store will never serve downloads.
    NoopDownloads,

    /// If the store is optimized for serving subscriptions to keys.
    SubscribeChanges,
}

/// A key that has been subscribed to in the store. This can be used
/// to wait for changes to the data for the key.
#[async_trait]
pub trait StoreSubscription: Send + Sync + Unpin {
    /// Get the current store subscription item.
    fn peek(&self) -> Result<Arc<dyn StoreSubscriptionItem>, Error>;

    /// Wait for the data to change and return the new store subscription item.
    /// Note: This will always have a value ready when struct is first created.
    async fn changed(&mut self) -> Result<Arc<dyn StoreSubscriptionItem>, Error>;
}

/// An item that has been subscribed to in the store. Some stores may have
/// the data already available when the data changes. This allows the store
/// to store a reference to the data and return it when requested, otherwise
/// the store can lazily retrieve the data when requested.
#[async_trait]
pub trait StoreSubscriptionItem: Send + Sync + Unpin {
    /// Returns the key of the item being represented.
    async fn get_key(&self) -> Result<StoreKey, Error>;

    /// Same as `StoreLike::get_part`, but without the key.
    async fn get_part(
        &self,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error>;

    /// Same as `Store::get`, but without the key.
    async fn get(&self, writer: &mut DropCloserWriteHalf) -> Result<(), Error> {
        self.get_part(writer, 0, None).await
    }
}

/// Holds something that can be converted into a key the
/// store API can understand. Generally this is a digest
/// but it can also be a string if the caller wishes to
/// store the data directly and reference it by a string
/// directly.
#[derive(Debug, Eq)]
pub enum StoreKey<'a> {
    /// A string key.
    Str(Cow<'a, str>),

    /// A key that is a digest.
    Digest(DigestInfo),
}

impl<'a> StoreKey<'a> {
    /// Creates a new store key from a string.
    pub const fn new_str(s: &'a str) -> Self {
        StoreKey::Str(Cow::Borrowed(s))
    }

    /// Returns a shallow clone of the key.
    /// This is extremely cheap and should be used when clone
    /// is needed but the key is not going to be modified.
    pub fn borrow(&'a self) -> StoreKey<'a> {
        match self {
            StoreKey::Str(Cow::Owned(s)) => StoreKey::Str(Cow::Borrowed(s)),
            StoreKey::Str(Cow::Borrowed(s)) => StoreKey::Str(Cow::Borrowed(s)),
            StoreKey::Digest(d) => StoreKey::Digest(*d),
        }
    }

    /// Converts the key into an owned version. This is useful
    /// when the caller needs an owned version of the key.
    pub fn into_owned(self) -> StoreKey<'static> {
        match self {
            StoreKey::Str(Cow::Owned(s)) => StoreKey::Str(Cow::Owned(s)),
            StoreKey::Str(Cow::Borrowed(s)) => StoreKey::Str(Cow::Owned(s.to_owned())),
            StoreKey::Digest(d) => StoreKey::Digest(d),
        }
    }

    /// Converts the key into a digest. This is useful when the caller
    /// must have a digest key. If the data is not a digest, it may
    /// hash the underlying key and return a digest of the hash of the key
    pub fn into_digest(self) -> DigestInfo {
        match self {
            StoreKey::Digest(digest) => digest,
            StoreKey::Str(s) => {
                let mut hasher = DigestHasherFunc::Blake3.hasher();
                hasher.update(s.as_bytes());
                hasher.finalize_digest()
            }
        }
    }

    /// Returns the key as a string. If the key is a digest, it will
    /// return a string representation of the digest. If the key is a string,
    /// it will return the string itself.
    pub fn as_str(&'a self) -> Cow<'a, str> {
        match self {
            StoreKey::Str(Cow::Owned(s)) => Cow::Borrowed(s),
            StoreKey::Str(Cow::Borrowed(s)) => Cow::Borrowed(s),
            StoreKey::Digest(d) => Cow::Owned(format!("{d}")),
        }
    }
}

impl Clone for StoreKey<'static> {
    fn clone(&self) -> Self {
        match self {
            StoreKey::Str(s) => StoreKey::Str(s.clone()),
            StoreKey::Digest(d) => StoreKey::Digest(*d),
        }
    }
}

impl PartialOrd for StoreKey<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for StoreKey<'_> {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (StoreKey::Str(a), StoreKey::Str(b)) => a.cmp(b),
            (StoreKey::Digest(a), StoreKey::Digest(b)) => a.cmp(b),
            (StoreKey::Str(_), StoreKey::Digest(_)) => std::cmp::Ordering::Less,
            (StoreKey::Digest(_), StoreKey::Str(_)) => std::cmp::Ordering::Greater,
        }
    }
}

impl PartialEq for StoreKey<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (StoreKey::Str(a), StoreKey::Str(b)) => a == b,
            (StoreKey::Digest(a), StoreKey::Digest(b)) => a == b,
            _ => false,
        }
    }
}

impl Hash for StoreKey<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        /// Salts the hash with the enum value that represents
        /// the type of the key.
        #[repr(u8)]
        enum HashId {
            Str = 0,
            Digest = 1,
        }
        match self {
            StoreKey::Str(s) => {
                (HashId::Str as u8).hash(state);
                s.hash(state)
            }
            StoreKey::Digest(d) => {
                (HashId::Digest as u8).hash(state);
                d.hash(state)
            }
        }
    }
}

impl<'a> From<&'a str> for StoreKey<'a> {
    fn from(s: &'a str) -> Self {
        StoreKey::Str(Cow::Borrowed(s))
    }
}

impl From<String> for StoreKey<'static> {
    fn from(s: String) -> Self {
        StoreKey::Str(Cow::Owned(s))
    }
}

impl<'a> From<DigestInfo> for StoreKey<'a> {
    fn from(d: DigestInfo) -> Self {
        StoreKey::Digest(d)
    }
}

impl<'a> From<&DigestInfo> for StoreKey<'a> {
    fn from(d: &DigestInfo) -> Self {
        StoreKey::Digest(*d)
    }
}

#[derive(Clone, MetricsComponent)]
#[repr(transparent)]
pub struct Store {
    #[metric]
    inner: Arc<dyn StoreDriver>,
}

impl Store {
    pub fn new(inner: Arc<dyn StoreDriver>) -> Self {
        Self { inner }
    }
}

impl Store {
    /// Returns the immediate inner store driver.
    /// Note: This does not recursively try to resolve underlying store drivers
    /// like `.inner_store()` does.
    #[inline]
    pub fn into_inner(self) -> Arc<dyn StoreDriver> {
        self.inner
    }

    /// Gets the underlying store for the given digest.
    /// A caller might want to use this to obtain a reference to the "real" underlying store
    /// (if applicable) and check if it implements some special traits that allow optimizations.
    /// Note: If the store performs complex operations on the data, it should return itself.
    #[inline]
    pub fn inner_store<'a, K: Into<StoreKey<'a>>>(&self, digest: Option<K>) -> &dyn StoreDriver {
        self.inner.inner_store(digest.map(|v| v.into()))
    }

    /// Tries to cast the underlying store to the given type.
    #[inline]
    pub fn downcast_ref<U: StoreDriver>(&self, maybe_digest: Option<StoreKey<'_>>) -> Option<&U> {
        self.inner.inner_store(maybe_digest).as_any().downcast_ref()
    }

    /// Subscribe to a key in the store. The store will notify the subscriber
    /// when the data for the key changes.
    /// There is no guarantee that the store will notify the subscriber of all changes,
    /// and there is no guarantee that the store will notify the subscriber of changes
    /// in a timely manner.
    /// Note: It can be quite expensive to subscribe to a key in stores that do not
    /// have the optimization for this. One may check if a store has the optimization
    /// by calling `optimized_for(StoreOptimizations::SubscribeChanges)`.
    pub fn subscribe<'a>(
        &self,
        key: impl Into<StoreKey<'a>>,
    ) -> impl Future<Output = Box<dyn StoreSubscription>> + 'a {
        self.inner.clone().subscribe(key.into())
    }

    /// Register health checks used to monitor the store.
    #[inline]
    pub fn register_health(&self, registry: &mut HealthRegistryBuilder) {
        self.inner.clone().register_health(registry)
    }
}

impl StoreLike for Store {
    #[inline]
    fn as_store_driver(&self) -> &'_ dyn StoreDriver {
        self.inner.as_ref()
    }

    fn as_pin(&self) -> Pin<&Self> {
        Pin::new(self)
    }
}

impl<T> StoreLike for T
where
    T: StoreDriver + Sized,
{
    #[inline]
    fn as_store_driver(&self) -> &'_ dyn StoreDriver {
        self
    }

    fn as_pin(&self) -> Pin<&Self> {
        Pin::new(self)
    }
}

pub trait StoreLike: Send + Sync + Sized + Unpin + 'static {
    /// Returns the immediate inner store driver.
    fn as_store_driver(&self) -> &'_ dyn StoreDriver;

    /// Utility function to return a pinned reference to self.
    fn as_pin(&self) -> Pin<&Self>;

    /// Utility function to return a pinned reference to the store driver.
    #[inline]
    fn as_store_driver_pin(&self) -> Pin<&'_ dyn StoreDriver> {
        Pin::new(self.as_store_driver())
    }

    /// Look up a digest in the store and return None if it does not exist in
    /// the store, or Some(size) if it does.
    /// Note: On an AC store the size will be incorrect and should not be used!
    #[inline]
    fn has<'a>(
        &'a self,
        digest: impl Into<StoreKey<'a>>,
    ) -> impl Future<Output = Result<Option<usize>, Error>> + 'a {
        self.as_store_driver_pin().has(digest.into())
    }

    /// Look up a list of digests in the store and return a result for each in
    /// the same order as input.  The result will either be None if it does not
    /// exist in the store, or Some(size) if it does.
    /// Note: On an AC store the size will be incorrect and should not be used!
    #[inline]
    fn has_many<'a>(
        &'a self,
        digests: &'a [StoreKey<'a>],
    ) -> impl Future<Output = Result<Vec<Option<usize>>, Error>> + Send + 'a {
        self.as_store_driver_pin().has_many(digests)
    }

    /// The implementation of the above has and has_many functions.  See their
    /// documentation for details.
    #[inline]
    fn has_with_results<'a>(
        &'a self,
        digests: &'a [StoreKey<'a>],
        results: &'a mut [Option<usize>],
    ) -> impl Future<Output = Result<(), Error>> + Send + 'a {
        self.as_store_driver_pin()
            .has_with_results(digests, results)
    }

    /// List all the keys in the store that are within the given range.
    /// `handler` is called for each key in the range. If `handler` returns
    /// false, the listing is stopped.
    ///
    /// The number of keys passed through the handler is the return value.
    #[inline]
    fn list<'a, 'b>(
        &'a self,
        range: impl RangeBounds<StoreKey<'b>> + Send + 'b,
        mut handler: impl for<'c> FnMut(&'c StoreKey) -> bool + Send + Sync + 'a,
    ) -> impl Future<Output = Result<usize, Error>> + Send + 'a
    where
        'b: 'a,
    {
        // Note: We use a manual async move, so the future can own the `range` and `handler`,
        // otherwise we'd require the caller to pass them in by reference making more borrow
        // checker noise.
        async move {
            self.as_store_driver_pin()
                .list(
                    (
                        range.start_bound().map(|v| v.borrow()),
                        range.end_bound().map(|v| v.borrow()),
                    ),
                    &mut handler,
                )
                .await
        }
    }

    /// Sends the data to the store.
    #[inline]
    fn update<'a>(
        &'a self,
        digest: impl Into<StoreKey<'a>>,
        reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
    ) -> impl Future<Output = Result<(), Error>> + Send + 'a {
        self.as_store_driver_pin()
            .update(digest.into(), reader, upload_size)
    }

    /// Any optimizations the store might want to expose to the callers.
    /// By default, no optimizations are exposed.
    #[inline]
    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        self.as_store_driver_pin().optimized_for(optimization)
    }

    /// Specialized version of `.update()` which takes a `ResumeableFileSlot`.
    /// This is useful if the underlying store can optimize the upload process
    /// when it knows the data is coming from a file.
    #[inline]
    fn update_with_whole_file<'a>(
        &'a self,
        digest: impl Into<StoreKey<'a>>,
        file: fs::ResumeableFileSlot,
        upload_size: UploadSizeInfo,
    ) -> impl Future<Output = Result<Option<fs::ResumeableFileSlot>, Error>> + Send + 'a {
        self.as_store_driver_pin()
            .update_with_whole_file(digest.into(), file, upload_size)
    }

    /// Utility to send all the data to the store when you have all the bytes.
    #[inline]
    fn update_oneshot<'a>(
        &'a self,
        digest: impl Into<StoreKey<'a>>,
        data: Bytes,
    ) -> impl Future<Output = Result<(), Error>> + Send + 'a {
        self.as_store_driver_pin()
            .update_oneshot(digest.into(), data)
    }

    /// Retrieves part of the data from the store and writes it to the given writer.
    #[inline]
    fn get_part<'a>(
        &'a self,
        digest: impl Into<StoreKey<'a>>,
        mut writer: impl BorrowMut<DropCloserWriteHalf> + Send + 'a,
        offset: usize,
        length: Option<usize>,
    ) -> impl Future<Output = Result<(), Error>> + Send + 'a {
        let key = digest.into();
        // Note: We need to capture `writer` just in case the caller
        // expects the drop() method to be called on it when the future
        // is done due to the complex interaction between the DropCloserWriteHalf
        // and the DropCloserReadHalf during drop().
        async move {
            self.as_store_driver_pin()
                .get_part(key, writer.borrow_mut(), offset, length)
                .await
        }
    }

    /// Utility that works the same as `.get_part()`, but writes all the data.
    #[inline]
    fn get<'a>(
        &'a self,
        key: impl Into<StoreKey<'a>>,
        writer: DropCloserWriteHalf,
    ) -> impl Future<Output = Result<(), Error>> + Send + 'a {
        self.as_store_driver_pin().get(key.into(), writer)
    }

    /// Utility that will return all the bytes at once instead of in a streaming manner.
    #[inline]
    fn get_part_unchunked<'a>(
        &'a self,
        key: impl Into<StoreKey<'a>>,
        offset: usize,
        length: Option<usize>,
    ) -> impl Future<Output = Result<Bytes, Error>> + Send + 'a {
        self.as_store_driver_pin()
            .get_part_unchunked(key.into(), offset, length)
    }

    /// Default implementation of the health check. Some stores may want to override this
    /// in situations where the default implementation is not sufficient.
    #[inline]
    fn check_health(
        &self,
        namespace: Cow<'static, str>,
    ) -> impl Future<Output = HealthStatus> + Send {
        self.as_store_driver_pin().check_health(namespace)
    }
}

#[async_trait]
pub trait StoreDriver:
    Sync + Send + Unpin + MetricsComponent + HealthStatusIndicator + 'static
{
    /// See: [`StoreLike::has`] for details.
    #[inline]
    async fn has(self: Pin<&Self>, key: StoreKey<'_>) -> Result<Option<usize>, Error> {
        let mut result = [None];
        self.has_with_results(&[key], &mut result).await?;
        Ok(result[0])
    }

    /// See: [`StoreLike::has_many`] for details.
    #[inline]
    async fn has_many(
        self: Pin<&Self>,
        digests: &[StoreKey<'_>],
    ) -> Result<Vec<Option<usize>>, Error> {
        let mut results = vec![None; digests.len()];
        self.has_with_results(digests, &mut results).await?;
        Ok(results)
    }

    /// See: [`StoreLike::has_with_results`] for details.
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[StoreKey<'_>],
        results: &mut [Option<usize>],
    ) -> Result<(), Error>;

    /// See: [`StoreLike::list`] for details.
    async fn list(
        self: Pin<&Self>,
        _range: (Bound<StoreKey<'_>>, Bound<StoreKey<'_>>),
        _handler: &mut (dyn for<'a> FnMut(&'a StoreKey) -> bool + Send + Sync + '_),
    ) -> Result<usize, Error> {
        // TODO(allada) We should force all stores to implement this function instead of
        // providing a default implementation.
        Err(make_err!(
            Code::Unimplemented,
            "Store::list() not implemented for this store"
        ))
    }

    /// See: [`StoreLike::update`] for details.
    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
    ) -> Result<(), Error>;

    /// See: [`StoreLike::optimized_for`] for details.
    fn optimized_for(&self, _optimization: StoreOptimizations) -> bool {
        false
    }

    /// See: [`StoreLike::update_with_whole_file`] for details.
    async fn update_with_whole_file(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut file: fs::ResumeableFileSlot,
        upload_size: UploadSizeInfo,
    ) -> Result<Option<fs::ResumeableFileSlot>, Error> {
        let inner_store = self.inner_store(Some(key.borrow()));
        if inner_store.optimized_for(StoreOptimizations::FileUpdates) {
            error_if!(
                addr_eq(inner_store, self.deref()),
                "Store::inner_store() returned self when optimization present"
            );
            return Pin::new(inner_store)
                .update_with_whole_file(key, file, upload_size)
                .await;
        }
        slow_update_store_with_file(self, key, &mut file, upload_size).await?;
        Ok(Some(file))
    }

    /// See: [`StoreLike::update_oneshot`] for details.
    async fn update_oneshot(self: Pin<&Self>, key: StoreKey<'_>, data: Bytes) -> Result<(), Error> {
        // TODO(blaise.bruer) This is extremely inefficient, since we have exactly
        // what we need here. Maybe we could instead make a version of the stream
        // that can take objects already fully in memory instead?
        let (mut tx, rx) = make_buf_channel_pair();

        let data_len = data.len();
        let send_fut = async move {
            // Only send if we are not EOF.
            if !data.is_empty() {
                tx.send(data)
                    .await
                    .err_tip(|| "Failed to write data in update_oneshot")?;
            }
            tx.send_eof()
                .err_tip(|| "Failed to write EOF in update_oneshot")?;
            Ok(())
        };
        try_join!(
            send_fut,
            self.update(key, rx, UploadSizeInfo::ExactSize(data_len))
        )?;
        Ok(())
    }

    /// See: [`StoreLike::get_part`] for details.
    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error>;

    /// See: [`StoreLike::get`] for details.
    #[inline]
    async fn get(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut writer: DropCloserWriteHalf,
    ) -> Result<(), Error> {
        self.get_part(key, &mut writer, 0, None).await
    }

    /// See: [`StoreLike::get_part_unchunked`] for details.
    async fn get_part_unchunked(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        offset: usize,
        length: Option<usize>,
    ) -> Result<Bytes, Error> {
        // TODO(blaise.bruer) This is extremely inefficient, since we have exactly
        // what we need here. Maybe we could instead make a version of the stream
        // that can take objects already fully in memory instead?
        let (mut tx, mut rx) = make_buf_channel_pair();

        let (data_res, get_part_res) = join!(
            rx.consume(length),
            // We use a closure here to ensure that the `tx` is dropped when the
            // future is done.
            async move { self.get_part(key, &mut tx, offset, length).await },
        );
        get_part_res
            .err_tip(|| "Failed to get_part in get_part_unchunked")
            .merge(data_res.err_tip(|| "Failed to read stream to completion in get_part_unchunked"))
    }

    /// See: [`Store::subscribe`] for details.
    async fn subscribe(self: Arc<Self>, key: StoreKey<'_>) -> Box<dyn StoreSubscription> {
        default_store_key_subscribe(self, key).await
    }

    /// See: [`StoreLike::check_health`] for details.
    async fn check_health(self: Pin<&Self>, namespace: Cow<'static, str>) -> HealthStatus {
        let digest_data_size = default_digest_size_health_check();
        let mut digest_data = vec![0u8; digest_data_size];

        let mut namespace_hasher = StdHasher::new();
        namespace.hash(&mut namespace_hasher);
        self.get_name().hash(&mut namespace_hasher);
        let hash_seed = namespace_hasher.finish();

        // Fill the digest data with random data based on a stable
        // hash of the namespace and store name. Intention is to
        // have randomly filled data that is unique per store and
        // does not change between health checks. This is to ensure
        // we are not adding more data to store on each health check.
        let mut rng: StdRng = StdRng::seed_from_u64(hash_seed);
        rng.fill_bytes(&mut digest_data);

        let mut digest_hasher = default_digest_hasher_func().hasher();
        digest_hasher.update(&digest_data);
        let digest_data_len = digest_data.len();
        let digest_info = StoreKey::from(digest_hasher.finalize_digest());

        let digest_bytes = bytes::Bytes::copy_from_slice(&digest_data);

        if let Err(e) = self
            .update_oneshot(digest_info.borrow(), digest_bytes.clone())
            .await
        {
            return HealthStatus::new_failed(
                self.get_ref(),
                format!("Store.update_oneshot() failed: {e}").into(),
            );
        }

        match self.has(digest_info.borrow()).await {
            Ok(Some(s)) => {
                if s != digest_data_len {
                    return HealthStatus::new_failed(
                        self.get_ref(),
                        format!("Store.has() size mismatch {s} != {digest_data_len}").into(),
                    );
                }
            }
            Ok(None) => {
                return HealthStatus::new_failed(
                    self.get_ref(),
                    "Store.has() size not found".into(),
                );
            }
            Err(e) => {
                return HealthStatus::new_failed(
                    self.get_ref(),
                    format!("Store.has() failed: {e}").into(),
                );
            }
        }

        match self
            .get_part_unchunked(digest_info, 0, Some(digest_data_len))
            .await
        {
            Ok(b) => {
                if b != digest_bytes {
                    return HealthStatus::new_failed(
                        self.get_ref(),
                        "Store.get_part_unchunked() data mismatch".into(),
                    );
                }
            }
            Err(e) => {
                return HealthStatus::new_failed(
                    self.get_ref(),
                    format!("Store.get_part_unchunked() failed: {e}").into(),
                );
            }
        }

        HealthStatus::new_ok(self.get_ref(), "Successfully store health check".into())
    }

    /// See: [`Store::inner_store`] for details.
    fn inner_store(&self, _digest: Option<StoreKey<'_>>) -> &dyn StoreDriver;

    /// Returns an Any variation of whatever Self is.
    fn as_any(&self) -> &(dyn std::any::Any + Sync + Send + 'static);
    fn as_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Sync + Send + 'static>;

    // Register health checks used to monitor the store.
    fn register_health(self: Arc<Self>, _registry: &mut HealthRegistryBuilder) {}
}

/// The instructions on how to decode a value from a Bytes & version into
/// the underlying type.
pub trait SchedulerStoreDecodeTo {
    type DecodeOutput;
    fn decode(version: u64, data: Bytes) -> Result<Self::DecodeOutput, Error>;
}

pub trait SchedulerSubscription: Send + Sync {
    fn changed(&mut self) -> impl Future<Output = Result<(), Error>> + Send;
}

pub trait SchedulerSubscriptionManager: Send + Sync {
    type Subscription: SchedulerSubscription;

    fn notify_for_test(&self, value: String);

    fn subscribe<K>(&self, key: K) -> Result<Self::Subscription, Error>
    where
        K: SchedulerStoreKeyProvider;
}

/// The API surface for a scheduler store.
pub trait SchedulerStore: Send + Sync + 'static {
    type SubscriptionManager: SchedulerSubscriptionManager;

    /// Returns the subscription manager for the scheduler store.
    fn subscription_manager(&self) -> Result<Arc<Self::SubscriptionManager>, Error>;

    /// Updates or inserts an entry into the underlying store.
    /// Metadata about the key is attached to the compile-time type.
    /// If StoreKeyProvider::Versioned is TrueValue, the data will not
    /// be updated if the current version in the database does not match
    /// the version in the passed in data.
    /// No guarantees are made about when Version is FalseValue.
    /// Indexes are guaranteed to be updated atomically with the data.
    fn update_data<T>(&self, data: T) -> impl Future<Output = Result<Option<u64>, Error>> + Send
    where
        T: SchedulerStoreDataProvider
            + SchedulerStoreKeyProvider
            + SchedulerCurrentVersionProvider
            + Send;

    /// Searches for all keys in the store that match the given index prefix.
    fn search_by_index_prefix<K>(
        &self,
        index: K,
    ) -> impl Future<
        Output = Result<
            impl Stream<Item = Result<<K as SchedulerStoreDecodeTo>::DecodeOutput, Error>> + Send,
            Error,
        >,
    > + Send
    where
        K: SchedulerIndexProvider + SchedulerStoreDecodeTo + Send;

    /// Returns data for the provided key with the given version if
    /// StoreKeyProvider::Versioned is TrueValue.
    fn get_and_decode<K>(
        &self,
        key: K,
    ) -> impl Future<Output = Result<Option<<K as SchedulerStoreDecodeTo>::DecodeOutput>, Error>> + Send
    where
        K: SchedulerStoreKeyProvider + SchedulerStoreDecodeTo + Send;
}

/// A type that is used to let the scheduler store know what
/// index is beign requested.
pub trait SchedulerIndexProvider {
    /// Only keys inserted with this prefix will be indexed.
    const KEY_PREFIX: &'static str;

    /// The name of the index.
    const INDEX_NAME: &'static str;

    /// If the data is versioned.
    type Versioned: BoolValue;

    /// The value of the index.
    fn index_value_prefix(&self) -> Cow<'_, str>;
}

/// Provides a key to lookup data in the store.
pub trait SchedulerStoreKeyProvider {
    /// If the data is versioned.
    type Versioned: BoolValue;

    /// Returns the key for the data.
    fn get_key(&self) -> StoreKey<'static>;
}

/// Provides data to be stored in the scheduler store.
pub trait SchedulerStoreDataProvider {
    /// Converts the data into bytes to be stored in the store.
    fn try_into_bytes(self) -> Result<Bytes, Error>;

    /// Returns the indexes for the data if any.
    fn get_indexes(&self) -> Result<Vec<(&'static str, Bytes)>, Error> {
        Ok(Vec::new())
    }
}

/// Provides the current version of the data in the store.
pub trait SchedulerCurrentVersionProvider {
    /// Returns the current version of the data in the store.
    fn current_version(&self) -> u64;
}

/// Default implementation for when we are not providing a version
/// for the data.
impl<T> SchedulerCurrentVersionProvider for T
where
    T: SchedulerStoreKeyProvider<Versioned = FalseValue>,
{
    fn current_version(&self) -> u64 {
        0
    }
}

/// Compile time types for booleans.
pub trait BoolValue {
    const VALUE: bool;
}
/// Compile time check if something is false.
pub trait IsFalse {}
/// Compile time check if something is true.
pub trait IsTrue {}

/// Compile time true value.
pub struct TrueValue;
impl BoolValue for TrueValue {
    const VALUE: bool = true;
}
impl IsTrue for TrueValue {}

/// Compile time false value.
pub struct FalseValue;
impl BoolValue for FalseValue {
    const VALUE: bool = false;
}
impl IsFalse for FalseValue {}
