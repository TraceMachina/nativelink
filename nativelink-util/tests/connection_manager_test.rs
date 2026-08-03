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
            futures::future::pending::<()>().await;
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

/// The production "shard-ring wedge": an endpoint that is dead (nothing
/// listening — killed or never started) must yield a fast `Unavailable`
/// error from `connection()` instead of queuing the request forever while
/// the worker retries the connect in the background. Before the fix this
/// test failed: `cm.connection()` never resolved and the outer timeout
/// elapsed.
#[nativelink_test]
async fn dead_endpoint_fails_fast_instead_of_queuing_forever() -> Result<(), Error> {
    // Bind a port and immediately drop the listener so nothing is
    // listening there — connects get ECONNREFUSED, the "peer killed or
    // never started" case observed in production.
    let dead_port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let endpoint = Endpoint::from_shared(format!("http://127.0.0.1:{dead_port}"))
        .unwrap()
        .connect_timeout(Duration::from_secs(1));

    let cm = ConnectionManager::new(vec![endpoint], 1, 2, Retry::default(), no_jitter());

    let result = timeout(Duration::from_secs(10), cm.connection("dead-peer".into()))
        .await
        .expect("connection() to a dead endpoint hung past 10s instead of erroring");
    assert_eq!(
        result.err().map(|err| err.code),
        Some(nativelink_error::Code::Unavailable),
        "expected fast Unavailable from a dead endpoint",
    );
    Ok(())
}

/// The false-negative guard on the fast-fail gating: a busy-but-healthy
/// pool must NOT be declared dead. One endpoint, two connection slots: a
/// single-accept proxy in front of a real gRPC server lets exactly one
/// connect succeed, then drops its listener so the second slot's connect
/// is refused (`ECONNREFUSED`), marking the endpoint's last connect cycle
/// as failed. We hold the one live connection (so `available_channels` is
/// empty) and issue a new `connection()` request: with gating keyed only
/// on `available_channels` + connect health this errored `Unavailable`
/// immediately; with live-channel tracking it queues, and resolves as
/// soon as the held connection is released.
#[nativelink_test]
async fn busy_pool_with_transient_connect_failure_does_not_fail_fast() -> Result<(), Error> {
    // Real gRPC server the proxied connection terminates at.
    let backend = fake_grpc_server_endpoint().await;
    let backend_uri = backend.uri().clone();
    let backend_addr = format!(
        "{}:{}",
        backend_uri.host().unwrap(),
        backend_uri.port_u16().unwrap()
    );

    // Proxy that accepts exactly ONE connection, pipes it to the backend,
    // then drops the listener — every later connect is refused.
    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = proxy_listener.local_addr().unwrap().port();
    background_spawn!("single_accept_proxy", async move {
        let (mut inbound, _) = proxy_listener.accept().await.unwrap();
        // Drop the listener so the second connect slot gets ECONNREFUSED.
        drop(proxy_listener);
        let mut outbound = tokio::net::TcpStream::connect(backend_addr).await.unwrap();
        drop(tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await);
    });

    let endpoint = Endpoint::from_shared(format!("http://127.0.0.1:{proxy_port}"))
        .unwrap()
        .connect_timeout(Duration::from_secs(1));
    let cm = Arc::new(ConnectionManager::new(
        vec![endpoint],
        /* connections_per_endpoint = */ 2,
        /* max_concurrent_requests = */ 2,
        Retry::default(),
        no_jitter(),
    ));

    // Take (and hold) the one connection that could be established. This
    // may need a moment while the connect race settles; the winner is the
    // proxied slot, the loser marks the endpoint's connect cycle failed.
    let held = timeout(Duration::from_secs(10), cm.connection("held-live".into()))
        .await
        .expect("acquiring the single live connection hung")?;

    // Give the losing connect slot ample time to fail its cycle (default
    // retry is a single ~1s-connect-timeout attempt) and set the
    // per-endpoint failed flag; pre-fix this is what armed the trap.
    tokio::time::sleep(Duration::from_secs(3)).await;

    // A new request must queue, not fail fast: the pool is busy, not dead.
    let cm_for_queued = Arc::clone(&cm);
    let queued =
        tokio::spawn(async move { cm_for_queued.connection("queued-while-busy".into()).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(
        queued.is_finished(),
        false,
        "connection() failed fast while a live connection was checked out — \
         busy-but-healthy pool wrongly declared dead",
    );

    // Releasing the held connection must let the queued request through.
    drop(held);
    let conn = timeout(Duration::from_secs(5), queued)
        .await
        .expect("queued request did not resolve after the live connection was released")
        .unwrap()?;
    drop(conn);
    Ok(())
}

/// The teardown-to-zero window: when the LAST live channel is torn down
/// by an in-use transport error while every endpoint's last connect cycle
/// had already failed, queued waiters must be flushed with `Unavailable`
/// at that instant — not parked until the freshly scheduled reconnect
/// fails a whole connect cycle later (`handle_worker` already fails new
/// requests fast in that state; queued ones must not be second-class).
///
/// Setup: one endpoint, two connection slots, both piped through a proxy
/// to a real gRPC server; `max_concurrent_requests` of 2 with both
/// permits held so returned channel clones can never be handed to the
/// queued request. The retrier is configured so a connect cycle to the
/// (killed) endpoint takes ~3s: after severing both connections, an RPC
/// on the second held connection tears its channel down and its
/// reconnect cycle fails, arming the endpoint's connect-failed flag
/// while the first channel is still live. A request is then queued, and
/// an RPC on the first held connection tears the last live channel down
/// to zero. Post-fix the queued request errors `Unavailable` right
/// there; pre-fix nothing flushed it until the next connect cycle failed
/// ~3s later, so the 1.5s deadline below distinguishes the two.
#[nativelink_test]
async fn last_live_channel_teardown_flushes_queued_requests() -> Result<(), Error> {
    let backend = fake_grpc_server_endpoint().await;
    let backend_uri = backend.uri().clone();
    let backend_addr = format!(
        "{}:{}",
        backend_uri.host().unwrap(),
        backend_uri.port_u16().unwrap()
    );

    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_port = proxy_listener.local_addr().unwrap().port();
    let (kill_tx, kill_rx) = tokio::sync::watch::channel(false);
    background_spawn!("teardown_test_proxy", async move {
        // Pipe exactly the two initial connections (one per slot). On the
        // kill signal, drop the listener first (so reconnects are refused,
        // failing their connect cycles) and then sever both pipes.
        for _ in 0..2 {
            let (mut inbound, _) = proxy_listener.accept().await.unwrap();
            let mut rx = kill_rx.clone();
            let addr = backend_addr.clone();
            background_spawn!("teardown_test_pipe", async move {
                let mut outbound = tokio::net::TcpStream::connect(addr).await.unwrap();
                tokio::select! {
                    _ = tokio::io::copy_bidirectional(&mut inbound, &mut outbound) => {}
                    _ = rx.changed() => {}
                }
            });
        }
        let mut rx = kill_rx.clone();
        drop(rx.changed().await);
        drop(proxy_listener);
        // Pipes observe the same signal and sever their connections.
    });

    let endpoint = Endpoint::from_shared(format!("http://127.0.0.1:{proxy_port}"))
        .unwrap()
        .connect_timeout(Duration::from_secs(1));
    // delay 1.5 with one retry makes a failing connect cycle take one
    // ~3s backoff sleep between two instant ECONNREFUSED attempts: long
    // enough that "flushed at teardown" and "flushed by the next connect
    // failure" are cleanly distinguishable below.
    let retry = Retry {
        max_retries: 1,
        delay: 1.5,
        ..Default::default()
    };
    let cm = Arc::new(ConnectionManager::new(
        vec![endpoint],
        /* connections_per_endpoint = */ 2,
        /* max_concurrent_requests = */ 2,
        retry,
        no_jitter(),
    ));

    // Hold both permits (and both live channels).
    let held1 = timeout(Duration::from_secs(10), cm.connection("held-1".into()))
        .await
        .expect("acquiring the first piped connection hung")?;
    let held2 = timeout(Duration::from_secs(10), cm.connection("held-2".into()))
        .await
        .expect("acquiring the second piped connection hung")?;

    // Kill the endpoint: refuse new connects and sever both connections.
    tokio::time::sleep(Duration::from_millis(500)).await;
    kill_tx.send(true).unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Tear down the second channel with an in-use error; keep the client
    // (and its permit) alive. Its reconnect cycle fails ~3s later,
    // arming the endpoint's connect-failed flag while the first channel
    // keeps live_channels at 1.
    let mut client2 =
        nativelink_proto::google::bytestream::byte_stream_client::ByteStreamClient::new(held2);
    let rpc2 = timeout(
        Duration::from_secs(5),
        client2.query_write_status(QueryWriteStatusRequest {
            resource_name: "poke-2".into(),
        }),
    )
    .await
    .expect("RPC on the severed second connection hung");
    assert_eq!(rpc2.is_err(), true, "RPC on a severed connection succeeded");
    tokio::time::sleep(Duration::from_secs(4)).await;

    // Queue a request; it must wait (one channel still live, pool busy).
    let cm_for_queued = Arc::clone(&cm);
    let queued =
        tokio::spawn(async move { cm_for_queued.connection("queued-behind-held".into()).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        queued.is_finished(),
        false,
        "queued request resolved while a channel was live and permits were held",
    );

    // Tear down the LAST live channel with an in-use error.
    let mut client1 =
        nativelink_proto::google::bytestream::byte_stream_client::ByteStreamClient::new(held1);
    let rpc1 = timeout(
        Duration::from_secs(5),
        client1.query_write_status(QueryWriteStatusRequest {
            resource_name: "poke-1".into(),
        }),
    )
    .await
    .expect("RPC on the severed first connection hung");
    assert_eq!(rpc1.is_err(), true, "RPC on a severed connection succeeded");

    // The queued request must be flushed with Unavailable at the
    // teardown itself — the next connect-cycle failure is ~3s away.
    let queued_result = timeout(Duration::from_millis(1500), queued)
        .await
        .expect(
            "queued request still parked after the last live channel was torn down \
             with all endpoints' connect cycles failed",
        )
        .unwrap();
    assert_eq!(
        queued_result.err().map(|err| err.code),
        Some(nativelink_error::Code::Unavailable),
        "expected Unavailable for the queued request after teardown-to-zero",
    );
    drop(client1);
    drop(client2);
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
