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
use std::cell::{OnceCell, UnsafeCell};
use std::fmt::Display;
use std::pin::Pin;
use std::sync::{Arc, Once};

use async_trait::async_trait;
use bytes::Bytes;
use futures::future::{ErrInto, FutureExt, Shared};
use futures::stream::FuturesOrdered;
use futures::{Future, TryFutureExt, TryStreamExt};
use nativelink_config::stores::RedisMode;
use nativelink_error::{error_if, make_err, Code, Error, ResultExt};
use nativelink_util::background_spawn;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::metrics_utils::{Collector, CollectorState, MetricsComponent, Registry};
use nativelink_util::store_trait::{StoreDriver, StoreKey, UploadSizeInfo};
use redis::aio::{ConnectionLike, ConnectionManager};
use redis::cluster_async::ClusterConnection;
use redis::{AsyncCommands, ToRedisArgs};
use tokio::task::JoinHandle;

use crate::cas_utils::is_zero_digest;

const READ_CHUNK_SIZE: isize = 64 * 1024;

/// A wrapper type containing the different Redis clients we support.
//
// Typically we would use `dyn ConnectionLike` instead of creating a wrapper type, but these clients are cheaply
// cloneable; you're meant to clone a client in order to get mutable access.
// [`Clone`] has a [`Sized`] bound, which means that any supertrait we constructed with a `Clone` bound
// wouldn't be object safe -- in short, this won't compile:
//
// ```compile_fail
// trait CloneableConnectionHandle: ConnectionLike + Clone {}
//
// impl <C: ConnectionLike + Clone> CloneableConnectionHandle for C {}
// ```
#[derive(Clone)]
pub enum ConnectionKind {
    Cluster(ClusterConnection),
    Single(ConnectionManager),
}

impl From<ClusterConnection> for ConnectionKind {
    fn from(value: ClusterConnection) -> Self {
        Self::Cluster(value)
    }
}

impl From<ConnectionManager> for ConnectionKind {
    fn from(value: ConnectionManager) -> Self {
        Self::Single(value)
    }
}

// delegate to the inner impl's
impl ConnectionLike for ConnectionKind {
    fn req_packed_command<'a>(
        &'a mut self,
        cmd: &'a redis::Cmd,
    ) -> redis::RedisFuture<'a, redis::Value> {
        match self {
            ConnectionKind::Cluster(inner) => inner.req_packed_command(cmd),
            ConnectionKind::Single(inner) => inner.req_packed_command(cmd),
        }
    }

    fn req_packed_commands<'a>(
        &'a mut self,
        cmd: &'a redis::Pipeline,
        offset: usize,
        count: usize,
    ) -> redis::RedisFuture<'a, Vec<redis::Value>> {
        match self {
            ConnectionKind::Cluster(inner) => inner.req_packed_commands(cmd, offset, count),
            ConnectionKind::Single(inner) => inner.req_packed_commands(cmd, offset, count),
        }
    }

    fn get_db(&self) -> i64 {
        match self {
            ConnectionKind::Cluster(inner) => inner.get_db(),
            ConnectionKind::Single(inner) => inner.get_db(),
        }
    }
}

/// Type alias for a [`Shared`] [`JoinHandle`] that has had its [`JoinError`](`tokio::task::JoinError`) mapped to an [`Error`]
type RedisConnectionFuture<C> = Shared<ErrInto<JoinHandle<Result<C, Error>>, Error>>;

/// Represents the possible states of a Redis connection.
enum ConnectionState<C> {
    /// Contains a future that must be polled to connect to Redis
    Connecting(RedisConnectionFuture<C>),

    /// Contains a connection that was made successfully
    Connected(C),

    /// Contains an error that occurred while connecting
    Errored(Error),
}

/// Represents a connection to Redis.
pub struct BackgroundConnection<C> {
    /// Synchronization primitive used for tracking if `self.state` has been changed from `ConnectionState::Connecting`
    /// to `ConnectionState::Error` or `ConnectionState::Connected`. Once it's been changed exactly once,
    /// [`Once::is_completed`] will return `true`.
    once: Once,

    /// Contains the current state of the connection.
    // Invariant: the state must be mutated exactly once.
    state: UnsafeCell<ConnectionState<C>>,
}

impl BackgroundConnection<ConnectionKind> {
    /// Connect to a single Redis instance.
    ///
    /// ## Errors
    ///
    /// Some cursory checks are performed on the given connection info that can fail before a connection is established.
    /// Errors that occur during the connection process are surfaced when the connection is first used.
    pub fn single<T: redis::IntoConnectionInfo>(params: T) -> Result<Self, Error> {
        let client = redis::Client::open(params).map_err(from_redis_err)?;
        let init = async move { client.get_connection_manager().await };
        Ok(Self::with_initializer(init))
    }

    /// Connect to multiple Redis instances configured in cluster mode
    ///
    /// ## Errors
    ///
    /// Some cursory checks are performed on the given connection info that can fail before a connection is established.
    /// Errors that occur during the connection are surfaced when the connection is first used.
    pub fn cluster<T: redis::IntoConnectionInfo>(
        params: impl IntoIterator<Item = T>,
    ) -> Result<Self, Error> {
        let client = redis::cluster::ClusterClient::new(params).map_err(from_redis_err)?;
        let init = async move { client.get_async_connection().await };
        Ok(Self::with_initializer(init))
    }
}

impl<C: Clone + Send + 'static> BackgroundConnection<C> {
    /// Initialize a new connection by spawning a background task to run the provided future to completion.
    ///
    /// Outside of testing, you will probably want to use [`BackgroundConnection::single`] or [`BackgroundConnection::cluster`].
    pub fn with_initializer<Fut, T, E>(init: Fut) -> Self
    where
        Fut: Future<Output = Result<T, E>> + Send + 'static,
        C: From<T>,
        T: Send + 'static,
        Error: From<E>,
        E: Send + 'static,
    {
        let handle = background_spawn!("redis_initial_connection", init.err_into().ok_into());
        let state = ConnectionState::Connecting(handle.err_into().shared());
        Self {
            once: Once::new(),
            state: UnsafeCell::new(state),
        }
    }

    /// Retrieve the underlying connection. If the connection hasn't been established yet, the current task will
    /// wait until the connection has been made.
    ///
    /// ## Errors
    ///
    /// Returns an error if there was an issue establishing a connection to Redis.
    async fn get(&self) -> Result<C, Error> {
        // Safety: we don't mutate state here, so normal borrowck rules are followed and the invariant is upheld
        let state_ref = unsafe { &*self.state.get() };
        let connection_future = match state_ref {
            ConnectionState::Connecting(handle) => Shared::clone(handle),
            ConnectionState::Connected(connection) => return Ok(connection.clone()),
            ConnectionState::Errored(error) => return Err(error.clone()),
        };

        let connection_result = connection_future.await.and_then(|conn| conn);
        self.once.call_once(|| {
            // Safety: This part is `unsafe` because we break borrowck's rules of aliasing XOR mutability;
            // calling `LazyConnection::get` takes `&self`, but now we're going to take `&mut self.state`.
            // This means that if multiple tasks call `LazyConnection::get` at once, they could potentially
            // attempt to mutate `self.state` simultaneously, which is `unsafe` -- it's a data race.
            //
            // The synchronization primitive we're using here to manually enforce borrowck's rules is [`Once`],
            // which allows for executing closures exactly once. We use this guarantee to ensure that we only
            // mutate `self.state` exactly once, despite having multiple tasks awaiting the same connection.
            //
            // Put another way: we only mutate state exactly once, inside this closure (exclusive). Outside of
            // the closure, multiple tasks can read state simultaneously (aliasing).
            //
            // More specifically, the `Once` can be in one of three states:
            //
            // 1. Uninitialized
            //  In this state, borrowck rules are followed because nobody has attempted to mutate `self.state` yet.
            // 2. Initializing
            //  In this state, borrowck rules are followed because exactly one thread has exclusive mutable access
            //  to `self.state`, while all other threads block -- i.e. they will not read or write state.
            // 3. Initialized
            //  In this state, borrowck rules are followed because this closure will never get called.
            //
            // Put a third way: we've essentially recreated a `RwLock` that always prioritizes writes, and only allows
            // one write. The invariant is upheld.
            let state_mut = unsafe { &mut *self.state.get() };
            *state_mut = match connection_result.clone() {
                Ok(connection) => ConnectionState::Connected(connection),
                Err(error) => ConnectionState::Errored(error),
            };
        });

        connection_result
    }
}

// Safety: We don't hold any raw pointers or `!Send` types except `UnsafeCell`.
// Why do we need `C: Send`?
// Task A creates a `BackgroundConnection` and shares it with task B,
// which mutates the cell, which is then destroyed by A.
// That is, destructor observes a sent value.
unsafe impl<C: Sync + Send> Send for BackgroundConnection<C> {}

// Safety: We ensure that exactly one task will mutate state exactly once.
unsafe impl<C: Sync> Sync for BackgroundConnection<C> {}

/// A [`StoreDriver`] implementation that uses Redis as a backing store.
pub struct RedisStore<C: ConnectionLike + Clone = ConnectionKind> {
    /// The connection to the underlying Redis instance(s).
    connection: BackgroundConnection<C>,

    /// A function used to generate names for temporary keys.
    temp_name_generator_fn: fn() -> String,
    pub_sub_channel: Option<String>,

    /// A common prefix to append to all keys before they are sent to Redis.
    ///
    /// See [`RedisStore::key_prefix`](`nativelink_config::stores::RedisStore::key_prefix`).
    key_prefix: String,
}

impl RedisStore<ConnectionKind> {
    pub fn new(config: &nativelink_config::stores::RedisStore) -> Result<Arc<Self>, Error> {
        if config.addresses.is_empty() {
            return Err(Error::new(
                Code::InvalidArgument,
                "At least one address must be specified to connect to Redis".to_string(),
            ));
        };
        let connection =
            match config.mode {
                RedisMode::Cluster => {
                    let addrs = config.addresses.iter().map(String::as_str);
                    BackgroundConnection::cluster(addrs)?
                }
                RedisMode::Standard if config.addresses.len() > 1 => return Err(Error::new(
                    Code::InvalidArgument,
                    "Attempted to connect to multiple addresses without setting `cluster = true`"
                        .to_string(),
                )),
                RedisMode::Standard => {
                    let addr = config.addresses[0].as_str();
                    BackgroundConnection::single(addr)?
                }
                RedisMode::Sentinel => {
                    return Err(Error::new(
                        Code::Unimplemented,
                        "Sentinel mode is currently not supported.".to_string(),
                    ))
                }
            };

        Ok(Arc::new(
            RedisStore::new_with_conn_and_name_generator_and_prefix(
                connection,
                || uuid::Uuid::new_v4().to_string(),
                config.experimental_pub_sub_channel.clone(),
                config.key_prefix.clone(),
            ),
        ))
    }
}

impl<C: ConnectionLike + Clone + Send + 'static> RedisStore<C> {
    #[inline]
    pub fn new_with_conn_and_name_generator(
        connection: BackgroundConnection<C>,
        temp_name_generator_fn: fn() -> String,
    ) -> Self {
        RedisStore::new_with_conn_and_name_generator_and_prefix(
            connection,
            temp_name_generator_fn,
            None,
            String::new(),
        )
    }

    #[inline]
    pub fn new_with_conn_and_name_generator_and_prefix(
        connection: BackgroundConnection<C>,
        temp_name_generator_fn: fn() -> String,
        pub_sub_channel: Option<String>,
        key_prefix: String,
    ) -> Self {
        RedisStore {
            connection,
            temp_name_generator_fn,
            pub_sub_channel,
            key_prefix,
        }
    }

    #[inline]
    pub async fn get_conn(&self) -> Result<C, Error> {
        self.connection.get().await
    }

    /// Encode a [`StoreKey`] so it can be sent to Redis.
    fn encode_key<'a>(&self, key: StoreKey<'a>) -> impl ToRedisArgs + Display + Send + Sync + 'a {
        // TODO(caass): Once https://github.com/redis-rs/redis-rs/pull/1219 makes it into a release,
        // this can be changed to
        // ```rust
        // if self.key_prefix.is_empty() {
        //   key.as_str()
        // } else {
        //   let mut encoded_key = String::with_capacity(self.key_prefix.len() + key_body.len());
        //   encoded_key.push_str(&self.key_prefix);
        //   encoded_key.push_str(&key_body);
        //   Cow::Owned(encoded_key)
        // }
        //```
        // and the return type changed to `Cow<'a, str>`
        let key_body = key.as_str();

        let mut encoded_key = String::with_capacity(self.key_prefix.len() + key_body.len());
        encoded_key.push_str(&self.key_prefix);
        encoded_key.push_str(&key_body);

        encoded_key
    }
}

#[async_trait]
impl<C> StoreDriver for RedisStore<C>
where
    C: ConnectionLike + Clone + Send + Sync + Unpin + 'static,
{
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        if keys.len() == 1 && is_zero_digest(keys[0].borrow()) {
            results[0] = Some(0);
            return Ok(());
        }

        let mut zero_digest_indexes = Vec::new();

        let queries =
            keys.iter()
                .enumerate()
                .map(|(index, key)| {
                    if is_zero_digest(key.borrow()) {
                        zero_digest_indexes.push(index);
                    }
                    let encoded_key = self.encode_key(key.borrow());

                    async {
                        let mut conn = self.get_conn().await.err_tip(|| {
                            "Error: Could not get connection handle in has_with_results"
                        })?;

                        conn.strlen::<_, usize>(encoded_key)
                            .await
                            .map_err(from_redis_err)
                            .err_tip(|| "Error: Could not call strlen in has_with_results")
                    }
                })
                .collect::<FuturesOrdered<_>>();

        let digest_sizes = queries.try_collect::<Vec<_>>().await?;

        error_if!(
            digest_sizes.len() != results.len(),
            "Mismatch in digest sizes and results length"
        );

        digest_sizes
            .into_iter()
            .zip(results.iter_mut())
            .for_each(|(size, result)| {
                *result = if size == 0 { None } else { Some(size) };
            });

        zero_digest_indexes.into_iter().for_each(|index| {
            results[index] = Some(0);
        });

        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        _upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        let temp_key = OnceCell::new();
        let final_key = self.encode_key(key.borrow());

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
        let make_temp_name = || {
            format!(
                "temp-{}-{{{}}}",
                (self.temp_name_generator_fn)(),
                &final_key
            )
        };

        let mut conn = self.get_conn().await?;
        let mut pipe = redis::pipe();
        pipe.atomic();

        'outer: loop {
            let mut force_recv = true;

            while force_recv || !reader.is_empty() {
                let chunk = reader
                    .recv()
                    .await
                    .err_tip(|| "Failed to reach chunk in update in redis store")?;

                if chunk.is_empty() {
                    if is_zero_digest(key.borrow()) {
                        return Ok(());
                    }
                    if force_recv {
                        conn.append(&final_key, &chunk[..])
                            .await
                            .map_err(from_redis_err)
                            .err_tip(|| "In RedisStore::update() single chunk")?;
                    }

                    break 'outer;
                }

                pipe.cmd("APPEND")
                    .arg(temp_key.get_or_init(make_temp_name))
                    .arg(&chunk[..]);
                force_recv = false;

                // Give other tasks a chance to run to populate the reader's
                // buffer if possible.
                tokio::task::yield_now().await;
            }

            pipe.query_async(&mut conn)
                .await
                .map_err(from_redis_err)
                .err_tip(|| "In RedisStore::update::query_async")?;

            pipe.clear();
        }

        pipe.cmd("RENAME")
            .arg(temp_key.get_or_init(make_temp_name))
            .arg(&final_key);

        pipe.query_async(&mut conn)
            .await
            .map_err(from_redis_err)
            .err_tip(|| "In RedisStore::update")?;

        if let Some(pub_sub_channel) = &self.pub_sub_channel {
            conn.publish(pub_sub_channel, &final_key)
                .await
                .map_err(from_redis_err)
                .err_tip(|| "Failed to publish temp key value to configured channel")?
        }

        Ok(())
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

        let mut conn = self.get_conn().await?;
        if length == Some(0) {
            let exists = conn
                .exists::<_, bool>(self.encode_key(key.borrow()))
                .await
                .map_err(from_redis_err)
                .err_tip(|| "In RedisStore::get_part::zero_exists")?;
            if !exists {
                return Err(make_err!(
                    Code::NotFound,
                    "Data not found in Redis store for digest: {key:?}"
                ));
            }
            writer
                .send_eof()
                .err_tip(|| "Failed to write EOF in redis store get_part")?;
            return Ok(());
        }

        let mut current_start = isize::try_from(offset)
            .err_tip(|| "Cannot convert offset to isize in RedisStore::get_part()")?;
        let max_length = isize::try_from(length.unwrap_or(isize::MAX as usize))
            .err_tip(|| "Cannot convert length to isize in RedisStore::get_part()")?;
        let end_position = current_start.saturating_add(max_length);

        loop {
            // Note: Redis getrange is inclusive, so we need to subtract 1 from the end.
            let current_end =
                std::cmp::min(current_start.saturating_add(READ_CHUNK_SIZE), end_position) - 1;
            let chunk = conn
                .getrange::<_, Bytes>(self.encode_key(key.borrow()), current_start, current_end)
                .await
                .map_err(from_redis_err)
                .err_tip(|| "In RedisStore::get_part::getrange")?;

            if chunk.is_empty() {
                writer
                    .send_eof()
                    .err_tip(|| "Failed to write EOF in redis store get_part")?;
                break;
            }

            // Note: Redis getrange is inclusive, so we need to add 1 to the end.
            let was_partial_data = chunk.len() as isize != current_end - current_start + 1;
            current_start += chunk.len() as isize;
            writer
                .send(chunk)
                .await
                .err_tip(|| "Failed to write data in Redis store")?;

            // If we got partial data or the exact requested number of bytes, we are done.
            if writer.get_bytes_written() as isize == max_length || was_partial_data {
                writer
                    .send_eof()
                    .err_tip(|| "Failed to write EOF in redis store get_part")?;

                break;
            }

            error_if!(
                writer.get_bytes_written() as isize > max_length,
                "Data received exceeds requested length"
            );
        }

        Ok(())
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

impl<C> MetricsComponent for RedisStore<C>
where
    C: ConnectionLike + Clone + Send + 'static,
{
    fn gather_metrics(&self, _c: &mut CollectorState) {}
}

#[async_trait]
impl<C> HealthStatusIndicator for RedisStore<C>
where
    C: ConnectionLike + Clone + Send + Sync + Unpin + 'static,
{
    fn get_name(&self) -> &'static str {
        "RedisStore"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}

fn from_redis_err(call_res: redis::RedisError) -> Error {
    make_err!(Code::Internal, "Redis Error: {call_res}")
}
