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

//! `OciStore` builds an SDK client and hands it to `S3Store`. The S3 wire
//! behaviour is covered by `s3_store_test.rs`.

use nativelink_config::stores::{CommonObjectSpec, ExperimentalOciSpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_store::oci_store::OciStore;
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
