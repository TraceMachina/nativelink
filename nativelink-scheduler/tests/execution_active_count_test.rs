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
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use mock_instant::thread_local::MockClock;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::awaited_action_db::{AwaitedActionDb, AwaitedActionSubscriber};
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_util::action_messages::{ActionStage, OperationId};
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use opentelemetry::metrics::{
    InstrumentBuilder, InstrumentProvider, Meter, MeterProvider, SyncInstrument, UpDownCounter,
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

#[derive(Clone)]
struct CountingMeterProvider(Arc<AtomicI64>);

impl MeterProvider for CountingMeterProvider {
    fn meter_with_scope(&self, _scope: InstrumentationScope) -> Meter {
        Meter::new(Arc::new(self.clone()))
    }
}

impl InstrumentProvider for CountingMeterProvider {
    fn i64_up_down_counter(
        &self,
        builder: InstrumentBuilder<'_, UpDownCounter<i64>>,
    ) -> UpDownCounter<i64> {
        UpDownCounter::new(Arc::new(CountingInstrument {
            counts: self.0.clone(),
            enabled: builder.name == "execution.active.count",
        }))
    }
}

struct CountingInstrument {
    counts: Arc<AtomicI64>,
    enabled: bool,
}

impl SyncInstrument<i64> for CountingInstrument {
    fn measure(&self, change: i64, attributes: &[KeyValue]) {
        if self.enabled
            && attributes.iter().any(|attr| {
                attr.key.as_str() == "execution.stage" && attr.value.to_string() == "executing"
            })
        {
            self.counts.fetch_add(change, Ordering::SeqCst);
        }
    }
}

#[nativelink_test]
async fn dropping_executing_action_decrements_active_count() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let counts = Arc::new(AtomicI64::new(0));
    global::set_meter_provider(CountingMeterProvider(counts.clone()));

    let notify = Arc::new(Notify::new());
    let db = memory_awaited_action_db_factory(0, &notify, MockInstantWrapped::default);
    let client_id = OperationId::default();
    let action_info = make_base_action_info(make_system_time(0), DigestInfo::new([99; 32], 512));
    let subscriber = db
        .add_action(client_id, action_info, Duration::from_mins(1))
        .await?;
    let mut action = subscriber.borrow().await?;
    let operation_id = action.operation_id().clone();
    let mut state = action.state().as_ref().clone();
    state.stage = ActionStage::Executing;
    action.worker_set_state(Arc::new(state), make_system_time(1));
    db.update_awaited_action(action).await?;

    assert_eq!(counts.load(Ordering::SeqCst), 1);

    // A new insert evicts the first client's entry after its retain window.
    MockClock::advance(Duration::from_mins(2));
    let second_client = OperationId::default();
    let second_info = make_base_action_info(make_system_time(2), DigestInfo::new([100; 32], 512));
    let _second_subscriber = db
        .add_action(second_client, second_info, Duration::from_mins(1))
        .await?;
    for _ in 0..20 {
        if db.get_by_operation_id(&operation_id).await?.is_none() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(db.get_by_operation_id(&operation_id).await?.is_none());

    assert_eq!(counts.load(Ordering::SeqCst), 0);
    Ok(())
}
