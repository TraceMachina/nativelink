#![allow(clippy::todo)]

use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use futures::{Stream, stream};
use mock_instant::thread_local::SystemTime as MockSystemTime;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::awaited_action_db::AwaitedAction;
use nativelink_scheduler::store_awaited_action_db::{
    StoreAwaitedActionDb, inner_update_awaited_action,
};
use nativelink_util::action_messages::{
    ActionInfo, ActionUniqueKey, ActionUniqueQualifier, OperationId,
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

    async fn search_by_index_prefix<K>(
        &self,
        _index: K,
    ) -> Result<
        impl Stream<Item = Result<<K as SchedulerStoreDecodeTo>::DecodeOutput, Error>> + Send,
        Error,
    >
    where
        K: SchedulerIndexProvider + SchedulerStoreDecodeTo + Send,
        <K as SchedulerStoreDecodeTo>::DecodeOutput: Send,
    {
        // todo!();
        Ok(stream::empty())
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
            "{\"version\":0,\"action_info\":{\"command_digest\":\"0101010101010101010101010101010101010101010101010101010101010101-10\",\"input_root_digest\":\"0202020202020202020202020202020202020202020202020202020202020202-10\",\"timeout\":{\"secs\":1,\"nanos\":0},\"platform_properties\":{},\"priority\":0,\"load_timestamp\":{\"secs_since_epoch\":0,\"nanos_since_epoch\":0},\"insert_timestamp\":{\"secs_since_epoch\":0,\"nanos_since_epoch\":0},\"unique_qualifier\":{\"Uncacheable\":{\"instance_name\":\"foo\",\"digest_function\":\"Sha256\",\"digest\":\"0303030303030303030303030303030303030303030303030303030303030303-10\"}}},\"operation_id\":{\"String\":\"DEMO_OPERATION_ID\"},\"sort_key\":9223372041149743103,\"last_worker_updated_timestamp\":{\"secs_since_epoch\":0,\"nanos_since_epoch\":0},\"last_client_keepalive_timestamp\":{\"secs_since_epoch\":0,\"nanos_since_epoch\":0},\"worker_id\":null,\"state\":{\"stage\":\"Queued\",\"last_transition_timestamp\":{\"secs_since_epoch\":0,\"nanos_since_epoch\":0},\"client_operation_id\":{\"String\":\"DEMO_OPERATION_ID\"},\"action_digest\":\"0303030303030303030303030303030303030303030303030303030303030303-10\"},\"maybe_origin_metadata\":null,\"attempts\":0}"
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
    empty_for: usize,
    encoded_action: Bytes,
}

impl EventuallyVisibleStore {
    fn new(empty_for: usize, action: &AwaitedAction) -> Self {
        let encoded = serde_json::to_vec(action).expect("serialize AwaitedAction for fake store");
        Self {
            search_calls: Arc::new(AtomicUsize::new(0)),
            empty_for,
            encoded_action: Bytes::from(encoded),
        }
    }
}

impl SchedulerStore for EventuallyVisibleStore {
    type SubscriptionManager = PendingSubscriptionManager;

    async fn subscription_manager(&self) -> Result<Arc<Self::SubscriptionManager>, Error> {
        Ok(Arc::new(PendingSubscriptionManager))
    }

    async fn update_data<T>(
        &self,
        _data: T,
        _expiry: Option<Duration>,
    ) -> Result<Option<i64>, Error>
    where
        T: SchedulerStoreDataProvider
            + SchedulerStoreKeyProvider
            + SchedulerCurrentVersionProvider
            + Send,
    {
        Ok(Some(1))
    }

    async fn search_by_index_prefix<K>(
        &self,
        _index: K,
    ) -> Result<
        impl Stream<Item = Result<<K as SchedulerStoreDecodeTo>::DecodeOutput, Error>> + Send,
        Error,
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
        Ok(stream::iter(items))
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
    StoreAwaitedActionDb::new(store, Arc::new(Notify::new()), now_fn, op_id_fn, 60)
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
    let db = build_db(store).await;

    let result = db
        .try_subscribe(
            &OperationId::from("client-1"),
            &qualifier,
            Duration::from_secs(60),
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
    let db = build_db(store).await;

    let result = db
        .try_subscribe(
            &OperationId::from("client-2"),
            &qualifier,
            Duration::from_secs(60),
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
    let db = build_db(store).await;

    let uncacheable = ActionUniqueQualifier::Uncacheable(ActionUniqueKey {
        instance_name: INSTANCE_NAME.to_string(),
        digest_function: DigestHasherFunc::Sha256,
        digest: DigestInfo::zero_digest(),
    });
    let result = db
        .try_subscribe(
            &OperationId::from("client-3"),
            &uncacheable,
            Duration::from_secs(60),
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
