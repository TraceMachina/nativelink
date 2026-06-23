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

use core::sync::atomic::{AtomicUsize, Ordering};
use core::time::Duration;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use azure_core::error::ErrorKind;
use azure_core::http::headers::Headers;
use azure_core::http::{
    AsyncRawResponse, HttpClient, Method, Request, RetryOptions, StatusCode, Transport, Url,
};
use azure_storage_blob::clients::{BlobContainerClient, BlobContainerClientOptions};
use bytes::{BufMut, Bytes, BytesMut};
use nativelink_config::stores::{CommonObjectSpec, ExperimentalAzureSpec, Retry};
use nativelink_error::{Code, Error, ResultExt};
use nativelink_macro::nativelink_test;
use nativelink_store::azure_blob_store::AzureBlobStore;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::store_trait::{StoreKey, StoreLike, UploadSizeInfo};
use sha2::{Digest, Sha256};

const TEST_CONTAINER: &str = "test-container";
const TEST_ACCOUNT: &str = "testaccount";
const TEST_HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";
const TEST_SIZE: u64 = 100;
const EPOCH_LAST_MODIFIED: &str = "Thu, 01 Jan 1970 00:00:00 GMT";

type TestStore = Arc<AzureBlobStore<fn() -> MockInstantWrapped>>;

/// A single canned HTTP response the mock transport hands back to the SDK.
#[derive(Clone, Debug)]
struct CannedResponse {
    status: StatusCode,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

fn properties_ok(content_length: u64) -> CannedResponse {
    CannedResponse {
        status: StatusCode::Ok,
        headers: vec![
            ("content-length", content_length.to_string()),
            ("last-modified", EPOCH_LAST_MODIFIED.to_string()),
        ],
        body: Vec::new(),
    }
}

const fn error_response(status: StatusCode) -> CannedResponse {
    CannedResponse {
        status,
        headers: Vec::new(),
        body: Vec::new(),
    }
}

const fn not_found() -> CannedResponse {
    error_response(StatusCode::NotFound)
}

/// Successful write result for the SDK upload/stage/commit operations.
const fn created() -> CannedResponse {
    CannedResponse {
        status: StatusCode::Created,
        headers: Vec::new(),
        body: Vec::new(),
    }
}

fn download_ok(status: StatusCode, body: Vec<u8>) -> CannedResponse {
    CannedResponse {
        status,
        headers: vec![("content-length", body.len().to_string())],
        body,
    }
}

/// Fake [`HttpClient`] injected into the SDK pipeline. It routes on the request's
/// method and the `comp` query parameter to mirror how the store calls the SDK:
///   * HEAD                 -> `get_properties`
///   * GET                  -> `download` (range is carried in a header)
///   * PUT `?comp=block`    -> stage a block
///   * PUT `?comp=blocklist`-> commit the block list
///   * PUT (no `comp`)      -> single-shot block-blob upload
///
/// Each operation has its own queue of responses consumed in order, which lets the
/// retry tests assert the exact request sequence.
#[derive(Debug, Default)]
struct MockTransport {
    properties: Mutex<VecDeque<CannedResponse>>,
    download: Mutex<VecDeque<CannedResponse>>,
    upload: Mutex<VecDeque<CannedResponse>>,
    stage_block: Mutex<VecDeque<CannedResponse>>,
    commit: Mutex<VecDeque<CannedResponse>>,
    /// Raw XML body of the last `commit_block_list` request (the committed block
    /// list), captured so tests can assert the exact set/order of block ids.
    committed_block_list: Mutex<Option<Bytes>>,
    count: AtomicUsize,
}

impl MockTransport {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn push_properties(&self, response: CannedResponse) {
        self.properties.lock().unwrap().push_back(response);
    }

    fn push_download(&self, response: CannedResponse) {
        self.download.lock().unwrap().push_back(response);
    }

    fn push_upload(&self, response: CannedResponse) {
        self.upload.lock().unwrap().push_back(response);
    }

    fn request_count(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }

    fn committed_block_list(&self) -> Option<Bytes> {
        self.committed_block_list.lock().unwrap().clone()
    }

    fn pop(queue: &Mutex<VecDeque<CannedResponse>>) -> Option<CannedResponse> {
        queue.lock().unwrap().pop_front()
    }
}

fn build_response(canned: CannedResponse) -> AsyncRawResponse {
    let mut headers = Headers::new();
    for (key, value) in canned.headers {
        headers.insert(key, value);
    }
    AsyncRawResponse::from_bytes(canned.status, headers, canned.body)
}

#[async_trait::async_trait]
impl HttpClient for MockTransport {
    async fn execute_request(&self, request: &Request) -> azure_core::Result<AsyncRawResponse> {
        self.count.fetch_add(1, Ordering::SeqCst);

        let query = request.url().query().unwrap_or_default().to_owned();
        let canned = match request.method() {
            Method::Head => Self::pop(&self.properties).unwrap_or_else(|| properties_ok(0)),
            Method::Get => {
                Self::pop(&self.download).unwrap_or_else(|| download_ok(StatusCode::Ok, Vec::new()))
            }
            Method::Put if query.contains("comp=blocklist") => {
                *self.committed_block_list.lock().unwrap() = Some(Bytes::from(request.body()));
                Self::pop(&self.commit).unwrap_or_else(created)
            }
            Method::Put if query.contains("comp=block") => {
                Self::pop(&self.stage_block).unwrap_or_else(created)
            }
            Method::Put => Self::pop(&self.upload).unwrap_or_else(created),
            other => {
                return Err(azure_core::Error::with_message(
                    ErrorKind::Other,
                    format!("Unexpected request: {other} {}", request.url()),
                ));
            }
        };

        Ok(build_response(canned))
    }
}

fn create_test_store(
    mock: Arc<MockTransport>,
    retry: Option<Retry>,
    consider_expired_after_s: Option<u32>,
) -> Result<TestStore, Error> {
    let mut options = BlobContainerClientOptions::default();
    // NativeLink's own `Retrier` wraps each operation, so disable the SDK's retry
    // policy to keep request counts deterministic.
    options.client_options.retry = RetryOptions::none();
    options.client_options.transport = Some(Transport::new(mock));

    let container_url = Url::parse(&format!(
        "https://{TEST_ACCOUNT}.blob.core.windows.net/{TEST_CONTAINER}"
    ))
    .expect("valid container URL");
    let client = BlobContainerClient::new(container_url, None, Some(options))
        .expect("failed to build BlobContainerClient");

    let spec = ExperimentalAzureSpec {
        account_name: TEST_ACCOUNT.to_string(),
        container: TEST_CONTAINER.to_string(),
        common: CommonObjectSpec {
            consider_expired_after_s: consider_expired_after_s.unwrap_or(0),
            retry: retry.unwrap_or_default(),
            ..Default::default()
        },
        ..Default::default()
    };

    AzureBlobStore::new_with_client_and_jitter(
        &spec,
        client,
        Arc::new(|_| Duration::from_secs(0)),
        MockInstantWrapped::default,
    )
}

fn create_test_digest() -> Result<DigestInfo, Error> {
    DigestInfo::try_new(TEST_HASH, TEST_SIZE)
}

#[nativelink_test]
async fn test_has_object_found() -> Result<(), Error> {
    let mock = MockTransport::new();
    mock.push_properties(properties_ok(512));
    let store = create_test_store(Arc::clone(&mock), None, None)?;

    let result = store.has(create_test_digest()?).await?;

    assert_eq!(result, Some(512));
    assert_eq!(mock.request_count(), 1);
    Ok(())
}

#[nativelink_test]
async fn test_has_object_not_found() -> Result<(), Error> {
    let mock = MockTransport::new();
    mock.push_properties(not_found());
    let store = create_test_store(Arc::clone(&mock), None, None)?;

    let result = store.has(create_test_digest()?).await?;

    assert_eq!(result, None);
    assert_eq!(mock.request_count(), 1);
    Ok(())
}

#[nativelink_test]
async fn test_has_with_retries() -> Result<(), Error> {
    let mock = MockTransport::new();
    mock.push_properties(error_response(StatusCode::InternalServerError));
    mock.push_properties(properties_ok(111));

    let retry = Retry {
        max_retries: 3,
        delay: 0.0,
        jitter: 0.0,
        ..Default::default()
    };

    let store = create_test_store(Arc::clone(&mock), Some(retry), None)?;
    let result = store.has(create_test_digest()?).await?;

    assert_eq!(result, Some(111));
    assert_eq!(mock.request_count(), 2);
    Ok(())
}

#[nativelink_test]
async fn test_has_with_results_zero_digest() -> Result<(), Error> {
    let digest = DigestInfo::new(Sha256::new().finalize().into(), 0);
    let keys = vec![StoreKey::from(&digest)];
    let mut results = vec![None];

    let mock = MockTransport::new();
    let store = create_test_store(Arc::clone(&mock), None, None)?;

    store.has_with_results(&keys, &mut results).await?;

    assert_eq!(results, vec![Some(0)]);
    assert_eq!(
        mock.request_count(),
        0,
        "Expected no requests for zero digest"
    );
    Ok(())
}

#[nativelink_test]
async fn test_has_with_expired_result() -> Result<(), Error> {
    use mock_instant::thread_local::MockClock;

    const CONTENT_SIZE: usize = 10;

    let mock = MockTransport::new();
    mock.push_properties(properties_ok(512));
    mock.push_properties(properties_ok(512));

    let store = create_test_store(
        Arc::clone(&mock),
        Some(Retry {
            max_retries: 1,
            delay: 0.0,
            jitter: 0.0,
            ..Default::default()
        }),
        Some(2 * 24 * 60 * 60),
    )?;

    // Time starts at 1970-01-01 00:00:00 and the blob is last-modified at the epoch.
    let digest = DigestInfo::try_new(TEST_HASH, CONTENT_SIZE)?;

    // 1 day in: not expired.
    {
        MockClock::advance(Duration::from_secs(24 * 60 * 60));
        let mut results = vec![None];
        store
            .has_with_results(&[digest.into()], &mut results)
            .await?;
        assert_eq!(
            results,
            vec![Some(512)],
            "Should find non-expired content after 1 day"
        );
    }

    // 4 days in: expired (older than the 2-day threshold).
    {
        MockClock::advance(Duration::from_secs(3 * 24 * 60 * 60));
        let mut results = vec![None];
        store
            .has_with_results(&[digest.into()], &mut results)
            .await?;
        assert_eq!(
            results,
            vec![None],
            "Should not find expired content after 4 days"
        );
    }

    assert_eq!(mock.request_count(), 2);
    Ok(())
}

#[nativelink_test]
async fn test_get() -> Result<(), Error> {
    const VALUE: &str = "test_content";

    let mock = MockTransport::new();
    mock.push_download(download_ok(StatusCode::Ok, VALUE.as_bytes().to_vec()));

    let store = create_test_store(Arc::clone(&mock), None, None)?;
    let result = store
        .get_part_unchunked(create_test_digest()?, 0, None)
        .await?;

    assert_eq!(result, VALUE.as_bytes());
    assert_eq!(mock.request_count(), 1);
    Ok(())
}

#[nativelink_test]
async fn test_get_byte_range() -> Result<(), Error> {
    const VALUE: &str = "0123456789abcdef";
    const OFFSET: u64 = 4;
    const LENGTH: u64 = 6;
    let start = usize::try_from(OFFSET).unwrap();
    let end = usize::try_from(OFFSET + LENGTH).unwrap();
    let expected = &VALUE.as_bytes()[start..end];

    let mock = MockTransport::new();
    // A 206 Partial Content with just the requested slice as the body.
    mock.push_download(download_ok(StatusCode::PartialContent, expected.to_vec()));

    let store = create_test_store(Arc::clone(&mock), None, None)?;
    let result = store
        .get_part_unchunked(create_test_digest()?, OFFSET, Some(LENGTH))
        .await?;

    assert_eq!(result, expected);
    assert_eq!(mock.request_count(), 1);
    Ok(())
}

#[nativelink_test]
async fn test_get_not_found() -> Result<(), Error> {
    let mock = MockTransport::new();
    mock.push_download(not_found());

    let store = create_test_store(Arc::clone(&mock), None, None)?;
    let err = store
        .get_part_unchunked(create_test_digest()?, 0, None)
        .await
        .expect_err("expected a NotFound error");

    assert_eq!(err.code, Code::NotFound);
    assert_eq!(mock.request_count(), 1);
    Ok(())
}

#[nativelink_test]
async fn test_get_with_retries() -> Result<(), Error> {
    const VALUE: &str = "test_content";

    let mock = MockTransport::new();
    mock.push_download(error_response(StatusCode::InternalServerError));
    mock.push_download(error_response(StatusCode::ServiceUnavailable));
    mock.push_download(error_response(StatusCode::Conflict));
    mock.push_download(download_ok(StatusCode::Ok, VALUE.as_bytes().to_vec()));

    let retry = Retry {
        max_retries: 1024,
        delay: 0.0,
        jitter: 0.0,
        ..Default::default()
    };

    let store = create_test_store(Arc::clone(&mock), Some(retry), None)?;
    let result = store
        .get_part_unchunked(create_test_digest()?, 0, None)
        .await?;

    assert_eq!(result, VALUE.as_bytes());
    assert_eq!(mock.request_count(), 4);
    Ok(())
}

#[nativelink_test]
async fn test_get_part_zero_digest() -> Result<(), Error> {
    let digest = DigestInfo::new(Sha256::new().finalize().into(), 0);
    let mock = MockTransport::new();
    let store = create_test_store(Arc::clone(&mock), None, None)?;
    let (mut writer, mut reader) = make_buf_channel_pair();

    let (get_result, file_data) = tokio::join!(
        store.get_part(digest, &mut writer, 0, None),
        reader.consume(Some(1024))
    );

    get_result?;
    let file_data = file_data.err_tip(|| "Error reading bytes")?;

    assert_eq!(file_data.len(), 0, "Expected empty file content");
    assert_eq!(
        mock.request_count(),
        0,
        "Expected no requests for zero digest"
    );
    Ok(())
}

#[nativelink_test]
async fn test_update_small_file() -> Result<(), Error> {
    const CONTENT_LENGTH: usize = 1024; // Below DEFAULT_BLOCK_SIZE: single-shot upload.
    let mut send_data = BytesMut::new();
    for i in 0..CONTENT_LENGTH {
        send_data.put_u8(u8::try_from((i % 93) + 33).unwrap());
    }
    let send_data = send_data.freeze();

    let mock = MockTransport::new();
    mock.push_upload(created());

    let store = create_test_store(Arc::clone(&mock), None, None)?;
    let (mut tx, rx) = make_buf_channel_pair();

    let update_fut = store.update(
        create_test_digest()?,
        rx,
        UploadSizeInfo::ExactSize(CONTENT_LENGTH as u64),
    );

    let send_data_copy = send_data.clone();
    let send_fut = Box::pin(async move {
        const CHUNK_SIZE: usize = 256;
        for chunk in send_data_copy.chunks(CHUNK_SIZE) {
            tx.send(Bytes::copy_from_slice(chunk)).await?;
        }
        tx.send_eof()
    });

    let (update_result, send_result) = tokio::join!(update_fut, send_fut);

    update_result?;
    send_result?;
    assert_eq!(mock.request_count(), 1);
    Ok(())
}

#[nativelink_test]
async fn test_update_zero_size() -> Result<(), Error> {
    let mock = MockTransport::new();
    let store = create_test_store(Arc::clone(&mock), None, None)?;
    let (mut tx, rx) = make_buf_channel_pair();

    let update_fut = store.update(create_test_digest()?, rx, UploadSizeInfo::ExactSize(0));
    let send_fut = async move { tx.send_eof() };

    let (update_result, send_result) = tokio::join!(update_fut, send_fut);

    update_result?;
    send_result?;
    assert_eq!(
        mock.request_count(),
        0,
        "Zero-size upload should not make any requests"
    );
    Ok(())
}

#[nativelink_test]
async fn test_update_with_retries() -> Result<(), Error> {
    const CONTENT_LENGTH: usize = 512;

    let mut send_data = BytesMut::new();
    for i in 0..CONTENT_LENGTH {
        send_data.put_u8(u8::try_from((i % 93) + 33).unwrap());
    }
    let send_data = send_data.freeze();

    let mock = MockTransport::new();
    mock.push_upload(error_response(StatusCode::InternalServerError));
    mock.push_upload(created());

    let retry = Retry {
        max_retries: 3,
        delay: 0.0,
        jitter: 0.0,
        ..Default::default()
    };

    let store = create_test_store(Arc::clone(&mock), Some(retry), None)?;
    let (mut tx, rx) = make_buf_channel_pair();

    let update_fut = store.update(
        create_test_digest()?,
        rx,
        UploadSizeInfo::ExactSize(CONTENT_LENGTH as u64),
    );

    let send_data_copy = send_data.clone();
    let send_fut = async move {
        tx.send(send_data_copy).await?;
        tx.send_eof()
    };

    let (update_result, send_result) = tokio::join!(update_fut, send_fut);

    update_result?;
    send_result?;
    assert_eq!(mock.request_count(), 2);
    Ok(())
}

#[nativelink_test]
async fn test_multipart_upload_large_file() -> Result<(), Error> {
    const MIN_BLOCK_SIZE: usize = 5 * 1024 * 1024; // 5 MiB DEFAULT_BLOCK_SIZE.
    const TOTAL_SIZE: usize = MIN_BLOCK_SIZE * 2 + 50;

    let mut send_data = Vec::with_capacity(TOTAL_SIZE);
    for i in 0..TOTAL_SIZE {
        send_data.push(u8::try_from((i * 3) % 256).unwrap());
    }

    // No queued responses: the mock returns 201 for every staged block and the
    // final commit. The 10 MiB payload splits into three 5-MiB-or-less blocks.
    let mock = MockTransport::new();
    let store = create_test_store(Arc::clone(&mock), None, None)?;

    let digest = DigestInfo::try_new(TEST_HASH, TOTAL_SIZE)?;
    let store_key: StoreKey = StoreKey::from(&digest);
    store.update_oneshot(store_key, send_data.into()).await?;

    // 3 staged blocks + 1 committed block list.
    assert_eq!(mock.request_count(), 4);

    // The store commits a block list of fixed-width, zero-padded block ids. Assert
    // the committed XML carries exactly those ids, base64-encoded, in order.
    let committed = mock
        .committed_block_list()
        .expect("commit_block_list must have been called");
    let committed = core::str::from_utf8(&committed).expect("committed block list must be utf-8");

    let mut last_pos = 0;
    for block_id in 0..3 {
        let encoded = azure_core::base64::encode(format!("{block_id:032}").into_bytes());
        let pos = committed.find(&encoded).unwrap_or_else(|| {
            panic!("block id {block_id} ({encoded}) missing from committed list: {committed}")
        });
        assert!(
            pos >= last_pos,
            "block ids must be committed in ascending order"
        );
        last_pos = pos;
    }
    Ok(())
}
