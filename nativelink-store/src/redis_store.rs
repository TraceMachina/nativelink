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
use nativelink_error::{Code, Error, ResultExt};
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::metrics_utils::{Collector, CollectorState, MetricsComponent, Registry};
use nativelink_util::store_trait::{Store, UploadSizeInfo};
use redis::aio::{ConnectionLike, MultiplexedConnection};
use redis::AsyncCommands;
use tokio::sync::Mutex;

#[async_trait]
pub trait RedisConnectionTrait: Sync + Send + Sized + 'static {
    type ConnType: Send + Sync + Sized + 'static;

    async fn new_with_config(config: &nativelink_config::stores::RedisStore)
        -> Result<Self, Error>;

    async fn new_with_connection(conn: Self::ConnType) -> Result<Self, Error>;
}

#[async_trait]
impl RedisConnectionTrait for MultiplexedConnection {
    type ConnType = MultiplexedConnection;

    async fn new_with_config(
        config: &nativelink_config::stores::RedisStore,
    ) -> Result<Self, Error> {
        let client = redis::Client::open(config.url.clone());
        let connection = client?.get_multiplexed_tokio_connection().await?;
        Ok(connection)
    }

    async fn new_with_connection(_conn: Self::ConnType) -> Result<Self, Error> {
        Err(Error::new(
            Code::Unimplemented,
            "new_with_connection method is not implemented".to_string(),
        ))
    }
}

pub struct RedisStore<T: RedisConnectionTrait + 'static> {
    pub conn: Arc<Mutex<T>>,
}

impl<T: RedisConnectionTrait + 'static> RedisStore<T> {
    pub async fn new(config: &nativelink_config::stores::RedisStore) -> Result<Self, Error> {
        let connection = T::new_with_config(config)
            .await
            .map(|conn| Arc::new(Mutex::new(conn)))?;

        Ok(RedisStore { conn: connection })
    }

    pub async fn new_with_connection(conn: T) -> Result<Self, Error> {
        Ok(RedisStore {
            conn: Arc::new(Mutex::new(conn)),
        })
    }
}

#[async_trait]
impl<T: RedisConnectionTrait + ConnectionLike + 'static> Store for RedisStore<T> {
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
                let conn = Arc::clone(&self.conn);
                let mut conn = conn.lock().await;
                let packed_hash = encode(digest.packed_hash);
                let exists: bool = conn.exists(&packed_hash).await?;
                Ok::<(usize, _), Error>((index, exists))
            })
            .collect();

        let results_vec: Vec<_> = try_join_all(futures).await?;

        for (index, _exists) in results_vec {
            results[index] = Some(0);
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        let size = match size_info {
            UploadSizeInfo::ExactSize(size) => size,
            UploadSizeInfo::MaxSize(size) => size,
        };

        let buffer = reader
            .collect_all_with_size_hint(size)
            .await
            .err_tip(|| "Failed to collect all bytes from reader in redis_store::update")?;

        let conn = Arc::clone(&self.conn);
        let mut conn = conn.lock().await;
        let packed_hash = encode(digest.packed_hash);
        let _: () = conn.set(&packed_hash, &buffer[..]).await?;
        Ok(())
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        let conn = Arc::clone(&self.conn);
        let mut conn = conn.lock().await;
        let packed_hash = encode(digest.packed_hash);
        let value = conn.get::<_, Vec<u8>>(&packed_hash).await?;
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

    fn as_any<'a>(&'a self) -> &'a (dyn std::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn std::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_metrics(self: Arc<Self>, registry: &mut Registry) {
        registry.register_collector(Box::new(Collector::new(&self)));
    }

    fn register_health(self: Arc<Self>, registry: &mut HealthRegistryBuilder) {
        registry.register_indicator(self);
    }
}

impl<T: RedisConnectionTrait + 'static> MetricsComponent for RedisStore<T> {
    fn gather_metrics(&self, _c: &mut CollectorState) {}
}

#[async_trait]
impl<T: RedisConnectionTrait + ConnectionLike + 'static> HealthStatusIndicator for RedisStore<T> {
    fn get_name(&self) -> &'static str {
        "RedisStore"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        Store::check_health(Pin::new(self), namespace).await
    }
}
