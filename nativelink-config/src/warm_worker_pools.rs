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

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Root configuration for the warm worker pool manager.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WarmWorkerPoolsConfig {
    /// All pools managed by the service.
    #[serde(default)]
    pub pools: Vec<WorkerPoolConfig>,
}

/// Supported language runtimes.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Language {
    Jvm,
    NodeJs,
    Custom(String),
}

/// Matcher used to select a warm worker pool based on action platform properties.
///
/// This is consumed by the scheduler for routing decisions only; the warm pool
/// manager itself does not interpret these values.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum PropertyMatcher {
    /// Exact string match.
    Exact(String),
    /// Match if the property starts with `prefix`.
    Prefix { prefix: String },
    /// Match if the property contains `contains` as a substring.
    Contains { contains: String },
}

impl PropertyMatcher {
    #[must_use]
    pub fn matches(&self, value: &str) -> bool {
        match self {
            Self::Exact(expected) => value == expected,
            Self::Prefix { prefix } => value.starts_with(prefix),
            Self::Contains { contains } => value.contains(contains),
        }
    }
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

/// Per-pool configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerPoolConfig {
    /// Pool name used for lookups and telemetry.
    pub name: String,
    /// Logical language runtime for the workers.
    pub language: Language,
    /// Optional matchers used by the scheduler to route actions into this pool.
    ///
    /// If all matchers are satisfied by an action's platform properties, the
    /// scheduler will select this pool before falling back to heuristic routing.
    #[serde(default)]
    pub match_platform_properties: HashMap<String, PropertyMatcher>,
    /// Path to the CRI-O unix socket.
    pub cri_socket: String,
    /// Container image to boot.
    pub container_image: String,
    /// Minimum number of warmed workers to keep ready.
    #[serde(default = "default_min_warm_workers")]
    pub min_warm_workers: usize,
    /// Maximum containers allowed in the pool.
    #[serde(default = "default_max_workers")]
    pub max_workers: usize,
    /// Warmup definition for the pool.
    #[serde(default)]
    pub warmup: WarmupConfig,
    /// Lifecycle configuration.
    #[serde(default)]
    pub lifecycle: LifecycleConfig,
    /// Isolation configuration for security between jobs.
    #[serde(default)]
    pub isolation: Option<IsolationConfig>,
}

/// Warmup command executed inside the worker container.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct WarmupCommand {
    /// Command argv executed inside the worker container.
    pub argv: Vec<String>,
    /// Optional timeout override in seconds.
    #[serde(default)]
    pub timeout_s: Option<u64>,
}

/// Warmup configuration for a pool.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(deny_unknown_fields)]
pub struct WarmupConfig {
    /// Commands that bring the runtime to a hot state.
    #[serde(default)]
    pub commands: Vec<WarmupCommand>,
    /// Cleanup commands executed after every job completes.
    #[serde(default)]
    pub post_job_cleanup: Vec<WarmupCommand>,
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

/// Isolation strategy for worker jobs.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum IsolationStrategy {
    /// No isolation - workers execute multiple jobs with shared state (default, backward compatible).
    None,
    /// OverlayFS-based copy-on-write isolation - each job gets isolated filesystem.
    Overlayfs,
    /// CRIU checkpoint/restore - maximum isolation with process snapshots.
    Criu,
}

impl Default for IsolationStrategy {
    fn default() -> Self {
        Self::None
    }
}

/// Isolation configuration for preventing state leakage between jobs.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct IsolationConfig {
    /// Isolation strategy to use.
    #[serde(default)]
    pub strategy: IsolationStrategy,
    /// Path where warm template containers are cached.
    #[serde(default = "default_template_cache_path")]
    pub template_cache_path: PathBuf,
    /// Path where ephemeral job workspaces are created.
    #[serde(default = "default_job_workspace_path")]
    pub job_workspace_path: PathBuf,
}

fn default_template_cache_path() -> PathBuf {
    PathBuf::from("/var/lib/nativelink/warm-templates")
}

fn default_job_workspace_path() -> PathBuf {
    PathBuf::from("/var/lib/nativelink/warm-jobs")
}

impl Default for IsolationConfig {
    fn default() -> Self {
        Self {
            strategy: IsolationStrategy::default(),
            template_cache_path: default_template_cache_path(),
            job_workspace_path: default_job_workspace_path(),
        }
    }
}
