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

use std::collections::HashMap;
use std::sync::Arc;

use nativelink_config::cas_server::{
    CapabilitiesConfig, CasStoreConfig, InstanceName, WithInstanceName,
};
use nativelink_error::{Error, ResultExt};
use nativelink_proto::build::bazel::remote::execution::v2::capabilities_server::{
    Capabilities, CapabilitiesServer as Server,
};
use nativelink_proto::build::bazel::remote::execution::v2::digest_function::Value as DigestFunction;
use nativelink_proto::build::bazel::remote::execution::v2::priority_capabilities::PriorityRange;
use nativelink_proto::build::bazel::remote::execution::v2::symlink_absolute_path_strategy::Value as SymlinkAbsolutePathStrategy;
use nativelink_proto::build::bazel::remote::execution::v2::{
    ActionCacheUpdateCapabilities, CacheCapabilities, ExecutionCapabilities, FastCdc2020Params,
    GetCapabilitiesRequest, PriorityCapabilities, ServerCapabilities, compressor,
};
use nativelink_proto::build::bazel::semver::SemVer;
use nativelink_scheduler::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::digest_hasher::default_digest_hasher_func;
use nativelink_util::store_trait::StoreLike;
use tonic::{Request, Response, Status};
use tracing::{Level, instrument};

use crate::wire_compression::RemoteCacheCompressionInstances;

/// `max_batch_total_size_bytes` counts only blob data, while the request also
/// carries each blob's digest and framing (about 80 bytes a blob). Advertising
/// 1/16 less than the listener accepts leaves room for that framing on
/// batches of thousands of small blobs.
/// `max_cas_blob_size_bytes` for a store's `max_upload_size()`. That is
/// `u64::MAX` when the store knows no limit, which every wrapper store
/// reports; REAPI spells "no limit" as 0.
fn advertised_max_cas_blob_size(max_upload_size: u64) -> i64 {
    if max_upload_size == u64::MAX {
        0
    } else {
        i64::try_from(max_upload_size).unwrap_or(i64::MAX)
    }
}

fn max_batch_total_size_bytes(max_decoding_message_size: usize) -> i64 {
    i64::try_from(max_decoding_message_size - max_decoding_message_size / 16).unwrap_or(i64::MAX)
}

#[derive(Debug, Default)]
pub struct CapabilitiesServer {
    supported_node_properties_for_instance: HashMap<InstanceName, Vec<String>>,
    // Kept separate from `supported_node_properties_for_instance`: that map is sent
    // verbatim to clients as `ExecutionCapabilities.supported_node_properties` (REAPI
    // file metadata like mtime/unix mode), while compression is advertised through
    // `CacheCapabilities.supported_compressors`. Merging them would leak a fake node
    // property onto the wire.
    remote_cache_compression_instances: RemoteCacheCompressionInstances,
    chunking_params_for_instance: HashMap<InstanceName, FastCdc2020Params>,
    // The CAS store's known upload limit per instance; absent when the store
    // reports none.
    max_cas_blob_size_for_instance: HashMap<InstanceName, i64>,
    max_batch_total_size_bytes: i64,
}

impl CapabilitiesServer {
    pub async fn new(
        configs: &[WithInstanceName<CapabilitiesConfig>],
        scheduler_map: &HashMap<String, Arc<dyn KnownPlatformPropertyProvider>>,
        remote_cache_compression_instances: &RemoteCacheCompressionInstances,
        cas_configs: &[WithInstanceName<CasStoreConfig>],
        store_manager: &StoreManager,
        max_decoding_message_size: usize,
    ) -> Result<Self, Error> {
        let mut max_cas_blob_size_for_instance = HashMap::new();
        for cas_config in cas_configs {
            if let Some(store) = store_manager.get_store(&cas_config.cas_store) {
                max_cas_blob_size_for_instance.insert(
                    cas_config.instance_name.clone(),
                    advertised_max_cas_blob_size(store.as_store_driver().max_upload_size()),
                );
            }
        }
        let mut chunking_params_for_instance = HashMap::new();
        for cas_config in cas_configs {
            if let Some(chunking_config) = &cas_config.experimental_chunking {
                let avg_chunk_size_bytes = chunking_config
                    .validated_avg_chunk_size_bytes()
                    .err_tip(|| {
                        format!(
                            "In 'experimental_chunking' of instance '{}'",
                            cas_config.instance_name
                        )
                    })?;
                chunking_params_for_instance.insert(
                    cas_config.instance_name.clone(),
                    FastCdc2020Params {
                        avg_chunk_size_bytes,
                        seed: 0,
                    },
                );
            }
        }

        let mut supported_node_properties_for_instance = HashMap::new();
        for config in configs {
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
                let properties = scheduler
                    .get_known_properties(&config.instance_name)
                    .await
                    .err_tip(|| {
                        format!(
                            "Failed to get platform properties for {}",
                            config.instance_name
                        )
                    })?;
                supported_node_properties_for_instance
                    .insert(config.instance_name.clone(), properties);
            }
        }
        Ok(Self {
            supported_node_properties_for_instance,
            remote_cache_compression_instances: remote_cache_compression_instances.clone(),
            chunking_params_for_instance,
            max_cas_blob_size_for_instance,
            max_batch_total_size_bytes: max_batch_total_size_bytes(max_decoding_message_size),
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
                exec_enabled: true, // TODO(palfrey) Make this configurable.
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

        let supported_compressors = if self
            .remote_cache_compression_instances
            .enabled_for(&instance_name)
        {
            vec![compressor::Value::Zstd.into()]
        } else {
            Vec::new()
        };

        let chunking_params = self.chunking_params_for_instance.get(&instance_name);
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
                max_batch_total_size_bytes: self.max_batch_total_size_bytes,
                symlink_absolute_path_strategy: SymlinkAbsolutePathStrategy::Disallowed.into(),
                supported_compressors: supported_compressors.clone(),
                supported_batch_update_compressors: supported_compressors,
                max_cas_blob_size_bytes: self
                    .max_cas_blob_size_for_instance
                    .get(&instance_name)
                    .copied()
                    .unwrap_or(0),
                split_blob_support: chunking_params.is_some(),
                splice_blob_support: chunking_params.is_some(),
                fast_cdc_2020_params: chunking_params.copied(),
                rep_max_cdc_params: None,
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

#[cfg(test)]
mod tests {
    use super::{advertised_max_cas_blob_size, max_batch_total_size_bytes};

    #[test]
    fn max_cas_blob_size_is_zero_only_when_the_limit_is_unknown() {
        assert_eq!(advertised_max_cas_blob_size(u64::MAX), 0);
        assert_eq!(advertised_max_cas_blob_size(5 << 40), 5 << 40);
        assert_eq!(advertised_max_cas_blob_size(u64::MAX - 1), i64::MAX);
    }

    #[test]
    fn batch_limit_leaves_headroom_below_the_decoding_limit() {
        assert_eq!(max_batch_total_size_bytes(4 << 20), (4 << 20) - (256 << 10));
        assert_eq!(max_batch_total_size_bytes(0), 0);
    }
}
