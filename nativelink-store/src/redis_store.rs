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
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::future::try_join_all;
use hex::encode;
use nativelink_error::{make_err, Code, Error, ResultExt};
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::metrics_utils::{Collector, CollectorState, MetricsComponent, Registry};
use nativelink_util::store_trait::{Store, UploadSizeInfo};
use redis::aio::{ConnectionLike, MultiplexedConnection};
use redis::AsyncCommands;

pub struct RedisStore<T: ConnectionLike + Unpin + Clone + Send + Sync = MultiplexedConnection> {
    pub conn: T,
    pub temp_name_generator: fn() -> uuid::Uuid,
}

impl<T: ConnectionLike + Unpin + Clone + Send + Sync + 'static> RedisStore<T> {
    pub async fn new(
        config: &nativelink_config::stores::RedisStore,
        temp_name_generator: fn() -> uuid::Uuid,
    ) -> Result<RedisStore<MultiplexedConnection>, Error> {
        // Note: Currently only one connection is supported, so use the first address in the list.
        let client = redis::Client::open(config.address[0].clone());

        let client_res: Result<redis::Client, Error> = match client {
            Ok(client) => Ok(client),
            Err(redis_error) => Err(handle_redis_error(redis_error)),
        };

        let conn = client_res
            .err_tip(|| "Failed to create Redis client")?
            .get_multiplexed_tokio_connection_with_response_timeouts(
                std::time::Duration::from_secs(config.response_timeout),
                std::time::Duration::from_secs(config.connection_timeout),
            )
            .await
            .map_err(handle_redis_error)?;

        Ok(RedisStore {
            conn,
            temp_name_generator,
        })
    }
}

#[async_trait]
impl<T: ConnectionLike + Unpin + Clone + Send + Sync + 'static> Store for RedisStore<T> {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        // TODO (Blake): Do not collect use buffer_unordered(SOME_N_LIMIT)
        let futures: Vec<_> = digests
            .iter()
            .enumerate()
            .map(|(index, digest)| async move {
                let mut conn = self.conn.clone();
                let packed_hash = encode(digest.packed_hash);
                let exists_call = conn.exists(&packed_hash).await;

                let exists = match exists_call {
                    Ok(()) => Ok(()),
                    Err(redis_error) => Err(handle_redis_error(redis_error)),
                };

                Ok::<(usize, _), Error>((index, exists))
            })
            .collect();

        let results_vec: Vec<_> = try_join_all(futures)
            .await
            .err_tip(|| "Failed to check if data exists in Redis store")?;

        for (index, exists) in results_vec {
            if exists.is_ok() {
                results[index] = Some(0);
            } else {
                results[index] = None;
            }
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        mut reader: DropCloserReadHalf,
        _upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        let mut conn = self.conn.clone();

        let temp_key = format!("temp-{}", (self.temp_name_generator)());

        loop {
            let chunk = reader
                .recv()
                .await
                .err_tip(|| "Failed to reach chunk in update in redis store")?;
            conn.append(&temp_key, &chunk[..])
                .await
                .map_err(handle_redis_error)?;

            if chunk.is_empty() {
                break;
            }
        }

        let packed_hash = encode(digest.packed_hash);

        conn.rename(&temp_key, &packed_hash)
            .await
            .map_err(handle_redis_error)?;

        Ok(())
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        let mut conn = self.conn.clone();
        let packed_hash = encode(digest.packed_hash);
        let get_call = conn.get::<_, Vec<u8>>(&packed_hash).await;

        let value_res: Result<Vec<u8>, Error> = match get_call {
            Ok(value) => Ok(value),
            Err(redis_error) => Err(handle_redis_error(redis_error)),
        };

        let value = value_res.err_tip(|| "Failed to get data from Redis store")?;

        let default_len = value.len() - offset;
        let bytes_wrapper = Bytes::from(value);
        let length = length.unwrap_or(default_len).min(default_len);
        if length > 0 {
            writer
                .send(bytes_wrapper.slice(offset..(offset + length)))
                .await
                .err_tip(|| "Failed to write data in Redis store")?;
        }
        writer
            .send_eof()
            .await
            .err_tip(|| "Failed to write EOF in redis store get_part")?;
        Ok(())
    }

    fn inner_store(&self, _digest: Option<DigestInfo>) -> &'_ dyn Store {
        self
    }

    fn inner_store_arc(self: Arc<Self>, _digest: Option<DigestInfo>) -> Arc<dyn Store> {
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
        Store::check_health(Pin::new(self), namespace).await
    }
}

fn handle_redis_error(call_res: redis::RedisError) -> Error {
    make_err!(Code::Internal, "Redis Error: {call_res}")
}
