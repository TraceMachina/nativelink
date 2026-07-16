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

use core::future::Future;
use core::pin::Pin;
use core::task::{Context as TaskContext, Waker};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_lock::Mutex;
use bytes::Bytes;
use futures::Stream;
use nativelink_config::stores::{GrpcEndpoint, GrpcReadBatchingConfig, GrpcSpec, Retry, StoreType};
use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::content_addressable_storage_server::{
    ContentAddressableStorage, ContentAddressableStorageServer,
};
use nativelink_proto::build::bazel::remote::execution::v2::{
    BatchReadBlobsRequest, BatchReadBlobsResponse, BatchUpdateBlobsRequest,
    BatchUpdateBlobsResponse, FindMissingBlobsRequest, FindMissingBlobsResponse, GetTreeRequest,
    GetTreeResponse, SpliceBlobRequest, SpliceBlobResponse, SplitBlobRequest, SplitBlobResponse,
    batch_read_blobs_response,
};
use nativelink_proto::google::bytestream::byte_stream_server::{ByteStream, ByteStreamServer};
use nativelink_proto::google::bytestream::{
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};
use nativelink_proto::google::rpc::Status as RpcStatus;
use nativelink_store::grpc_store::GrpcStore;
use nativelink_util::background_spawn;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::StoreLike;
use tonic::transport::Server;
use tonic::transport::server::TcpIncoming;
use tonic::{Request, Response, Status, Streaming};

/// A fake CAS server that serves seeded blobs from `BatchReadBlobs` and
/// records every `BatchReadBlobs` request it receives. Digests listed in
/// `not_found_hashes` are reported as `NOT_FOUND` in the per-blob status.
#[derive(Debug, Clone)]
struct FakeCasServer {
    blobs: Arc<Mutex<HashMap<String, Bytes>>>,
    not_found_hashes: Arc<Mutex<Vec<String>>>,
    batch_read_requests: Arc<Mutex<Vec<BatchReadBlobsRequest>>>,
}

impl FakeCasServer {
    fn new() -> Self {
        Self {
            blobs: Arc::new(Mutex::new(HashMap::new())),
            not_found_hashes: Arc::new(Mutex::new(vec![])),
            batch_read_requests: Arc::new(Mutex::new(vec![])),
        }
    }
}

type GetTreeStream = Pin<Box<dyn Stream<Item = Result<GetTreeResponse, Status>> + Send + 'static>>;

#[tonic::async_trait]
impl ContentAddressableStorage for FakeCasServer {
    type GetTreeStream = GetTreeStream;

    #[allow(clippy::unimplemented)]
    async fn find_missing_blobs(
        &self,
        _grpc_request: Request<FindMissingBlobsRequest>,
    ) -> Result<Response<FindMissingBlobsResponse>, Status> {
        unimplemented!();
    }

    #[allow(clippy::unimplemented)]
    async fn batch_update_blobs(
        &self,
        _grpc_request: Request<BatchUpdateBlobsRequest>,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Status> {
        unimplemented!();
    }

    async fn batch_read_blobs(
        &self,
        grpc_request: Request<BatchReadBlobsRequest>,
    ) -> Result<Response<BatchReadBlobsResponse>, Status> {
        let request = grpc_request.into_inner();
        self.batch_read_requests.lock().await.push(request.clone());

        let blobs = self.blobs.lock().await;
        let not_found_hashes = self.not_found_hashes.lock().await;
        let mut responses = Vec::with_capacity(request.digests.len());
        // Real servers may dedupe duplicate digests in one request; mimic
        // that by returning exactly one entry per unique digest.
        let mut seen_hashes = HashSet::new();
        for digest in request.digests {
            if !seen_hashes.insert(digest.hash.clone()) {
                continue;
            }
            let (data, status) = if not_found_hashes.contains(&digest.hash) {
                (
                    Bytes::new(),
                    RpcStatus {
                        code: Code::NotFound as i32,
                        message: format!("Blob {} not found", digest.hash),
                        details: vec![],
                    },
                )
            } else if let Some(data) = blobs.get(&digest.hash) {
                (
                    data.clone(),
                    RpcStatus {
                        code: Code::Ok as i32,
                        message: String::new(),
                        details: vec![],
                    },
                )
            } else {
                return Err(Status::invalid_argument(format!(
                    "Blob {} was never seeded in FakeCasServer",
                    digest.hash
                )));
            };
            responses.push(batch_read_blobs_response::Response {
                digest: Some(digest),
                data,
                compressor: 0,
                status: Some(status),
            });
        }
        Ok(Response::new(BatchReadBlobsResponse { responses }))
    }

    #[allow(clippy::unimplemented)]
    async fn get_tree(
        &self,
        _grpc_request: Request<GetTreeRequest>,
    ) -> Result<Response<Self::GetTreeStream>, Status> {
        unimplemented!();
    }

    #[allow(clippy::unimplemented)]
    async fn split_blob(
        &self,
        _grpc_request: Request<SplitBlobRequest>,
    ) -> Result<Response<SplitBlobResponse>, Status> {
        unimplemented!();
    }

    #[allow(clippy::unimplemented)]
    async fn splice_blob(
        &self,
        _grpc_request: Request<SpliceBlobRequest>,
    ) -> Result<Response<SpliceBlobResponse>, Status> {
        unimplemented!();
    }
}

type ReadStream = Pin<Box<dyn Stream<Item = Result<ReadResponse, Status>> + Send + 'static>>;

/// A fake `ByteStream` server that serves the same seeded blobs as
/// [`FakeCasServer`] and records every `Read` request it receives.
#[derive(Debug, Clone)]
struct FakeByteStreamServer {
    blobs: Arc<Mutex<HashMap<String, Bytes>>>,
    read_requests: Arc<Mutex<Vec<ReadRequest>>>,
}

impl FakeByteStreamServer {
    fn new(blobs: Arc<Mutex<HashMap<String, Bytes>>>) -> Self {
        Self {
            blobs,
            read_requests: Arc::new(Mutex::new(vec![])),
        }
    }
}

#[tonic::async_trait]
impl ByteStream for FakeByteStreamServer {
    type ReadStream = ReadStream;

    async fn read(
        &self,
        grpc_request: Request<ReadRequest>,
    ) -> Result<Response<Self::ReadStream>, Status> {
        let request = grpc_request.into_inner();
        self.read_requests.lock().await.push(request.clone());

        // Resource names look like `{instance}/blobs/{fn}/{hash}/{size}`.
        let mut components = request.resource_name.rsplit('/');
        let _size = components.next();
        let hash = components.next().unwrap_or_default().to_string();
        let data = self
            .blobs
            .lock()
            .await
            .get(&hash)
            .cloned()
            .ok_or_else(|| Status::not_found(format!("Blob {hash} not seeded")))?;
        let offset = usize::try_from(request.read_offset)
            .map_err(|_| Status::invalid_argument("Bad read_offset"))?;
        if offset > data.len() {
            return Err(Status::out_of_range("read_offset past end of blob"));
        }
        let stream: Self::ReadStream = Box::pin(futures::stream::iter(vec![Ok(ReadResponse {
            data: data.slice(offset..),
        })]));
        Ok(Response::new(stream))
    }

    #[allow(clippy::unimplemented)]
    async fn write(
        &self,
        _grpc_request: Request<Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        unimplemented!();
    }

    #[allow(clippy::unimplemented)]
    async fn query_write_status(
        &self,
        _grpc_request: Request<QueryWriteStatusRequest>,
    ) -> Result<Response<QueryWriteStatusResponse>, Status> {
        unimplemented!();
    }
}

struct TestFixture {
    cas_server: FakeCasServer,
    bytestream_server: FakeByteStreamServer,
    store: Arc<GrpcStore>,
}

async fn make_fixture(read_batching: Option<GrpcReadBatchingConfig>) -> Result<TestFixture, Error> {
    let cas_server = FakeCasServer::new();
    let bytestream_server = FakeByteStreamServer::new(cas_server.blobs.clone());
    let listener = TcpIncoming::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = listener.local_addr().unwrap().port();

    let cas_service = ContentAddressableStorageServer::new(cas_server.clone());
    let bytestream_service = ByteStreamServer::new(bytestream_server.clone());
    background_spawn!("grpc_read_batching_test_server", async move {
        Server::builder()
            .add_service(cas_service)
            .add_service(bytestream_service)
            .serve_with_incoming(listener)
            .await
            .unwrap();
    });

    let spec = GrpcSpec {
        instance_name: String::new(),
        endpoints: vec![GrpcEndpoint {
            address: format!("http://localhost:{port}"),
            tls_config: None,
            concurrency_limit: None,
            connect_timeout_s: 0,
            tcp_keepalive_s: 0,
            http2_keepalive_interval_s: 0,
            http2_keepalive_timeout_s: 0,
        }],
        store_type: StoreType::Cas,
        retry: Retry::default(),
        max_concurrent_requests: 0,
        connections_per_endpoint: 0,
        rpc_timeout_s: 0,
        use_legacy_resource_names: false,
        headers: HashMap::new(),
        forward_headers: vec![],
        experimental_read_batching: read_batching,
    };
    let store = GrpcStore::new(&spec).await?;
    Ok(TestFixture {
        cas_server,
        bytestream_server,
        store,
    })
}

const fn batching_config() -> GrpcReadBatchingConfig {
    GrpcReadBatchingConfig {
        max_blob_size_bytes: 128 * 1024,
        max_batch_bytes: 3 * 1024 * 1024,
        dispatch_slots: 1,
        max_queued_bytes: 32 * 1024 * 1024,
    }
}

/// Creates a unique digest and 64-byte content for blob index `i`.
fn make_blob(i: usize) -> (DigestInfo, Bytes) {
    let hash = format!("{:064x}", i + 1);
    let content = Bytes::from(format!("{i:0>64}"));
    let digest = DigestInfo::try_new(&hash, content.len()).unwrap();
    (digest, content)
}

async fn seed_blobs(fixture: &TestFixture, blobs: &[(DigestInfo, Bytes)]) {
    let mut map = fixture.cas_server.blobs.lock().await;
    for (digest, content) in blobs {
        map.insert(digest.packed_hash().to_string(), content.clone());
    }
}

// Many concurrent small reads must be coalesced into BatchReadBlobs RPCs
// and never touch the ByteStream Read path.
#[nativelink_test]
async fn small_blob_reads_are_batched() -> Result<(), Error> {
    const NUM_BLOBS: usize = 200;

    let fixture = make_fixture(Some(batching_config())).await?;
    let mut blobs = Vec::with_capacity(NUM_BLOBS);
    for i in 0..NUM_BLOBS {
        blobs.push(make_blob(i));
    }
    seed_blobs(&fixture, &blobs).await;

    let mut read_futures = Vec::with_capacity(NUM_BLOBS);
    for (digest, _) in &blobs {
        read_futures.push(fixture.store.get_part_unchunked(*digest, 0, None));
    }
    let results = futures::future::join_all(read_futures).await;
    for (result, (_, expected_content)) in results.into_iter().zip(&blobs) {
        assert_eq!(&result?, expected_content);
    }

    let batch_read_requests = fixture.cas_server.batch_read_requests.lock().await;
    assert!(
        !batch_read_requests.is_empty(),
        "Expected BatchReadBlobs to be used"
    );
    let total_batched_digests: usize = batch_read_requests
        .iter()
        .map(|request| request.digests.len())
        .sum();
    assert_eq!(total_batched_digests, NUM_BLOBS);
    assert!(
        batch_read_requests.len() < NUM_BLOBS / 2,
        "Expected reads to be coalesced, got {} BatchReadBlobs calls",
        batch_read_requests.len()
    );
    assert_eq!(
        fixture.bytestream_server.read_requests.lock().await.len(),
        0,
        "ByteStream Read must not be used for batched reads"
    );
    Ok(())
}

// Blobs above max_blob_size_bytes must use the ByteStream Read path.
#[nativelink_test]
async fn large_blob_uses_stream_path() -> Result<(), Error> {
    let mut config = batching_config();
    config.max_blob_size_bytes = 1024;
    let fixture = make_fixture(Some(config)).await?;

    let hash = format!("{:064x}", 42_u32);
    let content = Bytes::from(vec![42_u8; 2048]);
    let digest = DigestInfo::try_new(&hash, content.len()).unwrap();
    seed_blobs(&fixture, &[(digest, content.clone())]).await;

    let data = fixture.store.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(data, content);

    assert_eq!(
        fixture.bytestream_server.read_requests.lock().await.len(),
        1
    );
    assert_eq!(fixture.cas_server.batch_read_requests.lock().await.len(), 0);
    Ok(())
}

// Partial reads (offset != 0) must bypass batching.
#[nativelink_test]
async fn partial_read_bypasses_batching() -> Result<(), Error> {
    let fixture = make_fixture(Some(batching_config())).await?;
    let (digest, content) = make_blob(7);
    seed_blobs(&fixture, &[(digest, content.clone())]).await;

    let data = fixture.store.get_part_unchunked(digest, 1, None).await?;
    assert_eq!(data, content.slice(1..));

    let read_requests = fixture.bytestream_server.read_requests.lock().await;
    assert_eq!(read_requests.len(), 1);
    assert_eq!(read_requests[0].read_offset, 1);
    assert_eq!(fixture.cas_server.batch_read_requests.lock().await.len(), 0);
    Ok(())
}

// A NOT_FOUND for one digest in a batch must fail only that read; the
// other reads in the same batch must succeed.
#[nativelink_test]
async fn batch_item_failure_is_isolated() -> Result<(), Error> {
    let fixture = make_fixture(Some(batching_config())).await?;
    let (good_digest1, good_content1) = make_blob(1);
    let (bad_digest, _) = make_blob(2);
    let (good_digest2, good_content2) = make_blob(3);
    seed_blobs(
        &fixture,
        &[
            (good_digest1, good_content1.clone()),
            (good_digest2, good_content2.clone()),
        ],
    )
    .await;
    fixture
        .cas_server
        .not_found_hashes
        .lock()
        .await
        .push(bad_digest.packed_hash().to_string());

    // On the single-threaded test runtime all three reads enqueue before
    // the detached dispatcher task gets to run, so they coalesce.
    let (good_result1, bad_result, good_result2) = futures::join!(
        fixture.store.get_part_unchunked(good_digest1, 0, None),
        fixture.store.get_part_unchunked(bad_digest, 0, None),
        fixture.store.get_part_unchunked(good_digest2, 0, None),
    );
    assert_eq!(good_result1?, good_content1);
    assert_eq!(good_result2?, good_content2);
    let bad_err = bad_result.expect_err("Expected NOT_FOUND digest to fail");
    assert_eq!(bad_err.code, Code::NotFound, "Got error: {bad_err:?}");

    // Confirm that the failed digest actually shared a batch with another
    // read (rather than each read getting its own RPC).
    let batch_read_requests = fixture.cas_server.batch_read_requests.lock().await;
    let bad_hash = bad_digest.packed_hash().to_string();
    assert!(
        batch_read_requests
            .iter()
            .any(|request| request.digests.len() > 1
                && request.digests.iter().any(|d| d.hash == bad_hash)),
        "Expected the failing digest to share a batch with other reads: {batch_read_requests:?}"
    );
    assert_eq!(
        fixture.bytestream_server.read_requests.lock().await.len(),
        0
    );
    Ok(())
}

// With batching unconfigured the store must behave exactly as before and
// never call BatchReadBlobs.
#[nativelink_test]
async fn disabled_batching_uses_stream_path() -> Result<(), Error> {
    let fixture = make_fixture(None).await?;
    let (digest, content) = make_blob(11);
    seed_blobs(&fixture, &[(digest, content.clone())]).await;

    let data = fixture.store.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(data, content);

    assert_eq!(
        fixture.bytestream_server.read_requests.lock().await.len(),
        1
    );
    assert_eq!(fixture.cas_server.batch_read_requests.lock().await.len(), 0);
    Ok(())
}

// Cancelling one read mid-flight must not abort the shared batch RPC nor
// strand the other queued reads.
#[nativelink_test]
async fn cancelled_read_does_not_affect_batch_mates() -> Result<(), Error> {
    let fixture = make_fixture(Some(batching_config())).await?;
    let (cancelled_digest, cancelled_content) = make_blob(1);
    let (digest_b, content_b) = make_blob(2);
    let (digest_c, content_c) = make_blob(3);
    seed_blobs(
        &fixture,
        &[
            (cancelled_digest, cancelled_content),
            (digest_b, content_b.clone()),
            (digest_c, content_c.clone()),
        ],
    )
    .await;

    // Poll one read just far enough to enqueue it, then drop it to
    // simulate cancellation before its batch has been dispatched.
    let waker = Waker::noop();
    let mut task_context = TaskContext::from_waker(waker);
    let mut cancelled_read = Box::pin(fixture.store.get_part_unchunked(cancelled_digest, 0, None));
    assert!(cancelled_read.as_mut().poll(&mut task_context).is_pending());
    drop(cancelled_read);

    let (result_b, result_c) = futures::join!(
        fixture.store.get_part_unchunked(digest_b, 0, None),
        fixture.store.get_part_unchunked(digest_c, 0, None),
    );
    assert_eq!(result_b?, content_b);
    assert_eq!(result_c?, content_c);
    assert_eq!(
        fixture.bytestream_server.read_requests.lock().await.len(),
        0
    );
    Ok(())
}

// Concurrent reads of the same digest must all succeed even though the
// digest is requested only once per batch and the server responds with a
// single entry for it.
#[nativelink_test]
async fn duplicate_digest_reads_all_succeed() -> Result<(), Error> {
    let fixture = make_fixture(Some(batching_config())).await?;
    let (digest, content) = make_blob(5);
    seed_blobs(&fixture, &[(digest, content.clone())]).await;

    let (result_a, result_b) = futures::join!(
        fixture.store.get_part_unchunked(digest, 0, None),
        fixture.store.get_part_unchunked(digest, 0, None),
    );
    assert_eq!(result_a?, content);
    assert_eq!(result_b?, content);

    // The client must dedupe: each request may carry the digest only once.
    let batch_read_requests = fixture.cas_server.batch_read_requests.lock().await;
    assert!(!batch_read_requests.is_empty());
    for request in batch_read_requests.iter() {
        assert_eq!(
            request.digests.len(),
            1,
            "Expected deduped digests: {request:?}"
        );
    }
    assert_eq!(
        fixture.bytestream_server.read_requests.lock().await.len(),
        0
    );
    Ok(())
}

// Batched reads share one upstream RPC, so combining them with per-client
// forwarded headers must be rejected at construction.
#[nativelink_test]
async fn forward_headers_with_batching_rejected() -> Result<(), Error> {
    let spec = GrpcSpec {
        instance_name: String::new(),
        endpoints: vec![GrpcEndpoint {
            address: "http://foobar".to_string(),
            tls_config: None,
            concurrency_limit: None,
            connect_timeout_s: 0,
            tcp_keepalive_s: 0,
            http2_keepalive_interval_s: 0,
            http2_keepalive_timeout_s: 0,
        }],
        store_type: StoreType::Cas,
        retry: Retry::default(),
        max_concurrent_requests: 0,
        connections_per_endpoint: 0,
        rpc_timeout_s: 0,
        use_legacy_resource_names: false,
        headers: HashMap::new(),
        forward_headers: vec!["authorization".to_string()],
        experimental_read_batching: Some(batching_config()),
    };
    let err = GrpcStore::new(&spec)
        .await
        .expect_err("Expected construction to fail");
    assert_eq!(err.code, Code::InvalidArgument, "Got error: {err:?}");
    Ok(())
}
