// Copyright 2025 The NativeLink Authors. All rights reserved.
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

use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll};
use std::sync::Arc;

use aws_config::retry::ErrorKind::TransientError;
use aws_smithy_runtime_api::client::http::{
    HttpClient as SmithyHttpClient, HttpConnector as SmithyHttpConnector, HttpConnectorFuture,
    HttpConnectorSettings, SharedHttpConnector,
};
use aws_smithy_runtime_api::client::orchestrator::HttpRequest;
use aws_smithy_runtime_api::client::result::ConnectorError;
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_runtime_api::http::Response;
use aws_smithy_types::body::SdkBody;
use bytes::{Bytes, BytesMut};
use futures::Stream;
use http_body::{Frame, SizeHint};
use http_body_util::BodyExt;
use hyper::{Method, Request};
use hyper_rustls::{HttpsConnector, HttpsConnectorBuilder};
use hyper_util::client::legacy::Client as LegacyClient;
use hyper_util::client::legacy::connect::HttpConnector as LegacyHttpConnector;
use hyper_util::rt::TokioExecutor;
use nativelink_config::stores::CommonObjectSpec;
// Note: S3 store should be very careful about the error codes it returns
// when in a retryable wrapper. Always prefer Code::Aborted or another
// retryable code over Code::InvalidArgument or make_input_err!().
// ie: Don't import make_input_err!() to help prevent this.
use nativelink_error::{Code, make_err};
use nativelink_util::buf_channel::DropCloserReadHalf;
use nativelink_util::fs;
use nativelink_util::retry::{Retrier, RetryResult};
use tokio::time::sleep;

#[derive(Clone)]
pub struct TlsClient {
    client: LegacyClient<HttpsConnector<LegacyHttpConnector>, SdkBody>,
    retrier: Retrier,
}

impl TlsClient {
    #[must_use]
    pub fn new(common: &CommonObjectSpec) -> Self {
        let connector_with_roots = HttpsConnectorBuilder::new().with_platform_verifier();

        let connector_with_schemes = if common.insecure_allow_http {
            connector_with_roots.https_or_http()
        } else {
            connector_with_roots.https_only()
        };

        let connector = if common.disable_http2 {
            connector_with_schemes.enable_http1().build()
        } else {
            connector_with_schemes.enable_http1().enable_http2().build()
        };

        Self::with_https_connector(common, connector)
    }

    pub fn with_https_connector(
        common: &CommonObjectSpec,
        connector: HttpsConnector<LegacyHttpConnector>,
    ) -> Self {
        let client = LegacyClient::builder(TokioExecutor::new()).build(connector);

        Self {
            client,
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                common.retry.make_jitter_fn(),
                common.retry.clone(),
            ),
        }
    }
}

impl core::fmt::Debug for TlsClient {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> Result<(), core::fmt::Error> {
        f.debug_struct("TlsClient").finish_non_exhaustive()
    }
}

impl SmithyHttpClient for TlsClient {
    fn http_connector(
        &self,
        _settings: &HttpConnectorSettings,
        _components: &RuntimeComponents,
    ) -> SharedHttpConnector {
        SharedHttpConnector::new(self.clone())
    }
}

struct RequestBuilder<'a> {
    components: &'a RequestComponents,
}

impl<'a> RequestBuilder<'a> {
    #[inline]
    const fn new(components: &'a RequestComponents) -> Self {
        Self { components }
    }

    #[inline]
    #[allow(unused_qualifications, reason = "false positive on hyper::http::Error")]
    fn build(&self) -> Result<Request<SdkBody>, hyper::http::Error> {
        let mut req_builder = Request::builder()
            .method(self.components.method.clone())
            .uri(self.components.uri.clone())
            .version(self.components.version);

        let headers_map = req_builder.headers_mut().unwrap();
        for (name, value) in &self.components.headers {
            headers_map.insert(name, value.clone());
        }

        match &self.components.body_data {
            BufferedBodyState::Cloneable(body) => {
                let cloned_body = body.try_clone().expect("Body should be cloneable");
                req_builder.body(cloned_body)
            }
            BufferedBodyState::Buffered(bytes) => req_builder.body(SdkBody::from(bytes.clone())),
            BufferedBodyState::Empty => req_builder.body(SdkBody::empty()),
        }
    }
}

mod execution {
    use super::conversions::ResponseExt;
    use super::{
        Code, HttpsConnector, LegacyClient, LegacyHttpConnector, RequestBuilder, RequestComponents,
        Response, RetryResult, SdkBody, fs, make_err,
    };

    #[inline]
    pub(crate) async fn execute_request(
        client: LegacyClient<HttpsConnector<LegacyHttpConnector>, SdkBody>,
        components: &RequestComponents,
    ) -> RetryResult<Response<SdkBody>> {
        let _permit = match fs::get_permit().await {
            Ok(permit) => permit,
            Err(e) => {
                return RetryResult::Retry(make_err!(
                    Code::Unavailable,
                    "Failed to acquire permit: {e}"
                ));
            }
        };

        let request = match RequestBuilder::new(components).build() {
            Ok(req) => req,
            Err(e) => {
                return RetryResult::Err(make_err!(
                    Code::Internal,
                    "Failed to create request: {e}",
                ));
            }
        };

        match client.request(request).await {
            Ok(resp) => RetryResult::Ok(resp.into_smithy_response()),
            Err(e) => RetryResult::Retry(make_err!(
                Code::Unavailable,
                "Failed request in S3Store: {e}"
            )),
        }
    }

    #[inline]
    pub(crate) fn create_retry_stream(
        client: LegacyClient<HttpsConnector<LegacyHttpConnector>, SdkBody>,
        components: RequestComponents,
    ) -> impl futures::Stream<Item = RetryResult<Response<SdkBody>>> {
        futures::stream::unfold(components, move |components| {
            let client_clone = client.clone();
            async move {
                let result = execute_request(client_clone, &components).await;

                Some((result, components))
            }
        })
    }
}

enum BufferedBodyState {
    Cloneable(SdkBody),
    Buffered(Bytes),
    Empty,
}

mod body_processing {
    use super::{BodyExt, BufferedBodyState, BytesMut, ConnectorError, SdkBody, TransientError};

    /// Buffer a request body fully into memory.
    ///
    /// TODO(aaronmondal): This could lead to OOMs in extremely constrained
    ///                    environments. Probably better to implement something
    ///                    like a rewindable stream logic.
    #[inline]
    pub(crate) async fn buffer_body(body: SdkBody) -> Result<BufferedBodyState, ConnectorError> {
        let mut bytes = BytesMut::new();
        let mut body_stream = body;
        while let Some(frame) = body_stream.frame().await {
            match frame {
                Ok(frame) => {
                    if let Some(data) = frame.data_ref() {
                        bytes.extend_from_slice(data);
                    }
                }
                Err(e) => {
                    return Err(ConnectorError::other(
                        format!("Failed to read request body: {e}").into(),
                        Some(TransientError),
                    ));
                }
            }
        }

        Ok(BufferedBodyState::Buffered(bytes.freeze()))
    }
}

struct RequestComponents {
    method: Method,
    uri: hyper::Uri,
    version: hyper::Version,
    headers: hyper::HeaderMap,
    body_data: BufferedBodyState,
}

mod conversions {
    use super::{
        BufferedBodyState, ConnectorError, Future, HttpRequest, Method, RequestComponents,
        Response, SdkBody, TransientError, body_processing,
    };

    pub(crate) trait RequestExt {
        fn into_components(self)
        -> impl Future<Output = Result<RequestComponents, ConnectorError>>;
    }

    impl RequestExt for HttpRequest {
        async fn into_components(self) -> Result<RequestComponents, ConnectorError> {
            // Note: This does *not* refer the the HTTP protocol, but to the
            //       version of the http crate.
            let hyper_req = self.try_into_http1x().map_err(|e| {
                ConnectorError::other(
                    format!("Failed to convert to HTTP request: {e}").into(),
                    Some(TransientError),
                )
            })?;

            let method = hyper_req.method().clone();
            let uri = hyper_req.uri().clone();
            let version = hyper_req.version();
            let headers = hyper_req.headers().clone();

            let body = hyper_req.into_body();

            // Only buffer bodies for methods likely to have payloads.
            let needs_buffering = matches!(method, Method::POST | Method::PUT);

            // Preserve the body in case we need to retry.
            let body_data = if needs_buffering {
                if let Some(cloneable_body) = body.try_clone() {
                    BufferedBodyState::Cloneable(cloneable_body)
                } else {
                    body_processing::buffer_body(body).await?
                }
            } else {
                BufferedBodyState::Empty
            };

            Ok(RequestComponents {
                method,
                uri,
                version,
                headers,
                body_data,
            })
        }
    }

    pub(crate) trait ResponseExt {
        fn into_smithy_response(self) -> Response<SdkBody>;
    }

    impl ResponseExt for hyper::Response<hyper::body::Incoming> {
        fn into_smithy_response(self) -> Response<SdkBody> {
            let (parts, body) = self.into_parts();
            let sdk_body = SdkBody::from_body_1_x(body);
            let mut smithy_resp = Response::new(parts.status.into(), sdk_body);
            let header_pairs: Vec<(String, String)> = parts
                .headers
                .iter()
                .filter_map(|(name, value)| {
                    value
                        .to_str()
                        .ok()
                        .map(|value_str| (name.as_str().to_owned(), value_str.to_owned()))
                })
                .collect();

            for (name, value) in header_pairs {
                smithy_resp.headers_mut().insert(name, value);
            }

            smithy_resp
        }
    }
}

impl SmithyHttpConnector for TlsClient {
    fn call(&self, req: HttpRequest) -> HttpConnectorFuture {
        use conversions::RequestExt;

        let client = self.client.clone();
        let retrier = self.retrier.clone();

        HttpConnectorFuture::new(Box::pin(async move {
            let components = req.into_components().await?;

            let retry_stream = execution::create_retry_stream(client, components);

            match retrier.retry(retry_stream).await {
                Ok(response) => Ok(response),
                Err(e) => Err(ConnectorError::other(
                    format!("Connection failed after retries: {e}").into(),
                    Some(TransientError),
                )),
            }
        }))
    }
}

#[derive(Debug)]
pub struct BodyWrapper {
    pub reader: DropCloserReadHalf,
    pub size: u64,
}

impl http_body::Body for BodyWrapper {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let reader = Pin::new(&mut Pin::get_mut(self).reader);
        reader
            .poll_next(cx)
            .map(|maybe_bytes_res| maybe_bytes_res.map(|res| res.map(Frame::data)))
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(self.size)
    }
}
