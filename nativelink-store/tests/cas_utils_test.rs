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

use blake3::Hasher as Blake3;
use nativelink_store::cas_utils::is_zero_digest;
use nativelink_util::common::DigestInfo;
use sha2::{Digest, Sha256};

#[test]
fn sha256_is_zero_digest() {
    let digest = DigestInfo::new(Sha256::new().finalize().into(), 0);
    assert!(is_zero_digest(digest));
}

#[test]
fn sha256_is_non_zero_digest() {
    let mut hasher = Sha256::new();
    hasher.update(b"a");
    let digest = DigestInfo::new(hasher.finalize().into(), 1);
    assert!(!is_zero_digest(digest));
}

#[test]
fn blake_is_zero_digest() {
    let digest = DigestInfo::new(Blake3::new().finalize().into(), 0);
    assert!(is_zero_digest(digest));
}

#[test]
fn blake_is_non_zero_digest() {
    let mut hasher = Blake3::new();
    hasher.update(b"a");
    let digest = DigestInfo::new(hasher.finalize().into(), 1);
    assert!(!is_zero_digest(digest));
}
