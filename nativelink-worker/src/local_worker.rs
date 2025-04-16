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

use core::pin::Pin;
use core::str;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::process::Stdio;
use std::sync::Arc;

use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, StreamExt, TryFutureExt, select};
use nativelink_config::cas_server::LocalWorkerConfig;
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::update_for_worker::Update;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::worker_api_client::WorkerApiClient;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    ExecuteResult, GoingAwayRequest, KeepAliveRequest, UpdateForWorker, execute_result,
};
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_util::action_messages::{ActionResult, ActionStage, OperationId};
use nativelink_util::common::fs;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::shutdown_guard::ShutdownGuard;
use nativelink_util::store_trait::Store;
use nativelink_util::{spawn, tls_utils};
use opentelemetry::context::Context;
use opentelemetry::{InstrumentationScope, KeyValue, global, metrics};
use tokio::process;
use tokio::sync::{broadcast, mpsc};
use tokio::time::sleep;
use tokio_stream::wrappers::UnboundedReceiverStream;
use tonic::Streaming;
use tracing::{debug, error, info, info_span, instrument, warn};

use crate::running_actions_manager::{
    ExecutionConfiguration, RunningAction, RunningActionsManager, RunningActionsManagerArgs,
    RunningActionsManagerImpl,
};
use crate::worker_api_client_wrapper::{WorkerApiClientTrait, WorkerApiClientWrapper};
use crate::worker_utils::make_connect_worker_request;

/// Amount of time to wait if we have actions in transit before we try to
/// consider an error to have occurred.
const ACTIONS_IN_TRANSIT_TIMEOUT_S: f32 = 10.;

/// If we lose connection to the worker api server we will wait this many seconds
/// before trying to connect.
const CONNECTION_RETRY_DELAY_S: f32 = 0.5;

/// Default endpoint timeout. If this value gets modified the documentation in
/// `cas_server.rs` must also be updated.
const DEFAULT_ENDPOINT_TIMEOUT_S: f32 = 5.;

/// Default maximum amount of time a task is allowed to run for.
/// If this value gets modified the documentation in `cas_server.rs` must also be updated.
const DEFAULT_MAX_ACTION_TIMEOUT: Duration = Duration::from_secs(1200); // 20 mins.

fn init_metrics() -> WorkerMetrics {
    let meter = global::meter_with_scope(InstrumentationScope::builder("local_worker").build());

    WorkerMetrics {
        actions_received: meter
            .u64_counter("worker_actions_received_total")
            .with_description("Total number of actions sent to this worker to process")
            .build(),
        disconnects_received: meter
            .u64_counter("worker_disconnects_received_total")
            .with_description("Total number of disconnects received from the scheduler")
            .build(),
        keep_alives_received: meter
            .u64_counter("worker_keep_alives_received_total")
            .with_description("Total number of keep-alives received from the scheduler")
            .build(),
        action_execution_duration: meter
            .f64_histogram("worker_action_execution_duration")
            .with_description("Duration of action execution")
            .with_unit("ms")
            .build(),
        active_actions: meter
            .i64_up_down_counter("worker_active_actions")
            .with_description("Number of actions currently being executed")
            .build(),
    }
}

#[derive(Debug, Clone)]
struct WorkerMetrics {
    actions_received: metrics::Counter<u64>,
    disconnects_received: metrics::Counter<u64>,
    keep_alives_received: metrics::Counter<u64>,
    action_execution_duration: metrics::Histogram<f64>,
    active_actions: metrics::UpDownCounter<i64>,
}

struct LocalWorkerImpl<'a, T: WorkerApiClientTrait, U: RunningActionsManager> {
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
    metrics: WorkerMetrics,
}

#[instrument(skip(precondition_script), level = "debug")]
async fn preconditions_met(
    precondition_script: Option<String>,
    metrics: &WorkerMetrics,
) -> Result<(), Error> {
    let Some(precondition_script) = &precondition_script else {
        // No script means we are always ok to proceed.
        return Ok(());
    };

    debug!(?precondition_script, "Running precondition script");

    // TODO: Might want to pass some information about the command to the
    //       script, but at this point it's not even been downloaded yet,
    //       so that's not currently possible.  Perhaps we'll move this in
    //       future to pass useful information through?  Or perhaps we'll
    //       have a pre-condition and a pre-execute script instead, although
    //       arguably entrypoint already gives us that.
    let precondition_process = process::Command::new(precondition_script)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env_clear()
        .spawn()
        .err_tip(|| format!("Could not execute precondition command {precondition_script:?}"))?;
    let output = precondition_process.wait_with_output().await?;
    if output.status.code() == Some(0) {
        Ok(())
    } else {
        Err(make_err!(
            Code::ResourceExhausted,
            "Preconditions script returned status {} - {}",
            output.status,
            str::from_utf8(&output.stdout).unwrap_or("")
        ))
    }
}

impl<'a, T: WorkerApiClientTrait, U: RunningActionsManager> LocalWorkerImpl<'a, T, U> {
    fn new(
        config: &'a LocalWorkerConfig,
        grpc_client: T,
        worker_id: String,
        running_actions_manager: Arc<U>,
        metrics: WorkerMetrics,
    ) -> Self {
        Self {
            config,
            grpc_client,
            worker_id,
            running_actions_manager,
            // Number of actions that have been received in `Update::StartAction`, but
            // not yet processed by running_actions_manager's spawn. This number should
            // always be zero if there are no actions running and no actions being waited
            // on by the scheduler.
            actions_in_transit: Arc::new(AtomicU64::new(0)),
            metrics,
        }
    }

    /// Starts a background spawn/thread that will send a message to the server every `timeout / 2`.
    #[instrument(skip(self), level = "debug")]
    async fn start_keep_alive(&self) -> Result<(), Error> {
        // According to tonic's documentation this call should be cheap and is the same stream.
        let mut grpc_client = self.grpc_client.clone();

        loop {
            let timeout = self
                .config
                .worker_api_endpoint
                .timeout
                .unwrap_or(DEFAULT_ENDPOINT_TIMEOUT_S);
            // We always send 2 keep alive requests per timeout. Http2 should manage most of our
            // timeout issues, this is a secondary check to ensure we can still send data.
            sleep(Duration::from_secs_f32(timeout / 2.)).await;

            debug!("Sending keep-alive to worker API server");

            if let Err(e) = grpc_client
                .keep_alive(KeepAliveRequest {
                    worker_id: self.worker_id.clone(),
                })
                .await
            {
                error!(?e, "Failed to send keep-alive");
                return Err(make_err!(
                    Code::Internal,
                    "Failed to send KeepAlive in LocalWorker : {:?}",
                    e
                ));
            }
        }
    }

    #[instrument(skip(self, update_for_worker_stream, shutdown_rx), level = "debug")]
    async fn run(
        &mut self,
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

        info!("Starting worker event loop");

        loop {
            select! {
                maybe_update = update_for_worker_stream.next() => {
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
                        // TODO(allada) We should possibly do something with this notification.
                        Update::Disconnect(()) => {
                            debug!("Received disconnect notification");
                            self.metrics.disconnects_received.add(1, &[]);
                        }
                        Update::KeepAlive(()) => {
                            debug!("Received keep-alive notification");
                            self.metrics.keep_alives_received.add(1, &[]);
                        }
                        Update::KillOperationRequest(kill_operation_request) => {
                            let operation_id = OperationId::from(kill_operation_request.operation_id);
                            debug!(?operation_id, "Received kill operation request");

                            if let Err(err) = self.running_actions_manager.kill_operation(&operation_id).await {
                                error!(
                                    ?operation_id,
                                    ?err,
                                    "Failed to send kill request for operation"
                                );
                            };
                        }
                        Update::StartAction(start_execute) => {
                            self.metrics.actions_received.add(1, &[]);
                            self.metrics.active_actions.add(1, &[]);

                            let execute_request = start_execute.execute_request.as_ref();
                            let operation_id = start_execute.operation_id.clone();
                            let maybe_instance_name = execute_request.map(|v| v.instance_name.clone());
                            let action_digest = execute_request.and_then(|v| v.action_digest.clone());

                            debug!(
                                ?operation_id,
                                action_digest = ?action_digest,
                                "Received start action request"
                            );

                            let digest_hasher = execute_request
                                .ok_or(make_input_err!("Expected execute_request to be set"))
                                .and_then(|v| DigestHasherFunc::try_from(v.digest_function))
                                .err_tip(|| "In LocalWorkerImpl::new()")?;

                            let start_action_fut = {
                                let start_time = std::time::Instant::now();
                                let precondition_script_cfg = self.config.experimental_precondition_script.clone();
                                let actions_in_transit = self.actions_in_transit.clone();
                                let worker_id = self.worker_id.clone();
                                let running_actions_manager = self.running_actions_manager.clone();
                                let metrics = self.metrics.clone();

                                async move {
                                    if let Err(e) = preconditions_met(precondition_script_cfg, &metrics).await {
                                        actions_in_transit.fetch_sub(1, Ordering::Release);
                                        metrics.active_actions.add(-1, &[]);
                                        return Err(e);
                                    }

                                    let action_result = running_actions_manager
                                        .create_and_add_action(worker_id, start_execute)
                                        .await;

                                    actions_in_transit.fetch_sub(1, Ordering::Release);

                                    if let Err(e) = &action_result {
                                        metrics.active_actions.add(-1, &[]);
                                        return Err(e.clone());
                                    }

                                    let action = action_result.unwrap();

                                    info!(
                                        operation_id = ?action.get_operation_id(),
                                        "Received request to run action"
                                    );



                                    let result = action
                                        .clone()
                                        .prepare_action()
                                        .and_then(RunningAction::execute)
                                        .and_then(RunningAction::upload_results)
                                        .and_then(RunningAction::get_finished_result)
                                        // Note: We need ensure we run cleanup even if one of the other steps fail.
                                        .then(|result| async move {
                                            if let Err(e) = action.cleanup().await {
                                                return Result::<ActionResult, Error>::Err(e).merge(result);
                                            }
                                            result
                                        })
                                        .await;

                                    metrics.action_execution_duration.record(
                                        start_time.elapsed().as_millis() as f64,
                                        &[KeyValue::new("success", result.is_ok().to_string())],
                                    );
                                    metrics.active_actions.add(-1, &[]);

                                    result
                                }
                            };

                            let make_publish_future = {
                                let mut grpc_client = self.grpc_client.clone();
                                let worker_id = self.worker_id.clone();
                                let running_actions_manager = self.running_actions_manager.clone();
                                let operation_id_for_closure = operation_id.clone();

                                move |res: Result<ActionResult, Error>| async move {
                                    debug!("Publishing action result");

                                    let instance_name = maybe_instance_name
                                        .err_tip(|| "`instance_name` could not be resolved; this is likely an internal error in local_worker.")?;
                                    match res {
                                        Ok(mut action_result) => {
                                            // Save in the action cache before notifying the scheduler that we've completed.
                                            if let Some(digest_info) = action_digest.clone().and_then(|action_digest| action_digest.try_into().ok()) {
                                                if let Err(err) = running_actions_manager.cache_action_result(digest_info, &mut action_result, digest_hasher).await {
                                                    error!(
                                                        ?err,
                                                        ?action_digest,
                                                        "Error saving action in store",
                                                    );
                                                }
                                            }
                                            let action_stage = ActionStage::Completed(action_result);
                                            grpc_client.execution_response(
                                                ExecuteResult{
                                                    worker_id,
                                                    instance_name,
                                                    operation_id: operation_id_for_closure,
                                                    result: Some(execute_result::Result::ExecuteResponse(action_stage.into())),
                                                }
                                            )
                                            .await
                                            .err_tip(|| "Error while calling execution_response")?;
                                        },
                                        Err(e) => {
                                            grpc_client.execution_response(ExecuteResult{
                                                worker_id,
                                                instance_name,
                                                operation_id: operation_id_for_closure,
                                                result: Some(execute_result::Result::InternalError(e.into())),
                                            }).await.err_tip(|| "Error calling execution_response with error")?;
                                        },
                                    }
                                    Ok(())
                                }
                            };

                            self.actions_in_transit.fetch_add(1, Ordering::Release);
                            let futures_ref = &futures;

                            let add_future_channel = add_future_channel.clone();

                            info_span!(
                                "worker_start_action_ctx",
                                digest_function = %digest_hasher.to_string(),
                                opetation_id = %operation_id,
                            ).in_scope(|| {
                                let _guard = Context::current_with_value(digest_hasher)
                                    .attach();

                                futures_ref.push(
                                    spawn!("worker_start_action", start_action_fut).map(move |res| {
                                        let res = res.err_tip(|| "Failed to launch spawn")?;
                                        if let Err(err) = &res {
                                            error!(
                                                ?err,
                                                "Error executing action",
                                            );
                                        }
                                        add_future_channel
                                            .send(make_publish_future(res).boxed())
                                            .map_err(|_| make_err!(Code::Internal, "LocalWorker could not send future"))?;
                                        Ok(())
                                    })
                                    .boxed()
                                );
                            });
                        }
                    };
                },
                res = add_future_rx.next() => {
                    debug!("Received new future to process");
                    let fut = res.err_tip(|| "New future stream receives should never be closed")?;
                    futures.push(fut);
                },
                res = futures.next() => {
                    debug!("Future completed");
                    res.err_tip(|| "Keep-alive should always pending. Likely unable to send data to scheduler")??;
                }
                complete_msg = shutdown_rx.recv().fuse() => {
                    warn!("Worker loop reveiced shutdown signal. Shutting down worker...",);
                    let mut grpc_client = self.grpc_client.clone();
                    let worker_id = self.worker_id.clone();
                    let running_actions_manager = self.running_actions_manager.clone();
                    let complete_msg_clone = complete_msg.map_err(|e| make_err!(Code::Internal, "Failed to receive shutdown message: {e:?}"))?.clone();
                    let shutdown_future = async move {
                        if let Err(e) = grpc_client.going_away(GoingAwayRequest { worker_id }).await {
                            error!("Failed to send GoingAwayRequest: {e}");
                            return Err(e.into());
                        }
                        running_actions_manager.complete_actions(complete_msg_clone).await;
                        Ok::<(), Error>(())
                    };
                    futures.push(shutdown_future.boxed());
                },
            };
        }
        // Unreachable.
    }
}

type ConnectionFactory<T> = Box<dyn Fn() -> BoxFuture<'static, Result<T, Error>> + Send + Sync>;

pub struct LocalWorker<T: WorkerApiClientTrait, U: RunningActionsManager> {
    config: Arc<LocalWorkerConfig>,
    running_actions_manager: Arc<U>,
    connection_factory: ConnectionFactory<T>,
    sleep_fn: Option<Box<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>>,
    metrics: WorkerMetrics,
}

impl<T: WorkerApiClientTrait + core::fmt::Debug, U: RunningActionsManager + core::fmt::Debug>
    core::fmt::Debug for LocalWorker<T, U>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("LocalWorker")
            .field("config", &self.config)
            .field("running_actions_manager", &self.running_actions_manager)
            .finish_non_exhaustive()
    }
}

/// Creates a new `LocalWorker`. The `cas_store` must be an instance of
/// `FastSlowStore` and will be checked at runtime.
#[instrument(skip(config, cas_store, ac_store, historical_store), level = "info")]
pub async fn new_local_worker(
    config: Arc<LocalWorkerConfig>,
    cas_store: Store,
    ac_store: Option<Store>,
    historical_store: Store,
) -> Result<LocalWorker<WorkerApiClientWrapper, RunningActionsManagerImpl>, Error> {
    info!("Initializing local worker");

    let fast_slow_store = cas_store
        .downcast_ref::<FastSlowStore>(None)
        .err_tip(|| "Expected store for LocalWorker's store to be a FastSlowStore")?
        .get_arc()
        .err_tip(|| "FastSlowStore's Arc doesn't exist")?;

    if let Ok(path) = fs::canonicalize(&config.work_directory).await {
        debug!(
            "Removing existing work directory: {}",
            &config.work_directory
        );
        fs::remove_dir_all(path)
            .await
            .err_tip(|| "Could not remove work_directory in LocalWorker")?;
    }

    debug!("Creating work directory: {}", &config.work_directory);
    fs::create_dir_all(&config.work_directory)
        .await
        .err_tip(|| format!("Could not make work_directory : {}", config.work_directory))?;

    let entrypoint = if config.entrypoint.is_empty() {
        None
    } else {
        Some(config.entrypoint.clone())
    };

    let max_action_timeout = if config.max_action_timeout == 0 {
        DEFAULT_MAX_ACTION_TIMEOUT
    } else {
        Duration::from_secs(config.max_action_timeout as u64)
    };

    info!(
        ?max_action_timeout,
        has_entrypoint = entrypoint.is_some(),
        "Creating running actions manager"
    );

    let running_actions_manager =
        Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
            root_action_directory: config.work_directory.clone(),
            execution_configuration: ExecutionConfiguration {
                entrypoint,
                additional_environment: config.additional_environment.clone(),
            },
            cas_store: fast_slow_store,
            ac_store,
            historical_store,
            upload_action_result_config: &config.upload_action_result,
            max_action_timeout,
            timeout_handled_externally: config.timeout_handled_externally,
        })?);

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

                debug!(
                    endpoint = ?config.worker_api_endpoint.uri,
                    ?timeout_duration,
                    "Creating endpoint connection"
                );

                let tls_config =
                    tls_utils::load_client_config(&config.worker_api_endpoint.tls_config)
                        .err_tip(|| "Parsing local worker TLS configuration")?;

                let endpoint =
                    tls_utils::endpoint_from(&config.worker_api_endpoint.uri, tls_config)
                        .map_err(|e| make_input_err!("Invalid URI for worker endpoint : {e:?}"))?
                        .connect_timeout(timeout_duration)
                        .timeout(timeout_duration);

                info!(
                    "Connecting to worker API endpoint: {}",
                    config.worker_api_endpoint.uri
                );
                let transport = endpoint.connect().await.map_err(|e| {
                    make_err!(
                        Code::Internal,
                        "Could not connect to endpoint {}: {e:?}",
                        config.worker_api_endpoint.uri
                    )
                })?;
                Ok(WorkerApiClient::new(transport).into())
            })
        }),
        Box::new(move |d| Box::pin(sleep(d))),
    );

    info!("Local worker initialized successfully");
    Ok(local_worker)
}

impl<T: WorkerApiClientTrait, U: RunningActionsManager> LocalWorker<T, U> {
    pub fn new_with_connection_factory_and_actions_manager(
        config: Arc<LocalWorkerConfig>,
        running_actions_manager: Arc<U>,
        connection_factory: ConnectionFactory<T>,
        sleep_fn: Box<dyn Fn(Duration) -> BoxFuture<'static, ()> + Send + Sync>,
    ) -> Self {
        // let metrics = Arc::new(Metrics::new(Arc::downgrade(
        //     running_actions_manager.metrics(),
        // )));
        Self {
            config,
            running_actions_manager,
            connection_factory,
            sleep_fn: Some(sleep_fn),
            metrics: init_metrics(),
        }
    }

    #[allow(
        clippy::missing_const_for_fn,
        reason = "False positive on stable, but not on nightly"
    )]
    pub fn name(&self) -> &String {
        &self.config.name
    }

    #[instrument(skip(self, client), level = "debug")]
    async fn register_worker(
        &self,
        client: &mut T,
    ) -> Result<(String, Streaming<UpdateForWorker>), Error> {
        debug!("Registering worker with scheduler");

        let connect_worker_request =
            make_connect_worker_request(self.config.name.clone(), &self.config.platform_properties)
                .await?;

        info!("Sending connect worker request");
        let mut update_for_worker_stream = client
            .connect_worker(connect_worker_request)
            .await
            .err_tip(|| "Could not call connect_worker() in worker")?
            .into_inner();

        debug!("Waiting for first message from scheduler");
        let first_msg_update = update_for_worker_stream
            .next()
            .await
            .err_tip(|| "Got EOF expected UpdateForWorker")?
            .err_tip(|| "Got error when receiving UpdateForWorker")?
            .update;

        let worker_id = match first_msg_update {
            Some(Update::ConnectionResult(connection_result)) => {
                info!("Received worker ID: {}", connection_result.worker_id);
                connection_result.worker_id
            }
            other => {
                error!(?other, "Unexpected response from scheduler");
                return Err(make_input_err!(
                    "Expected first response from scheduler to be a ConnectResult got : {:?}",
                    other
                ));
            }
        };
        Ok((worker_id, update_for_worker_stream))
    }

    #[instrument(skip(self, shutdown_rx), level = "info")]
    pub async fn run(
        mut self,
        mut shutdown_rx: broadcast::Receiver<ShutdownGuard>,
    ) -> Result<(), Error> {
        info!("Starting worker run loop");

        let sleep_fn = self
            .sleep_fn
            .take()
            .err_tip(|| "Could not unwrap sleep_fn in LocalWorker::run")?;
        let sleep_fn_pin = Pin::new(&sleep_fn);

        let error_handler = |err| {
            let sleep_fn_pin = &sleep_fn_pin;
            async move {
                error!(?err, "Error connecting to scheduler");
                (*sleep_fn_pin)(Duration::from_secs_f32(CONNECTION_RETRY_DELAY_S)).await;
            }
        };

        // let error_handler = Box::pin(move |err| async move {
        //     event!(Level::ERROR, ?err, "Error");
        //     (sleep_fn_pin)(Duration::from_secs_f32(CONNECTION_RETRY_DELAY_S)).await;
        // });

        loop {
            // First connect to our endpoint.
            let mut client = match (self.connection_factory)().await {
                Ok(client) => client,
                Err(e) => {
                    (error_handler)(e).await;
                    continue; // Try to connect again.
                }
            };

            debug!("Connected to worker API endpoint");

            // Next register our worker with the scheduler.
            let (mut inner, update_for_worker_stream) =
                match self.register_worker(&mut client).await {
                    Err(e) => {
                        (error_handler)(e).await;
                        continue; // Try to connect again.
                    }
                    Ok((worker_id, update_for_worker_stream)) => (
                        LocalWorkerImpl::new(
                            &self.config,
                            client,
                            worker_id,
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

            // Now listen for connections and run all other services.
            if let Err(err) = inner.run(update_for_worker_stream, &mut shutdown_rx).await {
                'no_more_actions: {
                    // Ensure there are no actions in transit before we try to kill
                    // all our actions.
                    const ITERATIONS: usize = 1_000;

                    const ERROR_MSG: &str = "Actions in transit did not reach zero before we disconnected from the scheduler";

                    debug!("Waiting for in-transit actions to complete");

                    let sleep_duration = ACTIONS_IN_TRANSIT_TIMEOUT_S / ITERATIONS as f32;
                    for i in 0..ITERATIONS {
                        let actions_in_transit = inner.actions_in_transit.load(Ordering::Acquire);
                        if actions_in_transit == 0 {
                            debug!("All in-transit actions completed");
                            break 'no_more_actions;
                        }

                        if i % 100 == 0 {
                            debug!(
                                remaining_actions = actions_in_transit,
                                iteration = i,
                                "Waiting for in-transit actions"
                            );
                        }

                        (sleep_fn_pin)(Duration::from_secs_f32(sleep_duration)).await;
                    }

                    error!(ERROR_MSG);
                    return Err(err.append(ERROR_MSG));
                }
                error!(?err, "Worker disconnected from scheduler");

                // Kill off any existing actions because if we re-connect, we'll
                // get some more and it might resource lock us.
                info!("Killing all running actions");
                self.running_actions_manager.kill_all().await;

                (error_handler)(err).await; // Try to connect again.
            }
        }
        // Unreachable.
    }
}
