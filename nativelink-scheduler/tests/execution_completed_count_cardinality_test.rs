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
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use mock_instant::thread_local::MockClock;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::awaited_action_db::{AwaitedActionDb, AwaitedActionSubscriber};
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_util::action_messages::{ActionResult, ActionStage, OperationId};
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use opentelemetry::metrics::{
    Counter, InstrumentBuilder, InstrumentProvider, Meter, MeterProvider, SyncInstrument,
};
use opentelemetry::{InstrumentationScope, KeyValue, global};
use tokio::sync::Notify;
use utils::scheduler_utils::make_base_action_info;

mod utils {
    pub(crate) mod scheduler_utils;
}

const NOW_TIME: u64 = 10_000;

fn make_system_time(add_time: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(NOW_TIME + add_time)
}

/// One recorded attribute set and the total added under it.
type AttributeSets = Mutex<HashMap<BTreeMap<String, String>, u64>>;

#[derive(Clone)]
struct RecordingMeterProvider(Arc<AttributeSets>);

impl MeterProvider for RecordingMeterProvider {
    fn meter_with_scope(&self, _scope: InstrumentationScope) -> Meter {
        Meter::new(Arc::new(self.clone()))
    }
}

impl InstrumentProvider for RecordingMeterProvider {
    fn u64_counter(&self, builder: InstrumentBuilder<'_, Counter<u64>>) -> Counter<u64> {
        Counter::new(Arc::new(RecordingInstrument {
            sets: self.0.clone(),
            enabled: builder.name == "execution.completed.count",
        }))
    }
}

struct RecordingInstrument {
    sets: Arc<AttributeSets>,
    enabled: bool,
}

impl SyncInstrument<u64> for RecordingInstrument {
    fn measure(&self, value: u64, attributes: &[KeyValue]) {
        if self.enabled {
            let set: BTreeMap<String, String> = attributes
                .iter()
                .map(|attr| (attr.key.to_string(), attr.value.to_string()))
                .collect();
            *self.sets.lock().unwrap().entry(set).or_insert(0) += value;
        }
    }
}

/// Regression test for the `execution.completed.count` cardinality overflow:
/// the counter once carried `execution.action_digest`, so every distinct
/// action consumed one of the OpenTelemetry SDK's attribute-set slots (2000
/// by default) and later series, including the bounded per-worker ones, fell
/// into the single `otel.metric.overflow=true` point. Completing more
/// distinct actions than that budget must not grow the attribute-set count.
#[nativelink_test]
async fn completed_count_attribute_sets_stay_bounded() -> Result<(), Error> {
    // More distinct actions than the SDK's default cardinality limit (2000).
    const DISTINCT_ACTIONS: u64 = 2001;

    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let sets = Arc::new(AttributeSets::default());
    global::set_meter_provider(RecordingMeterProvider(sets.clone()));

    let notify = Arc::new(Notify::new());
    let db = memory_awaited_action_db_factory(0, &notify, MockInstantWrapped::default);
    for i in 0..DISTINCT_ACTIONS {
        let mut hash = [0u8; 32];
        hash[..8].copy_from_slice(&i.to_be_bytes());
        let action_info = make_base_action_info(make_system_time(0), DigestInfo::new(hash, 512));
        let subscriber = db
            .add_action(OperationId::default(), action_info, Duration::from_mins(1))
            .await?;
        let mut action = subscriber.borrow().await?;
        let mut state = action.state().as_ref().clone();
        state.stage = ActionStage::Completed(ActionResult {
            exit_code: 0,
            ..ActionResult::default()
        });
        action.worker_set_state(Arc::new(state), make_system_time(1));
        db.update_awaited_action(action).await?;
        drop(subscriber);
    }

    let recorded = sets.lock().unwrap();
    for set in recorded.keys() {
        assert!(
            !set.contains_key("execution.action_digest"),
            "execution.completed.count must not carry the per-action digest: {set:?}"
        );
    }
    // Every completion had the same result, so this database must contribute
    // exactly one attribute set, however many distinct actions complete.
    assert_eq!(
        recorded.len(),
        1,
        "attribute sets grew with distinct actions: {:?}",
        recorded.keys().collect::<Vec<_>>()
    );
    let (set, count) = recorded.iter().next().unwrap();
    assert_eq!(
        set.get("execution.result").map(String::as_str),
        Some("success")
    );
    assert_eq!(*count, DISTINCT_ACTIONS);
    Ok(())
}
