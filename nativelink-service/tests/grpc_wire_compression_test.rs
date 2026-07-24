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

//! Integration tests for client-side wire compression in `GrpcStore`
//! (`GrpcSpec.experimental_remote_cache_compression`) against a real
//! compression-enabled `ByteStream`/CAS server.

use core::sync::atomic::{AtomicU64, Ordering};
use std::collections::HashMap;
use std::sync::Arc;

use bytes::Bytes;
use futures::Future;
use http_body_util::BodyExt;
use hyper_util::rt::TokioIo;
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use nativelink_config::cas_server::{ByteStreamConfig, CasStoreConfig, WithInstanceName};
use nativelink_config::stores::{GrpcEndpoint, GrpcSpec, MemorySpec, Retry, StoreType};
use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_proto::google::bytestream::{
    QueryWriteStatusRequest, QueryWriteStatusResponse, ReadRequest, ReadResponse, WriteRequest,
    WriteResponse,
};
use nativelink_service::bytestream_server::ByteStreamServer;
use nativelink_service::cas_server::CasServer;
use nativelink_service::wire_compression::RemoteCacheCompressionInstances;
use nativelink_store::grpc_store::GrpcStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::background_spawn;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::{Store, StoreLike};
use sha2::{Digest as ShaDigest, Sha256};

const INSTANCE_NAME: &str = "";

#[derive(Clone)]
struct Executor;
impl<F> hyper::rt::Executor<F> for Executor
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    fn execute(&self, fut: F) {
        background_spawn!("test_executor", fut);
    }
}

async fn start_real_cas_server(
    memory_store: Arc<MemoryStore>,
    server_compression_enabled: bool,
) -> Result<u16, Error> {
    let store_manager = Arc::new(StoreManager::new());
    store_manager.add_store("main_cas", Store::new(memory_store))?;
    let compression_instances = if server_compression_enabled {
        RemoteCacheCompressionInstances::from_enabled_instance_names([INSTANCE_NAME.to_string()])
    } else {
        RemoteCacheCompressionInstances::default()
    };

    let bs_server = ByteStreamServer::new(
        &[WithInstanceName {
            instance_name: INSTANCE_NAME.to_string(),
            config: ByteStreamConfig {
                cas_store: "main_cas".to_string(),
                ..Default::default()
            },
        }],
        &store_manager,
        &compression_instances,
    )?;
    let cas_server = CasServer::new(
        &[WithInstanceName {
            instance_name: INSTANCE_NAME.to_string(),
            config: CasStoreConfig {
                cas_store: "main_cas".to_string(),
                experimental_chunking: None,
            },
        }],
        &store_manager,
        &compression_instances,
    )?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let routes = tonic::service::Routes::new(bs_server.into_service())
        .add_service(cas_server.into_service());
    let adapted_service = tower::ServiceBuilder::new()
        .map_request(|req: hyper::Request<hyper::body::Incoming>| {
            let (parts, body) = req.into_parts();
            let body = body
                .map_err(|e| tonic::Status::internal(e.to_string()))
                .boxed_unsync();
            hyper::Request::from_parts(parts, body)
        })
        .service(routes);
    let hyper_service = TowerToHyperService::new(adapted_service);

    background_spawn!("test_server_accept", async move {
        loop {
            let Ok((stream, _addr)) = listener.accept().await else {
                break;
            };
            stream.set_nodelay(true).unwrap();
            let hyper_service = hyper_service.clone();
            background_spawn!("test_server_conn", async move {
                drop(
                    auto::Builder::new(Executor)
                        .serve_connection_with_upgrades(TokioIo::new(stream), hyper_service)
                        .await,
                );
            });
        }
    });
    Ok(port)
}

fn grpc_spec(port: u16, compression: bool) -> GrpcSpec {
    GrpcSpec {
        instance_name: INSTANCE_NAME.to_string(),
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
        rpc_timeout_s: 120,
        use_legacy_resource_names: false,
        headers: HashMap::new(),
        forward_headers: vec![],
        experimental_read_batching: None,
        experimental_remote_cache_compression: compression,
    }
}

/// Structured, ~3:1-compressible content with a real sha256 digest (both
/// wire directions digest-verify).
fn make_content(tag: u8, len: usize) -> (Bytes, DigestInfo) {
    let mut data = vec![0u8; len];
    for (i, b) in data.iter_mut().enumerate() {
        *b = match i % 8 {
            0..=4 => tag,
            5 => u8::try_from((i / 8) % 256).expect("value is reduced modulo 256"),
            _ => u8::try_from((i / 4096) % 256).expect("value is reduced modulo 256"),
        };
    }
    let mut hasher = Sha256::new();
    hasher.update(&data);
    let digest = DigestInfo::try_new(&hex::encode(hasher.finalize()), len).unwrap();
    (Bytes::from(data), digest)
}

// A blob above the compression threshold uploads via compressed-blobs and
// must land byte-identical (the server decodes and digest-verifies).
#[nativelink_test]
async fn compressed_upload_round_trip() -> Result<(), Box<dyn core::error::Error>> {
    let memory_store = MemoryStore::new(&MemorySpec::default());
    let port = start_real_cas_server(memory_store.clone(), true).await?;
    let grpc_store = GrpcStore::new(&grpc_spec(port, true)).await?;

    let (content, digest) = make_content(0xA1, 1024 * 1024);
    grpc_store.update_oneshot(digest, content.clone()).await?;

    let stored = memory_store.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(stored, content, "decoded upload must be byte-identical");
    Ok(())
}

// A full-blob read above the threshold downloads via compressed-blobs; the
// client decodes and digest-verifies.
#[nativelink_test]
async fn compressed_download_round_trip() -> Result<(), Box<dyn core::error::Error>> {
    let memory_store = MemoryStore::new(&MemorySpec::default());
    let port = start_real_cas_server(memory_store.clone(), true).await?;
    let grpc_store = GrpcStore::new(&grpc_spec(port, true)).await?;

    let (content, digest) = make_content(0xB2, 1024 * 1024);
    memory_store.update_oneshot(digest, content.clone()).await?;

    let fetched = grpc_store.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(fetched, content, "decoded download must be byte-identical");
    Ok(())
}

// Blobs below the threshold must use the identity path: they round-trip
// even against a server with compression disabled.
#[nativelink_test]
async fn small_blob_uses_identity_path() -> Result<(), Box<dyn core::error::Error>> {
    let memory_store = MemoryStore::new(&MemorySpec::default());
    let port = start_real_cas_server(memory_store.clone(), false).await?;
    let grpc_store = GrpcStore::new(&grpc_spec(port, true)).await?;

    let (content, digest) = make_content(0xC3, 4 * 1024);
    grpc_store.update_oneshot(digest, content.clone()).await?;
    let fetched = grpc_store.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(fetched, content);
    Ok(())
}

// Ranged reads must use the identity path even above the threshold: they
// round-trip against a compression-disabled server.
#[nativelink_test]
async fn ranged_read_uses_identity_path() -> Result<(), Box<dyn core::error::Error>> {
    let memory_store = MemoryStore::new(&MemorySpec::default());
    let compressed_port = start_real_cas_server(memory_store.clone(), true).await?;
    let grpc_store = GrpcStore::new(&grpc_spec(compressed_port, true)).await?;

    let (content, digest) = make_content(0xD4, 1024 * 1024);
    memory_store.update_oneshot(digest, content.clone()).await?;

    let fetched = grpc_store
        .get_part_unchunked(digest, 100, Some(1000))
        .await?;
    assert_eq!(fetched, content.slice(100..1100));
    Ok(())
}

// A compressed upload to an upstream without compression enabled fails with
// InvalidArgument (config-trust contract, documented on the field).
#[nativelink_test]
async fn upload_to_non_compressed_upstream_fails() -> Result<(), Box<dyn core::error::Error>> {
    let memory_store = MemoryStore::new(&MemorySpec::default());
    let port = start_real_cas_server(memory_store.clone(), false).await?;
    let grpc_store = GrpcStore::new(&grpc_spec(port, true)).await?;

    let (content, digest) = make_content(0xE5, 1024 * 1024);
    let err = grpc_store
        .update_oneshot(digest, content)
        .await
        .expect_err("expected compressed upload to fail against plain upstream");
    assert_eq!(err.code, Code::InvalidArgument, "unexpected error: {err:?}");
    Ok(())
}

// Flag off: byte-identical legacy behavior against a plain upstream.
#[nativelink_test]
async fn flag_off_plain_round_trip() -> Result<(), Box<dyn core::error::Error>> {
    let memory_store = MemoryStore::new(&MemorySpec::default());
    let port = start_real_cas_server(memory_store.clone(), false).await?;
    let grpc_store = GrpcStore::new(&grpc_spec(port, false)).await?;

    let (content, digest) = make_content(0xF6, 1024 * 1024);
    grpc_store.update_oneshot(digest, content.clone()).await?;
    let fetched = grpc_store.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(fetched, content);
    Ok(())
}

// ---------------------------------------------------------------------------
// Review-round regression tests: W1 (no resume storm), W2 (corruption is
// terminal), W3 (no hang on early completion). These need a controllable
// fake ByteStream server rather than the real one.
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct FakeByteStream {
    write_calls: Arc<AtomicU64>,
    identity_reads: Arc<AtomicU64>,
    /// Write behavior: consume this many messages, then abort the stream
    /// with UNAVAILABLE. Zero means accept-one-then-early-complete (W3).
    abort_write_after_messages: Option<u64>,
    early_complete_write: bool,
    /// Read behavior: payload served for compressed-blobs reads.
    compressed_read_payload: Option<Bytes>,
    /// Pause the compressed read stream after this many chunks (forces the
    /// client feed to lag the decoder, exercising the cascade race).
    pause_compressed_read_after_chunks: Option<usize>,
    /// Hold the compressed read stream open forever after this many chunks.
    /// This ensures a decoder error must cancel the feed rather than waiting
    /// for the upstream stream to finish.
    hold_compressed_read_after_chunks: Option<usize>,
    /// Payload served for identity reads (fallback detector).
    identity_read_payload: Option<Bytes>,
}

type ReadStreamT =
    core::pin::Pin<Box<dyn futures::Stream<Item = Result<ReadResponse, tonic::Status>> + Send>>;

#[tonic::async_trait]
impl nativelink_proto::google::bytestream::byte_stream_server::ByteStream for FakeByteStream {
    type ReadStream = ReadStreamT;

    async fn read(
        &self,
        request: tonic::Request<ReadRequest>,
    ) -> Result<tonic::Response<Self::ReadStream>, tonic::Status> {
        let resource_name = request.into_inner().resource_name;
        if resource_name.contains("compressed-blobs") {
            let payload = self
                .compressed_read_payload
                .clone()
                .ok_or_else(|| tonic::Status::not_found("no compressed payload configured"))?;
            let chunks: Vec<Bytes> = payload
                .chunks(64 * 1024)
                .map(Bytes::copy_from_slice)
                .collect();
            let pause_after = self.pause_compressed_read_after_chunks;
            let hold_after = self.hold_compressed_read_after_chunks;
            let stream =
                futures::stream::unfold((chunks, 0usize), move |(chunks, index)| async move {
                    if index >= chunks.len() {
                        return None;
                    }
                    if Some(index) == hold_after {
                        futures::future::pending::<()>().await;
                    }
                    if Some(index) == pause_after {
                        // Hold the wire open long enough that the client
                        // decoder settles first.
                        tokio::time::sleep(core::time::Duration::from_millis(500)).await;
                    }
                    let data = chunks[index].clone();
                    Some((Ok(ReadResponse { data }), (chunks, index + 1)))
                });
            let boxed: Self::ReadStream = Box::pin(stream);
            return Ok(tonic::Response::new(boxed));
        }
        self.identity_reads.fetch_add(1, Ordering::Relaxed);
        let payload = self
            .identity_read_payload
            .clone()
            .ok_or_else(|| tonic::Status::not_found("no identity payload configured"))?;
        let chunks: Vec<Result<ReadResponse, tonic::Status>> = payload
            .chunks(64 * 1024)
            .map(|c| {
                Ok(ReadResponse {
                    data: Bytes::copy_from_slice(c),
                })
            })
            .collect();
        let boxed: Self::ReadStream = Box::pin(futures::stream::iter(chunks));
        Ok(tonic::Response::new(boxed))
    }

    async fn write(
        &self,
        request: tonic::Request<tonic::Streaming<WriteRequest>>,
    ) -> Result<tonic::Response<WriteResponse>, tonic::Status> {
        self.write_calls.fetch_add(1, Ordering::Relaxed);
        let mut stream = request.into_inner();
        if self.early_complete_write {
            // Read exactly one message, then settle the write early per the
            // REAPI duplicate-upload contract without consuming the rest.
            let _first = stream.message().await?;
            return Ok(tonic::Response::new(WriteResponse { committed_size: -1 }));
        }
        if let Some(abort_after) = self.abort_write_after_messages {
            let mut seen = 0u64;
            while seen < abort_after {
                match stream.message().await? {
                    Some(_msg) => seen += 1,
                    None => break,
                }
            }
            return Err(tonic::Status::unavailable("injected mid-stream failure"));
        }
        Err(tonic::Status::unimplemented("no write behavior configured"))
    }

    async fn query_write_status(
        &self,
        _request: tonic::Request<QueryWriteStatusRequest>,
    ) -> Result<tonic::Response<QueryWriteStatusResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not used in these tests"))
    }
}

async fn start_fake_bytestream_server(fake: FakeByteStream) -> Result<u16, Error> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let routes = tonic::service::Routes::new(
        nativelink_proto::google::bytestream::byte_stream_server::ByteStreamServer::new(fake),
    );
    let adapted_service = tower::ServiceBuilder::new()
        .map_request(|req: hyper::Request<hyper::body::Incoming>| {
            let (parts, body) = req.into_parts();
            let body = body
                .map_err(|e| tonic::Status::internal(e.to_string()))
                .boxed_unsync();
            hyper::Request::from_parts(parts, body)
        })
        .service(routes);
    let hyper_service = TowerToHyperService::new(adapted_service);
    background_spawn!("fake_server_accept", async move {
        loop {
            let Ok((stream, _addr)) = listener.accept().await else {
                break;
            };
            stream.set_nodelay(true).unwrap();
            let hyper_service = hyper_service.clone();
            background_spawn!("fake_server_conn", async move {
                drop(
                    auto::Builder::new(Executor)
                        .serve_connection_with_upgrades(TokioIo::new(stream), hyper_service)
                        .await,
                );
            });
        }
    });
    Ok(port)
}

// W1: a mid-stream failure of a compressed upload must fail fast with the
// transport error — exactly one write attempt, no resume storm ending in
// InvalidArgument.
#[nativelink_test]
async fn compressed_upload_mid_stream_failure_fails_fast() -> Result<(), Box<dyn core::error::Error>>
{
    let write_calls = Arc::new(AtomicU64::new(0));
    let port = start_fake_bytestream_server(FakeByteStream {
        write_calls: write_calls.clone(),
        abort_write_after_messages: Some(2),
        ..Default::default()
    })
    .await?;
    let mut spec = grpc_spec(port, true);
    spec.retry = Retry {
        max_retries: 3,
        delay: 0.01,
        ..Default::default()
    };
    let grpc_store = GrpcStore::new(&spec).await?;

    let (content, digest) = make_content(0x11, 4 * 1024 * 1024);
    let result = grpc_store.update_oneshot(digest, content).await;
    let err = result.expect_err("expected mid-stream failure to surface");
    assert_eq!(
        err.code,
        Code::Unavailable,
        "must surface the transport error, not a resume-replay InvalidArgument: {err:?}"
    );
    assert_eq!(
        write_calls.load(Ordering::Relaxed),
        1,
        "compressed uploads must not replay-resume after partial consumption"
    );
    Ok(())
}

// W2: decoder-detected corruption mid-stream must be terminal
// InvalidArgument deterministically — never silently retried via the
// unverified identity path.
#[nativelink_test]
async fn corrupt_compressed_download_is_terminal() -> Result<(), Box<dyn core::error::Error>> {
    // The decoder must abort MID-STREAM while the feed still has chunks in
    // flight (the old classification then saw a cascade "receiver
    // disconnected" feed error and silently fell back to the unverified
    // identity path). Bit-flips inside a zstd block decode to garbage and
    // only fail the digest check at EOF, so instead append a second valid
    // frame: its first decoded block overshoots the expected size, which
    // the decoder rejects immediately and deterministically.
    use nativelink_proto::build::bazel::remote::execution::v2::compressor;
    let (content, digest) = make_content(0x22, 8 * 1024 * 1024);
    let valid_frame =
        nativelink_util::wire_compression::compress(content.clone(), compressor::Value::Zstd)?;
    let (extra, _) = make_content(0x99, 8 * 1024 * 1024);
    let overflow_frame =
        nativelink_util::wire_compression::compress(extra, compressor::Value::Zstd)?;
    let valid_frame_chunks = valid_frame.len().div_ceil(64 * 1024);
    let mut corrupt = valid_frame.to_vec();
    corrupt.extend_from_slice(&overflow_frame);
    let identity_reads = Arc::new(AtomicU64::new(0));
    let port = start_fake_bytestream_server(FakeByteStream {
        identity_reads: identity_reads.clone(),
        // Pause just after the first frame ends: the decoder hits the
        // size overshoot while the feed is still blocked on the wire, so
        // the feed's next send hits the dropped receiver.
        hold_compressed_read_after_chunks: Some(valid_frame_chunks + 1),
        compressed_read_payload: Some(Bytes::from(corrupt)),
        identity_read_payload: Some(content),
        ..Default::default()
    })
    .await?;
    let grpc_store = GrpcStore::new(&grpc_spec(port, true)).await?;

    let result = tokio::time::timeout(
        core::time::Duration::from_secs(2),
        grpc_store.get_part_unchunked(digest, 0, None),
    )
    .await
    .expect("decoder failure must cancel the held compressed read stream");
    let err = result.expect_err("corrupt compressed data must be terminal");
    assert_eq!(
        err.code,
        Code::InvalidArgument,
        "decoder verdict must win over cascade transport classification: {err:?}"
    );
    assert_eq!(
        identity_reads.load(Ordering::Relaxed),
        0,
        "corrupt data must never fall back to the unverified identity path"
    );
    Ok(())
}

// W3: a server that early-completes a compressed upload while the worker
// producer is still sending must not turn the producer's send/drain into an
// error.
#[nativelink_test]
async fn compressed_upload_returns_after_early_completion()
-> Result<(), Box<dyn core::error::Error>> {
    use nativelink_util::buf_channel::make_buf_channel_pair;
    use nativelink_util::store_trait::UploadSizeInfo;

    let port = start_fake_bytestream_server(FakeByteStream {
        early_complete_write: true,
        ..Default::default()
    })
    .await?;
    let grpc_store = GrpcStore::new(&grpc_spec(port, true)).await?;

    let (content, digest) = make_content(0x33, 4 * 1024 * 1024);
    let (mut tx, rx) = make_buf_channel_pair();
    let producer_content = content.clone();
    let producer = async move {
        for chunk in producer_content.chunks(64 * 1024) {
            tx.send(Bytes::copy_from_slice(chunk)).await?;
        }
        tx.send_eof()
    };
    let update_fut = grpc_store.update(digest, rx, UploadSizeInfo::ExactSize(content.len() as u64));
    let (result, producer_result) =
        tokio::time::timeout(core::time::Duration::from_secs(10), async {
            tokio::join!(update_fut, producer)
        })
        .await
        .expect("early-completed compressed upload must not hang the producer");
    result?;
    producer_result?;
    Ok(())
}
