// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

use compression_store::CompressionStore;
use config::{self, stores::StoreConfig};
use dedup_store::DedupStore;
use error::Error;
use fast_slow_store::FastSlowStore;
use filesystem_store::FilesystemStore;
use futures::Future;
use grpc_store::GrpcStore;
use memory_store::MemoryStore;
use ref_store::RefStore;
use s3_store::S3Store;
use size_partitioning_store::SizePartitioningStore;
use store::{Store, StoreManager};
use verify_store::VerifyStore;

// TODO: Ugly.
type StoreFactoryOutput = Result<Arc<dyn Store>, Error>;

pub fn store_factory<'a>(
    backend: &'a StoreConfig,
    store_manager: &'a Arc<StoreManager>,
) -> Pin<Box<dyn Future<Output = StoreFactoryOutput> + 'a>> {
    Box::pin(async move {
        let store: Arc<dyn Store> = match backend {
            StoreConfig::memory(config) => Arc::new(MemoryStore::new(config)),
            StoreConfig::s3_store(config) => Arc::new(S3Store::new(config)?),
            StoreConfig::verify(config) => Arc::new(VerifyStore::new(
                config,
                store_factory(&config.backend, store_manager).await?,
            )),
            StoreConfig::compression(config) => Arc::new(CompressionStore::new(
                *config.clone(),
                store_factory(&config.backend, store_manager).await?,
            )?),
            StoreConfig::dedup(config) => Arc::new(DedupStore::new(
                config,
                store_factory(&config.index_store, store_manager).await?,
                store_factory(&config.content_store, store_manager).await?,
            )),
            StoreConfig::fast_slow(config) => Arc::new(FastSlowStore::new(
                config,
                store_factory(&config.fast, store_manager).await?,
                store_factory(&config.slow, store_manager).await?,
            )),
            StoreConfig::filesystem(config) => Arc::new(<FilesystemStore>::new(config).await?),
            StoreConfig::ref_store(config) => Arc::new(RefStore::new(config, store_manager.clone())),
            StoreConfig::size_partitioning(config) => Arc::new(SizePartitioningStore::new(
                config,
                store_factory(&config.lower_store, store_manager).await?,
                store_factory(&config.upper_store, store_manager).await?,
            )),
            StoreConfig::grpc(config) => Arc::new(GrpcStore::new(config).await?),
        };
        Ok(store)
    })
}
