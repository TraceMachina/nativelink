// Copyright 2023 The NativeLink Authors. All rights reserved.
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

pub mod action_messages;
pub mod buf_channel;
pub mod common;
pub mod connection_manager;
pub mod digest_hasher;
pub mod evicting_map;
pub mod fastcdc;
pub mod fs;
pub mod health_utils;
pub mod metrics_utils;
pub mod origin_context;
pub mod platform_properties;
pub mod proto_stream_utils;
pub mod resource_info;
pub mod retry;
pub mod store_trait;
pub mod task;
pub mod tls_utils;
pub mod write_counter;

// Re-export tracing mostly for use in macros.
pub use tracing as __tracing;

/// Initialize tracing.
pub fn init_tracing() -> Result<(), nativelink_error::Error> {
    use tracing_subscriber::prelude::*;

    static LOGGING_INITIALIZED: std::sync::Mutex<bool> = std::sync::Mutex::new(false);
    let mut logging_initized_guard = LOGGING_INITIALIZED.lock().unwrap();
    if *logging_initized_guard {
        return Err(nativelink_error::make_err!(
            nativelink_error::Code::Internal,
            "Logging already initialized"
        ));
    }
    *logging_initized_guard = true;
    let env_filter = tracing_subscriber::EnvFilter::builder()
        .with_default_directive(tracing::metadata::LevelFilter::WARN.into())
        .from_env_lossy();

    if cfg!(feature = "enable_tokio_console") {
        tracing_subscriber::registry()
            .with(console_subscriber::spawn())
            .with(
                tracing_subscriber::fmt::layer()
                    .pretty()
                    .with_timer(tracing_subscriber::fmt::time::time())
                    .with_filter(env_filter),
            )
            .init();
    } else {
        tracing_subscriber::fmt()
            .pretty()
            .with_timer(tracing_subscriber::fmt::time::time())
            .with_env_filter(env_filter)
            .init();
    }
    Ok(())
}
