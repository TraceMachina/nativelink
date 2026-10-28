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

//! The experimental sorted-set index of
//! [`RedisSpec::experimental_sorted_index`](nativelink_config::stores::RedisSpec::experimental_sorted_index).
//!
//! A sorted index (the scheduler's `state` index, sorted by `sort_key`) is
//! also kept as one Redis sorted set per index value, maintained by the
//! versioned-write script in the same atomic call as the hash. Every member
//! has score 0 and is `<sort_key>:<redis key>`, so the set's lexicographic
//! order is the order a `RediSearch` `SORTBY` over the `TAG SORTABLE` field
//! returns. Scores cannot carry it: the sort key is a `u128`, and a double
//! is exact only to 53 bits.
//!
//! Reads page the set by key range and never trust it blindly: a member
//! whose hash no longer says the same thing is skipped and removed. The
//! removal checks the hash again in the same script, since the action can
//! be written back between the read and the removal: a re-queued action
//! keeps its sort key and so comes back as the very same member.

use core::marker::PhantomData;
use core::time::Duration;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;
use std::time::Instant;

use bytes::Bytes;
use futures::{Stream, StreamExt};
use nativelink_config::stores::RedisSortedIndexSpec;
use nativelink_error::{Error, ResultExt};
use nativelink_util::store_trait::{SchedulerIndexProvider, SchedulerStoreDecodeTo, SortedIndex};
use parking_lot::Mutex;
use redis::aio::ConnectionLike;
use redis::{Script, Value};
use tracing::warn;

use crate::redis_store::{DATA_FIELD_NAME, VERSION_FIELD_NAME};
use crate::redis_utils::{FtAggregateCursor, FtAggregateOptions, ft_aggregate};

/// Every sorted set's name starts with this, after the store's key prefix.
/// Deliberately not the index's key prefix: the sets are not hashes, and a
/// reader that scans that prefix for hashes would trip over them.
const SET_NAMESPACE: &str = "nlsq:";

/// How long a failed reconcile waits before it is tried again. Shorter
/// than the reconcile interval, since a set that never reconciled is never
/// read and the queue stays on `RediSearch` until it does.
const RECONCILE_RETRY: Duration = Duration::from_secs(10);

/// Removes the members of `KEYS[1]` among `ARGV[5..]` whose hash still does
/// not match them, rechecking each hash inside the script. A read decides a
/// member is stale from an `HMGET` taken before; by the time it removes it,
/// the action may have been written back under the same member (a
/// re-queued action keeps its sort key), and removing it then would hide a
/// queued action until the next reconcile.
///
/// Like the write script, it reaches hashes it was not given as keys, which
/// is why the sorted index is refused in Cluster mode.
///   ARGV[1]: the data field, which a live hash always has.
///   ARGV[2]: the indexed field naming the set.
///   ARGV[3]: the sort field.
///   ARGV[4]: the index value the set holds.
///   ARGV[5..]: members, `<sort key>:<redis key>`.
/// Returns the number removed.
const REMOVE_STALE_SCRIPT: &str = r"
local set = KEYS[1]
local data_field = ARGV[1]
local index_field = ARGV[2]
local sort_field = ARGV[3]
local value = ARGV[4]
local removed = 0
for i = 5, #ARGV do
    local member = ARGV[i]
    local sep = string.find(member, ':', 1, true)
    local live = false
    if sep then
        local key = string.sub(member, sep + 1)
        live = redis.call('HEXISTS', key, data_field) == 1
            and redis.call('HGET', key, index_field) == value
            and redis.call('HGET', key, sort_field) == string.sub(member, 1, sep - 1)
    end
    if not live then
        removed = removed + redis.call('ZREM', set, member)
    end
end
return removed
";

static REMOVE_STALE: LazyLock<Script> = LazyLock::new(|| Script::new(REMOVE_STALE_SCRIPT));

/// Runs [`REMOVE_STALE_SCRIPT`] over `members` of `set`, which holds the
/// keys whose `index_field` is `value`.
pub(crate) async fn remove_stale<C: ConnectionLike + Send>(
    connection: &mut C,
    set: &str,
    index_field: &str,
    sort_field: &str,
    value: &str,
    members: &[String],
) -> redis::RedisResult<u64> {
    REMOVE_STALE
        .key(set)
        .arg(DATA_FIELD_NAME)
        .arg(index_field)
        .arg(sort_field)
        .arg(value)
        .arg(members)
        .invoke_async(connection)
        .await
}

/// How long the reconcile walk's cursor may sit idle between reads.
const RECONCILE_CURSOR_IDLE_MS: u64 = 30_000;

/// The runtime form of the spec, plus what this process knows about each
/// set's reconcile.
#[derive(Debug)]
pub(crate) struct SortedIndexState {
    values: HashSet<String>,
    read: bool,
    reconcile_interval: Duration,
    reconciles: Mutex<HashMap<String, Reconcile>>,
}

#[derive(Debug, Clone, Copy)]
struct Reconcile {
    last_attempt: Instant,
    succeeded_once: bool,
    last_failed: bool,
}

impl SortedIndexState {
    pub(crate) fn new(spec: &RedisSortedIndexSpec) -> Self {
        Self {
            values: spec.values.iter().cloned().collect(),
            read: spec.read,
            reconcile_interval: Duration::from_secs(spec.reconcile_interval_s),
            reconciles: Mutex::new(HashMap::new()),
        }
    }

    /// The set a read of `K` for `index_value` is served from, when it is
    /// served from one at all.
    pub(crate) fn readable_set<K: SchedulerIndexProvider>(
        &self,
        store_prefix: &str,
        index_value: &str,
    ) -> Option<String> {
        let index = SortedIndex::of::<K>()?;
        (self.read && !index_value.is_empty() && self.values.contains(index_value))
            .then(|| set_name(store_prefix, &index, index_value))
    }

    /// Claims the reconcile of `set` when one is due, so concurrent readers
    /// in this process do not all walk the index at once.
    pub(crate) fn claim_reconcile(&self, set: &str) -> bool {
        let now = Instant::now();
        let mut reconciles = self.reconciles.lock();
        let due = reconciles.get(set).is_none_or(|r| {
            let wait = if r.last_failed {
                RECONCILE_RETRY
            } else {
                self.reconcile_interval
            };
            now.duration_since(r.last_attempt) >= wait
        });
        if due {
            let succeeded_once = reconciles.get(set).is_some_and(|r| r.succeeded_once);
            reconciles.insert(
                set.to_string(),
                Reconcile {
                    last_attempt: now,
                    succeeded_once,
                    last_failed: false,
                },
            );
        }
        due
    }

    pub(crate) fn finish_reconcile(&self, set: &str, ok: bool) {
        if let Some(r) = self.reconciles.lock().get_mut(set) {
            r.succeeded_once |= ok;
            r.last_failed = !ok;
        }
    }

    /// A set is only read once it has been reconciled in this process:
    /// before that, actions written before the set was maintained are not
    /// in it.
    pub(crate) fn has_reconciled(&self, set: &str) -> bool {
        self.reconciles
            .lock()
            .get(set)
            .is_some_and(|r| r.succeeded_once)
    }

    /// The sorted-set arguments for one versioned write.
    pub(crate) fn write_args(
        &self,
        store_prefix: &str,
        index: &SortedIndex,
        indexes: &[(&'static str, Bytes)],
        redis_key: &str,
        has_expiry: bool,
    ) -> SortedWrite {
        let field = |name: &str| {
            indexes
                .iter()
                .find(|(n, _)| *n == name)
                .and_then(|(_, v)| core::str::from_utf8(v).ok())
        };
        // A key with an expiry is never added: Redis would expire the hash
        // and leave its member behind.
        let (new_set, new_member) = match (field(index.index_name), field(index.sort_key)) {
            (Some(value), Some(sort)) if !has_expiry && self.values.contains(value) => (
                set_name(store_prefix, index, value),
                member(sort, redis_key),
            ),
            _ => (String::new(), String::new()),
        };
        SortedWrite {
            set_prefix: set_prefix(store_prefix, index),
            index_field: index.index_name,
            sort_field: index.sort_key,
            new_set,
            new_member,
        }
    }
}

/// The extra arguments `LUA_VERSION_SET_SORTED_SCRIPT` takes, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SortedWrite {
    /// The old member is removed from `set_prefix .. <old value>`.
    pub set_prefix: String,
    pub index_field: &'static str,
    pub sort_field: &'static str,
    /// Empty when the write is not added to any set.
    pub new_set: String,
    pub new_member: String,
}

pub(crate) fn set_prefix(store_prefix: &str, index: &SortedIndex) -> String {
    format!(
        "{store_prefix}{SET_NAMESPACE}{}{}:",
        index.key_prefix, index.index_name
    )
}

pub(crate) fn set_name(store_prefix: &str, index: &SortedIndex, value: &str) -> String {
    format!("{}{value}", set_prefix(store_prefix, index))
}

/// A sort key is hex, so it never contains the separator, and splitting at
/// the first one recovers both halves whatever the key holds.
pub(crate) fn member(sort: &str, redis_key: &str) -> String {
    format!("{sort}:{redis_key}")
}

pub(crate) fn split_member(member: &str) -> Option<(&str, &str)> {
    member.split_once(':')
}

/// Adds every key the index holds for `query` to `set`. Idempotent: adding
/// an existing member is a no-op. A member added here for an action that
/// moves before the next read is stale, and that read drops it.
pub(crate) async fn reconcile<C>(
    connection: C,
    ft_index: String,
    query: String,
    set: &str,
    index: &SortedIndex,
    batch: u64,
) -> Result<u64, Error>
where
    C: ConnectionLike + Clone + Send,
{
    let mut writer = connection.clone();
    let rows = ft_aggregate(
        connection,
        ft_index,
        query,
        FtAggregateOptions {
            load: vec![
                "@__key".into(),
                index.index_name.into(),
                index.sort_key.into(),
            ],
            cursor: FtAggregateCursor {
                count: batch,
                max_idle: RECONCILE_CURSOR_IDLE_MS,
            },
            sort_by: Vec::new(),
            sort_desc: false,
        },
    )
    .await
    .err_tip(|| "In sorted_index::reconcile ft_aggregate")?;
    futures::pin_mut!(rows);

    let batch = usize::try_from(batch).unwrap_or(usize::MAX).max(1);
    let mut pending: Vec<String> = Vec::with_capacity(batch);
    let mut added = 0u64;
    while let Some(row) = rows.next().await {
        let row = row.err_tip(|| "In sorted_index::reconcile cursor read")?;
        if matches!(row, Value::Int(_)) {
            continue;
        }
        let (Some(key), Some(sort)) = (row_field(&row, "__key"), row_field(&row, index.sort_key))
        else {
            continue;
        };
        pending.push(member(&sort, &key));
        if pending.len() >= batch {
            added += zadd(&mut writer, set, &mut pending).await?;
        }
    }
    added += zadd(&mut writer, set, &mut pending).await?;
    Ok(added)
}

async fn zadd<C: ConnectionLike + Send>(
    connection: &mut C,
    set: &str,
    members: &mut Vec<String>,
) -> Result<u64, Error> {
    if members.is_empty() {
        return Ok(0);
    }
    let mut cmd = redis::cmd("ZADD");
    cmd.arg(set);
    for member in members.drain(..) {
        cmd.arg(0).arg(member);
    }
    cmd.query_async::<u64>(connection)
        .await
        .map_err(Error::from)
        .err_tip(|| "In sorted_index::reconcile ZADD")
}

/// A field of an aggregate row, as RESP2 (`[name, value, ...]`) or RESP3
/// (a map) returns it.
fn row_field(row: &Value, name: &str) -> Option<String> {
    let as_string = |v: &Value| match v {
        Value::BulkString(b) => String::from_utf8(b.clone()).ok(),
        Value::SimpleString(s) => Some(s.clone()),
        _ => None,
    };
    match row {
        Value::Array(fields) => fields
            .as_chunks::<2>()
            .0
            .iter()
            .find(|[field, _]| as_string(field).as_deref() == Some(name))
            .and_then(|[_, value]| as_string(value)),
        Value::Map(fields) => fields
            .iter()
            .find(|(k, _)| as_string(k).as_deref() == Some(name))
            .and_then(|(_, v)| as_string(v)),
        _ => None,
    }
}

/// `HMGET key data version <index field> <sort field>`.
type HashRow = (
    Option<Vec<u8>>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// A read of one set, a page at a time, as a stream of decoded rows.
pub(crate) struct SortedRead<C, K: SchedulerStoreDecodeTo> {
    connection: C,
    set: String,
    index_value: String,
    page: u64,
    descending: bool,
    /// The last member read; the next page starts after it.
    after: Option<String>,
    rows: VecDeque<Result<<K as SchedulerStoreDecodeTo>::DecodeOutput, Error>>,
    done: bool,
    _index: PhantomData<fn() -> K>,
}

impl<C, K> SortedRead<C, K>
where
    C: ConnectionLike + Clone + Send + 'static,
    K: SchedulerIndexProvider + SchedulerStoreDecodeTo,
    <K as SchedulerStoreDecodeTo>::DecodeOutput: Send,
{
    pub(crate) fn new(
        connection: C,
        set: String,
        index_value: String,
        page: u64,
        descending: bool,
    ) -> Self {
        Self {
            connection,
            set,
            index_value,
            page: page.max(1),
            descending,
            after: None,
            rows: VecDeque::new(),
            done: false,
            _index: PhantomData,
        }
    }

    /// Whether the pages read so far produced any row.
    pub(crate) fn has_rows(&self) -> bool {
        !self.rows.is_empty()
    }

    /// Reads the next page into `rows`. Keyset paging: the next page starts
    /// strictly after the last member read, so nothing is held on the
    /// server between pages and nothing can expire there.
    pub(crate) async fn fetch_page(&mut self) -> Result<(), Error> {
        let (start, end) = match (&self.after, self.descending) {
            (Some(after), _) => (format!("({after}"), if self.descending { "-" } else { "+" }),
            (None, true) => ("+".to_string(), "-"),
            (None, false) => ("-".to_string(), "+"),
        };
        let mut range = redis::cmd("ZRANGE");
        range.arg(&self.set).arg(start).arg(end).arg("BYLEX");
        if self.descending {
            range.arg("REV");
        }
        range.arg("LIMIT").arg(0).arg(self.page);
        let members: Vec<String> = range
            .query_async(&mut self.connection)
            .await
            .map_err(Error::from)
            .err_tip(|| format!("In SortedRead::fetch_page ZRANGE {}", self.set))?;
        if (members.len() as u64) < self.page {
            self.done = true;
        }
        let Some(last) = members.last() else {
            return Ok(());
        };
        self.after = Some(last.clone());

        let mut pipe = redis::pipe();
        for member in &members {
            let key = split_member(member).map_or(member.as_str(), |(_, key)| key);
            pipe.cmd("HMGET")
                .arg(key)
                .arg(DATA_FIELD_NAME)
                .arg(VERSION_FIELD_NAME)
                .arg(K::INDEX_NAME)
                .arg(K::MAYBE_SORT_KEY.unwrap_or_default());
        }
        let rows: Vec<HashRow> = pipe
            .query_async(&mut self.connection)
            .await
            .map_err(Error::from)
            .err_tip(|| format!("In SortedRead::fetch_page HMGET {}", self.set))?;

        let mut stale = Vec::new();
        for (member, (data, version, value, sort)) in members.iter().zip(rows) {
            let current = split_member(member).is_some_and(|(member_sort, _)| {
                value.as_deref() == Some(self.index_value.as_str())
                    && sort.as_deref() == Some(member_sort)
            });
            match data {
                Some(data) if current => {
                    let version = version.and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
                    self.rows.push_back(
                        K::decode(version, Bytes::from(data))
                            .err_tip(|| "In SortedRead::fetch_page decode"),
                    );
                }
                _ => stale.push(member.clone()),
            }
        }
        if !stale.is_empty() {
            // Best effort: a member left behind is skipped again next read.
            let removed = remove_stale(
                &mut self.connection,
                &self.set,
                K::INDEX_NAME,
                K::MAYBE_SORT_KEY.unwrap_or_default(),
                &self.index_value,
                &stale,
            )
            .await;
            if let Err(err) = removed {
                warn!(set = %self.set, ?err, "Could not remove stale sorted-index members");
            }
        }
        Ok(())
    }

    pub(crate) fn into_stream(
        self,
    ) -> impl Stream<Item = Result<<K as SchedulerStoreDecodeTo>::DecodeOutput, Error>> + Send
    where
        K: Send,
    {
        futures::stream::unfold(self, |mut read| async move {
            loop {
                if let Some(row) = read.rows.pop_front() {
                    return Some((row, read));
                }
                if read.done {
                    return None;
                }
                if let Err(err) = read.fetch_page().await {
                    read.done = true;
                    return Some((Err(err), read));
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: SortedIndex = SortedIndex {
        key_prefix: "aa_",
        index_name: "state",
        sort_key: "sort_key",
    };

    fn spec(values: &[&str]) -> RedisSortedIndexSpec {
        RedisSortedIndexSpec {
            values: values.iter().map(ToString::to_string).collect(),
            read: true,
            reconcile_interval_s: 300,
        }
    }

    fn indexes(state: &str, sort: &str) -> Vec<(&'static str, Bytes)> {
        vec![
            ("unique_qualifier", Bytes::from_static(b"uq")),
            ("state", Bytes::from(state.to_string())),
            ("sort_key", Bytes::from(sort.to_string())),
        ]
    }

    #[test]
    fn names_never_share_the_hash_prefix() {
        assert_eq!(set_name("", &INDEX, "queued"), "nlsq:aa_state:queued");
        assert_eq!(set_name("p/", &INDEX, "queued"), "p/nlsq:aa_state:queued");
        assert!(!set_name("", &INDEX, "queued").starts_with(INDEX.key_prefix));
    }

    #[test]
    fn a_member_splits_back_into_its_halves() {
        let m = member("0000000080000000ffffffff", "aa_op:with:colons");
        assert_eq!(
            split_member(&m),
            Some(("0000000080000000ffffffff", "aa_op:with:colons"))
        );
    }

    #[test]
    fn member_order_is_sort_key_order() {
        // Lexicographic over members agrees with lexicographic over sort
        // keys, including across the 16- and 32-digit layouts a rolling
        // upgrade mixes.
        let mut members = [
            member("00000000800000010000000000000001", "aa_b"),
            member("80000000fffffffe", "aa_old"),
            member("00000000800000000000000000000002", "aa_a"),
        ];
        members.sort();
        let sorts: Vec<_> = members.iter().map(|m| split_member(m).unwrap().0).collect();
        let mut expected = sorts.clone();
        expected.sort_unstable();
        assert_eq!(sorts, expected);
        assert_eq!(sorts.last(), Some(&"80000000fffffffe"));
    }

    #[test]
    fn a_listed_value_without_expiry_is_added() {
        let state = SortedIndexState::new(&spec(&["queued"]));
        let w = state.write_args("", &INDEX, &indexes("queued", "00ff"), "aa_op1", false);
        assert_eq!(w.set_prefix, "nlsq:aa_state:");
        assert_eq!(w.new_set, "nlsq:aa_state:queued");
        assert_eq!(w.new_member, "00ff:aa_op1");
        assert_eq!((w.index_field, w.sort_field), ("state", "sort_key"));
    }

    #[test]
    fn an_unlisted_value_or_an_expiring_write_is_only_removed() {
        let state = SortedIndexState::new(&spec(&["queued"]));
        for (value, expiry) in [("executing", false), ("completed", true), ("queued", true)] {
            let w = state.write_args("", &INDEX, &indexes(value, "00ff"), "aa_op1", expiry);
            assert_eq!(w.new_set, "", "{value} expiry={expiry}");
            assert_eq!(w.new_member, "");
            // The old member still goes, wherever it was.
            assert_eq!(w.set_prefix, "nlsq:aa_state:");
        }
    }

    #[test]
    fn only_listed_values_of_sorted_indexes_are_readable() {
        struct Sorted;
        impl SchedulerIndexProvider for Sorted {
            const KEY_PREFIX: &'static str = "aa_";
            const INDEX_NAME: &'static str = "state";
            const MAYBE_SORT_KEY: Option<&'static str> = Some("sort_key");
            type Versioned = nativelink_util::store_trait::TrueValue;
            fn index_value(&self) -> std::borrow::Cow<'_, str> {
                "".into()
            }
        }
        struct Unsorted;
        impl SchedulerIndexProvider for Unsorted {
            const KEY_PREFIX: &'static str = "aa_";
            const INDEX_NAME: &'static str = "unique_qualifier";
            type Versioned = nativelink_util::store_trait::TrueValue;
            fn index_value(&self) -> std::borrow::Cow<'_, str> {
                "".into()
            }
        }
        let state = SortedIndexState::new(&spec(&["queued"]));
        assert_eq!(
            state.readable_set::<Sorted>("", "queued").as_deref(),
            Some("nlsq:aa_state:queued")
        );
        assert_eq!(state.readable_set::<Sorted>("", "completed"), None);
        assert_eq!(state.readable_set::<Sorted>("", ""), None);
        assert_eq!(state.readable_set::<Unsorted>("", "queued"), None);

        let write_only = SortedIndexState::new(&RedisSortedIndexSpec {
            read: false,
            ..spec(&["queued"])
        });
        assert_eq!(write_only.readable_set::<Sorted>("", "queued"), None);
    }

    #[test]
    fn a_set_is_read_only_after_a_reconcile_succeeds() {
        let state = SortedIndexState::new(&spec(&["queued"]));
        let set = "nlsq:aa_state:queued";
        assert!(!state.has_reconciled(set));
        assert!(state.claim_reconcile(set));
        // Claimed: a second reader in this process does not walk too.
        assert!(!state.claim_reconcile(set));
        state.finish_reconcile(set, false);
        assert!(!state.has_reconciled(set));
        // A failure is retried after RECONCILE_RETRY, not by the next read.
        assert!(!state.claim_reconcile(set));
        state.finish_reconcile(set, true);
        assert!(state.has_reconciled(set));
        // A later failure does not unread a set that reconciled once.
        state.finish_reconcile(set, false);
        assert!(state.has_reconciled(set));
    }

    #[test]
    fn rows_parse_in_both_protocols() {
        let bulk = |s: &str| Value::BulkString(s.as_bytes().to_vec());
        let resp2 = Value::Array(vec![
            bulk("__key"),
            bulk("aa_1"),
            bulk("sort_key"),
            bulk("00ff"),
        ]);
        let resp3 = Value::Map(vec![
            (bulk("__key"), bulk("aa_1")),
            (bulk("sort_key"), bulk("00ff")),
        ]);
        for row in [resp2, resp3] {
            assert_eq!(row_field(&row, "__key").as_deref(), Some("aa_1"));
            assert_eq!(row_field(&row, "sort_key").as_deref(), Some("00ff"));
            assert_eq!(row_field(&row, "state"), None);
        }
    }

    /// Against a real Redis when `NATIVELINK_REDIS_TEST_URL` is set; the
    /// fake does not run Lua. A member that a read judged stale but whose
    /// action has since been written back under it must stay.
    #[nativelink_macro::nativelink_test]
    async fn removing_stale_members_rechecks_each_hash() {
        let Ok(url) = std::env::var("NATIVELINK_REDIS_TEST_URL") else {
            return;
        };
        let client = redis::Client::open(url).unwrap();
        let mut con = client.get_multiplexed_async_connection().await.unwrap();
        let set = "nlsq:test_remove_stale";
        let (requeued, moved, gone) = (
            "00000000000000000000000000000001:aa_test_requeued",
            "00000000000000000000000000000002:aa_test_moved",
            "00000000000000000000000000000003:aa_test_gone",
        );
        let _: () = redis::pipe()
            .del(&[set, "aa_test_requeued", "aa_test_moved", "aa_test_gone"])
            .zadd(set, requeued, 0)
            .zadd(set, moved, 0)
            .zadd(set, gone, 0)
            // Back in the queue under the same member since the read.
            .hset_multiple(
                "aa_test_requeued",
                &[
                    (DATA_FIELD_NAME, "x"),
                    ("state", "queued"),
                    ("sort_key", "00000000000000000000000000000001"),
                ],
            )
            // Executing now.
            .hset_multiple(
                "aa_test_moved",
                &[
                    (DATA_FIELD_NAME, "x"),
                    ("state", "executing"),
                    ("sort_key", "00000000000000000000000000000002"),
                ],
            )
            .query_async(&mut con)
            .await
            .unwrap();
        let members = [requeued, moved, gone].map(String::from);
        let removed = remove_stale(&mut con, set, "state", "sort_key", "queued", &members)
            .await
            .unwrap();
        assert_eq!(removed, 2);
        let left: Vec<String> = redis::cmd("ZRANGE")
            .arg(set)
            .arg(0)
            .arg(-1)
            .query_async(&mut con)
            .await
            .unwrap();
        assert_eq!(left, vec![requeued.to_string()]);
        let _: () = redis::cmd("DEL")
            .arg(&[set, "aa_test_requeued", "aa_test_moved"])
            .query_async(&mut con)
            .await
            .unwrap();
    }
}
