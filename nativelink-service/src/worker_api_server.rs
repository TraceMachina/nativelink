// Copyright 2023 The NativeLink Authors. All rights reserved.
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

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use futures::stream::unfold;
use futures::Stream;
use nativelink_config::cas_server::WorkerApiConfig;
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::worker_api_server::{
    WorkerApi, WorkerApiServer as Server,
};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    execute_result, ExecuteResult, GoingAwayRequest, KeepAliveRequest, SupportedProperties, UpdateForWorker,
};
use nativelink_scheduler::worker::{Worker, WorkerId};
use nativelink_scheduler::worker_scheduler::WorkerScheduler;
use nativelink_util::action_messages::ActionInfoHashKey;
use nativelink_util::common::DigestInfo;
use nativelink_util::platform_properties::PlatformProperties;
use tokio::sync::mpsc;
use tokio::time::interval;
use tonic::{Request, Response, Status};
use tracing::{error, info, warn};
use uuid::Uuid;

pub type ConnectWorkerStream = Pin<Box<dyn Stream<Item = Result<UpdateForWorker, Status>> + Send + Sync + 'static>>;

pub type NowFn = Box<dyn Fn() -> Result<Duration, Error> + Send + Sync>;

pub struct WorkerApiServer {
    scheduler: Arc<dyn WorkerScheduler>,
    now_fn: NowFn,
}

impl WorkerApiServer {
    pub fn new(
        config: &WorkerApiConfig,
        schedulers: &HashMap<String, Arc<dyn WorkerScheduler>>,
    ) -> Result<Self, Error> {
        for scheduler in schedulers.values() {
            // This will protect us from holding a reference to the scheduler forever in the
            // event our ExecutionServer dies. Our scheduler is a weak ref, so the spawn will
            // eventually see the Arc went away and return.
            let weak_scheduler = Arc::downgrade(scheduler);
            tokio::spawn(async move {
                let mut ticker = interval(Duration::from_secs(1));
                loop {
                    ticker.tick().await;
                    let timestamp = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("Error: system time is now behind unix epoch");
                    match weak_scheduler.upgrade() {
                        Some(scheduler) => {
                            if let Err(e) = scheduler.remove_timedout_workers(timestamp.as_secs()).await {
                                error!("Error while running remove_timedout_workers : {:?}", e);
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
        )
    }

    /// Same as new(), but you can pass a custom `now_fn`, that returns a Duration since UNIX_EPOCH
    /// representing the current time. Used mostly in  unit tests.
    pub fn new_with_now_fn(
        config: &WorkerApiConfig,
        schedulers: &HashMap<String, Arc<dyn WorkerScheduler>>,
        now_fn: NowFn,
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
        Ok(Self { scheduler, now_fn })
    }

    pub fn into_service(self) -> Server<WorkerApiServer> {
        Server::new(self)
    }

    async fn inner_connect_worker(
        &self,
        supported_properties: SupportedProperties,
    ) -> Result<Response<ConnectWorkerStream>, Error> {
        let (tx, rx) = mpsc::unbounded_channel();

        // First convert our proto platform properties into one our scheduler understands.
        let platform_properties = {
            let mut platform_properties = PlatformProperties::default();
            for property in supported_properties.properties {
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
            let worker_id = Uuid::new_v4().as_u128();
            let worker = Worker::new(WorkerId(worker_id), platform_properties, tx, (self.now_fn)()?.as_secs());
            self.scheduler
                .add_worker(worker)
                .await
                .err_tip(|| "Failed to add worker in inner_connect_worker()")?;
            worker_id
        };

        Ok(Response::new(Box::pin(unfold(
            (rx, worker_id),
            move |state| async move {
                let (mut rx, worker_id) = state;
                if let Some(update_for_worker) = rx.recv().await {
                    return Some((Ok(update_for_worker), (rx, worker_id)));
                }
                warn!(
                    "UpdateForWorker channel was closed, thus closing connection to worker node : {}",
                    worker_id
                );

                None
            },
        ))))
    }

    async fn inner_keep_alive(&self, keep_alive_request: KeepAliveRequest) -> Result<Response<()>, Error> {
        let worker_id: WorkerId = keep_alive_request.worker_id.try_into()?;
        self.scheduler
            .worker_keep_alive_received(&worker_id, (self.now_fn)()?.as_secs())
            .await
            .err_tip(|| "Could not process keep_alive from worker in inner_keep_alive()")?;
        Ok(Response::new(()))
    }

    async fn inner_going_away(&self, going_away_request: GoingAwayRequest) -> Result<Response<()>, Error> {
        let worker_id: WorkerId = going_away_request.worker_id.try_into()?;
        self.scheduler.remove_worker(worker_id).await;
        Ok(Response::new(()))
    }

    async fn inner_execution_response(&self, execute_result: ExecuteResult) -> Result<Response<()>, Error> {
        let worker_id: WorkerId = execute_result.worker_id.try_into()?;
        let action_digest: DigestInfo = execute_result
            .action_digest
            .err_tip(|| "Expected action_digest to exist")?
            .try_into()?;
        let action_info_hash_key = ActionInfoHashKey {
            instance_name: execute_result.instance_name,
            digest: action_digest,
            salt: execute_result.salt,
        };

        match execute_result
            .result
            .err_tip(|| "Expected result to exist in ExecuteResult")?
        {
            execute_result::Result::ExecuteResponse(finished_result) => {
                let action_stage = finished_result
                    .try_into()
                    .err_tip(|| "Failed to convert ExecuteResponse into an ActionStage")?;
                self.scheduler
                    .update_action(&worker_id, &action_info_hash_key, action_stage)
                    .await
                    .err_tip(|| format!("Failed to update_action {:?}", action_digest))?;
            }
            execute_result::Result::InternalError(e) => {
                self.scheduler
                    .update_action_with_internal_error(&worker_id, &action_info_hash_key, e.into())
                    .await;
            }
        }
        Ok(Response::new(()))
    }
}

#[tonic::async_trait]
impl WorkerApi for WorkerApiServer {
    type ConnectWorkerStream = ConnectWorkerStream;
    async fn connect_worker(
        &self,
        grpc_request: Request<SupportedProperties>,
    ) -> Result<Response<Self::ConnectWorkerStream>, Status> {
        let now = Instant::now();
        info!("\x1b[0;31mconnect_worker Req\x1b[0m: {:?}", grpc_request.get_ref());
        let supported_properties = grpc_request.into_inner();
        let resp = self.inner_connect_worker(supported_properties).await;
        let d = now.elapsed().as_secs_f32();
        if let Err(err) = resp.as_ref() {
            error!("\x1b[0;31mconnect_worker Resp\x1b[0m: {} {:?}", d, err);
        } else {
            info!("\x1b[0;31mconnect_worker Resp\x1b[0m: {}", d);
        }
        return resp.map_err(|e| e.into());
    }

    async fn keep_alive(&self, grpc_request: Request<KeepAliveRequest>) -> Result<Response<()>, Status> {
        let now = Instant::now();
        info!("\x1b[0;31mkeep_alive Req\x1b[0m: {:?}", grpc_request.get_ref());
        let keep_alive_request = grpc_request.into_inner();
        let resp = self.inner_keep_alive(keep_alive_request).await;
        let d = now.elapsed().as_secs_f32();
        if let Err(err) = resp.as_ref() {
            error!("\x1b[0;31mkeep_alive Resp\x1b[0m: {} {:?}", d, err);
        } else {
            info!("\x1b[0;31mkeep_alive Resp\x1b[0m: {}", d);
        }
        return resp.map_err(|e| e.into());
    }

    async fn going_away(&self, grpc_request: Request<GoingAwayRequest>) -> Result<Response<()>, Status> {
        let now = Instant::now();
        info!("\x1b[0;31mgoing_away Req\x1b[0m: {:?}", grpc_request.get_ref());
        let going_away_request = grpc_request.into_inner();
        let resp = self.inner_going_away(going_away_request).await;
        let d = now.elapsed().as_secs_f32();
        if let Err(err) = resp.as_ref() {
            error!("\x1b[0;31mgoing_away Resp\x1b[0m: {} {:?}", d, err);
        } else {
            info!("\x1b[0;31mgoing_away Resp\x1b[0m: {}", d);
        }
        return resp.map_err(|e| e.into());
    }

    async fn execution_response(&self, grpc_request: Request<ExecuteResult>) -> Result<Response<()>, Status> {
        let now = Instant::now();
        info!("\x1b[0;31mexecution_response Req\x1b[0m: {:?}", grpc_request.get_ref());
        let execute_result = grpc_request.into_inner();
        let resp = self.inner_execution_response(execute_result).await;
        let d = now.elapsed().as_secs_f32();
        if let Err(err) = resp.as_ref() {
            error!("\x1b[0;31mexecution_response Resp\x1b[0m: {} {:?}", d, err);
        } else {
            info!("\x1b[0;31mexecution_response Resp\x1b[0m: {}", d);
        }
        return resp.map_err(|e| e.into());
    }
}
