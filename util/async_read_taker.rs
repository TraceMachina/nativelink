// Copyright 2021 Nathan (Blaise) Bruer.  All rights reserved.

#![feature(ready_macro)]

use std::future::Future;
use std::marker::Send;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};

use fast_async_mutex::mutex::Mutex;
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, ReadBuf};

pub type ArcMutexAsyncRead = Arc<Mutex<Box<dyn AsyncRead + Send + Unpin + Sync + 'static>>>;

pin_project! {
    /// Useful object that can be used to chunk an AsyncReader by a specific size.
    /// This also requires the inner reader be sharable between threads. This allows
    /// the caller to still "own" the underlying reader in a way that once `limit` is
    /// reached the caller can keep using it, but still use this struct to read the data.
    pub struct AsyncReadTaker<F: FnOnce()> {
        inner: ArcMutexAsyncRead,
        done_fn: Option<F>,
        // Add '_' to avoid conflicts with `limit` method.
        limit_: usize,
    }
}

impl<F: FnOnce()> AsyncReadTaker<F> {
    /// `done_fn` can be used to pass a functor that will fire when the stream has no more data.
    pub fn new(inner: ArcMutexAsyncRead, done_fn: Option<F>, limit: usize) -> Self {
        AsyncReadTaker {
            inner,
            done_fn: done_fn,
            limit_: limit,
        }
    }
}

impl<F: FnOnce()> AsyncRead for AsyncReadTaker<F> {
    /// Note: This function is modeled after tokio::Take::poll_read.
    /// see: https://docs.rs/tokio/1.12.0/src/tokio/io/util/take.rs.html#77
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        if self.limit_ == 0 {
            return Poll::Ready(Ok(()));
        }

        let b = {
            let mut inner_fut = self.inner.lock_owned();
            let mut inner = Pin::new(ready!(Pin::new(&mut inner_fut).poll(cx)));
            // Now that we have all our locks lets begin changing our state.
            let mut b = buf.take(self.limit_);
            ready!(inner.as_mut().poll_read(cx, &mut b))?;
            b
        };
        let n = b.filled().len();
        // We need to update the original ReadBuf
        unsafe {
            buf.assume_init(n);
        }
        if n == 0 {
            if let Some(done_fn) = self.done_fn.take() {
                done_fn();
            }
        }
        buf.advance(n);
        self.limit_ -= n;

        Poll::Ready(Ok(()))
    }
}
