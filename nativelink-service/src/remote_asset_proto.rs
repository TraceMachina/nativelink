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

use bytes::Bytes;
use nativelink_proto::build::bazel::remote::asset::v1::Qualifier;
use nativelink_proto::build::bazel::remote::execution::v2::Digest;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use prost::Message;
use prost_types::Timestamp;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteAssetQuery {
    pub uri: String,
    pub qualifiers: Vec<Qualifier>,
}

impl RemoteAssetQuery {
    pub const fn new(uri: String, qualifiers: Vec<Qualifier>) -> Self {
        Self { uri, qualifiers }
    }

    pub fn digest(&self) -> DigestInfo {
        let mut hasher = DigestHasherFunc::Blake3.hasher();
        hasher.update(self.uri.as_bytes());
        for qualifier in &self.qualifiers {
            hasher.update(qualifier.name.as_bytes());
            hasher.update(qualifier.value.as_bytes());
        }
        hasher.finalize_digest()
    }
}

#[derive(Clone, PartialEq, Eq, prost::Message)]
pub struct RemoteAssetArtifact {
    #[prost(string, tag = "1")]
    pub uri: String,

    #[prost(message, repeated, tag = "2")]
    pub qualifiers: Vec<Qualifier>,

    #[prost(message, tag = "3")]
    pub blob_digest: Option<Digest>,

    #[prost(message, optional, tag = "4")]
    pub expire_at: Option<Timestamp>,

    #[prost(
        enumeration = "nativelink_proto::build::bazel::remote::execution::v2::digest_function::Value",
        tag = "5"
    )]
    pub digest_function: i32,
}

impl RemoteAssetArtifact {
    pub const fn new(
        uri: String,
        qualifiers: Vec<Qualifier>,
        blob_digest: Digest,
        expire_at: Option<Timestamp>,
        digest_function: i32,
    ) -> Self {
        Self {
            uri,
            qualifiers,
            blob_digest: Some(blob_digest),
            expire_at,
            digest_function,
        }
    }

    pub fn digest(&self) -> DigestInfo {
        let mut hasher = DigestHasherFunc::Blake3.hasher();
        hasher.update(self.uri.as_bytes());
        for qualifier in &self.qualifiers {
            hasher.update(qualifier.name.as_bytes());
            hasher.update(qualifier.value.as_bytes());
        }
        hasher.finalize_digest()
    }

    pub fn as_bytes(&self) -> Bytes {
        let encoded_asset = self.encode_to_vec();
        Bytes::from_owner(encoded_asset)
    }
}
