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

#![forbid(unsafe_code)]

use std::ops::DerefMut;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use fixed_buffer::FixedBuf;
use futures::Future;
use parking_lot::Mutex;
use pin_project::{pin_project, pinned_drop};
use tokio::io::{self, AsyncRead, AsyncWrite, ReadBuf, ReadHalf, WriteHalf};

// TODO (marcussorealheis): I'm not sure if the constant should be set at the module level or the struct level.
const DEFAULT_SIZE: usize = 4092;

/// This library was significantly inspired by:
/// https://docs.rs/fixed-buffer-tokio/0.1.1/fixed_buffer_tokio/
///
/// This struct will buffer the data between an `AsyncRead` and `AsyncWrite`
/// with a fixed buffer as an intermediary. In addition it gives the ability
/// to call `get_closer()` which if awaited will close the writer stream.
/// Finally, this struct can also deal with EOF in a more natural manner.
#[pin_project]
pub struct AsyncFixedBuf<const SIZE: usize> {
    inner: FixedBuf<SIZE>,
    waker: Arc<Mutex<Option<Waker>>>,
    did_shutdown: Arc<AtomicBool>,
    received_eof: AtomicBool,
}

fn wake(waker: &mut Option<Waker>) {
    if let Some(w) = waker.take() {
        w.wake();
    }
}

fn park(waker: &mut Option<Waker>, cx: &Context<'_>) {
    wake(waker);
    *waker = Some(cx.waker().clone());
}

impl <const SIZE: usize> AsyncFixedBuf<SIZE> {
    /// Creates a new FixedBuf and wraps it in an AsyncFixedBuf.
    ///
    /// See
    /// [`FixedBuf::new`](https://docs.rs/fixed-buffer/latest/fixed_buffer/struct.FixedBuf.html#method.new)
    /// for details.
    pub fn new() -> Self {
        AsyncFixedBuf {
            inner: FixedBuf::new(),
            waker: Arc::new(Mutex::new(None)),
            did_shutdown: Arc::new(AtomicBool::new(false)),
            received_eof: AtomicBool::new(false),
        }
    }

    /// Utility method that can be used to get a lambda that will close the
    /// stream. This is useful for the reader to close the stream.
    pub fn get_closer(&mut self) -> Pin<Box<dyn Future<Output = ()> + Sync + Send>> {
        let did_shutdown = self.did_shutdown.clone();
        let waker = self.waker.clone();
        Box::pin(async move {
            if did_shutdown.swap(true, Ordering::Relaxed) {
                return;
            }
            wake(waker.lock().deref_mut());
        })
    }
}

impl <const SIZE: usize> AsyncFixedBuf<SIZE> {
    pub fn split_into_reader_writer(mut self) -> (DropCloserReadHalf <SIZE>, DropCloserWriteHalf <SIZE>) {
        let did_shutdown_r = self.did_shutdown.clone();
        let did_shutdown_w = self.did_shutdown.clone();
        let closer_r = self.get_closer();
        let closer_w = self.get_closer();
        let (rx, tx) = io::split(self);
        (
            DropCloserReadHalf {
                inner: rx,
                did_shutdown: did_shutdown_r,
                close_fut: Some(closer_r),
            },
            DropCloserWriteHalf {
                inner: tx,
                did_shutdown: did_shutdown_w,
                close_fut: Some(closer_w),
            },
        )
    }
}

impl <const SIZE: usize> AsyncRead for AsyncFixedBuf<SIZE> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let me = self.project();
        let mut waker = me.waker.lock();

        let num_read = me.inner.read_and_copy_bytes(buf.initialize_unfilled());
        buf.advance(num_read);
        if num_read == 0 {
            if me.received_eof.load(Ordering::Relaxed) {
                wake(&mut waker);
                return Poll::Ready(Ok(()));
            } else if me.did_shutdown.load(Ordering::Relaxed) {
                wake(&mut waker);
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "Sender disconnected",
                )));
            }
        }
        // Have not yet reached EOF and have no data to read,
        // so we need to try and wake our writer, replace the waker and wait.
        if num_read == 0 {
            park(&mut waker, cx);
            return Poll::Pending;
        }
        wake(&mut waker);
        Poll::Ready(Ok(()))
    }
}

impl <const SIZE: usize> AsyncWrite for AsyncFixedBuf<SIZE> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, std::io::Error>> {
        let me = self.project();
        let mut waker = me.waker.lock();

        if me.did_shutdown.load(Ordering::Relaxed) {
            wake(&mut waker);
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Receiver disconnected",
            )));
        }
        if buf.is_empty() {
            // EOF happens when a zero byte message is sent.
            me.received_eof.store(true, Ordering::Relaxed);
            me.did_shutdown.store(true, Ordering::Relaxed);
            wake(&mut waker);
            return Poll::Ready(Ok(0));
        }
        let writable_slice = me.inner.writable();
        if !writable_slice.is_empty() {
            let write_amt = buf.len().min(writable_slice.len());
            writable_slice[..write_amt].clone_from_slice(&buf[..write_amt]);
            me.inner.wrote(write_amt);
            wake(&mut waker);
            Poll::Ready(Ok(write_amt))
        } else {
            // TODO (marcussorealheis): This is a placeholder for handling the no-space-left scenario.
            park(&mut waker, cx);
            Poll::Pending
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let mut waker = self.waker.lock();

        if self.inner.is_empty() {
            wake(&mut waker);
            return Poll::Ready(Ok(()));
        }
        park(&mut waker, cx);
        Poll::Pending
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let mut waker = self.waker.lock();

        self.did_shutdown.store(true, Ordering::Relaxed);
        wake(&mut waker);
        Poll::Ready(Ok(()))
    }
}

#[pin_project(PinnedDrop)]
pub struct DropCloserReadHalf<const SIZE: usize> {
    #[pin]
    inner: ReadHalf<AsyncFixedBuf<SIZE>>,
    did_shutdown: Arc<AtomicBool>,
    close_fut: Option<Pin<Box<dyn Future<Output = ()> + Sync + Send>>>,
}

#[pinned_drop]
impl <const SIZE: usize>PinnedDrop for DropCloserReadHalf<SIZE> {
    fn drop(mut self: Pin<&mut Self>) {
        if !self.did_shutdown.load(Ordering::Relaxed) {
            let close_fut = self.close_fut.take().unwrap();
            tokio::spawn(close_fut);
        }
    }
}

impl <const SIZE: usize>AsyncRead for DropCloserReadHalf<SIZE> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let me = self.project();
        me.inner.poll_read(cx, buf)
    }
}

#[pin_project(PinnedDrop)]
pub struct DropCloserWriteHalf<const SIZE: usize> {
    #[pin]
    inner: WriteHalf<AsyncFixedBuf<SIZE>>,
    did_shutdown: Arc<AtomicBool>,
    close_fut: Option<Pin<Box<dyn Future<Output = ()> + Sync + Send>>>,
}

#[pinned_drop]
impl <const SIZE: usize>PinnedDrop for DropCloserWriteHalf<SIZE> {
    fn drop(mut self: Pin<&mut Self>) {
        if !self.did_shutdown.load(Ordering::Relaxed) {
            let close_fut = self.close_fut.take().unwrap();
            tokio::spawn(close_fut);
        }
    }
}

impl <const SIZE: usize>AsyncWrite for DropCloserWriteHalf<SIZE> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, std::io::Error>> {
        let me = self.project();
        me.inner.poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let me = self.project();
        me.inner.poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        let me = self.project();
        me.inner.poll_shutdown(cx)
    }
}
