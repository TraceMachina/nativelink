// Copyright 2026 The NativeLink Authors. All rights reserved.
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
    CapabilitiesConfig, CapabilitiesRemoteExecutionConfig, WithInstanceName,
};
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::capabilities_server::Capabilities;
use nativelink_proto::build::bazel::remote::execution::v2::{
    GetCapabilitiesRequest, ServerCapabilities, compressor,
};
use nativelink_scheduler::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_scheduler::mock_scheduler::MockActionScheduler;
use nativelink_service::capabilities_server::CapabilitiesServer;
use nativelink_service::wire_compression::RemoteCacheCompressionInstances;
use pretty_assertions::assert_eq;
use tonic::Request;

const COMPRESSION_INSTANCE: &str = "compression";
const EXECUTION_INSTANCE: &str = "execution";
const SCHEDULER_NAME: &str = "main_scheduler";

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
    let server = CapabilitiesServer::new(
        &configs,
        &HashMap::new(),
        &remote_cache_compression_instances,
    )
    .await?;

    let response = get_capabilities(&server, COMPRESSION_INSTANCE).await?;
    let cache_capabilities = response
        .cache_capabilities
        .expect("cache capabilities should be set");

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
    let server = CapabilitiesServer::new(
        &configs,
        &HashMap::new(),
        &remote_cache_compression_instances,
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
    let new_server_fut = CapabilitiesServer::new(
        &configs,
        &scheduler_map,
        &remote_cache_compression_instances,
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
        execution_capabilities.supported_node_properties,
        expected_properties
    );
    Ok(())
}
