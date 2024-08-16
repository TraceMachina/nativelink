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

use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;

use bytes::{BufMut, Bytes, BytesMut};
use hex::FromHex;
use nativelink_error::{make_input_err, Error, ResultExt};
use nativelink_metric::{
    MetricFieldData, MetricKind, MetricPublishKnownKindData, MetricsComponent,
};
use nativelink_proto::build::bazel::remote::execution::v2::Digest;
use prost::Message;
use serde::{Deserialize, Serialize};

pub use crate::fs;

#[derive(Serialize, Deserialize, Default, Clone, Copy, Eq, PartialEq, Hash)]
#[repr(C)]
pub struct DigestInfo {
    /// Raw hash in packed form.
    pub packed_hash: [u8; 32],

    /// Possibly the size of the digest in bytes.
    pub size_bytes: i64,
}

impl MetricsComponent for DigestInfo {
    fn publish(
        &self,
        _kind: MetricKind,
        field_metadata: MetricFieldData,
    ) -> Result<MetricPublishKnownKindData, nativelink_metric::Error> {
        format!("{}-{}", self.hash_str(), self.size_bytes)
            .publish(MetricKind::String, field_metadata)
    }
}

impl DigestInfo {
    pub const fn new(packed_hash: [u8; 32], size_bytes: i64) -> Self {
        DigestInfo {
            size_bytes,
            packed_hash,
        }
    }

    pub fn try_new<T>(hash: &str, size_bytes: T) -> Result<Self, Error>
    where
        T: TryInto<i64> + std::fmt::Display + Copy,
    {
        let packed_hash =
            <[u8; 32]>::from_hex(hash).err_tip(|| format!("Invalid sha256 hash: {hash}"))?;
        let size_bytes = size_bytes
            .try_into()
            .map_err(|_| make_input_err!("Could not convert {} into i64", size_bytes))?;
        Ok(DigestInfo {
            size_bytes,
            packed_hash,
        })
    }

    pub fn hash_str(&self) -> String {
        hex::encode(self.packed_hash)
    }

    pub const fn zero_digest() -> DigestInfo {
        DigestInfo {
            size_bytes: 0,
            // Magic hash of a sha256 of empty string.
            packed_hash: [0u8; 32],
        }
    }
}

impl fmt::Debug for DigestInfo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DigestInfo")
            .field("size_bytes", &self.size_bytes)
            .field("hash", &self.hash_str())
            .finish()
    }
}

impl Ord for DigestInfo {
    fn cmp(&self, other: &Self) -> Ordering {
        self.packed_hash
            .cmp(&other.packed_hash)
            .then_with(|| self.size_bytes.cmp(&other.size_bytes))
    }
}

impl PartialOrd for DigestInfo {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl TryFrom<Digest> for DigestInfo {
    type Error = Error;

    fn try_from(digest: Digest) -> Result<Self, Self::Error> {
        let packed_hash = <[u8; 32]>::from_hex(&digest.hash)
            .err_tip(|| format!("Invalid sha256 hash: {}", digest.hash))?;
        Ok(DigestInfo {
            size_bytes: digest.size_bytes,
            packed_hash,
        })
    }
}

impl TryFrom<&Digest> for DigestInfo {
    type Error = Error;

    fn try_from(digest: &Digest) -> Result<Self, Self::Error> {
        let packed_hash = <[u8; 32]>::from_hex(&digest.hash)
            .err_tip(|| format!("Invalid sha256 hash: {}", digest.hash))?;
        Ok(DigestInfo {
            size_bytes: digest.size_bytes,
            packed_hash,
        })
    }
}

impl From<DigestInfo> for Digest {
    fn from(val: DigestInfo) -> Self {
        Digest {
            hash: val.hash_str(),
            size_bytes: val.size_bytes,
        }
    }
}

impl From<&DigestInfo> for Digest {
    fn from(val: &DigestInfo) -> Self {
        Digest {
            hash: val.hash_str(),
            size_bytes: val.size_bytes,
        }
    }
}

// Simple utility trait that makes it easier to apply `.try_map` to Vec.
// This will convert one vector into another vector with a different type.
pub trait VecExt<T> {
    fn try_map<F, U>(self, f: F) -> Result<Vec<U>, Error>
    where
        Self: Sized,
        F: (std::ops::Fn(T) -> Result<U, Error>) + Sized;
}

impl<T> VecExt<T> for Vec<T> {
    fn try_map<F, U>(self, f: F) -> Result<Vec<U>, Error>
    where
        Self: Sized,
        F: (std::ops::Fn(T) -> Result<U, Error>) + Sized,
    {
        let mut output = Vec::with_capacity(self.len());
        for item in self {
            output.push((f)(item)?);
        }
        Ok(output)
    }
}

// Simple utility trait that makes it easier to apply `.try_map` to HashMap.
// This will convert one HashMap into another keeping the key the same, but
// different value type.
pub trait HashMapExt<K: std::cmp::Eq + std::hash::Hash, T> {
    fn try_map<F, U>(self, f: F) -> Result<HashMap<K, U>, Error>
    where
        Self: Sized,
        F: (std::ops::Fn(T) -> Result<U, Error>) + Sized;
}

impl<K: std::cmp::Eq + std::hash::Hash, T> HashMapExt<K, T> for HashMap<K, T> {
    fn try_map<F, U>(self, f: F) -> Result<HashMap<K, U>, Error>
    where
        Self: Sized,
        F: (std::ops::Fn(T) -> Result<U, Error>) + Sized,
    {
        let mut output = HashMap::with_capacity(self.len());
        for (k, v) in self {
            output.insert(k, (f)(v)?);
        }
        Ok(output)
    }
}

// Utility to encode our proto into GRPC stream format.
pub fn encode_stream_proto<T: Message>(proto: &T) -> Result<Bytes, Box<dyn std::error::Error>> {
    let mut buf = BytesMut::new();
    // See below comment on spec.
    use std::mem::size_of;
    const PREFIX_BYTES: usize = size_of::<u8>() + size_of::<u32>();
    for _ in 0..PREFIX_BYTES {
        // Advance our buffer first.
        // We will backfill it once we know the size of the message.
        buf.put_u8(0);
    }
    proto.encode(&mut buf)?;
    let len = buf.len() - PREFIX_BYTES;
    {
        let mut buf = &mut buf[0..PREFIX_BYTES];
        // See: https://github.com/grpc/grpc/blob/master/doc/PROTOCOL-HTTP2.md#:~:text=Compressed-Flag
        // for more details on spec.
        // Compressed-Flag -> 0 / 1 # encoded as 1 byte unsigned integer.
        buf.put_u8(0);
        // Message-Length -> {length of Message} # encoded as 4 byte unsigned integer (big endian).
        buf.put_u32(len as u32);
        // Message -> *{binary octet}.
    }

    Ok(buf.freeze())
}
