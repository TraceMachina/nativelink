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
use std::collections::HashMap;
use std::future::IntoFuture;
use std::ops::Bound;
use std::time::{Duration, Instant};
use std::sync::Arc;

use async_lock::{Mutex, MutexGuard};
use nativelink_error::{Code, make_err, make_input_err, Error, ResultExt};
use nativelink_store::redis_store::RedisStore;
use nativelink_util::action_messages::{
    ActionInfo, ActionUniqueKey, ActionUniqueQualifier, ClientOperationId, OperationId
};

use futures::{FutureExt, Stream, StreamExt};
use nativelink_util::background_spawn;
use nativelink_util::chunked_stream::ChunkedStream;
use nativelink_util::connection_manager::ConnectionManager;
use nativelink_util::metrics_utils::MetricsComponent;
use nativelink_util::store_trait::{StoreKey, StoreLike};
use redis::{AsyncCommands, Commands};
use serde::de::value;
use tokio::sync::{mpsc, watch};
use tokio_stream::StreamExt;
use tonic::client;
use tracing::{event, Level};

use crate::awaited_action_db::{
    AwaitedAction, AwaitedActionDb, AwaitedActionSubscriber, SortedAwaitedAction,
    SortedAwaitedActionState,
};

/// Number of events to process per cycle.
const MAX_ACTION_EVENTS_RX_PER_CYCLE: usize = 1024;

/// Duration to wait before sending client keep alive messages.
const CLIENT_KEEPALIVE_DURATION: Duration = Duration::from_secs(10);

/// Information required to track an individual client
/// keep alive config and state.
// struct ClientKeepAlive {
//     /// The client operation id.
//     client_operation_id: ClientOperationId,
//     /// The last time a keep alive was sent.
//     last_keep_alive: Instant,
//     /// The sender to notify of this struct being dropped.
//     drop_tx: mpsc::UnboundedSender<RedisActionEvent>,
// }

/// Actions the AwaitedActionsDb needs to process.
enum RedisActionEvent {
    /// A client has sent a keep alive message.
    // ClientKeepAlive(ClientOperationId),
    /// A client has dropped and pointed to OperationId.
    SubscriberDroppedOperation(OperationId),
}

/// Subscriber that can be used to monitor when AwaitedActions change.
pub struct RedisAwaitedActionSubscriber {
    /// The receiver to listen for changes.
    awaited_action_rx: watch::Receiver<AwaitedAction>,
    /// The client operation id and keep alive information.
    operation_id: OperationId,
    /// Drop tx
    drop_tx: mpsc::UnboundedSender<RedisActionEvent>
}


impl RedisAwaitedActionSubscriber {}

impl AwaitedActionSubscriber for RedisAwaitedActionSubscriber {
    async fn changed(&mut self) -> Result<AwaitedAction, Error> {
        self.awaited_action_rx.changed().await;
        Ok(self.awaited_action_rx.borrow().clone())
    }

    fn borrow(&self) -> AwaitedAction {
        self.awaited_action_rx.borrow().clone()
    }
}

impl Drop for RedisAwaitedActionSubscriber {
    fn drop(&mut self) {
        self.drop_tx.send(RedisActionEvent::SubscriberDroppedOperation(self.operation_id.clone()));
    }
}


struct RedisAwaitedActionDbImpl {
    sub_count: HashMap<OperationId, usize>,
    // Sends to awaitedactionsubscriber
    tx_map: HashMap<OperationId, watch::Sender<AwaitedAction>>,
    // Should this really be here?
    drop_tx: mpsc::UnboundedSender<RedisActionEvent>
}

impl RedisAwaitedActionDbImpl {
    pub async fn get_operation_subscriber_count(&mut self, operation_id: &OperationId) -> usize {
        match self.sub_count.get(operation_id) {
            Some(count) => *count,
            None => 0
        }
    }

    pub async fn set_operation_subscriber_count(&mut self, operation_id: &OperationId, count: usize) -> Result<(), Error> {
        if count == 0 {
            // need to clean up the sender in other map when this happens
            self.sub_count.remove(operation_id);
        }
        else { self.sub_count.insert(operation_id.clone(), count); }
        Ok(())
    }

    pub async fn get_operation_sender(&mut self, operation_id: &OperationId) -> Option<watch::Sender<AwaitedAction>> {
        self.tx_map.get(operation_id).cloned()
    }

    pub async fn set_operation_sender(&mut self, operation_id: &OperationId, tx: watch::Sender<AwaitedAction>) -> Result<(), Error> {
        self.tx_map.insert(operation_id.clone(), tx);
        Ok(())
    }

}

pub struct RedisAwaitedActionDb {
    store: Arc<RedisStore>,
    inner: Arc<Mutex<RedisAwaitedActionDbImpl>>,
    // recv from action
    drop_event_rx: mpsc::UnboundedReceiver<RedisActionEvent>,
    drop_event_tx: mpsc::UnboundedSender<RedisActionEvent>,
}

impl RedisAwaitedActionDb {
    async fn get_operation_subscriber_count(&self, operation_id: &OperationId) -> usize {
        let mut inner = self.inner.lock().await;
        inner.get_operation_subscriber_count(operation_id).await
    }

    async fn inc_operation_subscriber_count(&self, operation_id: &OperationId) {
        let mut inner = self.inner.lock().await;
        let existing_count = inner.get_operation_subscriber_count(operation_id).await;
        inner.set_operation_subscriber_count(operation_id, existing_count+1).await;
    }

    async fn dec_operation_subscriber_count(&self, operation_id: &OperationId) {
        let mut inner = self.inner.lock().await;
        let existing_count = inner.get_operation_subscriber_count(operation_id).await;
        inner.set_operation_subscriber_count(operation_id, existing_count-1).await;
    }

    async fn subscribe_to_operation(&self, operation_id: &OperationId) -> Result<RedisAwaitedActionSubscriber, Error> {
        let mut inner = self.inner.lock().await;
        let Some(tx) = inner.get_operation_sender(operation_id).await else {
            return Err(make_err!(Code::NotFound, "Failed to find sender for operation: {operation_id}"));
        };

        // TODO: Create a spawn which will wait on this drop_rx to notify the db of the drop
        let drop_tx = self.drop_event_tx.clone();

        Ok(RedisAwaitedActionSubscriber {
            awaited_action_rx: tx.subscribe(),
            operation_id: operation_id.clone(),
            drop_tx
        })
    }

   async fn get_operation_id_by_client_id(&self, client_id: &ClientOperationId) -> Result<OperationId, Error> {
        let key = format!("cid:{client_id}");
        let bytes = self.store.get_part_unchunked(StoreKey::from(key), 0, None).await?;
        let s = String::from_utf8(bytes.to_vec()).map_err(|e| {
            make_input_err!("Decoding bytes failed with error: {e}")
        })?;
        OperationId::try_from(s.as_str())
    }

    async fn get_operation_id_by_hash_key(&self, unique_qualifier: &ActionUniqueQualifier) -> Result<OperationId, Error> {
        let key = format!("ahk:{unique_qualifier}");
        let bytes = self.store.get_part_unchunked(StoreKey::from(key), 0, None).await?;
        let s = String::from_utf8(bytes.to_vec()).map_err(|e| {
            make_input_err!("Decoding bytes failed with error: {e}")
        })?;
        OperationId::try_from(s.as_str())
    }

    async fn get_awaited_action_by_operation_id(&self, operation_id: &OperationId) -> Result<AwaitedAction, Error> {
        let key = format!("oid:{operation_id}");
        let bytes = self.store.get_part_unchunked(StoreKey::from(key), 0, None).await?;
        AwaitedAction::try_from(bytes.to_vec().as_slice()).err_tip(|| "In redis_awaited_action_db::get_awaited_action_by_operation_id")
    }

    async fn set_client_id(&self, client_id: &ClientOperationId, operation_id: &OperationId) -> Result<(), Error> {
        let key = StoreKey::from(format!("cid:{client_id}"));
        self.store.update_oneshot(key, operation_id.to_string().into()).await?;
        Ok(())
    }

    async fn set_hashkey_operation(&self, hash_key: &ActionUniqueQualifier, operation_id: &OperationId) -> Result<(), Error> {
        let key = StoreKey::from(format!("ahk:{hash_key}"));
        self.store.update_oneshot(key, operation_id.to_string().into()).await?;
        Ok(())
    }
}

impl RedisAwaitedActionDb {
    pub fn new(
        store: Arc<RedisStore>,
        inner: Arc<Mutex<RedisAwaitedActionDbImpl>>
    ) -> Self {
        let client = store.get_client();
        let weak_inner = Arc::downgrade(&inner);
        let _joinhandle = background_spawn!("redis_action_change_listener", async move {
            let pubsub_result = client.get_async_pubsub().await;
            let Ok(mut pubsub) = pubsub_result else {
                event!(Level::ERROR, "RedisAwaitedActionDb::new Failed to get pubsub");
                return
            };
            if let Err(e) = pubsub.subscribe("update:*").await {
                event!(Level::ERROR, ?e, "RedisAwaitedActionDb::new Failed to subscribe to channel");
                return
            }
            let mut stream = pubsub.into_on_message();
            loop {
                let msg = stream.next().await.unwrap();
                match weak_inner.upgrade() {
                    Some(inner_mutex) => {
                        let state: AwaitedAction = AwaitedAction::try_from(msg.get_payload_bytes()).unwrap();
                        let mut inner_mut = inner_mutex.lock().await;
                        let tx = inner_mut.get_operation_sender(state.operation_id()).await.unwrap();
                        tx.send_replace(state);
                    }
                    None => {
                        event!(Level::ERROR, "RedisAwaitedActionDb - Failed to upgrade inner");
                        return
                    }
                }
            }
        });

        let (drop_event_tx, drop_event_rx) = mpsc::unbounded_channel();
        Self {
            store,
            inner,
            drop_event_rx,
            drop_event_tx,
        }
    }
}

impl AwaitedActionDb for RedisAwaitedActionDb {
    type Subscriber = RedisAwaitedActionSubscriber;

    async fn add_action(
        &self,
        client_id: ClientOperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<Self::Subscriber, Error> {
        let id_result = self.get_operation_id_by_hash_key(&action_info.unique_qualifier).await;
        match id_result {
            Ok(operation_id) => {
                let sub = self.subscribe_to_operation(&operation_id).await;
                let key = format!("cid:{client_id}");
                self.store.update_oneshot(StoreKey::from(key), operation_id.to_string().into()).await?;
                self.inc_operation_subscriber_count(&operation_id).await;
                sub
            },
            Err(e) => {
                match e.code {
                    Code::NotFound => {
                        let operation_id = OperationId::new(action_info.unique_qualifier.clone());
                        let sub = self.subscribe_to_operation(&operation_id).await;
                        let key = format!("cid:{client_id}");
                        self.store.update_oneshot(StoreKey::from(key), operation_id.to_string().into()).await?;

                        let key = format!("ahk:{}", &action_info.unique_qualifier);
                        self.store.update_oneshot(StoreKey::from(key), operation_id.to_string().into()).await?;

                        let action = AwaitedAction::new(operation_id.clone(), action_info);

                        let key = format!("oid:{operation_id}");
                        self.store.update_oneshot(StoreKey::from(key), action.as_bytes().into()).await?;


                        self.inc_operation_subscriber_count(&operation_id).await;
                        sub
                    },
                    _ => { return Err(e) }
                }
            }
        }
    }

    async fn get_by_operation_id(
        &self,
        operation_id: &OperationId,
    ) -> Result<Option<Self::Subscriber>, Error> {
        let result = self.subscribe_to_operation(&operation_id).await;
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

    async fn get_range_of_actions(
        &self,
        _state: SortedAwaitedActionState,
        start: std::ops::Bound<SortedAwaitedAction>,
        end: std::ops::Bound<SortedAwaitedAction>,
        _desc: bool,
    ) -> impl Stream<Item = Result<Self::Subscriber, Error>> {
        ChunkedStream::new(start, end, move |_start, _end, /*mut*/ _output| async move {
            todo!()
        })
    }

    async fn update_awaited_action(
        &self,
        new_awaited_action: AwaitedAction,
    ) -> Result<(), Error> {
        let operation_id = new_awaited_action.operation_id();

        let mut con = self.store.get_conn().await?;
        let bytes: Vec<u8> = new_awaited_action.as_bytes();

        let key = StoreKey::from(format!("oid:{operation_id}"));
        self.store.update_oneshot(key, bytes.clone().into()).await;

        let pub_channel = format!("update:{operation_id}");
        Ok(con.publish(&pub_channel, bytes).await?)
    }

    async fn get_all_awaited_actions(
        &self
    ) -> impl Stream<Item = Result<Self::Subscriber, Error>> {
        let mut con = self.store.get_conn().await.unwrap();
        let mut vec: Vec<AwaitedAction> = Vec::new();
        let keys_iter = con.scan_match("oid:*").await.unwrap();
        let keys = keys_iter.collect::<Vec<String>>().await;
        let mut values: Vec<AwaitedAction> = con.mget::<Vec<String>, Vec<AwaitedAction>>(keys).await.unwrap();
        values.sort_by(|a, b| {
           a.action_info().priority.cmp(&b.action_info().priority)
        });
        // let all_actions = con.
        ChunkedStream::new(
            Bound::Unbounded,
            Bound::Unbounded,
            move |start, end, mut output| async move {
                for awaited_action in values {

                }
            }
        )
    }

    async fn get_awaited_action_by_id(
        &self,
        client_id: &ClientOperationId,
    ) -> Result<Option<Self::Subscriber>, Error> {
        let result = self.get_operation_id_by_client_id(client_id).await;
        let operation_id = match result {
            Ok(v) => OperationId::try_from(v)?,
            Err(e) => {
                match e.code {
                    Code::NotFound => return Ok(None),
                    _ => return Err(e)
                }
            }
        };
        let subscription_result = self.subscribe_to_operation(&operation_id).await;
        match subscription_result {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                match e.code {
                    Code::NotFound => return Ok(None),
                    _ => return Err(e)
                }
            }
        }
    }
}


impl MetricsComponent for RedisAwaitedActionDb {
    fn gather_metrics(&self, collector: &mut nativelink_util::metrics_utils::CollectorState) {
        todo!()
    }
}
