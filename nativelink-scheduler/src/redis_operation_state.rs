use std::sync::Arc;
use std::time::SystemTime;
use tonic::async_trait;
use nativelink_util::action_messages::{ActionInfo, ActionInfoHashKey, ActionResult, ActionStage, ActionState, OperationId, WorkerId};
use redis::aio::{MultiplexedConnection, PubSub};
use redis::{Client, Connection, JsonAsyncCommands, Pipeline};
use nativelink_error::{make_input_err, Error};
use tokio::sync::watch;
use redis_macros::Json;
use serde::{Serialize, Deserialize};
use crate::operation_state_manager::{ActionStateResult, ActionStateResultStream, ClientStateManager, MatchingEngineStateManager, OperationFilter, OperationStageFlags, WorkerStateManager};

pub struct RedisStateManager  {
    client: Client,
}

impl RedisStateManager {
    pub fn new(url: String) -> Self {
        Self { client: Client::open(url).unwrap() }
    }

    async fn _get_async_pubsub(&self) -> Result<PubSub, Error> {
        Ok(self.client.get_async_pubsub().await?)
    }

    // These getters avoid mapping errors everywhere and just use the ? operator.
    async fn get_multiplex_connection(&self) -> Result<MultiplexedConnection, Error> {
        Ok(self.client.get_multiplexed_tokio_connection().await?)
    }

    // These getters avoid mapping errors everywhere and just use the ? operator.
    fn _get_connection(&self) -> Result<Connection, Error> {
        Ok(self.client.get_connection()?)
    }

    /// Add a new action to the queue or joins an existing action.
    async fn inner_add_action(
        &self,
        _action_info: ActionInfo,
    ) -> Result<Arc<dyn ActionStateResult>, Error> {
        todo!()
    }

    /// Returns a stream of operations that match the filter.
    async fn inner_filter_operations(
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

    /// Update that state of an operation.
    /// The worker must also send periodic updates even if the state
    /// did not change with a modified timestamp in order to prevent
    /// the operation from being considered stale and being rescheduled.
    async fn inner_update_operation(
        &self,
        operation_id: OperationId,
        worker_id: Option<WorkerId>,
        action_stage: Result<ActionStage, Error>,
    ) -> Result<(), Error> {
        let key = format!("operations:{}", operation_id);
        let mut con = self.get_multiplex_connection().await.unwrap();
        let mut pipe = Pipeline::new();
        match action_stage {
            Ok(stage) => {
                let (maybe_operation_stage, maybe_result) = match stage {
                    ActionStage::CompletedFromCache(_) => (None, None),
                    ActionStage::Completed(result) => (Some(OperationStage::Completed), Some(result)),
                    ActionStage::Queued => (Some(OperationStage::Completed), None),
                    ActionStage::Unknown => (Some(OperationStage::Unknown), None),
                    ActionStage::Executing => (Some(OperationStage::Executing), None),
                    ActionStage::CacheCheck => (Some(OperationStage::CacheCheck), None),
                };
                if let Some(operation_stage) = maybe_operation_stage {
                    pipe.json_set(&key, "$.stage", &operation_stage)?;
                }
                if let Some(result) = maybe_result {
                    pipe.json_set(&key, "$.result", &result)?;
                }
            },
            Err(e) => { pipe.json_set(&key, "$.last_error", &e)?; }
        }
        match worker_id {
            Some(id) => { pipe.json_set(&key, "$.worker_id", &id)?; },
            None => { pipe.json_del(&key, "$.worker_id")?; }
        };
        // TODO (@zbirenbaum): This should check each call for an error and propogate.
        Ok(pipe.query_async(&mut con).await?)
    }
    /// Remove an operation from the state manager.
    /// It is important to use this function to remove operations
    /// that are no longer needed to prevent memory leaks.
    async fn inner_remove_operation(&self, operation_id: OperationId) -> Result<(), Error> {
        let mut con = self.get_multiplex_connection().await.unwrap();
        let key = format!("operations:{}", operation_id);
        Ok(con.json_del(key, "$").await?)
    }
}

#[derive(PartialEq, Eq, Serialize, Deserialize)]
enum OperationStage {
    CacheCheck,
    Queued,
    Executing,
    Completed,
    Unknown,
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
    last_error: Option<Error>,
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
            OperationStage::Unknown => Ok(ActionStage::Unknown),
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
impl ClientStateManager for RedisStateManager {
    /// Add a new action to the queue or joins an existing action.
    async fn add_action(
        &self,
        action_info: ActionInfo,
    ) -> Result<Arc<dyn ActionStateResult>, Error> {
        self.inner_add_action(action_info).await
    }

    /// Returns a stream of operations that match the filter.
    async fn filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        self.inner_filter_operations(filter).await
    }
}

#[async_trait]
impl WorkerStateManager for RedisStateManager {
    async fn update_operation(
        &self,
        operation_id: OperationId,
        worker_id: WorkerId,
        action_stage: Result<ActionStage, Error>,
    ) -> Result<(), Error> {
        self.inner_update_operation(operation_id, Some(worker_id), action_stage).await
    }
}

#[async_trait]
impl MatchingEngineStateManager for RedisStateManager {
    /// Returns a stream of operations that match the filter.
    async fn filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        self.inner_filter_operations(filter).await
    }

    async fn update_operation(
        &self,
        operation_id: OperationId,
        worker_id: Option<WorkerId>,
        action_stage: Result<ActionStage, Error>,
    ) -> Result<(), Error> {
        self.inner_update_operation(operation_id, worker_id, action_stage).await
    }

    /// Remove an operation from the state manager.
    /// It is important to use this function to remove operations
    /// that are no longer needed to prevent memory leaks.
    async fn remove_operation(&self, operation_id: OperationId) -> Result<(), Error> {
        self.inner_remove_operation(operation_id).await
    }
}
