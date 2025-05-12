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

use core::convert::Into;
use core::pin::Pin;
use core::time::Duration;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use futures::stream::unfold;
use futures::{Stream, StreamExt};
use nativelink_config::cas_server::{ExecutionConfig, InstanceName, WithInstanceName};
use nativelink_error::{Error, ResultExt, make_input_err};
use nativelink_proto::build::bazel::remote::execution::v2::execution_server::{
    Execution, ExecutionServer as Server,
};
use nativelink_proto::build::bazel::remote::execution::v2::{
    Action, Command, ExecuteRequest, WaitExecutionRequest,
};
use nativelink_proto::google::longrunning::Operation;
use nativelink_store::ac_utils::get_and_decode_digest;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::action_messages::{
    ActionInfo, ActionUniqueKey, ActionUniqueQualifier, DEFAULT_EXECUTION_PRIORITY, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasherFunc, make_ctx_for_hash_func};
use nativelink_util::operation_state_manager::{
    ActionStateResult, ClientStateManager, OperationFilter,
};
use nativelink_util::store_trait::Store;
use opentelemetry::context::FutureExt;
use tonic::{Request, Response, Status};
use tracing::{Instrument, Level, debug, error, error_span, info, instrument};

type InstanceInfoName = String;

struct NativelinkOperationId {
    instance_name: InstanceInfoName,
    client_operation_id: OperationId,
}

impl NativelinkOperationId {
    const fn new(instance_name: InstanceInfoName, client_operation_id: OperationId) -> Self {
        Self {
            instance_name,
            client_operation_id,
        }
    }

    fn from_name(name: &str) -> Result<Self, Error> {
        let (instance_name, name) = name
            .rsplit_once('/')
            .err_tip(|| "Expected instance_name and name to be separated by '/'")?;
        Ok(Self::new(
            instance_name.to_string(),
            OperationId::from(name),
        ))
    }
}

impl fmt::Display for NativelinkOperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.instance_name, self.client_operation_id)
    }
}

struct InstanceInfo {
    scheduler: Arc<dyn ClientStateManager>,
    cas_store: Store,
}

impl fmt::Debug for InstanceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstanceInfo")
            .field("cas_store", &self.cas_store)
            .finish_non_exhaustive()
    }
}

impl InstanceInfo {
    async fn build_action_info(
        &self,
        instance_name: String,
        action_digest: DigestInfo,
        action: Action,
        priority: i32,
        skip_cache_lookup: bool,
        digest_function: DigestHasherFunc,
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
        let timeout = action.timeout.map_or(Duration::MAX, |v| {
            Duration::new(v.seconds as u64, v.nanos as u32)
        });

        let mut platform_properties = HashMap::new();
        if let Some(platform) = action.platform {
            for property in platform.properties {
                platform_properties.insert(property.name, property.value);
            }
        }

        // Goma puts the properties in the Command.
        if platform_properties.is_empty() {
            let command =
                get_and_decode_digest::<Command>(&self.cas_store, command_digest.into()).await?;
            if let Some(platform) = command.platform {
                for property in platform.properties {
                    platform_properties.insert(property.name, property.value);
                }
            }
        }

        let action_key = ActionUniqueKey {
            instance_name,
            digest_function,
            digest: action_digest,
        };
        let unique_qualifier = if skip_cache_lookup {
            ActionUniqueQualifier::Uncachable(action_key)
        } else {
            ActionUniqueQualifier::Cachable(action_key)
        };

        Ok(ActionInfo {
            command_digest,
            input_root_digest,
            timeout,
            platform_properties,
            priority,
            load_timestamp: UNIX_EPOCH,
            insert_timestamp: SystemTime::now(),
            unique_qualifier,
        })
    }
}

#[derive(Debug)]
pub struct ExecutionServer {
    instance_infos: HashMap<InstanceName, InstanceInfo>,
}

type ExecuteStream = Pin<Box<dyn Stream<Item = Result<Operation, Status>> + Send>>;

impl ExecutionServer {
    pub fn new(
        configs: &[WithInstanceName<ExecutionConfig>],
        scheduler_map: &HashMap<String, Arc<dyn ClientStateManager>>,
        store_manager: &StoreManager,
    ) -> Result<Self, Error> {
        let mut instance_infos = HashMap::with_capacity(configs.len());
        for config in configs {
            let cas_store = store_manager.get_store(&config.cas_store).ok_or_else(|| {
                make_input_err!("'cas_store': '{}' does not exist", config.cas_store)
            })?;
            let scheduler = scheduler_map
                .get(&config.scheduler)
                .err_tip(|| {
                    format!(
                        "Scheduler needs config for '{}' because it exists in execution",
                        config.scheduler
                    )
                })?
                .clone();

            instance_infos.insert(
                config.instance_name.to_string(),
                InstanceInfo {
                    scheduler,
                    cas_store,
                },
            );
        }
        Ok(Self { instance_infos })
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
    }

    fn to_execute_stream(
        nl_client_operation_id: &NativelinkOperationId,
        action_listener: Box<dyn ActionStateResult>,
    ) -> impl Stream<Item = Result<Operation, Status>> + Send + use<> {
        let client_operation_id = OperationId::from(nl_client_operation_id.to_string());
        unfold(Some(action_listener), move |maybe_action_listener| {
            let client_operation_id = client_operation_id.clone();
            async move {
                let mut action_listener = maybe_action_listener?;
                match action_listener.changed().await {
                    Ok((action_update, _maybe_origin_metadata)) => {
                        info!(?action_update, "Execute Resp Stream");
                        // If the action is finished we won't be sending any more updates.
                        let maybe_action_listener = if action_update.stage.is_finished() {
                            None
                        } else {
                            Some(action_listener)
                        };
                        Some((
                            Ok(action_update.as_operation(client_operation_id)),
                            maybe_action_listener,
                        ))
                    }
                    Err(err) => {
                        error!(?err, "Error in action_listener stream");
                        Some((Err(err.into()), None))
                    }
                }
            }
        })
    }

    async fn inner_execute(
        &self,
        request: ExecuteRequest,
    ) -> Result<impl Stream<Item = Result<Operation, Status>> + Send + use<>, Error> {
        let instance_name = request.instance_name;

        let instance_info = self
            .instance_infos
            .get(&instance_name)
            .err_tip(|| "Instance name '{}' not configured")?;

        let digest = DigestInfo::try_from(
            request
                .action_digest
                .err_tip(|| "Expected action_digest to exist")?,
        )
        .err_tip(|| "Failed to unwrap action cache")?;

        let priority = request
            .execution_policy
            .map_or(DEFAULT_EXECUTION_PRIORITY, |p| p.priority);

        let action =
            get_and_decode_digest::<Action>(&instance_info.cas_store, digest.into()).await?;
        let action_info = instance_info
            .build_action_info(
                instance_name.clone(),
                digest,
                action,
                priority,
                request.skip_cache_lookup,
                request
                    .digest_function
                    .try_into()
                    .err_tip(|| "Could not convert digest function in inner_execute()")?,
            )
            .await?;

        let action_listener = instance_info
            .scheduler
            .add_action(OperationId::default(), Arc::new(action_info))
            .await
            .err_tip(|| "Failed to schedule task")?;

        Ok(Box::pin(Self::to_execute_stream(
            &NativelinkOperationId::new(
                instance_name,
                action_listener
                    .as_state()
                    .await
                    .err_tip(|| "In ExecutionServer::inner_execute")?
                    .0
                    .client_operation_id
                    .clone(),
            ),
            action_listener,
        )))
    }

    async fn inner_wait_execution(
        &self,
        request: WaitExecutionRequest,
    ) -> Result<impl Stream<Item = Result<Operation, Status>> + Send + use<>, Status> {
        let nl_operation_id = NativelinkOperationId::from_name(&request.name)
            .err_tip(|| "Failed to parse operation_id in ExecutionServer::wait_execution")?;
        let Some(instance_info) = self.instance_infos.get(&nl_operation_id.instance_name) else {
            return Err(Status::not_found(format!(
                "No scheduler with the instance name {}",
                nl_operation_id.instance_name,
            )));
        };
        let Some(rx) = instance_info
            .scheduler
            .filter_operations(OperationFilter {
                client_operation_id: Some(nl_operation_id.client_operation_id.clone()),
                ..Default::default()
            })
            .await
            .err_tip(|| "Error running find_existing_action in ExecutionServer::wait_execution")?
            .next()
            .await
        else {
            return Err(Status::not_found("Failed to find existing task"));
        };
        Ok(Self::to_execute_stream(&nl_operation_id, rx))
    }
}

#[tonic::async_trait]
impl Execution for ExecutionServer {
    type ExecuteStream = ExecuteStream;
    type WaitExecutionStream = ExecuteStream;

    #[instrument(
        err,
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn execute(
        &self,
        grpc_request: Request<ExecuteRequest>,
    ) -> Result<Response<ExecuteStream>, Status> {
        let request = grpc_request.into_inner();

        let digest_function = request.digest_function;
        let result = self
            .inner_execute(request)
            .instrument(error_span!("execution_server_execute"))
            .with_context(
                make_ctx_for_hash_func(digest_function)
                    .err_tip(|| "In ExecutionServer::execute")?,
            )
            .await
            .err_tip(|| "Failed on execute() command")?;

        Ok(Response::new(Box::pin(result)))
    }

    #[instrument(
        err,
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn wait_execution(
        &self,
        grpc_request: Request<WaitExecutionRequest>,
    ) -> Result<Response<ExecuteStream>, Status> {
        let request = grpc_request.into_inner();

        let stream_result = self
            .inner_wait_execution(request)
            .await
            .err_tip(|| "Failed on wait_execution() command")
            .map_err(Into::into);
        let stream = match stream_result {
            Ok(stream) => stream,
            Err(e) => return Err(e),
        };
        debug!(return = "Ok(<stream>)");
        Ok(Response::new(Box::pin(stream)))
    }
}

#[cfg(test)]
#[test]
fn test_nl_op_id_from_name() -> Result<(), Box<dyn core::error::Error>> {
    let examples = [("foo/bar", "foo"), ("a/b/c/d", "a/b/c")];

    for (input, expected) in examples {
        let id = NativelinkOperationId::from_name(input)?;
        assert_eq!(id.instance_name, expected);
    }

    Ok(())
}
