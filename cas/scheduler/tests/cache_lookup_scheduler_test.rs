// Copyright 2023 The Turbo Cache Authors. All rights reserved.
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
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use prost::Message;
use tokio::{self, join, sync::watch};

use action_messages::{ActionInfo, ActionInfoHashKey, ActionResult, ActionStage, ActionState, DirectoryInfo};
use cache_lookup_scheduler::CacheLookupScheduler;
use common::DigestInfo;
use config;
use error::Error;
use memory_store::MemoryStore;
use mock_scheduler::MockActionScheduler;
use platform_property_manager::{PlatformProperties, PlatformPropertyManager};
use proto::build::bazel::remote::execution::v2::ActionResult as ProtoActionResult;
use scheduler::ActionScheduler;
use store::Store;

const INSTANCE_NAME: &str = "foobar_instance_name";

fn make_base_action_info() -> ActionInfo {
    ActionInfo {
        instance_name: INSTANCE_NAME.to_string(),
        command_digest: DigestInfo::new([0u8; 32], 0),
        input_root_digest: DigestInfo::new([0u8; 32], 0),
        timeout: Duration::MAX,
        platform_properties: PlatformProperties {
            properties: HashMap::new(),
        },
        priority: 0,
        load_timestamp: UNIX_EPOCH,
        insert_timestamp: UNIX_EPOCH,
        unique_qualifier: ActionInfoHashKey {
            digest: DigestInfo::new([0u8; 32], 0),
            salt: 0,
        },
        skip_cache_lookup: false,
    }
}

fn make_cache_scheduler() -> Result<
    (
        Arc<MockActionScheduler>,
        Arc<dyn Store>,
        Arc<dyn Store>,
        CacheLookupScheduler,
    ),
    Error,
> {
    let mock_scheduler = Arc::new(MockActionScheduler::new());
    let cas_store = Arc::new(MemoryStore::new(&config::stores::MemoryStore::default()));
    let ac_store = Arc::new(MemoryStore::new(&config::stores::MemoryStore::default()));
    let cache_scheduler = CacheLookupScheduler::new(cas_store.clone(), ac_store.clone(), mock_scheduler.clone())?;
    Ok((mock_scheduler, cas_store, ac_store, cache_scheduler))
}

#[cfg(test)]
mod cache_lookup_scheduler_tests {
    use super::*;
    use pretty_assertions::assert_eq; // Must be declared in every module.

    #[tokio::test]
    async fn platform_property_manager_call_passed() -> Result<(), Error> {
        let (mock_scheduler, _, _, cache_scheduler) = make_cache_scheduler()?;
        let platform_property_manager = Arc::new(PlatformPropertyManager::new(HashMap::new()));
        let instance_name = INSTANCE_NAME.to_string();
        let (actual_manager, actual_instance_name) = join!(
            cache_scheduler.get_platform_property_manager(&instance_name),
            mock_scheduler.expect_get_platform_property_manager(Ok(platform_property_manager.clone())),
        );
        assert_eq!(Arc::as_ptr(&platform_property_manager), Arc::as_ptr(&actual_manager?));
        assert_eq!(instance_name, actual_instance_name);
        Ok(())
    }

    #[tokio::test]
    async fn add_action_does_cache_lookup() -> Result<(), Error> {
        let (_, _, ac_store, cache_scheduler) = make_cache_scheduler()?;
        let action_info = make_base_action_info();
        let action_result = ProtoActionResult::from(ActionResult::default());
        let store_pin = Pin::new(ac_store.as_ref());
        store_pin
            .update_oneshot(
                action_info.unique_qualifier.digest,
                action_result.encode_to_vec().into(),
            )
            .await?;
        let watch_channel = cache_scheduler.add_action(action_info.clone()).await?;
        let cached_action_state = watch_channel.borrow();
        let ActionStage::CompletedFromCache(proto_action_result) = cached_action_state.stage.clone() else {
            panic!("Did not complete from cache");
        };
        assert_eq!(action_info.unique_qualifier.digest, cached_action_state.action_digest);
        assert_eq!(action_result, proto_action_result);
        Ok(())
    }

    #[tokio::test]
    async fn add_action_validates_outputs() -> Result<(), Error> {
        let (mock_scheduler, _, ac_store, cache_scheduler) = make_cache_scheduler()?;
        let action_info = make_base_action_info();
        let mut action_result = ActionResult::default();
        action_result.output_folders.push(DirectoryInfo {
            path: "".to_string(),
            tree_digest: DigestInfo {
                size_bytes: 1,
                packed_hash: [8; 32],
            },
        });
        let action_result = ProtoActionResult::from(action_result);
        let store_pin = Pin::new(ac_store.as_ref());
        store_pin
            .update_oneshot(
                action_info.unique_qualifier.digest,
                action_result.encode_to_vec().into(),
            )
            .await?;
        let (_forward_watch_channel_tx, forward_watch_channel_rx) = watch::channel(Arc::new(ActionState {
            name: "".to_string(),
            action_digest: action_info.unique_qualifier.digest,
            stage: ActionStage::Queued,
        }));
        let _ = join!(
            cache_scheduler.add_action(action_info),
            mock_scheduler.expect_add_action(Ok(forward_watch_channel_rx))
        );
        Ok(())
    }

    #[tokio::test]
    async fn add_action_handles_skip_cache() -> Result<(), Error> {
        let (mock_scheduler, _, ac_store, cache_scheduler) = make_cache_scheduler()?;
        let action_info = make_base_action_info();
        let action_result = ProtoActionResult::from(ActionResult::default());
        let store_pin = Pin::new(ac_store.as_ref());
        store_pin
            .update_oneshot(
                action_info.unique_qualifier.digest,
                action_result.encode_to_vec().into(),
            )
            .await?;
        let (_forward_watch_channel_tx, forward_watch_channel_rx) = watch::channel(Arc::new(ActionState {
            name: "".to_string(),
            action_digest: action_info.unique_qualifier.digest,
            stage: ActionStage::Queued,
        }));
        let mut skip_cache_action = action_info.clone();
        skip_cache_action.skip_cache_lookup = true;
        let _ = join!(
            cache_scheduler.add_action(skip_cache_action),
            mock_scheduler.expect_add_action(Ok(forward_watch_channel_rx))
        );
        Ok(())
    }
}
