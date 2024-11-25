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

use std::sync::Arc;

use base64::prelude::BASE64_STANDARD_NO_PAD;
use base64::Engine;
use futures::future::BoxFuture;
use futures::task::{Context, Poll};
use hyper::http;
use nativelink_proto::build::bazel::remote::execution::v2::RequestMetadata;
use nativelink_proto::com::github::trace_machina::nativelink::events::OriginEvent;
use prost::Message;
use tokio::sync::mpsc;
use tower::layer::Layer;
use tower::Service;
use tracing::trace_span;

use crate::make_symbol;
use crate::origin_context::OriginContext;
use crate::origin_event::ORIGIN_EVENT_COLLECTOR;

make_symbol!(BAZEL_REQUEST_METDATA, RequestMetadata);

#[derive(Clone)]
pub struct OriginEventMiddlewareLayer {
    origin_event_tx: Option<mpsc::Sender<OriginEvent>>,
}

impl OriginEventMiddlewareLayer {
    pub fn new(origin_event_tx: Option<mpsc::Sender<OriginEvent>>) -> Self {
        Self { origin_event_tx }
    }
}

impl<S> Layer<S> for OriginEventMiddlewareLayer {
    type Service = OriginEventMiddleware<S>;

    fn layer(&self, service: S) -> Self::Service {
        OriginEventMiddleware {
            inner: service,
            origin_event_tx: self.origin_event_tx.clone(),
        }
    }
}

#[derive(Clone)]
pub struct OriginEventMiddleware<S> {
    inner: S,
    origin_event_tx: Option<mpsc::Sender<OriginEvent>>,
}

fn maybe_add_bazel_metadata_to_context<ReqBody>(
    context: &mut OriginContext,
    req: &http::Request<ReqBody>,
) {
    req.headers()
        .get("build.bazel.remote.execution.v2.requestmetadata-bin")
        .and_then(|header| BASE64_STANDARD_NO_PAD.decode(header.as_bytes()).ok())
        .and_then(|data| RequestMetadata::decode(data.as_slice()).ok())
        .and_then(|metadata| context.set_value(&BAZEL_REQUEST_METDATA, Arc::new(metadata)));
}

impl<S, ReqBody, ResBody> Service<http::Request<ReqBody>> for OriginEventMiddleware<S>
where
    S: Service<http::Request<ReqBody>, Response = http::Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = BoxFuture<'static, Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<ReqBody>) -> Self::Future {
        // We must take the current `inner` and not the clone.
        // See: https://docs.rs/tower/latest/tower/trait.Service.html#be-careful-when-cloning-inner-services
        let clone = self.inner.clone();
        let mut inner = std::mem::replace(&mut self.inner, clone);

        let mut context = OriginContext::new();
        if let Some(origin_event_tx) = self.origin_event_tx.as_ref() {
            context.set_value(&ORIGIN_EVENT_COLLECTOR, Arc::new(origin_event_tx.clone()));
            maybe_add_bazel_metadata_to_context(&mut context, &req);
        }

        Box::pin(async move {
            Arc::new(context)
                .wrap_async(trace_span!("OriginEventMiddleware"), inner.call(req))
                .await
        })
    }
}
