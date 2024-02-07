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
use std::collections::HashMap;
use std::fmt::Debug;
use std::marker::Send;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures::{Stream, StreamExt};
use parking_lot::Mutex;
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
    fn get_name(&self) -> &'static str;

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

type HealthRegistryBuilderState = Arc<Mutex<HashMap<Cow<'static, str>, Arc<dyn HealthStatusIndicator>>>>;
pub struct HealthRegistryBuilder {
    namespace: Cow<'static, str>,
    state: HealthRegistryBuilderState,
}

impl HealthRegistryBuilder {
    pub fn new(namespace: Cow<'static, str>) -> Self {
        Self {
            namespace: format!("/{}", namespace).into(),
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn register_indicator(&mut self, indicator: Arc<dyn HealthStatusIndicator>) {
        let name = format!("{}/{}", self.namespace, indicator.get_name());
        self.state.lock().insert(name.into(), indicator);
    }

    pub fn sub_builder(&mut self, namespace: Cow<'static, str>) -> HealthRegistryBuilder {
        HealthRegistryBuilder {
            namespace: format!("{}/{}", self.namespace, namespace).into(),
            state: self.state.clone(),
        }
    }

    pub fn build(&mut self) -> HealthRegistry {
        HealthRegistry {
            indicators: self.state.lock().clone().into_iter().collect(),
        }
    }
}

/// Health registry to register health status indicators and sub-registries.
/// Internal representation of the health registry is a tree structure.
#[derive(Default, Clone)]
pub struct HealthRegistry {
    indicators: Vec<(Cow<'static, str>, Arc<dyn HealthStatusIndicator>)>,
}

pub trait HealthStatusReporter {
    fn health_status_report(&self) -> Pin<Box<dyn Stream<Item = HealthStatusDescription> + '_>>;
}

impl HealthStatusReporter for HealthRegistry {
    fn health_status_report(&self) -> Pin<Box<dyn Stream<Item = HealthStatusDescription> + '_>> {
        Box::pin(
            futures::stream::iter(self.indicators.iter()).then(|(namespace, indicator)| async move {
                HealthStatusDescription {
                    namespace: namespace.clone(),
                    status: indicator.check_health(namespace.clone()).await,
                }
            }),
        )
    }
}

#[macro_export]
macro_rules! default_health_status_indicator {
    ($type:ty) => {
        #[async_trait::async_trait]
        impl HealthStatusIndicator for $type {
            fn get_name(&self) -> &'static str {
                stringify!($type)
            }
        }
    };
}
