use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;

use nativelink_config::warm_worker_pools::{IsolationStrategy, WarmWorkerPoolsConfig};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::action_messages::WorkerId;
use tokio::sync::{Mutex, Notify};
use tokio::time;
use uuid::Uuid;

use crate::cache::CachePrimingAgent;
use crate::config::WorkerPoolConfig;
use crate::cri_client::{
    ContainerConfig, ContainerMetadata, CriClient, ImageSpec, KeyValue, LinuxContainerConfig,
    LinuxContainerResources, LinuxPodSandboxConfig, LinuxSandboxSecurityContext, NamespaceOptions,
    PodSandboxConfig, PodSandboxMetadata,
};
use crate::isolation::OverlayFsMount;
use crate::lifecycle::LifecyclePolicy;
use crate::warmup::WarmupController;
use crate::worker::{WorkerOutcome, WorkerRecord, WorkerState};

/// Options for creating the pool manager.
#[derive(Debug)]
pub struct PoolCreateOptions {
    pub config: WarmWorkerPoolsConfig,
}

impl PoolCreateOptions {
    #[must_use]
    pub const fn new(config: WarmWorkerPoolsConfig) -> Self {
        Self { config }
    }
}

/// Aggregates metrics for a pool.
#[derive(Debug, Default, MetricsComponent)]
pub struct WarmWorkerPoolMetrics {
    #[metric(help = "Number of workers in ready state, available for assignment")]
    pub ready_workers: AtomicU64,

    #[metric(help = "Number of workers actively executing jobs")]
    pub active_workers: AtomicU64,

    #[metric(help = "Number of workers being provisioned (starting up and warming)")]
    pub provisioning_workers: AtomicU64,

    #[metric(help = "Total number of workers that have been recycled")]
    pub recycled_workers: AtomicU64,
}

/// Manages CRI-backed pools for multiple languages.
#[derive(Debug, MetricsComponent)]
pub struct WarmWorkerPoolManager {
    #[metric(group = "pools")]
    pools: HashMap<String, Arc<WorkerPool>>,
}

impl WarmWorkerPoolManager {
    pub async fn new(options: PoolCreateOptions) -> Result<Self, Error> {
        let mut pools = HashMap::new();
        for pool_config in options.config.pools {
            let pool = WorkerPool::new(pool_config.into()).await?;
            pools.insert(pool.pool_name().to_string(), pool);
        }
        Ok(Self { pools })
    }

    pub async fn acquire(&self, pool: &str) -> Result<WarmWorkerLease, Error> {
        let worker_pool = self
            .pools
            .get(pool)
            .ok_or_else(|| make_err!(Code::NotFound, "pool {pool} not found"))?;
        worker_pool.acquire().await
    }

    /// Acquires an isolated worker for the given job ID.
    ///
    /// If the pool has isolation enabled, this will create an ephemeral COW clone
    /// of a warm template. Otherwise, it falls back to regular acquisition.
    pub async fn acquire_isolated(
        &self,
        pool: &str,
        job_id: &str,
    ) -> Result<WarmWorkerLease, Error> {
        let worker_pool = self
            .pools
            .get(pool)
            .ok_or_else(|| make_err!(Code::NotFound, "pool {pool} not found"))?;
        worker_pool.acquire_isolated(job_id).await
    }
}

#[derive(Debug, MetricsComponent)]
struct WorkerPool {
    config: WorkerPoolConfig,
    runtime: CriClient,
    warmup: WarmupController,
    cache: CachePrimingAgent,
    lifecycle: LifecyclePolicy,
    state: Mutex<PoolState>,
    notifier: Notify,

    #[metric(group = "metrics")]
    metrics: Arc<WarmWorkerPoolMetrics>,
}

impl WorkerPool {
    async fn new(config: WorkerPoolConfig) -> Result<Arc<Self>, Error> {
        let runtime = CriClient::new(
            &config.crictl_binary,
            &config.cri_socket,
            config.image_socket.clone(),
        );
        let pool = Arc::new(Self {
            warmup: WarmupController::new(config.warmup.clone()),
            cache: CachePrimingAgent::new(config.cache.clone()),
            lifecycle: LifecyclePolicy::new(config.lifecycle.clone()),
            config,
            runtime,
            state: Mutex::new(PoolState::default()),
            notifier: Notify::new(),
            metrics: Arc::new(WarmWorkerPoolMetrics::default()),
        });
        let pool_clone = Arc::clone(&pool);
        tokio::spawn(async move {
            pool_clone.maintain_loop().await;
        });
        Ok(pool)
    }

    fn pool_name(&self) -> &str {
        &self.config.name
    }

    async fn acquire(self: &Arc<Self>) -> Result<WarmWorkerLease, Error> {
        loop {
            if let Some(lease) = self.try_acquire_from_ready().await? {
                return Ok(lease);
            }
            self.ensure_capacity().await?;
            self.notifier.notified().await;
        }
    }

    /// Acquires an isolated worker using COW isolation if enabled.
    async fn acquire_isolated(self: &Arc<Self>, job_id: &str) -> Result<WarmWorkerLease, Error> {
        // Check if isolation is enabled
        let isolation_config = match &self.config.isolation {
            Some(config) if config.strategy != IsolationStrategy::None => config,
            _ => {
                // No isolation configured, fall back to regular acquisition
                return self.acquire().await;
            }
        };

        // Get or create template
        let template = self.get_or_create_template().await?;

        // Create isolated clone
        self.clone_from_template(&template, job_id, isolation_config)
            .await
    }

    /// Gets the existing template or creates a new one.
    async fn get_or_create_template(self: &Arc<Self>) -> Result<TemplateState, Error> {
        // Check if template exists and is still valid
        {
            let state = self.state.lock().await;
            if let Some(template) = &state.template {
                // TODO: Check if template needs refresh based on age
                return Ok(template.clone());
            }
        }

        // Need to create template
        self.create_template().await
    }

    /// Creates a new template worker.
    async fn create_template(self: &Arc<Self>) -> Result<TemplateState, Error> {
        let worker_id = WorkerId(format!(
            "crio:{}:template:{}",
            self.config.name,
            Uuid::new_v4().simple()
        ));

        tracing::info!(
            pool = self.config.name,
            worker_id = %worker_id.0,
            "Creating warm template worker"
        );

        // Provision a worker normally
        self.runtime
            .pull_image(&self.config.container_image)
            .await
            .err_tip(|| format!("while pulling image for template {}", worker_id.0))?;

        let sandbox_config = self.build_sandbox_config(&worker_id);
        let container_config = self.build_container_config(&worker_id);

        let sandbox_id = self
            .runtime
            .run_pod_sandbox(&sandbox_config)
            .await
            .err_tip(|| format!("while starting sandbox for template {}", worker_id.0))?;

        let container_id = self
            .runtime
            .create_container(&sandbox_id, &container_config, &sandbox_config)
            .await
            .err_tip(|| format!("while creating container for template {}", worker_id.0))?;

        self.runtime
            .start_container(&container_id)
            .await
            .err_tip(|| format!("while booting container for template {}", worker_id.0))?;

        // Run warmup
        self.warmup
            .run_full_warmup(&self.runtime, &container_id)
            .await
            .err_tip(|| format!("while warming template {}", worker_id.0))?;

        // Create template path
        let template_path = self
            .config
            .isolation
            .as_ref()
            .map(|c| c.template_cache_path.join(&worker_id.0))
            .unwrap_or_else(|| PathBuf::from("/tmp/nativelink/templates").join(&worker_id.0));

        let template = TemplateState {
            worker_id,
            sandbox_id,
            container_id,
            template_path,
            created_at: time::Instant::now(),
        };

        // Store template in state
        {
            let mut state = self.state.lock().await;
            state.template = Some(template.clone());
        }

        tracing::info!(
            pool = self.config.name,
            template_path = ?template.template_path,
            "Warm template created successfully"
        );

        Ok(template)
    }

    /// Clones an ephemeral worker from a template using OverlayFS.
    async fn clone_from_template(
        self: &Arc<Self>,
        template: &TemplateState,
        job_id: &str,
        isolation_config: &nativelink_config::warm_worker_pools::IsolationConfig,
    ) -> Result<WarmWorkerLease, Error> {
        let worker_id = WorkerId(format!("crio:{}:isolated:{}", self.config.name, job_id));

        // Create OverlayFS mount structure
        let mount = OverlayFsMount::new(
            &template.template_path,
            &isolation_config.job_workspace_path,
            job_id,
        );

        // Create directories for OverlayFS
        mount.create_directories().await?;

        // TODO(isolation): Implement true OverlayFS mounting for zero-copy cloning.
        // Current implementation creates separate containers which provides isolation but not COW performance.
        // To implement true OverlayFS:
        //   1. Use CRI-O's Mount API to attach OverlayFS volumes to containers
        //   2. Pass mount.get_mount_options() to ContainerConfig.mounts
        //   3. Or use CRIU checkpoint/restore for full process+memory snapshot
        // See: https://github.com/cri-o/cri-o/blob/main/docs/crio.conf.5.md#crioruntimeworkloads-table
        // Performance impact: Currently ~2-3s container creation vs <100ms with true OverlayFS

        tracing::debug!(
            pool = self.config.name,
            job_id,
            template_worker = %template.worker_id.0,
            "Creating isolated worker clone (separate container for MVP)"
        );
        let sandbox_config = self.build_sandbox_config(&worker_id);
        let container_config = self.build_container_config(&worker_id);

        let sandbox_id = self
            .runtime
            .run_pod_sandbox(&sandbox_config)
            .await
            .err_tip(|| format!("while starting sandbox for isolated worker {}", worker_id.0))?;

        let container_id = self
            .runtime
            .create_container(&sandbox_id, &container_config, &sandbox_config)
            .await
            .err_tip(|| {
                format!(
                    "while creating container for isolated worker {}",
                    worker_id.0
                )
            })?;

        self.runtime
            .start_container(&container_id)
            .await
            .err_tip(|| format!("while booting isolated container {}", worker_id.0))?;

        Ok(WarmWorkerLease::new_isolated(
            Arc::clone(self),
            worker_id,
            sandbox_id,
            container_id,
            mount,
        ))
    }

    /// Releases an isolated (ephemeral) worker.
    async fn release_isolated_worker(
        &self,
        worker_id: WorkerId,
        container_id: &str,
        sandbox_id: &str,
        mount: OverlayFsMount,
        _outcome: WorkerOutcome,
    ) -> Result<(), Error> {
        tracing::debug!(
            pool = self.config.name,
            worker_id = %worker_id.0,
            "Releasing isolated worker"
        );

        // Stop and remove ephemeral container
        if let Err(err) = self.runtime.stop_container(container_id).await {
            tracing::debug!(
                error = ?err,
                container_id,
                "failed to stop isolated container"
            );
        }

        if let Err(err) = self.runtime.remove_container(container_id).await {
            tracing::debug!(
                error = ?err,
                container_id,
                "failed to remove isolated container"
            );
        }

        if let Err(err) = self.runtime.stop_pod(sandbox_id).await {
            tracing::debug!(
                error = ?err,
                sandbox_id,
                "failed to stop isolated sandbox"
            );
        }

        if let Err(err) = self.runtime.remove_pod(sandbox_id).await {
            tracing::debug!(
                error = ?err,
                sandbox_id,
                "failed to remove isolated sandbox"
            );
        }

        // Cleanup OverlayFS mount
        mount.cleanup().await?;

        Ok(())
    }

    async fn try_acquire_from_ready(self: &Arc<Self>) -> Result<Option<WarmWorkerLease>, Error> {
        let mut state = self.state.lock().await;
        if let Some(worker_id) = state.ready_queue.pop_front() {
            if let Some(worker) = state.workers.get_mut(&worker_id) {
                worker.transition(WorkerState::Active);
                self.metrics.ready_workers.fetch_sub(1, Ordering::Relaxed);
                self.metrics.active_workers.fetch_add(1, Ordering::Relaxed);
                return Ok(Some(WarmWorkerLease::new(
                    Arc::clone(self),
                    worker.id.clone(),
                    worker.sandbox_id.clone(),
                    worker.container_id.clone(),
                )));
            }
        }
        Ok(None)
    }

    async fn maintain_loop(self: Arc<Self>) {
        let mut interval = time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;
            if let Err(err) = self.ensure_capacity().await {
                tracing::error!(
                    pool = self.config.name,
                    error = ?err,
                    "failed to ensure warm worker capacity"
                );
            }
            if let Err(err) = self.reap_expired_workers().await {
                tracing::error!(
                    pool = self.config.name,
                    error = ?err,
                    "failed to recycle expired workers"
                );
            }
        }
    }

    async fn ensure_capacity(self: &Arc<Self>) -> Result<(), Error> {
        let mut to_create = 0usize;
        {
            let state = self.state.lock().await;
            let warm_count = state.ready_queue.len() + state.provisioning.len();
            if warm_count < self.config.min_warm_workers {
                to_create = self.config.min_warm_workers - warm_count;
            }
        }
        for _ in 0..to_create {
            self.spawn_worker().await?;
        }
        Ok(())
    }

    async fn reap_expired_workers(&self) -> Result<(), Error> {
        let mut expired = Vec::new();
        {
            let state = self.state.lock().await;
            for (worker_id, record) in &state.workers {
                if self
                    .lifecycle
                    .should_recycle(record.created_at, record.jobs_executed)
                {
                    expired.push((
                        worker_id.clone(),
                        record.container_id.clone(),
                        record.sandbox_id.clone(),
                    ));
                }
            }
        }

        for (worker_id, container_id, sandbox_id) in expired {
            self.recycle_worker(worker_id, container_id, sandbox_id)
                .await?;
        }
        Ok(())
    }

    async fn spawn_worker(self: &Arc<Self>) -> Result<(), Error> {
        let worker_id = WorkerId(format!(
            "crio:{}:{}",
            self.config.name,
            Uuid::new_v4().simple()
        ));
        {
            let mut state = self.state.lock().await;
            if state.total_workers() >= self.config.max_workers {
                tracing::warn!(
                    pool = self.config.name,
                    "max worker capacity reached; skipping spawn"
                );
                return Ok(());
            }
            if !state.provisioning.insert(worker_id.clone()) {
                return Ok(());
            }
            self.metrics
                .provisioning_workers
                .fetch_add(1, Ordering::Relaxed);
        }
        let pool = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(err) = Arc::clone(&pool).provision_worker(worker_id.clone()).await {
                tracing::error!(
                    pool = pool.config.name,
                    worker = %worker_id.0,
                    error = ?err,
                    "worker provisioning failed"
                );
                pool.finish_provisioning(worker_id, Err(err)).await;
            }
        });
        Ok(())
    }

    async fn provision_worker(self: Arc<Self>, worker_id: WorkerId) -> Result<(), Error> {
        self.runtime
            .pull_image(&self.config.container_image)
            .await
            .err_tip(|| format!("while pulling image for worker {}", worker_id.0))?;

        let sandbox_config = self.build_sandbox_config(&worker_id);
        let container_config = self.build_container_config(&worker_id);
        let mut sandbox_id: Option<String> = None;
        let mut container_id: Option<String> = None;
        let provision_result: Result<WorkerRecord, Error> = async {
            let sandbox = self
                .runtime
                .run_pod_sandbox(&sandbox_config)
                .await
                .err_tip(|| format!("while starting sandbox for {}", worker_id.0))?;
            sandbox_id = Some(sandbox.clone());
            let container = self
                .runtime
                .create_container(&sandbox, &container_config, &sandbox_config)
                .await
                .err_tip(|| format!("while creating container for {}", worker_id.0))?;
            container_id = Some(container.clone());
            self.runtime
                .start_container(&container)
                .await
                .err_tip(|| format!("while booting container for {}", worker_id.0))?;

            self.warmup
                .run_full_warmup(&self.runtime, &container)
                .await
                .err_tip(|| format!("while warming worker {}", worker_id.0))?;
            self.cache
                .prime(&self.runtime, &container)
                .await
                .err_tip(|| format!("while priming cache for {}", worker_id.0))?;

            Ok(WorkerRecord::new(
                worker_id.clone(),
                sandbox,
                container,
                WorkerState::Ready,
            ))
        }
        .await;

        match provision_result {
            Ok(record) => {
                self.finish_provisioning(worker_id, Ok(record)).await;
                Ok(())
            }
            Err(err) => {
                if let Some(container) = container_id {
                    if let Err(stop_err) = self.runtime.stop_container(&container).await {
                        tracing::debug!(
                            error = ?stop_err,
                            container,
                            "failed to stop container during cleanup"
                        );
                    }
                    if let Err(remove_err) = self.runtime.remove_container(&container).await {
                        tracing::debug!(
                            error = ?remove_err,
                            container,
                            "failed to remove container during cleanup"
                        );
                    }
                }
                if let Some(sandbox) = sandbox_id {
                    if let Err(stop_err) = self.runtime.stop_pod(&sandbox).await {
                        tracing::debug!(
                            error = ?stop_err,
                            sandbox,
                            "failed to stop sandbox during cleanup"
                        );
                    }
                    if let Err(remove_err) = self.runtime.remove_pod(&sandbox).await {
                        tracing::debug!(
                            error = ?remove_err,
                            sandbox,
                            "failed to remove sandbox during cleanup"
                        );
                    }
                }
                Err(err)
            }
        }
    }

    async fn finish_provisioning(&self, worker_id: WorkerId, record: Result<WorkerRecord, Error>) {
        let mut state = self.state.lock().await;
        state.provisioning.remove(&worker_id);
        self.metrics
            .provisioning_workers
            .fetch_sub(1, Ordering::Relaxed);
        match record {
            Ok(record) => {
                self.metrics.ready_workers.fetch_add(1, Ordering::Relaxed);
                state.ready_queue.push_back(worker_id.clone());
                state.workers.insert(worker_id, record);
                self.notifier.notify_one();
            }
            Err(err) => {
                tracing::warn!(worker = %worker_id.0, error = ?err, "provisioning failed");
            }
        }
    }

    async fn release_worker(
        &self,
        worker_id: WorkerId,
        outcome: WorkerOutcome,
    ) -> Result<(), Error> {
        let mut recycle = matches!(outcome, WorkerOutcome::Failed | WorkerOutcome::Recycle);
        let (container_id, sandbox_id) = {
            let mut state = self.state.lock().await;
            let record = state
                .workers
                .get_mut(&worker_id)
                .ok_or_else(|| make_err!(Code::NotFound, "unknown worker {}", worker_id.0))?;
            let container_id = record.container_id.clone();
            let sandbox_id = record.sandbox_id.clone();
            if !recycle {
                record.jobs_executed += 1;
                if self
                    .lifecycle
                    .should_recycle(record.created_at, record.jobs_executed)
                {
                    recycle = true;
                }
            }
            record.transition(WorkerState::Cooling);
            self.metrics.active_workers.fetch_sub(1, Ordering::Relaxed);
            (container_id, sandbox_id)
        };

        if recycle {
            self.recycle_worker(worker_id, container_id, sandbox_id)
                .await
        } else {
            self.warmup
                .post_job_cleanup(&self.runtime, &container_id)
                .await
                .err_tip(|| format!("while cleaning worker {}", worker_id.0))?;
            {
                let mut state = self.state.lock().await;
                if let Some(record) = state.workers.get_mut(&worker_id) {
                    record.transition(WorkerState::Ready);
                    state.ready_queue.push_back(worker_id.clone());
                    self.metrics.ready_workers.fetch_add(1, Ordering::Relaxed);
                }
            }
            self.notifier.notify_one();
            Ok(())
        }
    }

    async fn recycle_worker(
        &self,
        worker_id: WorkerId,
        container_id: String,
        sandbox_id: String,
    ) -> Result<(), Error> {
        {
            let mut state = self.state.lock().await;
            state.ready_queue.retain(|id| id != &worker_id);
            state.workers.remove(&worker_id);
        }
        self.metrics
            .recycled_workers
            .fetch_add(1, Ordering::Relaxed);
        if let Err(err) = self.runtime.stop_container(&container_id).await {
            tracing::debug!(
                error = ?err,
                container_id,
                "failed to stop container while recycling"
            );
        }
        if let Err(err) = self.runtime.remove_container(&container_id).await {
            tracing::debug!(
                error = ?err,
                container_id,
                "failed to remove container while recycling"
            );
        }
        if let Err(err) = self.runtime.stop_pod(&sandbox_id).await {
            tracing::debug!(
                error = ?err,
                sandbox_id,
                "failed to stop sandbox while recycling"
            );
        }
        if let Err(err) = self.runtime.remove_pod(&sandbox_id).await {
            tracing::debug!(
                error = ?err,
                sandbox_id,
                "failed to remove sandbox while recycling"
            );
        }
        self.notifier.notify_waiters();
        Ok(())
    }

    fn build_sandbox_config(&self, worker_id: &WorkerId) -> PodSandboxConfig {
        let metadata = PodSandboxMetadata {
            name: format!("{}-sandbox", self.sanitize_name(worker_id)),
            namespace: self.config.namespace.clone(),
            uid: worker_id.0.clone(),
            attempt: 0,
        };
        PodSandboxConfig {
            metadata,
            hostname: format!("{}-{}", self.config.name, worker_id.0.replace(':', "-")),
            log_directory: "/var/log/nativelink".to_string(),
            dns_config: None,
            port_mappings: Vec::new(),
            labels: HashMap::new(),
            annotations: HashMap::new(),
            linux: Some(LinuxPodSandboxConfig {
                security_context: Some(LinuxSandboxSecurityContext {
                    namespace_options: Some(NamespaceOptions {
                        network: Some(2),
                        pid: Some(2),
                        ipc: Some(2),
                    }),
                }),
            }),
        }
    }

    fn build_container_config(&self, worker_id: &WorkerId) -> ContainerConfig {
        ContainerConfig {
            metadata: ContainerMetadata {
                name: format!("{}-container", self.sanitize_name(worker_id)),
                attempt: 0,
            },
            image: ImageSpec {
                image: self.config.container_image.clone(),
            },
            command: self.config.worker_command.clone(),
            args: self.config.worker_args.clone(),
            working_dir: self.config.working_directory.clone(),
            envs: self
                .config
                .env
                .iter()
                .map(|(key, value)| KeyValue {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
            mounts: Vec::new(),
            log_path: format!("{}-worker.log", self.sanitize_name(worker_id)),
            stdin: false,
            stdin_once: false,
            tty: false,
            linux: Some(LinuxContainerConfig {
                resources: Some(LinuxContainerResources {
                    cpu_period: None,
                    cpu_quota: None,
                    memory_limit_in_bytes: None,
                }),
            }),
        }
    }

    fn sanitize_name(&self, worker_id: &WorkerId) -> String {
        worker_id.0.replace([':', '.'], "-")
    }
}

/// Template state for a warm worker that can be cloned for isolated jobs.
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct TemplateState {
    worker_id: WorkerId,
    sandbox_id: String,
    container_id: String,
    template_path: PathBuf,
    created_at: time::Instant,
}

#[derive(Debug, Default)]
struct PoolState {
    workers: HashMap<WorkerId, WorkerRecord>,
    ready_queue: VecDeque<WorkerId>,
    provisioning: HashSet<WorkerId>,
    /// Template worker for COW isolation (when isolation is enabled)
    template: Option<TemplateState>,
}

impl PoolState {
    fn total_workers(&self) -> usize {
        self.workers.len() + self.provisioning.len()
    }
}

/// Handle representing a checked-out worker.
#[derive(Debug)]
pub struct WarmWorkerLease {
    pool: Arc<WorkerPool>,
    worker_id: Option<WorkerId>,
    sandbox_id: String,
    container_id: String,
    /// If Some, this is an ephemeral isolated worker that needs special cleanup
    isolation_mount: Option<OverlayFsMount>,
}

impl WarmWorkerLease {
    const fn new(
        pool: Arc<WorkerPool>,
        worker_id: WorkerId,
        sandbox_id: String,
        container_id: String,
    ) -> Self {
        Self {
            pool,
            worker_id: Some(worker_id),
            sandbox_id,
            container_id,
            isolation_mount: None,
        }
    }

    /// Creates a new ephemeral isolated worker lease.
    const fn new_isolated(
        pool: Arc<WorkerPool>,
        worker_id: WorkerId,
        sandbox_id: String,
        container_id: String,
        isolation_mount: OverlayFsMount,
    ) -> Self {
        Self {
            pool,
            worker_id: Some(worker_id),
            sandbox_id,
            container_id,
            isolation_mount: Some(isolation_mount),
        }
    }

    pub const fn worker_id(&self) -> Option<&WorkerId> {
        self.worker_id.as_ref()
    }

    pub const fn is_isolated(&self) -> bool {
        self.isolation_mount.is_some()
    }

    pub async fn release(mut self, outcome: WorkerOutcome) -> Result<(), Error> {
        if let Some(worker_id) = self.worker_id.take() {
            // For isolated workers, we need different cleanup
            if let Some(mount) = self.isolation_mount.take() {
                self.pool
                    .release_isolated_worker(
                        worker_id,
                        &self.container_id,
                        &self.sandbox_id,
                        mount,
                        outcome,
                    )
                    .await
                    .err_tip(|| "while releasing isolated worker")
            } else {
                self.pool
                    .release_worker(worker_id, outcome)
                    .await
                    .err_tip(|| "while releasing worker")
            }
        } else {
            Ok(())
        }
    }
}

impl Drop for WarmWorkerLease {
    fn drop(&mut self) {
        if self.worker_id.is_some() {
            tracing::warn!(
                container_id = self.container_id,
                sandbox_id = self.sandbox_id,
                "worker lease dropped without explicit release; worker will leak until TTL"
            );
        }
    }
}
