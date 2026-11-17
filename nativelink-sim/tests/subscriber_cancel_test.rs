// Copyright 2026 Trace Machina, Inc. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! A Redis-backed operation subscriber keeps an update when its `changed()`
//! is dropped part way, as the `tokio::select!` in
//! `MatchingEngineActionStateResult::changed` drops it when its no-event
//! timer fires first. Written for the lost completions of RFC-0060 §11 (L3);
//! it passes, so the cancellation is not their cause.

use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use nativelink_config::stores::RedisSpec;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_redis_tester::FakeRedisBackend;
use nativelink_scheduler::awaited_action_db::{AwaitedActionDb, AwaitedActionSubscriber};
use nativelink_scheduler::store_awaited_action_db::StoreAwaitedActionDb;
use nativelink_store::redis_store::{RedisStore, RedisSubscriptionManager};
use nativelink_util::action_messages::{
    ActionInfo, ActionResult, ActionStage, ActionUniqueKey, ActionUniqueQualifier, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::store_trait::SchedulerStore;
use tokio::sync::Notify;

fn action() -> Arc<ActionInfo> {
    Arc::new(ActionInfo {
        command_digest: DigestInfo::new([1; 32], 1),
        input_root_digest: DigestInfo::new([1; 32], 2),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Cacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: "cancel".to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: DigestInfo::new([1; 32], 3),
        }),
    })
}

#[nativelink_test]
async fn a_dropped_changed_does_not_lose_the_completion() -> Result<(), Error> {
    let fake: FakeRedisBackend<RedisSubscriptionManager> = FakeRedisBackend::new();
    let port = fake.clone().run().await;
    let store = RedisStore::new_standard(RedisSpec {
        addresses: vec![format!("redis://127.0.0.1:{port}")],
        experimental_pub_sub_channel: Some("cancel_channel".to_string()),
        ..Default::default()
    })
    .await?;
    fake.set_subscription_manager(store.subscription_manager().await?);
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
    let mut subscriber = db
        .add_action(
            OperationId::from("client"),
            action(),
            Duration::from_mins(1),
        )
        .await?;
    let mut awaited = subscriber.borrow().await?;

    // Every reply from here takes 500 ms.
    fake.set_latency(Some(Duration::from_millis(500)));
    let mut state = awaited.state().as_ref().clone();
    state.stage = ActionStage::Completed(ActionResult::default());
    awaited.worker_set_state(Arc::new(state), SystemTime::now());
    db.update_awaited_action(awaited).await?;

    // The notification has arrived; give up on `changed()` while its read is
    // still waiting on the store.
    let first = tokio::time::timeout(Duration::from_millis(200), subscriber.changed()).await;
    assert!(
        first.is_err(),
        "the read should still be in flight at 200 ms"
    );

    // A cancel-safe subscriber still reports the completion.
    let second = tokio::time::timeout(Duration::from_secs(10), subscriber.changed()).await;
    let changed = second.expect("the completion was lost when the first changed() was dropped")?;
    assert!(changed.state().stage.is_finished());
    Ok(())
}
