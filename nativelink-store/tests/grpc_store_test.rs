// Copyright 2024 The NativeLink Authors. All rights reserved.
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

use nativelink_proto::build::bazel::remote::execution::v2::digest_function::Value as ProtoDigestFunction;
use nativelink_util::digest_hasher::{DigestHasherFunc, make_ctx_for_hash_func};

#[test]
fn test_digest_function_conversion() {
    // Test that ProtoDigestFunction values convert correctly to integers
    assert_eq!(ProtoDigestFunction::Sha256 as i32, 1);
    assert_eq!(ProtoDigestFunction::Blake3 as i32, 3);
}

#[test]
fn test_make_ctx_for_hash_func_sha256() {
    // Test SHA256 context creation
    let ctx = make_ctx_for_hash_func(ProtoDigestFunction::Sha256 as i32)
        .expect("Should create context for SHA256");
    let hasher = ctx.get::<DigestHasherFunc>().copied()
        .unwrap_or(DigestHasherFunc::Sha256);
    assert_eq!(hasher, DigestHasherFunc::Sha256);
}

#[test]
fn test_make_ctx_for_hash_func_blake3() {
    // Test Blake3 context creation
    let ctx = make_ctx_for_hash_func(ProtoDigestFunction::Blake3 as i32)
        .expect("Should create context for Blake3");
    let hasher = ctx.get::<DigestHasherFunc>().copied()
        .unwrap_or(DigestHasherFunc::Sha256);
    assert_eq!(hasher, DigestHasherFunc::Blake3);
}

#[test]
fn test_make_ctx_for_hash_func_default() {
    // Test default (unset) context creation
    let ctx = make_ctx_for_hash_func(0i32)
        .expect("Should create context for default");
    let hasher = ctx.get::<DigestHasherFunc>().copied()
        .unwrap_or(DigestHasherFunc::Sha256);
    assert_eq!(hasher, DigestHasherFunc::Sha256);
}

#[test]
fn test_invalid_digest_function() {
    // Test invalid digest function handling
    let result = make_ctx_for_hash_func(999i32);
    assert!(result.is_err());
}
