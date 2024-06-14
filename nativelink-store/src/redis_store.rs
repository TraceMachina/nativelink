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
use std::pin::Pin;
use std::sync::Arc;

use arc_cell::ArcCell;
use async_trait::async_trait;
use bytes::Bytes;
use futures::future::{BoxFuture, FutureExt, Shared};
use nativelink_error::{error_if, make_err, Code, Error, ResultExt};
use nativelink_util::background_spawn;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::metrics_utils::{Collector, CollectorState, MetricsComponent, Registry};
use nativelink_util::store_trait::{StoreDriver, StoreKey, UploadSizeInfo};
use redis::aio::{ConnectionLike, ConnectionManager};
use redis::{AsyncCommands, ToRedisArgs};

use crate::cas_utils::is_zero_digest;

const READ_CHUNK_SIZE: isize = 64 * 1024;

/// Holds a connection result or a future that resolves to a connection.
/// This is a utility to allow us to start a connection but not block on it.
pub enum LazyConnection<T: ConnectionLike + Unpin + Clone + Send + Sync> {
    Connection(Result<T, Error>),
    Future(Shared<BoxFuture<'static, Result<T, Error>>>),
}

pub struct RedisStore<T: ConnectionLike + Unpin + Clone + Send + Sync = ConnectionManager> {
    lazy_conn: ArcCell<LazyConnection<T>>,
    temp_name_generator_fn: fn() -> String,

    /// A common prefix to append to all keys before they are sent to Redis.
    ///
    /// See [`RedisStore::key_prefix`](`nativelink_config::stores::RedisStore::key_prefix`).
    key_prefix: String,
}

impl RedisStore {
    pub fn new(
        config: &nativelink_config::stores::RedisStore,
    ) -> Result<Arc<RedisStore<ConnectionManager>>, Error> {
        // Note: Currently only one connection is supported.
        error_if!(
            config.addresses.len() != 1,
            "Only one address is supported for Redis store"
        );

        let address = config.addresses[0].clone();
        let conn_fut = async move {
            redis::Client::open(address)
                .map_err(from_redis_err)?
                .get_connection_manager()
                .await
                .map_err(from_redis_err)
        }
        .boxed()
        .shared();

        let conn_fut_clone = conn_fut.clone();
        // Start connecting to redis, but don't block our construction on it.
        background_spawn!("redis_initial_connection", async move {
            if let Err(e) = conn_fut_clone.await {
                make_err!(Code::Unavailable, "Failed to connect to Redis: {:?}", e);
            }
        });

        let lazy_conn = LazyConnection::Future(conn_fut);

        Ok(Arc::new(
            RedisStore::new_with_conn_and_name_generator_and_prefix(
                lazy_conn,
                || uuid::Uuid::new_v4().to_string(),
                config.key_prefix.clone(),
            ),
        ))
    }
}

impl<T: ConnectionLike + Unpin + Clone + Send + Sync> RedisStore<T> {
    pub fn new_with_conn_and_name_generator(
        lazy_conn: LazyConnection<T>,
        temp_name_generator_fn: fn() -> String,
    ) -> RedisStore<T> {
        RedisStore::new_with_conn_and_name_generator_and_prefix(
            lazy_conn,
            temp_name_generator_fn,
            String::new(),
        )
    }

    pub fn new_with_conn_and_name_generator_and_prefix(
        lazy_conn: LazyConnection<T>,
        temp_name_generator_fn: fn() -> String,
        key_prefix: String,
    ) -> RedisStore<T> {
        RedisStore {
            lazy_conn: ArcCell::new(Arc::new(lazy_conn)),
            temp_name_generator_fn,
            key_prefix,
        }
    }

    pub async fn get_conn(&self) -> Result<T, Error> {
        let result = match self.lazy_conn.get().as_ref() {
            LazyConnection::Connection(conn_result) => return conn_result.clone(),
            LazyConnection::Future(fut) => fut.clone().await,
        };
        self.lazy_conn
            .set(Arc::new(LazyConnection::Connection(result.clone())));
        result
    }

    /// Encode a [`StoreKey`] so it can be sent to Redis.
    fn encode_key(&self, key: StoreKey) -> impl ToRedisArgs {
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
        // to avoid an allocation
        let key_body = key.as_str();

        let mut encoded_key = String::with_capacity(self.key_prefix.len() + key_body.len());
        encoded_key.push_str(&self.key_prefix);
        encoded_key.push_str(&key_body);

        encoded_key
    }
}

#[async_trait]
impl<T: ConnectionLike + Unpin + Clone + Send + Sync + 'static> StoreDriver for RedisStore<T> {
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        if keys.len() == 1 && is_zero_digest(keys[0].borrow()) {
            results[0] = Some(0);
            return Ok(());
        }
        let mut conn = self.get_conn().await?;

        let mut pipe = redis::pipe();
        pipe.atomic();

        let mut zero_digest_indexes = Vec::new();
        keys.iter().enumerate().for_each(|(index, key)| {
            if is_zero_digest(key.borrow()) {
                zero_digest_indexes.push(index);
            }

            pipe.strlen(self.encode_key(key.borrow()));
        });

        let digest_sizes = pipe
            .query_async::<_, Vec<usize>>(&mut conn)
            .await
            .map_err(from_redis_err)
            .err_tip(|| "Error: Could not call pipeline in has_with_results")?;

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
        let make_temp_name = || format!("temp-{}", (self.temp_name_generator_fn)());
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
                        conn.append(self.encode_key(key.borrow()), &chunk[..])
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
            .arg(self.encode_key(key));
        pipe.query_async(&mut conn)
            .await
            .map_err(from_redis_err)
            .err_tip(|| "In RedisStore::update")?;

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

impl<T: ConnectionLike + Unpin + Clone + Send + Sync + 'static> MetricsComponent for RedisStore<T> {
    fn gather_metrics(&self, _c: &mut CollectorState) {}
}

#[async_trait]
impl<T: ConnectionLike + ConnectionLike + Unpin + Clone + Send + Sync + 'static>
    HealthStatusIndicator for RedisStore<T>
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
