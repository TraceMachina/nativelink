use core::pin::Pin;
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;

use async_lock::Mutex;
use futures::stream::unfold;
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
    SplitBlobResponse, chunking_function, digest_function,
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
        experimental_remote_cache_compression: false,
    }
}

#[nativelink_test]
async fn fast_find_missing_blobs() -> Result<(), Error> {
    let spec = test_spec("http://foobar", false);
    let store = GrpcStore::new(&spec).await?;
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
}

impl FakeStreamServer {
    fn new() -> Self {
        Self {
            write_requests: Arc::new(Mutex::new(vec![])),
            read_requests: Arc::new(Mutex::new(vec![])),
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
        let write_request = match grpc_request.into_inner().next().await {
            None => {
                return Err(Status::unknown("Client closed stream"));
            }
            Some(Err(err)) => return Err(err),
            Some(Ok(write_request)) => write_request,
        };
        info!(?write_request, "write request");
        let committed_size = write_request.data.len() as i64;
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

async fn make_fake_bytestream_server() -> (FakeStreamServer, u16) {
    let fake_stream_server = FakeStreamServer::new();
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
    let store = GrpcStore::new(&spec).await?;
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
    let store = GrpcStore::new(&spec).await?;
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
}

impl FakeCasServer {
    fn new() -> Self {
        Self {
            split_requests: Arc::new(Mutex::new(vec![])),
            splice_requests: Arc::new(Mutex::new(vec![])),
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
    let store = GrpcStore::new(&spec).await?;

    let digest = Digest {
        hash: VALID_HASH.to_string(),
        size_bytes: RAW_INPUT.len() as i64,
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
