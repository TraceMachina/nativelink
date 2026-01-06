// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use core::pin::Pin;
use std::sync::Arc;
use std::time::SystemTime;

use futures::stream::FuturesOrdered;
use futures::{Future, TryStreamExt};
use nativelink_config::stores::{ExperimentalCloudObjectSpec, RedisMode, StoreSpec};
use nativelink_error::Error;
use nativelink_util::health_utils::HealthRegistryBuilder;
use nativelink_util::store_trait::{Store, StoreDriver};

use crate::completeness_checking_store::CompletenessCheckingStore;
use crate::compression_store::CompressionStore;
use crate::dedup_store::DedupStore;
use crate::existence_cache_store::ExistenceCacheStore;
use crate::fast_slow_store::FastSlowStore;
use crate::filesystem_store::FilesystemStore;
use crate::gcs_store::GcsStore;
use crate::grpc_store::GrpcStore;
use crate::memory_store::MemoryStore;
use crate::mongo_store::ExperimentalMongoStore;
use crate::noop_store::NoopStore;
use crate::ontap_s3_existence_cache_store::OntapS3ExistenceCache;
use crate::ontap_s3_store::OntapS3Store;
use crate::redis_store::RedisStore;
use crate::ref_store::RefStore;
use crate::s3_store::S3Store;
use crate::shard_store::ShardStore;
use crate::size_partitioning_store::SizePartitioningStore;
use crate::store_manager::StoreManager;
use crate::verify_store::VerifyStore;

type FutureMaybeStore<'a> = Box<dyn Future<Output = Result<Store, Error>> + Send + 'a>;

pub fn store_factory<'a>(
    backend: &'a StoreSpec,
    store_manager: &'a Arc<StoreManager>,
    maybe_health_registry_builder: Option<&'a mut HealthRegistryBuilder>,
) -> Pin<FutureMaybeStore<'a>> {
    Box::pin(async move {
        let store: Arc<dyn StoreDriver> = match backend {
            StoreSpec::Memory(spec) => MemoryStore::new(spec),
            StoreSpec::ExperimentalCloudObjectStore(spec) => match spec {
                ExperimentalCloudObjectSpec::Aws(aws_config) => {
                    S3Store::new(aws_config, SystemTime::now).await?
                }
                ExperimentalCloudObjectSpec::Ontap(ontap_config) => {
                    OntapS3Store::new(ontap_config, SystemTime::now).await?
                }
                ExperimentalCloudObjectSpec::Gcs(gcs_config) => {
                    GcsStore::new(gcs_config, SystemTime::now).await?
                }
            },
            StoreSpec::RedisStore(spec) => {
                if spec.mode == RedisMode::Cluster {
                    RedisStore::new_cluster(spec.clone()).await?
                } else {
                    RedisStore::new_standard(spec.clone()).await?
                }
            }
            StoreSpec::Verify(spec) => VerifyStore::new(
                spec,
                store_factory(&spec.backend, store_manager, None).await?,
            ),
            StoreSpec::Compression(spec) => CompressionStore::new(
                &spec.clone(),
                store_factory(&spec.backend, store_manager, None).await?,
            )?,
            StoreSpec::Dedup(spec) => DedupStore::new(
                spec,
                store_factory(&spec.index_store, store_manager, None).await?,
                store_factory(&spec.content_store, store_manager, None).await?,
            )?,
            StoreSpec::ExistenceCache(spec) => ExistenceCacheStore::new(
                spec,
                store_factory(&spec.backend, store_manager, None).await?,
            ),
            StoreSpec::OntapS3ExistenceCache(spec) => {
                OntapS3ExistenceCache::new(spec, SystemTime::now).await?
            }
            StoreSpec::CompletenessChecking(spec) => CompletenessCheckingStore::new(
                store_factory(&spec.backend, store_manager, None).await?,
                store_factory(&spec.cas_store, store_manager, None).await?,
            ),
            StoreSpec::FastSlow(spec) => FastSlowStore::new(
                spec,
                store_factory(&spec.fast, store_manager, None).await?,
                store_factory(&spec.slow, store_manager, None).await?,
            ),
            StoreSpec::Filesystem(spec) => <FilesystemStore>::new(spec).await?,
            StoreSpec::RefStore(spec) => RefStore::new(spec, Arc::downgrade(store_manager)),
            StoreSpec::SizePartitioning(spec) => SizePartitioningStore::new(
                spec,
                store_factory(&spec.lower_store, store_manager, None).await?,
                store_factory(&spec.upper_store, store_manager, None).await?,
            ),
            StoreSpec::Grpc(spec) => GrpcStore::new(spec).await?,
            StoreSpec::Noop(_) => NoopStore::new(),
            StoreSpec::ExperimentalMongo(spec) => ExperimentalMongoStore::new(spec.clone()).await?,
            StoreSpec::Shard(spec) => {
                let stores = spec
                    .stores
                    .iter()
                    .map(|store_spec| store_factory(&store_spec.store, store_manager, None))
                    .collect::<FuturesOrdered<_>>()
                    .try_collect::<Vec<_>>()
                    .await?;
                ShardStore::new(spec, stores)?
            }
        };

        if let Some(health_registry_builder) = maybe_health_registry_builder {
            store.clone().register_health(health_registry_builder);
        }

        Ok(Store::new(store))
    })
}
