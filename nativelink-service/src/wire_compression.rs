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

use bytes::Bytes;
use nativelink_config::cas_server::WireCompressor;
use nativelink_error::{Code, Error, make_err, make_input_err};
use nativelink_proto::build::bazel::remote::execution::v2::compressor;

/// Zstd compression level for wire compression.
/// Level 0 in the zstd crate means "use default" (currently 3).
/// We use an explicit level for clarity.
const ZSTD_COMPRESSION_LEVEL: i32 = 3;

/// Convert a `WireCompressor` config value to a proto `compressor::Value`.
/// Returns `None` for `Identity` (identity is always implied per spec and
/// should not appear in capability lists).
pub const fn wire_compressor_to_proto(c: WireCompressor) -> Option<compressor::Value> {
    match c {
        WireCompressor::Identity => None,
        WireCompressor::Zstd => Some(compressor::Value::Zstd),
    }
}

/// Convert a proto `compressor::Value` to a `WireCompressor` config value.
/// Returns an error for compressors we don't support.
pub fn proto_to_wire_compressor(c: compressor::Value) -> Result<WireCompressor, Error> {
    match c {
        compressor::Value::Identity => Ok(WireCompressor::Identity),
        compressor::Value::Zstd => Ok(WireCompressor::Zstd),
        _ => Err(make_input_err!("Unsupported wire compressor: {:?}", c)),
    }
}

/// Compress data using the specified wire compressor.
///
/// `data` is the raw (uncompressed) bytes from the store.
/// Returns the compressed bytes suitable for sending on the wire.
pub fn compress(data: &[u8], compressor_value: compressor::Value) -> Result<Bytes, Error> {
    match compressor_value {
        compressor::Value::Identity => Ok(Bytes::copy_from_slice(data)),
        compressor::Value::Zstd => {
            let compressed = zstd::bulk::compress(data, ZSTD_COMPRESSION_LEVEL)
                .map_err(|e| make_err!(Code::Internal, "Zstd compression failed: {}", e))?;
            Ok(Bytes::from(compressed))
        }
        _ => Err(make_input_err!(
            "Unsupported wire compressor for compression: {:?}",
            compressor_value
        )),
    }
}

/// Decompress data using the specified wire compressor.
///
/// `data` is the compressed bytes received from the wire.
/// `expected_size` is the uncompressed size (from the digest), used to
/// pre-allocate the output buffer and cap decompression to prevent memory
/// exhaustion from malicious data.
/// Returns the decompressed bytes suitable for storing.
pub fn decompress(
    data: &[u8],
    compressor_value: compressor::Value,
    expected_size: usize,
) -> Result<Bytes, Error> {
    match compressor_value {
        compressor::Value::Identity => {
            if data.len() != expected_size {
                return Err(make_err!(
                    Code::InvalidArgument,
                    "Identity data size {} does not match expected size {}",
                    data.len(),
                    expected_size
                ));
            }
            Ok(Bytes::copy_from_slice(data))
        }
        compressor::Value::Zstd => {
            // Use bulk::decompress with expected_size as the capacity cap.
            // This prevents memory exhaustion from malicious payloads that
            // decompress to gigabytes — the output buffer is capped at
            // expected_size bytes.
            let decoded = zstd::bulk::decompress(data, expected_size).map_err(|e| {
                make_err!(Code::InvalidArgument, "Zstd decompression failed: {}", e)
            })?;
            if decoded.len() != expected_size {
                return Err(make_err!(
                    Code::InvalidArgument,
                    "Decompressed size {} does not match expected size {}",
                    decoded.len(),
                    expected_size
                ));
            }
            Ok(Bytes::from(decoded))
        }
        _ => Err(make_input_err!(
            "Unsupported wire compressor for decompression: {:?}",
            compressor_value
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compress_decompress_identity() {
        let data = b"hello world";
        let compressed = compress(data, compressor::Value::Identity).unwrap();
        assert_eq!(&compressed[..], data);
        let decompressed =
            decompress(&compressed, compressor::Value::Identity, data.len()).unwrap();
        assert_eq!(&decompressed[..], data);
    }

    #[test]
    fn test_compress_decompress_zstd() {
        // Use repetitive data that zstd can actually compress.
        let data: Vec<u8> = "hello world ".repeat(100).into_bytes();
        assert!(data.len() > 1000, "test data should be reasonably large");
        let compressed = compress(&data, compressor::Value::Zstd).unwrap();
        // Zstd should compress this repetitive data significantly
        assert!(
            compressed.len() < data.len(),
            "Zstd compressed {} bytes should be less than original {} bytes",
            compressed.len(),
            data.len()
        );
        let decompressed = decompress(&compressed, compressor::Value::Zstd, data.len()).unwrap();
        assert_eq!(decompressed.as_ref(), data.as_slice());
    }

    #[test]
    fn test_decompress_zstd_size_mismatch() {
        let data = b"hello world";
        let compressed = compress(data, compressor::Value::Zstd).unwrap();
        let result = decompress(&compressed, compressor::Value::Zstd, 999);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, Code::InvalidArgument);
    }

    #[test]
    fn test_decompress_identity_size_mismatch() {
        let data = b"hello world";
        let result = decompress(data, compressor::Value::Identity, 999);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err().code, Code::InvalidArgument);
    }

    #[test]
    fn test_wire_compressor_roundtrip() {
        assert_eq!(wire_compressor_to_proto(WireCompressor::Identity), None);
        assert_eq!(
            wire_compressor_to_proto(WireCompressor::Zstd),
            Some(compressor::Value::Zstd)
        );
        assert_eq!(
            proto_to_wire_compressor(compressor::Value::Identity).unwrap(),
            WireCompressor::Identity
        );
        assert_eq!(
            proto_to_wire_compressor(compressor::Value::Zstd).unwrap(),
            WireCompressor::Zstd
        );
    }
}
