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
use std::time::Duration;

use futures::stream::unfold;
use hyper::client::connect::{Connected, Connection};
use hyper::service::Service;
use hyper::Uri;
use hyper_rustls::{HttpsConnector, MaybeHttpsStream};
use nativelink_config::stores::GcsSpec;
use nativelink_error::{make_err, Code, Error};
use nativelink_util::fs;
use nativelink_util::retry::{Retrier, RetryResult};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio::sync::SemaphorePermit;
use tokio::time::sleep;

/// A wrapper around connections that keeps a permit alive for the duration of the connection
pub struct ConnectionWithPermit<T: Connection + AsyncRead + AsyncWrite + Unpin> {
    connection: T,
    _permit: SemaphorePermit<'static>,
}

impl<T: Connection + AsyncRead + AsyncWrite + Unpin> Connection for ConnectionWithPermit<T> {
    fn connected(&self) -> Connected {
        self.connection.connected()
    }
}

impl<T: Connection + AsyncRead + AsyncWrite + Unpin> AsyncRead for ConnectionWithPermit<T> {
    #[inline]
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut Pin::get_mut(self).connection).poll_read(cx, buf)
    }
}

impl<T: Connection + AsyncWrite + AsyncRead + Unpin> AsyncWrite for ConnectionWithPermit<T> {
    #[inline]
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, tokio::io::Error>> {
        Pin::new(&mut Pin::get_mut(self).connection).poll_write(cx, buf)
    }

    #[inline]
    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut Pin::get_mut(self).connection).poll_flush(cx)
    }

    #[inline]
    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut Pin::get_mut(self).connection).poll_shutdown(cx)
    }
}

/// A TLS connector for GCS that handles connection pooling and retries
#[derive(Clone)]
pub struct GcsTlsConnector {
    connector: HttpsConnector<hyper::client::HttpConnector>,
    retrier: Retrier,
}

impl GcsTlsConnector {
    /// Create a new TLS connector
    #[must_use]
    pub fn new(spec: &GcsSpec, jitter_fn: Arc<dyn Fn(Duration) -> Duration + Send + Sync>) -> Self {
        let mut http_connector = hyper::client::HttpConnector::new();

        let timeout = spec.connect_timeout_secs.unwrap_or(15);
        http_connector.set_connect_timeout(Some(Duration::from_secs(timeout)));
        http_connector.enforce_http(false); // Allow HTTPS URLs
        http_connector.set_nodelay(true);

        let connector_with_roots = hyper_rustls::HttpsConnectorBuilder::new().with_native_roots();

        let connector_with_schemes = if spec.insecure_allow_http {
            connector_with_roots.https_or_http()
        } else {
            connector_with_roots.https_only()
        };

        let connector = if spec.disable_http2 {
            connector_with_schemes.enable_http1().build()
        } else {
            connector_with_schemes.enable_http1().enable_http2().build()
        };

        Self {
            connector,
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                jitter_fn,
                spec.retry.clone(),
            ),
        }
    }

    /// Call the connector with retry logic
    async fn call_with_retry(
        &self,
        req: &Uri,
    ) -> Result<ConnectionWithPermit<MaybeHttpsStream<TcpStream>>, Error> {
        let retry_stream_fn = unfold(self.connector.clone(), move |mut connector| async move {
            let _permit = fs::get_permit().await.unwrap();
            match connector.call(req.clone()).await {
                Ok(connection) => Some((
                    RetryResult::Ok(ConnectionWithPermit {
                        connection,
                        _permit,
                    }),
                    connector,
                )),
                Err(e) => Some((
                    RetryResult::Retry(make_err!(
                        Code::Unavailable,
                        "Failed to call GCS connector: {e:?}"
                    )),
                    connector,
                )),
            }
        });
        self.retrier.retry(retry_stream_fn).await
    }
}

impl Service<Uri> for GcsTlsConnector {
    type Response = ConnectionWithPermit<MaybeHttpsStream<TcpStream>>;
    type Error = Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.connector
            .poll_ready(cx)
            .map_err(|e| make_err!(Code::Unavailable, "Failed poll in GCS: {e}"))
    }

    fn call(&mut self, req: Uri) -> Self::Future {
        let connector_clone = self.clone();
        Box::pin(async move { connector_clone.call_with_retry(&req).await })
    }
}
