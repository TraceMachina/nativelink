use core::time::Duration;
use std::collections::HashMap;

use nativelink_config::warm_worker_pools::IsolationConfig;
use serde::{Deserialize, Serialize};
use serde_with::{DisplayFromStr, serde_as};

/// Root configuration for the warm worker pool manager.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WarmWorkerPoolsConfig {
    /// All pools managed by the service.
    #[serde(default)]
    pub pools: Vec<WorkerPoolConfig>,
}

impl WarmWorkerPoolsConfig {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.pools.is_empty()
    }
}

/// Supported language runtimes.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Jvm,
    NodeJs,
    Custom(String),
}

impl Language {
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match self {
            Self::Jvm => "jvm",
            Self::NodeJs => "nodejs",
            Self::Custom(value) => value.as_str(),
        }
    }
}

fn default_namespace() -> String {
    "nativelink".to_string()
}

const fn default_min_warm_workers() -> usize {
    2
}

const fn default_max_workers() -> usize {
    20
}

const fn default_worker_ttl_seconds() -> u64 {
    3600
}

const fn default_max_jobs_per_worker() -> usize {
    200
}

const fn default_gc_frequency() -> usize {
    25
}

fn default_crictl_binary() -> String {
    "crictl".to_string()
}

fn default_worker_command() -> Vec<String> {
    vec!["/usr/local/bin/nativelink-worker".to_string()]
}

/// Per-pool configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerPoolConfig {
    /// Pool name used for lookups and telemetry.
    pub name: String,
    /// Logical language runtime for the workers.
    pub language: Language,
    /// Path to the CRI-O unix socket.
    pub cri_socket: String,
    /// Optional dedicated image endpoint (defaults to runtime socket).
    #[serde(default)]
    pub image_socket: Option<String>,
    /// Container image to boot.
    pub container_image: String,
    /// CLI to interact with CRI.
    #[serde(default = "default_crictl_binary")]
    pub crictl_binary: String,
    /// Namespace for sandbox metadata.
    #[serde(default = "default_namespace")]
    pub namespace: String,
    /// Minimum number of warmed workers to keep ready.
    #[serde(default = "default_min_warm_workers")]
    pub min_warm_workers: usize,
    /// Maximum containers allowed in the pool.
    #[serde(default = "default_max_workers")]
    pub max_workers: usize,
    /// Command executed inside the container when the worker process starts.
    #[serde(default = "default_worker_command")]
    pub worker_command: Vec<String>,
    /// Arguments passed to the worker binary.
    #[serde(default)]
    pub worker_args: Vec<String>,
    /// Environment variables for the worker entrypoint.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Optional working directory for the worker process.
    #[serde(default)]
    pub working_directory: Option<String>,
    /// Warmup definition for the pool.
    #[serde(default)]
    pub warmup: WarmupConfig,
    /// Cache priming instructions.
    #[serde(default)]
    pub cache: CachePrimingConfig,
    /// Lifecycle configuration.
    #[serde(default)]
    pub lifecycle: LifecycleConfig,
    /// Isolation configuration for security between jobs.
    #[serde(default)]
    pub isolation: Option<IsolationConfig>,
}

impl WorkerPoolConfig {
    #[must_use]
    pub const fn worker_timeout(&self) -> Duration {
        Duration::from_secs(self.warmup.default_timeout_s)
    }
}

/// Warmup command executed inside the worker container.
#[serde_as]
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WarmupCommand {
    /// Command argv executed inside the worker container.
    pub argv: Vec<String>,
    /// Optional environment overrides for the command.
    #[serde(default)]
    pub env: HashMap<String, String>,
    /// Optional working directory.
    #[serde(default)]
    pub working_directory: Option<String>,
    /// Optional timeout override in seconds.
    #[serde(default)]
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub timeout_s: Option<u64>,
}

impl WarmupCommand {
    #[must_use]
    pub fn timeout(&self, fallback: u64) -> Duration {
        Duration::from_secs(self.timeout_s.unwrap_or(fallback))
    }
}

/// Warmup configuration for a pool.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WarmupConfig {
    /// Commands that bring the runtime to a hot state.
    #[serde(default)]
    pub commands: Vec<WarmupCommand>,
    /// Verification commands executed after warmup.
    #[serde(default)]
    pub verification: Vec<WarmupCommand>,
    /// Cleanup commands executed after every job completes.
    #[serde(default)]
    pub post_job_cleanup: Vec<WarmupCommand>,
    /// Default timeout applied to each command.
    #[serde(default = "WarmupConfig::default_timeout")]
    pub default_timeout_s: u64,
}

impl Default for WarmupConfig {
    fn default() -> Self {
        Self {
            commands: Vec::new(),
            verification: Vec::new(),
            post_job_cleanup: Vec::new(),
            default_timeout_s: Self::default_timeout(),
        }
    }
}

impl WarmupConfig {
    const fn default_timeout() -> u64 {
        30
    }
}

/// Cache priming instructions.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CachePrimingConfig {
    /// Enables cache priming.
    #[serde(default)]
    pub enabled: bool,
    /// Optional maximum number of bytes to hydrate per worker.
    #[serde(default)]
    pub max_bytes: Option<u64>,
    /// Commands executed to hydrate caches.
    #[serde(default)]
    pub commands: Vec<WarmupCommand>,
}

impl Default for CachePrimingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_bytes: None,
            commands: Vec::new(),
        }
    }
}

/// Lifecycle constraints for workers.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LifecycleConfig {
    /// Maximum lifetime for a worker before recycling (seconds).
    #[serde(default = "default_worker_ttl_seconds")]
    pub worker_ttl_seconds: u64,
    /// Maximum number of jobs executed by a worker before recycling.
    #[serde(default = "default_max_jobs_per_worker")]
    pub max_jobs_per_worker: usize,
    /// Run GC and cache refresh every N jobs.
    #[serde(default = "default_gc_frequency")]
    pub gc_job_frequency: usize,
}

impl Default for LifecycleConfig {
    fn default() -> Self {
        Self {
            worker_ttl_seconds: default_worker_ttl_seconds(),
            max_jobs_per_worker: default_max_jobs_per_worker(),
            gc_job_frequency: default_gc_frequency(),
        }
    }
}

// Conversion from nativelink-config types to local types with extended fields
impl From<nativelink_config::warm_worker_pools::WorkerPoolConfig> for WorkerPoolConfig {
    fn from(value: nativelink_config::warm_worker_pools::WorkerPoolConfig) -> Self {
        Self {
            name: value.name,
            language: value.language.into(),
            cri_socket: value.cri_socket,
            image_socket: None,
            container_image: value.container_image,
            crictl_binary: default_crictl_binary(),
            namespace: default_namespace(),
            min_warm_workers: value.min_warm_workers,
            max_workers: value.max_workers,
            worker_command: default_worker_command(),
            worker_args: vec![],
            env: HashMap::new(),
            working_directory: None,
            warmup: value.warmup.into(),
            cache: CachePrimingConfig::default(),
            lifecycle: value.lifecycle.into(),
            isolation: value.isolation,
        }
    }
}

impl From<nativelink_config::warm_worker_pools::Language> for Language {
    fn from(value: nativelink_config::warm_worker_pools::Language) -> Self {
        match value {
            nativelink_config::warm_worker_pools::Language::Jvm => Language::Jvm,
            nativelink_config::warm_worker_pools::Language::NodeJs => Language::NodeJs,
            nativelink_config::warm_worker_pools::Language::Custom(s) => Language::Custom(s),
        }
    }
}

impl From<nativelink_config::warm_worker_pools::WarmupConfig> for WarmupConfig {
    fn from(value: nativelink_config::warm_worker_pools::WarmupConfig) -> Self {
        Self {
            commands: value.commands.into_iter().map(Into::into).collect(),
            verification: vec![],
            post_job_cleanup: value.post_job_cleanup.into_iter().map(Into::into).collect(),
            default_timeout_s: WarmupConfig::default_timeout(),
        }
    }
}

impl From<nativelink_config::warm_worker_pools::WarmupCommand> for WarmupCommand {
    fn from(value: nativelink_config::warm_worker_pools::WarmupCommand) -> Self {
        Self {
            argv: value.argv,
            env: HashMap::new(),
            working_directory: None,
            timeout_s: value.timeout_s,
        }
    }
}

impl From<nativelink_config::warm_worker_pools::LifecycleConfig> for LifecycleConfig {
    fn from(value: nativelink_config::warm_worker_pools::LifecycleConfig) -> Self {
        Self {
            worker_ttl_seconds: value.worker_ttl_seconds,
            max_jobs_per_worker: value.max_jobs_per_worker,
            gc_job_frequency: value.gc_job_frequency,
        }
    }
}
