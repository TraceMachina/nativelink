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

use std::sync::Mutex;

use core::pin::Pin;
use core::task::{Context, Poll, Waker};
use fixed_buffer::FixedBuf;

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

// #[derive(Pin)]
pub struct AsyncFixedBuf<T> {
    inner: FixedBuf<T>,
    waker: Mutex<Option<Waker>>,
    did_shutdown: AtomicBool,
    write_amt: AtomicUsize,
    read_amt: AtomicUsize,
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
            waker: Mutex::new(None),
            did_shutdown: AtomicBool::new(false),
            write_amt: AtomicUsize::new(0),
            read_amt: AtomicUsize::new(0),
        }
    }

    fn park(&mut self, new_waker: &Waker) {
        let waker = self.waker.get_mut().unwrap();
        *waker = Some(new_waker.clone());
    }

    fn wake(&mut self) {
        let waker = self.waker.get_mut().unwrap();
        if let Some(w) = waker.take() {
            w.wake()
        }
    }
}

impl<T> Unpin for AsyncFixedBuf<T> {}

impl<T> std::ops::Deref for AsyncFixedBuf<T> {
    type Target = FixedBuf<T>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<T: AsRef<[u8]> + Unpin> tokio::io::AsyncRead for AsyncFixedBuf<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        let num_read = self.as_mut().inner.read_and_copy_bytes(buf);
        if num_read <= 0 && self.did_shutdown.load(Ordering::Relaxed) {
            return Poll::Ready(Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "Sender disconnected",
            )));
        }
        self.read_amt.fetch_add(num_read, Ordering::Relaxed);
        let mut result = Poll::Ready(Ok(num_read));
        if num_read <= 0 {
            self.park(cx.waker());
            result = Poll::Pending;
        } else {
            self.wake();
        }
        result
    }
}

impl<T: AsMut<[u8]>> tokio::io::AsyncWrite for AsyncFixedBuf<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        match self.inner.writable() {
            Some(writable_slice) => {
                let write_amt = buf.len().min(writable_slice.len());
                let mut result = Ok(write_amt);
                if write_amt > 0 {
                    writable_slice[..write_amt].clone_from_slice(&buf[..write_amt]);
                    self.inner.wrote(write_amt);
                } else if self.did_shutdown.load(Ordering::Relaxed) {
                    result = Err(std::io::Error::new(
                        std::io::ErrorKind::BrokenPipe,
                        "Receiver disconnected",
                    ));
                }

                self.wake();
                self.write_amt.fetch_add(write_amt, Ordering::Relaxed);
                Poll::Ready(result)
            }
            None => {
                self.park(cx.waker());
                Poll::Pending
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Result<(), std::io::Error>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        self.did_shutdown.store(true, Ordering::Relaxed);
        self.wake();
        Poll::Ready(Ok(()))
    }
}

impl<T> Drop for AsyncFixedBuf<T> {
    fn drop(&mut self) {
        self.wake();
    }
}
