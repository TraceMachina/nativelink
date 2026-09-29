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

use core::hash::BuildHasher;
use core::pin::Pin;
use core::str;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use core::time::Duration;
use std::borrow::Cow;
use std::collections::HashMap;
use std::env;
use std::process::Stdio;
use std::sync::{Arc, Weak};
use std::time::{SystemTime, UNIX_EPOCH};

use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use futures::{Future, FutureExt, StreamExt, TryFutureExt, select};
use nativelink_config::cas_server::{EnvironmentSource, LocalWorkerConfig};
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_metric::{MetricsComponent, RootMetricsComponent};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::update_for_worker::Update;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::worker_api_client::WorkerApiClient;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    ActionResourceUsage, ExecuteAccepted, ExecuteComplete, ExecuteDeclined, ExecuteResult,
    GoingAwayRequest, KeepAliveRequest, StartExecute, UpdateForWorker, WorkerLoad,
    execute_declined, execute_result,
};
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_util::action_messages::{ActionResult, ActionStage, OperationId};
use nativelink_util::common::fs;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::metrics_utils::{AsyncCounterWrapper, CounterWithTime};
use nativelink_util::shutdown_guard::ShutdownGuard;
use nativelink_util::store_trait::Store;
use nativelink_util::{background_spawn, spawn, tls_utils};
use opentelemetry::context::Context;
use tokio::sync::{broadcast, mpsc};
use tokio::{process, time};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::Streaming;
use tracing::{Level, debug, error, event, info, info_span, instrument, trace, warn};

use crate::running_actions_manager::{
    ExecutionConfiguration, Metrics as RunningActionManagerMetrics, RunningAction,
    RunningActionsManager, RunningActionsManagerArgs, RunningActionsManagerImpl,
};
use crate::worker_api_client_wrapper::{WorkerApiClientTrait, WorkerApiClientWrapper};
use crate::worker_utils::make_connect_worker_request;

/// Amount of time to wait if we have actions in transit before we try to
/// consider an error to have occurred.
const ACTIONS_IN_TRANSIT_TIMEOUT_S: f32 = 10.;

/// Increments `actions_in_transit` on creation and decrements it on drop, so
/// the count stays accurate even when the owning action future is aborted by
/// a disconnect. A stranded count makes the disconnect handler conclude the
/// in-transit actions never drained, turning every disconnect-with-work into
/// a fatal error instead of a reconnect.
struct ActionsInTransitGuard {
    actions_in_transit: Arc<AtomicU64>,
}

impl ActionsInTransitGuard {
    fn new(actions_in_transit: Arc<AtomicU64>) -> Self {
        actions_in_transit.fetch_add(1, Ordering::Release);
        Self { actions_in_transit }
    }
}

impl Drop for ActionsInTransitGuard {
    fn drop(&mut self) {
        self.actions_in_transit.fetch_sub(1, Ordering::Release);
    }
}

/// If we lose connection to the worker api server we will wait this many seconds
/// before trying to connect, doubling on every failed attempt up to
/// `CONNECTION_RETRY_MAX_DELAY_S`, with jitter so a fleet that lost its
/// scheduler together does not redial together.
const CONNECTION_RETRY_DELAY_S: f32 = 0.5;
const CONNECTION_RETRY_MAX_DELAY_S: f32 = 30.0;

/// Delay before the `attempt`th consecutive reconnect (0 = first retry),
/// between half and one and a half times the exponential figure.
fn reconnect_delay(attempt: u32) -> Duration {
    let exponent = i32::try_from(attempt.min(16)).unwrap_or(16);
    let base = (CONNECTION_RETRY_DELAY_S * 2f32.powi(exponent)).min(CONNECTION_RETRY_MAX_DELAY_S);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos());
    let jitter = 0.5 + (nanos % 1_000) as f32 / 1_000.0;
    Duration::from_secs_f32(base * jitter)
}

/// Default endpoint timeout. If this value gets modified the documentation in
/// `cas_server.rs` must also be updated.
const DEFAULT_ENDPOINT_TIMEOUT_S: f32 = 5.;

/// Default maximum amount of time a task is allowed to run for.
/// If this value gets modified the documentation in `cas_server.rs` must also be updated.
const DEFAULT_MAX_ACTION_TIMEOUT: Duration = Duration::from_mins(20);
const DEFAULT_MAX_UPLOAD_TIMEOUT: Duration = Duration::from_mins(10);
const DEFAULT_MAX_CLEANUP_WAIT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_CLEANUP_BACKOFF: Duration = Duration::from_millis(500);
/// If this value gets modified the documentation in `cas_server.rs` must also be updated.
const DEFAULT_PRECONDITION_TIMEOUT: Duration = Duration::from_secs(30);

struct FinishedActionResult {
    action_result: ActionResult,
    resource_usage: Option<ActionResourceUsage>,
}

struct LocalWorkerImpl<'a, T: WorkerApiClientTrait + 'static, U: RunningActionsManager> {
    config: &'a LocalWorkerConfig,
    // According to the tonic documentation it is a cheap operation to clone this.
    grpc_client: T,
    worker_id: String,
    running_actions_manager: Arc<U>,
    // Number of actions that have been received in `Update::StartAction`, but
    // not yet processed by running_actions_manager's spawn. This number should
    // always be zero if there are no actions running and no actions being waited
    // on by the scheduler.
    actions_in_transit: Arc<AtomicU64>,
    accepted_action: AtomicBool,
    /// The scheduler said it understands `ExecuteAccepted` and
    /// `ExecuteDeclined`. Without it the worker runs whatever it is sent,
    /// as every earlier release did.
    dispatch_ack: bool,
    /// The platform property the scheduler reads an action's memory
    /// reservation from, as it told us on connection; empty when it does
    /// not veto on memory. Read from the same property, a refusal for load
    /// here agrees with what the scheduler would have vetoed.
    memory_property: String,
    metrics: Arc<Metrics>,
}

/// Why this worker will not run an action it was just sent.
enum Refusal {
    AtCapacity { in_flight: u64, max: u64 },
    Load { needed_kb: u64, free_kb: u64 },
    ShuttingDown,
}

impl Refusal {
    fn into_declined(self, operation_id: String) -> ExecuteDeclined {
        let (reason, detail, needed_kb, free_kb) = match self {
            Self::AtCapacity { in_flight, max } => (
                execute_declined::Reason::AtCapacity,
                format!("{in_flight} of {max} in flight"),
                0,
                0,
            ),
            Self::Load { needed_kb, free_kb } => (
                execute_declined::Reason::Load,
                format!("needs {needed_kb} KiB, {free_kb} KiB free"),
                needed_kb,
                free_kb,
            ),
            Self::ShuttingDown => (
                execute_declined::Reason::ShuttingDown,
                "worker shutting down".to_string(),
                0,
                0,
            ),
        };
        ExecuteDeclined {
            operation_id,
            reason: reason as i32,
            detail,
            needed_kb,
            free_kb,
        }
    }
}

/// The memory reservation an action carries under `property`, in KiB, if
/// it declares one. An empty property name is a scheduler that does not
/// veto on memory, so nothing is ever read.
fn memory_reservation_kb(start_execute: &StartExecute, property: &str) -> Option<u64> {
    if property.is_empty() {
        return None;
    }
    start_execute
        .platform
        .as_ref()?
        .properties
        .iter()
        .find(|p| p.name == property)
        .and_then(|p| p.value.parse::<u64>().ok())
        .filter(|kb| *kb > 0)
}

pub async fn preconditions_met<H: BuildHasher + Sync>(
    precondition_script: Option<String>,
    extra_envs: &HashMap<String, String, H>,
    timeout: Duration,
) -> Result<(), Error> {
    let Some(precondition_script) = &precondition_script else {
        // No script means we are always ok to proceed.
        return Ok(());
    };
    // TODO: Might want to pass some information about the command to the
    //       script, but at this point it's not even been downloaded yet,
    //       so that's not currently possible.  Perhaps we'll move this in
    //       future to pass useful information through?  Or perhaps we'll
    //       have a pre-condition and a pre-execute script instead, although
    //       arguably entrypoint already gives us that.

    let maybe_split_cmd = shlex::split(precondition_script);
    let (command, args) = match &maybe_split_cmd {
        Some(split_cmd) => (&split_cmd[0], &split_cmd[1..]),
        None => {
            return Err(make_input_err!(
                "Could not parse the value of precondition_script: '{}'",
                precondition_script,
            ));
        }
    };

    let precondition_process = process::Command::new(command)
        .args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_clear()
        .envs(extra_envs)
        .spawn()
        .err_tip(|| format!("Could not execute precondition command {precondition_script:?}"))?;
    // Bounded: a script that hangs held the action forever, and
    // `kill_on_drop` ends the script when the timeout drops it.
    let output = match time::timeout(timeout, precondition_process.wait_with_output()).await {
        Ok(output) => output?,
        Err(_) => {
            return Err(make_err!(
                Code::ResourceExhausted,
                "Preconditions script {precondition_script:?} did not finish within {} ms",
                timeout.as_millis()
            ));
        }
    };
    let stdout = str::from_utf8(&output.stdout).unwrap_or("");
    trace!(status = %output.status, %stdout, "Preconditions script returned");
    if output.status.code() == Some(0) {
        Ok(())
    } else {
        Err(make_err!(
            Code::ResourceExhausted,
            "Preconditions script returned status {} - {}",
            output.status,
            stdout
        ))
    }
}

impl<'a, T: WorkerApiClientTrait + 'static, U: RunningActionsManager> LocalWorkerImpl<'a, T, U> {
    fn new(
        config: &'a LocalWorkerConfig,
        grpc_client: T,
        worker_id: String,
        dispatch_ack: bool,
        memory_property: String,
        running_actions_manager: Arc<U>,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            config,
            grpc_client,
            worker_id,
            dispatch_ack,
            memory_property,
            running_actions_manager,
            // Number of actions that have been received in `Update::StartAction`, but
            // not yet processed by running_actions_manager's spawn. This number should
            // always be zero if there are no actions running and no actions being waited
            // on by the scheduler.
            actions_in_transit: Arc::new(AtomicU64::new(0)),
            accepted_action: AtomicBool::new(false),
            metrics,
        }
    }

    /// Local admission: the worker's own word on whether it can take this
    /// action now. The scheduler's ledger says what it believes the worker
    /// has; this is what the worker has.
    fn admission(&self, start_execute: &StartExecute, in_flight: u64) -> Option<Refusal> {
        let max = self.config.max_inflight_tasks;
        if max > 0 && in_flight >= max {
            return Some(Refusal::AtCapacity { in_flight, max });
        }
        if let (Some(needed_kb), Some(free_kb)) = (
            memory_reservation_kb(start_execute, &self.memory_property),
            crate::capacity::free_memory_kb(),
        ) && free_kb < needed_kb
        {
            return Some(Refusal::Load { needed_kb, free_kb });
        }
        None
    }

    /// Tells the scheduler the action is refused; only meaningful when it
    /// understands the message.
    async fn decline(&self, operation_id: String, refusal: Refusal) -> Result<(), Error> {
        self.metrics.actions_declined.inc();
        self.grpc_client
            .clone()
            .execute_declined(refusal.into_declined(operation_id))
            .await
            .err_tip(|| "Could not send ExecuteDeclined")
    }

    /// Starts a background spawn/thread that will send a message to the server every `timeout / 2`.
    async fn start_keep_alive(&self) -> Result<(), Error> {
        // According to tonic's documentation this call should be cheap and is the same stream.
        let mut grpc_client = self.grpc_client.clone();
        let timeout = self
            .config
            .worker_api_endpoint
            .timeout
            .unwrap_or(DEFAULT_ENDPOINT_TIMEOUT_S);

        info!(timeout, "Started KeepAlive");

        // We always send 2 keep alive requests per timeout. Http2 should manage most of our
        // timeout issues, this is a secondary check to ensure we can still send data.
        let mut interval = time::interval(Duration::from_secs_f32(timeout) / 2);
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

        // Skip the first interval as it happens immediately and we don't need a keep alive until timeout/2 has passed
        interval.tick().await;

        // Explicitly spawn the keep alive loop so it goes onto a different thread from the execute commands.
        // Its failure is this worker's failure: a worker whose keepalives
        // stop reaching the scheduler is evicted `worker_timeout_s` later
        // with every action it holds requeued, so ending the stream now and
        // reconnecting is the cheaper outcome. Before, the task died quietly
        // and the worker ran on without keepalives.
        spawn!("keep alives", async move {
            loop {
                interval.tick().await;
                // What the worker has to spare rides on the keepalive, so a
                // scheduler with `live_memory_veto` can skip a worker whose
                // actions declared less than they use.
                let load = crate::capacity::free_memory_kb()
                    .map(|free_memory_kb| WorkerLoad { free_memory_kb });
                if let Err(e) = grpc_client.keep_alive(KeepAliveRequest { load }).await {
                    error!(?e, "Failed to send KeepAlive in LocalWorker");
                    return Err(e.append("KeepAlive failed; reconnecting to the scheduler"));
                }
                debug!("Sent KeepAlive");
            }
        })
        .await
        .map_err(|e| make_err!(Code::Internal, "KeepAlive task ended: {e:?}"))?
    }

    async fn run(
        &self,
        update_for_worker_stream: Streaming<UpdateForWorker>,
        shutdown_rx: &mut broadcast::Receiver<ShutdownGuard>,
    ) -> Result<(), Error> {
        // This big block of logic is designed to help simplify upstream components. Upstream
        // components can write standard futures that return a `Result<(), Error>` and this block
        // will forward the error up to the client and disconnect from the scheduler.
        // It is a common use case that an item sent through update_for_worker_stream will always
        // have a response but the response will be triggered through a callback to the scheduler.
        // This can be quite tricky to manage, so what we have done here is given access to a
        // `futures` variable which because this is in a single thread as well as a channel that you
        // send a future into that makes it into the `futures` variable.
        // This means that if you want to perform an action based on the result of the future
        // you use the `.map()` method and the new action will always come to live in this spawn,
        // giving mutable access to stuff in this struct.
        // NOTE: If you ever return from this function it will disconnect from the scheduler.
        let mut futures = FuturesUnordered::new();
        futures.push(self.start_keep_alive().boxed());

        let (add_future_channel, add_future_rx) = mpsc::unbounded_channel();
        let mut add_future_rx = UnboundedReceiverStream::new(add_future_rx).fuse();

        let mut update_for_worker_stream = update_for_worker_stream.fuse();
        // A notify which is triggered every time actions_in_flight is subtracted.
        let actions_notify = Arc::new(tokio::sync::Notify::new());
        // A counter of actions that are in-flight, this is similar to actions_in_transit but
        // includes the AC upload and notification to the scheduler.
        let actions_in_flight = Arc::new(AtomicU64::new(0));
        // Set to true when shutting down, this stops any new StartAction.
        let mut shutting_down = false;

        loop {
            if self.config.single_use
                && self.accepted_action.load(Ordering::Acquire)
                && actions_in_flight.load(Ordering::Acquire) == 0
            {
                // The action, CAS/AC uploads, cleanup, and execution_response
                // acknowledgment have all completed. This container is spent.
                if let Err(err) = self
                    .grpc_client
                    .clone()
                    .going_away(GoingAwayRequest { drain: false })
                    .await
                {
                    warn!(?err, "Could not unregister completed single-use worker");
                }
                return Ok(());
            }
            select! {
                maybe_update = update_for_worker_stream.next() => if !shutting_down || maybe_update.is_some() {
                    match maybe_update
                        .err_tip(|| "UpdateForWorker stream closed early")?
                        .err_tip(|| "Got error in UpdateForWorker stream")?
                        .update
                        .err_tip(|| "Expected update to exist in UpdateForWorker")?
                    {
                        Update::ConnectionResult(_) => {
                            return Err(make_input_err!(
                                "Got ConnectionResult in LocalWorker::run which should never happen"
                            ));
                        }
                        // TODO(palfrey) We should possibly do something with this notification.
                        Update::Disconnect(()) => {
                            self.metrics.disconnects_received.inc();
                        }
                        Update::KeepAlive(()) => {
                            self.metrics.keep_alives_received.inc();
                        }
                        Update::KillOperationRequest(kill_operation_request) => {
                            let operation_id = OperationId::from(kill_operation_request.operation_id);
                            if let Err(err) = self.running_actions_manager.kill_operation(&operation_id).await {
                                error!(
                                    %operation_id,
                                    ?err,
                                    "Failed to send kill request for operation"
                                );
                            }
                        }
                        Update::StartAction(start_execute) => {
                            // Don't accept any new requests if we're shutting down.
                            if shutting_down || (self.config.single_use
                                && self.accepted_action.load(Ordering::Acquire)) {
                                if self.dispatch_ack {
                                    self.decline(start_execute.operation_id, Refusal::ShuttingDown).await?;
                                } else if let Some(instance_name) = start_execute.execute_request.map(|request| request.instance_name) {
                                    self.grpc_client.clone().execution_response(
                                        ExecuteResult{
                                            instance_name,
                                            operation_id: start_execute.operation_id,
                                            result: Some(execute_result::Result::InternalError(make_err!(Code::ResourceExhausted, "Worker shutting down").into())),
                                            resource_usage: None,
                                        }
                                    ).await?;
                                }
                                continue;
                            }

                            // Admission, then the acknowledgement: what the
                            // scheduler charged on the send is confirmed or
                            // handed back before anything runs. A scheduler
                            // that does not speak the acknowledgement gets
                            // the old behaviour, run whatever arrives.
                            if self.dispatch_ack {
                                if let Some(refusal) = self.admission(
                                    &start_execute,
                                    actions_in_flight.load(Ordering::Acquire),
                                ) {
                                    self.decline(start_execute.operation_id, refusal).await?;
                                    continue;
                                }
                                self.grpc_client
                                    .clone()
                                    .execute_accepted(ExecuteAccepted {
                                        operation_id: start_execute.operation_id.clone(),
                                    })
                                    .await
                                    .err_tip(|| "Could not send ExecuteAccepted")?;
                            }
                            // Admitted: a single-use worker is spent from
                            // here. A decline above must not spend it, or
                            // every dispatch it turns away costs a pod.
                            if self.config.single_use {
                                self.accepted_action.store(true, Ordering::Release);
                            }

                            self.metrics.start_actions_received.inc();

                            let execute_request = start_execute.execute_request.as_ref();
                            let operation_id = start_execute.operation_id.clone();
                            let operation_id_to_log = operation_id.clone();
                            let maybe_instance_name = execute_request.map(|v| v.instance_name.clone());
                            let action_digest = execute_request.and_then(|v| v.action_digest.clone());
                            let digest_hasher = execute_request
                                .ok_or_else(|| make_input_err!("Expected execute_request to be set"))
                                .and_then(|v| DigestHasherFunc::try_from(v.digest_function))
                                .err_tip(|| "In LocalWorkerImpl::new()")?;

                            let start_action_fut = {
                                let precondition_script_cfg = self.config.experimental_precondition_script.clone();
                                let precondition_timeout = if self.config.precondition_timeout_ms == 0 {
                                    DEFAULT_PRECONDITION_TIMEOUT
                                } else {
                                    Duration::from_millis(self.config.precondition_timeout_ms)
                                };
                                let mut extra_envs: HashMap<String, String> = HashMap::new();
                                if let Some(ref additional_environment) = self.config.additional_environment {
                                    for (name, source) in additional_environment {
                                        let value = match source {
                                            EnvironmentSource::Property(property) => start_execute
                                                .platform.as_ref().and_then(|p|p.properties.iter().find(|pr| &pr.name == property))
                                                .map_or_else(|| Cow::Borrowed(""), |v| Cow::Borrowed(v.value.as_str())),
                                            EnvironmentSource::Value(value) => Cow::Borrowed(value.as_str()),
                                            EnvironmentSource::FromEnvironment => Cow::Owned(env::var(name).unwrap_or_default()),
                                            other => {
                                                debug!(?other, "Worker doesn't support this type of additional environment");
                                                continue;
                                            }
                                        };
                                        extra_envs.insert(name.clone(), value.into_owned());
                                    }
                                }
                                let actions_in_transit_guard =
                                    ActionsInTransitGuard::new(self.actions_in_transit.clone());
                                let worker_id = self.worker_id.clone();
                                let running_actions_manager = self.running_actions_manager.clone();
                                let mut grpc_client = self.grpc_client.clone();
                                let complete = ExecuteComplete {
                                    operation_id: operation_id.clone(),
                                };
                                let single_use = self.config.single_use;
                                self.metrics.clone().wrap(move |metrics| async move {
                                    metrics.preconditions.wrap(preconditions_met(precondition_script_cfg, &extra_envs, precondition_timeout))
                                    .and_then(|()| running_actions_manager.create_and_add_action(worker_id, start_execute))
                                    .map(move |r| {
                                        // Now that we either failed or registered our action, we can
                                        // consider the action to no longer be in transit.
                                        drop(actions_in_transit_guard);
                                        r
                                    })
                                    .and_then(|action| {
                                        debug!(
                                            operation_id = %action.get_operation_id(),
                                            "Received request to run action"
                                        );
                                        action
                                            .clone()
                                            .prepare_action()
                                            .and_then(RunningAction::execute)
                                            .and_then(|result| async move {
                                                // Reusable workers release their slot during upload.
                                                // A single-use worker must never advertise another slot.
                                                if !single_use {
                                                    drop(grpc_client.execution_complete(complete).await);
                                                }
                                                Ok(result)
                                            })
                                            .and_then(RunningAction::upload_results)
                                            .and_then(|action| async move {
                                                let resource_usage = action.resource_usage();
                                                let action_result = action.get_finished_result().await?;
                                                Ok(FinishedActionResult {
                                                    action_result,
                                                    resource_usage,
                                                })
                                            })
                                            // Note: We need ensure we run cleanup even if one of the other steps fail.
                                            .then(|result| async move {
                                                if let Err(e) = action.cleanup().await {
                                                    return Result::<FinishedActionResult, Error>::Err(e).merge(result);
                                                }
                                                result
                                            })
                                    }).await
                                })
                            };

                            let make_publish_future = {
                                let mut grpc_client = self.grpc_client.clone();

                                let running_actions_manager = self.running_actions_manager.clone();
                                let worker_id = self.worker_id.clone();
                                move |res: Result<FinishedActionResult, Error>| async move {
                                    let instance_name = maybe_instance_name
                                        .err_tip(|| "`instance_name` could not be resolved; this is likely an internal error in local_worker.")?;
                                    match res {
                                        Ok(FinishedActionResult { mut action_result, resource_usage }) => {
                                            // Save in the action cache before notifying the scheduler that we've completed.
                                            if let Some(digest_info) = action_digest.clone().and_then(|action_digest| action_digest.try_into().ok()) &&
                                                let Err(err) = running_actions_manager.cache_action_result(digest_info, &mut action_result, digest_hasher).await {
                                                    error!(
                                                        ?err,
                                                        ?action_digest,
                                                        "Error saving action in store",
                                                    );
                                                }
                                            let action_stage = ActionStage::Completed(action_result);
                                            let resource_usage = resource_usage.map(|mut resource_usage| {
                                                resource_usage.operation_id.clone_from(&operation_id);
                                                resource_usage.worker_id.clone_from(&worker_id);
                                                resource_usage
                                            });
                                            grpc_client.execution_response(
                                                ExecuteResult{
                                                    instance_name,
                                                    operation_id,
                                                    result: Some(execute_result::Result::ExecuteResponse(action_stage.into())),
                                                    resource_usage,
                                                }
                                            )
                                            .await
                                            .err_tip(|| "Error while calling execution_response")?;
                                        },
                                        Err(e) => {
                                            let is_cas_blob_missing = e.code == Code::NotFound
                                                && e.message_string().contains("not found in either fast or slow store");
                                            if is_cas_blob_missing {
                                                warn!(
                                                    ?e,
                                                    "Missing CAS inputs during prepare_action, returning FAILED_PRECONDITION"
                                                );
                                                let action_result = ActionResult {
                                                    error: Some(make_err!(
                                                        Code::FailedPrecondition,
                                                        "{}",
                                                        e.message_string()
                                                    )),
                                                    ..ActionResult::default()
                                                };
                                                let action_stage = ActionStage::Completed(action_result);
                                                grpc_client.execution_response(ExecuteResult{
                                                    instance_name,
                                                    operation_id,
                                                    result: Some(execute_result::Result::ExecuteResponse(action_stage.into())),
                                                    resource_usage: None,
                                                }).await.err_tip(|| "Error calling execution_response with missing inputs")?;
                                            } else {
                                                grpc_client.execution_response(ExecuteResult{
                                                    instance_name,
                                                    operation_id,
                                                    result: Some(execute_result::Result::InternalError(e.into())),
                                                    resource_usage: None,
                                                }).await.err_tip(|| "Error calling execution_response with error")?;
                                            }
                                        },
                                    }
                                    Ok(())
                                }
                            };

                            let add_future_channel = add_future_channel.clone();

                            info_span!(
                                "worker_start_action_ctx",
                                operation_id = operation_id_to_log,
                                digest_function = %digest_hasher.to_string(),
                            ).in_scope(|| {
                                let _guard = Context::current_with_value(digest_hasher)
                                    .attach();

                                let actions_in_flight = actions_in_flight.clone();
                                let actions_notify = actions_notify.clone();
                                let actions_in_flight_fail = actions_in_flight.clone();
                                let actions_notify_fail = actions_notify.clone();
                                actions_in_flight.fetch_add(1, Ordering::Release);

                                futures.push(
                                    spawn!("worker_start_action", start_action_fut).map(move |res| {
                                        let res = res.err_tip(|| "Failed to launch spawn")?;
                                        if let Err(err) = &res {
                                            error!(?err, "Error executing action");
                                        }
                                        add_future_channel
                                            .send(make_publish_future(res).then(move |res| {
                                                actions_in_flight.fetch_sub(1, Ordering::Release);
                                                actions_notify.notify_one();
                                                core::future::ready(res)
                                            }).boxed())
                                            .map_err(|err|
                                                Error::from_std_err(Code::Internal, &err).append("LocalWorker could not send future")
                                                )?;
                                        Ok(())
                                    })
                                    .or_else(move |err| {
                                        // If the make_publish_future is not run we still need to notify.
                                        actions_in_flight_fail.fetch_sub(1, Ordering::Release);
                                        actions_notify_fail.notify_one();
                                        core::future::ready(Err(err))
                                    })
                                    .boxed()
                                );
                            });
                        }
                    }
                },
                res = add_future_rx.next() => {
                    let fut = res.err_tip(|| "New future stream receives should never be closed")?;
                    futures.push(fut);
                },
                res = futures.next() => res.err_tip(|| "Keep-alive should always pending. Likely unable to send data to scheduler")??,
                complete_msg = shutdown_rx.recv().fuse() => {
                    warn!("Worker loop received shutdown signal. Shutting down worker...",);
                    let mut grpc_client = self.grpc_client.clone();
                    let shutdown_guard = complete_msg.map_err(|e|
                        Error::from_std_err(Code::Internal, &e).append("Failed to receive shutdown message"))?;
                    let actions_in_flight = actions_in_flight.clone();
                    let actions_notify = actions_notify.clone();
                    let drain_on_shutdown = self.config.drain_on_shutdown;
                    let drain_deadline = if self.config.max_action_timeout_s == 0 {
                        DEFAULT_MAX_ACTION_TIMEOUT
                    } else {
                        Duration::from_secs(self.config.max_action_timeout_s as u64)
                    };
                    let shutdown_future = async move {
                        if drain_on_shutdown {
                            // Say so first, so nothing new is dispatched here
                            // while the running actions finish; the scheduler
                            // removes this worker when the stream closes.
                            if let Err(e) = grpc_client.going_away(GoingAwayRequest { drain: true }).await {
                                error!("Failed to send GoingAwayRequest: {e}",);
                                return Err(e);
                            }
                        }
                        // Wait for in-flight operations to be fully completed,
                        // for as long as one action is allowed to run.
                        let wait = async {
                            while actions_in_flight.load(Ordering::Acquire) > 0 {
                                actions_notify.notified().await;
                            }
                        };
                        if time::timeout(drain_deadline, wait).await.is_err() {
                            error!(
                                actions_in_flight = actions_in_flight.load(Ordering::Acquire),
                                "Drain deadline passed with actions still in flight; shutting down anyway"
                            );
                        }
                        if !drain_on_shutdown {
                            // Sending this message immediately evicts all jobs from
                            // this worker, of which there should be none.
                            if let Err(e) = grpc_client.going_away(GoingAwayRequest { drain: false }).await {
                                error!("Failed to send GoingAwayRequest: {e}",);
                                return Err(e);
                            }
                        }
                        // Allow shutdown to occur now.
                        drop(shutdown_guard);
                        Ok::<(), Error>(())
                    };
                    futures.push(shutdown_future.boxed());
                    shutting_down = true;
                },
            };
        }
        // Unreachable.
    }
}

type ConnectionFactory<T> = Box<dyn Fn() -> BoxFuture<'static, Result<T, Error>> + Send + Sync>;

pub struct LocalWorker<T: WorkerApiClientTrait + 'static, U: RunningActionsManager> {
    config: Arc<LocalWorkerConfig>,
    running_actions_manager: Arc<U>,
    connection_factory: ConnectionFactory<T>,
    sleep_fn: Option<Box<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>>,
    metrics: Arc<Metrics>,
}

impl<
    T: WorkerApiClientTrait + core::fmt::Debug + 'static,
    U: RunningActionsManager + core::fmt::Debug,
> core::fmt::Debug for LocalWorker<T, U>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LocalWorker")
            .field("config", &self.config)
            .field("running_actions_manager", &self.running_actions_manager)
            .field("metrics", &self.metrics)
            .finish_non_exhaustive()
    }
}

/// Creates a new `LocalWorker`. The `cas_store` must be an instance of
/// `FastSlowStore` and will be checked at runtime.
pub async fn new_local_worker(
    config: Arc<LocalWorkerConfig>,
    cas_store: Store,
    ac_store: Option<Store>,
    historical_store: Store,
) -> Result<LocalWorker<WorkerApiClientWrapper, RunningActionsManagerImpl>, Error> {
    #[cfg(not(target_os = "linux"))]
    if config.experimental_buck2_file_capture.is_some() {
        return Err(make_input_err!(
            "Buck2 container file capture requires a Linux execution container"
        ));
    }
    let fast_slow_store = cas_store
        .downcast_ref::<FastSlowStore>(None)
        .err_tip(|| "Expected store for LocalWorker's store to be a FastSlowStore")?
        .get_arc()
        .err_tip(|| "FastSlowStore's Arc doesn't exist")?;

    // Log warning about CAS configuration for multi-worker setups
    event!(
        Level::INFO,
        worker_name = %config.name,
        "Starting worker '{}'. IMPORTANT: If running multiple workers, all workers \
        must share the same CAS storage path to avoid 'Object not found' errors.",
        config.name
    );

    if let Ok(path) = fs::canonicalize(&config.work_directory).await {
        fs::remove_dir_all(&path).await.err_tip(|| {
            format!(
                "Could not remove work_directory '{}' in LocalWorker",
                path.as_path().to_str().unwrap_or("bad path")
            )
        })?;
    }

    fs::create_dir_all(&config.work_directory)
        .await
        .err_tip(|| format!("Could not make work_directory : {}", config.work_directory))?;
    let entrypoint = if config.entrypoint.is_empty() {
        None
    } else {
        Some(config.entrypoint.clone())
    };
    let max_action_timeout = if config.max_action_timeout_s == 0 {
        DEFAULT_MAX_ACTION_TIMEOUT
    } else {
        Duration::from_secs(config.max_action_timeout_s as u64)
    };
    let max_upload_timeout = if config.max_upload_timeout_s == 0 {
        DEFAULT_MAX_UPLOAD_TIMEOUT
    } else {
        Duration::from_secs(config.max_upload_timeout_s as u64)
    };
    let max_cleanup_wait = if config.max_cleanup_wait_s == 0 {
        DEFAULT_MAX_CLEANUP_WAIT
    } else {
        Duration::from_secs(config.max_cleanup_wait_s as u64)
    };
    let max_cleanup_backoff = if config.max_cleanup_backoff_ms == 0 {
        DEFAULT_MAX_CLEANUP_BACKOFF
    } else {
        Duration::from_millis(config.max_cleanup_backoff_ms as u64)
    };

    // Initialize directory cache if configured
    let directory_cache = if let Some(cache_config) = &config.directory_cache {
        use std::path::PathBuf;

        use crate::directory_cache::{
            DirectoryCache, DirectoryCacheConfig as WorkerDirCacheConfig,
        };

        let cache_root = if cache_config.cache_root.is_empty() {
            PathBuf::from(&config.work_directory).parent().map_or_else(
                || PathBuf::from("/tmp/nativelink_directory_cache"),
                |p| p.join("directory_cache"),
            )
        } else {
            PathBuf::from(&cache_config.cache_root)
        };

        let worker_cache_config = WorkerDirCacheConfig {
            max_entries: cache_config.max_entries,
            max_size_bytes: cache_config.max_size_bytes,
            cache_root,
            experimental_subtree_caching: cache_config.experimental_subtree_caching,
            max_concurrent_fetches: cache_config.max_concurrent_fetches,
            experimental_get_tree_prefetch: cache_config.experimental_get_tree_prefetch,
        };

        match DirectoryCache::new(worker_cache_config, fast_slow_store.clone()).await {
            Ok(cache) => {
                tracing::info!("Directory cache initialized successfully");
                Some(Arc::new(cache))
            }
            Err(e) => {
                tracing::warn!("Failed to initialize directory cache: {:?}", e);
                None
            }
        }
    } else {
        None
    };

    #[cfg(target_os = "linux")]
    let use_namespaces = if let Some(use_namespaces) = &config.use_namespaces {
        if *use_namespaces
            && !crate::namespace_utils::namespaces_supported(
                config.use_mount_namespace.unwrap_or_default(),
            )
        {
            return Err(make_err!(Code::Unavailable, "Namespaces not supported"));
        }
        if !*use_namespaces {
            crate::running_actions_manager::UseNamespaces::No
        } else if config.use_mount_namespace.unwrap_or_default() {
            crate::running_actions_manager::UseNamespaces::YesAndMount
        } else {
            crate::running_actions_manager::UseNamespaces::Yes
        }
    } else if config
        .use_mount_namespace
        .is_some_and(core::convert::identity)
    {
        return Err(make_err!(
            Code::Unavailable,
            "Mount namespaces not supported"
        ));
    } else {
        crate::running_actions_manager::UseNamespaces::No
    };

    #[cfg(not(target_os = "linux"))]
    if config.use_namespaces.is_some_and(core::convert::identity) {
        return Err(make_err!(
            Code::Unavailable,
            "Namespaces not supported on non-Linux OSes"
        ));
    }
    #[cfg(not(target_os = "linux"))]
    if config
        .use_mount_namespace
        .is_some_and(core::convert::identity)
    {
        return Err(make_err!(
            Code::Unavailable,
            "Mount namespaces not supported on non-Linux OSes"
        ));
    }

    #[cfg(not(target_os = "linux"))]
    if !config.experimental_readonly_input_mounts.is_empty() {
        return Err(make_err!(
            Code::Unavailable,
            "Read-only input mounts not supported on non-Linux OSes"
        ));
    }

    let running_actions_manager =
        Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
            root_action_directory: config.work_directory.clone(),
            execution_configuration: ExecutionConfiguration {
                max_captured_output_bytes: config.max_captured_output_bytes,
                buck2_file_capture: config.experimental_buck2_file_capture.clone(),
                entrypoint,
                additional_environment: config.additional_environment.clone(),
            },
            cas_store: fast_slow_store,
            ac_store,
            historical_store,
            upload_action_result_config: &config.upload_action_result,
            max_action_timeout,
            max_upload_timeout,
            max_cleanup_wait,
            max_cleanup_backoff,
            timeout_handled_externally: config.timeout_handled_externally,
            directory_cache,
            active_input_leases: config.experimental_active_input_leases,
            #[cfg(target_os = "linux")]
            readonly_input_mounts: config.experimental_readonly_input_mounts.clone(),
            #[cfg(target_os = "linux")]
            use_namespaces,
        })?);
    if config.orphan_sweep_interval_s > 0 {
        let interval = Duration::from_secs(config.orphan_sweep_interval_s);
        let manager = running_actions_manager.clone();
        // Detached on purpose: the guarded spawn aborts its task when the
        // handle drops, and this one runs for the worker's whole life.
        drop(background_spawn!("orphan_sweep", async move {
            info!(interval_s = interval.as_secs(), "Orphan sweep scheduled");
            loop {
                time::sleep(interval).await;
                match manager.sweep_orphaned_directories().await {
                    Ok(0) => debug!("Orphan sweep found nothing"),
                    Ok(removed) => info!(removed, "Orphan sweep finished"),
                    Err(err) => warn!(?err, "Orphan sweep failed"),
                }
            }
        }));
    }
    let local_worker = LocalWorker::new_with_connection_factory_and_actions_manager(
        config.clone(),
        running_actions_manager,
        Box::new(move || {
            let config = config.clone();
            Box::pin(async move {
                let timeout = config
                    .worker_api_endpoint
                    .timeout
                    .unwrap_or(DEFAULT_ENDPOINT_TIMEOUT_S);
                let timeout_duration = Duration::from_secs_f32(timeout);
                let tls_config =
                    tls_utils::load_client_config(&config.worker_api_endpoint.tls_config)
                        .err_tip(|| "Parsing local worker TLS configuration")?;
                let endpoint =
                    tls_utils::endpoint_from(&config.worker_api_endpoint.uri, tls_config)
                        .map_err(|e| {
                            Error::from_std_err(Code::InvalidArgument, &e)
                                .append("Invalid URI for worker endpoint")
                        })?
                        .connect_timeout(timeout_duration)
                        .timeout(timeout_duration);

                let transport = endpoint.connect().await.map_err(|e| {
                    Error::from_std_err(Code::Internal, &e).append(format!(
                        "Could not connect to endpoint {}",
                        config.worker_api_endpoint.uri
                    ))
                })?;
                Ok(WorkerApiClient::new(transport).into())
            })
        }),
        Box::new(move |d| Box::pin(time::sleep(d))),
    );
    Ok(local_worker)
}

impl<T: WorkerApiClientTrait + 'static, U: RunningActionsManager> LocalWorker<T, U> {
    pub fn new_with_connection_factory_and_actions_manager(
        config: Arc<LocalWorkerConfig>,
        running_actions_manager: Arc<U>,
        connection_factory: ConnectionFactory<T>,
        sleep_fn: Box<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>,
    ) -> Self {
        let metrics = Arc::new(Metrics::new(Arc::downgrade(
            running_actions_manager.metrics(),
        )));
        Self {
            config,
            running_actions_manager,
            connection_factory,
            sleep_fn: Some(sleep_fn),
            metrics,
        }
    }

    #[allow(
        clippy::missing_const_for_fn,
        reason = "False positive on stable, but not on nightly"
    )]
    pub fn name(&self) -> &String {
        &self.config.name
    }

    async fn register_worker(
        &self,
        client: &mut T,
    ) -> Result<(String, bool, String, Streaming<UpdateForWorker>), Error> {
        let mut extra_envs: HashMap<String, String> = HashMap::new();
        if let Some(ref additional_environment) = self.config.additional_environment {
            for (name, source) in additional_environment {
                let value = match source {
                    EnvironmentSource::Value(value) => Cow::Borrowed(value.as_str()),
                    EnvironmentSource::FromEnvironment => {
                        Cow::Owned(env::var(name).unwrap_or_default())
                    }
                    other => {
                        debug!(
                            ?other,
                            "Worker registration doesn't support this type of additional environment"
                        );
                        continue;
                    }
                };
                extra_envs.insert(name.clone(), value.into_owned());
            }
        }

        let connect_worker_request = make_connect_worker_request(
            self.config.name.clone(),
            &self.config.platform_properties,
            &extra_envs,
            if self.config.single_use {
                1
            } else {
                self.config.max_inflight_tasks
            },
        )
        .await?;
        let mut update_for_worker_stream = client
            .connect_worker(connect_worker_request)
            .await
            .err_tip(|| "Could not call connect_worker() in worker")?
            .into_inner();

        let first_msg_update = update_for_worker_stream
            .next()
            .await
            .err_tip(|| "Got EOF expected UpdateForWorker")?
            .err_tip(|| "Got error when receiving UpdateForWorker")?
            .update;

        let (worker_id, dispatch_ack, memory_property) = match first_msg_update {
            Some(Update::ConnectionResult(connection_result)) => (
                connection_result.worker_id,
                connection_result.dispatch_ack,
                connection_result.memory_property,
            ),
            other => {
                return Err(make_input_err!(
                    "Expected first response from scheduler to be a ConnectionResult got : {:?}",
                    other
                ));
            }
        };
        Ok((
            worker_id,
            dispatch_ack,
            memory_property,
            update_for_worker_stream,
        ))
    }

    #[instrument(skip(self), level = Level::INFO)]
    pub async fn run(
        mut self,
        mut shutdown_rx: broadcast::Receiver<ShutdownGuard>,
    ) -> Result<(), Error> {
        // Belt-and-suspenders QoS bump: the main binary already calls
        // this before runtime creation so the tokio worker threads
        // inherit P-core preference via pthread QoS inheritance, but
        // any thread that reaches this point should also be tagged in
        // case it was spawned by a path that bypassed `on_thread_start`.
        // No-op on non-macOS.
        let _ = crate::qos::set_user_initiated();

        let sleep_fn = self
            .sleep_fn
            .take()
            .err_tip(|| "Could not unwrap sleep_fn in LocalWorker::run")?;
        let sleep_fn_pin = Pin::new(&sleep_fn);
        let attempts = AtomicU32::new(0);
        let attempts_ref = &attempts;
        let error_handler = Box::pin(move |err| async move {
            let attempt = attempts_ref.fetch_add(1, Ordering::AcqRel);
            let delay = reconnect_delay(attempt);
            error!(?err, attempt, delay_ms = delay.as_millis(), "Error");
            (sleep_fn_pin)(delay).await;
        });

        loop {
            // First connect to our endpoint.
            let mut client = match (self.connection_factory)().await {
                Ok(client) => client,
                Err(e) => {
                    (error_handler)(e).await;
                    continue; // Try to connect again.
                }
            };

            debug!("Connected to endpoint");

            // Next register our worker with the scheduler.
            let (inner, update_for_worker_stream) = match self.register_worker(&mut client).await {
                Err(e) => {
                    (error_handler)(e).await;
                    continue; // Try to connect again.
                }
                Ok((worker_id, dispatch_ack, memory_property, update_for_worker_stream)) => (
                    LocalWorkerImpl::new(
                        &self.config,
                        client,
                        worker_id,
                        dispatch_ack,
                        memory_property,
                        self.running_actions_manager.clone(),
                        self.metrics.clone(),
                    ),
                    update_for_worker_stream,
                ),
            };
            info!(
                worker_id = %inner.worker_id,
                "Worker registered with scheduler"
            );
            attempts.store(0, Ordering::Release);

            // Now listen for connections and run all other services.
            if let Err(err) = inner.run(update_for_worker_stream, &mut shutdown_rx).await {
                // Give in-transit actions a chance to settle before we kill
                // them, so their results still reach the scheduler.
                const ITERATIONS: usize = 1_000;

                let sleep_duration = ACTIONS_IN_TRANSIT_TIMEOUT_S / ITERATIONS as f32;
                let mut drained = false;
                for _ in 0..ITERATIONS {
                    if inner.actions_in_transit.load(Ordering::Acquire) == 0 {
                        drained = true;
                        break;
                    }
                    (sleep_fn_pin)(Duration::from_secs_f32(sleep_duration)).await;
                }
                if !drained {
                    // Deliberately not fatal. Returning here propagates out of
                    // the worker's main loop and aborts the process, so a
                    // scheduler blip that happened to catch an action in
                    // transit took the whole worker down — and every action it
                    // held then had to run again elsewhere. At fleet scale that
                    // is a restart storm. kill_all() below discards these
                    // actions anyway, so the wait is a courtesy and overrunning
                    // it costs nothing beyond the actions we were already
                    // giving up on.
                    error!(
                        actions_in_transit = inner.actions_in_transit.load(Ordering::Acquire),
                        "Actions in transit did not reach zero before we disconnected from the scheduler"
                    );
                }

                // Kill off any existing actions because if we re-connect, we'll
                // get some more and it might resource lock us.
                self.running_actions_manager.kill_all().await;

                if self.config.single_use && inner.accepted_action.load(Ordering::Acquire) {
                    return Err(
                        err.append("Single-use worker disconnected after accepting its action")
                    );
                }
                error!(?err, "Worker disconnected from scheduler, reconnecting");
                (error_handler)(err).await; // Try to connect again.
            } else if self.config.single_use {
                return Ok(());
            }
        }
        // Unreachable.
    }
}

#[derive(Debug, MetricsComponent)]
pub struct Metrics {
    #[metric(
        help = "Total number of actions sent to this worker to process. This does not mean it started them, it just means it received a request to execute it."
    )]
    start_actions_received: CounterWithTime,
    #[metric(help = "Total number of disconnects received from the scheduler.")]
    disconnects_received: CounterWithTime,
    #[metric(
        help = "Dispatches this worker declined: at capacity, short of memory, or shutting down."
    )]
    actions_declined: CounterWithTime,
    #[metric(help = "Total number of keep-alives received from the scheduler.")]
    keep_alives_received: CounterWithTime,
    #[metric(
        help = "Stats about the calls to check if an action satisfies the config supplied script."
    )]
    preconditions: AsyncCounterWrapper,
    #[metric]
    #[allow(
        clippy::struct_field_names,
        reason = "TODO Fix this. Triggers on nightly"
    )]
    running_actions_manager_metrics: Weak<RunningActionManagerMetrics>,
}

impl RootMetricsComponent for Metrics {}

impl Metrics {
    fn new(running_actions_manager_metrics: Weak<RunningActionManagerMetrics>) -> Self {
        Self {
            start_actions_received: CounterWithTime::default(),
            disconnects_received: CounterWithTime::default(),
            actions_declined: CounterWithTime::default(),
            keep_alives_received: CounterWithTime::default(),
            preconditions: AsyncCounterWrapper::default(),
            running_actions_manager_metrics,
        }
    }
}

impl Metrics {
    async fn wrap<U, T: Future<Output = U>, F: FnOnce(Arc<Self>) -> T>(
        self: Arc<Self>,
        fut: F,
    ) -> U {
        fut(self).await
    }
}
