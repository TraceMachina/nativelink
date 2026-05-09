#![allow(clippy::todo)]

use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use futures::{Stream, stream};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::awaited_action_db::AwaitedAction;
use nativelink_scheduler::store_awaited_action_db::inner_update_awaited_action;
use nativelink_util::action_messages::{
    ActionInfo, ActionUniqueKey, ActionUniqueQualifier, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::store_trait::{
    SchedulerCurrentVersionProvider, SchedulerIndexProvider, SchedulerStore,
    SchedulerStoreDataProvider, SchedulerStoreDecodeTo, SchedulerStoreKeyProvider,
    SchedulerSubscription, SchedulerSubscriptionManager,
};
use pretty_assertions::assert_eq;
use tokio::sync::Mutex;

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
