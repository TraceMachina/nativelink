use core::time::Duration;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use nativelink_error::{Code, Error, ResultExt, make_err};
use serde::Serialize;
use tempfile::NamedTempFile;
use tokio::process::Command;

/// Thin wrapper around the `crictl` CLI to talk to CRI-O.
#[derive(Clone, Debug)]
pub struct CriClient {
    inner: Arc<CriClientInner>,
}

#[derive(Debug)]
struct CriClientInner {
    binary: PathBuf,
    runtime_endpoint: String,
    image_endpoint: Option<String>,
}

impl CriClient {
    #[must_use]
    pub fn new(
        binary: impl Into<PathBuf>,
        runtime_endpoint: impl Into<String>,
        image_endpoint: Option<String>,
    ) -> Self {
        Self {
            inner: Arc::new(CriClientInner {
                binary: binary.into(),
                runtime_endpoint: runtime_endpoint.into(),
                image_endpoint,
            }),
        }
    }

    pub async fn pull_image(&self, image: &str) -> Result<(), Error> {
        self.run_crictl(vec!["pull".into(), image.into()])
            .await
            .err_tip(|| format!("while pulling image {image}"))?;
        Ok(())
    }

    pub async fn run_pod_sandbox(&self, config: &PodSandboxConfig) -> Result<String, Error> {
        let config_file = Self::write_config_file(config)?;
        self.run_crictl(vec![
            "runp".into(),
            config_file.path().display().to_string(),
        ])
        .await
        .err_tip(|| "while creating pod sandbox")
    }

    pub async fn create_container(
        &self,
        sandbox_id: &str,
        container_config: &ContainerConfig,
        sandbox_config: &PodSandboxConfig,
    ) -> Result<String, Error> {
        let container_file = Self::write_config_file(container_config)?;
        let sandbox_file = Self::write_config_file(sandbox_config)?;
        self.run_crictl(vec![
            "create".into(),
            sandbox_id.into(),
            container_file.path().display().to_string(),
            sandbox_file.path().display().to_string(),
        ])
        .await
        .err_tip(|| "while creating container")
    }

    pub async fn start_container(&self, container_id: &str) -> Result<(), Error> {
        self.run_crictl(vec!["start".into(), container_id.into()])
            .await
            .err_tip(|| format!("while starting container {container_id}"))?;
        Ok(())
    }

    pub async fn stop_container(&self, container_id: &str) -> Result<(), Error> {
        self.run_crictl(vec!["stop".into(), container_id.into()])
            .await
            .err_tip(|| format!("while stopping container {container_id}"))?;
        Ok(())
    }

    pub async fn remove_container(&self, container_id: &str) -> Result<(), Error> {
        self.run_crictl(vec!["rm".into(), container_id.into()])
            .await
            .err_tip(|| format!("while removing container {container_id}"))?;
        Ok(())
    }

    pub async fn stop_pod(&self, sandbox_id: &str) -> Result<(), Error> {
        self.run_crictl(vec!["stopp".into(), sandbox_id.into()])
            .await
            .err_tip(|| format!("while stopping sandbox {sandbox_id}"))?;
        Ok(())
    }

    pub async fn remove_pod(&self, sandbox_id: &str) -> Result<(), Error> {
        self.run_crictl(vec!["rmp".into(), sandbox_id.into()])
            .await
            .err_tip(|| format!("while removing sandbox {sandbox_id}"))?;
        Ok(())
    }

    pub async fn exec(
        &self,
        container_id: &str,
        argv: Vec<String>,
        timeout: Duration,
    ) -> Result<ExecResult, Error> {
        if argv.is_empty() {
            return Err(make_err!(Code::InvalidArgument, "exec requires argv"));
        }

        let mut args = vec![
            "exec".into(),
            "--timeout".into(),
            format!("{}s", timeout.as_secs()),
            container_id.into(),
            "--".into(),
        ];
        args.extend(argv);
        let output = self
            .run_crictl_raw(args)
            .await
            .err_tip(|| format!("while exec'ing in container {container_id}"))?;
        Ok(ExecResult {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    }

    async fn run_crictl(&self, args: Vec<String>) -> Result<String, Error> {
        let output = self.run_crictl_raw(args).await?;
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    async fn run_crictl_raw(&self, args: Vec<String>) -> Result<std::process::Output, Error> {
        let mut cmd = Command::new(&self.inner.binary);
        cmd.arg("--runtime-endpoint")
            .arg(&self.inner.runtime_endpoint);
        if let Some(image_endpoint) = &self.inner.image_endpoint {
            cmd.arg("--image-endpoint").arg(image_endpoint);
        }
        cmd.args(&args);
        let output = cmd.output().await?;
        if !output.status.success() {
            return Err(make_err!(
                Code::Internal,
                "crictl {:?} failed: {}",
                args,
                String::from_utf8_lossy(&output.stderr)
            ));
        }
        Ok(output)
    }

    fn write_config_file<T: Serialize>(value: &T) -> Result<NamedTempFile, Error> {
        let mut file = NamedTempFile::new()
            .map_err(|err| make_err!(Code::Internal, "unable to create temp file: {err}"))?;
        serde_json::to_writer_pretty(file.as_file_mut(), value)
            .map_err(|err| make_err!(Code::Internal, "failed to serialize CRI config: {err}"))?;
        file.as_file_mut()
            .sync_all()
            .map_err(|err| make_err!(Code::Internal, "unable to flush CRI config: {err}"))?;
        Ok(file)
    }
}

/// Response for an exec call.
#[derive(Debug, Clone)]
pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PodSandboxMetadata {
    pub name: String,
    pub namespace: String,
    pub uid: String,
    pub attempt: u32,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct NamespaceOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ipc: Option<i32>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct LinuxSandboxSecurityContext {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace_options: Option<NamespaceOptions>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct LinuxPodSandboxConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub security_context: Option<LinuxSandboxSecurityContext>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PodSandboxConfig {
    pub metadata: PodSandboxMetadata,
    pub hostname: String,
    pub log_directory: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_config: Option<HashMap<String, String>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub port_mappings: Vec<HashMap<String, String>>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub labels: HashMap<String, String>,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub annotations: HashMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linux: Option<LinuxPodSandboxConfig>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerMetadata {
    pub name: String,
    pub attempt: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImageSpec {
    pub image: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeyValue {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Mount {
    pub container_path: String,
    pub host_path: String,
    #[serde(default)]
    pub readonly: bool,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct LinuxContainerResources {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_period: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_quota: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory_limit_in_bytes: Option<i64>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct LinuxContainerConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<LinuxContainerResources>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ContainerConfig {
    pub metadata: ContainerMetadata,
    pub image: ImageSpec,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub working_dir: Option<String>,
    #[serde(default)]
    pub envs: Vec<KeyValue>,
    #[serde(default)]
    pub mounts: Vec<Mount>,
    pub log_path: String,
    pub stdin: bool,
    pub stdin_once: bool,
    pub tty: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub linux: Option<LinuxContainerConfig>,
}
