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

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::{watch, OnceCell};
use tonic::{transport, Request};

use action_messages::{ActionInfo, ActionState, DEFAULT_EXECUTION_PRIORITY};
use common::log;
use config;
use error::{make_err, Code, Error, ResultExt};
use platform_property_manager::PlatformPropertyManager;
use proto::build::bazel::remote::execution::v2::{
    capabilities_client::CapabilitiesClient, execution_client::ExecutionClient, ExecuteRequest, ExecutionPolicy,
    GetCapabilitiesRequest,
};
use scheduler::ActionScheduler;

/// We have to statically assign OnceCells for each instance_name that we forward
/// to because storing them in a dynamic structure breaks the lifetime requirements
/// as they could be removed.  Since we don't remove them ever we could add an
/// unsafe block, but that seems unwise.  Instead, we statically assign storage
/// for this many instance names (when 1 is probably sufficient anyway) and do
/// away with a HashMap.
const MAXIMUM_PROPERTY_MANAGERS: usize = 5;

pub struct GrpcScheduler {
    endpoint: transport::Channel,
    platform_property_managers: [(Mutex<Option<String>>, OnceCell<PlatformPropertyManager>); MAXIMUM_PROPERTY_MANAGERS],
}

fn empty_property_manager() -> (Mutex<Option<String>>, OnceCell<PlatformPropertyManager>) {
    (Mutex::new(None), OnceCell::new())
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
            platform_property_managers: [
                empty_property_manager(),
                empty_property_manager(),
                empty_property_manager(),
                empty_property_manager(),
                empty_property_manager(),
            ],
        })
    }

    fn get_cached_platform_property_manager(&self, instance_name: &str) -> Option<&PlatformPropertyManager> {
        self.platform_property_managers
            .iter()
            .find(|(name, _)| name.lock().unwrap().as_ref().is_some_and(|name| name == instance_name))
            .and_then(|(_, cached_manager)| cached_manager.get())
    }
}

#[async_trait]
impl ActionScheduler for GrpcScheduler {
    async fn get_platform_property_manager(&self, instance_name: &str) -> Result<&PlatformPropertyManager, Error> {
        if let Some(&ref platform_property_manager) = self.get_cached_platform_property_manager(&instance_name) {
            return Ok(&platform_property_manager);
        }

        // Not in the cache, lookup the capabilities with the upstream.
        let mut capabilities_client = CapabilitiesClient::new(self.endpoint.clone());
        let capabilities = capabilities_client
            .get_capabilities(GetCapabilitiesRequest {
                instance_name: instance_name.to_string(),
            })
            .await?
            .into_inner();
        let platform_property_manager = PlatformPropertyManager::new(
            capabilities
                .execution_capabilities
                .err_tip(|| "Unable to get execution properties in GrpcScheduler")?
                .supported_node_properties
                .iter()
                .map(|property| (property.clone(), config::schedulers::PropertyType::Exact))
                .collect(),
        );

        if let Some((_, manager_cell)) = self.platform_property_managers.iter().find(|(name, _)| {
            let mut name = name.lock().unwrap();
            if name.is_none() {
                *name = Some(instance_name.to_string());
                true
            } else {
                false
            }
        }) {
            let _ = manager_cell.set(platform_property_manager);
            Ok(manager_cell.get().unwrap())
        } else {
            Err(make_err!(
                Code::Internal,
                "platform_property_managers cache slots exhausted"
            ))
        }
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
            instance_name: action_info.instance_name.clone(),
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
