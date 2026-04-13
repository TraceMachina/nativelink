// Copyright 2024-2025 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::borrow::BorrowMut;
use core::cmp::{max, min};
use core::ops::Range;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::sync::{Arc, Weak};
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use futures::{FutureExt, join};
use nativelink_config::stores::{FastSlowSpec, StoreDirection};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::common::DigestInfo;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair_with_size,
};
use nativelink_util::fs;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    IS_MIRROR_REQUEST, ItemCallback, Store, StoreDriver, StoreKey, StoreLike, StoreOptimizations,
    UploadSizeInfo, slow_update_store_with_file,
};
use nativelink_util::streaming_blob::{StreamingBlobInner, StreamingBlobWriter};
use parking_lot::Mutex;
use tokio::sync::{Notify, OnceCell};
use tracing::{debug, error, trace, warn};

// TODO(palfrey) This store needs to be evaluated for more efficient memory usage,
// there are many copies happening internally.

type Loader = Arc<OnceCell<()>>;

/// Maximum aggregate bytes held in `mirror_blobs`. When exceeded, new mirror
/// blobs are silently dropped (the server already persisted them).
const MIRROR_BLOBS_MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024; // 2 GiB

// TODO(palfrey) We should consider copying the data in the background to allow the
// client to hang up while the data is buffered. An alternative is to possibly make a
// "BufferedStore" that could be placed on the "slow" store that would hang up early
// if data is in the buffer.
#[derive(Debug, MetricsComponent)]
pub struct FastSlowStore {
    #[metric(group = "fast_store")]
    fast_store: Store,
    fast_direction: StoreDirection,
    #[metric(group = "slow_store")]
    slow_store: Store,
    slow_direction: StoreDirection,
    weak_self: Weak<Self>,
    #[metric]
    metrics: FastSlowStoreMetrics,
    // De-duplicate requests for the fast store, only the first streams, others
    // are blocked.  This may feel like it's causing a slow down of tasks, but
    // actually it's faster because we're not downloading the file multiple
    // times are doing loads of duplicate IO.
    populating_digests: Mutex<HashMap<StoreKey<'static>, (Loader, Arc<StreamingBlobInner>)>>,
    /// Holds data for blobs whose background slow-store write is still in
    /// progress. If the fast store evicts the blob before the slow write
    /// completes, `get_part` serves from this map to prevent NotFound gaps.
    in_flight_slow_writes: Arc<Mutex<HashMap<StoreKey<'static>, Vec<Bytes>>>>,
    /// Notified when in_flight_slow_writes becomes empty. Used by
    /// `flush_slow_writes` to wait for all background writes to complete.
    in_flight_empty_notify: Arc<Notify>,
    /// Digests that have completed their background slow store write.
    /// Drained by the BlobsInStableStorage loop when notified.
    stable_digests: Arc<Mutex<Vec<DigestInfo>>>,
    /// Wakes the BlobsInStableStorage loop when new digests are available.
    stable_notify: Arc<Notify>,
    /// Set to true during shutdown to prevent new background slow writes
    /// from being spawned while we flush existing ones.
    shutting_down: AtomicBool,
    /// Digests whose background slow-store write failed. Tracked so the
    /// worker can retry uploads on reconnect.
    failed_slow_writes: Arc<Mutex<HashSet<DigestInfo>>>,
    /// Blobs received via server-side mirror that are held in memory only.
    /// The server has already persisted these blobs — we hold them so peers
    /// and local actions can read them without disk I/O. Cleaned up when
    /// `BlobsInStableStorage` arrives or after a TTL expiry.
    mirror_blobs: Mutex<HashMap<DigestInfo, (Bytes, Instant)>>,
    /// Total bytes currently held in `mirror_blobs`. Tracked separately to
    /// enforce `MIRROR_BLOBS_MAX_BYTES` without iterating the map.
    mirror_blobs_total_bytes: AtomicU64,
}

// This guard ensures that the populating_digests is cleared even if the future
// is dropped, it is cancel safe.
struct LoaderGuard<'a> {
    weak_store: Weak<FastSlowStore>,
    key: StoreKey<'a>,
    loader: Option<Loader>,
    /// Streaming buffer shared between the populating thread and waiters.
    /// Waiters read from this instead of blocking on the OnceCell.
    streaming_inner: Arc<StreamingBlobInner>,
    /// True if this guard created a new entry (we're the populator).
    /// False if another thread is already populating (we're a waiter).
    is_new: bool,
}

impl LoaderGuard<'_> {
    async fn get_or_try_init<E, F, Fut>(&self, f: F) -> Result<(), E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(), E>>,
    {
        if let Some(loader) = &self.loader {
            loader.get_or_try_init(f).await.map(|&()| ())
        } else {
            // This is impossible, but we do it anyway.
            f().await
        }
    }
}

impl Drop for LoaderGuard<'_> {
    fn drop(&mut self) {
        let Some(store) = self.weak_store.upgrade() else {
            // The store has already gone away, nothing to remove from.
            return;
        };
        let Some(loader) = self.loader.take() else {
            // This should never happen, but we do it to be safe.
            return;
        };

        // Pre-compute the owned key outside the lock to minimize lock hold time.
        let owned_key = self.key.borrow().into_owned();
        let mut guard = store.populating_digests.lock();
        if let std::collections::hash_map::Entry::Occupied(occupied_entry) =
            guard.entry(owned_key)
        {
            if Arc::ptr_eq(&occupied_entry.get().0, &loader) {
                drop(loader);
                if Arc::strong_count(&occupied_entry.get().0) == 1 {
                    // This is the last loader, so remove it.
                    occupied_entry.remove();
                }
            }
        }
    }
}

impl FastSlowStore {
    pub fn new(spec: &FastSlowSpec, fast_store: Store, slow_store: Store) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| Self {
            fast_store,
            fast_direction: spec.fast_direction,
            slow_store,
            slow_direction: spec.slow_direction,
            weak_self: weak_self.clone(),
            metrics: FastSlowStoreMetrics::default(),
            populating_digests: Mutex::new(HashMap::new()),
            in_flight_slow_writes: Arc::new(Mutex::new(HashMap::new())),
            in_flight_empty_notify: Arc::new(Notify::new()),
            stable_digests: Arc::new(Mutex::new(Vec::new())),
            stable_notify: Arc::new(Notify::new()),
            shutting_down: AtomicBool::new(false),
            failed_slow_writes: Arc::new(Mutex::new(HashSet::new())),
            mirror_blobs: Mutex::new(HashMap::new()),
            mirror_blobs_total_bytes: AtomicU64::new(0),
        })
    }

    pub fn in_flight_slow_write_count(&self) -> usize {
        self.in_flight_slow_writes.lock().len()
    }

    /// Fence out new background slow writes and wait for all existing
    /// ones to complete, with a timeout. Returns the number of writes
    /// still pending when the timeout expired (0 = all flushed).
    pub async fn flush_slow_writes(&self, timeout: Duration) -> usize {
        self.shutting_down.store(true, Ordering::Release);
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Register the notified future BEFORE checking the count to
            // avoid missing a notification between check and await.
            let notified = self.in_flight_empty_notify.notified();
            let count = self.in_flight_slow_writes.lock().len();
            if count == 0 {
                return 0;
            }
            match tokio::time::timeout_at(deadline, notified).await {
                Ok(()) => continue,
                Err(_) => {
                    let guard = self.in_flight_slow_writes.lock();
                    let remaining = guard.len();
                    if remaining > 0 {
                        warn!(
                            remaining,
                            "FastSlowStore::flush_slow_writes: timed out waiting \
                             for background writes to complete"
                        );
                        for (key, chunks) in guard.iter() {
                            let bytes: usize = chunks.iter().map(|b| b.len()).sum();
                            warn!(
                                ?key,
                                bytes,
                                "FastSlowStore: unflushed write at shutdown"
                            );
                        }
                    }
                    return remaining;
                }
            }
        }
    }

    pub const fn fast_store(&self) -> &Store {
        &self.fast_store
    }

    pub const fn slow_store(&self) -> &Store {
        &self.slow_store
    }

    pub const fn fast_direction(&self) -> StoreDirection {
        self.fast_direction
    }

    pub const fn slow_direction(&self) -> StoreDirection {
        self.slow_direction
    }

    pub fn get_arc(&self) -> Option<Arc<Self>> {
        self.weak_self.upgrade()
    }

    /// Drain all digests that have completed their slow store write since the last drain.
    /// Called by the BlobsInStableStorage batching loop.
    pub fn drain_stable_digests(&self) -> Vec<DigestInfo> {
        let mut guard = self.stable_digests.lock();
        std::mem::take(&mut *guard)
    }

    /// Drain digests whose background slow-store write failed.
    /// Called by the worker on reconnect to retry uploads.
    pub fn drain_failed_digests(&self) -> Vec<DigestInfo> {
        let mut guard = self.failed_slow_writes.lock();
        guard.drain().collect()
    }

    /// Remove digests from the failed/pending set, e.g. when the server
    /// confirms stable storage via BlobsInStableStorage.
    pub fn ack_digests(&self, digests: &[DigestInfo]) {
        let mut guard = self.failed_slow_writes.lock();
        for digest in digests {
            guard.remove(digest);
        }
    }

    /// Create a new FastSlowStore that shares the failed_slow_writes
    /// tracking set with another store. Used so the worker CAS server
    /// store and RunningActionsManager store track pending uploads in
    /// the same place.
    pub fn new_with_shared_failed_writes(
        spec: &FastSlowSpec,
        fast_store: Store,
        slow_store: Store,
        other: &Arc<Self>,
    ) -> Arc<Self> {
        let shared = other.failed_slow_writes.clone();
        Arc::new_cyclic(|weak_self| Self {
            fast_store,
            fast_direction: spec.fast_direction,
            slow_store,
            slow_direction: spec.slow_direction,
            weak_self: weak_self.clone(),
            metrics: FastSlowStoreMetrics::default(),
            populating_digests: Mutex::new(HashMap::new()),
            in_flight_slow_writes: Arc::new(Mutex::new(HashMap::new())),
            in_flight_empty_notify: Arc::new(Notify::new()),
            stable_digests: Arc::new(Mutex::new(Vec::new())),
            stable_notify: Arc::new(Notify::new()),
            shutting_down: AtomicBool::new(false),
            failed_slow_writes: shared,
            mirror_blobs: Mutex::new(HashMap::new()),
            mirror_blobs_total_bytes: AtomicU64::new(0),
        })
    }

    /// Remove mirror blobs that the server has confirmed are in stable storage.
    pub fn remove_mirror_blobs(&self, digests: &[DigestInfo]) {
        let mut guard = self.mirror_blobs.lock();
        let mut freed = 0u64;
        for digest in digests {
            if let Some((data, _)) = guard.remove(digest) {
                freed += data.len() as u64;
            }
        }
        if freed > 0 {
            self.mirror_blobs_total_bytes.fetch_sub(freed, Ordering::Relaxed);
        }
    }

    /// Remove mirror blobs older than the given duration. Returns the number
    /// of blobs expired.
    pub fn expire_mirror_blobs(&self, max_age: Duration) -> usize {
        let mut guard = self.mirror_blobs.lock();
        let before = guard.len();
        let mut freed = 0u64;
        guard.retain(|_, (data, inserted_at)| {
            if inserted_at.elapsed() < max_age {
                true
            } else {
                freed += data.len() as u64;
                false
            }
        });
        if freed > 0 {
            self.mirror_blobs_total_bytes.fetch_sub(freed, Ordering::Relaxed);
        }
        before - guard.len()
    }

    /// Current number of mirror blobs held in memory.
    pub fn mirror_blob_count(&self) -> usize {
        self.mirror_blobs.lock().len()
    }

    /// Default per-blob streaming buffer: 64 MiB sliding window.
    const POPULATE_STREAM_BUFFER_BYTES: u64 = 64 * 1024 * 1024;

    fn get_loader<'a>(&self, key: StoreKey<'a>) -> LoaderGuard<'a> {
        // Get a single loader instance that's used to populate the fast store
        // for this digest.  If another request comes in then it's de-duplicated.
        // Pre-compute the owned key outside the lock to minimize lock hold time.
        let owned_key = key.borrow().into_owned();
        let digest = match key.borrow() {
            StoreKey::Digest(d) => d,
            _ => DigestInfo::zero_digest(),
        };
        let (loader, streaming_inner, is_new) = match self
            .populating_digests
            .lock()
            .entry(owned_key)
        {
            std::collections::hash_map::Entry::Occupied(occupied_entry) => {
                let (l, s) = occupied_entry.get();
                (l.clone(), s.clone(), false)
            }
            std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                let inner = Arc::new(StreamingBlobInner::new(
                    digest,
                    Self::POPULATE_STREAM_BUFFER_BYTES,
                ));
                let entry = vacant_entry.insert((
                    Arc::new(OnceCell::new()),
                    Arc::clone(&inner),
                ));
                (entry.0.clone(), inner, true)
            }
        };
        LoaderGuard {
            weak_store: self.weak_self.clone(),
            key,
            loader: Some(loader),
            streaming_inner,
            is_new,
        }
    }

    async fn populate_and_maybe_stream(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        maybe_writer: Option<&mut DropCloserWriteHalf>,
        offset: u64,
        length: Option<u64>,
        mut streaming_writer: StreamingBlobWriter,
    ) -> Result<(), Error> {
        let reader_stream_size = if self
            .slow_store
            .inner_store(Some(key.borrow()))
            .optimized_for(StoreOptimizations::LazyExistenceOnSync)
        {
            trace!(
                %key,
                store_name = %self.slow_store.inner_store(Some(key.borrow())).get_name(),
                "Skipping .has() check due to LazyExistenceOnSync optimization"
            );
            UploadSizeInfo::MaxSize(u64::MAX)
        } else {
            UploadSizeInfo::ExactSize(self
                    .slow_store
                    .has(key.borrow())
                    .await
                    .err_tip(|| "Failed to run has() on slow store")?
                    .ok_or_else(|| {
                        debug!(
                            %key,
                            slow_store = %self.slow_store.inner_store(Some(key.borrow())).get_name(),
                            "CAS read miss: blob not found in slow store"
                        );
                        make_err!(
                            Code::NotFound,
                            "Object {} not found in either fast or slow store. \
                                If using multiple workers, ensure all workers share the same CAS storage path.",
                            key.as_str()
                        )
                    })?
            )
        };

        let send_range = offset..length.map_or(u64::MAX, |length| length + offset);
        let mut bytes_received: u64 = 0;
        let mut counted_hit = false;

        // Use 128 slots (~32MiB at 256KiB chunks) for dual-store
        // read-through to reduce backpressure between fast and slow stores.
        let (mut fast_tx, fast_rx) = make_buf_channel_pair_with_size(128);
        let (slow_tx, mut slow_rx) = make_buf_channel_pair_with_size(128);
        let data_stream_fut = async move {
            let mut maybe_writer_pin = maybe_writer.map(Pin::new);
            loop {
                let output_buf = slow_rx
                    .recv()
                    .await
                    .err_tip(|| "Failed to read data data buffer from slow store")?;
                if output_buf.is_empty() {
                    // Write out our EOF.
                    // We are dropped as soon as we send_eof to writer_pin, so
                    // we wait until we've finished all of our joins to do that.
                    let fast_res = fast_tx.send_eof();
                    // Signal EOF to streaming waiters.
                    let _ = streaming_writer.send_eof();
                    return Ok::<_, Error>((fast_res, maybe_writer_pin));
                }

                if !counted_hit {
                    self.metrics
                        .slow_store_hit_count
                        .fetch_add(1, Ordering::Acquire);
                    counted_hit = true;
                }

                let output_buf_len = u64::try_from(output_buf.len())
                    .err_tip(|| "Could not output_buf.len() to u64")?;
                self.metrics
                    .slow_store_downloaded_bytes
                    .fetch_add(output_buf_len, Ordering::Acquire);

                let writer_fut = Self::calculate_range(
                    &(bytes_received..bytes_received + output_buf_len),
                    &send_range,
                )?
                .zip(maybe_writer_pin.as_mut())
                .map_or_else(
                    || futures::future::ready(Ok(())).left_future(),
                    |(range, writer_pin)| writer_pin.send(output_buf.slice(range)).right_future(),
                );

                bytes_received += output_buf_len;

                // Tee data to the streaming buffer so waiters can read
                // concurrently instead of blocking until populate completes.
                // Errors are non-fatal (no waiters subscribed yet is fine).
                let _ = streaming_writer.send(output_buf.clone()).await;

                let (fast_tx_res, writer_res) = join!(fast_tx.send(output_buf), writer_fut);
                fast_tx_res.err_tip(|| "Failed to write to fast store in fast_slow store")?;
                writer_res.err_tip(|| "Failed to write result to writer in fast_slow store")?;
            }
        };

        let slow_store_fut = self.slow_store.get(key.borrow(), slow_tx);
        let fast_store_fut = self
            .fast_store
            .update(key.borrow(), fast_rx, reader_stream_size);

        let (data_stream_res, slow_res, fast_res) =
            join!(data_stream_fut, slow_store_fut, fast_store_fut);
        match data_stream_res {
            Ok((fast_eof_res, maybe_writer_pin)) =>
            // Sending the EOF will drop us almost immediately in bytestream_server
            // so we perform it as the very last action in this method.
            {
                fast_eof_res.merge(fast_res).merge(slow_res).merge(
                    if let Some(mut writer_pin) = maybe_writer_pin {
                        writer_pin.send_eof()
                    } else {
                        Ok(())
                    },
                )
            }
            Err(err) => match slow_res {
                Err(slow_err) if slow_err.code == Code::NotFound => Err(slow_err),
                _ => fast_res.merge(slow_res).merge(Err(err)),
            },
        }
    }

    /// Internal helper: copy a blob from the slow store into the fast store,
    /// using the de-duplicating loader. Assumes the caller has already verified
    /// the blob is not in the fast store (or does not care).
    async fn copy_slow_to_fast(&self, key: StoreKey<'_>) -> Result<(), Error> {
        // If the fast store is noop or read only or update only then this is an error.
        if self
            .fast_store
            .inner_store(Some(key.borrow()))
            .optimized_for(StoreOptimizations::NoopUpdates)
            || self.fast_direction == StoreDirection::ReadOnly
            || self.fast_direction == StoreDirection::Update
        {
            return Err(make_err!(
                Code::Internal,
                "Attempt to populate fast store that is read only or noop"
            ));
        }

        let loader_guard = self.get_loader(key.borrow());
        let sw = StreamingBlobWriter::new(loader_guard.streaming_inner.clone());
        loader_guard
            .get_or_try_init(|| {
                Pin::new(self).populate_and_maybe_stream(key.borrow(), None, 0, None, sw)
            })
            .await
            .err_tip(|| "Failed to populate()")
    }

    /// Ensure our fast store is populated. This should be kept as a low
    /// cost function. Since the data itself is shared and not copied it should be fairly
    /// low cost to just discard the data, but does cost a few mutex locks while
    /// streaming.
    pub async fn populate_fast_store(&self, key: StoreKey<'_>) -> Result<(), Error> {
        let maybe_size_info = self
            .fast_store
            .has(key.borrow())
            .await
            .err_tip(|| "While querying in populate_fast_store")?;
        if maybe_size_info.is_some() {
            return Ok(());
        }

        self.copy_slow_to_fast(key).await
    }

    /// Like [`populate_fast_store`](Self::populate_fast_store) but skips the
    /// `has()` check on the fast store. Use this when the caller has already
    /// verified that the blob is missing from the fast store (e.g. via a prior
    /// batch `has_with_results` call) to avoid a redundant existence check.
    pub async fn populate_fast_store_unchecked(&self, key: StoreKey<'_>) -> Result<(), Error> {
        self.copy_slow_to_fast(key).await
    }

    /// Returns the range of bytes that should be sent given a slice bounds
    /// offset so the output range maps the `received_range.start` to 0.
    // TODO(palfrey) This should be put into utils, as this logic is used
    // elsewhere in the code.
    pub fn calculate_range(
        received_range: &Range<u64>,
        send_range: &Range<u64>,
    ) -> Result<Option<Range<usize>>, Error> {
        // Protect against subtraction overflow.
        if received_range.start >= received_range.end {
            return Ok(None);
        }

        let start = max(received_range.start, send_range.start);
        let end = min(received_range.end, send_range.end);
        if received_range.contains(&start) && received_range.contains(&(end - 1)) {
            // Offset both to the start of the received_range.
            let calculated_range_start = usize::try_from(start - received_range.start)
                .err_tip(|| "Could not convert (start - received_range.start) to usize")?;
            let calculated_range_end = usize::try_from(end - received_range.start)
                .err_tip(|| "Could not convert (end - received_range.start) to usize")?;
            Ok(Some(calculated_range_start..calculated_range_end))
        } else {
            Ok(None)
        }
    }
}

#[async_trait]
impl StoreDriver for FastSlowStore {
    async fn has_with_results(
        self: Pin<&Self>,
        key: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        // If our slow store is a noop store, it'll always return a 404,
        // so only check the fast store in such case.
        let slow_store = self.slow_store.inner_store::<StoreKey<'_>>(None);
        if slow_store.optimized_for(StoreOptimizations::NoopDownloads) {
            return self.fast_store.has_with_results(key, results).await;
        }
        // Only check the slow store because if it's not there, then something
        // down stream might be unable to get it.  This should not affect
        // workers as they only use get() and a CAS can use an
        // ExistenceCacheStore to avoid the bottleneck.
        self.slow_store.has_with_results(key, results).await?;
        // Fill in any blobs that are in-flight (written to fast store but
        // background slow write not yet complete).
        {
            let in_flight = self.in_flight_slow_writes.lock();
            if !in_flight.is_empty() {
                for (k, result) in key.iter().zip(results.iter_mut()) {
                    if result.is_none() {
                        let owned = k.borrow().into_owned();
                        if let Some(chunks) = in_flight.get(&owned) {
                            let total_len: u64 =
                                chunks.iter().map(|c| c.len() as u64).sum();
                            debug!(
                                key = %owned.as_str(),
                                data_len = total_len,
                                "has_with_results: found blob in in-flight map \
                                 (not yet on slow store)",
                            );
                            *result = Some(total_len);
                        }
                    }
                }
            }
        }
        // Check mirror blobs for any still-missing digests.
        {
            let mirror = self.mirror_blobs.lock();
            if !mirror.is_empty() {
                for (k, result) in key.iter().zip(results.iter_mut()) {
                    if result.is_none() {
                        let digest = k.borrow().into_digest();
                        if let Some((data, _)) = mirror.get(&digest) {
                            *result = Some(data.len() as u64);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        // Mirror writes: hold blob data in memory only, skip both disk and
        // server. The server already has this blob persisted and is pushing
        // a copy to us for read locality. Data is cleaned up when
        // BlobsInStableStorage arrives or after a TTL.
        let is_mirror = IS_MIRROR_REQUEST.try_with(|v| *v).unwrap_or(false);
        if is_mirror {
            let digest = key.borrow().into_digest();
            let mut chunks = bytes::BytesMut::new();
            loop {
                let chunk = reader
                    .recv()
                    .await
                    .err_tip(|| "mirror recv in FastSlowStore::update")?;
                if chunk.is_empty() {
                    break; // EOF
                }
                chunks.extend_from_slice(&chunk);
            }
            let data = chunks.freeze();
            let data_len = data.len() as u64;
            {
                let mut guard = self.mirror_blobs.lock();
                let current = self.mirror_blobs_total_bytes.load(Ordering::Relaxed);
                if current + data_len > MIRROR_BLOBS_MAX_BYTES {
                    debug!(
                        %digest,
                        data_len,
                        current_total = current,
                        "mirror blob dropped — memory cap exceeded"
                    );
                    return Ok(());
                }
                if let Some((old_data, _)) = guard.insert(digest, (data, Instant::now())) {
                    // Replacing existing entry — adjust by net difference.
                    let old_len = old_data.len() as u64;
                    if data_len >= old_len {
                        self.mirror_blobs_total_bytes.fetch_add(data_len - old_len, Ordering::Relaxed);
                    } else {
                        self.mirror_blobs_total_bytes.fetch_sub(old_len - data_len, Ordering::Relaxed);
                    }
                } else {
                    self.mirror_blobs_total_bytes.fetch_add(data_len, Ordering::Relaxed);
                }
            }
            return Ok(());
        }

        // If either one of our stores is a noop store, bypass the multiplexing
        // and just use the store that is not a noop store.
        let ignore_slow = self
            .slow_store
            .inner_store(Some(key.borrow()))
            .optimized_for(StoreOptimizations::NoopUpdates)
            || self.slow_direction == StoreDirection::ReadOnly
            || self.slow_direction == StoreDirection::Get;
        let ignore_fast = self
            .fast_store
            .inner_store(Some(key.borrow()))
            .optimized_for(StoreOptimizations::NoopUpdates)
            || self.fast_direction == StoreDirection::ReadOnly
            || self.fast_direction == StoreDirection::Get;
        if ignore_slow && ignore_fast {
            // We need to drain the reader to avoid the writer complaining that we dropped
            // the connection prematurely.
            reader
                .drain()
                .await
                .err_tip(|| "In FastFlowStore::update")?;
            return Ok(());
        }
        if ignore_slow {
            let result = self.fast_store.update(key.borrow(), reader, size_info).await;
            if result.is_ok() {
                if let StoreKey::Digest(digest) = &key {
                    self.fast_store.pin_digests(&[*digest]);
                    // Track as needing upload — the slow store was skipped,
                    // so the blob only exists locally. On reconnect the
                    // worker will upload it if the server hasn't acked via
                    // BlobsInStableStorage.
                    self.failed_slow_writes.lock().insert(*digest);
                }
            }
            return result;
        }
        if ignore_fast {
            return self.slow_store.update(key, reader, size_info).await;
        }

        // Decoupled write: stream to fast store while accumulating data,
        // then spawn a background task for the slow store write.
        // This prevents slow-store latency (e.g. ZFS txg sync) from
        // blocking the fast-store (MemoryStore) write path.
        let (mut fast_tx, fast_rx) = make_buf_channel_pair_with_size(128);

        let update_start = std::time::Instant::now();
        debug!(
            ?key,
            ?size_info,
            "FastSlowStore::update: start",
        );

        // Read from upstream, forward to fast store, collect chunks as
        // Vec<Bytes> (O(1) refcount bump per chunk, no copying) for the
        // background slow store write.
        let data_stream_fut = async move {
            let mut chunks: Vec<Bytes> = Vec::new();
            loop {
                let buffer = reader
                    .recv()
                    .await
                    .err_tip(|| "Failed to read buffer in fastslow store")?;
                if buffer.is_empty() {
                    fast_tx.send_eof().err_tip(
                        || "Failed to write eof to fast store in fast_slow store update",
                    )?;
                    return Result::<Vec<Bytes>, Error>::Ok(chunks);
                }
                chunks.push(buffer.clone());
                fast_tx.send(buffer).await.map_err(|e| {
                    make_err!(
                        Code::Internal,
                        "Failed to send message to fast_store in fast_slow_store {:?}",
                        e
                    )
                })?;
            }
        };

        let fast_store_fut = self.fast_store.update(key.borrow(), fast_rx, size_info);
        let (data_res, fast_res) = join!(data_stream_fut, fast_store_fut);
        let data = match data_res {
            Ok(d) => d,
            Err(err) => {
                error!(
                    ?key,
                    elapsed_ms = update_start.elapsed().as_millis() as u64,
                    ?err,
                    "FastSlowStore::update: data stream failed",
                );
                return Err(err);
            }
        };
        if let Err(err) = &fast_res {
            error!(
                ?key,
                elapsed_ms = update_start.elapsed().as_millis() as u64,
                ?err,
                "FastSlowStore::update: fast store write failed",
            );
        }
        fast_res?;

        // Pin the digest in the fast store to prevent eviction until the
        // server confirms stable storage via BlobsInStableStorage.
        if let StoreKey::Digest(digest) = &key {
            self.fast_store.pin_digests(&[*digest]);
        }

        let bytes_sent: u64 = data.iter().map(|c| c.len() as u64).sum();
        let fast_elapsed = update_start.elapsed();
        debug!(
            ?key,
            fast_ms = fast_elapsed.as_millis(),
            total_bytes = bytes_sent,
            "FastSlowStore::update: fast store complete, spawning background slow write",
        );

        // During shutdown, write directly to the slow store (blocking the
        // caller) instead of spawning a background task that would be killed.
        if self.shutting_down.load(Ordering::Acquire) {
            let (mut tx, rx) = make_buf_channel_pair_with_size(128);
            let write_fut = self.slow_store.update(key.borrow(), rx, size_info);
            let send_fut = async {
                for chunk in data {
                    tx.send(chunk).await.map_err(|e| {
                        make_err!(Code::Internal, "shutdown flush send: {:?}", e)
                    })?;
                }
                tx.send_eof()
                    .err_tip(|| "shutdown flush send_eof")?;
                Result::<(), Error>::Ok(())
            };
            let (write_result, send_result) = tokio::join!(write_fut, send_fut);
            return send_result.and(write_result);
        }

        // Insert into in-flight map so get_part can serve this blob even if
        // the fast store evicts it before the slow write completes.
        let owned_key = key.borrow().into_owned();
        self.in_flight_slow_writes
            .lock()
            .insert(owned_key.clone(), data.clone());

        let in_flight = self.in_flight_slow_writes.clone();
        let in_flight_empty = self.in_flight_empty_notify.clone();
        let stable_digests_ref = self.stable_digests.clone();
        let stable_notify_ref = self.stable_notify.clone();
        let failed_writes_ref = self.failed_slow_writes.clone();
        let fast_store_ref = self.fast_store.clone();
        let slow_store = self.slow_store.clone();
        let key_for_bg = owned_key.clone();
        let spawn_instant = std::time::Instant::now();
        debug!(
            ?key,
            total_bytes = bytes_sent,
            "FastSlowStore::update: background slow write starting",
        );
        tokio::spawn(async move {
            let schedule_delay_ms = spawn_instant.elapsed().as_millis();
            if schedule_delay_ms > 100 {
                warn!(
                    key = ?key_for_bg,
                    schedule_delay_ms,
                    total_bytes = bytes_sent,
                    "FastSlowStore: background slow write task was \
                     delayed before starting",
                );
            }
            let slow_start = std::time::Instant::now();
            // Stream collected chunks to slow store via buf_channel,
            // avoiding a single large concatenation.
            let (mut slow_tx, slow_rx) = make_buf_channel_pair_with_size(128);
            let write_fut = slow_store.update(
                key_for_bg.borrow(),
                slow_rx,
                UploadSizeInfo::ExactSize(bytes_sent),
            );
            let send_fut = async {
                for chunk in data {
                    slow_tx.send(chunk).await.map_err(|e| {
                        make_err!(
                            Code::Internal,
                            "Failed to send chunk to slow store: {:?}",
                            e
                        )
                    })?;
                }
                slow_tx.send_eof().err_tip(
                    || "Failed to send eof to slow store in background write",
                )?;
                Result::<(), Error>::Ok(())
            };
            let (write_result, send_result) = tokio::join!(write_fut, send_fut);
            {
                let mut guard = in_flight.lock();
                guard.remove(&key_for_bg);
                if guard.is_empty() {
                    in_flight_empty.notify_waiters();
                }
            }
            let slow_ms = slow_start.elapsed().as_millis();
            let result = send_result.and(write_result);
            match result {
                Ok(()) => {
                    if let StoreKey::Digest(digest) = &key_for_bg {
                        stable_digests_ref.lock().push(*digest);
                        stable_notify_ref.notify_one();
                    }
                    debug!(
                        key = ?key_for_bg,
                        schedule_delay_ms,
                        slow_ms,
                        total_bytes = bytes_sent,
                        "FastSlowStore::update: background slow write complete",
                    );
                }
                Err(e) => {
                    if let StoreKey::Digest(digest) = &key_for_bg {
                        failed_writes_ref.lock().insert(*digest);
                        // Re-pin so the blob survives until reconnect retry.
                        // Without this, the 120s auto-expire could allow
                        // eviction before the worker reconnects.
                        fast_store_ref.pin_digests(&[*digest]);
                    }
                    error!(
                        key = ?key_for_bg,
                        schedule_delay_ms,
                        slow_ms,
                        total_bytes = bytes_sent,
                        error = ?e,
                        "FastSlowStore::update: background slow write FAILED — \
                         blob pinned, will retry on reconnect",
                    );
                }
            }
        });

        Ok(())
    }

    async fn update_oneshot(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        data: Bytes,
    ) -> Result<(), Error> {
        // Mirror writes: hold in memory only.
        let is_mirror = IS_MIRROR_REQUEST.try_with(|v| *v).unwrap_or(false);
        if is_mirror {
            let digest = key.borrow().into_digest();
            let data_len = data.len() as u64;
            {
                let mut guard = self.mirror_blobs.lock();
                let current = self.mirror_blobs_total_bytes.load(Ordering::Relaxed);
                if current + data_len > MIRROR_BLOBS_MAX_BYTES {
                    debug!(
                        %digest,
                        data_len,
                        current_total = current,
                        "mirror blob dropped — memory cap exceeded"
                    );
                    return Ok(());
                }
                if let Some((old_data, _)) = guard.insert(digest, (data, Instant::now())) {
                    let old_len = old_data.len() as u64;
                    if data_len >= old_len {
                        self.mirror_blobs_total_bytes.fetch_add(data_len - old_len, Ordering::Relaxed);
                    } else {
                        self.mirror_blobs_total_bytes.fetch_sub(old_len - data_len, Ordering::Relaxed);
                    }
                } else {
                    self.mirror_blobs_total_bytes.fetch_add(data_len, Ordering::Relaxed);
                }
            }
            return Ok(());
        }

        let ignore_slow = self
            .slow_store
            .inner_store(Some(key.borrow()))
            .optimized_for(StoreOptimizations::NoopUpdates)
            || self.slow_direction == StoreDirection::ReadOnly
            || self.slow_direction == StoreDirection::Get;
        let ignore_fast = self
            .fast_store
            .inner_store(Some(key.borrow()))
            .optimized_for(StoreOptimizations::NoopUpdates)
            || self.fast_direction == StoreDirection::ReadOnly
            || self.fast_direction == StoreDirection::Get;

        if ignore_slow && ignore_fast {
            return Ok(());
        }
        if ignore_slow {
            let result = self.fast_store.update_oneshot(key.borrow(), data).await;
            if result.is_ok() {
                if let StoreKey::Digest(digest) = &key {
                    self.fast_store.pin_digests(&[*digest]);
                    self.failed_slow_writes.lock().insert(*digest);
                }
            }
            return result;
        }
        if ignore_fast {
            return self.slow_store.update_oneshot(key, data).await;
        }

        let data_len = data.len();
        debug!(
            ?key,
            data_len,
            "FastSlowStore::update_oneshot: start",
        );

        // Write to fast store first (blocking — typically MemoryStore, near-instant).
        let fast_start = std::time::Instant::now();
        let fast_result = self
            .fast_store
            .update_oneshot(key.borrow(), data.clone())
            .await;
        let fast_ms = fast_start.elapsed().as_millis();
        if let Err(ref err) = fast_result {
            error!(
                ?key,
                fast_ms,
                data_len,
                ?err,
                "FastSlowStore::update_oneshot: fast store write failed",
            );
        }
        fast_result?;

        // Pin the digest in the fast store to prevent eviction until the
        // server confirms stable storage via BlobsInStableStorage.
        if let StoreKey::Digest(digest) = &key {
            self.fast_store.pin_digests(&[*digest]);
        }

        // During shutdown, write directly instead of spawning background task.
        if self.shutting_down.load(Ordering::Acquire) {
            return self.slow_store.update_oneshot(key, data).await;
        }

        // Spawn background slow store write.
        let owned_key = key.borrow().into_owned();
        self.in_flight_slow_writes
            .lock()
            .insert(owned_key.clone(), vec![data.clone()]);

        let in_flight = self.in_flight_slow_writes.clone();
        let in_flight_empty = self.in_flight_empty_notify.clone();
        let stable_digests_ref = self.stable_digests.clone();
        let stable_notify_ref = self.stable_notify.clone();
        let failed_writes_ref = self.failed_slow_writes.clone();
        let fast_store_ref = self.fast_store.clone();
        let slow_store = self.slow_store.clone();
        let key_for_bg = owned_key.clone();
        let spawn_instant = std::time::Instant::now();
        debug!(
            ?key,
            data_len,
            "FastSlowStore::update_oneshot: background slow write starting",
        );
        tokio::spawn(async move {
            let schedule_delay_ms = spawn_instant.elapsed().as_millis();
            if schedule_delay_ms > 100 {
                warn!(
                    key = ?key_for_bg,
                    schedule_delay_ms,
                    data_len,
                    "FastSlowStore::update_oneshot: background slow write task \
                     was delayed before starting",
                );
            }
            let slow_start = std::time::Instant::now();
            let result = slow_store
                .update_oneshot(key_for_bg.borrow(), data)
                .await;
            {
                let mut guard = in_flight.lock();
                guard.remove(&key_for_bg);
                if guard.is_empty() {
                    in_flight_empty.notify_waiters();
                }
            }
            let slow_ms = slow_start.elapsed().as_millis();
            match result {
                Ok(()) => {
                    if let StoreKey::Digest(digest) = &key_for_bg {
                        stable_digests_ref.lock().push(*digest);
                        stable_notify_ref.notify_one();
                    }
                    debug!(
                        key = ?key_for_bg,
                        schedule_delay_ms,
                        slow_ms,
                        data_len,
                        "FastSlowStore::update_oneshot: background slow write complete",
                    );
                }
                Err(e) => {
                    if let StoreKey::Digest(digest) = &key_for_bg {
                        failed_writes_ref.lock().insert(*digest);
                        // Re-pin so the blob survives until reconnect retry.
                        fast_store_ref.pin_digests(&[*digest]);
                    }
                    error!(
                        key = ?key_for_bg,
                        schedule_delay_ms,
                        slow_ms,
                        data_len,
                        error = ?e,
                        "FastSlowStore::update_oneshot: background slow write FAILED — \
                         blob pinned, will retry on reconnect",
                    );
                }
            }
        });

        Ok(())
    }

    /// `FastSlowStore` has optimizations for dealing with files.
    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        optimization == StoreOptimizations::FileUpdates
    }

    /// Optimized variation to consume the file if one of the stores is a
    /// filesystem store. This makes the operation a move instead of a copy
    /// dramatically increasing performance for large files.
    async fn update_with_whole_file(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        path: OsString,
        mut file: fs::FileSlot,
        upload_size: UploadSizeInfo,
    ) -> Result<Option<fs::FileSlot>, Error> {
        trace!(
            key = ?key,
            ?upload_size,
            "FastSlowStore::update_with_whole_file: starting",
        );
        if self
            .fast_store
            .optimized_for(StoreOptimizations::FileUpdates)
        {
            if !self
                .slow_store
                .inner_store(Some(key.borrow()))
                .optimized_for(StoreOptimizations::NoopUpdates)
                && self.slow_direction != StoreDirection::ReadOnly
                && self.slow_direction != StoreDirection::Get
            {
                // Intentionally write to slow store (remote CAS) synchronously
                // before the fast store. This ensures the blob reaches the
                // remote server before the action result is reported, avoiding
                // the case where an AC entry references CAS digests that were
                // never actually uploaded.
                trace!("FastSlowStore::update_with_whole_file: uploading to slow_store");
                let slow_start = std::time::Instant::now();
                file = slow_update_store_with_file(
                    self.slow_store.as_store_driver_pin(),
                    key.borrow(),
                    file,
                    upload_size,
                )
                .await
                .err_tip(|| "In FastSlowStore::update_with_whole_file slow_store")?;
                trace!(
                    elapsed_ms = slow_start.elapsed().as_millis(),
                    "FastSlowStore::update_with_whole_file: slow_store upload completed",
                );
            }
            if self.fast_direction == StoreDirection::ReadOnly
                || self.fast_direction == StoreDirection::Get
            {
                return Ok(Some(file));
            }
            return self
                .fast_store
                .update_with_whole_file(key, path, file, upload_size)
                .await;
        }

        if self
            .slow_store
            .optimized_for(StoreOptimizations::FileUpdates)
        {
            let ignore_fast = self
                .fast_store
                .inner_store(Some(key.borrow()))
                .optimized_for(StoreOptimizations::NoopUpdates)
                || self.fast_direction == StoreDirection::ReadOnly
                || self.fast_direction == StoreDirection::Get;
            if !ignore_fast {
                file = slow_update_store_with_file(
                    self.fast_store.as_store_driver_pin(),
                    key.borrow(),
                    file,
                    upload_size,
                )
                .await
                .err_tip(|| "In FastSlowStore::update_with_whole_file fast_store")?;
            }
            let ignore_slow = self.slow_direction == StoreDirection::ReadOnly
                || self.slow_direction == StoreDirection::Get;
            if ignore_slow {
                return Ok(Some(file));
            }
            return self
                .slow_store
                .update_with_whole_file(key, path, file, upload_size)
                .await;
        }

        let file = slow_update_store_with_file(self, key, file, upload_size)
            .await
            .err_tip(|| "In FastSlowStore::update_with_whole_file")?;
        Ok(Some(file))
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        // Check mirror blob cache first — these are blobs the server pushed
        // to us that we hold in memory only.
        {
            let digest = key.borrow().into_digest();
            let maybe_data = self.mirror_blobs.lock().get(&digest).map(|(d, _)| d.clone());
            if let Some(data) = maybe_data {
                let offset_usize = usize::try_from(offset).unwrap_or(usize::MAX);
                if offset_usize < data.len() {
                    let end = length
                        .and_then(|l| usize::try_from(l).ok())
                        .map(|l| offset_usize.saturating_add(l).min(data.len()))
                        .unwrap_or(data.len());
                    let slice = data.slice(offset_usize..end);
                    if !slice.is_empty() {
                        writer
                            .send(slice)
                            .await
                            .err_tip(|| "Failed to send mirror blob data")?;
                    }
                }
                writer
                    .send_eof()
                    .err_tip(|| "Failed to send EOF for mirror blob")?;
                return Ok(());
            }
        }

        let fast_has = self.fast_store.has(key.borrow()).await?;
        let expected_size = match key.borrow() {
            StoreKey::Digest(d) => d.size_bytes(),
            StoreKey::Str(_) => 0, // Can't validate size for string keys.
        };
        let fast_valid = match fast_has {
            Some(size) if expected_size > 0 && size < expected_size => {
                // Fast store has the key but with less data than expected —
                // truncated/corrupt entry. Skip it and fall through to the
                // slow store for correct data.
                // Note: size > expected_size is normal because FilesystemStore
                // reports size_on_disk (block-aligned), not data size.
                error!(
                    ?key,
                    fast_size = size,
                    expected_size,
                    "fast store has truncated entry, skipping to slow store"
                );
                false
            }
            Some(_) => true,
            None => false,
        };
        if fast_valid {
            // Try the fast store first. If the item was evicted between the
            // has() check and this get_part() call (TOCTOU race), fall through
            // to the slow-store path instead of propagating NotFound.
            match self
                .fast_store
                .get_part(key.borrow(), writer.borrow_mut(), offset, length)
                .await
            {
                Ok(()) => {
                    self.metrics
                        .fast_store_hit_count
                        .fetch_add(1, Ordering::Acquire);
                    self.metrics
                        .fast_store_downloaded_bytes
                        .fetch_add(writer.get_bytes_written(), Ordering::Acquire);
                    return Ok(());
                }
                Err(err) if err.code == Code::NotFound && writer.get_bytes_written() == 0 => {
                    // Item was evicted between has() and get_part().
                    // Only safe to fall through if no bytes were written yet.
                    debug!(
                        ?key,
                        "Fast store item evicted between has() and get_part(), falling through to slow store"
                    );
                }
                Err(err) => return Err(err),
            }
        }

        // Check in-flight slow writes: the blob may have been evicted from the
        // fast store while its background slow-store write is still in progress.
        {
            let owned_key = key.borrow().into_owned();
            let maybe_chunks = self.in_flight_slow_writes.lock().get(&owned_key).cloned();
            if let Some(chunks) = maybe_chunks {
                let total_len: usize = chunks.iter().map(|c| c.len()).sum();
                let offset_usize = usize::try_from(offset)
                    .err_tip(|| "Could not convert offset to usize")?;
                let end = length
                    .and_then(|l| usize::try_from(l).ok())
                    .map(|l| (offset_usize.saturating_add(l)).min(total_len))
                    .unwrap_or(total_len);
                if offset_usize < end {
                    // Walk the chunk list, skipping/slicing to honor offset and length.
                    let mut pos: usize = 0;
                    for chunk in &chunks {
                        let chunk_end = pos + chunk.len();
                        if chunk_end <= offset_usize {
                            pos = chunk_end;
                            continue;
                        }
                        if pos >= end {
                            break;
                        }
                        let start_in_chunk = offset_usize.saturating_sub(pos);
                        let end_in_chunk = (end - pos).min(chunk.len());
                        writer
                            .send(chunk.slice(start_in_chunk..end_in_chunk))
                            .await
                            .err_tip(|| "Failed to send in-flight data in fast_slow get_part")?;
                        pos = chunk_end;
                    }
                }
                writer
                    .send_eof()
                    .err_tip(|| "Failed to send EOF for in-flight data")?;
                debug!(
                    ?key,
                    data_len = total_len,
                    "Served blob from in-flight slow-write buffer (fast store evicted it)",
                );
                return Ok(());
            }
        }

        // If the fast store is noop or read only or update only then bypass it.
        if self
            .fast_store
            .inner_store(Some(key.borrow()))
            .optimized_for(StoreOptimizations::NoopUpdates)
            || self.fast_direction == StoreDirection::ReadOnly
            || self.fast_direction == StoreDirection::Update
        {
            self.metrics
                .slow_store_hit_count
                .fetch_add(1, Ordering::Acquire);
            self.slow_store
                .get_part(key, writer.borrow_mut(), offset, length)
                .await?;
            self.metrics
                .slow_store_downloaded_bytes
                .fetch_add(writer.get_bytes_written(), Ordering::Acquire);
            return Ok(());
        }

        let loader_guard = self.get_loader(key.borrow());
        let streaming_inner = loader_guard.streaming_inner.clone();
        let is_waiter = !loader_guard.is_new;

        if is_waiter {
            // Another thread is populating — stream from the populate buffer
            // concurrently. Data arrives as each chunk is read from the slow
            // store, giving us near-zero time-to-first-byte instead of
            // blocking until the full populate completes.
            //
            // For blobs larger than the sliding window, the reader may miss
            // early chunks. Detect this by checking if eviction has already
            // started and fall back to slow store.
            drop(loader_guard);
            let earliest = streaming_inner.earliest_chunk_idx();
            if earliest > 0 {
                // Chunks already evicted — can't serve from byte 0.
                debug!(
                    ?key,
                    earliest,
                    "streaming populate: chunks evicted, falling back to slow store"
                );
                return self.slow_store
                    .get_part(key.borrow(), &mut *writer, offset, length)
                    .await;
            }
            let mut reader = nativelink_util::streaming_blob::StreamingBlobReader::new(
                streaming_inner,
            );
            let mut pos = 0u64;
            let end = offset + length.unwrap_or(u64::MAX);
            loop {
                match reader.next_chunk().await {
                    Ok(chunk) if chunk.is_empty() => break, // EOF
                    Ok(chunk) => {
                        let chunk_end = pos + chunk.len() as u64;
                        if chunk_end > offset && pos < end {
                            let start = if pos < offset {
                                (offset - pos) as usize
                            } else {
                                0
                            };
                            let stop = if chunk_end > end {
                                chunk.len() - (chunk_end - end) as usize
                            } else {
                                chunk.len()
                            };
                            if start < stop {
                                writer
                                    .send(chunk.slice(start..stop))
                                    .await
                                    .err_tip(|| "Failed to send streaming populate data")?;
                            }
                        }
                        pos = chunk_end;
                        if pos >= end {
                            break;
                        }
                    }
                    Err(err) => {
                        // Streaming buffer error (populate failed or cursor
                        // fell behind sliding window). Fall back to slow store.
                        warn!(
                            ?key,
                            %err,
                            "streaming populate reader error, falling back to slow store"
                        );
                        return self.slow_store
                            .get_part(key.borrow(), &mut *writer, offset, length)
                            .await;
                    }
                }
            }
            writer
                .send_eof()
                .err_tip(|| "Failed to send EOF after streaming populate")?;
            Ok(())
        } else {
            // We're the populator — stream to the client directly AND tee
            // data into the streaming buffer for any concurrent waiters.
            let sw = StreamingBlobWriter::new(streaming_inner);
            loader_guard
                .get_or_try_init(|| {
                    self.populate_and_maybe_stream(
                        key.borrow(),
                        Some(writer),
                        offset,
                        length,
                        sw,
                    )
                })
                .await
        }
    }

    fn inner_store(&self, _key: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any<'a>(&'a self) -> &'a (dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_item_callback(
        self: Arc<Self>,
        callback: Arc<dyn ItemCallback>,
    ) -> Result<(), Error> {
        self.fast_store.register_item_callback(callback.clone())?;
        self.slow_store.register_item_callback(callback)?;
        Ok(())
    }

    fn drain_stable_digests(&self) -> Vec<DigestInfo> {
        let mut guard = self.stable_digests.lock();
        std::mem::take(&mut *guard)
    }

    fn stable_notify(&self) -> Arc<Notify> {
        self.stable_notify.clone()
    }

    fn pin_digests(&self, digests: &[DigestInfo]) {
        self.fast_store.pin_digests(digests);
        self.slow_store.pin_digests(digests);
    }

    fn drain_failed_digests(&self) -> Vec<DigestInfo> {
        let mut guard = self.failed_slow_writes.lock();
        guard.drain().collect()
    }
}

#[derive(Debug, Default, MetricsComponent)]
struct FastSlowStoreMetrics {
    #[metric(help = "Hit count for the fast store")]
    fast_store_hit_count: AtomicU64,
    #[metric(help = "Downloaded bytes from the fast store")]
    fast_store_downloaded_bytes: AtomicU64,
    #[metric(help = "Hit count for the slow store")]
    slow_store_hit_count: AtomicU64,
    #[metric(help = "Downloaded bytes from the slow store")]
    slow_store_downloaded_bytes: AtomicU64,
}

impl Drop for FastSlowStore {
    fn drop(&mut self) {
        let guard = self.in_flight_slow_writes.lock();
        if guard.is_empty() {
            return;
        }
        warn!(
            count = guard.len(),
            "FastSlowStore: dropping with in-flight slow writes, \
             these blobs will NOT be persisted to the slow store"
        );
        for (key, chunks) in guard.iter() {
            let bytes: usize = chunks.iter().map(|b| b.len()).sum();
            warn!(?key, bytes, "FastSlowStore: unflushed write lost on shutdown");
        }
    }
}

default_health_status_indicator!(FastSlowStore);
