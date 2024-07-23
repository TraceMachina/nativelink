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
use tokio::task::JoinHandle;

use crate::cas_utils::is_zero_digest;

const READ_CHUNK_SIZE: isize = 64 * 1024;

/// A [`StoreDriver`] implementation that uses Redis as a backing store.
pub struct RedisStore {
    // /// The connection to the underlying Redis instance(s).
    // TODO connection: ?,
    /// A function used to generate names for temporary keys.
    temp_name_generator_fn: fn() -> String,
    pub_sub_channel: Option<String>,

    /// A common prefix to append to all keys before they are sent to Redis.
    ///
    /// See [`RedisStore::key_prefix`](`nativelink_config::stores::RedisStore::key_prefix`).
    key_prefix: String,
}

impl RedisStore {
    pub fn new(config: &nativelink_config::stores::RedisStore) -> Result<Arc<Self>, Error> {
        todo!()
    }
}

impl RedisStore {
    #[inline]
    pub fn new_with_conn_and_name_generator(
        connection: (), // TODO
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
        connection: (), // TODO
        temp_name_generator_fn: fn() -> String,
        pub_sub_channel: Option<String>,
        key_prefix: String,
    ) -> Self {
        RedisStore {
            // connection,
            temp_name_generator_fn,
            pub_sub_channel,
            key_prefix,
        }
    }

    /// Encode a [`StoreKey`] so it can be sent to Redis.
    fn encode_key<'a>(&self, key: &'a StoreKey<'a>) -> Cow<'a, str> {
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
        if self.key_prefix.is_empty() {
            key.as_str()
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
        todo!()
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        _upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        todo!()
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        todo!()
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

fn from_redis_err(call_res: fred::error::RedisError) -> Error {
    make_err!(Code::Internal, "Redis Error: {call_res}")
}
