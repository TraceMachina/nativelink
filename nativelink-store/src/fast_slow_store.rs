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
use core::time::Duration;
use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{FuturesUnordered, StreamExt};
use futures::{FutureExt, join, try_join};
use nativelink_config::stores::{FastSlowSpec, StoreDirection};
use nativelink_error::{Code, Error, ErrorContext, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair,
};
use nativelink_util::fs;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    RemoveCallback, Store, StoreDriver, StoreKey, StoreLike, StoreOptimizations, UploadSizeInfo,
    slow_update_store_with_file,
};
use parking_lot::Mutex;
use tokio::sync::OnceCell;
use tracing::{debug, info, trace, warn};

// TODO(palfrey) This store needs to be evaluated for more efficient memory usage,
// there are many copies happening internally.

type Loader = Arc<OnceCell<()>>;
type MaybeSize = Option<u64>;

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
    /// See [`FastSlowSpec::bypass_dedup_threshold_bytes`].
    bypass_dedup_threshold_bytes: u64,
    weak_self: Weak<Self>,
    #[metric]
    metrics: FastSlowStoreMetrics,
    // De-duplicate requests for the fast store, only the first streams, others
    // are blocked.  This may feel like it's causing a slow down of tasks, but
    // actually it's faster because we're not downloading the file multiple
    // times are doing loads of duplicate IO.
    populating_digests: Mutex<HashMap<StoreKey<'static>, Loader>>,
    // Tracks keys whose slow-store write is currently in flight, along with
    // the best-known size. Consulted by `has_with_results` so that a
    // concurrent writer that has not yet finished pushing to the slow store
    // is still visible to a concurrent existence check, preventing redundant
    // duplicate uploads of the same blob.
    in_flight_slow_writes: Mutex<HashMap<StoreKey<'static>, Arc<tokio::sync::Mutex<MaybeSize>>>>,
}

// This guard ensures that the populating_digests is cleared even if the future
// is dropped, it is cancel safe.
struct LoaderGuard<'a> {
    weak_store: Weak<FastSlowStore>,
    key: StoreKey<'a>,
    loader: Option<Loader>,
    is_leader: bool,
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

/// Cancel-safe RAII guard that removes an entry from
/// `FastSlowStore::in_flight_slow_writes` when dropped. This ensures the
/// map does not leak entries if the surrounding `update()` future is
/// cancelled before the slow-store write completes.
struct InFlightSlowWriteGuard {
    weak_store: Weak<FastSlowStore>,
    key: Option<StoreKey<'static>>,
    write_complete_guard: tokio::sync::OwnedMutexGuard<MaybeSize>,
}

impl InFlightSlowWriteGuard {
    fn complete(mut self, size: MaybeSize) {
        *self.write_complete_guard = size;
    }
}

impl Drop for InFlightSlowWriteGuard {
    fn drop(&mut self) {
        let Some(store) = self.weak_store.upgrade() else {
            return;
        };
        let Some(key) = self.key.take() else {
            return;
        };
        store.in_flight_slow_writes.lock().remove(&key);
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

        let mut guard = store.populating_digests.lock();
        if let std::collections::hash_map::Entry::Occupied(occupied_entry) =
            guard.entry(self.key.borrow().into_owned())
            && Arc::ptr_eq(occupied_entry.get(), &loader)
        {
            drop(loader);
            if Arc::strong_count(occupied_entry.get()) == 1 {
                // This is the last loader, so remove it.
                occupied_entry.remove();
            }
        }
    }
}

/// Maximum concurrent fast-tier writes for one `update_many` batch.
const FAST_TIER_UPDATE_CONCURRENCY: usize = 16;

impl FastSlowStore {
    pub fn new(spec: &FastSlowSpec, fast_store: Store, slow_store: Store) -> Arc<Self> {
        Arc::new_cyclic(|weak_self| Self {
            fast_store,
            fast_direction: spec.fast_direction,
            slow_store,
            slow_direction: spec.slow_direction,
            // 0 (default) disables the bypass entirely (always dedup).
            bypass_dedup_threshold_bytes: spec.bypass_dedup_threshold_bytes,
            weak_self: weak_self.clone(),
            metrics: FastSlowStoreMetrics::default(),
            populating_digests: Mutex::new(HashMap::new()),
            in_flight_slow_writes: Mutex::new(HashMap::new()),
        })
    }

    fn register_in_flight_slow_write(&self, key: StoreKey<'_>) -> InFlightSlowWriteGuard {
        let owned = key.into_owned();
        let write_complete = Arc::new(tokio::sync::Mutex::new(None));
        let write_complete_guard = write_complete
            .clone()
            .try_lock_owned()
            .expect("Newly created mutex is locked");
        self.in_flight_slow_writes
            .lock()
            .insert(owned.borrow().into_owned(), write_complete);
        InFlightSlowWriteGuard {
            weak_store: self.weak_self.clone(),
            key: Some(owned),
            write_complete_guard,
        }
    }

    /// Digest size in bytes, or `None` for non-digest keys.
    const fn digest_size_bytes(key: &StoreKey<'_>) -> Option<u64> {
        match key {
            StoreKey::Digest(d) => Some(d.size_bytes()),
            StoreKey::Str(_) => None,
        }
    }

    /// Whether a read should skip dedup and hit the slow store directly. A
    /// threshold of 0 disables the bypass so every read goes through dedup.
    fn should_bypass_dedup(&self, key: &StoreKey<'_>) -> bool {
        self.bypass_dedup_threshold_bytes != 0
            && Self::digest_size_bytes(key)
                .is_some_and(|size| size >= self.bypass_dedup_threshold_bytes)
    }

    pub const fn fast_store(&self) -> &Store {
        &self.fast_store
    }

    pub const fn slow_store(&self) -> &Store {
        &self.slow_store
    }

    pub fn get_arc(&self) -> Option<Arc<Self>> {
        self.weak_self.upgrade()
    }

    fn get_loader<'a>(&self, key: StoreKey<'a>) -> LoaderGuard<'a> {
        // Get a single loader instance that's used to populate the fast store
        // for this digest.  If another request comes in then it's de-duplicated.
        let mut is_leader = false;
        let loader = match self
            .populating_digests
            .lock()
            .entry(key.borrow().into_owned())
        {
            std::collections::hash_map::Entry::Occupied(occupied_entry) => {
                occupied_entry.get().clone()
            }
            std::collections::hash_map::Entry::Vacant(vacant_entry) => {
                is_leader = true;
                vacant_entry.insert(Arc::new(OnceCell::new())).clone()
            }
        };
        LoaderGuard {
            weak_store: self.weak_self.clone(),
            key,
            loader: Some(loader),
            is_leader,
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
                        let err = make_err!(
                            Code::NotFound,
                            "Object {} not found in either fast or slow store. \
                                If using multiple workers, ensure all workers share the same CAS storage path.",
                            key.as_str()
                        );
                        if let StoreKey::Digest(d) = key.borrow() {
                            err.with_context(ErrorContext::MissingDigest {
                                hash: d.packed_hash().to_string(),
                                size: d.size_bytes() as i64,
                            })
                        } else {
                            err
                        }
                    })?
            )
        };

        let send_range = offset..length.map_or(u64::MAX, |length| length + offset);
        let mut bytes_received: u64 = 0;
        let mut counted_hit = false;

        let (mut fast_tx, fast_rx) = make_buf_channel_pair();
        let (slow_tx, mut slow_rx) = make_buf_channel_pair();
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
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        try_join!(
            self.fast_store.clone().into_inner().post_init(),
            self.slow_store.clone().into_inner().post_init()
        )?;
        Ok(())
    }

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

        // Check with the slow store first.
        self.slow_store.has_with_results(key, results).await?;

        // Check for any in-flight requests to the slow store next.
        let mut in_flight_futs = FuturesUnordered::new();
        {
            let in_flight = self.in_flight_slow_writes.lock();
            if !in_flight.is_empty() {
                for (i, (k, result)) in key.iter().zip(results.iter_mut()).enumerate() {
                    if result.is_none() {
                        let owned = k.borrow().into_owned();
                        if let Some(maybe_size) = in_flight.get(&owned) {
                            let maybe_size = maybe_size.clone();
                            in_flight_futs.push(async move { (i, *maybe_size.lock().await) });
                        }
                    }
                }
            }
        }
        while let Some((i, size)) = in_flight_futs.next().await {
            results[i] = size;
        }

        // NOTE: We intentionally *NEVER* check the fast store, this is to
        // ensure that we re-upload data to the slow store if it only exists
        // in the fast store.  This does not affect workers as they do not
        // check existence through `has` and instead go direct to loading the
        // data which bypasses the check and will load from the fast store if
        // it does not exist in the slow store.

        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
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
            return reader.drain().await.err_tip(|| "In FastFlowStore::update");
        }
        if ignore_slow {
            return self.fast_store.update(key, reader, size_info).await;
        }
        let slow_in_flight_guard = self.register_in_flight_slow_write(key.borrow());
        if ignore_fast {
            let result = self
                .slow_store
                .update(key.borrow(), reader, size_info)
                .await;
            if let Ok(size) = &result {
                slow_in_flight_guard.complete(Some(*size));
            }
            return result;
        }

        let (mut fast_tx, fast_rx) = make_buf_channel_pair();
        let (mut slow_tx, slow_rx) = make_buf_channel_pair();

        let key_debug = format!("{key:?}");
        trace!(
            key = %key_debug,
            "FastSlowStore::update: starting dual-store upload",
        );
        let update_start = std::time::Instant::now();

        let data_stream_fut = async move {
            let mut bytes_sent: u64 = 0;
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
                    return Result::<u64, Error>::Ok(bytes_sent);
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

        let fast_store_fut = self.fast_store.update(key.borrow(), fast_rx, size_info);
        let slow_store_fut = self.slow_store.update(key.borrow(), slow_rx, size_info);

        let (data_stream_res, fast_res, slow_res) =
            join!(data_stream_fut, fast_store_fut, slow_store_fut);

        if let Ok(size) = slow_res {
            slow_in_flight_guard.complete(Some(size));
        } else {
            drop(slow_in_flight_guard);
        }

        let total_elapsed = update_start.elapsed();
        if data_stream_res.is_err() || fast_res.is_err() || slow_res.is_err() {
            let all_not_found = [&data_stream_res, &fast_res, &slow_res]
                .iter()
                .all(|r| match r {
                    Ok(_size) => true,
                    Err(e) => e.code == Code::NotFound,
                });
            if all_not_found {
                info!(
                    key = %key_debug,
                    elapsed_ms = total_elapsed.as_millis(),
                    data_stream_ok = data_stream_res.is_ok(),
                    fast_store_ok = fast_res.is_ok(),
                    slow_store_ok = slow_res.is_ok(),
                    "FastSlowStore::update: completed with NotFound error(s)",
                );
            } else {
                warn!(
                    key = %key_debug,
                    elapsed_ms = total_elapsed.as_millis(),
                    data_stream_ok = data_stream_res.is_ok(),
                    fast_store_ok = fast_res.is_ok(),
                    slow_store_ok = slow_res.is_ok(),
                    "FastSlowStore::update: completed with error(s)",
                );
            }
        } else {
            trace!(
                key = %key_debug,
                elapsed_ms = total_elapsed.as_millis(),
                "FastSlowStore::update: completed successfully",
            );
        }
        data_stream_res.merge(fast_res).merge(slow_res)
    }

    /// `FastSlowStore` has optimizations for dealing with files, and
    /// advertises batched uploads when its slow tier can batch them.
    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        match optimization {
            StoreOptimizations::FileUpdates => true,
            StoreOptimizations::SubscribesToUpdateMany => {
                self.slow_store
                    .optimized_for(StoreOptimizations::SubscribesToUpdateMany)
                    && self.slow_direction != StoreDirection::ReadOnly
                    && self.slow_direction != StoreDirection::Get
            }
            _ => false,
        }
    }

    /// Batched variant of `update()`: publishes every item to both tiers,
    /// letting the slow tier amortize per-object wire costs via its own
    /// `update_many` implementation. Only used on the batched path when the
    /// slow tier subscribes to it; otherwise the default per-item loop
    /// preserves `update()` semantics exactly.
    ///
    /// Note: in-flight slow-write guards are held until the entire batch
    /// settles, so concurrent `has()` calls on any batch member block until
    /// the whole batch completes (coarser than `update()`'s per-key window).
    async fn update_many(
        self: Pin<&Self>,
        items: Vec<(StoreKey<'static>, Bytes)>,
    ) -> Result<(), Error> {
        let ignore_slow = self
            .slow_store
            .inner_store(None::<StoreKey>)
            .optimized_for(StoreOptimizations::NoopUpdates)
            || self.slow_direction == StoreDirection::ReadOnly
            || self.slow_direction == StoreDirection::Get;
        let ignore_fast = self
            .fast_store
            .inner_store(None::<StoreKey>)
            .optimized_for(StoreOptimizations::NoopUpdates)
            || self.fast_direction == StoreDirection::ReadOnly
            || self.fast_direction == StoreDirection::Get;
        let slow_can_batch = !ignore_slow
            && self
                .slow_store
                .optimized_for(StoreOptimizations::SubscribesToUpdateMany);
        if !slow_can_batch {
            // Per-item updates through `update()` keep every existing
            // semantic (noop/direction handling, in-flight registration).
            for (key, data) in items {
                self.as_store_driver_pin()
                    .update_oneshot(key, data)
                    .await
                    .err_tip(|| "In FastSlowStore::update_many fallback")?;
            }
            return Ok(());
        }

        // Make the in-flight slow writes visible to concurrent has() calls,
        // exactly like update() does per key.
        let guards: Vec<(InFlightSlowWriteGuard, u64)> = items
            .iter()
            .map(|(key, data)| {
                (
                    self.register_in_flight_slow_write(key.borrow()),
                    data.len() as u64,
                )
            })
            .collect();

        let slow_items: Vec<(StoreKey<'static>, Bytes)> = items
            .iter()
            .map(|(key, data)| (key.clone(), data.clone()))
            .collect();
        let slow_fut = self.slow_store.update_many(slow_items);
        let fast_fut = async {
            if ignore_fast {
                return Ok(());
            }
            // The fast tier has no batched implementation; write with
            // bounded concurrency instead of the serial default loop.
            // Plain loop instead of an iterator closure: async closures
            // over borrowed keys hit HRTB inference errors.
            let fast_store = &self.fast_store;
            let mut write_futures = Vec::with_capacity(items.len());
            for (key, data) in items {
                write_futures.push(async move { fast_store.update_oneshot(key, data).await });
            }
            let mut writes =
                futures::stream::iter(write_futures).buffer_unordered(FAST_TIER_UPDATE_CONCURRENCY);
            while let Some(result) = writes.next().await {
                result.err_tip(|| "In FastSlowStore::update_many fast store item")?;
            }
            Ok::<(), Error>(())
        };
        let (slow_res, fast_res) = tokio::join!(slow_fut, fast_fut);
        if slow_res.is_ok() {
            for (guard, size) in guards {
                guard.complete(Some(size));
            }
        }
        slow_res
            .err_tip(|| "In FastSlowStore::update_many slow store")
            .merge(fast_res.err_tip(|| "In FastSlowStore::update_many fast store"))
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
    ) -> Result<(u64, Option<fs::FileSlot>), Error> {
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
                let slow_in_flight_guard = self.register_in_flight_slow_write(key.borrow());
                let size = slow_update_store_with_file(
                    self.slow_store.as_store_driver_pin(),
                    key.borrow(),
                    &mut file,
                    upload_size,
                )
                .await
                .err_tip(|| "In FastSlowStore::update_with_whole_file slow_store")?;
                slow_in_flight_guard.complete(Some(size));
                trace!(
                    elapsed_ms = slow_start.elapsed().as_millis(),
                    "FastSlowStore::update_with_whole_file: slow_store upload completed",
                );
            }
            if self.fast_direction == StoreDirection::ReadOnly
                || self.fast_direction == StoreDirection::Get
            {
                let file_size = match upload_size {
                    UploadSizeInfo::ExactSize(size) => size,
                    UploadSizeInfo::MaxSize(_) => file
                        .as_ref()
                        .metadata()
                        .await
                        .err_tip(|| format!("While reading metadata for {}", path.display()))?
                        .len(),
                };
                return Ok((file_size, Some(file)));
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
            let maybe_size = if ignore_fast {
                None
            } else {
                Some(
                    slow_update_store_with_file(
                        self.fast_store.as_store_driver_pin(),
                        key.borrow(),
                        &mut file,
                        upload_size,
                    )
                    .await
                    .err_tip(|| "In FastSlowStore::update_with_whole_file fast_store")?,
                )
            };
            let ignore_slow = self.slow_direction == StoreDirection::ReadOnly
                || self.slow_direction == StoreDirection::Get;
            if ignore_slow {
                let size = match maybe_size {
                    Some(size) => size,
                    None => match upload_size {
                        UploadSizeInfo::ExactSize(size) => size,
                        UploadSizeInfo::MaxSize(_) => file
                            .as_ref()
                            .metadata()
                            .await
                            .err_tip(|| format!("While reading metadata for {}", path.display()))?
                            .len(),
                    },
                };
                return Ok((size, Some(file)));
            }
            let slow_in_flight_guard: InFlightSlowWriteGuard =
                self.register_in_flight_slow_write(key.borrow());
            let (size, maybe_file_slot) = self
                .slow_store
                .update_with_whole_file(key.borrow(), path, file, upload_size)
                .await?;
            slow_in_flight_guard.complete(Some(size));
            return Ok((size, maybe_file_slot));
        }

        let size = slow_update_store_with_file(self, key, &mut file, upload_size)
            .await
            .err_tip(|| "In FastSlowStore::update_with_whole_file")?;
        Ok((size, Some(file)))
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        // `has()` can report a stale map entry whose file is gone, so
        // get_part may still return NotFound; fall through to the slow
        // store unless we have already streamed bytes to the caller.
        if self.fast_store.has(key.borrow()).await?.is_some() {
            let bytes_before = writer.get_bytes_written();
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
                Err(e)
                    if e.code == Code::NotFound && writer.get_bytes_written() == bytes_before =>
                {
                    self.metrics
                        .fast_store_stale_map_falls_through
                        .fetch_add(1, Ordering::Acquire);
                    warn!(%key, ?e, "Stale fast-store map entry; falling through to slow store");
                    // fall through to populate path
                }
                Err(e) => return Err(e),
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

        // Huge blobs: dedup is a net loss (followers time out anyway and
        // the fast tier is evicted), so read straight from the slow store.
        if self.should_bypass_dedup(&key) {
            self.metrics
                .huge_blob_dedup_bypasses
                .fetch_add(1, Ordering::Acquire);
            self.metrics
                .slow_store_hit_count
                .fetch_add(1, Ordering::Acquire);
            debug!(%key, threshold_bytes = self.bypass_dedup_threshold_bytes, "Bypassing dedup for huge blob");
            self.slow_store
                .get_part(key, writer.borrow_mut(), offset, length)
                .await
                .err_tip(|| "In FastSlowStore::get_part huge-blob bypass")?;
            self.metrics
                .slow_store_downloaded_bytes
                .fetch_add(writer.get_bytes_written(), Ordering::Acquire);
            return Ok(());
        }

        let mut writer = Some(writer);

        // Drive the dedup loader. Two distinct paths:
        //
        //   * Leader (created the OnceCell entry): runs `populate` with
        //     OUR `writer`, streaming directly to the caller while
        //     filling the fast cache. No timeout: a multi-GB blob
        //     legitimately takes minutes to stream, and cancelling our
        //     own populate would propagate the failure to every other
        //     reader of this digest.
        //
        //   * Follower: bound the wait so a wedged leader does not pin
        //     us until the upstream `gRPC` deadline fires. The follower
        //     closure passes `None` for `writer`. This is critical: if
        //     the OnceCell ever promotes our follower closure to leader
        //     (because the original leader's future was dropped), and
        //     our `tokio::time::timeout` then cancels it, *no* caller
        //     `writer` was ever moved into the populate stream, so no
        //     `gRPC` sender is dropped without EOF. The follower then
        //     re-enters `get_part` below and reads from the now-warm
        //     fast cache, OR falls back to the slow store on timeout.
        let needs_slow_store_fallback: bool = {
            let loader_guard = self.get_loader(key.borrow());
            let is_leader = loader_guard.is_leader;
            if is_leader {
                loader_guard
                    .get_or_try_init(|| {
                        self.populate_and_maybe_stream(key.borrow(), writer.take(), offset, length)
                    })
                    .await?;
                false
            } else {
                let load_fut = loader_guard.get_or_try_init(|| {
                    self.populate_and_maybe_stream(key.borrow(), None, offset, length)
                });
                match tokio::time::timeout(LEADER_WAIT_TIMEOUT, load_fut).await {
                    Ok(result) => {
                        result?;
                        false
                    }
                    Err(_elapsed) => {
                        self.metrics
                            .leader_wait_timeouts
                            .fetch_add(1, Ordering::Acquire);
                        warn!(
                            %key,
                            timeout_secs = LEADER_WAIT_TIMEOUT.as_secs(),
                            "FastSlowStore::get_part: leader-wait exceeded timeout, bypassing dedup and reading slow store directly",
                        );
                        true
                    }
                }
            }
        };

        if needs_slow_store_fallback && let Some(writer) = writer.take() {
            return self
                .slow_store
                .get_part(key, writer, offset, length)
                .await
                .err_tip(
                    || "In FastSlowStore::get_part slow_store fallback after leader-wait timeout",
                );
        }

        // If we didn't stream then re-enter which will stream from the fast
        // store, or retry the download.  We should not get in a loop here
        // because OnceCell has the good sense to retry for all callers so in
        // order to get here the fast store will have been populated.  There's
        // an outside chance it was evicted, but that's slim.
        if let Some(writer) = writer.take() {
            self.get_part(key, writer, offset, length).await
        } else {
            // This was the thread that did the streaming already, lucky duck.
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

    fn register_remove_callback(self: Arc<Self>, callback: RemoveCallback) -> Result<(), Error> {
        self.fast_store.register_remove_callback(callback.clone())?;
        self.slow_store.register_remove_callback(callback)?;
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
    #[metric(
        help = "Number of times a follower bypassed the populating-digests dedup because the leader exceeded LEADER_WAIT_TIMEOUT"
    )]
    leader_wait_timeouts: AtomicU64,
    #[metric(help = "get_part calls that bypassed dedup for huge blobs")]
    huge_blob_dedup_bypasses: AtomicU64,
    #[metric(help = "Stale fast-store map entries that fell through to the slow store")]
    fast_store_stale_map_falls_through: AtomicU64,
}

/// Maximum time a follower will wait on the leader-populator before
/// bypassing the dedup map and reading directly from the slow store.
///
/// Without this bound a single wedged populator would block every
/// concurrent reader of the same digest until each one's own `gRPC`
/// deadline fired (e.g. Bazel's `--remote_timeout`), turning a
/// single slow read into a fan-out of `DEADLINE_EXCEEDED` errors.
const LEADER_WAIT_TIMEOUT: Duration = Duration::from_mins(1);

default_health_status_indicator!(FastSlowStore);
