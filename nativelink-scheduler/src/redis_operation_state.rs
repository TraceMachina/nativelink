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

use std::iter::zip;
use std::sync::Arc;
use std::time::SystemTime;
use futures::{join, StreamExt};
use nativelink_store::redis_store::RedisStore;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::background_spawn;
use std::collections::HashMap;
use tonic::async_trait;
use nativelink_util::action_messages::{ActionInfo, ActionInfoHashKey, ActionResult, ActionStage, ActionState, OperationId, WorkerId};
use redis::aio::{ConnectionLike, ConnectionManager};
use redis::{AsyncCommands, Pipeline};
use nativelink_error::{make_input_err, Error};
use tokio::sync::watch;
use redis_macros::{FromRedisValue, ToRedisArgs};
use serde::{Serialize, Deserialize};
use crate::operation_state_manager::{ActionStateResult, ActionStateResultStream, ClientStateManager, MatchingEngineStateManager, OperationFilter, OperationStageFlags, WorkerStateManager};
use nativelink_util::store_trait::{StoreDriver, StoreLike, StoreSubscription};


fn match_optional_filter<T: PartialEq>(value_opt: Option<T>, filter_opt: Option<T>, cond: impl Fn(T, T) -> bool) -> bool {
    let Some(filter) = filter_opt else { return true };
    let Some(value) = value_opt else { return false; };
    cond(filter, value)
}

pub fn matches_filter(operation: &RedisOperation, filter: &OperationFilter) -> bool {
    if !parse_stage_flags(&filter.stages).contains(&operation.stage) && !(filter.stages == OperationStageFlags::Any) {
        return false
    }
    match_optional_filter(Some(&operation.operation_id), filter.operation_id.as_ref(), |a, b| { a == b })
        && match_optional_filter(operation.worker_id, filter.worker_id, |a, b| { a == b })
        && match_optional_filter(Some(operation.unique_qualifier().digest), filter.action_digest, |a, b| { a == b })
        && match_optional_filter(operation.completed_at, filter.completed_before, |a, b| { a < b })
        && match_optional_filter(operation.last_client_update, filter.last_client_update_before, |a, b| { a < b })
}

pub struct RedisStateManager<T: ConnectionLike + Unpin + Clone + Send + Sync + 'static = ConnectionManager> {
    pub store: Arc<RedisStore<T>>
}

impl<T: ConnectionLike + Unpin + Clone + Send + Sync + 'static> RedisStateManager<T> {
    pub fn new(store: Arc<RedisStore<T>>) -> Self {
        Self { store }
    }

    async fn get_operations(&self) -> Result<HashMap<String, RedisOperation>, Error> {
        let mut con = self.get_conn().await?;
        let ids_iter = con.scan_match::<&str, String>("operations:*").await?;
        let mut pipe = Pipeline::new();
        let ids: Vec<String> = ids_iter.map(|v| {
            v.split("operations:").last().unwrap().to_string()
        }).collect().await;

        for id in ids.as_slice() {
            pipe.get(format!("operations:{}", id));
        }
        let str_values: Vec<String> = pipe.query_async(&mut con).await?;
        let values = str_values.iter().map(|s| {
            RedisOperation::from_str(s)
        });
        let pairs = zip(ids, values);
        Ok(HashMap::from_iter(pairs))
    }

    async fn get_actions(&self) -> Result<HashMap<String, RedisOperation>, Error> {
        let mut con = self.get_conn().await?;
        let ids_iter = con.scan_match::<&str, String>("actions:*").await?;
        let mut pipe = Pipeline::new();
        let ids: Vec<String> = ids_iter.map(|v| {
            v.split("actions:").last().unwrap().to_string()
        }).collect().await;

        for id in ids.as_slice() {
            pipe.get(format!("actions:{}", id));
        }
        let str_values: Vec<String> = pipe.query_async(&mut con).await?;
        let values = str_values.iter().map(|s| {
            RedisOperation::from_str(s)
        });
        let pairs = zip(ids, values);
        Ok(HashMap::from_iter(pairs))
    }

    pub async fn get_conn(&self) -> Result<T, Error> {
        self.store.get_conn().await
    }

    async fn inner_add_action(
        &self,
        action_info: ActionInfo,
    ) -> Result<Box<Arc<dyn ActionStateResult>>, Error> {
        let operation_id = OperationId::new(action_info.unique_qualifier.clone());
        let mut con = self.get_conn().await?;
        let hash_key = operation_id.unique_qualifier.action_name().clone();

        let action_key = format!("actions:{}", hash_key.clone());
        // TODO: List API call to find existing actions.
        let mut existing_operations: Vec<String> = Vec::new();
        let operation = match existing_operations.pop() {
            Some(existing_operation) => {
                let operation: RedisOperation = con.get(format!("operations:{}", &existing_operation)).await?;
                RedisOperation::from_existing(operation.clone(), operation_id.clone())
            },
            None => {
                RedisOperation::new(action_info, operation_id.clone())
            }
        };

        let operation_key = format!("operations:{}", operation_id).to_string();
        let store = self.store.as_store_driver_pin();
        store
            .update_oneshot(operation_key.into(), operation.as_json().into())
            .await?;
        store
            .update_oneshot(action_key.into(), operation_id.to_string().into())
            .await?;
        let store_subscription = self.store.clone().subscribe(format!("operations:{}", operation_id).into()).await;
        let state = RedisOperationState::new(operation, store_subscription);
        Ok(Box::new(Arc::new(state)))
    }

    async fn inner_filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        let existing_operations: HashMap<String, RedisOperation> = self.get_operations().await?;
        let mut v: Vec<Arc<dyn ActionStateResult>> = Vec::new();
        for operation in existing_operations.values() {
            if matches_filter(operation, &filter) {

                let store_subscription = self.store.clone().subscribe(format!("operations:{}", operation.operation_id).into()).await;
                v.push(Arc::new(
                        RedisOperationState::new(
                            operation.clone(), store_subscription
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
        let mut con = self.get_conn().await?;
        let store = self.store.as_store_driver_pin();
        let key = format!("operations:{}", operation_id);
        let operation_bytes_res = &store.get_part_unchunked(
            key.clone().into(),
            0,
            None
        ).await;
        let Ok(operation_bytes) = operation_bytes_res else {
            return Err(make_input_err!("Received request to update operation {operation_id}, but operation does not exist."))
        };
        let mut operation: RedisOperation = RedisOperation::from_slice(&operation_bytes[..]);

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
        store.update_oneshot(
            key.into(),
            operation.as_json().into()
        ).await
    }

    async fn inner_remove_operation(&self, operation_id: OperationId) -> Result<(), Error> {
        let mut con = self.get_conn().await?;
        let mut pipe = Pipeline::new();
        Ok(pipe
            .del(format!("operations:{}", operation_id))
            .query_async(&mut con)
            .await?)
    }
}

#[async_trait]
impl ClientStateManager for RedisStateManager {
    async fn add_action(
        &mut self,
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

pub struct RedisOperationState {
    rx: watch::Receiver<Arc<ActionState>>,
    inner: RedisOperation,
}

impl RedisOperationState {
    fn new(inner: RedisOperation, mut subscription: Box<dyn StoreSubscription>) -> Self {
        let (tx, rx) = watch::channel(inner.as_state().unwrap());

        let _join_handle = background_spawn!("redis_subscription_watcher", async move {
            loop {
                let Ok(item) = subscription.changed().await else {
                    return;
                };
                let (mut data_tx, mut data_rx) = make_buf_channel_pair();
                // if get fails, drop the tx
                let (res, data_res) = join!(async move { item.get(&mut data_tx).await }, data_rx.consume(None));
                if let Err(_e) = res {
                    todo!()
                }
                let Ok(data) = data_res else {
                    todo!()
                };
                let slice = &data[..];
                let state: Arc<ActionState> = RedisOperation::from_slice(slice).as_state().unwrap();
                let _ = tx.send(state).map_err(|_| {
                    todo!()
                });

            }
        });
        Self {
            rx,
            inner
        }
    }
}

impl TryFrom<RedisOperation> for ActionState {
    type Error = Error;
    fn try_from(value: RedisOperation) -> Result<Self, Self::Error> {
        Ok(ActionState {
            id: value.operation_id.clone(),
            stage: value.action_stage()?
        })
    }
}

#[async_trait]
impl ActionStateResult for RedisOperationState {
    async fn as_state(&self) -> Result<Arc<ActionState>, Error> {
        Ok(Arc::new(self.inner.clone().try_into()?))
    }
    async fn as_receiver(&self) -> Result<&'_ watch::Receiver<Arc<ActionState>>, Error> {
        Ok(&self.rx)
    }
    async fn as_action_info(&self) -> Result<Arc<ActionInfo>, Error> {
        Ok(Arc::new(self.inner.info.clone()))
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

impl RedisOperation {
    pub fn as_json(&self) -> String {
        serde_json::json!(&self).to_string()
    }

    pub fn from_str(s: &str) -> Self {
        serde_json::from_str(s).unwrap()
    }
    pub fn from_slice(s: &[u8]) -> Self {
        serde_json::from_slice(s).unwrap()
    }

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

    pub fn from_existing(existing: RedisOperation, operation_id: OperationId) -> Self {
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
