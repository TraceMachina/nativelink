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
use nativelink_proto::build::bazel::remote::execution::v2::{
    FindMissingBlobsRequest, digest_function,
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
