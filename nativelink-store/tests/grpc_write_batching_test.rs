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

use core::pin::Pin;
use std::collections::HashMap;
use std::sync::Arc;

use async_lock::Mutex;
use bytes::Bytes;
use futures::{Stream, StreamExt};
use nativelink_config::stores::{
    CacheMetricsSpec, FastSlowSpec, GrpcEndpoint, GrpcSpec, GrpcWriteBatchingConfig, MemorySpec,
    NoopSpec, Retry, StoreSpec, StoreType,
};
use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::content_addressable_storage_server::{
    ContentAddressableStorage, ContentAddressableStorageServer,
};
use nativelink_proto::build::bazel::remote::execution::v2::{
    BatchReadBlobsRequest, BatchReadBlobsResponse, BatchUpdateBlobsRequest,
    BatchUpdateBlobsResponse, FindMissingBlobsRequest, FindMissingBlobsResponse, GetTreeRequest,
    GetTreeResponse, SpliceBlobRequest, SpliceBlobResponse, SplitBlobRequest, SplitBlobResponse,
    batch_update_blobs_response,
};
use nativelink_proto::google::bytestream::byte_stream_server::{ByteStream, ByteStreamServer};
use nativelink_proto::google::bytestream::{
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};
use nativelink_proto::google::rpc::Status as RpcStatus;
use nativelink_store::cache_metrics_store::CacheMetricsStore;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::grpc_store::GrpcStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::background_spawn;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::{Store, StoreKey, StoreLike, StoreOptimizations};
use tonic::transport::Server;
use tonic::transport::server::TcpIncoming;
use tonic::{Request, Response, Status, Streaming};

/// A fake CAS server that records every `BatchUpdateBlobs` request and
/// stores its blobs. Digests listed in `error_hashes` are answered with the
/// configured per-entry status code instead of being stored.
#[derive(Debug, Clone)]
struct FakeCasServer {
    blobs: Arc<Mutex<HashMap<String, Bytes>>>,
    error_hashes: Arc<Mutex<HashMap<String, i32>>>,
    batch_update_requests: Arc<Mutex<Vec<BatchUpdateBlobsRequest>>>,
    /// When set, every `BatchUpdateBlobs` RPC fails as a whole.
    fail_batch_rpc: Arc<Mutex<bool>>,
}

impl FakeCasServer {
    fn new() -> Self {
        Self {
            blobs: Arc::new(Mutex::new(HashMap::new())),
            error_hashes: Arc::new(Mutex::new(HashMap::new())),
            batch_update_requests: Arc::new(Mutex::new(vec![])),
            fail_batch_rpc: Arc::new(Mutex::new(false)),
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

    async fn batch_update_blobs(
        &self,
        grpc_request: Request<BatchUpdateBlobsRequest>,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Status> {
        let request = grpc_request.into_inner();
        self.batch_update_requests
            .lock()
            .await
            .push(request.clone());
        if *self.fail_batch_rpc.lock().await {
            return Err(Status::resource_exhausted(
                "Injected whole-RPC failure (e.g. message-size limit)",
            ));
        }

        let mut blobs = self.blobs.lock().await;
        let error_hashes = self.error_hashes.lock().await;
        let mut responses = Vec::with_capacity(request.requests.len());
        for entry in request.requests {
            let Some(digest) = entry.digest else {
                return Err(Status::invalid_argument("Missing digest in request"));
            };
            let status = if let Some(&code) = error_hashes.get(&digest.hash) {
                RpcStatus {
                    code,
                    message: format!("Injected error for {}", digest.hash),
                    details: vec![],
                }
            } else {
                blobs.insert(digest.hash.clone(), entry.data);
                RpcStatus {
                    code: Code::Ok as i32,
                    message: String::new(),
                    details: vec![],
                }
            };
            responses.push(batch_update_blobs_response::Response {
                digest: Some(digest),
                status: Some(status),
            });
        }
        Ok(Response::new(BatchUpdateBlobsResponse { responses }))
    }

    #[allow(clippy::unimplemented)]
    async fn batch_read_blobs(
        &self,
        _grpc_request: Request<BatchReadBlobsRequest>,
    ) -> Result<Response<BatchReadBlobsResponse>, Status> {
        unimplemented!();
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

/// A fake `ByteStream` server that accepts uploads into the same blob map as
/// [`FakeCasServer`] and records the resource name of every `Write` stream.
#[derive(Debug, Clone)]
struct FakeByteStreamServer {
    blobs: Arc<Mutex<HashMap<String, Bytes>>>,
    write_resource_names: Arc<Mutex<Vec<String>>>,
}

impl FakeByteStreamServer {
    fn new(blobs: Arc<Mutex<HashMap<String, Bytes>>>) -> Self {
        Self {
            blobs,
            write_resource_names: Arc::new(Mutex::new(vec![])),
        }
    }
}

#[tonic::async_trait]
impl ByteStream for FakeByteStreamServer {
    type ReadStream = ReadStream;

    #[allow(clippy::unimplemented)]
    async fn read(
        &self,
        _grpc_request: Request<ReadRequest>,
    ) -> Result<Response<Self::ReadStream>, Status> {
        unimplemented!();
    }

    async fn write(
        &self,
        grpc_request: Request<Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        let mut stream = grpc_request.into_inner();
        let mut resource_name = String::new();
        let mut data = Vec::new();
        while let Some(request) = stream.next().await {
            let request = request?;
            if resource_name.is_empty() && !request.resource_name.is_empty() {
                resource_name.clone_from(&request.resource_name);
            }
            data.extend_from_slice(&request.data);
            if request.finish_write {
                break;
            }
        }
        // Resource names look like `{instance}/uploads/{uuid}/blobs/{hash}/{size}`.
        let mut components = resource_name.rsplit('/');
        let _size = components.next();
        let hash = components.next().unwrap_or_default().to_string();
        self.write_resource_names
            .lock()
            .await
            .push(resource_name.clone());
        let committed_size =
            i64::try_from(data.len()).map_err(|_| Status::invalid_argument("Upload too large"))?;
        self.blobs.lock().await.insert(hash, Bytes::from(data));
        Ok(Response::new(WriteResponse { committed_size }))
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

async fn make_fixture(
    write_batching: Option<GrpcWriteBatchingConfig>,
) -> Result<TestFixture, Error> {
    let cas_server = FakeCasServer::new();
    let bytestream_server = FakeByteStreamServer::new(cas_server.blobs.clone());
    let listener = TcpIncoming::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = listener.local_addr().unwrap().port();

    let cas_service = ContentAddressableStorageServer::new(cas_server.clone());
    let bytestream_service = ByteStreamServer::new(bytestream_server.clone());
    background_spawn!("grpc_write_batching_test_server", async move {
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
        experimental_read_batching: None,
        experimental_write_batching: write_batching,
    };
    let store = GrpcStore::new(&spec).await?;
    Ok(TestFixture {
        cas_server,
        bytestream_server,
        store,
    })
}

const fn batching_config() -> GrpcWriteBatchingConfig {
    GrpcWriteBatchingConfig {
        max_blob_size_bytes: 128 * 1024,
        max_batch_bytes: 3 * 1024 * 1024,
    }
}

/// Creates a unique digest and 64-byte content for blob index `i`.
fn make_blob(i: usize) -> (DigestInfo, Bytes) {
    let hash = format!("{:064x}", i + 1);
    let content = Bytes::from(format!("{i:0>64}"));
    let digest = DigestInfo::try_new(&hash, content.len()).unwrap();
    (digest, content)
}

fn make_items(blobs: &[(DigestInfo, Bytes)]) -> Vec<(StoreKey<'static>, Bytes)> {
    blobs
        .iter()
        .map(|(digest, content)| (StoreKey::from(*digest), content.clone()))
        .collect()
}

// Many small uploads through update_many must be packed into BatchUpdateBlobs
// RPCs and never open a ByteStream Write stream.
#[nativelink_test]
async fn small_blob_updates_are_batched() -> Result<(), Error> {
    const NUM_BLOBS: usize = 200;

    let fixture = make_fixture(Some(batching_config())).await?;
    let blobs: Vec<_> = (0..NUM_BLOBS).map(make_blob).collect();

    fixture.store.update_many(make_items(&blobs)).await?;

    for (digest, content) in &blobs {
        let stored = fixture
            .cas_server
            .blobs
            .lock()
            .await
            .get(&digest.packed_hash().to_string())
            .cloned();
        assert_eq!(stored.as_ref(), Some(content), "for {digest}");
    }
    let batch_requests = fixture.cas_server.batch_update_requests.lock().await;
    assert_eq!(batch_requests.len(), 1, "expected one batched RPC");
    assert_eq!(batch_requests[0].requests.len(), NUM_BLOBS);
    let stream_writes = fixture.bytestream_server.write_resource_names.lock().await;
    assert_eq!(stream_writes.len(), 0, "no ByteStream writes expected");
    Ok(())
}

// Duplicate digests within one update_many call must be uploaded exactly once.
#[nativelink_test]
async fn duplicate_digests_are_uploaded_once() -> Result<(), Error> {
    let fixture = make_fixture(Some(batching_config())).await?;
    let (digest, content) = make_blob(7);
    let items = vec![
        (StoreKey::from(digest), content.clone()),
        (StoreKey::from(digest), content.clone()),
        (StoreKey::from(digest), content.clone()),
    ];

    fixture.store.update_many(items).await?;

    let batch_requests = fixture.cas_server.batch_update_requests.lock().await;
    assert_eq!(batch_requests.len(), 1);
    assert_eq!(
        batch_requests[0].requests.len(),
        1,
        "digest must be deduped"
    );
    Ok(())
}

// Blobs above max_blob_size_bytes must use the ByteStream Write path even
// when batching is enabled.
#[nativelink_test]
async fn large_blobs_fall_back_to_streaming() -> Result<(), Error> {
    let config = GrpcWriteBatchingConfig {
        max_blob_size_bytes: 64,
        max_batch_bytes: 3 * 1024 * 1024,
    };
    let fixture = make_fixture(Some(config)).await?;

    let (small_digest, small_content) = make_blob(1);
    let large_content = Bytes::from(vec![0xabu8; 1024]);
    let large_hash = format!("{:064x}", 0xdead_beefu64);
    let large_digest = DigestInfo::try_new(&large_hash, large_content.len()).unwrap();

    fixture
        .store
        .update_many(vec![
            (StoreKey::from(small_digest), small_content.clone()),
            (StoreKey::from(large_digest), large_content.clone()),
        ])
        .await?;

    let batch_requests = fixture.cas_server.batch_update_requests.lock().await;
    assert_eq!(batch_requests.len(), 1);
    assert_eq!(batch_requests[0].requests.len(), 1, "only the small blob");
    let stream_writes = fixture.bytestream_server.write_resource_names.lock().await;
    assert_eq!(stream_writes.len(), 1, "large blob must stream");
    let stored_large = fixture
        .cas_server
        .blobs
        .lock()
        .await
        .get(&large_digest.packed_hash().to_string())
        .cloned();
    assert_eq!(stored_large, Some(large_content));
    Ok(())
}

// A retryable per-entry error must fall back to the ByteStream Write path
// for that entry while its batch peers succeed.
#[nativelink_test]
async fn retryable_entry_error_falls_back_to_streaming() -> Result<(), Error> {
    let fixture = make_fixture(Some(batching_config())).await?;
    let blobs: Vec<_> = (0..3).map(make_blob).collect();
    fixture.cas_server.error_hashes.lock().await.insert(
        blobs[1].0.packed_hash().to_string(),
        Code::Unavailable as i32,
    );

    fixture.store.update_many(make_items(&blobs)).await?;

    let stream_writes = fixture.bytestream_server.write_resource_names.lock().await;
    assert_eq!(
        stream_writes.len(),
        1,
        "only the failed entry falls back to streaming"
    );
    assert!(
        stream_writes[0].contains(&blobs[1].0.packed_hash().to_string()),
        "fallback must be for the failed digest"
    );
    // All blobs must be durable in the end.
    for (digest, content) in &blobs {
        let stored = fixture
            .cas_server
            .blobs
            .lock()
            .await
            .get(&digest.packed_hash().to_string())
            .cloned();
        assert_eq!(stored.as_ref(), Some(content), "for {digest}");
    }
    Ok(())
}

// A non-retryable per-entry error must fail the whole update_many call.
#[nativelink_test]
async fn non_retryable_entry_error_propagates() -> Result<(), Error> {
    let fixture = make_fixture(Some(batching_config())).await?;
    let blobs: Vec<_> = (0..3).map(make_blob).collect();
    fixture.cas_server.error_hashes.lock().await.insert(
        blobs[1].0.packed_hash().to_string(),
        Code::InvalidArgument as i32,
    );

    let result = fixture.store.update_many(make_items(&blobs)).await;
    let err = result.expect_err("expected update_many to fail");
    assert_eq!(err.code, Code::InvalidArgument, "{err:?}");
    Ok(())
}

// Without the config flag, update_many must use the ByteStream Write path
// for every blob (default loop behavior) and advertise no optimization.
#[nativelink_test]
async fn disabled_config_uses_streaming() -> Result<(), Error> {
    const NUM_BLOBS: usize = 5;

    let fixture = make_fixture(None).await?;
    assert!(
        !fixture
            .store
            .optimized_for(StoreOptimizations::SubscribesToUpdateMany),
        "must not advertise batching when disabled"
    );
    let blobs: Vec<_> = (0..NUM_BLOBS).map(make_blob).collect();

    fixture.store.update_many(make_items(&blobs)).await?;

    let batch_requests = fixture.cas_server.batch_update_requests.lock().await;
    assert_eq!(batch_requests.len(), 0, "no batched RPCs expected");
    let stream_writes = fixture.bytestream_server.write_resource_names.lock().await;
    assert_eq!(stream_writes.len(), NUM_BLOBS);
    Ok(())
}

// The batch byte budget must split very many blobs into multiple RPCs.
#[nativelink_test]
async fn batch_byte_budget_splits_requests() -> Result<(), Error> {
    // 64-byte blobs + 256-byte overhead = 320 bytes/entry. A 1600-byte
    // budget fits exactly 5 entries per request. The blob threshold must
    // stay within the budget to satisfy the construction-time invariant.
    let config = GrpcWriteBatchingConfig {
        max_blob_size_bytes: 1024,
        max_batch_bytes: 1600,
    };
    let fixture = make_fixture(Some(config)).await?;
    let blobs: Vec<_> = (0..12).map(make_blob).collect();

    fixture.store.update_many(make_items(&blobs)).await?;

    let batch_requests = fixture.cas_server.batch_update_requests.lock().await;
    // Chunks are dispatched concurrently, so assert sizes order-free.
    let mut sizes: Vec<usize> = batch_requests
        .iter()
        .map(|request| request.requests.len())
        .collect();
    sizes.sort_unstable();
    assert_eq!(sizes, vec![2, 5, 5], "expected 5+5+2 split");
    Ok(())
}

// The worker's real store shape: FastSlowStore with a batching gRPC slow
// tier must advertise SubscribesToUpdateMany, publish to both tiers, and
// send one batched RPC.
#[nativelink_test]
async fn fast_slow_chain_batches_to_slow_tier() -> Result<(), Error> {
    const NUM_BLOBS: usize = 20;

    let fixture = make_fixture(Some(batching_config())).await?;
    let fast_store = MemoryStore::new(&MemorySpec::default());
    let fast_slow_store = FastSlowStore::new(
        &FastSlowSpec {
            // The inner specs are unused by new(); the stores are passed in.
            fast: StoreSpec::Noop(NoopSpec::default()),
            fast_direction: nativelink_config::stores::StoreDirection::default(),
            slow: StoreSpec::Noop(NoopSpec::default()),
            slow_direction: nativelink_config::stores::StoreDirection::default(),
            bypass_dedup_threshold_bytes: 0,
        },
        Store::new(fast_store.clone()),
        Store::new(fixture.store.clone()),
    );
    assert!(
        fast_slow_store.optimized_for(StoreOptimizations::SubscribesToUpdateMany),
        "fast_slow must advertise batching when its slow tier batches"
    );

    let blobs: Vec<_> = (0..NUM_BLOBS).map(make_blob).collect();
    fast_slow_store.update_many(make_items(&blobs)).await?;

    for (digest, content) in &blobs {
        // Durable on the (fake) remote CAS.
        let stored = fixture
            .cas_server
            .blobs
            .lock()
            .await
            .get(&digest.packed_hash().to_string())
            .cloned();
        assert_eq!(stored.as_ref(), Some(content), "slow tier for {digest}");
        // And present in the fast tier.
        let fast_data = fast_store.get_part_unchunked(*digest, 0, None).await?;
        assert_eq!(&fast_data, content, "fast tier for {digest}");
    }
    let batch_requests = fixture.cas_server.batch_update_requests.lock().await;
    assert_eq!(batch_requests.len(), 1, "expected one batched RPC");
    Ok(())
}

// Regression: composite wrappers that forward optimized_for() must also
// forward update_many(), or the advertisement routes call sites into a
// batched path that silently dissolves into per-blob streams. The store
// factory wraps every backend in CacheMetricsStore, so this is the default
// production composition.
#[nativelink_test]
async fn cache_metrics_wrapper_preserves_batching() -> Result<(), Error> {
    const NUM_BLOBS: usize = 20;

    let fixture = make_fixture(Some(batching_config())).await?;
    let wrapped = CacheMetricsStore::new(
        &CacheMetricsSpec {
            backend: StoreSpec::Noop(NoopSpec::default()),
            cache_type: "test_cas".to_string(),
        },
        Store::new(fixture.store.clone()),
    );
    assert!(
        wrapped.optimized_for(StoreOptimizations::SubscribesToUpdateMany),
        "wrapper must forward the advertisement"
    );

    let blobs: Vec<_> = (0..NUM_BLOBS).map(make_blob).collect();
    wrapped.update_many(make_items(&blobs)).await?;

    for (digest, content) in &blobs {
        let stored = fixture
            .cas_server
            .blobs
            .lock()
            .await
            .get(&digest.packed_hash().to_string())
            .cloned();
        assert_eq!(stored.as_ref(), Some(content), "blob for {digest}");
    }
    let batch_requests = fixture.cas_server.batch_update_requests.lock().await;
    assert_eq!(
        batch_requests.len(),
        1,
        "the wrapper must dispatch one batched RPC, not per-blob streams"
    );
    Ok(())
}

// Regression: a blob at the batching threshold must fit in one batch;
// otherwise an over-budget request bypasses the per-entry streaming
// fallback. Reject the misconfiguration at startup.
#[nativelink_test]
async fn rejects_blob_threshold_larger_than_batch_budget() -> Result<(), Error> {
    let result = make_fixture(Some(GrpcWriteBatchingConfig {
        max_blob_size_bytes: 3 * 1024 * 1024,
        max_batch_bytes: 3 * 1024 * 1024,
    }))
    .await;
    let err = result.err().expect("expected construction to fail");
    assert!(
        err.to_string().contains("max_batch_bytes"),
        "unexpected error: {err}"
    );
    Ok(())
}

// A whole-RPC batch failure (e.g. an intermediary's gRPC message-size
// limit rejecting the batch) must fall back to per-blob streaming uploads
// instead of failing the call.
#[nativelink_test]
async fn whole_rpc_failure_falls_back_to_streaming() -> Result<(), Error> {
    const NUM_BLOBS: usize = 6;

    let fixture = make_fixture(Some(batching_config())).await?;
    *fixture.cas_server.fail_batch_rpc.lock().await = true;

    let blobs: Vec<_> = (0..NUM_BLOBS).map(make_blob).collect();
    fixture.store.update_many(make_items(&blobs)).await?;

    for (digest, content) in &blobs {
        let stored = fixture
            .cas_server
            .blobs
            .lock()
            .await
            .get(&digest.packed_hash().to_string())
            .cloned();
        assert_eq!(stored.as_ref(), Some(content), "blob for {digest}");
    }
    let streams = fixture.bytestream_server.write_resource_names.lock().await;
    assert_eq!(
        streams.len(),
        NUM_BLOBS,
        "every blob must arrive via the streaming fallback"
    );
    Ok(())
}
