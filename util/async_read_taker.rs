// Copyright 2021 Nathan (Blaise) Bruer.  All rights reserved.

use std::future::Future;
use std::marker::Send;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use fast_async_mutex::mutex::Mutex;
use futures::ready;
use pin_project_lite::pin_project;
use tokio::io::{AsyncRead, ReadBuf};

pub type ArcMutexAsyncRead = Arc<Mutex<Box<dyn AsyncRead + Send + Unpin + Sync + 'static>>>;

// TODO(blaise.bruer) It does not look like this class is needed any more. Should consider removing it.
pin_project! {
    /// Useful object that can be used to chunk an AsyncReader by a specific size.
    /// This also requires the inner reader be sharable between threads. This allows
    /// the caller to still "own" the underlying reader in a way that once `limit` is
    /// reached the caller can keep using it, but still use this struct to read the data.
    pub struct AsyncReadTaker {
        inner: ArcMutexAsyncRead,
        // Add '_' to avoid conflicts with `limit` method.
        limit_: usize,
    }
}

impl AsyncReadTaker {
    pub fn new(inner: ArcMutexAsyncRead, limit: usize) -> Self {
        AsyncReadTaker { inner, limit_: limit }
    }
}

impl AsyncRead for AsyncReadTaker {
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
        buf.advance(n);
        self.limit_ -= n;

        Poll::Ready(Ok(()))
    }
}
