// Copyright 2024 Trace Machina, Inc. All rights reserved.
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
use nativelink_error::{Error, ErrorContext, ResultExt, make_input_err};
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
use nativelink_proto::google::rpc::{
    PreconditionFailure, Status as GrpcStatusProto, precondition_failure,
};
use nativelink_scheduler::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_store::ac_utils::get_and_decode_digest;
use nativelink_store::existence_cache_store::ExistenceCacheStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionState, ActionUniqueKey, ActionUniqueQualifier,
    DEFAULT_EXECUTION_PRIORITY, OperationId, TypeUrl,
};
use nativelink_util::common::{self, DigestInfo};
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc, make_ctx_for_hash_func};
use nativelink_util::metrics::{record_execute_stream_drop, record_execute_stream_heartbeat};
use nativelink_util::operation_state_manager::{ActionStateResult, OperationFilter};
use nativelink_util::origin_event::{BAZEL_METADATA_KEY, request_metadata_from_baggage};
use nativelink_util::spawn;
use nativelink_util::store_trait::{Store, StoreLike};
use nativelink_util::task::JoinHandleDropGuard;
use opentelemetry::Context;
use opentelemetry::baggage::BaggageExt;
use opentelemetry::context::FutureExt;
use opentelemetry_semantic_conventions::attribute::ENDUSER_ID;
use prost::Message as _;
use tonic::{Code, Request, Response, Status};
use tracing::{Instrument, Level, debug, error, error_span, info, instrument, warn};

/// Result of a synchronous `Execute` decision before the async
/// scheduling stream begins. Stream is the happy path; Reject is a
/// client-facing gRPC `Status` returned without going through NL's
/// internal Error/instrumentation pipeline.
/// When an action fails because one of its blobs is missing, stop this
/// instance's existence cache vouching for that blob before the client
/// hears about it. Bazel answers the failure by asking `FindMissingBlobs`
/// again and uploading what it is told is missing; if the cache still
/// says present (the blob left the object store, which sends no removal
/// event, after the cache learned it), nothing is uploaded and the retry
/// fails the same way until the cache entry expires.
async fn forget_missing_input(cas_store: &Store, stage: &ActionStage) {
    let ActionStage::Completed(result) = stage else {
        return;
    };
    let Some(ErrorContext::MissingDigest { hash, size }) =
        result.error.as_ref().map(|err| &err.context)
    else {
        return;
    };
    let Ok(digest) = DigestInfo::try_new(hash, *size) else {
        return;
    };
    if let Some(cache) = cas_store.downcast_ref::<ExistenceCacheStore<SystemTime>>(None) {
        cache.remove_from_cache(&digest).await;
        debug!(%digest, "Action input missing; dropped it from the existence cache");
    }
}

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
    let pf = PreconditionFailure {
        violations: missing
            .iter()
            .map(|(d, ctx)| precondition_failure::Violation {
                r#type: common::VIOLATION_TYPE_MISSING.to_string(),
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
        type_url: PreconditionFailure::TYPE_URL.to_string(),
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
}

/// Splits an operation name into the instance that serves it and the id the
/// scheduler knows, trying the configured instance names longest first. A
/// client operation id may itself carry a `/`: a grpc scheduler hands back the
/// upstream's `<instance>/<uuid>` as the id, so the name a client holds reads
/// `main/main/<uuid>`, and splitting at the last `/` would look for an
/// instance called `main/main`. The instance is the longest configured name
/// the operation name starts with, and the id is whatever follows it.
fn resolve_operation_name<'a>(
    instances: impl IntoIterator<Item = &'a str>,
    name: &str,
) -> Option<(&'a str, OperationId)> {
    let mut instances: Vec<&str> = instances.into_iter().collect();
    instances.sort_by_key(|instance| core::cmp::Reverse(instance.len()));
    instances.into_iter().find_map(|instance| {
        name.strip_prefix(instance)
            .and_then(|rest| rest.strip_prefix('/'))
            .filter(|id| !id.is_empty())
            .map(|id| (instance, OperationId::from(id)))
    })
}

impl fmt::Display for NativelinkOperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.instance_name, self.client_operation_id)
    }
}

#[derive(Clone)]
struct InstanceInfo {
    scheduler: Arc<dyn KnownPlatformPropertyProvider>,
    cas_store: Store,
    buck2_invocation_isolation: bool,
    /// Re-send the current operation state on an otherwise idle `Execute`
    /// or `WaitExecution` stream at this interval. `None` sends no
    /// heartbeats.
    execute_stream_heartbeat: Option<Duration>,
    /// The time an action is queued at.
    now_fn: NowFn,
}

/// The clock queued actions are stamped from.
pub type NowFn = Arc<dyn Fn() -> SystemTime + Send + Sync>;

impl fmt::Debug for InstanceInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InstanceInfo")
            .field("cas_store", &self.cas_store)
            .finish_non_exhaustive()
    }
}

impl InstanceInfo {
    fn execution_scope(&self) -> Option<String> {
        if !self.buck2_invocation_isolation {
            return None;
        }
        let context = Context::current();
        let baggage = context.baggage();
        let metadata =
            request_metadata_from_baggage(baggage.get(BAZEL_METADATA_KEY)?.as_str()).ok()?;
        if metadata.tool_invocation_id.is_empty()
            || !metadata
                .tool_details
                .as_ref()
                .is_some_and(|tool| tool.tool_name.eq_ignore_ascii_case("buck2"))
        {
            return None;
        }
        let identity = baggage
            .get(ENDUSER_ID)
            .map(opentelemetry::StringValue::as_str)
            .unwrap_or_default();
        let mut hash = DigestHasherFunc::Sha256.hasher();
        // Length-prefix the identity so the two fields cannot alias. Use a
        // fixed width so all scheduler architectures compute the same scope.
        hash.update(&(identity.len() as u64).to_be_bytes());
        hash.update(identity.as_bytes());
        hash.update(metadata.tool_invocation_id.as_bytes());
        Some(hash.finalize_digest().packed_hash().to_string())
    }

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
                .ok_or_else(|| make_input_err!("Expected command_digest to exist"))?,
        )
        .err_tip(|| "Could not decode command digest")?;

        let input_root_digest = DigestInfo::try_from(
            action
                .clone()
                .input_root_digest
                .ok_or_else(|| make_input_err!("Expected input_digest_root"))?,
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
            execution_scope: self.execution_scope(),
            instance_name,
            digest_function,
            digest: action_digest,
        };
        // REv2 `Action.do_not_cache`: "the Action's result cannot be cached,
        // and in-flight requests for the same Action may not be merged".
        // `Uncacheable` skips the cache lookup and the in-flight merge.
        let unique_qualifier = if skip_cache_lookup || action.do_not_cache {
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
            insert_timestamp: (self.now_fn)(),
            unique_qualifier,
        })
    }
}

#[derive(Debug, Clone)]
pub struct ExecutionServer {
    instance_infos: HashMap<InstanceName, InstanceInfo>,
}

type ExecuteStream = Pin<Box<dyn Stream<Item = Result<Operation, Status>> + Send>>;

/// Watches an `Execute`/`WaitExecution` response stream so a stream torn
/// down before its operation finished is recorded — usually a client that
/// walked away, but a server shutdown counts too. tonic drops the stream on
/// disconnect without running any handler code, so the `Drop` impl is the
/// only place the server can see it; the operation's fate after this is the
/// scheduler's client keepalive timeout, which the dropped subscriber (or
/// the aborted drive task) stops refreshing.
struct ExecuteStreamMonitor {
    inner: ExecuteStream,
    operation_id: String,
    finished: bool,
}

impl Stream for ExecuteStreamMonitor {
    type Item = Result<Operation, Status>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        let poll = this.inner.as_mut().poll_next(cx);
        if let core::task::Poll::Ready(item) = &poll {
            match item {
                Some(Ok(operation)) if operation.done => this.finished = true,
                Some(Err(_)) | None => this.finished = true,
                Some(Ok(_)) => {}
            }
        }
        poll
    }
}

impl Drop for ExecuteStreamMonitor {
    fn drop(&mut self) {
        if !self.finished {
            info!(
                operation_id = %self.operation_id,
                "Execute stream dropped before the operation finished"
            );
            record_execute_stream_drop();
        }
    }
}

impl ExecutionServer {
    pub fn new(
        configs: &[WithInstanceName<ExecutionConfig>],
        scheduler_map: &HashMap<String, Arc<dyn KnownPlatformPropertyProvider>>,
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
                    buck2_invocation_isolation: config.experimental_buck2_invocation_isolation,
                    execute_stream_heartbeat: (config.execute_stream_heartbeat_s > 0)
                        .then(|| Duration::from_secs(config.execute_stream_heartbeat_s as u64)),
                    now_fn: Arc::new(SystemTime::now),
                },
            );
        }
        Ok(Self { instance_infos })
    }

    /// Stamps queued actions from `now_fn` instead of the wall clock, so
    /// they share the scheduler's clock in a test that injects one.
    #[must_use]
    pub fn with_now_fn(mut self, now_fn: impl Fn() -> SystemTime + Send + Sync + 'static) -> Self {
        let now_fn: NowFn = Arc::new(now_fn);
        for info in self.instance_infos.values_mut() {
            info.now_fn = now_fn.clone();
        }
        self
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
    }

    pub fn into_operations_service(self) -> OperationsServer<Self> {
        OperationsServer::new(self)
    }

    /// Converts one action state into the client's `Operation`, with the
    /// side effects every sent update carries. The bool says whether the
    /// operation is finished.
    async fn operation_from_update(
        action_update: Arc<ActionState>,
        client_operation_id: &OperationId,
        cas_store: &Store,
    ) -> (Operation, bool) {
        debug!(?action_update, "Execute Resp Stream");
        forget_missing_input(cas_store, &action_update.stage).await;
        let finished = action_update.stage.is_finished();
        (
            action_update.as_operation(client_operation_id.clone()),
            finished,
        )
    }

    /// Waits for the next update and converts it for the client. The bool
    /// says whether the stream is over (a finished stage or an error).
    async fn next_operation(
        action_listener: &mut Box<dyn ActionStateResult>,
        client_operation_id: &OperationId,
        cas_store: &Store,
    ) -> (Result<Operation, Status>, bool) {
        match action_listener.changed().await {
            Ok((action_update, _maybe_origin_metadata)) => {
                let (operation, finished) =
                    Self::operation_from_update(action_update, client_operation_id, cas_store)
                        .await;
                (Ok(operation), finished)
            }
            Err(err) => {
                error!(?err, "Error in action_listener stream");
                (Err(err.into()), true)
            }
        }
    }

    fn to_execute_stream(
        nl_client_operation_id: &NativelinkOperationId,
        action_listener: Box<dyn ActionStateResult>,
        cas_store: Store,
        heartbeat_interval: Option<Duration>,
    ) -> impl Stream<Item = Result<Operation, Status>> + Send + use<> {
        struct HeartbeatState {
            rx: tokio::sync::mpsc::Receiver<Result<Operation, Status>>,
            /// The last update sent, re-sent as the heartbeat.
            last_operation: Option<Operation>,
            /// Aborts the driving task when the stream is dropped.
            _drive_guard: JoinHandleDropGuard<()>,
        }
        let client_operation_id = OperationId::from(nl_client_operation_id.to_string());
        let inner: ExecuteStream = if let Some(interval) = heartbeat_interval {
            // `changed()` must be awaited without interruption: subscriber
            // implementations keep their no-progress watchdog (the
            // `no_event_action_timeout` select in the state manager) inside
            // one `changed()` call, so cancelling it on a timer would reset
            // that watchdog forever. A task drives the listener and feeds a
            // channel, whose `recv()` is cancel-safe, and the heartbeat
            // timer races the channel instead.
            let (tx, rx) = tokio::sync::mpsc::channel::<Result<Operation, Status>>(1);
            let task_client_operation_id = client_operation_id.clone();
            let mut action_listener = action_listener;
            // The guard aborts the task when the stream is dropped, so a
            // client disconnect still drops the subscriber and lets its
            // keepalive lapse, exactly as without the heartbeat.
            let drive_guard = spawn!("execute_stream_updates", async move {
                // Send the current state first: it seeds the heartbeat, so
                // the stream is never silent while waiting for the first
                // transition. Subscribers typically re-yield the current
                // state as their first update too; duplicates are permitted.
                match action_listener.as_state().await {
                    Ok((state, _maybe_origin_metadata)) => {
                        let (operation, finished) = Self::operation_from_update(
                            state,
                            &task_client_operation_id,
                            &cas_store,
                        )
                        .await;
                        if tx.send(Ok(operation)).await.is_err() || finished {
                            return;
                        }
                    }
                    Err(err) => {
                        drop(tx.send(Err(err.into())).await);
                        return;
                    }
                }
                loop {
                    let (item, finished) = Self::next_operation(
                        &mut action_listener,
                        &task_client_operation_id,
                        &cas_store,
                    )
                    .await;
                    if tx.send(item).await.is_err() || finished {
                        return;
                    }
                }
            });
            Box::pin(unfold(
                Some(HeartbeatState {
                    rx,
                    last_operation: None,
                    _drive_guard: drive_guard,
                }),
                move |maybe_state| async move {
                    let mut state = maybe_state?;
                    let received = if state.last_operation.is_some() {
                        match tokio::time::timeout(interval, state.rx.recv()).await {
                            Ok(received) => received,
                            // No update within the interval: re-send the last
                            // state. `recv()` is cancel-safe, so no update is
                            // lost, and the listener keeps running in its task.
                            Err(_elapsed) => {
                                let heartbeat =
                                    state.last_operation.clone().expect("checked is_some above");
                                record_execute_stream_heartbeat();
                                return Some((Ok(heartbeat), Some(state)));
                            }
                        }
                    } else {
                        state.rx.recv().await
                    };
                    match received {
                        Some(Ok(operation)) => {
                            let finished = operation.done;
                            if !finished {
                                state.last_operation = Some(operation.clone());
                            }
                            Some((Ok(operation), (!finished).then_some(state)))
                        }
                        Some(Err(err)) => Some((Err(err), None)),
                        // The producer only returns after a terminal item or
                        // when this side is gone, so an empty channel here
                        // means the drive task died (a panic). Surface an
                        // error rather than a clean, successful-looking end.
                        None => Some((
                            Err(Status::internal(
                                "Execute stream update task ended before the operation finished",
                            )),
                            None,
                        )),
                    }
                },
            ))
        } else {
            Box::pin(unfold(
                Some(action_listener),
                move |maybe_action_listener| {
                    let client_operation_id = client_operation_id.clone();
                    let cas_store = cas_store.clone();
                    async move {
                        let mut action_listener = maybe_action_listener?;
                        let (item, finished) = Self::next_operation(
                            &mut action_listener,
                            &client_operation_id,
                            &cas_store,
                        )
                        .await;
                        Some((item, (!finished).then_some(action_listener)))
                    }
                },
            ))
        };
        ExecuteStreamMonitor {
            inner,
            operation_id: nl_client_operation_id.to_string(),
            finished: false,
        }
    }

    async fn inner_execute(
        &self,
        request: ExecuteRequest,
    ) -> Result<ExecuteOutcome<impl Stream<Item = Result<Operation, Status>> + Send + use<>>, Error>
    {
        let instance_name = request.instance_name;

        let instance_info = self.instance_infos.get(&instance_name).ok_or_else(|| {
            make_input_err!("'instance_name' not configured for '{instance_name}'")
        })?;

        let digest = DigestInfo::try_from(
            request
                .action_digest
                .ok_or_else(|| make_input_err!("Expected action_digest to exist"))?,
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
            instance_info.cas_store.clone(),
            instance_info.execute_stream_heartbeat,
        )))
    }

    async fn inner_wait_execution(
        &self,
        request: WaitExecutionRequest,
    ) -> Result<impl Stream<Item = Result<Operation, Status>> + Send + use<>, Status> {
        let Some((instance_name, client_operation_id)) = resolve_operation_name(
            self.instance_infos.keys().map(String::as_str),
            &request.name,
        ) else {
            return Err(Status::not_found(format!(
                "No scheduler serves the instance of operation {}",
                request.name,
            )));
        };
        let nl_operation_id =
            NativelinkOperationId::new(instance_name.to_string(), client_operation_id);
        let instance_info = self
            .instance_infos
            .get(instance_name)
            .ok_or_else(|| Status::internal("Instance resolved from its own map is missing"))?;
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
        Ok(Self::to_execute_stream(
            &nl_operation_id,
            rx,
            instance_info.cas_store.clone(),
            instance_info.execute_stream_heartbeat,
        ))
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
fn test_resolve_operation_name() {
    let instances = ["main", "", "a/b/c"];
    let examples = [
        ("main/bar", Some(("main", "bar"))),
        // The id a grpc scheduler hands back carries the upstream's instance.
        ("main/main/bar", Some(("main", "main/bar"))),
        ("/bar", Some(("", "bar"))),
        ("a/b/c/d", Some(("a/b/c", "d"))),
        // Not an instance this server has, and not the default one either.
        ("other/bar", None),
        // A prefix that is not a whole instance name.
        ("mainline/bar", None),
        ("main/", None),
    ];

    for (input, expected) in examples {
        let resolved = resolve_operation_name(instances, input)
            .map(|(instance, id)| (instance, id.to_string()));
        assert_eq!(
            resolved.as_ref().map(|(i, id)| (*i, id.as_str())),
            expected,
            "{input}"
        );
    }
}
