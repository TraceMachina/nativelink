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
use std::io::Read;

use bytes::Bytes;
use nativelink_config::cas_server::{CapabilitiesConfig, InstanceName, WithInstanceName};
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_proto::build::bazel::remote::execution::v2::compressor;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use nativelink_util::spawn_blocking;
use tracing::warn;

/// Zstd compression level for wire compression.
/// Level 0 in the zstd crate means "use default" (currently 3).
/// We use an explicit level for clarity.
pub const ZSTD_COMPRESSION_LEVEL: i32 = 3;

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

#[derive(Debug)]
pub struct BufChannelReader {
    rx: DropCloserReadHalf,
    chunk: Bytes,
    chunk_offset: usize,
}

impl BufChannelReader {
    pub const fn new(rx: DropCloserReadHalf) -> Self {
        Self {
            rx,
            chunk: Bytes::new(),
            chunk_offset: 0,
        }
    }

    fn refill_chunk(&mut self) -> std::io::Result<bool> {
        while self.chunk_offset == self.chunk.len() {
            self.chunk = self.rx.blocking_recv().map_err(Error::to_std_err)?;
            self.chunk_offset = 0;
            if self.chunk.is_empty() {
                return Ok(false);
            }
        }

        Ok(true)
    }
}

impl Read for BufChannelReader {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }

        if !self.refill_chunk()? {
            return Ok(0);
        }

        let chunk_remaining = &self.chunk[self.chunk_offset..];
        let bytes_to_copy = output.len().min(chunk_remaining.len());

        output[..bytes_to_copy].copy_from_slice(&chunk_remaining[..bytes_to_copy]);
        self.chunk_offset += bytes_to_copy;

        Ok(bytes_to_copy)
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

/// Compress data using the specified wire compressor.
///
/// `data` is the raw (uncompressed) bytes from the store.
/// Returns the compressed bytes suitable for sending on the wire.
pub fn compress(data: Bytes, compressor_value: compressor::Value) -> Result<Bytes, Error> {
    match compressor_value {
        compressor::Value::Identity => Ok(data),
        compressor::Value::Zstd => {
            let compressed = zstd::bulk::compress(&data, ZSTD_COMPRESSION_LEVEL)
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

pub fn stream_decode_compressed_upload(
    compressed_rx: DropCloserReadHalf,
    wire_compressor: compressor::Value,
    digest: DigestInfo,
    digest_function: DigestHasherFunc,
    mut tx: DropCloserWriteHalf,
) -> Result<(), Error> {
    if wire_compressor != compressor::Value::Zstd {
        return Err(make_input_err!(
            "Streaming upload decompression only supports zstd, got {:?}",
            wire_compressor
        ));
    }

    let expected_size = digest.size_bytes();
    let mut hasher = digest_function.hasher();
    let mut decoded_size = 0u64;
    let mut buffer = vec![0u8; zstd::zstd_safe::DCtx::out_size()];

    let reader = BufChannelReader::new(compressed_rx);
    let mut decoder = zstd::stream::read::Decoder::new(reader)
        .map_err(|e| make_err!(Code::InvalidArgument, "Zstd decompression failed: {}", e))?;
    loop {
        let read = decoder
            .read(&mut buffer)
            .map_err(|e| make_err!(Code::InvalidArgument, "Zstd decompression failed: {}", e))?;
        if read == 0 {
            break;
        }
        let read_u64 =
            u64::try_from(read).err_tip(|| "Decoded chunk size was not convertible to u64")?;
        decoded_size = decoded_size.checked_add(read_u64).ok_or_else(|| {
            make_err!(
                Code::InvalidArgument,
                "Decoded compressed upload size overflow"
            )
        })?;
        if decoded_size > expected_size {
            return Err(make_err!(
                Code::InvalidArgument,
                "Decoded compressed upload size {} bytes exceeds digest size {} bytes",
                decoded_size,
                expected_size
            ));
        }
        hasher.update(&buffer[..read]);
        tx.blocking_send(Bytes::copy_from_slice(&buffer[..read]))?;
    }

    if decoded_size != expected_size {
        return Err(make_err!(
            Code::InvalidArgument,
            "Decompressed size {} does not match expected size {}",
            decoded_size,
            expected_size
        ));
    }
    let actual_digest = hasher.finalize_digest();
    if actual_digest != digest {
        return Err(make_err!(
            Code::InvalidArgument,
            "Decompressed digest {} does not match expected digest {}",
            actual_digest,
            digest
        ));
    }

    tx.send_eof()
        .err_tip(|| "Failed to send decompressed upload EOF")?;
    Ok(())
}

pub fn stream_encode_compressed_download(
    raw_rx: DropCloserReadHalf,
    wire_compressor: compressor::Value,
    mut tx: DropCloserWriteHalf,
) -> Result<(), Error> {
    if wire_compressor != compressor::Value::Zstd {
        return Err(make_input_err!(
            "Streaming download compression only supports zstd, got {:?}",
            wire_compressor
        ));
    }

    let reader = BufChannelReader::new(raw_rx);
    let mut encoder = zstd::stream::read::Encoder::new(reader, ZSTD_COMPRESSION_LEVEL)
        .map_err(|e| make_err!(Code::Internal, "Zstd compression failed: {}", e))?;
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = encoder
            .read(&mut buffer)
            .map_err(|e| make_err!(Code::Internal, "Zstd compression failed: {}", e))?;
        if read == 0 {
            break;
        }
        tx.blocking_send(Bytes::copy_from_slice(&buffer[..read]))?;
    }

    tx.send_eof()
        .err_tip(|| "Failed to send compressed download EOF")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use nativelink_util::buf_channel::make_buf_channel_pair;

    use super::*;

    #[test]
    fn test_compress_decompress_identity() {
        let data = Bytes::from_static(b"hello world");
        let compressed = compress(data.clone(), compressor::Value::Identity).unwrap();
        assert_eq!(compressed, data);
        let decompressed =
            decompress(&compressed, compressor::Value::Identity, data.len()).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_compress_decompress_zstd() {
        // Use repetitive data that zstd can actually compress.
        let data = Bytes::from("hello world ".repeat(100));
        assert!(data.len() > 1000, "test data should be reasonably large");
        let compressed = compress(data.clone(), compressor::Value::Zstd).unwrap();
        // Zstd should compress this repetitive data significantly
        assert!(
            compressed.len() < data.len(),
            "Zstd compressed {} bytes should be less than original {} bytes",
            compressed.len(),
            data.len()
        );
        let decompressed = decompress(&compressed, compressor::Value::Zstd, data.len()).unwrap();
        assert_eq!(decompressed, data);
    }

    #[test]
    fn test_decompress_zstd_size_mismatch() {
        let data = Bytes::from_static(b"hello world");
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
    fn validate_identity_batch_update_keeps_bytes_zero_copy() {
        let data = Bytes::from_static(b"identity batch update payload");
        let original_ptr = data.as_ptr();

        let decompressed = validate_identity_batch_update(data.clone(), data.len())
            .expect("identity batch update should validate without copying");

        assert_eq!(decompressed, data);
        assert_eq!(decompressed.as_ptr(), original_ptr);
    }

    #[test]
    fn test_resolve_wire_compressor() {
        assert_eq!(
            resolve_wire_compressor(None, false).unwrap(),
            compressor::Value::Identity
        );
        assert_eq!(
            resolve_wire_compressor(Some("identity"), false).unwrap(),
            compressor::Value::Identity
        );
        assert_eq!(
            resolve_wire_compressor(Some("zstd"), true).unwrap(),
            compressor::Value::Zstd
        );
        assert!(resolve_wire_compressor(Some("zstd"), false).is_err());
        assert!(resolve_wire_compressor(Some("brotli"), true).is_err());
    }

    #[test]
    fn buf_channel_reader_reads_partial_and_multiple_chunks() {
        let (mut tx, rx) = make_buf_channel_pair();
        tx.blocking_send(Bytes::from_static(b"abc"))
            .expect("failed to send first chunk");
        tx.blocking_send(Bytes::from_static(b"defgh"))
            .expect("failed to send second chunk");
        tx.send_eof().expect("failed to send EOF");

        let mut reader = BufChannelReader::new(rx);

        let mut first_partial_chunk = [0; 2];
        reader
            .read_exact(&mut first_partial_chunk)
            .expect("failed to read first partial chunk");
        assert_eq!(&first_partial_chunk, b"ab");

        let mut remaining_chunks = [0; 6];
        reader
            .read_exact(&mut remaining_chunks)
            .expect("failed to read remaining chunks");
        assert_eq!(&remaining_chunks, b"cdefgh");

        let mut eof_probe = [0; 1];
        assert_eq!(reader.read(&mut eof_probe).expect("failed to read EOF"), 0);
    }
}
