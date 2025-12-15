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
const DEFAULT_MAX_DECODING_MESSAGE_SIZE: usize = 4 * 1024 * 1024;

macro_rules! service_setup {
    ($v: tt, $http_config: tt) => {{
        let mut service = $v.into_service();
        let max_decoding_message_size = if $http_config.max_decoding_message_size == 0 {
            DEFAULT_MAX_DECODING_MESSAGE_SIZE
        } else {
            $http_config.max_decoding_message_size
        };
        service = service.max_decoding_message_size(max_decoding_message_size);
        let send_algo = &$http_config.compression.send_compression_algorithm;
        if let Some(encoding) = into_encoding(send_algo.unwrap_or(HttpCompressionAlgorithm::None)) {
            service = service.send_compressed(encoding);
        }
        for encoding in $http_config
            .compression
            .accepted_compression_algorithms
            .iter()
            // Filter None values.
            .filter_map(|from: &HttpCompressionAlgorithm| into_encoding(*from))
        {
            service = service.accept_compressed(encoding);
        }
        service
    }};
}

async fn inner_main(
    cfg: CasConfig,
    shutdown_tx: broadcast::Sender<ShutdownGuard>,
    scheduler_shutdown_tx: Sender<()>,
) -> Result<(), Error> {
    const fn into_encoding(from: HttpCompressionAlgorithm) -> Option<CompressionEncoding> {
        match from {
            HttpCompressionAlgorithm::Gzip => Some(CompressionEncoding::Gzip),
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

    let mut action_schedulers = HashMap::new();
    let mut worker_schedulers = HashMap::new();
    for SchedulerConfig { name, spec } in cfg.schedulers.iter().flatten() {
        let (maybe_action_scheduler, maybe_worker_scheduler) =
            scheduler_factory(spec, &store_manager, maybe_origin_event_tx.as_ref())
                .err_tip(|| format!("Failed to create scheduler '{name}'"))?;
        if let Some(action_scheduler) = maybe_action_scheduler {
            action_schedulers.insert(name.clone(), action_scheduler.clone());
        }
        if let Some(worker_scheduler) = maybe_worker_scheduler {
            worker_schedulers.insert(name.clone(), worker_scheduler.clone());
        }
    }

    let server_cfgs: Vec<ServerConfig> = cfg.servers.into_iter().collect();

    for server_cfg in server_cfgs {
        let services = server_cfg
            .services
            .err_tip(|| "'services' must be configured")?;

        // Currently we only support http as our socket type.
        let ListenerConfig::Http(http_config) = server_cfg.listener;

        let tonic_services = Routes::builder()
            .routes()
            .add_optional_service(
                services
                    .ac
                    .map_or(Ok(None), |cfg| {
                        AcServer::new(&cfg, &store_manager)
                            .map(|v| Some(service_setup!(v, http_config)))
                    })
                    .err_tip(|| "Could not create AC service")?,
            )
            .add_optional_service(
                services
                    .cas
                    .map_or(Ok(None), |cfg| {
                        CasServer::new(&cfg, &store_manager)
                            .map(|v| Some(service_setup!(v, http_config)))
                    })
                    .err_tip(|| "Could not create CAS service")?,
            )
            .add_optional_service(
                services
                    .execution
                    .map_or(Ok(None), |cfg| {
                        ExecutionServer::new(&cfg, &action_schedulers, &store_manager)
                            .map(|v| Some(service_setup!(v, http_config)))
                    })
                    .err_tip(|| "Could not create Execution service")?,
            )
            .add_optional_service(
                services
                    .fetch
                    .map_or(Ok(None), |cfg| {
                        FetchServer::new(&cfg, &store_manager)
                            .map(|v| Some(service_setup!(v, http_config)))
                    })
                    .err_tip(|| "Could not create Fetch service")?,
            )
            .add_optional_service(
                services
                    .push
                    .map_or(Ok(None), |cfg| {
                        PushServer::new(&cfg, &store_manager)
                            .map(|v| Some(service_setup!(v, http_config)))
                    })
                    .err_tip(|| "Could not create Push service")?,
            )
            .add_optional_service(
                services
                    .bytestream
                    .map_or(Ok(None), |cfg| {
                        ByteStreamServer::new(&cfg, &store_manager)
                            .map(|v| Some(service_setup!(v, http_config)))
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
                .map(|v| service_setup!(v, http_config)),
            )
            .add_optional_service(
                services
                    .worker_api
                    .map_or(Ok(None), |cfg| {
                        WorkerApiServer::new(&cfg, &worker_schedulers)
                            .map(|v| Some(service_setup!(v, http_config)))
                    })
                    .err_tip(|| "Could not create WorkerApi service")?,
            )
            .add_optional_service(
                services
                    .experimental_bep
                    .map_or(Ok(None), |cfg| {
                        BepServer::new(&cfg, &store_manager)
                            .map(|v| Some(service_setup!(v, http_config)))
                    })
                    .err_tip(|| "Could not create BEP service")?,
            );

        let health_registry = health_registry_builder.lock().await.build();

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
            let mut config = TlsServerConfig::builder()
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
        if let Some(value) = http_config.experimental_http2_initial_stream_window_size {
            http.http2().initial_stream_window_size(value);
        }
        if let Some(value) = http_config.experimental_http2_initial_connection_window_size {
            http.http2().initial_connection_window_size(value);
        }
        if let Some(value) = http_config.experimental_http2_adaptive_window {
            http.http2().adaptive_window(value);
        }
        if let Some(value) = http_config.experimental_http2_max_frame_size {
            http.http2().max_frame_size(value);
        }
        if let Some(value) = http_config.experimental_http2_max_concurrent_streams {
            http.http2().max_concurrent_streams(value);
        }
        if let Some(value) = http_config.experimental_http2_keep_alive_timeout {
            http.http2()
                .keep_alive_timeout(Duration::from_secs(u64::from(value)));
        }
        if let Some(value) = http_config.experimental_http2_max_send_buf_size {
            http.http2().max_send_buf_size(
                usize::try_from(value).err_tip(|| "Could not convert http2_max_send_buf_size")?,
            );
        }
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
                                            error!(
                                                target: "nativelink::services",
                                                ?err,
                                                "Failed running service"
                                            );
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

fn main() -> Result<(), Box<dyn core::error::Error>> {
    #[expect(clippy::disallowed_methods, reason = "starting main runtime")]
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // The OTLP exporters need to run in a Tokio context
    // Do this first so all the other logging works
    #[expect(clippy::disallowed_methods, reason = "tracing init on main runtime")]
    runtime.block_on(async { tokio::spawn(async { init_tracing() }).await? })?;

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
        }
    };
    set_open_file_limit(global_cfg.max_open_files);
    set_default_digest_hasher_func(DigestHasherFunc::from(
        global_cfg
            .default_digest_hash_function
            .unwrap_or(ConfigDigestHashFunction::Sha256),
    ))?;
    set_default_digest_size_health_check(global_cfg.default_digest_size_health_check)?;

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
