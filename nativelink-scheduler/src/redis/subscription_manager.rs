use std::collections::HashMap;
use std::sync::Arc;
use std::task::Poll;

use async_lock::Mutex;
use fred::clients::SubscriberClient;
use fred::interfaces::{EventInterface, PubsubInterface};
use fred::types::Message;
use futures::{poll, StreamExt};
use nativelink_error::{make_err, Code, Error};
use nativelink_util::action_messages::OperationId;
use nativelink_util::background_spawn;
use tokio::sync::watch;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tracing::{event, Level};

use crate::awaited_action_db::{AwaitedAction, AwaitedActionSubscriber};

/// Duration to wait before sending client keep alive messages.
/// Subscriber that can be used to monitor when AwaitedActions change.
#[derive(Debug)]
pub struct RedisOperationSubscriber {
    /// The receiver to listen for changes.
    pub awaited_action_rx: watch::Receiver<AwaitedAction>,
}
impl RedisOperationSubscriber {
    pub fn new(awaited_action_rx: watch::Receiver<AwaitedAction>) -> Self {
        Self { awaited_action_rx }
    }
}

impl std::fmt::Display for RedisOperationSubscriber {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.borrow().operation_id().to_string().as_str())
    }
}

impl AwaitedActionSubscriber for RedisOperationSubscriber {
    async fn changed(&mut self) -> Result<AwaitedAction, Error> {
        let _ = self.awaited_action_rx.changed().await;
        Ok(self.awaited_action_rx.borrow().clone())
    }

    fn borrow(&self) -> AwaitedAction {
        self.awaited_action_rx.borrow().clone()
    }
}

struct RedisOperationSubscribersImpl {
    tx_map: HashMap<OperationId, watch::Sender<AwaitedAction>>,
}

impl RedisOperationSubscribersImpl {
    pub fn new() -> Self {
        Self {
            tx_map: HashMap::new(),
        }
    }

    pub fn get_operation_sender(
        &mut self,
        operation_id: &OperationId,
    ) -> Option<watch::Sender<AwaitedAction>> {
        self.tx_map.get(operation_id).cloned()
    }

    pub fn _get_operations_list(&mut self) -> Vec<OperationId> {
        self.tx_map.keys().cloned().collect()
    }

    pub fn set_operation_sender(
        &mut self,
        operation_id: &OperationId,
        tx: watch::Sender<AwaitedAction>,
    ) {
        self.tx_map.insert(operation_id.clone(), tx);
    }

    pub fn get_operation_subscriber(
        &mut self,
        operation_id: &OperationId,
    ) -> Result<RedisOperationSubscriber, Error> {
        let Some(tx) = self.get_operation_sender(operation_id) else {
            return Err(make_err!(
                nativelink_error::Code::NotFound,
                "Could not find sender for operation {operation_id}"
            ));
        };
        Ok(RedisOperationSubscriber {
            awaited_action_rx: tx.subscribe(),
        })
    }
}

pub struct RedisOperationSubscribers {
    inner: Arc<Mutex<RedisOperationSubscribersImpl>>,
}

impl RedisOperationSubscribers {
    pub fn new(sub: SubscriberClient) -> Self {
        let inner = Arc::new(Mutex::new(RedisOperationSubscribersImpl::new()));
        let weak_inner = Arc::downgrade(&inner);

        let _join_handle = background_spawn!("redis_action_change_listener", async move {
            if let Err(e) = sub.psubscribe("updates:*").await {
                println!("Error subscribing to pattern - {e}");
                return;
            }

            let mut stream = tokio_stream::wrappers::BroadcastStream::from(sub.message_rx());
            // Unpack the Option<Result>> into just a Result
            fn handle_next(
                next: Option<Result<fred::types::Message, BroadcastStreamRecvError>>,
            ) -> Result<Message, Error> {
                match next {
                    Some(Ok(v)) => Ok(v),
                    Some(Err(e)) => {
                        // The reciever encountered an Error. Would only occur if the subscription is invalid.
                        Err(make_err!(Code::Internal, "{e}"))
                    }
                    None => {
                        // The stream has been closed, should not happen.
                        Err(make_err!(Code::Internal, "RedisAwaitedActionDb::subscription_listener subscription update stream was closed"))
                    }
                }
            }
            loop {
                let Ok(msg) = handle_next(stream.next().await) else {
                    event!(Level::ERROR, "RedisAwaitedActionDb::subscription_listener subscription update stream was closed");
                    return;
                };
                let Some(bytes) = msg.value.as_bytes() else {
                    continue;
                };
                match weak_inner.upgrade() {
                    Some(inner_mutex) => {
                        let mut inner = inner_mutex.lock().await;
                        let state: AwaitedAction = AwaitedAction::try_from(bytes).unwrap();
                        let operation_id = state.operation_id();
                        let tx = match inner.get_operation_sender(operation_id) {
                            Some(tx) => tx,
                            None => {
                                let tx = watch::Sender::new(state.clone());
                                inner.set_operation_sender(operation_id, tx.clone());
                                tx
                            }
                        };
                        // Use send_replace so that we can send the update even when there are no recievers.
                        tx.send_replace(state);
                        while let Poll::Ready(maybe_result) = poll!(stream.next()) {
                            match handle_next(maybe_result) {
                                Ok(msg) => {
                                    let Some(bytes) = msg.value.as_bytes() else {
                                        continue;
                                    };
                                    let state: AwaitedAction =
                                        AwaitedAction::try_from(bytes).unwrap();
                                    let tx =
                                        inner.get_operation_sender(state.operation_id()).unwrap();
                                    // Use send_replace so that we can send the update even when there are no recievers.
                                    tx.send_replace(state);
                                }
                                Err(e) => {
                                    event!(Level::ERROR, ?e);
                                    return;
                                }
                            }
                        }
                        drop(inner)
                    }
                    None => {
                        event!(
                            Level::ERROR,
                            "RedisAwaitedActionDb - Failed to upgrade inner"
                        );
                        return;
                    }
                };
            }
        });
        Self { inner }
    }

    pub async fn get_operation_sender(
        &self,
        operation_id: &OperationId,
    ) -> Option<watch::Sender<AwaitedAction>> {
        let mut inner = self.inner.lock().await;
        inner.get_operation_sender(operation_id)
    }

    pub async fn set_operation_sender(
        &self,
        operation_id: &OperationId,
        tx: watch::Sender<AwaitedAction>,
    ) {
        let mut inner = self.inner.lock().await;
        inner.set_operation_sender(operation_id, tx)
    }

    pub async fn get_operation_subscriber(
        &self,
        operation_id: &OperationId,
    ) -> Result<RedisOperationSubscriber, Error> {
        let mut inner = self.inner.lock().await;
        inner.get_operation_subscriber(operation_id)
    }

    pub async fn get_operation_subscribers(
        &self,
        operation_ids: &[OperationId],
    ) -> Vec<Result<RedisOperationSubscriber, Error>> {
        let mut inner = self.inner.lock().await;
        operation_ids
            .iter()
            .map(|id| inner.get_operation_subscriber(id))
            .collect()
    }
}
