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

use std::ops::Deref;
use std::pin::{pin, Pin};
use std::{ops::Bound, str::from_utf8};
use std::collections::HashMap;
use std::task::Poll;
use fred::clients::RedisClient;
use fred::error::RedisError;
use nativelink_metric::MetricsComponent;

use std::sync::Arc;
use nativelink_util::chunked_stream::ChunkedStream;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::BroadcastStream;

use async_lock::Mutex;
use fred::interfaces::{ClientLike, EventInterface, KeysInterface, PubsubInterface};
use nativelink_error::{Code, make_err, make_input_err, Error};
use nativelink_store::redis_store::RedisStore;
use nativelink_util::action_messages::{
    ActionInfo, ActionUniqueQualifier, ClientOperationId, OperationId
};

use futures::{join, poll, stream, Stream, StreamExt, TryStreamExt};
use nativelink_util::task::JoinHandleDropGuard;
use nativelink_util::spawn;
use nativelink_util::store_trait::{StoreDriver, StoreLike};
use fred::types::{Message, RedisKey, ScanResult, Scanner};
use tokio::sync::watch;
use tonic::async_trait;
use tracing::{event, Level};

use crate::awaited_action_db::{
    AwaitedAction, AwaitedActionDb, AwaitedActionSubscriber, SortedAwaitedAction, SortedAwaitedActionState
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
        self.awaited_action_rx.changed().await;
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
    inner: Arc<Mutex<RedisAwaitedActionDbImpl>>,
    _join_handle: JoinHandleDropGuard<()>,
}


// fn parse_message(msg: fred::
impl RedisAwaitedActionDb {
    pub async fn new(
        store: Arc<RedisStore>,
    ) -> Result<Self, Error> {
        let inner = Arc::new(Mutex::new(RedisAwaitedActionDbImpl::new()));
        let weak_inner = Arc::downgrade(&inner);
        let store_clone = store.clone();

        let join_handle = spawn!("redis_action_change_listener", async move {
            let sub = store_clone.get_subscriber_client();
            let Ok(_connection_handle) = sub.init().await else {
                event!(Level::ERROR, "RedisAwaitedActionDb::new Failed initialize pubsub");
                return
            };
            if sub.psubscribe("oid:*").await.is_err() {
                event!(Level::ERROR, "RedisAwaitedActionDb::new Failed to subscribe to AwaitedActionUpdates");
                return
            };

            let mut stream = BroadcastStream::from(sub.message_rx());
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
    //
    // async fn get_awaited_action_by_operation_id(&self, operation_id: &OperationId) -> Result<Option<AwaitedAction>, Error> {
    //     let result: Result<AwaitedAction, Error> = {
    //         let key = format!("oid:{operation_id}");
    //         let bytes = self.store.get_part_unchunked(key.as_str(), 0, None).await?;
    //         AwaitedAction::try_from(bytes.to_vec().as_slice())
    //     };
    //     match result {
    //         Ok(v) => Ok(Some(v)),
    //         Err(e) => {
    //             match e.code {
    //                 Code::NotFound => Ok(None),
    //                 _ => Err(e)
    //             }
    //         }
    //     }
    // }
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

    async fn get_all_awaited_actions(&self) -> impl Stream<Item = Result<RedisOperationSubscriber, Error>> {
        let ids: Vec<RedisKey> = {
            let mut inner = self.inner.lock().await;
            inner.get_operations_list()
                .await
                .into_iter()
                .map(|id| { RedisKey::from(id.to_string()) })
                .collect()
        };
        let client = self.store.get_redis_client();
        let result: Result<Vec<Vec<u8>>, Error> = client.mget(ids)
            .await
            .map_err(|e| { e.into() });
        let awaited_actions_result = result.unwrap();

        let mut result: Vec<AwaitedAction> = awaited_actions_result
            .into_iter()
            .map(|action_bytes| {
                AwaitedAction::try_from(action_bytes.as_slice()).unwrap()
            }).collect();
        result.sort_by(|a, b| {
            a.sort_key().cmp(&b.sort_key())
        });
        let sorted_ids: Vec<OperationId> = result.iter().map(|action| {
            action.operation_id().clone()
        }).collect();

        let subs = {
            let mut inner = self.inner.lock().await;
            let mut subscriptions: Vec<Result<RedisOperationSubscriber, Error>> = Vec::new();
            for id in sorted_ids.iter() {
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

        let con = self.store.get_redis_client();
        con.connect();
        let bytes: Vec<u8> = new_awaited_action.try_into()?;

        let key = format!("oid:{operation_id}");
        self.store.update_oneshot(key.as_str(), bytes.clone().into()).await?;

        let pub_channel = format!("update:{operation_id}");
        Ok(con.publish(&pub_channel, bytes).await?)
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
                let store: Pin<&dyn StoreDriver> = self.store.as_store_driver_pin();
                // RedisStore::update_oneshot(store, key.as_str(), operation_id.to_string().into()).await;
                store.update_oneshot(key.as_str().into(), operation_id.to_string().into()).await?;
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

                let (tx, rx) = tokio::sync::watch::channel(action);
                inner.set_operation_sender(&operation_id, tx).await;

                self.store.update_oneshot(operation_id_key.as_str(), bytes_action.into()).await?;
                self.store.update_oneshot(action_hash_key.as_str(), operation_id.to_string().into()).await?;
                Ok(RedisOperationSubscriber {
                    awaited_action_rx: rx,
                    operation_id: operation_id.clone(),
                })
            }
            Err(e) => Err(e)
        }
    }

    /// Get a range of AwaitedActions of a specific state in sorted order.
    async fn get_range_of_actions(
        &self,
        state: SortedAwaitedActionState,
        start: Bound<SortedAwaitedAction>,
        end: Bound<SortedAwaitedAction>,
        desc: bool,
    ) -> impl Stream<Item = Result<RedisOperationSubscriber, Error>> + Send + Sync {
        // TODO
        stream::iter(Vec::new())
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
