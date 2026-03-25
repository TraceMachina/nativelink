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
use core::sync::atomic::{AtomicU64, Ordering};
use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
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
    ItemCallback, Store, StoreDriver, StoreKey, StoreLike, StoreOptimizations,
    UploadSizeInfo, slow_update_store_with_file,
};
use parking_lot::Mutex;
use tokio::sync::OnceCell;
use tracing::{debug, error, info, trace, warn};

// TODO(palfrey) This store needs to be evaluated for more efficient memory usage,
// there are many copies happening internally.

type Loader = Arc<OnceCell<()>>;

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
    populating_digests: Mutex<HashMap<StoreKey<'static>, Loader>>,
    /// Holds data for blobs whose background slow-store write is still in
    /// progress. If the fast store evicts the blob before the slow write
    /// completes, `get_part` serves from this map to prevent NotFound gaps.
    in_flight_slow_writes: Arc<Mutex<HashMap<StoreKey<'static>, Bytes>>>,
    /// Digests that have completed their background slow store write.
    /// Drained every 100ms by the BlobsInStableStorage batching loop.
    stable_digests: Arc<Mutex<Vec<DigestInfo>>>,
}

// This guard ensures that the populating_digests is cleared even if the future
// is dropped, it is cancel safe.
struct LoaderGuard<'a> {
    weak_store: Weak<FastSlowStore>,
    key: StoreKey<'a>,
    loader: Option<Loader>,
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
            if Arc::ptr_eq(occupied_entry.get(), &loader) {
                drop(loader);
                if Arc::strong_count(occupied_entry.get()) == 1 {
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
            stable_digests: Arc::new(Mutex::new(Vec::new())),
        })
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

    fn get_loader<'a>(&self, key: StoreKey<'a>) -> LoaderGuard<'a> {
        // Get a single loader instance that's used to populate the fast store
        // for this digest.  If another request comes in then it's de-duplicated.
        // Pre-compute the owned key outside the lock to minimize lock hold time.
        let owned_key = key.borrow().into_owned();
        let loader = match self
            .populating_digests
            .lock()
            .entry(owned_key)
        {
            std::collections::hash_map::Entry::Occupied(occupied_entry) => {
                occupied_entry.get().clone()
            }
            std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                vacant_entry.insert(Arc::new(OnceCell::new())).clone()
            }
        };
        LoaderGuard {
            weak_store: self.weak_self.clone(),
            key,
            loader: Some(loader),
        }
    }

    async fn populate_and_maybe_stream(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        maybe_writer: Option<&mut DropCloserWriteHalf>,
        offset: u64,
        length: Option<u64>,
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

        self.get_loader(key.borrow())
            .get_or_try_init(|| {
                Pin::new(self).populate_and_maybe_stream(key.borrow(), None, 0, None)
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
                        if let Some(data) = in_flight.get(&owned) {
                            info!(
                                key = %owned.as_str(),
                                data_len = data.len(),
                                "has_with_results: found blob in in-flight map \
                                 (not yet on slow store)",
                            );
                            *result = Some(data.len() as u64);
                        }
                    }
                }
            }
            // Diagnostic: log when small blobs are missing from both slow
            // store and in-flight map — these cause FAILED_PRECONDITION.
            for (k, result) in key.iter().zip(results.iter()) {
                if result.is_none() {
                    let key_str = k.as_str();
                    if let Some(size_str) = key_str.rsplit('-').next() {
                        if let Ok(size) = size_str.parse::<u64>() {
                            if size < 1024 {
                                warn!(
                                    key = %key_str,
                                    in_flight_count = in_flight.len(),
                                    "has_with_results: small blob NOT FOUND in \
                                     slow store or in-flight map",
                                );
                            }
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
            return self.fast_store.update(key, reader, size_info).await;
        }
        if ignore_fast {
            return self.slow_store.update(key, reader, size_info).await;
        }

        // Decoupled write: stream to fast store while accumulating data,
        // then spawn a background task for the slow store write.
        // This prevents slow-store latency (e.g. ZFS txg sync) from
        // blocking the fast-store (MemoryStore) write path.
        let (mut fast_tx, fast_rx) = make_buf_channel_pair_with_size(128);

        let key_debug = format!("{key:?}");
        let update_start = std::time::Instant::now();
        info!(
            key = %key_debug,
            ?size_info,
            "FastSlowStore::update: start",
        );

        // Read from upstream, forward to fast store, build combined buffer
        // for background slow store write in a single pass (no second copy).
        let initial_cap = match size_info {
            UploadSizeInfo::ExactSize(s) => (s as usize).min(256 * 1024 * 1024),
            UploadSizeInfo::MaxSize(s) => (s as usize).min(64 * 1024 * 1024),
        };
        let data_stream_fut = async move {
            let mut combined = BytesMut::with_capacity(initial_cap);
            loop {
                let buffer = reader
                    .recv()
                    .await
                    .err_tip(|| "Failed to read buffer in fastslow store")?;
                if buffer.is_empty() {
                    fast_tx.send_eof().err_tip(
                        || "Failed to write eof to fast store in fast_slow store update",
                    )?;
                    return Result::<Bytes, Error>::Ok(combined.freeze());
                }
                combined.extend_from_slice(&buffer);
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
                    key = %key_debug,
                    elapsed_ms = update_start.elapsed().as_millis() as u64,
                    ?err,
                    "FastSlowStore::update: data stream failed",
                );
                return Err(err);
            }
        };
        if let Err(err) = &fast_res {
            error!(
                key = %key_debug,
                elapsed_ms = update_start.elapsed().as_millis() as u64,
                ?err,
                "FastSlowStore::update: fast store write failed",
            );
        }
        fast_res?;

        let bytes_sent = data.len() as u64;
        let fast_elapsed = update_start.elapsed();
        debug!(
            key = %key_debug,
            fast_ms = fast_elapsed.as_millis(),
            total_bytes = bytes_sent,
            "FastSlowStore::update: fast store complete, spawning background slow write",
        );

        // Insert into in-flight map so get_part can serve this blob even if
        // the fast store evicts it before the slow write completes.
        let owned_key = key.borrow().into_owned();
        self.in_flight_slow_writes
            .lock()
            .insert(owned_key.clone(), data.clone());

        let in_flight = self.in_flight_slow_writes.clone();
        let stable_digests_ref = self.stable_digests.clone();
        let slow_store = self.slow_store.clone();
        let key_for_bg = owned_key.clone();
        let key_debug_bg = key_debug.clone();
        let spawn_instant = std::time::Instant::now();
        info!(
            key = %key_debug,
            total_bytes = bytes_sent,
            "FastSlowStore::update: background slow write starting",
        );
        tokio::spawn(async move {
            let schedule_delay_ms = spawn_instant.elapsed().as_millis();
            if schedule_delay_ms > 100 {
                warn!(
                    key = %key_debug_bg,
                    schedule_delay_ms,
                    total_bytes = bytes_sent,
                    "FastSlowStore: background slow write task was \
                     delayed before starting",
                );
            }
            let slow_start = std::time::Instant::now();
            let result = slow_store
                .update_oneshot(key_for_bg.borrow(), data)
                .await;
            in_flight.lock().remove(&key_for_bg);
            let slow_ms = slow_start.elapsed().as_millis();
            match result {
                Ok(()) => {
                    if let StoreKey::Digest(digest) = &key_for_bg {
                        stable_digests_ref.lock().push(*digest);
                    }
                    info!(
                        key = %key_debug_bg,
                        schedule_delay_ms,
                        slow_ms,
                        total_bytes = bytes_sent,
                        "FastSlowStore::update: background slow write complete",
                    );
                }
                Err(e) => error!(
                    key = %key_debug_bg,
                    schedule_delay_ms,
                    slow_ms,
                    total_bytes = bytes_sent,
                    error = ?e,
                    "FastSlowStore::update: background slow write FAILED — \
                     blob may be lost when fast store evicts it",
                ),
            }
        });

        Ok(())
    }

    async fn update_oneshot(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        data: Bytes,
    ) -> Result<(), Error> {
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
            return self.fast_store.update_oneshot(key, data).await;
        }
        if ignore_fast {
            return self.slow_store.update_oneshot(key, data).await;
        }

        let key_debug = format!("{key:?}");
        let data_len = data.len();
        info!(
            key = %key_debug,
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
                key = %key_debug,
                fast_ms,
                data_len,
                ?err,
                "FastSlowStore::update_oneshot: fast store write failed",
            );
        }
        fast_result?;

        // Spawn background slow store write.
        let owned_key = key.borrow().into_owned();
        self.in_flight_slow_writes
            .lock()
            .insert(owned_key.clone(), data.clone());

        let in_flight = self.in_flight_slow_writes.clone();
        let stable_digests_ref = self.stable_digests.clone();
        let slow_store = self.slow_store.clone();
        let key_for_bg = owned_key.clone();
        let key_debug_bg = key_debug.clone();
        let spawn_instant = std::time::Instant::now();
        info!(
            key = %key_debug,
            data_len,
            "FastSlowStore::update_oneshot: background slow write starting",
        );
        tokio::spawn(async move {
            let schedule_delay_ms = spawn_instant.elapsed().as_millis();
            if schedule_delay_ms > 100 {
                warn!(
                    key = %key_debug_bg,
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
            in_flight.lock().remove(&key_for_bg);
            let slow_ms = slow_start.elapsed().as_millis();
            match result {
                Ok(()) => {
                    if let StoreKey::Digest(digest) = &key_for_bg {
                        stable_digests_ref.lock().push(*digest);
                    }
                    info!(
                        key = %key_debug_bg,
                        schedule_delay_ms,
                        slow_ms,
                        data_len,
                        "FastSlowStore::update_oneshot: background slow write complete",
                    );
                }
                Err(e) => error!(
                    key = %key_debug_bg,
                    schedule_delay_ms,
                    slow_ms,
                    data_len,
                    error = ?e,
                    "FastSlowStore::update_oneshot: background slow write FAILED",
                ),
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
        if self.fast_store.has(key.borrow()).await?.is_some() {
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
            let maybe_data = self.in_flight_slow_writes.lock().get(&owned_key).cloned();
            if let Some(data) = maybe_data {
                let data_len = data.len();
                let offset_usize = usize::try_from(offset)
                    .err_tip(|| "Could not convert offset to usize")?;
                let end = length
                    .and_then(|l| usize::try_from(l).ok())
                    .map(|l| (offset_usize.saturating_add(l)).min(data_len))
                    .unwrap_or(data_len);
                if offset_usize < end {
                    writer
                        .send(data.slice(offset_usize..end))
                        .await
                        .err_tip(|| "Failed to send in-flight data in fast_slow get_part")?;
                }
                writer
                    .send_eof()
                    .err_tip(|| "Failed to send EOF for in-flight data")?;
                debug!(
                    ?key,
                    data_len,
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

        let mut writer = Some(writer);
        self.get_loader(key.borrow())
            .get_or_try_init(|| {
                self.populate_and_maybe_stream(key.borrow(), writer.take(), offset, length)
            })
            .await?;

        // If we were a waiter (not the streaming thread), read from the
        // fast store which was just populated. If the blob was evicted
        // between populate and this read, fall back directly to the slow
        // store instead of recursing (which could loop indefinitely under
        // heavy eviction pressure).
        if let Some(writer) = writer.take() {
            let bytes_before = writer.get_bytes_written();
            match self
                .fast_store
                .get_part(key.borrow(), &mut *writer, offset, length)
                .await
            {
                Ok(()) => Ok(()),
                Err(err)
                    if err.code == Code::NotFound
                        && writer.get_bytes_written() == bytes_before =>
                {
                    warn!(
                        ?key,
                        "Fast store item evicted immediately after population, \
                         reading directly from slow store"
                    );
                    self.slow_store
                        .get_part(key, &mut *writer, offset, length)
                        .await
                }
                Err(err) => Err(err),
            }
        } else {
            // This was the thread that did the streaming already.
            Ok(())
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

default_health_status_indicator!(FastSlowStore);
