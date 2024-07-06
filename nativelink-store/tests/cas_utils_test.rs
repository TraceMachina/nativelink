// Copyright 2023 The NativeLink Authors. All rights reserved.
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

use blake3::Hasher as Blake3;
use nativelink_store::cas_utils::is_zero_digest;
use nativelink_util::common::DigestInfo;
use sha2::{Digest, Sha256};

#[test]
fn sha256_is_zero_digest() {
    let digest = DigestInfo {
        packed_hash: Sha256::new().finalize().into(),
        size_bytes: 0,
    };
    assert!(is_zero_digest(digest));
}

#[test]
fn sha256_is_non_zero_digest() {
    let mut hasher = Sha256::new();
    hasher.update(b"a");
    let digest = DigestInfo {
        packed_hash: hasher.finalize().into(),
        size_bytes: 1,
    };
    assert!(!is_zero_digest(digest));
}

#[test]
fn blake_is_zero_digest() {
    let digest = DigestInfo {
        packed_hash: Blake3::new().finalize().into(),
        size_bytes: 0,
    };
    assert!(is_zero_digest(digest));
}

#[test]
fn blake_is_non_zero_digest() {
    let mut hasher = Blake3::new();
    hasher.update(b"a");
    let digest = DigestInfo {
        packed_hash: hasher.finalize().into(),
        size_bytes: 1,
    };
    assert!(!is_zero_digest(digest));
}
