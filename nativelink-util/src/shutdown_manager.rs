// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};

use futures::future::ready;
use futures::FutureExt;
use parking_lot::Mutex;
use tokio::runtime::Handle;
#[cfg(target_family = "unix")]
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{broadcast, oneshot};
use tracing::{event, Level};

static SHUTDOWN_MANAGER: ShutdownManager = ShutdownManager {
    is_shutting_down: AtomicBool::new(false),
    shutdown_tx: Mutex::new(None), // Will be initialized in `init`.
    maybe_shutdown_weak_sender: Mutex::new(None),
};

/// Broadcast Channel Capacity
/// Note: The actual capacity may be greater than the provided capacity.
const BROADCAST_CAPACITY: usize = 1;

/// ShutdownManager is a singleton that manages the shutdown of the
/// application. Services can register to be notified when a graceful
/// shutdown is initiated using [`ShutdownManager::wait_for_shutdown`].
/// When the future returned by [`ShutdownManager::wait_for_shutdown`] is
/// completed, the caller will then be handed a [`ShutdownGuard`] which
/// must be held until the caller has completed its shutdown procedure.
/// Once the caller has completed its shutdown procedure, the caller
/// must drop the [`ShutdownGuard`]. When all [`ShutdownGuard`]s have
/// been dropped, the application will then exit.
pub struct ShutdownManager {
    is_shutting_down: AtomicBool,
    shutdown_tx: Mutex<Option<broadcast::Sender<Arc<oneshot::Sender<()>>>>>,
    maybe_shutdown_weak_sender: Mutex<Option<Weak<oneshot::Sender<()>>>>,
}

impl ShutdownManager {
    #[allow(clippy::disallowed_methods)]
    pub fn init(runtime: &Handle) {
        let (shutdown_tx, _) = broadcast::channel::<Arc<oneshot::Sender<()>>>(BROADCAST_CAPACITY);
        *SHUTDOWN_MANAGER.shutdown_tx.lock() = Some(shutdown_tx);

        runtime.spawn(async move {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to listen to SIGINT");
            event!(Level::WARN, "User terminated process via SIGINT");
            std::process::exit(130);
        });

        #[cfg(target_family = "unix")]
        {
            runtime.spawn(async move {
                signal(SignalKind::terminate())
                    .expect("Failed to listen to SIGTERM")
                    .recv()
                    .await;
                event!(Level::WARN, "Received SIGTERM, begginning shutdown.");
                Self::graceful_shutdown();
            });
        }
    }

    pub fn is_shutting_down() -> bool {
        SHUTDOWN_MANAGER.is_shutting_down.load(Ordering::Acquire)
    }

    #[allow(clippy::disallowed_methods)]
    fn graceful_shutdown() {
        if SHUTDOWN_MANAGER
            .is_shutting_down
            .swap(true, Ordering::Release)
        {
            event!(Level::WARN, "Shutdown already in progress.");
            return;
        }
        let (complete_tx, complete_rx) = oneshot::channel::<()>();
        let shutdown_guard = Arc::new(complete_tx);
        SHUTDOWN_MANAGER
            .maybe_shutdown_weak_sender
            .lock()
            .replace(Arc::downgrade(&shutdown_guard))
            .expect("Expected maybe_shutdown_weak_sender to be empty");
        tokio::spawn(async move {
            {
                let shutdown_tx_lock = SHUTDOWN_MANAGER.shutdown_tx.lock();
                // No need to check result of send, since it will only fail if
                // all receivers have been dropped, in which case it means we
                // can safely shutdown.
                let _ = shutdown_tx_lock
                    .as_ref()
                    .expect("ShutdownManager was never initialized")
                    .send(shutdown_guard);
            }
            // It is impossible for the result to be anything but Err(RecvError),
            // which means all receivers have been dropped and we can safely shutdown.
            let _ = complete_rx.await;
            event!(Level::WARN, "All services gracefully shutdown.",);
            std::process::exit(143);
        });
    }

    pub fn wait_for_shutdown(service_name: impl Into<String>) -> impl Future<Output = ShutdownGuard> + Send {
        let service_name = service_name.into();
        if Self::is_shutting_down() {
            let maybe_shutdown_weak_sender_lock = SHUTDOWN_MANAGER
                .maybe_shutdown_weak_sender
                .lock();
            let maybe_sender = maybe_shutdown_weak_sender_lock
                .as_ref()
                .expect("Expected maybe_shutdown_weak_sender to be set");
            if let Some(sender) = maybe_sender.upgrade() {
                event!(
                    Level::INFO,
                    "Service {service_name} has been notified of shutdown request"
                );
                return ready(ShutdownGuard {
                    service_name,
                    _maybe_guard: Some(sender),
                }).left_future();
            }
            return ready(ShutdownGuard {
                service_name,
                _maybe_guard: None,
            }).left_future();
        }
        let mut shutdown_receiver = SHUTDOWN_MANAGER
            .shutdown_tx
            .lock()
            .as_ref()
            .expect("ShutdownManager was never initialized")
            .subscribe();
        async move {
            let sender = shutdown_receiver
                .recv()
                .await
                .expect("Shutdown sender dropped. This should never happen.");
            event!(
                Level::INFO,
                "Service {service_name} has been notified of shutdown request"
            );
            ShutdownGuard {
                service_name,
                _maybe_guard: Some(sender),
            }
        }
        .right_future()
    }
}

#[derive(Clone)]
pub struct ShutdownGuard {
    service_name: String,
    _maybe_guard: Option<Arc<oneshot::Sender<()>>>,
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        event!(
            Level::INFO,
            "Service {} has completed shutdown.",
            self.service_name
        );
    }
}
