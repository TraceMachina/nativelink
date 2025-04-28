// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use nativelink_config::schedulers::{PropertyModification, PropertyModifierSpec};
use nativelink_error::{Error, ResultExt};
use nativelink_util::action_messages::{ActionInfo, OperationId};
use nativelink_util::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, OperationFilter,
};
use parking_lot::Mutex;
use tracing::{debug, info, instrument};

pub struct PropertyModifierScheduler {
    modifications: Vec<PropertyModification>,
    scheduler: Arc<dyn ClientStateManager>,
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
    #[instrument(skip(scheduler), level = "debug")]
    pub fn new(spec: &PropertyModifierSpec, scheduler: Arc<dyn ClientStateManager>) -> Self {
        info!(
            modifications_count = spec.modifications.len(),
            "Creating PropertyModifierScheduler"
        );

        Self {
            modifications: spec.modifications.clone(),
            scheduler,
            known_properties: Mutex::new(HashMap::new()),
        }
    }

    #[instrument(skip(self), level = "debug")]
    async fn inner_get_known_properties(&self, instance_name: &str) -> Result<Vec<String>, Error> {
        {
            let known_properties = self.known_properties.lock();
            if let Some(property_manager) = known_properties.get(instance_name) {
                debug!(
                    instance_name,
                    count = property_manager.len(),
                    "Using cached known properties"
                );
                return Ok(property_manager.clone());
            }
        }
        let known_platform_property_provider = self
            .scheduler
            .as_known_platform_property_provider()
            .err_tip(|| "Inner scheduler does not implement KnownPlatformPropertyProvider for PropertyModifierScheduler")?;

        let inner_properties = known_platform_property_provider
            .get_known_properties(instance_name)
            .await?;

        debug!(
            instance_name,
            count = inner_properties.len(),
            "Retrieved inner known properties"
        );

        let mut known_properties = HashSet::<String>::from_iter(inner_properties);

        for modification in &self.modifications {
            match modification {
                PropertyModification::Remove(name) => {
                    known_properties.insert(name.clone());
                    debug!(
                        property = name,
                        "Adding property from remove modification to known properties"
                    );
                }
                PropertyModification::Add(_) => (),
            }
        }
        let final_known_properties: Vec<String> = known_properties.into_iter().collect();
        self.known_properties
            .lock()
            .insert(instance_name.to_string(), final_known_properties.clone());

        info!(
            instance_name,
            count = final_known_properties.len(),
            "Built and cached known properties"
        );

        Ok(final_known_properties)
    }

    #[instrument(
        skip(self, action_info),
        fields(operation_id = ?client_operation_id),
    )]
    async fn inner_add_action(
        &self,
        client_operation_id: OperationId,
        mut action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        debug!(
            action_id = ?action_info.unique_qualifier.digest(),
            original_property_count = action_info.platform_properties.len(),
            "Modifying action properties"
        );

        let action_info_mut = Arc::make_mut(&mut action_info);

        for modification in &self.modifications {
            match modification {
                PropertyModification::Add(addition) => {
                    let previous = action_info_mut
                        .platform_properties
                        .insert(addition.name.clone(), addition.value.clone());

                    if previous.is_some() {
                        debug!(
                            property = &addition.name,
                            "Overriding existing property with add modification"
                        );
                    } else {
                        debug!(property = &addition.name, "Adding new property");
                    }
                }
                PropertyModification::Remove(name) => {
                    let removed = action_info_mut.platform_properties.remove(name);

                    if removed.is_some() {
                        debug!(property = name, "Removed property");
                    } else {
                        debug!(property = name, "Property to remove was not present");
                    }
                }
            };
        }

        info!(
            action_id = ?action_info.unique_qualifier.digest(),
            final_property_count = action_info.platform_properties.len(),
            "Properties modified, passing to inner scheduler"
        );

        self.scheduler
            .add_action(client_operation_id, action_info)
            .await
    }

    #[instrument(skip(self))]
    async fn inner_filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        debug!(?filter, "Passing filter operations to inner scheduler");
        self.scheduler.filter_operations(filter).await
    }
}

#[async_trait]
impl KnownPlatformPropertyProvider for PropertyModifierScheduler {
    #[instrument(skip(self))]
    async fn get_known_properties(&self, instance_name: &str) -> Result<Vec<String>, Error> {
        self.inner_get_known_properties(instance_name).await
    }
}

#[async_trait]
impl ClientStateManager for PropertyModifierScheduler {
    #[instrument(
        skip(self, action_info),
        fields(operation_id = ?client_operation_id),
    )]
    async fn add_action(
        &self,
        client_operation_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        self.inner_add_action(client_operation_id, action_info)
            .await
    }

    #[instrument(skip(self))]
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
