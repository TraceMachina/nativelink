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

/// Logical name of the health indicator component.
type HealthComponentName = Cow<'static, str>;
/// Struct name health indicator component.
type StructName = &'static str;
/// Readable message status of the health indicator.
pub type Message = Cow<'static, str>;

/// Status state of a health indicator.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub enum HealthStatus {
    Ok(StructName, Message),
    Initializing(StructName, Message),
    Warning(StructName, Message),
    Failed(StructName, Message),
}

/// Description of the health status of a component.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct HealthStatusDescription {
    pub component_name: HealthComponentName,
    pub status: HealthStatus,
}

/// Health status indicator trait. This trait is used to define
/// a health status indicator by implementing the `check_health` function.
/// A default implementation is provided for the `check_health` function
/// that returns healthy component.
#[async_trait]
pub trait HealthStatusIndicator: Sync + Send + Unpin {
    /// Returns the name of the struct implementing the trait.
    fn struct_name(&self) -> StructName {
        std::any::type_name::<Self>()
    }

    /// Check the health status of the component. This function should be
    /// implemented by the component to check the health status of the component.
    async fn check_health(self: Arc<Self>) -> HealthStatus {
        self.make_ok("ok".into())
    }

    /// Make a healthy status.
    fn make_ok(&self, message: Message) -> HealthStatus {
        HealthStatus::Ok(self.struct_name(), message)
    }

    /// Make an initializing status.
    fn make_initializing(&self, message: Message) -> HealthStatus {
        HealthStatus::Initializing(self.struct_name(), message)
    }

    /// Make a warning status.
    fn make_warning(&self, message: Message) -> HealthStatus {
        HealthStatus::Warning(self.struct_name(), message)
    }

    /// Make a failed status.
    fn make_failed(&self, message: Message) -> HealthStatus {
        HealthStatus::Failed(self.struct_name(), message)
    }
}

/// Health registry to register health status indicators and sub-registries.
/// Internal representation of the health registry is a tree structure.
#[derive(Default, Clone)]
pub struct HealthRegistry {
    component_name: HealthComponentName,
    indicators: Vec<Arc<dyn HealthStatusIndicator>>,
    registries: Vec<HealthRegistry>,
}

/// Health registry implementation provides creation and registration
/// of health status indicators. It also provides a method to get the
/// health status report of the component and its sub-components.
impl HealthRegistry {
    /// Create a new health registry with the given component name.
    pub fn new(component_name: HealthComponentName) -> Self {
        Self {
            component_name,
            ..Default::default()
        }
    }

    /// Register a health status indicator at the current component level.
    pub fn register_indicator(&mut self, indicator: Arc<dyn HealthStatusIndicator>) {
        self.indicators.push(indicator);
    }

    /// Create a sub-registry with the given component name and return a mutable reference to it.
    pub fn sub_registry(&mut self, component_name: HealthComponentName) -> &mut Self {
        let registry = HealthRegistry::new(component_name);

        self.registries.push(registry);
        self.registries
            .last_mut()
            .expect("dependencies should not to be empty.")
    }

    /// Get the health status report of the component and its sub-components.
    /// The health status report is returned as a stream of health status descriptions.
    /// The stream is a lazy stream and the health status is checked only when the stream is polled.
    pub fn get_health_status_report(&mut self) -> impl Stream<Item = HealthStatusDescription> {
        type Queue = VecDeque<(HealthComponentName, Arc<dyn HealthStatusIndicator>)>;
        // Recursive function to flatten the health registry tree structure into a queue.
        fn flatten(
            queue: &mut Queue,
            parent_component_name: &HealthComponentName,
            component_name: &HealthComponentName,
            indicators: &[Arc<dyn HealthStatusIndicator>],
            registries: &[HealthRegistry],
        ) {
            let parent_component_name: Cow<'static, str> =
                Cow::Owned(format!("{parent_component_name}/{component_name}"));
            // Append to end of queue.
            for indicator in indicators {
                queue.push_back((parent_component_name.clone(), indicator.clone()));
            }

            // Visit sub-registries and flatten them.
            for registry in registries {
                flatten(
                    queue,
                    &parent_component_name,
                    &registry.component_name,
                    &registry.indicators,
                    &registry.registries,
                );
            }
        }

        let mut queue: Queue = VecDeque::new();
        let parent_component_name: HealthComponentName = "".into();
        let component_name = &self.component_name;
        let indicators = &self.indicators[..];
        let registries = &self.registries[..];

        flatten(
            &mut queue,
            &parent_component_name,
            component_name,
            indicators,
            registries,
        );

        // Provide a stream of health status descriptions.
        // Indicators are checked when the stream is polled.
        Box::pin(unfold(queue, |mut state| async {
            let queue_item = state.pop_front()?;
            let health_status = HealthStatusDescription {
                component_name: queue_item.0,
                status: queue_item.1.check_health().await,
            };
            Some((health_status, state))
        }))
    }
}
