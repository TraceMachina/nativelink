// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

use futures::{stream::FuturesUnordered, Future, Stream, StreamExt};
use rand::{thread_rng, Rng};
use tokio::sync::watch;
use tokio_stream::wrappers::WatchStream;
use tonic::{Request, Response, Status};

use ac_utils::get_and_decode_digest;
use action_messages::{ActionInfo, ActionInfoHashKey, ActionState, DEFAULT_EXECUTION_PRIORITY};
use common::{log, DigestInfo};
use config::cas_server::{ExecutionConfig, InstanceName};
use error::{make_input_err, Error, ResultExt};
use platform_property_manager::PlatformProperties;
use proto::build::bazel::remote::execution::v2::{
    execution_server::Execution, execution_server::ExecutionServer as Server, Action, Command, ExecuteRequest,
    WaitExecutionRequest,
};
use proto::google::longrunning::{
    operations_server::Operations, operations_server::OperationsServer, CancelOperationRequest, DeleteOperationRequest,
    GetOperationRequest, ListOperationsRequest, ListOperationsResponse, Operation, WaitOperationRequest,
};
use scheduler::ActionScheduler;
use store::{Store, StoreManager};

struct InstanceInfo {
    scheduler: Arc<dyn ActionScheduler>,
    cas_store: Arc<dyn Store>,
}

impl InstanceInfo {
    fn cas_pin(&self) -> Pin<&dyn Store> {
        Pin::new(self.cas_store.as_ref())
    }

    async fn build_action_info(
        &self,
        instance_name: String,
        action_digest: DigestInfo,
        action: &Action,
        priority: i32,
        skip_cache_lookup: bool,
    ) -> Result<ActionInfo, Error> {
        let command_digest = DigestInfo::try_from(
            action
                .command_digest
                .clone()
                .err_tip(|| "Expected command_digest to exist")?,
        )
        .err_tip(|| "Could not decode command digest")?;

        let input_root_digest = DigestInfo::try_from(
            action
                .clone()
                .input_root_digest
                .err_tip(|| "Expected input_digest_root")?,
        )?;
        let timeout = action
            .timeout
            .clone()
            .map(|v| Duration::new(v.seconds as u64, v.nanos as u32))
            .unwrap_or(Duration::MAX);

        let mut platform_properties = HashMap::new();
        if let Some(platform) = &action.platform {
            for property in &platform.properties {
                let platform_property = self
                    .scheduler
                    .get_platform_property_manager(&instance_name)
                    .await
                    .err_tip(|| "Failed to get platform properties in build_action_info")?
                    .make_prop_value(&property.name, &property.value)
                    .err_tip(|| "Failed to convert platform property in build_action_info")?;
                platform_properties.insert(property.name.clone(), platform_property);
            }
        }

        // Goma puts the properties in the Command.
        if platform_properties.is_empty() {
            let command = get_and_decode_digest::<Command>(self.cas_pin(), &command_digest).await?;
            if let Some(platform) = &command.platform {
                for property in &platform.properties {
                    let platform_property = self
                        .scheduler
                        .get_platform_property_manager(&instance_name)
                        .await
                        .err_tip(|| "Failed to get platform properties in build_action_info")?
                        .make_prop_value(&property.name, &property.value)
                        .err_tip(|| "Failed to convert command platform property in build_action_info")?;
                    platform_properties.insert(property.name.clone(), platform_property);
                }
            }
        }

        Ok(ActionInfo {
            instance_name,
            command_digest,
            input_root_digest,
            timeout,
            platform_properties: PlatformProperties::new(platform_properties),
            priority,
            load_timestamp: UNIX_EPOCH,
            insert_timestamp: SystemTime::now(),
            unique_qualifier: ActionInfoHashKey {
                digest: action_digest,
                salt: if action.do_not_cache {
                    thread_rng().gen::<u64>()
                } else {
                    0
                },
            },
            skip_cache_lookup,
        })
    }
}

pub struct ExecutionServer {
    instance_infos: HashMap<InstanceName, InstanceInfo>,
}

type ExecuteStream = Pin<Box<dyn Stream<Item = Result<Operation, Status>> + Send + Sync + 'static>>;

impl ExecutionServer {
    pub fn new(
        config: &HashMap<InstanceName, ExecutionConfig>,
        scheduler_map: &HashMap<String, Arc<dyn ActionScheduler>>,
        store_manager: &StoreManager,
    ) -> Result<Self, Error> {
        let mut instance_infos = HashMap::with_capacity(config.len());
        for (instance_name, exec_cfg) in config {
            let cas_store = store_manager
                .get_store(&exec_cfg.cas_store)
                .ok_or_else(|| make_input_err!("'cas_store': '{}' does not exist", exec_cfg.cas_store))?;
            let scheduler = scheduler_map
                .get(&exec_cfg.scheduler)
                .err_tip(|| {
                    format!(
                        "Scheduler needs config for '{}' because it exists in execution",
                        exec_cfg.scheduler
                    )
                })?
                .clone();

            instance_infos.insert(instance_name.to_string(), InstanceInfo { scheduler, cas_store });
        }
        Ok(Self { instance_infos })
    }

    pub fn into_services(self) -> (Server<ExecutionServer>, OperationsServer<ExecutionServer>) {
        let arc_self = Arc::new(self);
        (Server::from_arc(arc_self.clone()), OperationsServer::from_arc(arc_self))
    }

    fn to_execute_stream(receiver: watch::Receiver<Arc<ActionState>>) -> Response<ExecuteStream> {
        let receiver_stream = Box::pin(WatchStream::new(receiver).map(|action_update| {
            log::info!("\x1b[0;31mexecute Resp Stream\x1b[0m: {:?}", action_update);
            Ok(action_update.as_ref().clone().into())
        }));
        tonic::Response::new(receiver_stream)
    }

    async fn inner_execute(&self, request: Request<ExecuteRequest>) -> Result<Response<ExecuteStream>, Error> {
        let execute_req = request.into_inner();
        let instance_name = execute_req.instance_name;

        let instance_info = self
            .instance_infos
            .get(&instance_name)
            .err_tip(|| "Instance name '{}' not configured")?;

        let digest = DigestInfo::try_from(
            execute_req
                .action_digest
                .err_tip(|| "Expected action_digest to exist")?,
        )
        .err_tip(|| "Failed to unwrap action cache")?;

        let priority = execute_req
            .execution_policy
            .map_or(DEFAULT_EXECUTION_PRIORITY, |p| p.priority);

        let action = get_and_decode_digest::<Action>(instance_info.cas_pin(), &digest).await?;
        let action_info = instance_info
            .build_action_info(instance_name, digest, &action, priority, execute_req.skip_cache_lookup)
            .await?;

        // Warning: This name is ignored by GrpcScheduler, so don't use it for
        //          operations mapping here...
        let name = format!("{:X}", thread_rng().gen::<u128>());
        let rx = instance_info
            .scheduler
            .add_action(name, action_info)
            .await
            .err_tip(|| "Failed to schedule task")?;

        Ok(Self::to_execute_stream(rx))
    }

    async fn lookup_action<'a, Fut, F, G, R, S>(&'a self, mut operation: F, mut filter: G) -> Result<S, Status>
    where
        F: FnMut(&'a InstanceInfo) -> Fut,
        G: FnMut(R) -> Option<S>,
        Fut: Future<Output = R>,
    {
        // Very innefficient having to search through all instances, however
        // the protobuf spec doesn't state that the instance_name can be
        // retrieved from the path.
        self.instance_infos
            .iter()
            .map(|(_instance_name, instance_info)| operation(instance_info))
            .collect::<FuturesUnordered<_>>()
            .filter_map(|result| futures::future::ready(filter(result)))
            .next()
            .await
            .ok_or_else(|| Status::not_found("Failed to find existing task"))
    }

    async fn inner_wait_execution(
        &self,
        request: Request<WaitExecutionRequest>,
    ) -> Result<Response<ExecuteStream>, Status> {
        let name = request.into_inner().name;
        self.lookup_action(
            |instance_info| instance_info.scheduler.find_existing_action(&name),
            |result| result.map(|result| Self::to_execute_stream(result)),
        )
        .await
    }

    async fn inner_cancel_operation(&self, request: Request<CancelOperationRequest>) -> Result<Response<()>, Status> {
        let name = request.into_inner().name;
        self.lookup_action(
            |instance_info| instance_info.scheduler.cancel_existing_action(&name),
            |result| if result { Some(tonic::Response::new(())) } else { None },
        )
        .await
    }
}

#[tonic::async_trait]
impl Execution for ExecutionServer {
    type ExecuteStream = ExecuteStream;

    async fn execute(&self, grpc_request: Request<ExecuteRequest>) -> Result<Response<ExecuteStream>, Status> {
        // TODO(blaise.bruer) This is a work in progress, remote execution likely won't work yet.
        log::info!("\x1b[0;31mexecute Req\x1b[0m: {:?}", grpc_request.get_ref());
        let now = Instant::now();
        let resp = self
            .inner_execute(grpc_request)
            .await
            .err_tip(|| "Failed on execute() command")
            .map_err(|e| e.into());
        let d = now.elapsed().as_secs_f32();
        if let Err(err) = &resp {
            log::error!("\x1b[0;31mexecute Resp\x1b[0m: {} {:?}", d, err);
        } else {
            log::info!("\x1b[0;31mexecute Resp\x1b[0m: {}", d);
        }
        resp
    }

    type WaitExecutionStream = ExecuteStream;
    async fn wait_execution(&self, request: Request<WaitExecutionRequest>) -> Result<Response<ExecuteStream>, Status> {
        let resp = self
            .inner_wait_execution(request)
            .await
            .err_tip(|| "Failed on wait_execution() command")
            .map_err(|e| e.into());
        resp
    }
}

#[tonic::async_trait]
impl Operations for ExecutionServer {
    async fn list_operations(
        &self,
        _request: Request<ListOperationsRequest>,
    ) -> Result<Response<ListOperationsResponse>, Status> {
        log::info!("Unimplemented call to list_operations");
        Err(Status::unimplemented(""))
    }

    async fn get_operation(&self, _request: Request<GetOperationRequest>) -> Result<Response<Operation>, Status> {
        log::info!("Unimplemented call to get_operation");
        Err(Status::unimplemented(""))
    }

    async fn delete_operation(&self, _request: Request<DeleteOperationRequest>) -> Result<Response<()>, Status> {
        log::info!("Unimplemented call to delete_operation");
        Err(Status::unimplemented(""))
    }

    async fn cancel_operation(&self, request: Request<CancelOperationRequest>) -> Result<Response<()>, Status> {
        self.inner_cancel_operation(request)
            .await
            .err_tip(|| "Failed on cancel_operation() command")
            .map_err(|e| e.into())
    }

    async fn wait_operation(&self, _request: Request<WaitOperationRequest>) -> Result<Response<Operation>, Status> {
        log::info!("Unimplemented call to wait_operation");
        Err(Status::unimplemented(""))
    }
}
