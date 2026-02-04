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

use core::mem::Discriminant;
use core::ops::Bound;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::borrow::Cow;
use std::sync::{Arc, Weak};
use std::time::UNIX_EPOCH;

use bytes::Bytes;
use futures::{Stream, TryStreamExt};
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionUniqueQualifier, OperationId,
};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::spawn;
use nativelink_util::store_trait::{
    FalseValue, SchedulerCurrentVersionProvider, SchedulerIndexProvider, SchedulerStore,
    SchedulerStoreDataProvider, SchedulerStoreDecodeTo, SchedulerStoreKeyProvider,
    SchedulerSubscription, SchedulerSubscriptionManager, StoreKey, TrueValue,
};
use nativelink_util::task::JoinHandleDropGuard;
use tokio::sync::Notify;
use tracing::{error, warn};

use crate::awaited_action_db::{
    AwaitedAction, AwaitedActionDb, AwaitedActionSubscriber, CLIENT_KEEPALIVE_DURATION,
    SortedAwaitedAction, SortedAwaitedActionState,
};

type ClientOperationId = OperationId;

/// Maximum number of retries to update client keep alive.
const MAX_RETRIES_FOR_CLIENT_KEEPALIVE: u32 = 8;

/// Use separate non-versioned Redis key for client keepalives.
const USE_SEPARATE_CLIENT_KEEPALIVE_KEY: bool = true;

enum OperationSubscriberState<Sub> {
    Unsubscribed,
    Subscribed(Sub),
}

pub struct OperationSubscriber<S: SchedulerStore, I: InstantWrapper, NowFn: Fn() -> I> {
    maybe_client_operation_id: Option<ClientOperationId>,
    subscription_key: OperationIdToAwaitedAction<'static>,
    weak_store: Weak<S>,
    state: OperationSubscriberState<
        <S::SubscriptionManager as SchedulerSubscriptionManager>::Subscription,
    >,
    last_known_keepalive_ts: AtomicU64,
    now_fn: NowFn,
    // If the SchedulerSubscriptionManager is not reliable, then this is populated
    // when the state is set to subscribed.  When set it causes the state to be polled
    // as well as listening for the publishing.
    maybe_last_stage: Option<Discriminant<ActionStage>>,
}

impl<S: SchedulerStore, I: InstantWrapper, NowFn: Fn() -> I + core::fmt::Debug> core::fmt::Debug
    for OperationSubscriber<S, I, NowFn>
where
    OperationSubscriberState<
        <S::SubscriptionManager as SchedulerSubscriptionManager>::Subscription,
    >: core::fmt::Debug,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("OperationSubscriber")
            .field("maybe_client_operation_id", &self.maybe_client_operation_id)
            .field("subscription_key", &self.subscription_key)
            .field("weak_store", &self.weak_store)
            .field("state", &self.state)
            .field("last_known_keepalive_ts", &self.last_known_keepalive_ts)
            .field("now_fn", &self.now_fn)
            .finish()
    }
}
impl<S, I, NowFn> OperationSubscriber<S, I, NowFn>
where
    S: SchedulerStore,
    I: InstantWrapper,
    NowFn: Fn() -> I,
{
    const fn new(
        maybe_client_operation_id: Option<ClientOperationId>,
        subscription_key: OperationIdToAwaitedAction<'static>,
        weak_store: Weak<S>,
        now_fn: NowFn,
    ) -> Self {
        Self {
            maybe_client_operation_id,
            subscription_key,
            weak_store,
            last_known_keepalive_ts: AtomicU64::new(0),
            state: OperationSubscriberState::Unsubscribed,
            now_fn,
            maybe_last_stage: None,
        }
    }

    async fn inner_get_awaited_action(
        store: &S,
        key: OperationIdToAwaitedAction<'_>,
        maybe_client_operation_id: Option<ClientOperationId>,
        last_known_keepalive_ts: &AtomicU64,
    ) -> Result<AwaitedAction, Error> {
        let mut awaited_action = store
            .get_and_decode(key.borrow())
            .await
            .err_tip(|| format!("In OperationSubscriber::get_awaited_action {key:?}"))?
            .ok_or_else(|| {
                make_err!(
                    Code::NotFound,
                    "Could not find AwaitedAction for the given operation id {key:?}",
                )
            })?;
        if let Some(client_operation_id) = maybe_client_operation_id {
            awaited_action.set_client_operation_id(client_operation_id);
        }

        // Helper to convert SystemTime to unix timestamp
        let to_unix_ts = |t: std::time::SystemTime| -> u64 {
            t.duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        };

        // Check the separate keepalive key for the most recent timestamp.
        let keepalive_ts = if USE_SEPARATE_CLIENT_KEEPALIVE_KEY {
            let operation_id = key.0.as_ref();
            match store.get_and_decode(ClientKeepaliveKey(operation_id)).await {
                Ok(Some(ts)) => {
                    let awaited_ts = to_unix_ts(awaited_action.last_client_keepalive_timestamp());
                    if ts > awaited_ts {
                        let timestamp = UNIX_EPOCH + Duration::from_secs(ts);
                        awaited_action.update_client_keep_alive(timestamp);
                        ts
                    } else {
                        awaited_ts
                    }
                }
                Ok(None) | Err(_) => to_unix_ts(awaited_action.last_client_keepalive_timestamp()),
            }
        } else {
            to_unix_ts(awaited_action.last_client_keepalive_timestamp())
        };

        last_known_keepalive_ts.store(keepalive_ts, Ordering::Release);
        Ok(awaited_action)
    }

    #[expect(clippy::future_not_send)] // TODO(jhpratt) remove this
    async fn get_awaited_action(&self) -> Result<AwaitedAction, Error> {
        let store = self
            .weak_store
            .upgrade()
            .err_tip(|| "Store gone in OperationSubscriber::get_awaited_action")?;
        Self::inner_get_awaited_action(
            store.as_ref(),
            self.subscription_key.borrow(),
            self.maybe_client_operation_id.clone(),
            &self.last_known_keepalive_ts,
        )
        .await
    }
}

impl<S, I, NowFn> AwaitedActionSubscriber for OperationSubscriber<S, I, NowFn>
where
    S: SchedulerStore,
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + 'static,
{
    async fn changed(&mut self) -> Result<AwaitedAction, Error> {
        let store = self
            .weak_store
            .upgrade()
            .err_tip(|| "Store gone in OperationSubscriber::get_awaited_action")?;
        let subscription = match &mut self.state {
            OperationSubscriberState::Subscribed(subscription) => subscription,
            OperationSubscriberState::Unsubscribed => {
                let subscription = store
                    .subscription_manager()
                    .err_tip(|| "In OperationSubscriber::changed::subscription_manager")?
                    .subscribe(self.subscription_key.borrow())
                    .err_tip(|| "In OperationSubscriber::changed::subscribe")?;
                self.state = OperationSubscriberState::Subscribed(subscription);
                // When we've just subscribed, there may have been changes before now.
                let action = Self::inner_get_awaited_action(
                    store.as_ref(),
                    self.subscription_key.borrow(),
                    self.maybe_client_operation_id.clone(),
                    &self.last_known_keepalive_ts,
                )
                .await
                .err_tip(|| "In OperationSubscriber::changed")?;
                if !<S as SchedulerStore>::SubscriptionManager::is_reliable() {
                    self.maybe_last_stage = Some(core::mem::discriminant(&action.state().stage));
                }
                // Existing changes are only interesting if the state is past queued.
                if !matches!(action.state().stage, ActionStage::Queued) {
                    return Ok(action);
                }
                let OperationSubscriberState::Subscribed(subscription) = &mut self.state else {
                    unreachable!("Subscription should be in Subscribed state");
                };
                subscription
            }
        };

        let changed_fut = subscription.changed();
        tokio::pin!(changed_fut);
        loop {
            // This is set if the maybe_last_state doesn't match the state in the store.
            let mut maybe_changed_action = None;

            let last_known_keepalive_ts = self.last_known_keepalive_ts.load(Ordering::Acquire);
            if I::from_secs(last_known_keepalive_ts).elapsed() > CLIENT_KEEPALIVE_DURATION {
                let now = (self.now_fn)().now();
                let now_ts = now
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);

                if USE_SEPARATE_CLIENT_KEEPALIVE_KEY {
                    let operation_id = self.subscription_key.0.as_ref();
                    let update_result = store
                        .update_data(UpdateClientKeepalive {
                            operation_id,
                            timestamp: now_ts,
                        })
                        .await;

                    if let Err(e) = update_result {
                        warn!(
                            ?self.subscription_key,
                            ?e,
                            "Failed to update client keepalive (non-versioned)"
                        );
                    }

                    // Update local timestamp
                    self.last_known_keepalive_ts
                        .store(now_ts, Ordering::Release);

                    // Check if state changed (for unreliable subscription managers)
                    if self.maybe_last_stage.is_some() {
                        let awaited_action = Self::inner_get_awaited_action(
                            store.as_ref(),
                            self.subscription_key.borrow(),
                            self.maybe_client_operation_id.clone(),
                            &self.last_known_keepalive_ts,
                        )
                        .await
                        .err_tip(|| "In OperationSubscriber::changed")?;

                        if self.maybe_last_stage.as_ref().is_some_and(|last_stage| {
                            *last_stage != core::mem::discriminant(&awaited_action.state().stage)
                        }) {
                            maybe_changed_action = Some(awaited_action);
                        }
                    }
                } else {
                    for attempt in 1..=MAX_RETRIES_FOR_CLIENT_KEEPALIVE {
                        if attempt > 1 {
                            (self.now_fn)().sleep(Duration::from_millis(100)).await;
                            warn!(
                                ?self.subscription_key,
                                attempt,
                                "Client keepalive retry due to version conflict"
                            );
                        }
                        let mut awaited_action = Self::inner_get_awaited_action(
                            store.as_ref(),
                            self.subscription_key.borrow(),
                            self.maybe_client_operation_id.clone(),
                            &self.last_known_keepalive_ts,
                        )
                        .await
                        .err_tip(|| "In OperationSubscriber::changed")?;
                        awaited_action.update_client_keep_alive(now);
                        maybe_changed_action = self
                            .maybe_last_stage
                            .as_ref()
                            .is_some_and(|last_stage| {
                                *last_stage
                                    != core::mem::discriminant(&awaited_action.state().stage)
                            })
                            .then(|| awaited_action.clone());
                        match inner_update_awaited_action(store.as_ref(), awaited_action).await {
                            Ok(()) => break,
                            err if attempt == MAX_RETRIES_FOR_CLIENT_KEEPALIVE => {
                                err.err_tip_with_code(|_| {
                                    (Code::Aborted, "Could not update client keep alive")
                                })?;
                            }
                            _ => (),
                        }
                    }
                }
            }

            // If the polling shows that it's changed state then publish now.
            if let Some(changed_action) = maybe_changed_action {
                self.maybe_last_stage =
                    Some(core::mem::discriminant(&changed_action.state().stage));
                return Ok(changed_action);
            }
            // Determine the sleep time based on the last client keep alive.
            let sleep_time = CLIENT_KEEPALIVE_DURATION
                .checked_sub(
                    I::from_secs(self.last_known_keepalive_ts.load(Ordering::Acquire)).elapsed(),
                )
                .unwrap_or(Duration::from_millis(100));
            tokio::select! {
                result = &mut changed_fut => {
                    result?;
                    break;
                }
                () = (self.now_fn)().sleep(sleep_time) => {
                    // If we haven't received any updates for a while, we should
                    // let the database know that we are still listening to prevent
                    // the action from being dropped.  Also poll for updates if the
                    // subscription manager is unreliable.
                }
            }
        }

        let awaited_action = Self::inner_get_awaited_action(
            store.as_ref(),
            self.subscription_key.borrow(),
            self.maybe_client_operation_id.clone(),
            &self.last_known_keepalive_ts,
        )
        .await
        .err_tip(|| "In OperationSubscriber::changed")?;
        if self.maybe_last_stage.is_some() {
            self.maybe_last_stage = Some(core::mem::discriminant(&awaited_action.state().stage));
        }
        Ok(awaited_action)
    }

    async fn borrow(&self) -> Result<AwaitedAction, Error> {
        self.get_awaited_action()
            .await
            .err_tip(|| "In OperationSubscriber::borrow")
    }
}

fn awaited_action_decode(version: i64, data: &Bytes) -> Result<AwaitedAction, Error> {
    let mut awaited_action: AwaitedAction = serde_json::from_slice(data)
        .map_err(|e| make_input_err!("In AwaitedAction::decode - {e:?}"))?;
    awaited_action.set_version(version);
    Ok(awaited_action)
}

const OPERATION_ID_TO_AWAITED_ACTION_KEY_PREFIX: &str = "aa_";
const CLIENT_ID_TO_OPERATION_ID_KEY_PREFIX: &str = "cid_";
/// Phase 2: Separate key prefix for client keepalives (non-versioned).
const CLIENT_KEEPALIVE_KEY_PREFIX: &str = "ck_";

#[derive(Debug)]
struct OperationIdToAwaitedAction<'a>(Cow<'a, OperationId>);
impl OperationIdToAwaitedAction<'_> {
    fn borrow(&self) -> OperationIdToAwaitedAction<'_> {
        OperationIdToAwaitedAction(Cow::Borrowed(self.0.as_ref()))
    }
}
impl SchedulerStoreKeyProvider for OperationIdToAwaitedAction<'_> {
    type Versioned = TrueValue;
    fn get_key(&self) -> StoreKey<'static> {
        StoreKey::Str(Cow::Owned(format!(
            "{OPERATION_ID_TO_AWAITED_ACTION_KEY_PREFIX}{}",
            self.0
        )))
    }
}
impl SchedulerStoreDecodeTo for OperationIdToAwaitedAction<'_> {
    type DecodeOutput = AwaitedAction;
    fn decode(version: i64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
        awaited_action_decode(version, &data)
    }
}

struct ClientIdToOperationId<'a>(&'a OperationId);
impl SchedulerStoreKeyProvider for ClientIdToOperationId<'_> {
    type Versioned = FalseValue;
    fn get_key(&self) -> StoreKey<'static> {
        StoreKey::Str(Cow::Owned(format!(
            "{CLIENT_ID_TO_OPERATION_ID_KEY_PREFIX}{}",
            self.0
        )))
    }
}
impl SchedulerStoreDecodeTo for ClientIdToOperationId<'_> {
    type DecodeOutput = OperationId;
    fn decode(_version: i64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
        serde_json::from_slice(&data).map_err(|e| {
            make_input_err!(
                "In ClientIdToOperationId::decode - {e:?} (data: {:02x?})",
                data
            )
        })
    }
}

struct ClientKeepaliveKey<'a>(&'a OperationId);
impl SchedulerStoreKeyProvider for ClientKeepaliveKey<'_> {
    type Versioned = FalseValue;
    fn get_key(&self) -> StoreKey<'static> {
        StoreKey::Str(Cow::Owned(format!(
            "{CLIENT_KEEPALIVE_KEY_PREFIX}{}",
            self.0
        )))
    }
}
impl SchedulerStoreDecodeTo for ClientKeepaliveKey<'_> {
    type DecodeOutput = u64;
    fn decode(_version: i64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
        let s = core::str::from_utf8(&data)
            .map_err(|e| make_input_err!("In ClientKeepaliveKey::decode utf8 - {e:?}"))?;
        s.parse::<u64>()
            .map_err(|e| make_input_err!("In ClientKeepaliveKey::decode parse - {e:?}"))
    }
}

struct UpdateClientKeepalive<'a> {
    operation_id: &'a OperationId,
    timestamp: u64,
}
impl SchedulerStoreKeyProvider for UpdateClientKeepalive<'_> {
    type Versioned = FalseValue;
    fn get_key(&self) -> StoreKey<'static> {
        ClientKeepaliveKey(self.operation_id).get_key()
    }
}
impl SchedulerStoreDataProvider for UpdateClientKeepalive<'_> {
    fn try_into_bytes(self) -> Result<Bytes, Error> {
        Ok(Bytes::from(self.timestamp.to_string()))
    }
}

// TODO(palfrey) We only need operation_id here, it would be nice if we had a way
// to tell the decoder we only care about specific fields.
struct SearchUniqueQualifierToAwaitedAction<'a>(&'a ActionUniqueQualifier);
impl SchedulerIndexProvider for SearchUniqueQualifierToAwaitedAction<'_> {
    const KEY_PREFIX: &'static str = OPERATION_ID_TO_AWAITED_ACTION_KEY_PREFIX;
    const INDEX_NAME: &'static str = "unique_qualifier";
    type Versioned = TrueValue;
    fn index_value(&self) -> Cow<'_, str> {
        Cow::Owned(format!("{}", self.0))
    }
}
impl SchedulerStoreDecodeTo for SearchUniqueQualifierToAwaitedAction<'_> {
    type DecodeOutput = AwaitedAction;
    fn decode(version: i64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
        awaited_action_decode(version, &data)
    }
}

struct SearchStateToAwaitedAction(&'static str);
impl SchedulerIndexProvider for SearchStateToAwaitedAction {
    const KEY_PREFIX: &'static str = OPERATION_ID_TO_AWAITED_ACTION_KEY_PREFIX;
    const INDEX_NAME: &'static str = "state";
    const MAYBE_SORT_KEY: Option<&'static str> = Some("sort_key");
    type Versioned = TrueValue;
    fn index_value(&self) -> Cow<'_, str> {
        Cow::Borrowed(self.0)
    }
}
impl SchedulerStoreDecodeTo for SearchStateToAwaitedAction {
    type DecodeOutput = AwaitedAction;
    fn decode(version: i64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
        awaited_action_decode(version, &data)
    }
}

const fn get_state_prefix(state: SortedAwaitedActionState) -> &'static str {
    match state {
        SortedAwaitedActionState::CacheCheck => "cache_check",
        SortedAwaitedActionState::Queued => "queued",
        SortedAwaitedActionState::Executing => "executing",
        SortedAwaitedActionState::Completed => "completed",
    }
}

struct UpdateOperationIdToAwaitedAction(AwaitedAction);
impl SchedulerCurrentVersionProvider for UpdateOperationIdToAwaitedAction {
    fn current_version(&self) -> i64 {
        self.0.version()
    }
}
impl SchedulerStoreKeyProvider for UpdateOperationIdToAwaitedAction {
    type Versioned = TrueValue;
    fn get_key(&self) -> StoreKey<'static> {
        OperationIdToAwaitedAction(Cow::Borrowed(self.0.operation_id())).get_key()
    }
}
impl SchedulerStoreDataProvider for UpdateOperationIdToAwaitedAction {
    fn try_into_bytes(self) -> Result<Bytes, Error> {
        serde_json::to_string(&self.0)
            .map(Bytes::from)
            .map_err(|e| make_input_err!("Could not convert AwaitedAction to json - {e:?}"))
    }
    fn get_indexes(&self) -> Result<Vec<(&'static str, Bytes)>, Error> {
        let unique_qualifier = &self.0.action_info().unique_qualifier;
        let maybe_unique_qualifier = match &unique_qualifier {
            ActionUniqueQualifier::Cacheable(_) => Some(unique_qualifier),
            ActionUniqueQualifier::Uncacheable(_) => None,
        };
        let mut output = Vec::with_capacity(2 + maybe_unique_qualifier.map_or(0, |_| 1));
        if maybe_unique_qualifier.is_some() {
            output.push((
                "unique_qualifier",
                Bytes::from(unique_qualifier.to_string()),
            ));
        }
        {
            let state = SortedAwaitedActionState::try_from(&self.0.state().stage)
                .err_tip(|| "In UpdateOperationIdToAwaitedAction::get_index")?;
            output.push(("state", Bytes::from(get_state_prefix(state))));
            let sorted_awaited_action = SortedAwaitedAction::from(&self.0);
            output.push((
                "sort_key",
                // We encode to hex to ensure that the sort key is lexicographically sorted.
                Bytes::from(format!("{:016x}", sorted_awaited_action.sort_key.as_u64())),
            ));
        }
        Ok(output)
    }
}

struct UpdateClientIdToOperationId {
    client_operation_id: ClientOperationId,
    operation_id: OperationId,
}
impl SchedulerStoreKeyProvider for UpdateClientIdToOperationId {
    type Versioned = FalseValue;
    fn get_key(&self) -> StoreKey<'static> {
        ClientIdToOperationId(&self.client_operation_id).get_key()
    }
}
impl SchedulerStoreDataProvider for UpdateClientIdToOperationId {
    fn try_into_bytes(self) -> Result<Bytes, Error> {
        serde_json::to_string(&self.operation_id)
            .map(Bytes::from)
            .map_err(|e| make_input_err!("Could not convert OperationId to json - {e:?}"))
    }
}

async fn inner_update_awaited_action(
    store: &impl SchedulerStore,
    mut new_awaited_action: AwaitedAction,
) -> Result<(), Error> {
    let operation_id = new_awaited_action.operation_id().clone();
    if new_awaited_action.state().client_operation_id != operation_id {
        new_awaited_action.set_client_operation_id(operation_id.clone());
    }

    let _is_finished = new_awaited_action.state().stage.is_finished();

    let maybe_version = store
        .update_data(UpdateOperationIdToAwaitedAction(new_awaited_action))
        .await
        .err_tip(|| "In RedisAwaitedActionDb::update_awaited_action")?;

    if maybe_version.is_none() {
        warn!(
            %operation_id,
            "Could not update AwaitedAction because the version did not match"
        );
        return Err(make_err!(
            Code::Aborted,
            "Could not update AwaitedAction because the version did not match for {operation_id}",
        ));
    }

    Ok(())
}

#[derive(Debug, MetricsComponent)]
pub struct StoreAwaitedActionDb<S, F, I, NowFn>
where
    S: SchedulerStore,
    F: Fn() -> OperationId,
    I: InstantWrapper,
    NowFn: Fn() -> I,
{
    store: Arc<S>,
    now_fn: NowFn,
    operation_id_creator: F,
    _pull_task_change_subscriber_spawn: JoinHandleDropGuard<()>,
}

impl<S, F, I, NowFn> StoreAwaitedActionDb<S, F, I, NowFn>
where
    S: SchedulerStore,
    F: Fn() -> OperationId,
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Clone + 'static,
{
    pub fn new(
        store: Arc<S>,
        task_change_publisher: Arc<Notify>,
        now_fn: NowFn,
        operation_id_creator: F,
    ) -> Result<Self, Error> {
        let mut subscription = store
            .subscription_manager()
            .err_tip(|| "In RedisAwaitedActionDb::new")?
            .subscribe(OperationIdToAwaitedAction(Cow::Owned(OperationId::String(
                String::new(),
            ))))
            .err_tip(|| "In RedisAwaitedActionDb::new")?;
        let pull_task_change_subscriber = spawn!(
            "redis_awaited_action_db_pull_task_change_subscriber",
            async move {
                loop {
                    let changed_res = subscription
                        .changed()
                        .await
                        .err_tip(|| "In RedisAwaitedActionDb::new");
                    if let Err(err) = changed_res {
                        error!(
                            "Error waiting for pull task change subscriber in RedisAwaitedActionDb::new  - {err:?}"
                        );
                        // Sleep for a second to avoid a busy loop, then trigger the notify
                        // so if a reconnect happens we let local resources know that things
                        // might have changed.
                        tokio::time::sleep(Duration::from_secs(1)).await;
                    }
                    task_change_publisher.as_ref().notify_one();
                }
            }
        );
        Ok(Self {
            store,
            now_fn,
            operation_id_creator,
            _pull_task_change_subscriber_spawn: pull_task_change_subscriber,
        })
    }

    #[expect(clippy::future_not_send)] // TODO(jhpratt) remove this
    async fn try_subscribe(
        &self,
        client_operation_id: &ClientOperationId,
        unique_qualifier: &ActionUniqueQualifier,
        no_event_action_timeout: Duration,
        // TODO(palfrey) To simplify the scheduler 2024 refactor, we
        // removed the ability to upgrade priorities of actions.
        // we should add priority upgrades back in.
        _priority: i32,
    ) -> Result<Option<AwaitedAction>, Error> {
        match unique_qualifier {
            ActionUniqueQualifier::Cacheable(_) => {}
            ActionUniqueQualifier::Uncacheable(_) => return Ok(None),
        }
        let stream = self
            .store
            .search_by_index_prefix(SearchUniqueQualifierToAwaitedAction(unique_qualifier))
            .await
            .err_tip(|| "In RedisAwaitedActionDb::try_subscribe")?;
        tokio::pin!(stream);
        let maybe_awaited_action = stream
            .try_next()
            .await
            .err_tip(|| "In RedisAwaitedActionDb::try_subscribe")?;
        match maybe_awaited_action {
            Some(awaited_action) => {
                // TODO(palfrey) We don't support joining completed jobs because we
                // need to also check that all the data is still in the cache.
                // If the existing job failed then we need to set back to queued or we get
                // a version mismatch.  Equally we need to check the timeout as the job
                // may be abandoned in the store.
                let worker_should_update_before = (awaited_action.state().stage
                    == ActionStage::Executing)
                    .then_some(())
                    .map(|()| awaited_action.last_worker_updated_timestamp())
                    .and_then(|last_worker_updated| {
                        last_worker_updated.checked_add(no_event_action_timeout)
                    });
                let awaited_action = if awaited_action.state().stage.is_finished()
                    || worker_should_update_before
                        .is_some_and(|timestamp| timestamp < (self.now_fn)().now())
                {
                    tracing::debug!(
                        "Recreating action {:?} for operation {client_operation_id}",
                        awaited_action.action_info().digest()
                    );
                    // The version is reset because we have a new operation ID.
                    AwaitedAction::new(
                        (self.operation_id_creator)(),
                        awaited_action.action_info().clone(),
                        (self.now_fn)().now(),
                    )
                } else {
                    tracing::debug!(
                        "Subscribing to existing action {:?} for operation {client_operation_id}",
                        awaited_action.action_info().digest()
                    );
                    awaited_action
                };
                Ok(Some(awaited_action))
            }
            None => Ok(None),
        }
    }

    #[expect(clippy::future_not_send)] // TODO(jhpratt) remove this
    async fn inner_get_awaited_action_by_id(
        &self,
        client_operation_id: &ClientOperationId,
    ) -> Result<Option<OperationSubscriber<S, I, NowFn>>, Error> {
        let maybe_operation_id = self
            .store
            .get_and_decode(ClientIdToOperationId(client_operation_id))
            .await
            .err_tip(|| "In RedisAwaitedActionDb::get_awaited_action_by_id")?;
        let Some(operation_id) = maybe_operation_id else {
            return Ok(None);
        };

        // Validate that the internal operation actually exists.
        // If it doesn't, this is an orphaned client operation mapping that should be cleaned up.
        // This can happen when an operation is deleted (completed/timed out) but the
        // client_id -> operation_id mapping persists in the store.
        let maybe_awaited_action = match self
            .store
            .get_and_decode(OperationIdToAwaitedAction(Cow::Borrowed(&operation_id)))
            .await
        {
            Ok(maybe_action) => maybe_action,
            Err(err) if err.code == Code::NotFound => {
                tracing::warn!(
                    "Orphaned client operation mapping detected: client_id={} maps to operation_id={}, \
                    but the operation does not exist in the store (NotFound). This typically happens when \
                    an operation completes or times out but the client mapping persists.",
                    client_operation_id,
                    operation_id
                );
                None
            }
            Err(err) => {
                // Some other error occurred
                return Err(err).err_tip(
                    || "In RedisAwaitedActionDb::get_awaited_action_by_id::validate_operation",
                );
            }
        };

        if maybe_awaited_action.is_none() {
            tracing::warn!(
                "Found orphaned client operation mapping: client_id={} -> operation_id={}, \
                but operation no longer exists. Returning None to prevent client from polling \
                a non-existent operation.",
                client_operation_id,
                operation_id
            );
            return Ok(None);
        }

        Ok(Some(OperationSubscriber::new(
            Some(client_operation_id.clone()),
            OperationIdToAwaitedAction(Cow::Owned(operation_id)),
            Arc::downgrade(&self.store),
            self.now_fn.clone(),
        )))
    }
}

impl<S, F, I, NowFn> AwaitedActionDb for StoreAwaitedActionDb<S, F, I, NowFn>
where
    S: SchedulerStore,
    F: Fn() -> OperationId + Send + Sync + Unpin + 'static,
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + Clone + 'static,
{
    type Subscriber = OperationSubscriber<S, I, NowFn>;

    async fn get_awaited_action_by_id(
        &self,
        client_operation_id: &ClientOperationId,
    ) -> Result<Option<Self::Subscriber>, Error> {
        self.inner_get_awaited_action_by_id(client_operation_id)
            .await
    }

    async fn get_by_operation_id(
        &self,
        operation_id: &OperationId,
    ) -> Result<Option<Self::Subscriber>, Error> {
        Ok(Some(OperationSubscriber::new(
            None,
            OperationIdToAwaitedAction(Cow::Owned(operation_id.clone())),
            Arc::downgrade(&self.store),
            self.now_fn.clone(),
        )))
    }

    async fn update_awaited_action(&self, new_awaited_action: AwaitedAction) -> Result<(), Error> {
        inner_update_awaited_action(self.store.as_ref(), new_awaited_action).await
    }

    async fn add_action(
        &self,
        client_operation_id: ClientOperationId,
        action_info: Arc<ActionInfo>,
        no_event_action_timeout: Duration,
    ) -> Result<Self::Subscriber, Error> {
        loop {
            // Check to see if the action is already known and subscribe if it is.
            let mut awaited_action = self
                .try_subscribe(
                    &client_operation_id,
                    &action_info.unique_qualifier,
                    no_event_action_timeout,
                    action_info.priority,
                )
                .await
                .err_tip(|| "In RedisAwaitedActionDb::add_action")?
                .unwrap_or_else(|| {
                    tracing::debug!(
                        "Creating new action {:?} for operation {client_operation_id}",
                        action_info.digest()
                    );
                    AwaitedAction::new(
                        (self.operation_id_creator)(),
                        action_info.clone(),
                        (self.now_fn)().now(),
                    )
                });

            debug_assert!(
                ActionStage::Queued == awaited_action.state().stage,
                "Expected action to be queued"
            );

            let operation_id = awaited_action.operation_id().clone();
            if awaited_action.state().client_operation_id != operation_id {
                // Just in case the client_operation_id was set to something else
                // we put it back to the underlying operation_id.
                awaited_action.set_client_operation_id(operation_id.clone());
            }
            awaited_action.update_client_keep_alive((self.now_fn)().now());

            let version = awaited_action.version();
            if self
                .store
                .update_data(UpdateOperationIdToAwaitedAction(awaited_action))
                .await
                .err_tip(|| "In RedisAwaitedActionDb::add_action")?
                .is_none()
            {
                // The version was out of date, try again.
                tracing::info!(
                    "Version out of date for {:?} {operation_id} {version}, retrying.",
                    action_info.digest()
                );
                continue;
            }

            // Add the client_operation_id to operation_id mapping
            self.store
                .update_data(UpdateClientIdToOperationId {
                    client_operation_id: client_operation_id.clone(),
                    operation_id: operation_id.clone(),
                })
                .await
                .err_tip(|| "In RedisAwaitedActionDb::add_action while adding client mapping")?;

            return Ok(OperationSubscriber::new(
                Some(client_operation_id),
                OperationIdToAwaitedAction(Cow::Owned(operation_id)),
                Arc::downgrade(&self.store),
                self.now_fn.clone(),
            ));
        }
    }

    async fn get_range_of_actions(
        &self,
        state: SortedAwaitedActionState,
        start: Bound<SortedAwaitedAction>,
        end: Bound<SortedAwaitedAction>,
        desc: bool,
    ) -> Result<impl Stream<Item = Result<Self::Subscriber, Error>> + Send, Error> {
        if !matches!(start, Bound::Unbounded) {
            return Err(make_err!(
                Code::Unimplemented,
                "Start bound is not supported in RedisAwaitedActionDb::get_range_of_actions",
            ));
        }
        if !matches!(end, Bound::Unbounded) {
            return Err(make_err!(
                Code::Unimplemented,
                "Start bound is not supported in RedisAwaitedActionDb::get_range_of_actions",
            ));
        }
        // TODO(palfrey) This API is not difficult to implement, but there is no code path
        // that uses it, so no reason to implement it yet.
        if !desc {
            return Err(make_err!(
                Code::Unimplemented,
                "Descending order is not supported in RedisAwaitedActionDb::get_range_of_actions",
            ));
        }
        Ok(self
            .store
            .search_by_index_prefix(SearchStateToAwaitedAction(get_state_prefix(state)))
            .await
            .err_tip(|| "In RedisAwaitedActionDb::get_range_of_actions")?
            .map_ok(move |awaited_action| {
                OperationSubscriber::new(
                    None,
                    OperationIdToAwaitedAction(Cow::Owned(awaited_action.operation_id().clone())),
                    Arc::downgrade(&self.store),
                    self.now_fn.clone(),
                )
            }))
    }

    async fn get_all_awaited_actions(
        &self,
    ) -> Result<impl Stream<Item = Result<Self::Subscriber, Error>>, Error> {
        Ok(self
            .store
            .search_by_index_prefix(SearchStateToAwaitedAction(""))
            .await
            .err_tip(|| "In RedisAwaitedActionDb::get_range_of_actions")?
            .map_ok(move |awaited_action| {
                OperationSubscriber::new(
                    None,
                    OperationIdToAwaitedAction(Cow::Owned(awaited_action.operation_id().clone())),
                    Arc::downgrade(&self.store),
                    self.now_fn.clone(),
                )
            }))
    }
}
