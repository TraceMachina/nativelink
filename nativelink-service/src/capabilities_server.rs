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

use std::collections::HashMap;
use std::sync::Arc;

use nativelink_config::cas_server::{CapabilitiesConfig, InstanceName, WithInstanceName};
use nativelink_error::{Error, ResultExt};
use nativelink_proto::build::bazel::remote::execution::v2::capabilities_server::{
    Capabilities, CapabilitiesServer as Server,
};
use nativelink_proto::build::bazel::remote::execution::v2::digest_function::Value as DigestFunction;
use nativelink_proto::build::bazel::remote::execution::v2::priority_capabilities::PriorityRange;
use nativelink_proto::build::bazel::remote::execution::v2::symlink_absolute_path_strategy::Value as SymlinkAbsolutePathStrategy;
use nativelink_proto::build::bazel::remote::execution::v2::{
    ActionCacheUpdateCapabilities, CacheCapabilities, ExecutionCapabilities,
    GetCapabilitiesRequest, PriorityCapabilities, ServerCapabilities,
};
use nativelink_proto::build::bazel::semver::SemVer;
use nativelink_util::digest_hasher::default_digest_hasher_func;
use nativelink_util::operation_state_manager::ClientStateManager;
use tonic::{Request, Response, Status};
use tracing::{Level, instrument, warn};

const MAX_BATCH_TOTAL_SIZE: i64 = 64 * 1024;

#[derive(Debug, Default)]
pub struct CapabilitiesServer {
    supported_node_properties_for_instance: HashMap<InstanceName, Vec<String>>,
}

impl CapabilitiesServer {
    pub async fn new(
        configs: &[WithInstanceName<CapabilitiesConfig>],
        scheduler_map: &HashMap<String, Arc<dyn ClientStateManager>>,
    ) -> Result<Self, Error> {
        let mut supported_node_properties_for_instance = HashMap::new();
        for config in configs {
            let mut properties = Vec::new();
            if let Some(remote_execution_cfg) = &config.remote_execution {
                let scheduler =
                    scheduler_map
                        .get(&remote_execution_cfg.scheduler)
                        .err_tip(|| {
                            format!(
                                "Scheduler needs config for '{}' because it exists in capabilities",
                                remote_execution_cfg.scheduler
                            )
                        })?;
                if let Some(props_provider) = scheduler.as_known_platform_property_provider() {
                    for platform_key in props_provider
                        .get_known_properties(&config.instance_name)
                        .await
                        .err_tip(|| {
                            format!(
                                "Failed to get platform properties for {}",
                                config.instance_name
                            )
                        })?
                    {
                        properties.push(platform_key.clone());
                    }
                } else {
                    warn!(
                        "Scheduler '{}' does not implement KnownPlatformPropertyProvider",
                        remote_execution_cfg.scheduler
                    );
                }
            }
            supported_node_properties_for_instance.insert(config.instance_name.clone(), properties);
        }
        Ok(Self {
            supported_node_properties_for_instance,
        })
    }

    pub fn into_service(self) -> Server<Self> {
        Server::new(self)
    }
}

#[tonic::async_trait]
impl Capabilities for CapabilitiesServer {
    #[instrument(
        err,
        ret(level = Level::INFO),
        level = Level::ERROR,
        skip_all,
        fields(request = ?grpc_request.get_ref())
    )]
    async fn get_capabilities(
        &self,
        grpc_request: Request<GetCapabilitiesRequest>,
    ) -> Result<Response<ServerCapabilities>, Status> {
        let request = grpc_request.into_inner();

        let instance_name = request.instance_name;
        let maybe_supported_node_properties = self
            .supported_node_properties_for_instance
            .get(&instance_name);
        let execution_capabilities =
            maybe_supported_node_properties.map(|props_for_instance| ExecutionCapabilities {
                digest_function: default_digest_hasher_func().proto_digest_func().into(),
                exec_enabled: true, // TODO(aaronmondal) Make this configurable.
                execution_priority_capabilities: Some(PriorityCapabilities {
                    priorities: vec![PriorityRange {
                        min_priority: 0,
                        max_priority: i32::MAX,
                    }],
                }),
                supported_node_properties: props_for_instance.clone(),
                digest_functions: vec![
                    DigestFunction::Sha256.into(),
                    DigestFunction::Blake3.into(),
                ],
            });

        let resp = ServerCapabilities {
            cache_capabilities: Some(CacheCapabilities {
                digest_functions: vec![
                    DigestFunction::Sha256.into(),
                    DigestFunction::Blake3.into(),
                ],
                action_cache_update_capabilities: Some(ActionCacheUpdateCapabilities {
                    update_enabled: true,
                }),
                cache_priority_capabilities: None,
                max_batch_total_size_bytes: MAX_BATCH_TOTAL_SIZE,
                symlink_absolute_path_strategy: SymlinkAbsolutePathStrategy::Disallowed.into(),
                supported_compressors: vec![],
                supported_batch_update_compressors: vec![],
            }),
            execution_capabilities,
            deprecated_api_version: None,
            low_api_version: Some(SemVer {
                major: 2,
                minor: 0,
                patch: 0,
                prerelease: String::new(),
            }),
            high_api_version: Some(SemVer {
                major: 2,
                minor: 3,
                patch: 0,
                prerelease: String::new(),
            }),
        };
        Ok(Response::new(resp))
    }
}
