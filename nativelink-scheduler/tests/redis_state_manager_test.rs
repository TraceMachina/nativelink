// Copyright 2023 The NativeLink Authors. All rights reserved.
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

use std::time::SystemTime;

use futures::StreamExt;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::operation_state_manager::{
    ClientStateManager, MatchingEngineStateManager, OperationFilter, OperationStageFlags,
};
use nativelink_scheduler::redis_operation_state::{RedisOperation, RedisStateManager};
use nativelink_store::redis_store::{LazyConnection, RedisStore};
use nativelink_util::action_messages::{ActionInfo, ActionStage, OperationId, WorkerId};
use nativelink_util::background_spawn;
use nativelink_util::common::DigestInfo;
use nativelink_util::platform_properties::PlatformProperties;
use redis::AsyncCommands;

// use std::ops::Deref;
// use nativelink_error::make_err;

mod utils {
    pub(crate) mod scheduler_utils;
}
use utils::scheduler_utils::make_base_action_info;

fn create_action_info(
    action_digest: DigestInfo,
    platform_properties: PlatformProperties,
    insert_timestamp: SystemTime,
) -> ActionInfo {
    let mut action_info = make_base_action_info(insert_timestamp);
    action_info.platform_properties = platform_properties;
    action_info.unique_qualifier.digest = action_digest;
    action_info
}

fn uuid_generator() -> String {
    uuid::Uuid::new_v4().to_string()
}

fn create_default_filter() -> OperationFilter {
    OperationFilter {
        stages: OperationStageFlags::Any,
        operation_id: None,
        worker_id: None,
        action_digest: None,
        worker_update_before: None,
        completed_before: None,
        last_client_update_before: None,
        unique_qualifier: None,
        order_by: None,
    }
}

#[cfg(test)]
mod redis_state_manager_tests {

    use pretty_assertions::assert_eq;

    use super::*; // Must be declared in every module.

    const WORKER_TIMEOUT_S: u64 = 100;

    #[nativelink_test]
    async fn basic_add_operation_test() -> Result<(), Error> {
        let client = redis::Client::open("redis://localhost/")?;
        let connection = redis::aio::ConnectionManager::new(client).await?;
        let store = RedisStore::new_with_conn_and_name_generator(
            LazyConnection::Connection(Ok(connection)),
            uuid_generator,
        );
        let mut state_manager = RedisStateManager::new(store.into());

        let action_info = create_action_info(
            DigestInfo::new([99u8; 32], 512),
            PlatformProperties::default(),
            SystemTime::now(),
        );
        let id = &state_manager
            .add_action(action_info)
            .await?
            .as_state()
            .await
            .unwrap()
            .id;
        let mut filter = create_default_filter();
        filter.operation_id = Some(id.clone());
        let mut stream =
            <RedisStateManager as ClientStateManager>::filter_operations(&state_manager, filter)
                .await?;
        assert_eq!(stream.next().await.is_some(), true);
        Ok(())
    }

    #[nativelink_test]
    async fn basic_remove_operation_test() -> Result<(), Error> {
        let client = redis::Client::open("redis://localhost/")?;
        let connection = redis::aio::ConnectionManager::new(client).await?;
        let store = RedisStore::new_with_conn_and_name_generator(
            LazyConnection::Connection(Ok(connection)),
            uuid_generator,
        );
        let mut state_manager = RedisStateManager::new(store.into());

        let action_info = create_action_info(
            DigestInfo::new([99u8; 32], 512),
            PlatformProperties::default(),
            SystemTime::now(),
        );
        let id = &state_manager
            .add_action(action_info)
            .await?
            .as_state()
            .await
            .unwrap()
            .id;
        let mut filter = create_default_filter();
        filter.operation_id = Some(id.clone());
        let mut stream =
            <RedisStateManager as ClientStateManager>::filter_operations(&state_manager, filter)
                .await?;
        let mut res = stream.next().await;
        while let Some(action_state_result) = res {
            let operation_id = &action_state_result.as_state().await.unwrap().id;
            state_manager.remove_operation(operation_id.clone()).await?;
            res = stream.next().await;
        }
        Ok(())
    }

    #[nativelink_test]
    async fn basic_update_operation_test() -> Result<(), Error> {
        let client = redis::Client::open("redis://localhost/")?;
        let connection = redis::aio::ConnectionManager::new(client).await?;
        let store = RedisStore::new_with_conn_and_name_generator(
            LazyConnection::Connection(Ok(connection)),
            uuid_generator,
        );
        let mut state_manager = RedisStateManager::new(store.into());
        // store.update_oneshot(digest, data)
        // store.get_part_unchunked(key, offset, length)

        let worker_id = WorkerId(uuid::Uuid::new_v4());
        let action_info = create_action_info(
            DigestInfo::new([99u8; 32], 512),
            PlatformProperties::default(),
            SystemTime::now(),
        );
        let action = state_manager.add_action(action_info.clone()).await?;
        let operation_id = &action.as_state().await.unwrap().id;
        <RedisStateManager as MatchingEngineStateManager>::update_operation(
            &state_manager,
            operation_id.clone(),
            Some(worker_id),
            Ok(ActionStage::Executing),
        )
        .await?;
        let mut filter = create_default_filter();
        filter.worker_id = Some(worker_id);
        filter.operation_id = Some(operation_id.clone());
        filter.stages = OperationStageFlags::Executing;
        let mut stream =
            <RedisStateManager as ClientStateManager>::filter_operations(&state_manager, filter)
                .await?;
        let res = stream.next().await;
        assert_eq!(res.is_some(), true);
        Ok(())
    }

    #[nativelink_test]
    async fn basic_filter_operation_test() -> Result<(), Error> {
        let client = redis::Client::open("redis://localhost/")?;
        let connection = redis::aio::ConnectionManager::new(client).await?;
        let store = RedisStore::new_with_conn_and_name_generator(
            LazyConnection::Connection(Ok(connection)),
            uuid_generator,
        );
        let mut con = store.get_conn().await?;
        let mut state_manager = RedisStateManager::new(store.into());
        let cmd = redis::cmd("FLUSHDB");
        cmd.query_async(&mut con).await?;

        let worker_id = WorkerId(uuid::Uuid::new_v4());
        let action_info = create_action_info(
            DigestInfo::new([99u8; 32], 512),
            PlatformProperties::default(),
            SystemTime::now(),
        );
        let action = state_manager.add_action(action_info).await?;
        let operation_id = &action.as_state().await.unwrap().id;
        <RedisStateManager as MatchingEngineStateManager>::update_operation(
            &state_manager,
            operation_id.clone(),
            Some(worker_id),
            Ok(ActionStage::Executing),
        )
        .await?;
        let mut filter = create_default_filter();
        filter.worker_id = Some(worker_id);
        filter.operation_id = Some(operation_id.clone());
        filter.stages = OperationStageFlags::Executing;
        let mut stream =
            <RedisStateManager as ClientStateManager>::filter_operations(&state_manager, filter)
                .await?;
        assert_eq!(stream.next().await.is_some(), true);
        Ok(())
    }

    #[nativelink_test]
    async fn basic_subscribe_test() -> Result<(), Error> {
        let client = redis::Client::open("redis://localhost/")?;
        let connection = redis::aio::ConnectionManager::new(client).await?;
        let store = RedisStore::new_with_conn_and_name_generator(
            LazyConnection::Connection(Ok(connection)),
            uuid_generator,
        );
        let mut con = store.get_conn().await?;
        let mut state_manager = RedisStateManager::new(store.into());

        let action_info = create_action_info(
            DigestInfo::new([99u8; 32], 512),
            PlatformProperties::default(),
            SystemTime::now(),
        );
        let action_state_result = state_manager.add_action(action_info).await?;
        let id = action_state_result.as_state().await?.id.clone();
        let mut recv = action_state_result.as_receiver().await.unwrap().clone();

        let notify_update_received = std::sync::Arc::new(tokio::sync::Notify::new());
        let notify_update_received_clone = notify_update_received.clone();
        let state: RedisOperation = con
            .get(format!(
                "operations:{}",
                action_state_result.as_state().await?.id
            ))
            .await?;
        println!("{:?}", state.action_stage());

        let _drop_guard = background_spawn!("subscription_change_watcher", async move {
            recv.changed().await.unwrap();
            println!("{:?}", recv.borrow_and_update());
            notify_update_received_clone.notify_one();
        });
        let stage = ActionStage::Executing;
        let _ = state_manager.update_operation(id, None, Ok(stage)).await;
        tokio::task::yield_now().await;
        let state: RedisOperation = con
            .get(format!(
                "operations:{}",
                action_state_result.as_state().await?.id
            ))
            .await?;
        println!("{:?}", state.action_stage());
        notify_update_received.notified().await;

        Ok(())
    }
}
