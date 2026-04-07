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

//! Benchmark measuring gRPC transport latency and throughput for NativeLink's
//! CAS and ByteStream operations over TCP (h2/tonic) and QUIC (h3/quinn).
//!
//! Spins up in-process TCP and QUIC gRPC servers backed by `MemoryStore` and
//! exercises them through `GrpcStore` (the production client) to measure
//! real end-to-end performance including serialization, framing, and
//! transport overhead.
//!
//! Run (TCP only):
//!   cargo bench --bench transport_bench
//!
//! Run (TCP + QUIC):
//!   cargo bench --features quic --bench transport_bench

use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use nativelink_config::cas_server::{ByteStreamConfig, CasStoreConfig, WithInstanceName};
use nativelink_config::stores::{
    EvictionPolicy, GrpcEndpoint, GrpcSpec, MemorySpec, Retry, StoreType,
};
use nativelink_service::bytestream_server::ByteStreamServer;
use nativelink_service::cas_server::CasServer;
use nativelink_store::grpc_store::GrpcStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::{Store, StoreDriver, StoreLike};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use tonic::transport::Server;

const INSTANCE_NAME: &str = "bench";

fn make_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
}

fn make_blob(size: usize) -> (DigestInfo, Bytes) {
    let data: Vec<u8> = (0..size).map(|i| (i % 256) as u8).collect();
    let hash = Sha256::digest(&data);
    let mut packed = [0u8; 32];
    packed.copy_from_slice(&hash);
    let digest = DigestInfo::new(packed, size as u64);
    (digest, Bytes::from(data))
}

fn make_store_manager() -> Arc<StoreManager> {
    let store_manager = Arc::new(StoreManager::new());
    let memory_store: Arc<MemoryStore> = MemoryStore::new(&MemorySpec {
        eviction_policy: Some(EvictionPolicy {
            max_bytes: 1_073_741_824,
            ..Default::default()
        }),
    });
    store_manager.add_store("main_cas", Store::new(memory_store));
    store_manager
}

fn make_services(
    store_manager: &StoreManager,
) -> (ByteStreamServer, CasServer) {
    let bytestream = ByteStreamServer::new(
        &[WithInstanceName {
            instance_name: INSTANCE_NAME.to_string(),
            config: ByteStreamConfig {
                cas_store: "main_cas".to_string(),
                max_bytes_per_stream: 3 * 1024 * 1024,
                ..Default::default()
            },
        }],
        store_manager,
    )
    .expect("failed to create ByteStreamServer");

    let cas = CasServer::new(
        &[WithInstanceName {
            instance_name: INSTANCE_NAME.to_string(),
            config: CasStoreConfig {
                cas_store: "main_cas".to_string(),
            },
        }],
        store_manager,
    )
    .expect("failed to create CasServer");

    (bytestream, cas)
}

// ---------------------------------------------------------------------------
// Self-signed TLS cert (shared by TCP+TLS and QUIC)
// ---------------------------------------------------------------------------

struct TlsCerts {
    cert_pem: String,
    key_pem: String,
    cert_file: tempfile::NamedTempFile,
    key_file: tempfile::NamedTempFile,
}

fn generate_tls_certs() -> TlsCerts {
    let certified_key = rcgen::generate_simple_self_signed(vec!["localhost".to_string()])
        .expect("failed to generate self-signed cert");
    let cert_pem = certified_key.cert.pem();
    let key_pem = certified_key.signing_key.serialize_pem();

    use std::io::Write;
    let mut cert_file = tempfile::NamedTempFile::new().expect("failed to create cert temp file");
    cert_file.write_all(cert_pem.as_bytes()).unwrap();
    cert_file.flush().unwrap();
    let mut key_file = tempfile::NamedTempFile::new().expect("failed to create key temp file");
    key_file.write_all(key_pem.as_bytes()).unwrap();
    key_file.flush().unwrap();

    TlsCerts {
        cert_pem,
        key_pem,
        cert_file,
        key_file,
    }
}

// ---------------------------------------------------------------------------
// TCP+TLS server/client
// ---------------------------------------------------------------------------

struct TcpServerHandle {
    port: u16,
    _handle: tokio::task::JoinHandle<()>,
}

async fn start_tcp_server(store_manager: &StoreManager, certs: &TlsCerts) -> TcpServerHandle {
    let (bytestream, cas) = make_services(store_manager);
    let max_msg = 256 * 1024 * 1024;

    let identity = tonic::transport::Identity::from_pem(
        certs.cert_pem.as_bytes(),
        certs.key_pem.as_bytes(),
    );
    let tls_config = tonic::transport::ServerTlsConfig::new().identity(identity);

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("failed to bind TCP listener");
    let port = listener.local_addr().unwrap().port();

    let handle = tokio::spawn(async move {
        let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
        Server::builder()
            .tls_config(tls_config)
            .expect("failed to configure TLS")
            .add_service(
                bytestream
                    .into_service()
                    .max_decoding_message_size(max_msg)
                    .max_encoding_message_size(max_msg),
            )
            .add_service(
                cas.into_service()
                    .max_decoding_message_size(max_msg)
                    .max_encoding_message_size(max_msg),
            )
            .serve_with_incoming(incoming)
            .await
            .expect("TCP+TLS gRPC server failed");
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    TcpServerHandle {
        port,
        _handle: handle,
    }
}

async fn make_tcp_client(port: u16, certs: &TlsCerts) -> Arc<GrpcStore> {
    use nativelink_config::stores::ClientTlsConfig;

    let spec = GrpcSpec {
        instance_name: INSTANCE_NAME.to_string(),
        endpoints: vec![GrpcEndpoint {
            address: format!("https://localhost:{port}"),
            tls_config: Some(ClientTlsConfig {
                ca_file: Some(certs.cert_file.path().to_string_lossy().to_string()),
                cert_file: None,
                key_file: None,
                use_native_roots: None,
            }),
            concurrency_limit: None,
            connect_timeout_s: 5,
            tcp_keepalive_s: 0,
            http2_keepalive_interval_s: 0,
            http2_keepalive_timeout_s: 0,
            tcp_nodelay: true,
            use_http3: false,
        }],
        store_type: StoreType::Cas,
        retry: Retry::default(),
        max_concurrent_requests: 0,
        connections_per_endpoint: 4,
        rpc_timeout_s: 120,
        batch_update_threshold_bytes: 1_048_576,
        max_concurrent_batch_rpcs: 8,
        parallel_chunk_read_threshold: 8 * 1024 * 1024,
        parallel_chunk_count: 64,
        dual_transport: false,
    };
    GrpcStore::new(&spec)
        .await
        .expect("failed to create TCP+TLS GrpcStore client")
}

// ---------------------------------------------------------------------------
// QUIC server/client
// ---------------------------------------------------------------------------

#[cfg(feature = "quic")]
struct QuicServerHandle {
    port: u16,
    _handle: tokio::task::JoinHandle<()>,
}

#[cfg(feature = "quic")]
async fn start_quic_server(store_manager: &StoreManager, certs: &TlsCerts) -> QuicServerHandle {
    use rustls_pki_types::pem::PemObject;
    use rustls_pki_types::{CertificateDer, PrivateKeyDer};

    let (bytestream, cas) = make_services(store_manager);
    let cert_pem = &certs.cert_pem;
    let key_pem = &certs.key_pem;

    let certs: Vec<CertificateDer> =
        CertificateDer::pem_reader_iter(&mut cert_pem.as_bytes())
            .collect::<Result<_, _>>()
            .expect("failed to parse cert PEM");
    let key = PrivateKeyDer::from_pem_reader(&mut key_pem.as_bytes())
        .expect("failed to parse key PEM");

    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let mut tls_config = rustls::ServerConfig::builder_with_provider(
        rustls::crypto::aws_lc_rs::default_provider().into(),
    )
    .with_safe_default_protocol_versions()
    .expect("failed to set TLS protocol versions")
    .with_no_client_auth()
    .with_single_cert(certs, key)
    .expect("failed to set server cert");
    tls_config.alpn_protocols = vec![b"h3".to_vec()];
    tls_config.max_early_data_size = u32::MAX;

    let mut quic_server_config = quinn::ServerConfig::with_crypto(Arc::new(
        quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(tls_config))
            .expect("failed to create QUIC server config"),
    ));

    // Aggressive loopback transport config — maximize throughput.
    let mut transport = quinn::TransportConfig::default();
    transport.stream_receive_window((16 * 1024 * 1024u32).into());
    transport.receive_window((512 * 1024 * 1024u32).into()); // 512 MiB connection window
    transport.send_window(512 * 1024 * 1024); // 512 MiB
    transport.max_concurrent_bidi_streams(8192u32.into());
    transport.max_concurrent_uni_streams(1024u32.into());
    transport.initial_rtt(std::time::Duration::from_micros(10)); // 10μs loopback
    // Disable ACK delay — process ACKs immediately on loopback.
    transport.ack_frequency_config(None);
    transport.max_idle_timeout(Some(
        std::time::Duration::from_secs(30)
            .try_into()
            .unwrap(),
    ));
    // No congestion controller — loopback has no congestion.
    // This removes BBR overhead entirely.
    transport.congestion_controller_factory(Arc::new(
        quinn::congestion::BbrConfig::default(),
    ));
    // TODO: quinn doesn't expose a way to disable congestion control entirely.
    // BBR with 10μs initial_rtt and huge windows is the closest we can get.
    quic_server_config.transport_config(Arc::new(transport));

    let udp_socket = std::net::UdpSocket::bind("127.0.0.1:0")
        .expect("failed to bind UDP socket");
    udp_socket
        .set_nonblocking(true)
        .expect("failed to set non-blocking");
    let port = udp_socket.local_addr().unwrap().port();

    let quinn_endpoint = quinn::Endpoint::new(
        quinn::EndpointConfig::default(),
        Some(quic_server_config),
        udp_socket,
        quinn::default_runtime().expect("failed to create quinn runtime"),
    )
    .expect("failed to create quinn endpoint");

    let max_msg = 256 * 1024 * 1024;
    let routes = tonic::service::Routes::new(
        bytestream
            .into_service()
            .max_decoding_message_size(max_msg)
            .max_encoding_message_size(max_msg),
    )
    .add_service(
        cas.into_service()
            .max_decoding_message_size(max_msg)
            .max_encoding_message_size(max_msg),
    );

    let acceptor = tonic_h3::quinn::H3QuinnAcceptor::new(quinn_endpoint);
    let h3_router = tonic_h3::server::H3Router::new(routes);

    let handle = tokio::spawn(async move {
        if let Err(e) = h3_router.serve(acceptor).await {
            eprintln!("QUIC gRPC server error: {e}");
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    QuicServerHandle {
        port,
        _handle: handle,
    }
}

#[cfg(feature = "quic")]
async fn make_quic_client(port: u16) -> Arc<GrpcStore> {
    let spec = GrpcSpec {
        instance_name: INSTANCE_NAME.to_string(),
        endpoints: vec![GrpcEndpoint {
            address: format!("https://127.0.0.1:{port}"),
            tls_config: None,
            concurrency_limit: None,
            connect_timeout_s: 5,
            tcp_keepalive_s: 0,
            http2_keepalive_interval_s: 0,
            http2_keepalive_timeout_s: 0,
            tcp_nodelay: true,
            use_http3: true,
        }],
        store_type: StoreType::Cas,
        retry: Retry::default(),
        max_concurrent_requests: 0,
        connections_per_endpoint: 32, // 32 QUIC connections = 32 ConnectionDrivers
        rpc_timeout_s: 120,
        batch_update_threshold_bytes: 1_048_576,
        max_concurrent_batch_rpcs: 8,
        parallel_chunk_read_threshold: 8 * 1024 * 1024,
        parallel_chunk_count: 64,
        dual_transport: false,
    };
    GrpcStore::new(&spec)
        .await
        .expect("failed to create QUIC GrpcStore client")
}

// ---------------------------------------------------------------------------
// Shared benchmark environment
// ---------------------------------------------------------------------------

async fn prepopulate_store(store_manager: &StoreManager, digest: &DigestInfo, data: &Bytes) {
    let store = store_manager.get_store("main_cas").expect("main_cas not found");
    store.update_oneshot(*digest, data.clone()).await.expect("failed to prepopulate");
}

struct BenchEnv {
    store_manager: Arc<StoreManager>,
    tcp_client: Arc<GrpcStore>,
    _tcp_server: TcpServerHandle,
    _certs: TlsCerts,
    #[cfg(feature = "quic")]
    quic_client: Arc<GrpcStore>,
    #[cfg(feature = "quic")]
    _quic_server: QuicServerHandle,
}

impl BenchEnv {
    async fn new() -> Self {
        // Install the TLS crypto provider before any TLS operations.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        let certs = generate_tls_certs();
        let store_manager = make_store_manager();
        let tcp_server = start_tcp_server(&store_manager, &certs).await;
        let tcp_client = make_tcp_client(tcp_server.port, &certs).await;

        #[cfg(feature = "quic")]
        let quic_server = start_quic_server(&store_manager, &certs).await;
        #[cfg(feature = "quic")]
        let quic_client = make_quic_client(quic_server.port).await;

        Self {
            store_manager,
            tcp_client,
            _tcp_server: tcp_server,
            _certs: certs,
            #[cfg(feature = "quic")]
            quic_client,
            #[cfg(feature = "quic")]
            _quic_server: quic_server,
        }
    }

    fn clients(&self) -> Vec<(&str, &Arc<GrpcStore>)> {
        let mut v = vec![("tcp", &self.tcp_client)];
        #[cfg(feature = "quic")]
        v.push(("quic", &self.quic_client));
        v
    }
}

// ---------------------------------------------------------------------------
// Benchmark: FindMissingBlobs latency
// ---------------------------------------------------------------------------

fn bench_find_missing_blobs(c: &mut Criterion) {
    let rt = make_runtime();
    let (env, digest) = rt.block_on(async {
        let env = BenchEnv::new().await;
        let (digest, data) = make_blob(1024);
        prepopulate_store(&env.store_manager, &digest, &data).await;
        (env, digest)
    });

    let mut group = c.benchmark_group("find_missing_blobs");

    for (transport, client) in env.clients() {
        group.bench_function(BenchmarkId::new(transport, "known"), |b| {
            b.to_async(&rt).iter(|| async {
                let key = nativelink_util::store_trait::StoreKey::from(digest);
                let mut results = [None];
                StoreDriver::has_with_results(
                    Pin::new(client.as_ref()),
                    &[key],
                    &mut results,
                )
                .await
                .expect("FindMissingBlobs failed");
                assert!(results[0].is_some());
            });
        });

        group.bench_function(BenchmarkId::new(transport, "missing"), |b| {
            let missing = DigestInfo::new([0xFFu8; 32], 999);
            b.to_async(&rt).iter(|| async {
                let key = nativelink_util::store_trait::StoreKey::from(missing);
                let mut results = [None];
                StoreDriver::has_with_results(
                    Pin::new(client.as_ref()),
                    &[key],
                    &mut results,
                )
                .await
                .expect("FindMissingBlobs failed");
                assert!(results[0].is_none());
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: ByteStream Write throughput
// ---------------------------------------------------------------------------

fn bench_bytestream_write(c: &mut Criterion) {
    let rt = make_runtime();
    let env = rt.block_on(BenchEnv::new());

    let sizes: &[(usize, &str)] = &[
        (1_000_000, "1MB"),
        (10_000_000, "10MB"),
        (100_000_000, "100MB"),
    ];

    let mut group = c.benchmark_group("bytestream_write");
    group.sample_size(10);

    for &(size, label) in sizes {
        let (digest, data) = make_blob(size);
        group.throughput(Throughput::Bytes(size as u64));

        for (transport, client) in env.clients() {
            group.bench_with_input(
                BenchmarkId::new(transport, label),
                &data,
                |b, data| {
                    b.to_async(&rt).iter(|| {
                        let client = client.clone();
                        let data = data.clone();
                        async move {
                            client
                                .update_oneshot(digest, data)
                                .await
                                .expect("ByteStream Write failed");
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: ByteStream Read throughput
// ---------------------------------------------------------------------------

fn bench_bytestream_read(c: &mut Criterion) {
    let rt = make_runtime();
    let env = rt.block_on(BenchEnv::new());

    let sizes: &[(usize, &str)] = &[
        (1_000_000, "1MB"),
        (10_000_000, "10MB"),
        (100_000_000, "100MB"),
    ];

    let digests: Vec<(DigestInfo, usize)> = rt.block_on(async {
        let mut digests = Vec::new();
        for &(size, _) in sizes {
            let (digest, data) = make_blob(size);
            prepopulate_store(&env.store_manager, &digest, &data).await;
            digests.push((digest, size));
        }
        digests
    });

    let mut group = c.benchmark_group("bytestream_read");
    group.sample_size(10);

    for (i, &(size, label)) in sizes.iter().enumerate() {
        let digest = digests[i].0;
        group.throughput(Throughput::Bytes(size as u64));

        for (transport, client) in env.clients() {
            group.bench_function(BenchmarkId::new(transport, label), |b| {
                b.to_async(&rt).iter(|| {
                    let client = client.clone();
                    async move {
                        let result = client
                            .get_part_unchunked(digest, 0, None)
                            .await
                            .expect("ByteStream Read failed");
                        assert_eq!(result.len(), size);
                    }
                });
            });
        }
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: BatchUpdateBlobs
// ---------------------------------------------------------------------------

fn bench_batch_update_blobs(c: &mut Criterion) {
    let rt = make_runtime();
    let env = rt.block_on(BenchEnv::new());

    let blob_count = 10;
    let blob_size = 100_000usize;
    let blobs: Vec<(DigestInfo, Bytes)> = (0..blob_count)
        .map(|i| {
            let data: Vec<u8> = (0..blob_size)
                .map(|j| ((i * blob_size + j) % 256) as u8)
                .collect();
            let hash = Sha256::digest(&data);
            let mut packed = [0u8; 32];
            packed.copy_from_slice(&hash);
            let digest = DigestInfo::new(packed, data.len() as u64);
            (digest, Bytes::from(data))
        })
        .collect();

    let total_bytes: u64 = blobs.iter().map(|(_, d)| d.len() as u64).sum();

    let mut group = c.benchmark_group("batch_update_blobs");
    group.throughput(Throughput::Bytes(total_bytes));
    group.sample_size(20);

    for (transport, client) in env.clients() {
        group.bench_function(BenchmarkId::new(transport, "10x100KB"), |b| {
            b.to_async(&rt).iter(|| {
                let client = client.clone();
                let blobs = blobs.clone();
                async move {
                    let futs: Vec<_> = blobs
                        .into_iter()
                        .map(|(digest, data)| {
                            let client = client.clone();
                            async move {
                                client
                                    .update_oneshot(digest, data)
                                    .await
                                    .expect("batch write failed");
                            }
                        })
                        .collect();
                    futures::future::join_all(futs).await;
                }
            });
        });
    }
    group.finish();
}

// ---------------------------------------------------------------------------
// Benchmark: Parallel concurrent reads
// ---------------------------------------------------------------------------

fn bench_parallel_reads(c: &mut Criterion) {
    let rt = make_runtime();
    let env = rt.block_on(BenchEnv::new());

    let blob_size = 10_000_000usize;
    let (digest, data) = make_blob(blob_size);
    rt.block_on(prepopulate_store(&env.store_manager, &digest, &data));

    let concurrencies: &[usize] = &[1, 4, 16, 64];

    // Atomic counters for max concurrent RPCs.
    let outstanding = Arc::new(AtomicU64::new(0));
    let max_outstanding = Arc::new(AtomicU64::new(0));

    let mut group = c.benchmark_group("parallel_reads");
    group.sample_size(10);

    for &concurrency in concurrencies {
        group.throughput(Throughput::Bytes(
            (blob_size as u64) * (concurrency as u64),
        ));

        for (transport, client) in env.clients() {
            // Reset counters for each transport × concurrency combination.
            outstanding.store(0, Ordering::Relaxed);
            max_outstanding.store(0, Ordering::Relaxed);

            let out = Arc::clone(&outstanding);
            let max_out = Arc::clone(&max_outstanding);

            group.bench_function(
                BenchmarkId::new(transport, format!("{concurrency}x10MB")),
                |b| {
                    b.to_async(&rt).iter(|| {
                        let client = client.clone();
                        let out = Arc::clone(&out);
                        let max_out = Arc::clone(&max_out);
                        async move {
                            let futs: Vec<_> = (0..concurrency)
                                .map(|_| {
                                    let client = client.clone();
                                    let out = Arc::clone(&out);
                                    let max_out = Arc::clone(&max_out);
                                    async move {
                                        let cur = out.fetch_add(1, Ordering::Relaxed) + 1;
                                        max_out.fetch_max(cur, Ordering::Relaxed);
                                        let result = client
                                            .get_part_unchunked(digest, 0, None)
                                            .await
                                            .expect("parallel read failed");
                                        out.fetch_sub(1, Ordering::Relaxed);
                                        assert_eq!(result.len(), blob_size);
                                    }
                                })
                                .collect();
                            futures::future::join_all(futs).await;
                        }
                    });
                },
            );

            let peak = max_outstanding.load(Ordering::Relaxed);
            eprintln!(
                "[CONCURRENCY] {transport} {concurrency}x10MB: max outstanding top-level RPCs = {peak}"
            );
        }
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_find_missing_blobs,
    bench_bytestream_write,
    bench_bytestream_read,
    bench_batch_update_blobs,
    bench_parallel_reads,
);
criterion_main!(benches);
