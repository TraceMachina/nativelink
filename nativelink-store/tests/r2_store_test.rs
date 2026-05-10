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

//! `R2Store` only translates config; the S3 wire behaviour is covered by
//! `s3_store_test.rs`. These tests cover the translation only.

use nativelink_config::stores::{CommonObjectSpec, ExperimentalR2Spec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_store::r2_store::R2Store;
use pretty_assertions::assert_eq;

const ACCOUNT_ID: &str = "abcdef0123456789";
const BUCKET_NAME: &str = "dummy-bucket-name";

#[nativelink_test]
async fn endpoint_is_derived_from_account_id() -> Result<(), Error> {
    let aws_spec = R2Store::to_aws_spec(&ExperimentalR2Spec {
        account_id: ACCOUNT_ID.to_string(),
        bucket: BUCKET_NAME.to_string(),
        ..Default::default()
    });
    assert_eq!(
        aws_spec.endpoint.as_deref(),
        Some("https://abcdef0123456789.r2.cloudflarestorage.com"),
    );
    Ok(())
}

#[nativelink_test]
async fn region_is_auto() -> Result<(), Error> {
    let aws_spec = R2Store::to_aws_spec(&ExperimentalR2Spec {
        account_id: ACCOUNT_ID.to_string(),
        bucket: BUCKET_NAME.to_string(),
        ..Default::default()
    });
    assert_eq!(aws_spec.region, "auto");
    Ok(())
}

#[nativelink_test]
async fn bucket_is_passed_through() -> Result<(), Error> {
    let aws_spec = R2Store::to_aws_spec(&ExperimentalR2Spec {
        account_id: ACCOUNT_ID.to_string(),
        bucket: BUCKET_NAME.to_string(),
        ..Default::default()
    });
    assert_eq!(aws_spec.bucket, BUCKET_NAME);
    Ok(())
}

#[nativelink_test]
async fn explicit_credentials_are_passed_through() -> Result<(), Error> {
    let aws_spec = R2Store::to_aws_spec(&ExperimentalR2Spec {
        account_id: ACCOUNT_ID.to_string(),
        bucket: BUCKET_NAME.to_string(),
        access_key_id: Some("explicit-key".to_string()),
        secret_access_key: Some("explicit-secret".to_string()),
        ..Default::default()
    });
    assert_eq!(aws_spec.access_key_id.as_deref(), Some("explicit-key"));
    assert_eq!(
        aws_spec.secret_access_key.as_deref(),
        Some("explicit-secret"),
    );
    Ok(())
}

#[nativelink_test]
async fn missing_credentials_produce_none() -> Result<(), Error> {
    let aws_spec = R2Store::to_aws_spec(&ExperimentalR2Spec {
        account_id: ACCOUNT_ID.to_string(),
        bucket: BUCKET_NAME.to_string(),
        ..Default::default()
    });
    assert_eq!(aws_spec.access_key_id, None);
    assert_eq!(aws_spec.secret_access_key, None);
    Ok(())
}

#[nativelink_test]
async fn common_settings_are_passed_through() -> Result<(), Error> {
    let aws_spec = R2Store::to_aws_spec(&ExperimentalR2Spec {
        account_id: ACCOUNT_ID.to_string(),
        bucket: BUCKET_NAME.to_string(),
        common: CommonObjectSpec {
            key_prefix: Some("cas/".to_string()),
            consider_expired_after_s: 7 * 24 * 60 * 60,
            ..Default::default()
        },
        ..Default::default()
    });
    assert_eq!(aws_spec.common.key_prefix.as_deref(), Some("cas/"));
    assert_eq!(aws_spec.common.consider_expired_after_s, 7 * 24 * 60 * 60);
    Ok(())
}
