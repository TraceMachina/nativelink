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

use std::collections::HashMap;
use std::iter::zip;
use std::str::FromStr;
use std::sync::Arc;
use std::time::SystemTime;

use futures::{join, StreamExt};
use nativelink_error::{make_input_err, Error};
use nativelink_store::redis_store::RedisStore;
use nativelink_util::action_messages::{
    ActionInfo, ActionInfoHashKey, ActionResult, ActionStage, ActionState, OperationId, WorkerId,
};
use nativelink_util::background_spawn;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::store_trait::{StoreDriver, StoreLike, StoreSubscription};
use redis::aio::{ConnectionLike, ConnectionManager};
use redis::AsyncCommands;
use redis_macros::{FromRedisValue, ToRedisArgs};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;
use tonic::async_trait;

use crate::operation_state_manager::{
    ActionStateResult, ActionStateResultStream, ClientStateManager, OperationFilter,
    OperationStageFlags,
};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
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
                // If get fails then we drop the tx.
                let (res, data_res) = join!(
                    async move { item.get(&mut data_tx).await },
                    data_rx.consume(None)
                );
                if let Err(_e) = res {
                    todo!()
                }
                let Ok(data) = data_res else { todo!() };
                let slice = &data[..];
                let state: Arc<ActionState> = RedisOperation::from_slice(slice).as_state().unwrap();
                let _ = tx.send(state).map_err(|_| todo!());
            }
        });
        Self { rx, inner }
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

#[derive(Serialize, Deserialize, Clone, Debug, ToRedisArgs, FromRedisValue)]
pub struct RedisOperation {
    operation_id: OperationId,
    info: ActionInfo,
    worker_id: Option<WorkerId>,
    result: Option<ActionResult>,
    stage: OperationStage,
    last_worker_update: Option<SystemTime>,
    last_client_update: Option<SystemTime>,
    last_error: Option<Error>,
    completed_at: Option<SystemTime>,
}

impl RedisOperation {
    pub fn as_json(&self) -> String {
        serde_json::json!(&self).to_string()
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
            completed_at: None,
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
            completed_at: existing.completed_at,
        }
    }

    pub fn as_state(&self) -> Result<Arc<ActionState>, Error> {
        let action_state = ActionState {
            stage: self.action_stage()?,
            id: self.operation_id.clone(),
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
                    return Err(make_input_err!(
                        "Operation {} was marked as completed but has no result",
                        self.operation_id.to_string()
                    ));
                };
                Ok(ActionStage::Completed(result.clone()))
            }
        }
    }
    fn unique_qualifier(&self) -> &ActionInfoHashKey {
        &self.operation_id.unique_qualifier
    }
}

impl FromStr for RedisOperation {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        serde_json::from_str(s).map_err(|e| {
            make_input_err!(
                "Decode string to RedisOperation failed with error: {}",
                e.to_string()
            )
        })
    }
}

impl TryFrom<RedisOperation> for ActionState {
    type Error = Error;
    fn try_from(value: RedisOperation) -> Result<Self, Self::Error> {
        Ok(ActionState {
            id: value.operation_id.clone(),
            stage: value.action_stage()?,
        })
    }
}

fn match_optional_filter<T: PartialEq>(
    value_opt: Option<T>,
    filter_opt: Option<T>,
    cond: impl Fn(T, T) -> bool,
) -> bool {
    let Some(filter) = filter_opt else {
        return true;
    };
    let Some(value) = value_opt else {
        return false;
    };
    cond(filter, value)
}

pub fn matches_filter(operation: &RedisOperation, filter: &OperationFilter) -> bool {
    if !parse_stage_flags(&filter.stages).contains(&operation.stage)
        && !(filter.stages == OperationStageFlags::Any)
    {
        return false;
    }
    match_optional_filter(
        Some(&operation.operation_id),
        filter.operation_id.as_ref(),
        |a, b| a == b,
    ) && match_optional_filter(operation.worker_id, filter.worker_id, |a, b| a == b)
        && match_optional_filter(
            Some(operation.unique_qualifier().digest),
            filter.action_digest,
            |a, b| a == b,
        )
        && match_optional_filter(operation.completed_at, filter.completed_before, |a, b| {
            a < b
        })
        && match_optional_filter(
            operation.last_client_update,
            filter.last_client_update_before,
            |a, b| a < b,
        )
}
pub struct RedisStateManager<
    T: ConnectionLike + Unpin + Clone + Send + Sync + 'static = ConnectionManager,
> {
    pub store: Arc<RedisStore<T>>,
}

impl<T: ConnectionLike + Unpin + Clone + Send + Sync + 'static> RedisStateManager<T> {
    pub async fn get_conn(&self) -> Result<T, Error> {
        self.store.get_conn().await
    }

    async fn list<'a, V>(
        &self,
        prefix: &str,
        result_map: &mut HashMap<String, V>,
    ) -> Result<(), Error>
    where
        V: FromStr,
    {
        let mut con = self.get_conn().await?;
        let ids_iter = con.scan_match::<&str, String>(prefix).await?;
        let keys = ids_iter.collect::<Vec<String>>().await;
        let raw_values: Vec<String> = con.get(&keys).await?;

        let value_res: Result<Vec<V>, Error> = raw_values
            .into_iter()
            .map(|s| {
                V::from_str(&s)
                    .map_err(|_| make_input_err!("list: Failed to convert value to type"))
            })
            .collect();
        match value_res {
            Ok(values) => {
                let zipped = zip(keys.into_iter(), values.into_iter());
                *result_map = HashMap::from_iter(zipped);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    async fn inner_add_action(
        &self,
        action_info: ActionInfo,
    ) -> Result<Arc<dyn ActionStateResult>, Error> {
        let operation_id = OperationId::new(action_info.unique_qualifier.clone());
        let mut con = self.get_conn().await?;
        let hash_key = operation_id.unique_qualifier.action_name().clone();

        let action_key = format!("actions:{}", hash_key.clone());
        // TODO: List API call to find existing actions.
        let mut existing_operations: Vec<String> = Vec::new();
        let operation = match existing_operations.pop() {
            Some(existing_operation) => {
                let operation: RedisOperation = con
                    .get(format!("operations:{}", &existing_operation))
                    .await?;
                RedisOperation::from_existing(operation.clone(), operation_id.clone())
            }
            None => RedisOperation::new(action_info, operation_id.clone()),
        };

        let operation_key = format!("operations:{operation_id}").to_string();
        let store = self.store.as_store_driver_pin();
        store
            .update_oneshot(operation_key.into(), operation.as_json().into())
            .await?;
        store
            .update_oneshot(action_key.into(), operation_id.to_string().into())
            .await?;
        let store_subscription = self
            .store
            .clone()
            .subscribe(format!("operations:{operation_id}").into())
            .await;
        let state = RedisOperationState::new(operation, store_subscription);
        Ok(Arc::new(state))
    }

    async fn inner_filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        let mut existing_operations: HashMap<String, RedisOperation> = HashMap::new();
        self.list("operations:*", &mut existing_operations).await?;
        let mut v: Vec<Arc<dyn ActionStateResult>> = Vec::new();
        for operation in existing_operations.values() {
            if matches_filter(operation, &filter) {
                let store_subscription = self
                    .store
                    .clone()
                    .subscribe(format!("operations:{}", operation.operation_id).into())
                    .await;
                v.push(Arc::new(RedisOperationState::new(
                    operation.clone(),
                    store_subscription,
                )));
            }
        }
        Ok(Box::pin(futures::stream::iter(v)))
    }
}

#[async_trait]
impl ClientStateManager for RedisStateManager {
    async fn add_action(
        &mut self,
        action_info: ActionInfo,
    ) -> Result<Arc<dyn ActionStateResult>, Error> {
        self.inner_add_action(action_info).await
    }

    async fn filter_operations(
        &self,
        filter: OperationFilter,
    ) -> Result<ActionStateResultStream, Error> {
        self.inner_filter_operations(filter).await
    }
}
