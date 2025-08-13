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

const VALID_HASH1: &str = "3031323334353637383961626364656630303030303030303030303030303030";

/// Creates a test `MongoDB` specification.
/// Note: These tests are designed to work with a real `MongoDB` instance.
/// For CI/local development, ensure `MongoDB` is available or skip these tests.
fn create_test_spec_with_key_prefix(key_prefix: Option<String>) -> ExperimentalMongoSpec {
    // Default connection string for CI - empty string will require env var
    let default_connection = String::new();

    // Create database name with timestamp for uniqueness
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();

    ExperimentalMongoSpec {
        connection_string: std::env::var("NATIVELINK_TEST_MONGO_URL").unwrap_or(default_connection),
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
    }
}

fn create_test_spec() -> ExperimentalMongoSpec {
    create_test_spec_with_key_prefix(Some("test:".to_string()))
}

/// Test helper that manages `MongoDB` database lifecycle
#[derive(Debug)]
pub struct TestMongoHelper {
    pub store: Arc<ExperimentalMongoStore>,
    pub database_name: String,
}

fn non_ci_test_store_access() {
    // Check if this is a local development environment without credentials
    let is_ci = std::env::var("CI").is_ok() || std::env::var("GITHUB_ACTIONS").is_ok();

    if !is_ci {
        eprintln!("\n🔒 MongoDB tests require access to the NativeLink test database.");
        eprintln!("   For local development access, please email: marcus@tracemachina.com");
        eprintln!("   ");
        eprintln!("   Alternatively, you can set NATIVELINK_TEST_MONGO_URL environment variable");
        eprintln!("   with your own MongoDB connection string.");
        eprintln!("   ");
        eprintln!("   Skipping MongoDB tests for now...\n");
    }
}

impl TestMongoHelper {
    /// Creates a new test store and database
    async fn new(key_prefix: Option<String>) -> Result<Self, Error> {
        let spec = create_test_spec_with_key_prefix(key_prefix);

        // Check if we have a connection string
        if spec.connection_string.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "MongoDB connection string not provided for local testing".to_string(),
            ));
        }

        let database_name = spec.database.clone();

        let store = ExperimentalMongoStore::new(spec).await?;

        Ok(Self {
            store,
            database_name,
        })
    }

    /// Creates a test helper, skipping if `MongoDB` is not available
    async fn new_or_skip_with_key_prefix(key_prefix: Option<String>) -> Result<Self, Error> {
        match Self::new(key_prefix).await {
            Ok(helper) => Ok(helper),
            Err(e) => {
                if e.code == Code::Unavailable
                    && e.messages[0].contains("connection string not provided")
                {
                    non_ci_test_store_access();
                }

                // Convert to a test skip-friendly error
                if e.code == Code::InvalidArgument || e.code == Code::Unavailable {
                    Err(Error::new(
                        Code::Unavailable,
                        "MongoDB not available for testing - skipping test".to_string(),
                    ))
                } else {
                    Err(e)
                }
            }
        }
    }

    async fn new_or_skip() -> Result<Self, Error> {
        Self::new_or_skip_with_key_prefix(Some("test:".to_string())).await
    }
}

impl Drop for TestMongoHelper {
    fn drop(&mut self) {
        // No longer cleaning up databases - we want to keep them for inspection
        eprintln!(
            "Test database '{}' retained for inspection",
            self.database_name
        );
    }
}

/// Creates a test `MongoDB` store with a unique database name to avoid conflicts.
async fn create_test_store() -> Result<Arc<ExperimentalMongoStore>, Error> {
    let spec = create_test_spec();

    // Check if we have a connection string
    if spec.connection_string.is_empty() {
        non_ci_test_store_access();

        return Err(Error::new(
            Code::Unavailable,
            "MongoDB connection string not provided for local testing".to_string(),
        ));
    }

    ExperimentalMongoStore::new(spec).await
}

/// Utility to check if `MongoDB` is available for testing.
/// Returns an error that can be used to skip tests when `MongoDB` is not available.
async fn check_mongodb_available() -> Result<(), Error> {
    TestMongoHelper::new_or_skip().await.map(|_| ())
}

#[nativelink_test]
async fn upload_and_get_data() -> Result<(), Error> {
    // Create test helper with automatic cleanup
    let Ok(helper) = TestMongoHelper::new_or_skip().await else {
        eprintln!("Skipping MongoDB test - MongoDB not available");
        return Ok(());
    };

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
    let Ok(helper) = TestMongoHelper::new_or_skip_with_key_prefix(None).await else {
        eprintln!("Skipping MongoDB test - MongoDB not available");
        return Ok(());
    };

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
    // Skip test if MongoDB is not available
    if check_mongodb_available().await.is_err() {
        eprintln!("Skipping MongoDB test - MongoDB not available");
        return Ok(());
    }

    let data = Bytes::from_static(b"");
    let digest = ZERO_BYTE_DIGESTS[0];

    let store = create_test_store().await?;

    store.update_oneshot(digest, data).await?;

    let result = store.has(digest).await?;
    assert!(
        result.is_some(),
        "Expected mongo store to have zero-byte hash",
    );

    Ok(())
}

#[nativelink_test]
#[allow(clippy::items_after_statements)]
async fn test_large_downloads_are_chunked() -> Result<(), Error> {
    // Skip test if MongoDB is not available
    if check_mongodb_available().await.is_err() {
        eprintln!("Skipping MongoDB test - MongoDB not available");
        return Ok(());
    }

    const READ_CHUNK_SIZE: usize = 1024;
    let data = Bytes::from(vec![0u8; READ_CHUNK_SIZE + 128]);

    let digest = DigestInfo::try_new(VALID_HASH1, data.len())?;

    let mut spec = create_test_spec();
    spec.read_chunk_size = READ_CHUNK_SIZE;

    let store = ExperimentalMongoStore::new(spec).await?;

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
    // Skip test if MongoDB is not available
    if check_mongodb_available().await.is_err() {
        eprintln!("Skipping MongoDB test - MongoDB not available");
        return Ok(());
    }

    const READ_CHUNK_SIZE: usize = 1024;
    let data_p1 = Bytes::from(vec![b'A'; READ_CHUNK_SIZE + 512]);
    let data_p2 = Bytes::from(vec![b'B'; READ_CHUNK_SIZE]);

    let mut data = Vec::new();
    data.extend_from_slice(&data_p1);
    data.extend_from_slice(&data_p2);
    let data = Bytes::from(data);

    let digest = DigestInfo::try_new(VALID_HASH1, data.len())?;

    let mut spec = create_test_spec();
    spec.read_chunk_size = READ_CHUNK_SIZE;

    let store = ExperimentalMongoStore::new(spec).await?;

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
    // Skip test if MongoDB is not available
    if check_mongodb_available().await.is_err() {
        eprintln!("Skipping MongoDB test - MongoDB not available");
        return Ok(());
    }

    let digest = DigestInfo::try_new(VALID_HASH1, 0)?;

    let store = create_test_store().await?;

    // Try to get a zero-length item that doesn't exist
    let result = store.get_part_unchunked(digest, 0, None).await;
    assert_eq!(result.unwrap_err().code, Code::NotFound);

    Ok(())
}

#[nativelink_test]
async fn test_invalid_connection_string() -> Result<(), Error> {
    let mut spec = create_test_spec();
    spec.connection_string = "invalid://connection_string".to_string();

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
    let mut spec = create_test_spec();
    spec.connection_string = String::new();

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
    // Skip test if MongoDB is not available
    if check_mongodb_available().await.is_err() {
        eprintln!("Skipping MongoDB test - MongoDB not available");
        return Ok(());
    }

    let mut spec = create_test_spec();
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
    let store = ExperimentalMongoStore::new(spec).await?;

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
    // Skip test if MongoDB is not available
    if check_mongodb_available().await.is_err() {
        eprintln!("Skipping MongoDB test - MongoDB not available");
        return Ok(());
    }

    let store = create_test_store().await?;
    let digest = DigestInfo::try_new(VALID_HASH1, 2)?;
    let (tx, rx) = make_buf_channel_pair();

    // Add timeout to prevent hanging - this test should fail quickly
    let result = tokio::time::timeout(core::time::Duration::from_secs(5), async {
        tokio::join!(
            async {
                store
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
    let Ok(helper) = TestMongoHelper::new_or_skip().await else {
        eprintln!("Skipping MongoDB test - MongoDB not available");
        return Ok(());
    };

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
    let spec = create_test_spec();
    let connection_string = spec.connection_string.clone();
    let database_name = spec.database.clone();

    // Skip if MongoDB is not available
    let Ok(client_options) = ClientOptions::parse(&connection_string).await else {
        eprintln!("Skipping MongoDB test - MongoDB not available");
        return Ok(());
    };

    let Ok(client) = MongoClient::with_options(client_options) else {
        eprintln!("Skipping MongoDB test - MongoDB not available");
        return Ok(());
    };

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
    {
        let store = ExperimentalMongoStore::new(spec.clone()).await?;
        let helper = TestMongoHelper {
            store,
            database_name: database_name.clone(),
        };

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

        // helper goes out of scope here, but NO cleanup happens anymore
    }

    // Database should still exist after helper is dropped
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
    let Ok(helper) = TestMongoHelper::new_or_skip().await else {
        eprintln!("Skipping MongoDB test - MongoDB not available");
        return Ok(());
    };

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
    let spec = create_test_spec();
    let client_options = ClientOptions::parse(&spec.connection_string)
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
        if doc_count < 3 {
            if let Ok(key) = doc.get_str("_id") {
                eprintln!("  - Key: {key}");
                if let Ok(size) = doc.get_i64("size") {
                    eprintln!("    Size: {size} bytes");
                }
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

#[nativelink_test]
async fn test_scheduler_store_operations() -> Result<(), Error> {
    // Create test helper
    let Ok(helper) = TestMongoHelper::new_or_skip().await else {
        eprintln!("Skipping MongoDB test - MongoDB not available");
        return Ok(());
    };

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
            .update_data(data.clone())
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
            .update_data(data.clone())
            .await?
            .ok_or_else(|| make_err!(Code::Internal, "Expected version"))?;

        // Update with correct version
        data.content = "Updated content".to_string();
        data.version = version1;
        let version2 = helper
            .store
            .update_data(data.clone())
            .await?
            .ok_or_else(|| make_err!(Code::Internal, "Expected version"))?;

        assert!(version2 > version1, "Version should increment");
        eprintln!("Version updated from {version1} to {version2}");

        // Try update with wrong version (should fail)
        data.content = "This should fail".to_string();
        data.version = version1; // Using old version
        let result = helper.store.update_data(data.clone()).await;

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

            helper.store.update_data(data).await?;
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

        eprintln!("Found {} entries matching search", search_results.len());
        assert!(
            search_results.len() >= 5,
            "Should find at least 5 matching entries"
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
    assert_eq!(
        Error::new(Code::InvalidArgument, "write_concern_w not set, but j and/or timeout set. Please set 'write_concern_w' to a non-default value. See https://www.mongodb.com/docs/manual/reference/write-concern/#w-option for options.".to_string()),
        ExperimentalMongoStore::new(ExperimentalMongoSpec {
            connection_string: "mongodb://dummy".to_string(),
            database: "dummy".to_string(),
            cas_collection: "test_cas".to_string(),
            scheduler_collection: "test_scheduler".to_string(),
            key_prefix: None,
            read_chunk_size: 1024,
            max_concurrent_uploads: 10,
            connection_timeout_ms: 10000,
            command_timeout_ms: 15000,
            enable_change_streams: false,
            write_concern_w: None,
            write_concern_j: Some(true),
            write_concern_timeout_ms: Some(1),
        })
        .await
        .unwrap_err()
    );
    Ok(())
}
