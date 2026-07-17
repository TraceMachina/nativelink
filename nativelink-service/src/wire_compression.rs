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

/// Upper bound on the buffer `decompress` reserves up front for a zstd blob.
/// `expected_size` comes from the client's claimed digest, so it must never be
/// used as an allocation hint directly: a small payload claiming a huge size
/// would otherwise force a large pre-emptive allocation before the real
/// decompressed length is ever known. We reserve `min(expected_size, this)`
/// so common payloads never reallocate while a hostile claim allocates at most
/// this. Sized comfortably above any honest `BatchUpdateBlobs` payload (the
/// only caller of this bulk path; large blobs stream through `ByteStream`).
const ZSTD_DECOMPRESS_PREALLOC_CAP: usize = 1024 * 1024;

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
/// `expected_size` is the uncompressed size (from the client's digest). It is
/// the hard cap on the decompressed output, but never a direct allocation
/// hint: the buffer grows with the real decoded bytes so a small payload
/// claiming a huge size cannot force a large up-front allocation.
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
            // Decode incrementally so `expected_size` (which is attacker
            // controlled — it is the client's claimed digest size) can bound
            // the output without being trusted as an allocation size. We
            // reserve only `min(expected_size, ZSTD_DECOMPRESS_PREALLOC_CAP)`,
            // then `take(expected_size + 1)` hard-caps the decoder so a
            // decompression bomb is rejected as soon as it overshoots. This
            // mirrors the real-byte-count validation the identity arm and the
            // streaming upload path already perform.
            let decoder = zstd::stream::read::Decoder::new(data)
                .map_err(|e| make_err!(Code::InvalidArgument, "Zstd decompression failed: {e}"))?;
            let mut output = Vec::with_capacity(expected_size.min(ZSTD_DECOMPRESS_PREALLOC_CAP));
            // `+ 1` lets an oversized stream produce one byte past the cap so
            // the size check below rejects it rather than silently truncating.
            let cap = u64::try_from(expected_size)
                .err_tip(|| "expected_size did not fit in u64")?
                .saturating_add(1);
            decoder
                .take(cap)
                .read_to_end(&mut output)
                .map_err(|e| make_err!(Code::InvalidArgument, "Zstd decompression failed: {e}"))?;
            if output.len() != expected_size {
                return Err(make_err!(
                    Code::InvalidArgument,
                    "Decompressed size {} does not match expected size {}",
                    output.len(),
                    expected_size
                ));
            }
            Ok(Bytes::from(output))
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

/// Decode a client's zstd wire stream into raw bytes on `tx`, asynchronously.
///
/// Like [`stream_encode_compressed_download`], this must not occupy a tokio
/// blocking-pool thread for the stream's lifetime: the input arrives at the
/// client's upload pace and `tx` drains at the store's write pace, so a
/// blocking implementation parks a pool thread on whichever side is slower
/// for as long as the upload lasts. The zstd frame is consumed incrementally
/// with the raw streaming API instead; per-chunk decode cost is bounded by
/// the channel chunk size, so it runs inline on the async runtime with
/// channel-native backpressure on both sides.
///
/// Validation semantics match the REAPI compressed-blobs contract: the
/// decoded byte count may never exceed the digest size (checked per chunk so
/// a decompression bomb is rejected as soon as it overshoots), the final
/// count must equal it exactly, and the decoded bytes must hash to `digest`.
pub async fn stream_decode_compressed_upload(
    mut compressed_rx: DropCloserReadHalf,
    wire_compressor: compressor::Value,
    digest: DigestInfo,
    digest_function: DigestHasherFunc,
    mut tx: DropCloserWriteHalf,
) -> Result<(), Error> {
    use zstd::stream::raw::{Decoder, InBuffer, Operation, OutBuffer};

    if wire_compressor != compressor::Value::Zstd {
        return Err(make_input_err!(
            "Streaming upload decompression only supports zstd, got {:?}",
            wire_compressor
        ));
    }

    let expected_size = digest.size_bytes();
    let mut hasher = digest_function.hasher();
    let mut decoded_size = 0u64;
    let mut decoder = Decoder::new()
        .map_err(|e| make_err!(Code::InvalidArgument, "Zstd decompression failed: {}", e))?;
    // `DCtx::out_size()` guarantees a full decompressed block always fits, so
    // the decoder never stalls for lack of output space within one `run`.
    let mut out_buf = vec![0u8; zstd::zstd_safe::DCtx::out_size()];
    // Last input-size hint from the decoder: nonzero at input EOF means the
    // stream ended in the middle of a frame and must be rejected.
    let mut frame_input_hint = 0usize;
    loop {
        let chunk = compressed_rx
            .recv()
            .await
            .err_tip(|| "Failed to receive compressed data in stream_decode_compressed_upload")?;
        if chunk.is_empty() {
            break; // EOF.
        }
        let mut in_buffer = InBuffer::around(&chunk);
        loop {
            let mut out_buffer = OutBuffer::around(out_buf.as_mut_slice());
            frame_input_hint = decoder.run(&mut in_buffer, &mut out_buffer).map_err(|e| {
                make_err!(Code::InvalidArgument, "Zstd decompression failed: {}", e)
            })?;
            let produced = Bytes::copy_from_slice(out_buffer.as_slice());
            // A completely full output buffer means the decoder may still
            // have buffered output to flush, even with no input left.
            let output_was_full = produced.len() == out_buf.len();
            if !produced.is_empty() {
                let produced_u64 = u64::try_from(produced.len())
                    .err_tip(|| "Decoded chunk size was not convertible to u64")?;
                decoded_size = decoded_size.checked_add(produced_u64).ok_or_else(|| {
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
                hasher.update(&produced);
                tx.send(produced).await?;
            }
            if in_buffer.pos() == in_buffer.src.len() && !output_was_full {
                break;
            }
        }
    }
    if frame_input_hint != 0 {
        return Err(make_err!(
            Code::InvalidArgument,
            "Compressed upload stream ended in the middle of a zstd frame"
        ));
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

/// Encode a raw byte stream into a single zstd frame on `tx`, asynchronously.
///
/// This runs entirely on the async runtime and must never occupy a tokio
/// blocking-pool thread for the stream's lifetime: `tx` drains at the gRPC
/// client's pace, so a blocking implementation (blocking reads from `raw_rx`
/// plus `blocking_send` into `tx`) parks one pool thread per concurrent
/// compressed download until the client finishes. Enough concurrent downloads
/// with slow consumers then exhaust the blocking pool and starve every other
/// `spawn_blocking` user (filesystem store I/O, upload decode, credential
/// resolution). The zstd frame is instead produced incrementally with the raw
/// streaming API: the CPU cost per iteration is bounded by the channel chunk
/// size (small — micro/milliseconds), so it is acceptable inline on a worker
/// thread, and `tx.send(...).await` gives backpressure without a parked
/// thread.
///
/// The output is one well-formed zstd frame, identical in wire format to what
/// `zstd::stream::read::Encoder` produces (both drive `ZSTD_compressStream`
/// on a fresh `CCtx`).
pub async fn stream_encode_compressed_download(
    mut raw_rx: DropCloserReadHalf,
    wire_compressor: compressor::Value,
    mut tx: DropCloserWriteHalf,
) -> Result<(), Error> {
    use zstd::stream::raw::{Encoder, InBuffer, Operation, OutBuffer};

    if wire_compressor != compressor::Value::Zstd {
        return Err(make_input_err!(
            "Streaming download compression only supports zstd, got {:?}",
            wire_compressor
        ));
    }

    let mut encoder = Encoder::new(ZSTD_COMPRESSION_LEVEL)
        .map_err(|e| make_err!(Code::Internal, "Zstd compression failed: {}", e))?;
    // `CCtx::out_size()` guarantees a full compressed block always fits, so
    // the encoder never stalls for lack of output space within one `run`.
    let mut out_buf = vec![0u8; zstd::zstd_safe::CCtx::out_size()];
    loop {
        let chunk = raw_rx
            .recv()
            .await
            .err_tip(|| "Failed to receive raw data in stream_encode_compressed_download")?;
        if chunk.is_empty() {
            break; // EOF.
        }
        let mut in_buffer = InBuffer::around(&chunk);
        while in_buffer.pos() < in_buffer.src.len() {
            let mut out_buffer = OutBuffer::around(out_buf.as_mut_slice());
            encoder
                .run(&mut in_buffer, &mut out_buffer)
                .map_err(|e| make_err!(Code::Internal, "Zstd compression failed: {}", e))?;
            let produced = out_buffer.as_slice();
            if !produced.is_empty() {
                tx.send(Bytes::copy_from_slice(produced)).await?;
            }
        }
    }

    // Finish the frame: flush any internally buffered compressed data plus
    // the frame epilogue. `finish` reports the bytes still pending, so loop
    // until it reports none.
    loop {
        let mut out_buffer = OutBuffer::around(out_buf.as_mut_slice());
        let remaining = encoder
            .finish(&mut out_buffer, true)
            .map_err(|e| make_err!(Code::Internal, "Zstd compression failed: {}", e))?;
        let produced = out_buffer.as_slice();
        if !produced.is_empty() {
            tx.send(Bytes::copy_from_slice(produced)).await?;
        }
        if remaining == 0 {
            break;
        }
    }

    tx.send_eof()
        .err_tip(|| "Failed to send compressed download EOF")?;
    Ok(())
}
