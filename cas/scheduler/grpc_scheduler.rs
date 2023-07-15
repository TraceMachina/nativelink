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

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio::sync::watch;
use tonic::{transport, Request};

use action_messages::{ActionInfo, ActionState, DEFAULT_EXECUTION_PRIORITY};
use common::log;
use error::{make_err, Code, Error, ResultExt};
use platform_property_manager::PlatformPropertyManager;
use proto::build::bazel::remote::execution::v2::{
    capabilities_client::CapabilitiesClient, execution_client::ExecutionClient, ExecuteRequest, ExecutionPolicy,
    GetCapabilitiesRequest,
};
use scheduler::ActionScheduler;

pub struct GrpcScheduler {
    endpoint: transport::Channel,
    platform_property_managers: Mutex<HashMap<String, Arc<PlatformPropertyManager>>>,
}

impl GrpcScheduler {
    pub async fn new(config: &config::schedulers::GrpcScheduler) -> Result<Self, Error> {
        let endpoint = transport::Endpoint::new(config.endpoint.clone())
            .err_tip(|| format!("Could not parse {} in GrpcScheduler", config.endpoint))?
            .connect()
            .await
            .err_tip(|| format!("Could not connect to {} in GrpcScheduler", config.endpoint))?;

        Ok(Self {
            endpoint,
            platform_property_managers: Mutex::new(HashMap::new()),
        })
    }
}

#[async_trait]
impl ActionScheduler for GrpcScheduler {
    async fn get_platform_property_manager(&self, instance_name: &str) -> Result<Arc<PlatformPropertyManager>, Error> {
        if let Some(platform_property_manager) = self.platform_property_managers.lock().get(instance_name) {
            return Ok(platform_property_manager.clone());
        }

        // Not in the cache, lookup the capabilities with the upstream.
        let mut capabilities_client = CapabilitiesClient::new(self.endpoint.clone());
        let capabilities = capabilities_client
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
                .map(|property| (property.clone(), config::schedulers::PropertyType::Exact))
                .collect(),
        ));

        self.platform_property_managers
            .lock()
            .insert(instance_name.to_string(), platform_property_manager.clone());
        Ok(platform_property_manager)
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
        };
        let mut result_stream = ExecutionClient::new(self.endpoint.clone())
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
                while let Ok(Some(response)) = result_stream.message().await {
                    match response.try_into() {
                        Ok(response) => {
                            if let Err(err) = tx.send(Arc::new(response)) {
                                log::info!("Client disconnected in GrpcScheduler: {}", err);
                                return;
                            }
                        }
                        Err(err) => log::error!("Error converting response to watch in GrpcScheduler: {}", err),
                    }
                }
            });
            return Ok(rx);
        }
        Err(make_err!(Code::Internal, "Upstream scheduler didn't accept action."))
    }
}
