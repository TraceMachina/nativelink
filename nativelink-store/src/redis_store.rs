// Copyright 2024-2026 The NativeLink Authors. All rights reserved.
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

use core::cmp;
use core::fmt::Debug;
use core::marker::PhantomData;
use core::ops::{Bound, RangeBounds};
use core::pin::Pin;
use core::str::FromStr;
use core::time::Duration;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Weak};
use std::time::Instant;

use async_trait::async_trait;
use bytes::Bytes;
use const_format::formatcp;
use futures::stream::FuturesUnordered;
use futures::{Stream, StreamExt, TryFutureExt, TryStreamExt, future};
use itertools::izip;
use nativelink_config::stores::{RedisMode, RedisSortedIndexSpec, RedisSpec};
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_metric::MetricsComponent;
use nativelink_redis_tester::SubscriptionManagerNotify;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::metrics::{record_connection_acquired, record_connection_reconnect};
use nativelink_util::store_trait::{
    BoolValue, RemoveCallback, SchedulerCurrentVersionProvider, SchedulerIndexProvider,
    SchedulerStore, SchedulerStoreDataProvider, SchedulerStoreDecodeTo, SchedulerStoreKeyProvider,
    SchedulerSubscription, SchedulerSubscriptionManager, SortedIndex, StoreDriver, StoreKey,
    UploadSizeInfo,
};
use nativelink_util::task::JoinHandleDropGuard;
use nativelink_util::{background_spawn, spawn};
use parking_lot::{Mutex, RwLock};
use patricia_tree::StringPatriciaMap;
use redis::aio::{ConnectionLike, ConnectionManager, ConnectionManagerConfig};
use redis::cluster::ClusterClient;
use redis::cluster_async::ClusterConnection;
use redis::sentinel::{SentinelClient, SentinelNodeConnectionInfo, SentinelServerType};
use redis::{
    AsyncCommands, AsyncIter, Client, IntoConnectionInfo, PushInfo, ScanOptions, Script, SetExpiry,
    SetOptions, Value, pipe,
};
use serde::Deserialize;
use serde::de::IntoDeserializer;
use tokio::select;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::sync::{OnceCell, OwnedSemaphorePermit, Semaphore};
use tokio::time::{sleep, timeout};
use tokio_stream::wrappers::UnboundedReceiverStream;
use tracing::{debug, error, info, trace, warn};
use url::Url;
use uuid::Uuid;

use crate::cas_utils::is_zero_digest;
use crate::redis_utils::index_migration::{
    self, IndexKind, Replacement, Step, drop_keeping_documents, index_kind, is_out_of_memory,
    migrated_name,
};
use crate::redis_utils::sorted_index::{self, SortedIndexState, SortedRead};
use crate::redis_utils::{
    FtAggregateCursor, FtAggregateOptions, FtCreateOptions, SearchSchema, ft_aggregate, ft_create,
    ft_info, ft_info_indexing, ft_search_count,
};

/// The default size of the read chunk when reading data from Redis.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_READ_CHUNK_SIZE: usize = 64 * 1024;

/// The default size of the connection pool if not specified.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_CONNECTION_POOL_SIZE: usize = 3;

/// The default delay between retries if not specified.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_RETRY_DELAY: f32 = 0.1;

/// Maximum attempts (re-resolving the master each time) for an operation to
/// ride out a transient Redis topology change — a failover, a dropped
/// connection, or brief replica lag. Used by `update`'s verify and by the read
/// paths so a transient error doesn't fail an otherwise healthy request.
const MAX_REDIS_RETRY_ATTEMPTS: u32 = 5;

/// Whether a Redis error is worth re-resolving the master and retrying: a
/// dropped/refused connection, an IO error, or a write that hit a freshly
/// demoted replica. Anything else (e.g. a real protocol/logic error) is
/// returned as-is.
fn is_retryable_redis_error(err: &redis::RedisError) -> bool {
    err.is_connection_dropped()
        || err.is_connection_refusal()
        || err.is_io_error()
        || err.kind() == redis::ErrorKind::Server(redis::ServerErrorKind::ReadOnly)
}

/// The default connection timeout in milliseconds if not specified.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_CONNECTION_TIMEOUT_MS: u64 = 3000;

/// The default command timeout in milliseconds if not specified.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 10_000;

/// The default `check_health` PING ceiling in milliseconds if not specified.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_HEALTH_CHECK_TIMEOUT_MS: u64 = 4000;

/// The default maximum number of chunk uploads per update.
/// Note: If this changes it should be updated in the config documentation.
pub const DEFAULT_MAX_CHUNK_UPLOADS_PER_UPDATE: usize = 10;

/// The default COUNT value passed when scanning keys in Redis.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_SCAN_COUNT: usize = 10_000;

/// The default COUNT value passed when scanning search indexes
/// Note: If this changes it should be updated in the config documentation.
pub const DEFAULT_MAX_COUNT_PER_CURSOR: u64 = 1_500;

const DEFAULT_CLIENT_PERMITS: usize = 500;

/// Converts a TTL to the seconds `EXPIRE` takes.
///
/// Clamped to at least one second. Redis treats `EXPIRE` with a value of zero
/// or less as "delete now", so a sub-second duration reaching here would
/// destroy the value it was meant to protect. Config already rejects zero, so
/// this only guards a future caller.
fn ttl_seconds(key_ttl: Duration) -> i64 {
    i64::try_from(key_ttl.as_secs()).unwrap_or(i64::MAX).max(1)
}

/// Lifetime of an upload's temp key on a store without `key_ttl_s`. Every
/// chunk arms it again, so it bounds the gap between two chunks, not the
/// upload.
///
/// Longer than any pause a client is allowed between chunks (Bazel's
/// `--remote_timeout` is 60 s by default, and a resumed upload waits at most
/// the `ByteStream` server's disconnect timeout), and short enough that the
/// uploads in flight when a CAS dies are gone within the hour. An upload that
/// goes quiet for longer fails rather than publishing a key that was
/// recreated without its first chunks.
const TEMP_KEY_TTL: Duration = Duration::from_hours(1);

/// `RENAME` of a key that is not there. On a retried publish that means
/// the first attempt applied before its reply was lost, or the failover
/// lost the temp key; the caller tells the two apart by the final key.
fn is_no_such_key(err: &redis::RedisError) -> bool {
    err.kind() == redis::ErrorKind::Server(redis::ServerErrorKind::ResponseError)
        && err
            .detail()
            .is_some_and(|detail| detail.contains("no such key"))
}

/// A wrapper around Redis to allow it to be reconnected.
pub trait RedisManager<C>
where
    C: ConnectionLike + Clone,
{
    /// Get a connection manager and a unique identifier for this connection
    /// which may be used to issue a reconnect later.
    fn get_connection(&self) -> impl Future<Output = Result<(C, Uuid), Error>> + Send;

    /// Reconnect if the uuid matches the uuid returned from `get_connection()`.
    fn reconnect(&self, uuid: Uuid) -> impl Future<Output = Result<(C, Uuid), Error>> + Send;

    /// Get an invocation of the update version script for a given `key`.
    fn update_script(&self, key: &str) -> redis::ScriptInvocation<'_>;

    /// Configure the connection to have a psubscribe on it and perform the
    /// subscription on reconnect.
    fn psubscribe(&self, pattern: &str) -> impl Future<Output = Result<(), Error>> + Send;
}

#[derive(Debug)]
pub struct ClusterRedisManager<C>
where
    C: ConnectionLike + Clone,
{
    /// A constant Uuid, we never reconnect.
    uuid: Uuid,

    /// Redis script used to update a value in redis if the version matches.
    /// This is done by incrementing the version number and then setting the new
    /// data only if the version number matches the existing version number.
    update_if_version_matches_script: Script,

    /// The client pool connecting to the backing Redis instance(s).
    connection_manager: C,
}

impl<C> ClusterRedisManager<C>
where
    C: ConnectionLike + Clone,
{
    pub async fn new(mut connection_manager: C) -> Result<Self, Error> {
        let update_if_version_matches_script = Script::new(LUA_VERSION_SET_SCRIPT);
        update_if_version_matches_script
            .load_async(&mut connection_manager)
            .await?;
        Ok(Self {
            uuid: Uuid::new_v4(),
            update_if_version_matches_script,
            connection_manager,
        })
    }
}

impl<C> RedisManager<C> for ClusterRedisManager<C>
where
    C: ConnectionLike + Clone + Send + Sync,
{
    fn get_connection(&self) -> impl Future<Output = Result<(C, Uuid), Error>> + Send {
        future::ready(Ok((self.connection_manager.clone(), self.uuid)))
    }

    fn reconnect(&self, _uuid: Uuid) -> impl Future<Output = Result<(C, Uuid), Error>> + Send {
        self.get_connection()
    }

    fn update_script(&self, key: &str) -> redis::ScriptInvocation<'_> {
        self.update_if_version_matches_script.key(key)
    }

    fn psubscribe(&self, _pattern: &str) -> impl Future<Output = Result<(), Error>> + Send {
        // This is a no-op for cluster connections.
        future::ready(Ok(()))
    }
}

type RedisConnectFuture<C> = dyn Future<Output = Result<C, Error>> + Send;
type RedisConnectFn<C> = dyn Fn() -> Pin<Box<RedisConnectFuture<C>>> + Send + Sync;

pub struct StandardRedisManager<C>
where
    C: ConnectionLike + Clone,
{
    /// Function used to re-connect to Redis.
    connect_func: Box<RedisConnectFn<C>>,

    /// Redis script used to update a value in redis if the version matches.
    /// This is done by incrementing the version number and then setting the new
    /// data only if the version number matches the existing version number.
    update_if_version_matches_script: Script,

    /// The client pool connecting to the backing Redis instance(s) and a Uuid
    /// for this connection in order to avoid multiple reconnection attempts.
    connection_manager: tokio::sync::RwLock<(C, Uuid)>,

    /// Serializes reconnect attempts so a Sentinel master failover triggers a
    /// single re-resolution instead of a thundering herd. Kept separate from
    /// `connection_manager` on purpose: the (potentially multi-second) connect
    /// runs while holding only this lock, so in-flight `get_connection` readers
    /// keep using the existing handle and are never frozen behind a slow
    /// reconnect. See [`Self::reconnect`].
    reconnect_lock: tokio::sync::Mutex<()>,

    /// A list of subscription that should be performed on reconnect.
    subscriptions: Mutex<HashSet<String>>,
}

impl<C> Debug for StandardRedisManager<C>
where
    C: ConnectionLike + Clone,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StandardRedisManager")
            .field(
                "update_if_version_matches_script",
                &self.update_if_version_matches_script,
            )
            .field("subscriptions", &self.subscriptions)
            .finish()
    }
}

impl<C> StandardRedisManager<C>
where
    C: ConnectionLike + Clone + Send + Sync,
{
    async fn configure(&self, connection_manager: &mut C) -> Result<(), Error> {
        self.update_if_version_matches_script
            .load_async(connection_manager)
            .await?;
        Ok(())
    }

    async fn new(connect_func: Box<RedisConnectFn<C>>) -> Result<Self, Error> {
        let connection_manager = connect_func().await?;
        let update_if_version_matches_script = Script::new(LUA_VERSION_SET_SCRIPT);
        let connection = Self {
            connect_func,
            update_if_version_matches_script,
            connection_manager: tokio::sync::RwLock::new((connection_manager, Uuid::new_v4())),
            reconnect_lock: tokio::sync::Mutex::new(()),
            subscriptions: Mutex::new(HashSet::new()),
        };
        {
            let mut connection_manager = connection.connection_manager.write().await;
            connection.configure(&mut connection_manager.0).await?;
        }
        Ok(connection)
    }
}

impl RedisManager<ConnectionManager> for StandardRedisManager<ConnectionManager> {
    async fn get_connection(&self) -> Result<(ConnectionManager, Uuid), Error> {
        Ok(self.connection_manager.read().await.clone())
    }

    async fn reconnect(&self, uuid: Uuid) -> Result<(ConnectionManager, Uuid), Error> {
        // Fast path: another caller already reconnected past this generation,
        // so the handle is fresh — hand it back without re-resolving.
        {
            let guard = self.connection_manager.read().await;
            if guard.1 != uuid {
                return Ok(guard.clone());
            }
        }
        // Serialize reconnect attempts on a dedicated lock so a failover storm
        // resolves the new master once. Critically this is NOT the
        // `connection_manager` lock, so the slow connect below does not block
        // the `get_connection` readers that every Redis op needs — they keep
        // using the old (now-failing) handle and fail fast on their own
        // command timeout instead of all freezing behind one reconnect.
        let _reconnect_guard = self.reconnect_lock.lock().await;
        // Re-check: a reconnect may have completed while we waited for the lock.
        {
            let guard = self.connection_manager.read().await;
            if guard.1 != uuid {
                return Ok(guard.clone());
            }
        }
        record_connection_reconnect("redis");
        let mut connection_manager = (self.connect_func)().await?;
        let new_uuid = Uuid::new_v4();
        self.configure(&mut connection_manager).await?;
        let subscriptions = {
            let guard = self.subscriptions.lock();
            guard.iter().map(Clone::clone).collect::<Vec<_>>()
        };
        for subscription in &subscriptions {
            connection_manager.psubscribe(subscription).await?;
        }
        let mut replayed = subscriptions.into_iter().collect::<HashSet<_>>();
        // Publish the new handle under a brief exclusive lock. Before
        // publishing, replay any patterns registered since the snapshot above:
        // `psubscribe` records its pattern in `self.subscriptions` *before* it
        // fetches a connection, so a pattern missing from this final delta was
        // inserted while we hold the write lock, and its `get_connection` will
        // block here and observe the new handle — it subscribes on the live
        // connection itself. Either way there is no window where a pattern is
        // recorded as active but subscribed only on the old connection.
        {
            let mut guard = self.connection_manager.write().await;
            let missing = {
                let subs = self.subscriptions.lock();
                subs.iter()
                    .filter(|s| !replayed.contains(*s))
                    .cloned()
                    .collect::<Vec<_>>()
            };
            for subscription in missing {
                connection_manager.psubscribe(&subscription).await?;
                replayed.insert(subscription);
            }
            *guard = (connection_manager.clone(), new_uuid);
        }
        info!(old = %uuid, new = %new_uuid, "StandardRedisManager re-resolved the Redis master");
        Ok((connection_manager, new_uuid))
    }

    fn update_script(&self, key: &str) -> redis::ScriptInvocation<'_> {
        self.update_if_version_matches_script.key(key)
    }

    async fn psubscribe(&self, pattern: &str) -> Result<(), Error> {
        debug!(pattern, "new psubscribe");
        // Record the pattern BEFORE fetching a connection. A concurrent
        // `reconnect` either sees the pattern in the snapshot it replays under
        // its pre-publish write lock (subscribing it on the new connection),
        // or it published first — in which case `get_connection` below returns
        // the new live connection and we subscribe on it ourselves. Inserting
        // after fetching the connection (the previous order) left a window
        // where the pattern was recorded as active but PSUBSCRIBEd only on the
        // old, about-to-be-replaced connection.
        let new_subscription = self.subscriptions.lock().insert(String::from(pattern));
        if new_subscription {
            let result = async {
                let mut connection = self.get_connection().await?.0;
                connection.psubscribe(pattern).await.map_err(Error::from)
            }
            .await;
            if result.is_err() {
                self.subscriptions.lock().remove(pattern);
            }
            result?;
        }
        debug!(pattern, new_subscription, "new psubscribe complete");
        Ok(())
    }
}

/// A [`StoreDriver`] implementation that uses Redis as a backing store.
#[derive(MetricsComponent)]
pub struct RedisStore<C, M>
where
    C: ConnectionLike + Clone,
    M: RedisManager<C>,
{
    /// The client pool connecting to the backing Redis instance(s).
    connection_manager: M,

    /// The underlying connection type in the connection manager.
    _connection_type: PhantomData<C>,

    /// A channel to publish updates to when a key is added, removed, or modified.
    #[metric(
        help = "The pubsub channel to publish updates to when a key is added, removed, or modified"
    )]
    pub_sub_channel: Option<String>,

    /// A function used to generate names for temporary keys.
    temp_name_generator_fn: fn() -> String,

    /// A common prefix to append to all keys before they are sent to Redis.
    ///
    /// See [`RedisStore::key_prefix`](`nativelink_config::stores::RedisStore::key_prefix`).
    #[metric(help = "Prefix to append to all keys before sending to Redis")]
    key_prefix: String,

    /// The amount of data to read from Redis at a time.
    #[metric(help = "The amount of data to read from Redis at a time")]
    read_chunk_size: usize,

    /// The maximum number of chunk uploads per update.
    /// This is used to limit the number of chunk uploads per update to prevent
    /// overloading when uploading large blocks of data
    #[metric(help = "The maximum number of chunk uploads per update")]
    max_chunk_uploads_per_update: usize,

    /// The COUNT value passed when scanning keys in Redis.
    /// This is used to hint the amount of work that should be done per response.
    #[metric(help = "The COUNT value passed when scanning keys in Redis")]
    scan_count: usize,

    /// The COUNT value used with search indexes
    #[metric(help = "The maximum number of results to return per cursor")]
    max_count_per_cursor: u64,

    /// See [`RedisSpec::migrate_temporary_indexes`].
    migrate_temporary_indexes: bool,

    /// For each scheduler index, the name reads use and until when that
    /// answer holds. A settled answer is rechecked after
    /// `INDEX_MIGRATION_RECHECK`, so an original that a process without the
    /// migration recreates is retired again.
    index_names: Mutex<HashMap<String, (String, Instant)>>,
    /// See [`RedisSpec::experimental_sorted_index`]. `None` leaves every
    /// read and write exactly as without it.
    sorted_index: Option<SortedIndexState>,

    /// [`LUA_VERSION_SET_SORTED_SCRIPT`], invoked only with `sorted_index`.
    /// Loaded on first use (`EVALSHA` falls back to loading it).
    sorted_script: Script,

    /// A manager for subscriptions to keys in Redis.
    subscription_manager: Arc<RedisSubscriptionManager>,

    /// Permits to limit inflight Redis requests. Technically only
    /// limits the calls to `get_client()`, but the requests per client
    /// are small enough that it works well enough.
    client_permits: Arc<Semaphore>,

    /// Per-call ceiling for `check_health` PING.
    health_check_timeout: Duration,

    /// Expire keys this store writes after this long. `None` keeps them
    /// forever, which is the default and what every store other than a BEP
    /// store wants.
    key_ttl: Option<Duration>,

    /// Have we done a subscribe for messages for `remove_callback` subscribes?
    has_remove_callback_subscribe: OnceCell<()>,

    remove_callbacks: Arc<async_lock::Mutex<Vec<RemoveCallback>>>,
}

impl<C, M> Debug for RedisStore<C, M>
where
    C: ConnectionLike + Clone,
    M: RedisManager<C>,
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RedisStore")
            .field("temp_name_generator_fn", &self.temp_name_generator_fn)
            .field("key_prefix", &self.key_prefix)
            .field("read_chunk_size", &self.read_chunk_size)
            .field(
                "max_chunk_uploads_per_update",
                &self.max_chunk_uploads_per_update,
            )
            .field("scan_count", &self.scan_count)
            .field("subscription_manager", &self.subscription_manager)
            .field("client_permits", &self.client_permits)
            .finish()
    }
}

struct ClientWithPermit<C: ConnectionLike> {
    connection_manager: C,
    uuid: Uuid,

    // here so it sticks around with the client and doesn't get dropped until that does
    #[allow(dead_code)]
    semaphore_permit: OwnedSemaphorePermit,
}

impl<C: ConnectionLike + Clone> ClientWithPermit<C> {
    async fn reconnect<M: RedisManager<C> + Sync>(&mut self, manager: &M) -> Result<(), Error> {
        (self.connection_manager, self.uuid) = manager.reconnect(self.uuid).await?;
        Ok(())
    }
}

impl<C: ConnectionLike> Drop for ClientWithPermit<C> {
    fn drop(&mut self) {
        trace!(
            remaining = self.semaphore_permit.semaphore().available_permits(),
            "Dropping a client permit"
        );
    }
}

/// Decode a `StoreKey` coming from Redis
pub fn decode_key<'a>(
    key_prefix: &String,
    encoded_key: Cow<'a, str>,
) -> Result<StoreKey<'a>, Error> {
    let key_no_prefix = if key_prefix.is_empty() {
        encoded_key
    } else {
        match encoded_key.strip_prefix(key_prefix) {
            Some(r) => Cow::from(r.to_string()),
            None => {
                return Err(make_err!(
                    Code::InvalidArgument,
                    "Redis key ({}) is missing prefix ({})",
                    encoded_key,
                    key_prefix
                ));
            }
        }
    };
    let maybe_digest_info: Result<_, serde::de::value::Error> =
        DigestInfo::deserialize(key_no_prefix.clone().into_deserializer());
    if let Ok(digest_info) = maybe_digest_info {
        Ok(StoreKey::Digest(digest_info))
    } else {
        Ok(StoreKey::Str(key_no_prefix))
    }
}

impl<C, M> RedisStore<C, M>
where
    C: ConnectionLike + Clone + Sync,
    M: RedisManager<C> + Sync,
{
    /// Used for testing when determinism is required.
    #[expect(clippy::too_many_arguments)]
    pub async fn new_from_builder_and_parts(
        pub_sub_channel: Option<String>,
        temp_name_generator_fn: fn() -> String,
        key_prefix: String,
        read_chunk_size: usize,
        max_chunk_uploads_per_update: usize,
        scan_count: usize,
        max_client_permits: usize,
        max_count_per_cursor: u64,
        health_check_timeout: Duration,
        key_ttl: Option<Duration>,
        subscriber_channel: UnboundedReceiver<PushInfo>,
        connection_manager: M,
    ) -> Result<Self, Error> {
        info!("Redis index fingerprint: {FINGERPRINT_CREATE_INDEX_HEX}");
        let remove_callbacks = Arc::new(async_lock::Mutex::new(Vec::new()));
        let subscription_manager = Arc::new(RedisSubscriptionManager::new(
            subscriber_channel,
            remove_callbacks.clone(),
            key_prefix.clone(),
        ));
        if let Some(channel) = &pub_sub_channel {
            connection_manager.psubscribe(channel).await?;
        }

        Ok(Self {
            connection_manager,
            _connection_type: PhantomData,
            pub_sub_channel,
            temp_name_generator_fn,
            key_prefix,
            read_chunk_size,
            max_chunk_uploads_per_update,
            scan_count,
            subscription_manager,
            client_permits: Arc::new(Semaphore::new(max_client_permits)),
            max_count_per_cursor,
            migrate_temporary_indexes: false,
            index_names: Mutex::new(HashMap::new()),
            sorted_index: None,
            sorted_script: Script::new(LUA_VERSION_SET_SORTED_SCRIPT),
            health_check_timeout,
            key_ttl,
            has_remove_callback_subscribe: OnceCell::const_new(),
            remove_callbacks,
        })
    }

    /// Turns on [`RedisSpec::migrate_temporary_indexes`].
    #[must_use]
    pub fn with_temporary_index_migration(mut self) -> Self {
        info!("Redis temporary-index migration enabled");
        self.migrate_temporary_indexes = true;
        self
    }

    /// Turns on [`RedisSpec::experimental_sorted_index`]. Not for Cluster:
    /// the write script reaches a set it cannot declare as a key.
    #[must_use]
    pub fn with_sorted_index(mut self, spec: &RedisSortedIndexSpec) -> Self {
        info!(?spec, "Redis experimental sorted index enabled");
        self.sorted_index = Some(SortedIndexState::new(spec));
        self
    }

    /// `STRLEN` of a key, retried on a transient error like the reads are.
    async fn strlen(
        &self,
        client: &mut ClientWithPermit<C>,
        encoded_key: &str,
    ) -> Result<u64, Error> {
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            match redis::cmd("STRLEN")
                .arg(encoded_key)
                .query_async::<u64>(&mut client.connection_manager)
                .await
            {
                Ok(v) => return Ok(v),
                Err(err)
                    if attempt < MAX_REDIS_RETRY_ATTEMPTS && is_retryable_redis_error(&err) =>
                {
                    client.reconnect(&self.connection_manager).await?;
                    sleep(Duration::from_secs_f32(DEFAULT_RETRY_DELAY)).await;
                }
                Err(err) => {
                    return Err(Error::from(err).append("In RedisStore::get_part::strlen"));
                }
            }
        }
    }

    async fn get_client(&self) -> Result<ClientWithPermit<C>, Error> {
        let local_client_permits = self.client_permits.clone();
        let remaining = local_client_permits.available_permits();
        // Zero here means every client is busy and this call is about to wait,
        // which is the saturation signal worth alerting on.
        record_connection_acquired("redis", Some(remaining), remaining == 0);
        let semaphore_permit = local_client_permits.acquire_owned().await?;
        trace!(remaining, "Got a client permit");
        let (connection_manager, uuid) = self.connection_manager.get_connection().await?;
        Ok(ClientWithPermit {
            connection_manager,
            uuid,
            semaphore_permit,
        })
    }

    /// Encode a `StoreKey` so it can be sent to Redis.
    pub fn encode_key<'a>(&self, key: &'a StoreKey<'a>) -> Cow<'a, str> {
        let key_body = key.as_str();
        if self.key_prefix.is_empty() {
            key_body
        } else {
            // This is in the hot path for all redis operations, so we try to reuse the allocation
            // from `key.as_str()` if possible.
            match key_body {
                Cow::Owned(mut encoded_key) => {
                    encoded_key.insert_str(0, &self.key_prefix);
                    Cow::Owned(encoded_key)
                }
                Cow::Borrowed(body) => {
                    let mut encoded_key = String::with_capacity(self.key_prefix.len() + body.len());
                    encoded_key.push_str(&self.key_prefix);
                    encoded_key.push_str(body);
                    Cow::Owned(encoded_key)
                }
            }
        }
    }

    fn set_spec_defaults(spec: &mut RedisSpec) -> Result<(), Error> {
        if spec.addresses.is_empty() {
            return Err(make_err!(
                Code::InvalidArgument,
                "No addresses were specified in redis store configuration."
            ));
        }

        if spec.broadcast_channel_capacity != 0 {
            warn!("broadcast_channel_capacity in Redis spec is deprecated and ignored");
        }
        if spec.response_timeout_s != 0 {
            warn!(
                "response_timeout_s in Redis spec is deprecated and ignored, use command_timeout_ms"
            );
        }
        if spec.connection_timeout_s != 0 {
            if spec.connection_timeout_ms != 0 {
                return Err(make_err!(
                    Code::InvalidArgument,
                    "Both connection_timeout_s and connection_timeout_ms were set, can only have one!"
                ));
            }
            warn!("connection_timeout_s in Redis spec is deprecated, use connection_timeout_ms");
            spec.connection_timeout_ms = spec.connection_timeout_s * 1000;
        }
        if spec.connection_timeout_ms == 0 {
            spec.connection_timeout_ms = DEFAULT_CONNECTION_TIMEOUT_MS;
        }
        if spec.command_timeout_ms == 0 {
            spec.command_timeout_ms = DEFAULT_COMMAND_TIMEOUT_MS;
        }
        if spec.health_check_timeout_ms == 0 {
            spec.health_check_timeout_ms = DEFAULT_HEALTH_CHECK_TIMEOUT_MS;
        }
        if spec.connection_pool_size == 0 {
            spec.connection_pool_size = DEFAULT_CONNECTION_POOL_SIZE;
        }
        if spec.read_chunk_size == 0 {
            spec.read_chunk_size = DEFAULT_READ_CHUNK_SIZE;
        }
        if spec.max_count_per_cursor == 0 {
            spec.max_count_per_cursor = DEFAULT_MAX_COUNT_PER_CURSOR;
        }
        if spec.max_chunk_uploads_per_update == 0 {
            spec.max_chunk_uploads_per_update = DEFAULT_MAX_CHUNK_UPLOADS_PER_UPDATE;
        }
        if spec.scan_count == 0 {
            spec.scan_count = DEFAULT_SCAN_COUNT;
        }
        if spec.max_client_permits == 0 {
            spec.max_client_permits = DEFAULT_CLIENT_PERMITS;
        }
        if spec.retry.delay == 0.0 {
            spec.retry.delay = DEFAULT_RETRY_DELAY;
        }
        if spec.retry.max_retries == 0 {
            spec.retry.max_retries = 1;
        }
        trace!(?spec, "redis spec is after setting defaults");
        Ok(())
    }

    // Only used by tests, because we need to make a real redis connection, then fix this to get fixed values
    pub fn replace_temp_name_generator(&mut self, replacement: fn() -> String) {
        self.temp_name_generator_fn = replacement;
    }
}

impl RedisStore<ClusterConnection, ClusterRedisManager<ClusterConnection>> {
    pub async fn new_cluster(mut spec: RedisSpec) -> Result<Arc<Self>, Error> {
        if spec.experimental_sorted_index.is_some() {
            return Err(make_err!(
                Code::InvalidArgument,
                "experimental_sorted_index is not supported in cluster mode"
            ));
        }
        let migrate_temporary_indexes = spec.migrate_temporary_indexes;
        if spec.mode != RedisMode::Cluster {
            return Err(Error::new(
                Code::InvalidArgument,
                "new_cluster only works for Cluster mode".to_string(),
            ));
        }
        Self::set_spec_defaults(&mut spec)?;

        let parsed_addrs: Vec<_> = spec
            .addresses
            .iter_mut()
            .map(|addr| {
                addr.clone().into_connection_info().map(|connection_info| {
                    let redis_settings = connection_info
                        .redis_settings()
                        .clone()
                        // We need RESP3 here because the cluster mode doesn't support RESP2 pubsub
                        // See also https://docs.rs/redis/latest/redis/cluster_async/index.html#pubsub
                        .set_protocol(redis::ProtocolVersion::RESP3);
                    connection_info.set_redis_settings(redis_settings)
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let connection_timeout = Duration::from_millis(spec.connection_timeout_ms);
        let command_timeout = Duration::from_millis(spec.command_timeout_ms);
        let (tx, subscriber_channel) = unbounded_channel();

        let builder = ClusterClient::builder(parsed_addrs)
            .connection_timeout(connection_timeout)
            .response_timeout(command_timeout)
            .push_sender(tx)
            .retries(u32::try_from(spec.retry.max_retries)?);

        let client = builder.build()?;

        Self::new_from_builder_and_parts(
            spec.experimental_pub_sub_channel,
            || Uuid::new_v4().to_string(),
            spec.key_prefix.clone(),
            spec.read_chunk_size,
            spec.max_chunk_uploads_per_update,
            spec.scan_count,
            spec.max_client_permits,
            spec.max_count_per_cursor,
            Duration::from_millis(spec.health_check_timeout_ms),
            (spec.key_ttl_s > 0).then(|| Duration::from_secs(spec.key_ttl_s)),
            subscriber_channel,
            ClusterRedisManager::new(client.get_async_connection().await?).await?,
        )
        .await
        .map(|store| {
            if migrate_temporary_indexes {
                store.with_temporary_index_migration()
            } else {
                store
            }
        })
        .map(Arc::new)
    }
}

impl RedisStore<ConnectionManager, StandardRedisManager<ConnectionManager>> {
    async fn connect(
        spec: RedisSpec,
        tx: UnboundedSender<PushInfo>,
    ) -> Result<ConnectionManager, Error> {
        let connection_timeout = Duration::from_millis(spec.connection_timeout_ms);
        let command_timeout = Duration::from_millis(spec.command_timeout_ms);

        let addr = &spec.addresses[0];
        let local_addr = addr.clone();
        let mut parsed_addr = local_addr
            .replace("redis+sentinel://", "redis://")
            .into_connection_info()?;

        let redis_settings = parsed_addr
            .redis_settings()
            .clone()
            // We need RESP3 here because we want to do set_push_sender
            .set_protocol(redis::ProtocolVersion::RESP3);
        parsed_addr = parsed_addr.set_redis_settings(redis_settings);
        debug!(?parsed_addr, "Parsed redis addr");

        let client = timeout(
            connection_timeout,
            spawn!("connect", async move {
                match spec.mode {
                    RedisMode::Standard => Client::open(parsed_addr).map_err(Into::<Error>::into),
                    RedisMode::Cluster => {
                        return Err(Error::new(
                            Code::Internal,
                            "Use RedisStore::new_cluster for cluster connections".to_owned(),
                        ));
                    }
                    RedisMode::Sentinel => async {
                        let url_parsing = Url::parse(&local_addr)?;
                        let master_name = url_parsing
                            .query_pairs()
                            .find(|(key, _)| key == "sentinelServiceName")
                            .map_or_else(|| "master".into(), |(_, value)| value.to_string());

                        let redis_connection_info = parsed_addr.redis_settings().clone();
                        let sentinel_connection_info = SentinelNodeConnectionInfo::default()
                            .set_redis_connection_info(redis_connection_info);

                        // We fish this out because sentinels don't support db, we need to set it
                        // on the client only. See also https://github.com/redis-rs/redis-rs/issues/1950
                        let original_db = parsed_addr.redis_settings().db();
                        if original_db != 0 {
                            // sentinel_connection_info has the actual DB set
                            let revised_settings = parsed_addr.redis_settings().clone().set_db(0);
                            parsed_addr = parsed_addr.set_redis_settings(revised_settings);
                        }

                        SentinelClient::build(
                            vec![parsed_addr],
                            master_name,
                            Some(sentinel_connection_info),
                            SentinelServerType::Master,
                        )
                        .map_err(Into::<Error>::into)
                    }
                    .and_then(|mut s| async move { Ok(s.async_get_client().await) })
                    .await?
                    .map_err(Into::<Error>::into),
                }
                // Keep the underlying error's code rather than forcing
                // InvalidArgument. Not every connect failure is a bad URL: a
                // sentinel failover in progress reports the master as missing,
                // which is transient and retryable, and overriding it to a
                // permanent status made clients give up on a recoverable blip.
                .err_tip(|| format!("While connecting to redis with url: {local_addr}"))
            }),
        )
        .await
        .err_tip(|| format!("Timeout while connecting to redis with url: {addr}"))???;

        let connection_manager_config = {
            ConnectionManagerConfig::new()
                .set_number_of_retries(spec.retry.max_retries)
                .set_connection_timeout(Some(connection_timeout))
                .set_response_timeout(Some(command_timeout))
                .set_push_sender(tx)
        };

        let mut connection_manager =
            ConnectionManager::new_with_config(client, connection_manager_config)
                .await
                .err_tip(|| format!("While connecting to redis with url: {addr}"))?;

        if let Some(pub_sub_channel) = spec.experimental_pub_sub_channel {
            connection_manager.psubscribe(pub_sub_channel).await?;
        }

        Ok(connection_manager)
    }

    /// Create a new `RedisStore` from the given configuration.
    pub async fn new_standard(mut spec: RedisSpec) -> Result<Arc<Self>, Error> {
        Self::set_spec_defaults(&mut spec)?;

        if spec.addresses.len() != 1 {
            return Err(make_err!(
                Code::Unimplemented,
                "Connecting directly to multiple redis nodes in a cluster is currently unsupported. Please specify a single URL to a single node, and nativelink will use cluster discover to find the other nodes."
            ));
        }

        let (tx, subscriber_channel) = unbounded_channel();
        let migrate_temporary_indexes = spec.migrate_temporary_indexes;
        let sorted_index = spec.experimental_sorted_index.clone();

        Self::new_from_builder_and_parts(
            spec.experimental_pub_sub_channel.clone(),
            || Uuid::new_v4().to_string(),
            spec.key_prefix.clone(),
            spec.read_chunk_size,
            spec.max_chunk_uploads_per_update,
            spec.scan_count,
            spec.max_client_permits,
            spec.max_count_per_cursor,
            Duration::from_millis(spec.health_check_timeout_ms),
            (spec.key_ttl_s > 0).then(|| Duration::from_secs(spec.key_ttl_s)),
            subscriber_channel,
            StandardRedisManager::new(Box::new(move || {
                Box::pin(Self::connect(spec.clone(), tx.clone()))
            }))
            .await?,
        )
        .await
        .map(|store| {
            let store = if migrate_temporary_indexes {
                store.with_temporary_index_migration()
            } else {
                store
            };
            match &sorted_index {
                Some(sorted_index) => store.with_sorted_index(sorted_index),
                None => store,
            }
        })
        .map(Arc::new)
    }
}

/// Deletes the temp key of an upload that did not publish, however it
/// ended. `delete` is for an error the store sees. `Drop` is for the update
/// future being dropped mid-stream, which is how the `ByteStream` server ends
/// an upload whose client went away or closed the stream early: no error
/// reaches the store, so without this the key stayed until its TTL. A chunk
/// still in flight on this connection was queued before the DEL, so Redis
/// applies the DEL after it; a chunk retried on another connection has no
/// such order and can recreate the key after the DEL, which is why every
/// chunk write arms the lifetime again and deletes a key it finds recreated
/// (see `update_through_temp_key`). What this cannot delete, the lifetime
/// removes.
struct TempKeyGuard<C>
where
    C: ConnectionLike + Clone + Send + Sync + 'static,
{
    connection: C,
    temp_key: String,
    armed: bool,
}

impl<C> TempKeyGuard<C>
where
    C: ConnectionLike + Clone + Send + Sync + 'static,
{
    /// The upload published; the key is gone under its final name.
    fn disarm(mut self) {
        self.armed = false;
    }

    async fn delete(self) {
        let connection = self.connection.clone();
        self.delete_with(connection).await;
    }

    /// `delete` on `connection` rather than the one the guard was given:
    /// after a failover the upload ended on a newer one.
    async fn delete_with(mut self, mut connection: C) {
        self.armed = false;
        if let Err(err) = connection.del::<_, ()>(&self.temp_key).await {
            debug!(
                ?err,
                temp_key = %self.temp_key,
                "Could not delete the temp key of a failed upload; its TTL removes it"
            );
        }
    }
}

impl<C> Drop for TempKeyGuard<C>
where
    C: ConnectionLike + Clone + Send + Sync + 'static,
{
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        // Dropped mid-stream. Nothing can be awaited here, so the delete runs
        // on its own task; without a runtime (shutdown) the TTL does it.
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let temp_key = core::mem::take(&mut self.temp_key);
        let mut connection = self.connection.clone();
        handle.spawn(async move {
            if let Err(err) = connection.del::<_, ()>(&temp_key).await {
                debug!(
                    ?err,
                    temp_key,
                    "Could not delete the temp key of a dropped upload; its TTL removes it"
                );
            }
        });
    }
}

impl<C, M> RedisStore<C, M>
where
    C: ConnectionLike + Clone + Send + Sync + Unpin + 'static,
    M: RedisManager<C> + Unpin + Send + Sync + 'static,
{
    /// Stages an upload under `temp_key` and publishes it as `final_key`.
    ///
    /// The temp key is created with a lifetime before any byte lands, and
    /// every chunk arms it again, so an upload that stops anywhere (the
    /// client went away, this process did) leaves a key Redis removes on its
    /// own. Nothing else cleans temp keys up: on a store without `key_ttl_s`
    /// they lived forever, and a CAS that failed or restarted under load left
    /// them in numbers. The lifetime is `key_ttl_s` when set, else
    /// [`TEMP_KEY_TTL`]; a key that expired or was lost in a failover and was
    /// recreated by a later chunk is refused rather than published, see the
    /// TTL checks on each chunk and after them. The caller's [`TempKeyGuard`]
    /// deletes the key when this does not publish.
    async fn update_through_temp_key(
        &self,
        key: &StoreKey<'_>,
        final_key: &str,
        temp_key: &str,
        reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
    ) -> Result<u64, Error> {
        let mut client = self.get_client().await?;

        let temp_ttl_secs = self.key_ttl.unwrap_or(TEMP_KEY_TTL).as_secs().max(1);
        let temp_ttl = ttl_seconds(self.key_ttl.unwrap_or(TEMP_KEY_TTL));
        let staged = SetOptions::default().with_expiration(SetExpiry::EX(temp_ttl_secs));
        match client
            .connection_manager
            .set_options::<_, _, ()>(temp_key, "", staged.clone())
            .await
        {
            Ok(()) => {}
            Err(err) if is_retryable_redis_error(&err) => {
                let (connection_manager, uuid) =
                    self.connection_manager.reconnect(client.uuid).await?;
                client.connection_manager = connection_manager;
                client.uuid = uuid;
                client
                    .connection_manager
                    .set_options::<_, _, ()>(temp_key, "", staged)
                    .await
                    .err_tip(|| {
                        format!(
                            "(after reconnect) while creating temp key ({temp_key}) in RedisStore::update"
                        )
                    })?;
            }
            Err(err) => {
                return Err(Error::from(err).append(format!(
                    "While creating temp key ({temp_key}) in RedisStore::update"
                )));
            }
        }

        let mut read_stream = reader
            .scan(0u32, |bytes_read, chunk_res| {
                future::ready(Some(
                    chunk_res
                        .err_tip(|| "Failed to read chunk in update in redis store")
                        .and_then(|chunk| {
                            let offset = isize::try_from(*bytes_read).err_tip(|| "Could not convert offset to isize in RedisStore::update")?;
                            let chunk_len = u32::try_from(chunk.len()).err_tip(
                                || "Could not convert chunk length to u32 in RedisStore::update",
                            )?;
                            let new_bytes_read = bytes_read
                                .checked_add(chunk_len)
                                .err_tip(|| "Overflow protection in RedisStore::update")?;
                            *bytes_read = new_bytes_read;
                            Ok::<_, Error>((offset, *bytes_read, chunk))
                        }),
                ))
            })
            .map(|res| {
                let (offset, end_pos, chunk) = res?;
                Ok(async move {
                    let (mut connection_manager, connect_id) = self.connection_manager.get_connection().await?;
                    // The chunk, the key's TTL as it stood before the chunk,
                    // and the lifetime armed again, in one round trip. A key
                    // that expired or was lost in a failover is recreated by
                    // the SETRANGE, zero-filled and with no TTL, and only the
                    // TTL tells. The EXPIRE makes the lifetime a bound on the
                    // gap between chunks rather than on the upload, and gives
                    // even a recreated key a lifetime, so a chunk that lands
                    // after the upload's DEL (one retried on another
                    // connection, say) leaves nothing behind for good.
                    let mut write = pipe();
                    write
                        .setrange(temp_key, offset, chunk.to_vec())
                        .ttl(temp_key)
                        .expire(temp_key, temp_ttl);
                    let (_, ttl_before, _): (usize, i64, i64) = match write
                        .query_async(&mut connection_manager)
                        .await
                    {
                        Ok(written) => written,
                        Err(err) if is_retryable_redis_error(&err) => {
                            let (reconnected, _connect_id) =
                                self.connection_manager.reconnect(connect_id).await?;
                            connection_manager = reconnected;
                            write
                                .query_async(&mut connection_manager)
                                .await
                                .err_tip(|| {
                                    format!("(after reconnect) while appending to temp key ({temp_key}) in RedisStore::update. offset = {offset}. end_pos = {end_pos}")
                                })?
                        }
                        Err(err) => {
                            let mut error: Error = err.into();
                            error
                                .messages
                                .push(format!("While appending to temp key ({temp_key}) in RedisStore::update. offset = {offset}. end_pos = {end_pos}"));
                            return Err(error);
                        }
                    };
                    if ttl_before < 0 {
                        // Recreated by this chunk: gone now rather than at
                        // the end of the lifetime the EXPIRE just gave it.
                        if let Err(err) = connection_manager.del::<_, ()>(temp_key).await {
                            debug!(
                                ?err,
                                temp_key,
                                "Could not delete a recreated temp key; its TTL removes it"
                            );
                        }
                        return Err(make_err!(
                            Code::Aborted,
                            "Chunk at offset {offset} of {}({temp_key}) found the temp key gone (expired after {temp_ttl}s without a chunk, or lost in a failover) and recreated it; not published, send the upload again",
                            key.as_str(),
                        ));
                    }
                    Ok::<u32, Error>(end_pos)
                })
            })
            .try_buffer_unordered(self.max_chunk_uploads_per_update);

        let mut total_len: u32 = 0;
        while let Some(last_pos) = read_stream.try_next().await? {
            if last_pos > total_len {
                total_len = last_pos;
            }
        }

        let expected_len = usize::try_from(total_len).unwrap_or(usize::MAX);
        upload_size.validate_size(u64::from(total_len))?;

        // The chunk writes above reconnect on any transient failover error, so on a mid-write Redis
        // failover the data lands on the *current* master. The length check and
        // rename below must run against that same master: the connection
        // captured before the writes may now point at a demoted replica, where
        // strlen reads 0 and would fail an otherwise healthy write. Re-resolve
        // the master and retry so a transient topology change (failover or brief
        // replica lag) doesn't drop the value.
        let mut attempt: u32 = 0;
        let blob_len = loop {
            attempt += 1;
            // The master can go away between the chunk writes and this check
            // too, and the connection then waits out the command timeout on a
            // dead peer. Re-resolve the master and ask it, like every other
            // step here does, instead of failing the upload. The error can
            // also be immediate (connection refused while the old master is
            // gone and Sentinel has not promoted a replica yet), so back off
            // before each retry, doubling, so the attempts together span a
            // typical failover of a few seconds rather than a few milliseconds.
            //
            // The key's remaining TTL comes back in the same round trip.
            let (blob_len, remaining_ttl): (usize, i64) = match pipe()
                .strlen(temp_key)
                .ttl(temp_key)
                .query_async(&mut client.connection_manager)
                .await
            {
                Ok(checked) => checked,
                Err(err)
                    if is_retryable_redis_error(&err) && attempt < MAX_REDIS_RETRY_ATTEMPTS =>
                {
                    let (connection_manager, uuid) =
                        self.connection_manager.reconnect(client.uuid).await?;
                    client.connection_manager = connection_manager;
                    client.uuid = uuid;
                    let backoff =
                        DEFAULT_RETRY_DELAY * 2f32.powi(i32::try_from(attempt).unwrap_or(i32::MAX));
                    sleep(Duration::from_secs_f32(backoff)).await;
                    continue;
                }
                Err(err) => {
                    return Err(Error::from(err)
                        .append(format!("In RedisStore::update strlen check for {temp_key}")));
                }
            };
            // Safety check: reject if a retried append double-wrote the data.
            if blob_len > expected_len {
                return Err(make_input_err!(
                    "Data length mismatch in RedisStore::update for {}({}) - expected {} bytes, got {} bytes",
                    key.as_str(),
                    temp_key,
                    total_len,
                    blob_len,
                ));
            }
            if blob_len == expected_len {
                // SETRANGE on a key that has expired or was lost creates it
                // again, zero-filled up to the offset and with no TTL, and
                // the length still matches. Each chunk checks this on its
                // own write; this is the check for whatever happened between
                // the last chunk's EXPIRE and here. Publishing would store
                // zeros under a digest they do not hash to.
                if remaining_ttl < 0 {
                    return Err(make_err!(
                        Code::Aborted,
                        "Upload of {}({}) lost its temp key (expired after {}s without a chunk, or lost in a failover) and a later chunk recreated it; not published, send it again",
                        key.as_str(),
                        temp_key,
                        temp_ttl_secs,
                    ));
                }
                break blob_len;
            }
            // blob_len < expected (typically 0): the temp key isn't visible on
            // this connection yet; re-resolve the master and retry.
            if attempt >= MAX_REDIS_RETRY_ATTEMPTS {
                return Err(make_input_err!(
                    "Data length mismatch in RedisStore::update for {}({}) - expected {} bytes, got {} bytes after {} attempts",
                    key.as_str(),
                    temp_key,
                    total_len,
                    blob_len,
                    attempt,
                ));
            }
            let (connection_manager, uuid) = self.connection_manager.reconnect(client.uuid).await?;
            client.connection_manager = connection_manager;
            client.uuid = uuid;
            sleep(Duration::from_secs_f32(DEFAULT_RETRY_DELAY)).await;
        };

        // Rename the temp key so that the data appears under the real key. Any data already present in the real key is lost.
        // The rename carries the temp key's TTL across, so the real key gets
        // its own lifetime in the same transaction: `key_ttl_s` counted from
        // completion rather than from the first byte, or none at all. One
        // transaction, so nothing can be published with the temp key's
        // lifetime still on it, and so a transaction that applied on the
        // old master before its reply was lost in a failover left the final
        // key complete, which the retry below recognizes.
        // Reconnect once on a transient failover error in case the master moved between the verify and here.
        let mut publish = pipe();
        publish.atomic().rename(temp_key, final_key);
        match self.key_ttl {
            Some(key_ttl) => publish.expire(final_key, ttl_seconds(key_ttl)),
            None => publish.persist(final_key),
        };
        let published: Option<((), bool)> =
            match publish.query_async(&mut client.connection_manager).await {
                Ok(published) => Some(published),
                Err(err) if is_retryable_redis_error(&err) => {
                    let (connection_manager, uuid) =
                        self.connection_manager.reconnect(client.uuid).await?;
                    client.connection_manager = connection_manager;
                    client.uuid = uuid;
                    match publish.query_async(&mut client.connection_manager).await {
                        Ok(published) => Some(published),
                        Err(err) if is_no_such_key(&err) => None,
                        Err(err) => {
                            return Err(Error::from(err).append(format!(
                            "While publishing {final_key} (after reconnect) in RedisStore::update()"
                        )));
                        }
                    }
                }
                Err(err) => {
                    return Err(Error::from(err).append(format!(
                        "While publishing {final_key} in RedisStore::update()"
                    )));
                }
            };
        let kept = if let Some(((), kept)) = published {
            kept
        } else {
            // The temp key is gone on the master the retry reached.
            // Either the first attempt applied and the final key is this
            // upload, or the failover lost the temp key and the final key
            // is whatever was there before. For a digest key the two are
            // the same bytes; for a string key this accepts the value
            // already there, as an upload that lost a race does.
            let len: usize = client
                    .connection_manager
                    .strlen(final_key)
                    .await
                    .err_tip(|| {
                        format!(
                            "While checking {final_key} after a publish whose reply was lost in RedisStore::update()"
                        )
                    })?;
            if len != expected_len {
                return Err(make_err!(
                    Code::Aborted,
                    "Upload of {}({}) lost its temp key in a failover and {} is not it ({} bytes, expected {}); send it again",
                    key.as_str(),
                    temp_key,
                    final_key,
                    len,
                    expected_len,
                ));
            }
            true
        };
        // EXPIRE and PERSIST answer 0 only when the key is gone, which inside
        // the transaction means it expired as the RENAME ran. The client has
        // to send it again.
        if !kept {
            return Err(make_err!(
                Code::Aborted,
                "Upload of {}({}) expired as it was published; not stored, send it again",
                key.as_str(),
                final_key,
            ));
        }

        // If we have a publish channel configured, send a notice that the key has been set.
        if let Some(pub_sub_channel) = &self.pub_sub_channel {
            client
                .connection_manager
                .publish::<_, _, ()>(pub_sub_channel, final_key)
                .await?;
        }

        Ok(blob_len.try_into().unwrap_or(0))
    }
}

#[async_trait]
impl<C, M> StoreDriver for RedisStore<C, M>
where
    C: ConnectionLike + Clone + Send + Sync + Unpin + 'static,
    M: RedisManager<C> + Unpin + Send + Sync + 'static,
{
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        // TODO(palfrey) We could use pipeline here, but it makes retry more
        // difficult and it doesn't work very well in cluster mode.
        // If we wanted to optimize this with pipeline be careful to
        // implement retry and to support cluster mode.

        izip!(keys.iter(), results.iter_mut(),)
            .map(|(key, result)| async move {
                // We need to do a special pass to ensure our zero key exist.
                if is_zero_digest(key.borrow()) {
                    *result = Some(0);
                    return Ok::<_, Error>(());
                }
                let encoded_key = self.encode_key(key);

                let mut client = self.get_client().await?;

                // Redis returns 0 when the key doesn't exist
                // AND when the key exists with value of length 0.
                // Therefore, we need to check both length and existence
                // and do it in a pipeline for efficiency
                // Re-resolve the master and retry on a transient failover so a
                // topology change doesn't fail the existence check.
                let (blob_len, exists) = {
                    let mut attempt: u32 = 0;
                    loop {
                        attempt += 1;
                        match pipe()
                            .strlen(encoded_key.as_ref())
                            .exists(encoded_key.as_ref())
                            .query_async::<(u64, bool)>(&mut client.connection_manager)
                            .await
                        {
                            Ok(v) => break v,
                            Err(err)
                                if attempt < MAX_REDIS_RETRY_ATTEMPTS
                                    && is_retryable_redis_error(&err) =>
                            {
                                client.reconnect(&self.connection_manager).await?;
                                sleep(Duration::from_secs_f32(DEFAULT_RETRY_DELAY)).await;
                            }
                            Err(err) => {
                                return Err(
                                    Error::from(err).append("In RedisStore::has_with_results::all")
                                );
                            }
                        }
                    }
                };

                // An empty value cannot be a blob whose digest names bytes: a
                // failed upload left it. Reported present it is read as
                // DataLoss forever and never uploaded again.
                *result = match (exists, blob_len) {
                    (false, _) => None,
                    (true, 0) if matches!(key.borrow(), StoreKey::Digest(digest) if digest.size_bytes() > 0) => {
                        warn!(
                            ?key,
                            "Redis holds an empty value for a digest that names bytes; reporting it missing"
                        );
                        None
                    }
                    (true, len) => Some(len),
                };

                Ok::<_, Error>(())
            })
            .collect::<FuturesUnordered<_>>()
            .try_collect()
            .await
    }

    async fn list(
        self: Pin<&Self>,
        range: (Bound<StoreKey<'_>>, Bound<StoreKey<'_>>),
        handler: &mut (dyn for<'a> FnMut(&'a StoreKey) -> bool + Send + Sync + '_),
    ) -> Result<u64, Error> {
        let range = (
            range.0.map(StoreKey::into_owned),
            range.1.map(StoreKey::into_owned),
        );
        let pattern = match range.0 {
            Bound::Included(ref start) | Bound::Excluded(ref start) => match range.1 {
                Bound::Included(ref end) | Bound::Excluded(ref end) => {
                    let start = start.as_str();
                    let end = end.as_str();
                    let max_length = start.len().min(end.len());
                    let length = start
                        .chars()
                        .zip(end.chars())
                        .position(|(a, b)| a != b)
                        .unwrap_or(max_length);
                    format!("{}{}*", self.key_prefix, &start[..length])
                }
                Bound::Unbounded => format!("{}*", self.key_prefix),
            },
            Bound::Unbounded => format!("{}*", self.key_prefix),
        };
        let mut client = self.get_client().await?;
        trace!(%pattern, count=self.scan_count, "Running SCAN");
        // Restart the scan on a transient failover. Redis SCAN may re-emit keys
        // even without failures, so callers already tolerate duplicates;
        // re-resolving the master and rescanning is within that contract and
        // avoids failing on a topology change mid-iteration.
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let opts = ScanOptions::default()
                .with_pattern(pattern.clone())
                .with_count(self.scan_count);
            // Scan via a cloned connection handle so the iterator doesn't hold a
            // borrow on `client` across a reconnect.
            let mut conn = client.connection_manager.clone();
            let mut scan_stream: AsyncIter<Value> = match conn.scan_options(opts).await {
                Ok(s) => s,
                Err(err)
                    if attempt < MAX_REDIS_RETRY_ATTEMPTS && is_retryable_redis_error(&err) =>
                {
                    client.reconnect(&self.connection_manager).await?;
                    sleep(Duration::from_secs_f32(DEFAULT_RETRY_DELAY)).await;
                    continue;
                }
                Err(err) => return Err(Error::from(err).append("During scan_options")),
            };
            let mut iterations = 0;
            let mut errors = vec![];
            let mut transient_err = false;
            while let Some(key) = scan_stream.next_item().await {
                match key {
                    Ok(Value::BulkString(raw_key)) => {
                        let Ok(str_key) = str::from_utf8(&raw_key) else {
                            error!(?raw_key, "Non-utf8 key");
                            errors.push(format!("Non-utf8 key {raw_key:?}"));
                            continue;
                        };
                        match decode_key(&self.key_prefix, Cow::from(str_key)) {
                            Ok(key) => {
                                if range.contains(&key) {
                                    iterations += 1;
                                    if !handler(&key) {
                                        error!("Issue in handler");
                                        errors.push("Issue in handler".to_string());
                                    }
                                } else {
                                    trace!(%key, ?range, "Key not in range");
                                }
                            }
                            Err(e) => {
                                errors.push(e.to_string());
                            }
                        }
                    }
                    Err(err)
                        if attempt < MAX_REDIS_RETRY_ATTEMPTS && is_retryable_redis_error(&err) =>
                    {
                        // Connection dropped mid-scan; restart from scratch.
                        transient_err = true;
                        break;
                    }
                    other => {
                        error!(?other, "Non-string in key");
                        errors.push("Non-string in key".to_string());
                    }
                }
            }
            if transient_err {
                drop(scan_stream);
                client.reconnect(&self.connection_manager).await?;
                sleep(Duration::from_secs_f32(DEFAULT_RETRY_DELAY)).await;
                continue;
            }
            return if errors.is_empty() {
                Ok(iterations)
            } else {
                error!(?errors, "Errors in scan stream");
                Err(Error::new(Code::Internal, format!("Errors: {errors:?}")))
            };
        }
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
    ) -> Result<u64, Error> {
        let final_key = self.encode_key(&key);

        // While the name generation function can be supplied by the user, we need to have the curly
        // braces in place in order to manage redis' hashing behavior and make sure that the temporary
        // key name and the final key name are directed to the same cluster node. See
        // https://redis.io/blog/redis-clustering-best-practices-with-keys/
        //
        // The TL;DR is that if we're in cluster mode and the names hash differently, we can't use request
        // pipelining. By using these braces, we tell redis to only hash the part of the temporary key that's
        // identical to the final key -- so they will always hash to the same node.
        let temp_key = format!("temp-{}-{{{final_key}}}", (self.temp_name_generator_fn)());

        if is_zero_digest(key.borrow()) {
            let chunk = reader
                .peek()
                .await
                .err_tip(|| "Failed to peek in RedisStore::update")?;
            if chunk.is_empty() {
                reader
                    .drain()
                    .await
                    .err_tip(|| "Failed to drain in RedisStore::update")?;
                // Zero-digest keys are special -- we don't need to do anything with it.
                return Ok(0);
            }
        }

        // The temp key expires on its own. Deleting it as soon as the upload
        // cannot publish frees the memory now rather than at the end of its
        // lifetime, which is the difference between a bounded and a full
        // Redis when uploads fail or are abandoned in numbers.
        let (connection, _uuid) = self.connection_manager.get_connection().await?;
        let guard = TempKeyGuard {
            connection,
            temp_key: temp_key.clone(),
            armed: true,
        };
        match self
            .update_through_temp_key(&key, &final_key, &temp_key, reader, upload_size)
            .await
        {
            Ok(len) => {
                guard.disarm();
                Ok(len)
            }
            Err(err) => {
                // On the connection the upload ended on, which after a
                // failover is a newer one than the guard was given.
                match self.connection_manager.get_connection().await {
                    Ok((connection, _uuid)) => guard.delete_with(connection).await,
                    Err(_) => guard.delete().await,
                }
                Err(err)
            }
        }
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        let offset = isize::try_from(offset).err_tip(|| "Could not convert offset to isize")?;
        let length = length
            .map(|v| usize::try_from(v).err_tip(|| "Could not convert length to usize"))
            .transpose()?;

        // To follow RBE spec we need to consider any digest's with
        // zero size to be existing.
        if is_zero_digest(key.borrow()) {
            return writer
                .send_eof()
                .err_tip(|| "Failed to send zero EOF in redis store get_part");
        }

        let encoded_key = self.encode_key(&key);
        let encoded_key = encoded_key.as_ref();

        // N.B. the `-1`'s you see here are because redis GETRANGE is inclusive at both the start and end, so when we
        // do math with indices we change them to be exclusive at the end.

        // We want to read the data at the key from `offset` to `offset + length`.
        let data_start = offset;
        let data_end = data_start
            .saturating_add(length.map_or(isize::MAX, |l| isize::try_from(l).unwrap_or(isize::MAX)))
            .saturating_sub(1);

        // And we don't ever want to read more than `read_chunk_size` bytes at a time, so we'll need to iterate.
        let mut chunk_start = data_start;
        let mut chunk_end = cmp::min(
            data_start.saturating_add(self.read_chunk_size.try_into().unwrap_or(isize::MAX)) - 1,
            data_end,
        );

        let mut client = self.get_client().await?;
        let mut bytes_read: u64 = 0;
        loop {
            // getrange is position-based and idempotent, so re-resolve the
            // master and retry on a transient failover without re-sending
            // already-written chunks.
            let chunk: Bytes = {
                let mut attempt: u32 = 0;
                loop {
                    attempt += 1;
                    match client
                        .connection_manager
                        .getrange(encoded_key, chunk_start, chunk_end)
                        .await
                    {
                        Ok(v) => break v,
                        Err(err)
                            if attempt < MAX_REDIS_RETRY_ATTEMPTS
                                && is_retryable_redis_error(&err) =>
                        {
                            client.reconnect(&self.connection_manager).await?;
                            sleep(Duration::from_secs_f32(DEFAULT_RETRY_DELAY)).await;
                        }
                        Err(err) => {
                            return Err(
                                Error::from(err).append("In RedisStore::get_part::getrange")
                            );
                        }
                    }
                }
            };

            let didnt_receive_full_chunk = chunk.len() < self.read_chunk_size;
            let reached_end_of_data = chunk_end == data_end;

            if didnt_receive_full_chunk || reached_end_of_data {
                if !chunk.is_empty() {
                    writer
                        .send(chunk)
                        .await
                        .err_tip(|| "Failed to write data in RedisStore::get_part")?;
                } else if bytes_read > 0 {
                    // GETRANGE answers the same for a value that ends on a
                    // chunk boundary and for a key removed between two
                    // chunks (eviction under allkeys-lru). Handing the second
                    // on as EOF is a truncated blob that every store above
                    // caches as complete, so ask how long the value is now
                    // and refuse anything but the end we stopped at.
                    let value_len = self.strlen(&mut client, encoded_key).await?;
                    let read_end = u64::try_from(data_start)
                        .unwrap_or(0)
                        .saturating_add(bytes_read);
                    if value_len != read_end {
                        return Err(make_err!(
                            Code::NotFound,
                            "Data for {key:?} changed under the read in the Redis store: read {bytes_read} bytes from offset {data_start}, the key now holds {value_len}"
                        ));
                    }
                }

                break; // No more data to read.
            }

            // We received a full chunk's worth of data, so write it...
            bytes_read += chunk.len() as u64;
            writer
                .send(chunk)
                .await
                .err_tip(|| "Failed to write data in RedisStore::get_part")?;

            // ...and go grab the next chunk.
            chunk_start = chunk_end + 1;
            chunk_end = cmp::min(
                chunk_start.saturating_add(self.read_chunk_size.try_into().unwrap_or(isize::MAX))
                    - 1,
                data_end,
            );
        }

        // If we didn't write any data, check if the key exists, if not return a NotFound error.
        // This is required by spec.
        if writer.get_bytes_written() == 0 {
            // We're supposed to read 0 bytes, so just check if the key exists.
            let exists: bool = {
                let mut attempt: u32 = 0;
                loop {
                    attempt += 1;
                    match client.connection_manager.exists(encoded_key).await {
                        Ok(v) => break v,
                        Err(err)
                            if attempt < MAX_REDIS_RETRY_ATTEMPTS
                                && is_retryable_redis_error(&err) =>
                        {
                            client.reconnect(&self.connection_manager).await?;
                            sleep(Duration::from_secs_f32(DEFAULT_RETRY_DELAY)).await;
                        }
                        Err(err) => {
                            return Err(
                                Error::from(err).append("In RedisStore::get_part::zero_exists")
                            );
                        }
                    }
                }
            };

            if !exists {
                return Err(make_err!(
                    Code::NotFound,
                    "Data not found in Redis store for digest: {key:?}"
                ));
            }
            // The key exists and holds nothing, yet the digest names bytes
            // from this offset on: that is the empty value a failed upload
            // left, not the blob. NotFound sends the client back to upload it.
            if let StoreKey::Digest(digest) = key.borrow()
                && digest.size_bytes() > u64::try_from(data_start).unwrap_or(u64::MAX)
                && length.is_none_or(|len| len > 0)
            {
                return Err(make_err!(
                    Code::NotFound,
                    "Redis holds an empty value for {key:?}, which names {} bytes; treating it as missing so the client uploads it again",
                    digest.size_bytes()
                ));
            }
        }

        writer
            .send_eof()
            .err_tip(|| "Failed to write EOF in redis store get_part")
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any(&self) -> &(dyn core::any::Any + Sync + Send) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send> {
        self
    }

    fn register_health(self: Arc<Self>, registry: &mut HealthRegistryBuilder) {
        registry.register_indicator(self);
    }

    fn register_remove_callback(self: Arc<Self>, callback: RemoveCallback) -> Result<(), Error> {
        debug!(?callback, "New callback");
        background_spawn!("remove_callback_subscribe", async move {
            self.remove_callbacks.lock().await.push(callback);
            // The subscription init is retried until it succeeds: this
            // function already told its caller Ok(()) (e.g.
            // ExistenceCacheStore, which relies on eviction callbacks for
            // invalidation), so a transient Redis error here (e.g. -LOADING
            // during a restart/failover) must not permanently disable remove
            // callbacks. Previously a one-shot `get_or_try_init` failure was
            // only logged, the OnceCell stayed empty, and nothing ever
            // retried, so the `__key*__:*` subscription was silently never
            // established.
            let mut retry_delay = Duration::from_millis(50);
            loop {
                let local_self = self.clone();
                let init_result = self
                    .has_remove_callback_subscribe
                    .get_or_try_init(|| async move {
                        // The notify-keyspace-events check is advisory: we log
                        // misconfiguration loudly, but a failure to *read* the
                        // config must not block the subscription itself.
                        let cfg_result = async {
                            let mut client = local_self.get_client().await?;
                            redis::cmd("CONFIG").arg("GET").arg("notify-keyspace-events").to_owned().query_async::<Vec<(String,String)>>(&mut client.connection_manager).await.map_err(|e| Error::from(e).append("Parsing notify-keyspace-events"))
                        }.await;
                        match &cfg_result {
                            Ok(cfg) => {
                                if cfg.len() != 1 {
                                    warn!(?cfg, "Got multiple items for CONFIG GET, expected one");
                                } else if let Some((_, events_cfg)) = cfg.first() {
                                    if events_cfg.is_empty() {
                                        error!("notify-keyspace-events not enabled for Redis, will fail to get remove callbacks");
                                    } else if !events_cfg.contains('K') {
                                        error!(notify_keyspace_events=events_cfg, "notify-keyspace-events does not contain 'K' so won't get keyspace events we need for eviction events");
                                    } else if !events_cfg.contains('A') {
                                        error!(notify_keyspace_events=events_cfg, "notify-keyspace-events does not contain 'A' so we won't get eviction events");
                                    }
                                    // FIXME: Redis events spec appears unreliable, so we subscribe anyways
                                    // It should just need Ke as per https://redis.io/docs/latest/develop/pubsub/keyspace-notifications/
                                    // but I'm yet to get reliable eviction events out of that
                                    info!(notify_keyspace_events=events_cfg, "Attempting to subscribe to eviction events");
                                }
                            }
                            Err(err) => {
                                error!(?err, "Error while trying to initialise remove_callback_subscribe: could not verify notify-keyspace-events; subscribing anyway");
                            }
                        }
                        local_self.connection_manager.psubscribe("__key*__:*").await?;
                        Ok::<(), Error>(())
                    })
                    .await;
                match init_result {
                    Ok(()) => break,
                    Err(err) => {
                        error!(
                            ?err,
                            "Error while trying to initialise remove_callback_subscribe; will retry"
                        );
                        sleep(retry_delay).await;
                        retry_delay = (retry_delay * 2).min(Duration::from_secs(5));
                    }
                }
            }
        });
        Ok(())
    }
}

#[async_trait]
impl<C, M> HealthStatusIndicator for RedisStore<C, M>
where
    C: ConnectionLike + Clone + Send + Sync + Unpin + 'static,
    M: RedisManager<C> + Send + Sync + Unpin + 'static,
{
    fn get_name(&self) -> &'static str {
        "RedisStore"
    }

    /// Lightweight health check: just `PING` the master, bounded by a
    /// short physical timeout. The default `StoreDriver::check_health`
    /// performs a full `update_oneshot` + `has` + `get_part_unchunked`
    /// roundtrip, which queues behind real production traffic on the
    /// same connection-permit semaphore and Redis master. When the
    /// store is even moderately loaded that easily exceeds the
    /// `HealthServer` per-indicator budget (default 5 s), each
    /// RedisStore-backed indicator (AC, small-blob CAS, scheduler)
    /// reports `HealthStatus::Timeout`, and `/status` returns 503 —
    /// surfaced as a readiness-probe failure that sheds traffic from
    /// an otherwise-functional pod. A `PING` proves the connection
    /// is reachable and the master is accepting commands; that is
    /// the only invariant a kubelet probe needs.
    async fn check_health(&self, _namespace: Cow<'static, str>) -> HealthStatus {
        let mut client = match self.get_client().await {
            Ok(c) => c,
            Err(e) => {
                return HealthStatus::new_failed(
                    self,
                    format!("RedisStore::check_health: failed to acquire connection: {e}").into(),
                );
            }
        };

        // Hold the `ClientWithPermit` for the duration of the call so
        // its `Drop` releases the semaphore permit on exit. We just
        // need a `&mut` to the connection manager underneath.
        let ping = async {
            redis::cmd("PING")
                .query_async::<()>(&mut client.connection_manager)
                .await
        };
        let retry_reason = match timeout(self.health_check_timeout, ping).await {
            Ok(Ok(())) => {
                return HealthStatus::new_ok(self, "RedisStore::check_health: PING ok".into());
            }
            // A PING that errors connection-wise or times out means the handle
            // points at a master that went away (a Sentinel failover). Without
            // re-resolving here, a store with no other traffic stays wedged on
            // the dead handle and reports unhealthy forever — shedding readiness
            // traffic from an otherwise-recovered pod until it is restarted.
            Ok(Err(e)) if is_retryable_redis_error(&e) => format!("PING errored: {e}"),
            Ok(Err(e)) => {
                return HealthStatus::new_failed(
                    self,
                    format!("RedisStore::check_health: PING errored: {e}").into(),
                );
            }
            Err(_) => format!(
                "PING exceeded {}ms timeout",
                self.health_check_timeout.as_millis()
            ),
        };

        // The reconnect (re-resolving the master via Sentinel) is logged by
        // `StandardRedisManager::reconnect`; `retry_reason` is surfaced in the
        // failure message below if the second PING still doesn't come back.
        if let Err(e) = client.reconnect(&self.connection_manager).await {
            return HealthStatus::new_failed(
                self,
                format!("RedisStore::check_health: {retry_reason}; reconnect failed: {e}").into(),
            );
        }
        let ping = async {
            redis::cmd("PING")
                .query_async::<()>(&mut client.connection_manager)
                .await
        };
        match timeout(self.health_check_timeout, ping).await {
            Ok(Ok(())) => HealthStatus::new_ok(
                self,
                "RedisStore::check_health: PING ok after re-resolving master".into(),
            ),
            Ok(Err(e)) => HealthStatus::new_failed(
                self,
                format!("RedisStore::check_health: PING still errored after reconnect: {e}").into(),
            ),
            Err(_) => HealthStatus::new_failed(
                self,
                format!(
                    "RedisStore::check_health: PING still exceeded {}ms timeout after reconnect",
                    self.health_check_timeout.as_millis()
                )
                .into(),
            ),
        }
    }
}

// -------------------------------------------------------------------
// Below this line are specific to the redis scheduler implementation.
// -------------------------------------------------------------------

/// The time in milliseconds that a redis cursor can be idle before it is closed.
const CURSOR_IDLE_MS: u64 = 30_000;
/// The name of the field in the Redis hash that stores the data.
pub(crate) const DATA_FIELD_NAME: &str = "data";
/// The name of the field in the Redis hash that stores the version.
pub(crate) const VERSION_FIELD_NAME: &str = "version";

#[allow(rustdoc::broken_intra_doc_links)]
/// Lua script to set a key if the version matches.
/// Args:
///   KEYS[1]: The key where the version is stored.
///   ARGV[1]: The expected version.
///   ARGV[2]: TTL in seconds, or 0 for forever
///   ARGV[3]: The new data.
///   ARGV[4*]: Key-value pairs of additional data to include.
/// Returns:
///   The new version if the version matches. nil is returned if the
///   value was not set.
pub const LUA_VERSION_SET_SCRIPT: &str = formatcp!(
    r"
local key = KEYS[1]
local expected_version = tonumber(ARGV[1])
local ttl = tonumber(ARGV[2])
local new_data = ARGV[3]
local new_version = redis.call('HINCRBY', key, '{VERSION_FIELD_NAME}', 1)
local i
local indexes = {{}}

if new_version-1 ~= expected_version then
    local reverted = redis.call('HINCRBY', key, '{VERSION_FIELD_NAME}', -1)
    -- HINCRBY creates the key before the version can be checked, so a
    -- caller holding a stale version for a key that has since gone leaves
    -- behind a hash with nothing in it but a zeroed version. It has no
    -- data, no expiry and never gains either, yet it still matches the
    -- index prefix, so it sits in the index as an empty document and
    -- crowds every search that reads it. Only ever removes a key this
    -- call brought into existence: a real record always carries data.
    if reverted == 0 and redis.call('HEXISTS', key, '{DATA_FIELD_NAME}') == 0 then
        redis.call('DEL', key)
    end
    return {{ 0, new_version-1 }}
end
-- Skip first 3 argvs, as they are known inputs.
-- Remember: Lua is 1-indexed.
for i=4, #ARGV do
    indexes[i-3] = ARGV[i]
end

-- In testing we witnessed redis sometimes not update our FT indexes
-- resulting in stale data. It appears if we delete our keys then insert
-- them again it works and reduces risk significantly.
redis.call('DEL', key)
redis.call('HSET', key, '{DATA_FIELD_NAME}', new_data, '{VERSION_FIELD_NAME}', new_version, unpack(indexes))

if ttl ~= 0 then
    redis.call('EXPIRE', key, ttl)
end
return {{ 1, new_version }}
"
);

#[allow(rustdoc::broken_intra_doc_links)]
/// [`LUA_VERSION_SET_SCRIPT`], also keeping the key's sorted-index member
/// in step (see `redis_utils::sorted_index`). Used only when
/// `experimental_sorted_index` is configured, so the plain script stays
/// what every other deployment runs.
///
/// The old member is found from the hash before it is rewritten, so the
/// script touches a set it was not given as a key. That is fine on a
/// standalone or Sentinel server and wrong on Cluster, which is why the
/// store refuses the sorted index in Cluster mode.
/// Args:
///   KEYS[1]: The key where the version is stored.
///   ARGV[1]: The expected version.
///   ARGV[2]: TTL in seconds, or 0 for forever
///   ARGV[3]: The new data.
///   ARGV[4]: Set name prefix; the old member leaves `prefix .. old value`.
///   ARGV[5]: The indexed field naming the set.
///   ARGV[6]: The sort field.
///   ARGV[7]: The set to add the new member to, or empty for none.
///   ARGV[8]: The new member.
///   ARGV[9*]: Key-value pairs of additional data to include.
/// Returns:
///   As [`LUA_VERSION_SET_SCRIPT`].
pub const LUA_VERSION_SET_SORTED_SCRIPT: &str = formatcp!(
    r"
local key = KEYS[1]
local expected_version = tonumber(ARGV[1])
local ttl = tonumber(ARGV[2])
local new_data = ARGV[3]
local set_prefix = ARGV[4]
local index_field = ARGV[5]
local sort_field = ARGV[6]
local new_set = ARGV[7]
local new_member = ARGV[8]
local new_version = redis.call('HINCRBY', key, '{VERSION_FIELD_NAME}', 1)
local i
local indexes = {{}}

if new_version-1 ~= expected_version then
    local reverted = redis.call('HINCRBY', key, '{VERSION_FIELD_NAME}', -1)
    if reverted == 0 and redis.call('HEXISTS', key, '{DATA_FIELD_NAME}') == 0 then
        redis.call('DEL', key)
    end
    return {{ 0, new_version-1 }}
end
for i=9, #ARGV do
    indexes[i-8] = ARGV[i]
end

local old_value = redis.call('HGET', key, index_field)
local old_sort = redis.call('HGET', key, sort_field)
if old_value and old_sort then
    redis.call('ZREM', set_prefix .. old_value, old_sort .. ':' .. key)
end

redis.call('DEL', key)
redis.call('HSET', key, '{DATA_FIELD_NAME}', new_data, '{VERSION_FIELD_NAME}', new_version, unpack(indexes))

if ttl ~= 0 then
    redis.call('EXPIRE', key, ttl)
end
if new_set ~= '' then
    redis.call('ZADD', new_set, 0, new_member)
end
return {{ 1, new_version }}
"
);

/// How long a freshly created index is given to finish its background
/// scan before a read goes ahead regardless, and how often it is asked.
const INDEX_READY_TIMEOUT: Duration = Duration::from_secs(5);
const INDEX_READY_POLL: Duration = Duration::from_millis(50);

/// `FT.CREATE` of an index that exists: `RediSearch` answers an `Index`
/// extension error saying so, which a create that races another node gets.
fn is_index_already_exists(err: &redis::RedisError) -> bool {
    err.kind() == redis::ErrorKind::Extension
        && err.code() == Some("Index")
        && err
            .detail()
            .is_some_and(|d| d.to_ascii_lowercase().contains("already exists"))
}

/// Polls `FT.INFO` until the index reports `indexing 0`, the timeout
/// passes, or the call fails; none of those stops the caller, which reads
/// the index either way.
async fn wait_for_index_ready<C>(connection_manager: C, index: &str)
where
    C: ConnectionLike + Send + Clone,
{
    wait_for_index_ready_within(connection_manager, index, INDEX_READY_TIMEOUT).await;
}

async fn wait_for_index_ready_within<C>(connection_manager: C, index: &str, timeout: Duration)
where
    C: ConnectionLike + Send + Clone,
{
    let started = Instant::now();
    loop {
        match ft_info_indexing(connection_manager.clone(), index).await {
            Ok(false) => return,
            Ok(true) => {}
            Err(err) => {
                warn!(
                    ?err,
                    index, "Could not read FT.INFO after creating the index; reading it now"
                );
                return;
            }
        }
        if started.elapsed() >= timeout {
            warn!(
                index,
                "Index still backfilling after {timeout:?}; reading it now"
            );
            return;
        }
        sleep(INDEX_READY_POLL).await;
    }
}

/// This is the output of the calculations below hardcoded into the executable.
const FINGERPRINT_CREATE_INDEX_HEX: &str = "3e762c15";

#[cfg(test)]
mod test {
    use super::FINGERPRINT_CREATE_INDEX_HEX;

    /// String of the `FT.CREATE` command used to create the index template.
    const CREATE_INDEX_TEMPLATE: &str = "FT.CREATE {} ON HASH PREFIX 1 {} NOOFFSETS NOHL NOFIELDS NOFREQS SCHEMA {} TAG CASESENSITIVE SORTABLE";

    /// Compile-time fingerprint of the `FT.CREATE` command used to create the
    /// index template. This is a simple CRC32 checksum of the command string.
    /// We don't care about it actually being a valid CRC32 checksum, just that
    /// it's a unique identifier with a low chance of collision.
    const fn fingerprint_create_index_template() -> u32 {
        const POLY: u32 = 0xEDB8_8320;
        const DATA: &[u8] = CREATE_INDEX_TEMPLATE.as_bytes();
        let mut crc = 0xFFFF_FFFF;
        let mut i = 0;
        while i < DATA.len() {
            let byte = DATA[i];
            crc ^= byte as u32;

            let mut j = 0;
            while j < 8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ POLY
                } else {
                    crc >> 1
                };
                j += 1;
            }
            i += 1;
        }
        crc
    }

    /// Verify that our calculation always evaluates to this fixed value.
    #[test]
    fn test_fingerprint_value() {
        assert_eq!(
            format!("{:08x}", &fingerprint_create_index_template()),
            FINGERPRINT_CREATE_INDEX_HEX,
        );
    }
}

/// Get the name of the index to create for the given field.
/// This will add some prefix data to the name to try and ensure
/// if the index definition changes, the name will get a new name.
macro_rules! get_index_name {
    ($prefix:expr, $field:expr, $maybe_sort:expr) => {
        format_args!(
            "{}_{}_{}_{}",
            $prefix,
            $field,
            $maybe_sort.unwrap_or(""),
            FINGERPRINT_CREATE_INDEX_HEX
        )
    };
}

/// Try to sanitize a string to be used as a Redis key.
/// We don't actually modify the string, just check if it's valid.
const fn try_sanitize(s: &str) -> bool {
    // Note: We cannot use for loops or iterators here because they are not const.
    // Allowing us to use a const function here gives the compiler the ability to
    // optimize this function away entirely in the case where the input is constant.
    let chars = s.as_bytes();
    let mut i: usize = 0;
    let len = s.len();
    loop {
        if i >= len {
            break;
        }
        let c = chars[i];
        if !c.is_ascii_alphanumeric() && c != b'_' {
            return false;
        }
        i += 1;
    }
    true
}

/// An individual subscription to a key in Redis.
#[derive(Debug)]
pub struct RedisSubscription {
    receiver: Option<tokio::sync::watch::Receiver<String>>,
    weak_subscribed_keys: Weak<RwLock<StringPatriciaMap<RedisSubscriptionPublisher>>>,
}

impl SchedulerSubscription for RedisSubscription {
    /// Wait for the subscription key to change.
    async fn changed(&mut self) -> Result<(), Error> {
        let receiver = self
            .receiver
            .as_mut()
            .ok_or_else(|| make_err!(Code::Internal, "In RedisSubscription::changed::as_mut"))?;
        receiver.changed().await.map_err(|err| {
            Error::from_std_err(Code::Internal, &err)
                .append("In RedisSubscription::changed::changed")
        })
    }
}

// If the subscription is dropped, we need to possibly remove the key from the
// subscribed keys map.
impl Drop for RedisSubscription {
    fn drop(&mut self) {
        let Some(receiver) = self.receiver.take() else {
            warn!("RedisSubscription has already been dropped, nothing to do.");
            return; // Already dropped, nothing to do.
        };
        let key = receiver.borrow().clone();
        let Some(subscribed_keys) = self.weak_subscribed_keys.upgrade() else {
            return; // Parent dropped — nothing to do.
        };
        let mut subscribed_keys = subscribed_keys.write();
        let Some(publisher) = subscribed_keys.get(&key) else {
            warn!(
                %key,
                "RedisSubscription::drop: key absent from subscribed_keys under write lock — \
                 indicates an unexpected removal path",
            );
            return;
        };
        // Count includes our own (still-alive) receiver. If we are the
        // sole subscriber, remove the publisher entry.
        if publisher.receiver_count() == 1 {
            subscribed_keys.remove(&key);
        }
        drop(receiver);
    }
}

/// A publisher for a key in Redis.
#[derive(Debug)]
struct RedisSubscriptionPublisher {
    sender: Mutex<tokio::sync::watch::Sender<String>>,
}

impl RedisSubscriptionPublisher {
    fn new(
        key: String,
        weak_subscribed_keys: Weak<RwLock<StringPatriciaMap<Self>>>,
    ) -> (Self, RedisSubscription) {
        let (sender, receiver) = tokio::sync::watch::channel(key);
        let publisher = Self {
            sender: Mutex::new(sender),
        };
        let subscription = RedisSubscription {
            receiver: Some(receiver),
            weak_subscribed_keys,
        };
        (publisher, subscription)
    }

    fn subscribe(
        &self,
        weak_subscribed_keys: Weak<RwLock<StringPatriciaMap<Self>>>,
    ) -> RedisSubscription {
        let receiver = self.sender.lock().subscribe();
        RedisSubscription {
            receiver: Some(receiver),
            weak_subscribed_keys,
        }
    }

    fn receiver_count(&self) -> usize {
        self.sender.lock().receiver_count()
    }

    fn notify(&self) {
        // TODO(https://github.com/sile/patricia_tree/issues/40) When this is addressed
        // we can remove the `Mutex` and use the mutable iterator directly.
        self.sender.lock().send_modify(|_| {});
    }
}

#[derive(Debug, Clone)]
pub struct RedisSubscriptionManager {
    subscribed_keys: Arc<RwLock<StringPatriciaMap<RedisSubscriptionPublisher>>>,
    tx_for_test: UnboundedSender<String>,
    _subscription_spawn: Arc<Mutex<JoinHandleDropGuard<()>>>,
}

impl RedisSubscriptionManager {
    pub fn new(
        subscriber_channel: UnboundedReceiver<PushInfo>,
        remove_callbacks: Arc<async_lock::Mutex<Vec<RemoveCallback>>>,
        key_prefix: String,
    ) -> Self {
        let subscribed_keys = Arc::new(RwLock::new(StringPatriciaMap::new()));
        let subscribed_keys_weak = Arc::downgrade(&subscribed_keys);
        let (tx_for_test, mut rx_for_test) = unbounded_channel();
        let mut local_subscriber_channel = UnboundedReceiverStream::new(subscriber_channel);
        Self {
            subscribed_keys,
            tx_for_test,
            _subscription_spawn: Arc::new(Mutex::new(spawn!(
                "redis_subscribe_spawn",
                async move {
                    debug!("running subscribe loop");
                    loop {
                        loop {
                            let key = select! {
                                value = rx_for_test.recv() => {
                                    let Some(value) = value else {
                                        unreachable!("Channel should never close");
                                    };
                                    value
                                },
                                maybe_push_info = local_subscriber_channel.next() => {
                                    if let Some(push_info) = maybe_push_info {
                                        match push_info.kind {
                                            redis::PushKind::PMessage => {},
                                            redis::PushKind::PSubscribe => {
                                                trace!(?push_info, "PSubscribe, ignore");
                                                continue;
                                            }
                                            _ => {
                                                warn!(?push_info, "Other push_info message, discarded");
                                                continue;
                                            },
                                        }
                                        if push_info.data.len() != 3 {
                                            error!(?push_info, "Expected exactly 3 values on subscriber channel (pattern, channel, value)");
                                            continue;
                                        }
                                        let value = match push_info.data.last().unwrap() {
                                            Value::SimpleString(s) => {
                                                s.clone()
                                            }
                                            Value::BulkString(v) => {
                                                String::from_utf8(v.clone()).expect("String message")
                                            }
                                            other => {
                                                error!(?other, "Received non-string message in RedisSubscriptionManager");
                                                continue;
                                            }
                                        };
                                        // Redis reports maxmemory eviction as
                                        // "evicted" and TTL expiry as "expired".
                                        // Both mean the key is gone, so both have
                                        // to invalidate anything caching its
                                        // existence; treating only one as a removal
                                        // leaves an ExistenceCacheStore claiming to
                                        // hold a key that Redis has already dropped.
                                        if value == "evicted" || value == "expired" {
                                            trace!(?push_info, %value, "Key removal event");
                                            let removed_key = if let Some(key) = push_info.data.get(1) {
                                                if let Value::BulkString(s) = key {
                                                    String::from_utf8(s.clone()).expect("String message")
                                                } else {
                                                    error!(?push_info, "Removed key wasn't bulk-string");
                                                    continue;
                                                }
                                            } else {
                                                error!(?push_info, "No key in removal event");
                                                continue;
                                            };
                                            trace!(?removed_key, "Removed key");
                                            let Some((_prefix, internal_key)) = removed_key.split_once(':') else {
                                                error!(?removed_key, "Removed key doesn't contain a colon");
                                                continue;
                                            };

                                            let store_key = match decode_key(&key_prefix, Cow::from(internal_key)) {
                                                Ok(k) => k.into_owned(),
                                                Err(err) => {
                                                    error!(%err, internal_key, "Bad redis key");
                                                    continue;
                                                }
                                            };
                                            let locked_remove_callbacks = remove_callbacks.lock().await;
                                            let mut callbacks: FuturesUnordered<_> =
                                                locked_remove_callbacks.iter()
                                                .map(|callback| callback.callback(store_key.borrow()))
                                                .collect();
                                            while callbacks.next().await.is_some() {}
                                            continue
                                        }
                                        value
                                    } else {
                                        error!("Error receiving message in RedisSubscriptionManager from subscriber_channel");
                                        break;
                                    }
                                }
                            };
                            trace!(key, "New subscription manager key");
                            let Some(subscribed_keys) = subscribed_keys_weak.upgrade() else {
                                warn!(
                                    "It appears our parent has been dropped, exiting RedisSubscriptionManager spawn"
                                );
                                return;
                            };
                            let subscribed_keys_mux = subscribed_keys.read();
                            subscribed_keys_mux
                                .common_prefix_values(&*key)
                                .for_each(RedisSubscriptionPublisher::notify);
                        }
                        // Sleep for a small amount of time to ensure we don't reconnect too quickly.
                        sleep(Duration::from_secs(1)).await;
                        // If we reconnect or lag behind we might have had dirty keys, so we need to
                        // flag all of them as changed.
                        let Some(subscribed_keys) = subscribed_keys_weak.upgrade() else {
                            warn!(
                                "It appears our parent has been dropped, exiting RedisSubscriptionManager spawn"
                            );
                            return;
                        };
                        let subscribed_keys_mux = subscribed_keys.read();
                        // Just in case also get a new receiver.
                        for publisher in subscribed_keys_mux.values() {
                            publisher.notify();
                        }
                    }
                }
            ))),
        }
    }
}

impl SubscriptionManagerNotify for RedisSubscriptionManager {
    fn notify_for_test(&self, value: String) {
        self.tx_for_test.send(value).unwrap();
    }
}

impl SchedulerSubscriptionManager for RedisSubscriptionManager {
    type Subscription = RedisSubscription;

    fn subscribe<K>(&self, key: K) -> Result<Self::Subscription, Error>
    where
        K: SchedulerStoreKeyProvider,
    {
        let weak_subscribed_keys = Arc::downgrade(&self.subscribed_keys);
        let mut subscribed_keys = self.subscribed_keys.write();
        let key = key.get_key();
        let key_str = key.as_str();
        let mut subscription = if let Some(publisher) = subscribed_keys.get(&key_str) {
            publisher.subscribe(weak_subscribed_keys)
        } else {
            let (publisher, subscription) =
                RedisSubscriptionPublisher::new(key_str.to_string(), weak_subscribed_keys);
            subscribed_keys.insert(key_str, publisher);
            subscription
        };
        subscription
            .receiver
            .as_mut()
            .ok_or_else(|| {
                make_err!(
                    Code::Internal,
                    "Receiver should be set in RedisSubscriptionManager::subscribe"
                )
            })?
            .mark_changed();

        Ok(subscription)
    }

    fn is_reliable() -> bool {
        false
    }
}

/// How the scheduler's index for `K` is created: never `TEMPORARY`, since
/// `RediSearch` 2.10 never collects a temporary index and every update of an
/// indexed hash leaves a dead entry behind for good. A persistent index is
/// collected. An index an older release created temporary must not be
/// dropped the ordinary way, which deletes its hashes: see
/// `redis_utils::index_migration`.
fn index_definition<K: SchedulerIndexProvider>() -> (FtCreateOptions, Vec<SearchSchema>) {
    let mut schema = vec![SearchSchema {
        field_name: K::INDEX_NAME.into(),
        sortable: false,
    }];
    if let Some(sort_key) = K::MAYBE_SORT_KEY {
        schema.push(SearchSchema {
            field_name: sort_key.into(),
            sortable: true,
        });
    }
    let create_options = FtCreateOptions {
        prefixes: vec![K::KEY_PREFIX.into()],
        nohl: true,
        nofields: true,
        nofreqs: true,
        nooffsets: true,
        temporary: None,
    };
    (create_options, schema)
}

/// How long a settled index name is trusted before it is checked again. A
/// failed check is trusted as long, so a server that refuses the check
/// (some managed Redis refuse `FT.DEBUG`) is asked once a minute, not on
/// every read.
const INDEX_MIGRATION_RECHECK: Duration = Duration::from_secs(60);

/// How long an answer that holds only while a replacement backfills is
/// trusted, so concurrent reads do not each repeat the check.
const INDEX_MIGRATION_POLL: Duration = Duration::from_secs(1);

/// How long a read waits for a replacement index to finish backfilling when
/// there is no original left to read meanwhile. Reads normally stay on the
/// original while the replacement fills, so this is only hit if the
/// original was removed some other way.
const MIGRATION_READY_TIMEOUT: Duration = Duration::from_secs(600);

impl<C, M> RedisStore<C, M>
where
    C: Clone + ConnectionLike + Sync + Send + 'static,
    M: RedisManager<C> + Sync + Send + 'static,
{
    fn original_index_name<K: SchedulerIndexProvider>() -> String {
        format!(
            "{}",
            get_index_name!(K::KEY_PREFIX, K::INDEX_NAME, K::MAYBE_SORT_KEY)
        )
    }

    /// The index a read of `K` uses, advancing the migration of an inherited
    /// temporary index by one step when `migrate_temporary_indexes` is set.
    /// Any failure reads the original, which is what reads did before.
    async fn read_index_name<K: SchedulerIndexProvider>(&self) -> String {
        let original = Self::original_index_name::<K>();
        if !self.migrate_temporary_indexes {
            return original;
        }
        if let Some((name, until)) = self.index_names.lock().get(&original)
            && Instant::now() < *until
        {
            return name.clone();
        }
        let (name, trust) = match self.migration_step::<K>(&original).await {
            Ok(decided) => decided,
            Err(err) => {
                warn!(index = %original, ?err, "Could not check the index for migration; reading it");
                (original.clone(), INDEX_MIGRATION_RECHECK)
            }
        };
        self.index_names
            .lock()
            .insert(original, (name.clone(), Instant::now() + trust));
        name
    }

    /// Drops the decided name for `K`, so the next read checks again. Used
    /// when the index a read was given has gone, which happens to a read
    /// that was handed the original just before another process retired it.
    fn forget_index_name<K: SchedulerIndexProvider>(&self) {
        self.index_names
            .lock()
            .remove(&Self::original_index_name::<K>());
    }

    /// The index a count of `K` uses. Counting never migrates: it follows
    /// what reads decided.
    fn count_index_name<K: SchedulerIndexProvider>(&self) -> String {
        let original = Self::original_index_name::<K>();
        self.index_names
            .lock()
            .get(&original)
            .map_or(original, |(name, _)| name.clone())
    }

    /// One step of `redis_utils::index_migration`. Returns the index to read
    /// and how long that answer holds.
    async fn migration_step<K: SchedulerIndexProvider>(
        &self,
        original: &str,
    ) -> Result<(String, Duration), Error> {
        let replacement = migrated_name(original);
        let (mut connection, _) = self.connection_manager.get_connection().await?;
        let original_kind = index_kind(&mut connection, original).await?;
        let replacement_state = match index_kind(&mut connection, &replacement).await? {
            IndexKind::Missing => None,
            IndexKind::Persistent => {
                // The original's count is read first: a write that lands
                // between the two reads then adds to the replacement, never
                // only to the original.
                let original_docs = match original_kind {
                    IndexKind::Missing => None,
                    _ => Some(ft_info(connection.clone(), original).await?.num_docs),
                };
                let info = ft_info(connection.clone(), &replacement).await?;
                let state = index_migration::replacement_state(info, original_docs);
                if state == Replacement::Incomplete {
                    error!(
                        %original,
                        %replacement,
                        replacement_docs = info.num_docs,
                        ?original_docs,
                        hash_indexing_failures = info.hash_indexing_failures,
                        "The replacement index does not hold every key; reads stay on the \
                         original. Check Redis memory, then drop the replacement with \
                         `FT.DROPINDEX {replacement} _FORCEKEEPDOCS` to build it again"
                    );
                }
                Some(state)
            }
            IndexKind::Temporary(_) => {
                return Err(make_err!(
                    Code::Internal,
                    "Replacement index {replacement} is temporary; it was not created by this code"
                ));
            }
        };
        match index_migration::plan(original_kind, replacement_state) {
            Step::UseOriginal => Ok((
                original.to_string(),
                if original_kind == IndexKind::Persistent {
                    INDEX_MIGRATION_RECHECK
                } else {
                    INDEX_MIGRATION_POLL
                },
            )),
            Step::CreateReplacement => {
                let (create_options, schema) = index_definition::<K>();
                match ft_create(
                    connection.clone(),
                    replacement.clone(),
                    create_options,
                    schema,
                )
                .await
                {
                    Ok(()) => {
                        info!(%original, %replacement, "Building a persistent index to replace a temporary one");
                    }
                    Err(err) if is_index_already_exists(&err) => {}
                    Err(err) if is_out_of_memory(&err) => {
                        error!(
                            %original,
                            %replacement,
                            "Redis is at maxmemory, so the persistent index that replaces the \
                             temporary one cannot be built; reads stay on the temporary index. \
                             Raise maxmemory, or restart Redis: a restart rebuilds the temporary \
                             index without its dead entries, which frees most of the memory"
                        );
                        return Ok((original.to_string(), INDEX_MIGRATION_RECHECK));
                    }
                    Err(err) => return Err(err.into()),
                }
                Ok((original.to_string(), INDEX_MIGRATION_POLL))
            }
            Step::WaitOnOriginal => Ok((original.to_string(), INDEX_MIGRATION_POLL)),
            Step::KeepOriginal => Ok((original.to_string(), INDEX_MIGRATION_RECHECK)),
            Step::WaitForReplacement => {
                wait_for_index_ready_within(connection, &replacement, MIGRATION_READY_TIMEOUT)
                    .await;
                Ok((replacement, INDEX_MIGRATION_POLL))
            }
            Step::UseReplacement { retire_original } => {
                if retire_original {
                    drop_keeping_documents(&mut connection, original).await?;
                    info!(%original, %replacement, "Retired the temporary index, keeping its keys");
                }
                Ok((replacement, INDEX_MIGRATION_RECHECK))
            }
        }
    }

    /// The sorted-set read of `K` for `index_value`, when this store serves
    /// it from one: configured to, reconciled at least once in this
    /// process, and its first page readable. `None` means read through
    /// `RediSearch` as without the sorted index.
    async fn sorted_read<K>(&self, index_value: &str) -> Option<SortedRead<C, K>>
    where
        K: SchedulerIndexProvider + SchedulerStoreDecodeTo,
        <K as SchedulerStoreDecodeTo>::DecodeOutput: Send,
    {
        let sorted_index = self.sorted_index.as_ref()?;
        let set = sorted_index.readable_set::<K>(&self.key_prefix, index_value)?;
        self.reconcile_sorted::<K>(sorted_index, &set, index_value)
            .await;
        if !sorted_index.has_reconciled(&set) {
            return None;
        }
        let connection = match self.connection_manager.get_connection().await {
            Ok((connection, _)) => connection,
            Err(err) => {
                warn!(%set, ?err, "No connection for a sorted-index read; reading through RediSearch");
                return None;
            }
        };
        let mut read = SortedRead::new(
            connection,
            set,
            index_value.to_string(),
            self.max_count_per_cursor,
            K::SORT_DESCENDING,
        );
        // The first page is read here, so a set that cannot be read sends
        // this call to RediSearch instead of failing the caller's pass. An
        // empty first page goes there too: an idle queue costs one cheap
        // query, and a set that is gone (evicted, or lost with its server)
        // cannot pass for an empty queue.
        match read.fetch_page().await {
            Ok(()) if read.has_rows() => Some(read),
            Ok(()) => None,
            Err(err) => {
                warn!(?err, "Sorted-index read failed; reading through RediSearch");
                None
            }
        }
    }

    /// Reconciles `set` when it is due. See `sorted_index::reconcile`.
    async fn reconcile_sorted<K: SchedulerIndexProvider>(
        &self,
        sorted_index: &SortedIndexState,
        set: &str,
        index_value: &str,
    ) {
        let Some(index) = SortedIndex::of::<K>() else {
            return;
        };
        if !sorted_index.claim_reconcile(set) {
            return;
        }
        let started = Instant::now();
        let result = async {
            let (mut connection, _) = self.connection_manager.get_connection().await?;
            let ft_index = self.count_index_name::<K>();
            // Serving reads from the set takes them off the index, and a
            // temporary index that goes unqueried expires, deleting every
            // hash it covers. A set that never reconciles is never read.
            if let Ok(IndexKind::Temporary(_)) = index_kind(&mut connection, &ft_index).await {
                return Err(make_err!(
                    Code::FailedPrecondition,
                    "{ft_index} is a temporary index: set migrate_temporary_indexes and let it \
                     finish before the sorted index is read"
                ));
            }
            sorted_index::reconcile(
                connection,
                ft_index,
                format!("@{}:{{ {} }}", K::INDEX_NAME, index_value),
                set,
                &index,
                self.max_count_per_cursor,
            )
            .await
        }
        .await;
        match result {
            Ok(added) => {
                info!(%set, added, elapsed = ?started.elapsed(), "Reconciled the sorted index");
                sorted_index.finish_reconcile(set, true);
            }
            Err(err) => {
                warn!(%set, ?err, "Sorted-index reconcile failed; retrying shortly");
                sorted_index.finish_reconcile(set, false);
            }
        }
    }

    /// `ZCARD` of the set a count of `K` for `index_value` would be served
    /// from, when there is one. Counts members a read has not yet found
    /// stale, which only a write that bypassed the script leaves.
    async fn sorted_count<K: SchedulerIndexProvider>(&self, index_value: &str) -> Option<u64> {
        let sorted_index = self.sorted_index.as_ref()?;
        let set = sorted_index.readable_set::<K>(&self.key_prefix, index_value)?;
        if !sorted_index.has_reconciled(&set) {
            return None;
        }
        let (mut connection, _) = self.connection_manager.get_connection().await.ok()?;
        match redis::cmd("ZCARD")
            .arg(&set)
            .query_async::<u64>(&mut connection)
            .await
        {
            // As for reads: an empty set is confirmed through RediSearch.
            Ok(0) => None,
            Ok(count) => Some(count),
            Err(err) => {
                warn!(%set, ?err, "Sorted-index count failed; counting through RediSearch");
                None
            }
        }
    }
}

impl<C, M> SchedulerStore for RedisStore<C, M>
where
    C: Clone + ConnectionLike + Sync + Send + 'static,
    M: RedisManager<C> + Sync + Send + 'static,
{
    type SubscriptionManager = RedisSubscriptionManager;

    fn subscription_manager(
        &self,
    ) -> impl Future<Output = Result<Arc<RedisSubscriptionManager>, Error>> {
        std::future::ready(if self.pub_sub_channel.is_none() {
            Err(make_input_err!(
                "RedisStore must have a pubsub for Redis Scheduler if using subscriptions"
            ))
        } else {
            Ok(self.subscription_manager.clone())
        })
    }

    async fn update_data<T>(&self, data: T, expiry: Option<Duration>) -> Result<Option<i64>, Error>
    where
        T: SchedulerStoreDataProvider
            + SchedulerStoreKeyProvider
            + SchedulerCurrentVersionProvider
            + Send,
    {
        let key = data.get_key();
        let redis_key = self.encode_key(&key);
        let mut client = self.get_client().await?;
        let maybe_index = data.get_indexes().err_tip(|| {
            format!("Err getting index in RedisStore::update_data::versioned for {redis_key}")
        })?;
        if <T as SchedulerStoreKeyProvider>::Versioned::VALUE {
            let current_version = data.current_version();
            let sorted_write =
                self.sorted_index
                    .as_ref()
                    .zip(data.sorted_index())
                    .map(|(sorted_index, index)| {
                        sorted_index.write_args(
                            &self.key_prefix,
                            &index,
                            &maybe_index,
                            redis_key.as_ref(),
                            expiry.is_some(),
                        )
                    });
            let data = data.try_into_bytes().err_tip(|| {
                format!("Could not convert value to bytes in RedisStore::update_data::versioned for {redis_key}")
            })?;
            let mut script = if sorted_write.is_some() {
                self.sorted_script.key(redis_key.as_ref())
            } else {
                self.connection_manager.update_script(redis_key.as_ref())
            };
            let mut script_invocation = script
                .arg(format!("{current_version}"))
                .arg(expiry.unwrap_or(Duration::ZERO).as_secs())
                .arg(data.to_vec());
            if let Some(sorted_write) = &sorted_write {
                script_invocation = script_invocation
                    .arg(&sorted_write.set_prefix)
                    .arg(sorted_write.index_field)
                    .arg(sorted_write.sort_field)
                    .arg(&sorted_write.new_set)
                    .arg(&sorted_write.new_member);
            }
            for (name, value) in maybe_index {
                script_invocation = script_invocation.arg(name).arg(value.to_vec());
            }
            let start = Instant::now();
            let (success, new_version): (bool, i64) = match script_invocation
                .invoke_async(&mut client.connection_manager)
                .await
            {
                Ok(v) => v,
                Err(err) if is_retryable_redis_error(&err) => {
                    client.reconnect(&self.connection_manager).await?;
                    script_invocation
                        .invoke_async(&mut client.connection_manager)
                        .await
                        .err_tip(|| format!("(after reconnect) In RedisStore::update_data::versioned for {key:?}"))?
                }
                Err(err) => {
                    let mut error: Error = err.into();
                    error
                        .messages
                        .push(format!("In RedisStore::update_data::versioned for {key:?}"));
                    return Err(error);
                }
            };

            let elapsed = start.elapsed();

            if elapsed > Duration::from_millis(100) {
                warn!(
                    %redis_key,
                    ?elapsed,
                    "Slow Redis version-set operation"
                );
            }
            if !success {
                warn!(
                    %redis_key,
                    %key,
                    %current_version,
                    %new_version,
                    caller = core::any::type_name::<T>(),
                    "Redis version conflict - optimistic lock failed"
                );
                return Ok(None);
            }
            trace!(
                %redis_key,
                %key,
                old_version = %current_version,
                %new_version,
                "Updated redis key to new version"
            );
            // If we have a publish channel configured, send a notice that the key has been set.
            if let Some(pub_sub_channel) = &self.pub_sub_channel {
                return Ok(client
                    .connection_manager
                    .publish(pub_sub_channel, redis_key.as_ref())
                    .await?);
            }
            Ok(Some(new_version))
        } else {
            let data = data.try_into_bytes().err_tip(|| {
                format!("Could not convert value to bytes in RedisStore::update_data::noversion for {redis_key}")
            })?;
            let mut fields: Vec<(String, _)> = vec![];
            fields.push((DATA_FIELD_NAME.into(), data.to_vec()));
            for (name, value) in maybe_index {
                fields.push((name.into(), value.to_vec()));
            }
            match client
                .connection_manager
                .hset_multiple::<_, _, _, ()>(redis_key.as_ref(), &fields)
                .await
            {
                Ok(_v) => {
                    if let Some(expiry_v) = expiry {
                        let seconds =
                            TryInto::<i64>::try_into(expiry_v.as_secs()).err_tip(|| {
                                format!("Expiry seconds doesn't map to i64: {expiry_v:#?}")
                            })?;
                        let expiry_result: u8 = client
                            .connection_manager
                            .expire(redis_key.as_ref(), seconds)
                            .await
                            .err_tip(|| {
                                format!(
                                    "In RedisStore::update_data::noversion (expiry) for {redis_key}"
                                )
                            })?;
                        if expiry_result != 1 {
                            warn!(%redis_key, seconds, "Wasn't able to set expiry for Redis key");
                        }
                    }
                }
                Err(err) if is_retryable_redis_error(&err) => {
                    client.reconnect(&self.connection_manager).await?;
                    client
                        .connection_manager
                        .hset_multiple::<_, _, _, ()>(redis_key.as_ref(), &fields)
                        .await
                        .err_tip(|| format!("(after reconnect) In RedisStore::update_data::noversion (hset) for {redis_key}"))?;
                    if let Some(expiry_v) = expiry {
                        let seconds =
                            TryInto::<i64>::try_into(expiry_v.as_secs()).err_tip(|| {
                                format!("Expiry seconds doesn't map to i64: {expiry_v:#?}")
                            })?;
                        let expiry_result: u8 = client.connection_manager.expire(redis_key.as_ref(), seconds).await
                        .err_tip(|| format!("(after reconnect) In RedisStore::update_data::noversion (expiry) for {redis_key}"))?;
                        if expiry_result != 1 {
                            warn!(%redis_key, seconds, "Wasn't able to set expiry for Redis key");
                        }
                    }
                }
                Err(err) => {
                    let mut error: Error = err.into();
                    error.messages.push(format!(
                        "In RedisStore::update_data::noversion for {redis_key}"
                    ));
                    return Err(error);
                }
            }
            // If we have a publish channel configured, send a notice that the key has been set.
            if let Some(pub_sub_channel) = &self.pub_sub_channel {
                return Ok(client
                    .connection_manager
                    .publish(pub_sub_channel, redis_key.as_ref())
                    .await?);
            }
            Ok(Some(0)) // Always use "0" version since this is not a versioned request.
        }
    }

    async fn search_by_index_prefix<K>(
        &self,
        index: K,
    ) -> Result<
        impl Stream<Item = Result<<K as SchedulerStoreDecodeTo>::DecodeOutput, Error>> + Send,
        Error,
    >
    where
        K: SchedulerIndexProvider + SchedulerStoreDecodeTo + Send,
        <K as SchedulerStoreDecodeTo>::DecodeOutput: Send,
    {
        let index_value = index.index_value();
        try_sanitize(index_value.as_ref())
            .then_some(())
            .err_tip(|| {
                format!("In RedisStore::search_by_index_prefix::try_sanitize - {index_value:?}")
            })?;
        if let Some(read) = self.sorted_read::<K>(index_value.as_ref()).await {
            return Ok(read.into_stream().left_stream());
        }
        let index_name = self.read_index_name::<K>().await;
        let run_ft_aggregate = |connection_manager: C, index_name: String| async {
            ft_aggregate(
                connection_manager,
                index_name,
                if index_value.is_empty() {
                    "*".to_string()
                } else {
                    format!("@{}:{{ {} }}", K::INDEX_NAME, index_value)
                },
                FtAggregateOptions {
                    load: vec![DATA_FIELD_NAME.into(), VERSION_FIELD_NAME.into()],
                    cursor: FtAggregateCursor {
                        count: self.max_count_per_cursor,
                        max_idle: CURSOR_IDLE_MS,
                    },
                    sort_by: K::MAYBE_SORT_KEY.map_or_else(Vec::new, |v| vec![format!("@{v}")]),
                    sort_desc: K::SORT_DESCENDING,
                },
            )
            .await
        };
        let run_ft_create = |connection_manager: C| async {
            let (create_options, schema) = index_definition::<K>();
            ft_create(
                connection_manager,
                index_name.clone(),
                create_options,
                schema,
            )
            .await
        };

        let (connection_manager, connect_id) = self.connection_manager.get_connection().await?;
        let stream = match run_ft_aggregate(connection_manager.clone(), index_name.clone()).await {
            // A demoted master answers READONLY and a dead/old master drops the
            // connection or times the command out. Both mean the master moved
            // (Sentinel failover) — re-resolve it and retry rather than letting
            // the scheduler's matching loop spin on a stale handle. (A missing
            // index is not retryable here; it falls through to the create path
            // below, which creates the index and retries the aggregate inline.)
            Err(err) if is_retryable_redis_error(&err) => {
                let (connection_manager, _connect_id) =
                    self.connection_manager.reconnect(connect_id).await?;
                run_ft_aggregate(connection_manager, index_name.clone())
                    .await
                    .err_tip(|| {
                        format!(
                            "Error with reconnected ft_aggregate in RedisStore::search_by_index_prefix({index_name})"
                        )
                    })
            }
            Err(_) => {
                // The index this read was given may have just been retired
                // by another process finishing the migration. Creating it
                // again would bring back the index the migration removed and
                // read it before it is full; ask again which index to read.
                let moved = if self.migrate_temporary_indexes {
                    self.forget_index_name::<K>();
                    let current = self.read_index_name::<K>().await;
                    (current != index_name).then_some(current)
                } else {
                    None
                };
                if let Some(current) = moved {
                    run_ft_aggregate(connection_manager, current.clone())
                        .await
                        .err_tip(|| {
                            format!(
                                "Error with re-resolved ft_aggregate in RedisStore::search_by_index_prefix({current})"
                            )
                        })
                } else {
                    let (connection_manager, result) =
                        match run_ft_create(connection_manager.clone()).await {
                            Err(err) if is_retryable_redis_error(&err) => {
                                let (connection_manager, _connect_id) =
                                    self.connection_manager.reconnect(connect_id).await?;
                                (
                                    connection_manager.clone(),
                                    run_ft_create(connection_manager).await,
                                )
                            }
                            result => (connection_manager, result),
                        };

                    // RediSearch returns ErrorKind::Extension with code "Index"
                    // and detail along the lines of "Index already exists" when
                    // FT.CREATE races with another node.
                    let create_result = result.or_else(|e| {
                        if is_index_already_exists(&e) {
                            Ok(())
                        } else {
                            Err(e).err_tip(|| {
                                format!(
                                    "Error with ft_create in RedisStore::search_by_index_prefix({index_name})"
                                )
                            })
                        }
                    });

                    // FT.CREATE returns before the index holds the keys that
                    // already exist; RediSearch scans them in the background.
                    // An aggregate in that window reads a partial index, and
                    // for the scheduler's queue that is a queue with actions
                    // missing, so wait for the scan to finish first. The wait
                    // is bounded; past it the aggregate runs anyway and the next
                    // pass re-reads.
                    if create_result.is_ok() {
                        wait_for_index_ready(connection_manager.clone(), &index_name).await;
                    }

                    let run_result = run_ft_aggregate(connection_manager, index_name.clone())
                        .await
                        .err_tip(|| {
                        format!(
                            "Error with second ft_aggregate in RedisStore::search_by_index_prefix({index_name})"
                        )
                    });

                    // Creating the index will race which is ok. If it fails to create, we only
                    // error if the second ft_aggregate call fails and fails to create.
                    run_result.or_else(move |e| create_result.merge(Err(e)))
                }
            }
            Ok(stream) => Ok(stream),
        }?;

        Ok(stream
            .filter_map(|result| async move {
                let raw_redis_map = match result {
                    Ok(v) => v,
                    Err(e) => {
                        return Some(Err(Error::from(e)).err_tip(
                            || "Error in stream of in RedisStore::search_by_index_prefix",
                        ));
                    }
                };

                if matches!(raw_redis_map, Value::Int(_)) {
                    return None;
                }

                let Some(redis_map) = raw_redis_map.as_sequence() else {
                    return Some(Err(Error::new(
                        Code::Internal,
                        format!("Non-array from ft_aggregate: {raw_redis_map:?}"),
                    )));
                };
                let mut redis_map_iter = redis_map.iter();
                let mut bytes_data: Option<Bytes> = None;
                let mut version: Option<i64> = None;
                while let Some(key) = redis_map_iter.next() {
                    let value = redis_map_iter.next().unwrap();
                    let Value::BulkString(k) = key else {
                        return Some(Err(Error::new(
                            Code::Internal,
                            format!("Non-BulkString key from ft_aggregate: {key:?}"),
                        )));
                    };
                    let Ok(str_key) = str::from_utf8(k) else {
                        return Some(Err(Error::new(
                            Code::Internal,
                            format!("Non-utf8 key from ft_aggregate: {key:?}"),
                        )));
                    };
                    let Value::BulkString(v) = value else {
                        return Some(Err(Error::new(
                            Code::Internal,
                            format!("Non-BulkString value from ft_aggregate: {key:?}"),
                        )));
                    };
                    match str_key {
                        DATA_FIELD_NAME => {
                            bytes_data = Some(v.clone().into());
                        }
                        VERSION_FIELD_NAME => {
                            let Ok(str_v) = str::from_utf8(v) else {
                                return Some(Err(Error::new(
                                    Code::Internal,
                                    format!("Non-utf8 version value from ft_aggregate: {v:?}"),
                                )));
                            };
                            let Ok(raw_version) = str_v.parse::<i64>() else {
                                return Some(Err(Error::new(
                                    Code::Internal,
                                    format!(
                                        "Non-integer version value from ft_aggregate: {str_v:?}"
                                    ),
                                )));
                            };
                            version = Some(raw_version);
                        }
                        other => {
                            if K::MAYBE_SORT_KEY == Some(other) {
                                // ignore sort keys
                            } else {
                                return Some(Err(Error::new(
                                    Code::Internal,
                                    format!("Extra keys from ft_aggregate: {other}"),
                                )));
                            }
                        }
                    }
                }
                let Some(found_bytes_data) = bytes_data else {
                    return Some(Err(Error::new(
                        Code::Internal,
                        format!(
                            "Missing '{DATA_FIELD_NAME}' in ft_aggregate, got: {raw_redis_map:?}"
                        ),
                    )));
                };
                Some(
                    K::decode(version.unwrap_or(0), found_bytes_data)
                        .err_tip(|| "In RedisStore::search_by_index_prefix::decode"),
                )
            })
            .right_stream())
    }

    async fn count_by_index_prefix<K>(&self, index: K) -> Result<u64, Error>
    where
        K: SchedulerIndexProvider + Send,
    {
        let index_value = index.index_value();
        try_sanitize(index_value.as_ref())
            .then_some(())
            .err_tip(|| {
                format!("In RedisStore::count_by_index_prefix::try_sanitize - {index_value:?}")
            })?;
        if let Some(count) = self.sorted_count::<K>(index_value.as_ref()).await {
            return Ok(count);
        }
        let index_name = self.count_index_name::<K>();
        let query = if index_value.is_empty() {
            "*".to_string()
        } else {
            format!("@{}:{{ {} }}", K::INDEX_NAME, index_value)
        };

        let (connection_manager, connect_id) = self.connection_manager.get_connection().await?;
        // Same failover handling as search_by_index_prefix: a demoted master
        // answers READONLY and a dead one drops the connection, both meaning the
        // master moved. Unlike that path this never creates the index; counting
        // is a read-only observer and the matching loop owns index creation.
        match ft_search_count(connection_manager, index_name.clone(), query.clone()).await {
            Err(err) if is_retryable_redis_error(&err) => {
                let (connection_manager, _connect_id) =
                    self.connection_manager.reconnect(connect_id).await?;
                ft_search_count(connection_manager, index_name.clone(), query)
                    .await
                    .map_err(Error::from)
                    .err_tip(|| {
                        format!(
                            "Error with reconnected ft_search_count in RedisStore::count_by_index_prefix({index_name})"
                        )
                    })
            }
            result => result.map_err(Error::from).err_tip(|| {
                format!(
                    "Error with ft_search_count in RedisStore::count_by_index_prefix({index_name})"
                )
            }),
        }
    }

    async fn get_and_decode<K>(
        &self,
        key: K,
    ) -> Result<Option<<K as SchedulerStoreDecodeTo>::DecodeOutput>, Error>
    where
        K: SchedulerStoreKeyProvider + SchedulerStoreDecodeTo + Send,
    {
        let key = key.get_key();
        let key = self.encode_key(&key);
        let mut client = self.get_client().await?;
        // hmget is idempotent, so re-resolve the master and retry on a transient
        // failover (matching the read paths in get_part/list) instead of failing
        // a scheduler-state read while the master is moving.
        let results: Vec<Value> = {
            let mut attempt: u32 = 0;
            loop {
                attempt += 1;
                match client
                    .connection_manager
                    .hmget::<_, Vec<String>, Vec<Value>>(
                        key.as_ref(),
                        vec![VERSION_FIELD_NAME.into(), DATA_FIELD_NAME.into()],
                    )
                    .await
                {
                    Ok(v) => break v,
                    Err(err)
                        if attempt < MAX_REDIS_RETRY_ATTEMPTS && is_retryable_redis_error(&err) =>
                    {
                        client.reconnect(&self.connection_manager).await?;
                        sleep(Duration::from_secs_f32(DEFAULT_RETRY_DELAY)).await;
                    }
                    Err(err) => {
                        return Err(Error::from(err).append(format!(
                            "In RedisStore::get_without_version::notversioned {key}"
                        )));
                    }
                }
            }
        };
        let Some(Value::BulkString(data)) = results.get(1) else {
            return Ok(None);
        };
        #[allow(clippy::get_first)]
        let version = if let Some(raw_v) = results.get(0) {
            match raw_v {
                Value::Int(v) => *v,
                Value::BulkString(v) => i64::from_str(str::from_utf8(v).expect("utf-8 bulkstring"))
                    .expect("integer bulkstring"),
                Value::Nil => 0,
                _ => {
                    warn!(?raw_v, "Non-integer version!");
                    0
                }
            }
        } else {
            0
        };
        Ok(Some(
            K::decode(version, Bytes::from(data.clone())).err_tip(|| {
                format!("In RedisStore::get_with_version::notversioned::decode {key}")
            })?,
        ))
    }
}

#[cfg(test)]
// This module hand-rolls a minimal RESP3 server plus a dedicated
// current-thread runtime so the psubscribe/reconnect interleaving is exact and
// timing-independent; that legitimately needs the otherwise-disallowed tokio
// runtime/spawn primitives.
#[expect(
    clippy::disallowed_methods,
    reason = "deterministic in-test RESP server needs a hand-built runtime and raw spawns"
)]
#[expect(
    clippy::doc_markdown,
    reason = "choreography docs reference bare Redis/RESP identifiers (PSUBSCRIBE, Sha1) by name"
)]
#[expect(
    trivial_casts,
    reason = "explicit Pin<Box<dyn Future>> coercion for the connect closure's return type"
)]
mod psubscribe_reconnect_race_tests {
    use core::time::Duration;
    use std::collections::HashSet;
    use std::sync::Arc;

    use redis::aio::{ConnectionManager, ConnectionManagerConfig};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::sync::Notify;

    use super::{RedisManager, StandardRedisManager};

    /// Reads one CRLF-terminated line from the stream.
    async fn read_line(stream: &mut TcpStream) -> Option<String> {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            if stream.read_exact(&mut byte).await.is_err() {
                return None;
            }
            if byte[0] == b'\n' {
                break;
            }
            if byte[0] != b'\r' {
                line.push(byte[0]);
            }
        }
        Some(String::from_utf8_lossy(&line).into_owned())
    }

    /// Parses one RESP command (array of bulk strings) from the client.
    async fn parse_command(stream: &mut TcpStream) -> Option<Vec<String>> {
        let header = read_line(stream).await?;
        if !header.starts_with('*') {
            return Some(vec![header]);
        }
        let count: usize = header[1..].parse().ok()?;
        let mut parts = Vec::with_capacity(count);
        for _ in 0..count {
            let len_line = read_line(stream).await?;
            let len: usize = len_line.strip_prefix('$')?.parse().ok()?;
            let mut data = vec![0u8; len + 2]; // include trailing CRLF
            stream.read_exact(&mut data).await.ok()?;
            parts.push(String::from_utf8_lossy(&data[..len]).into_owned());
        }
        Some(parts)
    }

    struct FakeServerState {
        /// Sha1 to return for SCRIPT LOAD (must match the client's own hash).
        script_hash: String,
        /// Patterns each connection (by accept order) was PSUBSCRIBEd to.
        patterns_per_conn: parking_lot::Mutex<Vec<Vec<String>>>,
        /// Signalled when conn #1 (the reconnect connection) receives its
        /// first PSUBSCRIBE (the replay of the pre-existing pattern).
        replay_psubscribe_arrived: Notify,
        /// The server holds conn #1's first PSUBSCRIBE reply until this is
        /// notified, keeping reconnect() parked inside its replay loop.
        release_replay_reply: Notify,
    }

    async fn handle_conn(mut stream: TcpStream, conn_idx: usize, state: Arc<FakeServerState>) {
        let mut first_psubscribe_on_this_conn = true;
        while let Some(cmd) = parse_command(&mut stream).await {
            let name = cmd
                .first()
                .map(|c| c.to_ascii_uppercase())
                .unwrap_or_default();
            match name.as_str() {
                "HELLO" => {
                    stream
                        .write_all(
                            b"%3\r\n$6\r\nserver\r\n$5\r\nredis\r\n$7\r\nversion\r\n$5\r\n7.4.0\r\n$5\r\nproto\r\n:3\r\n",
                        )
                        .await
                        .unwrap();
                }
                "SCRIPT" => {
                    let reply =
                        format!("${}\r\n{}\r\n", state.script_hash.len(), state.script_hash);
                    stream.write_all(reply.as_bytes()).await.unwrap();
                }
                "PSUBSCRIBE" => {
                    let pattern = cmd.get(1).cloned().unwrap_or_default();
                    state.patterns_per_conn.lock()[conn_idx].push(pattern.clone());
                    if conn_idx == 1 && first_psubscribe_on_this_conn {
                        first_psubscribe_on_this_conn = false;
                        // Tell the test that reconnect() is now parked awaiting
                        // this reply inside its replay loop, then hold the
                        // reply until the test releases it.
                        state.replay_psubscribe_arrived.notify_one();
                        state.release_replay_reply.notified().await;
                    }
                    let reply = format!(
                        ">3\r\n$10\r\npsubscribe\r\n${}\r\n{}\r\n:1\r\n",
                        pattern.len(),
                        pattern
                    );
                    stream.write_all(reply.as_bytes()).await.unwrap();
                }
                _ => {
                    stream.write_all(b"+OK\r\n").await.unwrap();
                }
            }
        }
    }

    /// Deterministic repro: a `psubscribe` that interleaves with a `reconnect`
    /// whose subscription snapshot was taken before the insert lands only on
    /// the old (about-to-be-replaced) connection. The new live connection is
    /// never subscribed, yet the pattern is recorded as active in
    /// `self.subscriptions`, so notifications are silently lost until an
    /// unrelated future reconnect replays the set.
    ///
    /// Choreography (all real await points, no timing races):
    ///   1. Manager connects (conn #0) and psubscribes "pre".
    ///   2. Task B calls reconnect(): connects conn #1, snapshots
    ///      subscriptions = {"pre"}, and parks in the replay loop because the
    ///      fake server withholds conn #1's PSUBSCRIBE "pre" reply.
    ///   3. The test (task A) calls psubscribe("pat"): get_connection still
    ///      returns conn #0 (reconnect has not published yet), the pattern is
    ///      inserted into `subscriptions`, and PSUBSCRIBE succeeds on conn #0.
    ///   4. The server releases the reply; reconnect publishes conn #1.
    ///   5. Conn #1 — now the live connection — was never subscribed to "pat".
    #[test]
    fn psubscribe_lost_across_reconnect() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let state = Arc::new(FakeServerState {
                script_hash: redis::Script::new(super::LUA_VERSION_SET_SCRIPT)
                    .get_hash()
                    .to_string(),
                patterns_per_conn: parking_lot::Mutex::new(Vec::new()),
                replay_psubscribe_arrived: Notify::new(),
                release_replay_reply: Notify::new(),
            });

            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            {
                let state = state.clone();
                tokio::spawn(async move {
                    let mut conn_idx = 0usize;
                    loop {
                        let (stream, _) = listener.accept().await.unwrap();
                        state.patterns_per_conn.lock().push(Vec::new());
                        tokio::spawn(handle_conn(stream, conn_idx, state.clone()));
                        conn_idx += 1;
                    }
                });
            }

            // Keep the push receiver alive for the whole test.
            let (push_tx, _push_rx) = tokio::sync::mpsc::unbounded_channel();
            let url = format!("redis://127.0.0.1:{port}/?protocol=resp3");
            let connect_func = Box::new(move || {
                let url = url.clone();
                let push_tx = push_tx.clone();
                Box::pin(async move {
                    let client = redis::Client::open(url.as_str()).unwrap();
                    let config = ConnectionManagerConfig::new()
                        .set_number_of_retries(0)
                        .set_connection_timeout(Some(Duration::from_secs(5)))
                        .set_response_timeout(Some(Duration::from_secs(5)))
                        .set_push_sender(push_tx);
                    ConnectionManager::new_with_config(client, config)
                        .await
                        .map_err(Into::into)
                }) as core::pin::Pin<Box<_>>
            });

            let manager = Arc::new(StandardRedisManager::new(connect_func).await.unwrap());

            // Step 1: a pre-existing subscription so reconnect's replay loop
            // has an await point we can park it on.
            manager.psubscribe("pre").await.unwrap();
            let (_conn, uuid0) = manager.get_connection().await.unwrap();

            // Step 2: start reconnect; it will connect conn #1, snapshot
            // subscriptions (= {"pre"}), and park replaying "pre".
            let reconnect_task = {
                let manager = manager.clone();
                tokio::spawn(async move { manager.reconnect(uuid0).await })
            };
            tokio::time::timeout(
                Duration::from_secs(5),
                state.replay_psubscribe_arrived.notified(),
            )
            .await
            .expect("reconnect never reached its subscription replay loop");

            // Step 3: concurrent psubscribe while reconnect is parked
            // post-snapshot, pre-publish. It succeeds — on the old connection.
            manager.psubscribe("pat").await.unwrap();

            // Step 4: let reconnect finish and publish conn #1.
            state.release_replay_reply.notify_one();
            let (_new_conn, new_uuid) = reconnect_task.await.unwrap().unwrap();
            assert_ne!(
                uuid0, new_uuid,
                "reconnect must have published a new connection"
            );

            // The manager believes "pat" is an active subscription...
            let recorded: HashSet<String> = manager.subscriptions.lock().clone();
            assert!(
                recorded.contains("pat"),
                "sanity: pattern recorded as active"
            );

            // ...but the live connection (conn #1) was never subscribed to it.
            let conn1_patterns = state.patterns_per_conn.lock()[1].clone();
            assert!(
                conn1_patterns.contains(&"pat".to_string()),
                "RACE REPRODUCED: pattern \"pat\" is recorded as an active \
                 subscription but the live post-reconnect connection was only \
                 subscribed to {conn1_patterns:?}; keyspace notifications for \
                 \"pat\" are silently lost until the next reconnect",
            );
        });
    }
}
