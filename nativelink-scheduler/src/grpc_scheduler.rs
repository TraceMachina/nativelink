// Copyright 2023 The Native Link Authors. All rights reserved.
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
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::unfold;
use futures::TryFutureExt;
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_proto::build::bazel::remote::execution::v2::capabilities_client::CapabilitiesClient;
use nativelink_proto::build::bazel::remote::execution::v2::execution_client::ExecutionClient;
use nativelink_proto::build::bazel::remote::execution::v2::{
    digest_function, ExecuteRequest, ExecutionPolicy, GetCapabilitiesRequest, WaitExecutionRequest,
};
use nativelink_proto::google::longrunning::Operation;
use nativelink_util::action_messages::{ActionInfo, ActionInfoHashKey, ActionState, DEFAULT_EXECUTION_PRIORITY};
use nativelink_util::retry::{ExponentialBackoff, Retrier, RetryResult};
use parking_lot::Mutex;
use rand::rngs::OsRng;
use rand::Rng;
use tokio::select;
use tokio::sync::watch;
use tokio::time::sleep;
use tonic::{transport, Request, Streaming};
use tracing::{error, info, warn};

use crate::action_scheduler::ActionScheduler;
use crate::platform_property_manager::PlatformPropertyManager;

pub struct GrpcScheduler {
    capabilities_client: CapabilitiesClient<transport::Channel>,
    execution_client: ExecutionClient<transport::Channel>,
    platform_property_managers: Mutex<HashMap<String, Arc<PlatformPropertyManager>>>,
    jitter_fn: Box<dyn Fn(Duration) -> Duration + Send + Sync>,
    retry: nativelink_config::stores::Retry,
    retrier: Retrier,
}

impl GrpcScheduler {
    pub fn new(config: &nativelink_config::schedulers::GrpcScheduler) -> Result<Self, Error> {
        let jitter_amt = config.retry.jitter;
        Self::new_with_jitter(
            config,
            Box::new(move |delay: Duration| {
                if jitter_amt == 0. {
                    return delay;
                }
                let min = 1. - (jitter_amt / 2.);
                let max = 1. + (jitter_amt / 2.);
                delay.mul_f32(OsRng.gen_range(min..max))
            }),
        )
    }

    pub fn new_with_jitter(
        config: &nativelink_config::schedulers::GrpcScheduler,
        jitter_fn: Box<dyn Fn(Duration) -> Duration + Send + Sync>,
    ) -> Result<Self, Error> {
        let endpoint = transport::Channel::balance_list(std::iter::once(
            transport::Endpoint::new(config.endpoint.clone())
                .err_tip(|| format!("Could not parse {} in GrpcScheduler", config.endpoint))?,
        ));

        Ok(Self {
            capabilities_client: CapabilitiesClient::new(endpoint.clone()),
            execution_client: ExecutionClient::new(endpoint),
            platform_property_managers: Mutex::new(HashMap::new()),
            jitter_fn,
            retry: config.retry.clone(),
            retrier: Retrier::new(Arc::new(|duration| Box::pin(sleep(duration)))),
        })
    }

    async fn perform_request<F, Fut, R, I>(&self, input: I, mut request: F) -> Result<R, Error>
    where
        F: FnMut(I) -> Fut + Send + Copy,
        Fut: Future<Output = Result<R, Error>> + Send,
        R: Send,
        I: Send + Clone,
    {
        let retry_config = ExponentialBackoff::new(Duration::from_millis(self.retry.delay as u64))
            .map(|d| (self.jitter_fn)(d))
            .take(self.retry.max_retries); // Remember this is number of retries, so will run max_retries + 1.
        self.retrier
            .retry(
                retry_config,
                unfold(input, move |input| async move {
                    let input_clone = input.clone();
                    Some((
                        request(input_clone)
                            .await
                            .map_or_else(RetryResult::Retry, RetryResult::Ok),
                        input,
                    ))
                }),
            )
            .await
    }

    async fn stream_state(mut result_stream: Streaming<Operation>) -> Result<watch::Receiver<Arc<ActionState>>, Error> {
        if let Some(initial_response) = result_stream
            .message()
            .await
            .err_tip(|| "Recieving response from upstream scheduler")?
        {
            let (tx, rx) = watch::channel(Arc::new(initial_response.try_into()?));
            tokio::spawn(async move {
                loop {
                    select!(
                        _ = tx.closed() => {
                            info!("Client disconnected in GrpcScheduler");
                            return;
                        }
                        response = result_stream.message() => {
                            // When the upstream closes the channel, close the
                            // downstream too.
                            let Ok(Some(response)) = response else {
                                return;
                            };
                            match response.try_into() {
                                Ok(response) => {
                                    if let Err(err) = tx.send(Arc::new(response)) {
                                        info!("Client disconnected in GrpcScheduler: {}", err);
                                        return;
                                    }
                                }
                                Err(err) => error!("Error converting response to ActionState in GrpcScheduler: {}", err),
                            }
                        }
                    )
                }
            });
            return Ok(rx);
        }
        Err(make_err!(Code::Internal, "Upstream scheduler didn't accept action."))
    }
}

#[async_trait]
impl ActionScheduler for GrpcScheduler {
    async fn get_platform_property_manager(&self, instance_name: &str) -> Result<Arc<PlatformPropertyManager>, Error> {
        if let Some(platform_property_manager) = self.platform_property_managers.lock().get(instance_name) {
            return Ok(platform_property_manager.clone());
        }

        self.perform_request(instance_name, |instance_name| async move {
            // Not in the cache, lookup the capabilities with the upstream.
            let capabilities = self
                .capabilities_client
                .clone()
                .get_capabilities(GetCapabilitiesRequest {
                    instance_name: instance_name.to_string(),
                })
                .await?
                .into_inner();
            let platform_property_manager = Arc::new(PlatformPropertyManager::new(
                capabilities
                    .execution_capabilities
                    .err_tip(|| "Unable to get execution properties in GrpcScheduler")?
                    .supported_node_properties
                    .iter()
                    .map(|property| (property.clone(), nativelink_config::schedulers::PropertyType::exact))
                    .collect(),
            ));

            self.platform_property_managers
                .lock()
                .insert(instance_name.to_string(), platform_property_manager.clone());
            Ok(platform_property_manager)
        })
        .await
    }

    async fn add_action(&self, action_info: ActionInfo) -> Result<watch::Receiver<Arc<ActionState>>, Error> {
        let execution_policy = if action_info.priority == DEFAULT_EXECUTION_PRIORITY {
            None
        } else {
            Some(ExecutionPolicy {
                priority: action_info.priority,
            })
        };
        let request = ExecuteRequest {
            instance_name: action_info.instance_name().clone(),
            skip_cache_lookup: action_info.skip_cache_lookup,
            action_digest: Some(action_info.digest().into()),
            execution_policy,
            // TODO: Get me from the original request, not very important as we ignore it.
            results_cache_policy: None,
            digest_function: digest_function::Value::Sha256.into(),
        };
        let result_stream = self
            .perform_request(request, |request| async move {
                self.execution_client
                    .clone()
                    .execute(Request::new(request))
                    .await
                    .err_tip(|| "Sending action to upstream scheduler")
            })
            .await?
            .into_inner();
        Self::stream_state(result_stream).await
    }

    async fn find_existing_action(
        &self,
        unique_qualifier: &ActionInfoHashKey,
    ) -> Option<watch::Receiver<Arc<ActionState>>> {
        let request = WaitExecutionRequest {
            name: unique_qualifier.action_name(),
        };
        let result_stream = self
            .perform_request(request, |request| async move {
                self.execution_client
                    .clone()
                    .wait_execution(Request::new(request))
                    .await
                    .err_tip(|| "While getting wait_execution stream")
            })
            .and_then(|result_stream| Self::stream_state(result_stream.into_inner()))
            .await;
        match result_stream {
            Ok(result_stream) => Some(result_stream),
            Err(err) => {
                warn!("Error response looking up action with upstream scheduler: {}", err);
                None
            }
        }
    }

    async fn clean_recently_completed_actions(&self) {}
}
