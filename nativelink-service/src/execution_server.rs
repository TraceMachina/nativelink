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
use std::fmt;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
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
use nativelink_proto::google::longrunning::operations_server::{Operations, OperationsServer};
use nativelink_proto::google::longrunning::{
    CancelOperationRequest, DeleteOperationRequest, GetOperationRequest, ListOperationsRequest,
    ListOperationsResponse, Operation, WaitOperationRequest,
};
use nativelink_proto::google::rpc::Status as GrpcStatusProto;
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
use nativelink_util::precondition_failure;
use nativelink_util::store_trait::{Store, StoreLike};
use opentelemetry::context::FutureExt;
use prost::Message as _;
use tonic::{Code, Request, Response, Status};
use tracing::{Instrument, Level, debug, error, error_span, instrument, warn};

/// Result of a synchronous `Execute` decision before the async
/// scheduling stream begins. Stream is the happy path; Reject is a
/// client-facing gRPC `Status` returned without going through NL's
/// internal Error/instrumentation pipeline.
enum ExecuteOutcome<S> {
    Stream(S),
    Reject(Status),
}

/// Build a tonic [`Status`] of code `FAILED_PRECONDITION` whose details
/// carry a `google.rpc.PreconditionFailure` listing the missing CAS
/// blobs.
///
/// The pre-check that calls this is intentionally shallow: it only
/// validates the top-level Action proto, `command_digest`, and
/// `input_root_digest`. Nested Directory protos and file contents
/// under the input root are not walked here — the worker path fetches
/// those lazily and reports them via the same mechanism (see
/// `action_messages::to_execute_response`, which dispatches on
/// `Error::context`).
///
/// Race note: the pre-check uses `has_many` then the action is
/// scheduled; a blob present at check time may be evicted before the
/// worker fetches it. That case is intentionally not addressed here —
/// the worker path covers it with the same `FAILED_PRECONDITION`
/// surfacing, so Bazel retries either way.
fn missing_blobs_failed_precondition(
    missing: &[(DigestInfo, &'static str)],
    summary: &str,
) -> Status {
    let pf = precondition_failure::PreconditionFailure {
        violations: missing
            .iter()
            .map(|(d, ctx)| precondition_failure::Violation {
                r#type: precondition_failure::VIOLATION_TYPE_MISSING.to_string(),
                // Per REv2, the subject for a missing-blob violation is
                // `blobs/<hash>/<size>` so the client knows exactly
                // which digest to re-upload.
                subject: format!("blobs/{}/{}", d.packed_hash(), d.size_bytes()),
                description: (*ctx).to_string(),
            })
            .collect(),
    };

    // Wrap PreconditionFailure into a google.protobuf.Any.
    let mut pf_buf: Vec<u8> = Vec::with_capacity(pf.encoded_len());
    pf.encode(&mut pf_buf)
        .expect("encoding prost message into Vec<u8> cannot fail");
    let any = prost_types::Any {
        type_url: precondition_failure::TYPE_URL.to_string(),
        value: pf_buf,
    };

    let status_proto = GrpcStatusProto {
        code: Code::FailedPrecondition as i32,
        message: summary.to_string(),
        details: vec![any],
    };
    let mut status_buf: Vec<u8> = Vec::with_capacity(status_proto.encoded_len());
    status_proto
        .encode(&mut status_buf)
        .expect("encoding prost message into Vec<u8> cannot fail");

    Status::with_details(
        Code::FailedPrecondition,
        summary.to_string(),
        Bytes::from(status_buf),
    )
}

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

#[derive(Clone)]
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
            ActionUniqueQualifier::Uncacheable(action_key)
        } else {
            ActionUniqueQualifier::Cacheable(action_key)
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

#[derive(Debug, Clone)]
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
                config.instance_name.clone(),
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

    pub fn into_operations_service(self) -> OperationsServer<Self> {
        OperationsServer::new(self)
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
                        debug!(?action_update, "Execute Resp Stream");
                        Some((
                            Ok(action_update.as_operation(client_operation_id)),
                            (!action_update.stage.is_finished()).then_some(action_listener),
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
    ) -> Result<ExecuteOutcome<impl Stream<Item = Result<Operation, Status>> + Send + use<>>, Error>
    {
        let instance_name = request.instance_name;

        let instance_info = self
            .instance_infos
            .get(&instance_name)
            .err_tip(|| format!("'instance_name' not configured for '{instance_name}'"))?;

        let digest = DigestInfo::try_from(
            request
                .action_digest
                .err_tip(|| "Expected action_digest to exist")?,
        )
        .err_tip(|| "Failed to unwrap action cache")?;

        let priority = request
            .execution_policy
            .map_or(DEFAULT_EXECUTION_PRIORITY, |p| p.priority);

        let action = match get_and_decode_digest::<Action>(&instance_info.cas_store, digest.into())
            .await
        {
            Ok(a) => a,
            Err(e) if e.code == Code::NotFound => {
                warn!(
                    %digest,
                    %e,
                    "Execute: Action proto missing from CAS; returning FAILED_PRECONDITION with PreconditionFailure detail so Bazel can re-upload"
                );
                let summary = format!(
                    "Action {digest} is missing from CAS; client should re-upload it and retry"
                );
                return Ok(ExecuteOutcome::Reject(missing_blobs_failed_precondition(
                    &[(digest, "Action")],
                    &summary,
                )));
            }
            Err(e) => return Err(e).err_tip(|| "Decoding Action proto in Execute")?,
        };

        let action_command_digest = action
            .command_digest
            .as_ref()
            .map(|d| DigestInfo::try_from(d.clone()))
            .transpose()
            .err_tip(|| "Failed to parse command_digest from Action")?;
        let action_input_root_digest = action
            .input_root_digest
            .as_ref()
            .map(|d| DigestInfo::try_from(d.clone()))
            .transpose()
            .err_tip(|| "Failed to parse input_root_digest from Action")?;
        let mut blobs_to_check: Vec<DigestInfo> = Vec::with_capacity(2);
        if let Some(d) = action_command_digest {
            blobs_to_check.push(d);
        }
        if let Some(d) = action_input_root_digest {
            blobs_to_check.push(d);
        }
        if !blobs_to_check.is_empty() {
            let store_keys: Vec<_> = blobs_to_check.iter().map(|d| (*d).into()).collect();
            let sizes = instance_info
                .cas_store
                .has_many(&store_keys)
                .await
                .err_tip(|| "Validating Action input blobs in CAS")?;
            let mut missing: Vec<(DigestInfo, &'static str)> = Vec::new();
            for ((digest, present), label) in blobs_to_check
                .iter()
                .zip(sizes.iter())
                .zip(["Action.command_digest", "Action.input_root_digest"].iter())
            {
                if present.is_none() {
                    missing.push((*digest, label));
                }
            }
            if !missing.is_empty() {
                warn!(
                    ?missing,
                    %digest,
                    "Execute pre-check found missing CAS blobs; returning FAILED_PRECONDITION with PreconditionFailure detail so Bazel can re-upload"
                );
                let summary = format!(
                    "{} CAS blob(s) referenced by action {} are missing; client should re-upload them and retry",
                    missing.len(),
                    digest,
                );
                return Ok(ExecuteOutcome::Reject(missing_blobs_failed_precondition(
                    &missing, &summary,
                )));
            }
        }

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

        Ok(ExecuteOutcome::Stream(Self::to_execute_stream(
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

        match result {
            ExecuteOutcome::Stream(stream) => Ok(Response::new(Box::pin(stream))),
            ExecuteOutcome::Reject(status) => Err(status),
        }
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

#[tonic::async_trait]
impl Operations for ExecutionServer {
    async fn list_operations(
        &self,
        _request: Request<ListOperationsRequest>,
    ) -> Result<Response<ListOperationsResponse>, Status> {
        Err(Status::unimplemented("list_operations not implemented"))
    }

    async fn delete_operation(
        &self,
        _request: Request<DeleteOperationRequest>,
    ) -> Result<Response<()>, Status> {
        Err(Status::unimplemented("delete_operation not implemented"))
    }

    async fn cancel_operation(
        &self,
        _request: Request<CancelOperationRequest>,
    ) -> Result<Response<()>, Status> {
        Err(Status::unimplemented("cancel_operation not implemented"))
    }

    async fn get_operation(
        &self,
        request: Request<GetOperationRequest>,
    ) -> Result<Response<Operation>, Status> {
        let inner_request = request.into_inner();

        let mut stream = Box::pin(
            self.inner_wait_execution(WaitExecutionRequest {
                name: inner_request.name,
            })
            .await?,
        );

        let operation = stream
            .next()
            .await
            .ok_or_else(|| Status::not_found("Operation not found"))??;

        Ok(Response::new(operation))
    }

    async fn wait_operation(
        &self,
        request: Request<WaitOperationRequest>,
    ) -> Result<Response<Operation>, Status> {
        let inner_request = request.into_inner();
        let timeout_opt = inner_request.timeout.map(|d| {
            let secs = u64::try_from(d.seconds).unwrap_or(0);
            let nanos = u32::try_from(d.nanos).unwrap_or(0);
            Duration::new(secs, nanos)
        });

        let mut stream = Box::pin(
            self.inner_wait_execution(WaitExecutionRequest {
                name: inner_request.name,
            })
            .await?,
        );

        let mut last_operation = stream
            .next()
            .await
            .ok_or_else(|| Status::not_found("Operation not found"))??;

        if last_operation.done {
            return Ok(Response::new(last_operation));
        }

        let end_time = timeout_opt.map(|t| tokio::time::Instant::now() + t);

        loop {
            let next_fut = stream.next();
            let next_res = if let Some(end) = end_time {
                match tokio::time::timeout_at(end, next_fut).await {
                    Ok(res) => res,
                    Err(_) => break,
                }
            } else {
                next_fut.await
            };

            match next_res {
                Some(Ok(operation)) => {
                    let is_done = operation.done;
                    last_operation = operation;
                    if is_done {
                        break;
                    }
                }
                Some(Err(e)) => return Err(e),
                None => break,
            }
        }

        Ok(Response::new(last_operation))
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
