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
use std::collections::{HashMap, HashSet};
use std::fs;
use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use nativelink_config::schedulers::{HistoricalResourceSpec, SizeClassSpec, TimeoutClassSpec};
use nativelink_error::{Error, ResultExt, make_input_err};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent, RootMetricsComponent,
};
use nativelink_util::action_messages::{ActionInfo, OperationId};
use nativelink_util::metrics::record_hint_resolution;
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, OperationFilter,
};
use nativelink_util::origin_event::{BAZEL_METADATA_KEY, request_metadata_from_baggage};
use opentelemetry::baggage::BaggageExt;
use opentelemetry::context::Context;
use parking_lot::Mutex;
use serde::Deserialize;
use tracing::{debug, warn};

use crate::known_platform_property_provider::KnownPlatformPropertyProvider;

#[derive(Debug, Clone, Default, Hash, Eq, PartialEq)]
struct HintKey {
    target_id: Option<String>,
    action_mnemonic: Option<String>,
    /// The action's `Command` digest as `hash-size`: an identity every
    /// client has, whether or not it sends `RequestMetadata`.
    command_digest: Option<String>,
    /// The `Action` digest as `hash-size`: what origin events carry, so
    /// what a producer fed by them can key on. Narrower than the command
    /// digest (the inputs are part of it).
    action_digest: Option<String>,
}

/// One hint. The v1 fields keep their meaning; the v2 fields are optional,
/// so a v1 file still loads. `class` names a point on the configured
/// ladder and stands in for any number the hint leaves out.
#[derive(Debug, Clone, Deserialize)]
struct HistoricalResourceHint {
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default)]
    action_mnemonic: Option<String>,
    #[serde(default)]
    command_digest: Option<String>,
    #[serde(default)]
    action_digest: Option<String>,
    #[serde(default)]
    cpu_count: Option<u64>,
    #[serde(default)]
    memory_kb: Option<u64>,
    #[serde(default)]
    memory_mib: Option<u64>,
    #[serde(default)]
    disk_kb: Option<u64>,
    #[serde(default)]
    class: Option<String>,
    /// Producer bookkeeping, carried for records.
    #[serde(default)]
    #[expect(dead_code, reason = "accepted from v2 files; not yet read")]
    samples: Option<u64>,
    #[serde(default)]
    #[expect(dead_code, reason = "accepted from v2 files; not yet read")]
    last_seen_s: Option<u64>,
}

/// `hash/size` and `hash-size` both name the same digest.
fn normalize_digest_key(digest: &str) -> String {
    digest.trim().replace('/', "-")
}

impl HistoricalResourceHint {
    /// Every key the lookup ladder could reach this hint by, one per
    /// family it names: the target (with the mnemonic when it has one),
    /// the action digest, the command digest, and the mnemonic alone when
    /// there is no target. A record naming a target and a digest is found
    /// by either; a single composite key would be found by neither.
    fn keys(&self) -> Vec<HintKey> {
        let digest =
            |value: Option<&str>| value.map(normalize_digest_key).filter(|d| !d.is_empty());
        let mut keys = Vec::new();
        if self.target_id.is_some() {
            keys.push(HintKey {
                target_id: self.target_id.clone(),
                action_mnemonic: self.action_mnemonic.clone(),
                ..HintKey::default()
            });
        } else if self.action_mnemonic.is_some() {
            keys.push(HintKey {
                action_mnemonic: self.action_mnemonic.clone(),
                ..HintKey::default()
            });
        }
        if let Some(action_digest) = digest(self.action_digest.as_deref()) {
            keys.push(HintKey {
                action_digest: Some(action_digest),
                ..HintKey::default()
            });
        }
        if let Some(command_digest) = digest(self.command_digest.as_deref()) {
            keys.push(HintKey {
                command_digest: Some(command_digest),
                ..HintKey::default()
            });
        }
        keys
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
}

/// The numbers an action leaves `add_action` with, before they become
/// minimum properties. Zero means the dimension is not set.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Numbers {
    cpu_count: u64,
    memory_kb: u64,
    disk_kb: u64,
}

impl From<&SizeClassSpec> for Numbers {
    fn from(class: &SizeClassSpec) -> Self {
        Self {
            cpu_count: class.cpu_count,
            memory_kb: class.memory_kb,
            disk_kb: class.disk_kb,
        }
    }
}

/// The dimensions this fleet reserves: the ones its ladder or its cold
/// start names. A hint may state any dimension its producer measured, and a
/// minimum on a dimension no worker advertises makes the action
/// unsatisfiable, so a hint's number on a dimension the fleet does not
/// reserve is dropped. A configuration with neither a ladder nor a cold
/// start reserves whatever its hints say, as it always did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Reserved {
    cpu: bool,
    memory: bool,
    disk: bool,
}

impl Reserved {
    fn from_spec(spec: &HistoricalResourceSpec) -> Self {
        let cold = spec.cold_start.unwrap_or_default();
        let sized = !spec.classes.is_empty()
            || cold.cpu_count > 0
            || cold.memory_kb > 0
            || cold.disk_kb > 0;
        if !sized {
            return Self {
                cpu: true,
                memory: true,
                disk: true,
            };
        }
        Self {
            cpu: cold.cpu_count > 0 || spec.classes.iter().any(|c| c.cpu_count > 0),
            memory: cold.memory_kb > 0 || spec.classes.iter().any(|c| c.memory_kb > 0),
            disk: cold.disk_kb > 0 || spec.classes.iter().any(|c| c.disk_kb > 0),
        }
    }
}

pub struct HistoricalResourceScheduler {
    hints_file: String,
    refresh_interval: Duration,
    cpu_property_name: String,
    memory_property_name: String,
    disk_property_name: String,
    reserved: Reserved,
    cold_start: Numbers,
    classes: Vec<SizeClassSpec>,
    default_class: Option<String>,
    class_by_mnemonic: HashMap<String, String>,
    class_by_timeout: Vec<TimeoutClassSpec>,
    scheduler: Option<Arc<dyn KnownPlatformPropertyProvider>>,
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
            .field("cold_start", &self.cold_start)
            .field("classes", &self.classes)
            .field("default_class", &self.default_class)
            .field("known_properties", &self.known_properties)
            .finish_non_exhaustive()
    }
}

impl HistoricalResourceScheduler {
    #[must_use]
    pub fn new(
        spec: &HistoricalResourceSpec,
        scheduler: Arc<dyn KnownPlatformPropertyProvider>,
    ) -> Self {
        Self {
            hints_file: spec.hints_file.clone(),
            refresh_interval: Duration::from_secs(spec.refresh_interval_s),
            cpu_property_name: spec.cpu_property_name.clone(),
            memory_property_name: spec.memory_property_name.clone(),
            disk_property_name: spec.disk_property_name.clone(),
            reserved: Reserved::from_spec(spec),
            cold_start: spec
                .cold_start
                .as_ref()
                .map_or_else(Numbers::default, |c| Numbers {
                    cpu_count: c.cpu_count,
                    memory_kb: c.memory_kb,
                    disk_kb: c.disk_kb,
                }),
            classes: spec.classes.clone(),
            default_class: spec.default_class.clone(),
            class_by_mnemonic: spec.class_by_mnemonic.clone(),
            class_by_timeout: spec.class_by_timeout.clone(),
            scheduler: Some(scheduler),
            known_properties: Mutex::new(HashMap::new()),
            hint_state: Mutex::default(),
        }
    }

    fn refresh_hints(&self) {
        let now = Instant::now();
        {
            let hint_state = self.hint_state.lock();
            if let Some(last_loaded) = hint_state.last_loaded
                && (self.refresh_interval.is_zero()
                    || now.duration_since(last_loaded) < self.refresh_interval)
            {
                return;
            }
        }

        let hints_file_contents = match fs::read_to_string(&self.hints_file) {
            Ok(contents) => contents,
            Err(err) => {
                warn!(?err, hints_file = %self.hints_file, "Failed to read historical resource hints");
                self.hint_state.lock().last_loaded = Some(now);
                return;
            }
        };
        let hints = match serde_json::from_str::<HistoricalResourceHintFile>(&hints_file_contents) {
            Ok(hints_file) => hints_file.into_hints(),
            Err(err) => {
                warn!(?err, hints_file = %self.hints_file, "Failed to parse historical resource hints");
                self.hint_state.lock().last_loaded = Some(now);
                return;
            }
        };
        // A class the ladder does not have is dropped here, once per name
        // per load, so the hot path never logs it; the hint keeps whatever
        // numbers it states.
        let mut unknown_classes = HashSet::new();
        // Likewise a number on a dimension the fleet does not reserve, once
        // per dimension per load: a producer measures what it can, and a
        // minimum no worker advertises would make the action unsatisfiable.
        let mut dropped_dimensions = HashSet::new();
        let mut hints: Vec<HistoricalResourceHint> = hints
            .into_iter()
            .map(|mut hint| {
                if let Some(name) = &hint.class
                    && self.class(name).is_none()
                {
                    if unknown_classes.insert(name.clone()) {
                        warn!(
                            class = %name,
                            hints_file = %self.hints_file,
                            "Hints name a size class the ladder does not have; the class is ignored"
                        );
                    }
                    hint.class = None;
                }
                let dropped = [
                    (
                        !self.reserved.cpu && hint.cpu_count.is_some(),
                        &self.cpu_property_name,
                    ),
                    (
                        !self.reserved.memory && hint.memory_kb().is_some(),
                        &self.memory_property_name,
                    ),
                    (
                        !self.reserved.disk && hint.disk_kb.is_some(),
                        &self.disk_property_name,
                    ),
                ];
                for (drop, property) in dropped {
                    if drop && dropped_dimensions.insert(property.clone()) {
                        warn!(
                            property = %property,
                            hints_file = %self.hints_file,
                            "Hints carry a dimension neither the ladder nor the cold start reserves; it is ignored"
                        );
                    }
                }
                if !self.reserved.cpu {
                    hint.cpu_count = None;
                }
                if !self.reserved.memory {
                    hint.memory_kb = None;
                    hint.memory_mib = None;
                }
                if !self.reserved.disk {
                    hint.disk_kb = None;
                }
                hint
            })
            .collect();
        // A record naming fewer families is the more general one, so it
        // owns a shared key: sort so those are inserted last.
        hints.sort_by_key(|hint| core::cmp::Reverse(hint.keys().len()));
        let hints = hints
            .into_iter()
            .flat_map(|hint| hint.keys().into_iter().map(move |key| (key, hint.clone())))
            .collect::<HashMap<_, _>>();
        debug!(hints_file = %self.hints_file, hints = hints.len(), "Loaded historical resource hints");
        let mut hint_state = self.hint_state.lock();
        hint_state.hints = hints;
        hint_state.last_loaded = Some(now);
    }

    /// The Bazel request metadata for the current action, if the client
    /// sent any: `(target_id, action_mnemonic)`, each empty string as `None`.
    fn current_metadata() -> (Option<String>, Option<String>) {
        let ctx = Context::current();
        let Some(metadata) = ctx
            .baggage()
            .get(BAZEL_METADATA_KEY)
            .and_then(|value| request_metadata_from_baggage(value.as_str()).ok())
        else {
            return (None, None);
        };
        (
            non_empty_string(metadata.target_id),
            non_empty_string(metadata.action_mnemonic),
        )
    }

    /// The key ladder, most specific first: `(target, mnemonic)`, then
    /// `target`, then the action digest, then the command digest, then the
    /// mnemonic alone. The digest keys are what make hints reach clients
    /// that send no `RequestMetadata`, and the same command regardless of
    /// who runs it.
    fn hint_for_action(
        &self,
        action_info: &ActionInfo,
        target_id: Option<&String>,
        action_mnemonic: Option<&String>,
    ) -> Option<(HistoricalResourceHint, &'static str)> {
        self.refresh_hints();
        let hint_state = self.hint_state.lock();
        if hint_state.hints.is_empty() {
            return None;
        }
        let by = |target_id: Option<&String>,
                  action_mnemonic: Option<&String>,
                  command_digest: Option<String>,
                  action_digest: Option<String>| {
            hint_state.hints.get(&HintKey {
                target_id: target_id.cloned(),
                action_mnemonic: action_mnemonic.cloned(),
                command_digest,
                action_digest,
            })
        };
        if target_id.is_some()
            && action_mnemonic.is_some()
            && let Some(hint) = by(target_id, action_mnemonic, None, None)
        {
            return Some((hint.clone(), "target"));
        }
        if target_id.is_some()
            && let Some(hint) = by(target_id, None, None, None)
        {
            return Some((hint.clone(), "target"));
        }
        if let Some(hint) = by(None, None, None, Some(action_info.digest().to_string())) {
            return Some((hint.clone(), "action_digest"));
        }
        if let Some(hint) = by(
            None,
            None,
            Some(action_info.command_digest.to_string()),
            None,
        ) {
            return Some((hint.clone(), "command_digest"));
        }
        if action_mnemonic.is_some()
            && let Some(hint) = by(None, action_mnemonic, None, None)
        {
            return Some((hint.clone(), "mnemonic"));
        }
        None
    }

    fn class(&self, name: &str) -> Option<&SizeClassSpec> {
        self.classes.iter().find(|class| class.name == name)
    }

    /// A hint's numbers: what it states, with its class filling anything
    /// it leaves out. An unknown class was dropped at load.
    fn numbers_from_hint(&self, hint: &HistoricalResourceHint) -> Numbers {
        let from_class = hint
            .class
            .as_deref()
            .and_then(|name| self.class(name))
            .map(Numbers::from)
            .unwrap_or_default();
        Numbers {
            cpu_count: hint.cpu_count.unwrap_or(from_class.cpu_count),
            memory_kb: hint.memory_kb().unwrap_or(from_class.memory_kb),
            disk_kb: hint.disk_kb.unwrap_or(from_class.disk_kb),
        }
    }

    /// Raises the action's minimums to `numbers`; never lowers a value the
    /// client or an earlier step set.
    fn apply_minimums(&self, action_info: &mut ActionInfo, numbers: Numbers) {
        apply_minimum_platform_property(
            &mut action_info.platform_properties,
            &self.cpu_property_name,
            numbers.cpu_count,
        );
        apply_minimum_platform_property(
            &mut action_info.platform_properties,
            &self.memory_property_name,
            numbers.memory_kb,
        );
        apply_minimum_platform_property(
            &mut action_info.platform_properties,
            &self.disk_property_name,
            numbers.disk_kb,
        );
    }

    /// The cold-start policy for an action no hint matched: a class by
    /// mnemonic, else by timeout, else the default class; without classes,
    /// the raw `cold_start` numbers. Only absent properties are filled.
    fn cold_start_numbers(
        &self,
        action_info: &ActionInfo,
        action_mnemonic: Option<&String>,
    ) -> Numbers {
        let by_mnemonic = action_mnemonic.and_then(|m| self.class_by_mnemonic.get(m));
        let by_timeout = if action_info.timeout.is_zero() {
            None
        } else {
            self.class_by_timeout
                .iter()
                .filter(|rule| Duration::from_secs(rule.min_timeout_s) <= action_info.timeout)
                .max_by_key(|rule| rule.min_timeout_s)
                .map(|rule| &rule.class)
        };
        // The rules were checked against the ladder at load; the first
        // candidate that names a class on it wins.
        [by_mnemonic, by_timeout, self.default_class.as_ref()]
            .into_iter()
            .flatten()
            .find_map(|name| self.class(name))
            .map_or(self.cold_start, Numbers::from)
    }

    /// The cold-start rules and the default class must name classes on the
    /// ladder, or the action they were written for gets nothing.
    pub fn validate(spec: &HistoricalResourceSpec) -> Result<(), Error> {
        let known = |name: &str| spec.classes.iter().any(|class| class.name == name);
        let mut rules = spec
            .class_by_mnemonic
            .iter()
            .map(|(mnemonic, class)| (format!("class_by_mnemonic[{mnemonic}]"), class.as_str()))
            .chain(spec.class_by_timeout.iter().map(|rule| {
                (
                    format!("class_by_timeout[{}]", rule.min_timeout_s),
                    rule.class.as_str(),
                )
            }))
            .chain(
                spec.default_class
                    .iter()
                    .map(|class| ("default_class".to_string(), class.as_str())),
            );
        if let Some((rule, class)) = rules.find(|(_, class)| !known(class)) {
            return Err(make_input_err!(
                "historical_resource.{rule} names size class {class:?}, which classes does not define"
            ));
        }
        Ok(())
    }

    fn apply_defaults(&self, action_info: &mut ActionInfo, numbers: Numbers) {
        apply_default_platform_property(
            &mut action_info.platform_properties,
            &self.cpu_property_name,
            numbers.cpu_count,
        );
        apply_default_platform_property(
            &mut action_info.platform_properties,
            &self.memory_property_name,
            numbers.memory_kb,
        );
        apply_default_platform_property(
            &mut action_info.platform_properties,
            &self.disk_property_name,
            numbers.disk_kb,
        );
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
            .as_ref()
            .err_tip(|| "Inner scheduler does not implement KnownPlatformPropertyProvider for HistoricalResourceScheduler")?;
        let mut known_properties = HashSet::<String>::from_iter(
            known_platform_property_provider
                .get_known_properties(instance_name)
                .await?,
        );
        known_properties.insert(self.cpu_property_name.clone());
        known_properties.insert(self.memory_property_name.clone());
        known_properties.insert(self.disk_property_name.clone());
        let final_known_properties: Vec<String> = known_properties.into_iter().collect();
        self.known_properties
            .lock()
            .insert(instance_name.to_string(), final_known_properties.clone());
        Ok(final_known_properties)
    }
}

fn non_empty_string(value: String) -> Option<String> {
    if value.is_empty() { None } else { Some(value) }
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

fn apply_default_platform_property(
    platform_properties: &mut HashMap<String, String>,
    property_name: &str,
    default_value: u64,
) {
    if default_value == 0 || platform_properties.contains_key(property_name) {
        return;
    }
    platform_properties.insert(property_name.to_string(), default_value.to_string());
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
        let (target_id, action_mnemonic) = Self::current_metadata();
        // A hint that comes out all zero (bookkeeping fields only, or a
        // class that was dropped at load) reserves nothing, so it is no
        // hint: the cold-start policy applies and the metric says so.
        let hint = self
            .hint_for_action(&action_info, target_id.as_ref(), action_mnemonic.as_ref())
            .map(|(hint, source)| (self.numbers_from_hint(&hint), source))
            .filter(|(numbers, _)| *numbers != Numbers::default());
        if let Some((numbers, source)) = hint {
            self.apply_minimums(Arc::make_mut(&mut action_info), numbers);
            record_hint_resolution(source);
        } else {
            let numbers = self.cold_start_numbers(&action_info, action_mnemonic.as_ref());
            if numbers == Numbers::default() {
                record_hint_resolution("none");
            } else {
                self.apply_defaults(Arc::make_mut(&mut action_info), numbers);
                record_hint_resolution("cold_start");
            }
        }
        self.scheduler
            .as_ref()
            .err_tip(|| "Inner scheduler not available for HistoricalResourceScheduler")?
            .add_action(client_operation_id, action_info)
            .await
    }

    async fn filter_operations<'a>(
        &'a self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream<'a>, Error> {
        self.scheduler
            .as_ref()
            .err_tip(|| "Inner scheduler not available for HistoricalResourceScheduler")?
            .filter_operations(filter)
            .await
    }
}

impl MetricsComponent for HistoricalResourceScheduler {
    fn publish(
        &self,
        kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        match &self.scheduler {
            Some(scheduler) => scheduler.publish(kind, field_metadata),
            None => Ok(MetricPublishKnownKindData::Component),
        }
    }
}

impl RootMetricsComponent for HistoricalResourceScheduler {}
