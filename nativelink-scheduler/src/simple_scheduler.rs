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

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use async_trait::async_trait;
use futures::{Future, StreamExt, future};
use nativelink_config::schedulers::SimpleSpec;
use nativelink_config::stores::ClientTlsConfig;
use nativelink_error::{Code, Error, ResultExt};
use nativelink_metric::{MetricsComponent, RootMetricsComponent};
use nativelink_proto::com::github::trace_machina::nativelink::events::OriginEvent;
use nativelink_util::action_messages::{ActionInfo, ActionState, OperationId, WorkerId};
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, MatchingEngineStateManager,
    OperationFilter, OperationStageFlags, OrderDirection, UpdateOperationType,
};
use nativelink_util::origin_event::OriginMetadata;
use nativelink_util::platform_properties::PlatformProperties;
use nativelink_util::shutdown_guard::ShutdownGuard;
use nativelink_util::spawn;
use nativelink_util::task::JoinHandleDropGuard;
use opentelemetry::KeyValue;
use opentelemetry::baggage::BaggageExt;
use opentelemetry::context::{Context, FutureExt as OtelFutureExt};
use opentelemetry_semantic_conventions::attribute::ENDUSER_ID;
use tokio::sync::{Notify, mpsc};
use tokio::time::Duration;
use tracing::{debug, error, info, info_span, warn};

use crate::api_worker_scheduler::ApiWorkerScheduler;
use crate::awaited_action_db::{AwaitedActionDb, CLIENT_KEEPALIVE_DURATION};
use crate::platform_property_manager::PlatformPropertyManager;
use crate::simple_scheduler_state_manager::SimpleSchedulerStateManager;
use crate::worker::{ActionInfoWithProps, Worker, WorkerTimestamp};
use crate::worker_registry::WorkerRegistry;
use crate::worker_scheduler::WorkerScheduler;

/// Default timeout for workers in seconds.
/// If this changes, remember to change the documentation in the config.
/// A 5-second timeout causes unnecessary worker churn on any brief network
/// hiccup or GC pause, so we use a more generous default.
const DEFAULT_WORKER_TIMEOUT_S: u64 = 30;

/// Mark operations as completed with error if no client has updated them
/// within this duration.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_CLIENT_ACTION_TIMEOUT_S: u64 = 60;

/// Default times a job can retry before failing.
/// If this changes, remember to change the documentation in the config.
const DEFAULT_MAX_JOB_RETRIES: usize = 3;

struct SimpleSchedulerActionStateResult {
    client_operation_id: OperationId,
    action_state_result: Box<dyn ActionStateResult>,
}

impl SimpleSchedulerActionStateResult {
    fn new(
        client_operation_id: OperationId,
        action_state_result: Box<dyn ActionStateResult>,
    ) -> Self {
        Self {
            client_operation_id,
            action_state_result,
        }
    }
}

#[async_trait]
impl ActionStateResult for SimpleSchedulerActionStateResult {
    async fn as_state(&self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        let (mut action_state, origin_metadata) = self
            .action_state_result
            .as_state()
            .await
            .err_tip(|| "In SimpleSchedulerActionStateResult")?;
        // We need to ensure the client is not aware of the downstream
        // operation id, so override it before it goes out.
        Arc::make_mut(&mut action_state).client_operation_id = self.client_operation_id.clone();
        Ok((action_state, origin_metadata))
    }

    async fn changed(&mut self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        let (mut action_state, origin_metadata) = self
            .action_state_result
            .changed()
            .await
            .err_tip(|| "In SimpleSchedulerActionStateResult")?;
        // We need to ensure the client is not aware of the downstream
        // operation id, so override it before it goes out.
        Arc::make_mut(&mut action_state).client_operation_id = self.client_operation_id.clone();
        Ok((action_state, origin_metadata))
    }

    async fn as_action_info(&self) -> Result<(Arc<ActionInfo>, Option<OriginMetadata>), Error> {
        self.action_state_result
            .as_action_info()
            .await
            .err_tip(|| "In SimpleSchedulerActionStateResult")
    }
}

/// Engine used to manage the queued/running tasks and relationship with
/// the worker nodes. All state on how the workers and actions are interacting
/// should be held in this struct.
#[derive(MetricsComponent)]
pub struct SimpleScheduler {
    /// Manager for matching engine side of the state manager.
    #[metric(group = "matching_engine_state_manager")]
    matching_engine_state_manager: Arc<dyn MatchingEngineStateManager>,

    /// Manager for client state of this scheduler.
    #[metric(group = "client_state_manager")]
    client_state_manager: Arc<dyn ClientStateManager>,

    /// Manager for platform of this scheduler.
    #[metric(group = "platform_properties")]
    platform_property_manager: Arc<PlatformPropertyManager>,

    /// A `Workers` pool that contains all workers that are available to execute actions in a priority
    /// order based on the allocation strategy.
    #[metric(group = "worker_scheduler")]
    worker_scheduler: Arc<ApiWorkerScheduler>,

    /// The sender to send origin events to the origin events.
    maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,

    /// Background task that tries to match actions to workers. If this struct
    /// is dropped the spawn will be cancelled as well.
    task_worker_matching_spawn: JoinHandleDropGuard<()>,

    /// Every duration, do logging of worker matching
    /// e.g. "worker busy", "can't find any worker"
    /// Set to None to disable. This is quite noisy, so we limit it
    worker_match_logging_interval: Option<Duration>,

    /// Maximum number of actions that can be matched per client
    /// (identified by `instance_name`) in one matching cycle.
    /// 0 means unlimited (fair scheduling disabled).
    max_matches_per_client_per_cycle: usize,
}

impl core::fmt::Debug for SimpleScheduler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("SimpleScheduler")
            .field("platform_property_manager", &self.platform_property_manager)
            .field("worker_scheduler", &self.worker_scheduler)
            .field("maybe_origin_event_tx", &self.maybe_origin_event_tx)
            .field(
                "task_worker_matching_spawn",
                &self.task_worker_matching_spawn,
            )
            .finish_non_exhaustive()
    }
}

impl SimpleScheduler {
    /// Attempts to find a worker to execute an action and begins executing it.
    /// If an action is already running that is cacheable it may merge this
    /// action with the results and state changes of the already running
    /// action. If the task cannot be executed immediately it will be queued
    /// for execution based on priority and other metrics.
    /// All further updates to the action will be provided through the returned
    /// value.
    async fn inner_add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        let action_state_result = self
            .client_state_manager
            .add_action(client_operation_id.clone(), action_info)
            .await
            .err_tip(|| "In SimpleScheduler::add_action")?;
        Ok(Box::new(SimpleSchedulerActionStateResult::new(
            client_operation_id.clone(),
            action_state_result,
        )))
    }

    async fn inner_filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream<'_>, Error> {
        self.client_state_manager
            .filter_operations(filter)
            .await
            .err_tip(|| "In SimpleScheduler::find_by_client_operation_id getting filter result")
    }

    async fn get_queued_operations(&self) -> Result<ActionStateResultStream<'_>, Error> {
        let filter = OperationFilter {
            stages: OperationStageFlags::Queued,
            order_by_priority_direction: Some(OrderDirection::Desc),
            ..Default::default()
        };
        self.matching_engine_state_manager
            .filter_operations(filter)
            .await
            .err_tip(|| "In SimpleScheduler::get_queued_operations getting filter result")
    }

    pub async fn do_try_match_for_test(&self) -> Result<(), Error> {
        self.do_try_match(true).await
    }

    // TODO(palfrey) This is an O(n*m) (aka n^2) algorithm. In theory we
    // can create a map of capabilities of each worker and then try and match
    // the actions to the worker using the map lookup (ie. map reduce).
    async fn do_try_match(&self, full_worker_logging: bool) -> Result<(), Error> {
        /// Maximum number of actions to process concurrently during matching.
        /// find_and_reserve_worker atomically finds AND reserves the worker
        /// (reducing platform properties and inserting into running_action_infos)
        /// under a single lock acquisition, so concurrent matches cannot
        /// select the same worker.
        ///
        /// Increased from 8 to 32 to reduce queue drain time during burst
        /// scheduling (e.g. build startup). With 10+ workers the higher
        /// concurrency prevents a backlog without meaningful lock contention
        /// since the worker registry write lock is held briefly per match.
        const MATCH_CONCURRENCY: usize = 32;

        // Cache for computed platform properties, keyed by sorted key-value
        // pairs. This avoids recomputing the same PlatformProperties for
        // actions that share identical platform requirements (the common case).
        let props_cache: std::sync::Mutex<
            HashMap<Vec<(String, String)>, Arc<PlatformProperties>>,
        > = std::sync::Mutex::new(HashMap::new());

        // Per-client match counter for fair scheduling. When
        // max_matches_per_client_per_cycle > 0, limits how many actions
        // from the same instance_name can be matched in one cycle,
        // preventing a single client from monopolizing all workers.
        let per_client_matches: std::sync::Mutex<HashMap<String, usize>> =
            std::sync::Mutex::new(HashMap::new());
        let max_per_client = self.max_matches_per_client_per_cycle;

        let start = Instant::now();

        let stream = self
            .get_queued_operations()
            .await
            .err_tip(|| "Failed to get queued operations in do_try_match")?;

        let query_elapsed = start.elapsed();
        if query_elapsed > Duration::from_secs(1) {
            warn!(
                elapsed_ms = query_elapsed.as_millis(),
                "Slow get_queued_operations query"
            );
        }

        // Collect all queued actions so we own them, then process up to
        // MATCH_CONCURRENCY concurrently using FuturesUnordered. Each action
        // independently finds a worker and assigns itself; conflicts are
        // resolved by the existing error handling (Aborted codes, None from
        // find_worker, etc.).
        let queued_actions: Vec<Box<dyn ActionStateResult>> = stream.collect().await;

        let mut futures_set = futures::stream::FuturesUnordered::<
            std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Error>> + Send + '_>>,
        >::new();
        let mut action_iter = queued_actions.into_iter();
        let mut result = Ok(());

        // Seed the initial batch.
        for action_state_result in action_iter.by_ref().take(MATCH_CONCURRENCY) {
            futures_set.push(Box::pin(Self::match_action_to_worker_cached(
                action_state_result,
                self.worker_scheduler.as_ref(),
                self.matching_engine_state_manager.as_ref(),
                self.platform_property_manager.as_ref(),
                &props_cache,
                &per_client_matches,
                max_per_client,
                full_worker_logging,
            )));
        }

        // Process futures as they complete, adding new ones to maintain concurrency.
        while let Some(match_result) = futures_set.next().await {
            result = result.merge(match_result);

            if let Some(action_state_result) = action_iter.next() {
                futures_set.push(Box::pin(Self::match_action_to_worker_cached(
                    action_state_result,
                    self.worker_scheduler.as_ref(),
                    self.matching_engine_state_manager.as_ref(),
                    self.platform_property_manager.as_ref(),
                    &props_cache,
                    &per_client_matches,
                    max_per_client,
                    full_worker_logging,
                )));
            }
        }

        let total_elapsed = start.elapsed();
        if total_elapsed > Duration::from_secs(5) {
            warn!(
                total_ms = total_elapsed.as_millis(),
                query_ms = query_elapsed.as_millis(),
                "Slow do_try_match cycle"
            );
        }

        result
    }

    /// Matches a single action to a worker, using a shared cache for computed
    /// platform properties to avoid redundant recomputation across actions
    /// with identical platform requirements.
    ///
    /// When `max_per_client > 0`, enforces fair scheduling by limiting how
    /// many actions from the same `instance_name` can be matched per cycle.
    /// Actions that exceed the limit are skipped (left in queue for next cycle).
    async fn match_action_to_worker_cached(
        action_state_result: Box<dyn ActionStateResult>,
        workers: &ApiWorkerScheduler,
        matching_engine_state_manager: &dyn MatchingEngineStateManager,
        platform_property_manager: &PlatformPropertyManager,
        props_cache: &std::sync::Mutex<
            HashMap<Vec<(String, String)>, Arc<PlatformProperties>>,
        >,
        per_client_matches: &std::sync::Mutex<HashMap<String, usize>>,
        max_per_client: usize,
        full_worker_logging: bool,
    ) -> Result<(), Error> {
        let (action_info, maybe_origin_metadata) = action_state_result
            .as_action_info()
            .await
            .err_tip(|| "Failed to get action_info from as_action_info_result stream")?;

        // Fair scheduling: atomically check and optimistically increment the
        // per-client counter. If the client has hit its limit, skip the action.
        // If the match later fails, we decrement to undo the reservation.
        let client_name = action_info.instance_name().clone();
        let claimed_slot = if max_per_client > 0 {
            let mut map = per_client_matches.lock().unwrap_or_else(|e| e.into_inner());
            let count = map.entry(client_name.clone()).or_insert(0);
            if *count >= max_per_client {
                // Skip — action stays queued for next cycle.
                return Ok(());
            }
            *count += 1;
            true
        } else {
            false
        };

        // Helper to undo the optimistic increment on failure paths.
        let undo_claim = |per_client_matches: &std::sync::Mutex<HashMap<String, usize>>,
                          client_name: &str| {
            let mut map = per_client_matches.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(count) = map.get_mut(client_name) {
                *count = count.saturating_sub(1);
            }
        };

        // Build a deterministic cache key from the raw platform
        // properties (sorted key-value pairs).
        let mut cache_key: Vec<(String, String)> =
            action_info.platform_properties.clone().into_iter().collect();
        cache_key.sort();

        // Look up or compute and cache the platform properties.
        let platform_properties = {
            let mut cache = props_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(cached) = cache.get(&cache_key) {
                cached.clone()
            } else {
                let computed = platform_property_manager
                    .make_platform_properties(action_info.platform_properties.clone())
                    .err_tip(|| {
                        "Failed to make platform properties in SimpleScheduler::do_try_match"
                    })?;
                let arc = Arc::new(computed);
                cache.insert(cache_key, arc.clone());
                arc
            }
        };

        let action_info_with_props = ActionInfoWithProps {
            inner: action_info,
            platform_properties: (*platform_properties).clone(),
        };

        // Extract the operation_id from the action_state BEFORE finding a
        // worker, so we can pass it to find_and_reserve_worker for atomic
        // reservation.
        let operation_id = {
            let (action_state, _origin_metadata) = action_state_result
                .as_state()
                .await
                .err_tip(|| "Failed to get action_info from as_state_result stream")?;
            action_state.client_operation_id.clone()
        };

        // Atomically find a worker AND reserve it for this operation.
        // The worker's platform properties are reduced and the action is
        // recorded in running_action_infos under a single lock acquisition,
        // preventing concurrent matches from selecting the same worker.
        let (worker_id, tx, msg) = match workers
            .find_and_reserve_worker(
                &action_info_with_props.platform_properties,
                &operation_id,
                &action_info_with_props,
                full_worker_logging,
            )
            .await
        {
            Some(result) => result,
            // No worker found — undo the optimistic increment.
            None => {
                if claimed_slot {
                    undo_claim(per_client_matches, &client_name);
                }
                return Ok(());
            }
        };

        // Tell the matching engine that the operation is being assigned to a worker.
        let assign_result = matching_engine_state_manager
            .assign_operation(&operation_id, Ok(&worker_id))
            .await
            .err_tip(|| "Failed to assign operation in do_try_match");
        if let Err(err) = assign_result {
            // Undo the worker reservation since the assignment failed.
            workers.unreserve_worker(&worker_id, &operation_id).await;
            if claimed_slot {
                undo_claim(per_client_matches, &client_name);
            }
            if err.code == Code::Aborted {
                // The operation was cancelled due to another operation
                // being assigned to the worker.
                return Ok(());
            }
            // Any other error is a real error.
            return Err(err);
        }

        let origin_metadata = maybe_origin_metadata.unwrap_or_default();
        let ctx = Context::current_with_baggage(vec![KeyValue::new(
            ENDUSER_ID,
            origin_metadata.identity,
        )]);

        let notify_fut = async {
            debug!(
                %worker_id,
                %operation_id,
                ?action_info_with_props,
                "Notifying worker of operation"
            );
            workers
                .send_reserved_worker_notification(&worker_id, tx, msg)
                .await
                .err_tip(|| {
                    "Failed to send_reserved_worker_notification in SimpleScheduler::do_try_match"
                })
        };

        info_span!("do_try_match")
            .in_scope(|| notify_fut)
            .with_context(ctx)
            .await
    }
}

impl SimpleScheduler {
    pub fn new<A: AwaitedActionDb>(
        spec: &SimpleSpec,
        awaited_action_db: A,
        task_change_notify: Arc<Notify>,
        maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,
    ) -> (Arc<Self>, Arc<dyn WorkerScheduler>) {
        Self::new_with_cas_store(
            spec,
            awaited_action_db,
            task_change_notify,
            maybe_origin_event_tx,
            None,
            None,
            None,
        )
    }

    pub fn new_with_cas_store<A: AwaitedActionDb>(
        spec: &SimpleSpec,
        awaited_action_db: A,
        task_change_notify: Arc<Notify>,
        maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,
        cas_store: Option<nativelink_util::store_trait::Store>,
        locality_map: Option<nativelink_util::blob_locality_map::SharedBlobLocalityMap>,
        worker_tls_config: Option<ClientTlsConfig>,
    ) -> (Arc<Self>, Arc<dyn WorkerScheduler>) {
        Self::new_with_callback(
            spec,
            awaited_action_db,
            || {
                // Yield to allow other tasks to make progress between match
                // cycles. A full 1ms sleep is too aggressive and caps matching
                // to ~1000 cycles/sec. sleep(ZERO) defers to the next timer
                // tick, preventing busy-spinning when no other tasks are
                // runnable (unlike yield_now which returns immediately).
                tokio::time::sleep(Duration::ZERO)
            },
            task_change_notify,
            SystemTime::now,
            maybe_origin_event_tx,
            cas_store,
            locality_map,
            worker_tls_config,
        )
    }

    pub fn new_with_callback<
        Fut: Future<Output = ()> + Send,
        F: Fn() -> Fut + Send + Sync + 'static,
        A: AwaitedActionDb,
        I: InstantWrapper,
        NowFn: Fn() -> I + Clone + Send + Unpin + Sync + 'static,
    >(
        spec: &SimpleSpec,
        awaited_action_db: A,
        on_matching_engine_run: F,
        task_change_notify: Arc<Notify>,
        now_fn: NowFn,
        maybe_origin_event_tx: Option<mpsc::Sender<OriginEvent>>,
        cas_store: Option<nativelink_util::store_trait::Store>,
        locality_map: Option<nativelink_util::blob_locality_map::SharedBlobLocalityMap>,
        worker_tls_config: Option<ClientTlsConfig>,
    ) -> (Arc<Self>, Arc<dyn WorkerScheduler>) {
        let platform_property_manager = Arc::new(PlatformPropertyManager::new(
            spec.supported_platform_properties
                .clone()
                .unwrap_or_default(),
        ));

        let mut worker_timeout_s = spec.worker_timeout_s;
        if worker_timeout_s == 0 {
            worker_timeout_s = DEFAULT_WORKER_TIMEOUT_S;
        }

        let mut client_action_timeout_s = spec.client_action_timeout_s;
        if client_action_timeout_s == 0 {
            client_action_timeout_s = DEFAULT_CLIENT_ACTION_TIMEOUT_S;
        }
        // This matches the value of CLIENT_KEEPALIVE_DURATION which means that
        // tasks are going to be dropped all over the place, this isn't a good
        // setting.
        if client_action_timeout_s <= CLIENT_KEEPALIVE_DURATION.as_secs() {
            error!(
                client_action_timeout_s,
                "Setting client_action_timeout_s to less than the client keep alive interval is going to cause issues, please set above {}.",
                CLIENT_KEEPALIVE_DURATION.as_secs()
            );
        }

        let mut max_job_retries = spec.max_job_retries;
        if max_job_retries == 0 {
            max_job_retries = DEFAULT_MAX_JOB_RETRIES;
        }

        let worker_change_notify = Arc::new(Notify::new());

        // Create shared worker registry for single heartbeat per worker.
        let worker_registry = Arc::new(WorkerRegistry::new());

        let state_manager = SimpleSchedulerStateManager::new(
            max_job_retries,
            Duration::from_secs(worker_timeout_s),
            Duration::from_secs(client_action_timeout_s),
            Duration::from_secs(spec.max_action_executing_timeout_s),
            awaited_action_db,
            now_fn,
            Some(worker_registry.clone()),
        );

        let worker_scheduler = ApiWorkerScheduler::new_with_locality_map(
            state_manager.clone(),
            platform_property_manager.clone(),
            spec.allocation_strategy,
            worker_change_notify.clone(),
            worker_timeout_s,
            worker_registry,
            locality_map,
            cas_store,
            worker_tls_config,
        );

        let worker_scheduler_clone = worker_scheduler.clone();

        let action_scheduler = Arc::new_cyclic(move |weak_self| -> Self {
            let weak_inner = weak_self.clone();
            let task_worker_matching_spawn =
                spawn!("simple_scheduler_task_worker_matching", async move {
                    let mut last_match_successful = true;
                    let mut worker_match_logging_last: Option<Instant> = None;
                    let mut last_stall_check: Option<Instant> = None;
                    let mut consecutive_match_errors: u32 = 0;
                    // Break out of the loop only when the inner is dropped.
                    loop {
                        let task_change_fut = task_change_notify.notified();
                        let worker_change_fut = worker_change_notify.notified();
                        tokio::pin!(task_change_fut);
                        tokio::pin!(worker_change_fut);
                        // Wait for either of these futures to be ready.
                        let state_changed = future::select(task_change_fut, worker_change_fut);
                        if last_match_successful {
                            let _ = state_changed.await;
                        } else {
                            // If the last match failed, then run again after a short sleep.
                            // This resolves issues where we tried to re-schedule a job to
                            // a disconnected worker.  The sleep ensures we don't enter a
                            // hard loop if there's something wrong inside do_try_match.
                            let sleep_fut = tokio::time::sleep(Duration::from_millis(100));
                            tokio::pin!(sleep_fut);
                            let _ = future::select(state_changed, sleep_fut).await;
                        }

                        let result = match weak_inner.upgrade() {
                            Some(scheduler) => {
                                let now = Instant::now();
                                let full_worker_logging = {
                                    match scheduler.worker_match_logging_interval {
                                        None => false,
                                        Some(duration) => match worker_match_logging_last {
                                            None => true,
                                            Some(when) => now.duration_since(when) >= duration,
                                        },
                                    }
                                };

                                let res = scheduler.do_try_match(full_worker_logging).await;
                                if full_worker_logging {
                                    let operations_stream = scheduler
                                        .matching_engine_state_manager
                                        .filter_operations(OperationFilter::default())
                                        .await
                                        .err_tip(|| "In action_scheduler getting filter result");

                                    let mut oldest_actions_in_state: HashMap<
                                        String,
                                        BTreeSet<Arc<ActionState>>,
                                    > = HashMap::new();
                                    let max_items = 5;

                                    match operations_stream {
                                        Ok(stream) => {
                                            let actions = stream
                                                .filter_map(|item| async move {
                                                    match item.as_ref().as_state().await {
                                                        Ok((action_state, _origin_metadata)) => {
                                                            Some(action_state)
                                                        }
                                                        Err(e) => {
                                                            error!(
                                                                ?e,
                                                                "Failed to get action state!"
                                                            );
                                                            None
                                                        }
                                                    }
                                                })
                                                .collect::<Vec<_>>()
                                                .await;
                                            for action_state in &actions {
                                                let name = action_state.stage.name();
                                                if let Some(values) =
                                                    oldest_actions_in_state.get_mut(&name)
                                                {
                                                    values.insert(action_state.clone());
                                                    if values.len() > max_items {
                                                        values.pop_first();
                                                    }
                                                } else {
                                                    let mut values = BTreeSet::new();
                                                    values.insert(action_state.clone());
                                                    oldest_actions_in_state.insert(name, values);
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            error!(?e, "Failed to get operations list!");
                                        }
                                    }

                                    for value in oldest_actions_in_state.values() {
                                        let mut items = vec![];
                                        for item in value {
                                            items.push(item.to_string());
                                        }
                                        info!(?items, "Oldest actions in state");
                                    }

                                    worker_match_logging_last.replace(now);
                                }

                                // Stall detection: every 30s, check for actions stuck
                                // in Queued state for >60s. Only fires as an error when
                                // no actions are executing (true deadlock). If workers are
                                // busy executing, queued stalls are just capacity limits.
                                let should_check_stalls = match last_stall_check {
                                    None => true,
                                    Some(when) => now.duration_since(when) >= Duration::from_secs(30),
                                };
                                if should_check_stalls {
                                    last_stall_check = Some(now);
                                    let stall_threshold = Duration::from_secs(60);
                                    match scheduler
                                        .matching_engine_state_manager
                                        .filter_operations(OperationFilter {
                                            stages: OperationStageFlags::Queued,
                                            order_by_priority_direction: Some(OrderDirection::Desc),
                                            ..Default::default()
                                        })
                                        .await
                                    {
                                        Ok(queued_stream) => {
                                            let queued_actions: Vec<_> = queued_stream.collect().await;
                                            let mut stalled_count: usize = 0;
                                            let mut unmatchable_count: usize = 0;
                                            let prop_manager = scheduler.worker_scheduler.get_platform_property_manager();
                                            for action_state_result in &queued_actions {
                                                if let Ok((state, _)) = action_state_result.as_state().await {
                                                    if let Ok(elapsed) = state.last_transition_timestamp.elapsed() {
                                                        if elapsed > stall_threshold {
                                                            stalled_count += 1;
                                                            // Check if any worker could ever match this action.
                                                            match action_state_result.as_action_info().await {
                                                                Ok((action_info, _)) => {
                                                                    match prop_manager.make_platform_properties(
                                                                        action_info.platform_properties.clone(),
                                                                    ) {
                                                                        Ok(props) => {
                                                                            if !scheduler.worker_scheduler.has_matching_workers(&props).await {
                                                                                error!(
                                                                                    operation_id = %state.client_operation_id,
                                                                                    action_digest = %state.action_digest,
                                                                                    properties = ?action_info.platform_properties,
                                                                                    "Action queued >60s with NO matching workers — \
                                                                                     no registered worker can satisfy its platform requirements"
                                                                                );
                                                                                unmatchable_count += 1;
                                                                            }
                                                                        }
                                                                        Err(e) => {
                                                                            warn!(
                                                                                operation_id = %state.client_operation_id,
                                                                                ?e,
                                                                                "Failed to parse platform properties for stalled action — cannot check matchability"
                                                                            );
                                                                        }
                                                                    }
                                                                }
                                                                Err(e) => {
                                                                    warn!(
                                                                        operation_id = %state.client_operation_id,
                                                                        ?e,
                                                                        "Failed to get action_info for stalled action — cannot check matchability"
                                                                    );
                                                                }
                                                            }
                                                        }
                                                    }
                                                }
                                            }
                                            let matchable_stalled = stalled_count - unmatchable_count;
                                            if matchable_stalled > 0 {
                                                // Check if workers are actively executing. If so,
                                                // the queue backlog is just capacity pressure.
                                                let executing_count = match scheduler
                                                    .matching_engine_state_manager
                                                    .filter_operations(OperationFilter {
                                                        stages: OperationStageFlags::Executing,
                                                        ..Default::default()
                                                    })
                                                    .await
                                                {
                                                    Ok(s) => s.count().await,
                                                    Err(e) => {
                                                        // Query failed — assume workers are busy
                                                        // rather than raising a false deadlock alarm.
                                                        warn!(?e, "Failed to query executing actions for stall check");
                                                        usize::MAX
                                                    }
                                                };

                                                if executing_count > 0 {
                                                    warn!(
                                                        stalled_count = matchable_stalled,
                                                        total_queued = queued_actions.len(),
                                                        executing_count,
                                                        unmatchable_count,
                                                        "Actions waiting in queue >60s (workers at capacity)"
                                                    );
                                                } else {
                                                    error!(
                                                        stalled_count = matchable_stalled,
                                                        total_queued = queued_actions.len(),
                                                        unmatchable_count,
                                                        "Actions stalled in Queued state >60s with NO executing actions (possible scheduling deadlock)"
                                                    );
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            error!(
                                                ?e,
                                                "Failed to query queued actions for stall check — scheduler state may be corrupted"
                                            );
                                        }
                                    }
                                }

                                res
                            }
                            // If the inner went away it means the scheduler is shutting
                            // down, so we need to resolve our future.
                            None => return,
                        };
                        last_match_successful = result.is_ok();
                        if let Err(err) = &result {
                            consecutive_match_errors += 1;
                            if consecutive_match_errors >= 10 {
                                error!(
                                    consecutive_match_errors,
                                    ?err,
                                    "do_try_match failing consecutively — \
                                     possible scheduler data structure corruption. \
                                     A server restart may be required to recover.",
                                );
                            } else {
                                error!(?err, "Error while running do_try_match");
                            }
                        } else {
                            consecutive_match_errors = 0;
                        }

                        on_matching_engine_run().await;
                    }
                    // Unreachable.
                });

            let worker_match_logging_interval = match spec.worker_match_logging_interval_s {
                // -1 or 0 means disabled (0 used to cause expensive logging on every call)
                -1 | 0 => None,
                signed_secs => {
                    if let Ok(secs) = TryInto::<u64>::try_into(signed_secs) {
                        Some(Duration::from_secs(secs))
                    } else {
                        error!(
                            worker_match_logging_interval_s = spec.worker_match_logging_interval_s,
                            "Valid values for worker_match_logging_interval_s are -1, 0, or a positive integer, setting to disabled",
                        );
                        None
                    }
                }
            };
            Self {
                matching_engine_state_manager: state_manager.clone(),
                client_state_manager: state_manager.clone(),
                worker_scheduler,
                platform_property_manager,
                maybe_origin_event_tx,
                task_worker_matching_spawn,
                worker_match_logging_interval,
                max_matches_per_client_per_cycle: spec.max_matches_per_client_per_cycle,
            }
        });
        (action_scheduler, worker_scheduler_clone)
    }
}

#[async_trait]
impl ClientStateManager for SimpleScheduler {
    async fn add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        self.inner_add_action(client_operation_id, action_info)
            .await
    }

    async fn filter_operations<'a>(
        &'a self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream<'a>, Error> {
        self.inner_filter_operations(filter).await
    }

    fn as_known_platform_property_provider(&self) -> Option<&dyn KnownPlatformPropertyProvider> {
        Some(self)
    }
}

#[async_trait]
impl KnownPlatformPropertyProvider for SimpleScheduler {
    async fn get_known_properties(&self, _instance_name: &str) -> Result<Vec<String>, Error> {
        Ok(self
            .worker_scheduler
            .get_platform_property_manager()
            .get_known_properties()
            .keys()
            .cloned()
            .collect())
    }
}

#[async_trait]
impl WorkerScheduler for SimpleScheduler {
    fn get_platform_property_manager(&self) -> &PlatformPropertyManager {
        self.worker_scheduler.get_platform_property_manager()
    }

    async fn add_worker(&self, worker: Worker) -> Result<(), Error> {
        self.worker_scheduler.add_worker(worker).await
    }

    async fn update_action(
        &self,
        worker_id: &WorkerId,
        operation_id: &OperationId,
        update: UpdateOperationType,
    ) -> Result<(), Error> {
        self.worker_scheduler
            .update_action(worker_id, operation_id, update)
            .await
    }

    async fn worker_keep_alive_received(
        &self,
        worker_id: &WorkerId,
        timestamp: WorkerTimestamp,
    ) -> Result<(), Error> {
        self.worker_scheduler
            .worker_keep_alive_received(worker_id, timestamp)
            .await
    }

    async fn remove_worker(&self, worker_id: &WorkerId) -> Result<(), Error> {
        self.worker_scheduler.remove_worker(worker_id).await
    }

    async fn shutdown(&self, shutdown_guard: ShutdownGuard) {
        self.worker_scheduler.shutdown(shutdown_guard).await;
    }

    async fn remove_timedout_workers(&self, now_timestamp: WorkerTimestamp) -> Result<(), Error> {
        self.worker_scheduler
            .remove_timedout_workers(now_timestamp)
            .await
    }

    async fn set_drain_worker(&self, worker_id: &WorkerId, is_draining: bool) -> Result<(), Error> {
        self.worker_scheduler
            .set_drain_worker(worker_id, is_draining)
            .await
    }

    async fn update_worker_load(
        &self,
        worker_id: &WorkerId,
        cpu_load_pct: u32,
        p_core_load_pct: u32,
        e_core_load_pct: u32,
    ) -> Result<(), Error> {
        self.worker_scheduler
            .update_worker_load(worker_id, cpu_load_pct, p_core_load_pct, e_core_load_pct)
            .await
    }

    async fn update_cached_directories(
        &self,
        worker_id: &WorkerId,
        digests: HashSet<DigestInfo>,
    ) -> Result<(), Error> {
        self.worker_scheduler
            .update_cached_directories(worker_id, digests)
            .await
    }

    async fn update_cached_subtrees(
        &self,
        worker_id: &WorkerId,
        is_full_snapshot: bool,
        full_set: Vec<DigestInfo>,
        added: Vec<DigestInfo>,
        removed: Vec<DigestInfo>,
    ) -> Result<(), Error> {
        self.worker_scheduler
            .update_cached_subtrees(worker_id, is_full_snapshot, full_set, added, removed)
            .await
    }

    async fn broadcast_blobs_in_stable_storage(&self, digests: Vec<DigestInfo>) {
        self.worker_scheduler
            .broadcast_blobs_in_stable_storage(digests)
            .await;
    }
}

impl RootMetricsComponent for SimpleScheduler {}
