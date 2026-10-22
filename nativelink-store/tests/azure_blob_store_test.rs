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
    block_list: Mutex<VecDeque<CannedResponse>>,
    /// Every blob path a `Delete Blob` request named, in order.
    deleted: Mutex<Vec<String>>,
    /// Raw XML body of the last `commit_block_list` request (the committed block
    /// list), captured so tests can assert the exact set/order of block ids.
    committed_block_list: Mutex<Option<Bytes>>,
    /// The `Range` header of every download, in order, so a test can assert
    /// that a retry resumed rather than started again.
    download_ranges: Mutex<Vec<Option<String>>>,
    count: AtomicUsize,
    uploaded_bodies: Mutex<Vec<Bytes>>,
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

    fn download_ranges(&self) -> Vec<Option<String>> {
        self.download_ranges.lock().unwrap().clone()
    }

    fn committed_block_list(&self) -> Option<Bytes> {
        self.committed_block_list.lock().unwrap().clone()
    }

    fn push_stage_block(&self, response: CannedResponse) {
        self.stage_block.lock().unwrap().push_back(response);
    }

    fn push_block_list(&self, response: CannedResponse) {
        self.block_list.lock().unwrap().push_back(response);
    }

    fn deleted(&self) -> Vec<String> {
        self.deleted.lock().unwrap().clone()
    }

    fn pop(queue: &Mutex<VecDeque<CannedResponse>>) -> Option<CannedResponse> {
        queue.lock().unwrap().pop_front()
    }
}

/// The XML Azure returns for Get Block List: block ids are base64 in the
/// body, as the SDK's own serializer writes them.
fn block_list_ok(committed: &[&str], uncommitted: &[&str]) -> CannedResponse {
    let blocks = |ids: &[&str]| -> String {
        let mut out = String::new();
        for id in ids {
            out.push_str("<Block><Name>");
            out.push_str(&azure_core::base64::encode(id.as_bytes()));
            out.push_str("</Name><Size>5242880</Size></Block>");
        }
        out
    };
    CannedResponse {
        status: StatusCode::Ok,
        headers: vec![("content-type", "application/xml".to_string())],
        body: format!(
            "<?xml version=\"1.0\" encoding=\"utf-8\"?><BlockList><CommittedBlocks>{}</CommittedBlocks><UncommittedBlocks>{}</UncommittedBlocks></BlockList>",
            blocks(committed),
            blocks(uncommitted)
        )
        .into_bytes(),
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
            Method::Get if query.contains("comp=blocklist") => {
                Self::pop(&self.block_list).unwrap_or_else(|| block_list_ok(&[], &[]))
            }
            Method::Delete => {
                self.deleted
                    .lock()
                    .unwrap()
                    .push(request.url().path().to_owned());
                CannedResponse {
                    status: StatusCode::Accepted,
                    headers: Vec::new(),
                    body: Vec::new(),
                }
            }
            Method::Get => {
                self.download_ranges.lock().unwrap().push(
                    request
                        .headers()
                        .iter()
                        .find(|(name, _)| name.as_str().eq_ignore_ascii_case("range"))
                        .map(|(_, value)| value.as_str().to_owned()),
                );
                Self::pop(&self.download).unwrap_or_else(|| download_ok(StatusCode::Ok, Vec::new()))
            }
            Method::Put if query.contains("comp=blocklist") => {
                *self.committed_block_list.lock().unwrap() = Some(Bytes::from(request.body()));
                Self::pop(&self.commit).unwrap_or_else(created)
            }
            Method::Put if query.contains("comp=block") => {
                Self::pop(&self.stage_block).unwrap_or_else(created)
            }
            Method::Put => {
                self.uploaded_bodies
                    .lock()
                    .unwrap()
                    .push(Bytes::from(request.body()));
                Self::pop(&self.upload).unwrap_or_else(created)
            }
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
    create_test_store_with_common(
        mock,
        CommonObjectSpec {
            consider_expired_after_s: consider_expired_after_s.unwrap_or(0),
            retry: retry.unwrap_or_default(),
            ..Default::default()
        },
    )
}

fn create_test_store_with_common(
    mock: Arc<MockTransport>,
    common: CommonObjectSpec,
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
        common,
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
async fn rejects_incomplete_small_azure_uploads() -> Result<(), Error> {
    for payload in [Bytes::new(), Bytes::from_static(b"short")] {
        let mock = MockTransport::new();
        let store = create_test_store(Arc::clone(&mock), Some(Retry::default()), None)?;
        let digest = DigestInfo::try_new(TEST_HASH, 78)?;
        let (mut tx, rx) = make_buf_channel_pair();
        let expected = payload.clone();
        let send = async move {
            if !payload.is_empty() {
                tx.send(payload).await?;
            }
            tx.send_eof()
        };
        let (result, sent) = tokio::join!(
            store.update(digest, rx, UploadSizeInfo::ExactSize(78)),
            send
        );
        sent?;
        assert_eq!(result.unwrap_err().code, Code::InvalidArgument);
        assert!(
            mock.uploaded_bodies.lock().unwrap().is_empty(),
            "invalid payload {expected:?} must not reach Azure"
        );
    }
    Ok(())
}

#[nativelink_test]
async fn rejects_disconnected_azure_upload_without_hanging() -> Result<(), Error> {
    let mock = MockTransport::new();
    let store = create_test_store(Arc::clone(&mock), Some(Retry::default()), None)?;
    let digest = DigestInfo::try_new(TEST_HASH, 78)?;
    let (tx, rx) = make_buf_channel_pair();
    drop(tx);
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        store.update(digest, rx, UploadSizeInfo::ExactSize(78)),
    )
    .await;
    assert!(
        result
            .expect("disconnected uploads must finish promptly")
            .is_err()
    );
    assert_eq!(mock.request_count(), 0);
    Ok(())
}

#[nativelink_test]
async fn rejects_incomplete_azure_upload_through_verify() -> Result<(), Error> {
    use nativelink_config::stores::{MemorySpec, StoreSpec, VerifySpec};
    use nativelink_store::verify_store::VerifyStore;
    use nativelink_util::store_trait::Store;

    let mock = MockTransport::new();
    let azure = create_test_store(Arc::clone(&mock), Some(Retry::default()), None)?;
    let store = VerifyStore::new(
        &VerifySpec {
            verify_size: true,
            verify_hash: false,
            backend: StoreSpec::Memory(MemorySpec::default()),
        },
        Store::new(azure),
    );
    let digest = DigestInfo::try_new(TEST_HASH, 78)?;
    let (mut tx, rx) = make_buf_channel_pair();
    tx.send_eof()?;
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        store.update(digest, rx, UploadSizeInfo::ExactSize(78)),
    )
    .await;
    assert_eq!(
        result
            .expect("verification errors must finish promptly")
            .unwrap_err()
            .code,
        Code::InvalidArgument
    );
    assert_eq!(mock.request_count(), 0);
    Ok(())
}

#[nativelink_test]
async fn rejects_incomplete_azure_cache_fill() -> Result<(), Error> {
    use nativelink_config::stores::{FastSlowSpec, MemorySpec, StoreSpec};
    use nativelink_store::fast_slow_store::FastSlowStore;
    use nativelink_store::memory_store::MemoryStore;
    use nativelink_util::store_trait::Store;

    let mock = MockTransport::new();
    mock.push_properties(properties_ok(78));
    mock.push_download(download_ok(StatusCode::Ok, Vec::new()));
    let azure = create_test_store(Arc::clone(&mock), Some(Retry::default()), None)?;
    let fast = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: nativelink_config::stores::StoreDirection::default(),
            slow_direction: nativelink_config::stores::StoreDirection::default(),
            bypass_dedup_threshold_bytes: 0,
        },
        fast.clone(),
        Store::new(nativelink_store::verify_store::VerifyStore::new(
            &nativelink_config::stores::VerifySpec {
                verify_size: true,
                verify_hash: false,
                backend: StoreSpec::Memory(MemorySpec::default()),
            },
            Store::new(azure),
        )),
    );
    let digest = DigestInfo::try_new(TEST_HASH, 78)?;
    assert_eq!(fast.has(digest).await?, None);
    // The empty object is reported missing rather than read as a short blob,
    // and either way nothing lands in the fast tier.
    assert_eq!(
        store
            .get_part_unchunked(digest, 0, None)
            .await
            .unwrap_err()
            .code,
        Code::NotFound
    );
    assert_eq!(fast.has(digest).await?, None);
    Ok(())
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

/// An empty object under a digest that names bytes is what a failed upload
/// published; the store reports it missing so the client uploads it again
/// instead of reading it as `DataLoss` forever.
#[nativelink_test]
async fn has_treats_an_empty_object_as_missing() -> Result<(), Error> {
    let mock = MockTransport::new();
    mock.push_properties(properties_ok(0));
    let store = create_test_store(Arc::clone(&mock), None, None)?;

    assert_eq!(store.has(create_test_digest()?).await?, None);
    assert_eq!(mock.request_count(), 1);
    Ok(())
}

#[nativelink_test]
async fn get_part_treats_an_empty_object_as_missing() -> Result<(), Error> {
    let mock = MockTransport::new();
    mock.push_download(download_ok(StatusCode::Ok, Vec::new()));
    let store = create_test_store(Arc::clone(&mock), None, None)?;

    let result = store
        .get_part_unchunked(create_test_digest()?, 0, None)
        .await;
    let err = result.unwrap_err();
    assert_eq!(err.code, Code::NotFound, "{err:?}");
    assert!(err.to_string().contains("empty object"), "{err:?}");
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
        MockClock::advance(Duration::from_hours(24));
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
        MockClock::advance(Duration::from_hours(3 * 24));
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

#[nativelink_test(start_paused = true)]
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

// Paused tokio time: the only thing that advances the clock is the store's
// own sleep, so the elapsed time is exactly what the retrier waited.
#[nativelink_test(start_paused = true)]
async fn a_throttled_download_waits_for_the_retry_after_header() -> Result<(), Error> {
    const VALUE: &str = "test_content";

    let mock = MockTransport::new();
    mock.push_download(CannedResponse {
        status: StatusCode::ServiceUnavailable,
        headers: vec![("retry-after", "3".to_string())],
        body: Vec::new(),
    });
    mock.push_download(download_ok(StatusCode::Ok, VALUE.as_bytes().to_vec()));

    let retry = Retry {
        max_retries: 2,
        delay: 0.0,
        jitter: 0.0,
        ..Default::default()
    };
    let store = create_test_store(Arc::clone(&mock), Some(retry), None)?;

    let started = tokio::time::Instant::now();
    let result = store
        .get_part_unchunked(create_test_digest()?, 0, None)
        .await?;

    assert_eq!(result, VALUE.as_bytes());
    assert_eq!(mock.request_count(), 2);
    assert!(
        started.elapsed() >= Duration::from_secs(3),
        "waited {:?}, expected at least the 3s the server asked for",
        started.elapsed()
    );
    Ok(())
}

/// A header asking for an hour is honoured only up to the cap.
#[nativelink_test(start_paused = true)]
async fn a_throttled_download_waits_at_most_the_cap() -> Result<(), Error> {
    const VALUE: &str = "test_content";

    let mock = MockTransport::new();
    mock.push_download(CannedResponse {
        status: StatusCode::ServiceUnavailable,
        headers: vec![("retry-after", "3600".to_string())],
        body: Vec::new(),
    });
    mock.push_download(download_ok(StatusCode::Ok, VALUE.as_bytes().to_vec()));

    let retry = Retry {
        max_retries: 2,
        delay: 0.0,
        jitter: 0.0,
        ..Default::default()
    };
    let store = create_test_store(Arc::clone(&mock), Some(retry), None)?;
    let cap = Duration::from_secs_f32(CommonObjectSpec::default().throttle_retry_max_delay_s);

    let started = tokio::time::Instant::now();
    let result = store
        .get_part_unchunked(create_test_digest()?, 0, None)
        .await?;

    assert_eq!(result, VALUE.as_bytes());
    assert_eq!(mock.request_count(), 2);
    assert!(
        started.elapsed() >= cap && started.elapsed() < Duration::from_hours(1),
        "waited {:?}, expected the {cap:?} cap, not the hour asked for",
        started.elapsed()
    );
    Ok(())
}

/// Throttles ride on their own budget: with no retries allowed at all, a
/// run of throttled responses still ends in the download.
#[nativelink_test(start_paused = true)]
async fn throttles_do_not_spend_the_retry_budget() -> Result<(), Error> {
    const VALUE: &str = "test_content";

    let mock = MockTransport::new();
    for _ in 0..3 {
        mock.push_download(error_response(StatusCode::ServiceUnavailable));
    }
    mock.push_download(download_ok(StatusCode::Ok, VALUE.as_bytes().to_vec()));

    let retry = Retry {
        max_retries: 0,
        delay: 0.0,
        jitter: 0.0,
        ..Default::default()
    };
    let store = create_test_store(Arc::clone(&mock), Some(retry), None)?;
    let floor = Duration::from_secs_f32(CommonObjectSpec::default().throttle_retry_delay_s);

    let started = tokio::time::Instant::now();
    let result = store
        .get_part_unchunked(create_test_digest()?, 0, None)
        .await?;

    assert_eq!(result, VALUE.as_bytes());
    assert_eq!(mock.request_count(), 4);
    assert!(
        started.elapsed() >= floor * 3,
        "waited {:?}, expected three floors of {floor:?}",
        started.elapsed()
    );
    Ok(())
}

/// Consecutive throttles with no header double from the floor up to the
/// cap: 2, 4, 8, 16, 32 s for five of them under the 60 s default cap, 62 s
/// in all, where five floors would be 10 s.
#[nativelink_test(start_paused = true)]
async fn consecutive_throttles_double_from_the_floor_up_to_the_cap() -> Result<(), Error> {
    const VALUE: &str = "test_content";

    let mock = MockTransport::new();
    for _ in 0..5 {
        mock.push_download(error_response(StatusCode::ServiceUnavailable));
    }
    mock.push_download(download_ok(StatusCode::Ok, VALUE.as_bytes().to_vec()));

    let retry = Retry {
        max_retries: 0,
        delay: 0.0,
        jitter: 0.0,
        ..Default::default()
    };
    let store = create_test_store(Arc::clone(&mock), Some(retry), None)?;
    let floor = Duration::from_secs_f32(CommonObjectSpec::default().throttle_retry_delay_s);
    let cap = Duration::from_secs_f32(CommonObjectSpec::default().throttle_retry_max_delay_s);
    let doubled = floor + floor * 2 + floor * 4 + floor * 8 + floor * 16;
    assert!(
        floor * 16 < cap,
        "the fifth wait must still be under the cap"
    );

    let started = tokio::time::Instant::now();
    let result = store
        .get_part_unchunked(create_test_digest()?, 0, None)
        .await?;

    assert_eq!(result, VALUE.as_bytes());
    assert_eq!(mock.request_count(), 6);
    assert!(
        started.elapsed() >= doubled && started.elapsed() < doubled + floor,
        "waited {:?}, expected the doubled waits of {doubled:?}, not five floors",
        started.elapsed()
    );
    Ok(())
}

#[nativelink_test(start_paused = true)]
async fn a_throttled_download_without_a_header_waits_the_configured_floor() -> Result<(), Error> {
    const VALUE: &str = "test_content";

    let mock = MockTransport::new();
    mock.push_download(error_response(StatusCode::ServiceUnavailable));
    mock.push_download(download_ok(StatusCode::Ok, VALUE.as_bytes().to_vec()));

    let retry = Retry {
        max_retries: 2,
        delay: 0.0,
        jitter: 0.0,
        ..Default::default()
    };
    let store = create_test_store(Arc::clone(&mock), Some(retry), None)?;
    let floor = Duration::from_secs_f32(CommonObjectSpec::default().throttle_retry_delay_s);

    let started = tokio::time::Instant::now();
    let result = store
        .get_part_unchunked(create_test_digest()?, 0, None)
        .await?;

    assert_eq!(result, VALUE.as_bytes());
    assert_eq!(mock.request_count(), 2);
    assert!(
        started.elapsed() >= floor,
        "waited {:?}, expected at least the {floor:?} floor",
        started.elapsed()
    );
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

    let update_fut = store.update(
        nativelink_store::cas_utils::ZERO_BYTE_DIGESTS[0],
        rx,
        UploadSizeInfo::ExactSize(0),
    );
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
async fn canonical_empty_upload_checks_content_and_declared_size() -> Result<(), Error> {
    let cases: [(UploadSizeInfo, &'static [u8], bool); 5] = [
        (UploadSizeInfo::ExactSize(1), b"", false),
        (UploadSizeInfo::ExactSize(1), b"x", false),
        (UploadSizeInfo::MaxSize(1), b"x", false),
        (UploadSizeInfo::ExactSize(0), b"", true),
        (UploadSizeInfo::MaxSize(1), b"", true),
    ];
    for digest in nativelink_store::cas_utils::ZERO_BYTE_DIGESTS {
        for (upload_size, payload, valid) in cases {
            let mock = MockTransport::new();
            let store = create_test_store(Arc::clone(&mock), None, None)?;
            let (mut tx, rx) = make_buf_channel_pair();
            let send = async move {
                if !payload.is_empty() {
                    tx.send(Bytes::from_static(payload)).await?;
                }
                tx.send_eof()
            };
            let (result, sent) = tokio::join!(store.update(digest, rx, upload_size), send);
            sent?;
            if valid {
                assert_eq!(result?, 0);
            } else {
                assert_eq!(result.unwrap_err().code, Code::InvalidArgument);
            }
            assert_eq!(mock.request_count(), 0);
        }
    }
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

/// Azure answers a download with the block it read, so for a blob whose
/// length is not a multiple of the block size the last block runs past the
/// end. The store hands the caller everything it was given; a raw surplus
/// is the verify store's to judge. A compression store above reads the
/// object it wrote under this key, and an lz4 frame of data that does not
/// compress is longer than the digest it is stored under, so a read cut at
/// the digest's size is a torn frame and the blob is unreadable.
#[nativelink_test]
async fn a_body_longer_than_the_digest_is_handed_on_whole() -> Result<(), Error> {
    const CONTENT: u64 = 100;
    const BLOCK: u64 = 64;
    let content: Vec<u8> = (0..CONTENT)
        .map(|i| u8::try_from(i % 251).unwrap())
        .collect();
    // What Azure holds: more than the digest names, as a compressed frame is.
    let mut stored = content.clone();
    stored.resize(usize::try_from(BLOCK * 2).unwrap(), 0xAA);

    let mock = MockTransport::new();
    mock.push_download(download_ok(StatusCode::Ok, stored.clone()));

    let store = create_test_store(Arc::clone(&mock), None, None)?;
    let result = store
        .get_part_unchunked(DigestInfo::try_new(TEST_HASH, CONTENT)?, 0, None)
        .await?;

    assert_eq!(result, stored);
    Ok(())
}

/// The same bound applies to an explicit length, and to a read that starts
/// part way in.
#[nativelink_test]
async fn an_explicit_length_bounds_an_over_long_body() -> Result<(), Error> {
    const OFFSET: u64 = 10;
    const LENGTH: u64 = 20;
    let body: Vec<u8> = (0..100u8).collect();

    let mock = MockTransport::new();
    mock.push_download(download_ok(StatusCode::PartialContent, body.clone()));

    let store = create_test_store(Arc::clone(&mock), None, None)?;
    let result = store
        .get_part_unchunked(create_test_digest()?, OFFSET, Some(LENGTH))
        .await?;

    assert_eq!(result.len(), usize::try_from(LENGTH).unwrap());
    assert_eq!(result, &body[..usize::try_from(LENGTH).unwrap()]);
    Ok(())
}

/// The range a read asks for is built from the offset plus whatever the
/// writer has already taken, so a retry continues the read instead of
/// starting it again and appending a second copy of what the consumer
/// already holds. The mock cannot fail a response part way through, so this
/// pins the arithmetic on the first attempt, where the written count is
/// zero; the resume itself follows the shape the S3 store already uses.
#[nativelink_test(start_paused = true)]
async fn the_requested_range_starts_at_the_offset_plus_what_was_sent() -> Result<(), Error> {
    const OFFSET: u64 = 10;
    const LENGTH: u64 = 20;
    let body: Vec<u8> = (0..100u8).collect();

    let mock = MockTransport::new();
    mock.push_download(error_response(StatusCode::ServiceUnavailable));
    mock.push_download(download_ok(StatusCode::PartialContent, body));

    let retry = Retry {
        max_retries: 8,
        delay: 0.0,
        jitter: 0.0,
        ..Default::default()
    };
    let store = create_test_store(Arc::clone(&mock), Some(retry), None)?;
    let result = store
        .get_part_unchunked(create_test_digest()?, OFFSET, Some(LENGTH))
        .await?;
    assert_eq!(result.len(), usize::try_from(LENGTH).unwrap());

    let ranges = mock.download_ranges();
    assert_eq!(ranges.len(), 2, "expected the failed attempt and the retry");
    for (n, range) in ranges.iter().enumerate() {
        assert!(
            range
                .as_deref()
                .is_some_and(|r| r.contains(&format!("bytes={OFFSET}"))),
            "attempt {n} asked for {range:?}, expected it to start at {OFFSET}"
        );
    }
    Ok(())
}

/// With adaptive read concurrency on, each throttled response halves the
/// read limit (the two here are a retry wait apart, so they are two
/// events), and a run of answered reads grows it back one at a time.
#[nativelink_test(start_paused = true)]
async fn throttled_reads_halve_the_read_limit_and_successes_regrow_it() -> Result<(), Error> {
    const VALUE: &str = "test_content";

    let mock = MockTransport::new();
    mock.push_download(error_response(StatusCode::ServiceUnavailable));
    mock.push_download(error_response(StatusCode::ServiceUnavailable));
    mock.push_download(download_ok(StatusCode::Ok, VALUE.as_bytes().to_vec()));

    let store = create_test_store_with_common(
        Arc::clone(&mock),
        CommonObjectSpec {
            retry: Retry {
                max_retries: 0,
                delay: 0.0,
                jitter: 0.0,
                ..Default::default()
            },
            max_concurrent_reads: 16,
            adaptive_read_concurrency: true,
            min_concurrent_reads: 2,
            ..Default::default()
        },
    )?;
    assert_eq!(store.read_limit(), Some(16));

    let result = store
        .get_part_unchunked(create_test_digest()?, 0, None)
        .await?;
    assert_eq!(result, VALUE.as_bytes());
    assert_eq!(mock.request_count(), 3);
    assert_eq!(
        store.read_limit(),
        Some(4),
        "two throttles, two halvings; the download that got through is one toward regrowth"
    );

    // Sixteen answered downloads in a row (the one above was the first)
    // add one permit.
    for _ in 0..15 {
        mock.push_download(download_ok(StatusCode::Ok, VALUE.as_bytes().to_vec()));
        store
            .get_part_unchunked(create_test_digest()?, 0, None)
            .await?;
    }
    assert_eq!(store.read_limit(), Some(5));
    Ok(())
}

/// Existence checks are outside the download pool: a throttled `HEAD`
/// does not cut the limit and an answered one does not grow it.
#[nativelink_test(start_paused = true)]
async fn existence_checks_do_not_move_the_read_limit() -> Result<(), Error> {
    let mock = MockTransport::new();
    mock.push_properties(error_response(StatusCode::ServiceUnavailable));
    mock.push_properties(properties_ok(512));

    let store = create_test_store_with_common(
        Arc::clone(&mock),
        CommonObjectSpec {
            retry: Retry {
                max_retries: 0,
                delay: 0.0,
                jitter: 0.0,
                ..Default::default()
            },
            max_concurrent_reads: 16,
            adaptive_read_concurrency: true,
            min_concurrent_reads: 2,
            ..Default::default()
        },
    )?;

    assert_eq!(store.has(create_test_digest()?).await?, Some(512));
    assert_eq!(mock.request_count(), 2, "the throttled HEAD was retried");
    assert_eq!(
        store.read_limit(),
        Some(16),
        "a throttled HEAD cuts nothing"
    );

    for _ in 0..16 {
        mock.push_properties(properties_ok(512));
        store.has(create_test_digest()?).await?;
    }
    assert_eq!(store.read_limit(), Some(16), "answered HEADs grow nothing");
    Ok(())
}

/// Without `adaptive_read_concurrency` the bound is fixed at
/// `max_concurrent_reads` whatever the store answers.
#[nativelink_test(start_paused = true)]
async fn a_fixed_read_limit_does_not_move_on_a_throttle() -> Result<(), Error> {
    const VALUE: &str = "test_content";

    let mock = MockTransport::new();
    mock.push_download(error_response(StatusCode::ServiceUnavailable));
    mock.push_download(download_ok(StatusCode::Ok, VALUE.as_bytes().to_vec()));

    let store = create_test_store_with_common(
        Arc::clone(&mock),
        CommonObjectSpec {
            retry: Retry {
                max_retries: 0,
                delay: 0.0,
                jitter: 0.0,
                ..Default::default()
            },
            max_concurrent_reads: 8,
            ..Default::default()
        },
    )?;

    store
        .get_part_unchunked(create_test_digest()?, 0, None)
        .await?;
    assert_eq!(store.read_limit(), Some(8));
    Ok(())
}

#[nativelink_test]
async fn incomplete_multipart_upload_never_commits_azure_blocks() -> Result<(), Error> {
    for payload in [Bytes::new(), Bytes::from_static(b"short")] {
        let mock = MockTransport::new();
        let store = create_test_store(mock.clone(), None, None)?;
        let (mut tx, rx) = make_buf_channel_pair();
        let send = async move {
            if !payload.is_empty() {
                tx.send(payload).await?;
            }
            tx.send_eof()
        };
        let (result, _) = tokio::join!(
            store.update(
                DigestInfo::try_new(TEST_HASH, 8 * 1024 * 1024)?,
                rx,
                UploadSizeInfo::ExactSize(8 * 1024 * 1024)
            ),
            send
        );
        assert_eq!(result.unwrap_err().code, Code::InvalidArgument);
        assert!(mock.committed_block_list().is_none());
        assert!(mock.uploaded_bodies.lock().unwrap().is_empty());
    }
    Ok(())
}

// Size validation must run inside the guarded upload so failure also discards
// staged blocks, while retaining any previously committed copy.
#[nativelink_test]
async fn invalid_multipart_size_discards_staged_blocks_and_preserves_committed_copy()
-> Result<(), Error> {
    const BLOCK: usize = 5 * 1024 * 1024;
    for upload_size in [
        UploadSizeInfo::ExactSize(8 * 1024 * 1024),
        UploadSizeInfo::MaxSize(4 * 1024 * 1024),
    ] {
        for existing_copy in [false, true] {
            let mock = MockTransport::new();
            let committed = if existing_copy {
                &["old-a", "old-b"][..]
            } else {
                &[]
            };
            mock.push_block_list(block_list_ok(
                committed,
                &["00000000000000000000000000000000"],
            ));
            let store = create_test_store(Arc::clone(&mock), None, None)?;
            let digest = DigestInfo::try_new(TEST_HASH, 8 * 1024 * 1024)?;
            let (mut tx, rx) = make_buf_channel_pair();
            let send = async move {
                tx.send(Bytes::from(vec![7u8; BLOCK])).await?;
                tx.send_eof()
            };
            let (result, sent) = tokio::join!(store.update(digest, rx, upload_size), send);
            sent?;
            assert_eq!(result.unwrap_err().code, Code::InvalidArgument);
            assert_eq!(mock.request_count(), 3, "stage, inspect, then discard");
            if existing_copy {
                assert_eq!(mock.deleted(), Vec::<String>::new());
                let retained = mock
                    .committed_block_list()
                    .expect("retain committed blocks");
                let retained = core::str::from_utf8(&retained).expect("utf-8");
                for id in committed {
                    let encoded = azure_core::base64::encode(id.as_bytes());
                    assert!(retained.contains(&format!("<Committed>{encoded}</Committed>")));
                }
                assert!(!retained.contains("<Latest>") && !retained.contains("<Uncommitted>"));
            } else {
                assert_eq!(mock.deleted(), vec![format!("/{TEST_CONTAINER}/{digest}")]);
                assert!(mock.committed_block_list().is_none());
            }
        }
    }
    Ok(())
}

/// Ten MiB of payload: two staged blocks.
fn two_block_payload() -> Vec<u8> {
    const TOTAL_SIZE: usize = 2 * 5 * 1024 * 1024;
    (0..TOTAL_SIZE)
        .map(|i| u8::try_from((i * 7) % 256).unwrap())
        .collect()
}

/// A block that fails to stage fails the upload, and the blocks that did
/// stage are discarded: with nothing committed under the name, the blob is
/// deleted, and no block list is ever committed.
#[nativelink_test]
async fn failed_staged_upload_deletes_the_uncommitted_blob() -> Result<(), Error> {
    let send_data = two_block_payload();
    let mock = MockTransport::new();
    // The first block stages, the second is refused for good.
    mock.push_stage_block(created());
    mock.push_stage_block(CannedResponse {
        status: StatusCode::BadRequest,
        headers: Vec::new(),
        body: Vec::new(),
    });
    mock.push_block_list(block_list_ok(&[], &["00000000000000000000000000000000"]));
    let store = create_test_store(Arc::clone(&mock), Some(Retry::default()), None)?;

    let digest = DigestInfo::try_new(TEST_HASH, send_data.len())?;
    let err = store
        .update_oneshot(StoreKey::from(&digest), send_data.into())
        .await
        .expect_err("the refused block fails the upload");
    assert_eq!(err.code, Code::Aborted, "{err:?}");

    assert_eq!(
        mock.deleted(),
        vec![format!(
            "/{TEST_CONTAINER}/{TEST_HASH}-{}",
            2 * 5 * 1024 * 1024
        )],
        "the name holding only staged blocks is deleted"
    );
    assert!(
        mock.committed_block_list().is_none(),
        "no block list is committed for a failed upload"
    );
    Ok(())
}

/// When the name already holds a committed blob (a re-upload of a key that
/// exists), the committed blocks are kept: re-committing that list is what
/// discards the staged ones, and nothing is deleted.
#[nativelink_test]
async fn failed_staged_upload_keeps_the_committed_blocks() -> Result<(), Error> {
    let send_data = two_block_payload();
    let mock = MockTransport::new();
    mock.push_stage_block(created());
    mock.push_stage_block(CannedResponse {
        status: StatusCode::BadRequest,
        headers: Vec::new(),
        body: Vec::new(),
    });
    mock.push_block_list(block_list_ok(
        &["old-a", "old-b"],
        &["00000000000000000000000000000000"],
    ));
    let store = create_test_store(Arc::clone(&mock), Some(Retry::default()), None)?;

    let digest = DigestInfo::try_new(TEST_HASH, send_data.len())?;
    store
        .update_oneshot(StoreKey::from(&digest), send_data.into())
        .await
        .expect_err("the refused block fails the upload");

    assert!(
        mock.deleted().is_empty(),
        "a committed blob is never deleted"
    );
    let committed = mock
        .committed_block_list()
        .expect("the committed list is re-committed to drop the staged blocks");
    let committed = core::str::from_utf8(&committed).expect("utf-8");
    for id in ["old-a", "old-b"] {
        let encoded = azure_core::base64::encode(id.as_bytes());
        assert!(
            committed.contains(&format!("<Committed>{encoded}</Committed>")),
            "{id} must be kept as a committed block: {committed}"
        );
    }
    assert!(
        !committed.contains("<Latest>") && !committed.contains("<Uncommitted>"),
        "only the committed blocks are kept: {committed}"
    );
    Ok(())
}

/// An upload whose future is dropped before it finishes (the `ByteStream` that
/// fed it died and the server swept the stream) still discards the blocks it
/// staged, from a background task, since the dropped future runs nothing.
#[nativelink_test(start_paused = true)]
async fn dropped_staged_upload_discards_its_blocks() -> Result<(), Error> {
    const BLOCK: usize = 5 * 1024 * 1024;
    let mock = MockTransport::new();
    mock.push_block_list(block_list_ok(&[], &["00000000000000000000000000000000"]));
    let store = create_test_store(Arc::clone(&mock), None, None)?;

    let digest = DigestInfo::try_new(TEST_HASH, 2 * BLOCK)?;
    let (mut tx, rx) = make_buf_channel_pair();
    let mut update_fut = Box::pin(store.update(
        StoreKey::from(&digest),
        rx,
        UploadSizeInfo::ExactSize(2 * BLOCK as u64),
    ));

    // One block and the start of the next arrive (the reader only hands a
    // block on once it sees data behind it); the rest never comes, so the
    // upload sits waiting on the reader until it is dropped.
    tx.send(Bytes::from(vec![7u8; BLOCK])).await?;
    tx.send(Bytes::from(vec![7u8; 1024])).await?;
    assert!(
        tokio::time::timeout(Duration::from_secs(1), &mut update_fut)
            .await
            .is_err(),
        "the upload must still be waiting for data"
    );
    assert_eq!(mock.request_count(), 1, "one staged block");
    drop(update_fut);

    // The guard's background task waits out its grace period, then reads
    // the block list and deletes the uncommitted blob.
    tokio::time::sleep(Duration::from_secs(10)).await;
    tokio::task::yield_now().await;
    assert_eq!(
        mock.deleted(),
        vec![format!("/{TEST_CONTAINER}/{TEST_HASH}-{}", 2 * BLOCK)],
        "the dropped upload's staged blocks are discarded"
    );
    assert!(mock.committed_block_list().is_none());
    Ok(())
}

#[test]
fn tls_roots_exclude_distrusted_emsign_ecc_root() {
    // RUSTSEC-2026-0381: webpki-roots before 1.0.10 still trusts this root,
    // which lets certificate paths through distrusted emSign intermediates.
    // The Azure transport builds its root store from this list.
    const DISTRUSTED_CN: &[u8] = b"emSign ECC Root CA - G3";
    let trusted = webpki_roots::TLS_SERVER_ROOTS.iter().find(|anchor| {
        anchor
            .subject
            .windows(DISTRUSTED_CN.len())
            .any(|window| window == DISTRUSTED_CN)
    });
    assert!(trusted.is_none(), "distrusted root is still trusted");
    // An empty store would make the check above pass without proving the
    // distrusted root was removed.
    assert_ne!(webpki_roots::TLS_SERVER_ROOTS, []);
}

#[test]
fn tls_verifier_rejects_distrusted_emsign_intermediate_key() {
    use rcgen::{
        BasicConstraints, CertificateParams, IsCa, Issuer, KeyPair, PKCS_ECDSA_P384_SHA384,
        PublicKeyData, SignatureAlgorithm,
    };
    use rustls::client::danger::ServerCertVerifier;
    use rustls::pki_types::{ServerName, UnixTime};

    // The public key of eMudhra's OV TLS CA - G3 (an uncompressed P-384
    // point), one of the intermediates rustls-webpki distrusts.
    struct DistrustedKey(Vec<u8>);
    impl PublicKeyData for DistrustedKey {
        fn der_bytes(&self) -> &[u8] {
            &self.0
        }
        fn algorithm(&self) -> &'static SignatureAlgorithm {
            &PKCS_ECDSA_P384_SHA384
        }
    }
    let point = [
        "0473897aa0eb3cfdca8b12402ffaf0abc4e1212baf8dee778ac9ee2d6db83fac",
        "280c5e05638ae7a2acd71c322a8e361cdeb9f69c4302b9dd8d73d1cc6caab2cc",
        "0aee4e216ce979dd518a4af4b626e5684cdd658037ed0c854e4cc12f4cf8646d",
        "0d",
    ]
    .concat();
    let distrusted_key = DistrustedKey(hex::decode(point).unwrap());

    // RUSTSEC-2026-0382: rustls-webpki before 0.103.16 accepts paths through
    // emSign intermediates whose private keys are public. A test root issues
    // one leaf for the distrusted key and a control leaf for a fresh key.
    let root_key = KeyPair::generate().unwrap();
    let mut root_params = CertificateParams::new(Vec::<String>::new()).unwrap();
    root_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let root = root_params.self_signed(&root_key).unwrap();
    let issuer = Issuer::new(root_params, root_key);
    let leaf_params = CertificateParams::new(vec!["distrusted.test".to_string()]).unwrap();
    let distrusted = leaf_params.signed_by(&distrusted_key, &issuer).unwrap();
    let control = leaf_params
        .signed_by(&KeyPair::generate().unwrap(), &issuer)
        .unwrap();

    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.der().clone()).unwrap();
    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::new(rustls::crypto::ring::default_provider()),
    )
    .build()
    .unwrap();
    let verify = |leaf: &rcgen::Certificate| {
        verifier.verify_server_cert(
            leaf.der(),
            &[],
            &ServerName::try_from("distrusted.test").unwrap(),
            &[],
            UnixTime::now(),
        )
    };

    verify(&control).unwrap();
    let err = verify(&distrusted).unwrap_err();
    assert!(format!("{err:?}").contains("CertDistrusted"), "{err:?}");
}
