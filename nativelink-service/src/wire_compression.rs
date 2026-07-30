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

//! Wire compression utilities for REAPI compressed-blobs.
//!
//! This module handles compression/decompression of blob data on the gRPC wire
//! between client and server, per the REAPI compressed-blobs specification.
//! This is orthogonal to at-rest compression (`CompressionStore` with LZ4).

use std::collections::HashSet;

use bytes::Bytes;
use nativelink_config::cas_server::{CapabilitiesConfig, InstanceName, WithInstanceName};
use nativelink_error::{Code, Error, make_err, make_input_err};
use nativelink_proto::build::bazel::remote::execution::v2::compressor;
use nativelink_util::spawn_blocking;
// The codecs are shared with client-side GrpcStore transfers; re-export so
// existing service callers keep their import paths.
pub use nativelink_util::wire_compression::{
    ZSTD_COMPRESSION_LEVEL, compress, decompress, stream_decode_compressed_upload,
    stream_encode_compressed_download,
};
use tracing::warn;

/// Which instances accept and advertise REAPI compressed-blobs (zstd).
///
/// Derived from `CapabilitiesConfig.remote_cache_compression` so that
/// advertisement (capabilities) and acceptance (ByteStream/CAS) cannot drift.
#[derive(Debug, Clone, Default)]
pub struct RemoteCacheCompressionInstances(HashSet<InstanceName>);

impl RemoteCacheCompressionInstances {
    #[must_use]
    pub fn from_capabilities_configs(configs: &[WithInstanceName<CapabilitiesConfig>]) -> Self {
        Self(
            configs
                .iter()
                .filter(|config| config.remote_cache_compression)
                .map(|config| config.instance_name.clone())
                .collect(),
        )
    }

    #[must_use]
    pub fn from_enabled_instance_names(
        instance_names: impl IntoIterator<Item = InstanceName>,
    ) -> Self {
        Self(instance_names.into_iter().collect())
    }

    #[must_use]
    pub fn enabled_for(&self, instance_name: &str) -> bool {
        self.0.contains(instance_name)
    }
}

/// Resolve a wire compressor from a URI compressor string (as it appears in
/// the `compressed-blobs/{compressor}/...` resource name) and validate it
/// against whether the instance supports remote cache compression.
///
/// Returns `compressor::Value::Identity` when `compressor_str` is `None` or
/// `"identity"`. This is the single source of truth for URI-to-compressor
/// parsing so that `ByteStream` and any other callers stay consistent.
pub fn resolve_wire_compressor(
    compressor_str: Option<&str>,
    remote_cache_compression_enabled: bool,
) -> Result<compressor::Value, Error> {
    match compressor_str {
        None | Some("identity") => Ok(compressor::Value::Identity),
        Some("zstd") => {
            if remote_cache_compression_enabled {
                Ok(compressor::Value::Zstd)
            } else {
                Err(make_input_err!(
                    "Remote cache compression is not supported by this instance"
                ))
            }
        }
        Some(other) => Err(make_input_err!("Unsupported wire compressor: '{}'", other)),
    }
}

#[must_use]
pub fn compress_for_batch_read(data: Bytes) -> (Bytes, compressor::Value) {
    match compress(data.clone(), compressor::Value::Zstd) {
        Ok(compressed) => {
            if compressed.len() < data.len() {
                (compressed, compressor::Value::Zstd)
            } else {
                (data, compressor::Value::Identity)
            }
        }
        Err(err) => {
            warn!("Wire compression failed, falling back to identity: {}", err);
            (data, compressor::Value::Identity)
        }
    }
}

fn validate_identity_batch_update(data: Bytes, expected_size: usize) -> Result<Bytes, Error> {
    if data.len() != expected_size {
        return Err(make_err!(
            Code::InvalidArgument,
            "Identity data size {} does not match expected size {}",
            data.len(),
            expected_size
        ));
    }
    Ok(data)
}

pub async fn decompress_batch_update(
    data: Bytes,
    compressor_i32: i32,
    expected_size: usize,
    remote_cache_compression_enabled: bool,
) -> Result<Bytes, Error> {
    let request_compressor = compressor::Value::try_from(compressor_i32)
        .map_err(|_| make_input_err!("Unknown compressor value: {}", compressor_i32))?;

    match request_compressor {
        compressor::Value::Identity => validate_identity_batch_update(data, expected_size),
        compressor::Value::Zstd => {
            if !remote_cache_compression_enabled {
                return Err(make_input_err!(
                    "Remote cache compression is not supported by this instance"
                ));
            }
            spawn_blocking!("cas_decode_compressed_upload", move || {
                decompress(&data, compressor::Value::Zstd, expected_size)
            })
            .await
            .map_err(|e| make_err!(Code::Internal, "Decompression task failed: {}", e))?
        }
        other => Err(make_input_err!("Unsupported wire compressor: {:?}", other)),
    }
}
