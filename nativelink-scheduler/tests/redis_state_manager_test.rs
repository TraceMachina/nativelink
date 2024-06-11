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

use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::operation_state_manager::{
    ClientStateManager, MatchingEngineStateManager, OperationFilter, OperationStageFlags,
};
use nativelink_scheduler::redis_operation_state::RedisStateManager;
use nativelink_util::action_messages::{ActionInfo, ActionStage, WorkerId};
use nativelink_util::common::DigestInfo;
use nativelink_util::platform_properties::PlatformProperties;

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
    use futures::StreamExt;
    use pretty_assertions::assert_eq;

    use super::*; // Must be declared in every module.

    const WORKER_TIMEOUT_S: u64 = 100;

    #[nativelink_test]
    async fn basic_add_operation_test() -> Result<(), Error> {
        let state_manager = RedisStateManager::new("redis://localhost/".to_string());
        let client = state_manager.get_client();
        let mut con = client.get_multiplexed_async_connection().await?;
        let cmd = redis::cmd("FLUSHDB");
        cmd.query_async(&mut con).await?;
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
        let state_manager = RedisStateManager::new("redis://localhost/".to_string());
        let client = state_manager.get_client();
        let mut con = client.get_multiplexed_async_connection().await?;
        let cmd = redis::cmd("FLUSHDB");
        cmd.query_async(&mut con).await?;
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
        let worker_id = WorkerId(uuid::Uuid::new_v4());
        let state_manager = RedisStateManager::new("redis://localhost/".to_string());
        let client = state_manager.get_client();
        let mut con = client.get_multiplexed_async_connection().await?;
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
        assert_eq!(stream.next().await.is_some(), true);
        Ok(())
    }

    #[nativelink_test]
    async fn basic_filter_operation_test() -> Result<(), Error> {
        let worker_id = WorkerId(uuid::Uuid::new_v4());
        let state_manager = RedisStateManager::new("redis://localhost/".to_string());
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
}
