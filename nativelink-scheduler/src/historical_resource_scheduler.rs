// Copyright 2026 The NativeLink Authors. All rights reserved.
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
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use nativelink_config::schedulers::HistoricalResourceSpec;
use nativelink_error::{Code, Error, make_err};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent, RootMetricsComponent,
};
use nativelink_util::action_messages::{ActionInfo, OperationId};
use nativelink_util::known_platform_property_provider::KnownPlatformPropertyProvider;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, OperationFilter,
};
use nativelink_util::origin_event::request_metadata_from_context;
use parking_lot::Mutex;
use serde::Deserialize;
use tokio::fs;
use tracing::{debug, warn};

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
struct HintKey {
    target_id: Option<String>,
    action_mnemonic: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct HistoricalResourceHint {
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default)]
    action_mnemonic: Option<String>,
    #[serde(default)]
    cpu_count: Option<u64>,
    #[serde(default)]
    memory_kb: Option<u64>,
    #[serde(default)]
    memory_mib: Option<u64>,
}

impl HistoricalResourceHint {
    fn key(&self) -> Option<HintKey> {
        if self.target_id.is_none() && self.action_mnemonic.is_none() {
            return None;
        }
        Some(HintKey {
            target_id: self.target_id.clone(),
            action_mnemonic: self.action_mnemonic.clone(),
        })
    }

    fn memory_kb(&self) -> Option<u64> {
        self.memory_kb.or_else(|| {
            self.memory_mib
                .map(|memory_mib| memory_mib.saturating_mul(1024))
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum HistoricalResourceHintFile {
    List(Vec<HistoricalResourceHint>),
    Object { hints: Vec<HistoricalResourceHint> },
}

impl HistoricalResourceHintFile {
    fn into_hints(self) -> Vec<HistoricalResourceHint> {
        match self {
            Self::List(hints) | Self::Object { hints } => hints,
        }
    }
}

#[derive(Debug, Default)]
struct HintState {
    hints: HashMap<HintKey, HistoricalResourceHint>,
    last_loaded: Option<Instant>,
    /// When the last refresh attempt was made (success or failure). Used to
    /// throttle retries and provide single-flight loading.
    last_attempt: Option<Instant>,
}

pub struct HistoricalResourceScheduler {
    hints_file: String,
    refresh_interval: Duration,
    cpu_property_name: String,
    memory_property_name: String,
    scheduler: Arc<dyn ClientStateManager>,
    known_properties: Mutex<HashMap<String, Vec<String>>>,
    hint_state: Mutex<HintState>,
}

impl core::fmt::Debug for HistoricalResourceScheduler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HistoricalResourceScheduler")
            .field("hints_file", &self.hints_file)
            .field("refresh_interval", &self.refresh_interval)
            .field("cpu_property_name", &self.cpu_property_name)
            .field("memory_property_name", &self.memory_property_name)
            .field("known_properties", &self.known_properties)
            .finish_non_exhaustive()
    }
}

impl HistoricalResourceScheduler {
    #[must_use]
    pub fn new(spec: &HistoricalResourceSpec, scheduler: Arc<dyn ClientStateManager>) -> Self {
        Self {
            hints_file: spec.hints_file.clone(),
            refresh_interval: Duration::from_secs(spec.refresh_interval_s),
            cpu_property_name: spec.cpu_property_name.clone(),
            memory_property_name: spec.memory_property_name.clone(),
            scheduler,
            known_properties: Mutex::new(HashMap::new()),
            hint_state: Mutex::default(),
        }
    }

    async fn refresh_hints(&self) {
        let now = Instant::now();
        {
            let mut hint_state = self.hint_state.lock();
            // If the hints were loaded recently, no refresh is needed.
            if let Some(last_loaded) = hint_state.last_loaded
                && (self.refresh_interval.is_zero()
                    || now.duration_since(last_loaded) < self.refresh_interval)
            {
                return;
            }
            // Throttle retries: if a recent attempt was made (even on failure),
            // don't retry until the refresh interval elapses. Recording
            // last_attempt while still holding the lock also provides
            // single-flight behavior — concurrent callers that observed an
            // expired last_loaded will see last_attempt already set and return
            // early instead of all reading the file at once.
            if let Some(last_attempt) = hint_state.last_attempt
                && (self.refresh_interval.is_zero()
                    || now.duration_since(last_attempt) < self.refresh_interval)
            {
                return;
            }
            hint_state.last_attempt = Some(now);
        }

        let hints_file_contents = match fs::read_to_string(&self.hints_file).await {
            Ok(contents) => contents,
            Err(err) => {
                warn!(?err, hints_file = %self.hints_file, "Failed to read historical resource hints");
                // last_attempt is already set, so the next call is throttled.
                return;
            }
        };
        let hints = match serde_json::from_str::<HistoricalResourceHintFile>(&hints_file_contents) {
            Ok(hints_file) => hints_file.into_hints(),
            Err(err) => {
                warn!(?err, hints_file = %self.hints_file, "Failed to parse historical resource hints");
                // last_attempt is already set, so the next call is throttled.
                return;
            }
        };
        let hints = hints
            .into_iter()
            .filter_map(|hint| hint.key().map(|key| (key, hint)))
            .collect::<HashMap<_, _>>();
        debug!(hints_file = %self.hints_file, hints = hints.len(), "Loaded historical resource hints");
        let mut hint_state = self.hint_state.lock();
        hint_state.hints = hints;
        hint_state.last_loaded = Some(Instant::now());
    }

    async fn hint_for_current_action(&self) -> Option<HistoricalResourceHint> {
        self.refresh_hints().await;
        let metadata = request_metadata_from_context()?;
        let target_id = (!metadata.target_id.is_empty()).then_some(metadata.target_id);
        let action_mnemonic =
            (!metadata.action_mnemonic.is_empty()).then_some(metadata.action_mnemonic);
        let hint_state = self.hint_state.lock();
        let exact_key = HintKey {
            target_id: target_id.clone(),
            action_mnemonic: action_mnemonic.clone(),
        };
        hint_state
            .hints
            .get(&exact_key)
            .or_else(|| {
                hint_state.hints.get(&HintKey {
                    target_id,
                    action_mnemonic: None,
                })
            })
            .or_else(|| {
                hint_state.hints.get(&HintKey {
                    target_id: None,
                    action_mnemonic,
                })
            })
            .cloned()
    }

    fn apply_hint(&self, action_info: &mut ActionInfo, hint: &HistoricalResourceHint) {
        if let Some(cpu_count) = hint.cpu_count {
            apply_minimum_platform_property(
                &mut action_info.platform_properties,
                &self.cpu_property_name,
                cpu_count,
            );
        }
        if let Some(memory_kb) = hint.memory_kb() {
            apply_minimum_platform_property(
                &mut action_info.platform_properties,
                &self.memory_property_name,
                memory_kb,
            );
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
            .ok_or_else(|| {
                make_err!(
                    Code::Internal,
                    "Inner scheduler does not implement KnownPlatformPropertyProvider for HistoricalResourceScheduler"
                )
            })?;
        let mut known_properties: Vec<String> = known_platform_property_provider
            .get_known_properties(instance_name)
            .await?;
        known_properties.push(self.cpu_property_name.clone());
        known_properties.push(self.memory_property_name.clone());
        known_properties.sort();
        known_properties.dedup();
        self.known_properties
            .lock()
            .insert(instance_name.to_string(), known_properties.clone());
        Ok(known_properties)
    }
}

fn apply_minimum_platform_property(
    platform_properties: &mut HashMap<String, String>,
    property_name: &str,
    minimum_value: u64,
) {
    if minimum_value == 0 {
        return;
    }
    let should_update = platform_properties
        .get(property_name)
        .and_then(|value| value.parse::<u64>().ok())
        .is_none_or(|current_value| current_value < minimum_value);
    if should_update {
        platform_properties.insert(property_name.to_string(), minimum_value.to_string());
    }
}

#[async_trait]
impl KnownPlatformPropertyProvider for HistoricalResourceScheduler {
    async fn get_known_properties(&self, instance_name: &str) -> Result<Vec<String>, Error> {
        self.inner_get_known_properties(instance_name).await
    }
}

#[async_trait]
impl ClientStateManager for HistoricalResourceScheduler {
    async fn add_action(
        &self,
        client_operation_id: OperationId,
        mut action_info: Arc<ActionInfo>,
    ) -> Result<Box<dyn ActionStateResult>, Error> {
        if let Some(hint) = self.hint_for_current_action().await {
            self.apply_hint(Arc::make_mut(&mut action_info), &hint);
        }
        self.scheduler
            .add_action(client_operation_id, action_info)
            .await
    }

    async fn filter_operations<'a>(
        &'a self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream<'a>, Error> {
        self.scheduler.filter_operations(filter).await
    }

    fn as_known_platform_property_provider(&self) -> Option<&dyn KnownPlatformPropertyProvider> {
        Some(self)
    }
}

impl MetricsComponent for HistoricalResourceScheduler {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        self.scheduler.publish(kind, field_metadata)
    }
}

impl RootMetricsComponent for HistoricalResourceScheduler {}
