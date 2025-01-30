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

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::Poll;

use bytes::{Bytes, BytesMut};
use futures::task::Context;
use futures::{Future, Stream, TryFutureExt};
use nativelink_error::{error_if, make_err, make_input_err, Code, Error, ResultExt};
use tokio::sync::mpsc;
use tracing::{event, Level};

const ZERO_DATA: Bytes = Bytes::new();

/// Create a channel pair that can be used to transport buffer objects around to
/// different components. This wrapper is used because the streams give some
/// utility like managing EOF in a more friendly way, ensure if no EOF is received
/// it will send an error to the receiver channel before shutting down and count
/// the number of bytes sent.
#[must_use]
pub fn make_buf_channel_pair() -> (DropCloserWriteHalf, DropCloserReadHalf) {
    // We allow up to 2 items in the buffer at any given time. There is no major
    // reason behind this magic number other than thinking it will be nice to give
    // a little time for another thread to wake up and consume data if another
    // thread is pumping large amounts of data into the channel.
    let (tx, rx) = mpsc::channel(2);
    let eof_sent = Arc::new(AtomicBool::new(false));
    (
        DropCloserWriteHalf {
            tx: Some(tx),
            bytes_written: 0,
            eof_sent: eof_sent.clone(),
        },
        DropCloserReadHalf {
            rx,
            queued_data: VecDeque::new(),
            last_err: None,
            eof_sent,
            bytes_received: 0,
            recent_data: Vec::new(),
            max_recent_data_size: 0,
        },
    )
}

/// Writer half of the pair.
pub struct DropCloserWriteHalf {
    tx: Option<mpsc::Sender<Bytes>>,
    bytes_written: u64,
    eof_sent: Arc<AtomicBool>,
}

impl DropCloserWriteHalf {
    /// Sends data over the channel to the receiver.
    pub fn send(&mut self, buf: Bytes) -> impl Future<Output = Result<(), Error>> + '_ {
        self.send_get_bytes_on_error(buf).map_err(|err| err.0)
    }

    /// Sends data over the channel to the receiver.
    #[inline]
    async fn send_get_bytes_on_error(&mut self, buf: Bytes) -> Result<(), (Error, Bytes)> {
        let tx = match self
            .tx
            .as_ref()
            .ok_or_else(|| make_err!(Code::Internal, "Tried to send while stream is closed"))
        {
            Ok(tx) => tx,
            Err(e) => return Err((e, buf)),
        };
        let Ok(buf_len) = u64::try_from(buf.len()) else {
            return Err((
                make_err!(Code::Internal, "Could not convert usize to u64"),
                buf,
            ));
        };
        if buf_len == 0 {
            return Err((
                make_input_err!("Cannot send EOF in send(). Instead use send_eof()"),
                buf,
            ));
        }
        if let Err(err) = tx.send(buf).await {
            // Close our channel.
            self.tx = None;
            return Err((
                make_err!(
                    Code::Internal,
                    "Failed to write to data, receiver disconnected"
                ),
                err.0,
            ));
        }
        self.bytes_written += buf_len;
        Ok(())
    }

    /// Binds a reader and a writer together. This will send all the data from the reader
    /// to the writer until an EOF is received.
    /// This will always read one message ahead to ensure that if an error happens
    /// on the EOF message it will not forward on the last payload message and instead
    /// forward on the error.
    pub async fn bind_buffered(&mut self, reader: &mut DropCloserReadHalf) -> Result<(), Error> {
        loop {
            let chunk = reader
                .recv()
                .await
                .err_tip(|| "In DropCloserWriteHalf::bind_buffered::recv")?;
            if chunk.is_empty() {
                self.send_eof()
                    .err_tip(|| "In DropCloserWriteHalf::bind_buffered::send_eof")?;
                break; // EOF.
            }
            // Always read one message ahead so if we get an error on our EOF
            // we forward it on to the reader.
            if reader.peek().await.is_err() {
                // Read our next message for good book keeping.
                let _ = reader
                    .recv()
                    .await
                    .err_tip(|| "In DropCloserWriteHalf::bind_buffered::peek::eof")?;
                return Err(make_err!(
                    Code::Internal,
                    "DropCloserReadHalf::peek() said error, but when data received said Ok. This should never happen."
                ));
            }
            match self.send_get_bytes_on_error(chunk).await {
                Ok(()) => {}
                Err(e) => {
                    reader.queued_data.push_front(e.1);
                    return Err(e.0).err_tip(|| "In DropCloserWriteHalf::bind_buffered::send");
                }
            }
        }
        Ok(())
    }

    /// Sends an EOF (End of File) message to the receiver which will gracefully let the
    /// stream know it has no more data. This will close the stream.
    pub fn send_eof(&mut self) -> Result<(), Error> {
        // Flag that we have sent the EOF.
        let eof_was_sent = self.eof_sent.swap(true, Ordering::Release);
        if eof_was_sent {
            event!(
                Level::WARN,
                "Stream already closed when eof already was sent. This is often ok for retry was triggered, but should not happen on happy path."
            );
            return Ok(());
        }

        // Now close our stream.
        self.tx = None;
        Ok(())
    }

    /// Returns the number of bytes written so far. This does not mean the receiver received
    /// all of the bytes written to the stream so far.
    #[must_use]
    pub const fn get_bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Returns if the pipe was broken. This is good for determining if the reader broke the
    /// pipe or the writer broke the pipe, since this will only return true if the pipe was
    /// broken by the writer.
    #[must_use]
    pub const fn is_pipe_broken(&self) -> bool {
        self.tx.is_none()
    }
}

/// Reader half of the pair.
pub struct DropCloserReadHalf {
    rx: mpsc::Receiver<Bytes>,
    /// Number of bytes received over the stream.
    bytes_received: u64,
    eof_sent: Arc<AtomicBool>,
    /// If there was an error in the stream, this will be set to the last error.
    last_err: Option<Error>,
    /// If not empty, this is the data that needs to be sent out before
    /// data from the underlying channel can should be sent.
    queued_data: VecDeque<Bytes>,
    /// As data is being read from the stream, this buffer will be filled
    /// with the most recent data. Once `max_recent_data_size` is reached
    /// this buffer will be cleared and no longer be populated.
    /// This is useful if the caller wants to reset the the reader to before
    /// any of the data was received if possible (eg: something failed and
    /// we want to retry).
    recent_data: Vec<Bytes>,
    /// Amount of data to keep in the `recent_data` buffer before clearing it
    /// and no longer populating it.
    max_recent_data_size: u64,
}

impl DropCloserReadHalf {
    /// Returns if the stream has data ready.
    pub fn is_empty(&self) -> bool {
        self.rx.is_empty()
    }

    fn recv_inner(&mut self, chunk: Bytes) -> Result<Bytes, Error> {
        // `queued_data` is allowed to have empty bytes that represent EOF
        if chunk.is_empty() {
            if !self.eof_sent.load(Ordering::Acquire) {
                let err = make_err!(Code::Internal, "Sender dropped before sending EOF");
                self.queued_data.clear();
                self.recent_data.clear();
                self.bytes_received = 0;
                self.last_err = Some(err.clone());
                return Err(err);
            };

            self.maybe_populate_recent_data(&ZERO_DATA);
            return Ok(ZERO_DATA);
        };

        self.bytes_received += chunk.len() as u64;
        self.maybe_populate_recent_data(&chunk);
        Ok(chunk)
    }

    /// Try to receive a chunk of data, returning `None` if none is available.
    pub fn try_recv(&mut self) -> Option<Result<Bytes, Error>> {
        if let Some(err) = &self.last_err {
            return Some(Err(err.clone()));
        }
        self.queued_data.pop_front().map(Ok)
    }

    /// Receive a chunk of data, waiting asynchronously until some is available.
    pub async fn recv(&mut self) -> Result<Bytes, Error> {
        if let Some(result) = self.try_recv() {
            result
        } else {
            // `None` here indicates EOF, which we represent as Zero data
            let data = self.rx.recv().await.unwrap_or(ZERO_DATA);
            self.recv_inner(data)
        }
    }

    fn maybe_populate_recent_data(&mut self, chunk: &Bytes) {
        if self.max_recent_data_size == 0 {
            return; // Fast path.
        }
        if self.bytes_received > self.max_recent_data_size {
            if !self.recent_data.is_empty() {
                self.recent_data.clear();
            }
            return;
        }
        self.recent_data.push(chunk.clone());
    }

    /// Sets the maximum size of the `recent_data` buffer. If the number of bytes
    /// received exceeds this size, the `recent_data` buffer will be cleared and
    /// no longer populated.
    pub fn set_max_recent_data_size(&mut self, size: u64) {
        self.max_recent_data_size = size;
    }

    /// Attempts to reset the stream to before any data was received. This will
    /// only work if the number of bytes received is less than `max_recent_data_size`.
    ///
    /// On error the state of the stream is undefined and the caller should not
    /// attempt to use the stream again.
    pub fn try_reset_stream(&mut self) -> Result<(), Error> {
        if self.bytes_received > self.max_recent_data_size {
            return Err(make_err!(
                Code::Internal,
                "Cannot reset stream, max_recent_data_size exceeded"
            ));
        }
        let mut data_sum = 0;
        for chunk in self.recent_data.drain(..).rev() {
            data_sum += chunk.len() as u64;
            self.queued_data.push_front(chunk);
        }
        assert!(self.recent_data.is_empty(), "Recent_data should be empty");
        // Ensure the sum of the bytes in recent_data is equal to the bytes_received.
        error_if!(
            data_sum != self.bytes_received,
            "Sum of recent_data bytes does not equal bytes_received"
        );
        self.bytes_received = 0;
        Ok(())
    }

    /// Drains the reader until an EOF is received, but sends data to the void.
    pub async fn drain(&mut self) -> Result<(), Error> {
        loop {
            if self
                .recv()
                .await
                .err_tip(|| "Failed to drain in buf_channel::drain")?
                .is_empty()
            {
                break; // EOF.
            }
        }
        Ok(())
    }

    /// Peek the next set of bytes in the stream without consuming them.
    pub async fn peek(&mut self) -> Result<&Bytes, Error> {
        if self.queued_data.is_empty() {
            let chunk = self.recv().await.err_tip(|| "In buf_channel::peek")?;
            self.queued_data.push_front(chunk);
        }
        Ok(self
            .queued_data
            .front()
            .expect("Should have data in the queue"))
    }

    /// The number of bytes received over this stream so far.
    pub const fn get_bytes_received(&self) -> u64 {
        self.bytes_received
    }

    /// Takes exactly `size` number of bytes from the stream and returns them.
    /// This means the stream will keep polling until either an EOF is received or
    /// `size` bytes are received and concat them all together then return them.
    /// This method is optimized to reduce copies when possible.
    /// If `size` is None, it will take all the bytes in the stream.
    pub async fn consume(&mut self, size: Option<usize>) -> Result<Bytes, Error> {
        let size = size.unwrap_or(usize::MAX);
        let first_chunk = {
            let mut chunk = self
                .recv()
                .await
                .err_tip(|| "During first read of buf_channel::take()")?;
            if chunk.is_empty() {
                return Ok(chunk); // EOF.
            }
            if chunk.len() > size {
                let remaining = chunk.split_off(size);
                self.queued_data.push_front(remaining);
                // No need to read EOF if we are a partial chunk.
                return Ok(chunk);
            }
            // Try to read our EOF to ensure our sender did not error out.
            match self.peek().await {
                Ok(peeked_chunk) => {
                    if peeked_chunk.is_empty() || chunk.len() == size {
                        return Ok(chunk);
                    }
                }
                Err(e) => {
                    return Err(e.clone()).err_tip(|| "Failed to check if next chunk is EOF")?
                }
            }
            chunk
        };
        let mut output = BytesMut::new();
        output.extend_from_slice(&first_chunk);

        loop {
            let mut chunk = self
                .recv()
                .await
                .err_tip(|| "During first read of buf_channel::take()")?;
            if chunk.is_empty() {
                break; // EOF.
            }
            if output.len() + chunk.len() > size {
                // Slice off the extra data and put it back into the queue. We are done.
                let remaining = chunk.split_off(size - output.len());
                self.queued_data.push_front(remaining);
            }
            output.extend_from_slice(&chunk);
            if output.len() == size {
                break; // We are done.
            }
        }
        Ok(output.freeze())
    }
}

impl Stream for DropCloserReadHalf {
    type Item = Result<Bytes, std::io::Error>;

    // TODO(blaise.bruer) This is not very efficient as we are creating a new future on every
    // poll() call. It might be better to use a waker.
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Box::pin(self.recv())
            .as_mut()
            .poll(cx)
            .map(|result| match result {
                Ok(bytes) => {
                    if bytes.is_empty() {
                        return None;
                    }
                    Some(Ok(bytes))
                }
                Err(e) => Some(Err(e.to_std_err())),
            })
    }
}
