// Copyright 2025 The NativeLink Authors. All rights reserved.
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

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use futures::TryStreamExt;
use mongodb::Client as MongoClient;
use mongodb::bson::{Document, doc};
use mongodb::options::ClientOptions;
use nativelink_config::stores::ExperimentalMongoSpec;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_store::cas_utils::ZERO_BYTE_DIGESTS;
use nativelink_store::mongo_store::ExperimentalMongoStore;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use nativelink_util::store_trait::{
    SchedulerCurrentVersionProvider, SchedulerIndexProvider, SchedulerStore,
    SchedulerStoreDataProvider, SchedulerStoreDecodeTo, SchedulerStoreKeyProvider, StoreKey,
    StoreLike, TrueValue, UploadSizeInfo,
};
use pretty_assertions::assert_eq;
use uuid::Uuid;

mod mongo_runner;
use mongo_runner::process::MongoProcess;

use crate::mongo_runner::MongoEmbedded;

const VALID_HASH1: &str = "3031323334353637383961626364656630303030303030303030303030303030";

/// Creates a test `MongoDB` specification.
fn create_test_spec_with_key_prefix(
    key_prefix: Option<String>,
    connection_string: String,
) -> ExperimentalMongoSpec {
    // Create database name with timestamp for uniqueness
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();

    ExperimentalMongoSpec {
        connection_string,
        database: format!(
            "nltest_{}_{}",
            timestamp,
            &Uuid::new_v4().simple().to_string()[..8]
        ),
        cas_collection: "test_cas".to_string(),
        scheduler_collection: "test_scheduler".to_string(),
        key_prefix,
        read_chunk_size: 1024,
        max_concurrent_uploads: 10,
        connection_timeout_ms: 10000, // Longer timeout for remote DB
        command_timeout_ms: 15000,    // Longer timeout for remote DB
        enable_change_streams: false,
        write_concern_w: Some("majority".to_string()),
        write_concern_j: Some(true),
        write_concern_timeout_ms: Some(10000), // Longer timeout for remote DB
        max_requests: None,
    }
}

// Helper to keep hold of Mongo process parts so we don't accidentally drop them!
#[derive(Debug)]
pub(crate) struct MongoParts {
    pub process: MongoProcess,
    #[allow(dead_code)]
    pub embedded: MongoEmbedded,
}

/// Test helper that manages `MongoDB` database lifecycle
#[derive(Debug)]
pub(crate) struct TestMongoHelper {
    pub store: Arc<ExperimentalMongoStore>,
    pub database_name: String,
    pub mongo_local: Option<MongoParts>,
    pub spec: ExperimentalMongoSpec,
}

impl TestMongoHelper {
    /// Creates a new test store and database
    async fn new_spec(
        key_prefix: Option<String>,
    ) -> Result<(ExperimentalMongoSpec, Option<MongoParts>), Error> {
        let mut mongo_local: Option<MongoParts> = None;
        let test_mongo_url = if let Ok(url) = std::env::var("NATIVELINK_TEST_MONGO_URL") {
            url
        } else {
            let port = redis_test::utils::get_random_available_port();
            let mongo_runner = MongoEmbedded::new("7.0.11").set_port(port);
            let process = mongo_runner
                .start()
                .await
                .err_tip(|| "Failed to start MongoDB")?;
            mongo_local.replace(MongoParts {
                process,
                embedded: mongo_runner,
            });
            format!("mongodb://127.0.0.1:{port}/")
        };

        let spec = create_test_spec_with_key_prefix(key_prefix, test_mongo_url);
        Ok((spec, mongo_local))
    }

    async fn new_with_spec_and_process(
        spec: ExperimentalMongoSpec,
        mongo_local: Option<MongoParts>,
    ) -> Result<Self, Error> {
        let database_name = spec.database.clone();

        let store = ExperimentalMongoStore::new(spec.clone()).await?;

        Ok(Self {
            store,
            database_name,
            mongo_local,
            spec,
        })
    }

    async fn new(key_prefix: Option<String>) -> Result<Self, Error> {
        // Split out like this because various tests want to edit the spec before launching the store
        // But they need new_spec to make the test process
        let (spec, mongo_process) = Self::new_spec(key_prefix).await?;
        Self::new_with_spec_and_process(spec, mongo_process).await
    }
}

impl Drop for TestMongoHelper {
    fn drop(&mut self) {
        // No longer cleaning up databases - we want to keep them for inspection
        eprintln!(
            "Test database '{}' retained for inspection",
            self.database_name
        );
        if let Some(mut parts) = self.mongo_local.take() {
            parts.process.kill().expect("Failed to kill mongo process");
        }
    }
}

#[nativelink_test]
async fn upload_and_get_data() -> Result<(), Error> {
    // Create test helper with automatic cleanup
    let helper = TestMongoHelper::new(None).await?;

    // Construct the data we want to send.
    let data = Bytes::from_static(b"14");

    // Construct a digest for our data.
    let digest = DigestInfo::try_new(VALID_HASH1, 2)?;

    // Upload the data.
    helper.store.update_oneshot(digest, data.clone()).await?;

    // Verify the data exists.
    let result = helper.store.has(digest).await?;
    assert!(
        result.is_some(),
        "Expected mongo store to have hash: {VALID_HASH1}",
    );

    // Retrieve and verify the data.
    let result = helper
        .store
        .get_part_unchunked(digest, 0, Some(data.len() as u64))
        .await?;

    assert_eq!(result, data, "Expected mongo store to have updated value");

    // Database will be automatically cleaned up when helper is dropped
    Ok(())
}

#[nativelink_test]
async fn upload_and_get_data_without_prefix() -> Result<(), Error> {
    // Create test helper with automatic cleanup
    let helper = TestMongoHelper::new(None).await?;

    let data = Bytes::from_static(b"14");
    let digest = DigestInfo::try_new(VALID_HASH1, 2)?;

    helper.store.update_oneshot(digest, data.clone()).await?;

    let result = helper.store.has(digest).await?;
    assert!(
        result.is_some(),
        "Expected mongo store to have hash: {VALID_HASH1}",
    );

    let result = helper
        .store
        .get_part_unchunked(digest, 0, Some(data.len() as u64))
        .await?;

    assert_eq!(result, data, "Expected mongo store to have updated value");

    // Database will be automatically cleaned up when helper is dropped
    Ok(())
}

#[nativelink_test]
async fn upload_empty_data() -> Result<(), Error> {
    let data = Bytes::from_static(b"");
    let digest = ZERO_BYTE_DIGESTS[0];
    let helper = TestMongoHelper::new(None).await?;

    helper.store.update_oneshot(digest, data).await?;

    let result = helper.store.has(digest).await?;
    assert!(
        result.is_some(),
        "Expected mongo store to have zero-byte hash",
    );

    Ok(())
}

#[nativelink_test]
#[allow(clippy::items_after_statements)]
async fn test_large_downloads_are_chunked() -> Result<(), Error> {
    const READ_CHUNK_SIZE: usize = 1024;
    let data = Bytes::from(vec![0u8; READ_CHUNK_SIZE + 128]);

    let digest = DigestInfo::try_new(VALID_HASH1, data.len())?;

    let (mut spec, mongo_process) = TestMongoHelper::new_spec(None).await?;
    spec.read_chunk_size = READ_CHUNK_SIZE;

    let helper = TestMongoHelper::new_with_spec_and_process(spec, mongo_process).await?;
    let store = helper.store.clone();

    store.update_oneshot(digest, data.clone()).await?;

    let result = store.has(digest).await?;
    assert!(
        result.is_some(),
        "Expected mongo store to have hash: {VALID_HASH1}",
    );

    let get_result: Bytes = store
        .get_part_unchunked(digest, 0, Some(data.len() as u64))
        .await?;

    assert_eq!(
        get_result, data,
        "Expected mongo store to have updated value",
    );

    Ok(())
}

#[nativelink_test]
#[allow(clippy::items_after_statements)]
async fn yield_between_sending_packets_in_update() -> Result<(), Error> {
    const READ_CHUNK_SIZE: usize = 1024;
    let data_p1 = Bytes::from(vec![b'A'; READ_CHUNK_SIZE + 512]);
    let data_p2 = Bytes::from(vec![b'B'; READ_CHUNK_SIZE]);

    let mut data = Vec::new();
    data.extend_from_slice(&data_p1);
    data.extend_from_slice(&data_p2);
    let data = Bytes::from(data);

    let digest = DigestInfo::try_new(VALID_HASH1, data.len())?;

    let (mut spec, mongo_process) = TestMongoHelper::new_spec(None).await?;
    spec.read_chunk_size = READ_CHUNK_SIZE;
    let helper = TestMongoHelper::new_with_spec_and_process(spec, mongo_process).await?;
    let store = helper.store.clone();

    let (mut tx, rx) = make_buf_channel_pair();

    tokio::try_join!(
        async {
            store
                .update(digest, rx, UploadSizeInfo::ExactSize(data.len() as u64))
                .await?;
            Ok::<_, Error>(())
        },
        async {
            tx.send(data_p1).await?;
            tx.send(data_p2).await?;
            tx.send_eof()?;
            Ok::<_, Error>(())
        },
    )?;

    let result = store.has(digest).await?;
    assert!(
        result.is_some(),
        "Expected mongo store to have hash: {VALID_HASH1}",
    );

    let result = store
        .get_part_unchunked(digest, 0, Some(data.len() as u64))
        .await?;

    assert_eq!(result, data, "Expected mongo store to have updated value");

    Ok(())
}

#[nativelink_test]
async fn zero_len_items_exist_check() -> Result<(), Error> {
    let digest = DigestInfo::try_new(VALID_HASH1, 0)?;
    let helper = TestMongoHelper::new(None).await?;

    // Try to get a zero-length item that doesn't exist
    let result = helper.store.get_part_unchunked(digest, 0, None).await;
    let err = result.unwrap_err();
    assert_eq!(err.code, Code::NotFound, "{:?}", err);

    Ok(())
}

#[nativelink_test]
async fn test_invalid_connection_string() -> Result<(), Error> {
    let spec = create_test_spec_with_key_prefix(None, "invalid://connection_string".to_string());
    let result = ExperimentalMongoStore::new(spec).await;
    assert!(
        result.is_err(),
        "Expected error for invalid connection string"
    );

    // Should be an InvalidArgument error
    let err = result.unwrap_err();
    assert_eq!(err.code, Code::InvalidArgument);

    Ok(())
}

#[nativelink_test]
async fn test_empty_connection_string() -> Result<(), Error> {
    let spec = create_test_spec_with_key_prefix(None, String::new());

    let result = ExperimentalMongoStore::new(spec).await;
    assert!(
        result.is_err(),
        "Expected error for empty connection string"
    );

    let err = result.unwrap_err();
    assert_eq!(err.code, Code::InvalidArgument);
    assert!(err.messages[0].contains("No connection string was specified"));

    Ok(())
}

#[nativelink_test]
async fn test_default_values() -> Result<(), Error> {
    let (mut spec, mongo_process) = TestMongoHelper::new_spec(None).await?;
    // Clear all optional fields to test defaults
    spec.database = String::new();
    spec.cas_collection = String::new();
    spec.scheduler_collection = String::new();
    spec.key_prefix = None;
    spec.read_chunk_size = 0;
    spec.connection_timeout_ms = 0;
    spec.command_timeout_ms = 0;
    spec.max_concurrent_uploads = 0;

    // This should succeed and use default values
    let helper = TestMongoHelper::new_with_spec_and_process(spec, mongo_process).await?;
    let store = helper.store.clone();

    // Test that the store is functional with defaults
    let data = Bytes::from_static(b"test_data");
    let digest = DigestInfo::try_new(VALID_HASH1, data.len())?;

    store.update_oneshot(digest, data.clone()).await?;

    let result = store.has(digest).await?;
    assert!(
        result.is_some(),
        "Expected mongo store to work with defaults"
    );

    Ok(())
}

#[nativelink_test]
async fn dont_loop_forever_on_empty() -> Result<(), Error> {
    let helper = TestMongoHelper::new(None).await?;
    let digest = DigestInfo::try_new(VALID_HASH1, 2)?;
    let (tx, rx) = make_buf_channel_pair();

    // Add timeout to prevent hanging - this test should fail quickly
    let result = tokio::time::timeout(core::time::Duration::from_secs(5), async {
        tokio::join!(
            async {
                helper
                    .store
                    .update(digest, rx, UploadSizeInfo::MaxSize(0))
                    .await
                    .unwrap_err();
            },
            async {
                drop(tx);
            },
        )
    })
    .await;

    // Verify the timeout completed (didn't hang)
    assert!(
        result.is_ok(),
        "Test should not hang - update should fail quickly"
    );

    Ok(())
}

// Test partial reads
#[nativelink_test]
async fn test_partial_reads() -> Result<(), Error> {
    // Create test helper with automatic cleanup
    let helper = TestMongoHelper::new(None).await?;

    let data = Bytes::from_static(b"Hello, MongoDB World!");
    let digest = DigestInfo::try_new(VALID_HASH1, data.len())?;

    helper.store.update_oneshot(digest, data.clone()).await?;

    // Read first 5 bytes
    let partial_result = helper.store.get_part_unchunked(digest, 0, Some(5)).await?;
    assert_eq!(partial_result, &data[0..5]);

    // Read middle section
    let partial_result = helper.store.get_part_unchunked(digest, 7, Some(7)).await?;
    assert_eq!(partial_result, &data[7..14]);

    // Read from offset to end
    let partial_result = helper
        .store
        .get_part_unchunked(digest, 7, Some((data.len() - 7) as u64))
        .await?;
    assert_eq!(partial_result, &data[7..]);

    // Database will be automatically cleaned up when helper is dropped
    Ok(())
}

/// Test that verifies database creation (cleanup disabled for inspection)
#[nativelink_test]
async fn test_database_lifecycle() -> Result<(), Error> {
    let (spec, mongo_process) = TestMongoHelper::new_spec(None).await?;
    let database_name = spec.database.clone();

    let client = MongoClient::with_uri_str(&spec.connection_string).await?;

    // Verify database doesn't exist initially
    let db_names = client
        .list_database_names()
        .await
        .map_err(|e| make_err!(Code::Internal, "Failed to list databases: {e}"))?;
    assert!(
        !db_names.contains(&database_name),
        "Test database should not exist initially"
    );

    // Create test helper (which creates the database)
    let helper = TestMongoHelper::new_with_spec_and_process(spec, mongo_process).await?;
    {
        // Upload some data to ensure database is created
        let data = Bytes::from_static(b"test_database_lifecycle");
        let digest = DigestInfo::try_new(VALID_HASH1, data.len())?;
        helper.store.update_oneshot(digest, data.clone()).await?;

        // Verify database now exists (with retry for MongoDB lazy creation)
        let mut db_exists = false;
        for _attempt in 0..5 {
            let db_names = client
                .list_database_names()
                .await
                .map_err(|e| make_err!(Code::Internal, "Failed to list databases: {e}"))?;
            if db_names.contains(&database_name) {
                db_exists = true;
                break;
            }
            tokio::time::sleep(core::time::Duration::from_millis(200)).await;
        }
        assert!(
            db_exists,
            "Test database should exist after creation, found databases: {:?}",
            client.list_database_names().await.unwrap_or_default()
        );

        // Verify data is accessible
        let result = helper.store.has(digest).await?;
        assert!(
            result.is_some(),
            "Data should be accessible in the database"
        );

        // NO cleanup happens anymore, but we keep the helper to keep the Mongo process and temp folders around
    }

    // Database should still exist
    let db_names = client
        .list_database_names()
        .await
        .map_err(|e| make_err!(Code::Internal, "Failed to list databases: {e}"))?;
    assert!(
        db_names.contains(&database_name),
        "Test database should still exist after drop (cleanup disabled)"
    );

    eprintln!("Database '{database_name}' retained for inspection");

    Ok(())
}

#[nativelink_test]
#[allow(clippy::use_debug)]
async fn create_ten_cas_entries() -> Result<(), Error> {
    // Create test helper with automatic cleanup
    let helper = TestMongoHelper::new(None).await?;

    eprintln!(
        "Creating 10 CAS entries in database: {}",
        helper.database_name
    );
    eprintln!("CAS Collection: test_cas");
    eprintln!("Key prefix: test:");

    // Create 10 different CAS entries with unique content and proper digests
    for i in 0..10 {
        // Create unique data for each entry
        let unique_content = format!(
            "Test CAS Entry #{} - Unique content with timestamp {} and random data: {}",
            i,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            Uuid::new_v4()
        );
        let data = Bytes::from(unique_content);

        // Calculate the actual SHA256 digest of the data
        let mut hasher = DigestHasherFunc::Sha256.hasher();
        hasher.update(&data);
        let digest = hasher.finalize_digest();

        eprintln!("Entry #{i}: Creating with digest: {digest}");
        eprintln!("  Content: {} bytes", data.len());
        eprintln!("  First 50 chars: {:?}", &data[..data.len().min(50)]);

        // Upload the data
        helper.store.update_oneshot(digest, data.clone()).await?;

        // Verify the data exists
        let result = helper.store.has(digest).await?;
        assert!(
            result.is_some(),
            "Expected mongo store to have hash for entry #{i}: {digest}"
        );

        // Verify we can retrieve the data
        let retrieved = helper
            .store
            .get_part_unchunked(digest, 0, Some(data.len() as u64))
            .await?;

        assert_eq!(retrieved, data, "Data mismatch for entry #{i}");

        eprintln!("Successfully created CAS entry #{i} with digest: {digest}");
    }

    // Query MongoDB directly to verify entries were written
    let helper = TestMongoHelper::new(None).await?;
    let client_options = ClientOptions::parse(&helper.spec.connection_string)
        .await
        .map_err(|e| make_err!(Code::Internal, "Failed to parse connection string: {e}"))?;

    let client = MongoClient::with_options(client_options)
        .map_err(|e| make_err!(Code::Internal, "Failed to create MongoDB client: {e}"))?;

    let db = client.database(&helper.database_name);
    let collection = db.collection::<Document>("test_cas");

    // Count documents in collection
    let count = collection
        .count_documents(doc! {})
        .await
        .map_err(|e| make_err!(Code::Internal, "Failed to count documents: {e}"))?;

    eprintln!("Total documents in CAS collection: {count}");

    // List first few documents
    let mut cursor = collection
        .find(doc! {})
        .await
        .map_err(|e| make_err!(Code::Internal, "Failed to find documents: {e}"))?;

    eprintln!("Sample documents in collection:");
    let mut doc_count = 0;
    while let Some(doc) = cursor
        .try_next()
        .await
        .map_err(|e| make_err!(Code::Internal, "Failed to get next document: {e}"))?
    {
        if doc_count < 3
            && let Ok(key) = doc.get_str("_id")
        {
            eprintln!("  - Key: {key}");
            if let Ok(size) = doc.get_i64("size") {
                eprintln!("    Size: {size} bytes");
            }
        }
        doc_count += 1;
    }

    eprintln!(
        "All 10 CAS entries created successfully in database: {}",
        helper.database_name
    );
    eprintln!("Collections: test_cas and test_scheduler");

    // Database will NOT be cleaned up - it's retained for inspection
    Ok(())
}

// Define test structures that implement the scheduler traits
#[derive(Debug, Clone, PartialEq)]
struct TestSchedulerData {
    key: String,
    content: String,
    version: i64,
}

impl SchedulerStoreKeyProvider for TestSchedulerData {
    type Versioned = TrueValue; // Using versioned storage

    fn get_key(&self) -> StoreKey<'static> {
        StoreKey::Str(std::borrow::Cow::Owned(self.key.clone()))
    }
}

impl SchedulerStoreDataProvider for TestSchedulerData {
    fn try_into_bytes(self) -> Result<Bytes, Error> {
        Ok(Bytes::from(self.content.into_bytes()))
    }

    fn get_indexes(&self) -> Result<Vec<(&'static str, Bytes)>, Error> {
        // Add some test indexes - need to use 'static strings
        Ok(vec![
            ("test_index", Bytes::from("test_value")),
            (
                "content_prefix",
                Bytes::from(self.content.chars().take(10).collect::<String>()),
            ),
        ])
    }
}

impl SchedulerCurrentVersionProvider for TestSchedulerData {
    fn current_version(&self) -> i64 {
        self.version
    }
}

impl SchedulerStoreDecodeTo for TestSchedulerData {
    type DecodeOutput = Self;

    fn decode(version: i64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
        let content = String::from_utf8(data.to_vec())
            .map_err(|e| make_err!(Code::InvalidArgument, "Invalid UTF-8 data: {e}"))?;
        // We don't have the key in the data, so we'll use a placeholder
        Ok(Self {
            key: "decoded".to_string(),
            content,
            version,
        })
    }
}

// Separate key type for lookups
#[derive(Debug, Clone)]
struct TestSchedulerKey(String);

impl SchedulerStoreKeyProvider for TestSchedulerKey {
    type Versioned = TrueValue;

    fn get_key(&self) -> StoreKey<'static> {
        StoreKey::Str(std::borrow::Cow::Owned(self.0.clone()))
    }
}

impl SchedulerStoreDecodeTo for TestSchedulerKey {
    type DecodeOutput = TestSchedulerData;

    fn decode(version: i64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
        TestSchedulerData::decode(version, data)
    }
}

// COMMENTED OUT FOR NOW BECAUSE IT IS UNUSED BUT MAY NEED TO RETURN
// TODO(marcussorealheis)determine if we need to bring this back as a durable backup option
// // Define test index provider
// #[derive(Debug, Clone)]
// struct TestIndexProvider {
//     value: String,
// }

// impl SchedulerIndexProvider for TestIndexProvider {
//     const KEY_PREFIX: &'static str = "test:";
//     const INDEX_NAME: &'static str = "test_index";
//     type Versioned = TrueValue;

//     fn index_value(&self) -> std::borrow::Cow<'_, str> {
//         std::borrow::Cow::Borrowed(&self.value)
//     }
// }

// impl SchedulerStoreKeyProvider for TestIndexProvider {
//     type Versioned = TrueValue;

//     fn get_key(&self) -> StoreKey<'static> {
//         StoreKey::Str(std::borrow::Cow::Owned(format!(
//             "{}indexed_key",
//             Self::KEY_PREFIX
//         )))
//     }
// }

// impl SchedulerStoreDecodeTo for TestIndexProvider {
//     type DecodeOutput = (StoreKey<'static>, Bytes);

//     fn decode(_version: u64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
//         // For search results, we just return the key and data
//         Ok((
//             StoreKey::Str(std::borrow::Cow::Owned("decoded_key".to_string())),
//             data,
//         ))
//     }
// }

// FIXME(palfrey): Test doesn't work on local Mongo. https://github.com/TraceMachina/nativelink/pull/1843 covers
// expanding Mongo to be usable as a scheduler backend, and should correct this problem there
#[nativelink_test]
#[ignore = "Broken with local mongo, only needed when we use this for a scheduler backend"]
async fn test_scheduler_store_operations() -> Result<(), Error> {
    // Create test helper
    let helper = TestMongoHelper::new(None).await?;

    eprintln!(
        "Testing scheduler store operations in database: {}",
        helper.database_name
    );
    eprintln!("Scheduler Collection: test_scheduler");

    // Test 1: Basic update and retrieval
    {
        let data = TestSchedulerData {
            key: "test:scheduler_key_1".to_string(),
            content: "Test scheduler data #1".to_string(),
            version: 0,
        };

        // Update data in the scheduler store
        let version = helper
            .store
            .update_data(data.clone(), None)
            .await
            .err_tip(|| "Failed to update scheduler data")?
            .ok_or_else(|| make_err!(Code::Internal, "Expected version from update"))?;

        eprintln!("Created scheduler entry with version: {version}");

        // Retrieve and decode the data using a key lookup
        let key = TestSchedulerKey("test:scheduler_key_1".to_string());
        let retrieved = helper
            .store
            .get_and_decode(key)
            .await
            .err_tip(|| "Failed to get and decode scheduler data")?
            .ok_or_else(|| make_err!(Code::NotFound, "Scheduler data not found"))?;

        assert_eq!(retrieved.content, data.content);
        assert_eq!(retrieved.version, version);
        eprintln!("Successfully retrieved scheduler data with version {version}");
    }

    // Test 2: Versioned updates
    {
        let mut data = TestSchedulerData {
            key: "test:scheduler_key_2".to_string(),
            content: "Initial content".to_string(),
            version: 0,
        };

        // First update
        let version1 = helper
            .store
            .update_data(data.clone(), None)
            .await?
            .ok_or_else(|| make_err!(Code::Internal, "Expected version"))?;

        // Update with correct version
        data.content = "Updated content".to_string();
        data.version = version1;
        let version2 = helper
            .store
            .update_data(data.clone(), None)
            .await?
            .ok_or_else(|| make_err!(Code::Internal, "Expected version"))?;

        assert!(version2 > version1, "Version should increment");
        eprintln!("Version updated from {version1} to {version2}");

        // Try update with wrong version (should fail)
        data.content = "This should fail".to_string();
        data.version = version1; // Using old version
        let result = helper.store.update_data(data.clone(), None).await;

        assert!(result.is_err(), "Update with old version should fail");
        eprintln!("Correctly rejected update with stale version");
    }

    // Test 3: Search by index
    {
        // Define a custom search index provider
        struct SearchByContentPrefix {
            prefix: String,
        }

        impl SchedulerIndexProvider for SearchByContentPrefix {
            const KEY_PREFIX: &'static str = "test:";
            const INDEX_NAME: &'static str = "content_prefix";
            type Versioned = TrueValue;

            fn index_value(&self) -> std::borrow::Cow<'_, str> {
                std::borrow::Cow::Borrowed(&self.prefix)
            }
        }

        impl SchedulerStoreKeyProvider for SearchByContentPrefix {
            type Versioned = TrueValue;

            fn get_key(&self) -> StoreKey<'static> {
                StoreKey::Str(std::borrow::Cow::Owned("dummy_key".to_string()))
            }
        }

        impl SchedulerStoreDecodeTo for SearchByContentPrefix {
            type DecodeOutput = TestSchedulerData;

            fn decode(version: i64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
                TestSchedulerKey::decode(version, data)
            }
        }

        // Create multiple entries with indexed data
        for i in 0..5 {
            let data = TestSchedulerData {
                key: format!("test:search_key_{i}"),
                content: format!("Searchable content #{i}"),
                version: 0,
            };

            let version = helper.store.update_data(data, None).await?;
            assert_eq!(Some(1), version);
        }

        // Search by index prefix
        let search_provider = SearchByContentPrefix {
            prefix: "Searchable".to_string(),
        };

        let search_results: Vec<TestSchedulerData> = helper
            .store
            .search_by_index_prefix(search_provider)
            .await
            .err_tip(|| "Failed to search by index")?
            .try_collect()
            .await?;

        assert!(
            search_results.len() >= 5,
            "Should find at least 5 matching entries, got {}",
            search_results.len()
        );

        // Verify search results
        for result in &search_results {
            assert!(
                result.content.starts_with("Searchable"),
                "Content should match search pattern"
            );
        }
    }

    // Test 4: Test a different index search
    {
        struct SearchByTestIndex;

        impl SchedulerIndexProvider for SearchByTestIndex {
            const KEY_PREFIX: &'static str = "test:";
            const INDEX_NAME: &'static str = "test_index";
            type Versioned = TrueValue;

            fn index_value(&self) -> std::borrow::Cow<'_, str> {
                std::borrow::Cow::Borrowed("test_value")
            }
        }

        impl SchedulerStoreKeyProvider for SearchByTestIndex {
            type Versioned = TrueValue;

            fn get_key(&self) -> StoreKey<'static> {
                StoreKey::Str(std::borrow::Cow::Owned("dummy_key".to_string()))
            }
        }

        impl SchedulerStoreDecodeTo for SearchByTestIndex {
            type DecodeOutput = TestSchedulerData;

            fn decode(version: i64, data: Bytes) -> Result<Self::DecodeOutput, Error> {
                TestSchedulerKey::decode(version, data)
            }
        }

        let test_index_results: Vec<TestSchedulerData> = helper
            .store
            .search_by_index_prefix(SearchByTestIndex)
            .await?
            .try_collect()
            .await?;

        // All entries should have test_index = "test_value"
        assert!(
            !test_index_results.is_empty(),
            "Should find entries with test_index"
        );
        eprintln!(
            "Found {} entries with test_index='test_value'",
            test_index_results.len()
        );
    }

    eprintln!(
        "Scheduler store test completed successfully in database: {}",
        helper.database_name
    );

    // Database will NOT be cleaned up - it's retained for inspection
    Ok(())
}

#[nativelink_test]
async fn test_non_w_config() -> Result<(), Error> {
    let (mut spec, mongo_process) = TestMongoHelper::new_spec(None).await?;
    spec.write_concern_w = None;
    spec.write_concern_j = Some(true);
    spec.write_concern_timeout_ms = Some(1);

    assert_eq!(
        Error::new(Code::InvalidArgument, "write_concern_w not set, but j and/or timeout set. Please set 'write_concern_w' to a non-default value. See https://www.mongodb.com/docs/manual/reference/write-concern/#w-option for options.".to_string()),
        TestMongoHelper::new_with_spec_and_process(spec, mongo_process).await
        .unwrap_err()
    );
    Ok(())
}

#[nativelink_test]
async fn empty_request_permit() -> Result<(), Error> {
    let (mut spec, mongo_process) = TestMongoHelper::new_spec(None).await?;
    spec.max_requests = Some(0);

    assert_eq!(
        Error::new(
            Code::InvalidArgument,
            "max_request_permits was set to zero, which will block mongo_store from working at all"
                .to_string()
        ),
        TestMongoHelper::new_with_spec_and_process(spec, mongo_process)
            .await
            .unwrap_err()
    );
    Ok(())
}

#[nativelink_test]
async fn single_request_permit() -> Result<(), Error> {
    let (mut spec, mongo_process) = TestMongoHelper::new_spec(None).await?;
    spec.max_requests = Some(1);
    let helper = TestMongoHelper::new_with_spec_and_process(spec, mongo_process).await?;

    let data = Bytes::from_static(b"14");
    let digest = DigestInfo::try_new(VALID_HASH1, 2)?;
    helper.store.update_oneshot(digest, data.clone()).await?;
    let result = helper.store.has(digest).await?;
    assert!(
        result.is_some(),
        "Expected mongo store to have hash: {VALID_HASH1}",
    );

    assert!(logs_contain(
        "Number of waiting permits for Mongo waiting=0"
    ));

    Ok(())
}
