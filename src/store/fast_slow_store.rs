// Copyright 2022 The Native Link Authors. All rights reserved.
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

use crate::error::{make_err, Code, Error, ResultExt};
use crate::store::traits::{Store, UploadSizeInfo};
use crate::util::buf_channel::{make_buf_channel_pair, DropCloserReadHalf, DropCloserWriteHalf};
use crate::util::common::DigestInfo;
use crate::util::metrics_utils::Registry;

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
        _config: &crate::config::stores::FastSlowStore,
        fast_store: Arc<dyn Store>,
        slow_store: Arc<dyn Store>,
    ) -> Self {
        Self { fast_store, slow_store }
    }

    pub fn fast_store(&self) -> &Arc<dyn Store> {
        &self.fast_store
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
    pub fn calculate_range(received_range: &Range<usize>, send_range: &Range<usize>) -> Option<Range<usize>> {
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
        self.pin_fast_store().has_with_results(digests, results).await?;
        let missing_digests: Vec<DigestInfo> = results
            .iter()
            .zip(digests.iter())
            .filter_map(|(result, digest)| result.map_or_else(|| Some(*digest), |_| None))
            .collect();
        if !missing_digests.is_empty() {
            let mut slow_store_results = self.pin_slow_store().has_many(&missing_digests).await?.into_iter();
            // We did not change our `result` order and the slow store's results are all the None
            // entries in `results`. This means we can just iterate over the `results`, find the None
            // items and put in whatever the next item in the results from `slow_store_results`.
            for result in results.iter_mut() {
                if result.is_none() {
                    *result = slow_store_results
                        .next()
                        .err_tip(|| "slow_store_results out of sync with missing_digests")?;
                }
            }
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        mut reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
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
                    fast_tx
                        .send_eof()
                        .await
                        .err_tip(|| "Failed to write eof to fast store in fast_slow store update")?;
                    slow_tx
                        .send_eof()
                        .await
                        .err_tip(|| "Failed to write eof to writer in fast_slow store update")?;
                    return Result::<(), Error>::Ok(());
                }

                let (fast_result, slow_result) = join!(fast_tx.send(buffer.clone()), slow_tx.send(buffer));
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

        let (data_stream_res, fast_res, slow_res) = join!(data_stream_fut, fast_store_fut, slow_store_fut);
        data_stream_res.merge(fast_res).merge(slow_res)?;
        Ok(())
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
            return fast_store.get_part_ref(digest, writer, offset, length).await;
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
                    // It is possible for the client to disconnect the stream because they got
                    // all the data they wanted, which could lead to an error when writing this
                    // EOF. If that was to happen, we could end up terminating this early and
                    // the resulting upload to the fast store might fail.
                    let (fast_res, slow_res) = join!(fast_tx.send_eof(), writer_pin.send_eof());
                    return fast_res.merge(slow_res);
                }

                let writer_fut = if let Some(range) =
                    Self::calculate_range(&(bytes_received..bytes_received + output_buf.len()), &send_range)
                {
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

        let (data_stream_res, slow_res, fast_res) = join!(data_stream_fut, slow_store_fut, fast_store_fut);
        data_stream_res.merge(fast_res).merge(slow_res)?;
        Ok(())
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }

    fn register_metrics(self: Arc<Self>, registry: &mut Registry) {
        let fast_store_registry = registry.sub_registry_with_prefix("fast");
        self.fast_store.clone().register_metrics(fast_store_registry);
        let slow_store_registry = registry.sub_registry_with_prefix("slow");
        self.slow_store.clone().register_metrics(slow_store_registry);
    }
}
