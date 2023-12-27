// Copyright 2023 The Native Link Authors. All rights reserved.
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

use std::sync::OnceLock;

use blake3::Hasher as Blake3Hasher;
use nativelink_config::stores::ConfigDigestHashFunction;
use nativelink_error::{make_err, make_input_err, Code, Error};
use nativelink_proto::build::bazel::remote::execution::v2::digest_function::Value as ProtoDigestFunction;
use sha2::{Digest, Sha256};

use crate::common::DigestInfo;

static DEFAULT_DIGEST_HASHER_FUNC: OnceLock<DigestHasherFunc> = OnceLock::new();

/// Get the default hasher.
pub fn default_digest_hasher_func() -> DigestHasherFunc {
    *DEFAULT_DIGEST_HASHER_FUNC.get_or_init(|| DigestHasherFunc::Sha256)
}

/// Sets the default hasher to use if no hasher was requested by the client.
pub fn set_default_digest_hasher_func(hasher: DigestHasherFunc) -> Result<(), Error> {
    DEFAULT_DIGEST_HASHER_FUNC
        .set(hasher)
        .map_err(|_| make_err!(Code::Internal, "default_digest_hasher_func already set"))
}

/// Supported digest hash functions.
#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
pub enum DigestHasherFunc {
    Sha256,
    Blake3,
}

impl DigestHasherFunc {
    #[must_use]
    pub const fn proto_digest_func(&self) -> ProtoDigestFunction {
        match self {
            Self::Sha256 => ProtoDigestFunction::Sha256,
            Self::Blake3 => ProtoDigestFunction::Blake3,
        }
    }
}

impl From<ConfigDigestHashFunction> for DigestHasherFunc {
    fn from(value: ConfigDigestHashFunction) -> Self {
        match value {
            ConfigDigestHashFunction::sha256 => Self::Sha256,
            ConfigDigestHashFunction::blake3 => Self::Blake3,
        }
    }
}

impl TryFrom<ProtoDigestFunction> for DigestHasherFunc {
    type Error = Error;

    fn try_from(value: ProtoDigestFunction) -> Result<Self, Self::Error> {
        match value {
            ProtoDigestFunction::Sha256 => Ok(Self::Sha256),
            ProtoDigestFunction::Blake3 => Ok(Self::Blake3),
            v => Err(make_input_err!("Unknown or unsupported digest function {v:?}")),
        }
    }
}

impl TryFrom<i32> for DigestHasherFunc {
    type Error = Error;

    fn try_from(value: i32) -> Result<Self, Self::Error> {
        match ProtoDigestFunction::try_from(value) {
            // Note: Unknown represents 0, which means non-set, so use default.
            Ok(ProtoDigestFunction::Unknown) => Ok(default_digest_hasher_func()),
            Ok(ProtoDigestFunction::Sha256) => Ok(Self::Sha256),
            Ok(ProtoDigestFunction::Blake3) => Ok(Self::Blake3),
            value => Err(make_input_err!(
                "Unknown or unsupported digest function: {:?}",
                value.map(|v| v.as_str_name())
            )),
        }
    }
}

impl From<DigestHasherFunc> for DigestHasher {
    fn from(value: DigestHasherFunc) -> Self {
        match value {
            DigestHasherFunc::Sha256 => Self::Sha256(Sha256::new()),
            DigestHasherFunc::Blake3 => Self::Blake3(Box::new(Blake3Hasher::new())),
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
            Self::Sha256(h) => sha2::digest::Update::update(h, input),
            Self::Blake3(h) => {
                Blake3Hasher::update(h, input);
            }
        }
    }

    /// Finalize the hash function and collect the results into a digest.
    #[inline]
    pub fn finalize_digest(&mut self, size: impl Into<i64>) -> DigestInfo {
        let hash = match self {
            Self::Sha256(h) => h.finalize_reset().into(),
            Self::Blake3(h) => h.finalize().into(),
        };
        DigestInfo::new(hash, size.into())
    }
}
