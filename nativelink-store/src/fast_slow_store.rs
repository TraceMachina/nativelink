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
use bytes::Bytes;
use futures::{FutureExt, join};
use nativelink_config::stores::{FastSlowSpec, StoreDirection};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
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
use tracing::{debug, trace, warn};

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
        self.slow_store.has_with_results(key, results).await
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

        // Use 128 slots (~32MiB at 256KiB chunks) for dual-store
        // update to reduce backpressure between fast and slow stores.
        let (mut fast_tx, fast_rx) = make_buf_channel_pair_with_size(128);
        let (mut slow_tx, slow_rx) = make_buf_channel_pair_with_size(128);

        let key_debug = format!("{key:?}");
        trace!(
            key = %key_debug,
            "FastSlowStore::update: starting dual-store upload",
        );
        let update_start = std::time::Instant::now();
        let mut bytes_sent: u64 = 0;

        let data_stream_fut = async move {
            loop {
                let buffer = reader
                    .recv()
                    .await
                    .err_tip(|| "Failed to read buffer in fastslow store")?;
                if buffer.is_empty() {
                    // EOF received.
                    fast_tx.send_eof().err_tip(
                        || "Failed to write eof to fast store in fast_slow store update",
                    )?;
                    slow_tx
                        .send_eof()
                        .err_tip(|| "Failed to write eof to writer in fast_slow store update")?;
                    debug!(
                        total_bytes = bytes_sent,
                        "FastSlowStore::update: data_stream sent EOF to both stores",
                    );
                    return Result::<(), Error>::Ok(());
                }

                let chunk_len = buffer.len();
                let send_start = std::time::Instant::now();
                let (fast_result, slow_result) =
                    join!(fast_tx.send(buffer.clone()), slow_tx.send(buffer));
                let send_elapsed = send_start.elapsed();
                if send_elapsed.as_secs() >= 5 {
                    warn!(
                        chunk_len,
                        send_elapsed_ms = send_elapsed.as_millis(),
                        total_bytes = bytes_sent,
                        "FastSlowStore::update: channel send stalled (>5s). A downstream store may be hanging",
                    );
                }
                bytes_sent += u64::try_from(chunk_len).unwrap_or(u64::MAX);
                fast_result
                    .map_err(|e| {
                        make_err!(
                            Code::Internal,
                            "Failed to send message to fast_store in fast_slow_store {:?}",
                            e
                        )
                    })
                    .merge(slow_result.map_err(|e| {
                        make_err!(
                            Code::Internal,
                            "Failed to send message to slow_store in fast_slow store {:?}",
                            e
                        )
                    }))?;
            }
        };

        let fast_start = std::time::Instant::now();
        let fast_store_fut = async {
            let res = self.fast_store.update(key.borrow(), fast_rx, size_info).await;
            (res, fast_start.elapsed())
        };
        let slow_start = std::time::Instant::now();
        let slow_store_fut = async {
            let res = self.slow_store.update(key.borrow(), slow_rx, size_info).await;
            (res, slow_start.elapsed())
        };

        let (data_stream_res, (fast_res, fast_elapsed), (slow_res, slow_elapsed)) =
            join!(data_stream_fut, fast_store_fut, slow_store_fut);

        let total_elapsed = update_start.elapsed();
        let fast_ms = fast_elapsed.as_millis();
        let slow_ms = slow_elapsed.as_millis();
        let slower_leg = if fast_ms >= slow_ms { "fast" } else { "slow" };
        if data_stream_res.is_err() || fast_res.is_err() || slow_res.is_err() {
            warn!(
                key = %key_debug,
                elapsed_ms = total_elapsed.as_millis(),
                fast_ms,
                slow_ms,
                slower_leg,
                total_bytes = bytes_sent,
                data_stream_ok = data_stream_res.is_ok(),
                fast_store_ok = fast_res.is_ok(),
                slow_store_ok = slow_res.is_ok(),
                "FastSlowStore::update: completed with error(s)",
            );
        } else {
            debug!(
                key = %key_debug,
                elapsed_ms = total_elapsed.as_millis(),
                fast_ms,
                slow_ms,
                slower_leg,
                total_bytes = bytes_sent,
                "FastSlowStore::update: completed successfully",
            );
        }
        data_stream_res.merge(fast_res).merge(slow_res)?;
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

        let oneshot_start = std::time::Instant::now();
        let key_debug = format!("{key:?}");
        let data_len = data.len();
        let fast_oneshot_start = std::time::Instant::now();
        let data_for_slow = data.clone();
        let fast_fut = async {
            let res = self.fast_store.update_oneshot(key.borrow(), data).await;
            (res, fast_oneshot_start.elapsed())
        };
        let slow_oneshot_start = std::time::Instant::now();
        let slow_fut = async {
            let res = self.slow_store.update_oneshot(key.borrow(), data_for_slow).await;
            (res, slow_oneshot_start.elapsed())
        };
        let ((fast_res, fast_elapsed), (slow_res, slow_elapsed)) = join!(fast_fut, slow_fut);
        let total_elapsed = oneshot_start.elapsed();
        let fast_ms = fast_elapsed.as_millis();
        let slow_ms = slow_elapsed.as_millis();
        let slower_leg = if fast_ms >= slow_ms { "fast" } else { "slow" };
        debug!(
            key = %key_debug,
            elapsed_ms = total_elapsed.as_millis(),
            fast_ms,
            slow_ms,
            slower_leg,
            data_len,
            "FastSlowStore::update_oneshot: completed",
        );
        fast_res.merge(slow_res)?;
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
