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

/// Trivial `ByteStream` service that errors on every method. We don't
/// actually call it — its sole purpose is to give the `tonic::Server`
/// an inner `Service` so `serve_with_incoming` accepts the listener.
/// `tonic::Channel`'s `poll_ready` only requires that the underlying
/// HTTP/2 connection can be established and stay open, which a vanilla
/// tonic Server with any service registered satisfies.
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

/// Stand up a tonic Server on `127.0.0.1:0` with the trivial
/// `FakeByteStream` registered. Returns an `Endpoint` pointing at it,
/// which `ConnectionManager` will drive through its full
/// `provide_channel` / `Connection` lifecycle paths.
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

/// Loop the request-acquire-drop cycle far more times than the permit
/// budget. Pre-fix, `available_connections` would underflow within the
/// first few iterations under any error path; post-fix, the
/// `OwnedSemaphorePermit` released by `Connection::drop` keeps the budget
/// balanced indefinitely. A 5-second per-call timeout is the failure
/// signal: the test fails if any single acquire blocks beyond it.
///
/// `connections_per_endpoint` matches `MAX_CONCURRENT` so the **permit**
/// is the gate being tested, not the channel pool.
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

/// When the requester drops the future returned by `connection(...)`
/// *before* the worker has handed the `Connection` over, the underlying
/// `oneshot::Sender::send` returns `Err(Connection)` inside
/// `provide_channel`. Pre-fix that path was particularly brittle: it
/// decremented the counter and depended on the synchronously-generated
/// `Connection::drop` → `ConnectionRequest::Dropped` round-trip
/// reaching the worker's queue *and* getting processed. Post-fix the
/// `OwnedSemaphorePermit` releases the moment the local `Connection`
/// goes out of scope on the worker's stack — no inter-task message
/// required. We verify that permit availability returns to baseline
/// even when caller futures are aborted in flight.
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

    // Round 1: spawn `MAX_CONCURRENT * 5` acquire futures and abort
    // them while still holding their `Connection`. Each task that
    // managed to take a permit must release it when its task is
    // aborted (Connection::drop runs on field drop). The queued tasks
    // (those that hadn't taken a permit yet) drop their oneshot
    // receiver, and when the worker later tries to deliver to them the
    // tx.send fails and the un-deliverable Connection is dropped on
    // the worker's stack — releasing its permit too.
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
    // Give the worker time to populate the channel pool, drain the
    // request queue, and let the first MAX_CONCURRENT tasks acquire
    // permits.
    tokio::time::sleep(Duration::from_millis(100)).await;
    for h in handles {
        h.abort();
    }
    // Yield long enough for the abort-driven drops to propagate the
    // permit release back to the semaphore, *and* for the worker to
    // process the cascade of `Dropped` messages that drain stale
    // entries from `waiting_connections`.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Round 2: full-budget acquire. Pre-fix the round-1 aborts could
    // leave the counter underflowed or saturated, and round-2 acquires
    // would block forever; post-fix every permit is back.
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

/// `MAX_CONCURRENT` simultaneous holders is the steady-state ceiling.
/// One more request must queue (rather than be served by an underflowed
/// counter, which is exactly what production was doing). We confirm
/// queuing by making the extra request race a 200 ms timeout against
/// the drop of one held connection: only after the drop does the queued
/// request resolve.
///
/// `connections_per_endpoint = MAX_CONCURRENT + 1` so the channel pool
/// is *not* the bottleneck — we want to prove the **permit** is what
/// blocks the third request, not channel availability.
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
