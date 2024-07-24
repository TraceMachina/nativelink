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
use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
use fred::clients::{RedisPool, SubscriberClient};
use fred::prelude::*;
use fred::types::SentinelConfig;
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
use uuid::Uuid;

use crate::cas_utils::is_zero_digest;

const READ_CHUNK_SIZE: isize = 64 * 1024;
const CONNECTION_POOL_SIZE: usize = 3;

#[cfg(feature = "redis-test")]
type Mocks = Arc<dyn fred::mocks::Mocks>;
#[cfg(not(feature = "redis-test"))]
type Mocks = Option<std::convert::Infallible>;

/// A [`StoreDriver`] implementation that uses Redis as a backing store.
pub struct RedisStore {
    /// The client pool connecting to the backing Redis instance(s).
    client_pool: RedisPool,

    /// A dedicated client running in `subscriber` mode
    subscriber: SubscriberClient,

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
        Self::new_with_name_generator_and_mocks(config, || Uuid::new_v4().to_string(), None)
            .map(Arc::new)
    }

    /// Used for testing, when determinism is required
    pub fn new_with_name_generator_and_mocks(
        config: &nativelink_config::stores::RedisStore,
        temp_name_generator_fn: fn() -> String,
        mocks: Mocks,
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
        let redis_config = match config.mode {
            RedisMode::Cluster => RedisConfig::from_url_clustered(addr),
            RedisMode::Sentinel => RedisConfig::from_url_sentinel(addr),
            RedisMode::Standard => RedisConfig::from_url_centralized(addr),
        }
        .err_tip_with_code(|_| (Code::InvalidArgument, "while parsing redis node address"))?;

        #[cfg(feature = "redis-test")]
        {
            redis_config.mocks = mocks;
        }

        #[cfg(not(feature = "redis-test"))]
        {
            debug_assert!(
                mocks.is_none(),
                "mocks should only be enabled during testing"
            )
        }

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
            subscriber,
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
