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

//! `migrate_temporary_indexes` against a real `RediSearch`: whether a
//! temporary index deletes its keys when dropped is the module's behavior,
//! which `FakeRedisBackend` does not have. Skipped unless
//! `NATIVELINK_REDIS_TEST_URL` names a redis-stack server, which every test
//! flushes:
//!
//! ```text
//! docker run -d --rm -p 16379:6379 redis/redis-stack-server:7.4.0-v0
//! NATIVELINK_REDIS_TEST_URL=redis://127.0.0.1:16379 \
//!   cargo test -p nativelink-scheduler --test redis_temporary_index_test -- --test-threads=1
//! ```

use core::ops::Bound;
use core::time::Duration;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use futures::StreamExt;
use nativelink_config::stores::RedisSpec;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_scheduler::awaited_action_db::{
    AwaitedActionDb, AwaitedActionSubscriber, SortedAwaitedActionState,
};
use nativelink_scheduler::store_awaited_action_db::StoreAwaitedActionDb;
use nativelink_store::redis_store::RedisStore;
use nativelink_util::action_messages::{
    ActionInfo, ActionUniqueKey, ActionUniqueQualifier, OperationId,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use pretty_assertions::assert_eq;
use redis::aio::MultiplexedConnection;
use tokio::sync::Notify;

const INSTANCE_NAME: &str = "temporary_index_instance";
const STATE_INDEX_PREFIX: &str = "aa__state_sort_key_";

/// Highest priority first, then oldest first, as `queue_order_test` pins.
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

async fn cmd<T: redis::FromRedisValue>(url: &str, args: &[&str]) -> redis::RedisResult<T> {
    let mut command = redis::cmd(args[0]);
    for arg in &args[1..] {
        command.arg(*arg);
    }
    command.query_async(&mut raw(url).await).await
}

/// Starts from nothing, dropping indexes without touching keys, since
/// `FLUSHALL` leaves the index definitions behind.
async fn reset(url: &str) {
    let indexes: Vec<String> = cmd(url, &["FT._LIST"]).await.unwrap();
    for index in indexes {
        let _: () = cmd(url, &["FT.DROPINDEX", &index, "_FORCEKEEPDOCS"])
            .await
            .unwrap();
    }
    let _: () = cmd(url, &["FLUSHALL"]).await.unwrap();
}

async fn indexes(url: &str) -> Vec<String> {
    let mut list: Vec<String> = cmd(url, &["FT._LIST"]).await.unwrap();
    list.sort();
    list
}

async fn state_indexes(url: &str) -> Vec<String> {
    indexes(url)
        .await
        .into_iter()
        .filter(|name| name.starts_with(STATE_INDEX_PREFIX))
        .collect()
}

/// `Some(ttl)` for a temporary index, `None` for a persistent one.
async fn ttl(url: &str, index: &str) -> Option<i64> {
    cmd::<i64>(url, &["FT.DEBUG", "TTL", index]).await.ok()
}

/// A count from `FT.INFO`, which reports some as integers and some as
/// decimal strings.
async fn info_field(url: &str, index: &str, field: &str) -> i64 {
    let info: Vec<redis::Value> = cmd(url, &["FT.INFO", index]).await.unwrap();
    let mut items = info.iter();
    while let (Some(name), Some(value)) = (items.next(), items.next()) {
        let text = |v: &redis::Value| match v {
            redis::Value::BulkString(b) => Some(String::from_utf8_lossy(b).into_owned()),
            redis::Value::SimpleString(s) => Some(s.clone()),
            _ => None,
        };
        if text(name).as_deref() == Some(field) {
            return match value {
                redis::Value::Int(n) => *n,
                redis::Value::Double(d) => format!("{d:.0}").parse().unwrap_or(0),
                other => text(other)
                    .and_then(|t| t.split('.').next().and_then(|whole| whole.parse().ok()))
                    .unwrap_or(0),
            };
        }
    }
    panic!("FT.INFO {index} has no {field}");
}

async fn action_keys(url: &str) -> Vec<String> {
    let mut keys: Vec<String> = cmd(url, &["KEYS", "aa_*"]).await.unwrap();
    keys.sort();
    keys
}

/// Rebuilds `index` the way releases before v1.8.1 created it, keeping its
/// keys, then rewrites every key `rewrites` times the way the version-set
/// script does (`DEL` then `HSET`), which leaves the dead entries a
/// temporary index never collects.
async fn make_temporary_and_bloated(url: &str, index: &str, rewrites: usize) {
    let _: () = cmd(url, &["FT.DROPINDEX", index, "_FORCEKEEPDOCS"])
        .await
        .unwrap();
    let _: () = cmd(
        url,
        &[
            "FT.CREATE",
            index,
            "ON",
            "HASH",
            "NOHL",
            "NOFIELDS",
            "NOFREQS",
            "NOOFFSETS",
            "TEMPORARY",
            "86400",
            "PREFIX",
            "1",
            "aa_",
            "SCHEMA",
            "state",
            "TAG",
            "sort_key",
            "TAG",
            "SORTABLE",
        ],
    )
    .await
    .unwrap();
    let mut con = raw(url).await;
    for key in action_keys(url).await {
        let fields: Vec<(String, Vec<u8>)> = redis::cmd("HGETALL")
            .arg(&key)
            .query_async(&mut con)
            .await
            .unwrap();
        for _ in 0..rewrites {
            let mut pipe = redis::pipe();
            pipe.atomic().cmd("DEL").arg(&key).ignore();
            pipe.cmd("HSET").arg(&key).arg(&fields).ignore();
            let _: () = pipe.query_async(&mut con).await.unwrap();
        }
    }
}

async fn wait_backfilled(url: &str, index: &str) {
    for _ in 0..200 {
        if info_field(url, index, "indexing").await == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{index} never finished backfilling");
}

/// Waits out the store's short trust in an answer that holds only while a
/// replacement backfills, so the next read checks the migration again.
async fn settle() {
    tokio::time::sleep(Duration::from_millis(1_100)).await;
}

async fn open_db(url: &str, migrate: bool) -> Result<impl AwaitedActionDb, Error> {
    let spec = RedisSpec {
        addresses: vec![url.to_string()],
        experimental_pub_sub_channel: Some("temporary_index_channel".to_string()),
        migrate_temporary_indexes: migrate,
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

async fn add_four<Db: AwaitedActionDb>(db: &Db) -> Result<(), Error> {
    for (client, seed, priority, ms) in [
        ("client-a", 1, 0, 100_000),
        ("client-d", 4, 0, 50_500),
        ("client-b", 2, 0, 50_000),
        ("client-c", 3, 5, 150_000),
    ] {
        db.add_action(
            OperationId::from(client),
            action(seed, priority, ms),
            Duration::from_mins(1),
        )
        .await?;
    }
    Ok(())
}

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

/// A queue written by an older release: its state index temporary and full
/// of dead entries. Returns that index's name and the action keys.
async fn inherited_queue(url: &str) -> Result<(String, Vec<String>), Error> {
    reset(url).await;
    let old = open_db(url, false).await?;
    add_four(&old).await?;
    assert_eq!(queued(&old).await?, EXPECTED);
    let [original] = state_indexes(url).await.try_into().unwrap();
    let keys = action_keys(url).await;
    make_temporary_and_bloated(url, &original, 25).await;
    assert!(ttl(url, &original).await.is_some(), "setup: not temporary");
    assert!(
        info_field(url, &original, "num_records").await
            > 10 * info_field(url, &original, "num_docs").await,
        "setup: not bloated"
    );
    Ok((original, keys))
}

#[nativelink_test]
async fn a_temporary_index_is_replaced_and_every_action_kept() -> Result<(), Error> {
    let Some(url) = test_url() else { return Ok(()) };
    let (original, keys) = inherited_queue(&url).await?;
    let replacement = format!("{original}_p");

    let db = open_db(&url, true).await?;
    // The first read builds the replacement and reads the original
    // meanwhile; the queue is whole throughout.
    assert_eq!(queued(&db).await?, EXPECTED);
    assert_eq!(
        state_indexes(&url).await,
        vec![original.clone(), replacement.clone()]
    );
    wait_backfilled(&url, &replacement).await;
    settle().await;

    // The next read moves to the replacement and retires the original.
    assert_eq!(queued(&db).await?, EXPECTED);
    assert_eq!(state_indexes(&url).await, vec![replacement.clone()]);
    assert_eq!(
        ttl(&url, &replacement).await,
        None,
        "replacement is temporary"
    );
    assert_eq!(action_keys(&url).await, keys, "an action key was lost");
    let (records, docs) = (
        info_field(&url, &replacement, "num_records").await,
        info_field(&url, &replacement, "num_docs").await,
    );
    // Two tag fields per document and nothing dead.
    assert!(
        records == 2 * docs,
        "the replacement carries the original's dead entries: {records} records, {docs} docs"
    );

    // A process started later reads the replacement as well.
    assert_eq!(queued(&open_db(&url, true).await?).await?, EXPECTED);
    Ok(())
}

#[nativelink_test]
async fn without_the_setting_a_temporary_index_is_left_alone() -> Result<(), Error> {
    let Some(url) = test_url() else { return Ok(()) };
    let (original, keys) = inherited_queue(&url).await?;

    let db = open_db(&url, false).await?;
    assert_eq!(queued(&db).await?, EXPECTED);
    assert_eq!(queued(&db).await?, EXPECTED);
    assert_eq!(state_indexes(&url).await, vec![original.clone()]);
    assert!(ttl(&url, &original).await.is_some());
    assert_eq!(action_keys(&url).await, keys);
    Ok(())
}

#[nativelink_test]
async fn a_persistent_index_is_not_rebuilt() -> Result<(), Error> {
    let Some(url) = test_url() else { return Ok(()) };
    reset(&url).await;
    let db = open_db(&url, true).await?;
    add_four(&db).await?;
    assert_eq!(queued(&db).await?, EXPECTED);
    assert_eq!(queued(&db).await?, EXPECTED);
    let [index] = state_indexes(&url).await.try_into().unwrap();
    assert!(
        !index.ends_with("_p"),
        "a fresh index was created under the migration name"
    );
    assert_eq!(ttl(&url, &index).await, None);
    Ok(())
}

#[nativelink_test]
async fn two_processes_migrating_at_once_keep_every_action() -> Result<(), Error> {
    let Some(url) = test_url() else { return Ok(()) };
    let (original, keys) = inherited_queue(&url).await?;
    let replacement = format!("{original}_p");

    let (a, b) = (open_db(&url, true).await?, open_db(&url, true).await?);
    let (ra, rb) = tokio::join!(queued(&a), queued(&b));
    assert_eq!((ra?, rb?), (EXPECTED.to_vec(), EXPECTED.to_vec()));
    wait_backfilled(&url, &replacement).await;
    settle().await;
    let (ra, rb) = tokio::join!(queued(&a), queued(&b));
    assert_eq!((ra?, rb?), (EXPECTED.to_vec(), EXPECTED.to_vec()));

    assert_eq!(state_indexes(&url).await, vec![replacement]);
    assert_eq!(action_keys(&url).await, keys);
    Ok(())
}

#[nativelink_test]
async fn an_incomplete_replacement_never_retires_the_original() -> Result<(), Error> {
    let Some(url) = test_url() else { return Ok(()) };
    let (original, keys) = inherited_queue(&url).await?;
    let replacement = format!("{original}_p");
    // A replacement whose scan finished without the keys, as one that ran
    // out of memory would: persistent, done indexing, and holding nothing.
    let _: () = cmd(
        &url,
        &[
            "FT.CREATE",
            &replacement,
            "ON",
            "HASH",
            "PREFIX",
            "1",
            "aa_no_such_prefix_",
            "SCHEMA",
            "state",
            "TAG",
            "sort_key",
            "TAG",
            "SORTABLE",
        ],
    )
    .await
    .unwrap();
    wait_backfilled(&url, &replacement).await;

    let db = open_db(&url, true).await?;
    assert_eq!(queued(&db).await?, EXPECTED);
    settle().await;
    assert_eq!(queued(&db).await?, EXPECTED);
    assert_eq!(
        state_indexes(&url).await,
        vec![original.clone(), replacement],
        "the original was retired for a replacement without its keys"
    );
    assert!(ttl(&url, &original).await.is_some());
    assert_eq!(action_keys(&url).await, keys);
    Ok(())
}

#[nativelink_test]
async fn a_read_given_the_retired_original_does_not_recreate_it() -> Result<(), Error> {
    let Some(url) = test_url() else { return Ok(()) };
    let (original, keys) = inherited_queue(&url).await?;
    let replacement = format!("{original}_p");
    let (a, b) = (open_db(&url, true).await?, open_db(&url, true).await?);

    // `a` builds the replacement and is told to read the original for now.
    let started = std::time::Instant::now();
    assert_eq!(queued(&a).await?, EXPECTED);
    wait_backfilled(&url, &replacement).await;
    // `b` finds the replacement complete and retires the original.
    assert_eq!(queued(&b).await?, EXPECTED);
    assert_eq!(state_indexes(&url).await, vec![replacement.clone()]);
    // `a` still holds "read the original"; its read must move, not create
    // the original again.
    assert_eq!(queued(&a).await?, EXPECTED);
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "setup: too slow to exercise a stale answer"
    );
    assert_eq!(state_indexes(&url).await, vec![replacement]);
    assert_eq!(action_keys(&url).await, keys);
    Ok(())
}
