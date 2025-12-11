// Copyright 2024 The NativeLink Authors. All rights reserved.
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

//! Integration tests for warm worker pools feature.
//!
//! These tests verify the warm worker pool integration with the scheduler
//! without requiring a real CRI-O environment. For full end-to-end tests
//! with CRI-O, see the E2E test documentation in docs/warm-worker-pools.md.

#[cfg(feature = "warm-worker-pools")]
mod warm_pools_tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use nativelink_config::schedulers::SimpleSpec;
    use nativelink_error::Error;
    use nativelink_macro::nativelink_test;
    use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
    use nativelink_scheduler::simple_scheduler::SimpleScheduler;
    use nativelink_util::action_messages::{ActionInfo, ActionUniqueKey, ActionUniqueQualifier};
    use nativelink_util::common::DigestInfo;
    use nativelink_util::digest_hasher::DigestHasherFunc;
    use nativelink_util::instant_wrapper::{InstantWrapper, MockInstantWrapped};
    use pretty_assertions::assert_eq;
    use tokio::sync::Notify;

    const INSTANCE_NAME: &str = "warm_pool_test";

    /// Helper to create ActionInfo for testing
    fn make_action_info_with_platform_props(props: HashMap<String, String>) -> ActionInfo {
        let now_fn = MockInstantWrapped::default();
        ActionInfo {
            command_digest: DigestInfo::new([0u8; 32], 0),
            input_root_digest: DigestInfo::new([0u8; 32], 0),
            timeout: std::time::Duration::from_secs(60),
            platform_properties: props,
            priority: 0,
            load_timestamp: now_fn.now(),
            insert_timestamp: now_fn.now(),
            unique_qualifier: ActionUniqueQualifier::Cacheable(ActionUniqueKey {
                instance_name: INSTANCE_NAME.to_string(),
                digest_function: DigestHasherFunc::Sha256,
                digest: DigestInfo::new([1u8; 32], 100),
            }),
        }
    }

    fn make_simple_spec_with_warm_pools() -> SimpleSpec {
        SimpleSpec {
            supported_platform_properties: Some(HashMap::from([
                (
                    "lang".to_string(),
                    nativelink_config::schedulers::PropertyType::Exact,
                ),
                (
                    "toolchain".to_string(),
                    nativelink_config::schedulers::PropertyType::Exact,
                ),
            ])),
            warm_worker_pools: Some(
                nativelink_config::warm_worker_pools::WarmWorkerPoolsConfig {
                    pools: vec![
                        nativelink_config::warm_worker_pools::WorkerPoolConfig {
                            name: "java-pool".to_string(),
                            language: nativelink_config::warm_worker_pools::Language::Jvm,
                            match_platform_properties: HashMap::new(),
                            cri_socket: "unix:///var/run/crio/crio.sock".to_string(),
                            container_image: "test-java-worker:latest".to_string(),
                            min_warm_workers: 2,
                            max_workers: 10,
                            warmup: nativelink_config::warm_worker_pools::WarmupConfig {
                                commands: vec![
                                    nativelink_config::warm_worker_pools::WarmupCommand {
                                        argv: vec!["/bin/true".to_string()],
                                        timeout_s: Some(60),
                                    },
                                ],
                                post_job_cleanup: vec![],
                            },
                            lifecycle: nativelink_config::warm_worker_pools::LifecycleConfig {
                                worker_ttl_seconds: 3600,
                                max_jobs_per_worker: 200,
                                gc_job_frequency: 50,
                            },
                        },
                        nativelink_config::warm_worker_pools::WorkerPoolConfig {
                            name: "typescript-pool".to_string(),
                            language: nativelink_config::warm_worker_pools::Language::NodeJs,
                            match_platform_properties: HashMap::new(),
                            cri_socket: "unix:///var/run/crio/crio.sock".to_string(),
                            container_image: "test-node-worker:latest".to_string(),
                            min_warm_workers: 1,
                            max_workers: 5,
                            warmup: nativelink_config::warm_worker_pools::WarmupConfig {
                                commands: vec![
                                    nativelink_config::warm_worker_pools::WarmupCommand {
                                        argv: vec!["/bin/true".to_string()],
                                        timeout_s: Some(30),
                                    },
                                ],
                                post_job_cleanup: vec![],
                            },
                            lifecycle: nativelink_config::warm_worker_pools::LifecycleConfig {
                                worker_ttl_seconds: 1800,
                                max_jobs_per_worker: 100,
                                gc_job_frequency: 25,
                            },
                        },
                    ],
                },
            ),
            ..Default::default()
        }
    }

    fn make_simple_spec_with_warm_pools_with_matchers() -> SimpleSpec {
        let mut spec = make_simple_spec_with_warm_pools();
        if let Some(warm_cfg) = &mut spec.warm_worker_pools {
            warm_cfg.pools[0].match_platform_properties = HashMap::from([(
                "container-image".to_string(),
                nativelink_config::warm_worker_pools::PropertyMatcher::Contains {
                    contains: "remotejdk_11".to_string(),
                },
            )]);
        }
        spec
    }

    /// Test that Java actions are correctly identified for warm pool routing
    #[nativelink_test]
    async fn test_should_use_warm_pool_java() -> Result<(), Error> {
        let spec = make_simple_spec_with_warm_pools();
        let task_change_notify = Arc::new(Notify::new());
        let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
            &spec,
            memory_awaited_action_db_factory(
                0,
                &task_change_notify.clone(),
                MockInstantWrapped::default,
            ),
            || async move {},
            task_change_notify,
            MockInstantWrapped::default,
            None,
        );

        // Wait for warm pool initialization to complete (happens asynchronously)
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Test explicit lang=java
        let action_info = make_action_info_with_platform_props(HashMap::from([(
            "lang".to_string(),
            "java".to_string(),
        )]));

        let pool_name = scheduler.should_use_warm_pool(&action_info).await;
        assert_eq!(pool_name, Some("java-pool".to_string()));

        Ok(())
    }

    /// Test that explicit matchers route actions to the configured pool.
    #[nativelink_test]
    async fn test_should_use_warm_pool_match_platform_properties() -> Result<(), Error> {
        let spec = make_simple_spec_with_warm_pools_with_matchers();
        let task_change_notify = Arc::new(Notify::new());
        let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
            &spec,
            memory_awaited_action_db_factory(
                0,
                &task_change_notify.clone(),
                MockInstantWrapped::default,
            ),
            || async move {},
            task_change_notify,
            MockInstantWrapped::default,
            None,
        );

        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let action_info = make_action_info_with_platform_props(HashMap::from([(
            "container-image".to_string(),
            "docker://test-java-worker:remotejdk_11".to_string(),
        )]));

        let pool_name = scheduler.should_use_warm_pool(&action_info).await;
        assert_eq!(pool_name, Some("java-pool".to_string()));

        Ok(())
    }

    /// Test that TypeScript actions are correctly identified for warm pool routing
    #[nativelink_test]
    async fn test_should_use_warm_pool_typescript() -> Result<(), Error> {
        let spec = make_simple_spec_with_warm_pools();
        let task_change_notify = Arc::new(Notify::new());
        let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
            &spec,
            memory_awaited_action_db_factory(
                0,
                &task_change_notify.clone(),
                MockInstantWrapped::default,
            ),
            || async move {},
            task_change_notify,
            MockInstantWrapped::default,
            None,
        );

        // Wait for warm pool initialization to complete (happens asynchronously)
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Test lang=typescript
        let action_info = make_action_info_with_platform_props(HashMap::from([(
            "lang".to_string(),
            "typescript".to_string(),
        )]));

        let pool_name = scheduler.should_use_warm_pool(&action_info).await;
        assert_eq!(pool_name, Some("typescript-pool".to_string()));

        Ok(())
    }

    /// Test that toolchain properties are detected for warm pool routing
    #[nativelink_test]
    async fn test_should_use_warm_pool_toolchain() -> Result<(), Error> {
        let spec = make_simple_spec_with_warm_pools();
        let task_change_notify = Arc::new(Notify::new());
        let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
            &spec,
            memory_awaited_action_db_factory(
                0,
                &task_change_notify.clone(),
                MockInstantWrapped::default,
            ),
            || async move {},
            task_change_notify,
            MockInstantWrapped::default,
            None,
        );

        // Wait for warm pool initialization to complete (happens asynchronously)
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Test toolchain containing "java"
        let action_info = make_action_info_with_platform_props(HashMap::from([(
            "toolchain".to_string(),
            "/opt/java-17/bin/javac".to_string(),
        )]));

        let pool_name = scheduler.should_use_warm_pool(&action_info).await;
        assert_eq!(pool_name, Some("java-pool".to_string()));

        Ok(())
    }

    /// Test that actions without warm pool hints return None
    #[nativelink_test]
    async fn test_should_use_warm_pool_no_match() -> Result<(), Error> {
        let spec = make_simple_spec_with_warm_pools();
        let task_change_notify = Arc::new(Notify::new());
        let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
            &spec,
            memory_awaited_action_db_factory(
                0,
                &task_change_notify.clone(),
                MockInstantWrapped::default,
            ),
            || async move {},
            task_change_notify,
            MockInstantWrapped::default,
            None,
        );

        // Test action without any pool hints
        let action_info = make_action_info_with_platform_props(HashMap::from([(
            "cpu".to_string(),
            "x86_64".to_string(),
        )]));

        let pool_name = scheduler.should_use_warm_pool(&action_info).await;
        assert_eq!(pool_name, None);

        Ok(())
    }

    /// Test that warm pool manager is not initialized when not configured
    #[nativelink_test]
    async fn test_no_warm_pools_when_not_configured() -> Result<(), Error> {
        let spec = SimpleSpec::default();
        let task_change_notify = Arc::new(Notify::new());
        let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
            &spec,
            memory_awaited_action_db_factory(
                0,
                &task_change_notify.clone(),
                MockInstantWrapped::default,
            ),
            || async move {},
            task_change_notify,
            MockInstantWrapped::default,
            None,
        );

        // Verify should_use_warm_pool returns None when no pools configured
        let action_info = make_action_info_with_platform_props(HashMap::from([(
            "lang".to_string(),
            "java".to_string(),
        )]));

        let pool_name = scheduler.should_use_warm_pool(&action_info).await;
        assert_eq!(pool_name, None);

        Ok(())
    }

    /// Test various language detection patterns
    #[nativelink_test]
    async fn test_language_detection_patterns() -> Result<(), Error> {
        let spec = make_simple_spec_with_warm_pools();
        let task_change_notify = Arc::new(Notify::new());
        let (scheduler, _worker_scheduler) = SimpleScheduler::new_with_callback(
            &spec,
            memory_awaited_action_db_factory(
                0,
                &task_change_notify.clone(),
                MockInstantWrapped::default,
            ),
            || async move {},
            task_change_notify,
            MockInstantWrapped::default,
            None,
        );

        // Wait for warm pool initialization to complete (happens asynchronously)
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        // Test various Java language patterns
        let java_patterns = vec!["java", "jvm", "kotlin", "scala"];
        for pattern in java_patterns {
            let action_info = make_action_info_with_platform_props(HashMap::from([(
                "lang".to_string(),
                pattern.to_string(),
            )]));

            let pool_name = scheduler.should_use_warm_pool(&action_info).await;
            assert_eq!(
                pool_name,
                Some("java-pool".to_string()),
                "Failed for pattern: {}",
                pattern
            );
        }

        // Test various TypeScript/Node.js language patterns
        let node_patterns = vec!["typescript", "ts", "javascript", "js", "node", "nodejs"];
        for pattern in node_patterns {
            let action_info = make_action_info_with_platform_props(HashMap::from([(
                "lang".to_string(),
                pattern.to_string(),
            )]));

            let pool_name = scheduler.should_use_warm_pool(&action_info).await;
            assert_eq!(
                pool_name,
                Some("typescript-pool".to_string()),
                "Failed for pattern: {}",
                pattern
            );
        }

        Ok(())
    }
}
