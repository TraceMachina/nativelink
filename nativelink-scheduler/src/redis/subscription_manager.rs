use std::collections::HashMap;
use std::sync::Arc;

use async_lock::Mutex;
use nativelink_error::{make_err, Error};
use nativelink_util::action_messages::OperationId;
use tokio::sync::watch;

use crate::awaited_action_db::{AwaitedAction, AwaitedActionSubscriber};

/// Duration to wait before sending client keep alive messages.
/// Subscriber that can be used to monitor when AwaitedActions change.
pub struct RedisOperationSubscriber {
    /// The receiver to listen for changes.
    pub awaited_action_rx: watch::Receiver<AwaitedAction>,
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

    pub fn get_operations_list(&mut self) -> Vec<OperationId> {
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
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(RedisOperationSubscribersImpl::new())),
        }
    }

    pub async fn get_operation_sender(
        &self,
        operation_id: &OperationId,
    ) -> Option<watch::Sender<AwaitedAction>> {
        let mut inner = self.inner.lock().await;
        inner.get_operation_sender(operation_id)
    }

    pub async fn get_operations_list(&self) -> Vec<OperationId> {
        let mut inner = self.inner.lock().await;
        inner.get_operations_list()
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
