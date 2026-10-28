// Copyright 2026 The NativeLink Authors. All rights reserved.
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

//! `experimental_sorted_index` against a real Redis: the write script is
//! Lua, which `FakeRedisBackend` does not run. Skipped unless
//! `NATIVELINK_REDIS_TEST_URL` names a redis-stack server, which every test
//! flushes:
//!
//! ```text
//! docker run -d --rm -p 16379:6379 redis/redis-stack-server:7.4.0-v0
//! NATIVELINK_REDIS_TEST_URL=redis://127.0.0.1:16379 \
//!   cargo test -p nativelink-scheduler --test redis_sorted_index_test -- --test-threads=1
//! ```

use core::ops::Bound;
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use futures::StreamExt;
use nativelink_config::stores::{RedisSortedIndexSpec, RedisSpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::awaited_action_db::{
    AwaitedActionDb, AwaitedActionSubscriber, SortedAwaitedActionState,
};
use nativelink_scheduler::store_awaited_action_db::StoreAwaitedActionDb;
use nativelink_store::redis_store::RedisStore;
use nativelink_util::action_messages::{
    ActionInfo, ActionStage, ActionState, ActionUniqueKey, ActionUniqueQualifier, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use pretty_assertions::assert_eq;
use redis::aio::MultiplexedConnection;
use tokio::sync::Notify;

const INSTANCE_NAME: &str = "sorted_index_instance";
const QUEUED_SET: &str = "nlsq:aa_state:queued";

/// Highest priority first, then oldest first, as `queue_order_test` pins
/// for every backend.
const EXPECTED: [(i32, u64); 4] = [(5, 150_000), (0, 50_000), (0, 50_500), (0, 100_000)];

fn test_url() -> Option<String> {
    std::env::var("NATIVELINK_REDIS_TEST_URL").ok()
}

async fn raw(url: &str) -> MultiplexedConnection {
    redis::Client::open(url)
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap()
}

/// Starts from nothing. `FLUSHALL` leaves index definitions behind, so they
/// are dropped first, keeping keys (a temporary index's plain drop would
/// delete them, which here is moot but never the habit to show).
async fn flush(url: &str) {
    let mut con = raw(url).await;
    let indexes: Vec<String> = redis::cmd("FT._LIST").query_async(&mut con).await.unwrap();
    for index in indexes {
        let _: () = redis::cmd("FT.DROPINDEX")
            .arg(&index)
            .arg("_FORCEKEEPDOCS")
            .query_async(&mut con)
            .await
            .unwrap();
    }
    let _: () = redis::cmd("FLUSHALL").query_async(&mut con).await.unwrap();
}

async fn zcard(url: &str, set: &str) -> u64 {
    redis::cmd("ZCARD")
        .arg(set)
        .query_async(&mut raw(url).await)
        .await
        .unwrap()
}

async fn calls(url: &str, command: &str) -> u64 {
    let info: String = redis::cmd("INFO")
        .arg("commandstats")
        .query_async(&mut raw(url).await)
        .await
        .unwrap();
    let needle = format!("cmdstat_{}:calls=", command.to_ascii_lowercase());
    info.lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix(&needle)
                .map(str::to_string)
        })
        .and_then(|rest| rest.split(',').next()?.parse().ok())
        .unwrap_or(0)
}

async fn reset_stats(url: &str) {
    let _: () = redis::cmd("CONFIG")
        .arg("RESETSTAT")
        .query_async(&mut raw(url).await)
        .await
        .unwrap();
}

fn sorted(read: bool) -> RedisSortedIndexSpec {
    RedisSortedIndexSpec {
        values: vec!["queued".to_string()],
        read,
        reconcile_interval_s: 300,
    }
}

async fn open_db(
    url: &str,
    sorted_index: Option<RedisSortedIndexSpec>,
    page: u64,
) -> Result<impl AwaitedActionDb, Error> {
    let spec = RedisSpec {
        addresses: vec![url.to_string()],
        experimental_pub_sub_channel: Some("sorted_index_channel".to_string()),
        max_count_per_cursor: page,
        experimental_sorted_index: sorted_index,
        ..Default::default()
    };
    StoreAwaitedActionDb::new(
        RedisStore::new_standard(spec).await?,
        Arc::new(Notify::new()),
        MockInstantWrapped::default,
        OperationId::default,
        60,
        60,
        false,
    )
    .await
}

/// A queue read through a plain store creates the state index, which a
/// sorted store needs before its first reconcile can walk it.
async fn create_index(url: &str) -> Result<(), Error> {
    queued(&open_db(url, None, 0).await?).await?;
    Ok(())
}

fn action(seed: u8, priority: i32, insert_ms: u64) -> Arc<ActionInfo> {
    let insert = SystemTime::UNIX_EPOCH + Duration::from_millis(insert_ms);
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

/// Queues the same four actions as `queue_order_test`, out of order.
async fn add_four<Db: AwaitedActionDb>(db: &Db) -> Result<Vec<Db::Subscriber>, Error> {
    let mut subscribers = Vec::new();
    for (client, seed, priority, ms) in [
        ("client-a", 1, 0, 100_000),
        ("client-d", 4, 0, 50_500),
        ("client-b", 2, 0, 50_000),
        ("client-c", 3, 5, 150_000),
    ] {
        subscribers.push(
            db.add_action(
                OperationId::from(client),
                action(seed, priority, ms),
                Duration::from_mins(1),
            )
            .await?,
        );
    }
    Ok(subscribers)
}

/// The queue as the matcher reads it: `(priority, insert_ms)` in order.
async fn queued<Db: AwaitedActionDb>(db: &Db) -> Result<Vec<(i32, u64)>, Error> {
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
        let ms = u64::try_from(
            info.insert_timestamp
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_millis(),
        )
        .unwrap();
        order.push((info.priority, ms));
    }
    Ok(order)
}

#[nativelink_test]
async fn a_maintained_queue_is_read_from_the_set_in_dispatch_order() -> Result<(), Error> {
    let Some(url) = test_url() else { return Ok(()) };
    flush(&url).await;
    create_index(&url).await?;

    let db = open_db(&url, Some(sorted(true)), 0).await?;
    add_four(&db).await?;
    assert_eq!(zcard(&url, QUEUED_SET).await, 4);

    reset_stats(&url).await;
    assert_eq!(queued(&db).await?, EXPECTED);
    assert!(
        calls(&url, "ZRANGE").await > 0,
        "the read did not use the sorted set"
    );
    // The same order RediSearch gives for the same data.
    assert_eq!(queued(&open_db(&url, None, 0).await?).await?, EXPECTED);
    Ok(())
}

#[nativelink_test]
async fn a_queue_written_before_the_set_existed_is_backfilled() -> Result<(), Error> {
    let Some(url) = test_url() else { return Ok(()) };
    flush(&url).await;
    // Written by a store that does not maintain the set, as a release
    // without it would.
    let plain = open_db(&url, None, 0).await?;
    add_four(&plain).await?;
    queued(&plain).await?;
    assert_eq!(zcard(&url, QUEUED_SET).await, 0);

    let sorted = open_db(&url, Some(sorted(true)), 0).await?;
    assert_eq!(queued(&sorted).await?, EXPECTED);
    assert_eq!(zcard(&url, QUEUED_SET).await, 4);
    Ok(())
}

#[nativelink_test]
async fn a_dispatched_action_leaves_the_queued_set() -> Result<(), Error> {
    let Some(url) = test_url() else { return Ok(()) };
    flush(&url).await;
    create_index(&url).await?;
    let db = open_db(&url, Some(sorted(true)), 0).await?;
    let subscribers = add_four(&db).await?;

    // client-c, the highest priority, starts executing.
    let mut awaited = subscribers[3].borrow().await?;
    let state = awaited.state().clone();
    awaited.worker_set_state(
        Arc::new(ActionState {
            stage: ActionStage::Executing,
            last_transition_timestamp: SystemTime::now(),
            ..(*state).clone()
        }),
        SystemTime::now(),
    );
    db.update_awaited_action(awaited).await?;

    assert_eq!(zcard(&url, QUEUED_SET).await, 3);
    assert_eq!(queued(&db).await?, EXPECTED[1..]);
    Ok(())
}

#[nativelink_test]
async fn a_stale_member_is_skipped_and_removed() -> Result<(), Error> {
    let Some(url) = test_url() else { return Ok(()) };
    flush(&url).await;
    create_index(&url).await?;
    let db = open_db(&url, Some(sorted(true)), 0).await?;
    add_four(&db).await?;
    queued(&db).await?;

    // Sorts first, and names a key that is not there.
    let ghost = "ffffffffffffffffffffffffffffffff:aa_ghost";
    let _: () = redis::cmd("ZADD")
        .arg(QUEUED_SET)
        .arg(0)
        .arg(ghost)
        .query_async(&mut raw(&url).await)
        .await
        .unwrap();

    assert_eq!(queued(&db).await?, EXPECTED);
    let score: Option<f64> = redis::cmd("ZSCORE")
        .arg(QUEUED_SET)
        .arg(ghost)
        .query_async(&mut raw(&url).await)
        .await
        .unwrap();
    assert_eq!(score, None, "the stale member should have been removed");
    Ok(())
}

#[nativelink_test]
async fn pages_lose_and_duplicate_nothing() -> Result<(), Error> {
    let Some(url) = test_url() else { return Ok(()) };
    flush(&url).await;
    create_index(&url).await?;
    // One member per page: every boundary is a page boundary.
    let db = open_db(&url, Some(sorted(true)), 1).await?;
    add_four(&db).await?;
    assert_eq!(queued(&db).await?, EXPECTED);
    Ok(())
}

#[nativelink_test]
async fn write_only_maintains_the_set_but_reads_the_index() -> Result<(), Error> {
    let Some(url) = test_url() else { return Ok(()) };
    flush(&url).await;
    create_index(&url).await?;
    let db = open_db(&url, Some(sorted(false)), 0).await?;
    add_four(&db).await?;
    assert_eq!(zcard(&url, QUEUED_SET).await, 4);

    reset_stats(&url).await;
    assert_eq!(queued(&db).await?, EXPECTED);
    assert_eq!(calls(&url, "ZRANGE").await, 0);
    Ok(())
}

#[nativelink_test]
async fn a_temporary_index_keeps_reads_off_the_set() -> Result<(), Error> {
    let Some(url) = test_url() else { return Ok(()) };
    flush(&url).await;
    create_index(&url).await?;
    // Rebuild the state index the way releases before v1.8.1 created it,
    // keeping its keys.
    let indexes: Vec<String> = redis::cmd("FT._LIST")
        .query_async(&mut raw(&url).await)
        .await
        .unwrap();
    let state = indexes
        .into_iter()
        .find(|name| name.starts_with("aa__state_sort_key_") && !name.ends_with("_p"))
        .unwrap();
    let mut con = raw(&url).await;
    let _: () = redis::cmd("FT.DROPINDEX")
        .arg(&state)
        .arg("_FORCEKEEPDOCS")
        .query_async(&mut con)
        .await
        .unwrap();
    let _: () = redis::cmd("FT.CREATE")
        .arg(&state)
        .arg("ON")
        .arg("HASH")
        .arg("TEMPORARY")
        .arg(86_400)
        .arg("PREFIX")
        .arg(1)
        .arg("aa_")
        .arg("SCHEMA")
        .arg("state")
        .arg("TAG")
        .arg("sort_key")
        .arg("TAG")
        .arg("SORTABLE")
        .query_async(&mut con)
        .await
        .unwrap();

    let db = open_db(&url, Some(sorted(true)), 0).await?;
    add_four(&db).await?;
    reset_stats(&url).await;
    assert_eq!(queued(&db).await?, EXPECTED);
    assert_eq!(
        calls(&url, "ZRANGE").await,
        0,
        "the set was read while the index is temporary"
    );
    // The writes still maintain it, ready for when the index is migrated.
    assert_eq!(zcard(&url, QUEUED_SET).await, 4);
    Ok(())
}
