// Copyright 2023 The Turbo Cache Authors. All rights reserved.
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

use blake3::Hasher as Blake3Hasher;
use sha2::{Digest, Sha256};

use common::DigestInfo;

/// Supported digest hash functions.
pub enum DigestHasherFunc {
    Sha256,
    Blake3,
}

impl From<DigestHasherFunc> for DigestHasher {
    fn from(value: DigestHasherFunc) -> Self {
        match value {
            DigestHasherFunc::Sha256 => DigestHasher::Sha256(Sha256::new()),
            DigestHasherFunc::Blake3 => DigestHasher::Blake3(Box::new(Blake3Hasher::new())),
        }
    }
}

/// The individual implementation of the hash function.
pub enum DigestHasher {
    Sha256(Sha256),
    Blake3(Box<Blake3Hasher>),
}

impl DigestHasher {
    /// Update the hasher with some additional data.
    #[inline]
    pub fn update(&mut self, input: &[u8]) {
        match self {
            DigestHasher::Sha256(h) => sha2::digest::Update::update(h, input),
            DigestHasher::Blake3(h) => {
                Blake3Hasher::update(h, input);
            }
        }
    }

    /// Finalize the hash function and collect the results into a digest.
    #[inline]
    pub fn finalize_digest(&mut self, size: impl Into<i64>) -> DigestInfo {
        let hash = match self {
            DigestHasher::Sha256(h) => h.finalize_reset().into(),
            DigestHasher::Blake3(h) => h.finalize().into(),
        };
        DigestInfo::new(hash, size.into())
    }
}
