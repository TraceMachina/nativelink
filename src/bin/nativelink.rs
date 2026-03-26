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

use core::net::SocketAddr;
use core::time::Duration;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_lock::Mutex as AsyncMutex;
use axum::Router;
use axum::http::Uri;
use clap::Parser;
use futures::FutureExt;
use futures::future::{BoxFuture, Either, OptionFuture, TryFutureExt, try_join_all};
use hyper::StatusCode;
use hyper_util::rt::tokio::TokioIo;
use hyper_util::server::conn::auto;
use hyper_util::service::TowerToHyperService;
use mimalloc::MiMalloc;
use nativelink_config::cas_server::{
    CasConfig, GlobalConfig, HttpCompressionAlgorithm, ListenerConfig, SchedulerConfig,
    ServerConfig, StoreConfig, WorkerConfig,
};
use nativelink_config::stores::ConfigDigestHashFunction;
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_scheduler::default_scheduler_factory::scheduler_factory;
use nativelink_service::ac_server::AcServer;
use nativelink_service::bep_server::BepServer;
use nativelink_service::bytestream_server::ByteStreamServer;
use nativelink_service::capabilities_server::CapabilitiesServer;
use nativelink_service::cas_server::CasServer;
use nativelink_service::execution_server::ExecutionServer;
use nativelink_service::fetch_server::FetchServer;
use nativelink_service::health_server::HealthServer;
use nativelink_service::push_server::PushServer;
use nativelink_service::worker_api_server::WorkerApiServer;
use nativelink_util::blob_locality_map;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::common::fs::set_open_file_limit;
use nativelink_util::digest_hasher::{DigestHasherFunc, set_default_digest_hasher_func};
use nativelink_util::health_utils::HealthRegistryBuilder;
use nativelink_util::origin_event_publisher::OriginEventPublisher;
#[cfg(target_family = "unix")]
use nativelink_util::shutdown_guard::Priority;
use nativelink_util::shutdown_guard::ShutdownGuard;
use nativelink_util::store_trait::{
    DEFAULT_DIGEST_SIZE_HEALTH_CHECK_CFG, set_default_digest_size_health_check,
};
use nativelink_util::task::TaskExecutor;
use nativelink_util::telemetry::init_tracing;
use nativelink_util::{background_spawn, fs, spawn};
use nativelink_worker::local_worker::new_local_worker;
use rustls_pki_types::pem::PemObject;
use rustls_pki_types::{CertificateRevocationListDer, PrivateKeyDer};
use socket2::SockRef;
use tokio::net::TcpListener;
use tokio::select;
#[cfg(target_family = "unix")]
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot::Sender;
use tokio::sync::{broadcast, mpsc, oneshot};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::pki_types::CertificateDer;
use tokio_rustls::rustls::server::WebPkiClientVerifier;
use tokio_rustls::rustls::{RootCertStore, ServerConfig as TlsServerConfig};
use tonic::codec::CompressionEncoding;
use tonic::service::Routes;
#[cfg(feature = "quic")]
use {quinn, tonic_h3};
use tracing::{error, error_span, info, trace_span, warn};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Note: This must be kept in sync with the documentation in `AdminConfig::path`.
const DEFAULT_ADMIN_API_PATH: &str = "/admin";

// Note: This must be kept in sync with the documentation in `HealthConfig::path`.
const DEFAULT_HEALTH_STATUS_CHECK_PATH: &str = "/status";

// Note: This must be kept in sync with the documentation in
// `OriginEventsConfig::max_event_queue_size`.
const DEFAULT_MAX_QUEUE_EVENTS: usize = 0x0001_0000;

/// Broadcast Channel Capacity
/// Note: The actual capacity may be greater than the provided capacity.
const BROADCAST_CAPACITY: usize = 1;

/// Backend for bazel remote execution / cache API.
#[derive(Parser, Debug)]
#[clap(
    author = "Trace Machina, Inc. <nativelink@tracemachina.com>",
    version,
    about,
    long_about = None
)]
struct Args {
    /// Config file to use.
    #[clap(value_parser)]
    config_file: String,
}

trait RoutesExt {
    fn add_optional_service<S>(self, svc: Option<S>) -> Self
    where
        S: tower::Service<
                axum::http::Request<tonic::body::Body>,
                Error = core::convert::Infallible,
            > + tonic::server::NamedService
            + Clone
            + Send
            + Sync
            + 'static,
        S::Response: axum::response::IntoResponse,
        S::Future: Send + 'static;
}

impl RoutesExt for Routes {
    fn add_optional_service<S>(mut self, svc: Option<S>) -> Self
    where
        S: tower::Service<
                axum::http::Request<tonic::body::Body>,
                Error = core::convert::Infallible,
            > + tonic::server::NamedService
            + Clone
            + Send
            + Sync
            + 'static,
        S::Response: axum::response::IntoResponse,
        S::Future: Send + 'static,
    {
        if let Some(svc) = svc {
            self = self.add_service(svc);
        }
        self
    }
}

/// If this value changes update the documentation in the config definition.
const DEFAULT_MAX_DECODING_MESSAGE_SIZE: usize = 64 * 1024 * 1024;

/// Server-side encoding (response) limit.  Bazel's Java gRPC client defaults
/// to 4 MiB max inbound message size, so we default to 4 MiB.  Workers that
/// need larger responses should use a separate listener with a higher
/// `max_encoding_message_size` in the config.
const DEFAULT_MAX_ENCODING_MESSAGE_SIZE: usize = 4 * 1024 * 1024;

async fn inner_main(
    cfg: CasConfig,
    shutdown_tx: broadcast::Sender<ShutdownGuard>,
    scheduler_shutdown_tx: Sender<()>,
) -> Result<(), Error> {
    const fn into_encoding(from: HttpCompressionAlgorithm) -> Option<CompressionEncoding> {
        match from {
            HttpCompressionAlgorithm::Gzip => Some(CompressionEncoding::Gzip),
            HttpCompressionAlgorithm::Zstd => Some(CompressionEncoding::Zstd),
            HttpCompressionAlgorithm::None => None,
        }
    }

    let health_registry_builder =
        Arc::new(AsyncMutex::new(HealthRegistryBuilder::new("nativelink")));

    let store_manager = Arc::new(StoreManager::new());
    {
        let mut health_registry_lock = health_registry_builder.lock().await;

        for StoreConfig { name, spec } in cfg.stores {
            let health_component_name = format!("stores/{name}");
            let mut health_register_store =
                health_registry_lock.sub_builder(&health_component_name);
            let store = store_factory(&spec, &store_manager, Some(&mut health_register_store))
                .await
                .err_tip(|| format!("Failed to create store '{name}'"))?;
            store_manager.add_store(&name, store);
        }
    }

    let mut root_futures: Vec<BoxFuture<Result<(), Error>>> = Vec::new();

    let maybe_origin_event_tx = cfg
        .experimental_origin_events
        .as_ref()
        .map(|origin_events_cfg| {
            let mut max_queued_events = origin_events_cfg.max_event_queue_size;
            if max_queued_events == 0 {
                max_queued_events = DEFAULT_MAX_QUEUE_EVENTS;
            }
            let (tx, rx) = mpsc::channel(max_queued_events);
            let store_name = origin_events_cfg.publisher.store.as_str();
            let store = store_manager.get_store(store_name).err_tip(|| {
                format!("Could not get store {store_name} for origin event publisher")
            })?;

            root_futures.push(Box::pin(
                OriginEventPublisher::new(store, rx, shutdown_tx.clone())
                    .run()
                    .map(Ok),
            ));

            Ok::<_, Error>(tx)
        })
        .transpose()?;

    // Create a shared blob locality map for peer-to-peer blob sharing.
    // This map is shared between the scheduler (for locality scoring and
    // peer hint generation) and WorkerApiServer (for receiving
    // BlobsAvailable updates from workers).
    let locality_map = blob_locality_map::new_shared_blob_locality_map();

    let mut action_schedulers = HashMap::new();
    let mut worker_schedulers = HashMap::new();
    for SchedulerConfig { name, spec } in cfg.schedulers.iter().flatten() {
        let (maybe_action_scheduler, maybe_worker_scheduler) =
            scheduler_factory(spec, &store_manager, maybe_origin_event_tx.as_ref(), Some(locality_map.clone()))
                .await
                .err_tip(|| format!("Failed to create scheduler '{name}'"))?;
        if let Some(action_scheduler) = maybe_action_scheduler {
            action_schedulers.insert(name.clone(), action_scheduler.clone());
        }
        if let Some(worker_scheduler) = maybe_worker_scheduler {
            worker_schedulers.insert(name.clone(), worker_scheduler.clone());
        }
    }

    let server_cfgs: Vec<ServerConfig> = cfg.servers.into_iter().collect();

    // Periodically log tokio runtime metrics to detect thread pool exhaustion.
    // Requires tokio_unstable cfg for blocking thread metrics.
    #[cfg(tokio_unstable)]
    {
        let metrics_handle = tokio::runtime::Handle::current();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(10));
            loop {
                interval.tick().await;
                let metrics = metrics_handle.metrics();
                let workers = metrics.num_workers();
                let blocking_threads = metrics.num_blocking_threads();
                let idle_blocking = metrics.num_idle_blocking_threads();
                let blocking_depth = metrics.blocking_queue_depth();
                if blocking_depth > 0 || (blocking_threads > 0 && idle_blocking == 0) {
                    warn!(
                        workers,
                        blocking_threads,
                        idle_blocking,
                        blocking_queue_depth = blocking_depth,
                        "tokio thread pool pressure detected"
                    );
                }
            }
        });
    }

    // Wrap CAS stores with WorkerProxyStore so the server can proxy reads
    // to workers that have the blob (discovered via BlobsAvailable reports).
    let cas_store_names: HashSet<String> = {
        let mut names: HashSet<String> = HashSet::new();
        for server_cfg in &server_cfgs {
            if let Some(ref services) = server_cfg.services {
                if let Some(ref cas_cfgs) = services.cas {
                    for c in cas_cfgs {
                        names.insert(c.config.cas_store.clone());
                    }
                }
                if let Some(ref bs_cfgs) = services.bytestream {
                    for c in bs_cfgs {
                        names.insert(c.config.cas_store.clone());
                    }
                }
            }
        }
        for store_name in &names {
            if let Some(original_store) = store_manager.get_store(store_name) {
                let proxy_store = nativelink_util::store_trait::Store::new(
                    nativelink_store::worker_proxy_store::WorkerProxyStore::new(
                        original_store,
                        locality_map.clone(),
                    ),
                );
                store_manager.add_store(store_name, proxy_store);
                info!(
                    store_name,
                    "Wrapped CAS store with WorkerProxyStore for peer blob sharing"
                );
            }
        }
        names
    };

    // Spawn the BlobsInStableStorage batching loop. Every 100ms it drains
    // digests that completed their write to the slow store (FilesystemStore)
    // in each CAS FastSlowStore and broadcasts them to all connected workers
    // so they can unpin those blobs from their local CAS.
    if !worker_schedulers.is_empty() {
        let cas_stores: Vec<nativelink_util::store_trait::Store> = cas_store_names
            .iter()
            .filter_map(|name| store_manager.get_store(name))
            .collect();
        let schedulers: Vec<Arc<dyn nativelink_scheduler::worker_scheduler::WorkerScheduler>> =
            worker_schedulers.values().cloned().collect();

        if !cas_stores.is_empty() {
            let cas_store_count = cas_stores.len();
            let scheduler_count = schedulers.len();
            background_spawn!("blobs_in_stable_storage_loop", async move {
                let mut interval = tokio::time::interval(Duration::from_millis(100));
                loop {
                    interval.tick().await;
                    let mut all_digests = Vec::new();
                    for store in &cas_stores {
                        let mut drained = store.drain_stable_digests();
                        if !drained.is_empty() {
                            all_digests.append(&mut drained);
                        }
                    }
                    if all_digests.is_empty() {
                        continue;
                    }
                    for scheduler in &schedulers {
                        scheduler
                            .broadcast_blobs_in_stable_storage(all_digests.clone())
                            .await;
                    }
                }
            });
            info!(
                cas_store_count,
                scheduler_count,
                "started BlobsInStableStorage batching loop (100ms interval)"
            );
        }
    }

    for server_cfg in server_cfgs {
        let services = server_cfg
            .services
            .err_tip(|| "'services' must be configured")?;

        // Extract message size limits from the listener config.
        // Both HTTP and HTTP3 listeners support these; HTTP also has compression.
        let (max_decode, max_encode) = match &server_cfg.listener {
            ListenerConfig::Http(http) => (http.max_decoding_message_size, http.max_encoding_message_size),
            ListenerConfig::Http3(h3) => (h3.max_decoding_message_size, h3.max_encoding_message_size),
        };
        let max_decoding = if max_decode == 0 { DEFAULT_MAX_DECODING_MESSAGE_SIZE } else { max_decode };
        let max_encoding = if max_encode == 0 { DEFAULT_MAX_ENCODING_MESSAGE_SIZE } else { max_encode };

        // Helper to configure a tonic service with message size limits and
        // optional compression from the HTTP listener config.
        macro_rules! svc_setup {
            ($v:expr) => {{
                let mut service = $v.into_service();
                service = service.max_decoding_message_size(max_decoding);
                service = service.max_encoding_message_size(max_encoding);
                if let ListenerConfig::Http(ref http_config) = server_cfg.listener {
                    let send_algo = &http_config.compression.send_compression_algorithm;
                    if let Some(encoding) = into_encoding(send_algo.unwrap_or(HttpCompressionAlgorithm::None)) {
                        service = service.send_compressed(encoding);
                    }
                    for encoding in http_config.compression.accepted_compression_algorithms.iter()
                        .filter_map(|from: &HttpCompressionAlgorithm| into_encoding(*from))
                    {
                        service = service.accept_compressed(encoding);
                    }
                }
                service
            }};
        }

        let execution_server = services
            .execution
            .as_ref()
            .map(|cfg| ExecutionServer::new(cfg, &action_schedulers, &store_manager))
            .transpose()
            .err_tip(|| "Could not create Execution service")?;

        let tonic_services = Routes::builder()
            .routes()
            .add_optional_service(
                services
                    .ac
                    .map_or(Ok(None), |cfg| {
                        AcServer::new(&cfg, &store_manager)
                            .map(|v| Some(svc_setup!(v)))
                    })
                    .err_tip(|| "Could not create AC service")?,
            )
            .add_optional_service(
                services
                    .cas
                    .map_or(Ok(None), |cfg| {
                        CasServer::new(&cfg, &store_manager)
                            .map(|v| Some(svc_setup!(v)))
                    })
                    .err_tip(|| "Could not create CAS service")?,
            )
            .add_optional_service(
                execution_server
                    .clone()
                    .map(|v| svc_setup!(v)),
            )
            .add_optional_service(
                execution_server.map(|v| {
                    let mut service = v.into_operations_service();
                    service = service.max_decoding_message_size(max_decoding);
                    service = service.max_encoding_message_size(max_encoding);
                    if let ListenerConfig::Http(ref http_config) = server_cfg.listener {
                        let send_algo = &http_config.compression.send_compression_algorithm;
                        if let Some(encoding) = into_encoding(send_algo.unwrap_or(HttpCompressionAlgorithm::None)) {
                            service = service.send_compressed(encoding);
                        }
                        for encoding in http_config.compression.accepted_compression_algorithms.iter()
                            .filter_map(|from: &HttpCompressionAlgorithm| into_encoding(*from))
                        {
                            service = service.accept_compressed(encoding);
                        }
                    }
                    service
                }),
            )
            .add_optional_service(
                services
                    .fetch
                    .map_or(Ok(None), |cfg| {
                        FetchServer::new(&cfg, &store_manager)
                            .map(|v| Some(svc_setup!(v)))
                    })
                    .err_tip(|| "Could not create Fetch service")?,
            )
            .add_optional_service(
                services
                    .push
                    .map_or(Ok(None), |cfg| {
                        PushServer::new(&cfg, &store_manager)
                            .map(|v| Some(svc_setup!(v)))
                    })
                    .err_tip(|| "Could not create Push service")?,
            )
            .add_optional_service(
                services
                    .bytestream
                    .map_or(Ok(None), |cfg| {
                        ByteStreamServer::new(&cfg, &store_manager)
                            .map(|v| Some(svc_setup!(v)))
                    })
                    .err_tip(|| "Could not create ByteStream service")?,
            )
            .add_optional_service(
                OptionFuture::from(
                    services
                        .capabilities
                        .as_ref()
                        .map(|cfg| CapabilitiesServer::new(cfg, &action_schedulers)),
                )
                .await
                .map_or(Ok::<Option<CapabilitiesServer>, Error>(None), |server| {
                    Ok(Some(server?))
                })
                .err_tip(|| "Could not create Capabilities service")?
                .map(|v| svc_setup!(v)),
            )
            .add_optional_service(
                services
                    .worker_api
                    .map_or(Ok(None), |cfg| {
                        WorkerApiServer::new(&cfg, &worker_schedulers, Some(locality_map.clone()))
                            .map(|v| Some(svc_setup!(v)))
                    })
                    .err_tip(|| "Could not create WorkerApi service")?,
            )
            .add_optional_service(
                services
                    .experimental_bep
                    .map_or(Ok(None), |cfg| {
                        BepServer::new(&cfg, &store_manager)
                            .map(|v| Some(svc_setup!(v)))
                    })
                    .err_tip(|| "Could not create BEP service")?,
            );

        let health_registry = health_registry_builder.lock().await.build();

        match server_cfg.listener {
        ListenerConfig::Http(http_config) => {
        let mut svc =
            tonic_services
                .into_axum_router()
                .layer(nativelink_util::telemetry::OtlpLayer::new(
                    server_cfg.experimental_identity_header.required,
                ));

        if let Some(health_cfg) = services.health {
            let path = if health_cfg.path.is_empty() {
                DEFAULT_HEALTH_STATUS_CHECK_PATH
            } else {
                &health_cfg.path
            };
            svc = svc.route_service(path, HealthServer::new(health_registry, &health_cfg));
        }

        if let Some(admin_config) = services.admin {
            let path = if admin_config.path.is_empty() {
                DEFAULT_ADMIN_API_PATH
            } else {
                &admin_config.path
            };
            let worker_schedulers = Arc::new(worker_schedulers.clone());
            svc = svc.nest_service(
                path,
                Router::new().route(
                    "/scheduler/{instance_name}/set_drain_worker/{worker_id}/{is_draining}",
                    axum::routing::post(
                        move |params: axum::extract::Path<(String, String, String)>| async move {
                            let (instance_name, worker_id, is_draining) = params.0;
                            (async move {
                                let is_draining = match is_draining.as_str() {
                                    "0" => false,
                                    "1" => true,
                                    _ => {
                                        return Err(make_err!(
                                            Code::Internal,
                                            "{} is neither 0 nor 1",
                                            is_draining
                                        ));
                                    }
                                };
                                worker_schedulers
                                    .get(&instance_name)
                                    .err_tip(|| {
                                        format!(
                                            "Can not get an instance with the name of '{}'",
                                            &instance_name
                                        )
                                    })?
                                    .clone()
                                    .set_drain_worker(&worker_id.clone().into(), is_draining)
                                    .await?;
                                Ok::<_, Error>(format!("Draining worker {worker_id}"))
                            })
                            .await
                            .map_err(|e| {
                                Err::<String, _>((
                                    StatusCode::INTERNAL_SERVER_ERROR,
                                    format!("Error: {e:?}"),
                                ))
                            })
                        },
                    ),
                ),
            );
        }

        // This is the default service that executes if no other endpoint matches.
        svc = svc.fallback(|uri: Uri| async move {
            warn!("No route for {uri}");
            (StatusCode::NOT_FOUND, format!("No route for {uri}"))
        });
        // Reject startup if require_tls is set but no TLS config is provided.
        if http_config.require_tls && http_config.tls.is_none() {
            return Err(make_input_err!(
                "Listener '{}' on {} has require_tls=true but no TLS configuration. \
                 Either add a tls block or set require_tls to false",
                server_cfg.name,
                http_config.socket_address
            ));
        }

        // Configure our TLS acceptor if we have TLS configured.
        let maybe_tls_acceptor = http_config.tls.map_or(Ok(None), |tls_config| {
            fn read_cert(cert_file: &str) -> Result<Vec<CertificateDer<'static>>, Error> {
                let mut cert_reader = std::io::BufReader::new(
                    std::fs::File::open(cert_file)
                        .err_tip(|| format!("Could not open cert file {cert_file}"))?,
                );
                let certs = CertificateDer::pem_reader_iter(&mut cert_reader)
                    .collect::<Result<Vec<CertificateDer<'_>>, _>>()
                    .err_tip(|| format!("Could not extract certs from file {cert_file}"))?;
                Ok(certs)
            }
            let certs = read_cert(&tls_config.cert_file)?;
            let mut key_reader = std::io::BufReader::new(
                std::fs::File::open(&tls_config.key_file)
                    .err_tip(|| format!("Could not open key file {}", tls_config.key_file))?,
            );
            let key = match PrivateKeyDer::from_pem_reader(&mut key_reader)
                .err_tip(|| format!("Could not extract key(s) from file {}", tls_config.key_file))?
            {
                PrivateKeyDer::Pkcs8(key) => key.into(),
                PrivateKeyDer::Sec1(key) => key.into(),
                PrivateKeyDer::Pkcs1(key) => key.into(),
                _ => {
                    return Err(make_err!(
                        Code::Internal,
                        "No keys found in file {}",
                        tls_config.key_file
                    ));
                }
            };
            if PrivateKeyDer::from_pem_reader(&mut key_reader).is_ok() {
                return Err(make_err!(
                    Code::InvalidArgument,
                    "Expected 1 key in file {}",
                    tls_config.key_file
                ));
            }
            let verifier = if let Some(client_ca_file) = &tls_config.client_ca_file {
                let mut client_auth_roots = RootCertStore::empty();
                for cert in read_cert(client_ca_file)? {
                    client_auth_roots.add(cert).map_err(|e| {
                        make_err!(Code::Internal, "Could not read client CA: {e:?}")
                    })?;
                }
                let crls = if let Some(client_crl_file) = &tls_config.client_crl_file {
                    let mut crl_reader = std::io::BufReader::new(
                        std::fs::File::open(client_crl_file)
                            .err_tip(|| format!("Could not open CRL file {client_crl_file}"))?,
                    );
                    CertificateRevocationListDer::pem_reader_iter(&mut crl_reader)
                        .collect::<Result<_, _>>()
                        .err_tip(|| format!("Could not extract CRLs from file {client_crl_file}"))?
                } else {
                    Vec::new()
                };
                WebPkiClientVerifier::builder(Arc::new(client_auth_roots))
                    .with_crls(crls)
                    .build()
                    .map_err(|e| {
                        make_err!(
                            Code::Internal,
                            "Could not create WebPkiClientVerifier: {e:?}"
                        )
                    })?
            } else {
                WebPkiClientVerifier::no_client_auth()
            };
            let mut config = TlsServerConfig::builder_with_provider(
                    tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().into(),
                )
                .with_safe_default_protocol_versions()
                .map_err(|e| make_err!(Code::Internal, "TLS version error: {e:?}"))?
                .with_client_cert_verifier(verifier)
                .with_single_cert(certs, key)
                .map_err(|e| {
                    make_err!(Code::Internal, "Could not create TlsServerConfig : {e:?}")
                })?;

            config.alpn_protocols.push("h2".into());
            Ok(Some(TlsAcceptor::from(Arc::new(config))))
        })?;

        let socket_addr = http_config
            .socket_address
            .parse::<SocketAddr>()
            .map_err(|e| {
                make_input_err!("Invalid address '{}' - {e:?}", http_config.socket_address)
            })?;
        let tcp_listener = TcpListener::bind(&socket_addr).await?;
        let mut http = auto::Builder::new(TaskExecutor::default());

        let http_config = &http_config.advanced_http;
        if let Some(value) = http_config.http2_keep_alive_interval {
            http.http2()
                .keep_alive_interval(Duration::from_secs(u64::from(value)));
        }

        if let Some(value) = http_config.experimental_http2_max_pending_accept_reset_streams {
            http.http2()
                .max_pending_accept_reset_streams(usize::try_from(value).err_tip(
                    || "Could not convert experimental_http2_max_pending_accept_reset_streams",
                )?);
        }
        // Default to 16 MiB stream window and 128 MiB connection window
        // to avoid capping per-stream throughput at ~64 MB/s with 1ms RTT
        // (hyper's default of 64 KiB is too small for high-bandwidth links).
        http.http2().initial_stream_window_size(
            http_config
                .experimental_http2_initial_stream_window_size
                .unwrap_or(16 * 1024 * 1024),
        );
        http.http2().initial_connection_window_size(
            http_config
                .experimental_http2_initial_connection_window_size
                .unwrap_or(128 * 1024 * 1024),
        );
        if let Some(value) = http_config.experimental_http2_adaptive_window {
            http.http2().adaptive_window(value);
        }
        http.http2().max_frame_size(
            http_config
                .experimental_http2_max_frame_size
                .unwrap_or(64 * 1024),
        );
        if let Some(value) = http_config.experimental_http2_max_concurrent_streams {
            http.http2().max_concurrent_streams(value);
        }
        if let Some(value) = http_config.experimental_http2_keep_alive_timeout {
            http.http2()
                .keep_alive_timeout(Duration::from_secs(u64::from(value)));
        }
        http.http2().max_send_buf_size(
            usize::try_from(
                http_config
                    .experimental_http2_max_send_buf_size
                    .unwrap_or(2 * 1024 * 1024),
            )
            .err_tip(|| "Could not convert http2_max_send_buf_size")?,
        );
        if http_config.experimental_http2_enable_connect_protocol == Some(true) {
            http.http2().enable_connect_protocol();
        }
        if let Some(value) = http_config.experimental_http2_max_header_list_size {
            http.http2().max_header_list_size(value);
        }
        info!("Ready, listening on {socket_addr}",);
        root_futures.push(Box::pin(async move {
            loop {
                select! {
                    accept_result = tcp_listener.accept() => {
                        match accept_result {
                            Ok((tcp_stream, remote_addr)) => {
                                // Disable Nagle's algorithm to reduce latency
                                // on small writes (e.g., gRPC frames).
                                if let Err(err) = tcp_stream.set_nodelay(true) {
                                    error!(
                                        target: "nativelink::services",
                                        ?err,
                                        "Failed to set TCP_NODELAY"
                                    );
                                }
                                // Enable TCP keepalive to detect dead connections.
                                // Uses system defaults (tcp_keepalive_time/intvl/probes).
                                let sock_ref = SockRef::from(&tcp_stream);
                                if let Err(err) = sock_ref.set_keepalive(true) {
                                    error!(
                                        target: "nativelink::services",
                                        ?err,
                                        "Failed to set SO_KEEPALIVE"
                                    );
                                }
                                // Set large socket buffers for 10 GbE throughput.
                                // BDP = 1.25 GB/s × 0.5ms RTT = 625 KB; 4 MiB
                                // provides headroom for bursts. Linux doubles the
                                // value internally for bookkeeping.
                                const SOCKET_BUF_SIZE: usize = 4 * 1024 * 1024;
                                if let Err(err) = sock_ref.set_send_buffer_size(SOCKET_BUF_SIZE) {
                                    error!(
                                        target: "nativelink::services",
                                        ?err,
                                        "Failed to set SO_SNDBUF"
                                    );
                                }
                                if let Err(err) = sock_ref.set_recv_buffer_size(SOCKET_BUF_SIZE) {
                                    error!(
                                        target: "nativelink::services",
                                        ?err,
                                        "Failed to set SO_RCVBUF"
                                    );
                                }
                                info!(
                                    target: "nativelink::services",
                                    ?remote_addr,
                                    ?socket_addr,
                                    "Client connected"
                                );

                                let (http, svc, maybe_tls_acceptor) =
                                    (http.clone(), svc.clone(), maybe_tls_acceptor.clone());

                                background_spawn!(
                                    name: "http_connection",
                                    fut: error_span!(
                                        "http_connection",
                                        remote_addr = %remote_addr,
                                        socket_addr = %socket_addr,
                                    ).in_scope(|| async move {
                                        let serve_connection = if let Some(tls_acceptor) = maybe_tls_acceptor {
                                            match tls_acceptor.accept(tcp_stream).await {
                                                Ok(tls_stream) => Either::Left(http.serve_connection(
                                                    TokioIo::new(tls_stream),
                                                    TowerToHyperService::new(svc),
                                                )),
                                                Err(err) => {
                                                    error!(?err, "Failed to accept tls stream");
                                                    return;
                                                }
                                            }
                                        } else {
                                            Either::Right(http.serve_connection(
                                                TokioIo::new(tcp_stream),
                                                TowerToHyperService::new(svc),
                                            ))
                                        };

                                        if let Err(err) = serve_connection.await {
                                            // Walk the error source chain looking
                                            // for a std::io::Error so we can
                                            // downgrade normal connection-close
                                            // events to info level.
                                            let is_conn_close = {
                                                let mut cur: Option<&(dyn std::error::Error + 'static)> = Some(err.as_ref());
                                                let mut found = false;
                                                while let Some(e) = cur {
                                                    if let Some(io_err) = e.downcast_ref::<std::io::Error>() {
                                                        found = matches!(
                                                            io_err.kind(),
                                                            std::io::ErrorKind::BrokenPipe
                                                            | std::io::ErrorKind::ConnectionReset
                                                            | std::io::ErrorKind::ConnectionAborted
                                                        );
                                                        break;
                                                    }
                                                    cur = e.source();
                                                }
                                                found
                                            };
                                            if is_conn_close {
                                                info!(
                                                    target: "nativelink::services",
                                                    ?err,
                                                    "client disconnected"
                                                );
                                            } else {
                                                error!(
                                                    target: "nativelink::services",
                                                    ?err,
                                                    "Failed running service"
                                                );
                                            }
                                        }
                                    }),
                                    target: "nativelink::services",
                                    ?remote_addr,
                                    ?socket_addr,
                                );
                            },
                            Err(err) => {
                                error!(?err, "Failed to accept tcp connection");
                            }
                        }
                    },
                }
            }
            // Unreachable
        }));
        } // end ListenerConfig::Http

        #[cfg(feature = "quic")]
        ListenerConfig::Http3(h3_config) => {
            let socket_addr = h3_config
                .socket_address
                .parse::<SocketAddr>()
                .map_err(|e| {
                    make_input_err!("Invalid address '{}' - {e:?}", h3_config.socket_address)
                })?;

            // Load TLS cert + key for QUIC (TLS 1.3 is mandatory).
            let cert_pem = std::fs::read(&h3_config.cert_file)
                .err_tip(|| format!("Could not read cert file {}", h3_config.cert_file))?;
            let key_pem = std::fs::read(&h3_config.key_file)
                .err_tip(|| format!("Could not read key file {}", h3_config.key_file))?;

            let certs: Vec<CertificateDer<'static>> =
                CertificateDer::pem_reader_iter(&mut &cert_pem[..])
                    .collect::<Result<_, _>>()
                    .err_tip(|| "Could not parse PEM certs for QUIC")?;
            let key = PrivateKeyDer::from_pem_reader(&mut &key_pem[..])
                .err_tip(|| "Could not parse PEM key for QUIC")?;

            use tokio_rustls::rustls as rustls;

            fn read_cert_quic(cert_file: &str) -> Result<Vec<CertificateDer<'static>>, Error> {
                let mut cert_reader = std::io::BufReader::new(
                    std::fs::File::open(cert_file)
                        .err_tip(|| format!("Could not open cert file {cert_file}"))?,
                );
                let certs = CertificateDer::pem_reader_iter(&mut cert_reader)
                    .collect::<Result<Vec<CertificateDer<'_>>, _>>()
                    .err_tip(|| format!("Could not extract certs from file {cert_file}"))?;
                Ok(certs)
            }

            // WebPkiClientVerifier::builder() needs a process-level crypto provider.
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
            let verifier = if let Some(client_ca_file) = &h3_config.client_ca_file {
                let mut client_auth_roots = RootCertStore::empty();
                for cert in read_cert_quic(client_ca_file)? {
                    client_auth_roots.add(cert).map_err(|e| {
                        make_err!(Code::Internal, "Could not read QUIC client CA: {e:?}")
                    })?;
                }
                WebPkiClientVerifier::builder(Arc::new(client_auth_roots))
                    .build()
                    .map_err(|e| {
                        make_err!(
                            Code::Internal,
                            "Could not create QUIC WebPkiClientVerifier: {e:?}"
                        )
                    })?
            } else {
                WebPkiClientVerifier::no_client_auth()
            };

            let mut tls_config = rustls::ServerConfig::builder_with_provider(
                rustls::crypto::aws_lc_rs::default_provider().into(),
            )
            .with_safe_default_protocol_versions()
            .map_err(|e| make_err!(Code::Internal, "QUIC TLS version error: {e:?}"))?
            .with_client_cert_verifier(verifier)
            .with_single_cert(certs, key)
            .map_err(|e| make_err!(Code::Internal, "QUIC TLS config error: {e:?}"))?;
            tls_config.alpn_protocols = vec![b"h3".to_vec()];
            tls_config.max_early_data_size = u32::MAX;

            let mut quic_server_config = quinn::ServerConfig::with_crypto(Arc::new(
                quinn::crypto::rustls::QuicServerConfig::try_from(Arc::new(tls_config))
                    .map_err(|e| make_err!(Code::Internal, "Quinn server config error: {e:?}"))?,
            ));

            // Tune QUIC transport for 10 GbE LAN (~0.5ms RTT).
            // BDP = 1.25 GB/s × 0.5ms ≈ 625 KB. Use generous windows to
            // handle bursts and multiple concurrent streams.
            let mut transport = quinn::TransportConfig::default();
            transport.stream_receive_window((16 * 1024 * 1024u32).into()); // 16 MiB per stream (vs 1 MiB)
            transport.receive_window((128 * 1024 * 1024u32).into()); // 128 MiB connection (vs 24 MiB)
            transport.send_window(128 * 1024 * 1024); // 128 MiB (vs 24 MiB)
            transport.max_concurrent_bidi_streams(1024u32.into()); // vs 256
            transport.max_concurrent_uni_streams(1024u32.into());
            transport.initial_rtt(Duration::from_micros(500)); // 0.5ms LAN RTT (vs 333ms)
            // Reduce ACK delay from default 25ms to 5ms for LAN.
            // 1ms caused H3_FRAME_ERROR from BBR pacing instability.
            let mut ack_freq = quinn::AckFrequencyConfig::default();
            ack_freq.max_ack_delay(Some(Duration::from_millis(5)));
            transport.ack_frequency_config(Some(ack_freq));
            transport.max_idle_timeout(Some(Duration::from_secs(30).try_into().unwrap()));
            // BBR handles bursty workloads better than Cubic on high-BDP LAN.
            transport.congestion_controller_factory(Arc::new(
                quinn::congestion::BbrConfig::default(),
            ));
            quic_server_config.transport_config(Arc::new(transport));

            // Pre-create UDP socket with large buffers for 10 GbE.
            // quinn-udp defaults to ~2 MiB; we want 8 MiB for burst absorption.
            let udp_socket = std::net::UdpSocket::bind(socket_addr)
                .map_err(|e| make_err!(Code::Internal, "QUIC UDP bind on {socket_addr}: {e:?}"))?;
            {
                const QUIC_UDP_BUF: usize = 8 * 1024 * 1024;
                let sock_ref = socket2::SockRef::from(&udp_socket);
                if let Err(err) = sock_ref.set_send_buffer_size(QUIC_UDP_BUF) {
                    warn!(?err, "Failed to set QUIC SO_SNDBUF");
                }
                if let Err(err) = sock_ref.set_recv_buffer_size(QUIC_UDP_BUF) {
                    warn!(?err, "Failed to set QUIC SO_RCVBUF");
                }
            }

            let quinn_endpoint = quinn::Endpoint::new(
                quinn::EndpointConfig::default(),
                Some(quic_server_config),
                udp_socket,
                quinn::default_runtime().ok_or_else(|| {
                    make_err!(Code::Internal, "No async runtime for QUIC endpoint")
                })?,
            )
            .map_err(|e| make_err!(Code::Internal, "Failed to create QUIC endpoint: {e:?}"))?;

            // Build tonic Routes from the same services.
            let routes = tonic_services;
            let acceptor = tonic_h3::quinn::H3QuinnAcceptor::new(quinn_endpoint.clone());
            let h3_router = tonic_h3::server::H3Router::new(routes);

            info!("Ready, listening on {socket_addr} (QUIC/HTTP3)");
            root_futures.push(Box::pin(async move {
                if let Err(err) = h3_router.serve(acceptor).await {
                    error!(?err, "QUIC/HTTP3 server error");
                }
                Ok(())
            }));
        }

        #[cfg(not(feature = "quic"))]
        ListenerConfig::Http3(_) => {
            return Err(make_err!(
                Code::InvalidArgument,
                "HTTP3/QUIC listener configured but the 'quic' feature is not enabled. \
                 Rebuild with: cargo build --features quic"
            ));
        }
        } // end match server_cfg.listener
    }

    {
        // We start workers after our TcpListener is setup so if our worker connects to one
        // of these services it will be able to connect.
        let worker_cfgs = cfg.workers.unwrap_or_default();
        let mut worker_names = HashSet::with_capacity(worker_cfgs.len());
        for (i, worker_cfg) in worker_cfgs.into_iter().enumerate() {
            let spawn_fut = match worker_cfg {
                WorkerConfig::Local(local_worker_cfg) => {
                    let fast_slow_store = store_manager
                        .get_store(&local_worker_cfg.cas_fast_slow_store)
                        .err_tip(|| {
                            format!(
                                "Failed to find store for cas_store_ref in worker config : {}",
                                local_worker_cfg.cas_fast_slow_store
                            )
                        })?;

                    let maybe_ac_store = if let Some(ac_store_ref) =
                        &local_worker_cfg.upload_action_result.ac_store
                    {
                        Some(store_manager.get_store(ac_store_ref).err_tip(|| {
                            format!("Failed to find store for ac_store in worker config : {ac_store_ref}")
                        })?)
                    } else {
                        None
                    };
                    // Note: Defaults to fast_slow_store if not specified. If this ever changes it must
                    // be updated in config documentation for the `historical_results_store` the field.
                    let historical_store = if let Some(cas_store_ref) = &local_worker_cfg
                        .upload_action_result
                        .historical_results_store
                    {
                        store_manager.get_store(cas_store_ref).err_tip(|| {
                                format!(
                                "Failed to find store for historical_results_store in worker config : {cas_store_ref}"
                            )
                            })?
                    } else {
                        fast_slow_store.clone()
                    };
                    let local_worker = new_local_worker(
                        Arc::new(local_worker_cfg),
                        fast_slow_store,
                        maybe_ac_store,
                        historical_store,
                    )
                    .await
                    .err_tip(|| "Could not make LocalWorker")?;

                    let name = if local_worker.name().is_empty() {
                        format!("worker_{i}")
                    } else {
                        local_worker.name().clone()
                    };

                    if worker_names.contains(&name) {
                        Err(make_input_err!(
                            "Duplicate worker name '{}' found in config",
                            name
                        ))?;
                    }
                    worker_names.insert(name.clone());
                    let shutdown_rx = shutdown_tx.subscribe();
                    let fut = trace_span!("worker_ctx", worker_name = %name)
                        .in_scope(|| local_worker.run(shutdown_rx));
                    spawn!("worker", fut, ?name)
                }
            };
            root_futures.push(Box::pin(spawn_fut.map_ok_or_else(|e| Err(e.into()), |v| v)));
        }
    }

    // Set up a shutdown handler for the worker schedulers.
    let mut shutdown_rx = shutdown_tx.subscribe();
    root_futures.push(Box::pin(async move {
        if let Ok(shutdown_guard) = shutdown_rx.recv().await {
            let _ = scheduler_shutdown_tx.send(());
            for (_name, scheduler) in worker_schedulers {
                scheduler.shutdown(shutdown_guard.clone()).await;
            }
        }
        Ok(())
    }));

    if let Err(e) = try_join_all(root_futures).await {
        panic!("{e:?}");
    }

    Ok(())
}

fn get_config() -> Result<CasConfig, Error> {
    let args = Args::parse();
    CasConfig::try_from_json5_file(&args.config_file)
}

/// Dump all thread stacks to a timestamped file for post-mortem analysis.
/// Reads /proc/self/task/*/comm, status, wchan, and stack (if permitted).
fn dump_thread_stacks() {
    nativelink_util::stall_detector::dump_thread_stacks("runtime-watchdog");
}

/// Sets the current thread's QoS class to USER_INITIATED on macOS so the
/// kernel prefers scheduling on performance cores instead of efficiency cores.
#[cfg(target_os = "macos")]
fn set_qos_user_initiated() {
    const QOS_CLASS_USER_INITIATED: u32 = 0x19;
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
    }
    let ret = unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INITIATED, 0) };
    if ret != 0 {
        eprintln!("warning: failed to set QoS to USER_INITIATED: {ret}");
    }
}

#[cfg(not(target_os = "macos"))]
fn set_qos_user_initiated() {}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    // Install the rustls crypto provider early so WebPkiClientVerifier::builder()
    // and other rustls APIs that need a process-level provider can find it.
    let _ = tokio_rustls::rustls::crypto::aws_lc_rs::default_provider().install_default();

    // Set QoS before runtime creation so tokio worker threads inherit
    // P-core scheduling preference via pthread_create QoS inheritance.
    set_qos_user_initiated();

    #[expect(clippy::disallowed_methods, reason = "starting main runtime")]
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .on_thread_start(set_qos_user_initiated)
        .enable_all()
        .build()?;

    // Parse config before tracing init so we can read disable_otlp.
    let mut cfg = get_config()?;

    let global_cfg = if let Some(global_cfg) = &mut cfg.global {
        if global_cfg.max_open_files == 0 {
            global_cfg.max_open_files = fs::DEFAULT_OPEN_FILE_LIMIT;
        }
        if global_cfg.default_digest_size_health_check == 0 {
            global_cfg.default_digest_size_health_check = DEFAULT_DIGEST_SIZE_HEALTH_CHECK_CFG;
        }

        *global_cfg
    } else {
        GlobalConfig {
            max_open_files: fs::DEFAULT_OPEN_FILE_LIMIT,
            default_digest_hash_function: None,
            default_digest_size_health_check: DEFAULT_DIGEST_SIZE_HEALTH_CHECK_CFG,
            pprof_port: 0,
            disable_otlp: true,
            nonblocking_log: true,
        }
    };

    // The OTLP exporters need to run in a Tokio context
    // Do this first so all the other logging works
    let disable_otlp = global_cfg.disable_otlp;
    let nonblocking_log = global_cfg.nonblocking_log;
    #[expect(clippy::disallowed_methods, reason = "tracing init on main runtime")]
    runtime.block_on(async { tokio::spawn(async move { init_tracing(disable_otlp, nonblocking_log) }).await? })?;
    set_open_file_limit(global_cfg.max_open_files);
    set_default_digest_hasher_func(DigestHasherFunc::from(
        global_cfg
            .default_digest_hash_function
            .unwrap_or(ConfigDigestHashFunction::Sha256),
    ))?;
    set_default_digest_size_health_check(global_cfg.default_digest_size_health_check)?;

    // Start pprof HTTP server if configured and the feature is enabled.
    // Must enter the runtime context since start_pprof_server spawns a tokio task.
    #[cfg(feature = "pprof")]
    if global_cfg.pprof_port != 0 {
        let _guard = runtime.enter();
        match nativelink_util::pprof_server::start_pprof_server(global_cfg.pprof_port) {
            Ok(guard) => {
                // Leak the guard so the server lives for the process lifetime.
                std::mem::forget(guard);
                info!(port = global_cfg.pprof_port, "pprof HTTP server started");
            }
            Err(e) => {
                warn!(?e, port = global_cfg.pprof_port, "failed to start pprof HTTP server");
            }
        }
    }

    // Initiates the shutdown process by broadcasting the shutdown signal via the `oneshot::Sender` to all listeners.
    // Each listener will perform its cleanup and then drop its `oneshot::Sender`, signaling completion.
    // Once all `oneshot::Sender` instances are dropped, the worker knows it can safely terminate.
    let (shutdown_tx, _) = broadcast::channel::<ShutdownGuard>(BROADCAST_CAPACITY);
    #[cfg(target_family = "unix")]
    let shutdown_tx_clone = shutdown_tx.clone();
    #[cfg(target_family = "unix")]
    let mut shutdown_guard = ShutdownGuard::default();

    #[expect(clippy::disallowed_methods, reason = "signal handler on main runtime")]
    runtime.spawn(async move {
        tokio::signal::ctrl_c()
            .await
            .expect("Failed to listen to SIGINT");
        eprintln!("User terminated process via SIGINT");
        std::process::exit(130);
    });

    #[allow(unused_variables)]
    let (scheduler_shutdown_tx, scheduler_shutdown_rx) = oneshot::channel();

    #[cfg(target_family = "unix")]
    #[expect(clippy::disallowed_methods, reason = "signal handler on main runtime")]
    runtime.spawn(async move {
        signal(SignalKind::terminate())
            .expect("Failed to listen to SIGTERM")
            .recv()
            .await;
        warn!("Process terminated via SIGTERM",);
        drop(shutdown_tx_clone.send(shutdown_guard.clone()));
        scheduler_shutdown_rx
            .await
            .expect("Failed to receive scheduler shutdown");
        let () = shutdown_guard.wait_for(Priority::P0).await;
        warn!("Successfully shut down nativelink.",);
        std::process::exit(143);
    });

    // Spawn a heartbeat task inside the tokio runtime and an external
    // watchdog OS thread that detects when the runtime stalls.
    let heartbeat_counter = Arc::new(AtomicU64::new(0));
    let heartbeat_counter_task = heartbeat_counter.clone();
    #[expect(clippy::disallowed_methods, reason = "runtime watchdog heartbeat")]
    runtime.spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_millis(500));
        loop {
            ticker.tick().await;
            heartbeat_counter_task.fetch_add(1, Ordering::Relaxed);
        }
    });
    std::thread::Builder::new()
        .name("runtime-watchdog".to_string())
        .spawn(move || {
            let stall_threshold = Duration::from_secs(2);
            let check_interval = Duration::from_secs(1);
            loop {
                let before = heartbeat_counter.load(Ordering::Relaxed);
                std::thread::sleep(check_interval);
                let after = heartbeat_counter.load(Ordering::Relaxed);
                if before == after {
                    let stall_start = std::time::Instant::now();
                    let mut stall_logged = false;
                    // Confirmed stall — wait until it resolves to measure duration.
                    loop {
                        std::thread::sleep(Duration::from_millis(100));
                        let now = heartbeat_counter.load(Ordering::Relaxed);
                        if now != after {
                            let stall_duration = stall_start.elapsed();
                            eprintln!(
                                "RUNTIME STALL RESOLVED: tokio runtime was unresponsive for {:.1}s (heartbeat stuck at {after})",
                                stall_duration.as_secs_f64() + check_interval.as_secs_f64(),
                            );
                            break;
                        }
                        if !stall_logged && stall_start.elapsed() > stall_threshold {
                            stall_logged = true;
                            let total = stall_threshold.as_secs_f64()
                                + check_interval.as_secs_f64();
                            eprintln!(
                                "RUNTIME STALL IN PROGRESS: tokio runtime unresponsive for >{total:.1}s (heartbeat stuck at {after})",
                            );
                            dump_thread_stacks();
                        }
                    }
                }
            }
        })
        .expect("Failed to spawn runtime watchdog thread");

    #[expect(clippy::disallowed_methods, reason = "waiting on everything to finish")]
    runtime
        .block_on(async {
            trace_span!("main")
                .in_scope(|| async { inner_main(cfg, shutdown_tx, scheduler_shutdown_tx).await })
                .await
        })
        .err_tip(|| "main() function failed")?;
    Ok(())
}
