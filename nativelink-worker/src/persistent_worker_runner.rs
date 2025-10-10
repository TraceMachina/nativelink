// Copyright 2025 The NativeLink Authors. All rights reserved.
//
// Licensed under the Business Source License, Version 1.1 (the "License");
// you may not use this file except in compliance with the License.
// You may requested a copy of the License by emailing contact@nativelink.com.
//
// Use of this module requires an enterprise license agreement, which can be
// attained by emailing contact@nativelink.com or signing up for Nativelink
// Cloud at app.nativelink.com.
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use nativelink_config::cas_server::LocalWorkerConfig;
use nativelink_error::{Error, ResultExt};
use nativelink_metric::MetricsComponent;
use nativelink_proto::build::bazel::remote::execution::v2::Platform;
use nativelink_util::action_messages::{ActionInfo, WorkerId};
use nativelink_util::platform_properties::PlatformProperties;
use parking_lot::RwLock;
use tokio::process::Command;
use tracing::{error, info};

use crate::persistent_worker::{PersistentWorkerKey, PersistentWorkerManager};

/// Runner that can fork persistent workers on demand
#[allow(dead_code)]
#[derive(Debug, MetricsComponent)]
pub struct PersistentWorkerRunner {
    /// Base configuration for workers
    base_config: LocalWorkerConfig,

    /// Manager for persistent workers
    #[metric(group = "persistent_worker_manager")]
    persistent_worker_manager: Arc<PersistentWorkerManager>,

    /// Map of spawned persistent worker processes
    spawned_workers: Arc<RwLock<HashMap<String, SpawnedWorker>>>,

    /// Base work directory for workers
    work_dir: PathBuf,

    /// Scheduler endpoint for new workers to connect to
    scheduler_endpoint: String,
}

#[allow(dead_code)]
#[derive(Debug)]
struct SpawnedWorker {
    worker_id: WorkerId,
    process_handle: tokio::process::Child,
    persistent_key: String,
}

impl PersistentWorkerRunner {
    pub fn new(
        base_config: LocalWorkerConfig,
        work_dir: PathBuf,
        scheduler_endpoint: String,
        max_idle_duration: core::time::Duration,
        max_worker_instances: usize,
    ) -> Self {
        let persistent_worker_manager = Arc::new(PersistentWorkerManager::new(
            work_dir.join("persistent_workers"),
            max_idle_duration,
            max_worker_instances,
        ));

        Self {
            base_config,
            persistent_worker_manager,
            spawned_workers: Arc::new(RwLock::new(HashMap::new())),
            work_dir,
            scheduler_endpoint,
        }
    }

    /// Fork a new persistent worker process for the given key
    pub async fn fork_persistent_worker(
        &self,
        persistent_key: &PersistentWorkerKey,
    ) -> Result<WorkerId, Error> {
        let worker_id = WorkerId(uuid::Uuid::new_v4().to_string());
        let work_dir = self.work_dir.join(format!("worker_{worker_id}"));

        // Create working directory for this worker
        tokio::fs::create_dir_all(&work_dir)
            .await
            .err_tip(|| format!("Failed to create work dir: {}", work_dir.display()))?;

        // Build command to spawn new nativelink worker process
        let mut cmd =
            Command::new(std::env::current_exe().err_tip(|| "Failed to get current exe")?);

        // Configure as a persistent worker
        cmd.arg("worker")
            .arg("--scheduler-endpoint")
            .arg(&self.scheduler_endpoint)
            .arg("--worker-id")
            .arg(worker_id.to_string())
            .arg("--work-dir")
            .arg(work_dir.to_string_lossy().to_string());

        // Add persistent worker platform properties
        for (key, value) in persistent_key.to_platform_properties() {
            cmd.arg("--platform-property").arg(format!("{key}={value}"));
        }

        // Set environment for the worker
        cmd.env("PERSISTENT_WORKER", "true");
        cmd.env("PERSISTENT_WORKER_KEY", &persistent_key.key);
        cmd.env("PERSISTENT_WORKER_TOOL", &persistent_key.tool);

        info!(
            "Forking persistent worker {} for key: {}",
            worker_id, persistent_key.key
        );

        let process_handle = cmd.spawn().err_tip(|| {
            format!(
                "Failed to spawn persistent worker for key: {}",
                persistent_key.key
            )
        })?;

        // Store the spawned worker information
        {
            let mut spawned = self.spawned_workers.write();
            spawned.insert(
                worker_id.to_string(),
                SpawnedWorker {
                    worker_id: worker_id.clone(),
                    process_handle,
                    persistent_key: persistent_key.key.clone(),
                },
            );
        }

        Ok(worker_id)
    }

    /// Check if a persistent worker needs to be spawned
    pub async fn maybe_spawn_persistent_worker(
        &self,
        action_info: &ActionInfo,
    ) -> Option<WorkerId> {
        // Extract platform from action_info
        // Check if this action requires a persistent worker
        // Create a temporary PlatformProperties object
        let platform_props = PlatformProperties::new(
            action_info
                .platform_properties
                .iter()
                .map(|(k, v)| {
                    (
                        k.clone(),
                        nativelink_util::platform_properties::PlatformPropertyValue::Unknown(
                            v.clone(),
                        ),
                    )
                })
                .collect(),
        );
        if let Some(persistent_key) = self.extract_persistent_key_from_platform(&platform_props) {
            // Check if we already have a worker for this key
            if !self.has_worker_for_key(&persistent_key.key) {
                // Spawn new persistent worker
                match self.fork_persistent_worker(&persistent_key).await {
                    Ok(worker_id) => {
                        info!(
                            "Spawned new persistent worker {} for key: {}",
                            worker_id, persistent_key.key
                        );
                        return Some(worker_id);
                    }
                    Err(err) => {
                        error!("Failed to spawn persistent worker: {:?}", err);
                    }
                }
            }
        }
        None
    }

    fn extract_persistent_key_from_platform(
        &self,
        platform: &PlatformProperties,
    ) -> Option<PersistentWorkerKey> {
        let mut persistent_key = None;
        let mut tool = None;
        let mut properties = HashMap::new();

        for (key, value) in &platform.properties {
            let value_str = match value {
                nativelink_util::platform_properties::PlatformPropertyValue::Exact(s)
                | nativelink_util::platform_properties::PlatformPropertyValue::Priority(s)
                | nativelink_util::platform_properties::PlatformPropertyValue::Unknown(s) => {
                    s.clone()
                }
                nativelink_util::platform_properties::PlatformPropertyValue::Minimum(v) => {
                    v.to_string()
                }
            };
            if key.as_str() == "persistentWorkerKey" {
                persistent_key = Some(value_str);
            } else if key.as_str() == "persistentWorkerTool" {
                tool = Some(value_str);
            } else {
                properties.insert(key.clone(), value_str);
            }
        }

        match (persistent_key, tool) {
            (Some(key), Some(tool)) => Some(PersistentWorkerKey {
                tool,
                key,
                platform_properties: properties.into_iter().collect(),
            }),
            _ => None,
        }
    }

    fn has_worker_for_key(&self, key: &str) -> bool {
        let spawned = self.spawned_workers.read();
        spawned.values().any(|w| w.persistent_key == key)
    }

    /// Clean up terminated workers
    pub async fn cleanup_terminated_workers(&self) {
        let mut to_remove = Vec::new();

        {
            let mut spawned = self.spawned_workers.write();
            for (id, worker) in spawned.iter_mut() {
                // Check if process is still running
                match worker.process_handle.try_wait() {
                    Ok(Some(status)) => {
                        info!(
                            "Persistent worker {} terminated with status: {:?}",
                            id, status
                        );
                        to_remove.push(id.clone());
                    }
                    Ok(None) => {
                        // Still running
                    }
                    Err(err) => {
                        error!("Failed to check worker {} status: {:?}", id, err);
                        to_remove.push(id.clone());
                    }
                }
            }

            for id in to_remove {
                spawned.remove(&id);
            }
        }
    }

    /// Shutdown all spawned workers
    pub async fn shutdown_all(&self) {
        let workers_to_shutdown = {
            let mut spawned = self.spawned_workers.write();
            spawned.drain().collect::<Vec<_>>()
        };

        for (id, mut worker) in workers_to_shutdown {
            info!("Shutting down persistent worker: {}", id);
            if let Err(err) = worker.process_handle.kill().await {
                error!("Failed to kill worker {}: {:?}", id, err);
            }
        }

        self.persistent_worker_manager.shutdown_all().await;
    }
}

/// Worker-side persistent execution handler
#[allow(dead_code)]
#[derive(Debug)]
pub struct PersistentWorkerExecutor {
    /// The persistent worker manager
    manager: Arc<PersistentWorkerManager>,

    /// Worker configuration
    config: LocalWorkerConfig,
}

impl PersistentWorkerExecutor {
    pub fn new(config: LocalWorkerConfig, work_dir: PathBuf) -> Self {
        let manager = Arc::new(PersistentWorkerManager::new(
            work_dir,
            core::time::Duration::from_secs(300),
            10,
        ));

        Self { manager, config }
    }

    /// Execute an action using persistent workers if applicable
    pub async fn maybe_execute_as_persistent(
        &self,
        action_info: &ActionInfo,
        command: &nativelink_proto::build::bazel::remote::execution::v2::Command,
        platform: &Platform,
    ) -> Option<Result<nativelink_util::action_messages::ActionResult, Error>> {
        // Check if this is a persistent worker action
        if PersistentWorkerKey::from_platform(platform).is_some() {
            tracing::debug!("Executing action as persistent worker");
            Some(
                self.manager
                    .execute_with_persistent_worker(action_info, command, platform)
                    .await,
            )
        } else {
            None
        }
    }
}
