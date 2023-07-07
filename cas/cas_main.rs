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
use std::sync::Arc;

use clap::Parser;
use futures::future::{select_all, BoxFuture, TryFutureExt};
use json5;
use runfiles::Runfiles;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;

use ac_server::AcServer;
use bytestream_server::ByteStreamServer;
use capabilities_server::CapabilitiesServer;
use cas_server::CasServer;
use common::fs::set_open_file_limit;
use config::cas_server::{CasConfig, CompressionAlgorithm, GlobalConfig, WorkerConfig};
use default_scheduler_factory::scheduler_factory;
use default_store_factory::store_factory;
use error::{make_err, Code, Error, ResultExt};
use execution_server::ExecutionServer;
use local_worker::new_local_worker;
use store::StoreManager;
use worker_api_server::WorkerApiServer;

const DEFAULT_CONFIG_FILE: &str = "<built-in example in config/examples/basic_cas.json>";

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
    #[clap(value_parser, default_value = DEFAULT_CONFIG_FILE)]
    config_file: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();
    // Note: We cannot mutate args, so we create another variable for it here.
    let mut config_file = args.config_file;
    if config_file.eq(DEFAULT_CONFIG_FILE) {
        let r = Runfiles::create().err_tip(|| "Failed to create runfiles lookup object")?;
        config_file = r
            .rlocation("turbo_cache/config/examples/basic_cas.json")
            .into_os_string()
            .into_string()
            .unwrap();
    }

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .format_timestamp_millis()
        .init();

    let json_contents = String::from_utf8(
        tokio::fs::read(&config_file)
            .await
            .err_tip(|| format!("Could not open config file {}", config_file))?,
    )?;
    let cfg: CasConfig = json5::from_str(&json_contents)?;

    // Note: If the default changes make sure you update the documentation in
    // `config/cas_server.rs`.
    const DEFAULT_MAX_OPEN_FILES: usize = 512;
    let global_cfg = if let Some(mut global_cfg) = cfg.global {
        if global_cfg.max_open_files == 0 {
            global_cfg.max_open_files = DEFAULT_MAX_OPEN_FILES;
        }
        global_cfg
    } else {
        GlobalConfig {
            max_open_files: DEFAULT_MAX_OPEN_FILES,
        }
    };
    set_open_file_limit(global_cfg.max_open_files);

    let store_manager = Arc::new(StoreManager::new());
    for (name, store_cfg) in cfg.stores {
        store_manager.add_store(
            &name,
            store_factory(&store_cfg, &store_manager)
                .await
                .err_tip(|| format!("Failed to create store '{}'", name))?,
        );
    }

    let mut action_schedulers = HashMap::new();
    let mut worker_schedulers = HashMap::new();
    if let Some(schedulers_cfg) = cfg.schedulers {
        for (name, scheduler_cfg) in schedulers_cfg {
            let (maybe_action_scheduler, maybe_worker_scheduler) =
                scheduler_factory(&scheduler_cfg, &store_manager, &action_schedulers)
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

    let mut futures: Vec<BoxFuture<Result<(), Error>>> = Vec::new();
    for worker_cfg in cfg.workers.unwrap_or(vec![]) {
        let spawn_fut = match worker_cfg {
            WorkerConfig::local(local_worker_cfg) => {
                let fast_slow_store = store_manager
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
                tokio::spawn(local_worker.run())
            }
        };
        futures.push(Box::pin(spawn_fut.map_ok_or_else(|e| Err(e.into()), |v| v)));
    }

    fn into_encoding(from: &CompressionAlgorithm) -> Option<CompressionEncoding> {
        match from {
            CompressionAlgorithm::Gzip => Some(CompressionEncoding::Gzip),
            CompressionAlgorithm::None => None,
        }
    }

    for server_cfg in cfg.servers {
        let server = Server::builder();
        let services = server_cfg.services.ok_or_else(|| "'services' must be configured")?;

        let server = server
            // TODO(allada) This is only used so we can get 200 status codes to know if our service
            // is running.
            .accept_http1(true)
            .add_optional_service(
                services
                    .ac
                    .map_or(Ok(None), |cfg| {
                        AcServer::new(&cfg, &store_manager).and_then(|v| {
                            let mut service = v.into_service();
                            let send_algo = &server_cfg.compression.send_compression_algorithm;
                            if let Some(encoding) = into_encoding(&send_algo.unwrap_or(CompressionAlgorithm::None)) {
                                service = service.send_compressed(encoding);
                            }
                            for encoding in server_cfg
                                .compression
                                .accepted_compression_algorithms
                                .iter()
                                .map(into_encoding)
                                // Filter None values.
                                .filter_map(|v| v)
                            {
                                service = service.accept_compressed(encoding);
                            }
                            Ok(Some(service))
                        })
                    })
                    .err_tip(|| "Could not create AC service")?,
            )
            .add_optional_service(
                services
                    .cas
                    .map_or(Ok(None), |cfg| {
                        CasServer::new(&cfg, &store_manager).and_then(|v| {
                            let mut service = v.into_service();
                            let send_algo = &server_cfg.compression.send_compression_algorithm;
                            if let Some(encoding) = into_encoding(&send_algo.unwrap_or(CompressionAlgorithm::None)) {
                                service = service.send_compressed(encoding);
                            }
                            for encoding in server_cfg
                                .compression
                                .accepted_compression_algorithms
                                .iter()
                                .map(into_encoding)
                                // Filter None values.
                                .filter_map(|v| v)
                            {
                                service = service.accept_compressed(encoding);
                            }
                            Ok(Some(service))
                        })
                    })
                    .err_tip(|| "Could not create CAS service")?,
            )
            .add_optional_service(
                services
                    .execution
                    .map_or(Ok(None), |cfg| {
                        ExecutionServer::new(&cfg, &action_schedulers, &store_manager).and_then(|v| {
                            let mut service = v.into_service();
                            let send_algo = &server_cfg.compression.send_compression_algorithm;
                            if let Some(encoding) = into_encoding(&send_algo.unwrap_or(CompressionAlgorithm::None)) {
                                service = service.send_compressed(encoding);
                            }
                            for encoding in server_cfg
                                .compression
                                .accepted_compression_algorithms
                                .iter()
                                .map(into_encoding)
                                // Filter None values.
                                .filter_map(|v| v)
                            {
                                service = service.accept_compressed(encoding);
                            }
                            Ok(Some(service))
                        })
                    })
                    .err_tip(|| "Could not create Execution service")?,
            )
            .add_optional_service(
                services
                    .bytestream
                    .map_or(Ok(None), |cfg| {
                        ByteStreamServer::new(&cfg, &store_manager).and_then(|v| {
                            let mut service = v.into_service();
                            let send_algo = &server_cfg.compression.send_compression_algorithm;
                            if let Some(encoding) = into_encoding(&send_algo.unwrap_or(CompressionAlgorithm::None)) {
                                service = service.send_compressed(encoding);
                            }
                            for encoding in server_cfg
                                .compression
                                .accepted_compression_algorithms
                                .iter()
                                .map(into_encoding)
                                // Filter None values.
                                .filter_map(|v| v)
                            {
                                service = service.accept_compressed(encoding);
                            }
                            Ok(Some(service))
                        })
                    })
                    .err_tip(|| "Could not create ByteStream service")?,
            )
            .add_optional_service(
                services
                    .capabilities
                    .map_or(Ok(None), |cfg| {
                        CapabilitiesServer::new(&cfg, &action_schedulers).and_then(|v| {
                            let mut service = v.into_service();
                            let send_algo = &server_cfg.compression.send_compression_algorithm;
                            if let Some(encoding) = into_encoding(&send_algo.unwrap_or(CompressionAlgorithm::None)) {
                                service = service.send_compressed(encoding);
                            }
                            for encoding in server_cfg
                                .compression
                                .accepted_compression_algorithms
                                .iter()
                                .map(into_encoding)
                                // Filter None values.
                                .filter_map(|v| v)
                            {
                                service = service.accept_compressed(encoding);
                            }
                            Ok(Some(service))
                        })
                    })
                    .err_tip(|| "Could not create Capabilities service")?,
            )
            .add_optional_service(
                services
                    .worker_api
                    .map_or(Ok(None), |cfg| {
                        WorkerApiServer::new(&cfg, &worker_schedulers).and_then(|v| {
                            let mut service = v.into_service();
                            let send_algo = &server_cfg.compression.send_compression_algorithm;
                            if let Some(encoding) = into_encoding(&send_algo.unwrap_or(CompressionAlgorithm::None)) {
                                service = service.send_compressed(encoding);
                            }
                            for encoding in server_cfg
                                .compression
                                .accepted_compression_algorithms
                                .iter()
                                .map(into_encoding)
                                // Filter None values.
                                .filter_map(|v| v)
                            {
                                service = service.accept_compressed(encoding);
                            }
                            Ok(Some(service))
                        })
                    })
                    .err_tip(|| "Could not create WorkerApi service")?,
            );

        let addr = server_cfg.listen_address.parse()?;
        futures.push(Box::pin(
            tokio::spawn(
                server
                    .serve(addr)
                    .map_err(|e| make_err!(Code::Internal, "Failed running service : {:?}", e)),
            )
            .map_ok_or_else(|e| Err(e.into()), |v| v),
        ));
    }

    if let Err(e) = select_all(futures).await.0 {
        panic!("{:?}", e);
    }
    unreachable!("None of the futures should resolve in main()");
}
