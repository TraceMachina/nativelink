// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use core::ops::{Bound, RangeBounds};
use core::pin::Pin;
use core::time::Duration;
use core::{cmp, iter};
use std::borrow::Cow;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use bytes::Bytes;
use const_format::formatcp;
use fred::clients::SubscriberClient;
use fred::interfaces::{ClientLike, KeysInterface, PubsubInterface};
use fred::prelude::{Client, EventInterface, HashesInterface, RediSearchInterface};
use fred::types::config::{
    Config as RedisConfig, ConnectionConfig, PerformanceConfig, ReconnectPolicy, UnresponsiveConfig,
};
use fred::types::redisearch::{
    AggregateOperation, FtAggregateOptions, FtCreateOptions, IndexKind, Load, SearchField,
    SearchSchema, SearchSchemaKind, WithCursor,
};
use fred::types::scan::Scanner;
use fred::types::scripts::Script;
use fred::types::{Builder, Key as RedisKey, Map as RedisMap, SortOrder, Value as RedisValue};
use futures::stream::FuturesUnordered;
use futures::{FutureExt, Stream, StreamExt, TryStreamExt, future};
use itertools::izip;
use nativelink_config::stores::{RedisMode, RedisSpec};
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::spawn;
use nativelink_util::store_trait::{
    BoolValue, RemoveItemCallback, SchedulerCurrentVersionProvider, SchedulerIndexProvider,
    SchedulerStore, SchedulerStoreDataProvider, SchedulerStoreDecodeTo, SchedulerStoreKeyProvider,
    SchedulerSubscription, SchedulerSubscriptionManager, StoreDriver, StoreKey, UploadSizeInfo,
};
use nativelink_util::task::JoinHandleDropGuard;
use parking_lot::{Mutex, RwLock};
use patricia_tree::StringPatriciaMap;
use tokio::select;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::sleep;
use tracing::{error, info, trace, warn};
use uuid::Uuid;

use crate::cas_utils::is_zero_digest;
use crate::redis_utils::ft_aggregate;

/// The default size of the read chunk when reading data from Redis.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_READ_CHUNK_SIZE: usize = 64 * 1024;

/// The default size of the connection pool if not specified.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_CONNECTION_POOL_SIZE: usize = 3;

/// The default delay between retries if not specified.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_RETRY_DELAY: f32 = 0.1;
/// The amount of jitter to add to the retry delay if not specified.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_RETRY_JITTER: f32 = 0.5;

/// The default maximum capacity of the broadcast channel if not specified.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_BROADCAST_CHANNEL_CAPACITY: usize = 4096;

/// The default connection timeout in milliseconds if not specified.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_CONNECTION_TIMEOUT_MS: u64 = 3000;

/// The default command timeout in milliseconds if not specified.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 10_000;

/// The default maximum number of chunk uploads per update.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_MAX_CHUNK_UPLOADS_PER_UPDATE: usize = 10;

/// The default COUNT value passed when scanning keys in Redis.
/// Note: If this changes it should be updated in the config documentation.
const DEFAULT_SCAN_COUNT: u32 = 10_000;

const DEFAULT_CLIENT_PERMITS: usize = 500;

#[derive(Clone, Debug)]
pub struct RecoverablePool {
    clients: Arc<RwLock<Vec<Client>>>,
    builder: Builder,
    counter: Arc<core::sync::atomic::AtomicUsize>,
}

impl RecoverablePool {
    pub fn new(builder: Builder, size: usize) -> Result<Self, Error> {
        let mut clients = Vec::with_capacity(size);
        for _ in 0..size {
            let client = builder
                .build()
                .err_tip(|| "Failed to build client in RecoverablePool::new")?;
            clients.push(client);
        }
        Ok(Self {
            clients: Arc::new(RwLock::new(clients)),
            builder,
            counter: Arc::new(core::sync::atomic::AtomicUsize::new(0)),
        })
    }

    fn connect(&self) {
        let clients = self.clients.read();
        for client in clients.iter() {
            client.connect();
        }
    }

    fn next(&self) -> Client {
        let clients = self.clients.read();
        let index = self
            .counter
            .fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        clients[index % clients.len()].clone()
    }

    async fn replace_client(&self, old_client: &Client) -> Result<Client, Error> {
        {
            let clients = self.clients.read();
            if !clients.iter().any(|c| c.id() == old_client.id()) {
                // Someone else swapped this client already; just hand out the next pooled one.
                return Ok(self.next());
            }
        }

        let new_client = self
            .builder
            .build()
            .err_tip(|| "Failed to build new client in RecoverablePool::replace_client")?;
        new_client.connect();
        new_client.wait_for_connect().await.err_tip(|| {
            format!(
                "Failed to connect new client while replacing Redis client {}",
                old_client.id()
            )
        })?;

        let replaced_client = {
            let mut clients = self.clients.write();
            clients
                .iter()
                .position(|c| c.id() == old_client.id())
                .map(|index| core::mem::replace(&mut clients[index], new_client.clone()))
        };

        if let Some(old_client) = replaced_client {
            let _unused = old_client.quit().await;
            info!("Replaced Redis client {}", old_client.id());
            Ok(new_client)
        } else {
            // Second race: pool entry changed after we connected the new client.
            let _unused = new_client.quit().await;
            Ok(self.next())
        }
    }
}

/// A [`StoreDriver`] implementation that uses Redis as a backing store.
#[derive(Debug, MetricsComponent)]
pub struct RedisStore {
    /// The client pool connecting to the backing Redis instance(s).
    client_pool: RecoverablePool,

    /// A channel to publish updates to when a key is added, removed, or modified.
    #[metric(
        help = "The pubsub channel to publish updates to when a key is added, removed, or modified"
    )]
    pub_sub_channel: Option<String>,

    /// A redis client for managing subscriptions.
    /// TODO: This should be moved into the store in followups once a standard use pattern has been determined.
    subscriber_client: SubscriberClient,

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
    #[metric(help = "The maximum number of chunk uploads per update")]
    max_chunk_uploads_per_update: usize,

    /// The COUNT value passed when scanning keys in Redis.
    /// This is used to hint the amount of work that should be done per response.
    #[metric(help = "The COUNT value passed when scanning keys in Redis")]
    scan_count: u32,

    /// Redis script used to update a value in redis if the version matches.
    /// This is done by incrementing the version number and then setting the new data
    /// only if the version number matches the existing version number.
    update_if_version_matches_script: Script,

    /// A manager for subscriptions to keys in Redis.
    subscription_manager: Mutex<Option<Arc<RedisSubscriptionManager>>>,

    /// Permits to limit inflight Redis requests. Technically only
    /// limits the calls to `get_client()`, but the requests per client
    /// are small enough that it works well enough.
    client_permits: Arc<Semaphore>,
}

struct ClientWithPermit {
    client: Client,

    // here so it sticks around with the client and doesn't get dropped until that does
    #[allow(dead_code)]
    semaphore_permit: OwnedSemaphorePermit,
}

impl Drop for ClientWithPermit {
    fn drop(&mut self) {
        trace!(
            remaining = self.semaphore_permit.semaphore().available_permits(),
            "Dropping a client permit"
        );
    }
}

impl RedisStore {
    /// Create a new `RedisStore` from the given configuration.
    pub fn new(mut spec: RedisSpec) -> Result<Arc<Self>, Error> {
        if spec.addresses.is_empty() {
            return Err(make_err!(
                Code::InvalidArgument,
                "No addresses were specified in redis store configuration."
            ));
        }
        let [addr] = spec.addresses.as_slice() else {
            return Err(make_err!(
                Code::Unimplemented,
                "Connecting directly to multiple redis nodes in a cluster is currently unsupported. Please specify a single URL to a single node, and nativelink will use cluster discover to find the other nodes."
            ));
        };
        let redis_config = match spec.mode {
            RedisMode::Cluster => RedisConfig::from_url_clustered(addr),
            RedisMode::Sentinel => RedisConfig::from_url_sentinel(addr),
            RedisMode::Standard => RedisConfig::from_url_centralized(addr),
        }
        .err_tip_with_code(|e| {
            (
                Code::InvalidArgument,
                format!("while parsing redis node address: {e}"),
            )
        })?;

        let reconnect_policy = {
            if spec.retry.delay == 0.0 {
                spec.retry.delay = DEFAULT_RETRY_DELAY;
            }
            if spec.retry.jitter == 0.0 {
                spec.retry.jitter = DEFAULT_RETRY_JITTER;
            }

            let to_ms = |secs: f32| -> u32 {
                Duration::from_secs_f32(secs)
                    .as_millis()
                    .try_into()
                    .unwrap_or(u32::MAX)
            };

            let max_retries = u32::try_from(spec.retry.max_retries)
                .err_tip(|| "max_retries could not be converted to u32 in RedisStore::new")?;

            let min_delay_ms = to_ms(spec.retry.delay);
            let max_delay_ms = 8000;
            let jitter = to_ms(spec.retry.jitter * spec.retry.delay);

            let mut reconnect_policy =
                ReconnectPolicy::new_exponential(max_retries, min_delay_ms, max_delay_ms, 2);
            reconnect_policy.set_jitter(jitter);
            reconnect_policy
        };

        {
            if spec.broadcast_channel_capacity == 0 {
                spec.broadcast_channel_capacity = DEFAULT_BROADCAST_CHANNEL_CAPACITY;
            }
            if spec.connection_timeout_ms == 0 {
                spec.connection_timeout_ms = DEFAULT_CONNECTION_TIMEOUT_MS;
            }
            if spec.command_timeout_ms == 0 {
                spec.command_timeout_ms = DEFAULT_COMMAND_TIMEOUT_MS;
            }
            if spec.connection_pool_size == 0 {
                spec.connection_pool_size = DEFAULT_CONNECTION_POOL_SIZE;
            }
            if spec.read_chunk_size == 0 {
                spec.read_chunk_size = DEFAULT_READ_CHUNK_SIZE;
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
        }
        let connection_timeout = Duration::from_millis(spec.connection_timeout_ms);
        let command_timeout = Duration::from_millis(spec.command_timeout_ms);

        let mut builder = Builder::from_config(redis_config);
        builder
            .set_performance_config(PerformanceConfig {
                default_command_timeout: command_timeout,
                broadcast_channel_capacity: spec.broadcast_channel_capacity,
                ..Default::default()
            })
            .set_connection_config(ConnectionConfig {
                connection_timeout,
                internal_command_timeout: command_timeout,
                unresponsive: UnresponsiveConfig {
                    max_timeout: Some(connection_timeout),
                    // This number needs to be less than the connection timeout.
                    // We use 4 as it is a good balance between not spamming the server
                    // and not waiting too long.
                    interval: connection_timeout / 4,
                },
                ..Default::default()
            })
            .set_policy(reconnect_policy);

        let client_pool = RecoverablePool::new(builder.clone(), spec.connection_pool_size)
            .err_tip(|| "while creating redis connection pool")?;

        let subscriber_client = builder
            .build_subscriber_client()
            .err_tip(|| "while creating redis subscriber client")?;

        Self::new_from_builder_and_parts(
            client_pool,
            subscriber_client,
            spec.experimental_pub_sub_channel.clone(),
            || Uuid::new_v4().to_string(),
            spec.key_prefix.clone(),
            spec.read_chunk_size,
            spec.max_chunk_uploads_per_update,
            spec.scan_count,
            spec.max_client_permits,
        )
        .map(Arc::new)
    }

    /// Used for testing when determinism is required.
    #[expect(clippy::too_many_arguments)]
    pub fn new_from_builder_and_parts(
        client_pool: RecoverablePool,
        subscriber_client: SubscriberClient,
        pub_sub_channel: Option<String>,
        temp_name_generator_fn: fn() -> String,
        key_prefix: String,
        read_chunk_size: usize,
        max_chunk_uploads_per_update: usize,
        scan_count: u32,
        max_client_permits: usize,
    ) -> Result<Self, Error> {
        // Start connection pool (this will retry forever by default).
        client_pool.connect();
        subscriber_client.connect();

        info!("Redis index fingerprint: {FINGERPRINT_CREATE_INDEX_HEX}");

        Ok(Self {
            client_pool,
            pub_sub_channel,
            subscriber_client,
            temp_name_generator_fn,
            key_prefix,
            read_chunk_size,
            max_chunk_uploads_per_update,
            scan_count,
            update_if_version_matches_script: Script::from_lua(LUA_VERSION_SET_SCRIPT),
            subscription_manager: Mutex::new(None),
            client_permits: Arc::new(Semaphore::new(max_client_permits)),
        })
    }

    async fn get_client(&self) -> Result<ClientWithPermit, Error> {
        let mut client = self.client_pool.next();
        loop {
            let config = client.client_config();
            if config.mocks.is_some() {
                break;
            }
            let connection_info = format!(
                "Connection issue connecting to redis server with hosts: {:?}, username: {}, database: {}",
                config
                    .server
                    .hosts()
                    .iter()
                    .map(|s| format!("{}:{}", s.host, s.port))
                    .collect::<Vec<String>>(),
                config
                    .username
                    .clone()
                    .unwrap_or_else(|| "None".to_string()),
                config.database.unwrap_or_default()
            );
            match client.wait_for_connect().await {
                Ok(()) => break,
                Err(e) => {
                    warn!("{connection_info}: {e:?}. Replacing client.");
                    client = self
                        .client_pool
                        .replace_client(&client)
                        .await
                        .err_tip(|| connection_info.clone())?;
                }
            }
        }
        let local_client_permits = self.client_permits.clone();
        let remaining = local_client_permits.available_permits();
        let semaphore_permit = local_client_permits.acquire_owned().await?;
        trace!(remaining, "Got a client permit");
        Ok(ClientWithPermit {
            client,
            semaphore_permit,
        })
    }

    /// Encode a [`StoreKey`] so it can be sent to Redis.
    fn encode_key<'a>(&self, key: &'a StoreKey<'a>) -> Cow<'a, str> {
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
}

#[async_trait]
impl StoreDriver for RedisStore {
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        // TODO(palfrey) We could use pipeline here, but it makes retry more
        // difficult and it doesn't work very well in cluster mode.
        // If we wanted to optimize this with pipeline be careful to
        // implement retry and to support cluster mode.

        let client = self.get_client().await?;

        // If we ask for many keys in one go, this can timeout, so limit that
        let max_in_one_go = Arc::new(Semaphore::const_new(5));

        izip!(
            keys.iter(),
            results.iter_mut(),
            iter::repeat(&max_in_one_go),
            iter::repeat(&client)
        )
        .map(|(key, result, local_semaphore, client)| async move {
            // We need to do a special pass to ensure our zero key exist.
            if is_zero_digest(key.borrow()) {
                *result = Some(0);
                return Ok::<_, Error>(());
            }
            let encoded_key = self.encode_key(key);

            let guard = local_semaphore.acquire().await?;

            let pipeline = client.client.pipeline();
            pipeline
                .strlen::<(), _>(encoded_key.as_ref())
                .await
                .err_tip(|| format!("In RedisStore::has_with_results::strlen for {encoded_key}"))?;
            // Redis returns 0 when the key doesn't exist
            // AND when the key exists with value of length 0.
            // Therefore, we need to check both length and existence
            // and do it in a pipeline for efficiency.
            pipeline
                .exists::<(), _>(encoded_key.as_ref())
                .await
                .err_tip(|| format!("In RedisStore::has_with_results::exists for {encoded_key}"))?;
            let (blob_len, exists) = pipeline
                .all::<(u64, bool)>()
                .await
                .err_tip(|| "In RedisStore::has_with_results::all")?;

            *result = if exists { Some(blob_len) } else { None };

            drop(guard);

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
        let client = self.get_client().await?;
        let mut scan_stream = client.client.scan(pattern, Some(self.scan_count), None);
        let mut iterations = 0;
        'outer: while let Some(mut page) = scan_stream.try_next().await? {
            if let Some(keys) = page.take_results() {
                for key in keys {
                    // TODO: Notification of conversion errors
                    // Any results that do not conform to expectations are ignored.
                    if let Some(key) = key.as_str() {
                        if let Some(key) = key.strip_prefix(&self.key_prefix) {
                            let key = StoreKey::new_str(key);
                            if range.contains(&key) {
                                iterations += 1;
                                if !handler(&key) {
                                    break 'outer;
                                }
                            }
                        }
                    }
                }
            }
            page.next();
        }
        Ok(iterations)
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        _upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        let final_key = self.encode_key(&key);

        // While the name generation function can be supplied by the user, we need to have the curly
        // braces in place in order to manage redis' hashing behavior and make sure that the temporary
        // key name and the final key name are directed to the same cluster node. See
        // https://redis.io/blog/redis-clustering-best-practices-with-keys/
        //
        // The TL;DR is that if we're in cluster mode and the names hash differently, we can't use request
        // pipelining. By using these braces, we tell redis to only hash the part of the temporary key that's
        // identical to the final key -- so they will always hash to the same node.
        let temp_key = format!(
            "temp-{}-{{{}}}",
            (self.temp_name_generator_fn)(),
            &final_key
        );

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
                return Ok(());
            }
        }

        let client = self.get_client().await?;

        let mut read_stream = reader
            .scan(0u32, |bytes_read, chunk_res| {
                future::ready(Some(
                    chunk_res
                        .err_tip(|| "Failed to read chunk in update in redis store")
                        .and_then(|chunk| {
                            let offset = *bytes_read;
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
                let temp_key_ref = &temp_key;
                let client = client.client.clone();
                Ok(async move {
                    client
                        .setrange::<(), _, _>(temp_key_ref, offset, chunk)
                        .await
                        .err_tip(
                            || format!("While appending to temp key ({temp_key_ref}) in RedisStore::update. offset = {offset}. end_pos = {end_pos}"),
                        )?;
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

        let blob_len = client
            .client
            .strlen::<u64, _>(&temp_key)
            .await
            .err_tip(|| format!("In RedisStore::update strlen check for {temp_key}"))?;
        // This is a safety check to ensure that in the event some kind of retry was to happen
        // and the data was appended to the key twice, we reject the data.
        if blob_len != u64::from(total_len) {
            return Err(make_input_err!(
                "Data length mismatch in RedisStore::update for {}({}) - expected {} bytes, got {} bytes",
                key.borrow().as_str(),
                temp_key,
                total_len,
                blob_len,
            ));
        }

        // Rename the temp key so that the data appears under the real key. Any data already present in the real key is lost.
        client
            .client
            .rename::<(), _, _>(&temp_key, final_key.as_ref())
            .await
            .err_tip(|| "While queueing key rename in RedisStore::update()")?;

        // If we have a publish channel configured, send a notice that the key has been set.
        if let Some(pub_sub_channel) = &self.pub_sub_channel {
            return Ok(client
                .client
                .publish(pub_sub_channel, final_key.as_ref())
                .await?);
        }

        Ok(())
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        let offset = usize::try_from(offset).err_tip(|| "Could not convert offset to usize")?;
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
            .saturating_add(length.unwrap_or(isize::MAX as usize))
            .saturating_sub(1);

        // And we don't ever want to read more than `read_chunk_size` bytes at a time, so we'll need to iterate.
        let mut chunk_start = data_start;
        let mut chunk_end = cmp::min(
            data_start.saturating_add(self.read_chunk_size) - 1,
            data_end,
        );

        let client = self.get_client().await?;
        loop {
            let chunk: Bytes = client
                .client
                .getrange(encoded_key, chunk_start, chunk_end)
                .await
                .err_tip(|| "In RedisStore::get_part::getrange")?;

            let didnt_receive_full_chunk = chunk.len() < self.read_chunk_size;
            let reached_end_of_data = chunk_end == data_end;

            if didnt_receive_full_chunk || reached_end_of_data {
                if !chunk.is_empty() {
                    writer
                        .send(chunk)
                        .await
                        .err_tip(|| "Failed to write data in RedisStore::get_part")?;
                }

                break; // No more data to read.
            }

            // We received a full chunk's worth of data, so write it...
            writer
                .send(chunk)
                .await
                .err_tip(|| "Failed to write data in RedisStore::get_part")?;

            // ...and go grab the next chunk.
            chunk_start = chunk_end + 1;
            chunk_end = cmp::min(
                chunk_start.saturating_add(self.read_chunk_size) - 1,
                data_end,
            );
        }

        // If we didn't write any data, check if the key exists, if not return a NotFound error.
        // This is required by spec.
        if writer.get_bytes_written() == 0 {
            // We're supposed to read 0 bytes, so just check if the key exists.
            let exists = client
                .client
                .exists::<bool, _>(encoded_key)
                .await
                .err_tip(|| "In RedisStore::get_part::zero_exists")?;

            if !exists {
                return Err(make_err!(
                    Code::NotFound,
                    "Data not found in Redis store for digest: {key:?}"
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

    fn register_remove_callback(
        self: Arc<Self>,
        _callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        // As redis doesn't drop stuff, we can just ignore this
        Ok(())
    }
}

#[async_trait]
impl HealthStatusIndicator for RedisStore {
    fn get_name(&self) -> &'static str {
        "RedisStore"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}

// -------------------------------------------------------------------
// Below this line are specific to the redis scheduler implementation.
// -------------------------------------------------------------------

/// The maximum number of results to return per cursor.
/// Increased from 256 to reduce round-trips for large task queues.
const MAX_COUNT_PER_CURSOR: u64 = 1500;
/// The time in milliseconds that a redis cursor can be idle before it is closed.
const CURSOR_IDLE_MS: u64 = 2_000;
/// The name of the field in the Redis hash that stores the data.
const DATA_FIELD_NAME: &str = "data";
/// The name of the field in the Redis hash that stores the version.
const VERSION_FIELD_NAME: &str = "version";
/// The time to live of indexes in seconds. After this time redis may delete the index.
const INDEX_TTL_S: u64 = 60 * 60 * 24; // 24 hours.

/// Lua script to set a key if the version matches.
/// Args:
///   KEYS[1]: The key where the version is stored.
///   ARGV[1]: The expected version.
///   ARGV[2]: The new data.
///   ARGV[3*]: Key-value pairs of additional data to include.
/// Returns:
///   The new version if the version matches. nil is returned if the
///   value was not set.
const LUA_VERSION_SET_SCRIPT: &str = formatcp!(
    r"
local key = KEYS[1]
local expected_version = tonumber(ARGV[1])
local new_data = ARGV[2]
local new_version = redis.call('HINCRBY', key, '{VERSION_FIELD_NAME}', 1)
local i
local indexes = {{}}

if new_version-1 ~= expected_version then
    redis.call('HINCRBY', key, '{VERSION_FIELD_NAME}', -1)
    return {{ 0, new_version-1 }}
end
-- Skip first 2 argvs, as they are known inputs.
-- Remember: Lua is 1-indexed.
for i=3, #ARGV do
    indexes[i-2] = ARGV[i]
end

-- In testing we witnessed redis sometimes not update our FT indexes
-- resulting in stale data. It appears if we delete our keys then insert
-- them again it works and reduces risk significantly.
redis.call('DEL', key)
redis.call('HSET', key, '{DATA_FIELD_NAME}', new_data, '{VERSION_FIELD_NAME}', new_version, unpack(indexes))

return {{ 1, new_version }}
"
);

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
const fn try_sanitize(s: &str) -> Option<&str> {
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
            return None;
        }
        i += 1;
    }
    Some(s)
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
        receiver
            .changed()
            .await
            .map_err(|_| make_err!(Code::Internal, "In RedisSubscription::changed::changed"))
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
        // IMPORTANT: This must be dropped before receiver_count() is called.
        drop(receiver);
        let Some(subscribed_keys) = self.weak_subscribed_keys.upgrade() else {
            return; // Already dropped, nothing to do.
        };
        let mut subscribed_keys = subscribed_keys.write();
        let Some(value) = subscribed_keys.get(&key) else {
            error!(
                "Key {key} was not found in subscribed keys when checking if it should be removed."
            );
            return;
        };
        // If we have no receivers, cleanup the entry from our map.
        if value.receiver_count() == 0 {
            subscribed_keys.remove(key);
        }
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

#[derive(Debug)]
pub struct RedisSubscriptionManager {
    subscribed_keys: Arc<RwLock<StringPatriciaMap<RedisSubscriptionPublisher>>>,
    tx_for_test: tokio::sync::mpsc::UnboundedSender<String>,
    _subscription_spawn: JoinHandleDropGuard<()>,
}

impl RedisSubscriptionManager {
    pub fn new(subscribe_client: SubscriberClient, pub_sub_channel: String) -> Self {
        let subscribed_keys = Arc::new(RwLock::new(StringPatriciaMap::new()));
        let subscribed_keys_weak = Arc::downgrade(&subscribed_keys);
        let (tx_for_test, mut rx_for_test) = tokio::sync::mpsc::unbounded_channel();
        Self {
            subscribed_keys,
            tx_for_test,
            _subscription_spawn: spawn!("redis_subscribe_spawn", async move {
                let mut rx = subscribe_client.message_rx();
                loop {
                    if let Err(e) = subscribe_client.subscribe(&pub_sub_channel).await {
                        error!("Error subscribing to pattern - {e}");
                        return;
                    }
                    let mut reconnect_rx = subscribe_client.reconnect_rx();
                    let reconnect_fut = reconnect_rx.recv().fuse();
                    tokio::pin!(reconnect_fut);
                    loop {
                        let key = select! {
                            value = rx_for_test.recv() => {
                                let Some(value) = value else {
                                    unreachable!("Channel should never close");
                                };
                                value.into()
                            },
                            msg = rx.recv() => {
                                match msg {
                                    Ok(msg) => {
                                        if let RedisValue::String(s) = msg.value {
                                            s
                                        } else {
                                            error!("Received non-string message in RedisSubscriptionManager");
                                            continue;
                                        }
                                    },
                                    Err(e) => {
                                        // Check to see if our parent has been dropped and if so kill spawn.
                                        if subscribed_keys_weak.upgrade().is_none() {
                                            warn!("It appears our parent has been dropped, exiting RedisSubscriptionManager spawn");
                                            return;
                                        }
                                        error!("Error receiving message in RedisSubscriptionManager reconnecting and flagging everything changed - {e}");
                                        break;
                                    }
                                }
                            },
                            _ = &mut reconnect_fut => {
                                warn!("Redis reconnected flagging all subscriptions as changed and resuming");
                                break;
                            }
                        };
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
                    rx = subscribe_client.message_rx();
                    // Drop all buffered messages, then flag everything as changed.
                    rx.resubscribe();
                    for publisher in subscribed_keys_mux.values() {
                        publisher.notify();
                    }
                }
            }),
        }
    }
}

impl SchedulerSubscriptionManager for RedisSubscriptionManager {
    type Subscription = RedisSubscription;

    fn notify_for_test(&self, value: String) {
        self.tx_for_test.send(value).unwrap();
    }

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

impl SchedulerStore for RedisStore {
    type SubscriptionManager = RedisSubscriptionManager;

    fn subscription_manager(&self) -> Result<Arc<RedisSubscriptionManager>, Error> {
        let mut subscription_manager = self.subscription_manager.lock();

        if let Some(subscription_manager) = &*subscription_manager {
            Ok(subscription_manager.clone())
        } else {
            let Some(pub_sub_channel) = &self.pub_sub_channel else {
                return Err(make_input_err!(
                    "RedisStore must have a pubsub channel for a Redis Scheduler if using subscriptions"
                ));
            };
            let sub = Arc::new(RedisSubscriptionManager::new(
                self.subscriber_client.clone(),
                pub_sub_channel.clone(),
            ));
            *subscription_manager = Some(sub.clone());
            Ok(sub)
        }
    }

    async fn update_data<T>(&self, data: T) -> Result<Option<i64>, Error>
    where
        T: SchedulerStoreDataProvider
            + SchedulerStoreKeyProvider
            + SchedulerCurrentVersionProvider
            + Send,
    {
        let key = data.get_key();
        let redis_key = self.encode_key(&key);
        let client = self.get_client().await?;
        let maybe_index = data.get_indexes().err_tip(|| {
            format!("Err getting index in RedisStore::update_data::versioned for {redis_key}")
        })?;
        if <T as SchedulerStoreKeyProvider>::Versioned::VALUE {
            let current_version = data.current_version();
            let data = data.try_into_bytes().err_tip(|| {
                format!("Could not convert value to bytes in RedisStore::update_data::versioned for {redis_key}")
            })?;
            let mut argv = Vec::with_capacity(3 + maybe_index.len() * 2);
            argv.push(Bytes::from(format!("{current_version}")));
            argv.push(data);
            for (name, value) in maybe_index {
                argv.push(Bytes::from_static(name.as_bytes()));
                argv.push(value);
            }
            let start = std::time::Instant::now();

            let (success, new_version): (bool, i64) = self
                .update_if_version_matches_script
                .evalsha_with_reload(&client.client, vec![redis_key.as_ref()], argv)
                .await
                .err_tip(|| format!("In RedisStore::update_data::versioned for {key:?}"))?;

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
                    .client
                    .publish(pub_sub_channel, redis_key.as_ref())
                    .await?);
            }
            Ok(Some(new_version))
        } else {
            let data = data.try_into_bytes().err_tip(|| {
                format!("Could not convert value to bytes in RedisStore::update_data::noversion for {redis_key}")
            })?;
            let mut fields = RedisMap::new();
            fields.reserve(1 + maybe_index.len());
            fields.insert(DATA_FIELD_NAME.into(), data.into());
            for (name, value) in maybe_index {
                fields.insert(name.into(), value.into());
            }
            client
                .client
                .hset::<(), _, _>(redis_key.as_ref(), fields)
                .await
                .err_tip(|| format!("In RedisStore::update_data::noversion for {redis_key}"))?;
            // If we have a publish channel configured, send a notice that the key has been set.
            if let Some(pub_sub_channel) = &self.pub_sub_channel {
                return Ok(client
                    .client
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
    {
        let index_value = index.index_value();
        let sanitized_field = try_sanitize(index_value.as_ref())
            .err_tip(|| {
                format!("In RedisStore::search_by_index_prefix::try_sanitize - {index_value:?}")
            })?
            .to_string();
        let index_name = format!(
            "{}",
            get_index_name!(K::KEY_PREFIX, K::INDEX_NAME, K::MAYBE_SORT_KEY)
        );

        let run_ft_aggregate = |client: Arc<ClientWithPermit>,
                                index_name: String,
                                sanitized_field: String| async move {
            ft_aggregate(
                client.client.clone(),
                index_name,
                if sanitized_field.is_empty() {
                    "*".to_string()
                } else {
                    format!("@{}:{{ {} }}", K::INDEX_NAME, sanitized_field)
                },
                FtAggregateOptions {
                    load: Some(Load::Some(vec![
                        SearchField {
                            identifier: DATA_FIELD_NAME.into(),
                            property: None,
                        },
                        SearchField {
                            identifier: VERSION_FIELD_NAME.into(),
                            property: None,
                        },
                    ])),
                    cursor: Some(WithCursor {
                        count: Some(MAX_COUNT_PER_CURSOR),
                        max_idle: Some(CURSOR_IDLE_MS),
                    }),
                    pipeline: vec![AggregateOperation::SortBy {
                        properties: K::MAYBE_SORT_KEY.map_or_else(Vec::new, |v| {
                            vec![(format!("@{v}").into(), SortOrder::Asc)]
                        }),
                        max: None,
                    }],
                    ..Default::default()
                },
            )
            .await
            .map(|stream| (stream, client))
        };

        let client = Arc::new(self.get_client().await?);
        let (stream, client_guard) = if let Ok(result) =
            run_ft_aggregate(client.clone(), index_name.clone(), sanitized_field.clone()).await
        {
            result
        } else {
            let mut schema = vec![SearchSchema {
                field_name: K::INDEX_NAME.into(),
                alias: None,
                kind: SearchSchemaKind::Tag {
                    sortable: false,
                    unf: false,
                    separator: None,
                    casesensitive: false,
                    withsuffixtrie: false,
                    noindex: false,
                },
            }];
            if let Some(sort_key) = K::MAYBE_SORT_KEY {
                schema.push(SearchSchema {
                    field_name: sort_key.into(),
                    alias: None,
                    kind: SearchSchemaKind::Tag {
                        sortable: true,
                        unf: false,
                        separator: None,
                        casesensitive: false,
                        withsuffixtrie: false,
                        noindex: false,
                    },
                });
            }
            let create_result: Result<(), Error> = {
                let create_client = self.get_client().await?;
                create_client
                    .client
                    .ft_create::<(), _>(
                        index_name.clone(),
                        FtCreateOptions {
                            on: Some(IndexKind::Hash),
                            prefixes: vec![K::KEY_PREFIX.into()],
                            nohl: true,
                            nofields: true,
                            nofreqs: true,
                            nooffsets: true,
                            temporary: Some(INDEX_TTL_S),
                            ..Default::default()
                        },
                        schema,
                    )
                    .await
                    .err_tip(|| {
                        format!(
                            "Error with ft_create in RedisStore::search_by_index_prefix({})",
                            get_index_name!(K::KEY_PREFIX, K::INDEX_NAME, K::MAYBE_SORT_KEY),
                        )
                    })?;
                Ok(())
            };
            let retry_client = Arc::new(self.get_client().await?);
            let retry_result =
                run_ft_aggregate(retry_client, index_name.clone(), sanitized_field.clone()).await;
            if let Ok(result) = retry_result {
                result
            } else {
                let e: Error = retry_result
                    .err()
                    .expect("Checked for Ok result above")
                    .into();
                let err = match create_result {
                    Ok(()) => e,
                    Err(create_err) => create_err.merge(e),
                };
                return Err(err);
            }
        };

        Ok(stream.map(move |result| {
            let keep_alive = client_guard.clone();
            let _ = &keep_alive;
            let mut redis_map =
                result.err_tip(|| "Error in stream of in RedisStore::search_by_index_prefix")?;
            let bytes_data = redis_map
                .remove(&RedisKey::from_static_str(DATA_FIELD_NAME))
                .err_tip(|| "Missing data field in RedisStore::search_by_index_prefix")?
                .into_bytes()
                .err_tip(|| {
                    formatcp!("'{DATA_FIELD_NAME}' is not Bytes in RedisStore::search_by_index_prefix::into_bytes")
                })?;
            let version = if <K as SchedulerIndexProvider>::Versioned::VALUE {
                redis_map
                    .remove(&RedisKey::from_static_str(VERSION_FIELD_NAME))
                    .err_tip(|| "Missing version field in RedisStore::search_by_index_prefix")?
                    .as_i64()
                    .err_tip(|| {
                        formatcp!("'{VERSION_FIELD_NAME}' is not u64 in RedisStore::search_by_index_prefix::as_u64")
                    })?
            } else {
                0
            };
            K::decode(version, bytes_data)
                .err_tip(|| "In RedisStore::search_by_index_prefix::decode")
        }))
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
        let client = self.get_client().await?;
        let (maybe_version, maybe_data) = client
            .client
            .hmget::<(Option<i64>, Option<Bytes>), _, _>(
                key.as_ref(),
                vec![
                    RedisKey::from(VERSION_FIELD_NAME),
                    RedisKey::from(DATA_FIELD_NAME),
                ],
            )
            .await
            .err_tip(|| format!("In RedisStore::get_without_version::notversioned {key}"))?;
        let Some(data) = maybe_data else {
            return Ok(None);
        };
        Ok(Some(K::decode(maybe_version.unwrap_or(0), data).err_tip(
            || format!("In RedisStore::get_with_version::notversioned::decode {key}"),
        )?))
    }
}
