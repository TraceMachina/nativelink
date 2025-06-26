// Copyright 2025 The NativeLink Authors. All rights reserved.
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

use core::pin::Pin;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use aws_config::BehaviorVersion;
use aws_config::default_provider::credentials::DefaultCredentialsChain;
use aws_config::provider_config::ProviderConfig;
use aws_sdk_s3::Client;
use aws_sdk_s3::config::Region;
use nativelink_config::stores::{ExperimentalOntapS3Spec, OntapS3ExistenceCacheSpec};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthStatus, HealthStatusIndicator};
use nativelink_util::instant_wrapper::InstantWrapper;
use nativelink_util::metrics_utils::CounterWithTime;
use nativelink_util::spawn;
use nativelink_util::store_trait::{Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo};
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::sync::RwLock;
use tokio::time::{interval, sleep};
use tracing::{Level, event};

use crate::cas_utils::is_zero_digest;
use crate::common_s3_utils::TlsClient;
use crate::ontap_s3_store::OntapS3Store;

#[derive(Serialize, Deserialize)]
struct CacheFile {
    digests: HashSet<DigestInfo>,
    timestamp: u64,
}

#[derive(Debug, MetricsComponent)]
pub struct OntapS3ExistenceCache<I, NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + Clone + 'static,
{
    inner_store: Store,
    s3_client: Arc<Client>,
    now_fn: NowFn,
    bucket: String,
    key_prefix: Option<String>,
    index_path: String,
    digests: Arc<RwLock<HashSet<DigestInfo>>>,
    sync_interval_seconds: u32,
    last_sync: Arc<AtomicU64>,
    sync_in_progress: Arc<AtomicBool>,

    #[metric(help = "Number of cache hits")]
    cache_hits: CounterWithTime,
    #[metric(help = "Number of cache misses")]
    cache_misses: CounterWithTime,
}

impl<I, NowFn> OntapS3ExistenceCache<I, NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + Clone + 'static,
{
    pub fn new_for_testing(
        inner_store: Store,
        s3_client: Arc<Client>,
        cache_file_path: String,
        digests: HashSet<DigestInfo>,
        sync_interval_seconds: u32,
        now_fn: NowFn,
    ) -> Arc<Self> {
        Arc::new(Self {
            inner_store,
            s3_client,
            bucket: "test-bucket".to_string(),
            key_prefix: None,
            index_path: cache_file_path,
            digests: Arc::new(RwLock::new(digests)),
            sync_interval_seconds,
            last_sync: Arc::new(AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
            )),
            sync_in_progress: Arc::new(AtomicBool::new(false)),
            now_fn,
            cache_hits: CounterWithTime::default(),
            cache_misses: CounterWithTime::default(),
        })
    }

    async fn list_objects(&self, test_client: Option<Arc<Client>>) -> Result<Vec<String>, Error> {
        let client = test_client.unwrap_or_else(|| self.s3_client.clone());

        let mut objects = Vec::new();
        let mut continuation_token = None;

        loop {
            let mut request = client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(self.key_prefix.as_deref().unwrap_or(""))
                .max_keys(1000); // Process in smaller batches for better responsiveness

            if let Some(token) = &continuation_token {
                request = request.continuation_token(token);
            }

            // Add a timeout to avoid hanging indefinitely
            let result = match tokio::time::timeout(Duration::from_secs(30), request.send()).await {
                Ok(Ok(response)) => response,
                Ok(Err(e)) => {
                    // Log the error but continue
                    event!(Level::ERROR, ?e, "Error listing objects in ONTAP S3");
                    return Err(make_err!(Code::Internal, "Failed to list objects: {}", e));
                }
                Err(_) => {
                    // Timeout occurred
                    event!(Level::ERROR, "Timeout listing objects in ONTAP S3");
                    return Err(make_err!(Code::Internal, "Timeout listing objects"));
                }
            };

            if let Some(contents) = result.contents {
                for object in contents {
                    if let Some(key) = object.key {
                        objects.push(key);
                    }
                }
            }

            match result.next_continuation_token {
                Some(token) => {
                    continuation_token = Some(token);
                    // Yield to other tasks periodically to avoid monopolizing the runtime
                    tokio::task::yield_now().await;
                }
                None => break,
            }
        }
        event!(Level::INFO, "Listed {} objects from S3", objects.len());
        event!(Level::INFO, "Used prefix: {:?}", self.key_prefix);
        Ok(objects)
    }

    async fn parse_key_to_digest(&self, key: &str) -> Option<DigestInfo> {
        // Remove the prefix if it exists
        let relative_key = key
            .strip_prefix(self.key_prefix.as_deref().unwrap_or(""))
            .unwrap_or(key)
            .trim_start_matches('/');

        // Skip the directory entry itself
        if relative_key.is_empty() {
            return None;
        }

        let parts: Vec<&str> = relative_key.split('-').collect();

        if parts.len() != 2 {
            return None;
        }

        match (hex::decode(parts[0]), parts[1].parse::<u64>()) {
            (Ok(hash), Ok(size)) => {
                if hash.len() == 32 {
                    // Clone the hash before converting
                    let hash_array: [u8; 32] = hash.clone().try_into().unwrap();
                    let digest = DigestInfo::new(hash_array, size);

                    Some(digest)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    pub async fn run_sync(&self) -> Result<(), Error> {
        // Only run if not already in progress
        if self
            .sync_in_progress
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            // Sync already in progress, just return
            return Ok(());
        }

        // Use a finally block equivalent to ensure we clear the flag even if there's an error
        let result = self.sync_cache(None).await;

        // Clear the in-progress flag
        self.sync_in_progress.store(false, Ordering::SeqCst);

        result
    }

    async fn sync_cache(&self, test_client: Option<Arc<Client>>) -> Result<(), Error> {
        let client = test_client.unwrap_or_else(|| self.s3_client.clone());
        let current_time = (self.now_fn)().unix_timestamp();

        // Load existing cache to compare against
        let mut existing_digests = HashSet::new();
        if let Ok(contents) = fs::read_to_string(&self.index_path).await {
            if let Ok(cache_file) = serde_json::from_str::<CacheFile>(&contents) {
                existing_digests = cache_file.digests;
            }
        }

        // Get all objects
        let objects = match self.list_objects(Some(client)).await {
            Ok(objects) => objects,
            Err(e) => {
                event!(Level::ERROR, ?e, "Failed to list objects during cache sync");
                // On error, keep using the existing cache
                return Err(e);
            }
        };

        let mut new_digests = HashSet::with_capacity(objects.len());

        // Process objects in chunks to avoid long processing times
        for chunk in objects.chunks(1000) {
            for obj_key in chunk {
                if let Some(digest) = self.parse_key_to_digest(obj_key).await {
                    new_digests.insert(digest);
                }
            }
            // Yield to other tasks periodically
            tokio::task::yield_now().await;
        }

        // Only write the cache file if something changed
        if new_digests != existing_digests {
            let cache_file = CacheFile {
                digests: new_digests.clone(),
                timestamp: current_time,
            };

            let json = serde_json::to_string(&cache_file)
                .map_err(|e| make_err!(Code::Internal, "Failed to serialize cache: {}", e))?;

            fs::write(&self.index_path, json)
                .await
                .map_err(|e| make_err!(Code::Internal, "Failed to write cache file: {}", e))?;
        }

        // Update the in-memory cache - this can be done regardless of file write status
        *self.digests.write().await = new_digests;
        self.last_sync.store(current_time, Ordering::SeqCst);

        Ok(())
    }

    pub async fn new(spec: &OntapS3ExistenceCacheSpec, now_fn: NowFn) -> Result<Arc<Self>, Error> {
        let inner_spec = &spec.backend;
        let inner_store = Arc::new(OntapS3Store::new(inner_spec, now_fn.clone()).await?);
        let inner_store = Store::new((*inner_store).clone());
        let s3_client = create_s3_client(inner_spec).await?;

        let digests = Arc::new(RwLock::new(HashSet::new()));
        let last_sync = Arc::new(AtomicU64::new(0));
        let sync_in_progress = Arc::new(AtomicBool::new(false));

        // Create the cache instance
        let cache = Arc::new(Self {
            inner_store,
            s3_client: Arc::new(s3_client),
            bucket: inner_spec.bucket.clone(),
            key_prefix: inner_spec.common.key_prefix.clone(),
            index_path: spec.index_path.clone(),
            digests: digests.clone(),
            sync_interval_seconds: spec.sync_interval_seconds,
            last_sync: last_sync.clone(),
            now_fn,
            sync_in_progress,
            cache_hits: CounterWithTime::default(),
            cache_misses: CounterWithTime::default(),
        });

        // Try to load existing cache file
        if let Ok(contents) = fs::read_to_string(&spec.index_path).await {
            let cache_file = serde_json::from_str::<CacheFile>(&contents)
                .map_err(|e| make_err!(Code::Internal, "Failed to parse cache file: {}", e))?;
            *cache.digests.write().await = cache_file.digests;
            cache
                .last_sync
                .store(cache_file.timestamp, Ordering::SeqCst);
        }

        // Start the background sync task
        let cache_clone = cache.clone();
        let _handle = spawn!("ontap_s3_existence_cache_sync", async move {
            // Wait a bit before starting the sync to allow the service to initialize
            sleep(Duration::from_secs(5)).await;

            // Run an initial sync if the cache is empty
            if cache_clone.digests.read().await.is_empty() {
                event!(Level::INFO, "Running initial cache sync");
                if let Err(e) = cache_clone.run_sync().await {
                    event!(Level::ERROR, ?e, "Initial cache sync error");
                }
            }

            // Set up periodic background sync
            let mut interval = interval(Duration::from_secs(u64::from(
                cache_clone.sync_interval_seconds,
            )));
            loop {
                interval.tick().await;
                event!(Level::INFO, "Starting periodic cache sync");
                if let Err(e) = cache_clone.run_sync().await {
                    event!(Level::ERROR, ?e, "Background sync error");
                } else {
                    event!(Level::INFO, "Completed periodic cache sync");
                }
            }
        });

        Ok(cache)
    }
}

async fn create_s3_client(spec: &ExperimentalOntapS3Spec) -> Result<Client, Error> {
    let http_client = TlsClient::new(&spec.common);
    let credentials_provider = DefaultCredentialsChain::builder()
        .configure(
            ProviderConfig::without_region()
                .with_region(Some(Region::new(Cow::Owned(spec.vserver_name.clone()))))
                .with_http_client(http_client.clone()),
        )
        .build()
        .await;

    let config = aws_sdk_s3::Config::builder()
        .credentials_provider(credentials_provider)
        .endpoint_url(&spec.endpoint)
        .region(Region::new(spec.vserver_name.clone()))
        .force_path_style(true)
        .behavior_version(BehaviorVersion::v2025_01_17())
        .build();

    Ok(Client::from_conf(config))
}

#[async_trait]
impl<I, NowFn> StoreDriver for OntapS3ExistenceCache<I, NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + Clone + 'static,
{
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        let cache = self.digests.read().await;

        for (key, result) in keys.iter().zip(results.iter_mut()) {
            // Handle zero digest case
            if is_zero_digest(key.borrow()) {
                *result = Some(0);
                self.cache_hits.inc();
                continue;
            }

            let digest = key.borrow().into_digest();

            // If not in cache, definitely doesn't exist
            if !cache.contains(&digest) {
                *result = None;
                self.cache_misses.inc();
                continue;
            }
            // If in cache, check actual store
            match self.inner_store.has(key.borrow()).await {
                Ok(size) => {
                    *result = size;
                    self.cache_hits.inc();
                }
                Err(e) => {
                    return Err(e);
                }
            }
        }

        Ok(())
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        if is_zero_digest(key.borrow()) {
            writer.send_eof().err_tip(|| "Failed to send zero EOF")?;
            return Ok(());
        }

        // Only pass through if in cache
        if !self
            .digests
            .read()
            .await
            .contains(&key.borrow().into_digest())
        {
            return Err(make_err!(Code::NotFound, "Object not in cache"));
        }

        self.inner_store.get_part(key, writer, offset, length).await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        let key_owned = key.into_owned();
        let result = self
            .inner_store
            .update(key_owned.clone(), reader, size_info)
            .await;
        if result.is_ok() {
            self.digests.write().await.insert(key_owned.into_digest());
        }
        result
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any(&self) -> &(dyn core::any::Any + Send + Sync + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Send + Sync + 'static> {
        self
    }
}

#[async_trait]
impl<I, NowFn> HealthStatusIndicator for OntapS3ExistenceCache<I, NowFn>
where
    I: InstantWrapper,
    NowFn: Fn() -> I + Send + Sync + Unpin + Clone + 'static,
{
    fn get_name(&self) -> &'static str {
        "OntapS3ExistenceCache"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}
