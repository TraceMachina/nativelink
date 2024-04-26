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

use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Future;
use tokio::task::{spawn_blocking, JoinError, JoinHandle};
pub use tracing::error_span as __error_span;
use tracing::{Instrument, Span};

#[inline(always)]
pub fn instrument_future<F, T>(f: F, span: Span) -> impl Future<Output = T>
where
    F: Future<Output = T> + Send + 'static,
{
    f.instrument(span)
}

#[inline(always)]
pub fn __spawn_with_span<F, T>(f: F, span: Span) -> JoinHandle<T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static,
{
    #[allow(clippy::disallowed_methods)]
    tokio::spawn(instrument_future(f, span))
}

#[inline(always)]
pub fn __spawn_blocking<F, T>(f: F, span: Span) -> JoinHandle<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    #[allow(clippy::disallowed_methods)]
    spawn_blocking(move || span.in_scope(f))
}

#[macro_export]
macro_rules! background_spawn {
    ($name:expr, $fut:expr) => {{
        $crate::task::__spawn_with_span($fut, $crate::task::__error_span!($name))
    }};
    ($name:expr, $fut:expr, $($fields:tt)*) => {{
        $crate::task::__spawn_with_span($fut, $crate::task::__error_span!($name, $($fields)*))
    }};
    (name: $name:expr, fut: $fut:expr, target: $target:expr, $($fields:tt)*) => {{
        $crate::task::__spawn_with_span($fut, $crate::task::__error_span!(target: $target, $name, $($fields)*))
    }};
}

#[macro_export]
macro_rules! spawn {
    ($name:expr, $fut:expr) => {{
        $crate::task::JoinHandleDropGuard::new($crate::background_spawn!($name, $fut))
    }};
    ($name:expr, $fut:expr, $($fields:tt)*) => {{
        $crate::task::JoinHandleDropGuard::new($crate::background_spawn!($name, $fut, $($fields)*))
    }};
    (name: $name:expr, fut: $fut:expr, target: $target:expr, $($fields:tt)*) => {{
        $crate::task::JoinHandleDropGuard::new($crate::background_spawn!($name, $fut, target: $target, $($fields)*))
    }};
}

#[macro_export]
macro_rules! spawn_blocking {
    ($name:expr, $fut:expr) => {{
        $crate::task::JoinHandleDropGuard::new($crate::task::__spawn_blocking($fut, $crate::task::__error_span!($name)))
    }};
    ($name:expr, $fut:expr, $($fields:tt)*) => {{
        $crate::task::JoinHandleDropGuard::new($crate::task::__spawn_blocking($fut, $crate::task::__error_span!($name, $($fields)*)))
    }};
    ($name:expr, $fut:expr, target: $target:expr) => {{
        $crate::task::JoinHandleDropGuard::new($crate::task::__spawn_blocking($fut, $crate::task::__error_span!(target: $target, $name)))
    }};
    ($name:expr, $fut:expr, target: $target:expr, $($fields:tt)*) => {{
        $crate::task::JoinHandleDropGuard::new($crate::task::__spawn_blocking($fut, $crate::task::__error_span!(target: $target, $name, $($fields)*)))
    }};
}

/// Simple wrapper that will abort a future that is running in another spawn in the
/// event that this handle gets dropped.
#[derive(Debug)]
#[must_use]
pub struct JoinHandleDropGuard<T> {
    inner: JoinHandle<T>,
}

impl<T> JoinHandleDropGuard<T> {
    pub fn new(inner: JoinHandle<T>) -> Self {
        Self { inner }
    }
}

impl<T> Future for JoinHandleDropGuard<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.inner).poll(cx)
    }
}

impl<T> Drop for JoinHandleDropGuard<T> {
    fn drop(&mut self) {
        self.inner.abort();
    }
}
