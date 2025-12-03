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

use core::fmt::Debug;
use core::pin::Pin;
use core::time::Duration;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use parking_lot::Mutex;
use serde::Serialize;
use tokio::time::timeout;
use tracing::warn;

/// Struct name health indicator component.
type StructName = str;
/// Readable message status of the health indicator.
type Message = str;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum HealthStatus {
    Ok {
        struct_name: &'static StructName,
        message: Cow<'static, Message>,
    },
    Initializing {
        struct_name: &'static StructName,
        message: Cow<'static, Message>,
    },
    /// This status is used to indicate a non-fatal issue with the component.
    Warning {
        struct_name: &'static StructName,
        message: Cow<'static, Message>,
    },
    Failed {
        struct_name: &'static StructName,
        message: Cow<'static, Message>,
    },
    Timeout {
        struct_name: &'static StructName,
    },
}

impl HealthStatus {
    pub fn new_ok(
        component: &(impl HealthStatusIndicator + ?Sized),
        message: Cow<'static, str>,
    ) -> Self {
        Self::Ok {
            struct_name: component.struct_name(),
            message,
        }
    }

    pub fn new_initializing(
        component: &(impl HealthStatusIndicator + ?Sized),
        message: Cow<'static, str>,
    ) -> Self {
        Self::Initializing {
            struct_name: component.struct_name(),
            message,
        }
    }

    pub fn new_warning(
        component: &(impl HealthStatusIndicator + ?Sized),
        message: Cow<'static, str>,
    ) -> Self {
        Self::Warning {
            struct_name: component.struct_name(),
            message,
        }
    }

    pub fn new_failed(
        component: &(impl HealthStatusIndicator + ?Sized),
        message: Cow<'static, str>,
    ) -> Self {
        Self::Failed {
            struct_name: component.struct_name(),
            message,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct HealthStatusDescription {
    pub namespace: Cow<'static, str>,
    pub status: HealthStatus,
}

/// Health status indicator trait. This trait is used to define
/// a health status indicator by implementing the `check_health` function.
/// A default implementation is provided for the `check_health` function
/// that returns healthy component.
#[async_trait]
pub trait HealthStatusIndicator: Sync + Send + Unpin {
    fn get_name(&self) -> &'static str;

    /// Returns the name of the struct implementing the trait.
    fn struct_name(&self) -> &'static str {
        core::any::type_name::<Self>()
    }

    /// Check the health status of the component. This function should be
    /// implemented by the component to check the health status of the component.
    async fn check_health(&self, _namespace: Cow<'static, str>) -> HealthStatus;
}

type HealthRegistryBuilderState =
    Arc<Mutex<HashMap<Cow<'static, str>, Arc<dyn HealthStatusIndicator>>>>;

pub struct HealthRegistryBuilder {
    namespace: Cow<'static, str>,
    state: HealthRegistryBuilderState,
}

impl Debug for HealthRegistryBuilder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HealthRegistryBuilder")
            .field("namespace", &self.namespace)
            .finish_non_exhaustive()
    }
}

/// Health registry builder that is used to build a health registry.
/// The builder provides creation, registering of health status indicators,
/// sub-building scoped health registries and building the health registry.
/// `build()` should be called once for finalizing the production of a health registry.
impl HealthRegistryBuilder {
    pub fn new(namespace: &str) -> Self {
        Self {
            namespace: format!("/{namespace}").into(),
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a health status indicator at current namespace.
    pub fn register_indicator(&mut self, indicator: Arc<dyn HealthStatusIndicator>) {
        let name = format!("{}/{}", self.namespace, indicator.get_name());
        self.state.lock().insert(name.into(), indicator);
    }

    /// Create a sub builder for a namespace.
    #[must_use]
    pub fn sub_builder(&mut self, namespace: &str) -> Self {
        Self {
            namespace: format!("{}/{}", self.namespace, namespace).into(),
            state: self.state.clone(),
        }
    }

    /// Finalize the production of the health registry.
    pub fn build(&mut self) -> HealthRegistry {
        HealthRegistry {
            indicators: self.state.lock().clone().into_iter().collect(),
        }
    }
}

#[derive(Default, Clone)]
pub struct HealthRegistry {
    indicators: Vec<(Cow<'static, str>, Arc<dyn HealthStatusIndicator>)>,
}

impl Debug for HealthRegistry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HealthRegistry")
            .field(
                "indicators",
                &self
                    .indicators
                    .iter()
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

pub trait HealthStatusReporter {
    fn health_status_report(
        &self,
        timeout: &Duration,
    ) -> Pin<Box<dyn Stream<Item = HealthStatusDescription> + Send + '_>>;
}

/// Health status reporter implementation for the health registry that provides a stream
/// of health status descriptions.
impl HealthStatusReporter for HealthRegistry {
    fn health_status_report(
        &self,
        timeout_limit: &Duration,
    ) -> Pin<Box<dyn Stream<Item = HealthStatusDescription> + Send + '_>> {
        let local_timeout_limit = Arc::new(*timeout_limit);
        Box::pin(
            futures::stream::iter(
                self.indicators
                    .iter()
                    .zip(core::iter::repeat(local_timeout_limit)),
            )
            .then(|((namespace, indicator), internal_timeout)| async move {
                let status_res =
                    timeout(*internal_timeout, indicator.check_health(namespace.clone())).await;
                HealthStatusDescription {
                    namespace: namespace.clone(),
                    status: status_res.unwrap_or_else(|_| {
                        let struct_name = indicator.struct_name();
                        warn!(struct_name, "Timeout during health check");
                        HealthStatus::Timeout { struct_name }
                    }),
                }
            }),
        )
    }
}

/// Default health status indicator implementation for a component.
/// Generally used for components that don't need custom implementations
/// of the `check_health` function.
#[macro_export]
macro_rules! default_health_status_indicator {
    ($type:ty) => {
        #[async_trait::async_trait]
        impl HealthStatusIndicator for $type {
            fn get_name(&self) -> &'static str {
                stringify!($type)
            }

            async fn check_health(
                &self,
                namespace: std::borrow::Cow<'static, str>,
            ) -> nativelink_util::health_utils::HealthStatus {
                StoreDriver::check_health(Pin::new(self), namespace).await
            }
        }
    };
}

// Re-scoped for the health_utils module.
pub use crate::default_health_status_indicator;
