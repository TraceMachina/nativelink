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
use std::iter::FromIterator;
use std::sync::Arc;

use futures::StreamExt;
use nativelink_error::Error;
use nativelink_util::health_utils::*;

#[cfg(test)]
mod health_utils_tests {

    use pretty_assertions::assert_eq;

    use super::*;

    #[tokio::test]
    async fn create_empty_indicator() -> Result<(), Error> {
        let mut health_registry = HealthRegistryRoot::new("nativelink".into());

        let health_status: Vec<HealthStatusDescription> = health_registry.get_health_status_report().collect().await;

        assert_eq!(health_status.len(), 0);

        Ok(())
    }

    #[tokio::test]
    async fn create_register_indicator() -> Result<(), Error> {
        generate_health_status_indicator!(MockComponentImpl, HealthStatus::Ok, "ok");

        let mut health_registry = HealthRegistryRoot::new("nativelink".into());

        health_registry.register_indicator(Arc::new(MockComponentImpl {}));

        let health_status: Vec<HealthStatusDescription> = health_registry.get_health_status_report().collect().await;

        assert_eq!(health_status.len(), 1);
        assert_eq!(
            health_status,
            vec![HealthStatusDescription {
                namespace: "/nativelink".into(),
                status: HealthStatus::Ok("MockComponentImpl".into(), "ok".into()),
            }]
        );

        Ok(())
    }

    #[tokio::test]
    async fn create_sub_registry() -> Result<(), Error> {
        generate_health_status_indicator!(MockComponentImpl, HealthStatus::Ok, "ok");

        let mut health_registry = HealthRegistryRoot::new("nativelink".into());

        health_registry.register_indicator(Arc::new(MockComponentImpl {}));

        let namespace1_registry = health_registry.sub_registry("namespace1".into());

        namespace1_registry.register_indicator(Arc::new(MockComponentImpl {}));

        let health_status: Vec<HealthStatusDescription> = health_registry.get_health_status_report().collect().await;

        assert_eq!(health_status.len(), 2);
        let expected_health_status = vec![
            HealthStatusDescription {
                namespace: "/nativelink".into(),
                status: HealthStatus::Ok("MockComponentImpl".into(), "ok".into()),
            },
            HealthStatusDescription {
                namespace: "/nativelink/namespace1".into(),
                status: HealthStatus::Ok("MockComponentImpl".into(), "ok".into()),
            },
        ];

        assert_eq!(health_status, expected_health_status);

        Ok(())
    }

    #[tokio::test]
    async fn create_multiple_indicators_same_registry() -> Result<(), Error> {
        generate_health_status_indicator!(MockComponentImpl1, HealthStatus::Ok, "ok");
        generate_health_status_indicator!(MockComponentImpl2, HealthStatus::Ok, "ok");
        generate_health_status_indicator!(MockComponentImpl3, HealthStatus::Ok, "ok");

        let mut health_registry = HealthRegistryRoot::new("nativelink".into());

        health_registry.register_indicator(Arc::new(MockComponentImpl1 {}));
        health_registry.register_indicator(Arc::new(MockComponentImpl2 {}));
        health_registry.register_indicator(Arc::new(MockComponentImpl3 {}));

        let health_status: Vec<HealthStatusDescription> = health_registry.get_health_status_report().collect().await;

        assert_eq!(health_status.len(), 3);
        let expected_health_status = vec_to_set(vec![
            HealthStatusDescription {
                namespace: "/nativelink".into(),
                status: HealthStatus::Ok("MockComponentImpl1".into(), "ok".into()),
            },
            HealthStatusDescription {
                namespace: "/nativelink".into(),
                status: HealthStatus::Ok("MockComponentImpl2".into(), "ok".into()),
            },
            HealthStatusDescription {
                namespace: "/nativelink".into(),
                status: HealthStatus::Ok("MockComponentImpl3".into(), "ok".into()),
            },
        ]);

        assert_eq!(vec_to_set(health_status), expected_health_status);

        Ok(())
    }

    #[tokio::test]
    async fn create_multiple_indicators_with_sub_registry() -> Result<(), Error> {
        generate_health_status_indicator!(MockComponentImpl1, HealthStatus::Ok, "ok");
        generate_health_status_indicator!(MockComponentImpl2, HealthStatus::Ok, "ok");
        generate_health_status_indicator!(MockComponentImpl3, HealthStatus::Ok, "ok");

        let mut health_registry = HealthRegistryRoot::new("nativelink".into());

        health_registry
            .sub_registry("namespace1".into())
            .register_indicator(Arc::new(MockComponentImpl1 {}));

        health_registry
            .sub_registry("namespace2".into())
            .register_indicator(Arc::new(MockComponentImpl2 {}));

        health_registry
            .sub_registry("namespace3".into())
            .register_indicator(Arc::new(MockComponentImpl3 {}));

        let health_status: Vec<HealthStatusDescription> = health_registry.get_health_status_report().collect().await;

        assert_eq!(health_status.len(), 3);
        let expected_health_status = vec_to_set(vec![
            HealthStatusDescription {
                namespace: "/nativelink/namespace1".into(),
                status: HealthStatus::Ok("MockComponentImpl1".into(), "ok".into()),
            },
            HealthStatusDescription {
                namespace: "/nativelink/namespace2".into(),
                status: HealthStatus::Ok("MockComponentImpl2".into(), "ok".into()),
            },
            HealthStatusDescription {
                namespace: "/nativelink/namespace3".into(),
                status: HealthStatus::Ok("MockComponentImpl3".into(), "ok".into()),
            },
        ]);

        assert_eq!(vec_to_set(health_status), expected_health_status);

        Ok(())
    }

    #[macro_export]
    macro_rules! generate_health_status_indicator {
        ($struct_name:ident, $health_status:expr, $status_msg:expr) => {
            struct $struct_name;

            #[async_trait::async_trait]
            impl HealthStatusIndicator for $struct_name {
                async fn check_health(&self, _namespace: Cow<'static, str>) -> HealthStatus {
                    $health_status(stringify!($struct_name).into(), $status_msg.into())
                }
            }
        };
    }

    fn vec_to_set(vec: Vec<HealthStatusDescription>) -> HashSet<HealthStatusDescription> {
        HashSet::from_iter(vec)
    }
}
