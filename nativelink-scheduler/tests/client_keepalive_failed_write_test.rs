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

// Repro for: a failed client-keepalive store write in
// `OperationSubscriber::changed` (USE_SEPARATE_CLIENT_KEEPALIVE_KEY branch)
// still advances the local `last_known_keepalive_ts`, so the retry gate
// (`elapsed() > CLIENT_KEEPALIVE_DURATION`) is suppressed for a full
// CLIENT_KEEPALIVE_DURATION even though the shared store's ck_* key was
// never written. Peer scheduler replicas judge client liveness solely from
// the store, so a couple of consecutive failed writes leave the store
// timestamp stale past `client_action_timeout` and the peer retires the
// action as abandoned while the client is actively listening.

use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use bytes::Bytes;
use futures::{Stream, stream};
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_scheduler::awaited_action_db::{
    AwaitedAction, AwaitedActionDb, AwaitedActionSubscriber,
};
use nativelink_scheduler::store_awaited_action_db::StoreAwaitedActionDb;
use nativelink_util::action_messages::{
    ActionInfo, ActionUniqueKey, ActionUniqueQualifier, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::store_trait::{
    SchedulerCurrentVersionProvider, SchedulerIndexProvider, SchedulerStore,
    SchedulerStoreDataProvider, SchedulerStoreDecodeTo, SchedulerStoreKeyProvider,
    SchedulerSubscription, SchedulerSubscriptionManager, StoreKey,
};
use tokio::sync::Notify;

const INSTANCE_NAME: &str = "foo";

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

/// A store holding one queued `AwaitedAction` whose `ck_*` (client
/// keepalive) writes always fail — a Redis blip / dead shard. Every other
/// operation succeeds.
struct KeepaliveWriteFailsStore {
    operation_id: OperationId,
    encoded_action: Bytes,
    keepalive_write_attempts: AtomicUsize,
}

impl SchedulerStore for KeepaliveWriteFailsStore {
    type SubscriptionManager = PendingSubscriptionManager;

    fn subscription_manager(
        &self,
    ) -> impl Future<Output = Result<Arc<Self::SubscriptionManager>, Error>> {
        std::future::ready(Ok(Arc::new(PendingSubscriptionManager)))
    }

    async fn update_data<T>(&self, data: T, _expiry: Option<Duration>) -> Result<Option<i64>, Error>
    where
        T: SchedulerStoreDataProvider
            + SchedulerStoreKeyProvider
            + SchedulerCurrentVersionProvider
            + Send,
    {
        let StoreKey::Str(key) = data.get_key() else {
            panic!("unexpected non-string scheduler key");
        };
        if key.starts_with("ck_") {
            self.keepalive_write_attempts.fetch_add(1, Ordering::SeqCst);
            return Err(make_err!(
                Code::Unavailable,
                "injected transient store failure for client keepalive write"
            ));
        }
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
        key: K,
    ) -> Result<Option<<K as SchedulerStoreDecodeTo>::DecodeOutput>, Error>
    where
        K: SchedulerStoreKeyProvider + SchedulerStoreDecodeTo + Send,
    {
        let StoreKey::Str(key) = key.get_key() else {
            panic!("unexpected non-string scheduler key");
        };
        if key.starts_with("cid_") {
            let bytes = Bytes::from(serde_json::to_vec(&self.operation_id).unwrap());
            return Ok(Some(K::decode(0, bytes)?));
        }
        if key.starts_with("aa_") {
            return Ok(Some(K::decode(1, self.encoded_action.clone())?));
        }
        if key.starts_with("ck_") {
            // No keepalive write ever landed in the store.
            return Ok(None);
        }
        panic!("unexpected key {key}");
    }
}

fn make_queued_action(operation_id: &OperationId) -> AwaitedAction {
    let action_info = Arc::new(ActionInfo {
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
    });
    // The action's embedded keepalive timestamp is at the epoch: from the
    // shared store's point of view the client has not checked in for a very
    // long time, exactly as after a couple of failed ck_ writes.
    AwaitedAction::new(operation_id.clone(), action_info, SystemTime::UNIX_EPOCH)
}

/// A live client's subscriber whose keepalive store write fails must retry
/// the write well before peer replicas' `client_action_timeout` elapses;
/// otherwise the store timestamp stays stale and
/// `sweep_abandoned_queued_actions` / `apply_filter_predicate` on another
/// replica retires the action as "no more clients listening" out from under
/// a client that is actively listening.
///
/// Uses `start_paused` tokio time: the subscriber's sleep between keepalive
/// attempts is virtual, so 30 virtual seconds (3x CLIENT_KEEPALIVE_DURATION)
/// elapse instantly, while the retry gate — which reads the *local*
/// `last_known_keepalive_ts` through real wall-clock `SystemTime::elapsed` —
/// sees essentially zero elapsed real time after the failed write advanced
/// it. This exposes exactly the bug: the local timestamp is advanced even
/// though the store write failed, so no retry happens for a full
/// CLIENT_KEEPALIVE_DURATION.
#[nativelink_test(start_paused = true)]
async fn failed_keepalive_write_must_be_retried() -> Result<(), Error> {
    let operation_id = OperationId::from("live-client-operation");
    let action = make_queued_action(&operation_id);
    let store = Arc::new(KeepaliveWriteFailsStore {
        operation_id: operation_id.clone(),
        encoded_action: Bytes::from(serde_json::to_vec(&action).unwrap()),
        keepalive_write_attempts: AtomicUsize::new(0),
    });

    fn new_op_id() -> OperationId {
        OperationId::from("unused-new-operation")
    }
    let now_fn: fn() -> SystemTime = SystemTime::now;
    let op_id_fn: fn() -> OperationId = new_op_id;
    let db: StoreAwaitedActionDb<
        KeepaliveWriteFailsStore,
        fn() -> OperationId,
        SystemTime,
        fn() -> SystemTime,
    > = StoreAwaitedActionDb::new(
        store.clone(),
        Arc::new(Notify::new()),
        now_fn,
        op_id_fn,
        60,
        60,
        false,
    )
    .await
    .expect("construct test db");

    let mut subscriber = db
        .get_awaited_action_by_id(&operation_id)
        .await?
        .expect("subscriber for live client");

    // Drive the client's changed() loop for 30 virtual seconds — three full
    // CLIENT_KEEPALIVE_DURATION periods. The subscription never fires
    // (pending) and the action never changes, so the only thing the loop can
    // do is maintain the keepalive.
    let changed_fut = subscriber.changed();
    tokio::pin!(changed_fut);
    tokio::select! {
        result = &mut changed_fut => {
            panic!("changed() must not resolve: nothing changed; got {result:?}");
        }
        () = tokio::time::sleep(Duration::from_secs(30)) => {}
    }

    let n = store.keepalive_write_attempts.load(Ordering::SeqCst);
    assert!(
        n >= 2,
        "keepalive write failed but was never retried within 3x \
         CLIENT_KEEPALIVE_DURATION (attempts = {n}); the local \
         last_known_keepalive_ts was advanced despite the store Err, so the \
         shared-store keepalive stays stale and a peer replica will retire \
         this live client's action as abandoned (DeadlineExceeded)"
    );
    Ok(())
}
