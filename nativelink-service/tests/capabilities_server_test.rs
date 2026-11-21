// Copyright 2026 Trace Machina, Inc. All rights reserved.
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

use futures::join;
use nativelink_config::cas_server::{
    CapabilitiesConfig, CapabilitiesRemoteExecutionConfig, CasStoreConfig, WithInstanceName,
};
use nativelink_config::stores::MemorySpec;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::capabilities_server::Capabilities;
use nativelink_proto::build::bazel::remote::execution::v2::{
    GetCapabilitiesRequest, ServerCapabilities, compressor, digest_function,
};
use nativelink_scheduler::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_scheduler::mock_scheduler::MockActionScheduler;
use nativelink_service::capabilities_server::CapabilitiesServer;
use nativelink_service::wire_compression::RemoteCacheCompressionInstances;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::store_trait::Store;
use pretty_assertions::assert_eq;
use tonic::Request;

const COMPRESSION_INSTANCE: &str = "compression";
const EXECUTION_INSTANCE: &str = "execution";
const SCHEDULER_NAME: &str = "main_scheduler";
/// The listener's default `max_decoding_message_size`.
const DECODING_LIMIT: usize = 4 * 1024 * 1024;

fn capabilities_config(
    instance_name: &str,
    remote_cache_compression: bool,
    remote_execution: bool,
) -> WithInstanceName<CapabilitiesConfig> {
    WithInstanceName {
        instance_name: instance_name.to_string(),
        config: CapabilitiesConfig {
            remote_execution: remote_execution.then(|| CapabilitiesRemoteExecutionConfig {
                scheduler: SCHEDULER_NAME.to_string(),
            }),
            remote_cache_compression,
        },
    }
}

async fn get_capabilities(
    server: &CapabilitiesServer,
    instance_name: &str,
) -> Result<ServerCapabilities, tonic::Status> {
    server
        .get_capabilities(Request::new(GetCapabilitiesRequest {
            instance_name: instance_name.to_string(),
        }))
        .await
        .map(tonic::Response::into_inner)
}

#[nativelink_test]
async fn compression_only_instance_advertises_zstd_cache_capabilities()
-> Result<(), Box<dyn core::error::Error>> {
    let configs = [capabilities_config(COMPRESSION_INSTANCE, true, false)];
    let remote_cache_compression_instances =
        RemoteCacheCompressionInstances::from_capabilities_configs(&configs);
    let store_manager = StoreManager::new();
    let server = CapabilitiesServer::new(
        &configs,
        &HashMap::new(),
        &remote_cache_compression_instances,
        &[],
        &store_manager,
        DECODING_LIMIT,
    )
    .await?;

    let response = get_capabilities(&server, COMPRESSION_INSTANCE).await?;
    let cache_capabilities = response
        .cache_capabilities
        .expect("cache capabilities should be set");

    assert_eq!(
        cache_capabilities.digest_functions,
        vec![
            digest_function::Value::Sha256 as i32,
            digest_function::Value::Blake3 as i32,
        ],
        "cache clients must be told that both digest functions are supported"
    );
    assert_eq!(
        cache_capabilities.supported_compressors,
        vec![compressor::Value::Zstd as i32]
    );
    assert_eq!(
        cache_capabilities.supported_batch_update_compressors,
        vec![compressor::Value::Zstd as i32]
    );
    Ok(())
}

#[nativelink_test]
async fn compression_only_instance_does_not_advertise_execution_capabilities()
-> Result<(), Box<dyn core::error::Error>> {
    let configs = [capabilities_config(COMPRESSION_INSTANCE, true, false)];
    let remote_cache_compression_instances =
        RemoteCacheCompressionInstances::from_capabilities_configs(&configs);
    let store_manager = StoreManager::new();
    let server = CapabilitiesServer::new(
        &configs,
        &HashMap::new(),
        &remote_cache_compression_instances,
        &[],
        &store_manager,
        DECODING_LIMIT,
    )
    .await?;

    let response = get_capabilities(&server, COMPRESSION_INSTANCE).await?;

    assert!(
        response.execution_capabilities.is_none(),
        "compression-only instance should not advertise execution capabilities"
    );
    Ok(())
}

#[nativelink_test]
async fn remote_execution_instance_advertises_execution_capabilities_and_node_properties()
-> Result<(), Box<dyn core::error::Error>> {
    let configs = [capabilities_config(EXECUTION_INSTANCE, false, true)];
    let remote_cache_compression_instances =
        RemoteCacheCompressionInstances::from_capabilities_configs(&configs);

    let mock_scheduler = Arc::new(MockActionScheduler::new());
    let mut scheduler_map: HashMap<String, Arc<dyn KnownPlatformPropertyProvider>> = HashMap::new();
    scheduler_map.insert(SCHEDULER_NAME.to_string(), mock_scheduler.clone());

    let expected_properties = vec!["cpu".to_string(), "os".to_string()];
    let store_manager = StoreManager::new();
    let new_server_fut = CapabilitiesServer::new(
        &configs,
        &scheduler_map,
        &remote_cache_compression_instances,
        &[],
        &store_manager,
        DECODING_LIMIT,
    );
    let expected_scheduler_call_fut =
        mock_scheduler.expect_get_known_properties(Ok(expected_properties.clone()));
    let (server, scheduler_instance_name) = join!(new_server_fut, expected_scheduler_call_fut);
    assert_eq!(scheduler_instance_name, EXECUTION_INSTANCE);
    let server = server?;

    let response = get_capabilities(&server, EXECUTION_INSTANCE).await?;
    let execution_capabilities = response
        .execution_capabilities
        .expect("execution capabilities should be set");

    assert!(execution_capabilities.exec_enabled);
    assert_eq!(
        execution_capabilities.digest_function,
        digest_function::Value::Sha256 as i32,
        "the legacy execution digest field should report the server default"
    );
    assert_eq!(
        execution_capabilities.digest_functions,
        vec![
            digest_function::Value::Sha256 as i32,
            digest_function::Value::Blake3 as i32,
        ],
        "execution clients must be told that both digest functions are supported"
    );
    assert_eq!(
        execution_capabilities.supported_node_properties,
        expected_properties
    );
    Ok(())
}

// REv2 CacheCapabilities: `max_batch_total_size_bytes` should match what the
// server accepts, and `max_cas_blob_size_bytes` is 0 when no limit is set.
// The batch limit follows the listener's decoding limit (less headroom for
// per-blob framing) instead of a hard-coded 64 KiB, and a CAS store that
// reports no upload limit, like the memory store, advertises 0.
#[nativelink_test]
async fn cache_size_limits_follow_listener_and_store() -> Result<(), Box<dyn core::error::Error>> {
    let configs = [capabilities_config(COMPRESSION_INSTANCE, false, false)];
    let store_manager = StoreManager::new();
    store_manager.add_store(
        "main_cas",
        Store::new(MemoryStore::new(&MemorySpec::default())),
    )?;
    let server = CapabilitiesServer::new(
        &configs,
        &HashMap::new(),
        &RemoteCacheCompressionInstances::from_capabilities_configs(&configs),
        &[WithInstanceName {
            instance_name: COMPRESSION_INSTANCE.to_string(),
            config: CasStoreConfig {
                cas_store: "main_cas".to_string(),
                experimental_chunking: None,
            },
        }],
        &store_manager,
        16 * 1024 * 1024,
    )
    .await?;

    let cache_capabilities = get_capabilities(&server, COMPRESSION_INSTANCE)
        .await?
        .cache_capabilities
        .expect("cache capabilities should be set");
    assert_eq!(
        cache_capabilities.max_batch_total_size_bytes,
        16 * 1024 * 1024 - 1024 * 1024
    );
    assert_eq!(cache_capabilities.max_cas_blob_size_bytes, 0);
    Ok(())
}
