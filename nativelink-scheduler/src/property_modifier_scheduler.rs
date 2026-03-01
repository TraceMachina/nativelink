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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use nativelink_config::schedulers::{
    PlatformPropertyReplacement, PropertyModification, PropertyModifierSpec,
};
use nativelink_error::{Error, ResultExt};
use nativelink_metric::{MetricsComponent, RootMetricsComponent};
use nativelink_util::action_messages::{ActionInfo, OperationId};
use nativelink_util::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, OperationFilter,
};
use parking_lot::Mutex;

#[derive(MetricsComponent)]
pub struct PropertyModifierScheduler {
    modifications: Vec<PropertyModification>,
    #[metric(group = "scheduler")]
    scheduler: Arc<dyn ClientStateManager>,
    #[metric(group = "property_manager")]
    known_properties: Mutex<HashMap<String, Vec<String>>>,
}

impl core::fmt::Debug for PropertyModifierScheduler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("PropertyModifierScheduler")
            .field("modifications", &self.modifications)
            .field("known_properties", &self.known_properties)
            .finish_non_exhaustive()
    }
}

impl PropertyModifierScheduler {
    pub fn new(spec: &PropertyModifierSpec, scheduler: Arc<dyn ClientStateManager>) -> Self {
        Self {
            modifications: spec.modifications.clone(),
            scheduler,
            known_properties: Mutex::new(HashMap::new()),
        }
    }

    async fn inner_get_known_properties(&self, instance_name: &str) -> Result<Vec<String>, Error> {
        {
            let known_properties = self.known_properties.lock();
            if let Some(property_manager) = known_properties.get(instance_name) {
                return Ok(property_manager.clone());
            }
        }
        let known_platform_property_provider = self
            .scheduler
            .as_known_platform_property_provider()
            .err_tip(|| "Inner scheduler does not implement KnownPlatformPropertyProvider for PropertyModifierScheduler")?;
        let mut known_properties = HashSet::<String>::from_iter(
            known_platform_property_provider
                .get_known_properties(instance_name)
                .await?,
        );
        for modification in &self.modifications {
            match modification {
                PropertyModification::Remove(name)
                | PropertyModification::Replace(PlatformPropertyReplacement { name, .. }) => {
                    known_properties.insert(name.clone());
                }
                PropertyModification::Add(_) => (),
            }
        }
        let final_known_properties: Vec<String> = known_properties.into_iter().collect();
        self.known_properties
            .lock()
            .insert(instance_name.to_string(), final_known_properties.clone());

        Ok(final_known_properties)
    }

    async fn inner_add_action(
        &self,
        client_operation_id: OperationId,
        mut action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        let action_info_mut = Arc::make_mut(&mut action_info);
        for modification in &self.modifications {
            match modification {
                PropertyModification::Add(addition) => {
                    action_info_mut
                        .platform_properties
                        .insert(addition.name.clone(), addition.value.clone());
                }
                PropertyModification::Remove(name) => {
                    action_info_mut.platform_properties.remove(name);
                }
                PropertyModification::Replace(replacement) => {
                    if let Some((existing_name, existing_value)) = action_info_mut
                        .platform_properties
                        .remove_entry(&replacement.name)
                    {
                        if replacement
                            .value
                            .as_ref()
                            .is_none_or(|value| *value == existing_value)
                        {
                            action_info_mut.platform_properties.insert(
                                replacement.new_name.clone(),
                                replacement.new_value.clone().unwrap_or(existing_value),
                            );
                        } else {
                            action_info_mut
                                .platform_properties
                                .insert(existing_name, existing_value);
                        }
                    }
                }
            }
        }
        self.scheduler
            .add_action(client_operation_id, action_info)
            .await
    }

    async fn inner_filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream<'_>, Error> {
        self.scheduler.filter_operations(filter).await
    }
}

#[async_trait]
impl KnownPlatformPropertyProvider for PropertyModifierScheduler {
    async fn get_known_properties(&self, instance_name: &str) -> Result<Vec<String>, Error> {
        self.inner_get_known_properties(instance_name).await
    }
}

#[async_trait]
impl ClientStateManager for PropertyModifierScheduler {
    async fn add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        self.inner_add_action(client_operation_id, action_info)
            .await
    }

    async fn filter_operations<'a>(
        &'a self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream<'a>, Error> {
        self.inner_filter_operations(filter).await
    }

    fn as_known_platform_property_provider(&self) -> Option<&dyn KnownPlatformPropertyProvider> {
        Some(self)
    }
}

impl RootMetricsComponent for PropertyModifierScheduler {}
