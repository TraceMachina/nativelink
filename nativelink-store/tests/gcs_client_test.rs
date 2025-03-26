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

use bytes::{BufMut, Bytes, BytesMut};
use futures::join;
use nativelink_error::{Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_store::gcs_client::mocks::MockGcsOperations;
use nativelink_store::gcs_client::operations::GcsOperations;
use nativelink_store::gcs_client::types::ObjectPath;
use nativelink_util::buf_channel::make_buf_channel_pair;

const BUCKET_NAME: &str = "test-bucket";
const OBJECT_PATH: &str = "test-folder/test-object";

#[nativelink_test]
async fn test_read_object_metadata_found() -> Result<(), Error> {
    // Create a mock implementation
    let mock_ops = Arc::new(MockGcsOperations::new());
    let object_path = ObjectPath::new(BUCKET_NAME.to_string(), OBJECT_PATH);
    let test_content = vec![1, 2, 3, 4, 5];
    mock_ops.add_object(&object_path, test_content).await;

    // Test the read_object_metadata method
    let result = mock_ops.read_object_metadata(object_path.clone()).await?;

    // Verify the result
    assert!(result.is_some(), "Expected to find metadata");
    let metadata = result.unwrap();
    assert_eq!(metadata.name, OBJECT_PATH);
    assert_eq!(metadata.bucket, BUCKET_NAME);
    assert_eq!(metadata.size, 5);
    assert_eq!(metadata.content_type, "application/octet-stream");
    assert!(metadata.update_time.is_some());

    // Verify call counts
    let call_counts = mock_ops.get_call_counts().await;
    assert_eq!(call_counts.metadata_calls, 1);

    Ok(())
}

#[nativelink_test]
async fn test_read_object_metadata_not_found() -> Result<(), Error> {
    // Create a mock implementation
    let mock_ops = Arc::new(MockGcsOperations::new());

    // Create an object path for an object that doesn't exist
    let object_path = ObjectPath::new(BUCKET_NAME.to_string(), "nonexistent-object");
    let result = mock_ops.read_object_metadata(object_path).await?;

    // Verify the result
    assert!(result.is_none(), "Expected not to find metadata");

    // Verify call counts
    let call_counts = mock_ops.get_call_counts().await;
    assert_eq!(call_counts.metadata_calls, 1);

    Ok(())
}

#[nativelink_test]
async fn test_read_object_content() -> Result<(), Error> {
    // Create a mock implementation
    let mock_ops = Arc::new(MockGcsOperations::new());
    let object_path = ObjectPath::new(BUCKET_NAME.to_string(), OBJECT_PATH);

    // Add a mock object with content "hello world"
    let test_content = b"hello world".to_vec();
    mock_ops
        .add_object(&object_path, test_content.clone())
        .await;

    // Test reading the full content
    let result = mock_ops
        .read_object_content(object_path.clone(), 0, None)
        .await?;
    assert_eq!(result, test_content);

    // Test reading a range of content
    let result = mock_ops
        .read_object_content(object_path.clone(), 6, Some(11))
        .await?;
    assert_eq!(result, b"world");

    // Verify call counts
    let call_counts = mock_ops.get_call_counts().await;
    assert_eq!(call_counts.read_calls, 2);

    Ok(())
}

#[nativelink_test]
async fn test_write_object() -> Result<(), Error> {
    // Create a mock implementation
    let mock_ops = Arc::new(MockGcsOperations::new());
    let object_path = ObjectPath::new(BUCKET_NAME.to_string(), OBJECT_PATH);

    // Test writing an object
    let test_content = b"test content".to_vec();
    mock_ops
        .write_object(&object_path, test_content.clone())
        .await?;

    // Verify the object was stored
    let result = mock_ops
        .read_object_content(object_path.clone(), 0, None)
        .await?;
    assert_eq!(result, test_content);

    // Verify call counts
    let call_counts = mock_ops.get_call_counts().await;
    assert_eq!(call_counts.write_calls, 1);
    assert_eq!(call_counts.read_calls, 1);

    Ok(())
}

#[nativelink_test]
async fn test_resumable_upload() -> Result<(), Error> {
    // Create a mock implementation
    let mock_ops = Arc::new(MockGcsOperations::new());
    let object_path = ObjectPath::new(BUCKET_NAME.to_string(), OBJECT_PATH);

    // Start a resumable upload
    let upload_id = mock_ops.start_resumable_write(&object_path).await?;
    assert!(!upload_id.is_empty(), "Expected non-empty upload ID");

    // Upload chunks
    let chunk1 = b"first chunk ".to_vec();
    let chunk2 = b"second chunk".to_vec();

    // Upload first chunk
    mock_ops
        .upload_chunk(
            &upload_id,
            &object_path,
            chunk1.clone(),
            0,
            chunk1.len() as i64,
            false,
        )
        .await?;

    // Upload second chunk and mark as final
    mock_ops
        .upload_chunk(
            &upload_id,
            &object_path,
            chunk2.clone(),
            chunk1.len() as i64,
            (chunk1.len() + chunk2.len()) as i64,
            true,
        )
        .await?;

    // Verify the content
    let result = mock_ops
        .read_object_content(object_path.clone(), 0, None)
        .await?;
    assert_eq!(result, [&chunk1[..], &chunk2[..]].concat());

    // Verify call counts
    let call_counts = mock_ops.get_call_counts().await;
    assert_eq!(call_counts.start_resumable_calls, 1);
    assert_eq!(call_counts.upload_chunk_calls, 2);
    assert_eq!(call_counts.read_calls, 1);

    Ok(())
}

#[nativelink_test]
async fn test_upload_from_reader() -> Result<(), Error> {
    // Create a mock implementation
    let mock_ops = Arc::new(MockGcsOperations::new());
    let object_path = ObjectPath::new(BUCKET_NAME.to_string(), OBJECT_PATH);

    // Create data to upload
    let data_size = 100;
    let mut send_data = BytesMut::new();
    for i in 0..data_size {
        send_data.put_u8(((i % 93) + 33) as u8);
    }
    let send_data = send_data.freeze();
    let (mut tx, rx) = make_buf_channel_pair();

    // Start upload from reader
    let upload_id = "test-upload-id";
    let mut reader = rx;

    let mock_ops_clone = mock_ops.clone();
    let object_path_clone = object_path.clone();
    let upload_task = nativelink_util::spawn!("upload_test", async move {
        mock_ops_clone
            .upload_from_reader(&object_path_clone, &mut reader, upload_id, data_size as i64)
            .await
    });

    // Send the data
    for i in 0..data_size {
        tx.send(send_data.slice(i..=i)).await?;
    }
    tx.send_eof()?;
    upload_task.await.unwrap()?;

    // Verify the content
    let result = mock_ops.read_object_content(object_path, 0, None).await?;
    assert_eq!(result.len(), data_size);
    assert_eq!(result, send_data);

    // Verify call counts
    let call_counts = mock_ops.get_call_counts().await;
    assert_eq!(call_counts.upload_from_reader_calls, 1);
    assert_eq!(call_counts.read_calls, 1);

    Ok(())
}

#[nativelink_test]
async fn test_object_exists() -> Result<(), Error> {
    // Create a mock implementation
    let mock_ops = Arc::new(MockGcsOperations::new());
    let existing_path = ObjectPath::new(BUCKET_NAME.to_string(), OBJECT_PATH);
    let nonexistent_path = ObjectPath::new(BUCKET_NAME.to_string(), "nonexistent-object");
    mock_ops
        .add_object(&existing_path, b"test content".to_vec())
        .await;

    // Test existing object
    let result = mock_ops.object_exists(&existing_path).await?;
    assert!(result, "Expected object to exist");

    // Test nonexistent object
    let result = mock_ops.object_exists(&nonexistent_path).await?;
    assert!(!result, "Expected object not to exist");

    // Verify call counts
    let call_counts = mock_ops.get_call_counts().await;
    assert_eq!(call_counts.object_exists_calls, 2);

    Ok(())
}

#[nativelink_test]
async fn test_simulated_failures() -> Result<(), Error> {
    // Create a mock implementation
    let mock_ops = Arc::new(MockGcsOperations::new());
    let object_path = ObjectPath::new(BUCKET_NAME.to_string(), OBJECT_PATH);

    // Set the mock to fail
    mock_ops.set_should_fail(true).await;

    // Test different operations and expect them to fail
    let metadata_result = mock_ops.read_object_metadata(object_path.clone()).await;
    assert!(
        metadata_result.is_err(),
        "Expected read_object_metadata to fail"
    );

    let content_result = mock_ops
        .read_object_content(object_path.clone(), 0, None)
        .await;
    assert!(
        content_result.is_err(),
        "Expected read_object_content to fail"
    );

    let write_result = mock_ops.write_object(&object_path, vec![1, 2, 3]).await;
    assert!(write_result.is_err(), "Expected write_object to fail");

    // Reset the mock to not fail
    mock_ops.set_should_fail(false).await;

    // Operations should succeed now
    let result = mock_ops.write_object(&object_path, vec![1, 2, 3]).await;
    assert!(
        result.is_ok(),
        "Expected write_object to succeed after resetting failure mode"
    );

    Ok(())
}

#[nativelink_test]
async fn test_empty_data_handling() -> Result<(), Error> {
    // Create a mock implementation
    let mock_ops = Arc::new(MockGcsOperations::new());
    let object_path = ObjectPath::new(BUCKET_NAME.to_string(), OBJECT_PATH);
    let expected_content = b"helloworld".to_vec();
    let (mut tx, mut rx) = make_buf_channel_pair();

    let mock_ops_for_verification = mock_ops.clone();
    let object_path_for_verification = object_path.clone();

    // Send content in parts without empty chunks
    let send_data_task = async move {
        tx.send(Bytes::from_static(b"hello")).await?;
        tx.send(Bytes::from_static(b"world")).await?;
        tx.send_eof()?;
        Result::<(), Error>::Ok(())
    };

    // Start upload from reader
    let mock_ops_for_upload = mock_ops.clone();
    let upload_task = async move {
        mock_ops_for_upload
            .upload_from_reader(&object_path, &mut rx, "test-upload-id", 1024)
            .await
    };

    // Run both tasks concurrently
    let (send_result, upload_result) = join!(send_data_task, upload_task);
    send_result.err_tip(|| "Failed to send data")?;
    upload_result.err_tip(|| "Failed to upload data")?;

    // Verify the content was correctly stored
    let stored_content = mock_ops_for_verification
        .read_object_content(object_path_for_verification, 0, None)
        .await?;
    assert_eq!(
        stored_content, expected_content,
        "Content wasn't stored correctly"
    );

    Ok(())
}
