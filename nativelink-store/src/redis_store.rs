// Copyright 2024-2025 The NativeLink Authors. All rights reserved.
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
use core::ops::{Bound, RangeBounds};
use core::pin::Pin;
use core::time::Duration;
use std::borrow::Cow;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use bytes::Bytes;
use const_format::formatcp;
use futures::stream::{self, FuturesUnordered};
use futures::{Stream, StreamExt, TryFutureExt, TryStreamExt, future};
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
use redis::aio::{ConnectionLike, ConnectionManager, ConnectionManagerConfig};
use redis::cluster::ClusterClient;
use redis::cluster_async::ClusterConnection;
use redis::sentinel::{SentinelClient, SentinelServerType};
use redis::{
    AsyncCommands, AsyncIter, Client, PushInfo, RedisResult, ScanOptions, Script, Value, pipe,
};
use redis_test::MockRedisConnection;
use tokio::select;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::sleep;
use tracing::{error, info, trace, warn};
use uuid::Uuid;

use crate::cas_utils::is_zero_digest;
use crate::redis_utils::{
    FtAggregateCursor, FtAggregateOptions, FtCreateOptions, SearchSchema, ft_aggregate, ft_create,
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
const DEFAULT_SCAN_COUNT: usize = 10_000;

const DEFAULT_CLIENT_PERMITS: usize = 500;

/// A [`StoreDriver`] implementation that uses Redis as a backing store.
#[derive(MetricsComponent)]
pub struct RedisStore<C>
where
    C: ConnectionLike + Clone,
{
    /// The client pool connecting to the backing Redis instance(s).
    connection_manager: C,

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
    #[metric(help = "The maximum number of chunk uploads per update")]
    max_chunk_uploads_per_update: usize,

    /// The COUNT value passed when scanning keys in Redis.
    /// This is used to hint the amount of work that should be done per response.
    #[metric(help = "The COUNT value passed when scanning keys in Redis")]
    scan_count: usize,

    /// Redis script used to update a value in redis if the version matches.
    /// This is done by incrementing the version number and then setting the new data
    /// only if the version number matches the existing version number.
    update_if_version_matches_script: Script,

    /// A manager for subscriptions to keys in Redis.
    subscription_manager: Mutex<Option<Arc<RedisSubscriptionManager>>>,

    /// Channel for getting subscription messages. Only used by cluster mode where
    /// the sender is connected at construction time. For standard mode, this is
    /// None and created on demand in `subscription_manager()`.
    subscriber_channel: Mutex<Option<UnboundedReceiver<PushInfo>>>,

    /// Permits to limit inflight Redis requests. Technically only
    /// limits the calls to `get_client()`, but the requests per client
    /// are small enough that it works well enough.
    client_permits: Arc<Semaphore>,
}

impl<C: ConnectionLike + Clone> Debug for RedisStore<C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RedisStore")
            .field("pub_sub_channel", &self.pub_sub_channel)
            .field("temp_name_generator_fn", &self.temp_name_generator_fn)
            .field("key_prefix", &self.key_prefix)
            .field("read_chunk_size", &self.read_chunk_size)
            .field(
                "max_chunk_uploads_per_update",
                &self.max_chunk_uploads_per_update,
            )
            .field("scan_count", &self.scan_count)
            .field(
                "update_if_version_matches_script",
                &self.update_if_version_matches_script,
            )
            .field("subscription_manager", &self.subscription_manager)
            .field("subscriber_channel", &self.subscriber_channel)
            .field("client_permits", &self.client_permits)
            .finish()
    }
}

struct ClientWithPermit<C: ConnectionLike> {
    connection_manager: C,

    // here so it sticks around with the client and doesn't get dropped until that does
    #[allow(dead_code)]
    semaphore_permit: OwnedSemaphorePermit,
}

impl<C: ConnectionLike> Drop for ClientWithPermit<C> {
    fn drop(&mut self) {
        trace!(
            remaining = self.semaphore_permit.semaphore().available_permits(),
            "Dropping a client permit"
        );
    }
}

impl<C: ConnectionLike + Clone + Sync> RedisStore<C> {
    /// Used for testing when determinism is required.
    #[expect(clippy::too_many_arguments)]
    pub async fn new_from_builder_and_parts(
        mut connection_manager: C,
        pub_sub_channel: Option<String>,
        temp_name_generator_fn: fn() -> String,
        key_prefix: String,
        read_chunk_size: usize,
        max_chunk_uploads_per_update: usize,
        scan_count: usize,
        max_client_permits: usize,
        subscriber_channel: Option<UnboundedReceiver<PushInfo>>,
    ) -> Result<Self, Error> {
        info!("Redis index fingerprint: {FINGERPRINT_CREATE_INDEX_HEX}");

        let version_set_script = Script::new(LUA_VERSION_SET_SCRIPT);
        version_set_script
            .load_async(&mut connection_manager)
            .await?;

        Ok(Self {
            connection_manager,
            pub_sub_channel,
            temp_name_generator_fn,
            key_prefix,
            read_chunk_size,
            max_chunk_uploads_per_update,
            scan_count,
            update_if_version_matches_script: version_set_script,
            subscription_manager: Mutex::new(None),
            subscriber_channel: Mutex::new(subscriber_channel),
            client_permits: Arc::new(Semaphore::new(max_client_permits)),
        })
    }

    async fn get_client(&self) -> Result<ClientWithPermit<C>, Error> {
        let local_client_permits = self.client_permits.clone();
        let remaining = local_client_permits.available_permits();
        let semaphore_permit = local_client_permits.acquire_owned().await?;
        trace!(remaining, "Got a client permit");
        Ok(ClientWithPermit {
            connection_manager: self.connection_manager.clone(),
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
        if spec.retry.delay == 0.0 {
            spec.retry.delay = DEFAULT_RETRY_DELAY;
        }
        if spec.retry.max_retries == 0 {
            spec.retry.max_retries = 1;
        }
        trace!(?spec, "redis spec is after setting defaults");
        Ok(())
    }
}

impl RedisStore<ClusterConnection> {
    pub async fn new_cluster(mut spec: RedisSpec) -> Result<Arc<Self>, Error> {
        if spec.mode != RedisMode::Cluster {
            return Err(Error::new(
                Code::InvalidArgument,
                "new_cluster only works for Cluster mode".to_string(),
            ));
        }
        Self::set_spec_defaults(&mut spec)?;

        let full_urls: Vec<_> = spec
            .addresses
            .iter_mut()
            .map(|addr| format!("{addr}?protocol=resp3"))
            .collect();

        let connection_timeout = Duration::from_millis(spec.connection_timeout_ms);
        let command_timeout = Duration::from_millis(spec.command_timeout_ms);
        let (tx, subscriber_channel) = unbounded_channel();

        let builder = ClusterClient::builder(full_urls)
            .connection_timeout(connection_timeout)
            .response_timeout(command_timeout)
            .push_sender(tx)
            .retries(u32::try_from(spec.retry.max_retries)?);

        let client = builder.build()?;

        Self::new_from_builder_and_parts(
            client.get_async_connection().await?,
            spec.experimental_pub_sub_channel.clone(),
            || Uuid::new_v4().to_string(),
            spec.key_prefix.clone(),
            spec.read_chunk_size,
            spec.max_chunk_uploads_per_update,
            spec.scan_count,
            spec.max_client_permits,
            Some(subscriber_channel),
        )
        .await
        .map(Arc::new)
    }
}

impl RedisStore<ConnectionManager> {
    /// Create a new `RedisStore` from the given configuration.
    pub async fn new_standard(mut spec: RedisSpec) -> Result<Arc<Self>, Error> {
        Self::set_spec_defaults(&mut spec)?;

        let addr = spec.addresses.remove(0);
        if !spec.addresses.is_empty() {
            return Err(make_err!(
                Code::Unimplemented,
                "Connecting directly to multiple redis nodes in a cluster is currently unsupported. Please specify a single URL to a single node, and nativelink will use cluster discover to find the other nodes."
            ));
        }

        let client = match spec.mode {
            RedisMode::Standard => Client::open(addr.clone()),
            RedisMode::Cluster => {
                return Err(Error::new(
                    Code::Internal,
                    "Use RedisStore::new_cluster for cluster connections".to_owned(),
                ));
            }
            RedisMode::Sentinel => SentinelClient::build(
                vec![addr.clone()],
                "master".to_string(),
                None,
                SentinelServerType::Master,
            )
            .and_then(|mut s| s.get_client()),
        }
        .err_tip_with_code(|_e| {
            (
                Code::InvalidArgument,
                format!("while connecting to redis with url: {addr}"),
            )
        })?;

        let connection_timeout = Duration::from_millis(spec.connection_timeout_ms);
        let command_timeout = Duration::from_millis(spec.command_timeout_ms);

        let connection_manager_config = {
            ConnectionManagerConfig::new()
                .set_number_of_retries(spec.retry.max_retries)
                .set_connection_timeout(connection_timeout)
                .set_response_timeout(command_timeout)
        };

        let connection_manager: ConnectionManager =
            ConnectionManager::new_with_config(client, connection_manager_config)
                .await
                .err_tip(|| format!("While connecting to {addr}"))?;

        Self::new_from_builder_and_parts(
            connection_manager,
            spec.experimental_pub_sub_channel.clone(),
            || Uuid::new_v4().to_string(),
            spec.key_prefix.clone(),
            spec.read_chunk_size,
            spec.max_chunk_uploads_per_update,
            spec.scan_count,
            spec.max_client_permits,
            None, // Standard mode creates subscription channel on demand
        )
        .await
        .map(Arc::new)
    }
}

#[async_trait]
impl<C: ConnectionLike + Clone + 'static + Send + Sync + Unpin> StoreDriver for RedisStore<C> {
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
                let (blob_len, exists) = pipe()
                    .strlen(encoded_key.as_ref())
                    .exists(encoded_key.as_ref())
                    .query_async::<(u64, bool)>(&mut client.connection_manager)
                    .await
                    .err_tip(|| "In RedisStore::has_with_results::all")?;

                *result = if exists { Some(blob_len) } else { None };

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
        let opts = ScanOptions::default()
            .with_pattern(pattern)
            .with_count(self.scan_count);
        let mut scan_stream: AsyncIter<Value> = client
            .connection_manager
            .scan_options(opts)
            .await
            .err_tip(|| "During scan_options")?;
        let mut iterations = 0;
        let mut errors = vec![];
        while let Some(key) = scan_stream.next_item().await {
            if let Ok(Value::BulkString(raw_key)) = key {
                let Ok(str_key) = str::from_utf8(&raw_key) else {
                    error!(?raw_key, "Non-utf8 key");
                    errors.push(format!("Non-utf8 key {raw_key:?}"));
                    continue;
                };
                if let Some(key) = str_key.strip_prefix(&self.key_prefix) {
                    let key = StoreKey::new_str(key);
                    if range.contains(&key) {
                        iterations += 1;
                        if !handler(&key) {
                            error!("Issue in handler");
                            errors.push("Issue in handler".to_string());
                        }
                    } else {
                        trace!(%key, ?range, "Key not in range");
                    }
                } else {
                    errors.push("Key doesn't match prefix".to_string());
                }
            } else {
                error!(?key, "Non-string in key");
                errors.push("Non-string in key".to_string());
            }
        }
        if errors.is_empty() {
            Ok(iterations)
        } else {
            error!(?errors, "Errors in scan stream");
            Err(Error::new(Code::Internal, format!("Errors: {errors:?}")))
        }
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

        let mut client = self.get_client().await?;

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
            }).zip(
            stream::repeat(client.connection_manager.clone()))
            .map(|(res, mut connection_manager)| {
                let (offset, end_pos, chunk) = res?;
                let temp_key_ref = &temp_key;
                Ok(async move {
                    connection_manager
                        .setrange::<_, _, usize>(temp_key_ref, offset, chunk.to_vec())
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

        let blob_len: usize = client
            .connection_manager
            .strlen(&temp_key)
            .await
            .err_tip(|| format!("In RedisStore::update strlen check for {temp_key}"))?;
        // This is a safety check to ensure that in the event some kind of retry was to happen
        // and the data was appended to the key twice, we reject the data.
        if blob_len != usize::try_from(total_len).unwrap_or(usize::MAX) {
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
            .connection_manager
            .rename::<_, _, ()>(&temp_key, final_key.as_ref())
            .await
            .err_tip(|| "While queueing key rename in RedisStore::update()")?;

        // If we have a publish channel configured, send a notice that the key has been set.
        if let Some(pub_sub_channel) = &self.pub_sub_channel {
            return Ok(client
                .connection_manager
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
            .saturating_add(length.unwrap_or(isize::MAX as usize) as isize)
            .saturating_sub(1);

        // And we don't ever want to read more than `read_chunk_size` bytes at a time, so we'll need to iterate.
        let mut chunk_start = data_start;
        let mut chunk_end = cmp::min(
            data_start.saturating_add(self.read_chunk_size as isize) - 1,
            data_end,
        );

        let mut client = self.get_client().await?;
        loop {
            let chunk: Bytes = client
                .connection_manager
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
                chunk_start.saturating_add(self.read_chunk_size as isize) - 1,
                data_end,
            );
        }

        // If we didn't write any data, check if the key exists, if not return a NotFound error.
        // This is required by spec.
        if writer.get_bytes_written() == 0 {
            // We're supposed to read 0 bytes, so just check if the key exists.
            let exists: bool = client
                .connection_manager
                .exists(encoded_key)
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
impl<C: ConnectionLike + Clone + 'static + Send + Sync + Unpin> HealthStatusIndicator
    for RedisStore<C>
{
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
const MAX_COUNT_PER_CURSOR: u64 = 256;
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
pub const LUA_VERSION_SET_SCRIPT: &str = formatcp!(
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

/// Trait for subscribing to Redis pub/sub channels with pattern matching.
pub trait RedisPatternSubscriber: Send + 'static {
    /// Subscribe to channels matching the given pattern.
    fn subscribe_to_pattern(
        &mut self,
        channel_pattern: &str,
    ) -> impl Future<Output = RedisResult<()>> + Send;
}

impl RedisPatternSubscriber for ConnectionManager {
    fn subscribe_to_pattern(
        &mut self,
        channel_pattern: &str,
    ) -> impl Future<Output = RedisResult<()>> + Send {
        self.psubscribe(channel_pattern)
    }
}

impl RedisPatternSubscriber for MockRedisConnection {
    fn subscribe_to_pattern(
        &mut self,
        _channel_pattern: &str,
    ) -> impl Future<Output = RedisResult<()>> + Send {
        future::ready(Ok(()))
    }
}

impl RedisSubscriptionManager {
    pub fn new<C>(
        mut connection_manager: C,
        mut subscription_channel: UnboundedReceiver<PushInfo>,
        pub_sub_channel: String,
    ) -> Self
    where
        C: RedisPatternSubscriber,
    {
        let subscribed_keys = Arc::new(RwLock::new(StringPatriciaMap::new()));
        let subscribed_keys_weak = Arc::downgrade(&subscribed_keys);
        let (tx_for_test, mut rx_for_test) = unbounded_channel();
        Self {
            subscribed_keys,
            tx_for_test,
            _subscription_spawn: spawn!("redis_subscribe_spawn", async move {
                if let Err(e) = connection_manager
                    .subscribe_to_pattern(&pub_sub_channel)
                    .await
                {
                    error!(?e, "Failed to subscribe to Redis pattern");
                    return;
                }
                loop {
                    loop {
                        let key = select! {
                            value = rx_for_test.recv() => {
                                let Some(value) = value else {
                                    unreachable!("Channel should never close");
                                };
                                value
                            },
                            msg = subscription_channel.recv() => {
                                if let Some(msg) = msg {
                                    if msg.data.len() != 1 {
                                        error!(?msg.data, "Received several messages!");
                                    }
                                    if let Value::SimpleString(s) = msg.data.first().expect("Expected data") {
                                        s.clone()
                                    } else {
                                        error!("Received non-string message in RedisSubscriptionManager");
                                        continue;
                                    }
                                } else {
                                    // Check to see if our parent has been dropped and if so kill spawn.
                                    if subscribed_keys_weak.upgrade().is_none() {
                                        warn!("It appears our parent has been dropped, exiting RedisSubscriptionManager spawn");
                                        return;
                                    }
                                    error!("Error receiving message in RedisSubscriptionManager reconnecting and flagging everything changed");
                                    break;
                                }
                            },
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

impl<C: RedisPatternSubscriber + Clone + ConnectionLike + Sync> SchedulerStore for RedisStore<C> {
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
            // Use pre-created channel if available (cluster mode), otherwise create on demand.
            // TODO: For standard mode, the sender should be connected to Redis push notifications.
            let subscriber_channel = self
                .subscriber_channel
                .lock()
                .take()
                .unwrap_or_else(|| unbounded_channel().1);
            let sub = Arc::new(RedisSubscriptionManager::new(
                self.connection_manager.clone(),
                subscriber_channel,
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
        let mut client = self.get_client().await?;
        let maybe_index = data.get_indexes().err_tip(|| {
            format!("Err getting index in RedisStore::update_data::versioned for {redis_key}")
        })?;
        if <T as SchedulerStoreKeyProvider>::Versioned::VALUE {
            let current_version = data.current_version();
            let data = data.try_into_bytes().err_tip(|| {
                format!("Could not convert value to bytes in RedisStore::update_data::versioned for {redis_key}")
            })?;
            let mut script = self
                .update_if_version_matches_script
                .key(redis_key.as_ref());
            let mut script_invocation = script.arg(format!("{current_version}")).arg(data.to_vec());
            for (name, value) in maybe_index {
                script_invocation = script_invocation.arg(name).arg(value.to_vec());
            }
            let (success, new_version): (bool, i64) = script_invocation
                .invoke_async(&mut client.connection_manager)
                .await
                .err_tip(|| format!("In RedisStore::update_data::versioned for {key:?}"))?;
            if !success {
                warn!(
                    %redis_key,
                    %key,
                    %current_version,
                    %new_version,
                    "Error updating Redis key"
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
            client
                .connection_manager
                .hset_multiple::<_, _, _, ()>(redis_key.as_ref(), &fields)
                .await
                .err_tip(|| format!("In RedisStore::update_data::noversion for {redis_key}"))?;
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
    {
        let index_value = index.index_value();
        let run_ft_aggregate = || {
            let connection_manager = self.connection_manager.clone();
            let sanitized_field = try_sanitize(index_value.as_ref()).err_tip(|| {
                format!("In RedisStore::search_by_index_prefix::try_sanitize - {index_value:?}")
            })?;
            Ok::<_, Error>(async move {
                ft_aggregate(
                    connection_manager,
                    format!(
                        "{}",
                        get_index_name!(K::KEY_PREFIX, K::INDEX_NAME, K::MAYBE_SORT_KEY)
                    ),
                    format!("@{}:{{ {} }}", K::INDEX_NAME, sanitized_field),
                    FtAggregateOptions {
                        load: vec![DATA_FIELD_NAME.into(), VERSION_FIELD_NAME.into()],
                        cursor: FtAggregateCursor {
                            count: MAX_COUNT_PER_CURSOR,
                            max_idle: CURSOR_IDLE_MS,
                        },
                        sort_by: K::MAYBE_SORT_KEY.map_or_else(Vec::new, |v| vec![format!("@{v}")]),
                    },
                )
                .await
            })
        };
        let stream = run_ft_aggregate()?
            .or_else(|_| async move {
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

                let create_result = ft_create(
                    self.connection_manager.clone(),
                    format!(
                        "{}",
                        get_index_name!(K::KEY_PREFIX, K::INDEX_NAME, K::MAYBE_SORT_KEY)
                    ),
                    FtCreateOptions {
                        prefixes: vec![K::KEY_PREFIX.into()],
                        nohl: true,
                        nofields: true,
                        nofreqs: true,
                        nooffsets: true,
                        temporary: Some(INDEX_TTL_S),
                    },
                    schema,
                )
                .await
                .err_tip(|| {
                    format!(
                        "Error with ft_create in RedisStore::search_by_index_prefix({})",
                        get_index_name!(K::KEY_PREFIX, K::INDEX_NAME, K::MAYBE_SORT_KEY),
                    )
                });
                let run_result = run_ft_aggregate()?.await.err_tip(|| {
                    format!(
                        "Error with second ft_aggregate in RedisStore::search_by_index_prefix({})",
                        get_index_name!(K::KEY_PREFIX, K::INDEX_NAME, K::MAYBE_SORT_KEY),
                    )
                });
                // Creating the index will race which is ok. If it fails to create, we only
                // error if the second ft_aggregate call fails and fails to create.
                run_result.or_else(move |e| create_result.merge(Err(e)))
            })
            .await?;
        Ok(stream.filter_map(|result| async move {
            let raw_redis_map = match result {
                Ok(v) => v,
                Err(e) => {
                    return Some(
                        Err(Error::from(e))
                            .err_tip(|| "Error in stream of in RedisStore::search_by_index_prefix"),
                    );
                }
            };

            let Some(redis_map) = raw_redis_map.as_sequence() else {
                return Some(Err(Error::new(
                    Code::Internal,
                    format!("Non-map from ft_aggregate: {raw_redis_map:?}"),
                )));
            };
            let mut redis_map_iter = redis_map.iter();
            let mut bytes_data: Option<Bytes> = None;
            let mut version: Option<i64> = None;
            loop {
                let Some(key) = redis_map_iter.next() else {
                    break;
                };
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
                                format!("Non-integer version value from ft_aggregate: {str_v:?}"),
                            )));
                        };
                        version = Some(raw_version);
                    }
                    other => {
                        return Some(Err(Error::new(
                            Code::Internal,
                            format!("Extra keys from ft_aggregate: {other}"),
                        )));
                    }
                }
            }
            let Some(found_bytes_data) = bytes_data else {
                return Some(Err(Error::new(
                    Code::Internal,
                    format!("Missing '{DATA_FIELD_NAME}' in ft_aggregate, got: {raw_redis_map:?}"),
                )));
            };
            Some(
                K::decode(version.unwrap_or(0), found_bytes_data)
                    .err_tip(|| "In RedisStore::search_by_index_prefix::decode"),
            )
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
        let mut client = self.get_client().await?;
        let results: Vec<Value> = client
            .connection_manager
            .hmget::<_, Vec<String>, Vec<Value>>(
                key.as_ref(),
                vec![VERSION_FIELD_NAME.into(), DATA_FIELD_NAME.into()],
            )
            .await
            .err_tip(|| format!("In RedisStore::get_without_version::notversioned {key}"))?;
        let Some(Value::BulkString(data)) = results.get(1) else {
            return Ok(None);
        };
        #[allow(clippy::get_first)]
        let version = if let Some(Value::Int(v)) = results.get(0) {
            *v
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
