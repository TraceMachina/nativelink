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

//! Each fault of the fake Redis is what a `RedisStore` client sees.

use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use nativelink_config::stores::RedisSpec;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_redis_tester::{FakeRedisBackend, Fault};
use nativelink_scheduler::awaited_action_db::AwaitedActionDb;
use nativelink_scheduler::store_awaited_action_db::StoreAwaitedActionDb;
use nativelink_store::redis_store::{RedisStore, RedisSubscriptionManager};
use nativelink_util::action_messages::{
    ActionInfo, ActionUniqueKey, ActionUniqueQualifier, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::store_trait::SchedulerStore;
use tokio::sync::Notify;

fn action(seed: u8) -> Arc<ActionInfo> {
    Arc::new(ActionInfo {
        command_digest: DigestInfo::new([seed; 32], 1),
        input_root_digest: DigestInfo::new([seed; 32], 2),
        timeout: Duration::from_secs(1),
        platform_properties: HashMap::new(),
        priority: 0,
        load_timestamp: SystemTime::UNIX_EPOCH,
        insert_timestamp: SystemTime::UNIX_EPOCH,
        unique_qualifier: ActionUniqueQualifier::Cacheable(ActionUniqueKey {
            execution_scope: None,
            instance_name: "fault_policy".to_string(),
            digest_function: DigestHasherFunc::Sha256,
            digest: DigestInfo::new([seed; 32], 3),
        }),
    })
}

async fn setup() -> Result<
    (
        FakeRedisBackend<RedisSubscriptionManager>,
        impl AwaitedActionDb,
    ),
    Error,
> {
    let fake: FakeRedisBackend<RedisSubscriptionManager> = FakeRedisBackend::new();
    let port = fake.clone().run().await;
    let store = RedisStore::new_standard(RedisSpec {
        addresses: vec![format!("redis://127.0.0.1:{port}")],
        experimental_pub_sub_channel: Some("fault_policy_channel".to_string()),
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
    Ok((fake, db))
}

async fn add<Db: AwaitedActionDb>(db: &Db, seed: u8) -> Result<(), Error> {
    db.add_action(
        OperationId::from(format!("client-{seed}")),
        action(seed),
        Duration::from_mins(1),
    )
    .await
    .map(drop)
}

#[nativelink_test]
async fn commands_are_counted() -> Result<(), Error> {
    let (fake, db) = setup().await?;
    fake.reset_command_counts();
    add(&db, 1).await?;
    let counts = fake.command_counts();
    assert!(
        counts.get("EVALSHA").copied().unwrap_or(0) >= 1,
        "{counts:?}"
    );
    fake.reset_command_counts();
    assert!(fake.command_counts().is_empty());
    Ok(())
}

#[nativelink_test]
async fn a_failing_write_fails_the_client_until_cleared() -> Result<(), Error> {
    let (fake, db) = setup().await?;
    fake.fail_commands("EVALSHA", None, Fault::Error("injected".into()), None);
    let err = add(&db, 1).await.expect_err("EVALSHA fails");
    assert!(format!("{err:?}").contains("injected"), "{err:?}");
    fake.clear_faults();
    add(&db, 2).await?;
    Ok(())
}

#[nativelink_test]
async fn a_counted_fault_runs_out() -> Result<(), Error> {
    let (fake, db) = setup().await?;
    fake.fail_commands("EVALSHA", None, Fault::Oom, Some(1));
    // The first write meets the fault; whatever the client makes of it, the
    // next write goes through.
    drop(add(&db, 1).await);
    add(&db, 2).await?;
    Ok(())
}

#[nativelink_test]
async fn readonly_after_failover_is_what_the_client_reads() -> Result<(), Error> {
    let (fake, db) = setup().await?;
    fake.fail_commands("EVALSHA", None, Fault::ReadOnly, None);
    let err = add(&db, 1).await.expect_err("EVALSHA answers READONLY");
    assert!(format!("{err:?}").contains("ReadOnly"), "{err:?}");
    Ok(())
}

#[nativelink_test]
async fn a_partition_fails_the_client_and_healing_restores_it() -> Result<(), Error> {
    let (fake, db) = setup().await?;
    add(&db, 1).await?;
    fake.partition(true);
    let during = tokio::time::timeout(Duration::from_mins(1), add(&db, 2)).await;
    assert!(
        !matches!(during, Ok(Ok(()))),
        "a write through a partition must not succeed"
    );
    fake.partition(false);
    let after = tokio::time::timeout(Duration::from_mins(1), async {
        loop {
            if add(&db, 3).await.is_ok() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await;
    assert!(
        after.is_ok(),
        "the client did not recover after the partition"
    );
    Ok(())
}

#[nativelink_test]
async fn eviction_removes_matching_keys() -> Result<(), Error> {
    let (fake, db) = setup().await?;
    add(&db, 1).await?;
    let before = fake.table.lock().unwrap().len();
    assert!(before > 0);
    let evicted = fake.evict_prefix("");
    assert_eq!(evicted, before);
    assert!(fake.table.lock().unwrap().is_empty());
    Ok(())
}
