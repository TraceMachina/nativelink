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

use nativelink_store::grpc_store::GrpcStore;
use nativelink_util::resource_info::{ResourceInfo, is_supported_digest_function};
use opentelemetry::context::Context;

#[test]
fn test_is_supported_digest_function() {
    assert!(is_supported_digest_function("sha256"));
    assert!(is_supported_digest_function("sha512"));
    assert!(!is_supported_digest_function("crc32"));
}

#[test]
fn test_read_rejects_invalid_digest_function() -> Result<(), Box<dyn core::error::Error>> {
    const RESOURCE_NAME: &str = "instance_name/blobs/sha256/0123456789abcdef0123456789abcdef/123";
    let mut resource_info = ResourceInfo::new(RESOURCE_NAME, false)?;
    resource_info.digest_function = Some("sha3".into());
    let digest_func = resource_info.digest_function.clone().unwrap();

    let result = GrpcStore::validate_digest_function(&digest_func, Some(RESOURCE_NAME));
    assert!(result.is_err(), "Expected error on invalid digest_function");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Unsupported digest_function"),
        "Unexpected error: {}",
        msg
    );
    Ok(())
}

#[test]
fn test_has_with_results_rejects_invalid_digest_function_in_context() {
    let ctx = Context::current().with_value("sha3_256".to_string());
    let _guard = ctx.attach();

    let result = GrpcStore::validate_digest_function("sha3_256", None);
    assert!(result.is_err(), "Expected error from context digest check");
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("Unsupported digest_function"),
        "Unexpected error: {}",
        msg
    );
}
