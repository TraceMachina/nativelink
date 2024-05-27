// Copyright 2023 The NativeLink Authors. All rights reserved.
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

use std::borrow::Cow;
use std::collections::hash_map::DefaultHasher as StdHasher;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::future::{select, Either};
use futures::{join, try_join, Future, FutureExt};
use nativelink_error::{error_if, make_err, Code, Error, ResultExt};
use rand::rngs::StdRng;
use rand::{RngCore, SeedableRng};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncSeekExt;
use tokio::time::timeout;

use crate::buf_channel::{make_buf_channel_pair, DropCloserReadHalf, DropCloserWriteHalf};
use crate::common::DigestInfo;
use crate::digest_hasher::{default_digest_hasher_func, DigestHasher};
use crate::fs::{self, idle_file_descriptor_timeout};
use crate::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use crate::metrics_utils::Registry;

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
    digest: DigestInfo,
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
        .update(digest, rx, upload_size)
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

// TODO(allada) When 1.76.0 stabalizes more we can use `core::ptr::addr_eq` instead.
fn addr_eq<T: ?Sized, U: ?Sized>(p: *const T, q: *const U) -> bool {
    std::ptr::eq(p as *const (), q as *const ())
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
}

#[derive(Clone)]
#[repr(transparent)]
pub struct Store {
    inner: Arc<dyn StoreDriver>,
}

impl Store {
    pub fn new(inner: Arc<dyn StoreDriver>) -> Self {
        Self { inner }
    }
}

impl Store {
    #[inline]
    pub fn into_inner(self) -> Arc<dyn StoreDriver> {
        self.inner
    }

    /// Gets the underlying store for the given digest.
    /// A caller might want to use this to obtain a reference to the "real" underlying store
    /// (if applicable) and check if it implements some special traits that allow optimizations.
    /// Note: If the store performs complex operations on the data, it should return itself.
    #[inline]
    pub fn inner_store(&self, digest: Option<DigestInfo>) -> &Arc<dyn StoreDriver> {
        self.inner.maybe_inner_store(digest).unwrap_or(&self.inner)
    }

    #[inline]
    pub fn downcast<U>(self, maybe_digest: Option<DigestInfo>) -> Option<Arc<U>>
    where
        Self: Sized,
        U: StoreDriver,
    {
        let Some(inner_store) = self.inner.maybe_inner_store(maybe_digest) else {
            return self.inner.as_any_arc().downcast().ok();
        };
        inner_store.clone().as_any_arc().downcast().ok()
    }

    #[inline]
    pub fn downcast_ref<U: StoreDriver>(&self, maybe_digest: Option<DigestInfo>) -> Option<&U> {
        let Some(inner_store) = self.inner.maybe_inner_store(maybe_digest) else {
            return self.inner.as_any().downcast_ref();
        };
        inner_store.as_any().downcast_ref()
    }

    /// Register any metrics that this store wants to expose to the Prometheus.
    #[inline]
    pub fn register_metrics(&self, registry: &mut Registry) {
        self.inner.clone().register_metrics(registry)
    }

    /// Register health checks used to monitor the store.
    #[inline]
    pub fn register_health(&self, registry: &mut HealthRegistryBuilder) {
        self.inner.clone().register_health(registry)
    }
}

impl StoreLike for Store {
    #[inline]
    fn as_store_api(&self) -> &'_ dyn StoreDriver {
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
    fn as_store_api(&self) -> &'_ dyn StoreDriver {
        self
    }

    fn as_pin(&self) -> Pin<&Self> {
        Pin::new(self)
    }
}

pub trait StoreLike: Send + Sync + Sized {
    fn as_store_api(&self) -> &'_ dyn StoreDriver;
    fn as_pin(&self) -> Pin<&Self>;

    #[inline]
    fn as_store_api_pin(&self) -> Pin<&'_ dyn StoreDriver> {
        Pin::new(self.as_store_api())
    }

    /// Look up a digest in the store and return None if it does not exist in
    /// the store, or Some(size) if it does.
    /// Note: On an AC store the size will be incorrect and should not be used!
    #[inline]
    fn has(&self, digest: DigestInfo) -> impl Future<Output = Result<Option<usize>, Error>> + '_ {
        self.as_store_api_pin().has(digest)
    }

    /// Look up a list of digests in the store and return a result for each in
    /// the same order as input.  The result will either be None if it does not
    /// exist in the store, or Some(size) if it does.
    /// Note: On an AC store the size will be incorrect and should not be used!
    #[inline]
    fn has_many<'b>(
        &'b self,
        digests: &'b [DigestInfo],
    ) -> impl Future<Output = Result<Vec<Option<usize>>, Error>> + Send + 'b {
        self.as_store_api_pin().has_many(digests)
    }

    /// The implementation of the above has and has_many functions.  See their
    /// documentation for details.
    #[inline]
    fn has_with_results<'b>(
        &'b self,
        digests: &'b [DigestInfo],
        results: &'b mut [Option<usize>],
    ) -> impl Future<Output = Result<(), Error>> + Send + 'b {
        self.as_store_api_pin().has_with_results(digests, results)
    }

    #[inline]
    fn update(
        &self,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
    ) -> impl Future<Output = Result<(), Error>> + Send + '_ {
        self.as_store_api_pin().update(digest, reader, upload_size)
    }

    /// Any optimizations the store might want to expose to the callers.
    /// By default, no optimizations are exposed.
    #[inline]
    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        self.as_store_api_pin().optimized_for(optimization)
    }

    /// Specialized version of `.update()` which takes a `ResumeableFileSlot`.
    /// This is useful if the underlying store can optimize the upload process
    /// when it knows the data is coming from a file.
    #[inline]
    fn update_with_whole_file(
        &self,
        digest: DigestInfo,
        file: fs::ResumeableFileSlot,
        upload_size: UploadSizeInfo,
    ) -> impl Future<Output = Result<Option<fs::ResumeableFileSlot>, Error>> + Send + '_ {
        self.as_store_api_pin()
            .update_with_whole_file(digest, file, upload_size)
    }

    /// Utility to send all the data to the store when you have all the bytes.
    #[inline]
    fn update_oneshot(
        &self,
        digest: DigestInfo,
        data: Bytes,
    ) -> impl Future<Output = Result<(), Error>> + Send + '_ {
        self.as_store_api_pin().update_oneshot(digest, data)
    }

    /// Retreives part of the data from the store and writes it to the given writer.
    #[inline]
    fn get_part_ref<'b>(
        &'b self,
        digest: DigestInfo,
        writer: &'b mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> impl Future<Output = Result<(), Error>> + Send + 'b {
        self.as_store_api_pin()
            .get_part_ref(digest, writer, offset, length)
    }

    /// Same as `get_part_ref`, but takes ownership of the writer. This is preferred
    /// when the writer is definitly not going to be needed after the function returns.
    /// This is useful because the read half of the writer will block until the writer
    /// is dropped or EOF is sent. If the writer was passed as a reference, and the
    /// reader was being waited with the `.get_part()`, it could deadlock if the writer
    /// is not dropped or EOF sent. `.get_part_ref()` should be used when the writer
    /// might be used after the function returns.
    #[inline]
    fn get_part(
        &self,
        digest: DigestInfo,
        writer: DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> impl Future<Output = Result<(), Error>> + Send + '_ {
        self.as_store_api_pin()
            .get_part(digest, writer, offset, length)
    }

    /// Utility that works the same as ``.get_part()`, but writes all the data.
    #[inline]
    fn get(
        &self,
        digest: DigestInfo,
        writer: DropCloserWriteHalf,
    ) -> impl Future<Output = Result<(), Error>> + Send + '_ {
        self.as_store_api_pin().get(digest, writer)
    }

    /// Utility that will return all the bytes at once instead of in a streaming manner.
    #[inline]
    fn get_part_unchunked(
        &self,
        digest: DigestInfo,
        offset: usize,
        length: Option<usize>,
    ) -> impl Future<Output = Result<Bytes, Error>> + Send + '_ {
        self.as_store_api_pin()
            .get_part_unchunked(digest, offset, length)
    }

    /// Default implementation of the health check. Some stores may want to override this
    /// in situations where the default implementation is not sufficient.
    #[inline]
    fn check_health(
        &self,
        namespace: Cow<'static, str>,
    ) -> impl Future<Output = HealthStatus> + Send + '_ {
        self.as_store_api_pin().check_health(namespace)
    }
}

#[async_trait]
pub trait StoreDriver: Sync + Send + Unpin + HealthStatusIndicator + 'static {
    /// Look up a digest in the store and return None if it does not exist in
    /// the store, or Some(size) if it does.
    /// Note: On an AC store the size will be incorrect and should not be used!
    #[inline]
    async fn has(self: Pin<&Self>, digest: DigestInfo) -> Result<Option<usize>, Error> {
        let mut result = [None];
        self.has_with_results(&[digest], &mut result).await?;
        Ok(result[0])
    }

    /// Look up a list of digests in the store and return a result for each in
    /// the same order as input.  The result will either be None if it does not
    /// exist in the store, or Some(size) if it does.
    /// Note: On an AC store the size will be incorrect and should not be used!
    #[inline]
    async fn has_many(
        self: Pin<&Self>,
        digests: &[DigestInfo],
    ) -> Result<Vec<Option<usize>>, Error> {
        let mut results = vec![None; digests.len()];
        self.has_with_results(digests, &mut results).await?;
        Ok(results)
    }

    /// The implementation of the above has and has_many functions.  See their
    /// documentation for details.
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error>;

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
    ) -> Result<(), Error>;

    /// Any optimizations the store might want to expose to the callers.
    /// By default, no optimizations are exposed.
    fn optimized_for(&self, _optimization: StoreOptimizations) -> bool {
        false
    }

    /// Specialized version of `.update()` which takes a `ResumeableFileSlot`.
    /// This is useful if the underlying store can optimize the upload process
    /// when it knows the data is coming from a file.
    async fn update_with_whole_file(
        self: Pin<&Self>,
        digest: DigestInfo,
        mut file: fs::ResumeableFileSlot,
        upload_size: UploadSizeInfo,
    ) -> Result<Option<fs::ResumeableFileSlot>, Error> {
        if let Some(inner_store) = self.maybe_inner_store(Some(digest)) {
            if inner_store.optimized_for(StoreOptimizations::FileUpdates) {
                error_if!(
                    addr_eq(inner_store, self.deref()),
                    "Store::inner_store() returned self when optimization present"
                );
                return Pin::new(inner_store.as_ref())
                    .update_with_whole_file(digest, file, upload_size)
                    .await;
            }
        }
        slow_update_store_with_file(self, digest, &mut file, upload_size).await?;
        Ok(Some(file))
    }

    // Utility to send all the data to the store when you have all the bytes.
    async fn update_oneshot(
        self: Pin<&Self>,
        digest: DigestInfo,
        data: Bytes,
    ) -> Result<(), Error> {
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
            self.update(digest, rx, UploadSizeInfo::ExactSize(data_len))
        )?;
        Ok(())
    }

    /// Retreives part of the data from the store and writes it to the given writer.
    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error>;

    /// Same as `get_part_ref`, but takes ownership of the writer. This is preferred
    /// when the writer is definitly not going to be needed after the function returns.
    /// This is useful because the read half of the writer will block until the writer
    /// is dropped or EOF is sent. If the writer was passed as a reference, and the
    /// reader was being waited with the `.get_part()`, it could deadlock if the writer
    /// is not dropped or EOF sent. `.get_part_ref()` should be used when the writer
    /// might be used after the function returns.
    #[inline]
    async fn get_part(
        self: Pin<&Self>,
        digest: DigestInfo,
        mut writer: DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        self.get_part_ref(digest, &mut writer, offset, length).await
    }

    /// Utility that works the same as ``.get_part()`, but writes all the data.
    #[inline]
    async fn get(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: DropCloserWriteHalf,
    ) -> Result<(), Error> {
        self.get_part(digest, writer, 0, None).await
    }

    /// Utility for when `self` is an `Arc` to make the code easier to write.
    #[inline]
    async fn get_part_arc(
        self: Arc<Self>,
        digest: DigestInfo,
        writer: DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        Pin::new(self.as_ref())
            .get_part(digest, writer, offset, length)
            .await
    }

    // Utility that will return all the bytes at once instead of in a streaming manner.
    async fn get_part_unchunked(
        self: Pin<&Self>,
        digest: DigestInfo,
        offset: usize,
        length: Option<usize>,
    ) -> Result<Bytes, Error> {
        // TODO(blaise.bruer) This is extremely inefficient, since we have exactly
        // what we need here. Maybe we could instead make a version of the stream
        // that can take objects already fully in memory instead?
        let (tx, mut rx) = make_buf_channel_pair();

        let (data_res, get_part_res) = join!(
            rx.consume(length),
            self.get_part(digest, tx, offset, length),
        );
        get_part_res
            .err_tip(|| "Failed to get_part in get_part_unchunked")
            .merge(data_res.err_tip(|| "Failed to read stream to completion in get_part_unchunked"))
    }

    // Default implementation of the health check. Some stores may want to override this
    // in situations where the default implementation is not sufficient.
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
        let digest_info = digest_hasher.finalize_digest();

        let digest_bytes = bytes::Bytes::copy_from_slice(&digest_data);

        if let Err(e) = self.update_oneshot(digest_info, digest_bytes.clone()).await {
            return HealthStatus::new_failed(
                self.get_ref(),
                format!("Store.update_oneshot() failed: {e}").into(),
            );
        }

        match self.has(digest_info).await {
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

    /// Gets the underlying store for the given digest.
    /// A caller might want to use this to obtain a reference to the "real" underlying store
    /// (if applicable) and check if it implements some special traits that allow optimizations.
    /// Note: If the store performs complex operations on the data, it should return itself.
    fn maybe_inner_store(&self, _digest: Option<DigestInfo>) -> Option<&Arc<dyn StoreDriver>> {
        None
    }

    /// Returns an Any variation of whatever Self is.
    fn as_any(&self) -> &(dyn std::any::Any + Sync + Send + 'static);
    fn as_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Sync + Send + 'static>;

    /// Register any metrics that this store wants to expose to the Prometheus.
    fn register_metrics(self: Arc<Self>, _registry: &mut Registry) {}

    // Register health checks used to monitor the store.
    fn register_health(self: Arc<Self>, _registry: &mut HealthRegistryBuilder) {}
}
