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

use std::collections::HashMap;
use std::fs;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

mod utils {
    pub(crate) mod scheduler_utils;
}

use futures::join;
use nativelink_config::schedulers::{HistoricalResourceSpec, SchedulerSpec, SimpleSpec};
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
    let mock_scheduler = Arc::new(MockActionScheduler::new());
    let spec = HistoricalResourceSpec {
        hints_file,
        refresh_interval_s: 0,
        cpu_property_name: "cpu_count".to_string(),
        memory_property_name: "memory_kb".to_string(),
        scheduler: Box::new(SchedulerSpec::Simple(SimpleSpec::default())),
    };
    let scheduler = HistoricalResourceScheduler::new(&spec, mock_scheduler.clone());
    (mock_scheduler, scheduler)
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
