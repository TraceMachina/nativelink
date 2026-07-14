// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use std::sync::Arc;

use bytes::Bytes;
use futures::task::Poll;
use futures::{Future, poll};
use http_body_util::BodyExt;
use hyper::Uri;
use hyper::body::Frame;
use hyper_util::rt::TokioIo;
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use nativelink_config::cas_server::{ByteStreamConfig, HttpListener, WithInstanceName};
use nativelink_config::stores::{MemorySpec, StoreSpec};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_proto::google::bytestream::byte_stream_client::ByteStreamClient;
use nativelink_proto::google::bytestream::byte_stream_server::ByteStream;
use nativelink_proto::google::bytestream::{
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, WriteRequest, WriteResponse,
};
use nativelink_service::bytestream_server::ByteStreamServer;
use nativelink_service::wire_compression::RemoteCacheCompressionInstances;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::channel_body_for_tests::ChannelBody;
use nativelink_util::common::{DigestInfo, encode_stream_proto};
use nativelink_util::store_trait::StoreLike;
use nativelink_util::task::JoinHandleDropGuard;
use nativelink_util::{background_spawn, spawn};
use pretty_assertions::assert_eq;
use sha2::{Digest as ShaDigest, Sha256};
use tokio::io::DuplexStream;
use tokio::sync::mpsc;
use tokio::sync::mpsc::unbounded_channel;
use tokio::task::yield_now;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::codec::{Codec, CompressionEncoding};
use tonic::transport::{Channel, Endpoint};
use tonic::{Request, Response, Streaming};
use tonic_prost::ProstCodec;
use tower::service_fn;

const INSTANCE_NAME: &str = "foo_instance_name";
const HASH1: &str = "0123456789abcdef000000000000000000000000000000000123456789abcdef";

async fn make_store_manager() -> Result<Arc<StoreManager>, Error> {
    let store_manager = Arc::new(StoreManager::new());
    store_manager.add_store(
        "main_cas",
        store_factory(
            &StoreSpec::Memory(MemorySpec::default()),
            &store_manager,
            None,
        )
        .await?,
    )?;
    Ok(store_manager)
}

fn make_bytestream_server(
    store_manager: &StoreManager,
    config: Option<Vec<WithInstanceName<ByteStreamConfig>>>,
) -> Result<ByteStreamServer, Error> {
    make_bytestream_server_with_remote_cache_compression(store_manager, config, false)
}

fn make_bytestream_server_with_remote_cache_compression(
    store_manager: &StoreManager,
    config: Option<Vec<WithInstanceName<ByteStreamConfig>>>,
    remote_cache_compression_enabled: bool,
) -> Result<ByteStreamServer, Error> {
    let config = config.unwrap_or_else(|| {
        vec![WithInstanceName {
            instance_name: "foo_instance_name".to_string(),
            config: ByteStreamConfig {
                cas_store: "main_cas".to_string(),
                persist_stream_on_disconnect_timeout_s: 0,
                max_bytes_per_stream: 1024,
            },
        }]
    });
    let remote_cache_compression_instances = if remote_cache_compression_enabled {
        RemoteCacheCompressionInstances::from_enabled_instance_names([
            "foo_instance_name".to_string()
        ])
    } else {
        RemoteCacheCompressionInstances::default()
    };
    ByteStreamServer::new(&config, store_manager, &remote_cache_compression_instances)
}

fn make_stream(
    encoding: Option<CompressionEncoding>,
) -> (mpsc::Sender<Frame<Bytes>>, Streaming<WriteRequest>) {
    let (tx, body) = ChannelBody::new();
    let mut codec = ProstCodec::<WriteRequest, WriteRequest>::default();
    let stream = Streaming::new_request(codec.decoder(), body, encoding, None);
    (tx, stream)
}

type JoinHandle = JoinHandleDropGuard<Result<Response<WriteResponse>, tonic::Status>>;

fn make_stream_and_writer_spawn(
    bs_server: Arc<ByteStreamServer>,
    encoding: Option<CompressionEncoding>,
) -> (mpsc::Sender<Frame<Bytes>>, JoinHandle) {
    let (tx, stream) = make_stream(encoding);
    let join_handle = spawn!("bs_server_write", async move {
        bs_server.write(Request::new(stream)).await
    });
    (tx, join_handle)
}

fn make_resource_name(data_len: impl core::fmt::Display) -> String {
    format!(
        "{}/uploads/{}/blobs/{}/{}",
        INSTANCE_NAME,
        "4dcec57e-1389-4ab5-b188-4a59f22ceb4b", // Randomly generated.
        HASH1,
        data_len,
    )
}

fn make_compressed_resource_name(
    uuid: &str,
    hash: &str,
    data_len: impl core::fmt::Display,
) -> String {
    make_compressed_resource_name_with_compressor("zstd", uuid, hash, data_len)
}

fn make_compressed_resource_name_with_compressor(
    compressor: &str,
    uuid: &str,
    hash: &str,
    data_len: impl core::fmt::Display,
) -> String {
    format!("{INSTANCE_NAME}/uploads/{uuid}/compressed-blobs/{compressor}/{hash}/{data_len}")
}

fn sha256_hex(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hex::encode(hasher.finalize())
}

async fn read_all_bytes(
    bs_server: &ByteStreamServer,
    read_request: ReadRequest,
) -> Result<Vec<u8>, tonic::Status> {
    let mut read_stream = bs_server
        .read(Request::new(read_request))
        .await?
        .into_inner();
    let mut data = Vec::new();
    while let Some(result_read_response) = read_stream.next().await {
        data.extend_from_slice(&result_read_response?.data);
    }
    Ok(data)
}

async fn server_and_client_stub(
    bs_server: ByteStreamServer,
    http_listener: HttpListener,
) -> (JoinHandleDropGuard<()>, ByteStreamClient<Channel>) {
    #[derive(Clone)]
    struct Executor;
    impl<F> hyper::rt::Executor<F> for Executor
    where
        F: Future + Send + 'static,
        F::Output: Send + 'static,
    {
        fn execute(&self, fut: F) {
            background_spawn!("executor_spawn", fut);
        }
    }

    let (tx, rx) = unbounded_channel::<Result<DuplexStream, Error>>();
    let mut rx = UnboundedReceiverStream::new(rx);

    let server_spawn = spawn!("grpc_server", async move {
        let http = auto::Builder::new(Executor);
        let mut service = bs_server.into_service();
        // Done in nativelink.rs in real versions
        if http_listener.max_decoding_message_size != 0 {
            service = service.max_decoding_message_size(http_listener.max_decoding_message_size);
        }
        let grpc_service = tonic::service::Routes::new(service);

        let adapted_service = tower::ServiceBuilder::new()
            .map_request(|req: hyper::Request<hyper::body::Incoming>| {
                let (parts, body) = req.into_parts();
                let body = body
                    .map_err(|e| tonic::Status::internal(e.to_string()))
                    .boxed_unsync();
                hyper::Request::from_parts(parts, body)
            })
            .service(grpc_service);

        let hyper_service = TowerToHyperService::new(adapted_service);

        while let Some(stream) = rx.next().await {
            http.serve_connection_with_upgrades(
                TokioIo::new(stream.expect("Failed to get stream")),
                hyper_service.clone(),
            )
            .await
            .expect("Connection failed");
        }
    });

    // Note: This is a dummy address, it will not actually connect to it,
    // instead it will be connecting via mpsc.
    let channel = Endpoint::try_from("http://[::]:50051")
        .unwrap()
        .executor(Executor)
        .connect_with_connector(service_fn(move |_: Uri| {
            let tx = tx.clone();
            async move {
                const MAX_BUFFER_SIZE: usize = 4096;
                let (client, server) = tokio::io::duplex(MAX_BUFFER_SIZE);
                tx.send(Ok(server)).unwrap();
                Result::<_, Error>::Ok(TokioIo::new(client))
            }
        }))
        .await
        .unwrap();

    let client = ByteStreamClient::new(channel);

    (server_spawn, client)
}

#[nativelink_test]
pub async fn chunked_stream_receives_all_data() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    // Setup stream.
    let (tx, join_handle) =
        make_stream_and_writer_spawn(bs_server, Some(CompressionEncoding::Gzip));

    // Send data.
    let raw_data = {
        // Chunk our data into two chunks to simulate something a client
        // might do.
        const BYTE_SPLIT_OFFSET: usize = 8;

        let raw_data = b"12456789abcdefghijk";

        let resource_name = format!(
            "{}/uploads/{}/blobs/{}/{}",
            INSTANCE_NAME,
            "4dcec57e-1389-4ab5-b188-4a59f22ceb4b", // Randomly generated.
            HASH1,
            raw_data.len()
        );
        let mut write_request = WriteRequest {
            resource_name,
            write_offset: 0,
            finish_write: false,
            data: vec![].into(),
        };
        // Write first chunk of data.
        write_request.write_offset = 0;
        write_request.data = raw_data[..BYTE_SPLIT_OFFSET].into();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;

        // Write empty set of data (clients are allowed to do this.
        write_request.write_offset = BYTE_SPLIT_OFFSET as i64;
        write_request.data = vec![].into();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;

        // Write final bit of data.
        write_request.write_offset = BYTE_SPLIT_OFFSET as i64;
        write_request.data = raw_data[BYTE_SPLIT_OFFSET..].into();
        write_request.finish_write = true;
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;

        raw_data
    };
    // Check results of server.
    {
        // One for spawn() future and one for result.
        let server_result = join_handle
            .await
            .expect("Failed to join")
            .expect("Failed write");
        let committed_size = usize::try_from(server_result.into_inner().committed_size)
            .or(Err("Cant convert i64 to usize"))?;
        assert_eq!(committed_size, raw_data.len());

        // Now lets check our store to ensure it was written with proper data.
        assert!(
            store
                .has(DigestInfo::try_new(HASH1, raw_data.len())?)
                .await?
                .is_some(),
            "Not found in store",
        );
        let store_data = store
            .get_part_unchunked(DigestInfo::try_new(HASH1, raw_data.len())?, 0, None)
            .await?;
        assert_eq!(
            core::str::from_utf8(&store_data),
            core::str::from_utf8(raw_data),
            "Expected store to have been updated to new value"
        );
    }

    Ok(())
}

#[nativelink_test]
pub async fn resume_write_success() -> Result<(), Box<dyn core::error::Error>> {
    const WRITE_DATA: &str = "12456789abcdefghijk";

    // Chunk our data into two chunks to simulate something a client
    // might do.
    const BYTE_SPLIT_OFFSET: usize = 8;

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let (tx, join_handle) =
        make_stream_and_writer_spawn(bs_server.clone(), Some(CompressionEncoding::Gzip));

    let resource_name = format!(
        "{}/uploads/{}/blobs/{}/{}",
        INSTANCE_NAME,
        "4dcec57e-1389-4ab5-b188-4a59f22ceb4b", // Randomly generated.
        HASH1,
        WRITE_DATA.len()
    );
    let mut write_request = WriteRequest {
        resource_name,
        write_offset: 0,
        finish_write: false,
        data: vec![].into(),
    };
    {
        // Write first chunk of data.
        write_request.write_offset = 0;
        write_request.data = WRITE_DATA[..BYTE_SPLIT_OFFSET].into();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
    }
    {
        // Now disconnect our stream.
        drop(tx);
        let result = join_handle.await.expect("Failed to join");
        assert_eq!(result.is_err(), true, "Expected error to be returned");
    }
    // Now reconnect.
    let (tx, join_handle) =
        make_stream_and_writer_spawn(bs_server, Some(CompressionEncoding::Gzip));
    {
        // Write the remainder of our data.
        write_request.write_offset = BYTE_SPLIT_OFFSET as i64;
        write_request.finish_write = true;
        write_request.data = WRITE_DATA[BYTE_SPLIT_OFFSET..].into();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
    }
    {
        // Now disconnect our stream.
        drop(tx);
        join_handle
            .await
            .expect("Failed to join")
            .expect("Failed write");
    }
    {
        // Check to make sure our store recorded the data properly.
        let digest = DigestInfo::try_new(HASH1, WRITE_DATA.len())?;
        assert_eq!(
            store.get_part_unchunked(digest, 0, None).await?,
            WRITE_DATA,
            "Data written to store did not match expected data",
        );
    }
    Ok(())
}

#[nativelink_test]
pub async fn restart_write_success() -> Result<(), Box<dyn core::error::Error>> {
    const WRITE_DATA: &str = "12456789abcdefghijk";

    // Chunk our data into two chunks to simulate something a client
    // might do.
    const BYTE_SPLIT_OFFSET: usize = 8;

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let (tx, join_handle) =
        make_stream_and_writer_spawn(bs_server.clone(), Some(CompressionEncoding::Gzip));

    let resource_name = format!(
        "{}/uploads/{}/blobs/{}/{}",
        INSTANCE_NAME,
        "4dcec57e-1389-4ab5-b188-4a59f22ceb4b", // Randomly generated.
        HASH1,
        WRITE_DATA.len()
    );
    let mut write_request = WriteRequest {
        resource_name,
        write_offset: 0,
        finish_write: false,
        data: vec![].into(),
    };
    {
        // Write first chunk of data.
        write_request.write_offset = 0;
        write_request.data = WRITE_DATA[..BYTE_SPLIT_OFFSET].into();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
    }
    {
        // Now disconnect our stream.
        drop(tx);
        let result = join_handle.await.expect("Failed to join");
        assert_eq!(result.is_err(), true, "Expected error to be returned");
    }
    // Now reconnect.
    let (tx, join_handle) =
        make_stream_and_writer_spawn(bs_server, Some(CompressionEncoding::Gzip));
    {
        // Write first chunk of data again.
        write_request.write_offset = 0;
        write_request.data = WRITE_DATA[..BYTE_SPLIT_OFFSET].into();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
    }
    {
        // Write the remainder of our data.
        write_request.write_offset = BYTE_SPLIT_OFFSET as i64;
        write_request.finish_write = true;
        write_request.data = WRITE_DATA[BYTE_SPLIT_OFFSET..].into();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
    }
    {
        // Now disconnect our stream.
        drop(tx);
        let result = join_handle.await.expect("Failed to join");
        assert!(result.is_ok(), "Expected success to be returned");
    }
    {
        // Check to make sure our store recorded the data properly.
        let digest = DigestInfo::try_new(HASH1, WRITE_DATA.len())?;
        assert_eq!(
            store.get_part_unchunked(digest, 0, None).await?,
            WRITE_DATA,
            "Data written to store did not match expected data",
        );
    }
    Ok(())
}

#[nativelink_test]
pub async fn restart_mid_stream_write_success() -> Result<(), Box<dyn core::error::Error>> {
    const WRITE_DATA: &str = "12456789abcdefghijk";

    // Chunk our data into two chunks to simulate something a client
    // might do.
    const BYTE_SPLIT_OFFSET: usize = 8;

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let (tx, join_handle) =
        make_stream_and_writer_spawn(bs_server.clone(), Some(CompressionEncoding::Gzip));

    let resource_name = format!(
        "{}/uploads/{}/blobs/{}/{}",
        INSTANCE_NAME,
        "4dcec57e-1389-4ab5-b188-4a59f22ceb4b", // Randomly generated.
        HASH1,
        WRITE_DATA.len()
    );
    let mut write_request = WriteRequest {
        resource_name,
        write_offset: 0,
        finish_write: false,
        data: vec![].into(),
    };
    {
        // Write first chunk of data.
        write_request.write_offset = 0;
        write_request.data = WRITE_DATA[..BYTE_SPLIT_OFFSET].into();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
    }
    {
        // Now disconnect our stream.
        drop(tx);
        let result = join_handle.await.expect("Failed to join");
        assert_eq!(result.is_err(), true, "Expected error to be returned");
    }
    // Now reconnect.
    let (tx, join_handle) =
        make_stream_and_writer_spawn(bs_server, Some(CompressionEncoding::Gzip));
    {
        // Write some of the first chunk of data again.
        write_request.write_offset = 2;
        write_request.data = WRITE_DATA[2..BYTE_SPLIT_OFFSET].into();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
    }
    {
        // Write the remainder of our data.
        write_request.write_offset = BYTE_SPLIT_OFFSET as i64;
        write_request.finish_write = true;
        write_request.data = WRITE_DATA[BYTE_SPLIT_OFFSET..].into();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
    }
    {
        // Now disconnect our stream.
        drop(tx);
        let result = join_handle.await.expect("Failed to join");
        assert!(result.is_ok(), "Expected success to be returned");
    }
    {
        // Check to make sure our store recorded the data properly.
        let digest = DigestInfo::try_new(HASH1, WRITE_DATA.len())?;
        assert_eq!(
            store.get_part_unchunked(digest, 0, None).await?,
            WRITE_DATA,
            "Data written to store did not match expected data",
        );
    }
    Ok(())
}

#[nativelink_test]
pub async fn ensure_write_is_not_done_until_write_request_is_set()
-> Result<(), Box<dyn core::error::Error>> {
    const WRITE_DATA: &str = "12456789abcdefghijk";

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    // Setup stream.
    let (tx, stream) = make_stream(Some(CompressionEncoding::Gzip));
    let mut write_fut = bs_server.write(Request::new(stream));

    let resource_name = make_resource_name(WRITE_DATA.len());
    let mut write_request = WriteRequest {
        resource_name,
        write_offset: 0,
        finish_write: false,
        data: vec![].into(),
    };
    {
        // Write our data.
        write_request.write_offset = 0;
        write_request.data = WRITE_DATA[..].into();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
    }
    // Note: We have to pull multiple times because there are multiple futures
    // joined onto this one future and we need to ensure we run the state machine as
    // far as possible.
    for _ in 0..100 {
        assert!(
            poll!(&mut write_fut).is_pending(),
            "Expected the future to not be completed yet"
        );
    }
    {
        // Write our EOF.
        write_request.write_offset = WRITE_DATA.len() as i64;
        write_request.finish_write = true;
        write_request.data.clear();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
    }
    let mut result = None;
    for _ in 0..100 {
        if let Poll::Ready(r) = poll!(&mut write_fut) {
            result = Some(r);
            break;
        }
    }
    {
        // Check our results.
        assert_eq!(
            result
                .err_tip(|| "bs_server.write never returned a value")?
                .err_tip(|| "bs_server.write returned an error")?
                .into_inner(),
            WriteResponse {
                committed_size: WRITE_DATA.len() as i64
            },
            "Expected Responses to match"
        );
    }
    {
        // Check to make sure our store recorded the data properly.
        let digest = DigestInfo::try_new(HASH1, WRITE_DATA.len())?;
        assert_eq!(
            store.get_part_unchunked(digest, 0, None).await?,
            WRITE_DATA,
            "Data written to store did not match expected data",
        );
    }
    Ok(())
}

#[nativelink_test]
pub async fn zstd_write_committed_size_matches_wire_bytes()
-> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server_with_remote_cache_compression(store_manager.as_ref(), None, true)
            .expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let raw_data = "zstd ByteStream committed_size regression ".repeat(256);
    let compressed_data = zstd::bulk::compress(raw_data.as_bytes(), 3)?;
    assert!(
        compressed_data.len() < raw_data.len(),
        "test data must compress to reproduce the committed_size mismatch"
    );

    let (tx, join_handle) = make_stream_and_writer_spawn(bs_server, None);
    let hash = sha256_hex(raw_data.as_bytes());
    let resource_name = make_compressed_resource_name(
        "4dcec57e-1389-4ab5-b188-4a59f22ceb4b",
        &hash,
        raw_data.len(),
    );
    tx.send(Frame::data(encode_stream_proto(&WriteRequest {
        resource_name,
        write_offset: 0,
        finish_write: true,
        data: compressed_data.clone().into(),
    })?))
    .await?;

    let server_result = join_handle
        .await
        .expect("Failed to join")
        .expect("Failed write");

    let digest = DigestInfo::try_new(&hash, raw_data.len())?;
    assert_eq!(
        store.get_part_unchunked(digest, 0, None).await?.as_ref(),
        raw_data.as_bytes(),
        "server should store the decompressed bytes"
    );

    let committed_size = server_result.into_inner().committed_size;
    assert!(
        committed_size == -1 || committed_size == compressed_data.len() as i64,
        "compressed write committed_size must be -1 or the compressed byte count {}; got {} for uncompressed size {}",
        compressed_data.len(),
        committed_size,
        raw_data.len()
    );

    Ok(())
}

#[nativelink_test]
pub async fn zstd_write_allows_wire_bytes_larger_than_digest_size()
-> Result<(), Box<dyn core::error::Error>> {
    let raw_data = b"x";
    let compressed_data = zstd::bulk::compress(raw_data, 3)?;
    assert!(
        compressed_data.len() > raw_data.len(),
        "test data must expand when compressed to cover compressed wire sizes larger than the digest"
    );

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server_with_remote_cache_compression(store_manager.as_ref(), None, true)
            .expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let (tx, join_handle) = make_stream_and_writer_spawn(bs_server, None);
    let hash = sha256_hex(raw_data);
    let resource_name = make_compressed_resource_name(
        "4dcec57e-1389-4ab5-b188-4a59f22ceb4c",
        &hash,
        raw_data.len(),
    );
    tx.send(Frame::data(encode_stream_proto(&WriteRequest {
        resource_name,
        write_offset: 0,
        finish_write: true,
        data: compressed_data.clone().into(),
    })?))
    .await?;

    let server_result = join_handle
        .await
        .expect("Failed to join")
        .expect("Failed write");

    let digest = DigestInfo::try_new(&hash, raw_data.len())?;
    assert_eq!(
        store.get_part_unchunked(digest, 0, None).await?.as_ref(),
        raw_data,
        "server should store the decompressed bytes"
    );

    assert_eq!(
        server_result.into_inner().committed_size,
        compressed_data.len() as i64,
        "compressed write should report the compressed wire byte count"
    );

    Ok(())
}

#[nativelink_test]
pub async fn zstd_write_rejected_when_remote_cache_compression_disabled()
-> Result<(), Box<dyn core::error::Error>> {
    let raw_data = b"compression disabled";
    let compressed_data = zstd::bulk::compress(raw_data, 3)?;

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );

    let (tx, join_handle) = make_stream_and_writer_spawn(bs_server, None);
    let hash = sha256_hex(raw_data);
    tx.send(Frame::data(encode_stream_proto(&WriteRequest {
        resource_name: make_compressed_resource_name(
            "4dcec57e-1389-4ab5-b188-4a59f22ceb54",
            &hash,
            raw_data.len(),
        ),
        write_offset: 0,
        finish_write: true,
        data: compressed_data.into(),
    })?))
    .await?;

    let Err(status) = join_handle.await.expect("Failed to join") else {
        panic!("zstd write should fail when remote cache compression is disabled");
    };
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status
            .message()
            .contains("Remote cache compression is not supported"),
        "unexpected error: {}",
        status.message()
    );

    Ok(())
}

#[nativelink_test]
pub async fn zstd_write_rejects_decompressed_digest_mismatch()
-> Result<(), Box<dyn core::error::Error>> {
    let raw_data = b"valid zstd payload with wrong digest";
    let compressed_data = zstd::bulk::compress(raw_data, 3)?;

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server_with_remote_cache_compression(store_manager.as_ref(), None, true)
            .expect("Failed to make server"),
    );

    let (tx, join_handle) = make_stream_and_writer_spawn(bs_server, None);
    let wrong_hash = sha256_hex(b"different data");
    tx.send(Frame::data(encode_stream_proto(&WriteRequest {
        resource_name: make_compressed_resource_name(
            "4dcec57e-1389-4ab5-b188-4a59f22ceb55",
            &wrong_hash,
            raw_data.len(),
        ),
        write_offset: 0,
        finish_write: true,
        data: compressed_data.into(),
    })?))
    .await?;

    let Err(status) = join_handle.await.expect("Failed to join") else {
        panic!("zstd write should fail when decompressed digest mismatches");
    };
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("Decompressed digest"),
        "unexpected error: {}",
        status.message()
    );

    Ok(())
}

#[nativelink_test]
pub async fn compressed_blobs_identity_write_and_read_use_identity_path()
-> Result<(), Box<dyn core::error::Error>> {
    let raw_data = Bytes::from_static(b"identity compressed-blobs payload");
    let hash = sha256_hex(raw_data.as_ref());

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let (tx, join_handle) = make_stream_and_writer_spawn(bs_server.clone(), None);
    tx.send(Frame::data(encode_stream_proto(&WriteRequest {
        resource_name: make_compressed_resource_name_with_compressor(
            "identity",
            "4dcec57e-1389-4ab5-b188-4a59f22ceb56",
            &hash,
            raw_data.len(),
        ),
        write_offset: 0,
        finish_write: true,
        data: raw_data.clone(),
    })?))
    .await?;

    let server_result = join_handle
        .await
        .expect("Failed to join")
        .expect("Failed write");
    assert_eq!(
        server_result.into_inner().committed_size,
        raw_data.len() as i64
    );

    let digest = DigestInfo::try_new(&hash, raw_data.len())?;
    assert_eq!(
        store.get_part_unchunked(digest, 0, None).await?,
        raw_data,
        "identity compressed-blobs write should store raw bytes"
    );

    let read_data = read_all_bytes(
        bs_server.as_ref(),
        ReadRequest {
            resource_name: format!(
                "{}/compressed-blobs/identity/{}/{}",
                INSTANCE_NAME,
                hash,
                raw_data.len()
            ),
            read_offset: 0,
            read_limit: raw_data.len() as i64,
        },
    )
    .await?;

    assert_eq!(read_data.as_slice(), raw_data.as_ref());

    Ok(())
}

#[nativelink_test]
pub async fn zstd_write_streams_chunked_compressed_upload()
-> Result<(), Box<dyn core::error::Error>> {
    let raw_data = "streamed zstd upload ".repeat(4096);
    let compressed_data = zstd::bulk::compress(raw_data.as_bytes(), 3)?;
    assert!(
        compressed_data.len() > 16,
        "test data must produce multiple compressed chunks"
    );

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server_with_remote_cache_compression(store_manager.as_ref(), None, true)
            .expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let (tx, join_handle) = make_stream_and_writer_spawn(bs_server, None);
    let hash = sha256_hex(raw_data.as_bytes());
    let resource_name = make_compressed_resource_name(
        "4dcec57e-1389-4ab5-b188-4a59f22ceb4d",
        &hash,
        raw_data.len(),
    );

    let mut write_offset = 0usize;
    while write_offset < compressed_data.len() {
        let end = (write_offset + 7).min(compressed_data.len());
        tx.send(Frame::data(encode_stream_proto(&WriteRequest {
            resource_name: resource_name.clone(),
            write_offset: write_offset as i64,
            finish_write: end == compressed_data.len(),
            data: Bytes::copy_from_slice(&compressed_data[write_offset..end]),
        })?))
        .await?;
        write_offset = end;
    }

    let server_result = join_handle
        .await
        .expect("Failed to join")
        .expect("Failed write");
    assert_eq!(
        server_result.into_inner().committed_size,
        compressed_data.len() as i64
    );

    let digest = DigestInfo::try_new(&hash, raw_data.len())?;
    assert_eq!(
        store.get_part_unchunked(digest, 0, None).await?.as_ref(),
        raw_data.as_bytes()
    );

    Ok(())
}

#[nativelink_test]
pub async fn zstd_write_rejects_mismatched_compressed_write_offset_before_decompressing()
-> Result<(), Box<dyn core::error::Error>> {
    let raw_data = b"offset check";

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server_with_remote_cache_compression(store_manager.as_ref(), None, true)
            .expect("Failed to make server"),
    );

    let (tx, join_handle) = make_stream_and_writer_spawn(bs_server, None);
    let hash = sha256_hex(raw_data);
    let resource_name = make_compressed_resource_name(
        "4dcec57e-1389-4ab5-b188-4a59f22ceb50",
        &hash,
        raw_data.len(),
    );
    tx.send(Frame::data(encode_stream_proto(&WriteRequest {
        resource_name,
        write_offset: 1,
        finish_write: true,
        data: Bytes::from_static(b"not zstd"),
    })?))
    .await?;

    let status = join_handle
        .await
        .expect("Failed to join")
        .expect_err("Expected compressed write to fail");
    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("out of order compressed data"),
        "unexpected error: {}",
        status.message()
    );

    Ok(())
}

#[nativelink_test]
pub async fn zstd_write_query_status_reports_compressed_wire_bytes()
-> Result<(), Box<dyn core::error::Error>> {
    let raw_data = "compressed query progress ".repeat(128);
    let compressed_data = zstd::bulk::compress(raw_data.as_bytes(), 3)?;
    let first_chunk_len = compressed_data.len() / 2;
    assert!(first_chunk_len > 0);

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server_with_remote_cache_compression(store_manager.as_ref(), None, true)
            .expect("Failed to make server"),
    );

    let (tx, join_handle) = make_stream_and_writer_spawn(bs_server.clone(), None);
    let hash = sha256_hex(raw_data.as_bytes());
    let resource_name = make_compressed_resource_name(
        "4dcec57e-1389-4ab5-b188-4a59f22ceb51",
        &hash,
        raw_data.len(),
    );

    tx.send(Frame::data(encode_stream_proto(&WriteRequest {
        resource_name: resource_name.clone(),
        write_offset: 0,
        finish_write: false,
        data: Bytes::copy_from_slice(&compressed_data[..first_chunk_len]),
    })?))
    .await?;

    let mut status_response = None;
    for _ in 0..100 {
        yield_now().await;
        let response = bs_server
            .query_write_status(Request::new(QueryWriteStatusRequest {
                resource_name: resource_name.clone(),
            }))
            .await?
            .into_inner();
        if response.committed_size == first_chunk_len as i64 {
            status_response = Some(response);
            break;
        }
    }
    assert_eq!(
        status_response.err_tip(|| "compressed write progress was not reported")?,
        QueryWriteStatusResponse {
            committed_size: first_chunk_len as i64,
            complete: false,
        }
    );

    tx.send(Frame::data(encode_stream_proto(&WriteRequest {
        resource_name,
        write_offset: first_chunk_len as i64,
        finish_write: true,
        data: Bytes::copy_from_slice(&compressed_data[first_chunk_len..]),
    })?))
    .await?;

    let server_result = join_handle
        .await
        .expect("Failed to join")
        .expect("Failed write");
    assert_eq!(
        server_result.into_inner().committed_size,
        compressed_data.len() as i64
    );

    Ok(())
}

#[nativelink_test]
pub async fn out_of_order_data_fails() -> Result<(), Box<dyn core::error::Error>> {
    const WRITE_DATA: &str = "12456789abcdefghijk";

    // Chunk our data into two chunks to simulate something a client
    // might do.
    const BYTE_SPLIT_OFFSET: usize = 8;

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );

    let (tx, join_handle) =
        make_stream_and_writer_spawn(bs_server, Some(CompressionEncoding::Gzip));

    let resource_name = make_resource_name(WRITE_DATA.len());
    let mut write_request = WriteRequest {
        resource_name,
        write_offset: 0,
        finish_write: false,
        data: vec![].into(),
    };
    {
        // Write first chunk of data.
        write_request.write_offset = 0;
        write_request.data = WRITE_DATA[..BYTE_SPLIT_OFFSET].into();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
    }
    {
        // Write data it already has.
        write_request.write_offset = (BYTE_SPLIT_OFFSET - 1) as i64;
        write_request.data = WRITE_DATA[(BYTE_SPLIT_OFFSET - 1)..].into();
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
    }
    assert!(
        join_handle.await.expect("Failed to join").is_err(),
        "Expected error to be returned"
    );
    {
        // Make sure stream was closed.
        write_request.write_offset = (BYTE_SPLIT_OFFSET - 1) as i64;
        write_request.data = WRITE_DATA[(BYTE_SPLIT_OFFSET - 1)..].into();
        assert!(
            tx.send(Frame::data(encode_stream_proto(&write_request)?))
                .await
                .is_err(),
            "Expected error to be returned"
        );
    }
    Ok(())
}

#[nativelink_test]
pub async fn upload_zero_byte_chunk() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let (tx, join_handle) =
        make_stream_and_writer_spawn(bs_server, Some(CompressionEncoding::Gzip));

    let resource_name = make_resource_name(0);
    let write_request = WriteRequest {
        resource_name,
        write_offset: 0,
        finish_write: true,
        data: vec![].into(),
    };

    {
        // Write our zero byte data.
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
        // Wait for stream to finish.
        join_handle
            .await
            .expect("Failed to join")
            .expect("Failed write");
    }
    {
        // Check to make sure our store recorded the data properly.
        let data = store
            .get_part_unchunked(DigestInfo::try_new(HASH1, 0)?, 0, None)
            .await?;
        assert_eq!(data, "", "Expected data to exist and be empty");
    }
    Ok(())
}

#[nativelink_test]
pub async fn disallow_negative_write_offset() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );

    let (tx, join_handle) =
        make_stream_and_writer_spawn(bs_server, Some(CompressionEncoding::Gzip));

    let resource_name = make_resource_name(0);
    let write_request = WriteRequest {
        resource_name,
        write_offset: -1,
        finish_write: true,
        data: vec![].into(),
    };

    {
        // Write our zero byte data.
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
        // Expect the write command to fail.
        assert!(join_handle.await.expect("Failed to join").is_err());
    }
    Ok(())
}

#[nativelink_test]
pub async fn out_of_sequence_write() -> Result<(), Box<dyn core::error::Error>> {
    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );

    let (tx, join_handle) =
        make_stream_and_writer_spawn(bs_server, Some(CompressionEncoding::Gzip));

    let resource_name = make_resource_name(100);
    let write_request = WriteRequest {
        resource_name,
        write_offset: 10,
        finish_write: false,
        data: "TEST".into(),
    };

    {
        // Write our zero byte data.
        tx.send(Frame::data(encode_stream_proto(&write_request)?))
            .await?;
        // Expect the write command to fail.
        assert!(join_handle.await.expect("Failed to join").is_err());
    }
    Ok(())
}

#[nativelink_test]
pub async fn chunked_stream_reads_small_set_of_data() -> Result<(), Box<dyn core::error::Error>> {
    const VALUE1: &str = "12456789abcdefghijk";

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let digest = DigestInfo::try_new(HASH1, VALUE1.len())?;
    store.update_oneshot(digest, VALUE1.into()).await?;

    let read_request = ReadRequest {
        resource_name: format!("{}/blobs/{}/{}", INSTANCE_NAME, HASH1, VALUE1.len()),
        read_offset: 0,
        read_limit: VALUE1.len() as i64,
    };
    let mut read_stream = bs_server
        .read(Request::new(read_request))
        .await?
        .into_inner();
    {
        let mut roundtrip_data = Vec::with_capacity(VALUE1.len());
        while let Some(result_read_response) = read_stream.next().await {
            roundtrip_data.append(&mut result_read_response?.data.to_vec());
        }
        assert_eq!(
            roundtrip_data,
            VALUE1.as_bytes(),
            "Expected response to match what is in store"
        );
    }
    Ok(())
}

#[nativelink_test]
pub async fn zstd_read_returns_compressed_bytes_that_decompress_to_stored_blob()
-> Result<(), Box<dyn core::error::Error>> {
    let raw_data = Bytes::from("zstd ByteStream compressed read ".repeat(256));
    let hash = sha256_hex(raw_data.as_ref());

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server_with_remote_cache_compression(store_manager.as_ref(), None, true)
            .expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let digest = DigestInfo::try_new(&hash, raw_data.len())?;
    store.update_oneshot(digest, raw_data.clone()).await?;

    let read_request = ReadRequest {
        resource_name: format!(
            "{}/compressed-blobs/zstd/{}/{}",
            INSTANCE_NAME,
            hash,
            raw_data.len()
        ),
        read_offset: 0,
        read_limit: 0,
    };
    let mut read_stream = bs_server
        .read(Request::new(read_request))
        .await?
        .into_inner();

    let mut compressed_data = Vec::new();
    while let Some(result_read_response) = read_stream.next().await {
        compressed_data.extend_from_slice(&result_read_response?.data);
    }

    assert!(
        !compressed_data.is_empty(),
        "Expected compressed read response to contain data"
    );
    assert_ne!(
        compressed_data.as_slice(),
        raw_data.as_ref(),
        "Expected compressible test data to differ from zstd wire bytes"
    );

    let decompressed_data = zstd::bulk::decompress(&compressed_data, raw_data.len())?;
    assert_eq!(
        decompressed_data.as_slice(),
        raw_data.as_ref(),
        "Expected decompressed read response to match stored raw blob"
    );

    Ok(())
}

#[nativelink_test]
pub async fn zstd_read_rejected_when_remote_cache_compression_disabled()
-> Result<(), Box<dyn core::error::Error>> {
    let raw_data = Bytes::from_static(b"compression disabled read");
    let hash = sha256_hex(raw_data.as_ref());

    let store_manager = make_store_manager().await?;
    let bs_server =
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server");

    let Err(status) = bs_server
        .read(Request::new(ReadRequest {
            resource_name: format!(
                "{}/compressed-blobs/zstd/{}/{}",
                INSTANCE_NAME,
                hash,
                raw_data.len()
            ),
            read_offset: 0,
            read_limit: 0,
        }))
        .await
    else {
        panic!("zstd read should fail when remote cache compression is disabled");
    };

    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status
            .message()
            .contains("Remote cache compression is not supported"),
        "unexpected error: {}",
        status.message()
    );

    Ok(())
}

#[nativelink_test]
pub async fn zstd_read_chunks_compressed_wire_bytes_by_configured_stream_size()
-> Result<(), Box<dyn core::error::Error>> {
    const MAX_BYTES_PER_STREAM: usize = 1;

    let raw_data = Bytes::from("zstd ByteStream compressed chunked read ".repeat(256));
    let hash = sha256_hex(raw_data.as_ref());

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server_with_remote_cache_compression(
            store_manager.as_ref(),
            Some(vec![WithInstanceName {
                instance_name: INSTANCE_NAME.to_string(),
                config: ByteStreamConfig {
                    cas_store: "main_cas".to_string(),
                    max_bytes_per_stream: MAX_BYTES_PER_STREAM,
                    ..Default::default()
                },
            }]),
            true,
        )
        .expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let digest = DigestInfo::try_new(&hash, raw_data.len())?;
    store.update_oneshot(digest, raw_data.clone()).await?;

    let read_request = ReadRequest {
        resource_name: format!(
            "{}/compressed-blobs/zstd/{}/{}",
            INSTANCE_NAME,
            hash,
            raw_data.len()
        ),
        read_offset: 0,
        read_limit: 0,
    };
    let mut read_stream = bs_server
        .read(Request::new(read_request))
        .await?
        .into_inner();

    let mut compressed_data = Vec::new();
    let mut chunk_lengths = Vec::new();
    while let Some(result_read_response) = read_stream.next().await {
        let data = result_read_response?.data;
        chunk_lengths.push(data.len());
        compressed_data.extend_from_slice(&data);
    }

    assert!(
        chunk_lengths.len() > 1,
        "Expected compressed read response to be split into multiple chunks"
    );
    assert!(
        chunk_lengths
            .iter()
            .all(|chunk_len| *chunk_len <= MAX_BYTES_PER_STREAM),
        "Expected compressed read chunks {chunk_lengths:?} to respect max_bytes_per_stream {MAX_BYTES_PER_STREAM}"
    );

    let decompressed_data = zstd::bulk::decompress(&compressed_data, raw_data.len())?;
    assert_eq!(
        decompressed_data.as_slice(),
        raw_data.as_ref(),
        "Expected decompressed chunked read response to match stored raw blob"
    );

    Ok(())
}

#[nativelink_test]
pub async fn zstd_read_offset_applies_to_uncompressed_blob()
-> Result<(), Box<dyn core::error::Error>> {
    let raw_data = Bytes::from(
        (0usize..4096)
            .map(|i| u8::try_from((i * 31 + i / 7) % 251).expect("modulo 251 fits in u8"))
            .collect::<Vec<_>>(),
    );
    let hash = sha256_hex(raw_data.as_ref());

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server_with_remote_cache_compression(store_manager.as_ref(), None, true)
            .expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let digest = DigestInfo::try_new(&hash, raw_data.len())?;
    store.update_oneshot(digest, raw_data.clone()).await?;

    let read_offset = 13usize;

    let ranged_data = read_all_bytes(
        bs_server.as_ref(),
        ReadRequest {
            resource_name: format!(
                "{}/compressed-blobs/zstd/{}/{}",
                INSTANCE_NAME,
                hash,
                raw_data.len()
            ),
            read_offset: read_offset as i64,
            read_limit: 0,
        },
    )
    .await?;

    let decompressed_data = zstd::bulk::decompress(&ranged_data, raw_data.len() - read_offset)?;
    assert_eq!(
        decompressed_data.as_slice(),
        &raw_data.as_ref()[read_offset..],
        "Expected read_offset to apply to the uncompressed blob before compression"
    );

    Ok(())
}

#[nativelink_test]
pub async fn zstd_read_rejects_nonzero_read_limit() -> Result<(), Box<dyn core::error::Error>> {
    let raw_data = Bytes::from(
        (0usize..8192)
            .map(|i| u8::try_from((i * 17 + i / 3) % 251).expect("modulo 251 fits in u8"))
            .collect::<Vec<_>>(),
    );
    let hash = sha256_hex(raw_data.as_ref());

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server_with_remote_cache_compression(store_manager.as_ref(), None, true)
            .expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let digest = DigestInfo::try_new(&hash, raw_data.len())?;
    store.update_oneshot(digest, raw_data.clone()).await?;

    let Err(status) = bs_server
        .read(Request::new(ReadRequest {
            resource_name: format!(
                "{}/compressed-blobs/zstd/{}/{}",
                INSTANCE_NAME,
                hash,
                raw_data.len()
            ),
            read_offset: 0,
            read_limit: 1,
        }))
        .await
    else {
        panic!("compressed read with read_limit should fail");
    };

    assert_eq!(status.code(), Code::InvalidArgument);
    assert!(
        status.message().contains("read_limit must be 0"),
        "unexpected error: {}",
        status.message()
    );

    Ok(())
}

#[nativelink_test]
pub async fn chunked_stream_reads_10mb_of_data() -> Result<(), Box<dyn core::error::Error>> {
    const DATA_SIZE: usize = 10_000_000;

    let store_manager = make_store_manager().await?;
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );
    let store = store_manager.get_store("main_cas").unwrap();

    let mut raw_data = vec![41u8; DATA_SIZE];
    // Change just a few bits to ensure we don't get same packet
    // over and over.
    raw_data[5] = 42u8;
    raw_data[DATA_SIZE - 2] = 43u8;

    let data_len = raw_data.len();
    let digest = DigestInfo::try_new(HASH1, data_len)?;
    store
        .update_oneshot(digest, raw_data.clone().into())
        .await?;

    let read_request = ReadRequest {
        resource_name: format!("{}/blobs/{}/{}", INSTANCE_NAME, HASH1, raw_data.len()),
        read_offset: 0,
        read_limit: raw_data.len() as i64,
    };
    let mut read_stream = bs_server
        .read(Request::new(read_request))
        .await?
        .into_inner();
    {
        let mut roundtrip_data = Vec::with_capacity(raw_data.len());
        assert!(
            !raw_data.is_empty(),
            "Expected at least one byte to be sent"
        );
        while let Some(result_read_response) = read_stream.next().await {
            roundtrip_data.append(&mut result_read_response?.data.to_vec());
        }
        assert_eq!(
            roundtrip_data, raw_data,
            "Expected response to match what is in store"
        );
    }
    Ok(())
}

/// A bug was found in early development where we could deadlock when reading a stream if the
/// store backend resulted in an error. This was because we were not shutting down the stream
/// when on the backend store error which caused the `AsyncReader` to block forever because the
/// stream was never shutdown.
#[nativelink_test]
pub async fn read_with_not_found_does_not_deadlock() -> Result<(), Error> {
    let store_manager = make_store_manager()
        .await
        .err_tip(|| "Couldn't get store manager")?;
    let mut read_stream = {
        let bs_server = make_bytestream_server(store_manager.as_ref(), None)
            .err_tip(|| "Couldn't make store")?;
        let read_request = ReadRequest {
            resource_name: format!(
                "{}/blobs/{}/{}",
                INSTANCE_NAME,
                HASH1,
                55, // Dummy value
            ),
            read_offset: 0,
            read_limit: 55,
        };
        // This should fail because there's no data in the store yet.
        bs_server
            .read(Request::new(read_request))
            .await
            .err_tip(|| "Couldn't send read")?
            .into_inner()
    };
    // We need to give a chance for the other spawns to do some work before we poll.
    yield_now().await;
    {
        let result_fut = read_stream.next();

        let result = result_fut.await.err_tip(|| "Expected result to be ready")?;
        let expected_err_str = "code: 'Some requested entity was not found', message: \"Key Digest(DigestInfo(\\\"0123456789abcdef000000000000000000000000000000000123456789abcdef-55\\\")) not found\"";
        assert_eq!(
            Error::from(result.unwrap_err()),
            make_err!(Code::NotFound, "{expected_err_str}"),
            "Expected error data to match"
        );
    }
    Ok(())
}

#[nativelink_test]
pub async fn test_query_write_status_smoke_test() -> Result<(), Box<dyn core::error::Error>> {
    const BYTE_SPLIT_OFFSET: usize = 8;

    let store_manager = make_store_manager()
        .await
        .expect("Failed to make store manager");
    let bs_server = Arc::new(
        make_bytestream_server(store_manager.as_ref(), None).expect("Failed to make server"),
    );

    let raw_data = b"12456789abcdefghijk";
    let resource_name = make_resource_name(raw_data.len());

    {
        // If the write does not exist it should respond with a 0 size.
        // This is because the client may have tried to write, but the
        // connection closed before the payload had been received.
        let response = bs_server
            .query_write_status(Request::new(QueryWriteStatusRequest {
                resource_name: resource_name.clone(),
            }))
            .await;
        assert_eq!(
            response.unwrap().into_inner(),
            QueryWriteStatusResponse {
                committed_size: 0,
                complete: false,
            }
        );
    }

    // Setup stream.
    let (tx, join_handle) =
        make_stream_and_writer_spawn(bs_server.clone(), Some(CompressionEncoding::Gzip));

    let mut write_request = WriteRequest {
        resource_name: resource_name.clone(),
        write_offset: 0,
        finish_write: false,
        data: vec![].into(),
    };

    // Write first chunk of data.
    write_request.write_offset = 0;
    write_request.data = raw_data[..BYTE_SPLIT_OFFSET].into();
    tx.send(Frame::data(encode_stream_proto(&write_request)?))
        .await?;

    {
        // Check to see if our request is active.
        yield_now().await;
        let data = bs_server
            .query_write_status(Request::new(QueryWriteStatusRequest {
                resource_name: resource_name.clone(),
            }))
            .await?;
        assert_eq!(
            data.into_inner(),
            QueryWriteStatusResponse {
                committed_size: write_request.data.len() as i64,
                complete: false,
            }
        );
    }

    // Finish writing our data.
    write_request.write_offset = BYTE_SPLIT_OFFSET as i64;
    write_request.data = raw_data[BYTE_SPLIT_OFFSET..].into();
    write_request.finish_write = true;
    tx.send(Frame::data(encode_stream_proto(&write_request)?))
        .await?;

    {
        // Now that it's done uploading, ensure it returns a success when requested again.
        yield_now().await;
        let data = bs_server
            .query_write_status(Request::new(QueryWriteStatusRequest { resource_name }))
            .await?;
        assert_eq!(
            data.into_inner(),
            QueryWriteStatusResponse {
                committed_size: raw_data.len() as i64,
                complete: true,
            }
        );
    }
    join_handle
        .await
        .expect("Failed to join")
        .expect("Failed write");
    Ok(())
}

#[nativelink_test]
pub async fn max_decoding_message_size_test() -> Result<(), Box<dyn core::error::Error>> {
    const MAX_MESSAGE_SIZE: usize = 1024 * 1024; // 1MB.

    // This is the size of the wrapper proto around the data.
    const WRITE_REQUEST_MSG_WRAPPER_SIZE: usize = 150;

    let store_manager = make_store_manager().await?;
    let config = vec![WithInstanceName {
        instance_name: INSTANCE_NAME.to_string(),
        config: ByteStreamConfig {
            cas_store: "main_cas".to_string(),
            ..Default::default()
        },
    }];
    let bs_server = make_bytestream_server(store_manager.as_ref(), Some(config))
        .expect("Failed to make server");
    let (server_join_handle, mut bs_client) = server_and_client_stub(
        bs_server,
        HttpListener {
            max_decoding_message_size: MAX_MESSAGE_SIZE,
            ..Default::default()
        },
    )
    .await;

    {
        // Test to ensure if we send exactly our max message size, it will succeed.
        let data = Bytes::from(vec![0u8; MAX_MESSAGE_SIZE - WRITE_REQUEST_MSG_WRAPPER_SIZE]);
        let write_request = WriteRequest {
            resource_name: make_resource_name(MAX_MESSAGE_SIZE),
            write_offset: 0,
            finish_write: true,
            data,
        };

        let (tx, rx) = unbounded_channel();
        let rx = UnboundedReceiverStream::new(rx);

        tx.send(write_request).expect("Failed to send data");

        let result = bs_client.write(Request::new(rx)).await;
        assert!(result.is_ok(), "Expected success, got {result:?}");
    }
    {
        // Test to ensure if we send exactly our max message size plus one, it will fail.
        let data = Bytes::from(vec![
            0u8;
            MAX_MESSAGE_SIZE - WRITE_REQUEST_MSG_WRAPPER_SIZE + 1
        ]);
        let write_request = WriteRequest {
            resource_name: make_resource_name(MAX_MESSAGE_SIZE),
            write_offset: 0,
            finish_write: true,
            data,
        };

        let (tx, rx) = unbounded_channel();
        let rx = UnboundedReceiverStream::new(rx);

        tx.send(write_request).expect("Failed to send data");

        let result = bs_client.write(Request::new(rx)).await;
        assert!(result.is_err(), "Expected error, got {result:?}");
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Error, decoded message length too large"),
            "Message should be too large message"
        );
    }

    drop(bs_client);
    // Wait for server to shutdown. This should happen when `bs_client` is dropped.
    server_join_handle.await.expect("Failed to join");

    Ok(())
}

#[nativelink_test]
async fn write_too_many_bytes_fails() -> Result<(), Box<dyn core::error::Error>> {
    const MAX_MESSAGE_SIZE: usize = 3;
    const DATA_SIZE: usize = 4;

    let (tx, join_handle) = make_stream_and_writer_spawn(
        Arc::new(make_bytestream_server(make_store_manager().await?.as_ref(), None).unwrap()),
        None,
    );

    tx.send(Frame::data(encode_stream_proto(&WriteRequest {
        resource_name: make_resource_name(MAX_MESSAGE_SIZE),
        write_offset: 0,
        finish_write: true,
        data: vec![0u8; DATA_SIZE].into(),
    })?))
    .await?;

    drop(tx);

    let err = join_handle
        .await?
        .expect_err("Expected an error for sending too many bytes");

    assert!(
        err.to_string().contains("Sent too much data"),
        "Got wrong error: {err:?}"
    );
    Ok(())
}

// NOTE: UUID collision fix has been verified manually.
// When two uploads use the same UUID and one is active, the server generates
// a unique UUID using nanosecond timestamp for the second upload.
// This prevents the "Cannot upload same UUID simultaneously" error that occurred
// in production with large C++ builds using Bazel.
// Manual testing shows the warning: "UUID collision detected, generating unique UUID"
// and both uploads complete successfully.

#[nativelink_test]
async fn uuid_collision_does_not_deadlock() -> Result<(), Box<dyn core::error::Error>> {
    // Two Write RPCs share a UUID while the first is mid-stream
    // (Entry::Occupied + idle_stream == None — Case 3 in
    // create_or_join_upload_stream). The server must release the
    // active_uploads lock before re-acquiring it for the unique-key insert,
    // otherwise it self-deadlocks on parking_lot's non-reentrant Mutex.
    //
    // A std::thread watchdog hard-exits at 5 s: tokio::time::timeout does
    // not reliably fire when a runtime worker is parked on a sync mutex.
    use core::sync::atomic::{AtomicBool, Ordering};

    struct WatchdogGuard(Arc<AtomicBool>);
    impl Drop for WatchdogGuard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    const DATA: &[u8] = &[0u8; 10];
    let resource_name = make_resource_name(DATA.len());

    let done = Arc::new(AtomicBool::new(false));
    let _wd = WatchdogGuard(done.clone());
    std::thread::spawn({
        let done = done.clone();
        move || {
            std::thread::sleep(core::time::Duration::from_secs(5));
            if !done.load(Ordering::SeqCst) {
                eprintln!("watchdog: timed out — UUID collision deadlock still present");
                std::process::exit(2);
            }
        }
    });

    let bs_server = Arc::new(
        make_bytestream_server(make_store_manager().await?.as_ref(), None)
            .expect("Failed to make server"),
    );

    // Write 1: keep the stream open with partial data so the UUID lands in
    // active_uploads with idle_stream == None.
    let (tx1, _join1) = make_stream_and_writer_spawn(bs_server.clone(), None);
    tx1.send(Frame::data(encode_stream_proto(&WriteRequest {
        resource_name: resource_name.clone(),
        write_offset: 0,
        finish_write: false,
        data: DATA[..5].into(),
    })?))
    .await?;
    tokio::time::sleep(core::time::Duration::from_millis(50)).await;

    // Write 2: same UUID while write 1 is active — triggers Case 3.
    let (tx2, join2) = make_stream_and_writer_spawn(bs_server.clone(), None);
    tx2.send(Frame::data(encode_stream_proto(&WriteRequest {
        resource_name: resource_name.clone(),
        write_offset: 0,
        finish_write: true,
        data: DATA.into(),
    })?))
    .await?;
    drop(tx2);

    join2.await.expect("task panicked")?;
    drop(tx1);
    Ok(())
}
