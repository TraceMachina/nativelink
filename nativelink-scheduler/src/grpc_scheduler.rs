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

use core::future::Future;
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::unfold;
use futures::{StreamExt, TryFutureExt};
use nativelink_config::schedulers::GrpcSpec;
use nativelink_error::{Code, Error, ResultExt, error_if, make_err};
use nativelink_proto::build::bazel::remote::execution::v2::capabilities_client::CapabilitiesClient;
use nativelink_proto::build::bazel::remote::execution::v2::execution_client::ExecutionClient;
use nativelink_proto::build::bazel::remote::execution::v2::{
    ExecuteRequest, ExecutionPolicy, GetCapabilitiesRequest, WaitExecutionRequest,
};
use nativelink_proto::google::longrunning::Operation;
use nativelink_util::action_messages::{
    ActionInfo, ActionState, ActionUniqueQualifier, DEFAULT_EXECUTION_PRIORITY, OperationId,
};
use nativelink_util::connection_manager::ConnectionManager;
use nativelink_util::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, OperationFilter,
};
use nativelink_util::origin_event::OriginMetadata;
use nativelink_util::retry::{Retrier, RetryResult};
use nativelink_util::{background_spawn, tls_utils};
use opentelemetry::{InstrumentationScope, KeyValue, global, metrics};
use parking_lot::Mutex;
use rand::Rng;
use tokio::select;
use tokio::sync::watch;
use tokio::time::sleep;
use tonic::{Request, Streaming};
use tracing::{Level, debug, error, event, info, instrument};

struct GrpcActionStateResult {
    client_operation_id: OperationId,
    rx: watch::Receiver<Arc<ActionState>>,
}

#[async_trait]
impl ActionStateResult for GrpcActionStateResult {
    async fn as_state(&self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        let mut action_state = self.rx.borrow().clone();
        Arc::make_mut(&mut action_state).client_operation_id = self.client_operation_id.clone();
        // TODO(allada) We currently don't support OriginMetadata in this implementation, but
        // we should.
        Ok((action_state, None))
    }

    async fn changed(&mut self) -> Result<(Arc<ActionState>, Option<OriginMetadata>), Error> {
        self.rx.changed().await.map_err(|_| {
            make_err!(
                Code::Internal,
                "Channel closed in GrpcActionStateResult::changed"
            )
        })?;
        let mut action_state = self.rx.borrow().clone();
        Arc::make_mut(&mut action_state).client_operation_id = self.client_operation_id.clone();
        // TODO(allada) We currently don't support OriginMetadata in this implementation, but
        // we should.
        Ok((action_state, None))
    }

    async fn as_action_info(&self) -> Result<(Arc<ActionInfo>, Option<OriginMetadata>), Error> {
        // TODO(allada) We should probably remove as_action_info()
        // or implement it properly.
        return Err(make_err!(
            Code::Unimplemented,
            "as_action_info not implemented for GrpcActionStateResult::as_action_info"
        ));
    }
}

fn init_metrics() -> GrpcSchedulerMetrics {
    let meter = global::meter_with_scope(InstrumentationScope::builder("grpc_scheduler").build());

    GrpcSchedulerMetrics {
        actions_executed: meter
            .u64_counter("grpc_actions_executed_total")
            .with_description("Total number of actions executed via the GRPC scheduler")
            .build(),
        request_errors: meter
            .u64_counter("grpc_request_errors_total")
            .with_description("Number of errors when making requests to the upstream scheduler")
            .build(),
        request_duration: meter
            .f64_histogram("grpc_request_duration")
            .with_description("Duration of requests to the upstream scheduler")
            .with_unit("ms")
            .build(),
        retried_requests: meter
            .u64_counter("grpc_retried_requests_total")
            .with_description("Number of requests that needed to be retried")
            .build(),
        active_operations: meter
            .i64_up_down_counter("grpc_active_operations")
            .with_description("Number of currently active operations")
            .build(),
    }
}

#[derive(Debug, Clone)]
struct GrpcSchedulerMetrics {
    actions_executed: metrics::Counter<u64>,
    request_errors: metrics::Counter<u64>,
    request_duration: metrics::Histogram<f64>,
    retried_requests: metrics::Counter<u64>,
    active_operations: metrics::UpDownCounter<i64>,
}

#[derive(Debug)]
pub struct GrpcScheduler {
    supported_props: Mutex<HashMap<String, Vec<String>>>,
    retrier: Retrier,
    connection_manager: ConnectionManager,
    metrics: GrpcSchedulerMetrics,
}

impl GrpcScheduler {
    pub fn new(spec: &GrpcSpec) -> Result<Self, Error> {
        let jitter_amt = spec.retry.jitter;
        Self::new_with_jitter(
            spec,
            Box::new(move |delay: Duration| {
                if jitter_amt == 0. {
                    return delay;
                }
                let min = 1. - (jitter_amt / 2.);
                let max = 1. + (jitter_amt / 2.);
                delay.mul_f32(rand::rng().random_range(min..max))
            }),
        )
    }

    #[instrument(
        skip(jitter_fn, spec),
        fields(endpoint = ?spec.endpoint),
        level = "info"
    )]
    pub fn new_with_jitter(
        spec: &GrpcSpec,
        jitter_fn: Box<dyn Fn(Duration) -> Duration + Send + Sync>,
    ) -> Result<Self, Error> {
        let endpoint = tls_utils::endpoint(&spec.endpoint)?;
        let jitter_fn = Arc::new(jitter_fn);
        Ok(Self {
            supported_props: Mutex::new(HashMap::new()),
            retrier: Retrier::new(
                Arc::new(|duration| Box::pin(sleep(duration))),
                jitter_fn.clone(),
                spec.retry.clone(),
            ),
            connection_manager: ConnectionManager::new(
                core::iter::once(endpoint),
                spec.connections_per_endpoint,
                spec.max_concurrent_requests,
                spec.retry.clone(),
                jitter_fn,
            ),
            metrics: init_metrics(),
        })
    }

    #[instrument(skip(self, input, request), level = "debug")]
    async fn perform_request<F, Fut, R, I>(&self, input: I, mut request: F) -> Result<R, Error>
    where
        F: FnMut(I) -> Fut + Send + Copy,
        Fut: Future<Output = Result<R, Error>> + Send,
        R: Send,
        I: Send + Clone,
    {
        let start_time = std::time::Instant::now();
        let result = self
            .retrier
            .retry(unfold(input, move |input| async move {
                let input_clone = input.clone();
                Some((
                    request(input_clone).await.map_or_else(
                        |err| {
                            debug!(?err, "Request failed, will retry");
                            self.metrics.request_errors.add(1, &[]);
                            self.metrics.retried_requests.add(1, &[]);
                            RetryResult::Retry(err)
                        },
                        RetryResult::Ok,
                    ),
                    input,
                ))
            }))
            .await;

        self.metrics.request_duration.record(
            start_time.elapsed().as_millis() as f64,
            &[KeyValue::new("success", result.is_ok().to_string())],
        );

        result
    }

    #[instrument(skip(result_stream), level = "debug")]
    async fn stream_state(
        mut result_stream: Streaming<Operation>,
        metrics: Arc<GrpcSchedulerMetrics>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        if let Some(initial_response) = result_stream
            .message()
            .await
            .err_tip(|| "Recieving response from upstream scheduler")?
        {
            let client_operation_id = OperationId::from(initial_response.name.as_str());
            // Our operation_id is not needed here is just a place holder to recycle existing object.
            // The only thing that actually matters is the operation_id.
            let operation_id = OperationId::default();
            let action_state =
                ActionState::try_from_operation(initial_response, operation_id.clone())
                    .err_tip(|| "In GrpcScheduler::stream_state")?;
            let (tx, mut rx) = watch::channel(Arc::new(action_state));
            rx.mark_changed();

            metrics.active_operations.add(1, &[]);
            let metrics_clone = metrics.clone();

            background_spawn!("grpc_scheduler_stream_state", async move {
                loop {
                    select!(
                        () = tx.closed() => {
                            info!("Client disconnected in GrpcScheduler");
                            metrics_clone.active_operations.add(-1, &[]);
                            return;
                        }
                        response = result_stream.message() => {
                            // When the upstream closes the channel, close the
                            // downstream too.
                            let Ok(Some(response)) = response else {
                                metrics_clone.active_operations.add(-1, &[]);
                                return;
                            };
                            let maybe_action_state = ActionState::try_from_operation(response, operation_id.clone());
                            match maybe_action_state {
                                Ok(response) => {
                                    if let Err(err) = tx.send(Arc::new(response)) {
                                        info!(
                                            ?err,
                                            "Client error in GrpcScheduler"
                                        );
                                        return;
                                    }
                                }
                                Err(err) => {
                                    error!(
                                        ?err,
                                        "Error converting response to ActionState in GrpcScheduler"
                                    );
                                },
                            }
                        }
                    );
                }
            });

            return Ok(Box::new(GrpcActionStateResult {
                client_operation_id,
                rx,
            }));
        }
        Err(make_err!(
            Code::Internal,
            "Upstream scheduler didn't accept action."
        ))
    }

    #[instrument(
        skip(self),
        fields(instance_name = %instance_name),
        level = "debug"
    )]
    async fn inner_get_known_properties(&self, instance_name: &str) -> Result<Vec<String>, Error> {
        if let Some(supported_props) = self.supported_props.lock().get(instance_name) {
            return Ok(supported_props.clone());
        }

        self.perform_request(instance_name, |instance_name| async move {
            // Not in the cache, lookup the capabilities with the upstream.
            let channel = self
                .connection_manager
                .connection()
                .await
                .err_tip(|| "in get_platform_property_manager()")?;
            let capabilities_result = CapabilitiesClient::new(channel)
                .get_capabilities(GetCapabilitiesRequest {
                    instance_name: instance_name.to_string(),
                })
                .await
                .err_tip(|| "Retrieving upstream GrpcScheduler capabilities");
            let capabilities = capabilities_result?.into_inner();
            let supported_props = capabilities
                .execution_capabilities
                .err_tip(|| "Unable to get execution properties in GrpcScheduler")?
                .supported_node_properties
                .into_iter()
                .collect::<Vec<String>>();

            self.supported_props
                .lock()
                .insert(instance_name.to_string(), supported_props.clone());
            Ok(supported_props)
        })
        .await
    }

    #[instrument(
        skip(self, action_info),
        fields(
            client_op_id = ?client_operation_id,
            action_digest = ?action_info.digest()
        ),
        level = "debug"
    )]
    async fn inner_add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        let start_time = std::time::Instant::now();

        let execution_policy = if action_info.priority == DEFAULT_EXECUTION_PRIORITY {
            None
        } else {
            Some(ExecutionPolicy {
                priority: action_info.priority,
            })
        };
        let skip_cache_lookup = match action_info.unique_qualifier {
            ActionUniqueQualifier::Cachable(_) => false,
            ActionUniqueQualifier::Uncachable(_) => true,
        };
        let request = ExecuteRequest {
            instance_name: action_info.instance_name().clone(),
            skip_cache_lookup,
            action_digest: Some(action_info.digest().into()),
            execution_policy,
            // TODO: Get me from the original request, not very important as we ignore it.
            results_cache_policy: None,
            digest_function: action_info
                .unique_qualifier
                .digest_function()
                .proto_digest_func()
                .into(),
        };
        let result = self
            .perform_request(request, |request| async move {
                let channel = self
                    .connection_manager
                    .connection()
                    .await
                    .err_tip(|| "in add_action()")?;
                ExecutionClient::new(channel)
                    .execute(Request::new(request))
                    .await
                    .err_tip(|| "Sending action to upstream scheduler")
            })
            .await;

        match &result {
            Ok(_) => {
                self.metrics.actions_executed.add(1, &[]);
                self.metrics.request_duration.record(
                    start_time.elapsed().as_millis() as f64,
                    &[KeyValue::new("operation", "add_action")],
                );
            }
            Err(err) => {
                self.metrics.request_errors.add(
                    1,
                    &[
                        KeyValue::new("operation", "add_action"),
                        KeyValue::new("error_code", err.code.to_string()),
                    ],
                );
            }
        }

        let result_stream = result?.into_inner();
        Self::stream_state(result_stream, Arc::new(self.metrics.clone())).await
    }

    #[instrument(
        skip(self),
        fields(?filter),
        level = "debug"
    )]
    async fn inner_filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        error_if!(
            filter
                != OperationFilter {
                    client_operation_id: filter.client_operation_id.clone(),
                    ..Default::default()
                },
            "Unsupported filter in GrpcScheduler::filter_operations. Only client_operation_id is supported - {filter:?}"
        );
        let client_operation_id = filter.client_operation_id.ok_or_else(|| {
            make_err!(Code::InvalidArgument, "`client_operation_id` is the only supported filter in GrpcScheduler::filter_operations")
        })?;
        let request = WaitExecutionRequest {
            name: client_operation_id.to_string(),
        };
        let result_stream = self
            .perform_request(request, |request| async move {
                let channel = self
                    .connection_manager
                    .connection()
                    .await
                    .err_tip(|| "in find_by_client_operation_id()")?;
                ExecutionClient::new(channel)
                    .wait_execution(Request::new(request))
                    .await
                    .err_tip(|| "While getting wait_execution stream")
            })
            .and_then(|result_stream| {
                Self::stream_state(result_stream.into_inner(), Arc::new(self.metrics.clone()))
            })
            .await;
        match result_stream {
            Ok(result_stream) => Ok(unfold(
                Some(result_stream),
                |maybe_result_stream| async move { maybe_result_stream.map(|v| (v, None)) },
            )
            .boxed()),
            Err(err) => {
                event!(
                    Level::WARN,
                    ?err,
                    "Error looking up action with upstream scheduler"
                );
                Ok(futures::stream::empty().boxed())
            }
        }
    }
}

#[async_trait]
impl ClientStateManager for GrpcScheduler {
    #[instrument(
        skip(self, action_info),
        fields(
            client_op_id = ?client_operation_id,
            action_digest = ?action_info.digest()
        ),
        level = "debug"
    )]
    async fn add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        self.inner_add_action(client_operation_id, action_info)
            .await
    }

    #[instrument(
        skip(self),
        fields(?filter),
        level = "debug"
    )]
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
impl KnownPlatformPropertyProvider for GrpcScheduler {
    #[instrument(
        skip(self),
        fields(instance_name = %instance_name),
        level = "debug"
    )]
    async fn get_known_properties(&self, instance_name: &str) -> Result<Vec<String>, Error> {
        self.inner_get_known_properties(instance_name).await
    }
}
