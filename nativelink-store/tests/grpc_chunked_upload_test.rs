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
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use futures::Stream;
use nativelink_config::stores::{
    GrpcChunkedUploadsConfig, GrpcEndpoint, GrpcSpec, Retry, StoreType,
};
use nativelink_error::{Code, Error, make_err};
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
use nativelink_store::grpc_store::GrpcStore;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use nativelink_util::store_trait::{StoreLike, UploadSizeInfo};
use pretty_assertions::assert_eq;
use tokio::sync::Mutex;
use tokio_stream::StreamExt;
use tonic::{Request, Response, Status, Streaming};

/// Minimal in-memory CAS implementing exactly what chunked uploads use:
/// `FindMissingBlobs`, `BatchUpdateBlobs`, `SpliceBlob` — plus a `ByteStream` `Write`
/// sink so the plain path is observable.
#[derive(Debug, Default)]
struct FakeCas {
    blobs: Mutex<HashMap<String, Bytes>>,
    batch_update_payload_bytes: AtomicU64,
    bytestream_writes: AtomicU64,
    splice_requests: AtomicU64,
    fail_splice_not_found: AtomicBool,
    duplicate_missing_digests: AtomicBool,
}

/// Local handle type so tonic service traits can be implemented (coherence
/// forbids implementing them for `Arc<FakeCas>` directly).
#[derive(Debug, Clone)]
struct FakeCasHandle(Arc<FakeCas>);

impl core::ops::Deref for FakeCasHandle {
    type Target = FakeCas;
    fn deref(&self) -> &FakeCas {
        &self.0
    }
}

fn digest_key(digest: &nativelink_proto::build::bazel::remote::execution::v2::Digest) -> String {
    format!("{}-{}", digest.hash, digest.size_bytes)
}

#[tonic::async_trait]
impl ContentAddressableStorage for FakeCasHandle {
    async fn find_missing_blobs(
        &self,
        request: Request<FindMissingBlobsRequest>,
    ) -> Result<Response<FindMissingBlobsResponse>, Status> {
        let request = request.into_inner();
        let blobs = self.blobs.lock().await;
        let mut missing_blob_digests: Vec<_> = request
            .blob_digests
            .into_iter()
            .filter(|digest| !blobs.contains_key(&digest_key(digest)))
            .collect();
        // REAPI does not forbid repeated entries; some backends emit them.
        if self.duplicate_missing_digests.load(Ordering::Relaxed) {
            let duplicates = missing_blob_digests.clone();
            missing_blob_digests.extend(duplicates);
        }
        Ok(Response::new(FindMissingBlobsResponse {
            missing_blob_digests,
        }))
    }

    async fn batch_update_blobs(
        &self,
        request: Request<BatchUpdateBlobsRequest>,
    ) -> Result<Response<BatchUpdateBlobsResponse>, Status> {
        let request = request.into_inner();
        let mut blobs = self.blobs.lock().await;
        let mut responses = Vec::with_capacity(request.requests.len());
        for entry in request.requests {
            let digest = entry.digest.clone().expect("digest must be set");
            self.batch_update_payload_bytes
                .fetch_add(entry.data.len() as u64, Ordering::Relaxed);
            blobs.insert(digest_key(&digest), entry.data);
            responses.push(batch_update_blobs_response::Response {
                digest: Some(digest),
                status: Some(nativelink_proto::google::rpc::Status::default()),
            });
        }
        Ok(Response::new(BatchUpdateBlobsResponse { responses }))
    }

    async fn splice_blob(
        &self,
        request: Request<SpliceBlobRequest>,
    ) -> Result<Response<SpliceBlobResponse>, Status> {
        self.splice_requests.fetch_add(1, Ordering::Relaxed);
        if self.fail_splice_not_found.load(Ordering::Relaxed) {
            return Err(Status::not_found("chunk evicted (injected)"));
        }
        let request = request.into_inner();
        let blob_digest = request.blob_digest.expect("blob_digest must be set");
        let mut blobs = self.blobs.lock().await;
        let mut assembled = Vec::with_capacity(usize::try_from(blob_digest.size_bytes).unwrap());
        for chunk_digest in &request.chunk_digests {
            let chunk = blobs
                .get(&digest_key(chunk_digest))
                .ok_or_else(|| Status::not_found(format!("chunk {chunk_digest:?} missing")))?;
            assembled.extend_from_slice(chunk);
        }
        if assembled.len() as i64 != blob_digest.size_bytes {
            return Err(Status::invalid_argument("assembled size mismatch"));
        }
        blobs.insert(digest_key(&blob_digest), assembled.into());
        Ok(Response::new(SpliceBlobResponse {
            blob_digest: Some(blob_digest),
        }))
    }

    async fn batch_read_blobs(
        &self,
        _request: Request<BatchReadBlobsRequest>,
    ) -> Result<Response<BatchReadBlobsResponse>, Status> {
        Err(Status::unimplemented("not needed"))
    }

    type GetTreeStream =
        Pin<Box<dyn Stream<Item = Result<GetTreeResponse, Status>> + Send + 'static>>;
    async fn get_tree(
        &self,
        _request: Request<GetTreeRequest>,
    ) -> Result<Response<Self::GetTreeStream>, Status> {
        Err(Status::unimplemented("not needed"))
    }

    async fn split_blob(
        &self,
        _request: Request<SplitBlobRequest>,
    ) -> Result<Response<SplitBlobResponse>, Status> {
        Err(Status::unimplemented("not needed"))
    }
}

#[tonic::async_trait]
impl ByteStream for FakeCasHandle {
    type ReadStream = Pin<Box<dyn Stream<Item = Result<ReadResponse, Status>> + Send + 'static>>;
    async fn read(
        &self,
        _request: Request<ReadRequest>,
    ) -> Result<Response<Self::ReadStream>, Status> {
        Err(Status::unimplemented("not needed"))
    }

    async fn write(
        &self,
        request: Request<Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        self.bytestream_writes.fetch_add(1, Ordering::Relaxed);
        let mut stream = request.into_inner();
        let mut first_resource_name = None;
        let mut data = Vec::new();
        while let Some(message) = stream.next().await {
            let message = message?;
            if first_resource_name.is_none() {
                first_resource_name = Some(message.resource_name.clone());
            }
            data.extend_from_slice(&message.data);
            if message.finish_write {
                break;
            }
        }
        let resource_name = first_resource_name.unwrap_or_default();
        // uploads/{uuid}/blobs/{hash}/{size}
        let mut parts = resource_name.split('/').rev();
        let size: i64 = parts.next().unwrap().parse().unwrap();
        let hash = parts.next().unwrap().to_string();
        self.blobs
            .lock()
            .await
            .insert(format!("{hash}-{size}"), Bytes::from(data.clone()));
        Ok(Response::new(WriteResponse {
            committed_size: data.len() as i64,
        }))
    }

    async fn query_write_status(
        &self,
        _request: Request<QueryWriteStatusRequest>,
    ) -> Result<Response<QueryWriteStatusResponse>, Status> {
        Err(Status::unimplemented("not needed"))
    }
}

async fn start_fake_cas(cas: Arc<FakeCas>) -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    nativelink_util::background_spawn!("fake_cas_server", async move {
        tonic::transport::Server::builder()
            .add_service(ContentAddressableStorageServer::new(FakeCasHandle(
                cas.clone(),
            )))
            .add_service(ByteStreamServer::new(FakeCasHandle(cas)))
            .serve_with_incoming(incoming)
            .await
            .unwrap();
    });
    port
}

fn chunked_spec(port: u16, config: GrpcChunkedUploadsConfig) -> GrpcSpec {
    GrpcSpec {
        instance_name: String::new(),
        endpoints: vec![GrpcEndpoint {
            address: format!("http://127.0.0.1:{port}"),
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
        rpc_timeout_s: 60,
        use_legacy_resource_names: false,
        headers: HashMap::new(),
        forward_headers: vec![],
        experimental_read_batching: None,
        experimental_chunked_uploads: Some(config),
    }
}

const fn test_config() -> GrpcChunkedUploadsConfig {
    GrpcChunkedUploadsConfig {
        min_blob_size_bytes: 1024 * 1024,
        avg_chunk_size_bytes: 16 * 1024,
        max_chunk_count: 50_000,
    }
}

fn make_payload(len: usize, seed: u64) -> Vec<u8> {
    let mut data = vec![0u8; len];
    let mut state = seed;
    for word in data.chunks_mut(8) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        word.copy_from_slice(&state.to_le_bytes()[..word.len()]);
    }
    data
}

fn digest_of(data: &[u8]) -> DigestInfo {
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(data);
    hasher.finalize_digest()
}

async fn stream_upload(store: &Arc<GrpcStore>, data: &[u8]) -> Result<DigestInfo, Error> {
    let digest = digest_of(data);
    let content = Bytes::copy_from_slice(data);
    let (mut tx, rx) = make_buf_channel_pair();
    let blob_len = content.len();
    let send_fut = async move {
        let mut offset = 0usize;
        while offset < blob_len {
            let end = (offset + 64 * 1024).min(blob_len);
            tx.send(content.slice(offset..end)).await?;
            offset = end;
        }
        tx.send_eof()
    };
    let update_fut = store.update(digest, rx, UploadSizeInfo::ExactSize(blob_len as u64));
    let (send_result, update_result) = tokio::join!(send_fut, update_fut);
    send_result?;
    update_result?;
    Ok(digest)
}

async fn stored_blob(cas: &FakeCas, digest: DigestInfo) -> Option<Bytes> {
    cas.blobs
        .lock()
        .await
        .get(&format!("{}-{}", digest.packed_hash(), digest.size_bytes()))
        .cloned()
}

// A blob below min_blob_size_bytes must use the plain ByteStream path.
#[nativelink_test]
async fn small_blob_uses_plain_path() -> Result<(), Error> {
    let cas = Arc::new(FakeCas::default());
    let port = start_fake_cas(cas.clone()).await;
    let store = GrpcStore::new(&chunked_spec(port, test_config())).await?;

    let data = make_payload(256 * 1024, 1);
    let digest = stream_upload(&store, &data).await?;

    assert_eq!(cas.bytestream_writes.load(Ordering::Relaxed), 1);
    assert_eq!(cas.splice_requests.load(Ordering::Relaxed), 0);
    assert_eq!(stored_blob(&cas, digest).await.as_deref(), Some(&data[..]));
    Ok(())
}

// A large blob must be uploaded as chunks and assembled with SpliceBlob,
// byte-identical.
#[nativelink_test]
async fn large_blob_chunk_uploads_and_splices() -> Result<(), Error> {
    let cas = Arc::new(FakeCas::default());
    let port = start_fake_cas(cas.clone()).await;
    let store = GrpcStore::new(&chunked_spec(port, test_config())).await?;

    let data = make_payload(4 * 1024 * 1024, 2);
    let digest = stream_upload(&store, &data).await?;

    assert_eq!(cas.bytestream_writes.load(Ordering::Relaxed), 0);
    assert_eq!(cas.splice_requests.load(Ordering::Relaxed), 1);
    assert_eq!(stored_blob(&cas, digest).await.as_deref(), Some(&data[..]));
    Ok(())
}

// Re-uploading a churned version must transfer only the changed chunks.
#[nativelink_test]
async fn churned_blob_transfers_only_missing_chunks() -> Result<(), Error> {
    let cas = Arc::new(FakeCas::default());
    let port = start_fake_cas(cas.clone()).await;
    let store = GrpcStore::new(&chunked_spec(port, test_config())).await?;

    let v1 = make_payload(4 * 1024 * 1024, 3);
    stream_upload(&store, &v1).await?;
    let v1_payload_bytes = cas.batch_update_payload_bytes.load(Ordering::Relaxed);

    // v2: one contiguous 128KiB region rewritten.
    let mut v2 = v1.clone();
    let mutated = make_payload(128 * 1024, 4);
    v2[1024 * 1024..1024 * 1024 + 128 * 1024].copy_from_slice(&mutated);
    let v2_digest = stream_upload(&store, &v2).await?;

    let v2_payload_bytes =
        cas.batch_update_payload_bytes.load(Ordering::Relaxed) - v1_payload_bytes;
    assert!(
        v2_payload_bytes < v2.len() as u64 / 4,
        "churned upload transferred {v2_payload_bytes} bytes, expected far less than {}",
        v2.len()
    );
    assert_eq!(stored_blob(&cas, v2_digest).await.as_deref(), Some(&v2[..]));
    Ok(())
}

// A SpliceBlob NotFound (chunk evicted mid-upload) must surface as a
// retryable ABORTED error.
#[nativelink_test]
async fn splice_not_found_maps_to_aborted() -> Result<(), Error> {
    let cas = Arc::new(FakeCas::default());
    let port = start_fake_cas(cas.clone()).await;
    let store = GrpcStore::new(&chunked_spec(port, test_config())).await?;

    cas.fail_splice_not_found.store(true, Ordering::Relaxed);
    let data = make_payload(2 * 1024 * 1024, 5);
    let err = stream_upload(&store, &data)
        .await
        .expect_err("expected splice failure");
    assert_eq!(err.code, Code::Aborted, "unexpected error: {err:?}");
    Ok(())
}

// Blobs that could exceed max_chunk_count must use the plain path (a chunked
// upload cannot fall back once the stream is consumed).
#[nativelink_test]
async fn oversized_chunk_count_uses_plain_path() -> Result<(), Error> {
    let cas = Arc::new(FakeCas::default());
    let port = start_fake_cas(cas.clone()).await;
    let mut config = test_config();
    config.max_chunk_count = 8;
    let store = GrpcStore::new(&chunked_spec(port, config)).await?;

    let data = make_payload(4 * 1024 * 1024, 6);
    let digest = stream_upload(&store, &data).await?;

    assert_eq!(cas.bytestream_writes.load(Ordering::Relaxed), 1);
    assert_eq!(cas.splice_requests.load(Ordering::Relaxed), 0);
    assert_eq!(stored_blob(&cas, digest).await.as_deref(), Some(&data[..]));
    Ok(())
}

// Invalid configurations must be rejected at construction.
#[nativelink_test]
async fn invalid_configs_rejected() -> Result<(), Error> {
    let make = |config| chunked_spec(1, config);

    let too_big_avg = GrpcStore::new(&make(GrpcChunkedUploadsConfig {
        min_blob_size_bytes: 8 * 1024 * 1024,
        avg_chunk_size_bytes: 1024 * 1024,
        max_chunk_count: 50_000,
    }))
    .await;
    assert!(too_big_avg.is_err(), "avg over v1 cap must be rejected");

    let min_below_max_chunk = GrpcStore::new(&make(GrpcChunkedUploadsConfig {
        min_blob_size_bytes: 64 * 1024,
        avg_chunk_size_bytes: 512 * 1024,
        max_chunk_count: 50_000,
    }))
    .await;
    assert!(
        min_below_max_chunk.is_err(),
        "min_blob_size below avg*4 must be rejected"
    );

    let zero_chunk_count = GrpcStore::new(&make(GrpcChunkedUploadsConfig {
        min_blob_size_bytes: 8 * 1024 * 1024,
        avg_chunk_size_bytes: 512 * 1024,
        max_chunk_count: 0,
    }))
    .await;
    assert!(zero_chunk_count.is_err(), "zero max_chunk_count rejected");
    drop(make_err!(Code::Ok, "unused")); // keep make_err import used
    Ok(())
}

// A backend that repeats digests in missing_blob_digests must not fail the
// upload: repeats are tolerated, unknown digests still error.
#[nativelink_test]
async fn duplicate_missing_digests_are_tolerated() -> Result<(), Error> {
    let cas = Arc::new(FakeCas::default());
    cas.duplicate_missing_digests.store(true, Ordering::Relaxed);
    let port = start_fake_cas(cas.clone()).await;
    let store = GrpcStore::new(&chunked_spec(port, test_config())).await?;

    let data = make_payload(12 * 1024 * 1024, 7);
    let digest = stream_upload(&store, &data).await?;

    assert_eq!(cas.splice_requests.load(Ordering::Relaxed), 1);
    assert_eq!(stored_blob(&cas, digest).await.as_deref(), Some(&data[..]));
    Ok(())
}
