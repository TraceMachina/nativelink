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

use bytes::Bytes;
use nativelink_config::stores::FilesystemSpec;
use nativelink_error::Error;
use nativelink_store::filesystem_store::{FileEntryImpl, FilesystemStore};
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::StoreLike;
use tracing::info;

/// Test configuration that creates multiple workers with isolated CAS stores
/// This should reproduce the "Object not found" error
#[tokio::test]
async fn test_multi_worker_isolated_cas_fails() -> Result<(), Error> {
    // Create temporary directories for each worker's CAS
    let temp_dir = std::env::temp_dir().join(format!("test-worker-cas-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let worker1_cas_path = temp_dir.join("worker1_cas");
    let worker2_cas_path = temp_dir.join("worker2_cas");

    // Create isolated CAS stores for each worker
    let worker1_store = Arc::new(
        FilesystemStore::<FileEntryImpl>::new(&FilesystemSpec {
            content_path: worker1_cas_path.to_string_lossy().to_string(),
            temp_path: temp_dir.join("worker1_temp").to_string_lossy().to_string(),
            eviction_policy: None,
            ..Default::default()
        })
        .await?,
    );

    let worker2_store = Arc::new(
        FilesystemStore::<FileEntryImpl>::new(&FilesystemSpec {
            content_path: worker2_cas_path.to_string_lossy().to_string(),
            temp_path: temp_dir.join("worker2_temp").to_string_lossy().to_string(),
            eviction_policy: None,
            ..Default::default()
        })
        .await?,
    );

    // Create test data that will be uploaded by worker1
    let test_data = Bytes::from("This is test data that should be shared between workers");

    // Create a valid digest using SHA256 hash
    let digest = DigestInfo::try_new(
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", // Empty file SHA256 for testing
        test_data.len(),
    )?;

    // Worker 1 uploads the data to its CAS
    worker1_store
        .update_oneshot(digest, test_data.clone())
        .await?;

    // Worker 2 tries to access the same data - this should fail
    let result = worker2_store.get_part_unchunked(digest, 0, None).await;

    // Verify that worker2 cannot find the data
    assert!(
        result.is_err(),
        "Worker 2 should not find data in isolated CAS"
    );
    if let Err(e) = result {
        assert!(
            e.to_string().contains("not found") || e.to_string().contains("No such"),
            "Expected 'not found' error, got: {}",
            e
        );
    }

    info!("Successfully reproduced isolated CAS error");

    // Clean up temp dir
    drop(std::fs::remove_dir_all(&temp_dir));

    Ok(())
}

/// Test configuration with shared CAS store
/// This should work correctly with multiple workers
#[tokio::test]
async fn test_multi_worker_shared_cas_works() -> Result<(), Error> {
    // Create a single shared CAS store
    let temp_dir = std::env::temp_dir().join(format!("test-shared-cas-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let shared_cas_path = temp_dir.join("shared_cas");

    let shared_store = Arc::new(
        FilesystemStore::<FileEntryImpl>::new(&FilesystemSpec {
            content_path: shared_cas_path.to_string_lossy().to_string(),
            temp_path: temp_dir.join("shared_temp").to_string_lossy().to_string(),
            eviction_policy: None,
            ..Default::default()
        })
        .await?,
    );

    // Both workers use the same store
    let worker1_store = shared_store.clone();
    let worker2_store = shared_store.clone();

    // Create test data
    let test_data = Bytes::from("Shared data between workers");

    // Create a valid digest
    let digest = DigestInfo::try_new(
        "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3", // SHA256 of "123"
        test_data.len(),
    )?;

    // Worker 1 uploads the data
    worker1_store
        .update_oneshot(digest, test_data.clone())
        .await?;

    // Worker 2 can access the same data
    let retrieved_data = worker2_store.get_part_unchunked(digest, 0, None).await?;

    assert_eq!(
        retrieved_data, test_data,
        "Worker 2 should retrieve the same data"
    );

    info!("Successfully verified shared CAS works");

    // Clean up temp dir
    drop(std::fs::remove_dir_all(&temp_dir));

    Ok(())
}

/// Stress test with many workers and concurrent actions
#[tokio::test]
#[ignore] // This is a longer-running test
async fn stress_test_multi_worker_cas() -> Result<(), Error> {
    let temp_dir = std::env::temp_dir().join(format!("test-stress-cas-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).unwrap();
    let shared_cas = Arc::new(
        FilesystemStore::<FileEntryImpl>::new(&FilesystemSpec {
            content_path: temp_dir.join("cas").to_string_lossy().to_string(),
            temp_path: temp_dir.join("temp").to_string_lossy().to_string(),
            eviction_policy: None,
            ..Default::default()
        })
        .await?,
    );

    const NUM_WORKERS: usize = 10;
    const NUM_FILES: usize = 100;
    const FILE_SIZE: usize = 1024 * 1024; // 1MB files

    // Upload many files concurrently
    let mut upload_handles = vec![];
    for i in 0..NUM_FILES {
        let cas = shared_cas.clone();
        let handle = tokio::spawn(async move {
            let data = vec![i as u8; FILE_SIZE];
            // Use a simple digest for testing (not a real hash)
            let digest = DigestInfo::try_new(
                &format!("{:064}", i), // Create a 64-char hex string
                FILE_SIZE,
            )?;
            cas.update_oneshot(digest, data.into()).await?;
            Ok::<_, Error>(digest)
        });
        upload_handles.push(handle);
    }

    // Wait for all uploads
    let mut digests = vec![];
    for handle in upload_handles {
        digests.push(handle.await.unwrap()?);
    }

    // Simulate multiple workers reading files concurrently
    let mut read_handles = vec![];
    for worker_id in 0..NUM_WORKERS {
        let cas = shared_cas.clone();
        let digests_clone = digests.clone();
        let handle = tokio::spawn(async move {
            for digest in digests_clone {
                let data = cas.get_part_unchunked(digest, 0, None).await?;
                assert_eq!(
                    data.len(),
                    FILE_SIZE,
                    "Worker {} got wrong data size",
                    worker_id
                );
            }
            Ok::<_, Error>(())
        });
        read_handles.push(handle);
    }

    // Wait for all reads
    for handle in read_handles {
        handle.await.unwrap()?;
    }

    info!(
        "Stress test completed successfully with {} workers and {} files",
        NUM_WORKERS, NUM_FILES
    );

    // Clean up temp dir
    drop(std::fs::remove_dir_all(&temp_dir));

    Ok(())
}
