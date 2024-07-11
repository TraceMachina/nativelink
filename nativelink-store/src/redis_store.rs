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

use async_trait::async_trait;
use bytes::Bytes;
use fred::prelude::*;
use futures::stream::FuturesOrdered;
use futures::TryStreamExt;
use nativelink_error::{error_if, make_err, Code, Error, ResultExt};
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::metrics_utils::{Collector, CollectorState, MetricsComponent, Registry};
use nativelink_util::store_trait::{StoreDriver, StoreKey, UploadSizeInfo};

use crate::cas_utils::is_zero_digest;

const READ_CHUNK_SIZE: isize = 64 * 1024;

trait ClientLikeExt: ClientLike + KeysInterface + TransactionInterface + PubsubInterface {}

impl<C> ClientLikeExt for C where
    C: ClientLike + KeysInterface + TransactionInterface + PubsubInterface
{
}

/// A [`StoreDriver`] implementation that uses Redis as a backing store.
pub struct RedisStore<C = RedisClient, F = fn() -> String> {
    /// The client for the underlying Redis instance(s).
    client: C,

    /// A function used to generate names for temporary keys.
    temp_name_generator_fn: F,
    pub_sub_channel: Option<String>,

    /// A common prefix to append to all keys before they are sent to Redis.
    ///
    /// See [`RedisStore::key_prefix`](`nativelink_config::stores::RedisStore::key_prefix`).
    key_prefix: String,
}

impl RedisStore<RedisClient, fn() -> String> {
    pub fn new(config: &nativelink_config::stores::RedisStore) -> Result<Arc<Self>, Error> {
        let client = Builder::from_config(RedisConfig::from_url(&config.url)?).build()?;
        client.connect();

        Ok(Arc::new(
            RedisStore::new_with_client_and_name_generator_and_prefix(
                client,
                || uuid::Uuid::new_v4().to_string(),
                config.experimental_pub_sub_channel.clone(),
                config.key_prefix.clone(),
            ),
        ))
    }
}

impl<C: ClientLike, F: Fn() -> String> RedisStore<C, F> {
    #[inline]
    pub fn new_with_client_and_name_generator(client: C, temp_name_generator_fn: F) -> Self {
        RedisStore::new_with_client_and_name_generator_and_prefix(
            client,
            temp_name_generator_fn,
            None,
            String::new(),
        )
    }

    #[inline]
    pub fn new_with_client_and_name_generator_and_prefix(
        client: C,
        temp_name_generator_fn: F,
        pub_sub_channel: Option<String>,
        key_prefix: String,
    ) -> Self {
        RedisStore {
            client,
            temp_name_generator_fn,
            pub_sub_channel,
            key_prefix,
        }
    }

    #[inline]
    pub async fn client(&self) -> Result<C, Error> {
        if self.client.is_connected() {
            Ok(self.client.clone())
        } else {
            self.client.wait_for_connect().await?;
            self.client().await
        }
    }

    /// Encode a [`StoreKey`] so it can be sent to Redis.
    fn encode_key(&self, key: StoreKey<'_>) -> String {
        let key_body = key.as_str();
        if self.key_prefix.is_empty() {
            key_body.into_owned()
        } else {
            let mut encoded_key = String::with_capacity(self.key_prefix.len() + key_body.len());
            encoded_key.push_str(&self.key_prefix);
            encoded_key.push_str(&key_body);
            encoded_key
        }
    }
}

#[async_trait]
impl<C, F> StoreDriver for RedisStore<C, F>
where
    C: ClientLikeExt + Unpin + 'static,
    F: Fn() -> String + Unpin + Send + Sync + 'static,
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
                        let client = self.client().await.err_tip(|| {
                            "Error: Could not get connection handle in has_with_results"
                        })?;

                        KeysInterface::strlen::<usize, _>(&client, encoded_key)
                            .await
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

        let client = self.client().await?;
        let trx = client.multi();

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
                        trx.append(&final_key, &chunk[..])
                            .await
                            .err_tip(|| "In RedisStore::update() single chunk")?;
                    }

                    break 'outer;
                }

                trx.append(temp_key.get_or_init(make_temp_name), &chunk[..])
                    .await?;

                force_recv = false;

                // Give other tasks a chance to run to populate the reader's
                // buffer if possible.
                tokio::task::yield_now().await;
            }

            trx.exec(false)
                .await
                .err_tip(|| "In RedisStore::update::query_async")?;
        }

        client
            .rename(temp_key.get_or_init(make_temp_name), &final_key)
            .await
            .err_tip(|| "In RedisStore::update")?;

        if let Some(pub_sub_channel) = &self.pub_sub_channel {
            client
                .publish(pub_sub_channel, &final_key)
                .await
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

        let client = self.client().await?;
        if length == Some(0) {
            let exists: bool = client
                .exists(self.encode_key(key.borrow()))
                .await
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

        let mut current_start = offset;
        let max_length = length.unwrap_or(usize::MAX);
        let end_position = current_start.saturating_add(max_length);

        loop {
            // Note: Redis getrange is inclusive, so we need to subtract 1 from the end.
            let current_end = std::cmp::min(
                current_start.saturating_add(READ_CHUNK_SIZE as usize),
                end_position,
            ) - 1;
            let chunk = client
                .getrange::<Bytes, _>(self.encode_key(key.borrow()), current_start, current_end)
                .await
                .err_tip(|| "In RedisStore::get_part::getrange")?;

            if chunk.is_empty() {
                writer
                    .send_eof()
                    .err_tip(|| "Failed to write EOF in redis store get_part")?;
                break;
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
                writer
                    .send_eof()
                    .err_tip(|| "Failed to write EOF in redis store get_part")?;

                break;
            }

            error_if!(
                writer.get_bytes_written() as usize > max_length,
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

impl<C, F> MetricsComponent for RedisStore<C, F>
where
    C: ClientLike,
{
    fn gather_metrics(&self, _c: &mut CollectorState) {}
}

#[async_trait]
impl<C, F> HealthStatusIndicator for RedisStore<C, F>
where
    C: ClientLikeExt + Unpin + 'static,
    F: Fn() -> String + Unpin + Send + Sync + 'static,
{
    fn get_name(&self) -> &'static str {
        "RedisStore"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}
