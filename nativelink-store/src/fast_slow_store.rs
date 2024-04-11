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

use std::cmp::{max, min};
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{join, FutureExt};
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_util::buf_channel::{
    make_buf_channel_pair, DropCloserReadHalf, DropCloserWriteHalf,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::fs;
use nativelink_util::health_utils::{default_health_status_indicator, HealthStatusIndicator};
use nativelink_util::metrics_utils::Registry;
use nativelink_util::store_trait::{
    slow_update_store_with_file, Store, StoreOptimizations, UploadSizeInfo,
};

// TODO(blaise.bruer) This store needs to be evaluated for more efficient memory usage,
// there are many copies happening internally.

// TODO(blaise.bruer) We should consider copying the data in the background to allow the
// client to hang up while the data is buffered. An alternative is to possibly make a
// "BufferedStore" that could be placed on the "slow" store that would hang up early
// if data is in the buffer.
pub struct FastSlowStore {
    fast_store: Arc<dyn Store>,
    slow_store: Arc<dyn Store>,
}

impl FastSlowStore {
    pub fn new(
        _config: &nativelink_config::stores::FastSlowStore,
        fast_store: Arc<dyn Store>,
        slow_store: Arc<dyn Store>,
    ) -> Self {
        Self {
            fast_store,
            slow_store,
        }
    }

    pub fn fast_store(&self) -> &Arc<dyn Store> {
        &self.fast_store
    }

    pub fn slow_store(&self) -> &Arc<dyn Store> {
        &self.slow_store
    }

    /// Ensure our fast store is populated. This should be kept as a low
    /// cost function. Since the data itself is shared and not copied it should be fairly
    /// low cost to just discard the data, but does cost a few mutex locks while
    /// streaming.
    pub async fn populate_fast_store(self: Pin<&Self>, digest: DigestInfo) -> Result<(), Error> {
        let maybe_size_info = self
            .pin_fast_store()
            .has(digest)
            .await
            .err_tip(|| "While querying in populate_fast_store")?;
        if maybe_size_info.is_some() {
            return Ok(());
        }
        // TODO(blaise.bruer) This is extremely inefficient, since we are just trying
        // to send the stream to /dev/null. Maybe we could instead make a version of
        // the stream that can send to the drain more efficiently?
        let (tx, mut rx) = make_buf_channel_pair();
        let drain_fut = async move {
            while !rx.recv().await?.is_empty() {}
            Ok(())
        };
        let (drain_res, get_res) = join!(drain_fut, self.get(digest, tx));
        get_res.err_tip(|| "Failed to populate()").merge(drain_res)
    }

    fn pin_fast_store(&self) -> Pin<&dyn Store> {
        Pin::new(self.fast_store.as_ref())
    }

    fn pin_slow_store(&self) -> Pin<&dyn Store> {
        Pin::new(self.slow_store.as_ref())
    }

    /// Returns the range of bytes that should be sent given a slice bounds
    /// offset so the output range maps the received_range.start to 0.
    // TODO(allada) This should be put into utils, as this logic is used
    // elsewhere in the code.
    pub fn calculate_range(
        received_range: &Range<usize>,
        send_range: &Range<usize>,
    ) -> Option<Range<usize>> {
        // Protect against subtraction overflow.
        if received_range.start >= received_range.end {
            return None;
        }

        let start = max(received_range.start, send_range.start);
        let end = min(received_range.end, send_range.end);
        if received_range.contains(&start) && received_range.contains(&(end - 1)) {
            // Offset both to the start of the received_range.
            Some(start - received_range.start..end - received_range.start)
        } else {
            None
        }
    }
}

#[async_trait]
impl Store for FastSlowStore {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        // If our slow store is a noop store, it'll always return a 404,
        // so only check the fast store in such case.
        let slow_store = self.slow_store.inner_store(None);
        if slow_store.optimized_for(StoreOptimizations::NoopDownloads) {
            return self
                .pin_fast_store()
                .has_with_results(digests, results)
                .await;
        }
        // Only check the slow store because if it's not there, then something
        // down stream might be unable to get it.  This should not affect
        // workers as they only use get() and a CAS can use an
        // ExistenceCacheStore to avoid the bottleneck.
        self.pin_slow_store()
            .has_with_results(digests, results)
            .await
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        mut reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        // If either one of our stores is a noop store, bypass the multiplexing
        // and just use the store that is not a noop store.
        let slow_store = self.slow_store.inner_store(Some(digest));
        if slow_store.optimized_for(StoreOptimizations::NoopUpdates) {
            return self
                .pin_fast_store()
                .update(digest, reader, size_info)
                .await;
        }
        let fast_store = self.fast_store.inner_store(Some(digest));
        if fast_store.optimized_for(StoreOptimizations::NoopUpdates) {
            return self
                .pin_slow_store()
                .update(digest, reader, size_info)
                .await;
        }

        let (mut fast_tx, fast_rx) = make_buf_channel_pair();
        let (mut slow_tx, slow_rx) = make_buf_channel_pair();

        let data_stream_fut = async move {
            loop {
                let buffer = reader
                    .recv()
                    .await
                    .err_tip(|| "Failed to read buffer in fastslow store")?;
                if buffer.is_empty() {
                    // EOF received.
                    fast_tx.send_eof().err_tip(|| {
                        "Failed to write eof to fast store in fast_slow store update"
                    })?;
                    slow_tx
                        .send_eof()
                        .err_tip(|| "Failed to write eof to writer in fast_slow store update")?;
                    return Result::<(), Error>::Ok(());
                }

                let (fast_result, slow_result) =
                    join!(fast_tx.send(buffer.clone()), slow_tx.send(buffer));
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

        let fast_store_fut = self.pin_fast_store().update(digest, fast_rx, size_info);
        let slow_store_fut = self.pin_slow_store().update(digest, slow_rx, size_info);

        let (data_stream_res, fast_res, slow_res) =
            join!(data_stream_fut, fast_store_fut, slow_store_fut);
        data_stream_res.merge(fast_res).merge(slow_res)?;
        Ok(())
    }

    /// FastSlowStore has optimiations for dealing with files.
    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        optimization == StoreOptimizations::FileUpdates
    }

    /// Optimized variation to consume the file if one of the stores is a
    /// filesystem store. This makes the operation a move instead of a copy
    /// dramatically increasing performance for large files.
    async fn update_with_whole_file(
        self: Pin<&Self>,
        digest: DigestInfo,
        mut file: fs::ResumeableFileSlot,
        upload_size: UploadSizeInfo,
    ) -> Result<Option<fs::ResumeableFileSlot>, Error> {
        let fast_store = self.fast_store.inner_store(Some(digest));
        let slow_store = self.slow_store.inner_store(Some(digest));
        if fast_store.optimized_for(StoreOptimizations::FileUpdates) {
            if !slow_store.optimized_for(StoreOptimizations::NoopUpdates) {
                slow_update_store_with_file(Pin::new(slow_store), digest, &mut file, upload_size)
                    .await
                    .err_tip(|| "In FastSlowStore::update_with_whole_file slow_store")?;
            }
            return Pin::new(fast_store)
                .update_with_whole_file(digest, file, upload_size)
                .await;
        }

        if slow_store.optimized_for(StoreOptimizations::FileUpdates) {
            if !fast_store.optimized_for(StoreOptimizations::NoopUpdates) {
                slow_update_store_with_file(Pin::new(fast_store), digest, &mut file, upload_size)
                    .await
                    .err_tip(|| "In FastSlowStore::update_with_whole_file fast_store")?;
            }
            return Pin::new(slow_store)
                .update_with_whole_file(digest, file, upload_size)
                .await;
        }

        slow_update_store_with_file(self, digest, &mut file, upload_size)
            .await
            .err_tip(|| "In FastSlowStore::update_with_whole_file")?;
        Ok(Some(file))
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        // TODO(blaise.bruer) Investigate if we should maybe ignore errors here instead of
        // forwarding the up.
        let fast_store = self.pin_fast_store();
        let slow_store = self.pin_slow_store();
        if fast_store.has(digest).await?.is_some() {
            return fast_store
                .get_part_ref(digest, writer, offset, length)
                .await;
        }

        let sz = slow_store
            .has(digest)
            .await
            .err_tip(|| "Failed to run has() on slow store")?
            .ok_or_else(|| {
                make_err!(
                    Code::NotFound,
                    "Object {} not found in either fast or slow store",
                    digest.hash_str()
                )
            })?;

        let send_range = offset..length.map_or(usize::MAX, |length| length + offset);
        let mut bytes_received: usize = 0;

        let (mut fast_tx, fast_rx) = make_buf_channel_pair();
        let (slow_tx, mut slow_rx) = make_buf_channel_pair();
        let data_stream_fut = async move {
            let mut writer_pin = Pin::new(writer);
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
                    return Ok::<_, Error>((fast_res, writer_pin));
                }

                let writer_fut = if let Some(range) = Self::calculate_range(
                    &(bytes_received..bytes_received + output_buf.len()),
                    &send_range,
                ) {
                    writer_pin.send(output_buf.slice(range)).right_future()
                } else {
                    futures::future::ready(Ok(())).left_future()
                };
                bytes_received += output_buf.len();

                let (fast_tx_res, writer_res) = join!(fast_tx.send(output_buf), writer_fut);
                fast_tx_res.err_tip(|| "Failed to write to fast store in fast_slow store")?;
                writer_res.err_tip(|| "Failed to write result to writer in fast_slow store")?;
            }
        };

        let slow_store_fut = slow_store.get(digest, slow_tx);
        let fast_store_fut = fast_store.update(digest, fast_rx, UploadSizeInfo::ExactSize(sz));

        let (data_stream_res, slow_res, fast_res) =
            join!(data_stream_fut, slow_store_fut, fast_store_fut);
        match data_stream_res {
            Ok((fast_eof_res, mut writer_pin)) =>
            // Sending the EOF will drop us almost immediately in bytestream_server
            // so we perform it as the very last action in this method.
            {
                fast_eof_res
                    .merge(fast_res)
                    .merge(slow_res)
                    .merge(writer_pin.send_eof())
            }
            Err(err) => fast_res.merge(slow_res).merge(Err(err)),
        }
    }

    fn inner_store(&self, _digest: Option<DigestInfo>) -> &'_ dyn Store {
        self
    }

    fn inner_store_arc(self: Arc<Self>, _digest: Option<DigestInfo>) -> Arc<dyn Store> {
        self
    }

    fn as_any<'a>(&'a self) -> &'a (dyn std::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_metrics(self: Arc<Self>, registry: &mut Registry) {
        let fast_store_registry = registry.sub_registry_with_prefix("fast");
        self.fast_store
            .clone()
            .register_metrics(fast_store_registry);
        let slow_store_registry = registry.sub_registry_with_prefix("slow");
        self.slow_store
            .clone()
            .register_metrics(slow_store_registry);
    }
}

default_health_status_indicator!(FastSlowStore);
