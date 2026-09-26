// Copyright 2026 The NativeLink Authors. All rights reserved.
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

//! The queue order the matcher sees must be the same on every backend:
//! highest priority first, then oldest first. The memory backend reverses
//! its `BTreeSet`; the Redis backend sorts the index. This runs one scenario
//! through both so they cannot disagree again.

use core::ops::Bound;
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use futures::StreamExt;
use nativelink_config::stores::{EvictionPolicy, RedisSpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_redis_tester::FakeRedisBackend;
use nativelink_scheduler::awaited_action_db::{
    AwaitedActionDb, AwaitedActionSubscriber, SortedAwaitedActionState,
};
use nativelink_scheduler::memory_awaited_action_db::MemoryAwaitedActionDb;
use nativelink_scheduler::store_awaited_action_db::StoreAwaitedActionDb;
use nativelink_store::redis_store::{RedisStore, RedisSubscriptionManager};
use nativelink_util::action_messages::{
    ActionInfo, ActionUniqueKey, ActionUniqueQualifier, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::store_trait::SchedulerStore;
use pretty_assertions::assert_eq;
use tokio::sync::Notify;

const INSTANCE_NAME: &str = "queue_order_instance";

/// A distinct action (its own digest, so nothing coalesces) with the given
/// priority and insert time.
fn action(seed: u8, priority: i32, insert_secs: u64) -> Arc<ActionInfo> {
    let insert = SystemTime::UNIX_EPOCH + Duration::from_secs(insert_secs);
    Arc::new(ActionInfo {
        command_digest: DigestInfo::new([seed; 32], 1),
        input_root_digest: DigestInfo::new([seed; 32], 2),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority,
        load_timestamp: insert,
        insert_timestamp: insert,
        unique_qualifier: ActionUniqueQualifier::Cacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: INSTANCE_NAME.to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: DigestInfo::new([seed; 32], 3),
        }),
    })
}

/// Queue three actions out of order and read them back the way the matcher
/// does. Returns `(priority, insert_secs)` per action in dispatch order.
async fn queued_order<Db: AwaitedActionDb>(db: &Db) -> Result<Vec<(i32, u64)>, Error> {
    // Inserted newest-first and low-priority-first on purpose: the order the
    // backend returns must not depend on insertion order.
    db.add_action(
        OperationId::from("client-a"),
        action(1, 0, 100),
        Duration::from_secs(60),
    )
    .await?;
    db.add_action(
        OperationId::from("client-b"),
        action(2, 0, 50),
        Duration::from_secs(60),
    )
    .await?;
    db.add_action(
        OperationId::from("client-c"),
        action(3, 5, 150),
        Duration::from_secs(60),
    )
    .await?;

    let stream = db
        .get_range_of_actions(
            SortedAwaitedActionState::Queued,
            Bound::Unbounded,
            Bound::Unbounded,
            true,
        )
        .await?;
    let mut stream = core::pin::pin!(stream);
    let mut order = Vec::new();
    while let Some(subscriber) = stream.next().await {
        let awaited = subscriber?.borrow().await?;
        let info = awaited.action_info();
        let secs = info
            .insert_timestamp
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        order.push((info.priority, secs));
    }
    Ok(order)
}

/// Highest priority first, then oldest first.
const EXPECTED: [(i32, u64); 3] = [(5, 150), (0, 50), (0, 100)];

#[nativelink_test]
async fn memory_backend_serves_highest_priority_then_oldest() -> Result<(), Error> {
    let db = MemoryAwaitedActionDb::new(
        &EvictionPolicy::default(),
        Arc::new(Notify::new()),
        MockInstantWrapped::default,
    );
    assert_eq!(queued_order(&db).await?, EXPECTED);
    Ok(())
}

#[nativelink_test]
async fn redis_backend_serves_highest_priority_then_oldest() -> Result<(), Error> {
    let fake_redis_backend: FakeRedisBackend<RedisSubscriptionManager> = FakeRedisBackend::new();
    let fake_redis_port = fake_redis_backend.clone().run().await;
    let spec = RedisSpec {
        addresses: vec![format!("redis://127.0.0.1:{fake_redis_port}")],
        experimental_pub_sub_channel: Some("queue_order_channel".to_string()),
        ..Default::default()
    };
    let store = RedisStore::new_standard(spec).await?;
    fake_redis_backend.set_subscription_manager(store.subscription_manager().await?);

    let db = StoreAwaitedActionDb::new(
        store,
        Arc::new(Notify::new()),
        MockInstantWrapped::default,
        OperationId::default,
        60,
        60,
        false,
    )
    .await?;
    assert_eq!(queued_order(&db).await?, EXPECTED);
    Ok(())
}
