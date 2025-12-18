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

use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use async_lock::RwLock;
use nativelink_util::action_messages::WorkerId;
use tracing::{debug, trace};

/// In-memory worker registry that tracks worker liveness.
#[derive(Debug)]
pub struct WorkerRegistry {
    workers: RwLock<HashMap<WorkerId, SystemTime>>,
}

impl Default for WorkerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerRegistry {
    /// Creates a new worker registry.
    pub fn new() -> Self {
        Self {
            workers: RwLock::new(HashMap::new()),
        }
    }

    /// Updates the heartbeat timestamp for a worker.
    pub async fn update_worker_heartbeat(&self, worker_id: &WorkerId, now: SystemTime) {
        let mut workers = self.workers.write().await;
        workers.insert(worker_id.clone(), now);
        trace!(?worker_id, "FLOW: Worker heartbeat updated in registry");
    }

    pub async fn register_worker(&self, worker_id: &WorkerId, now: SystemTime) {
        let mut workers = self.workers.write().await;
        workers.insert(worker_id.clone(), now);
        debug!(?worker_id, "FLOW: Worker registered in registry");
    }

    pub async fn remove_worker(&self, worker_id: &WorkerId) {
        let mut workers = self.workers.write().await;
        workers.remove(worker_id);
        debug!(?worker_id, "FLOW: Worker removed from registry");
    }

    pub async fn is_worker_alive(
        &self,
        worker_id: &WorkerId,
        timeout: Duration,
        now: SystemTime,
    ) -> bool {
        let workers = self.workers.read().await;

        if let Some(last_seen) = workers.get(worker_id) {
            if let Some(deadline) = last_seen.checked_add(timeout) {
                let is_alive = deadline > now;
                trace!(
                    ?worker_id,
                    ?last_seen,
                    ?timeout,
                    is_alive,
                    "FLOW: Worker liveness check"
                );
                return is_alive;
            }
        }

        trace!(?worker_id, "FLOW: Worker not found or timed out");
        false
    }

    pub async fn get_worker_last_seen(&self, worker_id: &WorkerId) -> Option<SystemTime> {
        let workers = self.workers.read().await;
        workers.get(worker_id).copied()
    }
}

pub type SharedWorkerRegistry = Arc<WorkerRegistry>;

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::disallowed_methods)] // tokio::test uses block_on internally
    #[tokio::test]
    async fn test_worker_heartbeat() {
        let registry = WorkerRegistry::new();
        let worker_id = WorkerId::from(String::from("test"));
        let now = SystemTime::now();

        // Worker not registered yet
        assert!(
            !registry
                .is_worker_alive(&worker_id, Duration::from_secs(5), now)
                .await
        );

        // Register worker
        registry.register_worker(&worker_id, now).await;
        assert!(
            registry
                .is_worker_alive(&worker_id, Duration::from_secs(5), now)
                .await
        );

        // Check with expired timeout
        let future = now.checked_add(Duration::from_secs(10)).unwrap();
        assert!(
            !registry
                .is_worker_alive(&worker_id, Duration::from_secs(5), future)
                .await
        );

        // Update heartbeat
        registry.update_worker_heartbeat(&worker_id, future).await;
        assert!(
            registry
                .is_worker_alive(&worker_id, Duration::from_secs(5), future)
                .await
        );
    }

    #[allow(clippy::disallowed_methods)] // tokio::test uses block_on internally
    #[tokio::test]
    async fn test_remove_worker() {
        let registry = WorkerRegistry::new();
        let worker_id = WorkerId::from(String::from("test-worker"));
        let now = SystemTime::now();

        registry.register_worker(&worker_id, now).await;
        assert!(
            registry
                .is_worker_alive(&worker_id, Duration::from_secs(5), now)
                .await
        );

        registry.remove_worker(&worker_id).await;
        assert!(
            !registry
                .is_worker_alive(&worker_id, Duration::from_secs(5), now)
                .await
        );
    }
}
