// Copyright 2021 Nathan (Blaise) Bruer.  All rights reserved.

use std::pin::Pin;
use std::task::{Context, Poll};

use pin_project_lite::pin_project;
use tokio::io::AsyncWrite;

pin_project! {
    /// Utility struct that counts the number of bytes sent and passes everything else through to
    /// the underlying writer.
    pub struct WriteCounter<T: AsyncWrite> {
        #[pin]
        inner: T,
        bytes_written: u64,
    }
}

impl<T: AsyncWrite> WriteCounter<T> {
    pub fn new(inner: T) -> Self {
        WriteCounter {
            inner,
            bytes_written: 0,
        }
    }

    pub fn inner_ref<'a>(&'a self) -> &'a T {
        &self.inner
    }

    pub fn inner_mut<'a>(&'a mut self) -> &'a mut T {
        &mut self.inner
    }

    /// Returns the number of bytes written.
    pub fn get_bytes_written(&self) -> u64 {
        self.bytes_written
    }
}

impl<T: AsyncWrite> AsyncWrite for WriteCounter<T> {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, std::io::Error>> {
        let me = self.project();
        let result = me.inner.poll_write(cx, buf);
        if let Poll::Ready(Result::Ok(sz)) = &result {
            *me.bytes_written += *sz as u64;
        }
        result
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
