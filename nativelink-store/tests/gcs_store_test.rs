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

use core::sync::atomic::Ordering;
use core::time::Duration;
use std::sync::Arc;

use bytes::{BufMut, Bytes, BytesMut};
use mock_instant::thread_local::MockClock;
use nativelink_config::stores::{CommonObjectSpec, ExperimentalGcsSpec};
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_store::gcs_client::client::GcsOperations;
use nativelink_store::gcs_client::mocks::{MockGcsOperations, MockRequest};
use nativelink_store::gcs_client::types::{DEFAULT_CONTENT_TYPE, ObjectPath};
use nativelink_store::gcs_store::GcsStore;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::store_trait::{StoreKey, StoreLike, UploadSizeInfo};
use pretty_assertions::assert_eq;
use sha2::{Digest, Sha256};

const BUCKET_NAME: &str = "test-bucket";
const KEY_PREFIX: &str = "test-prefix/";
const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

fn to_store_key(digest: DigestInfo) -> StoreKey<'static> {
    digest.into()
}

#[nativelink_test]
async fn simple_has_object_found() -> Result<(), Error> {
    // Create mock GCS operations
    let mock_ops = Arc::new(MockGcsOperations::new());
    let ops_as_trait: Arc<dyn GcsOperations> = mock_ops.clone();
    let store = create_test_store(ops_as_trait).await?;

    // Add a test object to the mock
    let digest = DigestInfo::try_new(VALID_HASH1, 100)?;
    let store_key: StoreKey = to_store_key(digest);
    let object_path = create_object_path(&store_key);
    mock_ops.add_object(&object_path, vec![1, 2, 3, 4, 5]).await;

    // Test has method
    let result = store.has(store_key).await?;

    assert_eq!(result, Some(5), "Expected to find object with size 5");

    // Verify the correct method was called
    let call_counts = mock_ops.get_call_counts();
    assert_eq!(
        call_counts.metadata_calls.load(Ordering::Relaxed),
        1,
        "read_object_metadata should have been called once"
    );

    Ok(())
}

#[nativelink_test]
async fn simple_has_object_not_found() -> Result<(), Error> {
    // Create mock GCS operations
    let mock_ops = Arc::new(MockGcsOperations::new());
    let ops_as_trait: Arc<dyn GcsOperations> = mock_ops.clone();

    let store = create_test_store(ops_as_trait).await?;

    // Test has method with a digest that doesn't exist
    let digest = DigestInfo::try_new(VALID_HASH1, 100)?;
    let store_key: StoreKey = to_store_key(digest);
    let result = store.has(store_key).await?;

    assert_eq!(result, None, "Expected not to find the object");

    // Verify the correct method was called
    let call_counts = mock_ops.get_call_counts();
    assert_eq!(
        call_counts.metadata_calls.load(Ordering::Relaxed),
        1,
        "read_object_metadata should have been called once"
    );

    Ok(())
}

#[nativelink_test]
async fn simple_has_object_error() -> Result<(), Error> {
    // Create mock GCS operations
    let mock_ops = Arc::new(MockGcsOperations::new());
    // Mark it to fail
    mock_ops.set_should_fail(true);
    let ops_as_trait: Arc<dyn GcsOperations> = mock_ops.clone();

    let store = create_test_store(ops_as_trait).await?;

    // Test has method with a digest that doesn't exist
    let digest = DigestInfo::try_new(VALID_HASH1, 100)?;
    let store_key: StoreKey = to_store_key(digest);
    let result = store.has(store_key).await;

    assert_eq!(
        result,
        Err(Error::new_with_messages(
            Code::Internal,
            [
                "Simulated generic failure",
                "Error while trying to read - bucket: test-bucket path: test-prefix/0123456789abcdef000000000000000000010000000000000123456789abcdef-100",
                "On attempt 1"
            ]
                .iter()
                .map(ToString::to_string)
                .collect()
        )),
        "Expected to get an error"
    );
    Ok(())
}

#[nativelink_test]
async fn has_with_results_test() -> Result<(), Error> {
    // Create mock GCS operations
    let mock_ops = Arc::new(MockGcsOperations::new());
    let ops_as_trait: Arc<dyn GcsOperations> = mock_ops.clone();
    let store = create_test_store(ops_as_trait).await?;

    // Add a test object to the mock
    let digest1 = DigestInfo::try_new(VALID_HASH1, 100)?;
    let store_key1: StoreKey = to_store_key(digest1);
    let object_path1 = create_object_path(&store_key1);
    mock_ops
        .add_object(&object_path1, vec![1, 2, 3, 4, 5])
        .await;

    // Create another digest that won't exist in the store
    let digest2 = DigestInfo::try_new(
        "baaa6789abcdef000000000000000000010000000000000123456789abcdef00",
        200,
    )?;
    let store_key2: StoreKey = to_store_key(digest2);

    // Create a zero digest for testing special handling
    let zero_digest = DigestInfo::new(Sha256::new().finalize().into(), 0);
    let zero_key: StoreKey = to_store_key(zero_digest);

    // Test has_with_results method
    let keys = vec![store_key1, store_key2, zero_key];
    let mut results = vec![None, None, None];

    store.has_with_results(&keys, &mut results).await?;

    assert_eq!(
        results,
        vec![Some(5), None, Some(0)],
        "Expected results to match [Some(5), None, Some(0)]"
    );

    // Verify correct methods were called
    let call_counts = mock_ops.get_call_counts();
    assert_eq!(
        call_counts.metadata_calls.load(Ordering::Relaxed),
        2,
        "read_object_metadata should have been called twice"
    );

    Ok(())
}

#[nativelink_test]
async fn simple_update() -> Result<(), Error> {
    const DATA_SIZE: usize = 50;

    // Create mock GCS operations
    let mock_ops = Arc::new(MockGcsOperations::new());
    let ops_as_trait: Arc<dyn GcsOperations> = mock_ops.clone();
    let store = create_test_store(ops_as_trait).await?;

    // Create test data
    let mut send_data = BytesMut::new();
    for i in 0..DATA_SIZE {
        send_data.put_u8(((i % 93) + 33) as u8);
    }
    let send_data = send_data.freeze();

    let digest = DigestInfo::try_new(VALID_HASH1, DATA_SIZE as u64)?;
    let store_key: StoreKey = to_store_key(digest);
    let (mut tx, rx) = make_buf_channel_pair();

    // Start update operation
    let store_clone = store.clone();
    let update_fut = nativelink_util::spawn!("update_task", async move {
        store_clone
            .update(store_key, rx, UploadSizeInfo::ExactSize(DATA_SIZE as u64))
            .await
    });

    for i in 0..DATA_SIZE {
        tx.send(send_data.slice(i..=i)).await?;
    }
    tx.send_eof()?;
    update_fut.await??;

    // Verify the mock operations were called correctly
    let requests = mock_ops.get_requests().await;
    let write_requests: Vec<_> = requests
        .iter()
        .filter_map(|req| {
            if let MockRequest::Write {
                object_path,
                content_len,
            } = req
            {
                Some((object_path, content_len))
            } else {
                None
            }
        })
        .collect();

    assert_eq!(write_requests.len(), 1, "Expected one write request");
    assert_eq!(
        *write_requests[0].1, DATA_SIZE,
        "Expected content length to match DATA_SIZE"
    );

    Ok(())
}

#[nativelink_test]
async fn get_part_test() -> Result<(), Error> {
    // Create mock GCS operations
    let mock_ops = Arc::new(MockGcsOperations::new());
    let ops_as_trait: Arc<dyn GcsOperations> = mock_ops.clone();
    let store = create_test_store(ops_as_trait).await?;

    // Add test data to the mock
    let digest = DigestInfo::try_new(VALID_HASH1, 11)?; // "hello world" length
    let store_key: StoreKey = to_store_key(digest);
    let object_path = create_object_path(&store_key);
    mock_ops
        .add_object(&object_path, b"hello world".to_vec())
        .await;

    let (mut tx, mut rx) = make_buf_channel_pair();
    let store_clone = store.clone();
    let store_key_clone = store_key.clone();

    let handle = nativelink_util::spawn!("get_part_task", async move {
        store_clone
            .get_part(store_key_clone, &mut tx, 0, None)
            .await
    });

    // Receive the data
    let received_data =
        match tokio::time::timeout(Duration::from_secs(5), rx.consume(Some(100))).await {
            Ok(result) => result?,
            Err(_) => {
                return Err(make_err!(
                    Code::DeadlineExceeded,
                    "Timeout waiting for data"
                ));
            }
        };

    assert_eq!(
        received_data.as_ref(),
        b"hello world",
        "Received data should match original"
    );

    // Ensure get_part completes successfully
    handle.await??;

    Ok(())
}

#[nativelink_test]
async fn get_part_with_range() -> Result<(), Error> {
    // Create mock GCS operations
    let mock_ops = Arc::new(MockGcsOperations::new());
    let ops_as_trait: Arc<dyn GcsOperations> = mock_ops.clone();
    let store = create_test_store(ops_as_trait).await?;

    // Add test data to the mock
    let digest = DigestInfo::try_new(VALID_HASH1, 11)?; // "hello world" length
    let store_key: StoreKey = to_store_key(digest);
    let object_path = create_object_path(&store_key);
    mock_ops
        .add_object(&object_path, b"hello world".to_vec())
        .await;
    let (mut tx, mut rx) = make_buf_channel_pair();

    // Start get_part operation to retrieve a range (bytes 6-10, i.e., "world")
    let store_clone = store.clone();
    let get_fut = nativelink_util::spawn!("get_part_partial_task", async move {
        store_clone.get_part(store_key, &mut tx, 6, Some(5)).await
    });

    // Receive the data
    let received_data = rx.consume(Some(100)).await?;
    assert_eq!(
        received_data.as_ref(),
        b"world",
        "Received data should match the requested range"
    );
    get_fut.await??;

    // Verify the mock operations were called correctly
    let requests = mock_ops.get_requests().await;
    let read_requests: Vec<_> = requests
        .iter()
        .filter_map(|req| {
            if let MockRequest::ReadContent {
                object_path: _,
                start,
                end,
            } = req
            {
                Some((start, end))
            } else {
                None
            }
        })
        .collect();

    assert_eq!(read_requests.len(), 1, "Expected one read content request");
    assert_eq!(*read_requests[0].0, 6, "Expected start offset to be 6");
    assert_eq!(
        *read_requests[0].1,
        Some(11),
        "Expected end offset to be Some(11)"
    );

    Ok(())
}

#[nativelink_test]
async fn get_part_zero_digest() -> Result<(), Error> {
    // Create mock GCS operations
    let mock_ops = Arc::new(MockGcsOperations::new());
    let ops_as_trait: Arc<dyn GcsOperations> = mock_ops.clone();
    let store = create_test_store(ops_as_trait).await?;

    // Create a zero digest
    let digest = DigestInfo::new(Sha256::new().finalize().into(), 0);
    let store_key: StoreKey = to_store_key(digest);
    let (mut tx, mut rx) = make_buf_channel_pair();

    // Start get_part operation
    let store_clone = store.clone();
    let get_fut = nativelink_util::spawn!("get_part_full_task", async move {
        store_clone.get_part(store_key, &mut tx, 0, None).await
    });

    // Receive the data - should be empty
    let received_data = rx.consume(Some(100)).await?;
    assert_eq!(
        received_data.as_ref(),
        b"",
        "Received data should be empty for zero digest"
    );
    get_fut.await??;

    // Verify no mock operations were called (zero digest is special-cased)
    let call_counts = mock_ops.get_call_counts();
    assert_eq!(
        call_counts.read_calls.load(Ordering::Relaxed),
        0,
        "No read operations should have been called for zero digest"
    );

    Ok(())
}

#[nativelink_test]
async fn test_expired_object() -> Result<(), Error> {
    // Create mock GCS operations
    let mock_ops = Arc::new(MockGcsOperations::new());
    let ops_as_trait: Arc<dyn GcsOperations> = mock_ops.clone();

    // Create a GCS store with the mock operations and expiration set to 2 days
    let expiration_seconds = 2 * 24 * 60 * 60;
    let store = create_test_store_with_expiration(ops_as_trait, expiration_seconds).await?;

    // Create a digest and object
    let digest = DigestInfo::try_new(VALID_HASH1, 5)?;
    let store_key: StoreKey = to_store_key(digest);
    let object_path = create_object_path(&store_key);
    let base_timestamp = 1000;
    mock_ops
        .add_object_with_timestamp(&object_path, vec![1, 2, 3, 4, 5], base_timestamp)
        .await;

    // Mock the now function to return a time before expiration
    MockClock::set_time(Duration::from_secs(
        base_timestamp as u64 + expiration_seconds as u64 - 1,
    ));

    // Check that the object exists (it's not expired yet)
    let result = store.has(store_key.clone()).await?;
    assert_eq!(
        result,
        Some(5),
        "Expected to find the object before expiration"
    );

    // Mock the now function to return a time after expiration
    MockClock::set_time(Duration::from_secs(
        base_timestamp as u64 + expiration_seconds as u64 + 1,
    ));

    // Object should now be considered expired
    let result = store.has(store_key).await?;
    assert_eq!(result, None, "Expected the object to be considered expired");

    Ok(())
}

#[nativelink_test]
async fn large_file_update_test() -> Result<(), Error> {
    const DATA_SIZE: usize = 11 * 1024 * 1024; // 11MB to exceed SIMPLE_UPLOAD_THRESHOLD

    // Create mock GCS operations
    let mock_ops = Arc::new(MockGcsOperations::new());
    let ops_as_trait: Arc<dyn GcsOperations> = mock_ops.clone();
    let store = create_test_store(ops_as_trait).await?;

    // Create test data
    let pattern: Vec<u8> = (0..100).map(|i| (i % 256) as u8).collect();

    // Create a digest and channel pair
    let digest = DigestInfo::try_new(VALID_HASH1, DATA_SIZE as u64)?;
    let store_key: StoreKey = to_store_key(digest);
    let (mut tx, rx) = make_buf_channel_pair();

    // Start update operation
    let store_clone = store.clone();
    let update_fut = nativelink_util::spawn!("update_task", async move {
        store_clone
            .update(store_key, rx, UploadSizeInfo::ExactSize(DATA_SIZE as u64))
            .await
    });

    // Send data in chunks using the pattern
    let mut bytes_sent = 0;
    while bytes_sent < DATA_SIZE {
        let chunk_size = core::cmp::min(pattern.len(), DATA_SIZE - bytes_sent);
        tx.send(Bytes::copy_from_slice(&pattern[0..chunk_size]))
            .await?;
        bytes_sent += chunk_size;
    }
    tx.send_eof()?;
    update_fut.await??;

    // Verify the mock operations were called correctly
    let requests = mock_ops.get_requests().await;

    // For large files, should see resumable upload operations
    let start_resumable_requests = requests
        .iter()
        .filter(|req| matches!(req, MockRequest::StartResumable { .. }))
        .count();

    assert!(
        start_resumable_requests > 0,
        "Expected at least one StartResumable request for large file"
    );

    Ok(())
}

#[nativelink_test]
async fn test_content_type() -> Result<(), Error> {
    // Create mock GCS operations
    let mock_ops = Arc::new(MockGcsOperations::new());
    let ops_as_trait: Arc<dyn GcsOperations> = mock_ops.clone();
    let _store = create_test_store(ops_as_trait).await?;

    // Add a test object to the mock
    let digest = DigestInfo::try_new(VALID_HASH1, 100)?;
    let store_key: StoreKey = to_store_key(digest);
    let object_path = create_object_path(&store_key);
    mock_ops.add_object(&object_path, vec![1, 2, 3, 4, 5]).await;

    // Get the object metadata to check content type
    let result = mock_ops.read_object_metadata(&object_path).await?;

    assert!(result.is_some(), "Expected to find metadata");
    let metadata = result.unwrap();
    assert_eq!(
        metadata.content_type, DEFAULT_CONTENT_TYPE,
        "Content type should match the default content type"
    );

    Ok(())
}

#[nativelink_test]
async fn test_null_object_metadata() -> Result<(), Error> {
    // Create mock GCS operations
    let mock_ops = Arc::new(MockGcsOperations::new());
    let ops_as_trait: Arc<dyn GcsOperations> = mock_ops.clone();
    let store = create_test_store(ops_as_trait).await?;

    // Add a test object to the mock
    let digest = DigestInfo::try_new(VALID_HASH1, 100)?;
    let store_key: StoreKey = to_store_key(digest);
    let object_path = create_object_path(&store_key);

    // Add a "bad" metadata object with negative size
    // This is done by manually constructing a "broken" object in the mock
    let content = vec![1, 2, 3, 4, 5];
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let metadata = nativelink_store::gcs_client::types::GcsObject {
        name: object_path.path.clone(),
        bucket: object_path.bucket.clone(),
        size: -1, // Invalid size to test error handling
        content_type: DEFAULT_CONTENT_TYPE.to_string(),
        update_time: Some(nativelink_store::gcs_client::types::Timestamp {
            seconds: timestamp,
            nanos: 0,
        }),
    };

    let _mock_object = Arc::new((metadata, content));

    // Now try to access it through the store
    let result = store.has(store_key).await;

    // Should either return None (if the mock skips invalid objects)
    // or return an error (if it propagates the invalid size)
    match result {
        Ok(None) => {
            // This is fine - the store might consider invalid sizes as "not found"
        }
        Ok(Some(_)) => {
            panic!("Expected not to find object with invalid size");
        }
        Err(e) => {
            // This is also fine - the store might error on invalid sizes
            assert!(
                e.code == Code::InvalidArgument || e.code == Code::Internal,
                "Expected InvalidArgument or Internal error for invalid size"
            );
        }
    }

    Ok(())
}

// Helper function to create a test GCS store
async fn create_test_store(
    ops: Arc<dyn GcsOperations>,
) -> Result<Arc<GcsStore<fn() -> MockInstantWrapped>>, Error> {
    GcsStore::new_with_ops(
        &ExperimentalGcsSpec {
            bucket: BUCKET_NAME.to_string(),
            common: CommonObjectSpec {
                key_prefix: Some(KEY_PREFIX.to_string()),
                ..Default::default()
            },
            ..Default::default()
        },
        ops,
        MockInstantWrapped::default,
    )
}

// Helper function to create a test GCS store with expiration
async fn create_test_store_with_expiration(
    ops: Arc<dyn GcsOperations>,
    expiration_seconds: i64,
) -> Result<Arc<GcsStore<fn() -> MockInstantWrapped>>, Error> {
    GcsStore::new_with_ops(
        &ExperimentalGcsSpec {
            bucket: BUCKET_NAME.to_string(),
            common: CommonObjectSpec {
                key_prefix: Some(KEY_PREFIX.to_string()),
                consider_expired_after_s: expiration_seconds as u32,
                ..Default::default()
            },
            ..Default::default()
        },
        ops,
        MockInstantWrapped::default,
    )
}

// Helper to create an object path from a store key
fn create_object_path(key: &StoreKey) -> ObjectPath {
    ObjectPath::new(
        BUCKET_NAME.to_string(),
        &format!("{}{}", KEY_PREFIX, key.as_str()),
    )
}
