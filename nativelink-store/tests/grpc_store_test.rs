use core::pin::Pin;
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;

use async_lock::Mutex;
use futures::stream::{self, unfold};
use futures::{Stream, StreamExt};
use nativelink_config::stores::{GrpcEndpoint, GrpcSpec, Retry, StoreType};
use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::content_addressable_storage_server::{
    ContentAddressableStorage, ContentAddressableStorageServer,
};
use nativelink_proto::build::bazel::remote::execution::v2::{
    BatchReadBlobsRequest, BatchReadBlobsResponse, BatchUpdateBlobsRequest,
    BatchUpdateBlobsResponse, Digest, FindMissingBlobsRequest, FindMissingBlobsResponse,
    GetTreeRequest, GetTreeResponse, SpliceBlobRequest, SpliceBlobResponse, SplitBlobRequest,
    SplitBlobResponse, batch_update_blobs_request, batch_update_blobs_response, chunking_function,
    compressor, digest_function,
};
use nativelink_proto::google::bytestream::byte_stream_server::{ByteStream, ByteStreamServer};
use nativelink_proto::google::bytestream::{
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};
use nativelink_store::grpc_store::GrpcStore;
use nativelink_util::background_spawn;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::DigestInfo;
use nativelink_util::proto_stream_utils::WriteRequestStreamWrapper;
use nativelink_util::store_trait::{StoreLike, UploadSizeInfo};
use nativelink_util::telemetry::ClientHeaders;
use opentelemetry::Context;
use regex::Regex;
use tokio::time::timeout;
use tonic::metadata::KeyAndValueRef;
use tonic::transport::Server;
use tonic::transport::server::TcpIncoming;
use tonic::{Request, Response, Status, Streaming};
use tracing::info;

const VALID_HASH: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";
const RAW_INPUT: &str = "123";

fn test_spec<T: Into<String>>(endpoint: T, use_legacy_resource_names: bool) -> GrpcSpec {
    GrpcSpec {
        instance_name: String::new(),
        endpoints: vec![GrpcEndpoint {
            address: endpoint.into(),
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
        rpc_timeout_s: 1,
        use_legacy_resource_names,
        headers: HashMap::new(),
        forward_headers: vec![],
        experimental_read_batching: None,
        experimental_remote_cache_compression: Some(false),
    }
}

#[nativelink_test]
async fn fast_find_missing_blobs() -> Result<(), Error> {
    let spec = test_spec("http://foobar", false);
    let store = GrpcStore::new(&spec)?;
    let request = Request::new(FindMissingBlobsRequest {
        instance_name: String::new(),
        blob_digests: vec![],
        digest_function: digest_function::Value::Sha256.into(),
    });
    let res = timeout(Duration::from_secs(1), async move {
        store.find_missing_blobs(request).await
    })
    .await??;
    let inner_res = res.into_inner();
    assert_eq!(inner_res.missing_blob_digests.len(), 0);
    Ok(())
}

#[derive(Debug, Clone)]
struct ReadRequestHolder {
    request: ReadRequest,
    metadata: HashMap<String, String>,
}

#[derive(Debug, Clone)]
struct FakeStreamServer {
    write_requests: Arc<Mutex<Vec<WriteRequest>>>,
    read_requests: Arc<Mutex<Vec<ReadRequestHolder>>>,
    /// Record every `WriteRequest`, not just the first, so a test can assert
    /// on how the client chunked the stream.
    drain_all: bool,
}

impl FakeStreamServer {
    fn new() -> Self {
        Self {
            write_requests: Arc::new(Mutex::new(vec![])),
            read_requests: Arc::new(Mutex::new(vec![])),
            drain_all: false,
        }
    }

    fn new_draining() -> Self {
        Self {
            drain_all: true,
            ..Self::new()
        }
    }
}

type ReadStream = Pin<Box<dyn Stream<Item = Result<ReadResponse, Status>> + Send + 'static>>;

struct ReaderState {
    responded: bool,
}

#[tonic::async_trait]
impl ByteStream for FakeStreamServer {
    type ReadStream = ReadStream;

    async fn read(
        &self,
        grpc_request: Request<ReadRequest>,
    ) -> Result<Response<Self::ReadStream>, Status> {
        let mut request_metadata: HashMap<String, String> = HashMap::new();
        for kv in grpc_request.metadata().iter() {
            match kv {
                KeyAndValueRef::Ascii(metadata_key, metadata_value) => {
                    request_metadata.insert(
                        metadata_key.to_string(),
                        metadata_value.to_str().unwrap().to_string(),
                    );
                }
                KeyAndValueRef::Binary(metadata_key, metadata_value) => {
                    request_metadata
                        .insert(metadata_key.to_string(), format!("{metadata_value:#?}"));
                }
            }
        }
        let read_request = grpc_request.into_inner();
        self.read_requests.lock().await.push(ReadRequestHolder {
            request: read_request,
            metadata: request_metadata,
        });

        let folded = unfold(ReaderState { responded: false }, async move |state| {
            if state.responded {
                return None;
            }
            let response = ReadResponse {
                data: RAW_INPUT.as_bytes().into(),
            };
            Some((Ok(response), ReaderState { responded: true }))
        });
        Ok(Response::new(Box::pin(folded)))
    }

    async fn write(
        &self,
        grpc_request: Request<Streaming<WriteRequest>>,
    ) -> Result<Response<WriteResponse>, Status> {
        let mut stream = grpc_request.into_inner();
        if self.drain_all {
            let mut committed_size = 0i64;
            while let Some(req) = stream.next().await {
                let req = req?;
                committed_size += i64::try_from(req.data.len()).unwrap();
                self.write_requests.lock().await.push(req);
            }
            return Ok(Response::new(WriteResponse { committed_size }));
        }
        let write_request = match stream.next().await {
            None => {
                return Err(Status::unknown("Client closed stream"));
            }
            Some(Err(err)) => return Err(err),
            Some(Ok(write_request)) => write_request,
        };
        info!(?write_request, "write request");
        let committed_size = write_request.data.len().try_into().unwrap_or(i64::MAX);
        self.write_requests.lock().await.push(write_request);
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

async fn make_fake_bytestream_server_draining() -> (FakeStreamServer, u16) {
    spawn_bytestream_server(FakeStreamServer::new_draining()).await
}

async fn make_fake_bytestream_server() -> (FakeStreamServer, u16) {
    spawn_bytestream_server(FakeStreamServer::new()).await
}

async fn spawn_bytestream_server(fake_stream_server: FakeStreamServer) -> (FakeStreamServer, u16) {
    let server = ByteStreamServer::new(fake_stream_server.clone());
    let listener = TcpIncoming::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = listener.local_addr().unwrap().port();

    background_spawn!("server", async move {
        Server::builder()
            .add_service(server)
            .serve_with_incoming(listener)
            .await
            .unwrap();
    });

    (fake_stream_server, port)
}

async fn write_update_works_core(
    use_legacy_resource_names: bool,
    upload_pattern: Regex,
) -> Result<(), Error> {
    let (server, port) = make_fake_bytestream_server().await;
    let spec = test_spec(
        format!("http://localhost:{port}"),
        use_legacy_resource_names,
    );
    let store = GrpcStore::new(&spec)?;
    let digest = DigestInfo::try_new(VALID_HASH, RAW_INPUT.len()).unwrap();

    let (mut tx, rx) = make_buf_channel_pair();
    let send_fut = async move {
        tx.send(RAW_INPUT.into()).await?;
        tx.send_eof()
    };
    let (res1, res2) = futures::join!(
        send_fut,
        store.update(
            digest,
            rx,
            UploadSizeInfo::ExactSize(RAW_INPUT.len().try_into().unwrap())
        )
    );
    res1.merge(res2)?;

    let write_requests = server.write_requests.lock().await;
    assert_eq!(write_requests.len(), 1);
    let write_request = write_requests.first().unwrap();
    assert!(
        upload_pattern.is_match(&write_request.resource_name),
        "resource name: {}",
        write_request.resource_name
    );
    assert_eq!(write_request.data, RAW_INPUT.as_bytes());
    Ok(())
}

#[nativelink_test]
async fn write_update_works() -> Result<(), Error> {
    let upload_pattern = Regex::new("/uploads/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/blobs/sha256/0123456789abcdef000000000000000000010000000000000123456789abcdef/3").unwrap();
    write_update_works_core(false, upload_pattern).await
}

#[nativelink_test]
async fn write_update_works_with_legacy_resource_names() -> Result<(), Error> {
    let upload_pattern = Regex::new("/uploads/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}/blobs/0123456789abcdef000000000000000000010000000000000123456789abcdef/3").unwrap();
    write_update_works_core(true, upload_pattern).await
}

async fn read_works_core<F>(
    use_legacy_resource_names: bool,
    upload_pattern: &str,
    edit_spec: F,
) -> Result<ReadRequestHolder, Error>
where
    F: FnOnce(GrpcSpec) -> GrpcSpec,
{
    let (server, port) = make_fake_bytestream_server().await;
    let spec = edit_spec(test_spec(
        format!("http://localhost:{port}"),
        use_legacy_resource_names,
    ));
    let store = GrpcStore::new(&spec)?;
    let digest = DigestInfo::try_new(VALID_HASH, RAW_INPUT.len()).unwrap();

    let (tx, mut rx) = make_buf_channel_pair();
    store.get_part(digest, tx, 0, None).await.unwrap();
    let bytes = rx.recv().await?;
    assert_eq!(bytes, RAW_INPUT.as_bytes());

    let read_requests = server.read_requests.lock().await;
    assert_eq!(read_requests.len(), 1);
    let read_request = read_requests.first().unwrap();
    assert_eq!(upload_pattern, &read_request.request.resource_name);

    Ok(read_request.clone())
}

#[nativelink_test]
async fn read_works() -> Result<(), Error> {
    let upload_pattern =
        "/blobs/sha256/0123456789abcdef000000000000000000010000000000000123456789abcdef/3";
    read_works_core(false, upload_pattern, core::convert::identity)
        .await
        .unwrap();
    Ok(())
}

#[nativelink_test]
async fn read_works_with_legacy_resource_names() -> Result<(), Error> {
    let upload_pattern =
        "/blobs/0123456789abcdef000000000000000000010000000000000123456789abcdef/3";
    read_works_core(true, upload_pattern, core::convert::identity)
        .await
        .unwrap();
    Ok(())
}

#[nativelink_test]
async fn read_works_with_headers() -> Result<(), Error> {
    fn set_spec(mut spec: GrpcSpec) -> GrpcSpec {
        spec.headers.insert("foo".into(), "bar".into());
        // Testing with mixed case, as it gets lowercased internally
        spec.forward_headers.push("SomeTHING".into());
        spec
    }

    let upload_pattern =
        "/blobs/sha256/0123456789abcdef000000000000000000010000000000000123456789abcdef/3";

    let client_headers = {
        let mut headers: HashMap<String, String> = HashMap::new();
        // We're inserting a lowercase one here as the telemetry insertion uses a lowercase one
        headers.insert("something".to_string(), "From outside".to_string());
        ClientHeaders(Arc::new(headers))
    };

    let cx_guard = Context::map_current(|cx| cx.with_value(client_headers)).attach();

    let read_request = read_works_core(false, upload_pattern, set_spec)
        .await
        .unwrap();
    assert_eq!(read_request.metadata.get("foo"), Some(&"bar".to_string()));
    assert_eq!(
        read_request.metadata.get("something"),
        Some(&"From outside".to_string()),
        "{:#?}",
        read_request.metadata
    );
    drop(cx_guard);

    Ok(())
}

#[derive(Debug, Clone)]
struct FakeCasServer {
    split_requests: Arc<Mutex<Vec<SplitBlobRequest>>>,
    splice_requests: Arc<Mutex<Vec<SpliceBlobRequest>>>,
    /// Every `BatchUpdateBlobs` RPC received, so a test can assert on how the
    /// store split an oversized batch.
    batch_updates: Arc<Mutex<Vec<BatchUpdateBlobsRequest>>>,
}

impl FakeCasServer {
    fn new() -> Self {
        Self {
            split_requests: Arc::new(Mutex::new(vec![])),
            splice_requests: Arc::new(Mutex::new(vec![])),
            batch_updates: Arc::new(Mutex::new(vec![])),
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
        // One response per entry, echoing the digest, so a test can check the
        // stitched-together result keeps request order.
        let responses = request
            .requests
            .iter()
            .map(|r| batch_update_blobs_response::Response {
                digest: r.digest.clone(),
                status: None,
            })
            .collect();
        self.batch_updates.lock().await.push(request);
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

    async fn split_blob(
        &self,
        grpc_request: Request<SplitBlobRequest>,
    ) -> Result<Response<SplitBlobResponse>, Status> {
        let request = grpc_request.into_inner();
        self.split_requests.lock().await.push(request.clone());
        Ok(Response::new(SplitBlobResponse {
            chunk_digests: request.blob_digest.into_iter().collect(),
            chunking_function: request.chunking_function,
        }))
    }

    async fn splice_blob(
        &self,
        grpc_request: Request<SpliceBlobRequest>,
    ) -> Result<Response<SpliceBlobResponse>, Status> {
        let request = grpc_request.into_inner();
        self.splice_requests.lock().await.push(request.clone());
        Ok(Response::new(SpliceBlobResponse {
            blob_digest: request.blob_digest,
        }))
    }
}

async fn make_fake_cas_server() -> (FakeCasServer, u16) {
    let fake_cas_server = FakeCasServer::new();
    let server = ContentAddressableStorageServer::new(fake_cas_server.clone());
    let listener = TcpIncoming::bind("127.0.0.1:0".parse().unwrap()).unwrap();
    let port = listener.local_addr().unwrap().port();

    background_spawn!("server", async move {
        Server::builder()
            .add_service(server)
            .serve_with_incoming(listener)
            .await
            .unwrap();
    });

    (fake_cas_server, port)
}

#[nativelink_test]
async fn split_and_splice_blob_forward_to_backend() -> Result<(), Error> {
    let (server, port) = make_fake_cas_server().await;
    let mut spec = test_spec(format!("http://localhost:{port}"), false);
    spec.instance_name = "backend_instance".to_string();
    let store = GrpcStore::new(&spec)?;

    let digest = Digest {
        hash: VALID_HASH.to_string(),
        size_bytes: RAW_INPUT.len().try_into().unwrap_or(i64::MAX),
    };

    let split_response = store
        .split_blob(Request::new(SplitBlobRequest {
            instance_name: "local_instance".to_string(),
            blob_digest: Some(digest.clone()),
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await?
        .into_inner();
    assert_eq!(split_response.chunk_digests, vec![digest.clone()]);
    {
        let split_requests = server.split_requests.lock().await;
        assert_eq!(split_requests.len(), 1);
        // The instance name must be rewritten to the backend's.
        assert_eq!(split_requests[0].instance_name, "backend_instance");
    }

    let splice_response = store
        .splice_blob(Request::new(SpliceBlobRequest {
            instance_name: "local_instance".to_string(),
            blob_digest: Some(digest.clone()),
            chunk_digests: vec![digest.clone()],
            digest_function: digest_function::Value::Sha256.into(),
            chunking_function: chunking_function::Value::FastCdc2020.into(),
        }))
        .await?
        .into_inner();
    assert_eq!(splice_response.blob_digest, Some(digest));
    {
        let splice_requests = server.splice_requests.lock().await;
        assert_eq!(splice_requests.len(), 1);
        assert_eq!(splice_requests[0].instance_name, "backend_instance");
    }
    Ok(())
}

/// A whole-blob write (`update_oneshot`, how a worker uploads stdout) must not
/// become one giant `WriteRequest`: receivers cap decoded message size and
/// reject an oversized message outright rather than degrading.
#[nativelink_test]
async fn update_splits_buffers_larger_than_the_grpc_message_limit() -> Result<(), Error> {
    const TONIC_DEFAULT_DECODE_LIMIT: usize = 4 * 1024 * 1024;
    // Over the limit, and not a multiple of the chunk size so the last piece
    // is a partial one.
    const BLOB_LEN: usize = 9_869_079;

    let (server, port) = make_fake_bytestream_server_draining().await;
    let mut spec = test_spec(format!("http://localhost:{port}"), false);
    // This test moves and reassembles almost 10 MiB under instrumented builds.
    // Keep the RPC deadline comfortably above sanitizer overhead; timeout
    // behavior is not what this test exercises.
    spec.rpc_timeout_s = 5;
    let store = GrpcStore::new(&spec)?;
    let digest = DigestInfo::try_new(VALID_HASH, BLOB_LEN).unwrap();

    let payload = vec![0xABu8; BLOB_LEN];
    let (mut tx, rx) = make_buf_channel_pair();
    let send_payload = payload.clone();
    let send_fut = async move {
        // One write of the entire blob, exactly what update_oneshot does.
        tx.send(send_payload.into()).await?;
        tx.send_eof()
    };
    let (res1, res2) = futures::join!(
        send_fut,
        store.update(
            digest,
            rx,
            UploadSizeInfo::ExactSize(BLOB_LEN.try_into().unwrap())
        )
    );
    res1.merge(res2)?;

    let write_requests = server.write_requests.lock().await;
    assert!(
        write_requests.len() > 1,
        "expected the blob to be split across several WriteRequests, got {}",
        write_requests.len()
    );

    for (i, req) in write_requests.iter().enumerate() {
        assert!(
            req.data.len() <= TONIC_DEFAULT_DECODE_LIMIT,
            "WriteRequest {i} carries {} bytes, over the {TONIC_DEFAULT_DECODE_LIMIT} byte default decode limit",
            req.data.len()
        );
    }

    // Contiguous offsets and identical bytes: the split must not corrupt or
    // reorder the blob.
    let mut reassembled = Vec::with_capacity(BLOB_LEN);
    let mut expected_offset = 0i64;
    for req in write_requests.iter() {
        assert_eq!(
            req.write_offset, expected_offset,
            "write_offset must be contiguous across chunks"
        );
        expected_offset += i64::try_from(req.data.len()).unwrap();
        reassembled.extend_from_slice(&req.data);
    }
    assert_eq!(reassembled.len(), BLOB_LEN);
    assert_eq!(
        reassembled, payload,
        "reassembled blob must match the input"
    );

    // Exactly one terminator, and it must be the last message.
    let finish_flags: Vec<bool> = write_requests.iter().map(|r| r.finish_write).collect();
    assert_eq!(
        finish_flags.iter().filter(|f| **f).count(),
        1,
        "expected exactly one finish_write, got {finish_flags:?}"
    );
    assert!(
        *finish_flags.last().unwrap(),
        "finish_write must be on the final message"
    );
    Ok(())
}

/// The `ByteStream` pass-through forwards the caller's frames as it receives
/// them, so a caller that hands over a whole blob in one frame would produce
/// one oversized message and be rejected at the receiver's 4MiB default.
#[nativelink_test]
async fn write_splits_frames_larger_than_the_grpc_message_limit() -> Result<(), Error> {
    const TONIC_DEFAULT_DECODE_LIMIT: usize = 4 * 1024 * 1024;
    // Over the limit and not a multiple of the chunk size, so the tail is a
    // partial piece. Kept just past the limit rather than at the ~9.4MB seen
    // in the wild: the extra bytes prove nothing here and cost real time under
    // sanitizers.
    const BLOB_LEN: usize = 5_242_883;

    let (server, port) = make_fake_bytestream_server_draining().await;
    let mut spec = test_spec(format!("http://localhost:{port}"), false);
    // Streaming several MiB through ByteStream is slow under instrumented
    // builds. This deadline is headroom for that, not part of the test.
    spec.rpc_timeout_s = 30;
    let store = GrpcStore::new(&spec)?;

    let payload = vec![0xCDu8; BLOB_LEN];
    let resource_name = format!(
        "instance_name/uploads/{}/blobs/{VALID_HASH}/{BLOB_LEN}",
        uuid::Uuid::new_v4()
    );
    // A single frame carrying the entire blob, which is what a client doing
    // one big write looks like on the wire.
    let requests = vec![WriteRequest {
        resource_name,
        write_offset: 0,
        finish_write: true,
        data: payload.clone().into(),
    }];
    let stream =
        WriteRequestStreamWrapper::from(stream::iter(requests.into_iter().map(Ok::<_, Error>)))
            .await?;

    store.write(stream).await?;

    let write_requests = server.write_requests.lock().await;
    assert!(
        write_requests.len() > 1,
        "expected the frame to be split, got {}",
        write_requests.len()
    );

    for (i, req) in write_requests.iter().enumerate() {
        assert!(
            req.data.len() <= TONIC_DEFAULT_DECODE_LIMIT,
            "WriteRequest {i} carries {} bytes, over the {TONIC_DEFAULT_DECODE_LIMIT} byte default decode limit",
            req.data.len()
        );
    }

    let mut reassembled = Vec::with_capacity(BLOB_LEN);
    let mut expected_offset = 0i64;
    for req in write_requests.iter() {
        assert_eq!(
            req.write_offset, expected_offset,
            "write_offset must be contiguous across chunks"
        );
        expected_offset += i64::try_from(req.data.len()).unwrap();
        reassembled.extend_from_slice(&req.data);
    }
    assert_eq!(reassembled, payload, "the split must not corrupt the blob");

    let finish_flags: Vec<bool> = write_requests.iter().map(|r| r.finish_write).collect();
    assert_eq!(
        finish_flags.iter().filter(|f| **f).count(),
        1,
        "expected exactly one finish_write, got {finish_flags:?}"
    );
    assert!(
        *finish_flags.last().unwrap(),
        "finish_write must be on the final chunk"
    );
    Ok(())
}

/// A batch is a single message, so an oversized one has to become several
/// RPCs rather than several chunks of one RPC.
#[nativelink_test]
async fn batch_update_blobs_splits_an_oversized_batch_across_rpcs() -> Result<(), Error> {
    const TONIC_DEFAULT_DECODE_LIMIT: usize = 4 * 1024 * 1024;
    const ENTRY_LEN: usize = 900 * 1024;
    const ENTRIES: usize = 12; // ~10.5MiB total, comfortably over the limit.

    let (server, port) = make_fake_cas_server().await;
    let mut spec = test_spec(format!("http://localhost:{port}"), false);
    spec.instance_name = "backend_instance".to_string();
    spec.rpc_timeout_s = 5;
    let store = GrpcStore::new(&spec)?;

    let requests: Vec<batch_update_blobs_request::Request> = (0..ENTRIES)
        .map(|i| batch_update_blobs_request::Request {
            digest: Some(Digest {
                hash: format!("{i:064x}"),
                size_bytes: i64::try_from(ENTRY_LEN).unwrap(),
            }),
            data: vec![u8::try_from(i).unwrap_or(0); ENTRY_LEN].into(),
            compressor: compressor::Value::Identity.into(),
        })
        .collect();
    let expected_digests: Vec<Option<Digest>> = requests.iter().map(|r| r.digest.clone()).collect();

    let response = store
        .batch_update_blobs(Request::new(BatchUpdateBlobsRequest {
            instance_name: "local_instance".to_string(),
            requests,
            digest_function: digest_function::Value::Sha256.into(),
        }))
        .await?;

    let batches = server.batch_updates.lock().await;
    assert!(
        batches.len() > 1,
        "expected the batch to be split across several RPCs, got {}",
        batches.len()
    );

    for (i, batch) in batches.iter().enumerate() {
        let bytes: usize = batch.requests.iter().map(|r| r.data.len()).sum();
        assert!(
            bytes <= TONIC_DEFAULT_DECODE_LIMIT,
            "batch {i} carries {bytes} bytes, over the {TONIC_DEFAULT_DECODE_LIMIT} byte default decode limit",
        );
        assert_eq!(
            batch.instance_name, "backend_instance",
            "every split batch keeps the rewritten instance name"
        );
    }

    // Nothing dropped or reordered by the split.
    let sent: Vec<Option<Digest>> = batches
        .iter()
        .flat_map(|b| b.requests.iter().map(|r| r.digest.clone()))
        .collect();
    assert_eq!(sent, expected_digests, "every blob must be forwarded once");

    let got: Vec<Option<Digest>> = response
        .into_inner()
        .responses
        .into_iter()
        .map(|r| r.digest)
        .collect();
    assert_eq!(
        got, expected_digests,
        "stitched responses must stay in request order"
    );
    Ok(())
}
