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

use core::convert::Into;
use core::pin::Pin;
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::stream::unfold;
use futures::{Stream, StreamExt};
use nativelink_config::cas_server::WorkerApiConfig;
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::update_for_scheduler::Update;
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::worker_api_server::{
    WorkerApi, WorkerApiServer as Server,
};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    execute_result, ExecuteComplete, ExecuteResult, GoingAwayRequest, KeepAliveRequest, UpdateForScheduler, UpdateForWorker
};
use nativelink_scheduler::worker::Worker;
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::background_spawn;
use nativelink_util::action_messages::{OperationId, WorkerId};
use nativelink_util::operation_state_manager::UpdateOperationType;
use nativelink_util::platform_properties::PlatformProperties;
use rand::RngCore;
use tokio::sync::mpsc;
use tokio::time::interval;
use tonic::{Response, Status};
use tracing::{debug, error, warn, instrument, Level};
use uuid::Uuid;

pub type ConnectWorkerStream =
    Pin<Box<dyn Stream<Item = Result<UpdateForWorker, Status>> + Send + Sync + 'static>>;

pub type NowFn = Box<dyn Fn() -> Result<Duration, Error> + Send + Sync>;

pub struct WorkerApiServer {
    scheduler: Arc<dyn WorkerScheduler>,
    now_fn: Arc<NowFn>,
    node_id: [u8; 6],
}

impl core::fmt::Debug for WorkerApiServer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("WorkerApiServer")
            .field("node_id", &self.node_id)
            .finish_non_exhaustive()
    }
}

impl WorkerApiServer {
    pub fn new(
        config: &WorkerApiConfig,
        schedulers: &HashMap<String, Arc<dyn WorkerScheduler>>,
    ) -> Result<Self, Error> {
        let node_id = {
            let mut out = [0; 6];
            rand::rng().fill_bytes(&mut out);
            out
        };
        for scheduler in schedulers.values() {
            // This will protect us from holding a reference to the scheduler forever in the
            // event our ExecutionServer dies. Our scheduler is a weak ref, so the spawn will
            // eventually see the Arc went away and return.
            let weak_scheduler = Arc::downgrade(scheduler);
            background_spawn!("worker_api_server", async move {
                let mut ticker = interval(Duration::from_secs(1));
                loop {
                    ticker.tick().await;
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("Error: system time is now behind unix epoch");
                    match weak_scheduler.upgrade() {
                        Some(scheduler) => {
                            if let Err(err) =
                                scheduler.remove_timedout_workers(timestamp.as_secs()).await
                            {
                                error!(?err, "Failed to remove_timedout_workers",);
                            }
                        }
                        // If we fail to upgrade, our service is probably destroyed, so return.
                        None => return,
                    }
                }
            });
        }

        Self::new_with_now_fn(
            config,
            schedulers,
            Box::new(move || {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|_| make_err!(Code::Internal, "System time is now behind unix epoch"))
            }),
            node_id,
        )
    }

    /// Same as `new()`, but you can pass a custom `now_fn`, that returns a Duration since `UNIX_EPOCH`
    /// representing the current time. Used mostly in  unit tests.
    pub fn new_with_now_fn(
        config: &WorkerApiConfig,
        schedulers: &HashMap<String, Arc<dyn WorkerScheduler>>,
        now_fn: NowFn,
        node_id: [u8; 6],
    ) -> Result<Self, Error> {
        let scheduler = schedulers
            .get(&config.scheduler)
            .err_tip(|| {
                format!(
                    "Scheduler needs config for '{}' because it exists in worker_api",
                    config.scheduler
                )
            })?
            .clone();
        Ok(Self {
            scheduler,
            now_fn: Arc::new(now_fn),
            node_id,
        })
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
    }

    async fn inner_connect_worker(
        &self,
        mut update_stream: impl Stream<Item = Result<UpdateForScheduler, Status>>
        + Unpin
        + Send
        + 'static,
    ) -> Result<Response<ConnectWorkerStream>, Error> {
        let first_message = update_stream
            .next()
            .await
            .err_tip(|| "Missing first message for connect_worker")?
            .err_tip(|| "Error reading first message for connect_worker")?;
        let Some(Update::ConnectWorkerRequest(connect_worker_request)) = first_message.update
        else {
            return Err(make_err!(
                Code::Internal,
                "First message was not a ConnectWorkerRequest"
            ));
        };

        let (tx, rx) = mpsc::unbounded_channel();

        // First convert our proto platform properties into one our scheduler understands.
        let platform_properties = {
            let mut platform_properties = PlatformProperties::default();
            for property in connect_worker_request.properties {
                let platform_property_value = self
                    .scheduler
                    .get_platform_property_manager()
                    .make_prop_value(&property.name, &property.value)
                    .err_tip(|| "Bad Property during connect_worker()")?;
                platform_properties
                    .properties
                    .insert(property.name.clone(), platform_property_value);
            }
            platform_properties
        };

        // Now register the worker with the scheduler.
        let worker_id = {
            let worker_id = WorkerId(format!(
                "{}{}",
                connect_worker_request.worker_id_prefix,
                Uuid::now_v6(&self.node_id).hyphenated()
            ));
            let worker = Worker::new(
                worker_id.clone(),
                platform_properties,
                tx,
                (self.now_fn)()?.as_secs(),
                connect_worker_request.max_inflight_tasks,
            );
            self.scheduler
                .add_worker(worker)
                .await
                .err_tip(|| "Failed to add worker in inner_connect_worker()")?;
            worker_id
        };

        WorkerConnection::start(
            self.scheduler.clone(),
            self.now_fn.clone(),
            worker_id.clone(),
            update_stream,
        );

        Ok(Response::new(Box::pin(unfold(
            (rx, worker_id),
            move |state| async move {
                let (mut rx, worker_id) = state;
                if let Some(update_for_worker) = rx.recv().await {
                    return Some((Ok(update_for_worker), (rx, worker_id)));
                }
                warn!(
                    ?worker_id,
                    "UpdateForWorker channel was closed, thus closing connection to worker node",
                );

                None
            },
        ))))
    }

    pub async fn inner_connect_worker_for_testing(
        &self,
        update_stream: impl Stream<Item = Result<UpdateForScheduler, Status>> + Unpin + Send + 'static,
    ) -> Result<Response<ConnectWorkerStream>, Error> {
        self.inner_connect_worker(update_stream).await
    }
}

#[tonic::async_trait]
impl WorkerApi for WorkerApiServer {
    type ConnectWorkerStream = ConnectWorkerStream;

    #[instrument(
        err,
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn connect_worker(
        &self,
        grpc_request: tonic::Request<tonic::Streaming<UpdateForScheduler>>,
    ) -> Result<Response<Self::ConnectWorkerStream>, Status> {
        let resp = self
            .inner_connect_worker(grpc_request.into_inner())
            .await
            .map_err(Into::into);
        if resp.is_ok() {
            debug!(return = "Ok(<stream>)");
        }
        resp
    }
}

struct WorkerConnection {
    scheduler: Arc<dyn WorkerScheduler>,
    now_fn: Arc<NowFn>,
    worker_id: WorkerId,
}

impl WorkerConnection {
    fn start(
        scheduler: Arc<dyn WorkerScheduler>,
        now_fn: Arc<NowFn>,
        worker_id: WorkerId,
        mut connection: impl Stream<Item = Result<UpdateForScheduler, Status>> + Unpin + Send + 'static,
    ) {
        let instance = Self {
            scheduler,
            now_fn,
            worker_id,
        };

        background_spawn!("worker_api", async move {
            let mut had_going_away = false;
            while let Some(maybe_update) = connection.next().await {
                let update = match maybe_update.map(|u| u.update) {
                    Ok(Some(update)) => update,
                    Ok(None) => {
                        tracing::warn!(worker_id=?instance.worker_id, "Empty update");
                        continue;
                    }
                    Err(err) => {
                        tracing::warn!(worker_id=?instance.worker_id, ?err, "Error from worker");
                        break;
                    }
                };
                let result = match update {
                    Update::ConnectWorkerRequest(_connect_worker_request) => Err(make_err!(
                        Code::Internal,
                        "Got ConnectWorkerRequest after initial message for {}",
                        instance.worker_id
                    )),
                    Update::KeepAliveRequest(keep_alive_request) => {
                        instance.inner_keep_alive(keep_alive_request).await
                    }
                    Update::GoingAwayRequest(going_away_request) => {
                        had_going_away = true;
                        instance.inner_going_away(going_away_request).await
                    }
                    Update::ExecuteResult(execute_result) => {
                        instance.inner_execution_response(execute_result).await
                    }
                    Update::ExecuteComplete(execute_complete) => {
                        instance.execution_complete(execute_complete).await
                    }
                };
                if let Err(err) = result {
                    tracing::warn!(worker_id=?instance.worker_id, ?err, "Error processing worker message");
                }
            }
            tracing::debug!(worker_id=?instance.worker_id, "Update for scheduler dropped");
            if !had_going_away {
                drop(instance.scheduler.remove_worker(&instance.worker_id).await);
            }
        });
    }

    async fn inner_keep_alive(&self, _keep_alive_request: KeepAliveRequest) -> Result<(), Error> {
        self.scheduler
            .worker_keep_alive_received(&self.worker_id, (self.now_fn)()?.as_secs())
            .await
            .err_tip(|| "Could not process keep_alive from worker in inner_keep_alive()")?;
        Ok(())
    }

    async fn inner_going_away(&self, _going_away_request: GoingAwayRequest) -> Result<(), Error> {
        self.scheduler
            .remove_worker(&self.worker_id)
            .await
            .err_tip(|| "While calling WorkerApiServer::inner_going_away")?;
        Ok(())
    }

    async fn inner_execution_response(&self, execute_result: ExecuteResult) -> Result<(), Error> {
        let operation_id = OperationId::from(execute_result.operation_id);

        match execute_result
            .result
            .err_tip(|| "Expected result to exist in ExecuteResult")?
        {
            execute_result::Result::ExecuteResponse(finished_result) => {
                let action_stage = finished_result
                    .try_into()
                    .err_tip(|| "Failed to convert ExecuteResponse into an ActionStage")?;
                self.scheduler
                    .update_action(
                        &self.worker_id,
                        &operation_id,
                        UpdateOperationType::UpdateWithActionStage(action_stage),
                    )
                    .await
                    .err_tip(|| format!("Failed to operation {operation_id}"))?;
            }
            execute_result::Result::InternalError(e) => {
                self.scheduler
                    .update_action(
                        &self.worker_id,
                        &operation_id,
                        UpdateOperationType::UpdateWithError(e.into()),
                    )
                    .await
                    .err_tip(|| format!("Failed to operation {operation_id}"))?;
            }
        }
        Ok(())
    }

    async fn execution_complete(&self, execute_complete: ExecuteComplete) -> Result<(), Error> {
        let operation_id = OperationId::from(execute_complete.operation_id);
        self.scheduler
            .update_action(
                &self.worker_id,
                &operation_id,
                UpdateOperationType::ExecutionComplete,
            )
            .await
            .err_tip(|| format!("Failed to operation {operation_id}"))?;
        Ok(())
    }
}
