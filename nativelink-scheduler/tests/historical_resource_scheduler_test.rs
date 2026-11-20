// Copyright 2026 Trace Machina, Inc. All rights reserved.
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
use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

mod utils {
    pub(crate) mod scheduler_utils;
}

use futures::join;
use nativelink_config::schedulers::{
    ColdStartSpec, HistoricalResourceSpec, SchedulerSpec, SimpleSpec, SizeClassSpec,
    TimeoutClassSpec,
};
use nativelink_config::stores::{MemorySpec, SizePartitioningSpec, StoreSpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::RequestMetadata;
use nativelink_scheduler::default_scheduler_factory::scheduler_factory;
use nativelink_scheduler::historical_resource_scheduler::{
    HistoricalResourceScheduler, replica_id, snapshot_key,
};
use nativelink_scheduler::mock_scheduler::MockActionScheduler;
use nativelink_scheduler::resource_sketch::{Observation, SketchKey};
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::size_partitioning_store::SizePartitioningStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_util::action_messages::{ActionStage, ActionState, OperationId};
use nativelink_util::common::DigestInfo;
use nativelink_util::operation_state_manager::ClientStateManager;
use nativelink_util::origin_event::{BAZEL_METADATA_KEY, request_metadata_to_baggage};
use nativelink_util::store_trait::{Store, StoreKey, StoreLike};
use opentelemetry::KeyValue;
use opentelemetry::baggage::BaggageExt;
use opentelemetry::context::{Context, FutureExt as OtelFutureExt};
use pretty_assertions::assert_eq;
use tokio::sync::watch;
use utils::scheduler_utils::{TokioWatchActionStateResult, make_base_action_info};

fn write_hints_file(contents: &str) -> String {
    let path = std::env::temp_dir().join(format!(
        "nativelink-historical-resource-hints-{}.json",
        uuid::Uuid::new_v4()
    ));
    fs::write(&path, contents).unwrap();
    path.to_string_lossy().to_string()
}

fn make_scheduler(hints_file: String) -> (Arc<MockActionScheduler>, HistoricalResourceScheduler) {
    make_scheduler_with_cold_start(hints_file, None)
}

fn make_scheduler_with_cold_start(
    hints_file: String,
    cold_start: Option<ColdStartSpec>,
) -> (Arc<MockActionScheduler>, HistoricalResourceScheduler) {
    make_scheduler_with_spec(&base_spec(hints_file, cold_start))
}

fn base_spec(hints_file: String, cold_start: Option<ColdStartSpec>) -> HistoricalResourceSpec {
    HistoricalResourceSpec {
        hints_file,
        refresh_interval_s: 0,
        learn_from_workers: true,
        // No headroom, so a seeded number comes back as stated.
        headroom_percent: 0,
        window_days: 7,
        memory_budget_bytes: 16 << 20,
        snapshot_store: None,
        replica_id: String::new(),
        snapshot_interval_s: 0,
        cpu_property_name: "cpu_count".to_string(),
        memory_property_name: "memory_kb".to_string(),
        disk_property_name: "disk_kb".to_string(),
        gpu_property_name: "gpu_count".to_string(),
        gpu_memory_property_name: "gpu_memory_kb".to_string(),
        classes: vec![],
        default_class: None,
        class_by_mnemonic: HashMap::new(),
        class_by_timeout: vec![],
        cold_start,
        scheduler: Box::new(SchedulerSpec::Simple(SimpleSpec::default())),
    }
}

fn make_scheduler_with_spec(
    spec: &HistoricalResourceSpec,
) -> (Arc<MockActionScheduler>, HistoricalResourceScheduler) {
    let mock_scheduler = Arc::new(MockActionScheduler::new());
    let sketch = Arc::new(HistoricalResourceScheduler::sketch_from_spec(spec));
    let scheduler = HistoricalResourceScheduler::new(spec, mock_scheduler.clone(), sketch);
    (mock_scheduler, scheduler)
}

fn now_s() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn class(name: &str, cpu_count: u64, memory_kb: u64, disk_kb: u64) -> SizeClassSpec {
    SizeClassSpec {
        name: name.to_string(),
        cpu_count,
        memory_kb,
        disk_kb,
        gpu_count: 0,
        gpu_memory_kb: 0,
    }
}

/// Runs `add_action` under the given Bazel metadata (none when `None`) and
/// returns the platform properties the nested scheduler received.
async fn properties_after_add(
    scheduler: &HistoricalResourceScheduler,
    mock_scheduler: &MockActionScheduler,
    action_info: Arc<nativelink_util::action_messages::ActionInfo>,
    metadata: Option<(&str, &str)>,
) -> Result<HashMap<String, String>, Error> {
    let client_operation_id = OperationId::default();
    let (_tx, rx) = watch::channel(Arc::new(ActionState {
        client_operation_id: OperationId::default(),
        stage: ActionStage::Queued,
        action_digest: action_info.unique_qualifier.digest(),
        last_transition_timestamp: SystemTime::now(),
    }));
    let context = match metadata {
        Some((target_id, action_mnemonic)) => Context::current_with_baggage(vec![KeyValue::new(
            BAZEL_METADATA_KEY,
            request_metadata_to_baggage(&RequestMetadata {
                target_id: target_id.to_string(),
                action_mnemonic: action_mnemonic.to_string(),
                ..Default::default()
            }),
        )]),
        None => Context::new(),
    };
    let (_, (_, passed_action_info)) = join!(
        scheduler
            .add_action(client_operation_id.clone(), action_info.clone())
            .with_context(context),
        mock_scheduler.expect_add_action(Ok(Box::new(TokioWatchActionStateResult::new(
            client_operation_id,
            action_info,
            rx,
        )))),
    );
    Ok(passed_action_info.platform_properties)
}

/// A hint keyed by the command digest reaches an action that carries no
/// Bazel metadata at all, in either digest spelling.
#[nativelink_test]
async fn command_digest_hint_matches_an_action_without_metadata() -> Result<(), Error> {
    let digest = DigestInfo::new([7; 32], 10);
    let hints_file = write_hints_file(&format!(
        r#"[ {{ "command_digest": "{}", "cpu_count": 3000, "memory_kb": 8000000, "disk_kb": 500000 }} ]"#,
        digest.to_string().replace('-', "/")
    ));
    let (mock_scheduler, scheduler) = make_scheduler(hints_file.clone());
    let mut action_info = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest())
        .as_ref()
        .clone();
    action_info.command_digest = digest;
    let properties =
        properties_after_add(&scheduler, &mock_scheduler, Arc::new(action_info), None).await?;
    assert_eq!(
        properties,
        HashMap::from([
            ("cpu_count".to_string(), "3000".to_string()),
            ("memory_kb".to_string(), "8000000".to_string()),
            ("disk_kb".to_string(), "500000".to_string()),
        ])
    );
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// The sketch keys a nameless action by its command digest, not its action
/// digest: a seed naming only an action digest seeds nothing.
#[nativelink_test]
async fn an_action_digest_only_hint_seeds_nothing() -> Result<(), Error> {
    let digest = DigestInfo::new([9; 32], 42);
    let hints_file = write_hints_file(&format!(
        r#"[ {{ "action_digest": "{digest}", "memory_kb": 7000000 }} ]"#
    ));
    let (mock_scheduler, scheduler) = make_scheduler(hints_file.clone());
    let matching = make_base_action_info(UNIX_EPOCH, digest);
    let properties = properties_after_add(&scheduler, &mock_scheduler, matching, None).await?;
    assert!(properties.is_empty(), "{properties:?}");
    assert_eq!(scheduler.sketch().stats().entries, [0, 0, 0]);
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// What the workers report is what sizes the next run: an attempt observed
/// twice is reserved its measured p95 with the headroom, and the seed it
/// replaced is gone.
#[nativelink_test]
async fn a_measured_action_is_sized_from_its_own_samples() -> Result<(), Error> {
    let hints_file = write_hints_file(
        r#"[ { "target_id": "//pkg:t", "action_mnemonic": "Genrule", "memory_kb": 500000 } ]"#,
    );
    let mut spec = base_spec(hints_file.clone(), None);
    spec.headroom_percent = 20;
    let (mock_scheduler, scheduler) = make_scheduler_with_spec(&spec);
    let base = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest());

    // The seed, with the headroom.
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        base.clone(),
        Some(("//pkg:t", "Genrule")),
    )
    .await?;
    assert_eq!(properties["memory_kb"], "600000");

    // Two attempts land through the worker scheduler's path.
    let key = SketchKey::for_attempt(Some("//pkg:t"), Some("Genrule"), "unused");
    for peak in [3_000_000u64, 3_500_000] {
        scheduler.sketch().observe(
            key.clone(),
            &Observation {
                peak_memory_kb: peak,
                cpu_millicores: Some(1200),
                now_s: now_s(),
                ..Observation::default()
            },
        );
    }
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        base,
        Some(("//pkg:t", "Genrule")),
    )
    .await?;
    // The p95 bucket of 3.5M KiB is [2M, 4M), capped by the exact maximum,
    // plus 20 percent.
    assert_eq!(properties["memory_kb"], "4200000");
    assert_eq!(properties["cpu_count"], "1440");
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// Bazel names both the target and the mnemonic, so a target the sketch has
/// never seen is sized from what its mnemonic's other targets used, not
/// from the cold start.
#[nativelink_test]
async fn a_new_target_is_sized_from_its_mnemonic() -> Result<(), Error> {
    let hints_file = write_hints_file("[]");
    let mut spec = base_spec(hints_file.clone(), None);
    spec.headroom_percent = 0;
    let (mock_scheduler, scheduler) = make_scheduler_with_spec(&spec);
    let base = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest());
    for (target, peak) in [("//lib:a", 2_000_000u64), ("//lib:b", 3_000_000)] {
        scheduler.sketch().observe(
            SketchKey::for_attempt(Some(target), Some("Javac"), "unused"),
            &Observation {
                peak_memory_kb: peak,
                now_s: now_s(),
                ..Observation::default()
            },
        );
    }
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        base,
        Some(("//lib:never-seen", "Javac")),
    )
    .await?;
    // The p95 bucket of the two Javac samples is [2M, 4M), capped by the
    // exact maximum.
    assert_eq!(properties["memory_kb"], "3000000");
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// Rolling restarts must not publish another replica's measurements again.
#[nativelink_test]
async fn repeated_replica_restarts_preserve_disjoint_history() -> Result<(), Error> {
    let store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let spec = base_spec(String::new(), None);
    let key = SketchKey::for_attempt(Some("//pkg:t"), Some("Genrule"), "unused");
    let now = now_s();
    for (replica, peak) in [("replica-a", 1000), ("replica-b", 8_000_000)] {
        let sketch = HistoricalResourceScheduler::sketch_from_spec(&spec);
        sketch.observe(
            key.clone(),
            &Observation {
                peak_memory_kb: peak,
                now_s: now,
                ..Observation::default()
            },
        );
        store
            .update_oneshot(
                StoreKey::Str(snapshot_key(replica).into()),
                sketch.encode_snapshot(now).into(),
            )
            .await?;
    }
    for _ in 0..4 {
        for (replica, peak) in [("replica-a", 1000), ("replica-b", 8_000_000)] {
            let sketch = HistoricalResourceScheduler::sketch_from_spec(&spec);
            HistoricalResourceScheduler::load_snapshots(&sketch, &store, &snapshot_key(replica))
                .await;
            let entry = sketch.lookup(&key, now).unwrap();
            assert_eq!(entry.memory_samples(), 1);
            assert_eq!(entry.max_memory_kb(), peak);
            assert_eq!(sketch.shape(&entry).memory_kb, peak);
            store
                .update_oneshot(
                    StoreKey::Str(snapshot_key(replica).into()),
                    sketch.encode_snapshot(now).into(),
                )
                .await?;
        }
    }
    let replacement = HistoricalResourceScheduler::sketch_from_spec(&spec);
    HistoricalResourceScheduler::load_snapshots(&replacement, &store, &snapshot_key("new-replica"))
        .await;
    let restored = replacement.lookup(&key, now).unwrap();
    assert_eq!(
        restored.memory_samples(),
        1,
        "only one snapshot is restored"
    );
    assert_eq!(restored.max_memory_kb(), 8_000_000);

    Ok(())
}

/// A store that routes by digest cannot hold a string key; the probe says
/// so at start.
#[nativelink_test]
async fn a_store_that_refuses_string_keys_fails_the_probe() -> Result<(), Error> {
    let store = Store::new(SizePartitioningStore::new(
        &SizePartitioningSpec {
            size: 1,
            lower_store: StoreSpec::Memory(MemorySpec::default()),
            upper_store: StoreSpec::Memory(MemorySpec::default()),
        },
        Store::new(MemoryStore::new(&MemorySpec::default())),
        Store::new(MemoryStore::new(&MemorySpec::default())),
    ));
    let err = HistoricalResourceScheduler::probe_snapshot_store(&store, "split")
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("must take a string key"),
        "{err:?}"
    );
    let stores = StoreManager::new();
    stores.add_store("split", store)?;
    let mut spec = base_spec(String::new(), None);
    spec.snapshot_store = Some("split".to_string());
    let (action, worker, sketch) =
        scheduler_factory(&SchedulerSpec::HistoricalResource(spec), &stores, None).await?;
    assert!(action.is_some());
    assert!(worker.is_some());
    assert!(
        sketch.is_some(),
        "a failed probe must leave in-memory learning available"
    );
    Ok(())
}

/// A hint may name a class instead of numbers; an untagged action takes the
/// class the cold-start policy picks: by mnemonic, else by timeout, else the
/// default. A value the client sent is never lowered.
#[nativelink_test]
async fn classes_resolve_hints_and_cold_start() -> Result<(), Error> {
    let hints_file = write_hints_file(
        r#"[ { "action_mnemonic": "TestRunner", "class": "m", "cpu_count": 5000 } ]"#,
    );
    let mut spec = base_spec(hints_file.clone(), None);
    spec.classes = vec![
        class("s", 1000, 4_000_000, 1_000_000),
        class("m", 2000, 12_000_000, 4_000_000),
        class("l", 4000, 36_000_000, 16_000_000),
    ];
    spec.default_class = Some("s".to_string());
    spec.class_by_mnemonic = HashMap::from([("Link".to_string(), "l".to_string())]);
    spec.class_by_timeout = vec![TimeoutClassSpec {
        min_timeout_s: 900,
        class: "m".to_string(),
    }];
    let (mock_scheduler, scheduler) = make_scheduler_with_spec(&spec);
    let base = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest());

    // A hint with a class: the class fills what the hint leaves out, and
    // the seed is then shaped like a measurement. 5000 millicores is above
    // every class on this ladder, so the top class stands for it.
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        base.clone(),
        Some(("//pkg:t", "TestRunner")),
    )
    .await?;
    assert_eq!(
        properties["cpu_count"], "4000",
        "past the top of the ladder, the top class"
    );
    assert_eq!(properties["memory_kb"], "36000000");
    assert_eq!(properties["disk_kb"], "16000000");

    // No hint, a known heavy mnemonic: its class.
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        base.clone(),
        Some(("//pkg:x", "Link")),
    )
    .await?;
    assert_eq!(properties["memory_kb"], "36000000");

    // No hint, an unknown mnemonic, a long timeout: the timeout rule.
    let mut long = (*base).clone();
    long.timeout = Duration::from_hours(1);
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        Arc::new(long),
        Some(("//pkg:y", "Other")),
    )
    .await?;
    assert_eq!(properties["memory_kb"], "12000000");

    // Nothing at all: the default class, but a client value stands.
    let mut tagged = (*base).clone();
    // The helper's timeout is MAX, which every timeout rule reaches; give it
    // one none does, so the default class is what gets tested.
    tagged.timeout = Duration::from_secs(10);
    tagged
        .platform_properties
        .insert("memory_kb".to_string(), "9".to_string());
    let properties =
        properties_after_add(&scheduler, &mock_scheduler, Arc::new(tagged), None).await?;
    assert_eq!(properties["memory_kb"], "9");
    assert_eq!(properties["cpu_count"], "1000");
    assert_eq!(properties["disk_kb"], "1000000");
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// A hint's number on a dimension the ladder does not reserve is dropped:
/// no worker on a fleet sized by CPU and memory advertises disk, so a disk
/// minimum would make the action unsatisfiable. Without a ladder or a cold
/// start the hint reserves what it states, as before.
#[nativelink_test]
async fn a_hint_dimension_the_ladder_does_not_reserve_is_dropped() -> Result<(), Error> {
    let hints_file = write_hints_file(
        r#"{ "hints": [
          { "action_mnemonic": "Genrule", "class": "m", "cpu_count": 1500, "disk_kb": 614 },
          { "action_mnemonic": "Link", "disk_kb": 9 }
        ] }"#,
    );
    let mut spec = base_spec(hints_file.clone(), None);
    spec.classes = vec![
        class("s", 1000, 2_097_152, 0),
        class("m", 2000, 6_291_456, 0),
    ];
    spec.default_class = Some("m".to_string());
    let (mock_scheduler, scheduler) = make_scheduler_with_spec(&spec);
    let base = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest());

    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        base.clone(),
        Some(("//pkg:a", "Genrule")),
    )
    .await?;
    assert_eq!(
        properties["cpu_count"], "2000",
        "the class that holds the hint's cpu"
    );
    assert_eq!(properties["memory_kb"], "6291456", "the class fills memory");
    assert!(
        !properties.contains_key("disk_kb"),
        "disk is not a dimension this ladder reserves: {properties:?}"
    );

    // A hint left with nothing but disk falls through to the cold start.
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        base.clone(),
        Some(("//pkg:b", "Link")),
    )
    .await?;
    assert_eq!(properties["memory_kb"], "6291456", "the default class");
    assert!(!properties.contains_key("disk_kb"));

    // No ladder and no cold start: the hint's dimensions all stand.
    let (mock_scheduler, scheduler) = make_scheduler(hints_file.clone());
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        base,
        Some(("//pkg:a", "Genrule")),
    )
    .await?;
    assert_eq!(properties["disk_kb"], "614");
    assert_eq!(properties["cpu_count"], "1500");
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// An untagged action nothing hints at gets the cold-start reservation;
/// a value the client sent is left as it is.
#[nativelink_test]
async fn cold_start_fills_only_absent_properties() -> Result<(), Error> {
    let hints_file = write_hints_file("[]");
    let (mock_scheduler, scheduler) = make_scheduler_with_cold_start(
        hints_file.clone(),
        Some(ColdStartSpec {
            cpu_count: 1000,
            memory_kb: 4_000_000,
            disk_kb: 0,
        }),
    );
    let client_operation_id = OperationId::default();
    let mut action_info = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest());
    Arc::make_mut(&mut action_info)
        .platform_properties
        .insert("memory_kb".to_string(), "1000".to_string());
    let (forward_watch_channel_tx, forward_watch_channel_rx) =
        watch::channel(Arc::new(ActionState {
            client_operation_id: client_operation_id.clone(),
            stage: ActionStage::Queued,
            action_digest: action_info.unique_qualifier.digest(),
            last_transition_timestamp: SystemTime::UNIX_EPOCH,
        }));
    drop(forward_watch_channel_tx);

    let (_, (_, passed_action_info)) = join!(
        scheduler.add_action(client_operation_id.clone(), action_info.clone()),
        mock_scheduler.expect_add_action(Ok(Box::new(TokioWatchActionStateResult::new(
            client_operation_id.clone(),
            action_info,
            forward_watch_channel_rx,
        )))),
    );

    assert_eq!(
        HashMap::from([
            ("cpu_count".to_string(), "1000".to_string()),
            ("memory_kb".to_string(), "1000".to_string()),
        ]),
        passed_action_info.platform_properties
    );
    drop(fs::remove_file(hints_file));
    Ok(())
}

#[nativelink_test]
async fn add_action_applies_historical_resource_hint() -> Result<(), Error> {
    let hints_file = write_hints_file(
        r#"{
          "hints": [
            {
              "target_id": "//pkg:heavy_test",
              "action_mnemonic": "TestRunner",
              "cpu_count": 2,
              "memory_kb": 12000000
            }
          ]
        }"#,
    );
    let (mock_scheduler, scheduler) = make_scheduler(hints_file.clone());
    let mut action_info = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest())
        .as_ref()
        .clone();
    action_info
        .platform_properties
        .insert("cpu_count".to_string(), "8".to_string());
    action_info
        .platform_properties
        .insert("memory_kb".to_string(), "1000".to_string());
    let action_info = Arc::new(action_info);
    let client_operation_id = OperationId::default();
    let (_forward_watch_channel_tx, forward_watch_channel_rx) =
        watch::channel(Arc::new(ActionState {
            client_operation_id: OperationId::default(),
            stage: ActionStage::Queued,
            action_digest: action_info.unique_qualifier.digest(),
            last_transition_timestamp: SystemTime::now(),
        }));
    let request_metadata = RequestMetadata {
        target_id: "//pkg:heavy_test".to_string(),
        action_mnemonic: "TestRunner".to_string(),
        ..Default::default()
    };
    let context = Context::current_with_baggage(vec![KeyValue::new(
        BAZEL_METADATA_KEY,
        request_metadata_to_baggage(&request_metadata),
    )]);

    let (_, (passed_client_operation_id, passed_action_info)) = join!(
        scheduler
            .add_action(client_operation_id.clone(), action_info.clone())
            .with_context(context),
        mock_scheduler.expect_add_action(Ok(Box::new(TokioWatchActionStateResult::new(
            client_operation_id.clone(),
            action_info,
            forward_watch_channel_rx,
        )))),
    );

    assert_eq!(client_operation_id, passed_client_operation_id);
    assert_eq!(
        HashMap::from([
            ("cpu_count".to_string(), "8".to_string()),
            ("memory_kb".to_string(), "12000000".to_string()),
        ]),
        passed_action_info.platform_properties
    );
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// A seed has one key: the target when it names one. The digest it may
/// also carry is not a second key, so a nameless action does not find it.
#[nativelink_test]
async fn a_hint_naming_a_target_is_found_by_the_target_only() -> Result<(), Error> {
    let digest = DigestInfo::new([3; 32], 12);
    let hints_file = write_hints_file(&format!(
        r#"[ {{ "target_id": "//pkg:t", "command_digest": "{digest}", "memory_kb": 5000000 }} ]"#
    ));
    let (mock_scheduler, scheduler) = make_scheduler(hints_file.clone());
    let mut by_digest = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest())
        .as_ref()
        .clone();
    by_digest.command_digest = digest;
    let properties =
        properties_after_add(&scheduler, &mock_scheduler, Arc::new(by_digest), None).await?;
    assert!(properties.is_empty(), "not a digest key: {properties:?}");
    // The ladder's second rung, the target alone, reaches a seed that
    // named no mnemonic whatever mnemonic the action carries.
    let by_target = make_base_action_info(UNIX_EPOCH, DigestInfo::new([4; 32], 12));
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        by_target,
        Some(("//pkg:t", "Other")),
    )
    .await?;
    assert_eq!(properties["memory_kb"], "5000000", "found by the target");
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// The ladder holds: an action with a mnemonic and no target takes the
/// command digest hint before the mnemonic one.
#[nativelink_test]
async fn a_mnemonic_alone_does_not_outrank_a_digest_hint() -> Result<(), Error> {
    let digest = DigestInfo::new([5; 32], 12);
    let hints_file = write_hints_file(&format!(
        r#"[ {{ "action_mnemonic": "Link", "memory_kb": 1000000 }},
             {{ "command_digest": "{digest}", "memory_kb": 9000000 }} ]"#
    ));
    let (mock_scheduler, scheduler) = make_scheduler(hints_file.clone());
    let mut action = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest())
        .as_ref()
        .clone();
    action.command_digest = digest;
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        Arc::new(action),
        Some(("", "Link")),
    )
    .await?;
    assert_eq!(properties["memory_kb"], "9000000");
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// A hint that reserves nothing is no hint: the cold start applies.
#[nativelink_test]
async fn an_all_zero_hint_falls_through_to_the_cold_start() -> Result<(), Error> {
    let digest = DigestInfo::new([6; 32], 12);
    let hints_file = write_hints_file(&format!(
        r#"[ {{ "action_digest": "{digest}", "samples": 3, "last_seen_s": 1 }} ]"#
    ));
    let (mock_scheduler, scheduler) = make_scheduler_with_cold_start(
        hints_file.clone(),
        Some(ColdStartSpec {
            cpu_count: 1500,
            memory_kb: 0,
            disk_kb: 250_000,
        }),
    );
    let action = make_base_action_info(UNIX_EPOCH, digest);
    let properties = properties_after_add(&scheduler, &mock_scheduler, action, None).await?;
    assert_eq!(properties["cpu_count"], "1500");
    assert_eq!(
        properties["disk_kb"], "250000",
        "the cold start reserves disk too"
    );
    assert!(!properties.contains_key("memory_kb"));
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// A rule naming a class the ladder lacks is refused at load; at run time
/// the next candidate that exists is taken, never the raw numbers.
#[nativelink_test]
async fn a_rule_naming_no_class_is_refused_and_the_default_class_stands_in() -> Result<(), Error> {
    let hints_file = write_hints_file("[]");
    let mut spec = base_spec(hints_file.clone(), None);
    spec.classes = vec![class("s", 1000, 4_000_000, 0)];
    spec.default_class = Some("s".to_string());
    spec.class_by_mnemonic = HashMap::from([("Link".to_string(), "xl".to_string())]);
    let err = HistoricalResourceScheduler::validate(&spec).expect_err("xl is not on the ladder");
    assert!(err.to_string().contains("class_by_mnemonic[Link]"), "{err}");
    let (mock_scheduler, scheduler) = make_scheduler_with_spec(&spec);
    let action = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest());
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        action,
        Some(("//pkg:x", "Link")),
    )
    .await?;
    assert_eq!(
        properties["memory_kb"], "4000000",
        "the default class, not nothing"
    );
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// Old configurations remain file-backed unless learning is explicitly enabled.
#[nativelink_test]
async fn file_hints_refresh_and_keep_last_good_generation() -> Result<(), Error> {
    let path = write_hints_file(r#"[{"target_id":"//file:t","memory_kb":1000}]"#);
    let mut spec = base_spec(path.clone(), None);
    spec.learn_from_workers = false;
    spec.refresh_interval_s = 1;
    spec.headroom_percent = 20;
    let (mock, scheduler) = make_scheduler_with_spec(&spec);
    let action = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest());
    let read = || properties_after_add(&scheduler, &mock, action.clone(), Some(("//file:t", "X")));
    assert_eq!(
        read().await?["memory_kb"],
        "1000",
        "static hints are not shaped again"
    );
    scheduler.sketch().observe(
        SketchKey::for_attempt(Some("//file:t"), Some("X"), "unused"),
        &Observation {
            peak_memory_kb: 1,
            now_s: now_s(),
            ..Observation::default()
        },
    );
    assert_eq!(
        read().await?["memory_kb"],
        "1000",
        "samples do not replace static hints"
    );
    fs::write(&path, r#"[{"target_id":"//file:t","memory_kb":8000}]"#)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if read().await?["memory_kb"] == "8000" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "file did not refresh"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    fs::write(&path, "invalid JSON")?;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    assert_eq!(
        read().await?["memory_kb"],
        "8000",
        "failed refresh retains the previous generation"
    );
    fs::remove_file(path)?;
    Ok(())
}

#[nativelink_test]
async fn static_file_preserves_action_digest_lookup_and_read_once() -> Result<(), Error> {
    let digest = DigestInfo::new([9; 32], 42);
    let path = write_hints_file(&format!(
        r#"[{{"action_digest":"{digest}","memory_kb":7000}}]"#
    ));
    let mut spec = base_spec(path.clone(), None);
    spec.learn_from_workers = false;
    let (mock, scheduler) = make_scheduler_with_spec(&spec);
    let action = make_base_action_info(UNIX_EPOCH, digest);
    fs::write(&path, "[]")?;
    let properties = properties_after_add(&scheduler, &mock, action, None).await?;
    assert_eq!(properties["memory_kb"], "7000");
    fs::remove_file(path)?;
    Ok(())
}

#[test]
fn existing_configuration_does_not_enable_learning() {
    let spec: HistoricalResourceSpec = serde_json::from_value(serde_json::json!({
        "hints_file": "hints.json", "refresh_interval_s": 30, "scheduler": {"simple": {}}
    }))
    .unwrap();
    assert!(!spec.learn_from_workers);
    assert_eq!(spec.refresh_interval_s, 30);
    let empty = HistoricalResourceScheduler::sketch_from_spec(&spec);
    assert!(
        empty.charged_bytes() < 1 << 20,
        "static mode must not preallocate the learned cache"
    );
}

#[nativelink_test]
async fn factory_only_exposes_learning_and_uses_snapshot_store_when_enabled() -> Result<(), Error> {
    let stores = StoreManager::new();
    for (learning, snapshot_store) in [
        (false, Some("missing-store")),
        (true, Some("missing-store")),
        (true, None),
    ] {
        let mut spec = base_spec(String::new(), None);
        spec.learn_from_workers = learning;
        spec.snapshot_store = snapshot_store.map(str::to_string);
        let result =
            scheduler_factory(&SchedulerSpec::HistoricalResource(spec), &stores, None).await;
        let (action, worker, sketch) = result?;
        assert!(
            action.is_some(),
            "optional persistence must not prevent startup"
        );
        assert!(worker.is_some());
        assert_eq!(sketch.is_some(), learning);
    }
    Ok(())
}

/// The default must leave room for learning after loading a realistic seed.
#[nativelink_test]
async fn default_budget_keeps_seed_and_new_target_history() -> Result<(), Error> {
    let label = |kind: &str, i: usize| {
        format!(
            "//{kind}/{}/component_{i}:integration_test",
            "package/".repeat(20)
        )
    };
    let hints = (0..10_208)
        .map(|i| {
            serde_json::json!({
                "target_id": label("seed", i), "action_mnemonic": "TestRunner", "memory_kb": 4096,
            })
        })
        .collect::<Vec<_>>();
    let path = write_hints_file(&serde_json::to_string(&hints).unwrap());
    let spec: HistoricalResourceSpec = serde_json::from_value(serde_json::json!({
        "learn_from_workers": true, "hints_file": path, "scheduler": {"simple": {}},
    }))
    .unwrap();
    let (_mock, scheduler) = make_scheduler_with_spec(&spec);
    let sketch = scheduler.sketch();
    let now = now_s();
    for i in 0..20_000 {
        sketch.observe(
            SketchKey::for_attempt(Some(&label("learned", i)), Some("TestRunner"), "unused"),
            &Observation {
                peak_memory_kb: 8192,
                now_s: now,
                ..Observation::default()
            },
        );
    }
    assert_eq!(sketch.stats().entries[0], 30_208);
    assert_eq!(sketch.stats().evictions, [0, 0, 0]);
    assert!(sketch.stats().capacity[0] >= 50_000);
    for i in 0..10_208 {
        let key = SketchKey::for_attempt(Some(&label("seed", i)), Some("TestRunner"), "unused");
        assert!(sketch.lookup(&key, now).unwrap().seeded);
    }
    fs::remove_file(path)?;
    Ok(())
}

#[nativelink_test]
async fn configured_replica_identity_restores_without_hostname_dependency() -> Result<(), Error> {
    let store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let mut spec = base_spec(String::new(), None);
    spec.snapshot_store = Some("snapshots".to_string());
    spec.replica_id = "stable-scheduler".to_string();
    assert_eq!(replica_id(&spec.replica_id), "stable-scheduler");
    let now = now_s();
    let key = SketchKey::for_attempt(Some("//restored:t"), Some("TestRunner"), "unused");
    let old = HistoricalResourceScheduler::sketch_from_spec(&spec);
    old.observe(
        key.clone(),
        &Observation {
            peak_memory_kb: 4096,
            now_s: now,
            ..Observation::default()
        },
    );
    store
        .update_oneshot(
            StoreKey::Str(snapshot_key(&spec.replica_id).into()),
            old.encode_snapshot(now).into(),
        )
        .await?;
    let stores = StoreManager::new();
    stores.add_store("snapshots", store)?;
    let (_, _, restored) =
        scheduler_factory(&SchedulerSpec::HistoricalResource(spec), &stores, None).await?;
    assert_eq!(
        restored
            .unwrap()
            .lookup(&key, now)
            .unwrap()
            .memory_samples(),
        1
    );
    let mut invalid = base_spec(String::new(), None);
    invalid.replica_id = "x".repeat(257);
    assert!(HistoricalResourceScheduler::validate(&invalid).is_err());
    Ok(())
}

#[nativelink_test]
async fn replacement_restores_newest_usable_snapshot_without_merging() -> Result<(), Error> {
    let store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let spec = base_spec(String::new(), None);
    let now = now_s();
    let key = SketchKey::for_attempt(Some("//restored:t"), Some("TestRunner"), "unused");
    for (name, age, peak) in [
        ("old", 30, 1024),
        ("recent", 20, 8192),
        ("truncated", 10, 16384),
        ("wrong-sampler", 0, 32768),
    ] {
        let sketch = HistoricalResourceScheduler::sketch_from_spec(&spec);
        sketch.observe(
            key.clone(),
            &Observation {
                peak_memory_kb: peak,
                now_s: now,
                ..Observation::default()
            },
        );
        let mut bytes = sketch.encode_snapshot(now - age);
        if name == "truncated" {
            bytes.pop();
        }
        if name == "wrong-sampler" {
            bytes[5..9].copy_from_slice(&u32::MAX.to_le_bytes());
        }
        store
            .update_oneshot(StoreKey::Str(snapshot_key(name).into()), bytes.into())
            .await?;
    }
    for i in 0..4 {
        let sketch = HistoricalResourceScheduler::sketch_from_spec(&spec);
        let own_key = snapshot_key(&format!("replacement-{i}"));
        HistoricalResourceScheduler::load_snapshots(&sketch, &store, &own_key).await;
        let entry = sketch.lookup(&key, now).unwrap();
        assert_eq!(entry.max_memory_kb(), 8192);
        assert_eq!(
            entry.memory_samples(),
            1,
            "replacement must never add histories together"
        );
        store
            .update_oneshot(
                StoreKey::Str(own_key.into()),
                sketch.encode_snapshot(now).into(),
            )
            .await?;
    }
    Ok(())
}

#[nativelink_test]
async fn snapshot_discovery_is_bounded_but_owned_restore_still_works() -> Result<(), Error> {
    let store = Store::new(MemoryStore::new(&MemorySpec::default()));
    let spec = base_spec(String::new(), None);
    let now = now_s();
    let key = SketchKey::for_attempt(Some("//restored:t"), Some("TestRunner"), "unused");
    let original = HistoricalResourceScheduler::sketch_from_spec(&spec);
    original.observe(
        key.clone(),
        &Observation {
            peak_memory_kb: 4096,
            now_s: now,
            ..Observation::default()
        },
    );
    let bytes = bytes::Bytes::from(original.encode_snapshot(now));
    for i in 0..129 {
        store
            .update_oneshot(
                StoreKey::Str(snapshot_key(&format!("retired-{i}")).into()),
                bytes.clone(),
            )
            .await?;
    }
    let fresh = HistoricalResourceScheduler::sketch_from_spec(&spec);
    HistoricalResourceScheduler::load_snapshots(&fresh, &store, &snapshot_key("new-id")).await;
    assert_eq!(fresh.stats().entries, [0, 0, 0]);
    HistoricalResourceScheduler::load_snapshots(&fresh, &store, &snapshot_key("retired-0")).await;
    assert_eq!(fresh.lookup(&key, now).unwrap().memory_samples(), 1);
    Ok(())
}

/// A size class that also carries a GPU share, on a fleet that counts a
/// device in hundredths.
fn gpu_class(name: &str, cpu_count: u64, memory_kb: u64, gpu_count: u64) -> SizeClassSpec {
    SizeClassSpec {
        gpu_count,
        ..class(name, cpu_count, memory_kb, 0)
    }
}

/// While the hints file is what sizes actions, a hint states the GPU share
/// like any other number: its own `gpu_count`, or its class's. A share the
/// client asked for is raised to the hint and never lowered, and a hint
/// that states no share leaves the property alone.
#[nativelink_test]
async fn a_file_hint_states_the_gpu_share() -> Result<(), Error> {
    let path = write_hints_file(
        r#"[
            { "target_id": "//sim:render_test", "gpu_count": 50, "memory_kb": 8000000 },
            { "target_id": "//sim:physics_test", "class": "quarter" },
            { "target_id": "//sim:whole_test", "class": "quarter", "gpu_count": 100 },
            { "target_id": "//pkg:compile", "memory_kb": 2000000 }
        ]"#,
    );
    let mut spec = base_spec(path.clone(), None);
    spec.learn_from_workers = false;
    spec.classes = vec![gpu_class("quarter", 4000, 16_000_000, 25)];
    let (mock, scheduler) = make_scheduler_with_spec(&spec);
    let base = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest());
    let sized = |target: &'static str, action| {
        properties_after_add(&scheduler, &mock, action, Some((target, "TestRunner")))
    };

    let properties = sized("//sim:render_test", base.clone()).await?;
    assert_eq!(properties["gpu_count"], "50");
    assert_eq!(properties["memory_kb"], "8000000");

    let properties = sized("//sim:physics_test", base.clone()).await?;
    assert_eq!(properties["gpu_count"], "25", "the class's share");
    assert_eq!(properties["cpu_count"], "4000");

    let properties = sized("//sim:whole_test", base.clone()).await?;
    assert_eq!(properties["gpu_count"], "100", "the hint's own number wins");

    let properties = sized("//pkg:compile", base.clone()).await?;
    assert!(!properties.contains_key("gpu_count"), "{properties:?}");

    let mut asks_more = (*base).clone();
    asks_more
        .platform_properties
        .insert("gpu_count".to_string(), "100".to_string());
    let properties = sized("//sim:render_test", Arc::new(asks_more)).await?;
    assert_eq!(properties["gpu_count"], "100", "never lowered");

    let mut asks_less = (*base).clone();
    asks_less
        .platform_properties
        .insert("gpu_count".to_string(), "10".to_string());
    let properties = sized("//sim:render_test", Arc::new(asks_less)).await?;
    assert_eq!(properties["gpu_count"], "50", "raised to the hint");

    fs::remove_file(path)?;
    Ok(())
}

/// A GPU share is stated, never measured, so a learning scheduler gets it
/// from the class its rules name for the action, and goes on getting it
/// after the action has been measured. Without that the rule would apply to
/// an action's first run only: once the sketch held its cpu and memory, the
/// hint would size it and the share would be gone.
#[nativelink_test]
async fn a_class_rule_gives_a_learned_action_its_gpu_share() -> Result<(), Error> {
    let hints_file = write_hints_file("[]");
    let mut spec = base_spec(hints_file.clone(), None);
    spec.classes = vec![
        class("s", 1000, 4_000_000, 0),
        gpu_class("quarter", 4000, 16_000_000, 25),
    ];
    spec.class_by_mnemonic = HashMap::from([("CudaTest".to_string(), "quarter".to_string())]);
    let (mock_scheduler, scheduler) = make_scheduler_with_spec(&spec);
    let base = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest());
    let sized = |action| {
        properties_after_add(
            &scheduler,
            &mock_scheduler,
            action,
            Some(("//sim:physics_test", "CudaTest")),
        )
    };

    // Nothing learned yet: the rule's class, share included.
    let properties = sized(base.clone()).await?;
    assert_eq!(properties["gpu_count"], "25");
    assert_eq!(properties["memory_kb"], "16000000");

    // Measured small: the hint now sizes cpu and memory from the samples,
    // and the share still comes from the rule.
    let key = SketchKey::for_attempt(Some("//sim:physics_test"), Some("CudaTest"), "unused");
    for _ in 0..2 {
        scheduler.sketch().observe(
            key.clone(),
            &Observation {
                peak_memory_kb: 1_000_000,
                cpu_millicores: Some(500),
                now_s: now_s(),
                ..Observation::default()
            },
        );
    }
    let properties = sized(base.clone()).await?;
    assert_eq!(properties["memory_kb"], "4000000", "sized from the samples");
    assert_eq!(properties["gpu_count"], "25", "the rule still applies");

    // A share the client sent stands, in either direction: a rule fills
    // the property only when it is absent.
    let mut tagged = (*base).clone();
    tagged
        .platform_properties
        .insert("gpu_count".to_string(), "100".to_string());
    assert_eq!(sized(Arc::new(tagged)).await?["gpu_count"], "100");

    drop(fs::remove_file(hints_file));
    Ok(())
}

/// A class is chosen for a measured action by the cpu and memory it used.
/// That choice says nothing about a device: an action measured into a class
/// that carries a share, with no rule naming that class for it, gets the
/// class's cpu and memory and no GPU.
#[nativelink_test]
async fn a_measured_action_is_not_given_the_gpu_of_the_class_it_fits() -> Result<(), Error> {
    let hints_file = write_hints_file("[]");
    let mut spec = base_spec(hints_file.clone(), None);
    spec.classes = vec![
        class("s", 1000, 4_000_000, 0),
        gpu_class("quarter", 4000, 16_000_000, 25),
    ];
    let (mock_scheduler, scheduler) = make_scheduler_with_spec(&spec);
    let key = SketchKey::for_attempt(Some("//pkg:link"), Some("CppLink"), "unused");
    for _ in 0..2 {
        scheduler.sketch().observe(
            key.clone(),
            &Observation {
                peak_memory_kb: 10_000_000,
                cpu_millicores: Some(3000),
                now_s: now_s(),
                ..Observation::default()
            },
        );
    }
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest()),
        Some(("//pkg:link", "CppLink")),
    )
    .await?;
    assert_eq!(properties["memory_kb"], "16000000", "the class it fits");
    assert!(!properties.contains_key("gpu_count"), "{properties:?}");
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// The sketch holds what workers measure, so a learning scheduler seeds a
/// hint's cpu and memory and has nowhere to keep its `gpu_count`.
#[nativelink_test]
async fn a_learning_scheduler_does_not_seed_a_gpu_share() -> Result<(), Error> {
    let hints_file = write_hints_file(
        r#"[
            { "target_id": "//sim:render_test", "gpu_count": 50, "memory_kb": 8000000 },
            { "target_id": "//sim:gpu_only", "gpu_count": 50 }
        ]"#,
    );
    let (mock_scheduler, scheduler) = make_scheduler(hints_file.clone());
    let base = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest());

    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        base.clone(),
        Some(("//sim:render_test", "TestRunner")),
    )
    .await?;
    assert_eq!(properties["memory_kb"], "8000000");
    assert!(!properties.contains_key("gpu_count"), "{properties:?}");

    // A hint that states nothing a worker measures seeds no entry at all.
    assert_eq!(scheduler.sketch().stats().entries, [1, 0, 0]);
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// The property name follows the configuration, and an existing one that
/// never mentions a GPU gets the default without stating it.
#[nativelink_test]
async fn the_gpu_property_name_is_configurable() -> Result<(), Error> {
    let defaulted: HistoricalResourceSpec = serde_json::from_value(serde_json::json!({
        "hints_file": "hints.json", "scheduler": {"simple": {}}
    }))
    .unwrap();
    assert_eq!(defaulted.gpu_property_name, "gpu_count");

    let path = write_hints_file(r#"[ { "target_id": "//sim:render_test", "gpu_count": 50 } ]"#);
    let mut spec = base_spec(path.clone(), None);
    spec.learn_from_workers = false;
    spec.gpu_property_name = "gpu".to_string();
    let (mock, scheduler) = make_scheduler_with_spec(&spec);
    let properties = properties_after_add(
        &scheduler,
        &mock,
        make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest()),
        Some(("//sim:render_test", "TestRunner")),
    )
    .await?;
    assert_eq!(
        properties,
        HashMap::from([("gpu".to_string(), "50".to_string())])
    );
    fs::remove_file(path)?;
    Ok(())
}

/// A class standing for a slice of a GPU: its share of a device and the
/// device memory the slice has.
fn slice_class(name: &str, memory_kb: u64, gpu_count: u64, gpu_memory_kb: u64) -> SizeClassSpec {
    SizeClassSpec {
        gpu_memory_kb,
        ..gpu_class(name, 4000, memory_kb, gpu_count)
    }
}

fn observe_on_gpu(scheduler: &HistoricalResourceScheduler, target: &str, gpu_memory_kb: u64) {
    scheduler.sketch().observe(
        SketchKey::for_attempt(Some(target), Some("CudaTest"), "unused"),
        &Observation {
            peak_memory_kb: 2_000_000,
            peak_gpu_memory_kb: gpu_memory_kb,
            now_s: now_s(),
            ..Observation::default()
        },
    );
}

/// What a wrapper measured an action holding on its device sizes the next
/// run: the smallest slice that holds it, as that slice's device memory and
/// share of a device. The client states the least it could run on and the
/// hint raises it; a share or a memory the client set higher stands.
#[nativelink_test]
async fn a_measured_action_takes_the_slice_its_device_memory_fits() -> Result<(), Error> {
    let hints_file = write_hints_file("[]");
    let mut spec = base_spec(hints_file.clone(), None);
    spec.classes = vec![
        class("s", 1000, 4_000_000, 0),
        slice_class("slice-10", 16_000_000, 14, 10_485_760),
        slice_class("slice-20", 32_000_000, 28, 20_971_520),
    ];
    let (mock_scheduler, scheduler) = make_scheduler_with_spec(&spec);
    let base = make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest());
    let sized = |target: &'static str, action| {
        properties_after_add(
            &scheduler,
            &mock_scheduler,
            action,
            Some((target, "CudaTest")),
        )
    };

    observe_on_gpu(&scheduler, "//sim:physics_test", 7_000_000);
    let properties = sized("//sim:physics_test", base.clone()).await?;
    assert_eq!(properties["gpu_memory_kb"], "10485760");
    assert_eq!(properties["gpu_count"], "14");
    assert_eq!(properties["memory_kb"], "16000000");

    // A later run held more: the next slice up.
    observe_on_gpu(&scheduler, "//sim:physics_test", 15_000_000);
    let properties = sized("//sim:physics_test", base.clone()).await?;
    assert_eq!(properties["gpu_memory_kb"], "20971520");
    assert_eq!(properties["gpu_count"], "28");

    // The client asked for the smallest share and is raised to the slice's;
    // one that asked for a whole device keeps it.
    let mut least = (*base).clone();
    least
        .platform_properties
        .insert("gpu_count".to_string(), "14".to_string());
    assert_eq!(
        sized("//sim:physics_test", Arc::new(least)).await?["gpu_count"],
        "28"
    );
    let mut whole = (*base).clone();
    whole
        .platform_properties
        .insert("gpu_count".to_string(), "100".to_string());
    assert_eq!(
        sized("//sim:physics_test", Arc::new(whole)).await?["gpu_count"],
        "100"
    );

    // Past the largest slice the measured number stands, so only a worker
    // advertising that much device memory is asked, and no share is
    // invented for it.
    observe_on_gpu(&scheduler, "//sim:train_test", 30_000_000);
    let properties = sized("//sim:train_test", base.clone()).await?;
    assert_eq!(properties["gpu_memory_kb"], "30000000");
    assert!(!properties.contains_key("gpu_count"), "{properties:?}");

    // An action nothing measured on a device gets no device memory.
    scheduler.sketch().observe(
        SketchKey::for_attempt(Some("//pkg:link"), Some("CppLink"), "unused"),
        &Observation {
            peak_memory_kb: 3_000_000,
            now_s: now_s(),
            ..Observation::default()
        },
    );
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        base,
        Some(("//pkg:link", "CppLink")),
    )
    .await?;
    assert_eq!(properties["memory_kb"], "4000000");
    assert!(!properties.contains_key("gpu_memory_kb"), "{properties:?}");

    drop(fs::remove_file(hints_file));
    Ok(())
}

/// A fleet whose ladder names no device memory reserves none: no worker
/// there advertises it, and a minimum no worker advertises is an action
/// that never runs. The measurement is dropped like any other on a
/// dimension the fleet does not reserve.
#[nativelink_test]
async fn a_fleet_without_slices_does_not_reserve_measured_device_memory() -> Result<(), Error> {
    let hints_file = write_hints_file("[]");
    let mut spec = base_spec(hints_file.clone(), None);
    spec.classes = vec![
        class("s", 1000, 4_000_000, 0),
        class("m", 2000, 16_000_000, 0),
    ];
    let (mock_scheduler, scheduler) = make_scheduler_with_spec(&spec);
    observe_on_gpu(&scheduler, "//sim:physics_test", 7_000_000);
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest()),
        Some(("//sim:physics_test", "CudaTest")),
    )
    .await?;
    assert_eq!(properties["memory_kb"], "4000000");
    assert!(!properties.contains_key("gpu_memory_kb"), "{properties:?}");
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// A configuration with no ladder and no cold start reserves what was
/// measured of memory, as it always did, and no device memory: nothing in
/// it says a worker advertises any. Reserving it would leave the action's
/// next run, after the first report from a wrapper, waiting for a worker
/// that does not exist.
#[nativelink_test]
async fn an_unsized_fleet_does_not_reserve_measured_device_memory() -> Result<(), Error> {
    let hints_file = write_hints_file("[]");
    let (mock_scheduler, scheduler) = make_scheduler(hints_file.clone());
    observe_on_gpu(&scheduler, "//sim:physics_test", 7_000_000);
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest()),
        Some(("//sim:physics_test", "CudaTest")),
    )
    .await?;
    assert_eq!(
        properties,
        HashMap::from([("memory_kb".to_string(), "2000000".to_string())])
    );
    drop(fs::remove_file(hints_file));
    Ok(())
}

/// While the hints file sizes actions, an entry states device memory like
/// any other number, under the configured property name. Someone wrote the
/// number down, so it is applied as written whether or not a class names
/// device memory; only a measurement waits for one.
#[nativelink_test]
async fn a_file_hint_states_device_memory() -> Result<(), Error> {
    let path = write_hints_file(
        r#"[ { "target_id": "//sim:render_test", "gpu_count": 28, "gpu_memory_kb": 20971520 } ]"#,
    );
    let mut spec = base_spec(path.clone(), None);
    spec.learn_from_workers = false;
    spec.gpu_memory_property_name = "gpu_ram".to_string();
    let stated = HashMap::from([
        ("gpu_count".to_string(), "28".to_string()),
        ("gpu_ram".to_string(), "20971520".to_string()),
    ]);
    for classes in [vec![], vec![class("s", 1000, 4_000_000, 0)]] {
        spec.classes = classes;
        let (mock, scheduler) = make_scheduler_with_spec(&spec);
        let properties = properties_after_add(
            &scheduler,
            &mock,
            make_base_action_info(UNIX_EPOCH, DigestInfo::zero_digest()),
            Some(("//sim:render_test", "TestRunner")),
        )
        .await?;
        assert_eq!(properties, stated);
    }
    fs::remove_file(path)?;
    Ok(())
}
