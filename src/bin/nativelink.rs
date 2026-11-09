// Copyright 2024 Trace Machina, Inc. All rights reserved.
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

// The worker's action future is a long chain of combinators; proving it
// Send for the spawn walks the whole chain, and the default limit of 128 is
// a few steps short of it on the sanitizer toolchain.
#![recursion_limit = "256"]

use clap::Parser;
use futures::future::try_join_all;
use mimalloc::MiMalloc;
use nativelink_config::cas_server::{CasConfig, GlobalConfig};
use nativelink_config::stores::ConfigDigestHashFunction;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_util::common::fs::set_open_file_limit;
use nativelink_util::digest_hasher::{DigestHasherFunc, set_default_digest_hasher_func};
use nativelink_util::fs;
#[cfg(target_family = "unix")]
use nativelink_util::shutdown_guard::Priority;
use nativelink_util::shutdown_guard::ShutdownGuard;
use nativelink_util::store_trait::{
    DEFAULT_DIGEST_SIZE_HEALTH_CHECK_CFG, set_default_digest_size_health_check,
};
use nativelink_util::telemetry::init_tracing;
use tokio::select;
#[cfg(target_family = "unix")]
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot::Sender;
use tokio::sync::{broadcast, oneshot};
use tracing::{trace_span, warn};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

/// Broadcast Channel Capacity
/// Note: The actual capacity may be greater than the provided capacity.
const BROADCAST_CAPACITY: usize = 1;

fn install_default_rustls_crypto_provider() {
    drop(tokio_rustls::rustls::crypto::ring::default_provider().install_default());
}

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

async fn inner_main(
    cfg: CasConfig,
    shutdown_tx: broadcast::Sender<ShutdownGuard>,
    scheduler_shutdown_tx: Sender<()>,
) -> Result<(), Error> {
    let server = nativelink::build_server(cfg, shutdown_tx, scheduler_shutdown_tx).await?;

    let result = select! {
        result = try_join_all(server.root_futures) => result.map(|_| ()),
        result = server.single_use_complete_rx, if server.single_use_enabled => {
            result.map_err(|err| make_err!(Code::Internal, "Single-use worker completion channel closed: {err}"))
        },
    };
    if let Err(e) = result {
        panic!("{e:?}");
    }

    Ok(())
}

fn get_config() -> Result<CasConfig, Error> {
    let args = Args::parse();
    CasConfig::try_from_json5_file(&args.config_file)
}

fn main() -> Result<(), Box<dyn core::error::Error>> {
    install_default_rustls_crypto_provider();

    // Set QoS to USER_INITIATED on the main thread *before* the tokio
    // runtime is built so the spawned worker threads inherit P-core
    // scheduling preference via pthread QoS inheritance on Apple
    // Silicon. `on_thread_start` below is a belt-and-suspenders hook
    // for any thread that misses the inherited class (e.g. tokio
    // blocking pool threads created lazily). No-op on non-macOS.
    let _ = nativelink_worker::qos::set_user_initiated();

    #[expect(clippy::disallowed_methods, reason = "starting main runtime")]
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .on_thread_start(|| {
            let _ = nativelink_worker::qos::set_user_initiated();
        })
        .enable_all()
        .build()?;

    // The OTLP exporters need to run in a Tokio context
    // Do this first so all the other logging works
    #[expect(clippy::disallowed_methods, reason = "tracing init on main runtime")]
    runtime.block_on(async { tokio::spawn(async { init_tracing().await }).await? })?;

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

    let (scheduler_shutdown_tx, scheduler_shutdown_rx) = oneshot::channel();

    // SIGINT takes the SIGTERM path: a worker stopped from a terminal
    // drains like one stopped by its supervisor. A second SIGINT during
    // the drain exits at once, for the operator who meant it.
    #[cfg(target_family = "unix")]
    #[expect(clippy::disallowed_methods, reason = "signal handler on main runtime")]
    runtime.spawn(async move {
        let mut sigterm = signal(SignalKind::terminate()).expect("Failed to listen to SIGTERM");
        let mut sigint = signal(SignalKind::interrupt()).expect("Failed to listen to SIGINT");
        let exit_code = tokio::select! {
            _ = sigterm.recv() => { warn!("Process terminated via SIGTERM"); 143 }
            _ = sigint.recv() => { warn!("Process terminated via SIGINT, draining; send it again to exit at once"); 130 }
        };
        tokio::spawn(async move {
            sigint.recv().await;
            eprintln!("User terminated process via second SIGINT");
            std::process::exit(130);
        });
        drop(shutdown_tx_clone.send(shutdown_guard.clone()));
        scheduler_shutdown_rx
            .await
            .expect("Failed to receive scheduler shutdown");
        let () = shutdown_guard.wait_for(Priority::P0).await;
        warn!("Successfully shut down nativelink.");
        std::process::exit(exit_code);
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
