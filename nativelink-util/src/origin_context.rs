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

use core::panic;
use std::cell::RefCell;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::Future;
use pin_project_lite::pin_project;
use tracing::instrument::Instrumented;
use tracing::{Instrument, Span};

use crate::background_spawn;

/// Context used to store data about the origin of a request.
#[derive(Default)]
pub struct OriginContext {
    // TODO(allada) Add fields that need to be shared globally.
}

impl OriginContext {
    /// Creates a builder that can be used to create an `OriginContext`.
    pub fn builder() -> OriginContextBuilder {
        OriginContextBuilder::default()
    }

    /// Returns the current running context.
    #[inline(always)]
    pub(crate) fn current() -> Option<Arc<OriginContext>> {
        GLOBAL_ORIGIN_CONTEXT.with_borrow(|maybe_global_ctx| maybe_global_ctx.clone())
    }

    /// Wraps a function so when it is called the passed in context is set as
    /// the current context and when the function exists the context is restored
    /// to the previous global context.
    #[inline]
    fn wrap<T>(
        self: Arc<OriginContext>,
        span: Span,
        func: impl FnOnce() -> T,
    ) -> impl FnOnce() -> T {
        move || {
            let enter = OriginContext::enter(self);
            let result = span.in_scope(func);
            enter.release();
            result
        }
    }

    /// Enters the context, storing the previous context. If the returned
    /// `ContextDropGuard` is dropped, the previous context is restored.
    #[inline(always)]
    fn enter(self: Arc<OriginContext>) -> ContextDropGuard {
        ContextDropGuard::new(self)
    }
}

/// Builder for creating a new `OriginContext`.
#[derive(Default)]
pub struct OriginContextBuilder {}

impl OriginContextBuilder {
    /// Builds the `OriginContext` and runs a function with the new context.
    /// When function is done running the previous context will be restored.
    /// Note: No global context can be set when using this function.
    #[inline]
    pub fn run<T>(self, span: Span, func: impl FnOnce() -> T) -> T {
        self.build().wrap(span, func)()
    }

    /// Builds the `OriginContext` and wraps a future so when it is called the
    /// passed in context is set as the current context and when the future
    /// exists the context is restored to the previous global context.
    #[inline(always)]
    pub fn wrap_async<T>(
        self,
        span: Span,
        fut: impl Future<Output = T>,
    ) -> impl Future<Output = T> {
        ContextAwareFuture::new(Some(self.build()), fut.instrument(span))
    }

    /// Builds the context and spawns a future with the context and span set.
    pub fn background_spawn(self, span: Span, fut: impl Future<Output = ()> + Send + 'static) {
        background_spawn!(span: span, ctx: Some(self.build()), fut: fut);
    }

    fn build(self) -> Arc<OriginContext> {
        Arc::new(OriginContext::default())
    }
}

thread_local! {
  /// Global context that is used to store the current context.
  static GLOBAL_ORIGIN_CONTEXT: RefCell<Option<Arc<OriginContext>>> = const { RefCell::new(None) };
}

/// Special guard struct that is used to hold the previous context and restore it
/// when the guard is released or dropped.
struct ContextDropGuard {
    prev_ctx: Option<Arc<OriginContext>>,
    new_ctx_ptr: *const OriginContext,
}

impl ContextDropGuard {
    /// Places the new context in the global context, stores the previous context
    /// and returns a new `ContextDropGuard`, so if it is ever released or dropped
    /// the previous context will be restored.
    #[inline(always)]
    fn new(new_ctx: Arc<OriginContext>) -> Self {
        let new_ctx_ptr = Arc::as_ptr(&new_ctx);
        let prev_ctx = GLOBAL_ORIGIN_CONTEXT.replace(Some(new_ctx));
        Self {
            prev_ctx,
            new_ctx_ptr,
        }
    }

    /// Swap the global context with the previous context.
    /// Returns the context used in the `.enter()`/`.new()` call.
    fn restore_global_context(&mut self) -> Arc<OriginContext> {
        let new_ctx = GLOBAL_ORIGIN_CONTEXT
            .replace(self.prev_ctx.take())
            .expect("Expected global context to be set");
        assert_eq!(self.new_ctx_ptr, Arc::as_ptr(&new_ctx));
        new_ctx
    }

    /// Release the context guard and restore the previous context.
    /// This is an optimization, so we don't need to clone our Arc
    /// if the caller can use the context after releasing it.
    #[inline(always)]
    fn release(mut self) -> Arc<OriginContext> {
        let new_ctx = self.restore_global_context();
        std::mem::forget(self); // Prevent the destructor from being called.
        new_ctx
    }
}

impl Drop for ContextDropGuard {
    #[inline(always)]
    fn drop(&mut self) {
        // If the user calls `.release()` the drop() will not be triggered.
        // If a panic happens, the drop() will be called and the `.release()`
        // will not be called, this is our safety net.
        self.restore_global_context();
    }
}

pin_project! {
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    pub struct ContextAwareFuture<F> {
        // `ManuallyDrop` is used so we can call `self.span.enter()` in the `drop()`
        // of our inner future, then drop the span.
        #[pin]
        inner: ManuallyDrop<Instrumented<F>>,
        context: Option<Arc<OriginContext>>,
    }

    impl <F> PinnedDrop for ContextAwareFuture<F> {
        #[inline]
        fn drop(this: Pin<&mut Self>) {
            let this = this.project();
            // Note: If the future panics, the context will not be restored, so
            // this is a best effort to provide access to our global context
            // in the desturctors the event of a panic.
            let _enter = this.context.take().map(|ctx| ctx.enter());
            // SAFETY: 1. `Pin::get_unchecked_mut()` is safe, because this isn't
            //             different from wrapping `T` in `Option` and calling
            //             `Pin::set(&mut this.inner, None)`, except avoiding
            //             additional memory overhead.
            //         2. `ManuallyDrop::drop()` is safe, because
            //            `PinnedDrop::drop()` is guaranteed to be called only
            //            once.
            unsafe {
                ManuallyDrop::drop(this.inner.get_unchecked_mut())
            }
        }
    }
}

impl<T> ContextAwareFuture<T> {
    #[must_use = "futures do nothing unless you `.await` or poll them"]
    #[inline(always)]
    pub fn new_from_current(inner: Instrumented<T>) -> ContextAwareFuture<T> {
        match OriginContext::current() {
            Some(ctx) => ContextAwareFuture::new(Some(ctx), inner),
            None => {
                // Useful to get tracing stack trace.
                tracing::error!("OriginContext must be set");
                panic!("OriginContext must be set");
            }
        }
    }

    #[must_use = "futures do nothing unless you `.await` or poll them"]
    #[inline(always)]
    pub(crate) fn new(context: Option<Arc<OriginContext>>, inner: Instrumented<T>) -> Self {
        Self {
            inner: ManuallyDrop::new(inner),
            context,
        }
    }
}

impl<T: Future> Future for ContextAwareFuture<T> {
    type Output = T::Output;

    #[inline]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let Some(ctx) = this.context.take() else {
            // Useful to get tracing stack trace.
            tracing::error!("Expected context to be set");
            panic!("Expected context to be set");
        };
        let enter = ctx.enter();
        // Since `inner` is only moved when the future itself is dropped, `inner` will
        // never move, so this should be safe.
        // see: https://docs.rs/tracing/0.1.40/src/tracing/instrument.rs.html#297
        let inner = unsafe { this.inner.map_unchecked_mut(|v| &mut **v) };
        let result = inner.poll(cx);
        assert!(
            this.context.replace(enter.release()).is_none(),
            "Expected context to be unset"
        );
        result
    }
}
