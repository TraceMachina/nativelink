// Copyright 2023 The Native Link Authors. All rights reserved.
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

use std::pin::Pin;
use std::task::Poll;

use bytes::{BufMut, Bytes, BytesMut};
use futures::task::Context;
use futures::{Future, Stream};
use nativelink_error::{error_if, make_err, Code, Error, ResultExt};
use tokio::sync::{mpsc, oneshot};
pub use tokio_util::io::StreamReader;

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
    let (close_tx, close_rx) = oneshot::channel();
    (
        DropCloserWriteHalf {
            tx: Some(tx),
            bytes_written: 0,
            close_rx,
            disable_eof_check: false,
        },
        DropCloserReadHalf {
            rx,
            partial: None,
            close_tx: Some(close_tx),
            close_after_size: u64::MAX,
        },
    )
}

/// Writer half of the pair.
pub struct DropCloserWriteHalf {
    tx: Option<mpsc::Sender<Result<Bytes, Error>>>,
    bytes_written: u64,
    /// Receiver channel used to know the error (or success) value of the
    /// receiver end's drop status (ie: if the receiver dropped unexpectedly).
    close_rx: oneshot::Receiver<Result<(), Error>>,
    disable_eof_check: bool,
}

impl DropCloserWriteHalf {
    /// Sends data over the channel to the receiver.
    pub async fn send(&mut self, buf: Bytes) -> Result<(), Error> {
        let tx = self
            .tx
            .as_ref()
            .ok_or_else(|| make_err!(Code::Internal, "Tried to send while stream is closed"))?;
        let buf_len = buf.len() as u64;
        error_if!(buf_len == 0, "Cannot send EOF in send(). Instead use send_eof()");
        let result = tx
            .send(Ok(buf))
            .await
            .map_err(|_| make_err!(Code::Internal, "Failed to write to data, receiver disconnected"));
        if result.is_err() {
            // Close our channel to prevent drop() from spawning a task.
            self.tx = None;
        }
        self.bytes_written += buf_len;
        result
    }

    /// Sends an EOF (End of File) message to the receiver which will gracefully let the
    /// stream know it has no more data. This will close the stream.
    pub async fn send_eof(&mut self) -> Result<(), Error> {
        error_if!(self.tx.is_none(), "Tried to send an EOF when pipe is broken");
        self.tx = None;

        // The final result will be provided in this oneshot channel.
        Pin::new(&mut self.close_rx)
            .await
            .map_err(|_| make_err!(Code::Internal, "Receiver went away before receiving EOF"))?
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

    /// Some remote receivers drop connections before we can send the EOF check.
    /// If the receiver handles failing streams it is safe to disable it.
    pub fn set_ignore_eof(&mut self) {
        self.disable_eof_check = true;
    }
}

impl Drop for DropCloserWriteHalf {
    /// This will notify the reader of an error if we did not send an EOF.
    fn drop(&mut self) {
        if tokio::runtime::Handle::try_current().is_err() {
            eprintln!("No tokio runtime active. Tx was dropped but can't send error.");
            return; // Cant send error, no runtime.
        }
        // Some remote receivers out of our control may close connections before
        // we can send the EOF check. If the remote receiver can be trusted to
        // handle incomplete data on its side we can disable this check.
        if !self.disable_eof_check {
            if let Some(tx) = self.tx.take() {
                // If we do not notify the receiver of the premature close of the stream (ie: without EOF)
                // we could end up with the receiver thinking everything is good and saving this bad data.
                tokio::spawn(async move {
                    let _ = tx
                        .send(Err(
                            make_err!(Code::Internal, "Writer was dropped before EOF was sent",),
                        ))
                        .await; // Nowhere to send failure to write here.
                });
            }
        }
    }
}

/// Reader half of the pair.
pub struct DropCloserReadHalf {
    rx: mpsc::Receiver<Result<Bytes, Error>>,
    /// Represents a partial chunk of data. This is used when we only wanted
    /// to take a part of the chunk in the stream and leave the rest.
    partial: Option<Result<Bytes, Error>>,
    /// A channel used to notify the sender that we are closed (with error).
    close_tx: Option<oneshot::Sender<Result<(), Error>>>,
    /// Once this number of bytes is sent the stream will be considered closed.
    /// This is a work around for cases when we never receive an EOF because the
    /// reader's future is dropped because it got the exact amount of data and
    /// will never poll more. This prevents the `drop()` handle from sending an
    /// error to our writer that we dropped the stream before receiving an EOF
    /// if we know the exact amount of data we will receive in this stream.
    close_after_size: u64,
}

impl DropCloserReadHalf {
    /// Receive a chunk of data.
    pub async fn recv(&mut self) -> Result<Bytes, Error> {
        let maybe_chunk = match self.partial.take() {
            Some(result_bytes) => Some(result_bytes),
            None => self.rx.recv().await,
        };
        match maybe_chunk {
            Some(Ok(chunk)) => {
                let chunk_len = chunk.len() as u64;
                error_if!(chunk_len == 0, "Chunk should never be EOF, expected None in this case");
                error_if!(
                    self.close_after_size < chunk_len,
                    "Received too much data. This only happens when `close_after_size` is set."
                );
                self.close_after_size -= chunk_len;
                if self.close_after_size == 0 {
                    error_if!(self.close_tx.is_none(), "Expected stream to not be closed");
                    self.close_tx.take().unwrap().send(Ok(())).map_err(|_| {
                        make_err!(Code::Internal, "Failed to send closing ok message to write with size")
                    })?;
                }
                Ok(chunk)
            }

            Some(Err(e)) => Err(make_err!(Code::Internal, "Received erroneous partial chunk: {e}")),

            // None is a safe EOF received.
            None => {
                // Notify our sender that we received the EOF with success.
                if let Some(close_tx) = self.close_tx.take() {
                    close_tx
                        .send(Ok(()))
                        .map_err(|_| make_err!(Code::Internal, "Failed to send closing ok message to write"))?;
                }
                Ok(Bytes::new())
            }
        }
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
    pub async fn peek(&mut self) -> &Result<Bytes, Error> {
        assert!(
            self.close_after_size == u64::MAX,
            "Can't call peek() when take() was called"
        );
        if self.partial.is_none() {
            self.partial = Some(self.recv().await);
        }
        if let Some(result) = &self.partial {
            return result;
        }
        unreachable!();
    }

    /// Sets the number of bytes before the stream will be considered closed.
    pub fn set_close_after_size(&mut self, size: u64) {
        self.close_after_size = size;
    }

    /// Utility function that will collect all the data of the stream into a Bytes struct.
    /// This method is optimized to reduce copies when possible.
    pub async fn collect_all_with_size_hint(mut self, size_hint: usize) -> Result<Bytes, Error> {
        let (first_chunk, second_chunk) = {
            // This is an optimization for when there's only one chunk and an EOF.
            // This prevents us from any copies and we just shuttle the bytes.
            let first_chunk = self
                .recv()
                .await
                .err_tip(|| "Failed to recv first chunk in collect_all_with_size_hint")?;

            if first_chunk.is_empty() {
                return Ok(first_chunk);
            }

            let second_chunk = self
                .recv()
                .await
                .err_tip(|| "Failed to recv second chunk in collect_all_with_size_hint")?;

            if second_chunk.is_empty() {
                return Ok(first_chunk);
            }
            (first_chunk, second_chunk)
        };

        let mut buf = BytesMut::with_capacity(size_hint);
        buf.put(first_chunk);
        buf.put(second_chunk);

        loop {
            let chunk = self
                .recv()
                .await
                .err_tip(|| "Failed to recv in collect_all_with_size_hint")?;
            if chunk.is_empty() {
                break; // EOF.
            }
            buf.put(chunk);
        }
        Ok(buf.freeze())
    }

    /// Takes exactly `size` number of bytes from the stream and returns them.
    /// This means the stream will keep polling until either an EOF is received or
    /// `size` bytes are received and concat them all together then return them.
    /// This method is optimized to reduce copies when possible.
    pub async fn take(&mut self, size: usize) -> Result<Bytes, Error> {
        fn populate_partial_if_needed(
            current_size: usize,
            desired_size: usize,
            chunk: &mut Bytes,
            partial: &mut Option<Result<Bytes, Error>>,
        ) {
            if current_size + chunk.len() <= desired_size {
                return;
            }
            assert!(partial.is_none(), "Partial should have been consumed during the recv()");
            let local_partial = chunk.split_off(desired_size - current_size);
            *partial = if local_partial.is_empty() {
                None
            } else {
                Some(Ok(local_partial))
            };
        }

        let (first_chunk, second_chunk) = {
            // This is an optimization for a relatively common case when the first chunk in the
            // stream satisfies all the requirements to fill this `take()`.
            // This will us from needing to copy the data into a new buffer and instead we can
            // just forward on the original Bytes object. If we need more than the first chunk
            // we will then go the slow path and actually copy our data.
            let mut first_chunk = self.recv().await.err_tip(|| "During first buf_channel::take()")?;
            populate_partial_if_needed(0, size, &mut first_chunk, &mut self.partial);
            if first_chunk.is_empty() || first_chunk.len() >= size {
                assert!(
                    first_chunk.is_empty() || first_chunk.len() == size,
                    "Length should be exactly size here"
                );
                return Ok(first_chunk);
            }

            let mut second_chunk = self.recv().await.err_tip(|| "During second buf_channel::take()")?;
            if second_chunk.is_empty() {
                assert!(
                    first_chunk.len() <= size,
                    "Length should never be larger than size here"
                );
                return Ok(first_chunk);
            }
            populate_partial_if_needed(first_chunk.len(), size, &mut second_chunk, &mut self.partial);
            (first_chunk, second_chunk)
        };
        let mut output = BytesMut::with_capacity(size);
        output.put(first_chunk);
        output.put(second_chunk);

        loop {
            let mut chunk = self.recv().await.err_tip(|| "During buf_channel::take()")?;
            if chunk.is_empty() {
                break; // EOF.
            }

            populate_partial_if_needed(output.len(), size, &mut chunk, &mut self.partial);

            output.put(chunk);

            if output.len() >= size {
                assert!(output.len() == size); // Length should never be larger than size here.
                break;
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
        Box::pin(self.recv()).as_mut().poll(cx).map(|result| match result {
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
