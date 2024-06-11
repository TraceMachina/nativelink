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

use std::sync::Arc;
use std::time::SystemTime;
use futures::StreamExt;
use std::collections::HashMap;
use tonic::async_trait;
use nativelink_util::action_messages::{ActionInfo, ActionInfoHashKey, ActionResult, ActionStage, ActionState, OperationId, WorkerId};
use redis::aio::{MultiplexedConnection, PubSub};
use redis::{AsyncCommands, Client, Connection, Pipeline};
use nativelink_error::{make_input_err, Error};
use tokio::sync::watch;
use redis_macros::{FromRedisValue, ToRedisArgs};
use serde::{Serialize, Deserialize};
use crate::operation_state_manager::{ActionStateResult, ActionStateResultStream, ClientStateManager, MatchingEngineStateManager, OperationFilter, OperationStageFlags, WorkerStateManager};

pub struct RedisOperationState {
    client: Client,
    inner: RedisOperationImpl,
}

impl RedisOperationState {
    fn new(con: Client, inner: RedisOperationImpl) -> Self {
        Self {
            client: con,
            inner
        }
    }
}

impl TryFrom<RedisOperationImpl> for ActionState {
    type Error = Error;
    fn try_from(value: RedisOperationImpl) -> Result<Self, Self::Error> {
        Ok(ActionState {
            id: value.operation_id.clone(),
            stage: value.action_stage()?
        })
    }
}

impl RedisOperationState {
    async fn subscribe<'a>(
        &'a self,
        client: &'a Client
    ) -> Result<watch::Receiver<Arc<ActionState>>, nativelink_error::Error> {
        let mut sub = client.get_async_pubsub().await?;
        let sub_channel = format!("{}:*", &self.inner.operation_id.unique_qualifier.action_name());
        // Subscribe to action name: any completed operation can return status
        sub.subscribe(sub_channel).await.unwrap();
        let mut stream = sub.into_on_message();
        let action_state: ActionState = self.inner.clone().try_into()?;
        // let arc_action_state: Arc<ActionState> = Arc::new();
        // This hangs forever atm
        let (tx, rx) = tokio::sync::watch::channel(Arc::new(action_state));
        // Hand tuple of rx and future to pump the rx
        // Note: nativelink spawn macro name field doesn't accept variables so for now we have to use this to avoid conflicts.
        #[allow(clippy::disallowed_methods)]
        tokio::spawn(async move {
            let closed_fut = tx.closed();
            tokio::pin!(closed_fut);
            loop {
                tokio::select! {
                    msg = stream.next() => {
                        println!("got message");
                        let state: RedisOperationImpl = msg.unwrap().get_payload().unwrap();
                        let finished = state.stage == OperationStage::Completed;
                        let value: Arc<ActionState> = Arc::new(state.try_into().unwrap());
                        if tx.send(value).is_err() {
                            println!("Error sending value");
                            return;
                        }
                        if finished {
                            return;
                        }
                    }
                    _  = &mut closed_fut => {
                        println!("Future closed");
                        return
                    }
                }

            }
        });
        Ok(rx)
    }
}

#[async_trait]
impl ActionStateResult for RedisOperationState {
    async fn as_state(&self) -> Result<Arc<ActionState>, Error> {
        Ok(Arc::new(self.inner.clone().try_into()?))
    }
    async fn as_receiver(&self) -> Result<watch::Receiver<Arc<ActionState>>, Error> {
        Ok(self.subscribe(&self.client).await?)
    }
    async fn as_action_info(&self) -> Result<Arc<ActionInfo>, Error> {
        Ok(Arc::new(self.inner.info.clone()))
    }
}
pub struct RedisStateManager  {
    pub client: Client,
}

impl RedisStateManager {
    pub fn new(url: String) -> Self {
        Self { client: Client::open(url).unwrap() }
    }

    pub fn get_client(&self) -> Client {
        self.client.clone()
    }

    async fn _get_async_pubsub(&self) -> Result<PubSub, Error> {
        Ok(self.client.get_async_pubsub().await?)
    }

    async fn get_multiplex_connection(&self) -> Result<MultiplexedConnection, Error> {
        Ok(self.client.get_multiplexed_tokio_connection().await?)
    }

    fn _get_connection(&self) -> Result<Connection, Error> {
        Ok(self.client.get_connection()?)
    }

    async fn inner_add_action(
        &self,
        action_info: ActionInfo,
    ) -> Result<Box<Arc<dyn ActionStateResult>>, Error> {
        let operation_id = OperationId::new(action_info.unique_qualifier.clone());
        let mut con = self.get_multiplex_connection().await?;
        let hash_key = operation_id.unique_qualifier.action_name().clone();
        let action_key = format!("actions:{}", hash_key.clone());
        let mut existing_operations: Vec<String> = con.smembers(&action_key).await?;

        let operation = match existing_operations.pop() {
            Some(existing_operation) => {
                let operation: RedisOperationImpl = con.hget("operations:", &existing_operation).await?;
                RedisOperationImpl::from_existing(operation.clone(), operation_id.clone())
            },
            None => {
                RedisOperationImpl::new(action_info, operation_id.clone())
            }
        };

        let mut pipe = Pipeline::new();
        pipe
            .hset("operations:", operation_id.to_string(), &operation)
            .sadd(action_key, &operation_id.to_string())
            .query_async(&mut con)
            .await?;
        let state = RedisOperationState::new(self.client.clone(), operation);
        Ok(Box::new(Arc::new(state)))
    }

    async fn inner_filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        let mut con = self.get_multiplex_connection().await?;
        let existing_operations: HashMap<String, RedisOperationImpl> = con.hgetall("operations:").await?;
        let mut v: Vec<Arc<dyn ActionStateResult>> = Vec::new();
        for operation in existing_operations.values() {
            if matches_filter(operation, &filter) {
                v.push(Arc::new(
                        RedisOperationState::new(
                            self.client.clone(), operation.clone()
                        )
                    )
                );
            }
        }
        Ok(Box::pin(futures::stream::iter(v)))
    }

    async fn inner_update_operation(
        &self,
        operation_id: OperationId,
        worker_id: Option<WorkerId>,
        action_stage: Result<ActionStage, Error>,
    ) -> Result<(), Error> {
        let mut con = self.get_multiplex_connection().await?;
        let maybe_operation: Option<RedisOperationImpl> = con
            .hget("operations:", operation_id.to_string())
            .await?;
        let Some(mut operation) = maybe_operation else {
            return Err(make_input_err!("Received request to update operation {operation_id}, but operation does not exist."))
        };
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
                    operation.stage = operation_stage;
                }
                operation.result = maybe_result;
                operation.worker_id = worker_id;
            },
            Err(e) => { operation.last_error = Some(e); }
        }
        Ok(con.hset("operations:", operation_id.to_string(), operation).await?)
    }

    async fn inner_remove_operation(&self, operation_id: OperationId) -> Result<(), Error> {
        let mut con = self.get_multiplex_connection().await.unwrap();
        let action_key = format!("actions:{}", operation_id.action_name());
        let mut pipe = Pipeline::new();
        Ok(pipe
            .srem(action_key, operation_id.to_string())
            .hdel("operations:", operation_id.to_string())
            .query_async(&mut con)
            .await?)
    }
}

#[derive(PartialEq, Eq, Serialize, Deserialize, Clone)]
enum OperationStage {
    CacheCheck,
    Queued,
    Executing,
    Completed,
    Unknown,
}

fn parse_stage_flags(flags: &OperationStageFlags) -> Vec<OperationStage> {
    if flags.contains(OperationStageFlags::Any) {
        return Vec::from([
            OperationStage::CacheCheck,
            OperationStage::Queued,
            OperationStage::Executing,
            OperationStage::Completed,
            OperationStage::Unknown,
        ]);
    }
    let mut stage_vec = Vec::new();
    if flags.contains(OperationStageFlags::CacheCheck) {
        stage_vec.push(OperationStage::CacheCheck)
    }
    if flags.contains(OperationStageFlags::Executing) {
        stage_vec.push(OperationStage::Executing)
    }
    if flags.contains(OperationStageFlags::Completed) {
        stage_vec.push(OperationStage::Completed)
    }
    if flags.contains(OperationStageFlags::Queued) {
        stage_vec.push(OperationStage::Queued)
    }
    stage_vec
}


#[derive(Serialize, Deserialize, Clone, ToRedisArgs, FromRedisValue)]
pub struct RedisOperationImpl {
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

impl RedisOperationImpl {
    pub fn new(info: ActionInfo, operation_id: OperationId) -> Self {
        Self {
            operation_id,
            info,
            worker_id: None,
            result: None,
            stage: OperationStage::CacheCheck,
            last_worker_update: None,
            last_client_update: None,
            last_error: None,
            completed_at: None
        }
    }

    pub fn from_existing(existing: RedisOperationImpl, operation_id: OperationId) -> Self {
        Self {
            operation_id,
            info: existing.info,
            worker_id: existing.worker_id,
            result: existing.result,
            stage: existing.stage,
            last_worker_update: existing.last_worker_update,
            last_client_update: existing.last_client_update,
            last_error: existing.last_error,
            completed_at: existing.completed_at
        }
    }

    pub fn as_state(&self) -> Result<Arc<ActionState>, Error> {
        let action_state = ActionState {
            stage: self.action_stage()?,
            id: self.operation_id.clone()
        };
        Ok(Arc::new(action_state))
    }

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
    fn unique_qualifier(&self) -> &ActionInfoHashKey {
        &self.operation_id.unique_qualifier
    }
}

fn match_optional_filter<T: PartialEq>(value_opt: Option<T>, filter_opt: Option<T>, cond: impl Fn(T, T) -> bool) -> bool {
    let Some(filter) = filter_opt else { return true };
    let Some(value) = value_opt else { return false; };
    cond(filter, value)
}

pub fn matches_filter(operation: &RedisOperationImpl, filter: &OperationFilter) -> bool {
    if !parse_stage_flags(&filter.stages).contains(&operation.stage) && !(filter.stages == OperationStageFlags::Any) {
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
    async fn add_action(
        &self,
        action_info: ActionInfo,
    ) -> Result<Box<Arc<dyn ActionStateResult>>, Error> {
        self.inner_add_action(action_info).await
    }

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

    async fn remove_operation(&self, operation_id: OperationId) -> Result<(), Error> {
        self.inner_remove_operation(operation_id).await
    }
}
