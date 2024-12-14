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
use hyper::http::{self, StatusCode};
use nativelink_config::cas_server::IdentityHeaderSpec;
use nativelink_proto::build::bazel::remote::execution::v2::RequestMetadata;
use nativelink_proto::com::github::trace_machina::nativelink::events::OriginEvent;
use prost::Message;
use tokio::sync::mpsc;
use tower::layer::Layer;
use tower::Service;
use tracing::trace_span;

use crate::origin_context::{ActiveOriginContext, ORIGIN_IDENTITY};
use crate::origin_event::{OriginEventCollector, ORIGIN_EVENT_COLLECTOR};

/// Default identity header name.
/// Note: If this is changed, the default value in the [`IdentityHeaderSpec`]
// TODO(allada) This has a mirror in bep_server.rs.
// We should consolidate these.
const DEFAULT_IDENTITY_HEADER: &str = "x-identity";

#[derive(Default, Clone)]
pub struct OriginRequestMetadata {
    pub identity: String,
    pub bazel_metadata: Option<RequestMetadata>,
}

#[derive(Clone)]
pub struct OriginEventMiddlewareLayer {
    maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,
    idenity_header_config: Arc<IdentityHeaderSpec>,
}

impl OriginEventMiddlewareLayer {
    pub fn new(
        maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,
        idenity_header_config: IdentityHeaderSpec,
    ) -> Self {
        Self {
            maybe_origin_event_tx,
            idenity_header_config: Arc::new(idenity_header_config),
        }
    }
}

impl<S> Layer<S> for OriginEventMiddlewareLayer {
    type Service = OriginEventMiddleware<S>;

    fn layer(&self, service: S) -> Self::Service {
        OriginEventMiddleware {
            inner: service,
            maybe_origin_event_tx: self.maybe_origin_event_tx.clone(),
            idenity_header_config: self.idenity_header_config.clone(),
        }
    }
}

#[derive(Clone)]
pub struct OriginEventMiddleware<S> {
    inner: S,
    maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,
    idenity_header_config: Arc<IdentityHeaderSpec>,
}

impl<S, ReqBody, ResBody> Service<http::Request<ReqBody>> for OriginEventMiddleware<S>
where
    S: Service<http::Request<ReqBody>, Response = http::Response<ResBody>> + Clone + Send + 'static,
    S::Future: Send + 'static,
    ReqBody: std::fmt::Debug + Send + 'static,
    ResBody: From<String> + Send + 'static,
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

        let mut context = ActiveOriginContext::fork().unwrap_or_default();
        let identity = {
            let identity_header = self
                .idenity_header_config
                .header_name
                .as_deref()
                .unwrap_or(DEFAULT_IDENTITY_HEADER);
            let identity = if !identity_header.is_empty() {
                req.headers()
                    .get(identity_header)
                    .and_then(|header| header.to_str().ok().map(str::to_string))
                    .unwrap_or_default()
            } else {
                String::new()
            };

            if identity.is_empty() && self.idenity_header_config.required {
                return Box::pin(async move {
                    Ok(http::Response::builder()
                        .status(StatusCode::UNAUTHORIZED)
                        .body("'identity_header' header is required".to_string().into())
                        .unwrap())
                });
            }
            context.set_value(&ORIGIN_IDENTITY, Arc::new(identity.clone()));
            identity
        };
        if let Some(origin_event_tx) = &self.maybe_origin_event_tx {
            let bazel_metadata = req
                .headers()
                .get("build.bazel.remote.execution.v2.requestmetadata-bin")
                .and_then(|header| BASE64_STANDARD_NO_PAD.decode(header.as_bytes()).ok())
                .and_then(|data| RequestMetadata::decode(data.as_slice()).ok());
            context.set_value(
                &ORIGIN_EVENT_COLLECTOR,
                Arc::new(OriginEventCollector::new(
                    origin_event_tx.clone(),
                    identity,
                    bazel_metadata,
                )),
            );
        }

        Box::pin(async move {
            Arc::new(context)
                .wrap_async(trace_span!("OriginEventMiddleware"), inner.call(req))
                .await
        })
    }
}
