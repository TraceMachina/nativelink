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
use std::time::Duration;

use nativelink_config::stores::EvictionPolicy;
use nativelink_error::Error;
use nativelink_scheduler::awaited_action_db::AwaitedActionDb;
use nativelink_scheduler::memory_awaited_action_db::MemoryAwaitedActionDb;
use nativelink_util::action_messages::{
    ActionInfo, ActionUniqueKey, ActionUniqueQualifier, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::instant_wrapper::{InstantWrapper, MockInstantWrapped};
use tokio::sync::Notify;

/// Helper function to create a test action info with unique qualifier
fn make_test_action_info(digest_str: &str, cacheable: bool) -> ActionInfo {
    let digest = DigestInfo::try_new(digest_str, 32).unwrap();
    let unique_key = ActionUniqueKey {
        instance_name: "test".to_string(),
        digest_function: DigestHasherFunc::Sha256,
        digest: DigestInfo::try_new(digest_str, 32).unwrap(),
    };

    ActionInfo {
        command_digest: digest.clone(),
        input_root_digest: digest,
        timeout: Duration::from_secs(300),
        platform_properties: Default::default(),
        priority: 0,
        load_timestamp: MockInstantWrapped::default().now(),
        insert_timestamp: MockInstantWrapped::default().now(),
        unique_qualifier: if cacheable {
            ActionUniqueQualifier::Cacheable(unique_key)
        } else {
            ActionUniqueQualifier::Uncacheable(unique_key)
        },
    }
}

/// Test that validate_and_recover works correctly when database is consistent
#[tokio::test]
async fn test_validate_and_recover_consistent_database() -> Result<(), Error> {
    let eviction_config = EvictionPolicy {
        max_count: 100,
        max_bytes: 1024 * 1024,
        ..Default::default()
    };

    let tasks_change_notify = Arc::new(Notify::new());
    let now_fn = MockInstantWrapped::default;

    let db = MemoryAwaitedActionDb::new(&eviction_config, tasks_change_notify, now_fn);

    // Add a test action
    let action_info = make_test_action_info(
        "abcd1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab",
        true,
    );
    let operation_id = OperationId::default();

    let _subscriber = db
        .add_action(operation_id.clone(), Arc::new(action_info))
        .await?;

    // Validation should pass on a consistent database
    let result = db.validate_and_recover().await;
    assert!(
        result.is_ok(),
        "Validation should pass for consistent database"
    );

    Ok(())
}

/// Test that the database can recover from simulated inconsistency
/// Note: This is a conceptual test - in practice we can't easily induce
/// inconsistency from the public API without internal access
#[tokio::test]
async fn test_basic_functionality_with_validation() -> Result<(), Error> {
    let eviction_config = EvictionPolicy {
        max_count: 100,
        max_bytes: 1024 * 1024,
        ..Default::default()
    };

    let tasks_change_notify = Arc::new(Notify::new());
    let now_fn = MockInstantWrapped::default;

    let db = MemoryAwaitedActionDb::new(&eviction_config, tasks_change_notify, now_fn);

    // Test adding multiple cacheable actions
    let action_info1 = make_test_action_info(
        "abcd1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab",
        true,
    );
    let action_info2 = make_test_action_info(
        "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        true,
    );
    let operation_id1 = OperationId::default();
    let operation_id2 = OperationId::default();

    let _subscriber1 = db
        .add_action(operation_id1.clone(), Arc::new(action_info1))
        .await?;
    let _subscriber2 = db
        .add_action(operation_id2.clone(), Arc::new(action_info2))
        .await?;

    // Validate consistency after adding actions
    db.validate_and_recover().await?;

    // Check that we can retrieve the actions
    let retrieved1 = db.get_awaited_action_by_id(&operation_id1).await?;
    let retrieved2 = db.get_awaited_action_by_id(&operation_id2).await?;

    assert!(
        retrieved1.is_some(),
        "Should be able to retrieve first action"
    );
    assert!(
        retrieved2.is_some(),
        "Should be able to retrieve second action"
    );

    // Validate consistency again
    db.validate_and_recover().await?;

    Ok(())
}

/// Test that uncacheable actions don't interfere with validation
#[tokio::test]
async fn test_uncacheable_actions_validation() -> Result<(), Error> {
    let eviction_config = EvictionPolicy {
        max_count: 100,
        max_bytes: 1024 * 1024,
        ..Default::default()
    };

    let tasks_change_notify = Arc::new(Notify::new());
    let now_fn = MockInstantWrapped::default;

    let db = MemoryAwaitedActionDb::new(&eviction_config, tasks_change_notify, now_fn);

    // Add both cacheable and uncacheable actions
    let cacheable_action = make_test_action_info(
        "abcd1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab",
        true,
    );
    let uncacheable_action = make_test_action_info(
        "1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef",
        false,
    );

    let operation_id1 = OperationId::default();
    let operation_id2 = OperationId::default();

    let _subscriber1 = db
        .add_action(operation_id1.clone(), Arc::new(cacheable_action))
        .await?;
    let _subscriber2 = db
        .add_action(operation_id2.clone(), Arc::new(uncacheable_action))
        .await?;

    // Validation should pass with mixed action types
    db.validate_and_recover().await?;

    // Check that both actions exist
    let retrieved1 = db.get_awaited_action_by_id(&operation_id1).await?;
    let retrieved2 = db.get_awaited_action_by_id(&operation_id2).await?;

    assert!(retrieved1.is_some(), "Cacheable action should exist");
    assert!(retrieved2.is_some(), "Uncacheable action should exist");

    Ok(())
}

/// Test duplicate cacheable action handling with validation
#[tokio::test]
async fn test_duplicate_cacheable_actions_with_validation() -> Result<(), Error> {
    let eviction_config = EvictionPolicy {
        max_count: 100,
        max_bytes: 1024 * 1024,
        ..Default::default()
    };

    let tasks_change_notify = Arc::new(Notify::new());
    let now_fn = MockInstantWrapped::default;

    let db = MemoryAwaitedActionDb::new(&eviction_config, tasks_change_notify, now_fn);

    // Create two actions with the same digest (should be deduplicated)
    let action_info1 = make_test_action_info(
        "abcd1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab",
        true,
    );
    let action_info2 = make_test_action_info(
        "abcd1234567890abcdef1234567890abcdef1234567890abcdef1234567890ab",
        true,
    );

    let operation_id1 = OperationId::default();
    let operation_id2 = OperationId::default();

    let _subscriber1 = db
        .add_action(operation_id1.clone(), Arc::new(action_info1))
        .await?;
    let _subscriber2 = db
        .add_action(operation_id2.clone(), Arc::new(action_info2))
        .await?;

    // Both should succeed but subscriber2 should be for the same underlying action
    // If we got here, both add_action calls succeeded (otherwise await? would have failed)

    // Validation should pass even with deduplication
    db.validate_and_recover().await?;

    Ok(())
}

/// Test that validation works correctly during eviction scenarios
#[tokio::test]
async fn test_validation_with_eviction() -> Result<(), Error> {
    let eviction_config = EvictionPolicy {
        max_count: 2, // Very small to trigger eviction
        max_bytes: 1024,
        ..Default::default()
    };

    let tasks_change_notify = Arc::new(Notify::new());
    let now_fn = MockInstantWrapped::default;

    let db = MemoryAwaitedActionDb::new(&eviction_config, tasks_change_notify, now_fn);

    // Add more actions than the eviction limit
    for i in 0..5 {
        let action_info = make_test_action_info(
            &format!("{:064x}", i), // Create unique digests
            true,
        );
        let operation_id = OperationId::default();

        let _subscriber = db.add_action(operation_id, Arc::new(action_info)).await?;

        // Validate after each addition
        db.validate_and_recover().await?;
    }

    Ok(())
}
