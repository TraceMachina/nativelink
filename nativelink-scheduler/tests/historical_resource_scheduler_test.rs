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
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::RequestMetadata;
use nativelink_scheduler::historical_resource_scheduler::HistoricalResourceScheduler;
use nativelink_scheduler::mock_scheduler::MockActionScheduler;
use nativelink_util::action_messages::{ActionStage, ActionState, OperationId};
use nativelink_util::common::DigestInfo;
use nativelink_util::operation_state_manager::ClientStateManager;
use nativelink_util::origin_event::{BAZEL_METADATA_KEY, request_metadata_to_baggage};
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
        cpu_property_name: "cpu_count".to_string(),
        memory_property_name: "memory_kb".to_string(),
        disk_property_name: "disk_kb".to_string(),
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
    let scheduler = HistoricalResourceScheduler::new(spec, mock_scheduler.clone());
    (mock_scheduler, scheduler)
}

fn class(name: &str, cpu_count: u64, memory_kb: u64, disk_kb: u64) -> SizeClassSpec {
    SizeClassSpec {
        name: name.to_string(),
        cpu_count,
        memory_kb,
        disk_kb,
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

/// A hint keyed by the action digest, which is what origin events carry,
/// matches the action whose digest it is and no other.
#[nativelink_test]
async fn action_digest_hint_matches_only_its_action() -> Result<(), Error> {
    let digest = DigestInfo::new([9; 32], 42);
    let hints_file = write_hints_file(&format!(
        r#"[ {{ "action_digest": "{digest}", "memory_kb": 7000000 }} ]"#
    ));
    let (mock_scheduler, scheduler) = make_scheduler(hints_file.clone());
    let matching = make_base_action_info(UNIX_EPOCH, digest);
    let properties = properties_after_add(&scheduler, &mock_scheduler, matching, None).await?;
    assert_eq!(properties["memory_kb"], "7000000");
    let other = make_base_action_info(UNIX_EPOCH, DigestInfo::new([8; 32], 42));
    let properties = properties_after_add(&scheduler, &mock_scheduler, other, None).await?;
    assert!(properties.is_empty(), "{properties:?}");
    drop(fs::remove_file(hints_file));
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

    // A hint with a class: the class fills what the hint leaves out.
    let properties = properties_after_add(
        &scheduler,
        &mock_scheduler,
        base.clone(),
        Some(("//pkg:t", "TestRunner")),
    )
    .await?;
    assert_eq!(
        properties["cpu_count"], "5000",
        "the hint's own number wins"
    );
    assert_eq!(
        properties["memory_kb"], "12000000",
        "the class fills memory"
    );
    assert_eq!(properties["disk_kb"], "4000000");

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
    assert_eq!(properties["cpu_count"], "1500", "the hint's own cpu stands");
    assert_eq!(properties["memory_kb"], "6291456", "the class fills memory");
    assert!(
        !properties.contains_key("disk_kb"),
        "disk is not a dimension this ladder reserves: {properties:?}"
    );
    assert!(logs_contain(
        "Hints carry a dimension neither the ladder nor the cold start reserves; it is ignored"
    ));

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

/// A record naming a target and a digest is reachable by either key; a
/// single composite key was reachable by neither.
#[nativelink_test]
async fn a_hint_naming_a_target_and_a_digest_is_found_by_either() -> Result<(), Error> {
    let digest = DigestInfo::new([3; 32], 12);
    let hints_file = write_hints_file(&format!(
        r#"[ {{ "target_id": "//pkg:t", "action_digest": "{digest}", "memory_kb": 5000000 }} ]"#
    ));
    let (mock_scheduler, scheduler) = make_scheduler(hints_file.clone());
    let by_digest = make_base_action_info(UNIX_EPOCH, digest);
    let properties = properties_after_add(&scheduler, &mock_scheduler, by_digest, None).await?;
    assert_eq!(properties["memory_kb"], "5000000", "found by the digest");
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
/// digest hint before the mnemonic one.
#[nativelink_test]
async fn a_mnemonic_alone_does_not_outrank_a_digest_hint() -> Result<(), Error> {
    let digest = DigestInfo::new([5; 32], 12);
    let hints_file = write_hints_file(&format!(
        r#"[ {{ "action_mnemonic": "Link", "memory_kb": 1000000 }},
             {{ "action_digest": "{digest}", "memory_kb": 9000000 }} ]"#
    ));
    let (mock_scheduler, scheduler) = make_scheduler(hints_file.clone());
    let action = make_base_action_info(UNIX_EPOCH, digest);
    let properties =
        properties_after_add(&scheduler, &mock_scheduler, action, Some(("", "Link"))).await?;
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
