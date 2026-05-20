// Copyright 2026 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Tests for `ConnectionManager`'s permit accounting.
//!
//! The bug these tests exist to prevent: in production we observed
//! `available_connections: 18446744073709551589` (`u64::MAX − 26`) while
//! `waiting_connections` climbed unbounded, ultimately killing the worker
//! process via `OOMKilled` (exit 137). Switching from a manual `usize`
//! counter to `Arc<Semaphore>` with `OwnedSemaphorePermit` makes the leak
//! structurally impossible — these tests pin that property by exercising
//! the full request-acquire-release cycle through the public API many
//! times over a tight permit budget. With a leak, the cycle eventually
//! blocks forever; without one, every iteration completes inside the
//! per-call timeout.

use core::pin::Pin;
use core::time::Duration;
use std::sync::Arc;

use nativelink_config::stores::Retry;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::google::bytestream::byte_stream_server::{ByteStream, ByteStreamServer};
use nativelink_proto::google::bytestream::{
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};
use nativelink_util::background_spawn;
use nativelink_util::connection_manager::ConnectionManager;
use pretty_assertions::assert_eq;
use tokio::time::timeout;
use tokio_stream::Stream;
use tonic::transport::server::TcpIncoming;
use tonic::transport::{Endpoint, Server};
use tonic::{Request, Response, Status, Streaming};

#[derive(Clone)]
struct FakeByteStream;

#[tonic::async_trait]
impl ByteStream for FakeByteStream {
    type ReadStream = Pin<Box<dyn Stream<Item = Result<ReadResponse, Status>> + Send + 'static>>;

    async fn read(
        &self,
        _request: Request<ReadRequest>,
    ) -> Result<Response<Self::ReadStream>, Status> {
        Err(Status::unimplemented("fake"))
    }

    async fn write(
        &self,
        _request: Request<Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        Err(Status::unimplemented("fake"))
    }

    async fn query_write_status(
        &self,
        _request: Request<QueryWriteStatusRequest>,
    ) -> Result<Response<QueryWriteStatusResponse>, Status> {
        Err(Status::unimplemented("fake"))
    }
}

async fn fake_grpc_server_endpoint() -> Endpoint {
    let listener = TcpIncoming::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = listener.local_addr().unwrap().port();
    background_spawn!("connection_manager_test_server", async move {
        Server::builder()
            .add_service(ByteStreamServer::new(FakeByteStream))
            .serve_with_incoming(listener)
            .await
            .unwrap();
    });
    Endpoint::from_shared(format!("http://127.0.0.1:{port}")).unwrap()
}

/// Identity jitter so retry timing stays predictable in tests.
fn no_jitter() -> Arc<dyn Fn(Duration) -> Duration + Send + Sync> {
    Arc::new(|d| d)
}

#[nativelink_test]
async fn permits_released_on_drop_no_leak() -> Result<(), Error> {
    const MAX_CONCURRENT: usize = 2;
    const ITERATIONS: usize = 100;

    let endpoint = fake_grpc_server_endpoint().await;
    let cm = ConnectionManager::new(
        vec![endpoint],
        /* connections_per_endpoint = */ MAX_CONCURRENT,
        MAX_CONCURRENT,
        Retry::default(),
        no_jitter(),
    );

    for i in 0..ITERATIONS {
        let c1 = timeout(Duration::from_secs(5), cm.connection(format!("iter-{i}-a")))
            .await
            .unwrap_or_else(|_| panic!("iter {i}: first acquire blocked >5s — permit leak"))?;
        let c2 = timeout(Duration::from_secs(5), cm.connection(format!("iter-{i}-b")))
            .await
            .unwrap_or_else(|_| panic!("iter {i}: second acquire blocked >5s — permit leak"))?;
        drop(c1);
        drop(c2);
    }

    Ok(())
}

#[nativelink_test]
async fn aborted_caller_future_does_not_leak_permits() -> Result<(), Error> {
    const MAX_CONCURRENT: usize = 2;

    let endpoint = fake_grpc_server_endpoint().await;
    let cm = Arc::new(ConnectionManager::new(
        vec![endpoint],
        /* connections_per_endpoint = */ MAX_CONCURRENT,
        MAX_CONCURRENT,
        Retry::default(),
        no_jitter(),
    ));

    let mut handles = Vec::new();
    for i in 0..(MAX_CONCURRENT * 5) {
        let cm = Arc::clone(&cm);
        handles.push(tokio::spawn(async move {
            // Bind to `_conn` (not `_`) so the Connection lives until
            // task abort; bare `let _ = ...` would drop it immediately
            // and defeat the test.
            let _conn = cm.connection(format!("aborted-{i}")).await;
            futures::future::pending::<()>().await
        }));
    }
    tokio::time::sleep(Duration::from_millis(100)).await;
    for h in handles {
        h.abort();
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let c1 = timeout(Duration::from_secs(5), cm.connection("post-abort-a".into()))
        .await
        .expect("post-abort acquire 1 blocked >5s — permit leak")?;
    let c2 = timeout(Duration::from_secs(5), cm.connection("post-abort-b".into()))
        .await
        .expect("post-abort acquire 2 blocked >5s — permit leak")?;
    drop(c1);
    drop(c2);
    Ok(())
}

#[nativelink_test]
async fn extra_request_above_max_blocks_until_a_release() -> Result<(), Error> {
    const MAX_CONCURRENT: usize = 2;

    let endpoint = fake_grpc_server_endpoint().await;
    let cm = Arc::new(ConnectionManager::new(
        vec![endpoint],
        MAX_CONCURRENT + 1,
        MAX_CONCURRENT,
        Retry::default(),
        no_jitter(),
    ));

    let c1 = cm.connection("hold-1".into()).await?;
    let c2 = cm.connection("hold-2".into()).await?;

    // Third request must be queued — racing it against a short timeout
    // proves it doesn't resolve while permits are exhausted.
    let cm_for_third = Arc::clone(&cm);
    let third = tokio::spawn(async move { cm_for_third.connection("queued-3".into()).await });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        third.is_finished(),
        false,
        "third connection resolved while permits were exhausted",
    );

    // Drop one held permit; the queued request should now resolve.
    drop(c1);
    let c3 = timeout(Duration::from_secs(5), third)
        .await
        .expect("queued request did not resolve within 5s of permit release")
        .unwrap()?;

    drop(c2);
    drop(c3);
    Ok(())
}
