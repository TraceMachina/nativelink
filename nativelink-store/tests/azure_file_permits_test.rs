// Copyright 2026 Trace Machina, Inc. All rights reserved.
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

// This test owns the process-wide file permit pool. Keep it in its own test
// binary so unrelated filesystem tests cannot acquire or release those permits.

use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;
use std::sync::Arc;

use azure_core::http::headers::Headers;
use azure_core::http::{
    AsyncRawResponse, HttpClient, Method, Request, RetryOptions, StatusCode, Transport, Url,
};
use azure_storage_blob::clients::{BlobContainerClient, BlobContainerClientOptions};
use bytes::Bytes;
use nativelink_config::stores::{
    CommonObjectSpec, ExperimentalAzureSpec, ExperimentalCloudObjectSpec, FastSlowSpec,
    FilesystemSpec, StoreDirection, StoreSpec,
};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_store::azure_blob_store::AzureBlobStore;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::filesystem_store::{FileEntryImpl, FilesystemStore};
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::{DigestInfo, fs, make_temp_path};
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::store_trait::{Store, StoreLike, UploadSizeInfo};

#[derive(Debug, Default)]
struct SuccessfulTransport(AtomicUsize);

#[async_trait::async_trait]
impl HttpClient for SuccessfulTransport {
    async fn execute_request(&self, request: &Request) -> azure_core::Result<AsyncRawResponse> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let mut headers = Headers::new();
        headers.insert("content-length", "3");
        let (status, body) = match request.method() {
            Method::Head => (StatusCode::Ok, Bytes::new()),
            Method::Get => (StatusCode::Ok, Bytes::from_static(b"abc")),
            Method::Put => (StatusCode::Created, Bytes::new()),
            method => panic!("unexpected request {method:?}"),
        };
        Ok(AsyncRawResponse::from_bytes(status, headers, body))
    }
}

#[nativelink_test]
async fn azure_requests_progress_when_filesystem_permits_are_exhausted() -> Result<(), Error> {
    let transport = Arc::new(SuccessfulTransport::default());
    let mut options = BlobContainerClientOptions::default();
    options.client_options.retry = RetryOptions::none();
    options.client_options.transport = Some(Transport::new(transport.clone()));
    let client = BlobContainerClient::new(
        Url::parse("https://testaccount.blob.core.windows.net/test-container").unwrap(),
        None,
        Some(options),
    )
    .unwrap();
    let spec = ExperimentalAzureSpec {
        common: CommonObjectSpec {
            multipart_max_concurrent_uploads: Some(1),
            max_concurrent_requests: 2,
            max_concurrent_reads: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let azure = AzureBlobStore::new_with_client_and_jitter(
        &spec,
        client,
        Arc::new(|_| Duration::ZERO),
        MockInstantWrapped::default,
    )?;

    let available = fs::OPEN_FILE_SEMAPHORE.available_permits();
    let reserved = fs::OPEN_FILE_SEMAPHORE
        .acquire_many(u32::try_from(available).unwrap())
        .await
        .unwrap();
    let small_digest = DigestInfo::try_new(&"1".repeat(64), 3)?;
    tokio::time::timeout(Duration::from_secs(5), async {
        assert_eq!(azure.has(small_digest).await?, Some(3));
        azure
            .update_oneshot(small_digest, Bytes::from_static(b"abc"))
            .await?;
        assert_eq!(
            azure.get_part_unchunked(small_digest, 0, None).await?,
            Bytes::from_static(b"abc")
        );
        // Exercise both staging and commit with no filesystem permits free.
        let large = Bytes::from(vec![7u8; 10 * 1024 * 1024]);
        azure
            .update_oneshot(DigestInfo::try_new(&"2".repeat(64), large.len())?, large)
            .await?;
        Ok::<_, Error>(())
    })
    .await
    .expect("Azure HTTP requests must not wait for filesystem permits")?;
    assert_eq!(transport.0.load(Ordering::SeqCst), 6);
    drop(reserved);

    let filesystem_spec = FilesystemSpec {
        content_path: make_temp_path("content_path"),
        temp_path: make_temp_path("temp_path"),
        ..Default::default()
    };
    let fast = Store::new(FilesystemStore::<FileEntryImpl>::new(&filesystem_spec).await?);
    let store = Store::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Filesystem(filesystem_spec),
            slow: StoreSpec::ExperimentalCloudObjectStore(ExperimentalCloudObjectSpec::Azure(spec)),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
            bypass_dedup_threshold_bytes: 0,
            has_slow_write_wait_timeout_s: 0,
            leader_inactivity_timeout_s: 0,
        },
        fast.clone(),
        Store::new(azure),
    ));
    let available = fs::OPEN_FILE_SEMAPHORE.available_permits();
    let reserved = fs::OPEN_FILE_SEMAPHORE
        .acquire_many(u32::try_from(available - 1).unwrap())
        .await
        .unwrap();
    let (mut tx, rx) = make_buf_channel_pair();
    let block = Bytes::from(vec![7u8; 1024 * 1024]);
    let digest = DigestInfo::try_new(&"3".repeat(64), 40 * block.len())?;
    let send = async {
        tx.send(block.clone()).await?;
        // The file opens on the first MiB, before Azure has a whole 5 MiB block.
        while fs::OPEN_FILE_SEMAPHORE.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
        for _ in 1..40 {
            tx.send(block.clone()).await?;
        }
        tx.send_eof()
    };
    let upload = store.update(digest, rx, UploadSizeInfo::ExactSize(digest.size_bytes()));
    tokio::time::timeout(Duration::from_secs(5), async {
        tokio::try_join!(upload, send)
    })
    .await
    .expect("the fast file and Azure blocks must not wait on each other's permits")?;
    drop(reserved);
    assert_eq!(transport.0.load(Ordering::SeqCst), 15);
    assert_eq!(
        fast.get_part_unchunked(digest, 0, None).await?,
        Bytes::from(vec![7u8; 40 * block.len()])
    );
    Ok(())
}
