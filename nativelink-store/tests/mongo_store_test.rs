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

use std::sync::Arc;

use bytes::Bytes;
use mongodb::Client as MongoClient;
use mongodb::options::ClientOptions;
use nativelink_config::stores::ExperimentalMongoSpec;
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_store::cas_utils::ZERO_BYTE_DIGESTS;
use nativelink_store::mongo_store::ExperimentalMongoStore;
use nativelink_util::background_spawn;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::{StoreLike, UploadSizeInfo};
use pretty_assertions::assert_eq;
use uuid::Uuid;

const VALID_HASH1: &str = "3031323334353637383961626364656630303030303030303030303030303030";

/// Creates a test `MongoDB` specification.
/// Note: These tests are designed to work with a real `MongoDB` instance.
/// For CI/local development, ensure `MongoDB` is available or skip these tests.
fn create_test_spec() -> ExperimentalMongoSpec {
    // Default connection string for CI - empty string will require env var
    let default_connection = String::new();

    ExperimentalMongoSpec {
        connection_string: std::env::var("NATIVELINK_TEST_MONGO_URL").unwrap_or(default_connection),
        database: format!("nltest{}", &Uuid::new_v4().simple().to_string()[..8]),
        cas_collection: "test_cas".to_string(),
        scheduler_collection: "test_scheduler".to_string(),
        key_prefix: Some("test:".to_string()),
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

/// Test helper that manages `MongoDB` database lifecycle
#[derive(Debug)]
pub struct TestMongoHelper {
    pub store: Arc<ExperimentalMongoStore>,
    pub database_name: String,
    connection_string: String,
}

impl TestMongoHelper {
    /// Creates a new test store and database
    async fn new() -> Result<Self, Error> {
        let spec = create_test_spec();

        // Check if we have a connection string
        if spec.connection_string.is_empty() {
            return Err(Error::new(
                Code::Unavailable,
                "MongoDB connection string not provided for local testing".to_string(),
            ));
        }

        let database_name = spec.database.clone();
        let connection_string = spec.connection_string.clone();

        let store = ExperimentalMongoStore::new(spec).await?;

        Ok(Self {
            store,
            database_name,
            connection_string,
        })
    }

    /// Creates a test helper, skipping if `MongoDB` is not available
    async fn new_or_skip() -> Result<Self, Error> {
        match Self::new().await {
            Ok(helper) => Ok(helper),
            Err(e) => {
                // Check if this is a local development environment without credentials
                let is_ci = std::env::var("CI").is_ok() || std::env::var("GITHUB_ACTIONS").is_ok();

                if !is_ci
                    && e.code == Code::Unavailable
                    && e.messages[0].contains("connection string not provided")
                {
                    eprintln!("\n🔒 MongoDB tests require access to the NativeLink test database.");
                    eprintln!(
                        "   For local development access, please email: marcus@tracemachina.com"
                    );
                    eprintln!("   ");
                    eprintln!(
                        "   Alternatively, you can set NATIVELINK_TEST_MONGO_URL environment variable"
                    );
                    eprintln!("   with your own MongoDB connection string.");
                    eprintln!("   ");
                    eprintln!("   Skipping MongoDB tests for now...\n");
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
}

impl Drop for TestMongoHelper {
    fn drop(&mut self) {
        // Schedule database cleanup in the background
        let database_name = self.database_name.clone();
        let connection_string = self.connection_string.clone();

        // Spawn cleanup task
        let _cleanup_task = background_spawn!("mongo_test_cleanup", async move {
            if let Err(e) = cleanup_test_database(&connection_string, &database_name).await {
                eprintln!("Failed to cleanup test database {database_name}: {e}");
            }
        });
    }
}

/// Clean up test database
async fn cleanup_test_database(connection_string: &str, database_name: &str) -> Result<(), Error> {
    let client_options = ClientOptions::parse(connection_string)
        .await
        .map_err(|e| make_err!(Code::Internal, "Failed to parse connection string: {e}"))?;

    let client = MongoClient::with_options(client_options)
        .map_err(|e| make_err!(Code::Internal, "Failed to create MongoDB client: {e}"))?;

    client
        .database(database_name)
        .drop(None)
        .await
        .map_err(|e| make_err!(Code::Internal, "Failed to drop test database: {e}"))?;

    Ok(())
}

/// Creates a test `MongoDB` store with a unique database name to avoid conflicts.
async fn create_test_store() -> Result<Arc<ExperimentalMongoStore>, Error> {
    let spec = create_test_spec();

    // Check if we have a connection string
    if spec.connection_string.is_empty() {
        // Check if this is a local development environment without credentials
        let is_ci = std::env::var("CI").is_ok() || std::env::var("GITHUB_ACTIONS").is_ok();

        if !is_ci {
            eprintln!("\n🔒 MongoDB tests require access to the NativeLink test database.");
            eprintln!("   For local development access, please email: marcus@tracemachina.com");
            eprintln!("   ");
            eprintln!(
                "   Alternatively, you can set NATIVELINK_TEST_MONGO_URL environment variable"
            );
            eprintln!("   with your own MongoDB connection string.");
            eprintln!("   ");
            eprintln!("   Skipping MongoDB tests for now...\n");
        }

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
    let helper = match TestMongoHelper::new_or_skip().await {
        Ok(h) => h,
        Err(_) => {
            eprintln!("Skipping MongoDB test - MongoDB not available");
            return Ok(());
        }
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
async fn upload_and_get_data_with_prefix() -> Result<(), Error> {
    // Create test helper with automatic cleanup
    let helper = match TestMongoHelper::new_or_skip().await {
        Ok(h) => h,
        Err(_) => {
            eprintln!("Skipping MongoDB test - MongoDB not available");
            return Ok(());
        }
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
    let helper = match TestMongoHelper::new_or_skip().await {
        Ok(h) => h,
        Err(_) => {
            eprintln!("Skipping MongoDB test - MongoDB not available");
            return Ok(());
        }
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

/// Test that verifies database creation and cleanup lifecycle
#[nativelink_test]
async fn test_database_lifecycle() -> Result<(), Error> {
    let spec = create_test_spec();
    let connection_string = spec.connection_string.clone();
    let database_name = spec.database.clone();

    // Skip if MongoDB is not available
    let client_options = match ClientOptions::parse(&connection_string).await {
        Ok(opts) => opts,
        Err(_) => {
            eprintln!("Skipping MongoDB test - MongoDB not available");
            return Ok(());
        }
    };

    let client = match MongoClient::with_options(client_options) {
        Ok(c) => c,
        Err(_) => {
            eprintln!("Skipping MongoDB test - MongoDB not available");
            return Ok(());
        }
    };

    // Verify database doesn't exist initially
    let db_names = client
        .list_database_names(None, None)
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
            connection_string: connection_string.clone(),
        };

        // Upload some data to ensure database is created
        let data = Bytes::from_static(b"test_database_lifecycle");
        let digest = DigestInfo::try_new(VALID_HASH1, data.len())?;
        helper.store.update_oneshot(digest, data.clone()).await?;

        // Verify database now exists (with retry for MongoDB lazy creation)
        let mut db_exists = false;
        for _attempt in 0..5 {
            let db_names = client
                .list_database_names(None, None)
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
            client
                .list_database_names(None, None)
                .await
                .unwrap_or_default()
        );

        // Verify data is accessible
        let result = helper.store.has(digest).await?;
        assert!(
            result.is_some(),
            "Data should be accessible in the database"
        );

        // helper goes out of scope here, triggering cleanup
    }

    // Give cleanup task time to complete
    tokio::time::sleep(core::time::Duration::from_millis(1000)).await;

    // Verify database is cleaned up
    let db_names = client
        .list_database_names(None, None)
        .await
        .map_err(|e| make_err!(Code::Internal, "Failed to list databases: {e}"))?;
    assert!(
        !db_names.contains(&database_name),
        "Test database should be cleaned up after drop"
    );

    Ok(())
}
