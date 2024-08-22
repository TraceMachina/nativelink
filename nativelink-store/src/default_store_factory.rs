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

use std::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use futures::stream::FuturesOrdered;
use futures::{Future, TryStreamExt};
use nativelink_config::stores::StoreConfig;
use nativelink_error::Error;
use nativelink_util::health_utils::HealthRegistryBuilder;
use nativelink_util::store_trait::{Store, StoreDriver};

use crate::completeness_checking_store::CompletenessCheckingStore;
use crate::compression_store::CompressionStore;
use crate::dedup_store::DedupStore;
use crate::existence_cache_store::ExistenceCacheStore;
use crate::fast_slow_store::FastSlowStore;
use crate::filesystem_store::FilesystemStore;
use crate::grpc_store::GrpcStore;
use crate::memory_store::MemoryStore;
use crate::noop_store::NoopStore;
use crate::redis_store::RedisStore;
use crate::ref_store::RefStore;
use crate::s3_store::S3Store;
use crate::shard_store::ShardStore;
use crate::size_partitioning_store::SizePartitioningStore;
use crate::store_manager::StoreManager;
use crate::verify_store::VerifyStore;

type FutureMaybeStore<'a> = Box<dyn Future<Output = Result<Store, Error>> + 'a>;

pub fn store_factory<'a>(
    backend: &'a StoreConfig,
    store_manager: &'a Arc<StoreManager>,
    maybe_health_registry_builder: Option<&'a mut HealthRegistryBuilder>,
) -> Pin<FutureMaybeStore<'a>> {
    Box::pin(async move {
        let store: Arc<dyn StoreDriver> = match backend {
            StoreConfig::Memory(config) => MemoryStore::new(config),
            StoreConfig::ExperimentalS3Store(config) => {
                S3Store::new(config, SystemTime::now).await?
            }
            StoreConfig::Redis(config) => RedisStore::new(config)?,
            StoreConfig::Verify(config) => VerifyStore::new(
                config,
                store_factory(&config.backend, store_manager, None).await?,
            ),
            StoreConfig::Compression(config) => CompressionStore::new(
                *config.clone(),
                store_factory(&config.backend, store_manager, None).await?,
            )?,
            StoreConfig::Dedup(config) => DedupStore::new(
                config,
                store_factory(&config.index_store, store_manager, None).await?,
                store_factory(&config.content_store, store_manager, None).await?,
            ),
            StoreConfig::ExistenceCache(config) => ExistenceCacheStore::new(
                config,
                store_factory(&config.backend, store_manager, None).await?,
            ),
            StoreConfig::CompletenessChecking(config) => CompletenessCheckingStore::new(
                store_factory(&config.backend, store_manager, None).await?,
                store_factory(&config.cas_store, store_manager, None).await?,
            ),
            StoreConfig::FastSlow(config) => FastSlowStore::new(
                config,
                store_factory(&config.fast, store_manager, None).await?,
                store_factory(&config.slow, store_manager, None).await?,
            ),
            StoreConfig::Filesystem(config) => <FilesystemStore>::new(config).await?,
            StoreConfig::Ref(config) => RefStore::new(config, Arc::downgrade(store_manager)),
            StoreConfig::SizePartitioning(config) => SizePartitioningStore::new(
                config,
                store_factory(&config.lower_store, store_manager, None).await?,
                store_factory(&config.upper_store, store_manager, None).await?,
            ),
            StoreConfig::Grpc(config) => GrpcStore::new(config).await?,
            StoreConfig::Noop => NoopStore::new(),
            StoreConfig::Shard(config) => {
                let stores = config
                    .stores
                    .iter()
                    .map(|store_config| store_factory(&store_config.store, store_manager, None))
                    .collect::<FuturesOrdered<_>>()
                    .try_collect::<Vec<_>>()
                    .await?;
                ShardStore::new(config, stores)?
            }
        };

        if let Some(health_registry_builder) = maybe_health_registry_builder {
            store.clone().register_health(health_registry_builder);
        }

        Ok(Store::new(store))
    })
}
