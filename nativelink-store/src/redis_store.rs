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
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
// use futures::future::try_join_all;
use nativelink_error::{error_if, make_err, Code, Error, ResultExt};
use nativelink_util::buf_channel::{
    make_buf_channel_pair, DropCloserReadHalf, DropCloserWriteHalf,
};
use nativelink_util::common::{DigestInfo, JoinHandleDropGuard};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::metrics_utils::{Collector, CollectorState, MetricsComponent, Registry};
use nativelink_util::store_trait::{Store, UploadSizeInfo};
use redis::aio::{ConnectionLike, MultiplexedConnection};
use redis::AsyncCommands;

use crate::cas_utils::is_zero_digest;

fn digest_to_key(digest: &DigestInfo) -> String {
    format!("{}-{}", digest.hash_str(), digest.size_bytes)
}

pub struct RedisStore<T: ConnectionLike + Unpin + Clone + Send + Sync = MultiplexedConnection> {
    conn: T,
    temp_name_generator_fn: fn() -> String,
}

impl RedisStore {
    pub async fn new(
        config: &nativelink_config::stores::RedisStore,
    ) -> Result<RedisStore<MultiplexedConnection>, Error> {
        // Note: Currently only one connection is supported.
        error_if!(
            config.addresses.len() != 1,
            "Only one address is supported for Redis store"
        );

        let response_timeout_s = if config.response_timeout_s == 0 {
            10
        } else {
            config.response_timeout_s
        };
        let connection_timeout_s = if config.connection_timeout_s == 0 {
            10
        } else {
            config.connection_timeout_s
        };

        let conn = redis::Client::open(config.addresses[0].clone())
            .map_err(handle_redis_error)?
            .get_multiplexed_tokio_connection_with_response_timeouts(
                Duration::from_secs(response_timeout_s),
                Duration::from_secs(connection_timeout_s),
            )
            .await
            .map_err(handle_redis_error)?;

        Ok(RedisStore {
            conn,
            temp_name_generator_fn: || uuid::Uuid::new_v4().to_string(),
        })
    }
}

impl<T: ConnectionLike + Unpin + Clone + Send + Sync> RedisStore<T> {
    pub fn new_with_conn_and_name_generator(
        conn: T,
        temp_name_generator_fn: fn() -> String,
    ) -> Result<RedisStore<T>, Error> {
        Ok(RedisStore {
            conn,
            temp_name_generator_fn: temp_name_generator_fn,
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

        let mut conn = self.conn.clone();

        let mut pipe = redis::pipe();
        pipe.atomic();

        digests.iter().for_each(|digest| {
            pipe.exists(digest_to_key(digest));
        });

        let digest_sizes = pipe
            .query_async::<_, Vec<usize>>(&mut conn)
            .await
            .map_err(handle_redis_error)?;

        error_if!(digest_sizes.len() != results.len(), "Mismatch in digest sizes and results length");

        digest_sizes.into_iter().zip(results.iter_mut()).for_each(|(size, result)| {
            *result = if size == 0 { None } else { Some(size) };
        });

        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        mut reader: DropCloserReadHalf,
        _upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        let mut conn = self.conn.clone();

        let temp_key = format!("temp-{}", (self.temp_name_generator_fn)());

        let (mut tx, mut rx) = make_buf_channel_pair();

        let mut pipe = redis::pipe();
        pipe.atomic();

        let _drop_guard = JoinHandleDropGuard::new(tokio::spawn(async move {
            loop {
                let buf_chunk = rx
                    .recv()
                    .await
                    .err_tip(|| "Failed to reach chunk in update in redis store")
                    .unwrap_or_else(|_| Bytes::new());

                if buf_chunk.is_empty() {
                    pipe.cmd("RENAME").arg(&temp_key).arg(format!(
                        "{}-{}",
                        digest.hash_str(),
                        digest.size_bytes
                    ));

                    let _ = pipe
                        .query_async::<_, ()>(&mut conn)
                        .await
                        .map_err(handle_redis_error);
                    break;
                }

                pipe.cmd("APPEND").arg(&temp_key).arg(&buf_chunk[..]);
            }
        }));

        loop {
            tx.bind(&mut reader)
                .await
                .err_tip(|| "Failed to bind reader to buffer in update in redis store")?;
            let chunk = reader
                .recv()
                .await
                .err_tip(|| "Failed to reach chunk in update in redis store")?;

            match chunk.is_empty() {
                true => {
                    tx.send_eof()
                        .await
                        .expect("Failed empty to write chunk to buffer");
                    break;
                }
                false => tx
                    .send(chunk)
                    .await
                    .expect("Failed empty to write chunk to buffer"),
            }
        }

        Ok(())
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        mut offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        if is_zero_digest(&digest) {
            writer
                .send_eof()
                .await
                .err_tip(|| "Failed to send zero EOF in redis store get_part_ref")?;
            return Ok(());
        }

        let mut conn = self.conn.clone();
        let exists = conn
            .exists::<_, bool>(digest_to_key(&digest))
            .await
            .map_err(handle_redis_error)?;
        if !exists {
            return Err(make_err!(
                Code::NotFound,
                "Data not found in Redis store for digest: {}",
                digest_to_key(&digest)
            ));
        }

        const OFFSET_CHUNK: usize = 65535; // 64K chunks
        let end_point: usize = offset + length.unwrap_or(usize::MAX - offset);
        loop {
            let inc = if offset + OFFSET_CHUNK > end_point {
                end_point
            } else {
                offset + OFFSET_CHUNK
            };

            let value = conn
                .getrange::<_, Bytes>(digest.hash_str(), offset as isize, inc as isize)
                .await
                .map_err(handle_redis_error)?;

            if value.is_empty() {
                writer
                    .send_eof()
                    .await
                    .err_tip(|| "Failed to write EOF in redis store get_part")?;
                break;
            }

            let chunk_length = inc - offset;
            let slice_end = std::cmp::min(value.len(), chunk_length);
            writer
                .send(value.slice(offset..offset + slice_end))
                .await
                .err_tip(|| "Failed to write data in Redis store")?;

            offset += OFFSET_CHUNK;

            if offset >= end_point {
                writer
                    .send_eof()
                    .await
                    .err_tip(|| "Failed to write EOF in redis store get_part")?;
                break;
            }
        }

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
