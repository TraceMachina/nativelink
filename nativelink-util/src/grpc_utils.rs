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

use std::collections::{HashMap, VecDeque};
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use futures::Future;
use nativelink_error::{make_err, Code, Error};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tonic::transport::{channel, Channel, Endpoint};
use tracing::{debug, info, warn};

/// A helper utility that enables management of a suite of connections to an
/// upstream gRPC endpoint using Tonic.
pub struct ConnectionManager {
    // The channel to request connections from the worker.
    worker_tx: UnboundedSender<WorkerRequest>,
}

/// The requests that can be made from the ConnectionManager to the
/// ConnectionManagerWorker such as requesting
enum WorkerRequest {
    Request(oneshot::Sender<Connection>),
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct ChannelIdentifier {
    /// The index into ConnectionManagerWorker::endpoints that established this
    /// Channel.
    endpoint_index: usize,
    /// A unique identifier for this particular connection to the Endpoint.
    iteration: usize,
}

/// The requests that can be made from a Connection to the
/// ConnectionManagerWorker such as informing it that it's been dropped or that
/// an error occurred.
enum ConnectionRequest {
    Dropped(ChannelIdentifier),
    Connected(ChannelIdentifier),
    Error(ChannelIdentifier),
}

/// The result of a Future that connects to a given Endpoint.  This is a tuple
/// of the index into the ConnectionManagerWorker::endpoints that this
/// connection is for, the iteration of the connection and the result of the
/// connection itself.
type IndexedChannel = (ChannelIdentifier, Result<Channel, tonic::transport::Error>);

/// A channel that has been established to an endpoint with some metadata around
/// it to allow identification of the Channel if it errors in order to correctly
/// remove it.
#[derive(Clone)]
struct EstablishedChannel {
    /// The Channel itself that the meta data relates to.
    channel: Channel,
    /// The identifier of the channel in the worker.
    identifier: ChannelIdentifier,
}

/// The context of the worker used to manage all of the connections.  This
/// handles reconnecting to endpoints on errors and multiple connections to a
/// given endpoint.
struct ConnectionManagerWorker {
    /// The endpoints to establish Channels and the current iteration of
    /// connection to that endpoint.
    endpoints: Vec<(usize, Endpoint)>,
    /// The channel used to communicate between a Connection and the worker.
    connection_tx: UnboundedSender<ConnectionRequest>,
    /// If the number of connections are restricted, then the number of
    /// connections that are currently allowed to be made.
    available_connections: Option<usize>,
    /// Channels that are currently being connected.
    connecting_channels: FuturesUnordered<Pin<Box<dyn Future<Output = IndexedChannel> + Send>>>,
    /// Connected channels that are available for use.
    available_channels: VecDeque<EstablishedChannel>,
    /// Connected channels that are waiting for poll_ready for the Service.
    /// This has a key of the endpoint index and iteration to the Channel value.
    pending_channels: HashMap<ChannelIdentifier, Channel>,
    /// Requests for a Channel when available.
    waiting_connections: VecDeque<oneshot::Sender<Connection>>,
}

impl ConnectionManager {
    /// Create a connection manager that creates a balance list between a given
    /// set of Endpoints.  This will restrict the number of concurrent requests
    /// assuming that the user of this connection manager uses the connection
    /// only once and reports all errors.
    pub fn new(
        endpoints: impl IntoIterator<Item = Endpoint>,
        connections_per_endpoint: usize,
        max_concurrent_requests: usize,
    ) -> Self {
        let (worker_tx, worker_rx) = mpsc::unbounded_channel();
        let (connection_tx, connection_rx) = mpsc::unbounded_channel();
        let endpoints = Vec::from_iter(endpoints.into_iter().map(|endpoint| (0, endpoint)));
        let worker = ConnectionManagerWorker {
            endpoints,
            available_connections: (max_concurrent_requests > 0).then_some(max_concurrent_requests),
            connection_tx,
            connecting_channels: FuturesUnordered::new(),
            available_channels: VecDeque::new(),
            pending_channels: HashMap::new(),
            waiting_connections: VecDeque::new(),
        };
        let connections_per_endpoint = if connections_per_endpoint > 0 {
            connections_per_endpoint
        } else {
            1
        };
        tokio::spawn(async move {
            worker
                .execute(connections_per_endpoint, worker_rx, connection_rx)
                .await;
        });
        Self { worker_tx }
    }

    /// Get a Connection that can be used as a tonic::Channel, except it
    /// performs some additional counting to reconnect on error and restrict
    /// the number of concurrent connections.
    pub async fn connection(&self) -> Result<Connection, Error> {
        let (tx, rx) = oneshot::channel();
        self.worker_tx
            .send(WorkerRequest::Request(tx))
            .map_err(|err| make_err!(Code::Unavailable, "Requesting a new connection: {err:?}"))?;
        rx.await
            .map_err(|err| make_err!(Code::Unavailable, "Waiting for a new connection: {err:?}"))
    }
}

impl ConnectionManagerWorker {
    async fn execute(
        mut self,
        connections_per_endpoint: usize,
        mut worker_rx: UnboundedReceiver<WorkerRequest>,
        mut connection_rx: UnboundedReceiver<ConnectionRequest>,
    ) {
        for (endpoint_index, endpoint) in self.endpoints.iter_mut().enumerate() {
            endpoint.0 = connections_per_endpoint;
            for iteration in 0..connections_per_endpoint {
                let endpoint = endpoint.1.clone();
                self.connecting_channels.push(Box::pin(async move {
                    debug!("Starting connection {iteration} to {endpoint:?}");
                    (
                        ChannelIdentifier {
                            endpoint_index,
                            iteration,
                        },
                        endpoint.connect().await,
                    )
                }));
            }
        }

        loop {
            tokio::select! {
                request = worker_rx.recv() => {
                    let Some(request) = request else {
                        // The ConnectionManager was dropped, shut down the
                        // worker.
                        break;
                    };
                    self.handle_worker(request);
                }
                // We own the tx, so this will never be None.
                Some(request) = connection_rx.recv() => {
                    self.handle_connection(request);
                }
                _ = self.connect_next() => {}
            }
        }
    }

    async fn connect_next(&mut self) {
        if self.connecting_channels.is_empty() {
            // Sleep for a really long time, this will be cancelled by the
            // select in execute.
            tokio::time::sleep(Duration::from_secs(100000)).await;
        } else if let Some((identifier, connection)) = self.connecting_channels.next().await {
            if let Ok(connection) = connection {
                self.available_channels.push_back(EstablishedChannel {
                    channel: connection,
                    identifier,
                });
                self.maybe_available_connection();
            } else {
                self.connect_endpoint(identifier.endpoint_index, Some(identifier.iteration))
            }
        }
    }

    fn connect_endpoint(&mut self, endpoint_index: usize, iteration: Option<usize>) {
        if let Some(endpoint) = self.endpoints.get_mut(endpoint_index) {
            let is_backoff = iteration.is_some();
            let iteration = iteration.unwrap_or_else(|| {
                endpoint.0 += 1;
                endpoint.0
            });
            let endpoint = endpoint.1.clone();
            if is_backoff {
                warn!("Connection {iteration} failed to {endpoint:?}, reconnecting.");
            } else {
                info!("Connection {iteration} to {endpoint:?} in error, reconnecting.");
            }
            self.connecting_channels.push(Box::pin(async move {
                if is_backoff {
                    // Re-connect after an error, add some backoff.
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
                (
                    ChannelIdentifier {
                        endpoint_index,
                        iteration,
                    },
                    endpoint.connect().await,
                )
            }))
        }
    }

    fn handle_worker(&mut self, request: WorkerRequest) {
        match request {
            WorkerRequest::Request(tx) => {
                if self
                    .available_connections
                    .is_some_and(|connections| connections == 0)
                {
                    self.waiting_connections.push_back(tx);
                } else if let Some(channel) = self.available_channels.pop_front() {
                    self.provide_channel(channel, tx);
                } else {
                    self.waiting_connections.push_back(tx);
                }
            }
        }
    }

    fn provide_channel(&mut self, channel: EstablishedChannel, tx: oneshot::Sender<Connection>) {
        if tx
            .send(Connection {
                connection_tx: self.connection_tx.clone(),
                channel: channel.clone(),
            })
            .is_ok()
        {
            self.pending_channels
                .insert(channel.identifier, channel.channel);
        } else {
            self.available_channels.push_front(channel);
        }
    }

    fn maybe_available_connection(&mut self) {
        if !self
            .available_connections
            .is_some_and(|available| available == 0)
            && !self.waiting_connections.is_empty()
        {
            if let Some(channel) = self.available_channels.pop_front() {
                if let Some(tx) = self.waiting_connections.pop_front() {
                    self.provide_channel(channel, tx);
                } else {
                    // This should never happen, but better than an unwrap.
                    self.available_channels.push_front(channel);
                }
            }
        }
    }

    fn pending_to_available(&mut self, identifier: ChannelIdentifier) {
        if let Some(channel) = self.pending_channels.remove(&identifier) {
            self.available_channels.push_back(EstablishedChannel {
                identifier,
                channel,
            })
        }
    }

    fn handle_connection(&mut self, request: ConnectionRequest) {
        match request {
            ConnectionRequest::Dropped(identifier) => {
                self.pending_to_available(identifier);
                if let Some(available_connections) = &mut self.available_connections {
                    *available_connections += 1;
                }
                self.maybe_available_connection();
            }
            ConnectionRequest::Connected(identifier) => {
                self.pending_to_available(identifier);
                self.maybe_available_connection();
            }
            // Handle a transport error on a connection by making it unavailable
            // for use and establishing a new connection to the endpoint.
            ConnectionRequest::Error(identifier) => {
                if self.pending_channels.remove(&identifier).is_some() {
                    self.connect_endpoint(identifier.endpoint_index, None);
                } else {
                    let original_length = self.available_channels.len();
                    self.available_channels
                        .retain(|channel| channel.identifier != identifier);
                    // Only reconnect if it wasn't already disconnected.
                    if original_length != self.available_channels.len() {
                        self.connect_endpoint(identifier.endpoint_index, None);
                    }
                }
            }
        }
    }
}

/// An instance of this is obtained for every communication with the gGRPC
/// service.  This handles the permit for limiting concurrency, and also
/// re-connecting the underlying channel on error.  It depends on users
/// reporting all errors.
pub struct Connection {
    /// Communication with ConnectionManagerWorker to inform about transport
    /// errors and when the Connection is dropped.
    connection_tx: UnboundedSender<ConnectionRequest>,
    /// The identifier to send to connection_tx.
    channel: EstablishedChannel,
}

impl Drop for Connection {
    fn drop(&mut self) {
        let _ = self
            .connection_tx
            .send(ConnectionRequest::Dropped(self.channel.identifier));
    }
}

/// A wrapper around the channel::ResponseFuture that forwards errors to the
/// connection_tx.
pub struct ResponseFuture {
    /// Communication with ConnectionManagerWorker to inform about transport
    /// errors.
    connection_tx: UnboundedSender<ConnectionRequest>,
    /// The identifier to send to connection_tx on a transport error.
    identifier: ChannelIdentifier,
    /// The wrapped future that actually does the work.
    inner: channel::ResponseFuture,
}

/// This is mostly copied from tonic::transport::channel except it wraps it
/// to allow messaging about connection success and failure.
impl tonic::codegen::Service<tonic::codegen::http::Request<tonic::body::BoxBody>> for Connection {
    type Response = tonic::codegen::http::Response<tonic::transport::Body>;
    type Error = tonic::transport::Error;
    type Future = ResponseFuture;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        let result = self.channel.channel.poll_ready(cx);
        if let Poll::Ready(result) = &result {
            let message = if result.is_ok() {
                ConnectionRequest::Connected(self.channel.identifier)
            } else {
                ConnectionRequest::Error(self.channel.identifier)
            };
            let _ = self.connection_tx.send(message);
        }
        result
    }

    fn call(
        &mut self,
        request: tonic::codegen::http::Request<tonic::body::BoxBody>,
    ) -> Self::Future {
        ResponseFuture {
            inner: self.channel.channel.call(request),
            connection_tx: self.connection_tx.clone(),
            identifier: self.channel.identifier,
        }
    }
}

/// This is mostly copied from tonic::transport::channel except it wraps it
/// to allow messaging about connection failure.
impl Future for ResponseFuture {
    type Output =
        Result<tonic::codegen::http::Response<tonic::transport::Body>, tonic::transport::Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let result = Pin::new(&mut self.inner).poll(cx);
        if let Poll::Ready(Err(_err)) = &result {
            let _ = self
                .connection_tx
                .send(ConnectionRequest::Error(self.identifier));
        }
        result
    }
}
