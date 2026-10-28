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

//! Sizes actions from a periodically refreshed hints file by default.
//! With learning enabled, sampled worker usage lands in a [`ResourceSketch`]; an action
//! is reserved what the sketch has learned for its key, shaped with the
//! headroom and the ladder configured here, or by the cold-start policy
//! when the sketch has nothing. The sketch is written to a store so a
//! restart keeps it; a hints file seeds an empty sketch once.

use core::time::Duration;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use nativelink_config::schedulers::{HistoricalResourceSpec, SizeClassSpec, TimeoutClassSpec};
use nativelink_error::{Code, Error, ResultExt, make_input_err};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent, RootMetricsComponent,
};
use nativelink_util::action_messages::{ActionInfo, OperationId};
use nativelink_util::metrics::{
    record_hint_resolution, record_hints_family, record_hints_snapshot,
    record_hints_snapshot_failure,
};
use nativelink_util::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, OperationFilter,
};
use nativelink_util::origin_event::{BAZEL_METADATA_KEY, request_metadata_from_baggage};
use nativelink_util::store_trait::{Store, StoreKey, StoreLike};
use nativelink_util::{background_spawn, spawn_blocking};
use opentelemetry::baggage::BaggageExt;
use opentelemetry::context::Context;
use parking_lot::Mutex;
use serde::Deserialize;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::known_platform_property_provider::KnownPlatformPropertyProvider;
use crate::resource_sketch::{
    Budget, Entry, Family, Policy, ResourceSketch, Shaped, SketchKey, Snapshot, Stats,
};

/// The prefix the sketch snapshots are written under in `snapshot_store`,
/// one key per scheduler replica; see [`snapshot_key`].
pub const SNAPSHOT_KEY_PREFIX: &str = "historical_resource_sketch/";
const SNAPSHOT_KEY_PREFIX_END: &str = "historical_resource_sketch0";
const MAX_SNAPSHOT_CANDIDATES: usize = 128;
const MAX_REPLICA_ID_BYTES: usize = 256;

/// The key the store is probed with at start; outside the prefix, so a
/// listing never reads it as a snapshot.
const SNAPSHOT_PROBE_KEY: &str = "historical_resource_sketch.probe";

/// The snapshot key of one replica.
#[must_use]
pub fn snapshot_key(replica: &str) -> String {
    format!("{SNAPSHOT_KEY_PREFIX}{replica}")
}

/// The replica this process writes its snapshot as: the pod's hostname,
/// else a fresh id. Every replica sees the usage of the workers connected
/// to it and no other. A configured identity takes precedence over the hostname.
#[must_use]
pub fn replica_id(configured: &str) -> String {
    if !configured.is_empty() {
        return configured.to_string();
    }
    std::env::var("HOSTNAME")
        .ok()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string())
}

/// One hint of the seed file. The v1 fields keep their meaning; the v2
/// fields are optional, so a v1 file still loads. `class` names a point on
/// the configured ladder and stands in for any number the hint leaves out.
#[derive(Debug, Clone, Deserialize)]
struct HistoricalResourceHint {
    #[serde(default)]
    target_id: Option<String>,
    #[serde(default)]
    action_mnemonic: Option<String>,
    #[serde(default)]
    command_digest: Option<String>,
    /// Used by file-backed hints. Learning keys by command instead.
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
    #[serde(default)]
    samples: Option<u64>,
    #[serde(default)]
    last_seen_s: Option<u64>,
}

/// `hash/size` and `hash-size` both name the same digest.
fn normalize_digest_key(digest: &str) -> String {
    digest.trim().replace('/', "-")
}

#[derive(Clone, Debug, Default, Hash, PartialEq, Eq)]
struct HintKey {
    target_id: Option<String>,
    action_mnemonic: Option<String>,
    command_digest: Option<String>,
    action_digest: Option<String>,
}

impl HistoricalResourceHint {
    fn keys(&self) -> Vec<HintKey> {
        let mut keys = Vec::new();
        if self.target_id.is_some() || self.action_mnemonic.is_some() {
            keys.push(HintKey {
                target_id: self.target_id.clone(),
                action_mnemonic: self.action_mnemonic.clone(),
                ..HintKey::default()
            });
        }
        for (digest, action) in [
            (self.action_digest.as_deref(), true),
            (self.command_digest.as_deref(), false),
        ] {
            if let Some(digest) = digest.map(normalize_digest_key).filter(|d| !d.is_empty()) {
                keys.push(if action {
                    HintKey {
                        action_digest: Some(digest),
                        ..HintKey::default()
                    }
                } else {
                    HintKey {
                        command_digest: Some(digest),
                        ..HintKey::default()
                    }
                });
            }
        }
        keys
    }

    fn memory_kb(&self) -> Option<u64> {
        self.memory_kb.or_else(|| {
            self.memory_mib
                .map(|memory_mib| memory_mib.saturating_mul(1024))
        })
    }

    /// The sketch key this hint seeds: the target, else the mnemonic, else
    /// the command digest. `None` for a hint that names only an action
    /// digest, which the sketch does not key by.
    fn sketch_key(&self) -> Option<SketchKey> {
        let target = self.target_id.as_deref().filter(|s| !s.is_empty());
        let mnemonic = self.action_mnemonic.as_deref().filter(|s| !s.is_empty());
        let command = self
            .command_digest
            .as_deref()
            .map(normalize_digest_key)
            .filter(|d| !d.is_empty());
        match (target, mnemonic, command) {
            (Some(target), mnemonic, _) => Some(SketchKey::Target {
                target: Arc::from(target),
                mnemonic: mnemonic.map(Arc::from),
            }),
            (None, Some(mnemonic), _) => Some(SketchKey::Mnemonic {
                mnemonic: Arc::from(mnemonic),
            }),
            (None, None, Some(digest)) => Some(SketchKey::Command {
                digest: Arc::from(digest.as_str()),
            }),
            (None, None, None) => None,
        }
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

impl From<&Shaped> for Numbers {
    fn from(shaped: &Shaped) -> Self {
        Self {
            cpu_count: shaped.cpu_count,
            memory_kb: shaped.memory_kb,
            disk_kb: shaped.disk_kb,
        }
    }
}

/// The dimensions this fleet reserves: the ones its ladder or its cold
/// start names. A measurement may cover any dimension the worker samples,
/// and a minimum on a dimension no worker advertises makes the action
/// unsatisfiable, so a number on a dimension the fleet does not reserve is
/// dropped when the hint is shaped. A configuration with neither a ladder
/// nor a cold start reserves whatever was measured, as it always did.
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

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// A complete file generation is swapped under one short lock. File reads,
/// parsing and disposal of the old generation happen outside Execute.
struct FileHints {
    path: String,
    classes: Vec<SizeClassSpec>,
    reserved: Reserved,
    hints: Mutex<HashMap<HintKey, Numbers>>,
}

impl FileHints {
    fn refresh(&self) {
        let parsed = fs::read_to_string(&self.path)
            .map_err(|e| e.to_string())
            .and_then(|contents| {
                serde_json::from_str::<HistoricalResourceHintFile>(&contents)
                    .map_err(|e| e.to_string())
            });
        let mut hints = match parsed {
            Ok(file) => file.into_hints(),
            Err(error) => {
                warn!(%error, "Could not refresh resource hints; keeping previous generation");
                return;
            }
        };
        hints.sort_by_key(|hint| core::cmp::Reverse(hint.keys().len()));
        let mut next = HashMap::new();
        for hint in hints {
            let class = hint
                .class
                .as_ref()
                .and_then(|name| self.classes.iter().find(|c| &c.name == name))
                .map_or_else(Numbers::default, Numbers::from);
            let numbers = Numbers {
                cpu_count: if self.reserved.cpu {
                    hint.cpu_count.unwrap_or(class.cpu_count)
                } else {
                    0
                },
                memory_kb: if self.reserved.memory {
                    hint.memory_kb().unwrap_or(class.memory_kb)
                } else {
                    0
                },
                disk_kb: if self.reserved.disk {
                    hint.disk_kb.unwrap_or(class.disk_kb)
                } else {
                    0
                },
            };
            for key in hint.keys() {
                next.insert(key, numbers);
            }
        }
        let old = { core::mem::replace(&mut *self.hints.lock(), next) };
        drop(old);
    }

    fn new(spec: &HistoricalResourceSpec) -> Arc<Self> {
        let this = Arc::new(Self {
            path: spec.hints_file.clone(),
            classes: spec.classes.clone(),
            reserved: Reserved::from_spec(spec),
            hints: Mutex::new(HashMap::new()),
        });
        if this.path.is_empty() {
            return this;
        }
        this.refresh();
        if spec.refresh_interval_s > 0 {
            let weak = Arc::downgrade(&this);
            let interval = Duration::from_secs(spec.refresh_interval_s);
            background_spawn!("historical_resource_file_refresh", async move {
                loop {
                    tokio::time::sleep(interval).await;
                    let Some(this) = weak.upgrade() else {
                        break;
                    };
                    if let Err(error) =
                        spawn_blocking!("refresh_resource_hints", move || this.refresh()).await
                    {
                        warn!(?error, "Resource hints refresh task failed");
                    }
                }
            });
        }
        this
    }

    fn lookup(
        &self,
        action: &ActionInfo,
        target: Option<&String>,
        mnemonic: Option<&String>,
    ) -> Option<Numbers> {
        let keys = [
            target.zip(mnemonic).map(|(t, m)| HintKey {
                target_id: Some(t.clone()),
                action_mnemonic: Some(m.clone()),
                ..HintKey::default()
            }),
            target.map(|t| HintKey {
                target_id: Some(t.clone()),
                ..HintKey::default()
            }),
            Some(HintKey {
                action_digest: Some(action.digest().to_string()),
                ..HintKey::default()
            }),
            Some(HintKey {
                command_digest: Some(action.command_digest.to_string()),
                ..HintKey::default()
            }),
            mnemonic.map(|m| HintKey {
                action_mnemonic: Some(m.clone()),
                ..HintKey::default()
            }),
        ];
        let hints = self.hints.lock();
        keys.iter()
            .flatten()
            .find_map(|key| hints.get(key).copied())
    }
}

pub struct HistoricalResourceScheduler {
    cpu_property_name: String,
    memory_property_name: String,
    disk_property_name: String,
    cold_start: Numbers,
    classes: Vec<SizeClassSpec>,
    default_class: Option<String>,
    class_by_mnemonic: HashMap<String, String>,
    class_by_timeout: Vec<TimeoutClassSpec>,
    scheduler: Option<Arc<dyn KnownPlatformPropertyProvider>>,
    known_properties: Mutex<HashMap<String, Vec<String>>>,
    sketch: Arc<ResourceSketch>,
    file_hints: Option<Arc<FileHints>>,
}

impl core::fmt::Debug for HistoricalResourceScheduler {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HistoricalResourceScheduler")
            .field("cpu_property_name", &self.cpu_property_name)
            .field("memory_property_name", &self.memory_property_name)
            .field("cold_start", &self.cold_start)
            .field("classes", &self.classes)
            .field("default_class", &self.default_class)
            .field("known_properties", &self.known_properties)
            .field("sketch", &self.sketch)
            .finish_non_exhaustive()
    }
}

impl HistoricalResourceScheduler {
    /// The sketch a spec describes: its headroom, ladder and reserved
    /// dimensions as the shaping policy, its budget and window.
    #[must_use]
    pub fn sketch_from_spec(spec: &HistoricalResourceSpec) -> ResourceSketch {
        let reserved = Reserved::from_spec(spec);
        ResourceSketch::new(
            Policy {
                headroom_percent: spec.headroom_percent,
                classes: spec.classes.clone(),
                reserve_cpu: reserved.cpu,
                reserve_memory: reserved.memory,
                reserve_disk: reserved.disk,
            },
            if spec.learn_from_workers {
                Budget::from_bytes(spec.memory_budget_bytes)
            } else {
                // File-backed mode never receives worker samples. Do not
                // preallocate the learned cache when learning is disabled.
                Budget {
                    target: 1,
                    mnemonic: 1,
                    command: 1,
                    probation: 1,
                }
            },
            Duration::from_secs(spec.window_days.saturating_mul(86_400)),
        )
    }

    /// Wraps `scheduler` with sizing from `sketch`. An empty sketch is
    /// seeded from `spec.hints_file` when one is named; a sketch that was
    /// loaded from a snapshot keeps what it has.
    #[must_use]
    pub fn new(
        spec: &HistoricalResourceSpec,
        scheduler: Arc<dyn KnownPlatformPropertyProvider>,
        sketch: Arc<ResourceSketch>,
    ) -> Self {
        let this = Self {
            cpu_property_name: spec.cpu_property_name.clone(),
            memory_property_name: spec.memory_property_name.clone(),
            disk_property_name: spec.disk_property_name.clone(),
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
            sketch,
            file_hints: (!spec.learn_from_workers).then(|| FileHints::new(spec)),
        };
        if spec.learn_from_workers && !spec.hints_file.is_empty() {
            let stats = this.sketch.stats();
            if stats.entries.iter().sum::<usize>() == 0 {
                this.seed_from_file(&spec.hints_file);
            } else {
                info!(
                    hints_file = %spec.hints_file,
                    "A snapshot was loaded; the hints file is not read"
                );
            }
        }
        this
    }

    /// What this layer sizes from.
    #[must_use]
    pub const fn sketch(&self) -> &Arc<ResourceSketch> {
        &self.sketch
    }

    /// `snapshot_store` must take a string key: a stack that routes by
    /// digest (`size_partitioning`) refuses one. A probe at start writes a
    /// key beside the snapshots and reads it back, so such a stack fails at
    /// start with a message rather than at the first write a minute later.
    pub async fn probe_snapshot_store(store: &Store, name: &str) -> Result<(), Error> {
        const HELP: &str = "it must take a string key: memory, filesystem, redis, s3, gcs or azure, or verify, compression, shard and fast_slow over one; not size_partitioning";
        let bytes = Bytes::from_static(b"resource sketch probe");
        store
            .update_oneshot(StoreKey::Str(SNAPSHOT_PROBE_KEY.into()), bytes.clone())
            .await
            .err_tip(|| {
                format!(
                    "historical_resource.snapshot_store {name:?} refused the probe write; {HELP}"
                )
            })?;
        let read = store
            .get_part_unchunked(StoreKey::Str(SNAPSHOT_PROBE_KEY.into()), 0, None)
            .await
            .err_tip(|| {
                format!(
                    "historical_resource.snapshot_store {name:?} could not read the probe back; {HELP}"
                )
            })?;
        if read != bytes {
            return Err(make_input_err!(
                "historical_resource.snapshot_store {name:?} read a different value back for the probe key; {HELP}"
            ));
        }
        Ok(())
    }

    async fn read_snapshot(
        sketch: &ResourceSketch,
        store: &Store,
        key: &str,
    ) -> Result<Snapshot, Error> {
        let bytes = store
            .get_part_unchunked(
                StoreKey::Str(key.into()),
                0,
                Some(sketch.snapshot_byte_limit()),
            )
            .await?;
        let budget = sketch.budget();
        Snapshot::decode(&bytes, budget.target + budget.mnemonic + budget.command)
    }

    /// Prefer this replica's snapshot. A Deployment replacement has a new
    /// hostname: when its key is absent, restore one newest compatible history
    /// from the same store prefix. Never merge or add together replicas' samples.
    pub async fn load_snapshots(sketch: &ResourceSketch, store: &Store, own_key: &str) {
        match Self::read_snapshot(sketch, store, own_key).await {
            Ok(snapshot) => {
                match sketch.load(snapshot, now_s()) {
                    Ok(entries) => info!(entries, "Restored this replica's resource sketch"),
                    Err(error) => warn!(?error, "Resource sketch not loaded; using seed"),
                }
                return;
            }
            Err(error) if error.code == Code::NotFound => {}
            Err(error) => {
                warn!(
                    ?error,
                    "Could not read this replica's resource sketch; using seed"
                );
                return;
            }
        }

        // Limit both the number and length of retained keys. Redis SCAN may
        // repeat keys, so duplicates must not consume the discovery allowance.
        let mut keys = BTreeSet::new();
        let mut overflow = false;
        let listed = store
            .list(
                StoreKey::Str(SNAPSHOT_KEY_PREFIX.into())
                    ..StoreKey::Str(SNAPSHOT_KEY_PREFIX_END.into()),
                |key| {
                    if let StoreKey::Str(key) = key
                        && key.len() <= SNAPSHOT_KEY_PREFIX.len() + MAX_REPLICA_ID_BYTES
                    {
                        keys.insert(key.to_string());
                        if keys.len() > MAX_SNAPSHOT_CANDIDATES {
                            overflow = true;
                            return false;
                        }
                    }
                    true
                },
            )
            .await;
        if let Err(error) = listed {
            warn!(
                ?error,
                "Cannot discover resource snapshots; use a stable replica_id on stores without listing support. Using seed"
            );
            return;
        }
        if overflow {
            warn!(
                "Too many resource snapshots to discover safely; set a stable replica_id or retire old snapshot keys. Using seed"
            );
            return;
        }
        let mut candidates = Vec::new();
        for key in keys {
            if let Ok(header) = store
                .get_part_unchunked(
                    StoreKey::Str(key.as_str().into()),
                    0,
                    Some(Snapshot::HEADER_BYTES),
                )
                .await
                && let Ok(written_at) = Snapshot::compatible_written_at(&header)
            {
                candidates.push((written_at, key));
            }
        }
        candidates.sort_unstable();
        for (_, key) in candidates.into_iter().rev() {
            let loaded = Self::read_snapshot(sketch, store, &key)
                .await
                .and_then(|snapshot| sketch.load(snapshot, now_s()));
            match loaded {
                Ok(entries) => {
                    info!(
                        entries,
                        snapshot_key = key,
                        "Restored newest compatible resource sketch after replica replacement"
                    );
                    return;
                }
                Err(error) => warn!(
                    ?error,
                    "Restart snapshot was unreadable; trying the next newest"
                ),
            }
        }
    }

    /// Every `interval`: ages the sketch (the two half-windows
    /// are enforced here, store or not), writes it to `store` under `key`
    /// when there is one, and reports its size. Without a store, or with
    /// an interval of zero, the tick is a minute and the write is skipped.
    pub fn spawn_snapshot_writer(
        sketch: Arc<ResourceSketch>,
        store: Option<Store>,
        interval: Duration,
        key: String,
    ) {
        let write = store.filter(|_| !interval.is_zero());
        let tick = if interval.is_zero() {
            Duration::from_mins(1)
        } else {
            interval
        };
        background_spawn!("historical_resource_sketch_snapshot", async move {
            let mut last_stats = Stats::default();
            let mut last_written_s: Option<u64> = None;
            loop {
                tokio::time::sleep(tick).await;
                let now = now_s();
                let dropped = sketch.age_out(now);
                if dropped > 0 {
                    debug!(
                        dropped,
                        "Resource sketch keys not seen for the window were dropped"
                    );
                }
                if let Some(store) = &write {
                    let bytes = sketch.encode_snapshot(now);
                    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                    match store
                        .update_oneshot(StoreKey::Str(key.as_str().into()), Bytes::from(bytes))
                        .await
                    {
                        Ok(()) => {
                            last_written_s = Some(now);
                            record_hints_snapshot(len, 0);
                        }
                        Err(err) => {
                            warn!(
                                ?err,
                                key,
                                "Could not write the resource sketch snapshot; retrying next interval"
                            );
                            record_hints_snapshot_failure();
                            record_hints_snapshot(
                                len,
                                last_written_s.map_or(0, |at| now.saturating_sub(at)),
                            );
                        }
                    }
                }
                let stats = sketch.stats();
                for family in Family::ALL {
                    let i = match family {
                        Family::Target => 0,
                        Family::Mnemonic => 1,
                        Family::Command => 2,
                    };
                    record_hints_family(
                        family.as_str(),
                        u64::try_from(stats.entries[i]).unwrap_or(u64::MAX),
                        u64::try_from(stats.capacity[i]).unwrap_or(u64::MAX),
                        stats.evictions[i].saturating_sub(last_stats.evictions[i]),
                        stats.samples[i].saturating_sub(last_stats.samples[i]),
                    );
                }
                last_stats = stats;
            }
        });
    }

    /// Seeds the sketch from a hints file: one entry per hint, at the
    /// numbers the hint states (its class resolved through the ladder),
    /// marked seeded so the first real sample replaces it.
    fn seed_from_file(&self, path: &str) {
        let contents = match fs::read_to_string(path) {
            Ok(contents) => contents,
            Err(err) => {
                warn!(?err, hints_file = %path, "Could not read the hints file; nothing seeded");
                return;
            }
        };
        let hints = match serde_json::from_str::<HistoricalResourceHintFile>(&contents) {
            Ok(file) => file.into_hints(),
            Err(err) => {
                warn!(?err, hints_file = %path, "Could not parse the hints file; nothing seeded");
                return;
            }
        };
        let now = now_s();
        let (mut seeded, mut skipped) = (0usize, 0usize);
        for hint in hints {
            let numbers = self.numbers_from_hint(&hint);
            let Some(key) = hint.sketch_key().filter(|_| numbers != Numbers::default()) else {
                skipped += 1;
                continue;
            };
            let samples = hint
                .samples
                .and_then(|n| u32::try_from(n).ok())
                .unwrap_or(1);
            let entry = Entry::seeded(
                numbers.cpu_count,
                numbers.memory_kb,
                numbers.disk_kb,
                samples,
                hint.last_seen_s.unwrap_or(now),
            );
            if self.sketch.seed(key, entry) {
                seeded += 1;
            }
        }
        info!(hints_file = %path, seeded, skipped, "Seeded the resource sketch from the hints file");
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
    /// `target` alone, then the command digest, then the mnemonic alone.
    /// The command digest is what reaches clients that send no
    /// `RequestMetadata`, and the same command regardless of who runs it.
    fn hint_for_action(
        &self,
        action_info: &ActionInfo,
        target_id: Option<&String>,
        action_mnemonic: Option<&String>,
    ) -> Option<(Entry, &'static str)> {
        let ladder = [
            (
                target_id
                    .zip(action_mnemonic)
                    .map(|(target, mnemonic)| SketchKey::Target {
                        target: Arc::from(target.as_str()),
                        mnemonic: Some(Arc::from(mnemonic.as_str())),
                    }),
                "target",
            ),
            (
                target_id.map(|target| SketchKey::Target {
                    target: Arc::from(target.as_str()),
                    mnemonic: None,
                }),
                "target",
            ),
            (
                Some(SketchKey::Command {
                    digest: Arc::from(action_info.command_digest.to_string().as_str()),
                }),
                "command_digest",
            ),
            (
                action_mnemonic.map(|mnemonic| SketchKey::Mnemonic {
                    mnemonic: Arc::from(mnemonic.as_str()),
                }),
                "mnemonic",
            ),
        ];
        let now = now_s();
        for (key, source) in ladder {
            if let Some(key) = key
                && let Some(entry) = self.sketch.lookup(&key, now)
            {
                let source = if entry.seeded { "seed" } else { source };
                return Some((entry, source));
            }
        }
        None
    }

    fn class(&self, name: &str) -> Option<&SizeClassSpec> {
        self.classes.iter().find(|class| class.name == name)
    }

    /// A seed hint's numbers: what it states, with its class filling
    /// anything it leaves out. An unknown class contributes nothing.
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

    /// The cold-start policy for an action the sketch has nothing for: a
    /// class by mnemonic, else by timeout, else the default class; without
    /// classes, the raw `cold_start` numbers. Only absent properties are
    /// filled.
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
    /// ladder, or the action they were written for gets nothing. The budget
    /// must hold at least a few keys.
    pub fn validate(spec: &HistoricalResourceSpec) -> Result<(), Error> {
        if spec.replica_id.len() > MAX_REPLICA_ID_BYTES {
            return Err(make_input_err!(
                "historical_resource.replica_id must be at most {MAX_REPLICA_ID_BYTES} bytes"
            ));
        }
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
        if spec.memory_budget_bytes < 1 << 20 {
            return Err(make_input_err!(
                "historical_resource.memory_budget_bytes must be at least 1 MiB, got {}",
                spec.memory_budget_bytes
            ));
        }
        if spec.window_days == 0 {
            return Err(make_input_err!(
                "historical_resource.window_days must be above zero"
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
        // An entry that shapes to all zero (nothing measured on a dimension
        // the fleet reserves) reserves nothing, so it is no hint: the
        // cold-start policy applies and the metric says so.
        let hint = if let Some(file) = &self.file_hints {
            file.lookup(&action_info, target_id.as_ref(), action_mnemonic.as_ref())
                .map(|numbers| (numbers, "file"))
        } else {
            self.hint_for_action(&action_info, target_id.as_ref(), action_mnemonic.as_ref())
                .map(|(entry, source)| (self.sketch.shape(&entry), source))
                .filter(|(shaped, _)| !shaped.is_empty())
                .map(|(shaped, source)| (Numbers::from(&shaped), source))
        }
        .filter(|(numbers, _)| *numbers != Numbers::default());
        if let Some((shaped, source)) = hint {
            self.apply_minimums(Arc::make_mut(&mut action_info), shaped);
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
