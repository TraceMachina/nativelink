// Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::watch;
use tonic::{transport, Request};

use action_messages::{ActionInfo, ActionState, DEFAULT_EXECUTION_PRIORITY};
use config;
use error::{make_err, Code, Error, ResultExt};
use platform_property_manager::PlatformPropertyManager;
use proto::build::bazel::remote::execution::v2::{
    capabilities_client::CapabilitiesClient, execution_client::ExecutionClient, ExecuteRequest, ExecutionPolicy,
    GetCapabilitiesRequest,
};
use scheduler::ActionScheduler;

pub struct GrpcScheduler {
    scheduler: ExecutionClient<transport::Channel>,
    platform_property_manager: PlatformPropertyManager,
    instance_name: String,
}

impl GrpcScheduler {
    pub async fn new(config: &config::schedulers::GrpcScheduler) -> Result<Self, Error> {
        let endpoint = transport::Endpoint::new(config.endpoint.clone())
            .err_tip(|| format!("Could not parse {} in GrpcScheduler", config.endpoint))?
            .connect()
            .await
            .err_tip(|| format!("Could not connect to {} in GrpcScheduler", config.endpoint))?;

        let mut capabilities_client = CapabilitiesClient::new(endpoint.clone());
        let capabilities = capabilities_client
            .get_capabilities(GetCapabilitiesRequest {
                instance_name: config.instance_name.clone(),
            })
            .await?
            .into_inner();
        let platform_property_manager = PlatformPropertyManager::new(
            capabilities
                .execution_capabilities
                .err_tip(|| {
                    format!(
                        "Unable to get execution properties in GrpcScheduler for {}",
                        config.endpoint
                    )
                })?
                .supported_node_properties
                .iter()
                .map(|property| (property.clone(), config::schedulers::PropertyType::Exact))
                .collect(),
        );

        Ok(GrpcScheduler {
            scheduler: ExecutionClient::new(endpoint),
            platform_property_manager,
            instance_name: config.instance_name.clone(),
        })
    }
}

#[async_trait]
impl ActionScheduler for GrpcScheduler {
    fn get_platform_property_manager(&self) -> &PlatformPropertyManager {
        &self.platform_property_manager
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
            instance_name: self.instance_name.clone(),
            skip_cache_lookup: action_info.skip_cache_lookup,
            action_digest: Some(action_info.digest().into()),
            execution_policy,
            // TODO: Get me from the original request, not very important as we ignore it.
            results_cache_policy: None,
        };
        let mut result_stream = self
            .scheduler
            .clone()
            .execute(Request::new(request))
            .await
            .err_tip(|| "Sending action to upstream scheduler")?
            .into_inner();
        if let Some(initial_response) = result_stream
            .message()
            .await
            .err_tip(|| "Recieving response from upstream scheduler")?
        {
            let (tx, rx) = watch::channel(Arc::new(initial_response.try_into()?));
            tokio::spawn(async move {
                while let Ok(response) = result_stream.message().await {
                    if let Some(response) = response {
                        if let Ok(response) = response.try_into() {
                            if tx.send(Arc::new(response)).is_err() {
                                break;
                            }
                        }
                    } else {
                        break;
                    }
                }
            });
            return Ok(rx);
        }
        Err(make_err!(Code::Internal, "Upstream scheduler didn't accept action."))
    }
}
