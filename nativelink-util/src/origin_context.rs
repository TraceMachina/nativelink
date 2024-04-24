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

use std::cell::RefCell;
use std::env::var_os;
use std::mem::ManuallyDrop;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::Future;
use pin_project_lite::pin_project;
use tracing::instrument::Instrumented;

/// Context used to store data about the origin of a request.
#[derive(Default)]
pub struct OriginContext {
    // TODO(allada) Add fields that need to be shared globally.
}

impl OriginContext {
    pub fn init_for_test() {
        GLOBAL_ORIGIN_CONTEXT.set(Some(Arc::new(OriginContext::default())));
    }

    #[inline(always)]
    pub(crate) fn enter(self: Arc<OriginContext>) -> ContextDropGuard {
        ContextDropGuard::new(self)
    }

    /// Gets the hash algorithm requested by the origin.
    #[inline(always)]
    pub(crate) fn current() -> Option<Arc<OriginContext>> {
        GLOBAL_ORIGIN_CONTEXT.with_borrow(|maybe_global_ctx| maybe_global_ctx.clone())
    }
}

thread_local! {
  static GLOBAL_ORIGIN_CONTEXT: RefCell<Option<Arc<OriginContext>>> = const { RefCell::new(None) };
}

pub(crate) struct ContextDropGuard {
    prev_ctx: Option<Arc<OriginContext>>,
    new_ctx_ptr: *const OriginContext,
}

impl ContextDropGuard {
    #[inline(always)]
    fn new(new_ctx: Arc<OriginContext>) -> Self {
        let new_ctx_ptr = Arc::as_ptr(&new_ctx);
        let prev_ctx = GLOBAL_ORIGIN_CONTEXT.replace(Some(new_ctx));
        Self {
            prev_ctx,
            new_ctx_ptr,
        }
    }

    /// Release the context guard and restore the previous context.
    #[inline(always)]
    pub(crate) fn release(mut self) -> Arc<OriginContext> {
        let new_ctx = GLOBAL_ORIGIN_CONTEXT
            .replace(self.prev_ctx.take())
            .expect("Expected global context to be set");
        assert_eq!(self.new_ctx_ptr, Arc::as_ptr(&new_ctx));
        std::mem::forget(self); // Prevent the destructor from being called.
        new_ctx
    }
}

impl Drop for ContextDropGuard {
    #[inline(always)]
    fn drop(&mut self) {
        panic!("ContextDropGuard must be released using `release()`");
    }
}

fn enter_or_panic(value: &mut Option<Arc<OriginContext>>) -> ContextDropGuard {
    value.take().map_or_else(
        || {
            if var_os("BAZEL_TEST").is_some() || var_os("CARGO").is_some() {
                panic!(concat!(
                    "Failed to get context from thread local. ",
                    "Try calling `OriginContext::init_for_test()` ",
                    "before test starts (if this is a test)."
                ));
            }
            panic!("Expected OriginContext to be set on thread local");
        },
        |ctx| ctx.enter(),
    )
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
        #[inline(always)]
        fn drop(this: Pin<&mut Self>) {
            let this = this.project();
            let context = this.context.take().expect("Expected context to exist");
            let enter = context.enter();
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
            enter.release();
        }
    }
}

impl<T> ContextAwareFuture<T> {
    #[inline(always)]
    pub(crate) fn new(inner: Instrumented<T>, context: Option<Arc<OriginContext>>) -> Self {
        Self {
            inner: ManuallyDrop::new(inner),
            context,
        }
    }
}

impl<T: Future> Future for ContextAwareFuture<T> {
    type Output = T::Output;

    #[inline(always)]
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let enter = enter_or_panic(this.context);
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

#[inline(always)]
pub(crate) fn create_context_aware_fn_once<T, F: FnOnce() -> T>(inner: F) -> impl FnOnce() -> T {
    let mut maybe_ctx = OriginContext::current();
    move || {
        let enter = enter_or_panic(&mut maybe_ctx);
        let result = inner();
        enter.release();
        result
    }
}
