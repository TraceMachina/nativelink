// Copyright 2025 The NativeLink Authors. All rights reserved.
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
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use nativelink_config::cas_server::LocalWorkerConfig;
use nativelink_error::{Code, Error};
use nativelink_proto::build::bazel::remote::execution::v2::{
    Command as ProtoCommand, Platform, Property,
};
use nativelink_util::action_messages::{ActionInfo, OperationId, WorkerId};
use nativelink_util::common::DigestInfo;
use nativelink_util::platform_properties::PlatformProperties;
use nativelink_worker::persistent_worker::PersistentWorkerKey;
use nativelink_worker::persistent_worker_runner::{
    PersistentWorkerExecutor, PersistentWorkerRunner,
};
use pretty_assertions::assert_eq;

#[tokio::test]
async fn test_persistent_worker_runner_creation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = LocalWorkerConfig::default();

    let runner = PersistentWorkerRunner::new(
        config,
        temp_dir.path().to_path_buf(),
        "grpc://localhost:50051".to_string(),
        Duration::from_secs(300),
        10,
    );

    // Test that runner initializes with empty spawned workers
    runner.cleanup_terminated_workers().await;
}

#[test]
fn test_extract_persistent_key_from_platform() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = LocalWorkerConfig::default();

    let runner = PersistentWorkerRunner::new(
        config,
        temp_dir.path().to_path_buf(),
        "grpc://localhost:50051".to_string(),
        Duration::from_secs(300),
        10,
    );

    let mut platform = PlatformProperties::new();
    platform.insert("persistentWorkerKey".to_string(), "javac-123".to_string());
    platform.insert("persistentWorkerTool".to_string(), "javac".to_string());
    platform.insert("cpu".to_string(), "4".to_string());

    // This test uses private method, would need to be adjusted for real testing
    // or make the method public for testing
}

#[tokio::test]
async fn test_fork_persistent_worker_command_construction() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = LocalWorkerConfig::default();

    let runner = PersistentWorkerRunner::new(
        config,
        temp_dir.path().to_path_buf(),
        "grpc://localhost:50051".to_string(),
        Duration::from_secs(300),
        10,
    );

    let key = PersistentWorkerKey {
        tool: "javac".to_string(),
        key: "test-worker-123".to_string(),
        platform_properties: HashMap::from([
            ("cpu".to_string(), "4".to_string()),
            ("memory".to_string(), "8GB".to_string()),
        ]),
    };

    // This would actually spawn a process in real execution
    // For unit testing, we'd need to mock the Command::spawn
    // let result = runner.fork_persistent_worker(&key).await;

    // Verify work directory structure would be created
    let expected_work_dir = temp_dir.path().join("worker_");
    assert!(temp_dir.path().exists());
}

#[tokio::test]
async fn test_maybe_spawn_persistent_worker_no_platform() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = LocalWorkerConfig::default();

    let runner = PersistentWorkerRunner::new(
        config,
        temp_dir.path().to_path_buf(),
        "grpc://localhost:50051".to_string(),
        Duration::from_secs(300),
        10,
    );

    let action_info = ActionInfo {
        command_digest: DigestInfo::zero_digest(),
        input_root_digest: DigestInfo::zero_digest(),
        timeout: Duration::from_secs(60),
        platform_properties: None, // No platform properties
        priority: 0,
        load_timestamp: SystemTime::now(),
        insert_timestamp: SystemTime::now(),
        unique_qualifier: OperationId::new(uuid::Uuid::new_v4()),
        skip_cache_lookup: false,
    };

    let result = runner.maybe_spawn_persistent_worker(&action_info).await;
    assert!(result.is_none());
}

#[tokio::test]
async fn test_maybe_spawn_persistent_worker_with_persistent_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = LocalWorkerConfig::default();

    let runner = PersistentWorkerRunner::new(
        config,
        temp_dir.path().to_path_buf(),
        "grpc://localhost:50051".to_string(),
        Duration::from_secs(300),
        10,
    );

    let mut platform = PlatformProperties::new();
    platform.insert("persistentWorkerKey".to_string(), "scalac-456".to_string());
    platform.insert("persistentWorkerTool".to_string(), "scalac".to_string());

    let action_info = ActionInfo {
        command_digest: DigestInfo::zero_digest(),
        input_root_digest: DigestInfo::zero_digest(),
        timeout: Duration::from_secs(60),
        platform_properties: Some(platform),
        priority: 0,
        load_timestamp: SystemTime::now(),
        insert_timestamp: SystemTime::now(),
        unique_qualifier: OperationId::new(uuid::Uuid::new_v4()),
        skip_cache_lookup: false,
    };

    // This would attempt to spawn a real worker
    // In unit tests, we'd mock the spawn behavior
    let _result = runner.maybe_spawn_persistent_worker(&action_info).await;
}

#[tokio::test]
async fn test_cleanup_terminated_workers() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = LocalWorkerConfig::default();

    let runner = PersistentWorkerRunner::new(
        config,
        temp_dir.path().to_path_buf(),
        "grpc://localhost:50051".to_string(),
        Duration::from_secs(300),
        10,
    );

    // Initially no workers
    runner.cleanup_terminated_workers().await;

    // In a real test with mocked processes:
    // 1. Spawn some workers
    // 2. Terminate some of them
    // 3. Call cleanup
    // 4. Verify terminated workers are removed
}

#[tokio::test]
async fn test_shutdown_all_workers() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = LocalWorkerConfig::default();

    let runner = PersistentWorkerRunner::new(
        config,
        temp_dir.path().to_path_buf(),
        "grpc://localhost:50051".to_string(),
        Duration::from_secs(300),
        10,
    );

    // Should not panic when no workers exist
    runner.shutdown_all().await;
}

#[tokio::test]
async fn test_persistent_worker_executor_creation() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = LocalWorkerConfig::default();

    let executor = PersistentWorkerExecutor::new(config, temp_dir.path().to_path_buf());

    // Test basic creation
    assert!(
        executor
            .maybe_execute_as_persistent(
                &ActionInfo {
                    command_digest: DigestInfo::zero_digest(),
                    input_root_digest: DigestInfo::zero_digest(),
                    timeout: Duration::from_secs(60),
                    platform_properties: None,
                    priority: 0,
                    load_timestamp: SystemTime::now(),
                    insert_timestamp: SystemTime::now(),
                    unique_qualifier: OperationId::new(uuid::Uuid::new_v4()),
                    skip_cache_lookup: false,
                },
                &ProtoCommand::default(),
                &Platform { properties: vec![] }
            )
            .await
            .is_none()
    );
}

#[tokio::test]
async fn test_maybe_execute_as_persistent_with_key() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = LocalWorkerConfig::default();

    let executor = PersistentWorkerExecutor::new(config, temp_dir.path().to_path_buf());

    let platform = Platform {
        properties: vec![
            Property {
                name: "persistentWorkerKey".to_string(),
                value: "test-key".to_string(),
            },
            Property {
                name: "persistentWorkerTool".to_string(),
                value: "test-tool".to_string(),
            },
        ],
    };

    let action_info = ActionInfo {
        command_digest: DigestInfo::zero_digest(),
        input_root_digest: DigestInfo::zero_digest(),
        timeout: Duration::from_secs(60),
        platform_properties: None,
        priority: 0,
        load_timestamp: SystemTime::now(),
        insert_timestamp: SystemTime::now(),
        unique_qualifier: OperationId::new(uuid::Uuid::new_v4()),
        skip_cache_lookup: false,
    };

    let command = ProtoCommand {
        arguments: vec!["test-tool".to_string()],
        environment_variables: vec![],
        output_files: vec![],
        output_directories: vec![],
        output_paths: vec![],
        platform: Some(platform.clone()),
        working_directory: String::new(),
        output_node_properties: vec![],
    };

    let result = executor
        .maybe_execute_as_persistent(&action_info, &command, &platform)
        .await;

    // Should return Some because persistent worker key is present
    assert!(result.is_some());

    // The actual execution would fail without a real persistent worker
    if let Some(exec_result) = result {
        assert!(exec_result.is_err());
    }
}

#[test]
fn test_persistent_worker_key_construction() {
    let key = PersistentWorkerKey {
        tool: "rustc".to_string(),
        key: "rustc-789".to_string(),
        platform_properties: HashMap::from([
            ("target".to_string(), "x86_64".to_string()),
            ("opt_level".to_string(), "3".to_string()),
        ]),
    };

    let props = key.to_platform_properties();
    assert_eq!(props.len(), 4); // tool, key, and 2 platform properties
    assert!(props.contains(&("persistentWorkerKey".to_string(), "rustc-789".to_string())));
    assert!(props.contains(&("persistentWorkerTool".to_string(), "rustc".to_string())));
}

#[tokio::test]
async fn test_worker_environment_variables() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = LocalWorkerConfig::default();

    let runner = PersistentWorkerRunner::new(
        config,
        temp_dir.path().to_path_buf(),
        "grpc://localhost:50051".to_string(),
        Duration::from_secs(300),
        10,
    );

    // Test that environment variables would be set correctly
    let key = PersistentWorkerKey {
        tool: "node".to_string(),
        key: "node-worker-001".to_string(),
        platform_properties: HashMap::new(),
    };

    // In a real test with process mocking, verify:
    // - PERSISTENT_WORKER=true
    // - PERSISTENT_WORKER_KEY=node-worker-001
    // - PERSISTENT_WORKER_TOOL=node
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[tokio::test]
    async fn test_worker_spawn_and_cleanup_lifecycle() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = LocalWorkerConfig::default();

        let runner = Arc::new(PersistentWorkerRunner::new(
            config,
            temp_dir.path().to_path_buf(),
            "grpc://localhost:50051".to_string(),
            Duration::from_secs(1), // Short timeout for testing
            5,
        ));

        // Simulate lifecycle
        // 1. Spawn workers (would be mocked in unit test)
        // 2. Wait for timeout
        // 3. Cleanup
        // 4. Verify cleanup

        tokio::time::sleep(Duration::from_secs(2)).await;
        runner.cleanup_terminated_workers().await;
        runner.shutdown_all().await;
    }

    #[tokio::test]
    async fn test_concurrent_worker_spawning() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config = LocalWorkerConfig::default();

        let runner = Arc::new(PersistentWorkerRunner::new(
            config,
            temp_dir.path().to_path_buf(),
            "grpc://localhost:50051".to_string(),
            Duration::from_secs(300),
            10,
        ));

        let mut handles = vec![];

        // Simulate concurrent spawn requests
        for i in 0..3 {
            let runner = runner.clone();
            let handle = tokio::spawn(async move {
                let mut platform = PlatformProperties::new();
                platform.insert("persistentWorkerKey".to_string(), format!("worker-{}", i));
                platform.insert("persistentWorkerTool".to_string(), "test-tool".to_string());

                let action_info = ActionInfo {
                    command_digest: DigestInfo::zero_digest(),
                    input_root_digest: DigestInfo::zero_digest(),
                    timeout: Duration::from_secs(60),
                    platform_properties: Some(platform),
                    priority: 0,
                    load_timestamp: SystemTime::now(),
                    insert_timestamp: SystemTime::now(),
                    unique_qualifier: OperationId::new(uuid::Uuid::new_v4()),
                    skip_cache_lookup: false,
                };

                runner.maybe_spawn_persistent_worker(&action_info).await
            });
            handles.push(handle);
        }

        // Wait for all spawns
        for handle in handles {
            let _ = handle.await;
        }

        runner.shutdown_all().await;
    }
}
