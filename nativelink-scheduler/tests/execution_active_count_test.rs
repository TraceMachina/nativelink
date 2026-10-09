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
use std::sync::{Arc, LazyLock, Mutex, Once};
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::{Stream, stream};
use mock_instant::thread_local::MockClock;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::awaited_action_db::{
    AwaitedAction, AwaitedActionDb, AwaitedActionSubscriber,
};
use nativelink_scheduler::default_scheduler_factory::memory_awaited_action_db_factory;
use nativelink_scheduler::memory_awaited_action_db::MemoryAwaitedActionDb;
use nativelink_scheduler::store_awaited_action_db::StoreAwaitedActionDb;
use nativelink_util::action_messages::{ActionInfo, ActionResult, ActionStage, OperationId};
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::metrics::ActiveCountAttributes;
use nativelink_util::store_trait::{
    SchedulerCurrentVersionProvider, SchedulerIndexProvider, SchedulerStore,
    SchedulerStoreDataProvider, SchedulerStoreDecodeTo, SchedulerStoreKeyProvider,
    SchedulerSubscription, SchedulerSubscriptionManager,
};
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
const STAGE: &str = "execution.stage";
const OS: &str = "execution.platform.OSFamily";
/// Just past the store backend's 15 second recount.
const REFRESH: Duration = Duration::from_secs(16);

fn make_system_time(add_time: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(NOW_TIME + add_time)
}

/// One series of `execution.active.count`: its attributes, sorted by key.
type Series = Vec<(String, String)>;

/// Every `execution.active.count` series recorded so far and its value.
#[derive(Default)]
struct Recorder(Mutex<HashMap<Series, i64>>);

impl Recorder {
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }

    /// The value of the series with exactly these attributes, 0 if never
    /// recorded.
    fn value(&self, attributes: &[(&str, &str)]) -> i64 {
        let mut series: Series = attributes
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        series.sort();
        self.0.lock().unwrap().get(&series).copied().unwrap_or(0)
    }

    /// The sum of every series in `stage`, whatever else it is attributed by.
    fn stage_total(&self, stage: &str) -> i64 {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|(series, _)| {
                series
                    .iter()
                    .any(|(key, value)| key == "execution.stage" && value == stage)
            })
            .map(|(_, value)| value)
            .sum()
    }

    /// Every series that is not at zero.
    fn non_zero(&self) -> Vec<(Series, i64)> {
        let mut non_zero: Vec<_> = self
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, value)| **value != 0)
            .map(|(series, value)| (series.clone(), *value))
            .collect();
        non_zero.sort();
        non_zero
    }
}

#[derive(Clone)]
struct RecordingMeterProvider(Arc<Recorder>);

impl MeterProvider for RecordingMeterProvider {
    fn meter_with_scope(&self, _scope: InstrumentationScope) -> Meter {
        Meter::new(Arc::new(self.clone()))
    }
}

impl InstrumentProvider for RecordingMeterProvider {
    fn i64_up_down_counter(
        &self,
        builder: InstrumentBuilder<'_, UpDownCounter<i64>>,
    ) -> UpDownCounter<i64> {
        UpDownCounter::new(Arc::new(RecordingInstrument {
            recorder: self.0.clone(),
            enabled: builder.name == "execution.active.count",
        }))
    }
}

struct RecordingInstrument {
    recorder: Arc<Recorder>,
    enabled: bool,
}

impl SyncInstrument<i64> for RecordingInstrument {
    fn measure(&self, change: i64, attributes: &[KeyValue]) {
        if !self.enabled {
            return;
        }
        let mut series: Series = attributes
            .iter()
            .map(|attr| (attr.key.to_string(), attr.value.to_string()))
            .collect();
        series.sort();
        *self.recorder.0.lock().unwrap().entry(series).or_default() += change;
    }
}

/// `EXECUTION_METRICS` binds its instruments to whichever meter provider is
/// global when it is first touched, so one recorder serves every test in this
/// binary and the tests take turns on it.
static RECORDER: LazyLock<Arc<Recorder>> = LazyLock::new(Arc::default);
static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn install_recorder() -> Arc<Recorder> {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| global::set_meter_provider(RecordingMeterProvider(RECORDER.clone())));
    RECORDER.clear();
    RECORDER.clone()
}

fn make_action_info(digest: u8, platform_properties: &[(&str, &str)]) -> Arc<ActionInfo> {
    let mut action_info =
        make_base_action_info(make_system_time(0), DigestInfo::new([digest; 32], 512));
    Arc::make_mut(&mut action_info).platform_properties = platform_properties
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect();
    action_info
}

async fn set_stage<I, NowFn>(
    db: &MemoryAwaitedActionDb<I, NowFn>,
    operation_id: &OperationId,
    stage: ActionStage,
) -> Result<(), Error>
where
    I: nativelink_util::instant_wrapper::InstantWrapper,
    NowFn: Fn() -> I + Clone + Send + Sync + 'static,
{
    let mut action = db
        .get_by_operation_id(operation_id)
        .await?
        .expect("operation exists")
        .borrow()
        .await?;
    let mut new_state = action.state().as_ref().clone();
    new_state.stage = stage;
    action.worker_set_state(Arc::new(new_state), make_system_time(1));
    db.update_awaited_action(action).await
}

#[nativelink_test]
async fn dropping_executing_action_decrements_active_count() -> Result<(), Error> {
    let _serial = SERIAL.lock().await;
    let recorder = install_recorder();
    MockClock::set_time(Duration::from_secs(NOW_TIME));

    let notify = Arc::new(Notify::new());
    let db = memory_awaited_action_db_factory(
        0,
        &notify,
        MockInstantWrapped::default,
        ActiveCountAttributes::default(),
    );
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

    assert_eq!(recorder.stage_total("executing"), 1);

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

    assert_eq!(recorder.stage_total("executing"), 0);
    Ok(())
}

/// With `active_action_count_platform_properties: ["OSFamily"]`, each action
/// is counted under its own `OSFamily` value through every stage, an action
/// without the key is counted under `""`, and the series all return to zero
/// once the actions are gone.
#[nativelink_test]
async fn platform_property_attributes_follow_each_action() -> Result<(), Error> {
    let _serial = SERIAL.lock().await;
    let recorder = install_recorder();
    MockClock::set_time(Duration::from_secs(NOW_TIME));

    let notify = Arc::new(Notify::new());
    let db = memory_awaited_action_db_factory(
        0,
        &notify,
        MockInstantWrapped::default,
        ActiveCountAttributes::new(&["OSFamily".to_string()])?,
    );

    let actions = [
        (1, &[("OSFamily", "linux"), ("ISA", "x86-64")][..], "linux"),
        (2, &[("OSFamily", "macos")][..], "macos"),
        (3, &[("ISA", "x86-64")][..], ""),
    ];
    let mut operations = Vec::new();
    for (digest, platform_properties, _) in actions {
        let client_id = OperationId::default();
        let subscriber = db
            .add_action(
                client_id.clone(),
                make_action_info(digest, platform_properties),
                Duration::from_mins(1),
            )
            .await?;
        let operation_id = subscriber.borrow().await?.operation_id().clone();
        operations.push((client_id, operation_id, subscriber));
    }

    // Every action is queued under its own value, and the key the
    // configuration does not name leaves no trace.
    for (_, _, os) in actions {
        assert_eq!(recorder.value(&[(STAGE, "queued"), (OS, os)]), 1, "{os:?}");
    }
    assert_eq!(recorder.stage_total("queued"), 3);
    assert_eq!(recorder.non_zero().len(), 3);

    // Linux moves to executing: its series move with it, nothing else does.
    set_stage(&db, &operations[0].1, ActionStage::Executing).await?;
    assert_eq!(recorder.value(&[(STAGE, "queued"), (OS, "linux")]), 0);
    assert_eq!(recorder.value(&[(STAGE, "executing"), (OS, "linux")]), 1);
    assert_eq!(recorder.value(&[(STAGE, "queued"), (OS, "macos")]), 1);
    assert_eq!(recorder.value(&[(STAGE, "queued"), (OS, "")]), 1);
    assert_eq!(recorder.stage_total("executing"), 1);
    assert_eq!(recorder.stage_total("queued"), 2);

    // macOS follows while linux completes.
    set_stage(&db, &operations[1].1, ActionStage::Executing).await?;
    set_stage(
        &db,
        &operations[0].1,
        ActionStage::Completed(ActionResult::default()),
    )
    .await?;
    assert_eq!(recorder.value(&[(STAGE, "executing"), (OS, "linux")]), 0);
    assert_eq!(recorder.value(&[(STAGE, "completed"), (OS, "linux")]), 1);
    assert_eq!(recorder.value(&[(STAGE, "executing"), (OS, "macos")]), 1);
    assert_eq!(recorder.value(&[(STAGE, "queued"), (OS, "macos")]), 0);
    assert_eq!(recorder.value(&[(STAGE, "queued"), (OS, "")]), 1);

    set_stage(
        &db,
        &operations[1].1,
        ActionStage::Completed(ActionResult::default()),
    )
    .await?;
    set_stage(&db, &operations[2].1, ActionStage::Executing).await?;
    set_stage(
        &db,
        &operations[2].1,
        ActionStage::Completed(ActionResult::default()),
    )
    .await?;
    assert_eq!(recorder.stage_total("queued"), 0);
    assert_eq!(recorder.stage_total("executing"), 0);
    assert_eq!(recorder.stage_total("completed"), 3);
    for (_, _, os) in actions {
        assert_eq!(
            recorder.value(&[(STAGE, "completed"), (OS, os)]),
            1,
            "{os:?}"
        );
    }

    // The clients go away and their entries expire: every series is back at
    // zero, so nothing was counted in under one attribute set and counted
    // out under another.
    let operation_ids: Vec<_> = operations
        .iter()
        .map(|(client_id, operation_id, _)| (client_id.clone(), operation_id.clone()))
        .collect();
    drop(operations);
    MockClock::advance(Duration::from_mins(2));
    for (client_id, operation_id) in &operation_ids {
        assert!(db.get_awaited_action_by_id(client_id).await?.is_none());
        for _ in 0..20 {
            if db.get_by_operation_id(operation_id).await?.is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(db.get_by_operation_id(operation_id).await?.is_none());
    }
    assert_eq!(recorder.non_zero(), Vec::new());
    Ok(())
}

struct PendingSubscription;
impl SchedulerSubscription for PendingSubscription {
    async fn changed(&mut self) -> Result<(), Error> {
        futures::future::pending().await
    }
}

struct PendingSubscriptionManager;
impl SchedulerSubscriptionManager for PendingSubscriptionManager {
    type Subscription = PendingSubscription;
    fn subscribe<K>(&self, _key: K) -> Result<Self::Subscription, Error>
    where
        K: SchedulerStoreKeyProvider,
    {
        Ok(PendingSubscription)
    }
    fn is_reliable() -> bool {
        true
    }
}

/// Fake `SchedulerStore` holding a set of actions the test rewrites between
/// passes, indexed by stage the way the real store is: a listing or a count
/// for a stage sees the actions in that stage.
#[derive(Default)]
struct StagedStore {
    actions: Mutex<Vec<AwaitedAction>>,
}

impl StagedStore {
    fn set(&self, actions: Vec<AwaitedAction>) {
        *self.actions.lock().unwrap() = actions;
    }

    fn in_index(&self, index_value: &str) -> Vec<Bytes> {
        self.actions
            .lock()
            .unwrap()
            .iter()
            .filter(|action| {
                let stage = match action.state().stage {
                    ActionStage::CacheCheck => "cache_check",
                    ActionStage::Queued => "queued",
                    ActionStage::Executing => "executing",
                    ActionStage::Completed(_) | ActionStage::CompletedFromCache(_) => "completed",
                    ActionStage::Unknown => "unknown",
                };
                stage.starts_with(index_value)
            })
            .map(|action| Bytes::from(serde_json::to_vec(action).expect("serialize")))
            .collect()
    }
}

impl SchedulerStore for StagedStore {
    type SubscriptionManager = PendingSubscriptionManager;

    fn subscription_manager(
        &self,
    ) -> impl Future<Output = Result<Arc<Self::SubscriptionManager>, Error>> {
        std::future::ready(Ok(Arc::new(PendingSubscriptionManager)))
    }

    fn update_data<T>(
        &self,
        _data: T,
        _expiry: Option<Duration>,
    ) -> impl Future<Output = Result<Option<i64>, Error>>
    where
        T: SchedulerStoreDataProvider
            + SchedulerStoreKeyProvider
            + SchedulerCurrentVersionProvider
            + Send,
    {
        std::future::ready(Ok(Some(1)))
    }

    fn search_by_index_prefix<K>(
        &self,
        index: K,
    ) -> impl Future<
        Output = Result<
            impl Stream<Item = Result<<K as SchedulerStoreDecodeTo>::DecodeOutput, Error>> + Send,
            Error,
        >,
    >
    where
        K: SchedulerIndexProvider + SchedulerStoreDecodeTo + Send,
        <K as SchedulerStoreDecodeTo>::DecodeOutput: Send,
    {
        let items: Vec<_> = self
            .in_index(&index.index_value())
            .into_iter()
            .map(|encoded| K::decode(1, encoded))
            .collect();
        std::future::ready(Ok(stream::iter(items)))
    }

    async fn count_by_index_prefix<K>(&self, index: K) -> Result<u64, Error>
    where
        K: SchedulerIndexProvider + Send,
    {
        Ok(self.in_index(&index.index_value()).len() as u64)
    }

    async fn get_and_decode<K>(
        &self,
        _key: K,
    ) -> Result<Option<<K as SchedulerStoreDecodeTo>::DecodeOutput>, Error>
    where
        K: SchedulerStoreKeyProvider + SchedulerStoreDecodeTo + Send,
    {
        Ok(None)
    }
}

fn staged(name: &str, platform_properties: &[(&str, &str)], stage: ActionStage) -> AwaitedAction {
    let mut action = AwaitedAction::new(
        OperationId::from(name),
        make_action_info(1, platform_properties),
        make_system_time(0),
    );
    let mut new_state = action.state().as_ref().clone();
    new_state.stage = stage;
    action.worker_set_state(Arc::new(new_state), make_system_time(0));
    action
}

/// The store backend counts by reading the store on a timer. With keys
/// configured it reads the actions rather than asking for totals, reports
/// each series as the change since its last pass, and records a series that
/// emptied down to zero.
#[nativelink_test(start_paused = true)]
async fn store_backend_counts_each_platform_property_value() -> Result<(), Error> {
    let _serial = SERIAL.lock().await;
    let recorder = install_recorder();

    let store = Arc::new(StagedStore::default());
    store.set(vec![
        staged("linux-1", &[("OSFamily", "linux")], ActionStage::Queued),
        staged("linux-2", &[("OSFamily", "linux")], ActionStage::Queued),
        staged("macos-1", &[("OSFamily", "macos")], ActionStage::Queued),
        staged("macos-2", &[("OSFamily", "macos")], ActionStage::Executing),
        staged("bare", &[], ActionStage::Executing),
    ]);
    let now_fn: fn() -> MockInstantWrapped = MockInstantWrapped::default;
    let op_id_fn: fn() -> OperationId = OperationId::default;
    let _db = StoreAwaitedActionDb::new(
        store.clone(),
        Arc::new(Notify::new()),
        now_fn,
        op_id_fn,
        60,
        60,
        true,
        ActiveCountAttributes::new(&["OSFamily".to_string()])?,
    )
    .await?;

    tokio::time::sleep(REFRESH).await;
    assert_eq!(recorder.value(&[(STAGE, "queued"), (OS, "linux")]), 2);
    assert_eq!(recorder.value(&[(STAGE, "queued"), (OS, "macos")]), 1);
    assert_eq!(recorder.value(&[(STAGE, "executing"), (OS, "macos")]), 1);
    assert_eq!(recorder.value(&[(STAGE, "executing"), (OS, "")]), 1);
    // Nothing is reported under the stage alone: that would double the
    // totals of anyone summing over the platform label.
    assert_eq!(recorder.value(&[(STAGE, "queued")]), 0);
    assert_eq!(recorder.stage_total("queued"), 3);
    assert_eq!(recorder.stage_total("executing"), 2);

    // One linux action starts and the bare one finishes.
    store.set(vec![
        staged("linux-1", &[("OSFamily", "linux")], ActionStage::Executing),
        staged("linux-2", &[("OSFamily", "linux")], ActionStage::Queued),
        staged("macos-1", &[("OSFamily", "macos")], ActionStage::Queued),
        staged("macos-2", &[("OSFamily", "macos")], ActionStage::Executing),
        staged("bare", &[], ActionStage::Completed(ActionResult::default())),
    ]);
    tokio::time::sleep(REFRESH).await;
    assert_eq!(recorder.value(&[(STAGE, "queued"), (OS, "linux")]), 1);
    assert_eq!(recorder.value(&[(STAGE, "executing"), (OS, "linux")]), 1);
    assert_eq!(recorder.value(&[(STAGE, "queued"), (OS, "macos")]), 1);
    assert_eq!(recorder.value(&[(STAGE, "executing"), (OS, "macos")]), 1);
    assert_eq!(recorder.value(&[(STAGE, "executing"), (OS, "")]), 0);
    assert_eq!(recorder.value(&[(STAGE, "completed"), (OS, "")]), 1);

    // Everything is gone: every series the pass ever reported reads 0.
    store.set(Vec::new());
    tokio::time::sleep(REFRESH).await;
    assert_eq!(recorder.non_zero(), Vec::new());
    Ok(())
}
