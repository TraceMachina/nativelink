#![allow(clippy::todo)]

use core::ops::Bound;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use futures::{Stream, StreamExt, stream};
use mock_instant::thread_local::{MockClock, SystemTime as MockSystemTime};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::awaited_action_db::{
    AwaitedAction, AwaitedActionDb, AwaitedActionSubscriber, PersistedSortKey, SortedAwaitedAction,
    SortedAwaitedActionState,
};
use nativelink_scheduler::store_awaited_action_db::{
    StoreAwaitedActionDb, inner_update_awaited_action,
};
use nativelink_scheduler::worker_registry::{ORPHANED_ACTION_TIMEOUT, WorkerRegistry};
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionUniqueKey, ActionUniqueQualifier, OperationId, WorkerId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::store_trait::{
    SchedulerCurrentVersionProvider, SchedulerIndexProvider, SchedulerStore,
    SchedulerStoreDataProvider, SchedulerStoreDecodeTo, SchedulerStoreKeyProvider,
    SchedulerSubscription, SchedulerSubscriptionManager,
};
use pretty_assertions::assert_eq;
use tokio::sync::{Mutex, Notify};

const INSTANCE_NAME: &str = "foo";

struct FakeSchedulerStore {
    #[allow(clippy::type_complexity)]
    updates: Arc<Mutex<Vec<(Bytes, Option<Duration>)>>>,
}

impl FakeSchedulerStore {
    fn new() -> Self {
        Self {
            updates: Arc::new(Mutex::new(vec![])),
        }
    }
}
struct FakeSubscriptionManager {}
struct FakeSubscription {}

impl SchedulerSubscription for FakeSubscription {
    async fn changed(&mut self) -> Result<(), Error> {
        todo!()
    }
}

impl SchedulerSubscriptionManager for FakeSubscriptionManager {
    type Subscription = FakeSubscription;

    fn subscribe<K>(&self, _key: K) -> Result<Self::Subscription, Error>
    where
        K: SchedulerStoreKeyProvider,
    {
        todo!()
    }

    fn is_reliable() -> bool {
        todo!()
    }
}

impl SchedulerStore for FakeSchedulerStore {
    type SubscriptionManager = FakeSubscriptionManager;

    async fn subscription_manager(&self) -> Result<Arc<Self::SubscriptionManager>, Error> {
        todo!()
    }

    async fn update_data<T>(&self, data: T, expiry: Option<Duration>) -> Result<Option<i64>, Error>
    where
        T: SchedulerStoreDataProvider
            + SchedulerStoreKeyProvider
            + SchedulerCurrentVersionProvider
            + Send,
    {
        self.updates
            .lock()
            .await
            .push((data.try_into_bytes()?, expiry));
        Ok(Some(1))
    }

    fn search_by_index_prefix<K>(
        &self,
        _index: K,
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
        std::future::ready(Ok(stream::empty()))
    }

    async fn count_by_index_prefix<K>(&self, _index: K) -> Result<u64, Error>
    where
        K: SchedulerIndexProvider + Send,
    {
        Ok(0)
    }

    async fn get_and_decode<K>(
        &self,
        _key: K,
    ) -> Result<Option<<K as SchedulerStoreDecodeTo>::DecodeOutput>, Error>
    where
        K: SchedulerStoreKeyProvider + SchedulerStoreDecodeTo + Send,
    {
        todo!()
    }
}

#[nativelink_test]
async fn test_inner_update_awaited_action() -> Result<(), Error> {
    let store = FakeSchedulerStore::new();
    let action_digest = DigestInfo::new([3u8; 32], 10);
    let action_info = ActionInfo {
        command_digest: DigestInfo::new([1u8; 32], 10),
        input_root_digest: DigestInfo::new([2u8; 32], 10),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: action_digest,
        }),
    };

    let awaited_action = AwaitedAction::new(
        OperationId::from("DEMO_OPERATION_ID"),
        action_info.into(),
        SystemTime::UNIX_EPOCH,
    );
    inner_update_awaited_action(&store, awaited_action, Some(Duration::from_mins(5))).await?;
    let updates = store.updates.lock().await;
    assert_eq!(updates.len(), 1, "{updates:#?}");
    let update = updates.first().unwrap();
    assert_eq!(
        update.0,
        Bytes::from(
            "{\"version\":0,\"action_info\":{\"command_digest\":\"0101010101010101010101010101010101010101010101010101010101010101-10\",\"input_root_digest\":\"0202020202020202020202020202020202020202020202020202020202020202-10\",\"timeout\":{\"secs\":1,\"nanos\":0},\"platform_properties\":{},\"priority\":0,\"load_timestamp\":{\"secs_since_epoch\":0,\"nanos_since_epoch\":0},\"insert_timestamp\":{\"secs_since_epoch\":0,\"nanos_since_epoch\":0},\"unique_qualifier\":{\"Uncacheable\":{\"instance_name\":\"foo\",\"digest_function\":\"Sha256\",\"digest\":\"0303030303030303030303030303030303030303030303030303030303030303-10\"}}},\"operation_id\":{\"String\":\"DEMO_OPERATION_ID\"},\"sort_key\":9223372041149743103,\"last_worker_updated_timestamp\":{\"secs_since_epoch\":0,\"nanos_since_epoch\":0},\"last_client_keepalive_timestamp\":{\"secs_since_epoch\":0,\"nanos_since_epoch\":0},\"worker_id\":null,\"state\":{\"stage\":\"Queued\",\"last_transition_timestamp\":{\"secs_since_epoch\":0,\"nanos_since_epoch\":0},\"client_operation_id\":{\"String\":\"DEMO_OPERATION_ID\"},\"action_digest\":\"0303030303030303030303030303030303030303030303030303030303030303-10\"},\"maybe_origin_metadata\":null,\"attempts\":0,\"worker_losses\":0,\"escalations\":0}"
        ),
        "{update:#?}"
    );
    assert_eq!(update.1, Some(Duration::from_mins(5)));
    Ok(())
}

// ---------------------------------------------------------------------------
// `try_subscribe` retry behavior.
// ---------------------------------------------------------------------------

fn make_cacheable_action_info() -> Arc<ActionInfo> {
    Arc::new(ActionInfo {
        command_digest: DigestInfo::zero_digest(),
        input_root_digest: DigestInfo::zero_digest(),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Cacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: DigestInfo::zero_digest(),
        }),
    })
}

fn make_existing_awaited_action() -> AwaitedAction {
    AwaitedAction::new(
        OperationId::from("existing-operation"),
        make_cacheable_action_info(),
        MockSystemTime::now().into(),
    )
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

/// Fake `SchedulerStore` whose `search_by_index_prefix` returns an empty
/// stream for the first `empty_for` calls and a single-item decoded stream
/// thereafter — simulates the `RediSearch` index-visibility lag the retry
/// is meant to absorb.
struct EventuallyVisibleStore {
    search_calls: Arc<AtomicUsize>,
    count_calls: Arc<AtomicUsize>,
    empty_for: usize,
    encoded_action: Bytes,
}

impl EventuallyVisibleStore {
    fn new(empty_for: usize, action: &AwaitedAction) -> Self {
        let encoded = serde_json::to_vec(action).expect("serialize AwaitedAction for fake store");
        Self {
            search_calls: Arc::new(AtomicUsize::new(0)),
            count_calls: Arc::new(AtomicUsize::new(0)),
            empty_for,
            encoded_action: Bytes::from(encoded),
        }
    }
}

impl SchedulerStore for EventuallyVisibleStore {
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
        _index: K,
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
        let n = self.search_calls.fetch_add(1, Ordering::SeqCst);
        let items: Vec<Result<<K as SchedulerStoreDecodeTo>::DecodeOutput, Error>> =
            if n < self.empty_for {
                Vec::new()
            } else {
                vec![K::decode(1, self.encoded_action.clone())]
            };
        std::future::ready(Ok(stream::iter(items)))
    }

    async fn count_by_index_prefix<K>(&self, _index: K) -> Result<u64, Error>
    where
        K: SchedulerIndexProvider + Send,
    {
        self.count_calls.fetch_add(1, Ordering::SeqCst);
        Ok(0)
    }

    async fn get_and_decode<K>(
        &self,
        _key: K,
    ) -> Result<Option<<K as SchedulerStoreDecodeTo>::DecodeOutput>, Error>
    where
        K: SchedulerStoreKeyProvider + SchedulerStoreDecodeTo + Send,
    {
        todo!("not exercised by try_subscribe")
    }
}

async fn build_db(
    store: Arc<EventuallyVisibleStore>,
    enable_active_action_count_metric: bool,
) -> StoreAwaitedActionDb<
    EventuallyVisibleStore,
    fn() -> OperationId,
    MockInstantWrapped,
    fn() -> MockInstantWrapped,
> {
    fn new_op_id() -> OperationId {
        OperationId::from("new-operation")
    }
    let now_fn: fn() -> MockInstantWrapped = MockInstantWrapped::default;
    let op_id_fn: fn() -> OperationId = new_op_id;
    StoreAwaitedActionDb::new(
        store,
        Arc::new(Notify::new()),
        now_fn,
        op_id_fn,
        60,
        60,
        enable_active_action_count_metric,
    )
    .await
    .expect("construct test db")
}

#[nativelink_test]
async fn try_subscribe_retries_once_on_miss_then_returns_existing() -> Result<(), Error> {
    let action = make_existing_awaited_action();
    let qualifier = action.action_info().unique_qualifier.clone();
    let store = Arc::new(EventuallyVisibleStore::new(
        /* empty_for = */ 1, &action,
    ));
    let counter = store.search_calls.clone();
    let db = build_db(store, false).await;

    let result = db
        .try_subscribe(
            &OperationId::from("client-1"),
            &qualifier,
            Duration::from_mins(1),
            0,
        )
        .await?;

    assert!(
        result.is_some(),
        "retry should surface the action that became visible on the second lookup",
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "search_by_index_prefix must run exactly twice (first miss, then hit)",
    );
    Ok(())
}

#[nativelink_test]
async fn try_subscribe_returns_none_after_two_consecutive_misses() -> Result<(), Error> {
    let action = make_existing_awaited_action();
    let qualifier = action.action_info().unique_qualifier.clone();
    let store = Arc::new(EventuallyVisibleStore::new(
        /* empty_for = */ usize::MAX,
        &action,
    ));
    let counter = store.search_calls.clone();
    let db = build_db(store, false).await;

    let result = db
        .try_subscribe(
            &OperationId::from("client-2"),
            &qualifier,
            Duration::from_mins(1),
            0,
        )
        .await?;

    assert!(
        result.is_none(),
        "two consecutive misses must return None — no further retries",
    );
    assert_eq!(
        counter.load(Ordering::SeqCst),
        2,
        "retry bound must cap search_by_index_prefix calls at 2",
    );
    Ok(())
}

#[nativelink_test]
async fn try_subscribe_skips_lookup_for_uncacheable_qualifier() -> Result<(), Error> {
    let action = make_existing_awaited_action();
    let store = Arc::new(EventuallyVisibleStore::new(
        /* empty_for = */ 0, &action,
    ));
    let counter = store.search_calls.clone();
    let db = build_db(store, false).await;

    let uncacheable = ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
        execution_scope: None,
        instance_name: INSTANCE_NAME.to_string(),
        digest_function: DigestHasherFunc::Sha256,
        digest: DigestInfo::zero_digest(),
    });
    let result = db
        .try_subscribe(
            &OperationId::from("client-3"),
            &uncacheable,
            Duration::from_mins(1),
            0,
        )
        .await?;

    assert!(result.is_none());
    assert_eq!(
        counter.load(Ordering::SeqCst),
        0,
        "uncacheable qualifier must short-circuit before any lookup",
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// `try_subscribe` abandonment check.
//
// The stage/timestamp pair lives in the shared store and heartbeats never
// refresh it, so without a liveness check a healthy long-running action looks
// abandoned and a duplicate Execute forks a second execution of it.
// ---------------------------------------------------------------------------

const WORKER_TIMEOUT: Duration = Duration::from_mins(2);
const NOW_TIME: u64 = 10_000;

/// `MockInstantWrapped::now` reads `MockClock::time`, so timestamps have to be
/// built from the same clock or nothing lines up.
fn mock_now() -> SystemTime {
    SystemTime::UNIX_EPOCH + MockClock::time()
}

fn make_executing_awaited_action(worker_id: &WorkerId) -> AwaitedAction {
    let now = mock_now();
    let mut action = AwaitedAction::new(
        OperationId::from("existing-operation"),
        make_cacheable_action_info(),
        now,
    );
    action.set_worker_id(Some(worker_id.clone()), now);
    let mut state = action.state().as_ref().clone();
    state.stage = ActionStage::Executing;
    action.worker_set_state(Arc::new(state), now);
    action
}

/// Runs `try_subscribe` against an executing action owned by `worker_id`,
/// after `elapsed` has passed, and reports whether the action was recreated
/// rather than joined.
async fn recreated_after(
    registry: Option<Arc<WorkerRegistry>>,
    worker_id: &WorkerId,
    elapsed: Duration,
) -> Result<bool, Error> {
    let action = make_executing_awaited_action(worker_id);
    let qualifier = action.action_info().unique_qualifier.clone();
    let store = Arc::new(EventuallyVisibleStore::new(
        /* empty_for = */ 0, &action,
    ));
    let mut db = build_db(store, false).await;
    if let Some(registry) = registry {
        db.set_worker_registry(registry);
    }

    MockClock::advance(elapsed);

    let result = db
        .try_subscribe(
            &OperationId::from("client-abandon"),
            &qualifier,
            WORKER_TIMEOUT,
            0,
        )
        .await?
        .expect("the action is visible, so try_subscribe must return it");

    Ok(*result.operation_id() == OperationId::from("new-operation"))
}

#[nativelink_test]
async fn joins_an_executing_action_whose_worker_is_ours_and_alive() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let worker_id = WorkerId::from("worker-alive".to_string());
    let registry = Arc::new(WorkerRegistry::new());

    // Heartbeats keep the registry current even though the shared timestamp
    // on the action does not move.
    let action = make_executing_awaited_action(&worker_id);
    let qualifier = action.action_info().unique_qualifier.clone();
    let store = Arc::new(EventuallyVisibleStore::new(
        /* empty_for = */ 0, &action,
    ));
    let mut db = build_db(store, false).await;
    db.set_worker_registry(registry.clone());

    // The action's shared timestamp stays put while the worker keeps
    // heartbeating, which is exactly the case that used to look abandoned.
    MockClock::advance(WORKER_TIMEOUT * 3);
    registry
        .update_worker_heartbeat(&worker_id, mock_now())
        .await;

    let result = db
        .try_subscribe(
            &OperationId::from("client-alive"),
            &qualifier,
            WORKER_TIMEOUT,
            0,
        )
        .await?
        .expect("action is visible");
    let recreated = *result.operation_id() == OperationId::from("new-operation");

    assert!(
        !recreated,
        "a worker we can see heartbeating owns this action; recreating it would run the work twice",
    );
    Ok(())
}

#[nativelink_test]
async fn recreates_an_executing_action_whose_worker_went_quiet() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let worker_id = WorkerId::from("worker-stale".to_string());
    let registry = Arc::new(WorkerRegistry::new());
    registry.register_worker(&worker_id, mock_now()).await;

    let recreated = recreated_after(Some(registry), &worker_id, WORKER_TIMEOUT * 2).await?;

    assert!(
        recreated,
        "the worker is registered here and has stopped reporting, so the action is genuinely abandoned",
    );
    Ok(())
}

#[nativelink_test]
async fn joins_an_executing_action_on_a_peers_worker() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let worker_id = WorkerId::from("worker-elsewhere".to_string());
    // Empty registry: the worker belongs to another scheduler instance.
    let registry = Arc::new(WorkerRegistry::new());

    let recreated = recreated_after(Some(registry), &worker_id, WORKER_TIMEOUT * 5).await?;

    assert!(
        !recreated,
        "worker_timeout_s must not apply to a worker this instance never sees heartbeats for",
    );
    Ok(())
}

#[nativelink_test]
async fn recreates_an_orphan_once_past_the_backstop() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let worker_id = WorkerId::from("worker-orphaned".to_string());
    let registry = Arc::new(WorkerRegistry::new());

    let recreated =
        recreated_after(Some(registry), &worker_id, ORPHANED_ACTION_TIMEOUT * 2).await?;

    assert!(
        recreated,
        "an action nobody claims still has to be reaped eventually or it leaks forever",
    );
    Ok(())
}

#[nativelink_test]
async fn falls_back_to_the_timestamp_when_there_is_no_registry() -> Result<(), Error> {
    MockClock::set_time(Duration::from_secs(NOW_TIME));
    let worker_id = WorkerId::from("worker-no-registry".to_string());

    let recreated = recreated_after(None, &worker_id, WORKER_TIMEOUT * 2).await?;

    assert!(
        recreated,
        "without a registry there is nothing better than the timestamp, so behaviour is unchanged",
    );
    Ok(())
}

/// Counting the actions in each stage queries the scheduler store on a timer,
/// from every replica, against the same backend that serves scheduling. A
/// deployment that does not read `execution.active.count` should not pay for
/// it, so the work only happens when it is asked for.
#[nativelink_test(start_paused = true)]
async fn active_action_count_only_queries_the_store_when_enabled() -> Result<(), Error> {
    let action = make_existing_awaited_action();

    let off_store = Arc::new(EventuallyVisibleStore::new(
        /* empty_for = */ 0, &action,
    ));
    let off_counts = off_store.count_calls.clone();
    let _off_db = build_db(off_store, false).await;
    tokio::time::sleep(Duration::from_mins(1)).await;
    assert_eq!(
        off_counts.load(Ordering::SeqCst),
        0,
        "the store must not be queried at all while the metric is disabled",
    );

    let on_store = Arc::new(EventuallyVisibleStore::new(
        /* empty_for = */ 0, &action,
    ));
    let on_counts = on_store.count_calls.clone();
    let _on_db = build_db(on_store, true).await;
    tokio::time::sleep(Duration::from_mins(1)).await;
    assert!(
        on_counts.load(Ordering::SeqCst) >= 4,
        "enabling it should count every stage at least once a minute, got {}",
        on_counts.load(Ordering::SeqCst),
    );
    Ok(())
}

fn queued_at(insert_timestamp: SystemTime, priority: i32) -> AwaitedAction {
    let mut action_info = make_cacheable_action_info();
    let action_info_mut = Arc::make_mut(&mut action_info);
    action_info_mut.insert_timestamp = insert_timestamp;
    action_info_mut.priority = priority;
    AwaitedAction::new(OperationId::default(), action_info, insert_timestamp)
}

/// The record's `sort_key` stays a `u64` in the layout released schedulers
/// read, so a record written here loads on an older scheduler during a
/// rolling upgrade or after a rollback.
#[nativelink_test]
async fn record_sort_key_is_readable_by_an_older_scheduler() -> Result<(), Error> {
    let insert = SystemTime::UNIX_EPOCH + Duration::from_nanos(1_700_000_000_123_456_789);
    let record = serde_json::to_value(queued_at(insert, 0)).expect("a record serializes");
    let sort_key = record["sort_key"]
        .as_u64()
        .expect("sort_key must be a u64 for older readers");
    // Priority 0 sits at the top of the unsigned range, then the inverted
    // whole seconds.
    assert_eq!(
        sort_key,
        (0x8000_0000u64 << 32) | u64::from(0x6553_f100_u32 ^ u32::MAX)
    );
    Ok(())
}

/// A record written by an older scheduler carries only the seconds key,
/// yet once loaded here it orders at nanosecond resolution like any other,
/// because the key is computed from the action info both versions store.
#[nativelink_test]
async fn a_record_from_an_older_scheduler_orders_at_nanosecond_resolution() -> Result<(), Error> {
    let first = SystemTime::UNIX_EPOCH + Duration::from_millis(1_700_000_000_100);
    let second = first + Duration::from_millis(1);
    let reload = |action: AwaitedAction| -> Result<AwaitedAction, Error> {
        let bytes = serde_json::to_vec(&action).expect("a record serializes");
        AwaitedAction::try_from(bytes.as_slice())
    };
    let first = reload(queued_at(first, 0))?;
    let second = reload(queued_at(second, 0))?;
    // The descending read serves the larger key first.
    assert!(
        SortedAwaitedAction::from(&first).sort_key > SortedAwaitedAction::from(&second).sort_key,
        "the action queued first must be served first"
    );
    Ok(())
}

/// During a rolling upgrade the Redis index holds 16-digit keys from older
/// schedulers next to 32-digit keys from this one. It sorts them as strings,
/// and the descending read serves the older entries first.
#[nativelink_test]
async fn index_keys_from_both_versions_serve_older_entries_first() -> Result<(), Error> {
    let earlier = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    let later = earlier + Duration::from_mins(1);
    for priority in [i32::MIN, -1, 0, 1, i32::MAX] {
        let old = PersistedSortKey::from_action_info(queued_at(earlier, priority).action_info())
            .index_field();
        let new = SortedAwaitedAction::from(&queued_at(later, priority))
            .sort_key
            .index_field();
        assert_eq!(old.len(), 16);
        assert_eq!(new.len(), 32);
        let mut keys = vec![new.clone(), old.clone()];
        keys.sort_unstable_by(|a, b| b.cmp(a));
        assert_eq!(keys, vec![old, new], "priority {priority}");
    }
    Ok(())
}

/// Fake `SchedulerStore` whose listing returns the given actions and which
/// counts every `get_and_decode` by key prefix, so a test can see what a
/// listed subscriber reads when borrowed.
struct ListingStore {
    encoded_actions: Vec<Bytes>,
    reads_by_prefix: Mutex<HashMap<String, usize>>,
}

impl ListingStore {
    fn new(actions: &[AwaitedAction]) -> Self {
        Self {
            encoded_actions: actions
                .iter()
                .map(|action| Bytes::from(serde_json::to_vec(action).expect("serialize")))
                .collect(),
            reads_by_prefix: Mutex::new(HashMap::new()),
        }
    }
}

impl SchedulerStore for ListingStore {
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
        _index: K,
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
            .encoded_actions
            .iter()
            .map(|encoded| K::decode(1, encoded.clone()))
            .collect();
        std::future::ready(Ok(stream::iter(items)))
    }

    async fn count_by_index_prefix<K>(&self, _index: K) -> Result<u64, Error>
    where
        K: SchedulerIndexProvider + Send,
    {
        Ok(self.encoded_actions.len() as u64)
    }

    async fn get_and_decode<K>(
        &self,
        key: K,
    ) -> Result<Option<<K as SchedulerStoreDecodeTo>::DecodeOutput>, Error>
    where
        K: SchedulerStoreKeyProvider + SchedulerStoreDecodeTo + Send,
    {
        let key = key.get_key().as_str().to_string();
        let prefix = key.split('_').next().unwrap_or("").to_string();
        *self.reads_by_prefix.lock().await.entry(prefix).or_default() += 1;
        Ok(None)
    }
}

/// A listing already loads every record it returns, so borrowing a listed
/// subscriber must not read the record again. Only the client keepalive,
/// kept under its own key, is read, and once per subscriber.
#[nativelink_test]
async fn listed_subscriber_borrows_without_reading_the_record_again() -> Result<(), Error> {
    fn new_op_id() -> OperationId {
        OperationId::from("new-operation")
    }
    let actions: Vec<AwaitedAction> = (0..3)
        .map(|i| {
            AwaitedAction::new(
                OperationId::from(format!("op-{i}")),
                make_cacheable_action_info(),
                MockSystemTime::now().into(),
            )
        })
        .collect();
    let store = Arc::new(ListingStore::new(&actions));
    let now_fn: fn() -> MockInstantWrapped = MockInstantWrapped::default;
    let op_id_fn: fn() -> OperationId = new_op_id;
    let db = StoreAwaitedActionDb::new(
        store.clone(),
        Arc::new(Notify::new()),
        now_fn,
        op_id_fn,
        60,
        60,
        false,
    )
    .await?;

    let subscribers: Vec<_> = db
        .get_range_of_actions(
            SortedAwaitedActionState::Queued,
            Bound::Unbounded,
            Bound::Unbounded,
            true,
        )
        .await?
        .collect::<Vec<_>>()
        .await;
    assert_eq!(subscribers.len(), 3);
    for (subscriber, action) in subscribers.iter().zip(&actions) {
        let subscriber = subscriber.as_ref().map_err(Clone::clone)?;
        // Borrowed twice, as the matcher does.
        assert_eq!(
            subscriber.borrow().await?.operation_id(),
            action.operation_id()
        );
        assert_eq!(
            subscriber.borrow().await?.operation_id(),
            action.operation_id()
        );
    }

    let reads = store.reads_by_prefix.lock().await;
    assert_eq!(reads.get("aa"), None, "the record is not read again");
    assert_eq!(
        reads.get("ck"),
        Some(&3),
        "the keepalive is read once per subscriber"
    );
    Ok(())
}
