// Copyright 2020 Leonhard LLC
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use these files except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.
//
// This library is a heavily modified version of:
// https://docs.rs/fixed-buffer-tokio/0.1.1/fixed_buffer_tokio/

#![forbid(unsafe_code)]

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll, Waker};

use fast_async_mutex::mutex::Mutex;
use fixed_buffer::FixedBuf;
use futures::{ready, Future};
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

pin_project! {
    pub struct AsyncFixedBuf<T> {
        inner: FixedBuf<T>,
        waker: Arc<Mutex<Option<Waker>>>,
        did_shutdown: Arc<AtomicBool>,
        write_amt: AtomicUsize,
        read_amt: AtomicUsize,
        received_eof: AtomicBool,
    }
}

impl<T> AsyncFixedBuf<T> {
    /// Creates a new FixedBuf and wraps it in an AsyncFixedBuf.
    ///
    /// See
    /// [`FixedBuf::new`](https://docs.rs/fixed-buffer/latest/fixed_buffer/struct.FixedBuf.html#method.new)
    /// for details.
    pub fn new(mem: T) -> Self {
        AsyncFixedBuf {
            inner: FixedBuf::new(mem),
            waker: Arc::new(Mutex::new(None)),
            did_shutdown: Arc::new(AtomicBool::new(false)),
            write_amt: AtomicUsize::new(0),
            read_amt: AtomicUsize::new(0),
            received_eof: AtomicBool::new(false),
        }
    }

    // Utility method that can be used to get a lambda that will close the
    // stream. This is useful for the reader to close the stream.
    pub fn get_closer(&mut self) -> Pin<Box<dyn Future<Output = ()> + Sync + Send>> {
        let did_shutdown = self.did_shutdown.clone();
        let waker = self.waker.clone();
        Box::pin(async move {
            if did_shutdown.load(Ordering::Relaxed) {
                return;
            }
            did_shutdown.store(true, Ordering::Relaxed);
            let mut waker = waker.lock().await;
            if let Some(w) = waker.take() {
                w.wake();
            }
        })
    }

    fn park_poll(self: &Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut waker_lock = self.waker.lock();
        let mut waker = ready!(Pin::new(&mut waker_lock).poll(cx));
        assert!(waker.is_none(), "Can't park while waker is populated");
        *waker = Some(cx.waker().clone());
        Poll::Ready(())
    }

    fn wake_poll(self: &Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        let mut waker_lock = self.waker.lock();
        let mut waker = ready!(Pin::new(&mut waker_lock).poll(cx));
        if let Some(w) = waker.take() {
            w.wake();
        }
        Poll::Ready(())
    }
}

impl<T> std::ops::Deref for AsyncFixedBuf<T> {
    type Target = FixedBuf<T>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: AsRef<[u8]> + Unpin> AsyncRead for AsyncFixedBuf<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        let num_read = self.as_mut().inner.read_and_copy_bytes(buf.initialize_unfilled());
        buf.advance(num_read);
        if num_read <= 0 {
            if self.received_eof.load(Ordering::Relaxed) {
                self.received_eof.store(false, Ordering::Relaxed);
                return Poll::Ready(Ok(()));
            } else if self.did_shutdown.load(Ordering::Relaxed) {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "Sender disconnected",
                )));
            }
        }
        self.read_amt.fetch_add(num_read, Ordering::Relaxed);
        if num_read <= 0 {
            ready!(self.park_poll(cx));
            return Poll::Pending;
        }
        ready!(self.wake_poll(cx));
        Poll::Ready(Ok(()))
    }
}

impl<T: AsMut<[u8]>> AsyncWrite for AsyncFixedBuf<T> {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, std::io::Error>> {
        if self.did_shutdown.load(Ordering::Relaxed) {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Receiver disconnected",
            )));
        }
        match self.inner.writable() {
            Some(writable_slice) => {
                let write_amt = buf.len().min(writable_slice.len());
                if write_amt > 0 {
                    writable_slice[..write_amt].clone_from_slice(&buf[..write_amt]);
                    self.inner.wrote(write_amt);
                } else if buf.len() == 0 {
                    // EOF happens when a zero byte message is sent.
                    self.received_eof.store(true, Ordering::Relaxed);
                }

                ready!(self.wake_poll(cx));
                self.write_amt.fetch_add(write_amt, Ordering::Relaxed);
                Poll::Ready(Ok(write_amt))
            }
            None => {
                ready!(self.park_poll(cx));
                Poll::Pending
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        if self.inner.is_empty() {
            return Poll::Ready(Ok(()));
        }
        ready!(self.park_poll(cx));
        Poll::Pending
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        self.did_shutdown.store(true, Ordering::Relaxed);
        ready!(self.wake_poll(cx));
        Poll::Ready(Ok(()))
    }
}
