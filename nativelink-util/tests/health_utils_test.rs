// Copyright 2024 The Native Link Authors. All rights reserved.
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

use core::time::Duration;
use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use futures::StreamExt;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_util::health_utils::{
    HealthRegistryBuilder, HealthStatus, HealthStatusDescription, HealthStatusIndicator,
    HealthStatusReporter,
};
use pretty_assertions::assert_eq;
use tokio::sync::Barrier;
use tokio::time::Instant;

#[nativelink_test]
async fn create_empty_indicator() -> Result<(), Error> {
    let mut health_registry_builder = HealthRegistryBuilder::new("nativelink");
    let health_registry = health_registry_builder.build();
    let health_status: Vec<HealthStatusDescription> = health_registry
        .health_status_report(&Duration::MAX)
        .collect()
        .await;
    assert_eq!(health_status.len(), 0);
    Ok(())
}

#[nativelink_test]
async fn create_register_indicator() -> Result<(), Error> {
    generate_health_status_indicator!(MockComponentImpl, Ok, "ok");

    let mut health_registry_builder = HealthRegistryBuilder::new("nativelink");

    health_registry_builder.register_indicator(Arc::new(MockComponentImpl {}));

    let health_registry = health_registry_builder.build();
    let health_status: Vec<HealthStatusDescription> = health_registry
        .health_status_report(&Duration::MAX)
        .collect()
        .await;

    assert_eq!(health_status.len(), 1);
    assert_eq!(
        health_status,
        vec![HealthStatusDescription {
            namespace: "/nativelink/MockComponentImpl".into(),
            status: HealthStatus::Ok {
                struct_name: "MockComponentImpl",
                message: "ok".into()
            },
        }]
    );

    Ok(())
}

#[nativelink_test]
async fn create_sub_registry() -> Result<(), Error> {
    generate_health_status_indicator!(MockComponentImpl, Ok, "ok");

    let mut health_registry_builder = HealthRegistryBuilder::new("nativelink");

    health_registry_builder.register_indicator(Arc::new(MockComponentImpl {}));

    let mut namespace1_registry = health_registry_builder.sub_builder("namespace1");

    namespace1_registry.register_indicator(Arc::new(MockComponentImpl {}));

    let health_registry = health_registry_builder.build();
    let health_status: Vec<HealthStatusDescription> = health_registry
        .health_status_report(&Duration::MAX)
        .collect()
        .await;

    assert_eq!(health_status.len(), 2);
    let expected_health_status = vec_to_set(vec![
        HealthStatusDescription {
            namespace: "/nativelink/MockComponentImpl".into(),
            status: HealthStatus::Ok {
                struct_name: "MockComponentImpl",
                message: "ok".into(),
            },
        },
        HealthStatusDescription {
            namespace: "/nativelink/namespace1/MockComponentImpl".into(),
            status: HealthStatus::Ok {
                struct_name: "MockComponentImpl",
                message: "ok".into(),
            },
        },
    ]);

    assert_eq!(vec_to_set(health_status), expected_health_status);

    Ok(())
}

#[nativelink_test]
async fn create_multiple_indicators_same_registry() -> Result<(), Error> {
    generate_health_status_indicator!(MockComponentImpl1, Ok, "ok");
    generate_health_status_indicator!(MockComponentImpl2, Ok, "ok");
    generate_health_status_indicator!(MockComponentImpl3, Ok, "ok");

    let mut health_registry_builder = HealthRegistryBuilder::new("nativelink");

    health_registry_builder.register_indicator(Arc::new(MockComponentImpl1 {}));
    health_registry_builder.register_indicator(Arc::new(MockComponentImpl2 {}));
    health_registry_builder.register_indicator(Arc::new(MockComponentImpl3 {}));

    let health_registry = health_registry_builder.build();
    let health_status: Vec<HealthStatusDescription> = health_registry
        .health_status_report(&Duration::MAX)
        .collect()
        .await;

    assert_eq!(health_status.len(), 3);
    let expected_health_status = vec_to_set(vec![
        HealthStatusDescription {
            namespace: "/nativelink/MockComponentImpl1".into(),
            status: HealthStatus::Ok {
                struct_name: "MockComponentImpl1",
                message: "ok".into(),
            },
        },
        HealthStatusDescription {
            namespace: "/nativelink/MockComponentImpl2".into(),
            status: HealthStatus::Ok {
                struct_name: "MockComponentImpl2",
                message: "ok".into(),
            },
        },
        HealthStatusDescription {
            namespace: "/nativelink/MockComponentImpl3".into(),
            status: HealthStatus::Ok {
                struct_name: "MockComponentImpl3",
                message: "ok".into(),
            },
        },
    ]);

    assert_eq!(vec_to_set(health_status), expected_health_status);

    Ok(())
}

#[nativelink_test]
async fn create_multiple_indicators_with_sub_registry() -> Result<(), Error> {
    generate_health_status_indicator!(MockComponentImpl1, Ok, "ok");
    generate_health_status_indicator!(MockComponentImpl2, Ok, "ok");
    generate_health_status_indicator!(MockComponentImpl3, Ok, "ok");

    let mut health_registry_builder = HealthRegistryBuilder::new("nativelink");

    let mut sub_builder = health_registry_builder.sub_builder("namespace1");
    sub_builder.register_indicator(Arc::new(MockComponentImpl1 {}));
    let mut sub_builder = health_registry_builder.sub_builder("namespace2");
    sub_builder.register_indicator(Arc::new(MockComponentImpl2 {}));

    health_registry_builder
        .sub_builder("namespace3")
        .register_indicator(Arc::new(MockComponentImpl3 {}));

    let health_registry = health_registry_builder.build();
    let health_status: Vec<HealthStatusDescription> = health_registry
        .health_status_report(&Duration::MAX)
        .collect()
        .await;

    assert_eq!(health_status.len(), 3);
    let expected_health_status = vec_to_set(vec![
        HealthStatusDescription {
            namespace: "/nativelink/namespace1/MockComponentImpl1".into(),
            status: HealthStatus::Ok {
                struct_name: "MockComponentImpl1",
                message: "ok".into(),
            },
        },
        HealthStatusDescription {
            namespace: "/nativelink/namespace2/MockComponentImpl2".into(),
            status: HealthStatus::Ok {
                struct_name: "MockComponentImpl2",
                message: "ok".into(),
            },
        },
        HealthStatusDescription {
            namespace: "/nativelink/namespace3/MockComponentImpl3".into(),
            status: HealthStatus::Ok {
                struct_name: "MockComponentImpl3",
                message: "ok".into(),
            },
        },
    ]);

    assert_eq!(vec_to_set(health_status), expected_health_status);

    Ok(())
}

#[nativelink_test]
async fn indicator_checks_run_in_parallel() -> Result<(), Error> {
    const NUM_INDICATORS: usize = 3;

    struct BarrierIndicator {
        name: &'static str,
        barrier: Arc<Barrier>,
    }

    #[async_trait::async_trait]
    impl HealthStatusIndicator for BarrierIndicator {
        fn get_name(&self) -> &'static str {
            self.name
        }

        async fn check_health(&self, _namespace: Cow<'static, str>) -> HealthStatus {
            // Only returns once all NUM_INDICATORS checks are in flight at
            // the same time. With serial execution no check could ever get
            // past this point before its per-indicator timeout fires.
            self.barrier.wait().await;
            HealthStatus::new_ok(self, "ok".into())
        }
    }

    let barrier = Arc::new(Barrier::new(NUM_INDICATORS));
    let mut health_registry_builder = HealthRegistryBuilder::new("nativelink");
    for name in ["indicator1", "indicator2", "indicator3"] {
        health_registry_builder.register_indicator(Arc::new(BarrierIndicator {
            name,
            barrier: barrier.clone(),
        }));
    }

    let health_registry = health_registry_builder.build();
    // The timeout only bounds the failure mode: with serial execution every
    // indicator would block at the barrier until this fires.
    let health_status: Vec<HealthStatusDescription> = health_registry
        .health_status_report(&Duration::from_secs(5))
        .collect()
        .await;

    assert_eq!(health_status.len(), NUM_INDICATORS);
    for description in &health_status {
        assert!(
            matches!(description.status, HealthStatus::Ok { .. }),
            "indicator checks did not run in parallel, got {:?} for {}",
            description.status,
            description.namespace
        );
    }

    Ok(())
}

#[nativelink_test(start_paused = true)]
async fn stuck_indicators_time_out_concurrently() -> Result<(), Error> {
    const NUM_INDICATORS: u32 = 3;
    const TIMEOUT_LIMIT: Duration = Duration::from_secs(1);

    struct StuckIndicator {
        name: &'static str,
    }

    #[async_trait::async_trait]
    impl HealthStatusIndicator for StuckIndicator {
        fn get_name(&self) -> &'static str {
            self.name
        }

        async fn check_health(&self, _namespace: Cow<'static, str>) -> HealthStatus {
            futures::future::pending().await
        }
    }

    let mut health_registry_builder = HealthRegistryBuilder::new("nativelink");
    for name in ["indicator1", "indicator2", "indicator3"] {
        health_registry_builder.register_indicator(Arc::new(StuckIndicator { name }));
    }

    let health_registry = health_registry_builder.build();
    let start = Instant::now();
    let health_status: Vec<HealthStatusDescription> = health_registry
        .health_status_report(&TIMEOUT_LIMIT)
        .collect()
        .await;
    let elapsed = start.elapsed();

    assert_eq!(health_status.len(), NUM_INDICATORS as usize);
    for description in &health_status {
        assert!(
            matches!(description.status, HealthStatus::Timeout { .. }),
            "expected Timeout, got {:?} for {}",
            description.status,
            description.namespace
        );
    }

    // Tokio time is paused, so elapsed virtual time is deterministic. The
    // per-indicator timeouts must overlap: a full report takes ~one timeout
    // total, not NUM_INDICATORS timeouts back to back.
    assert!(
        elapsed < TIMEOUT_LIMIT * NUM_INDICATORS,
        "indicator timeouts did not overlap: {elapsed:?} elapsed for {NUM_INDICATORS} indicators with a {TIMEOUT_LIMIT:?} timeout each"
    );

    Ok(())
}

#[macro_export]
macro_rules! generate_health_status_indicator {
    ($struct_name:ident, $health_status:ident, $status_msg:expr) => {
        struct $struct_name;

        #[async_trait::async_trait]
        impl HealthStatusIndicator for $struct_name {
            fn get_name(&self) -> &'static str {
                stringify!($struct_name).into()
            }
            async fn check_health(&self, _namespace: Cow<'static, str>) -> HealthStatus {
                HealthStatus::$health_status {
                    struct_name: stringify!($struct_name).into(),
                    message: $status_msg.into(),
                }
            }
        }
    };
}

fn vec_to_set(vec: Vec<HealthStatusDescription>) -> HashSet<HealthStatusDescription> {
    HashSet::from_iter(vec)
}
