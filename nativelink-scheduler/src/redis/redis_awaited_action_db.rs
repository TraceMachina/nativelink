use std::ops::Bound;
use std::sync::Arc;

use futures::{stream, Stream};
use nativelink_error::{Code, Error, ResultExt};
use nativelink_metric::MetricsComponent;
use nativelink_store::redis_store::RedisStore;
use nativelink_util::action_messages::{ActionInfo, OperationId};

use crate::awaited_action_db::{
    AwaitedAction, AwaitedActionDb, SortedAwaitedAction, SortedAwaitedActionState,
};
use crate::redis::redis_adapter::RedisAdapter;
use crate::redis::subscription_manager::RedisOperationSubscriber;

#[derive(MetricsComponent)]
pub struct RedisAwaitedActionDb {
    redis_adapter: RedisAdapter,
    // sub_manager: RedisOperationSubscribers
}

impl RedisAwaitedActionDb {
    pub fn new(store: Arc<RedisStore>) -> Self {
        Self {
            redis_adapter: RedisAdapter::new(store),
        }
    }
}

impl AwaitedActionDb for RedisAwaitedActionDb {
    type Subscriber = RedisOperationSubscriber;
    /// Get the AwaitedAction by the client operation id.
    async fn get_awaited_action_by_id(
        &self,
        client_operation_id: &OperationId,
    ) -> Result<Option<Self::Subscriber>, Error> {
        match self
            .redis_adapter
            .get_operation_id_by_client_id(client_operation_id)
            .await
        {
            Ok(operation_id) => {
                // TODO: Match to return None
                let rx = self
                    .redis_adapter
                    .subscribe_to_operation(&operation_id)
                    .await?;
                Ok(Some(RedisOperationSubscriber {
                    awaited_action_rx: rx,
                }))
            }
            Err(e) => match e.code {
                Code::NotFound => Ok(None),
                _ => Err(e),
            },
        }
    }

    /// Get the AwaitedAction by the operation id.
    async fn get_by_operation_id(
        &self,
        operation_id: &OperationId,
    ) -> Result<Option<RedisOperationSubscriber>, Error> {
        let rx = self
            .redis_adapter
            .subscribe_to_operation(operation_id)
            .await
            .err_tip(|| "In RedisAwaitedActionDb::get_by_operation_id")?;
        Ok(Some(RedisOperationSubscriber {
            awaited_action_rx: rx,
        }))
    }

    /// Process a change changed AwaitedAction and notify any listeners.
    async fn update_awaited_action(&self, new_awaited_action: AwaitedAction) -> Result<(), Error> {
        self.redis_adapter
            .update_awaited_action(new_awaited_action)
            .await
    }

    /// Add (or join) an action to the AwaitedActionDb and subscribe
    /// to changes.
    async fn add_action(
        &self,
        client_id: OperationId,
        action_info: Arc<ActionInfo>,
    ) -> Result<RedisOperationSubscriber, Error> {
        let rx = self
            .redis_adapter
            .subscribe_client_to_operation(client_id, action_info)
            .await
            .err_tip(|| "In RedisAwaitedActionDb::add_action")?;
        Ok(RedisOperationSubscriber {
            awaited_action_rx: rx,
        })
    }

    async fn get_range_of_actions(
        &self,
        state: SortedAwaitedActionState,
        start: Bound<SortedAwaitedAction>,
        end: Bound<SortedAwaitedAction>,
        desc: bool,
    ) -> impl Stream<Item = Result<Self::Subscriber, Error>> + Send {
        match self
            .redis_adapter
            .get_range_of_actions(state, start, end, desc)
            .await
        {
            Ok(subscribers) => stream::iter(subscribers),
            Err(e) => stream::iter(vec![Err(e)]),
        }
    }

    async fn get_all_awaited_actions(
        &self,
    ) -> impl Stream<Item = Result<RedisOperationSubscriber, Error>> {
        match self.redis_adapter.list_all_sorted_actions().await {
            Ok(subscribers) => stream::iter(subscribers),
            Err(e) => stream::iter(vec![Err(e)]),
        }
    }
}
