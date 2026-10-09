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

//! ONTAP-specific wire behavior of `OntapS3Store`. Generic S3 behavior is
//! covered by `s3_store_test.rs`.

use std::sync::Arc;

use aws_sdk_s3::config::Credentials;
use aws_smithy_http_client::test_util::{ReplayEvent, StaticReplayClient};
use aws_smithy_types::body::SdkBody;
use http::{StatusCode, header};
use nativelink_config::stores::{CommonObjectSpec, ExperimentalOntapS3Spec};
use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_store::ontap_s3_store::OntapS3Store;
use nativelink_store::s3_store::S3Store;
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::store_trait::StoreLike;
use pretty_assertions::assert_eq;

const ENDPOINT: &str = "https://ontap.example.com";
const VSERVER_NAME: &str = "testvserver";
const BUCKET: &str = "test-bucket";
const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

// Coerces the function item to a plain fn pointer matching the store type.
const NOW_FN: fn() -> MockInstantWrapped = MockInstantWrapped::default;

#[nativelink_test]
async fn aws_spec_passes_vserver_bucket_and_common_through() -> Result<(), Error> {
    let aws_spec = OntapS3Store::build_aws_spec(&ExperimentalOntapS3Spec {
        endpoint: ENDPOINT.to_string(),
        vserver_name: VSERVER_NAME.to_string(),
        bucket: BUCKET.to_string(),
        common: CommonObjectSpec {
            key_prefix: Some("cas/".to_string()),
            consider_expired_after_s: 86400,
            ..Default::default()
        },
        ..Default::default()
    });
    // ONTAP signs with the vserver name as the region.
    assert_eq!(aws_spec.region, VSERVER_NAME);
    assert_eq!(aws_spec.bucket, BUCKET);
    assert_eq!(aws_spec.common.key_prefix.as_deref(), Some("cas/"));
    assert_eq!(aws_spec.common.consider_expired_after_s, 86400);
    Ok(())
}

fn mock_spec() -> ExperimentalOntapS3Spec {
    ExperimentalOntapS3Spec {
        endpoint: ENDPOINT.to_string(),
        vserver_name: VSERVER_NAME.to_string(),
        bucket: BUCKET.to_string(),
        ..Default::default()
    }
}

type TestStore = S3Store<fn() -> MockInstantWrapped>;

fn store_with(mock: StaticReplayClient) -> Result<Arc<TestStore>, Error> {
    let spec = mock_spec();
    let credentials = Credentials::new("AKIDTEST", "SECRETTEST", None, None, "test");
    let s3_client = OntapS3Store::make_client_with(&spec, mock, credentials);
    OntapS3Store::new_with_client(&spec, s3_client, NOW_FN)
}

fn ok_response() -> http::Response<SdkBody> {
    http::Response::builder()
        .status(StatusCode::OK)
        .body(SdkBody::empty())
        .unwrap()
}

/// No `aws-chunked` encoding and no checksum headers.
fn assert_plain_upload(req: &aws_smithy_runtime_api::http::Request) {
    let sha = req
        .headers()
        .get("x-amz-content-sha256")
        .unwrap_or_default();
    assert!(
        !sha.contains("STREAMING"),
        "upload must not use aws-chunked payload signing, got x-amz-content-sha256={sha}"
    );
    assert_ne!(
        req.headers().get("content-encoding"),
        Some("aws-chunked"),
        "upload must not set Content-Encoding: aws-chunked"
    );
    for (name, value) in req.headers() {
        assert!(
            !name.starts_with("x-amz-checksum-") && name != "x-amz-sdk-checksum-algorithm",
            "upload must not carry checksum headers, got {name}: {value}"
        );
    }
}

#[nativelink_test]
async fn has_object_found_uses_path_style_endpoint() -> Result<(), Error> {
    const SIZE: u64 = 512;
    let mock = StaticReplayClient::new(vec![ReplayEvent::new(
        http::Request::builder()
            .method("HEAD")
            .uri(format!("{ENDPOINT}/{BUCKET}/{VALID_HASH1}-{SIZE}"))
            .body(SdkBody::empty())
            .unwrap(),
        http::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_LENGTH, SIZE.to_string())
            .body(SdkBody::empty())
            .unwrap(),
    )]);
    let store = store_with(mock.clone())?;

    let result = store.has(DigestInfo::try_new(VALID_HASH1, SIZE)?).await?;
    assert_eq!(result, Some(SIZE), "expected object to be found");
    mock.assert_requests_match(&[]);
    Ok(())
}

#[nativelink_test]
async fn requests_are_signed_for_the_vserver() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![ReplayEvent::new(
        http::Request::builder().body(SdkBody::empty()).unwrap(),
        http::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(SdkBody::empty())
            .unwrap(),
    )]);
    let store = store_with(mock.clone())?;

    let result = store.has(DigestInfo::try_new(VALID_HASH1, 100)?).await?;
    assert_eq!(result, None, "expected absent object to map to None");

    let reqs: Vec<_> = mock.actual_requests().collect();
    assert_eq!(reqs.len(), 1, "expected exactly one HeadObject request");
    let authorization = reqs[0].headers().get("authorization").unwrap_or_default();
    assert!(
        authorization.contains(&format!("/{VSERVER_NAME}/s3/aws4_request")),
        "expected the SigV4 scope to use the vserver as region, got {authorization}"
    );
    Ok(())
}

#[nativelink_test]
async fn get_missing_object_maps_to_not_found() -> Result<(), Error> {
    const NO_SUCH_KEY: &str = concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>",
        "<Error><Code>NoSuchKey</Code>",
        "<Message>The specified key does not exist.</Message></Error>"
    );
    let mock = StaticReplayClient::new(vec![ReplayEvent::new(
        http::Request::builder().body(SdkBody::empty()).unwrap(),
        http::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(SdkBody::from(NO_SUCH_KEY))
            .unwrap(),
    )]);
    let store = store_with(mock)?;

    let err = store
        .get_part_unchunked(DigestInfo::try_new(VALID_HASH1, 100)?, 0, None)
        .await
        .expect_err("get on a missing object must error");
    assert_eq!(
        err.code,
        Code::NotFound,
        "expected NotFound code for an absent object"
    );
    Ok(())
}

#[nativelink_test]
async fn ranged_get_sends_range_header_path_style() -> Result<(), Error> {
    const VALUE: &str = "ontap-object-contents";
    const OFFSET: u64 = 105;
    const LENGTH: u64 = 50_000;
    let mock = StaticReplayClient::new(vec![ReplayEvent::new(
        http::Request::builder()
            .uri(format!(
                "{ENDPOINT}/{BUCKET}/{VALID_HASH1}-1000?x-id=GetObject"
            ))
            .header("range", format!("bytes={OFFSET}-{}", OFFSET + LENGTH))
            .body(SdkBody::empty())
            .unwrap(),
        http::Response::builder()
            .status(StatusCode::OK)
            .body(SdkBody::from(VALUE))
            .unwrap(),
    )]);
    let store = store_with(mock.clone())?;

    let got = store
        .get_part_unchunked(
            DigestInfo::try_new(VALID_HASH1, 1000)?,
            OFFSET,
            Some(LENGTH),
        )
        .await?;
    assert_eq!(got, VALUE.as_bytes());
    mock.assert_requests_match(&[]);
    Ok(())
}

// Regression guard for #2767 (unpadded `x-amz-checksum-sha256`).
#[nativelink_test]
async fn update_single_put_is_path_style_and_unchunked() -> Result<(), Error> {
    const DATA: &[u8] = b"hello-ontap-single-put-body";
    let mock = StaticReplayClient::new(vec![ReplayEvent::new(
        http::Request::builder().body(SdkBody::empty()).unwrap(),
        ok_response(),
    )]);
    let store = store_with(mock.clone())?;

    store
        .update_oneshot(
            DigestInfo::try_new(VALID_HASH1, DATA.len() as u64)?,
            DATA.into(),
        )
        .await?;

    let reqs: Vec<_> = mock.actual_requests().collect();
    assert_eq!(reqs.len(), 1, "expected exactly one PutObject request");
    let req = reqs[0];
    assert_eq!(req.method(), "PUT", "single-shot upload should be a PUT");
    let want_prefix = format!("{ENDPOINT}/{BUCKET}/{VALID_HASH1}-{}", DATA.len());
    assert!(
        req.uri().starts_with(&want_prefix),
        "expected path-style URL starting with {want_prefix}, got {}",
        req.uri()
    );
    assert_eq!(
        req.headers().get("x-amz-content-sha256"),
        Some("UNSIGNED-PAYLOAD"),
    );
    assert_plain_upload(req);
    Ok(())
}

#[nativelink_test]
async fn update_multipart_parts_are_path_style_and_unchunked() -> Result<(), Error> {
    const MIN_MULTIPART_SIZE: usize = 5 * 1024 * 1024; // 5MB.
    const DATA_SIZE: usize = MIN_MULTIPART_SIZE * 2;
    let data = vec![7u8; DATA_SIZE];

    let mock = StaticReplayClient::new(vec![
        ReplayEvent::new(
            http::Request::builder().body(SdkBody::empty()).unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .body(SdkBody::from(concat!(
                    "<InitiateMultipartUploadResult>",
                    "<UploadId>Dummy-uploadid</UploadId>",
                    "</InitiateMultipartUploadResult>"
                )))
                .unwrap(),
        ),
        ReplayEvent::new(
            http::Request::builder().body(SdkBody::empty()).unwrap(),
            ok_response(),
        ),
        ReplayEvent::new(
            http::Request::builder().body(SdkBody::empty()).unwrap(),
            ok_response(),
        ),
        ReplayEvent::new(
            http::Request::builder().body(SdkBody::empty()).unwrap(),
            http::Response::builder()
                .status(StatusCode::OK)
                .body(SdkBody::from(
                    "<CompleteMultipartUploadResult></CompleteMultipartUploadResult>",
                ))
                .unwrap(),
        ),
    ]);
    let store = store_with(mock.clone())?;

    store
        .update_oneshot(
            DigestInfo::try_new(VALID_HASH1, DATA_SIZE as u64)?,
            data.into(),
        )
        .await?;

    let reqs: Vec<_> = mock.actual_requests().collect();
    assert_eq!(reqs.len(), 4, "expected create, two parts and complete");
    let object_url = format!("{ENDPOINT}/{BUCKET}/{VALID_HASH1}-{DATA_SIZE}");
    for req in &reqs {
        assert!(
            req.uri().starts_with(&object_url),
            "expected path-style URL starting with {object_url}, got {}",
            req.uri()
        );
    }
    let parts: Vec<_> = reqs
        .iter()
        .filter(|req| req.uri().contains("x-id=UploadPart"))
        .collect();
    assert_eq!(parts.len(), 2, "expected two UploadPart requests");
    for part in parts {
        assert_plain_upload(part);
    }
    Ok(())
}
