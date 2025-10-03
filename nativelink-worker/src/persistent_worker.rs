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

use core::time::Duration;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;

use bytes::Bytes;
use nativelink_error::{Code, Error, ResultExt};
use nativelink_metric::MetricsComponent;
use nativelink_proto::build::bazel::remote::execution::v2::{Command as ProtoCommand, Platform};
use nativelink_util::action_messages::{ActionInfo, ActionResult, ExecutionMetadata};
use nativelink_util::background_spawn;
use nativelink_util::common::DigestInfo;
use parking_lot::Mutex;
use tokio::process::{Child, Command};
use tokio::sync::{RwLock, mpsc};
use tokio::time::Instant;
use tracing::info;

/// Key used to identify persistent workers based on tool and configuration
#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PersistentWorkerKey {
    /// The tool being executed (e.g., javac, scalac)
    pub tool: String,
    /// Unique key identifying this persistent worker configuration
    pub key: String,
    /// Platform properties for matching
    pub platform_properties: BTreeMap<String, String>,
}

impl PersistentWorkerKey {
    pub fn from_platform(platform: &Platform) -> Option<Self> {
        let mut properties = BTreeMap::new();
        let mut persistent_key = None;
        let mut tool = None;

        for property in &platform.properties {
            if property.name == "persistentWorkerKey" {
                persistent_key = Some(property.value.clone());
            } else if property.name == "persistentWorkerTool" {
                tool = Some(property.value.clone());
            } else {
                properties.insert(property.name.clone(), property.value.clone());
            }
        }

        match (persistent_key, tool) {
            (Some(key), Some(tool)) => Some(Self {
                tool,
                key,
                platform_properties: properties,
            }),
            _ => None,
        }
    }

    pub fn to_platform_properties(&self) -> Vec<(String, String)> {
        let mut props = vec![
            ("persistentWorkerKey".to_string(), self.key.clone()),
            ("persistentWorkerTool".to_string(), self.tool.clone()),
        ];
        for (k, v) in &self.platform_properties {
            props.push((k.clone(), v.clone()));
        }
        props
    }
}

/// Manages lifecycle of a single persistent worker process
#[allow(dead_code)]
pub struct PersistentWorkerInstance {
    key: PersistentWorkerKey,
    process: Arc<Mutex<Option<Child>>>,
    work_dir: PathBuf,
    last_used: Arc<RwLock<Instant>>,
    request_count: Arc<RwLock<u64>>,
    stdin_tx: Option<mpsc::Sender<Bytes>>,
    stdout_rx: Option<mpsc::Receiver<Bytes>>,
}

impl PersistentWorkerInstance {
    pub async fn new(
        key: PersistentWorkerKey,
        work_dir: PathBuf,
        command: &ProtoCommand,
    ) -> Result<Self, Error> {
        // Create working directory for this persistent worker
        tokio::fs::create_dir_all(&work_dir)
            .await
            .err_tip(|| format!("Failed to create work dir: {}", work_dir.display()))?;

        // Start the persistent worker process with --persistent_worker flag
        let mut cmd = Command::new(&command.arguments[0]);
        cmd.args(&command.arguments[1..]);
        cmd.arg("--persistent_worker");
        cmd.current_dir(&work_dir);
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        // Set environment variables
        for env_var in &command.environment_variables {
            cmd.env(&env_var.name, &env_var.value);
        }

        let mut process = cmd
            .spawn()
            .err_tip(|| format!("Failed to spawn persistent worker: {}", key.tool))?;

        // Set up communication channels
        let (stdin_tx, mut stdin_rx) = mpsc::channel::<Bytes>(100);
        let (stdout_tx, stdout_rx) = mpsc::channel::<Bytes>(100);

        // Forward stdin
        let stdin = process
            .stdin
            .take()
            .ok_or_else(|| Error::new(Code::Internal, "Failed to get stdin".to_string()))?;
        background_spawn!(name: "stdin_forwarder", fut: async move {
            use tokio::io::AsyncWriteExt;
            let mut stdin = stdin;
            while let Some(data) = stdin_rx.recv().await {
                if stdin.write_all(&data).await.is_err() {
                    break;
                }
            }
        }, target: "nativelink::persistent_worker",);

        // Forward stdout
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| Error::new(Code::Internal, "Failed to get stdout".to_string()))?;
        background_spawn!(name: "stdout_forwarder", fut: async move {
            use tokio::io::AsyncReadExt;
            let mut stdout = stdout;
            let mut buffer = vec![0u8; 8192];
            loop {
                match stdout.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if stdout_tx
                            .send(Bytes::copy_from_slice(&buffer[..n]))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        }, target: "nativelink::persistent_worker",);

        Ok(Self {
            key,
            process: Arc::new(Mutex::new(Some(process))),
            work_dir,
            last_used: Arc::new(RwLock::new(Instant::now())),
            request_count: Arc::new(RwLock::new(0)),
            stdin_tx: Some(stdin_tx),
            stdout_rx: Some(stdout_rx),
        })
    }

    pub async fn execute_action(
        &mut self,
        action_info: &ActionInfo,
        command: &ProtoCommand,
    ) -> Result<ActionResult, Error> {
        // Update usage statistics
        {
            let mut last_used = self.last_used.write().await;
            *last_used = Instant::now();
            let mut count = self.request_count.write().await;
            *count += 1;
        }

        // Send work request to persistent worker
        let request = self.build_work_request(action_info, command)?;

        let stdin_tx = self
            .stdin_tx
            .as_ref()
            .ok_or_else(|| Error::new(Code::Internal, "stdin channel closed".to_string()))?;

        stdin_tx
            .send(request)
            .await
            .map_err(|_| Error::new(Code::Internal, "Failed to send work request".to_string()))?;

        // Receive work response
        let stdout_rx = self
            .stdout_rx
            .as_mut()
            .ok_or_else(|| Error::new(Code::Internal, "stdout channel closed".to_string()))?;

        let response = stdout_rx
            .recv()
            .await
            .ok_or_else(|| Error::new(Code::Internal, "No response from worker".to_string()))?;

        self.parse_work_response(response)
    }

    fn build_work_request(
        &self,
        action_info: &ActionInfo,
        command: &ProtoCommand,
    ) -> Result<Bytes, Error> {
        // Build work request protocol buffer for persistent worker
        // This follows the Bazel persistent worker protocol
        use nativelink_proto::build::bazel::remote::execution::worker_protocol::WorkRequest;
        use prost::Message;

        let work_request = WorkRequest {
            arguments: command.arguments.clone(),
            inputs: vec![], // Will be populated based on action inputs
            request_id: format!("{}", action_info.unique_qualifier),
            cancel: false,
            verbosity: 0,
            sandbox_dir: String::new(),
        };

        let mut buf = Vec::new();
        work_request
            .encode(&mut buf)
            .err_tip(|| "Failed to encode work request")?;

        Ok(Bytes::from(buf))
    }

    fn parse_work_response(&self, response: Bytes) -> Result<ActionResult, Error> {
        use nativelink_proto::build::bazel::remote::execution::worker_protocol::WorkResponse;
        use prost::Message;

        let work_response =
            WorkResponse::decode(response).err_tip(|| "Failed to decode work response")?;

        if work_response.exit_code != 0 {
            return Err(Error::new(
                Code::Internal,
                format!(
                    "Worker returned non-zero exit code: {}",
                    work_response.exit_code
                ),
            ));
        }

        // Convert work response to ActionResult
        Ok(ActionResult {
            output_files: vec![],
            output_folders: vec![],
            output_file_symlinks: vec![],
            output_directory_symlinks: vec![],
            exit_code: work_response.exit_code,
            stdout_digest: DigestInfo::zero_digest(),
            stderr_digest: DigestInfo::zero_digest(),
            execution_metadata: ExecutionMetadata::default(),
            server_logs: HashMap::new(),
            error: None,
            message: String::new(),
        })
    }

    pub async fn shutdown(&mut self) {
        let process_opt = self.process.lock().take();
        if let Some(mut process) = process_opt {
            drop(process.kill().await);
        }
    }

    pub fn is_alive(&self) -> bool {
        if let Some(ref _process) = *self.process.lock() {
            // Check if process is still running
            true // Simplified for now
        } else {
            false
        }
    }

    pub async fn get_stats(&self) -> WorkerStats {
        WorkerStats {
            last_used: *self.last_used.read().await,
            request_count: *self.request_count.read().await,
            is_alive: self.is_alive(),
        }
    }
}

impl core::fmt::Debug for PersistentWorkerInstance {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PersistentWorkerInstance")
            .field("key", &self.key)
            .field("work_dir", &self.work_dir)
            .finish()
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WorkerStats {
    pub last_used: Instant,
    pub request_count: u64,
    pub is_alive: bool,
}

/// Manages pool of persistent workers
#[allow(dead_code)]
#[derive(MetricsComponent)]
pub struct PersistentWorkerManager {
    workers: Arc<RwLock<HashMap<PersistentWorkerKey, Arc<RwLock<PersistentWorkerInstance>>>>>,
    base_work_dir: PathBuf,
    max_idle_duration: Duration,
    max_worker_instances: usize,
}

impl PersistentWorkerManager {
    pub fn new(
        base_work_dir: PathBuf,
        max_idle_duration: Duration,
        max_worker_instances: usize,
    ) -> Self {
        let manager = Self {
            workers: Arc::new(RwLock::new(HashMap::new())),
            base_work_dir,
            max_idle_duration,
            max_worker_instances,
        };

        // Start cleanup task
        let workers = manager.workers.clone();
        let max_idle = max_idle_duration;
        background_spawn!(name: "persistent_worker_cleanup", fut: async move {
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                Self::cleanup_idle_workers(&workers, max_idle).await;
            }
        }, target: "nativelink::persistent_worker",);

        manager
    }

    #[allow(clippy::type_complexity)]
    async fn cleanup_idle_workers(
        workers: &Arc<RwLock<HashMap<PersistentWorkerKey, Arc<RwLock<PersistentWorkerInstance>>>>>,
        max_idle: Duration,
    ) {
        let now = Instant::now();
        let mut workers_guard = workers.write().await;
        let mut to_remove = Vec::new();

        for (key, worker) in workers_guard.iter() {
            let stats = worker.read().await.get_stats().await;
            if now.duration_since(stats.last_used) > max_idle || !stats.is_alive {
                to_remove.push(key.clone());
            }
        }

        for key in to_remove {
            if let Some(worker) = workers_guard.remove(&key) {
                let mut w_guard = worker.write().await;
                w_guard.shutdown().await;
                info!("Cleaned up idle persistent worker: {:?}", key);
            }
        }
    }

    pub async fn get_or_create_worker(
        &self,
        key: &PersistentWorkerKey,
        command: &ProtoCommand,
    ) -> Result<Arc<RwLock<PersistentWorkerInstance>>, Error> {
        // Check if worker already exists
        {
            let workers = self.workers.read().await;
            if let Some(worker) = workers.get(key) {
                let stats = worker.read().await.get_stats().await;
                if stats.is_alive {
                    return Ok(worker.clone());
                }
            }
        }

        // Create new worker
        let work_dir = self.base_work_dir.join(format!("pw_{}", key.key));
        let worker = PersistentWorkerInstance::new(key.clone(), work_dir, command).await?;

        let worker = Arc::new(RwLock::new(worker));

        // Store worker
        {
            let mut workers = self.workers.write().await;

            // Check capacity
            if workers.len() >= self.max_worker_instances {
                // Find and remove least recently used worker
                let mut lru_key = None;
                let mut oldest_time = Instant::now();

                for (k, w) in workers.iter() {
                    let stats = w.read().await.get_stats().await;
                    if stats.last_used < oldest_time {
                        oldest_time = stats.last_used;
                        lru_key = Some(k.clone());
                    }
                }

                if let Some(key_to_remove) = lru_key {
                    if let Some(old_worker) = workers.remove(&key_to_remove) {
                        let mut old_worker_guard = old_worker.write().await;
                        old_worker_guard.shutdown().await;
                    }
                }
            }

            workers.insert(key.clone(), worker.clone());
        }

        Ok(worker)
    }

    pub async fn execute_with_persistent_worker(
        &self,
        action_info: &ActionInfo,
        command: &ProtoCommand,
        platform: &Platform,
    ) -> Result<ActionResult, Error> {
        let key = PersistentWorkerKey::from_platform(platform).ok_or_else(|| {
            Error::new(
                Code::InvalidArgument,
                "No persistent worker key in platform".to_string(),
            )
        })?;

        let worker = self.get_or_create_worker(&key, command).await?;
        let mut worker_guard = worker.write().await;

        worker_guard.execute_action(action_info, command).await
    }

    pub async fn shutdown_all(&self) {
        let mut workers = self.workers.write().await;
        for (_, worker) in workers.drain() {
            let mut worker_guard = worker.write().await;
            worker_guard.shutdown().await;
        }
    }

    pub async fn get_all_stats(&self) -> HashMap<PersistentWorkerKey, WorkerStats> {
        let workers = self.workers.read().await;
        let mut stats = HashMap::new();

        for (key, worker) in workers.iter() {
            stats.insert(key.clone(), worker.read().await.get_stats().await);
        }

        stats
    }
}

impl core::fmt::Debug for PersistentWorkerManager {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PersistentWorkerManager")
            .field("base_work_dir", &self.base_work_dir)
            .field("max_idle_duration", &self.max_idle_duration)
            .field("max_worker_instances", &self.max_worker_instances)
            .finish()
    }
}
