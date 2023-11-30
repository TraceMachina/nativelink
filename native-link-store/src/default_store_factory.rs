// Copyright 2023 The Native Link Authors. All rights reserved.
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

use error::Error;
use futures::stream::FuturesOrdered;
use futures::{Future, TryStreamExt};
use native_link_config::stores::StoreConfig;
use native_link_util::metrics_utils::Registry;
use native_link_util::store_trait::Store;

use crate::compression_store::CompressionStore;
use crate::dedup_store::DedupStore;
use crate::existence_store::ExistenceStore;
use crate::fast_slow_store::FastSlowStore;
use crate::filesystem_store::FilesystemStore;
use crate::grpc_store::GrpcStore;
use crate::memory_store::MemoryStore;
use crate::noop_store::NoopStore;
use crate::ref_store::RefStore;
use crate::s3_store::S3Store;
use crate::shard_store::ShardStore;
use crate::size_partitioning_store::SizePartitioningStore;
use crate::store_manager::StoreManager;
use crate::verify_store::VerifyStore;

type FutureMaybeStore<'a> = Box<dyn Future<Output = Result<Arc<dyn Store>, Error>> + 'a>;

pub fn store_factory<'a>(
    backend: &'a StoreConfig,
    store_manager: &'a Arc<StoreManager>,
    maybe_store_metrics: Option<&'a mut Registry>,
) -> Pin<FutureMaybeStore<'a>> {
    Box::pin(async move {
        let store: Arc<dyn Store> = match backend {
            StoreConfig::memory(config) => Arc::new(MemoryStore::new(config)),
            StoreConfig::s3_store(config) => Arc::new(S3Store::new(config).await?),
            StoreConfig::verify(config) => Arc::new(VerifyStore::new(
                config,
                store_factory(&config.backend, store_manager, None).await?,
            )),
            StoreConfig::compression(config) => Arc::new(CompressionStore::new(
                *config.clone(),
                store_factory(&config.backend, store_manager, None).await?,
            )?),
            StoreConfig::dedup(config) => Arc::new(DedupStore::new(
                config,
                store_factory(&config.index_store, store_manager, None).await?,
                store_factory(&config.content_store, store_manager, None).await?,
            )),
            StoreConfig::existence_store(config) => Arc::new(ExistenceStore::new(
                config,
                store_factory(&config.inner, store_manager, None).await?,
            )),
            StoreConfig::fast_slow(config) => Arc::new(FastSlowStore::new(
                config,
                store_factory(&config.fast, store_manager, None).await?,
                store_factory(&config.slow, store_manager, None).await?,
            )),
            StoreConfig::filesystem(config) => Arc::new(<FilesystemStore>::new(config).await?),
            StoreConfig::ref_store(config) => Arc::new(RefStore::new(config, Arc::downgrade(store_manager))),
            StoreConfig::size_partitioning(config) => Arc::new(SizePartitioningStore::new(
                config,
                store_factory(&config.lower_store, store_manager, None).await?,
                store_factory(&config.upper_store, store_manager, None).await?,
            )),
            StoreConfig::grpc(config) => Arc::new(GrpcStore::new(config).await?),
            StoreConfig::noop => Arc::new(NoopStore::new()),
            StoreConfig::shard(config) => {
                let stores = config
                    .stores
                    .iter()
                    .map(|store_config| store_factory(&store_config.store, store_manager, None))
                    .collect::<FuturesOrdered<_>>()
                    .try_collect::<Vec<_>>()
                    .await?;
                Arc::new(ShardStore::new(config, stores)?)
            }
        };
        if let Some(store_metrics) = maybe_store_metrics {
            store.clone().register_metrics(store_metrics);
        }
        Ok(store)
    })
}
