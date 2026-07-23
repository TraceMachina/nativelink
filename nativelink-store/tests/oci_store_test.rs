// Copyright 2026 The NativeLink Authors. All rights reserved.
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

//! `OciStore` builds an SDK client and hands it to `S3Store`. These tests cover
//! the config -> client mapping and the OCI-specific wire behavior (path-style
//! URLs and non-`aws-chunked` uploads) using a `StaticReplayClient`, all without
//! a live bucket. The generic S3 wire behavior is covered by `s3_store_test.rs`;
//! live round-trip / multipart / load coverage is kept out of the repo.

use std::sync::Arc;

use aws_smithy_http_client::test_util::{ReplayEvent, StaticReplayClient};
use aws_smithy_types::body::SdkBody;
use http::{StatusCode, header};
use nativelink_config::stores::{CommonObjectSpec, ExperimentalOciSpec};
use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_store::oci_store::OciStore;
use nativelink_store::s3_store::S3Store;
use nativelink_util::common::DigestInfo;
use nativelink_util::instant_wrapper::MockInstantWrapped;
use nativelink_util::store_trait::StoreLike;
use pretty_assertions::assert_eq;

#[nativelink_test]
async fn endpoint_is_derived_from_namespace_and_region() -> Result<(), Error> {
    let endpoint = OciStore::derive_endpoint(&ExperimentalOciSpec {
        namespace: "axaxnpcrorw5".to_string(),
        region: "us-phoenix-1".to_string(),
        bucket: "ignored".to_string(),
        ..Default::default()
    });
    assert_eq!(
        endpoint,
        "https://axaxnpcrorw5.compat.objectstorage.us-phoenix-1.oci.customer-oci.com",
    );
    Ok(())
}

#[nativelink_test]
async fn aws_spec_passes_region_bucket_and_common_through() -> Result<(), Error> {
    let aws_spec = OciStore::build_aws_spec(&ExperimentalOciSpec {
        namespace: "ignored".to_string(),
        region: "us-ashburn-1".to_string(),
        bucket: "test-bucket".to_string(),
        common: CommonObjectSpec {
            key_prefix: Some("cas/".to_string()),
            consider_expired_after_s: 86400,
            ..Default::default()
        },
        ..Default::default()
    });
    // The OCI region is forwarded verbatim so it is used as the SigV4 signing
    // region (unlike R2, which always signs against `auto`).
    assert_eq!(aws_spec.region, "us-ashburn-1");
    assert_eq!(aws_spec.bucket, "test-bucket");
    assert_eq!(aws_spec.common.key_prefix.as_deref(), Some("cas/"));
    assert_eq!(aws_spec.common.consider_expired_after_s, 86400);
    Ok(())
}

// ---------------------------------------------------------------------------
// Mock wire-behavior tests. Drive the store through `new_with_http_client` with
// a `StaticReplayClient` to verify the OCI-specific request shaping without a
// live bucket.
// ---------------------------------------------------------------------------

const NAMESPACE: &str = "ns";
const REGION: &str = "us-region-1";
const BUCKET: &str = "test-bucket";
const HOST: &str = "ns.compat.objectstorage.us-region-1.oci.customer-oci.com";
const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

// Coerces the function item to a plain fn pointer matching the store type.
const NOW_FN: fn() -> MockInstantWrapped = MockInstantWrapped::default;

fn mock_spec() -> ExperimentalOciSpec {
    ExperimentalOciSpec {
        namespace: NAMESPACE.to_string(),
        region: REGION.to_string(),
        bucket: BUCKET.to_string(),
        access_key_id: Some("AKIDTEST".to_string()),
        secret_access_key: Some("SECRETTEST".to_string()),
        ..Default::default()
    }
}

async fn store_with(
    mock: StaticReplayClient,
) -> Result<Arc<S3Store<fn() -> MockInstantWrapped>>, Error> {
    OciStore::new_with_http_client(&mock_spec(), mock, NOW_FN).await
}

#[nativelink_test]
async fn has_object_found_uses_path_style_endpoint() -> Result<(), Error> {
    const SIZE: u64 = 512;
    let mock = StaticReplayClient::new(vec![ReplayEvent::new(
        http::Request::builder()
            .method("HEAD")
            .uri(format!("https://{HOST}/{BUCKET}/{VALID_HASH1}-{SIZE}"))
            .body(SdkBody::empty())
            .unwrap(),
        http::Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_LENGTH, SIZE.to_string())
            .body(SdkBody::empty())
            .unwrap(),
    )]);
    let store = store_with(mock.clone()).await?;

    let result = store.has(DigestInfo::try_new(VALID_HASH1, SIZE)?).await?;
    assert_eq!(result, Some(SIZE), "expected object to be found");
    mock.assert_requests_match(&[]);
    Ok(())
}

#[nativelink_test]
async fn has_object_not_found_returns_none() -> Result<(), Error> {
    let mock = StaticReplayClient::new(vec![ReplayEvent::new(
        http::Request::builder().body(SdkBody::empty()).unwrap(),
        http::Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(SdkBody::empty())
            .unwrap(),
    )]);
    let store = store_with(mock).await?;

    let result = store.has(DigestInfo::try_new(VALID_HASH1, 100)?).await?;
    assert_eq!(result, None, "expected absent object to map to None");
    Ok(())
}

#[nativelink_test]
async fn get_missing_object_maps_to_not_found() -> Result<(), Error> {
    // OCI (like S3) returns a NoSuchKey error document on a missing GET; the SDK
    // parses it and the store maps it to `Code::NotFound`.
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
    let store = store_with(mock).await?;

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
async fn update_single_put_is_path_style_and_unchunked() -> Result<(), Error> {
    const DATA: &[u8] = b"hello-oci-single-put-body";
    let mock = StaticReplayClient::new(vec![ReplayEvent::new(
        http::Request::builder().body(SdkBody::empty()).unwrap(),
        http::Response::builder()
            .status(StatusCode::OK)
            .body(SdkBody::empty())
            .unwrap(),
    )]);
    let store = store_with(mock.clone()).await?;

    store
        .update_oneshot(
            DigestInfo::try_new(VALID_HASH1, DATA.len() as u64)?,
            DATA.into(),
        )
        .await?;

    let reqs: Vec<_> = mock.actual_requests().collect();
    assert_eq!(reqs.len(), 1, "expected exactly one PutObject request");
    let req = &reqs[0];
    assert_eq!(req.method(), "PUT", "single-shot upload should be a PUT");
    let want_prefix = format!("https://{HOST}/{BUCKET}/{VALID_HASH1}-{}", DATA.len());
    assert!(
        req.uri().starts_with(&want_prefix),
        "expected path-style URL starting with {want_prefix}, got {}",
        req.uri()
    );
    let sha = req
        .headers()
        .get("x-amz-content-sha256")
        .unwrap_or_default();
    assert!(
        !sha.contains("STREAMING") && !sha.contains("CHUNKED"),
        "PutObject must not use aws-chunked payload signing, got x-amz-content-sha256={sha}"
    );
    assert_ne!(
        req.headers().get("content-encoding"),
        Some("aws-chunked"),
        "PutObject must not set Content-Encoding: aws-chunked"
    );
    Ok(())
}

#[nativelink_test]
async fn get_object_returns_bytes() -> Result<(), Error> {
    const VALUE: &str = "oci-object-contents";
    let mock = StaticReplayClient::new(vec![ReplayEvent::new(
        http::Request::builder()
            .uri(format!(
                "https://{HOST}/{BUCKET}/{VALID_HASH1}-1000?x-id=GetObject"
            ))
            .body(SdkBody::empty())
            .unwrap(),
        http::Response::builder()
            .status(StatusCode::OK)
            .body(SdkBody::from(VALUE))
            .unwrap(),
    )]);
    let store = store_with(mock.clone()).await?;

    let got = store
        .get_part_unchunked(DigestInfo::try_new(VALID_HASH1, 1000)?, 0, None)
        .await?;
    assert_eq!(got, VALUE.as_bytes());
    mock.assert_requests_match(&[]);
    Ok(())
}

#[nativelink_test]
async fn ranged_get_sends_range_header_path_style() -> Result<(), Error> {
    const OFFSET: usize = 105;
    const LENGTH: usize = 50_000;
    let mock = StaticReplayClient::new(vec![ReplayEvent::new(
        http::Request::builder()
            .uri(format!(
                "https://{HOST}/{BUCKET}/{VALID_HASH1}-1000?x-id=GetObject"
            ))
            .header("range", format!("bytes={OFFSET}-{}", OFFSET + LENGTH))
            .body(SdkBody::empty())
            .unwrap(),
        http::Response::builder()
            .status(StatusCode::OK)
            .body(SdkBody::empty())
            .unwrap(),
    )]);
    let store = store_with(mock.clone()).await?;

    store
        .get_part_unchunked(
            DigestInfo::try_new(VALID_HASH1, 1000)?,
            OFFSET as u64,
            Some(LENGTH as u64),
        )
        .await?;
    mock.assert_requests_match(&[]);
    Ok(())
}
