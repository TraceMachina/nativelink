// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::fmt;
use std::future::Future;
use std::hash::Hash;
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{BufMut, Bytes, BytesMut};
pub use fs;
use hex::FromHex;
pub use log;
use prost::Message;
use proto::build::bazel::remote::execution::v2::Digest;
use serde::{Deserialize, Serialize};
use tokio::task::{JoinError, JoinHandle};

use error::{make_input_err, Error, ResultExt};

#[derive(Serialize, Deserialize, Default, Clone, Copy, Eq, PartialEq, Hash)]
#[repr(C)]
pub struct DigestInfo {
    /// Raw hash in packed form.
    pub packed_hash: [u8; 32],

    /// Possibly the size of the digest in bytes.
    pub size_bytes: i64,
}

impl DigestInfo {
    pub fn new(packed_hash: [u8; 32], size_bytes: i64) -> Self {
        DigestInfo {
            size_bytes,
            packed_hash,
        }
    }

    pub fn try_new<T>(hash: &str, size_bytes: T) -> Result<Self, Error>
    where
        T: TryInto<i64> + std::fmt::Display + Copy,
    {
        let packed_hash = <[u8; 32]>::from_hex(hash).err_tip(|| format!("Invalid sha256 hash: {}", hash))?;
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

    pub const fn empty_digest() -> DigestInfo {
        DigestInfo {
            size_bytes: 0,
            // Magic hash of a sha256 of empty string.
            packed_hash: [
                0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f, 0xb9, 0x24, 0x27,
                0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b, 0x78, 0x52, 0xb8, 0x55,
            ],
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

impl TryFrom<Digest> for DigestInfo {
    type Error = Error;

    fn try_from(digest: Digest) -> Result<Self, Self::Error> {
        let packed_hash =
            <[u8; 32]>::from_hex(&digest.hash).err_tip(|| format!("Invalid sha256 hash: {}", digest.hash))?;
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

/// Simple wrapper that will abort a future that is running in another spawn in the
/// event that this handle gets dropped.
pub struct JoinHandleDropGuard<T> {
    inner: JoinHandle<T>,
}

impl<T> JoinHandleDropGuard<T> {
    pub fn new(inner: JoinHandle<T>) -> Self {
        Self { inner }
    }
}

impl<T> Future for JoinHandleDropGuard<T> {
    type Output = Result<T, JoinError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.inner).poll(cx)
    }
}

impl<T> Drop for JoinHandleDropGuard<T> {
    fn drop(&mut self) {
        self.inner.abort();
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
