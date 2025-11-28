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

//! Native gRPC client for CRI-O communication.
//!
//! This implementation provides direct gRPC communication with CRI-O over Unix sockets,
//! replacing the slower crictl CLI-based approach.

use core::time::Duration;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use hyper_util::rt::TokioIo;
use nativelink_error::{Code, Error, ResultExt, make_err};
use tonic::transport::{Channel, Endpoint, Uri};
use tower::service_fn;

// Generated from CRI protocol buffers
#[allow(
    clippy::all,
    clippy::pedantic,
    clippy::restriction,
    clippy::nursery,
    unused_qualifications,
    unreachable_pub,
    dead_code,
    unused_imports
)]
pub mod runtime {
    // When building with Bazel, include the generated proto directly
    #[cfg(not(feature = "cargo-build"))]
    include!("../runtime.v1.pb.rs");

    // When building with Cargo, use tonic's include_proto macro
    #[cfg(feature = "cargo-build")]
    tonic::include_proto!("runtime.v1");
}

use runtime::{
    image_service_client::ImageServiceClient, runtime_service_client::RuntimeServiceClient, *,
};

/// Native gRPC client for CRI-O.
///
/// This client communicates directly with CRI-O over a Unix domain socket using gRPC,
/// providing much better performance than spawning crictl processes.
#[derive(Clone, Debug)]
pub struct CriClientGrpc {
    inner: Arc<CriClientInner>,
}

#[derive(Debug)]
struct CriClientInner {
    runtime_client: RuntimeServiceClient<Channel>,
    image_client: ImageServiceClient<Channel>,
}

impl CriClientGrpc {
    /// Create a new gRPC CRI client connected to the specified Unix socket.
    ///
    /// # Arguments
    /// * `socket_path` - Path to the CRI-O Unix socket (e.g., /var/run/crio/crio.sock)
    ///
    /// # Example
    /// ```no_run
    /// # use nativelink_crio_worker_pool::CriClientGrpc;
    /// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
    /// let client = CriClientGrpc::new("unix:///var/run/crio/crio.sock").await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn new(socket_path: impl AsRef<Path>) -> Result<Self, Error> {
        let socket_path = socket_path.as_ref().to_path_buf();

        // Create a channel that connects to the Unix domain socket
        let channel = Endpoint::try_from("http://[::]:50051")
            .map_err(|e| make_err!(Code::Internal, "Failed to create endpoint: {e}"))?
            .connect_with_connector(service_fn(move |_: Uri| {
                let path = socket_path.clone();
                async move {
                    tokio::net::UnixStream::connect(path)
                        .await
                        .map(TokioIo::new)
                        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
                }
            }))
            .await
            .map_err(|e| make_err!(Code::Internal, "Failed to connect to CRI-O socket: {e}"))?;

        Ok(Self {
            inner: Arc::new(CriClientInner {
                runtime_client: RuntimeServiceClient::new(channel.clone()),
                image_client: ImageServiceClient::new(channel),
            }),
        })
    }

    /// Get the CRI runtime version.
    pub async fn version(&self) -> Result<VersionResponse, Error> {
        let mut client = self.inner.runtime_client.clone();
        let response = client
            .version(VersionRequest {
                version: "v1".to_string(),
            })
            .await
            .err_tip(|| "Failed to get CRI version")?;
        Ok(response.into_inner())
    }

    /// Pull a container image.
    pub async fn pull_image(&self, image: &str) -> Result<String, Error> {
        let mut client = self.inner.image_client.clone();
        let response = client
            .pull_image(PullImageRequest {
                image: Some(ImageSpec {
                    image: image.to_string(),
                    annotations: HashMap::new(),
                }),
                auth: None,
                sandbox_config: None,
            })
            .await
            .err_tip(|| format!("Failed to pull image: {image}"))?;
        Ok(response.into_inner().image_ref)
    }

    /// Create and start a pod sandbox.
    pub async fn run_pod_sandbox(
        &self,
        name: &str,
        namespace: &str,
        uid: &str,
        labels: HashMap<String, String>,
    ) -> Result<String, Error> {
        let mut client = self.inner.runtime_client.clone();

        let config = PodSandboxConfig {
            metadata: Some(PodSandboxMetadata {
                name: name.to_string(),
                uid: uid.to_string(),
                namespace: namespace.to_string(),
                attempt: 0,
            }),
            hostname: name.to_string(),
            log_directory: "/var/log/pods".to_string(),
            dns_config: None,
            port_mappings: vec![],
            labels,
            annotations: HashMap::new(),
            linux: Some(LinuxPodSandboxConfig {
                cgroup_parent: String::new(),
                security_context: Some(LinuxSandboxSecurityContext {
                    namespace_options: Some(NamespaceOption {
                        network: 2, // NETWORK_PRIVATE
                        pid: 2,     // PID_PRIVATE
                        ipc: 2,     // IPC_PRIVATE
                    }),
                    ..Default::default()
                }),
            }),
        };

        let response = client
            .run_pod_sandbox(RunPodSandboxRequest {
                config: Some(config),
                runtime_handler: String::new(),
            })
            .await
            .err_tip(|| format!("Failed to create pod sandbox: {name}"))?;

        Ok(response.into_inner().pod_sandbox_id)
    }

    /// Create a container within a pod sandbox.
    pub async fn create_container(
        &self,
        sandbox_id: &str,
        name: &str,
        image: &str,
        command: Vec<String>,
        args: Vec<String>,
        env: HashMap<String, String>,
        working_dir: Option<String>,
    ) -> Result<String, Error> {
        let mut client = self.inner.runtime_client.clone();

        let container_config = ContainerConfig {
            metadata: Some(ContainerMetadata {
                name: name.to_string(),
                attempt: 0,
            }),
            image: Some(ImageSpec {
                image: image.to_string(),
                annotations: HashMap::new(),
            }),
            command,
            args,
            working_dir: working_dir.unwrap_or_default(),
            envs: env
                .into_iter()
                .map(|(key, value)| KeyValue { key, value })
                .collect(),
            mounts: vec![],
            devices: vec![],
            labels: HashMap::new(),
            annotations: HashMap::new(),
            log_path: format!("{name}.log"),
            stdin: false,
            stdin_once: false,
            tty: false,
            linux: Some(LinuxContainerConfig {
                resources: Some(LinuxContainerResources::default()),
                security_context: None,
            }),
        };

        let response = client
            .create_container(CreateContainerRequest {
                pod_sandbox_id: sandbox_id.to_string(),
                config: Some(container_config),
                sandbox_config: None, // We already have the sandbox
            })
            .await
            .err_tip(|| format!("Failed to create container: {name}"))?;

        Ok(response.into_inner().container_id)
    }

    /// Start a container.
    pub async fn start_container(&self, container_id: &str) -> Result<(), Error> {
        let mut client = self.inner.runtime_client.clone();
        client
            .start_container(StartContainerRequest {
                container_id: container_id.to_string(),
            })
            .await
            .err_tip(|| format!("Failed to start container: {container_id}"))?;
        Ok(())
    }

    /// Execute a command in a container synchronously.
    pub async fn exec_sync(
        &self,
        container_id: &str,
        command: Vec<String>,
        timeout: Duration,
    ) -> Result<ExecResult, Error> {
        let mut client = self.inner.runtime_client.clone();

        let response = client
            .exec_sync(ExecSyncRequest {
                container_id: container_id.to_string(),
                cmd: command,
                timeout: timeout.as_secs() as i64,
            })
            .await
            .err_tip(|| format!("Failed to exec in container: {container_id}"))?;

        let inner = response.into_inner();
        Ok(ExecResult {
            stdout: String::from_utf8_lossy(&inner.stdout).to_string(),
            stderr: String::from_utf8_lossy(&inner.stderr).to_string(),
            exit_code: inner.exit_code,
        })
    }

    /// Stop a container.
    pub async fn stop_container(&self, container_id: &str, timeout: Duration) -> Result<(), Error> {
        let mut client = self.inner.runtime_client.clone();
        client
            .stop_container(StopContainerRequest {
                container_id: container_id.to_string(),
                timeout: timeout.as_secs() as i64,
            })
            .await
            .err_tip(|| format!("Failed to stop container: {container_id}"))?;
        Ok(())
    }

    /// Remove a container.
    pub async fn remove_container(&self, container_id: &str) -> Result<(), Error> {
        let mut client = self.inner.runtime_client.clone();
        client
            .remove_container(RemoveContainerRequest {
                container_id: container_id.to_string(),
            })
            .await
            .err_tip(|| format!("Failed to remove container: {container_id}"))?;
        Ok(())
    }

    /// Stop a pod sandbox.
    pub async fn stop_pod_sandbox(&self, sandbox_id: &str) -> Result<(), Error> {
        let mut client = self.inner.runtime_client.clone();
        client
            .stop_pod_sandbox(StopPodSandboxRequest {
                pod_sandbox_id: sandbox_id.to_string(),
            })
            .await
            .err_tip(|| format!("Failed to stop pod sandbox: {sandbox_id}"))?;
        Ok(())
    }

    /// Remove a pod sandbox.
    pub async fn remove_pod_sandbox(&self, sandbox_id: &str) -> Result<(), Error> {
        let mut client = self.inner.runtime_client.clone();
        client
            .remove_pod_sandbox(RemovePodSandboxRequest {
                pod_sandbox_id: sandbox_id.to_string(),
            })
            .await
            .err_tip(|| format!("Failed to remove pod sandbox: {sandbox_id}"))?;
        Ok(())
    }

    /// Get container statistics.
    pub async fn container_stats(&self, container_id: &str) -> Result<ContainerStats, Error> {
        let mut client = self.inner.runtime_client.clone();
        let response = client
            .container_stats(ContainerStatsRequest {
                container_id: container_id.to_string(),
            })
            .await
            .err_tip(|| format!("Failed to get container stats: {container_id}"))?;

        response
            .into_inner()
            .stats
            .ok_or_else(|| make_err!(Code::Internal, "No stats returned"))
    }

    /// Get container status.
    pub async fn container_status(&self, container_id: &str) -> Result<ContainerStatus, Error> {
        let mut client = self.inner.runtime_client.clone();
        let response = client
            .container_status(ContainerStatusRequest {
                container_id: container_id.to_string(),
                verbose: false,
            })
            .await
            .err_tip(|| format!("Failed to get container status: {container_id}"))?;

        response
            .into_inner()
            .status
            .ok_or_else(|| make_err!(Code::Internal, "No status returned"))
    }
}

/// Result of an exec_sync operation.
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}
