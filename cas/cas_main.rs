// Copyright 2022 The Turbo Cache Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use async_lock::Mutex as AsyncMutex;
use axum::Router;
use clap::Parser;
use futures::future::{select_all, BoxFuture, OptionFuture, TryFutureExt};
use hyper::server::conn::Http;
use hyper::{Body, Response};
use parking_lot::Mutex;
use scopeguard::guard;
use tokio::net::TcpListener;
use tokio::task::spawn_blocking;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server as TonicServer;
use tower::util::ServiceExt;

use ac_server::AcServer;
use bytestream_server::ByteStreamServer;
use capabilities_server::CapabilitiesServer;
use cas_server::CasServer;
use common::fs::{set_idle_file_descriptor_timeout, set_open_file_limit};
use common::log;
use config::cas_server::{CasConfig, CompressionAlgorithm, GlobalConfig, ServerConfig, WorkerConfig};
use default_scheduler_factory::scheduler_factory;
use default_store_factory::store_factory;
use error::{make_err, Code, Error, ResultExt};
use execution_server::ExecutionServer;
use local_worker::new_local_worker;
use metrics_utils::{set_metrics_enabled_for_this_thread, Collector, CollectorState, MetricsComponent, Registry};
use store::StoreManager;
use worker_api_server::WorkerApiServer;

/// Note: This must be kept in sync with the documentation in `PrometheusConfig::path`.
const DEFAULT_PROMETHEUS_METRICS_PATH: &str = "/metrics";

/// Name of environment variable to disable metrics.
const METRICS_DISABLE_ENV: &str = "TURBO_CACHE_DISABLE_METRICS";

/// Backend for bazel remote execution / cache API.
#[derive(Parser, Debug)]
#[clap(
    author = "Nathan (Blaise) Bruer <turbo-cache.blaise@allada.com>",
    version = "0.0.1",
    about,
    long_about = None
)]
struct Args {
    /// Config file to use.
    #[clap(value_parser)]
    config_file: String,
}

async fn inner_main(cfg: CasConfig) -> Result<(), Box<dyn std::error::Error>> {
    let mut root_metrics_registry = <Registry>::with_prefix("turbo_cache");

    let store_manager = Arc::new(StoreManager::new());
    {
        let root_store_metrics = root_metrics_registry.sub_registry_with_prefix("stores");
        for (name, store_cfg) in cfg.stores {
            let store_metrics = root_store_metrics.sub_registry_with_prefix(&name);
            store_manager.add_store(
                &name,
                store_factory(&store_cfg, &store_manager, Some(store_metrics))
                    .await
                    .err_tip(|| format!("Failed to create store '{}'", name))?,
            );
        }
    }

    let mut action_schedulers = HashMap::new();
    let mut worker_schedulers = HashMap::new();
    if let Some(schedulers_cfg) = cfg.schedulers {
        let root_scheduler_metrics = root_metrics_registry.sub_registry_with_prefix("schedulers");
        for (name, scheduler_cfg) in schedulers_cfg {
            let scheduler_metrics = root_scheduler_metrics.sub_registry_with_prefix(&name);
            let (maybe_action_scheduler, maybe_worker_scheduler) =
                scheduler_factory(&scheduler_cfg, &store_manager, scheduler_metrics)
                    .await
                    .err_tip(|| format!("Failed to create scheduler '{}'", name))?;
            if let Some(action_scheduler) = maybe_action_scheduler {
                action_schedulers.insert(name.clone(), action_scheduler);
            }
            if let Some(worker_scheduler) = maybe_worker_scheduler {
                worker_schedulers.insert(name.clone(), worker_scheduler);
            }
        }
    }

    fn into_encoding(from: &CompressionAlgorithm) -> Option<CompressionEncoding> {
        match from {
            CompressionAlgorithm::Gzip => Some(CompressionEncoding::Gzip),
            CompressionAlgorithm::None => None,
        }
    }

    /// Simple wrapper to enable us to register the Hashmap so it can
    /// report metrics about what clients are connected.
    struct ConnectedClientsMetrics {
        inner: Mutex<HashSet<SocketAddr>>,
    }
    impl MetricsComponent for ConnectedClientsMetrics {
        fn gather_metrics(&self, c: &mut CollectorState) {
            let connected_clients = self.inner.lock();
            for client in connected_clients.iter() {
                c.publish_with_labels(
                    "connected_clients",
                    &1,
                    "The endpoint of the connected clients",
                    vec![("endpoint".into(), format!("{}", client).into())],
                );
            }
        }
    }

    // Registers all the ConnectedClientsMetrics to the registries
    // and zips them in. It is done this way to get around the need
    // for `root_metrics_registry` to become immutable in the loop.
    let servers_and_clients: Vec<(ServerConfig, _)> = cfg
        .servers
        .into_iter()
        .enumerate()
        .map(|(i, server_cfg)| {
            let name = if server_cfg.name.is_empty() {
                format!("{}", i)
            } else {
                server_cfg.name.clone()
            };
            let connected_clients_mux = Arc::new(ConnectedClientsMetrics {
                inner: Mutex::new(HashSet::new()),
            });
            let server_metrics = root_metrics_registry.sub_registry_with_prefix(format!("server_{}", name));
            server_metrics.register_collector(Box::new(Collector::new(&connected_clients_mux)));

            (server_cfg, connected_clients_mux)
        })
        .collect();

    let mut root_futures: Vec<BoxFuture<Result<(), Error>>> = Vec::new();

    // Lock our registry as immutable and clonable.
    let root_metrics_registry = Arc::new(AsyncMutex::new(root_metrics_registry));
    for (server_cfg, connected_clients_mux) in servers_and_clients {
        let services = server_cfg.services.ok_or("'services' must be configured")?;

        let tonic_services = TonicServer::builder()
            .add_optional_service(
                services
                    .ac
                    .map_or(Ok(None), |cfg| {
                        AcServer::new(&cfg, &store_manager).map(|v| {
                            let mut service = v.into_service();
                            let send_algo = &server_cfg.compression.send_compression_algorithm;
                            if let Some(encoding) = into_encoding(&send_algo.unwrap_or(CompressionAlgorithm::None)) {
                                service = service.send_compressed(encoding);
                            }
                            for encoding in server_cfg
                                .compression
                                .accepted_compression_algorithms
                                .iter()
                                // Filter None values.
                                .filter_map(into_encoding)
                            {
                                service = service.accept_compressed(encoding);
                            }
                            Some(service)
                        })
                    })
                    .err_tip(|| "Could not create AC service")?,
            )
            .add_optional_service(
                services
                    .cas
                    .map_or(Ok(None), |cfg| {
                        CasServer::new(&cfg, &store_manager).map(|v| {
                            let mut service = v.into_service();
                            let send_algo = &server_cfg.compression.send_compression_algorithm;
                            if let Some(encoding) = into_encoding(&send_algo.unwrap_or(CompressionAlgorithm::None)) {
                                service = service.send_compressed(encoding);
                            }
                            for encoding in server_cfg
                                .compression
                                .accepted_compression_algorithms
                                .iter()
                                // Filter None values.
                                .filter_map(into_encoding)
                            {
                                service = service.accept_compressed(encoding);
                            }
                            Some(service)
                        })
                    })
                    .err_tip(|| "Could not create CAS service")?,
            )
            .add_optional_service(
                services
                    .execution
                    .map_or(Ok(None), |cfg| {
                        ExecutionServer::new(&cfg, &action_schedulers, &store_manager).map(|v| {
                            let mut service = v.into_service();
                            let send_algo = &server_cfg.compression.send_compression_algorithm;
                            if let Some(encoding) = into_encoding(&send_algo.unwrap_or(CompressionAlgorithm::None)) {
                                service = service.send_compressed(encoding);
                            }
                            for encoding in server_cfg
                                .compression
                                .accepted_compression_algorithms
                                .iter()
                                // Filter None values.
                                .filter_map(into_encoding)
                            {
                                service = service.accept_compressed(encoding);
                            }
                            Some(service)
                        })
                    })
                    .err_tip(|| "Could not create Execution service")?,
            )
            .add_optional_service(
                services
                    .bytestream
                    .map_or(Ok(None), |cfg| {
                        ByteStreamServer::new(&cfg, &store_manager).map(|v| {
                            let mut service = v.into_service();
                            let send_algo = &server_cfg.compression.send_compression_algorithm;
                            if let Some(encoding) = into_encoding(&send_algo.unwrap_or(CompressionAlgorithm::None)) {
                                service = service.send_compressed(encoding);
                            }
                            for encoding in server_cfg
                                .compression
                                .accepted_compression_algorithms
                                .iter()
                                // Filter None values.
                                .filter_map(into_encoding)
                            {
                                service = service.accept_compressed(encoding);
                            }
                            Some(service)
                        })
                    })
                    .err_tip(|| "Could not create ByteStream service")?,
            )
            .add_optional_service(
                OptionFuture::from(
                    services
                        .capabilities
                        .as_ref()
                        // Borrow checker fighting here...
                        .map(|_| CapabilitiesServer::new(services.capabilities.as_ref().unwrap(), &action_schedulers)),
                )
                .await
                .map_or(Ok::<Option<CapabilitiesServer>, Error>(None), |server| {
                    Ok(Some(server?))
                })
                .err_tip(|| "Could not create Capabilities service")?
                .map(|v| {
                    let mut service = v.into_service();
                    let send_algo = &server_cfg.compression.send_compression_algorithm;
                    if let Some(encoding) = into_encoding(&send_algo.unwrap_or(CompressionAlgorithm::None)) {
                        service = service.send_compressed(encoding);
                    }
                    for encoding in server_cfg
                        .compression
                        .accepted_compression_algorithms
                        .iter()
                        // Filter None values.
                        .filter_map(into_encoding)
                    {
                        service = service.accept_compressed(encoding);
                    }
                    service
                }),
            )
            .add_optional_service(
                services
                    .worker_api
                    .map_or(Ok(None), |cfg| {
                        WorkerApiServer::new(&cfg, &worker_schedulers).map(|v| {
                            let mut service = v.into_service();
                            let send_algo = &server_cfg.compression.send_compression_algorithm;
                            if let Some(encoding) = into_encoding(&send_algo.unwrap_or(CompressionAlgorithm::None)) {
                                service = service.send_compressed(encoding);
                            }
                            for encoding in server_cfg
                                .compression
                                .accepted_compression_algorithms
                                .iter()
                                // Filter None values.
                                .filter_map(into_encoding)
                            {
                                service = service.accept_compressed(encoding);
                            }
                            Some(service)
                        })
                    })
                    .err_tip(|| "Could not create WorkerApi service")?,
            );

        let root_metrics_registry = root_metrics_registry.clone();

        let mut svc = Router::new()
            // This is the default service that executes if no other endpoint matches.
            .fallback_service(tonic_services.into_service().map_err(|e| panic!("{e}")))
            // This is a generic endpoint used to check if the server is up.
            .route_service("/status", axum::routing::get(move || async move { "Ok".to_string() }));

        if let Some(prometheus_cfg) = services.prometheus {
            fn error_to_response<E: std::error::Error>(e: E) -> hyper::Response<Body> {
                hyper::Response::builder()
                    .status(500)
                    .body(format!("Error: {:?}", e).into())
                    .unwrap()
            }
            let path = if prometheus_cfg.path.is_empty() {
                DEFAULT_PROMETHEUS_METRICS_PATH
            } else {
                &prometheus_cfg.path
            };
            svc = svc.route_service(
                path,
                axum::routing::get(move |_request: hyper::Request<hyper::Body>| async move {
                    // We spawn on a thread that can block to give more freedom to our metrics
                    // collection. This allows it to call functions like `tokio::block_in_place`
                    // if it needs to wait on a future.
                    spawn_blocking(move || {
                        let mut buf = String::new();
                        let root_metrics_registry_guard = futures::executor::block_on(root_metrics_registry.lock());
                        prometheus_client::encoding::text::encode(&mut buf, &root_metrics_registry_guard)
                            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
                            .map(|_| {
                                // This is a hack to get around this bug: https://github.com/prometheus/client_rust/issues/155
                                buf = buf.replace("turbo_cache_turbo_cache_stores_", "");
                                buf = buf.replace("turbo_cache_turbo_cache_workers_", "");
                                let body = Body::from(buf);
                                Response::builder()
                                    .header(
                                        hyper::header::CONTENT_TYPE,
                                        // Per spec we should probably use `application/openmetrics-text; version=1.0.0; charset=utf-8`
                                        // https://github.com/OpenObservability/OpenMetrics/blob/1386544931307dff279688f332890c31b6c5de36/specification/OpenMetrics.md#overall-structure
                                        // However, this makes debugging more difficult, so we use the old text/plain instead.
                                        "text/plain; version=0.0.4; charset=utf-8",
                                    )
                                    .body(body)
                                    .unwrap()
                            })
                            .unwrap_or_else(error_to_response)
                    })
                    .await
                    .unwrap_or_else(error_to_response)
                }),
            )
        }

        let socket_addr = server_cfg.listen_address.parse::<SocketAddr>()?;
        let tcp_listener = TcpListener::bind(&socket_addr).await?;
        log::warn!("Ready, listening on {}", socket_addr);
        root_futures.push(Box::pin(async move {
            loop {
                // Wait for client to connect.
                let (tcp_stream, remote_addr) = tcp_listener.accept().await?;
                connected_clients_mux.inner.lock().insert(remote_addr);
                // This is the safest way to guarantee that if our future
                // is ever dropped we will cleanup our data.
                let scope_guard = guard(connected_clients_mux.clone(), move |connected_clients_mux| {
                    connected_clients_mux.inner.lock().remove(&remote_addr);
                });
                let fut = Http::new().serve_connection(tcp_stream, svc.clone());
                tokio::spawn(async move {
                    // Move it into our spawn, so if our spawn dies the cleanup happens.
                    let _guard = scope_guard;
                    if let Err(e) = fut.await {
                        log::error!("Failed running service : {:?}", e);
                    }
                });
            }
        }));
    }

    {
        // We start workers after our TcpListener is setup so if our worker connects to one
        // of these services it will be able to connect.
        let worker_cfgs = cfg.workers.unwrap_or(vec![]);
        let mut root_metrics_registry_guard = root_metrics_registry.lock().await;
        let root_worker_metrics = root_metrics_registry_guard.sub_registry_with_prefix("workers");
        let mut worker_names = HashSet::with_capacity(worker_cfgs.len());
        for (i, worker_cfg) in worker_cfgs.into_iter().enumerate() {
            let spawn_fut = match worker_cfg {
                WorkerConfig::local(local_worker_cfg) => {
                    let fast_slow_store =
                        store_manager
                            .get_store(&local_worker_cfg.cas_fast_slow_store)
                            .err_tip(|| {
                                format!(
                                    "Failed to find store for cas_store_ref in worker config : {}",
                                    local_worker_cfg.cas_fast_slow_store
                                )
                            })?;
                    let ac_store = store_manager.get_store(&local_worker_cfg.ac_store).err_tip(|| {
                        format!(
                            "Failed to find store for ac_store_ref in worker config : {}",
                            local_worker_cfg.ac_store
                        )
                    })?;
                    let local_worker =
                        new_local_worker(Arc::new(local_worker_cfg), fast_slow_store.clone(), ac_store.clone())
                            .await
                            .err_tip(|| "Could not make LocalWorker")?;
                    let name = if local_worker.name().is_empty() {
                        format!("worker_{}", i)
                    } else {
                        local_worker.name().clone()
                    };
                    if worker_names.contains(&name) {
                        Err(Box::new(make_err!(
                            Code::InvalidArgument,
                            "Duplicate worker name '{}' found in config",
                            name
                        )))?;
                    }
                    let worker_metrics = root_worker_metrics.sub_registry_with_prefix(&name);
                    local_worker.register_metrics(worker_metrics);
                    worker_names.insert(name);
                    tokio::spawn(local_worker.run())
                }
            };
            root_futures.push(Box::pin(spawn_fut.map_ok_or_else(|e| Err(e.into()), |v| v)));
        }
    }

    if let Err(e) = select_all(root_futures).await.0 {
        panic!("{:?}", e);
    }
    unreachable!("None of the futures should resolve in main()");
}

async fn get_config() -> Result<CasConfig, Box<dyn std::error::Error>> {
    let args = Args::parse();
    // Note: We cannot mutate args, so we create another variable for it here.

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .format_timestamp_millis()
        .init();

    let json_contents = String::from_utf8(
        std::fs::read(&args.config_file).err_tip(|| format!("Could not open config file {}", args.config_file))?,
    )?;
    Ok(json5::from_str(&json_contents)?)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut cfg = futures::executor::block_on(get_config())?;

    let mut metrics_enabled = {
        // Note: If the default changes make sure you update the documentation in
        // `config/cas_server.rs`.
        const DEFAULT_MAX_OPEN_FILES: usize = 512;
        // Note: If the default changes make sure you update the documentation in
        // `config/cas_server.rs`.
        const DEFAULT_IDLE_FILE_DESCRIPTOR_TIMEOUT_MILLIS: u64 = 1000;
        let global_cfg = if let Some(global_cfg) = &mut cfg.global {
            if global_cfg.max_open_files == 0 {
                global_cfg.max_open_files = DEFAULT_MAX_OPEN_FILES;
            }
            if global_cfg.idle_file_descriptor_timeout_millis == 0 {
                global_cfg.idle_file_descriptor_timeout_millis = DEFAULT_IDLE_FILE_DESCRIPTOR_TIMEOUT_MILLIS;
            }
            *global_cfg
        } else {
            GlobalConfig {
                max_open_files: DEFAULT_MAX_OPEN_FILES,
                idle_file_descriptor_timeout_millis: DEFAULT_IDLE_FILE_DESCRIPTOR_TIMEOUT_MILLIS,
                disable_metrics: cfg.servers.iter().all(|v| {
                    let Some(service) = &v.services else {
                        return true;
                    };
                    service.prometheus.is_none()
                }),
            }
        };
        set_open_file_limit(global_cfg.max_open_files);
        set_idle_file_descriptor_timeout(Duration::from_millis(global_cfg.idle_file_descriptor_timeout_millis))?;
        !global_cfg.disable_metrics
    };
    // Override metrics enabled if the environment variable is set.
    if std::env::var(METRICS_DISABLE_ENV).is_ok() {
        metrics_enabled = false;
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .on_thread_start(move || set_metrics_enabled_for_this_thread(metrics_enabled))
        .build()?;
    runtime.block_on(inner_main(cfg))
}
