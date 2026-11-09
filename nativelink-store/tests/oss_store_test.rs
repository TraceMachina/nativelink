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

//! OSS-specific config and S3 wire behavior. No real bucket or credentials required.

use std::sync::Arc;

use aws_sdk_s3::config::Credentials;
use aws_smithy_http_client::test_util::{ReplayEvent, StaticReplayClient};
use aws_smithy_types::body::SdkBody;
use http::StatusCode;
use nativelink_config::stores::{ExperimentalOssSpec, Retry};
use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_store::oss_store::OssStore;
use nativelink_store::s3_store::S3Store;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::store_trait::{StoreKey, StoreLike, UploadSizeInfo};

const HOST: &str = "test-bucket.s3.oss-cn-hangzhou.aliyuncs.com";
const NOW: fn() -> MockInstantWrapped = MockInstantWrapped::default;

fn spec() -> ExperimentalOssSpec {
    ExperimentalOssSpec {
        region: "cn-hangzhou".into(),
        bucket: "test-bucket".into(),
        key_prefix: Some("cas/".into()),
        retry: Retry {
            max_retries: 1,
            delay: 0.0,
            jitter: 0.0,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn store(mock: StaticReplayClient) -> Result<Arc<S3Store<fn() -> MockInstantWrapped>>, Error> {
    OssStore::new_with_http_client(
        &spec(),
        mock,
        Credentials::new("TESTKEY", "TESTSECRET", None, None, "test"),
        NOW,
    )
}

fn event(status: StatusCode, body: &'static str) -> ReplayEvent {
    ReplayEvent::new(
        http::Request::builder().body(SdkBody::empty()).unwrap(),
        http::Response::builder()
            .status(status)
            .body(SdkBody::from(body))
            .unwrap(),
    )
}

#[test]
fn endpoint_and_minimal_settings() -> Result<(), Error> {
    assert_eq!(
        OssStore::endpoint(&spec())?,
        "https://s3.oss-cn-hangzhou.aliyuncs.com"
    );
    let aws = OssStore::build_aws_spec(&spec())?;
    assert_eq!(aws.region, "cn-hangzhou");
    assert_eq!(aws.bucket, "test-bucket");
    assert_eq!(aws.common.key_prefix.as_deref(), Some("cas/"));
    assert_eq!(aws.common.retry.max_retries, 1);
    assert_eq!(aws.common.consider_expired_after_s, 0);
    assert_eq!(aws.common.multipart_max_concurrent_uploads, Some(2));
    Ok(())
}

#[test]
fn rejects_invalid_config_without_echoing_endpoint_credentials() {
    for endpoint in [
        "http://example.com",
        "https://user:secret@example.com",
        "https://example.com/bucket",
        "https://example.com/?secret=hidden",
        "https://example.com/#fragment",
        "not a url",
    ] {
        let err = OssStore::build_aws_spec(&ExperimentalOssSpec {
            endpoint: Some(endpoint.into()),
            ..spec()
        })
        .unwrap_err();
        assert_eq!(err.code, Code::InvalidArgument);
        assert!(!err.to_string().contains("hidden"));
        assert!(!err.to_string().contains("secret@example"));
    }
    assert_eq!(
        OssStore::build_aws_spec(&ExperimentalOssSpec {
            multipart_max_concurrent_uploads: Some(0),
            ..spec()
        })
        .unwrap_err()
        .code,
        Code::InvalidArgument
    );
    for bucket in [
        "",
        "ab",
        "Uppercase",
        "with.dot",
        "with/slash",
        "-leading",
        "trailing-",
    ] {
        assert_eq!(
            OssStore::build_aws_spec(&ExperimentalOssSpec {
                bucket: bucket.into(),
                ..spec()
            })
            .unwrap_err()
            .code,
            Code::InvalidArgument
        );
    }
    for region in ["", "../region", "region/", "region?token=hidden"] {
        assert_eq!(
            OssStore::build_aws_spec(&ExperimentalOssSpec {
                region: region.into(),
                ..spec()
            })
            .unwrap_err()
            .code,
            Code::InvalidArgument
        );
    }
}

#[nativelink_test]
async fn head_uses_virtual_host_prefix_and_signing_region() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![ReplayEvent::new(
        http::Request::builder().body(SdkBody::empty()).unwrap(),
        http::Response::builder()
            .status(StatusCode::OK)
            .header("content-length", "17")
            .body(SdkBody::empty())
            .unwrap(),
    )]);
    assert_eq!(
        store(mock.clone())?.has(StoreKey::new_str("key")).await?,
        Some(17)
    );
    let requests: Vec<_> = mock.actual_requests().collect();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method(), "HEAD");
    assert_eq!(requests[0].uri(), format!("https://{HOST}/cas/key"));
    assert!(
        requests[0]
            .headers()
            .get("authorization")
            .unwrap()
            .contains("/cn-hangzhou/s3/aws4_request")
    );
    Ok(())
}

#[nativelink_test]
async fn head_missing_is_a_cache_miss() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![event(StatusCode::NOT_FOUND, "")]);
    assert_eq!(store(mock)?.has(StoreKey::new_str("missing")).await?, None);
    Ok(())
}

#[nativelink_test]
async fn get_missing_reports_not_found() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![event(
        StatusCode::NOT_FOUND,
        "<Error><Code>NoSuchKey</Code></Error>",
    )]);
    let err = store(mock)?
        .get_part_unchunked(StoreKey::new_str("missing"), 0, None)
        .await
        .unwrap_err();
    assert_eq!(err.code, Code::NotFound);
    Ok(())
}

#[nativelink_test]
async fn ranged_get_and_encoded_key_use_oss_endpoint() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![event(StatusCode::PARTIAL_CONTENT, "bcd")]);
    let bytes = store(mock.clone())?
        .get_part_unchunked(StoreKey::new_str("a b?c"), 1, Some(3))
        .await?;
    assert_eq!(bytes, "bcd");
    let requests: Vec<_> = mock.actual_requests().collect();
    assert!(
        requests[0]
            .uri()
            .starts_with(&format!("https://{HOST}/cas/a%20b%3Fc"))
    );
    assert_eq!(requests[0].headers().get("range"), Some("bytes=1-3"));
    Ok(())
}

#[nativelink_test]
async fn single_put_does_not_enable_aws_chunked_encoding() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![event(StatusCode::OK, "")]);
    store(mock.clone())?
        .update_oneshot(StoreKey::new_str("key"), "hello".into())
        .await?;
    let requests: Vec<_> = mock.actual_requests().collect();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(request.method(), "PUT");
    assert_eq!(request.headers().get("content-length"), Some("5"));
    assert!(request.headers().get("transfer-encoding").is_none());
    assert!(
        request
            .uri()
            .starts_with(&format!("https://{HOST}/cas/key"))
    );
    assert_ne!(
        request.headers().get("content-encoding"),
        Some("aws-chunked")
    );
    assert!(
        !request
            .headers()
            .get("x-amz-content-sha256")
            .unwrap_or_default()
            .contains("STREAMING")
    );
    assert!(
        request
            .headers()
            .get("x-amz-sdk-checksum-algorithm")
            .is_none()
    );
    Ok(())
}

#[nativelink_test]
async fn multipart_upload_initiates_uploads_and_completes() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![
        event(
            StatusCode::OK,
            "<InitiateMultipartUploadResult><UploadId>upload-1</UploadId></InitiateMultipartUploadResult>",
        ),
        ReplayEvent::new(
            http::Request::builder().body(SdkBody::empty()).unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .header("etag", "part-etag")
                .body(SdkBody::empty())
                .unwrap(),
        ),
        event(
            StatusCode::OK,
            "<CompleteMultipartUploadResult><ETag>object-etag</ETag></CompleteMultipartUploadResult>",
        ),
    ]);
    let store = store(mock.clone())?;
    let (mut tx, rx) = make_buf_channel_pair();
    let upload = store.update(StoreKey::new_str("key"), rx, UploadSizeInfo::MaxSize(10));
    let send = async {
        tx.send("hello".into()).await?;
        tx.send_eof()
    };
    let (written, ()) = futures::try_join!(upload, send)?;
    assert_eq!(written, 5);
    let requests: Vec<_> = mock.actual_requests().collect();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].method(), "POST");
    assert!(requests[0].uri().contains("uploads"));
    assert_eq!(requests[1].method(), "PUT");
    assert!(requests[1].uri().contains("partNumber=1"));
    assert!(requests[1].uri().contains("uploadId=upload-1"));
    assert_eq!(requests[2].method(), "POST");
    assert!(requests[2].uri().contains("uploadId=upload-1"));
    let completion = std::str::from_utf8(requests[2].body().bytes().unwrap()).unwrap();
    assert!(completion.contains("<PartNumber>1</PartNumber>"));
    assert!(completion.contains("<ETag>part-etag</ETag>"));
    Ok(())
}

#[nativelink_test]
async fn multipart_size_error_aborts_without_publishing() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![
        event(
            StatusCode::OK,
            "<InitiateMultipartUploadResult><UploadId>upload-1</UploadId></InitiateMultipartUploadResult>",
        ),
        ReplayEvent::new(
            http::Request::builder().body(SdkBody::empty()).unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .header("etag", "part-etag")
                .body(SdkBody::empty())
                .unwrap(),
        ),
        event(StatusCode::NO_CONTENT, ""),
    ]);
    let store = store(mock.clone())?;
    let (mut tx, rx) = make_buf_channel_pair();
    let upload = store.update(StoreKey::new_str("key"), rx, UploadSizeInfo::MaxSize(2));
    let send = async {
        tx.send("hello".into()).await?;
        tx.send_eof()
    };
    let (written, sent) = tokio::join!(upload, send);
    sent?;
    assert_eq!(written.unwrap_err().code, Code::InvalidArgument);
    let requests: Vec<_> = mock.actual_requests().collect();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[2].method(), "DELETE");
    Ok(())
}

#[nativelink_test]
async fn transient_head_retries_through_shared_store() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![
        event(
            StatusCode::SERVICE_UNAVAILABLE,
            "<Error><Code>ServiceUnavailable</Code></Error>",
        ),
        ReplayEvent::new(
            http::Request::builder().body(SdkBody::empty()).unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .header("content-length", "5")
                .body(SdkBody::empty())
                .unwrap(),
        ),
    ]);
    assert_eq!(
        store(mock.clone())?.has(StoreKey::new_str("key")).await?,
        Some(5)
    );
    assert_eq!(mock.actual_requests().count(), 2);
    Ok(())
}

#[nativelink_test]
async fn streaming_read_requests_exact_inclusive_range() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![event(StatusCode::PARTIAL_CONTENT, "bcd")]);
    let store = store(mock.clone())?;
    let (mut tx, mut rx) = make_buf_channel_pair();
    let read = store.get_part(StoreKey::new_str("key"), &mut tx, 1, Some(3));
    let (_, bytes) = futures::try_join!(read, rx.consume(None))?;
    assert_eq!(bytes, "bcd");
    let requests: Vec<_> = mock.actual_requests().collect();
    assert_eq!(requests[0].headers().get("range"), Some("bytes=1-3"));
    Ok(())
}

#[nativelink_test]
async fn zero_length_read_makes_no_request() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![]);
    let bytes = store(mock.clone())?
        .get_part_unchunked(StoreKey::new_str("key"), 1, Some(0))
        .await?;
    assert!(bytes.is_empty());
    assert_eq!(mock.actual_requests().count(), 0);
    Ok(())
}

#[nativelink_test]
async fn endpoint_override_and_temporary_credentials_reach_the_wire() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![event(StatusCode::NOT_FOUND, "")]);
    let spec = ExperimentalOssSpec {
        endpoint: Some("https://s3.oss-cn-hangzhou-internal.aliyuncs.com".into()),
        ..spec()
    };
    let store = OssStore::new_with_http_client(
        &spec,
        mock.clone(),
        Credentials::new(
            "TESTKEY",
            "TESTSECRET",
            Some("TESTTOKEN".into()),
            None,
            "test",
        ),
        NOW,
    )?;
    assert_eq!(store.has(StoreKey::new_str("key")).await?, None);
    let requests: Vec<_> = mock.actual_requests().collect();
    assert_eq!(
        requests[0].uri(),
        "https://test-bucket.s3.oss-cn-hangzhou-internal.aliyuncs.com/cas/key"
    );
    assert_eq!(
        requests[0].headers().get("x-amz-security-token"),
        Some("TESTTOKEN")
    );
    Ok(())
}

#[nativelink_test]
async fn exact_size_error_does_not_publish_an_object() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![]);
    let store = store(mock.clone())?;
    let (mut tx, rx) = make_buf_channel_pair();
    let upload = store.update(StoreKey::new_str("key"), rx, UploadSizeInfo::ExactSize(4));
    let send = async {
        tx.send("abc".into()).await?;
        tx.send_eof()
    };
    let (written, sent) = tokio::join!(upload, send);
    sent?;
    assert_eq!(written.unwrap_err().code, Code::InvalidArgument);
    assert_eq!(mock.actual_requests().count(), 0);
    Ok(())
}

#[derive(Clone, Debug)]
struct GatedPartClient {
    gate: Arc<tokio::sync::Semaphore>,
    started: Arc<tokio::sync::Notify>,
    active: Arc<std::sync::atomic::AtomicUsize>,
    peak: Arc<std::sync::atomic::AtomicUsize>,
    parts: Arc<std::sync::atomic::AtomicUsize>,
}

impl aws_smithy_runtime_api::client::http::HttpConnector for GatedPartClient {
    fn call(
        &self,
        request: aws_smithy_runtime_api::client::orchestrator::HttpRequest,
    ) -> aws_smithy_runtime_api::client::http::HttpConnectorFuture {
        use std::sync::atomic::Ordering;
        let client = self.clone();
        aws_smithy_runtime_api::client::http::HttpConnectorFuture::new(async move {
            let response = if request.uri().contains("partNumber=") {
                client.parts.fetch_add(1, Ordering::SeqCst);
                let active = client.active.fetch_add(1, Ordering::SeqCst) + 1;
                client.peak.fetch_max(active, Ordering::SeqCst);
                client.started.notify_one();
                client.gate.acquire().await.unwrap().forget();
                client.active.fetch_sub(1, Ordering::SeqCst);
                http::Response::builder()
                    .header("etag", "part-etag")
                    .body(SdkBody::empty())
                    .unwrap()
            } else if request.uri().contains("uploads") {
                http::Response::builder().body(SdkBody::from("<InitiateMultipartUploadResult><UploadId>upload-1</UploadId></InitiateMultipartUploadResult>")).unwrap()
            } else {
                http::Response::builder().body(SdkBody::from("<CompleteMultipartUploadResult><ETag>object-etag</ETag></CompleteMultipartUploadResult>")).unwrap()
            };
            Ok(response.try_into().unwrap())
        })
    }
}

impl aws_smithy_runtime_api::client::http::HttpClient for GatedPartClient {
    fn http_connector(
        &self,
        _: &aws_smithy_runtime_api::client::http::HttpConnectorSettings,
        _: &aws_smithy_runtime_api::client::runtime_components::RuntimeComponents,
    ) -> aws_smithy_runtime_api::client::http::SharedHttpConnector {
        aws_smithy_runtime_api::client::http::SharedHttpConnector::new(self.clone())
    }
}

#[nativelink_test]
async fn slow_backend_bounds_in_flight_multipart_requests() -> Result<(), Error> {
    use std::sync::atomic::{AtomicUsize, Ordering};
    const PARTS: usize = 12;
    const PART_BYTES: usize = 5 * 1024 * 1024;
    let client = GatedPartClient {
        gate: Arc::new(tokio::sync::Semaphore::new(0)),
        started: Arc::new(tokio::sync::Notify::new()),
        active: Arc::new(AtomicUsize::new(0)),
        peak: Arc::new(AtomicUsize::new(0)),
        parts: Arc::new(AtomicUsize::new(0)),
    };
    let store = OssStore::new_with_http_client(
        &spec(),
        client.clone(),
        Credentials::new("TESTKEY", "TESTSECRET", None, None, "test"),
        NOW,
    )?;
    let (mut tx, rx) = make_buf_channel_pair();
    let upload = tokio::spawn(async move {
        store
            .update(
                StoreKey::new_str("key"),
                rx,
                UploadSizeInfo::ExactSize((PARTS * PART_BYTES) as u64),
            )
            .await
    });
    let send = tokio::spawn(async move {
        let chunk = bytes::Bytes::from(vec![1; PART_BYTES]);
        for _ in 0..PARTS {
            tx.send(chunk.clone()).await?;
        }
        tx.send_eof()
    });
    client.started.notified().await;
    // Give the producer and upload loop opportunities to fill the limit while
    // the connector deliberately keeps all part requests pending.
    for _ in 0..1024 {
        tokio::task::yield_now().await;
    }
    let peak_while_blocked = client.peak.load(Ordering::SeqCst);
    client.gate.add_permits(PARTS);
    let written = upload.await.unwrap()?;
    send.await.unwrap()?;
    assert_eq!(written, (PARTS * PART_BYTES) as u64);
    assert_eq!(client.parts.load(Ordering::SeqCst), PARTS);
    assert!(peak_while_blocked > 0 && peak_while_blocked <= 2);
    assert!(client.peak.load(Ordering::SeqCst) <= 2);
    Ok(())
}
