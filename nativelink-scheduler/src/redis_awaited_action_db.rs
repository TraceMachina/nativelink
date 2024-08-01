// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::borrow::Borrow;
use std::ops::RangeBounds;
use std::{ops::Bound, str::from_utf8};
use std::collections::HashMap;
use std::task::Poll;
use nativelink_metric::MetricsComponent;
use nativelink_store::scheduler_store::SchedulerStore;

use std::sync::Arc;
use nativelink_util::chunked_stream::ChunkedStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;

use async_lock::Mutex;
use fred::interfaces::{ClientLike, EventInterface,  PubsubInterface };
use nativelink_error::{Code, make_err, make_input_err, Error, ResultExt};
use nativelink_store::redis_store::RedisStore;
use nativelink_util::action_messages::{
    ActionInfo, ActionUniqueQualifier, ClientOperationId, OperationId
};

use futures::{ poll, stream, Stream, StreamExt};
use nativelink_util::task::JoinHandleDropGuard;
use nativelink_util::spawn;
use nativelink_util::store_trait::{StoreLike, StoreKey};
use fred::types::{Message};
use tokio::sync::watch;
use tracing::{event, Level};

use crate::awaited_action_db::{
    AwaitedAction, AwaitedActionDb, AwaitedActionSortKey, AwaitedActionSubscriber, SortedAwaitedAction, SortedAwaitedActionState, SortedAwaitedActions
};

/// Duration to wait before sending client keep alive messages.
/// Subscriber that can be used to monitor when AwaitedActions change.
pub struct RedisOperationSubscriber {
    /// The receiver to listen for changes.
    awaited_action_rx: watch::Receiver<AwaitedAction>,
    /// The client operation id and keep alive information.
    operation_id: OperationId,
}

impl RedisOperationSubscriber {}

impl AwaitedActionSubscriber for RedisOperationSubscriber {
    async fn changed(&mut self) -> Result<AwaitedAction, Error> {
        let _ = self.awaited_action_rx.changed().await;
        Ok(self.awaited_action_rx.borrow().clone())
    }

    fn borrow(&self) -> AwaitedAction {
        self.awaited_action_rx.borrow().clone()
    }
}

struct RedisAwaitedActionDbImpl {
    tx_map: HashMap<OperationId, watch::Sender<AwaitedAction>>,
}

impl RedisAwaitedActionDbImpl {
    pub fn new() -> Self {
        Self { tx_map: HashMap::new() }
    }

    pub async fn get_operation_sender(&mut self, operation_id: &OperationId) -> Option<watch::Sender<AwaitedAction>> {
        self.tx_map.get(operation_id).cloned()
    }

    pub async fn get_operations_list(&mut self) -> Vec<OperationId> {
        self.tx_map.keys().into_iter().map(|key|{ key.clone().into() }).collect()
    }

    pub async fn set_operation_sender(&mut self, operation_id: &OperationId, tx: watch::Sender<AwaitedAction>) {
        self.tx_map.insert(operation_id.clone(), tx);
    }

}


#[derive(MetricsComponent)]
pub struct RedisAwaitedActionDb {
    store: Arc<RedisStore>,
    sorted_action_info_hash_keys: SortedAwaitedActions,
    inner: Arc<Mutex<RedisAwaitedActionDbImpl>>,
    _join_handle: JoinHandleDropGuard<()>,
}

impl RedisAwaitedActionDb {
    async fn get_range_of_actions<'a, 'b>(
        &'a self,
        state: SortedAwaitedActionState,
        range: impl RangeBounds<SortedAwaitedAction> + 'b,
    ) -> impl DoubleEndedIterator<
        Item = Result<
            (
                &'a SortedAwaitedAction,
                RedisOperationSubscriber,
            ),
            Error,
        >,
    > + 'a {
        let btree = match state {
            SortedAwaitedActionState::CacheCheck => &self.sorted_action_info_hash_keys.cache_check,
            SortedAwaitedActionState::Queued => &self.sorted_action_info_hash_keys.queued,
            SortedAwaitedActionState::Executing => &self.sorted_action_info_hash_keys.executing,
            SortedAwaitedActionState::Completed => &self.sorted_action_info_hash_keys.completed,
        };
        let mut results: Vec<Result<
            (
                &'a SortedAwaitedAction,
                RedisOperationSubscriber,
            ),
            Error,
        >> = Vec::new();
        for sorted_action in btree.range(range).into_iter() {
            self.get_by_operation_id(&sorted_action.operation_id).await.map(|maybe_sub| {
                let val = match maybe_sub {
                    Some(sub) => {
                        Ok((sorted_action, sub))
                    },
                    None => {
                        Err(make_err!(
                            Code::Internal,
                            "Failed to get operation id {}",
                            sorted_action.operation_id
                        ))
                    }
                };
                results.push(val);
            });
        }

        // .map(|sorted_awaited_action| async {
        //     let operation_id = &sorted_awaited_action.operation_id;
        //         .or_else(|_| {
        //         })
        //         .map(|subscriber| (sorted_awaited_action, subscriber))
        // })
        results.into_iter()
    }
    pub async fn new(
        store: Arc<RedisStore>,
    ) -> Result<Self, Error> {
        let inner = Arc::new(Mutex::new(RedisAwaitedActionDbImpl::new()));
        let weak_inner = Arc::downgrade(&inner);
        let store_clone = store.clone();

        let sub_key = StoreKey::Str(std::borrow::Cow::Borrowed("oid:*"));
        let sub_client = store.get_subscriber_client();
        sub_client.connect().await;
        sub_client.psubscribe("oid:*").await;
        let message_rx: tokio::sync::broadcast::Receiver<Message> = sub_client.message_rx();
        let join_handle = spawn!("redis_action_change_listener", async move {
            let mut stream = BroadcastStream::from(message_rx);
            // Unpack the Option<Result>> into just a Result
            fn handle_next(next: Option<Result<Message, BroadcastStreamRecvError>>) -> Result<Message, Error> {
                match next {
                    Some(Ok(v)) => { Ok(v) },
                    Some(Err(e)) => {
                        // The reciever encountered an Error. Would only occur if the subscription is invalid.
                        Err(make_err!(Code::Internal, "{}", e.to_string()))
                    },
                    None => {
                        // The stream has been closed, should not happen.
                        Err(make_err!(Code::Internal, "RedisAwaitedActionDb::subscription_listener subscription update stream was closed"))
                    }
                }
            }
            loop {
                let Ok(msg) = handle_next(stream.next().await) else {
                    event!(Level::ERROR, "RedisAwaitedActionDb::subscription_listener subscription update stream was closed");
                    return
                };
                let Some(bytes) = msg.value.as_bytes() else {
                    continue;
                };
                match weak_inner.upgrade() {
                    Some(inner_mutex) => {
                        let mut inner = inner_mutex.lock().await;
                        let state: AwaitedAction = AwaitedAction::try_from(bytes).unwrap();
                        let tx = inner.get_operation_sender(state.operation_id()).await.unwrap();
                        // Use send_replace so that we can send the update even when there are no recievers.
                        tx.send_replace(state);
                        while let Poll::Ready(maybe_result) = poll!(stream.next()) {
                            match handle_next(maybe_result) {
                                Ok(msg) => {
                                    let Some(bytes) = msg.value.as_bytes() else {
                                        continue;
                                    };
                                    let state: AwaitedAction = AwaitedAction::try_from(bytes).unwrap();
                                    let tx = inner.get_operation_sender(state.operation_id()).await.unwrap();
                                    // Use send_replace so that we can send the update even when there are no recievers.
                                    tx.send_replace(state);
                                },
                                Err(e) => {
                                    event!(Level::ERROR, ?e);
                                    return
                                }
                            }
                        }
                        drop(inner)
                    }
                    None => {
                        event!(Level::ERROR, "RedisAwaitedActionDb - Failed to upgrade inner");
                        return
                    }
                };

            }
        });
        Ok(Self {
            store,
            sorted_action_info_hash_keys: SortedAwaitedActions::default(),
            inner,
            _join_handle: join_handle,
        })
    }

    async fn subscribe_to_operation(
        &self,
        operation_id: &OperationId
    ) -> Result<Option<RedisOperationSubscriber>, Error> {
        let mut inner = self.inner.lock().await;
        let Some(tx) = inner.get_operation_sender(operation_id).await else {
            return Ok(None)
        };
        Ok(Some(RedisOperationSubscriber {
            awaited_action_rx: tx.subscribe(),
            operation_id: operation_id.clone(),
        }))
    }

    async fn get_operation_id_by_client_id(&self, client_id: &ClientOperationId) -> Result<Option<OperationId>, Error> {
        let result: Result<OperationId, Error> = {
            let key = format!("cid:{client_id}");
            let bytes = self.store.get_part_unchunked(key.as_str(), 0, None).await?;
            from_utf8(&bytes).map(|s| {
                OperationId::try_from(s)
            }).map_err(|e| {
                make_input_err!("Decoding bytes failed with error: {e}")
            })?
        };
        match result {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                match e.code {
                    Code::NotFound => Ok(None),
                    _ => Err(e)
                }
            }
        }
    }

    async fn get_operation_id_by_hash_key(&self, unique_qualifier: &ActionUniqueQualifier) -> Result<Option<OperationId>, Error> {
        let result: Result<OperationId, Error> = {
            let key = format!("ahk:{unique_qualifier}");
            let bytes = self.store.get_part_unchunked(key.as_str(), 0, None).await?;
            from_utf8(&bytes).map(|s| {
                OperationId::try_from(s)
            }).map_err(|e| {
                make_input_err!("Decoding bytes failed with error: {e}")
            })?
        };
        match result {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                match e.code {
                    Code::NotFound => Ok(None),
                    _ => Err(e)
                }
            }
        }
    }

    async fn get_awaited_action_by_operation_id(&self, operation_id: &OperationId) -> Result<Option<AwaitedAction>, Error> {
        let result: Result<AwaitedAction, Error> = {
            let key = format!("oid:{operation_id}");
            let bytes = self.store.get_part_unchunked(key.as_str(), 0, None).await?;
            AwaitedAction::try_from(bytes.to_vec().as_slice())
        };
        match result {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                match e.code {
                    Code::NotFound => Ok(None),
                    _ => Err(e)
                }
            }
        }
    }
}

impl AwaitedActionDb for RedisAwaitedActionDb {
    type Subscriber = RedisOperationSubscriber;
    /// Get the AwaitedAction by the client operation id.
    async fn get_awaited_action_by_id(
        &self,
        client_operation_id: &ClientOperationId,
    ) -> Result<Option<RedisOperationSubscriber>, Error> {
        match self.get_operation_id_by_client_id(client_operation_id).await {
            Ok(Some(operation_id)) => {
                self.subscribe_to_operation(&operation_id).await
            }
            Ok(None) => Ok(None),
            Err(e) => Err(e)
        }
    }

    /// Get the AwaitedAction by the operation id.
    async fn get_by_operation_id(
        &self,
        operation_id: &OperationId,
    ) -> Result<Option<RedisOperationSubscriber>, Error> {
        self.subscribe_to_operation(operation_id).await
    }

    /// Process a change changed AwaitedAction and notify any listeners.
    async fn update_awaited_action(
        &self,
        new_awaited_action: AwaitedAction,
    ) -> Result<(), Error> {
        let operation_id = new_awaited_action.operation_id().clone();
        let current_action = self.get_awaited_action_by_operation_id(&new_awaited_action.operation_id()).await?;
        let Some(action) = current_action else {
            // TODO: Should this try to fall back to redis?
            return Err(make_err!(Code::NotFound, "Existing operation was found in redis but was not present in db"))
        };
        let new_sorted_state = SortedAwaitedActionState::try_from(&new_awaited_action.state().stage)?;
        let current_sorted_state = SortedAwaitedActionState::try_from(&action.state().stage)?;
        let sorted_awaited_action = SortedAwaitedAction::from(&new_awaited_action);

        let awaited_action_bytes: Vec<u8> = new_awaited_action.clone().try_into()?;
        let sorted_awaited_action_bytes: Vec<u8> = sorted_awaited_action.clone().try_into()?;

        let base_key = format!("oid:{operation_id}");

        // Lets us have a sorted index of actions prefixed by their stage.
        let old_key = format!("{current_sorted_state}:{}", sorted_awaited_action.to_string());
        let new_key = format!("{new_sorted_state}:{sorted_awaited_action}");
        self.store.remove(old_key.into()).await?;
        self.store.update_oneshot(new_key.as_str(), sorted_awaited_action_bytes.clone().into()).await?;

        // Update the actual oid key to contain the awaited action.
        self.store.update_oneshot(base_key.as_str(), awaited_action_bytes.into()).await?;
        self.store.publish_channel(base_key.into(), new_awaited_action).await
    }

    /// Add (or join) an action to the AwaitedActionDb and subscribe
    /// to changes.
    async fn add_action(
        &self,
        client_operation_id: ClientOperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<RedisOperationSubscriber, Error> {
        match self.get_operation_id_by_hash_key(&action_info.unique_qualifier).await {
            Ok(Some(operation_id)) => {
                let Some(sub) = self.subscribe_to_operation(&operation_id).await? else {
                    return Err(make_err!(Code::NotFound, "Existing operation was found in redis but was not present in db"))
                };
                let key = format!("cid:{client_operation_id}");
                // RedisStore::update_oneshot(store, key.as_str(), operation_id.to_string().into()).await;
                self.store.update_oneshot(key.as_str(), operation_id.to_string().into()).await?;
                Ok(sub)
            },
            Ok(None) => {
                let operation_id = OperationId::new(action_info.unique_qualifier.clone());
                let mut inner = self.inner.lock().await;
                // let sub = self.subscribe_to_operation(&operation_id).await;
                let key = format!("cid:{client_operation_id}");
                self.store.update_oneshot(key.as_str(), operation_id.to_string().into()).await?;
                // Doing this here saves us a `clone` call on `action_info`.
                let action_hash_key = format!("ahk:{}", &action_info.unique_qualifier);
                let action = AwaitedAction::new(operation_id.clone(), action_info);

                let operation_id_key = format!("oid:{operation_id}");
                let bytes_action: Vec<u8> = action.clone().try_into()?;

                let sorted_state = SortedAwaitedActionState::try_from(action.state().stage.clone())?;
                let sorted_action: SortedAwaitedAction = action.borrow().into();

                let (tx, rx) = tokio::sync::watch::channel(action);
                inner.set_operation_sender(&operation_id, tx).await;
                self.store.update_oneshot(operation_id_key.as_str(), bytes_action.into()).await?;
                self.store.update_oneshot(action_hash_key.as_str(), operation_id.to_string().into()).await?;

                let sort_key = format!("{sorted_state}:{sorted_action}");
                let sorted_action_bytes: Vec<u8> = sorted_action.try_into()?;
                self.store.update_oneshot(sort_key.as_str(), sorted_action_bytes.into());
                self.store.update_oneshot(action_hash_key.as_str(), operation_id.to_string().into()).await?;

                // If we are going in or out of Queued State, need to add/remove the sort key from the index
                // key:
                // value: AwaitedAction
                // key: Sorted Set Table Name
                //
                // client.zscan("QUEUED_ACTIONS", pattern, count)
                Ok(RedisOperationSubscriber {
                    awaited_action_rx: rx,
                    operation_id: operation_id.clone(),
                })
            }
            Err(e) => Err(e)
        }
    }

    async fn get_range_of_actions(
        &self,
        state: SortedAwaitedActionState,
        start: Bound<SortedAwaitedAction>,
        end: Bound<SortedAwaitedAction>,
        desc: bool,
    ) -> impl Stream<Item = Result<Self::Subscriber, Error>> + Send + Sync {
        ChunkedStream::new(start, end, move |start, end, mut output| async move {
            let inner = self.inner.lock().await;
            let mut done = true;
            let mut new_start = start.as_ref();
            let mut new_end = end.as_ref();

            let iterator = self.get_range_of_actions(state, (start.as_ref(), end.as_ref())).await;
            // TODO(allada) This should probably use the `.left()/right()` pattern,
            // but that doesn't exist in the std or any libraries we use.
            if desc {
                for result in iterator.rev() {
                    let (sorted_awaited_action, item) =
                        result.err_tip(|| "In AwaitedActionDb::get_range_of_actions")?;
                    output.push_back(item);
                    new_end = Bound::Excluded(sorted_awaited_action);
                    done = false;
                }
            } else {
                for result in iterator {
                    let (sorted_awaited_action, item) =
                        result.err_tip(|| "In AwaitedActionDb::get_range_of_actions")?;
                    output.push_back(item);
                    new_start = Bound::Excluded(sorted_awaited_action);
                    done = false;
                }
            }
            if done {
                return Ok(None);
            }
            Ok(Some(((new_start.cloned(), new_end.cloned()), output)))
        })
    }

    async fn get_all_awaited_actions(&self) -> impl Stream<Item = Result<RedisOperationSubscriber, Error>> {
        // Return a stream of RedisOperationSubscribers
        let ids: Vec<OperationId> = {
            let mut inner = self.inner.lock().await;
            inner.get_operations_list()
                .await
        };
        let subs = {
            let mut inner = self.inner.lock().await;
            let mut subscriptions: Vec<Result<RedisOperationSubscriber, Error>> = Vec::new();
            for id in ids.iter() {
                let maybe_tx = inner.get_operation_sender(id).await;
                if let Some(tx) = maybe_tx {
                    subscriptions.push(Ok(RedisOperationSubscriber {
                        operation_id: id.clone(),
                        awaited_action_rx: tx.subscribe()
                    }))
                } else {
                    let err = make_err!(Code::NotFound, "Failed to find subscription for Operation {}", id);
                    subscriptions.push(Err(err))
                };
            }
            subscriptions
        };
        stream::iter(subs)
    }
}

//
// impl RedisActionStream {
//     pub async fn new(con: RedisClient) {
//         let mut vec: Vec<AwaitedAction> = Vec::new();
//         let keys_iter = con.scan("oid:*", None, None).collect();
//         let keys = keys_iter.collect::<Vec<String>>().await;
//         let mut values: Vec<AwaitedAction> = con.mget::<Vec<String>, Vec<AwaitedAction>>(keys).await.unwrap();
//         values.sort_by(|a, b| {
//            a.action_info().priority.cmp(&b.action_info().priority)
//         });
//     }
// }
//
