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

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tracing::{Instrument, Span};

use crate::origin_context::{ContextAwareFuture, OriginContext};
use crate::spawn;
#[derive(Clone)]
pub struct DropProtectedFuture<F, T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static + Unpin + Clone,
{
    future: ContextAwareFuture<F>,
}

impl<F, T> DropProtectedFuture<F, T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static + Unpin + Clone,
{
    pub fn new(f: F, span: Span, ctx: Option<Arc<OriginContext>>) -> Self {
        Self {
            future: ContextAwareFuture::new(ctx, f.instrument(span)),
        }
    }
}

impl<F, T> Future for DropProtectedFuture<F, T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static + Unpin + Clone,
{
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        match Pin::new(&mut self.future).poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(output) => Poll::Ready(output),
        }
    }
}

impl<F, T> Drop for DropProtectedFuture<F, T>
where
    T: Send + 'static,
    F: Future<Output = T> + Send + 'static + Unpin + Clone,
{
    fn drop(&mut self) {
        spawn!("DropProtectedFuture::drop", self.future.clone());
    }
}
