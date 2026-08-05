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

//! `R2Store` builds an SDK client and hands it to `S3Store`. The S3 wire
//! behaviour is covered by `s3_store_test.rs`.

use nativelink_config::stores::{CommonObjectSpec, ExperimentalR2Spec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_store::r2_store::R2Store;
use pretty_assertions::assert_eq;

#[nativelink_test]
async fn endpoint_is_derived_from_account_id() -> Result<(), Error> {
    let endpoint = R2Store::derive_endpoint(&ExperimentalR2Spec {
        account_id: "abcdef0123456789".to_string(),
        bucket: "ignored".to_string(),
        ..Default::default()
    });
    assert_eq!(
        endpoint,
        "https://abcdef0123456789.r2.cloudflarestorage.com",
    );
    Ok(())
}

#[nativelink_test]
async fn aws_spec_passes_bucket_and_common_through() -> Result<(), Error> {
    let aws_spec = R2Store::build_aws_spec(&ExperimentalR2Spec {
        account_id: "ignored".to_string(),
        bucket: "test-bucket".to_string(),
        common: CommonObjectSpec {
            key_prefix: Some("cas/".to_string()),
            consider_expired_after_s: 86400,
            ..Default::default()
        },
        ..Default::default()
    });
    assert_eq!(aws_spec.region, "auto");
    assert_eq!(aws_spec.bucket, "test-bucket");
    assert_eq!(aws_spec.common.key_prefix.as_deref(), Some("cas/"));
    assert_eq!(aws_spec.common.consider_expired_after_s, 86400);
    Ok(())
}
