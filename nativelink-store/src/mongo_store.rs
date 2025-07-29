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

use core::cmp;
use core::ops::Bound;
use core::pin::Pin;
use core::time::Duration;
use std::borrow::Cow;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use bytes::Bytes;
use futures::stream::{Stream, StreamExt, TryStreamExt};
use mongodb::bson::{Bson, Document, doc};
use mongodb::options::{ClientOptions, FindOptions, IndexOptions, ReturnDocument, WriteConcern};
use mongodb::{Client as MongoClient, Collection, Database, IndexModel};
use nativelink_config::stores::ExperimentalMongoSpec;
use nativelink_error::{Code, Error, make_err, make_input_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::spawn;
use nativelink_util::store_trait::{
    BoolValue, SchedulerCurrentVersionProvider, SchedulerIndexProvider, SchedulerStore,
    SchedulerStoreDataProvider, SchedulerStoreDecodeTo, SchedulerStoreKeyProvider,
    SchedulerSubscription, SchedulerSubscriptionManager, StoreDriver, StoreKey, UploadSizeInfo,
};
use nativelink_util::task::JoinHandleDropGuard;
use parking_lot::{Mutex, RwLock};
use patricia_tree::StringPatriciaMap;
use tokio::sync::watch;
use tokio::time::sleep;
use tracing::{error, info, warn};

use crate::cas_utils::is_zero_digest;

/// The default database name if not specified.
const DEFAULT_DB_NAME: &str = "nativelink";

/// The default collection name if not specified.
const DEFAULT_COLLECTION_NAME: &str = "cas";

/// The default scheduler collection name if not specified.
const DEFAULT_SCHEDULER_COLLECTION_NAME: &str = "scheduler";

/// The default size of the read chunk when reading data from `MongoDB`.
const DEFAULT_READ_CHUNK_SIZE: usize = 64 * 1024;

/// The default connection timeout in milliseconds if not specified.
const DEFAULT_CONNECTION_TIMEOUT_MS: u64 = 3000;

/// The default command timeout in milliseconds if not specified.
const DEFAULT_COMMAND_TIMEOUT_MS: u64 = 10_000;

/// The default maximum number of concurrent uploads.
const DEFAULT_MAX_CONCURRENT_UPLOADS: usize = 10;

/// The name of the field in `MongoDB` documents that stores the key.
const KEY_FIELD: &str = "_id";

/// The name of the field in `MongoDB` documents that stores the data.
const DATA_FIELD: &str = "data";

/// The name of the field in `MongoDB` documents that stores the version.
const VERSION_FIELD: &str = "version";

/// The name of the field in `MongoDB` documents that stores the size.
const SIZE_FIELD: &str = "size";

/// A [`StoreDriver`] implementation that uses `MongoDB` as a backing store.
#[derive(Debug, MetricsComponent)]
pub struct ExperimentalMongoStore {
    /// The `MongoDB` client.
    #[allow(dead_code)]
    client: MongoClient,

    /// The database to use.
    database: Database,

    /// The collection for CAS data.
    cas_collection: Collection<Document>,

    /// The collection for scheduler data.
    scheduler_collection: Collection<Document>,

    /// A common prefix to append to all keys before they are sent to `MongoDB`.
    #[metric(help = "Prefix to append to all keys before sending to MongoDB")]
    key_prefix: String,

    /// The amount of data to read from `MongoDB` at a time.
    #[metric(help = "The amount of data to read from MongoDB at a time")]
    read_chunk_size: usize,

    /// The maximum number of concurrent uploads.
    #[metric(help = "The maximum number of concurrent uploads")]
    max_concurrent_uploads: usize,

    /// Enable change streams for real-time updates.
    #[metric(help = "Whether change streams are enabled")]
    enable_change_streams: bool,

    /// A manager for subscriptions to keys in `MongoDB`.
    subscription_manager: Mutex<Option<Arc<ExperimentalMongoSubscriptionManager>>>,
}

impl ExperimentalMongoStore {
    /// Create a new `ExperimentalMongoStore` from the given configuration.
    pub async fn new(mut spec: ExperimentalMongoSpec) -> Result<Arc<Self>, Error> {
        // Set defaults
        if spec.connection_string.is_empty() {
            return Err(make_err!(
                Code::InvalidArgument,
                "No connection string was specified in mongo store configuration."
            ));
        }

        if spec.database.is_empty() {
            spec.database = DEFAULT_DB_NAME.to_string();
        }

        if spec.cas_collection.is_empty() {
            spec.cas_collection = DEFAULT_COLLECTION_NAME.to_string();
        }

        if spec.scheduler_collection.is_empty() {
            spec.scheduler_collection = DEFAULT_SCHEDULER_COLLECTION_NAME.to_string();
        }

        if spec.read_chunk_size == 0 {
            spec.read_chunk_size = DEFAULT_READ_CHUNK_SIZE;
        }

        if spec.connection_timeout_ms == 0 {
            spec.connection_timeout_ms = DEFAULT_CONNECTION_TIMEOUT_MS;
        }

        if spec.command_timeout_ms == 0 {
            spec.command_timeout_ms = DEFAULT_COMMAND_TIMEOUT_MS;
        }

        if spec.max_concurrent_uploads == 0 {
            spec.max_concurrent_uploads = DEFAULT_MAX_CONCURRENT_UPLOADS;
        }

        // Configure client options
        let mut client_options = ClientOptions::parse(&spec.connection_string)
            .await
            .map_err(|e| {
                make_err!(
                    Code::InvalidArgument,
                    "Failed to parse MongoDB connection string: {e}"
                )
            })?;

        client_options.server_selection_timeout =
            Some(Duration::from_millis(spec.connection_timeout_ms));
        client_options.connect_timeout = Some(Duration::from_millis(spec.connection_timeout_ms));

        // Set write concern if specified
        if let Some(w) = spec.write_concern_w {
            // Parse write concern w - can be a number or string like "majority"
            let w_value = if let Ok(num) = w.parse::<u32>() {
                Some(num.into())
            } else {
                Some(w.into())
            };

            // Build write concern based on which options are set
            let write_concern = match (w_value, spec.write_concern_j, spec.write_concern_timeout_ms)
            {
                (Some(w), Some(j), Some(timeout)) => WriteConcern::builder()
                    .w(Some(w))
                    .journal(j)
                    .w_timeout(Some(Duration::from_millis(u64::from(timeout))))
                    .build(),
                (Some(w), Some(j), None) => WriteConcern::builder().w(Some(w)).journal(j).build(),
                (Some(w), None, Some(timeout)) => WriteConcern::builder()
                    .w(Some(w))
                    .w_timeout(Some(Duration::from_millis(u64::from(timeout))))
                    .build(),
                (Some(w), None, None) => WriteConcern::builder().w(Some(w)).build(),
                _ => unreachable!(), // We know w is Some because we're in the if let Some(w) block
            };

            client_options.write_concern = Some(write_concern);
        } else if spec.write_concern_j.is_some() || spec.write_concern_timeout_ms.is_some() {
            return Err(make_err!(
                Code::InvalidArgument,
                "write_concern_w not set, but j and/or timeout set. Please set 'write_concern_w' to a non-default value. See https://www.mongodb.com/docs/manual/reference/write-concern/#w-option for options."
            ));
        }

        // Create client
        let client = MongoClient::with_options(client_options).map_err(|e| {
            make_err!(
                Code::InvalidArgument,
                "Failed to create MongoDB client: {e}"
            )
        })?;

        // Get database and collections
        let database = client.database(&spec.database);
        let cas_collection = database.collection::<Document>(&spec.cas_collection);
        let scheduler_collection = database.collection::<Document>(&spec.scheduler_collection);

        // Create indexes
        Self::create_indexes(&cas_collection, &scheduler_collection).await?;

        let store = Self {
            client,
            database,
            cas_collection,
            scheduler_collection,
            key_prefix: spec.key_prefix.clone().unwrap_or_default(),
            read_chunk_size: spec.read_chunk_size,
            max_concurrent_uploads: spec.max_concurrent_uploads,
            enable_change_streams: spec.enable_change_streams,
            subscription_manager: Mutex::new(None),
        };

        Ok(Arc::new(store))
    }

    /// Create necessary indexes for efficient operations.
    async fn create_indexes(
        cas_collection: &Collection<Document>,
        _scheduler_collection: &Collection<Document>,
    ) -> Result<(), Error> {
        // CAS collection indexes
        cas_collection
            .create_index(IndexModel::builder().keys(doc! { SIZE_FIELD: 1 }).build())
            .await
            .map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Failed to create size index on CAS collection: {e}"
                )
            })?;

        // Scheduler collection will have dynamic indexes created as needed

        Ok(())
    }

    /// Encode a [`StoreKey`] so it can be sent to `MongoDB`.
    fn encode_key<'a>(&self, key: &'a StoreKey<'a>) -> Cow<'a, str> {
        let key_body = key.as_str();
        if self.key_prefix.is_empty() {
            key_body
        } else {
            match key_body {
                Cow::Owned(mut encoded_key) => {
                    encoded_key.insert_str(0, &self.key_prefix);
                    Cow::Owned(encoded_key)
                }
                Cow::Borrowed(body) => {
                    let mut encoded_key = String::with_capacity(self.key_prefix.len() + body.len());
                    encoded_key.push_str(&self.key_prefix);
                    encoded_key.push_str(body);
                    Cow::Owned(encoded_key)
                }
            }
        }
    }

    /// Decode a key from `MongoDB` by removing the prefix.
    fn decode_key(&self, key: &str) -> Option<String> {
        if self.key_prefix.is_empty() {
            Some(key.to_string())
        } else {
            key.strip_prefix(&self.key_prefix).map(ToString::to_string)
        }
    }
}

#[async_trait]
impl StoreDriver for ExperimentalMongoStore {
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        for (key, result) in keys.iter().zip(results.iter_mut()) {
            // Handle zero digest specially
            if is_zero_digest(key.borrow()) {
                *result = Some(0);
                continue;
            }

            let encoded_key = self.encode_key(key);
            let filter = doc! { KEY_FIELD: encoded_key.as_ref() };

            match self.cas_collection.find_one(filter).await {
                Ok(Some(doc)) => {
                    *result = doc.get_i64(SIZE_FIELD).ok().map(|v| v as u64);
                }
                Ok(None) => {
                    *result = None;
                }
                Err(e) => {
                    return Err(make_err!(
                        Code::Internal,
                        "MongoDB error in has_with_results: {e}"
                    ));
                }
            }
        }
        Ok(())
    }

    async fn list(
        self: Pin<&Self>,
        range: (Bound<StoreKey<'_>>, Bound<StoreKey<'_>>),
        handler: &mut (dyn for<'a> FnMut(&'a StoreKey) -> bool + Send + Sync + '_),
    ) -> Result<u64, Error> {
        let mut filter = Document::new();

        // Build range query
        let mut key_filter = Document::new();
        match &range.0 {
            Bound::Included(start) => {
                let encoded = self.encode_key(start);
                key_filter.insert("$gte", encoded.as_ref());
            }
            Bound::Excluded(start) => {
                let encoded = self.encode_key(start);
                key_filter.insert("$gt", encoded.as_ref());
            }
            Bound::Unbounded => {}
        }

        match &range.1 {
            Bound::Included(end) => {
                let encoded = self.encode_key(end);
                key_filter.insert("$lte", encoded.as_ref());
            }
            Bound::Excluded(end) => {
                let encoded = self.encode_key(end);
                key_filter.insert("$lt", encoded.as_ref());
            }
            Bound::Unbounded => {}
        }

        if !key_filter.is_empty() {
            filter.insert(KEY_FIELD, key_filter);
        }

        // Add prefix filter if needed
        if !self.key_prefix.is_empty() {
            let regex_filter = doc! {
                KEY_FIELD: {
                    "$regex": format!("^{}", regex::escape(&self.key_prefix)),
                }
            };
            if filter.is_empty() {
                filter = regex_filter;
            } else {
                filter = doc! { "$and": [filter, regex_filter] };
            }
        }

        let mut cursor = self
            .cas_collection
            .find(filter)
            .projection(doc! { KEY_FIELD: 1 })
            .await
            .map_err(|e| make_err!(Code::Internal, "Failed to create cursor in list: {e}"))?;

        let mut count = 0u64;
        while let Some(doc) = cursor
            .try_next()
            .await
            .map_err(|e| make_err!(Code::Internal, "Failed to get next document in list: {e}"))?
        {
            if let Ok(key) = doc.get_str(KEY_FIELD) {
                if let Some(decoded_key) = self.decode_key(key) {
                    let store_key = StoreKey::new_str(&decoded_key);
                    count += 1;
                    if !handler(&store_key) {
                        break;
                    }
                }
            }
        }

        Ok(count)
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        let encoded_key = self.encode_key(&key);

        // Handle zero digest
        if is_zero_digest(key.borrow()) {
            let chunk = reader.peek().await.map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Failed to peek in ExperimentalMongoStore::update: {e}"
                )
            })?;
            if chunk.is_empty() {
                reader.drain().await.map_err(|e| {
                    make_err!(
                        Code::Internal,
                        "Failed to drain in ExperimentalMongoStore::update: {e}"
                    )
                })?;
                return Ok(());
            }
        }

        // Special handling for MaxSize(0) - this should error if stream is closed
        if upload_size == UploadSizeInfo::MaxSize(0) {
            // Try to read from the stream - if it's closed immediately, this should error
            match reader.recv().await {
                Ok(_chunk) => {
                    return Err(make_input_err!(
                        "Received data when MaxSize is 0 in MongoDB store"
                    ));
                }
                Err(e) => {
                    return Err(make_err!(
                        Code::InvalidArgument,
                        "Stream closed for MaxSize(0) upload: {e}"
                    ));
                }
            }
        }

        // Read all data into memory with proper EOF handling
        let mut data = Vec::new();
        while let Ok(chunk) = reader.recv().await {
            if chunk.is_empty() {
                break; // Empty chunk signals EOF
            }
            data.extend_from_slice(&chunk);
        }

        let size = data.len() as i64;

        // Create document
        let doc = doc! {
            KEY_FIELD: encoded_key.as_ref(),
            DATA_FIELD: Bson::Binary(mongodb::bson::Binary {
                subtype: mongodb::bson::spec::BinarySubtype::Generic,
                bytes: data,
            }),
            SIZE_FIELD: size,
        };

        // Upsert the document
        self.cas_collection
            .update_one(
                doc! { KEY_FIELD: encoded_key.as_ref() },
                doc! { "$set": doc },
            )
            .upsert(true)
            .await
            .map_err(|e| make_err!(Code::Internal, "Failed to update document in MongoDB: {e}"))?;

        Ok(())
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        // Handle zero digest
        if is_zero_digest(key.borrow()) {
            return writer.send_eof().map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Failed to send zero EOF in mongo store get_part: {e}"
                )
            });
        }

        let encoded_key = self.encode_key(&key);
        let filter = doc! { KEY_FIELD: encoded_key.as_ref() };

        let doc = self
            .cas_collection
            .find_one(filter)
            .await
            .map_err(|e| make_err!(Code::Internal, "Failed to find document in get_part: {e}"))?
            .ok_or_else(|| {
                make_err!(
                    Code::NotFound,
                    "Data not found in MongoDB store for digest: {key:?}"
                )
            })?;

        let data = match doc.get(DATA_FIELD) {
            Some(Bson::Binary(binary)) => &binary.bytes,
            _ => {
                return Err(make_err!(
                    Code::Internal,
                    "Invalid data field in MongoDB document"
                ));
            }
        };

        let offset = offset as usize;
        let data_len = data.len();

        if offset > data_len {
            return writer.send_eof().map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Failed to send EOF in mongo store get_part: {e}"
                )
            });
        }

        let end = if let Some(len) = length {
            cmp::min(offset + len as usize, data_len)
        } else {
            data_len
        };

        if offset < end {
            let chunk = &data[offset..end];

            // Send data in chunks
            for chunk_data in chunk.chunks(self.read_chunk_size) {
                writer.send(chunk_data.to_vec().into()).await.map_err(|e| {
                    make_err!(
                        Code::Internal,
                        "Failed to write data in ExperimentalMongoStore::get_part: {e}"
                    )
                })?;
            }
        }

        writer.send_eof().map_err(|e| {
            make_err!(
                Code::Internal,
                "Failed to write EOF in mongo store get_part: {e}"
            )
        })
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any(&self) -> &(dyn core::any::Any + Sync + Send) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send> {
        self
    }

    fn register_health(self: Arc<Self>, registry: &mut HealthRegistryBuilder) {
        registry.register_indicator(self);
    }
}

#[async_trait]
impl HealthStatusIndicator for ExperimentalMongoStore {
    fn get_name(&self) -> &'static str {
        "ExperimentalMongoStore"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        match self.database.run_command(doc! { "ping": 1 }).await {
            Ok(_) => HealthStatus::new_ok(self, "Connection healthy".into()),
            Err(e) => HealthStatus::new_failed(
                self,
                format!("{namespace} - MongoDB connection error: {e}").into(),
            ),
        }
    }
}

// -------------------------------------------------------------------
//
// `ExperimentalMongoDB` scheduler implementation. Likely to change
// -------------------------------------------------------------------

/// An individual subscription to a key in `ExperimentalMongoDB`.
#[derive(Debug)]
pub struct ExperimentalMongoSubscription {
    receiver: Option<watch::Receiver<String>>,
    weak_subscribed_keys: Weak<RwLock<StringPatriciaMap<ExperimentalMongoSubscriptionPublisher>>>,
}

impl SchedulerSubscription for ExperimentalMongoSubscription {
    async fn changed(&mut self) -> Result<(), Error> {
        let receiver = self.receiver.as_mut().ok_or_else(|| {
            make_err!(
                Code::Internal,
                "In ExperimentalMongoSubscription::changed::as_mut"
            )
        })?;
        receiver.changed().await.map_err(|_| {
            make_err!(
                Code::Internal,
                "In ExperimentalMongoSubscription::changed::changed"
            )
        })
    }
}

impl Drop for ExperimentalMongoSubscription {
    fn drop(&mut self) {
        let Some(receiver) = self.receiver.take() else {
            warn!("ExperimentalMongoSubscription has already been dropped, nothing to do.");
            return;
        };
        let key = receiver.borrow().clone();
        drop(receiver);

        let Some(subscribed_keys) = self.weak_subscribed_keys.upgrade() else {
            return;
        };
        let mut subscribed_keys = subscribed_keys.write();
        let Some(value) = subscribed_keys.get(&key) else {
            error!(
                "Key {key} was not found in subscribed keys when checking if it should be removed."
            );
            return;
        };

        if value.receiver_count() == 0 {
            subscribed_keys.remove(key);
        }
    }
}

/// A publisher for a key in `ExperimentalMongoDB`.
#[derive(Debug)]
struct ExperimentalMongoSubscriptionPublisher {
    sender: Mutex<watch::Sender<String>>,
}

impl ExperimentalMongoSubscriptionPublisher {
    fn new(
        key: String,
        weak_subscribed_keys: Weak<RwLock<StringPatriciaMap<Self>>>,
    ) -> (Self, ExperimentalMongoSubscription) {
        let (sender, receiver) = watch::channel(key);
        let publisher = Self {
            sender: Mutex::new(sender),
        };
        let subscription = ExperimentalMongoSubscription {
            receiver: Some(receiver),
            weak_subscribed_keys,
        };
        (publisher, subscription)
    }

    fn subscribe(
        &self,
        weak_subscribed_keys: Weak<RwLock<StringPatriciaMap<Self>>>,
    ) -> ExperimentalMongoSubscription {
        let receiver = self.sender.lock().subscribe();
        ExperimentalMongoSubscription {
            receiver: Some(receiver),
            weak_subscribed_keys,
        }
    }

    fn receiver_count(&self) -> usize {
        self.sender.lock().receiver_count()
    }

    fn notify(&self) {
        self.sender.lock().send_modify(|_| {});
    }
}

#[derive(Debug)]
pub struct ExperimentalMongoSubscriptionManager {
    subscribed_keys: Arc<RwLock<StringPatriciaMap<ExperimentalMongoSubscriptionPublisher>>>,
    _subscription_spawn: JoinHandleDropGuard<()>,
}

impl ExperimentalMongoSubscriptionManager {
    pub fn new(database: Database, collection_name: String, key_prefix: String) -> Self {
        let subscribed_keys = Arc::new(RwLock::new(StringPatriciaMap::new()));
        let subscribed_keys_weak = Arc::downgrade(&subscribed_keys);

        Self {
            subscribed_keys,
            _subscription_spawn: spawn!("mongo_subscribe_spawn", async move {
                let collection = database.collection::<Document>(&collection_name);

                loop {
                    // Try to create change stream
                    let change_stream_result = collection.watch().await;

                    match change_stream_result {
                        Ok(mut change_stream) => {
                            info!("MongoDB change stream connected");

                            while let Some(event_result) = change_stream.next().await {
                                match event_result {
                                    Ok(event) => {
                                        use mongodb::change_stream::event::OperationType;
                                        match event.operation_type {
                                            OperationType::Insert
                                            | OperationType::Update
                                            | OperationType::Replace
                                            | OperationType::Delete => {
                                                if let Some(doc_key) = event.document_key {
                                                    if let Ok(key) = doc_key.get_str(KEY_FIELD) {
                                                        // Remove prefix if present
                                                        let key = if key_prefix.is_empty() {
                                                            key
                                                        } else {
                                                            key.strip_prefix(&key_prefix)
                                                                .unwrap_or(key)
                                                        };

                                                        let Some(subscribed_keys) =
                                                            subscribed_keys_weak.upgrade()
                                                        else {
                                                            warn!(
                                                                "Parent dropped, exiting ExperimentalMongoSubscriptionManager"
                                                            );
                                                            return;
                                                        };

                                                        let subscribed_keys_mux =
                                                            subscribed_keys.read();
                                                        subscribed_keys_mux
                                                                .common_prefix_values(key)
                                                                .for_each(ExperimentalMongoSubscriptionPublisher::notify);
                                                    }
                                                }
                                            }
                                            _ => {}
                                        }
                                    }
                                    Err(e) => {
                                        error!("Error in change stream: {e}");
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to create change stream: {e}. Will retry in 5 seconds.");
                        }
                    }

                    // Check if parent is still alive
                    if subscribed_keys_weak.upgrade().is_none() {
                        warn!("Parent dropped, exiting ExperimentalMongoSubscriptionManager");
                        return;
                    }

                    // Sleep before retry
                    sleep(Duration::from_secs(5)).await;

                    // Notify all subscribers on reconnection
                    if let Some(subscribed_keys) = subscribed_keys_weak.upgrade() {
                        let subscribed_keys_mux = subscribed_keys.read();
                        for publisher in subscribed_keys_mux.values() {
                            publisher.notify();
                        }
                    }
                }
            }),
        }
    }
}

impl SchedulerSubscriptionManager for ExperimentalMongoSubscriptionManager {
    type Subscription = ExperimentalMongoSubscription;

    fn notify_for_test(&self, value: String) {
        let subscribed_keys_mux = self.subscribed_keys.read();
        subscribed_keys_mux
            .common_prefix_values(&value)
            .for_each(ExperimentalMongoSubscriptionPublisher::notify);
    }

    fn subscribe<K>(&self, key: K) -> Result<Self::Subscription, Error>
    where
        K: SchedulerStoreKeyProvider,
    {
        let weak_subscribed_keys = Arc::downgrade(&self.subscribed_keys);
        let mut subscribed_keys = self.subscribed_keys.write();
        let key = key.get_key();
        let key_str = key.as_str();

        let mut subscription = if let Some(publisher) = subscribed_keys.get(&key_str) {
            publisher.subscribe(weak_subscribed_keys)
        } else {
            let (publisher, subscription) = ExperimentalMongoSubscriptionPublisher::new(
                key_str.to_string(),
                weak_subscribed_keys,
            );
            subscribed_keys.insert(key_str, publisher);
            subscription
        };

        subscription
            .receiver
            .as_mut()
            .ok_or_else(|| {
                make_err!(
                    Code::Internal,
                    "Receiver should be set in ExperimentalMongoSubscriptionManager::subscribe"
                )
            })?
            .mark_changed();

        Ok(subscription)
    }
}

impl SchedulerStore for ExperimentalMongoStore {
    type SubscriptionManager = ExperimentalMongoSubscriptionManager;

    fn subscription_manager(&self) -> Result<Arc<ExperimentalMongoSubscriptionManager>, Error> {
        let mut subscription_manager = self.subscription_manager.lock();
        if let Some(subscription_manager) = &*subscription_manager {
            Ok(subscription_manager.clone())
        } else {
            if !self.enable_change_streams {
                return Err(make_input_err!(
                    "ExperimentalMongoStore must have change streams enabled for scheduler subscriptions"
                ));
            }

            let sub = Arc::new(ExperimentalMongoSubscriptionManager::new(
                self.database.clone(),
                self.scheduler_collection.name().to_string(),
                self.key_prefix.clone(),
            ));
            *subscription_manager = Some(sub.clone());
            Ok(sub)
        }
    }

    async fn update_data<T>(&self, data: T) -> Result<Option<u64>, Error>
    where
        T: SchedulerStoreDataProvider
            + SchedulerStoreKeyProvider
            + SchedulerCurrentVersionProvider
            + Send,
    {
        let key = data.get_key();
        let encoded_key = self.encode_key(&key);
        let maybe_index = data.get_indexes().map_err(|e| {
            make_err!(
                Code::Internal,
                "Error getting indexes in ExperimentalMongoStore::update_data: {e}"
            )
        })?;

        if <T as SchedulerStoreKeyProvider>::Versioned::VALUE {
            let current_version = data.current_version();
            let data_bytes = data.try_into_bytes().map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Could not convert value to bytes in ExperimentalMongoStore::update_data: {e}"
                )
            })?;

            let mut update_doc = doc! {
                "$set": {
                    DATA_FIELD: Bson::Binary(mongodb::bson::Binary {
                        subtype: mongodb::bson::spec::BinarySubtype::Generic,
                        bytes: data_bytes.to_vec(),
                    }),
                },
                "$inc": {
                    VERSION_FIELD: 1i64,
                }
            };

            // Add indexes
            for (name, value) in maybe_index {
                update_doc.get_document_mut("$set").unwrap().insert(
                    name,
                    Bson::Binary(mongodb::bson::Binary {
                        subtype: mongodb::bson::spec::BinarySubtype::Generic,
                        bytes: value.to_vec(),
                    }),
                );
            }

            let filter = doc! {
                KEY_FIELD: encoded_key.as_ref(),
                VERSION_FIELD: current_version as i64,
            };

            match self
                .scheduler_collection
                .find_one_and_update(filter, update_doc)
                .upsert(true)
                .return_document(ReturnDocument::After)
                .await
            {
                Ok(Some(doc)) => {
                    let new_version = doc.get_i64(VERSION_FIELD).unwrap_or(1) as u64;
                    Ok(Some(new_version))
                }
                Ok(None) => Ok(None),
                Err(e) => Err(make_err!(
                    Code::Internal,
                    "MongoDB error in update_data: {e}"
                )),
            }
        } else {
            let data_bytes = data.try_into_bytes().map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Could not convert value to bytes in ExperimentalMongoStore::update_data: {e}"
                )
            })?;

            let mut doc = doc! {
                KEY_FIELD: encoded_key.as_ref(),
                DATA_FIELD: Bson::Binary(mongodb::bson::Binary {
                    subtype: mongodb::bson::spec::BinarySubtype::Generic,
                    bytes: data_bytes.to_vec(),
                }),
            };

            // Add indexes
            for (name, value) in maybe_index {
                doc.insert(
                    name,
                    Bson::Binary(mongodb::bson::Binary {
                        subtype: mongodb::bson::spec::BinarySubtype::Generic,
                        bytes: value.to_vec(),
                    }),
                );
            }

            self.scheduler_collection
                .update_one(
                    doc! { KEY_FIELD: encoded_key.as_ref() },
                    doc! { "$set": doc },
                )
                .upsert(true)
                .await
                .map_err(|e| {
                    make_err!(Code::Internal, "Failed to update scheduler document: {e}")
                })?;

            Ok(Some(0))
        }
    }

    async fn search_by_index_prefix<K>(
        &self,
        index: K,
    ) -> Result<
        impl Stream<Item = Result<<K as SchedulerStoreDecodeTo>::DecodeOutput, Error>> + Send,
        Error,
    >
    where
        K: SchedulerIndexProvider + SchedulerStoreDecodeTo + Send,
    {
        let index_value = index.index_value();

        // Create index if it doesn't exist
        let index_name = format!("{}_{}", K::KEY_PREFIX, K::INDEX_NAME);
        self.scheduler_collection
            .create_index(
                IndexModel::builder()
                    .keys(doc! { K::INDEX_NAME: 1 })
                    .options(IndexOptions::builder().name(index_name).build())
                    .build(),
            )
            .await
            .map_err(|e| make_err!(Code::Internal, "Failed to create scheduler index: {e}"))?;

        // Build filter
        let filter = doc! {
            K::INDEX_NAME: {
                "$regex": format!("^{}", regex::escape(index_value.as_ref())),
            }
        };

        // Add sort if specified
        let find_options = if let Some(sort_key) = K::MAYBE_SORT_KEY {
            FindOptions::builder().sort(doc! { sort_key: 1 }).build()
        } else {
            FindOptions::default()
        };

        let cursor = self
            .scheduler_collection
            .find(filter)
            .with_options(find_options)
            .await
            .map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Failed to create cursor in search_by_index_prefix: {e}"
                )
            })?;

        Ok(cursor.map(move |result| {
            let doc = result.map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Error reading document in search_by_index_prefix: {e}"
                )
            })?;

            let data = match doc.get(DATA_FIELD) {
                Some(Bson::Binary(binary)) => Bytes::from(binary.bytes.clone()),
                _ => {
                    return Err(make_err!(
                        Code::Internal,
                        "Missing or invalid data field in search_by_index_prefix"
                    ));
                }
            };

            let version = if <K as SchedulerIndexProvider>::Versioned::VALUE {
                doc.get_i64(VERSION_FIELD).unwrap_or(0) as u64
            } else {
                0
            };

            K::decode(version, data).map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Failed to decode in search_by_index_prefix: {e}"
                )
            })
        }))
    }

    async fn get_and_decode<K>(
        &self,
        key: K,
    ) -> Result<Option<<K as SchedulerStoreDecodeTo>::DecodeOutput>, Error>
    where
        K: SchedulerStoreKeyProvider + SchedulerStoreDecodeTo + Send,
    {
        let key = key.get_key();
        let encoded_key = self.encode_key(&key);
        let filter = doc! { KEY_FIELD: encoded_key.as_ref() };

        let doc = self
            .scheduler_collection
            .find_one(filter)
            .await
            .map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Failed to find document in get_and_decode: {e}"
                )
            })?;

        let Some(doc) = doc else {
            return Ok(None);
        };

        let data = match doc.get(DATA_FIELD) {
            Some(Bson::Binary(binary)) => Bytes::from(binary.bytes.clone()),
            _ => return Ok(None),
        };

        let version = if <K as SchedulerStoreKeyProvider>::Versioned::VALUE {
            doc.get_i64(VERSION_FIELD).unwrap_or(0) as u64
        } else {
            0
        };

        Ok(Some(K::decode(version, data).map_err(|e| {
            make_err!(Code::Internal, "Failed to decode in get_and_decode: {e}")
        })?))
    }
}
