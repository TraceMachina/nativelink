// Copyright 2026 Trace Machina, Inc. All rights reserved.
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

//! The production components of one run, wired in-process: the
//! `SimpleScheduler` on the chosen backend, the `ExecutionServer` and the
//! `WorkerApiServer` in front of it, `LocalWorker`s over the in-process
//! network, and client tasks that drive `Execute` the way Bazel does.

use core::sync::atomic::{AtomicU8, AtomicU32, AtomicU64, Ordering};
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;

use futures::StreamExt;
use nativelink_config::cas_server::{
    EndpointConfig, ExecutionConfig, LocalWorkerConfig, WithInstanceName, WorkerApiConfig,
    WorkerProperty,
};
use nativelink_config::schedulers::SimpleSpec;
use nativelink_config::stores::{EvictionPolicy, MemorySpec, RedisSpec, StoreSpec};
use nativelink_error::{Code, Error, ResultExt};
use nativelink_proto::build::bazel::remote::execution::v2::execution_server::Execution;
use nativelink_proto::build::bazel::remote::execution::v2::{
    Action as ActionProto, Command, Directory, ExecuteOperationMetadata, ExecuteRequest,
    ExecuteResponse, ExecutionPolicy, Platform, WaitExecutionRequest, digest_function, platform,
};
use nativelink_proto::google::longrunning::{Operation, operation};
use nativelink_redis_tester::FakeRedisBackend;
use nativelink_scheduler::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_scheduler::memory_awaited_action_db::{MemoryAwaitedActionDb, OperationIdFn};
use nativelink_scheduler::simple_scheduler::SimpleScheduler;
use nativelink_scheduler::store_awaited_action_db::StoreAwaitedActionDb;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_service::execution_server::ExecutionServer;
use nativelink_service::worker_api_server::{NowFn, WorkerApiServer};
use nativelink_store::ac_utils::serialize_and_upload_message;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::redis_store::{RedisStore, RedisSubscriptionManager};
use nativelink_store::store_manager::StoreManager;
use nativelink_util::action_messages::OperationId;
use nativelink_util::background_spawn;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::operation_state_manager::{
    ClientStateManager, OperationFilter, OperationStageFlags, OrderDirection,
};
use nativelink_util::shutdown_guard::ShutdownGuard;
use nativelink_util::store_trait::{SchedulerStore, Store, StoreLike};
use nativelink_worker::local_worker::LocalWorker;
use prost::Message as _;
use tokio::sync::{Notify, broadcast, watch};
use tokio::task::JoinHandle;
use tonic::{Code as TonicCode, Request};

use crate::actions::{Action, Registry, SimActionsManager};
use crate::clock::{SimClock, SimInstant};
use crate::net::{InProcessWorkerClient, Link, Net};
use crate::trace::{Event, Outcome, Stage, Trace};

const INSTANCE_NAME: &str = "sim";
const SCHEDULER_NAME: &str = "sim";
const CAS_NAME: &str = "cas";

/// Which `AwaitedActionDb` the scheduler runs on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Backend {
    Memory,
    /// `StoreAwaitedActionDb` over a `RedisStore` on the fake Redis.
    FakeRedis,
}

/// One kind of simulated worker.
#[derive(Clone, Debug)]
pub struct WorkerSpec {
    pub(crate) max_inflight: u64,
    pub(crate) properties: Vec<(String, String)>,
    /// The worker's endpoint timeout; it keeps alive every half of it.
    pub(crate) keepalive_timeout_s: f32,
}

impl Default for WorkerSpec {
    fn default() -> Self {
        Self {
            max_inflight: 1,
            properties: Vec::new(),
            keepalive_timeout_s: 5.0,
        }
    }
}

impl WorkerSpec {
    #[must_use]
    pub const fn max_inflight(mut self, max_inflight: u64) -> Self {
        self.max_inflight = max_inflight;
        self
    }

    #[must_use]
    pub fn property(mut self, name: &str, value: &str) -> Self {
        self.properties.push((name.to_string(), value.to_string()));
        self
    }

    #[must_use]
    pub const fn keepalive_timeout_s(mut self, seconds: f32) -> Self {
        self.keepalive_timeout_s = seconds;
        self
    }
}

const CLIENT_RUNNING: u8 = 0;
const CLIENT_DONE: u8 = 1;
const CLIENT_GONE: u8 = 2;

struct Client {
    state: Arc<AtomicU8>,
    gone: watch::Sender<bool>,
    action: Action,
}

struct Worker {
    task: JoinHandle<Result<(), Error>>,
    manager: Arc<SimActionsManager>,
    spec: WorkerSpec,
    // Held so the worker's shutdown channel stays open.
    _shutdown: broadcast::Sender<ShutdownGuard>,
}

struct Scheduler {
    client: Arc<SimpleScheduler>,
    /// The fake Redis listener this incarnation reached the store through.
    redis_port: Option<u16>,
    // Held so the worker side lives as long as the servers in `Net` do.
    _worker: Arc<dyn WorkerScheduler>,
}

/// Everything one run of a scenario on one backend owns.
pub struct World {
    backend: Backend,
    spec: SimpleSpec,
    net: Arc<Net>,
    trace: Trace,
    registry: Arc<Registry>,
    store_manager: Arc<StoreManager>,
    cas: Store,
    redis: Option<FakeRedisBackend<RedisSubscriptionManager>>,
    scheduler: Option<Scheduler>,
    workers: Vec<Worker>,
    clients: HashMap<String, Client>,
    next_worker_id: Arc<AtomicU64>,
    next_operation_id: Arc<AtomicU64>,
    /// Matching passes, and the most store commands between two of them.
    passes: Arc<AtomicU64>,
    max_commands_per_pass: Arc<AtomicU64>,
    /// Passes before this time are not measured.
    measure_from_ms: Arc<AtomicU64>,
    seed: u64,
}

impl core::fmt::Debug for World {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("World")
            .field("backend", &self.backend)
            .finish_non_exhaustive()
    }
}

impl World {
    pub(crate) async fn new(
        backend: Backend,
        spec: SimpleSpec,
        seed: u64,
        trace: Trace,
    ) -> Result<Self, Error> {
        let registry = Arc::new(Registry::default());
        let net = Net::new(registry.clone(), trace.clone());
        let store_manager = Arc::new(StoreManager::new());
        store_manager.add_store(
            CAS_NAME,
            store_factory(
                &StoreSpec::Memory(MemorySpec::default()),
                &store_manager,
                None,
            )
            .await?,
        )?;
        let cas = store_manager
            .get_store(CAS_NAME)
            .err_tip(|| "The CAS was just added")?;
        let redis = match backend {
            Backend::Memory => None,
            Backend::FakeRedis => Some(FakeRedisBackend::new()),
        };
        let mut world = Self {
            backend,
            spec,
            net,
            trace,
            registry,
            store_manager,
            cas,
            redis,
            scheduler: None,
            workers: Vec::new(),
            clients: HashMap::new(),
            next_worker_id: Arc::new(AtomicU64::new(1)),
            next_operation_id: Arc::new(AtomicU64::new(1)),
            passes: Arc::new(AtomicU64::new(0)),
            max_commands_per_pass: Arc::new(AtomicU64::new(0)),
            measure_from_ms: Arc::new(AtomicU64::new(0)),
            seed,
        };
        world.scheduler_up().await?;
        Ok(world)
    }

    pub const fn backend(&self) -> Backend {
        self.backend
    }

    pub const fn spec(&self) -> &SimpleSpec {
        &self.spec
    }

    pub const fn trace(&self) -> &Trace {
        &self.trace
    }

    pub const fn redis(&self) -> Option<&FakeRedisBackend<RedisSubscriptionManager>> {
        self.redis.as_ref()
    }

    pub fn passes(&self) -> u64 {
        self.passes.load(Ordering::Acquire)
    }

    pub fn max_commands_per_pass(&self) -> u64 {
        self.max_commands_per_pass.load(Ordering::Acquire)
    }

    /// Measures store commands per pass only from `ms` on.
    pub(crate) fn measure_from(&self, ms: u64) {
        self.measure_from_ms.store(ms, Ordering::Release);
    }

    pub(crate) fn worker_specs(&self) -> Vec<(String, WorkerSpec)> {
        self.workers
            .iter()
            .enumerate()
            .map(|(i, w)| (worker_name(i), w.spec.clone()))
            .collect()
    }

    /// Builds the scheduler and its servers and starts serving.
    pub(crate) async fn scheduler_up(&mut self) -> Result<(), Error> {
        if self.scheduler.is_some() {
            return Ok(());
        }
        let notify = Arc::new(Notify::new());
        let passes = self.passes.clone();
        let max_commands = self.max_commands_per_pass.clone();
        let measure_from_ms = self.measure_from_ms.clone();
        let redis = self.redis.clone();
        let on_pass = move || {
            passes.fetch_add(1, Ordering::AcqRel);
            if let Some(redis) = &redis {
                let commands: u64 = redis.command_counts().values().sum();
                redis.reset_command_counts();
                if SimClock::now_ms() >= measure_from_ms.load(Ordering::Acquire) {
                    max_commands.fetch_max(commands, Ordering::AcqRel);
                }
            }
            tokio::time::sleep(Duration::from_millis(1))
        };
        let mut redis_port = None;
        let (client, worker) = match self.backend {
            Backend::Memory => SimpleScheduler::new_with_fleet_exchange_interval(
                &self.spec,
                MemoryAwaitedActionDb::new_with_operation_id_fn(
                    &EvictionPolicy::default(),
                    notify.clone(),
                    SimInstant::now,
                    OperationIdFn::new(operation_ids(self.next_operation_id.clone())),
                ),
                on_pass,
                notify,
                SimInstant::now,
                None,
                None,
                Duration::from_secs(5),
            ),
            Backend::FakeRedis => {
                let fake = self
                    .redis
                    .clone()
                    .err_tip(|| "FakeRedis backend without a fake")?;
                // A listener per incarnation, so stopping this scheduler can
                // take its connections with it.
                let port = fake.clone().run().await;
                redis_port = Some(port);
                let store = RedisStore::new_standard(RedisSpec {
                    addresses: vec![format!("redis://127.0.0.1:{port}")],
                    experimental_pub_sub_channel: Some("sim_channel".to_string()),
                    ..Default::default()
                })
                .await?;
                fake.set_subscription_manager(store.subscription_manager().await?);
                let db = StoreAwaitedActionDb::new(
                    store,
                    notify.clone(),
                    SimInstant::now,
                    operation_ids(self.next_operation_id.clone()),
                    self.spec.retain_completed_for_s,
                    self.spec.client_action_timeout_s,
                    false,
                )
                .await?;
                SimpleScheduler::new_with_fleet_exchange_interval(
                    &self.spec,
                    db,
                    on_pass,
                    notify,
                    SimInstant::now,
                    None,
                    None,
                    Duration::from_secs(5),
                )
            }
        };

        let mut providers: HashMap<String, Arc<dyn KnownPlatformPropertyProvider>> = HashMap::new();
        providers.insert(SCHEDULER_NAME.to_string(), client.clone());
        let execution = ExecutionServer::new(
            &[WithInstanceName {
                instance_name: INSTANCE_NAME.to_string(),
                config: ExecutionConfig {
                    cas_store: CAS_NAME.to_string(),
                    scheduler: SCHEDULER_NAME.to_string(),
                    experimental_buck2_invocation_isolation: false,
                    execute_stream_heartbeat_s: 0,
                },
            }],
            &providers,
            &self.store_manager,
        )?
        .with_now_fn(SimClock::now);

        let mut schedulers: HashMap<String, Arc<dyn WorkerScheduler>> = HashMap::new();
        schedulers.insert(SCHEDULER_NAME.to_string(), worker.clone());
        let config = WorkerApiConfig {
            scheduler: SCHEDULER_NAME.to_string(),
            disable_kill_revoked_operations: false,
            kill_revoked_operations_interval_s: 0,
        };
        let now_fn: Arc<NowFn> = Arc::new(Box::new(since_epoch));
        WorkerApiServer::spawn_timers(&config, &schedulers, &now_fn);
        let next_worker_id = self.next_worker_id.clone();
        let worker_api =
            WorkerApiServer::new_with_now_fn(&config, &schedulers, Box::new(since_epoch), [0; 6])?
                .with_worker_id_fn(Box::new(move || {
                    format!("{}", next_worker_id.fetch_add(1, Ordering::AcqRel))
                }));

        self.net.serve(Arc::new(worker_api), execution);
        self.scheduler = Some(Scheduler {
            client,
            redis_port,
            _worker: worker,
        });
        self.trace.record(Event::SchedulerUp);
        Ok(())
    }

    /// Stops serving and drops this process's scheduler; every connection
    /// to it closes. The store, Redis or memory, is what survives.
    /// What a dead process leaves behind is only the store: its tasks may
    /// linger in this runtime, but they lose the servers and, on Redis,
    /// every connection to the store.
    pub(crate) fn scheduler_down(&mut self) {
        if let Some(scheduler) = self.scheduler.take() {
            self.net.unserve();
            if let (Some(redis), Some(port)) = (&self.redis, scheduler.redis_port) {
                redis.retire_listener(port);
            }
            self.trace.record(Event::SchedulerDown);
        }
    }

    pub(crate) fn add_workers(&mut self, count: usize, spec: &WorkerSpec) {
        for _ in 0..count {
            let index = self.workers.len();
            let name = worker_name(index);
            let config = LocalWorkerConfig {
                name: name.clone(),
                max_inflight_tasks: spec.max_inflight,
                platform_properties: spec
                    .properties
                    .iter()
                    .map(|(k, v)| (k.clone(), WorkerProperty::Values(vec![v.clone()])))
                    .collect(),
                worker_api_endpoint: EndpointConfig {
                    timeout: Some(spec.keepalive_timeout_s),
                    ..Default::default()
                },
                ..Default::default()
            };
            let manager = Arc::new(SimActionsManager::new(&name, self.net.clone()));
            let net = self.net.clone();
            let connect_name = name.clone();
            // A fixed sequence per worker, so backoffs repeat run to run.
            let jitter = AtomicU32::new(
                u32::try_from(self.seed.wrapping_add(index as u64 * 7919) % 1000).unwrap_or(0),
            );
            let worker = LocalWorker::new_with_connection_factory_and_actions_manager(
                Arc::new(config),
                manager.clone(),
                Box::new(move || {
                    let client = InProcessWorkerClient::connect(&connect_name, &net);
                    Box::pin(async move { client })
                }),
                Box::new(|duration| Box::pin(tokio::time::sleep(duration))),
            )
            .with_jitter_fn(Box::new(move || jitter.fetch_add(379, Ordering::AcqRel)));
            let (shutdown, shutdown_rx) = broadcast::channel(1);
            let trace = self.trace.clone();
            let stopped_name = name.clone();
            let task = background_spawn!("sim_worker", async move {
                let result = worker.run(shutdown_rx).await;
                trace.record(Event::WorkerStopped {
                    worker: stopped_name,
                });
                result
            });
            self.trace.record(Event::WorkerStarted { worker: name });
            self.workers.push(Worker {
                task,
                manager,
                spec: spec.clone(),
                _shutdown: shutdown,
            });
        }
    }

    /// The worker process dies: its task is aborted and its connection
    /// closes.
    pub(crate) fn kill_worker(&self, index: usize) {
        if let Some(worker) = self.workers.get(index) {
            if worker.task.is_finished() {
                return;
            }
            worker.task.abort();
            self.net.cut(&worker_name(index));
            self.trace.record(Event::WorkerStopped {
                worker: worker_name(index),
            });
        }
    }

    pub(crate) fn cut_worker(&self, index: usize) {
        self.net.cut(&worker_name(index));
    }

    pub(crate) fn set_link(&self, index: usize, link: Link) {
        self.net.set_link(&worker_name(index), link);
    }

    pub(crate) async fn execute(&mut self, op: &str, action: &Action) -> Result<(), Error> {
        let action_digest = self.upload(action).await?;
        self.trace.record(Event::Submit {
            op: op.to_string(),
            action: action.label(),
            priority: action.priority,
        });
        let request = ExecuteRequest {
            instance_name: INSTANCE_NAME.to_string(),
            skip_cache_lookup: !action.cacheable,
            action_digest: Some(action_digest.into()),
            execution_policy: Some(ExecutionPolicy {
                priority: action.priority,
            }),
            results_cache_policy: None,
            digest_function: digest_function::Value::Sha256.into(),
            inline_stdout: false,
            inline_stderr: false,
            inline_output_files: Vec::new(),
        };
        let state = Arc::new(AtomicU8::new(CLIENT_RUNNING));
        let (gone, gone_rx) = watch::channel(false);
        drop(background_spawn!(
            "sim_client",
            run_client(
                self.net.clone(),
                op.to_string(),
                request,
                state.clone(),
                gone_rx
            )
        ));
        self.clients.insert(
            op.to_string(),
            Client {
                state,
                gone,
                action: action.clone(),
            },
        );
        Ok(())
    }

    pub(crate) fn client_gone(&self, op: &str) {
        if let Some(client) = self.clients.get(op) {
            client.gone.send_replace(true);
        }
    }

    pub(crate) fn action_of(&self, op: &str) -> Option<Action> {
        self.clients.get(op).map(|c| c.action.clone())
    }

    async fn upload(&self, action: &Action) -> Result<DigestInfo, Error> {
        let command = Command {
            arguments: vec!["sim".to_string(), action.label()],
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(
            &command,
            self.cas.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let input_root_digest = serialize_and_upload_message(
            &Directory::default(),
            self.cas.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let proto = ActionProto {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            timeout: action.timeout.map(prost_types_duration),
            do_not_cache: !action.cacheable,
            platform: (!action.properties.is_empty()).then(|| Platform {
                properties: action
                    .properties
                    .iter()
                    .map(|(name, value)| platform::Property {
                        name: name.clone(),
                        value: value.clone(),
                    })
                    .collect(),
            }),
            ..Default::default()
        };
        let digest = serialize_and_upload_message(
            &proto,
            self.cas.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        self.registry.insert(digest, action.clone());
        Ok(digest)
    }

    /// Clients still waiting on an operation.
    fn clients_waiting(&self) -> usize {
        self.clients
            .values()
            .filter(|c| c.state.load(Ordering::Acquire) == CLIENT_RUNNING)
            .count()
    }

    /// Executions the live workers have not finished.
    fn executions_running(&self) -> usize {
        self.workers
            .iter()
            .filter(|w| !w.task.is_finished())
            .map(|w| w.manager.running())
            .sum()
    }

    /// Advances until every client has its answer, no worker is running
    /// anything and nothing has reached the trace for `QUIET`, or `budget`
    /// runs out. Returns whether it got quiet.
    pub(crate) async fn settle(&self, clock: &SimClock, budget: Duration) -> bool {
        const QUIET: Duration = Duration::from_secs(2);
        let poll = Duration::from_millis(100);
        let mut waited = Duration::ZERO;
        let mut quiet_for = Duration::ZERO;
        let mut seen = self.trace.len();
        loop {
            let idle = self.clients_waiting() == 0 && self.executions_running() == 0;
            let records = self.trace.len();
            if idle && records == seen {
                if quiet_for >= QUIET {
                    return true;
                }
            } else {
                quiet_for = Duration::ZERO;
            }
            seen = records;
            if waited >= budget {
                return false;
            }
            clock.advance(poll).await;
            waited += poll;
            if idle {
                quiet_for += poll;
            }
        }
    }

    /// Records what the scheduler has executing next to what the workers
    /// run.
    pub(crate) async fn record_ledger(&self) -> Result<(), Error> {
        let scheduler_executing = match &self.scheduler {
            Some(scheduler) => {
                let stream = scheduler
                    .client
                    .filter_operations(OperationFilter {
                        stages: OperationStageFlags::Executing,
                        // The Redis backend only lists in this order.
                        order_by_priority_direction: Some(OrderDirection::Desc),
                        ..Default::default()
                    })
                    .await?;
                stream.count().await
            }
            None => 0,
        };
        self.trace.record(Event::Ledger {
            scheduler_executing,
            workers_running: self.executions_running(),
        });
        Ok(())
    }

    /// Stops every task this world started, so the runtime can go.
    pub(crate) fn shutdown(&mut self) {
        for client in self.clients.values() {
            client.gone.send_replace(true);
        }
        for (index, worker) in self.workers.iter().enumerate() {
            worker.task.abort();
            self.net.cut(&worker_name(index));
        }
        self.scheduler_down();
    }
}

/// Operation ids in order, `op-1` first, carried over a restart so a new
/// scheduler never reuses one.
fn operation_ids(next: Arc<AtomicU64>) -> impl Fn() -> OperationId + Clone + Send + Sync + 'static {
    move || OperationId::from(format!("op-{}", next.fetch_add(1, Ordering::AcqRel)))
}

/// The virtual time since the Unix epoch, as `WorkerApiServer` reads it.
#[allow(clippy::unnecessary_wraps, reason = "the shape `NowFn` takes")]
fn since_epoch() -> Result<Duration, Error> {
    Ok(SimClock::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default())
}

pub(crate) fn worker_name(index: usize) -> String {
    format!("w{index}")
}

fn prost_types_duration(duration: Duration) -> prost_types::Duration {
    prost_types::Duration {
        seconds: i64::try_from(duration.as_secs()).unwrap_or(i64::MAX),
        nanos: i32::try_from(duration.subsec_nanos()).unwrap_or(0),
    }
}

fn stage_of(operation: &Operation) -> Stage {
    operation
        .metadata
        .as_ref()
        .and_then(|any| ExecuteOperationMetadata::decode(any.value.as_slice()).ok())
        .map_or(Stage::Unknown, |metadata| Stage::from_proto(metadata.stage))
}

fn outcome_of(operation: &Operation) -> Outcome {
    match &operation.result {
        Some(operation::Result::Error(status)) => Outcome::Failed(status.code),
        Some(operation::Result::Response(any)) => {
            match ExecuteResponse::decode(any.value.as_slice()) {
                Ok(response) => match response.status {
                    Some(status) if status.code != 0 => Outcome::Failed(status.code),
                    _ => Outcome::Exited(response.result.map_or(0, |r| r.exit_code)),
                },
                Err(_) => Outcome::Failed(Code::Internal as i32),
            }
        }
        None => Outcome::Failed(Code::Unknown as i32),
    }
}

/// How many times a client retries a call that failed outright before it
/// gives up, as Bazel's `--remote_retries` does.
const CLIENT_RETRIES: u32 = 5;
const CLIENT_RETRY_DELAY: Duration = Duration::from_secs(1);

/// One client operation: `Execute`, then read the stream to the end. A
/// stream that ends or fails early is re-subscribed with `WaitExecution`;
/// one the server no longer knows is executed again.
async fn run_client(
    net: Arc<Net>,
    op: String,
    request: ExecuteRequest,
    state: Arc<AtomicU8>,
    mut gone: watch::Receiver<bool>,
) {
    let trace = net.trace().clone();
    let mut name: Option<String> = None;
    let mut failures = 0;
    let mut last_stage = None;
    loop {
        if *gone.borrow() {
            break;
        }
        let Some(server) = net.execution() else {
            if wait_or_gone(&mut gone, CLIENT_RETRY_DELAY).await {
                break;
            }
            continue;
        };
        let mut epoch = net.epoch();
        let start_epoch = *epoch.borrow_and_update();
        let response = match &name {
            None => server.execute(Request::new(request.clone())).await,
            Some(name) => {
                server
                    .wait_execution(Request::new(WaitExecutionRequest { name: name.clone() }))
                    .await
            }
        };
        let mut stream = match response {
            Ok(response) => response.into_inner(),
            Err(status) => {
                if name.is_some() && status.code() == TonicCode::NotFound {
                    trace.record(Event::ClientResubmit { op: op.clone() });
                    name = None;
                    last_stage = None;
                    continue;
                }
                failures += 1;
                if failures > CLIENT_RETRIES {
                    trace.record(Event::ClientDone {
                        op: op.clone(),
                        outcome: Outcome::Failed(status.code() as i32),
                    });
                    state.store(CLIENT_DONE, Ordering::Release);
                    return;
                }
                trace.record(Event::ClientReconnect {
                    op: op.clone(),
                    code: status.code() as i32,
                });
                if wait_or_gone(&mut gone, CLIENT_RETRY_DELAY).await {
                    break;
                }
                continue;
            }
        };
        loop {
            tokio::select! {
                item = stream.next() => match item {
                    Some(Ok(operation)) => {
                        name = Some(operation.name.clone());
                        let seen = stage_of(&operation);
                        if last_stage != Some(seen) {
                            trace.record(Event::ClientStage { op: op.clone(), stage: seen });
                            last_stage = Some(seen);
                        }
                        if operation.done {
                            trace.record(Event::ClientDone {
                                op: op.clone(),
                                outcome: outcome_of(&operation),
                            });
                            state.store(CLIENT_DONE, Ordering::Release);
                            return;
                        }
                    }
                    Some(Err(status)) => {
                        trace.record(Event::ClientReconnect { op: op.clone(), code: status.code() as i32 });
                        break;
                    }
                    None => {
                        trace.record(Event::ClientReconnect { op: op.clone(), code: -1 });
                        break;
                    }
                },
                changed = gone.changed() => {
                    if changed.is_err() || *gone.borrow() {
                        drop(stream);
                        trace.record(Event::ClientGone { op: op.clone() });
                        state.store(CLIENT_GONE, Ordering::Release);
                        return;
                    }
                }
                changed = epoch.changed() => {
                    if changed.is_err() || *epoch.borrow() != start_epoch {
                        trace.record(Event::ClientReconnect { op: op.clone(), code: Code::Unavailable as i32 });
                        break;
                    }
                }
            }
        }
        last_stage = None;
        if wait_or_gone(&mut gone, CLIENT_RETRY_DELAY).await {
            break;
        }
    }
    trace.record(Event::ClientGone { op });
    state.store(CLIENT_GONE, Ordering::Release);
}

/// Sleeps `delay`; true when the client left meanwhile.
async fn wait_or_gone(gone: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    tokio::select! {
        () = tokio::time::sleep(delay) => *gone.borrow(),
        changed = gone.changed() => changed.is_err() || *gone.borrow(),
    }
}
