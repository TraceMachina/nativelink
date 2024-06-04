use std::sync::Arc;
use std::time::SystemTime;
use tonic::async_trait;
use nativelink_util::action_messages::{ActionInfo, ActionInfoHashKey, ActionResult, ActionStage, ActionState, OperationId, WorkerId};
use redis::aio::{MultiplexedConnection, PubSub};
use redis::{Client, Connection, JsonAsyncCommands};
use nativelink_error::{make_input_err, Error};
use tokio::sync::watch;
use redis_macros::Json;
use serde::{Serialize, Deserialize};
use crate::operation_state_manager::{ActionStateResult, ActionStateResultStream, MatchingEngineStateManager, OperationFilter, OperationStageFlags};

pub struct RedisStateManager  {
    client: Client,
}

impl RedisStateManager {
    pub fn new(url: String) -> Self {
        Self { client: Client::open(url).unwrap() }
    }

    async fn get_async_pubsub(&self) -> Result<PubSub, Error> {
        Ok(self.client.get_async_pubsub().await?)
    }

    // These getters avoid mapping errors everywhere and just use the ? operator.
    async fn get_multiplex_connection(&self) -> Result<MultiplexedConnection, Error> {
        Ok(self.client.get_multiplexed_tokio_connection().await?)
    }

    // These getters avoid mapping errors everywhere and just use the ? operator.
    fn get_connection(&self) -> Result<Connection, Error> {
        Ok(self.client.get_connection()?)
    }
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
enum OperationStage {
    CacheCheck,
    Queued,
    Executing,
    Completed,
}


fn parse_stage_flags(_flags: &OperationStageFlags) -> Vec<OperationStage> {
    todo!()
}


#[derive(Serialize, Deserialize)]
pub struct RedisOperation {
    operation_id: OperationId,
    info: ActionInfo,
    worker_id: Option<WorkerId>,
    result: Option<ActionResult>,
    stage: OperationStage,
    last_worker_update: Option<SystemTime>,
    last_client_update: Option<SystemTime>,
    completed_at: Option<SystemTime>
}

#[async_trait]
impl ActionStateResult for RedisOperation {
    // Provides the current state of the action.
    async fn as_state(&self) -> Result<Arc<ActionState>, Error> {
        let action_state = ActionState {
            stage: self.action_stage()?,
            id: self.operation_id.clone()
        };
        Ok(Arc::new(action_state))
    }
    // Subscribes to the state of the action, receiving updates as they are published.
    async fn as_receiver(&self) -> Result<&'_ watch::Receiver<Arc<ActionState>>, Error> {
        todo!()
    }
}
impl RedisOperation {
    pub fn action_stage(&self) -> Result<ActionStage, Error> {
        match self.stage {
            OperationStage::CacheCheck => Ok(ActionStage::CacheCheck),
            OperationStage::Queued => Ok(ActionStage::Queued),
            OperationStage::Executing => Ok(ActionStage::Executing),
            OperationStage::Completed => {
                let Some(result) = &self.result else {
                    return Err(
                        make_input_err!(
                            "Operation {} was marked as completed but has no result",
                            self.operation_id.to_string()
                        )
                    )
                };
                Ok(ActionStage::Completed(result.clone()))
            }
        }
    }
    pub fn unique_qualifier(&self) -> &ActionInfoHashKey {
        &self.operation_id.unique_qualifier
    }
}

fn match_optional_filter<T: PartialEq>(value_opt: Option<T>, filter_opt: Option<T>, cond: impl Fn(T, T) -> bool) -> bool {
    let Some(filter) = filter_opt else { return true };
    let Some(value) = value_opt else { return false; };
    cond(filter, value)
}

pub fn matches_filter(operation: &RedisOperation, filter: &OperationFilter) -> bool {
    if !parse_stage_flags(&filter.stages).contains(&operation.stage) {
        return false
    }
    match_optional_filter(Some(&operation.operation_id), filter.operation_id.as_ref(), |a, b| { a == b })
        && match_optional_filter(operation.worker_id, filter.worker_id, |a, b| { a == b })
        && match_optional_filter(Some(operation.unique_qualifier().digest), filter.action_digest, |a, b| { a == b })
        && match_optional_filter(operation.completed_at, filter.completed_before, |a, b| { a < b })
        && match_optional_filter(operation.last_client_update, filter.last_client_update_before, |a, b| { a < b })
}

#[async_trait]
impl MatchingEngineStateManager for RedisStateManager {
    /// Returns a stream of operations that match the filter.
    async fn filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        let mut con = self.get_multiplex_connection().await.unwrap();
        let operations = {
            // returns list of all operations
            let Json(operations): Json<Vec<RedisOperation>> = con.json_get("operations:", "$").await?;
            operations
        };
        let mut v: Vec<Arc<dyn ActionStateResult>> = Vec::new();
        for operation in operations {
            if matches_filter(&operation, &filter) {
                v.push(Arc::new(operation));
            }
        }
        Ok(Box::pin(futures::stream::iter(v)))
    }

    async fn update_operation(
        &self,
        operation_id: OperationId,
        _worker_id: Option<WorkerId>,
        _action_stage: Result<ActionStage, Error>,
    ) -> Result<(), Error> {
        let mut con = self.get_multiplex_connection().await.unwrap();
        let path = format!("$.{}", operation_id);
        let Json(_stored_operation_json): Json<RedisOperation> = con.json_get("operations", path).await?;
        todo!()
    }

    /// Remove an operation from the state manager.
    /// It is important to use this function to remove operations
    /// that are no longer needed to prevent memory leaks.
    async fn remove_operation(&self, _operation_id: OperationId) -> Result<(), Error> {
        todo!()
    }
}
