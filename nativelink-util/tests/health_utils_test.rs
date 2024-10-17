// Copyright 2024 The Native Link Authors. All rights reserved.
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

#[nativelink_test]
async fn create_empty_indicator() -> Result<(), Error> {
    let mut health_registry_builder = HealthRegistryBuilder::new("nativelink");
    let health_registry = health_registry_builder.build();
    let health_status: Vec<HealthStatusDescription> =
        health_registry.health_status_report().collect().await;
    assert_eq!(health_status.len(), 0);
    Ok(())
}

#[nativelink_test]
async fn create_register_indicator() -> Result<(), Error> {
    generate_health_status_indicator!(MockComponentImpl, Ok, "ok");

    let mut health_registry_builder = HealthRegistryBuilder::new("nativelink");

    health_registry_builder.register_indicator(Arc::new(MockComponentImpl {}));

    let health_registry = health_registry_builder.build();
    let health_status: Vec<HealthStatusDescription> =
        health_registry.health_status_report().collect().await;

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
    let health_status: Vec<HealthStatusDescription> =
        health_registry.health_status_report().collect().await;

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
    let health_status: Vec<HealthStatusDescription> =
        health_registry.health_status_report().collect().await;

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
    let health_status: Vec<HealthStatusDescription> =
        health_registry.health_status_report().collect().await;

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
