// Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::watch;

use action_messages::{ActionInfo, ActionInfoHashKey, ActionState};
use config::schedulers::PropertyModification;
use error::Error;
use platform_property_manager::{PlatformPropertyManager, PlatformPropertyValue};
use scheduler::ActionScheduler;

pub struct PropertyModifierScheduler {
    modifications: Vec<config::schedulers::PropertyModification>,
    scheduler: Arc<dyn ActionScheduler>,
}

impl PropertyModifierScheduler {
    pub fn new(config: &config::schedulers::PropertyModifierScheduler, scheduler: Arc<dyn ActionScheduler>) -> Self {
        Self {
            modifications: config.modifications.clone(),
            scheduler,
        }
    }
}

#[async_trait]
impl ActionScheduler for PropertyModifierScheduler {
    async fn get_platform_property_manager(&self, instance_name: &str) -> Result<Arc<PlatformPropertyManager>, Error> {
        self.scheduler.get_platform_property_manager(instance_name).await
    }

    async fn add_action(&self, mut action_info: ActionInfo) -> Result<watch::Receiver<Arc<ActionState>>, Error> {
        for modification in &self.modifications {
            match modification {
                PropertyModification::Add(addition) => action_info.platform_properties.properties.insert(
                    addition.name.clone(),
                    PlatformPropertyValue::Unknown(addition.value.clone()),
                ),
                PropertyModification::Remove(name) => action_info.platform_properties.properties.remove(name),
            };
        }
        self.scheduler.add_action(action_info).await
    }

    async fn find_existing_action(
        &self,
        unique_qualifier: &ActionInfoHashKey,
    ) -> Option<watch::Receiver<Arc<ActionState>>> {
        self.scheduler.find_existing_action(unique_qualifier).await
    }

    async fn clean_recently_completed_actions(&self) {
        self.scheduler.clean_recently_completed_actions().await
    }
}
