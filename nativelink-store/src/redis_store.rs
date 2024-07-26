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

use std::borrow::Cow;
use std::cell::OnceCell;
use std::cmp;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use fred::clients::{RedisPool, SubscriberClient};
use fred::mocks::{MockCommand, Mocks};
use fred::prelude::*;
use nativelink_config::stores::RedisMode;
use nativelink_error::{error_if, make_err, Code, Error, ResultExt};
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::metrics_utils::{Collector, CollectorState, MetricsComponent, Registry};
use nativelink_util::store_trait::{StoreDriver, StoreKey, UploadSizeInfo};
use uuid::Uuid;

use crate::cas_utils::is_zero_digest;

const READ_CHUNK_SIZE: usize = 64 * 1024;
const CONNECTION_POOL_SIZE: usize = 3;

/// A [`StoreDriver`] implementation that uses Redis as a backing store.
pub struct RedisStore {
    /// The client pool connecting to the backing Redis instance(s).
    client_pool: RedisPool,

    /// A dedicated client running in `subscriber` mode
    _subscriber: SubscriberClient,

    /// A channel to publish updates to when a key is added, removed, or modified
    pub_sub_channel: Option<String>,

    /// A function used to generate names for temporary keys.
    temp_name_generator_fn: fn() -> String,

    /// A common prefix to append to all keys before they are sent to Redis.
    ///
    /// See [`RedisStore::key_prefix`](`nativelink_config::stores::RedisStore::key_prefix`).
    key_prefix: String,
}

impl RedisStore {
    /// Create a new `RedisStore` from the given configuration
    pub fn new(config: &nativelink_config::stores::RedisStore) -> Result<Arc<Self>, Error> {
        #[derive(Debug)]
        struct PanicMock;
        impl Mocks for PanicMock {
            fn process_command(&self, _: MockCommand) -> Result<RedisValue, RedisError> {
                panic!()
            }
        }
        const NO_MOCKS: Option<PanicMock> = None;

        Self::new_with_name_generator_and_mocks(config, || Uuid::new_v4().to_string(), NO_MOCKS)
            .map(Arc::new)
    }

    /// Used for testing, when determinism is required
    pub fn new_with_name_generator_and_mocks(
        config: &nativelink_config::stores::RedisStore,
        temp_name_generator_fn: fn() -> String,
        mocks: Option<impl Mocks>,
    ) -> Result<Self, Error> {
        if config.addresses.is_empty() {
            return Err(make_err!(
                Code::InvalidArgument,
                "No addresses were specified in redis store configuration."
            ));
        };
        let [addr] = config.addresses.as_slice() else {
            return Err(make_err!(Code::Unimplemented, "Connecting directly to multiple redis nodes in a cluster is currently unsupported. Please specify a single URL to a single node, and nativelink will use cluster discover to find the other nodes."));
        };
        let mut redis_config = match config.mode {
            RedisMode::Cluster => RedisConfig::from_url_clustered(addr),
            RedisMode::Sentinel => RedisConfig::from_url_sentinel(addr),
            RedisMode::Standard => RedisConfig::from_url_centralized(addr),
        }
        .err_tip_with_code(|_| (Code::InvalidArgument, "while parsing redis node address"))?;

        redis_config.mocks = mocks.map(|m| Arc::new(m) as Arc<dyn Mocks>);

        let mut builder = Builder::from_config(redis_config);
        builder
            .set_performance_config(PerformanceConfig {
                default_command_timeout: Duration::from_secs(config.response_timeout_s),
                ..Default::default()
            })
            .set_connection_config(ConnectionConfig {
                connection_timeout: Duration::from_secs(config.connection_timeout_s),
                internal_command_timeout: Duration::from_secs(config.response_timeout_s),
                ..Default::default()
            })
            .set_policy(ReconnectPolicy::new_constant(1, 0));

        let client_pool = builder
            .build_pool(CONNECTION_POOL_SIZE)
            .err_tip(|| "while creating redis connection pool")?;
        let subscriber = builder
            .build_subscriber_client()
            .err_tip(|| "while creating redis subscription client")?;

        client_pool.connect();
        subscriber.connect();

        Ok(Self {
            client_pool,
            _subscriber: subscriber,
            pub_sub_channel: config.experimental_pub_sub_channel.clone(),
            temp_name_generator_fn,
            key_prefix: config.key_prefix.clone(),
        })
    }

    /// Encode a [`StoreKey`] so it can be sent to Redis.
    fn encode_key<'a>(&self, key: &'a StoreKey<'a>) -> Cow<'a, str> {
        let key_body = key.as_str();
        if self.key_prefix.is_empty() {
            key_body
        } else {
            let mut encoded_key = String::with_capacity(self.key_prefix.len() + key_body.len());
            encoded_key.push_str(&self.key_prefix);
            encoded_key.push_str(&key_body);
            Cow::Owned(encoded_key)
        }
    }
}

#[async_trait]
impl StoreDriver for RedisStore {
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        let pipeline = self.client_pool.next().pipeline();

        results.iter_mut().for_each(|result| *result = None); // is this necessary?

        for (idx, key) in keys.iter().enumerate() {
            // don't bother with zero-length digests
            if is_zero_digest(key.borrow()) {
                results[idx] = Some(0);
                continue;
            }

            let encoded_key = self.encode_key(key);

            // this command is queued in memory, but not yet sent down the pipeline; the `await` returns instantly
            let _: () = pipeline
                .strlen(encoded_key.as_ref())
                .await
                .err_tip(|| "could not call `strlen` in has_with_results")?;
        }

        // now we send the commands. kind of a weird api, maybe we could submit a patch?
        let responses = pipeline.all::<Vec<_>>().await?;
        let remaining_results = results.iter_mut().filter(|option| {
            // anything that's `Some` was already set from `is_zero_digest`
            option.is_none()
        });

        for (response, result_slot) in responses.into_iter().zip(remaining_results) {
            if response == 0 {
                // redis returns 0 when the key doesn't exist AND when the key exists with value of length 0,
                // but since we already checked zero-lengths with `is_zero_digest`, this means the value doesn't exist
                continue;
            }

            *result_slot = Some(response)
        }

        Ok(())
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
        //
        // TODO(caass): the stabilization PR for [`LazyCell`](`std::cell::LazyCell`) has been merged into rust-lang,
        // so in the next stable release we can use LazyCell::new(|| { ... }) instead.
        let temp_key = OnceCell::new();
        let make_temp_name = || {
            format!(
                "temp-{}-{{{}}}",
                (self.temp_name_generator_fn)(),
                &final_key
            )
        };

        let client = self.client_pool.next();

        // This loop is a little confusing at first glance, but essentially the process is:
        // - Get as much data from the reader as possible
        // - When the reader is empty, but the writer isn't done sending data, write that data to redis
        // - When the writer is done sending data, write the data and break from the loop
        //
        // At one extreme, we could append data in redis every time we read some bytes -- that is, make one TCP request
        // per channel read. This is wasteful since we anticipate reading many small chunks of bytes from the reader.
        //
        // At the other extreme, we could make a single TCP request to write all of the data all at once.
        // This could also be an issue if we read loads of data, since we'd send one massive TCP request
        // rather than a few moderately-sized requests.
        //
        // To compromise, we buffer opportunistically -- when the reader doesn't have any data ready to read, but it's
        // not done getting data, we flush the data we _have_ read to redis before waiting for the reader to get more.
        //
        // As a result of this, there will be a span of time where a key in Redis has only partial data. We want other
        // observers to notice atomic updates to keys, rather than partial updates, so we first write to a temporary key
        // and then rename that key once we're done appending data.
        'outer: loop {
            let mut expecting_first_chunk = true;
            let pipe = client.pipeline();

            while expecting_first_chunk || !reader.is_empty() {
                let chunk = reader
                    .recv()
                    .await
                    .err_tip(|| "Failed to reach chunk in update in redis store")?;

                if chunk.is_empty() {
                    if is_zero_digest(key.borrow()) {
                        return Ok(());
                    }

                    // reader sent empty chunk, we're done here.
                    break 'outer;
                }

                // queued
                pipe.append(temp_key.get_or_init(make_temp_name), &chunk[..])
                    .await?;
                expecting_first_chunk = false;

                // Give other tasks a chance to run to populate the reader's
                // buffer if possible.
                tokio::task::yield_now().await;
            }

            // reader is empty, but we're expecting more data. execute queued appends to the temp key
            pipe.all().await?;
        }

        // We're done receiving all data, so let's
        // - create a key with the correct name
        // - move the data from the temp key to the final key
        // - publish the update if we have somewhere to publish to
        //
        // This is done atomically via MULTI/EXEC, so from an outside observer's perspective the key appears
        // with all the data.
        let tx = client.multi();

        tx.append(final_key.as_ref(), "").await?;
        tx.rename(temp_key.get_or_init(make_temp_name), final_key.as_ref())
            .await?;
        if let Some(pub_sub_channel) = &self.pub_sub_channel {
            tx.publish(pub_sub_channel, final_key.as_ref()).await?;
        }

        tx.exec(false)
            .await
            .err_tip(|| "While renaming key in RedisStore::update()")
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        // To follow RBE spec we need to consider any digest's with
        // zero size to be existing.
        if is_zero_digest(key.borrow()) {
            writer
                .send_eof()
                .err_tip(|| "Failed to send zero EOF in redis store get_part")?;
            return Ok(());
        }

        let client = self.client_pool.next();
        let encoded_key = self.encode_key(&key);
        let encoded_key = encoded_key.as_ref();

        if length == Some(0) {
            // we're supposed to read 0 bytes, so just check if the key exists
            let exists = client
                .exists(encoded_key)
                .await
                .err_tip(|| "In RedisStore::get_part::zero_exists")?;

            return match exists {
                0u8 => Err(make_err!(
                    Code::NotFound,
                    "Data not found in Redis store for digest: {key:?}"
                )),
                1 => writer
                    .send_eof()
                    .err_tip(|| "Failed to write EOF in redis store get_part"),
                _ => unreachable!("only checked for existence of a single key"),
            };
        }

        let mut current_start = offset;
        let max_length = length.unwrap_or(isize::MAX as usize);
        let end_position = current_start.saturating_add(max_length);

        loop {
            // Note: Redis getrange is inclusive, so we need to subtract 1 from the end.
            let current_end =
                cmp::min(current_start.saturating_add(READ_CHUNK_SIZE), end_position) - 1;
            let chunk: Bytes = client
                .getrange(encoded_key, current_start, current_end)
                .await
                .err_tip(|| "In RedisStore::get_part::getrange")?;

            if chunk.is_empty() {
                break writer
                    .send_eof()
                    .err_tip(|| "Failed to write EOF in redis store get_part");
            }

            // Note: Redis getrange is inclusive, so we need to add 1 to the end.
            let was_partial_data = chunk.len() != current_end - current_start + 1;
            current_start += chunk.len();
            writer
                .send(chunk)
                .await
                .err_tip(|| "Failed to write data in Redis store")?;

            // If we got partial data or the exact requested number of bytes, we are done.
            if writer.get_bytes_written() as usize == max_length || was_partial_data {
                break writer
                    .send_eof()
                    .err_tip(|| "Failed to write EOF in redis store get_part");
            }

            error_if!(
                writer.get_bytes_written() as usize > max_length,
                "Data received exceeds requested length"
            );
        }
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any(&self) -> &(dyn std::any::Any + Sync + Send) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Sync + Send> {
        self
    }

    fn register_metrics(self: Arc<Self>, registry: &mut Registry) {
        registry.register_collector(Box::new(Collector::new(&self)));
    }

    fn register_health(self: Arc<Self>, registry: &mut HealthRegistryBuilder) {
        registry.register_indicator(self);
    }
}

impl MetricsComponent for RedisStore {
    fn gather_metrics(&self, _c: &mut CollectorState) {}
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
