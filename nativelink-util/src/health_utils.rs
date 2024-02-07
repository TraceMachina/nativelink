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
use std::collections::vec_deque::VecDeque;
use std::fmt::Debug;
use std::marker::Send;
use std::sync::Arc;

use async_trait::async_trait;
use futures::stream::unfold;
use futures::Stream;
use serde::Serialize;

/// Struct name health indicator component.
type StructName = str;
/// Readable message status of the health indicator.
type Message = str;

/// Status state of a health indicator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum HealthStatus {
    Ok(&'static StructName, Cow<'static, Message>),
    Initializing(&'static StructName, Cow<'static, Message>),
    Warning(&'static StructName, Cow<'static, Message>),
    Failed(&'static StructName, Cow<'static, Message>),
}

impl HealthStatus {
    /// Make a healthy status.
    pub fn new_ok(component: &(impl HealthStatusIndicator + ?Sized), message: Cow<'static, str>) -> Self {
        Self::Ok(component.struct_name(), message)
    }

    /// Make an initializing status.
    pub fn new_initializing(
        component: &(impl HealthStatusIndicator + ?Sized),
        message: Cow<'static, str>,
    ) -> HealthStatus {
        Self::Initializing(component.struct_name(), message)
    }

    /// Make a warning status.
    pub fn new_warning(component: &(impl HealthStatusIndicator + ?Sized), message: Cow<'static, str>) -> HealthStatus {
        Self::Warning(component.struct_name(), message)
    }

    /// Make a failed status.
    pub fn new_failed(component: &(impl HealthStatusIndicator + ?Sized), message: Cow<'static, str>) -> HealthStatus {
        Self::Failed(component.struct_name(), message)
    }
}

/// Description of the health status of a component.
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
    /// Returns the name of the struct implementing the trait.
    fn struct_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Check the health status of the component. This function should be
    /// implemented by the component to check the health status of the component.
    async fn check_health(&self, _namespace: Cow<'static, str>) -> HealthStatus {
        HealthStatus::new_ok(self, "ok".into())
    }
}

#[derive(Default, Clone)]
pub struct HealthRegistryRoot {
    registry: HealthRegistry,
}

impl HealthRegistryRoot {
    pub fn new(namespace: Cow<'static, str>) -> Self {
        Self {
            registry: HealthRegistry::new(namespace),
        }
    }

    pub fn register_indicator(&mut self, indicator: Arc<dyn HealthStatusIndicator>) {
        self.registry.register_indicator(indicator);
    }

    pub fn sub_registry(&mut self, namespace: Cow<'static, str>) -> &mut HealthRegistry {
        self.registry.sub_registry(namespace)
    }

    pub fn get_health_status_report(&mut self) -> impl Stream<Item = HealthStatusDescription> {
        self.registry.get_health_status_report()
    }
}

/// Health registry to register health status indicators and sub-registries.
/// Internal representation of the health registry is a tree structure.
#[derive(Default, Clone)]
pub struct HealthRegistry {
    namespace: Cow<'static, str>,
    indicators: Vec<Arc<dyn HealthStatusIndicator>>,
    registries: Vec<HealthRegistry>,
}

/// Health registry implementation provides creation and registration
/// of health status indicators. It also provides a method to get the
/// health status report of the component and its sub-components.
impl HealthRegistry {
    /// Create a new health registry with a given namespace.
    fn new(namespace: Cow<'static, str>) -> Self {
        Self {
            namespace,
            ..Default::default()
        }
    }

    /// Register a health status indicator at the current component level.
    pub fn register_indicator(&mut self, indicator: Arc<dyn HealthStatusIndicator>) {
        self.indicators.push(indicator);
    }

    /// Create a sub-registry with the given namespace and return a mutable reference to it.
    pub fn sub_registry(&mut self, namespace: Cow<'static, str>) -> &mut Self {
        let registry = HealthRegistry::new(namespace);

        self.registries.push(registry);
        self.registries
            .last_mut()
            .expect("dependencies should not to be empty.")
    }

    /// Get the health status report of the component and its sub-components.
    /// The health status report is returned as a stream of health status descriptions.
    /// The stream is a lazy stream and the health status is checked only when the stream is polled.
    fn get_health_status_report(&mut self) -> impl Stream<Item = HealthStatusDescription> {
        type Queue = VecDeque<(Cow<'static, str>, Arc<dyn HealthStatusIndicator>)>;
        // Recursive function to flatten the health registry tree structure into a queue.
        fn flatten(
            queue: &mut Queue,
            parent_namespace: &str,
            namespace: &str,
            indicators: &[Arc<dyn HealthStatusIndicator>],
            registries: &[HealthRegistry],
        ) {
            let parent_namespace: Cow<'static, str> = Cow::Owned(format!("{parent_namespace}/{namespace}"));
            // Append to end of queue.
            for indicator in indicators {
                queue.push_back((parent_namespace.clone(), indicator.clone()));
            }

            // Visit sub-registries and flatten them.
            for registry in registries {
                flatten(
                    queue,
                    &parent_namespace,
                    &registry.namespace,
                    &registry.indicators,
                    &registry.registries,
                );
            }
        }

        let mut queue: Queue = VecDeque::new();
        let parent_namespace: Cow<'static, str> = "".into();
        let namespace = &self.namespace;
        let indicators = &self.indicators[..];
        let registries = &self.registries[..];

        flatten(&mut queue, &parent_namespace, namespace, indicators, registries);

        // Provide a stream of health status descriptions.
        // Indicators are checked when the stream is polled.
        Box::pin(unfold(queue, |mut state| async {
            let queue_item = state.pop_front()?;
            let health_status = HealthStatusDescription {
                namespace: queue_item.0.clone(),
                status: queue_item.1.check_health(queue_item.0).await,
            };
            Some((health_status, state))
        }))
    }
}
