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

use core::cmp;
use core::future::Future;
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::ffi::OsString;
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use bytes::Bytes;
use futures::future::FutureExt;
use nativelink_config::stores::ZstdConfig;
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{
    BufChannelReader, BufChannelWriter, DropCloserReadHalf, DropCloserWriteHalf,
    make_buf_channel_pair,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{
    DigestHasher, DigestHasherFunc, DigestHasherImpl, digest_hasher_func_from_context,
};
use nativelink_util::fs::{self, FileSlot};
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    RemoveCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo, WireCompressionStore,
    WireCompressor,
};
use nativelink_util::{spawn, spawn_blocking};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::cas_utils::is_zero_digest;

/// Default number of concurrent staged uploads when the config value is `0`.
const DEFAULT_MAX_CONCURRENT_STAGED_UPLOADS: usize = 4;
/// Default number of concurrent identity (uncompressed) reads/writes when the
/// config value is `0`.
const DEFAULT_MAX_CONCURRENT_IDENTITY_OPS: usize = 256;
/// Default number of concurrent recompressions when the config value is `0`.
const DEFAULT_MAX_CONCURRENT_RECOMPRESSIONS: usize = 1;
/// Default total validate-and-stage deadline (seconds) when the config value is
/// `0`. Bounds a client that trickles bytes to hold a staging slot open.
const DEFAULT_STAGE_TIMEOUT_S: u64 = 600;
/// Default commit timeout (seconds) when the config value is `0`. Bounds how
/// long a stalled inner-store commit (or recompression) may hold a staging
/// permit before the upload fails with `DeadlineExceeded` and cleans up.
const DEFAULT_COMMIT_TIMEOUT_S: u64 = 300;
/// Default ceiling for committing a compressed upload straight from memory
/// (no staging file, no `fsync`) when the config value is `0`.
const DEFAULT_MAX_INLINE_COMMIT_SIZE: u64 = 4 * 1024 * 1024;
/// Level used when no `compression_level` is configured, matching the service
/// wire codec (`nativelink-service` `wire_compression::ZSTD_COMPRESSION_LEVEL`).
const DEFAULT_ENCODE_LEVEL: i32 = 3;

/// The canonical zstd encoding of empty input, computed once. Emitted verbatim
/// for zero-digest `get_zstd` responses so the client always receives a valid
/// zstd stream that decodes to nothing.
fn empty_zstd_frame() -> Bytes {
    static EMPTY: OnceLock<Bytes> = OnceLock::new();
    EMPTY
        .get_or_init(|| {
            Bytes::from(
                zstd::bulk::compress(&[], DEFAULT_ENCODE_LEVEL)
                    .expect("zstd compression of empty input cannot fail"),
            )
        })
        .clone()
}

/// Flattens a `spawn!`/`spawn_blocking!` join result into the task's own result.
fn flatten_join<T, E: Into<Error>>(
    joined: Result<Result<T, Error>, E>,
    tip: &'static str,
) -> Result<T, Error> {
    joined.err_tip(|| tip)?
}

/// Maps an `io::Error` from a staging-file operation onto an internal error.
fn stage_file_err(action: &'static str) -> impl FnOnce(std::io::Error) -> Error {
    move |e| make_err!(Code::Internal, "Failed to {action} zstd staging file: {e}")
}

/// Increments a gauge for as long as it is held.
struct Inflight<'a>(&'a AtomicU64);

impl<'a> Inflight<'a> {
    fn enter(gauge: &'a AtomicU64) -> Self {
        gauge.fetch_add(1, Ordering::Relaxed);
        Self(gauge)
    }
}

impl Drop for Inflight<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Upper bound on the number of compressed bytes a zstd frame can produce for
/// `src_size` uncompressed bytes. Mirrors zstd's `ZSTD_COMPRESSBOUND` with a
/// little extra slack for the frame header/epilogue. Used to bound the inner
/// store upload since the physical size is not known ahead of time.
const fn zstd_compress_bound(src_size: u64) -> u64 {
    let low_bound_slack = if src_size < 128 * 1024 {
        (128 * 1024 - src_size) >> 11
    } else {
        0
    };
    src_size
        .saturating_add(src_size >> 8)
        .saturating_add(low_bound_slack)
        .saturating_add(64)
}

/// A blocking source of compressed chunks. An empty chunk signals EOF.
///
/// This is what lets the streaming and whole-buffer entry points share one
/// bounded decoder: `DropCloserReadHalf` feeds it from a client stream, while
/// [`OneChunk`] feeds it a payload already in memory.
trait ChunkSource {
    fn next_chunk(&mut self) -> Result<Bytes, Error>;
}

impl ChunkSource for DropCloserReadHalf {
    fn next_chunk(&mut self) -> Result<Bytes, Error> {
        self.blocking_recv()
    }
}

/// Yields one in-memory buffer, then EOF.
struct OneChunk(Option<Bytes>);

impl ChunkSource for OneChunk {
    fn next_chunk(&mut self) -> Result<Bytes, Error> {
        Ok(self.0.take().unwrap_or_default())
    }
}

/// Streaming zstd encode of an identity (raw) upload.
///
/// The inner store EOF is withheld until the finalized hash and uncompressed
/// size match `digest`; only then is the encoder finished and EOF sent, so a
/// mismatch can never commit the inner upload.
fn encode_identity(
    mut reader: DropCloserReadHalf,
    tx: DropCloserWriteHalf,
    level: i32,
    hasher_func: DigestHasherFunc,
    digest: DigestInfo,
) -> Result<u64, Error> {
    let expected_size = digest.size_bytes();
    let mut hasher = hasher_func.hasher();
    let mut uncompressed_size: u64 = 0;

    let mut encoder =
        zstd::stream::write::Encoder::new(BufChannelWriter::new(tx), level).map_err(|e| {
            make_err!(
                Code::Internal,
                "Zstd encoder init failed in zstd store: {e}"
            )
        })?;

    loop {
        let chunk = reader
            .blocking_recv()
            .err_tip(|| "Failed to read chunk in zstd store update")?;
        if chunk.is_empty() {
            break; // EOF.
        }

        uncompressed_size = uncompressed_size
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| make_input_err!("Uncompressed size overflow in zstd store update"))?;
        if uncompressed_size > expected_size {
            return Err(make_input_err!(
                "Received more data than digest size in zstd store update, got at least {} but digest says {}",
                uncompressed_size,
                expected_size
            ));
        }

        hasher.update(&chunk);
        encoder
            .write_all(&chunk)
            .map_err(|e| make_err!(Code::Internal, "Zstd encode failed in zstd store: {e}"))?;
    }

    if uncompressed_size != expected_size {
        return Err(make_input_err!(
            "Expected size {} but got size {} in zstd store update",
            expected_size,
            uncompressed_size
        ));
    }
    let actual_digest = hasher.finalize_digest();
    if actual_digest.packed_hash() != digest.packed_hash() {
        return Err(make_input_err!(
            "Hashes do not match in zstd store update, expected {} but got {}",
            digest.packed_hash(),
            actual_digest.packed_hash()
        ));
    }

    let mut writer = encoder.finish().map_err(|e| {
        make_err!(
            Code::Internal,
            "Zstd encoder finish failed in zstd store: {e}"
        )
    })?;
    writer
        .send_eof()
        .err_tip(|| "Failed to send EOF in zstd store update")?;
    Ok(expected_size)
}

/// Streaming zstd decode of stored (physical) data back to raw bytes,
/// forwarding at most `length` bytes starting at `offset`. A decode failure of
/// already-stored data is `DataLoss`: it was validated at upload time.
fn decode_identity(
    physical_rx: DropCloserReadHalf,
    mut raw_tx: DropCloserWriteHalf,
    offset: u64,
    length: Option<u64>,
) -> Result<(), Error> {
    let reader = BufChannelReader::new(physical_rx);
    let mut decoder = zstd::stream::read::Decoder::new(reader).map_err(|e| {
        make_err!(
            Code::DataLoss,
            "Zstd decoder init failed in zstd store: {e}"
        )
    })?;

    let mut to_skip = offset;
    let mut remaining = length.unwrap_or(u64::MAX);
    let mut buffer = vec![0u8; zstd::zstd_safe::DCtx::out_size()];

    // The decoder must be drained to EOF even once the byte budget is met:
    // dropping `physical_rx` early would fail the still-running inner `get`
    // with "receiver disconnected". Mirrors `compression_store::get_part`.
    loop {
        let read = decoder
            .read(&mut buffer)
            .map_err(|e| make_err!(Code::DataLoss, "Zstd decode failed in zstd store: {e}"))?;
        if read == 0 {
            break; // EOF: physical reader fully drained.
        }
        if remaining == 0 {
            continue; // Budget met; keep draining but stop forwarding.
        }

        let mut slice = &buffer[..read];
        if to_skip > 0 {
            // `slice.len()` bounds the min, so the result always fits in usize.
            let skip = usize::try_from(cmp::min(to_skip, slice.len() as u64)).unwrap_or(usize::MAX);
            slice = &slice[skip..];
            to_skip -= skip as u64;
        }
        if slice.is_empty() {
            continue;
        }

        // `slice.len()` bounds the min, so the result always fits in usize.
        let take = usize::try_from(cmp::min(remaining, slice.len() as u64)).unwrap_or(usize::MAX);
        remaining -= take as u64;
        raw_tx
            .blocking_send(Bytes::copy_from_slice(&slice[..take]))
            .err_tip(|| "Failed to send decoded chunk in zstd store get_part")?;
    }

    raw_tx
        .send_eof()
        .err_tip(|| "Failed to send decoded EOF in zstd store get_part")?;
    Ok(())
}

/// Best-effort removal of a staged temp file when dropped. Staged files are pure
/// scratch — the inner store takes the bytes at commit — so the path is removed
/// on success, error, and cancellation alike.
#[derive(Debug)]
struct TempFileGuard {
    path: Option<String>,
}

impl TempFileGuard {
    const fn arm(path: String) -> Self {
        Self { path: Some(path) }
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        if let Some(path) = &self.path {
            // Ignore failures: a backend that commits by moving the file has
            // already renamed this path away.
            drop(std::fs::remove_file(path));
        }
    }
}

/// A `std::io::Write` sink that hashes and counts decoded output instead of
/// storing it, optionally buffering it for recompression up to `collect_limit`.
///
/// This is the enforcement point for the decoded-output bound: a `write` that
/// would push `decoded_len` past `max_decoded_size` is rejected *before* any of
/// the offending bytes are hashed or collected. Because the zstd streaming
/// decoder emits output in bounded blocks, a small "zstd bomb" is stopped at the
/// first over-limit block rather than being fully materialized.
struct DecodeSink {
    hasher: DigestHasherImpl,
    decoded_len: u64,
    /// Hard ceiling on total decoded bytes; equals the digest's uncompressed
    /// size.
    max_decoded_size: u64,
    collect: Option<Vec<u8>>,
    collect_limit: u64,
}

impl DecodeSink {
    fn new(
        hasher_func: DigestHasherFunc,
        max_decoded_size: u64,
        collect_limit: Option<u64>,
    ) -> Self {
        Self {
            hasher: hasher_func.hasher(),
            decoded_len: 0,
            max_decoded_size,
            collect: collect_limit.map(|_| Vec::new()),
            collect_limit: collect_limit.unwrap_or(0),
        }
    }
}

impl Write for DecodeSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // `checked_add` (never `saturating_add`) so a length overflow is an
        // error, not a silently clamped value.
        let new_len = self
            .decoded_len
            .checked_add(buf.len() as u64)
            .ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "decoded length overflow in zstd store",
                )
            })?;
        if new_len > self.max_decoded_size {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "decoded output exceeded digest size {} in zstd store",
                    self.max_decoded_size
                ),
            ));
        }
        self.hasher.update(buf);
        self.decoded_len = new_len;
        if let Some(collected) = self.collect.as_mut() {
            if new_len <= self.collect_limit {
                collected.extend_from_slice(buf);
            } else {
                // Too large to re-compress in-memory; stop collecting.
                self.collect = None;
            }
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Final check of a bounded decode against the requested digest. The sink
/// already guarantees `decoded_len <= digest.size_bytes()`; this catches the
/// short case and the hash.
fn verify_decoded_digest(sink: &mut DecodeSink, digest: DigestInfo) -> Result<(), Error> {
    let expected_size = digest.size_bytes();
    if sink.decoded_len != expected_size {
        return Err(make_input_err!(
            "Decoded size {} does not match digest size {expected_size} in zstd store",
            sink.decoded_len
        ));
    }
    let actual_digest = sink.hasher.finalize_digest();
    if actual_digest.packed_hash() != digest.packed_hash() {
        return Err(make_input_err!(
            "Hashes do not match in zstd store update, expected {} but got {}",
            digest.packed_hash(),
            actual_digest.packed_hash()
        ));
    }
    Ok(())
}

/// Exclusively create a staged temp file at `path`, returning its open
/// descriptor. `create_new(true)` (`O_CREAT | O_EXCL`) fails if the path already
/// exists or is a symlink, defeating an observe-and-replace race; on unix the
/// owner-only `0o600` mode is applied atomically at creation rather than via a
/// later `chmod` by path. Runs inside `spawn_blocking!`.
fn create_temp_exclusive(path: &str) -> Result<std::fs::File, Error> {
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).map_err(|e| {
        make_err!(
            Code::Internal,
            "Failed to exclusively create zstd staging file {path}: {e}"
        )
    })
}

/// Decode a complete concatenation of zstd frames from `source` with bounded
/// wire and decoded sizes. `write_wire` receives every compressed chunk before
/// it is decoded (the staging path uses it to persist the exact client bytes).
///
/// [`zio::Writer::finish`](zstd::stream::zio::Writer::finish) is deliberate:
/// unlike `flush`, it drives the decoder with EOF and rejects an incomplete
/// final frame, so a valid frame followed by a truncated second frame is
/// refused rather than committed.
fn decode_bounded_zstd_stream<S, F>(
    mut source: S,
    sink: DecodeSink,
    max_compressed_upload_size: u64,
    read_context: &'static str,
    decode_context: &'static str,
    mut write_wire: F,
) -> Result<(DecodeSink, u64), Error>
where
    S: ChunkSource,
    F: FnMut(&[u8]) -> Result<(), Error>,
{
    let raw_decoder = zstd::stream::raw::Decoder::new().map_err(|e| {
        make_err!(
            Code::Internal,
            "Zstd decoder init failed in zstd store: {e}"
        )
    })?;
    let mut decoder = zstd::stream::zio::Writer::new(sink, raw_decoder);
    let mut wire_bytes_consumed: u64 = 0;
    loop {
        let chunk = source.next_chunk().err_tip(|| read_context)?;
        if chunk.is_empty() {
            break;
        }
        wire_bytes_consumed = wire_bytes_consumed
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| make_err!(Code::Internal, "Wire byte count overflow in zstd store"))?;
        if wire_bytes_consumed > max_compressed_upload_size {
            return Err(make_err!(
                Code::ResourceExhausted,
                "Compressed upload exceeded max_compressed_upload_size ({max_compressed_upload_size} bytes) in zstd store"
            ));
        }
        write_wire(&chunk)?;
        decoder
            .write_all(&chunk)
            .map_err(|e| make_input_err!("{decode_context}: {e}"))?;
    }
    decoder
        .finish()
        .map_err(|e| make_input_err!("{decode_context}: {e}"))?;
    let (sink, _raw_decoder) = decoder.into_inner();
    Ok((sink, wire_bytes_consumed))
}

/// What the blocking staging pass hands back on success.
struct StageOutput {
    /// Compressed wire bytes consumed from the client, which is also the size of
    /// the staged file.
    wire_bytes_consumed: u64,
    /// Decoded content, present only when recompression is eligible (decoded
    /// length stayed within the configured `max_recompression_size`).
    collected: Option<Vec<u8>>,
    /// The descriptor that was validated, rewound to offset 0.
    file: FileSlot,
    /// The cleanup guard, owned by the blocking task until it returns.
    guard: TempFileGuard,
}

/// Resources transferred to the detached staging task. In particular, the
/// armed cleanup guard stays owned by the task until it returns.
struct StageCompressedInput {
    reader: DropCloserReadHalf,
    path: String,
    digest: DigestInfo,
    hasher_func: DigestHasherFunc,
    max_compressed_upload_size: u64,
    collect_limit: Option<u64>,
    fs_permit: tokio::sync::SemaphorePermit<'static>,
    guard: TempFileGuard,
}

/// Blocking validation pass: exclusively create the temp file at `path`, stream
/// compressed chunks from `reader` into it while decoding them to recompute the
/// uncompressed length and hash, then check both against `digest`. Enforces the
/// compressed-size cap (`ResourceExhausted`) and, via [`DecodeSink`], the
/// decoded-output cap (`InvalidArgument`) inside the decoder write.
///
/// The cleanup guard is owned by this detached blocking task until it returns,
/// so cancellation of the async caller cannot leak a file created after the
/// caller stopped awaiting. Only runs from within `spawn_blocking!`.
fn stage_compressed_blocking(
    StageCompressedInput {
        reader,
        path,
        digest,
        hasher_func,
        max_compressed_upload_size,
        collect_limit,
        fs_permit,
        guard,
    }: StageCompressedInput,
) -> Result<StageOutput, Error> {
    let mut file = create_temp_exclusive(&path)?;

    let sink = DecodeSink::new(hasher_func, digest.size_bytes(), collect_limit);
    let (mut sink, wire_bytes_consumed) = decode_bounded_zstd_stream(
        reader,
        sink,
        max_compressed_upload_size,
        "Failed to read compressed chunk in zstd store staging",
        "Zstd decode failed in zstd store staging",
        |chunk| file.write_all(chunk).map_err(stage_file_err("write")),
    )?;
    let collected = sink.collect.take();
    verify_decoded_digest(&mut sink, digest)?;

    file.flush().map_err(stage_file_err("flush"))?;
    file.sync_all().map_err(stage_file_err("sync"))?;
    // Rewind the *same* descriptor so the commit streams it from the start.
    file.seek(SeekFrom::Start(0))
        .map_err(stage_file_err("rewind"))?;

    Ok(StageOutput {
        wire_bytes_consumed,
        collected,
        file: FileSlot::from_std(fs_permit, file),
        guard,
    })
}

/// Blocking bounded decode used to validate that a stream decodes to exactly
/// `expected_size` bytes without buffering the decoded output or touching disk.
/// Returns the compressed wire bytes consumed.
fn validate_bounded_zstd_blocking<S: ChunkSource>(
    source: S,
    hasher_func: DigestHasherFunc,
    expected_size: u64,
    max_compressed_upload_size: u64,
) -> Result<u64, Error> {
    let sink = DecodeSink::new(hasher_func, expected_size, None);
    let (sink, wire_bytes_consumed) = decode_bounded_zstd_stream(
        source,
        sink,
        max_compressed_upload_size,
        "Failed to read chunk validating zero-digest zstd",
        "Zstd decode failed for zero digest",
        |_| Ok(()),
    )?;
    if sink.decoded_len != expected_size {
        return Err(make_input_err!(
            "Zero-digest zstd upload did not decode to empty in zstd store"
        ));
    }
    Ok(wire_bytes_consumed)
}

/// Blocking whole-buffer validation of a compressed upload: bounded decode, no
/// disk involvement. Returns the wire bytes consumed and, when recompression is
/// eligible, the decoded content.
fn validate_zstd_buffer(
    data: Bytes,
    digest: DigestInfo,
    hasher_func: DigestHasherFunc,
    max_compressed_upload_size: u64,
    collect_limit: Option<u64>,
) -> Result<(u64, Option<Vec<u8>>), Error> {
    let sink = DecodeSink::new(hasher_func, digest.size_bytes(), collect_limit);
    let (mut sink, wire_bytes_consumed) = decode_bounded_zstd_stream(
        OneChunk(Some(data)),
        sink,
        max_compressed_upload_size,
        "Failed to read compressed buffer in zstd store",
        "Zstd decode failed in zstd store inline validation",
        |_| Ok(()),
    )?;
    let collected = sink.collect.take();
    verify_decoded_digest(&mut sink, digest)?;
    Ok((wire_bytes_consumed, collected))
}

/// A validated staged upload, ready to commit. Dropping it removes the temp file
/// and releases the staging permit.
struct StagedStream {
    /// The descriptor validated by staging, rewound to offset 0, taken by the
    /// commit. Declared before `_guard` deliberately: Rust drops struct fields
    /// in declaration order, closing the descriptor before best-effort path
    /// removal on every cancellation/error path.
    file: Option<FileSlot>,
    /// Path of the temp file. Backends that commit by moving the file
    /// (`filesystem`) act on this path rather than on the descriptor; see
    /// [`ZstdStore::commit_staged`].
    path: String,
    /// Physical (compressed) size of what will be committed.
    compressed_size: u64,
    /// Compressed wire bytes actually consumed from the client.
    wire_bytes_consumed: u64,
    /// Decoded bytes retained only for optional recompression. Staging bounds
    /// this by `max_recompression_size`; it is consumed before commit.
    collected: Option<Vec<u8>>,
    _permit: OwnedSemaphorePermit,
    _guard: TempFileGuard,
}

/// A pass-through store that stores blobs zstd-compressed in the inner CAS
/// store while presenting the raw (uncompressed) view to clients.
///
/// This is CAS-only: only digest keys are supported. Zero-byte digests are
/// never forwarded to the inner store.
///
/// Zstd implementation selected by `CompressionAlgorithm::Zstd`.
#[derive(MetricsComponent)]
pub struct ZstdStore {
    #[metric(group = "inner_store")]
    inner_store: Store,
    /// Operator-controlled staging directory. Created/permission-checked in
    /// `post_init`; used by the zstd fast path.
    #[metric(help = "Staging directory for validated compressed uploads")]
    temp_path: String,
    /// Configured encode level, already validated to `1..=19`; `None` uses
    /// [`DEFAULT_ENCODE_LEVEL`].
    compression_level: Option<i32>,
    /// Hard cap on the number of compressed wire bytes accepted for a single
    /// upload before it is rejected with `ResourceExhausted`.
    #[metric(help = "Max compressed wire bytes accepted for one upload")]
    max_compressed_upload_size: u64,
    /// Uncompressed-size ceiling below which an upload is eligible for
    /// re-compression at `compression_level`. `0` disables re-compression.
    #[metric(help = "Max uncompressed size eligible for recompression")]
    max_recompression_size: u64,
    /// Compressed uploads at or below this size skip the staging file entirely.
    #[metric(help = "Max compressed upload size committed inline from memory")]
    max_inline_commit_size: u64,
    /// Bounds concurrent staged uploads (temp files being validated at once).
    staged_upload_semaphore: Arc<Semaphore>,
    /// Bounds concurrent identity (uncompressed) reads and writes, each of which
    /// occupies a blocking thread for the whole transfer.
    identity_semaphore: Arc<Semaphore>,
    /// Bounds concurrent re-compression passes. Acquired with `try_acquire` so a
    /// busy pool skips recompression instead of holding a staging slot.
    recompression_semaphore: Arc<Semaphore>,
    /// Total time one upload may spend being validated and staged, from the
    /// moment it is admitted. Unlike a per-message idle timeout, slow continuous
    /// progress does not reset it, so a trickling client cannot hold a staging
    /// slot open indefinitely.
    stage_timeout: Duration,
    /// Upper bound on how long the inner-store commit (and any recompression)
    /// of a staged upload may take before it fails with `DeadlineExceeded`,
    /// releasing the staging permit and removing the temp file.
    commit_timeout: Duration,

    #[metric(help = "Compressed uploads accepted through the wire fast path")]
    wire_uploads: AtomicU64,
    #[metric(help = "Compressed wire bytes accepted through the wire fast path")]
    wire_upload_bytes: AtomicU64,
    #[metric(help = "Stored zstd streams served byte-for-byte to clients")]
    wire_downloads: AtomicU64,
    #[metric(help = "Batch reads answered with stored zstd bytes")]
    batch_zstd_passthroughs: AtomicU64,
    #[metric(help = "Batch reads answered with decoded identity bytes")]
    batch_identity_decodes: AtomicU64,
    #[metric(help = "Compressed uploads committed inline with no staging file")]
    inline_commits: AtomicU64,
    #[metric(help = "Compressed uploads committed from a staging file")]
    staged_commits: AtomicU64,
    #[metric(help = "Compressed uploads currently validating or staging")]
    staged_uploads_inflight: AtomicU64,
    #[metric(help = "Identity reads and writes currently in flight")]
    identity_ops_inflight: AtomicU64,
    #[metric(help = "Recompressions that produced a smaller stream and were kept")]
    recompressions_applied: AtomicU64,
    #[metric(help = "Recompressions that were not smaller and were discarded")]
    recompressions_rejected: AtomicU64,
    #[metric(help = "Recompressions skipped because every slot was busy")]
    recompressions_skipped_busy: AtomicU64,
    #[metric(help = "Uploads failed by the validate-and-stage deadline")]
    stage_timeouts: AtomicU64,
    #[metric(help = "Uploads failed by the recompress-and-commit deadline")]
    commit_timeouts: AtomicU64,
}

impl core::fmt::Debug for ZstdStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ZstdStore")
            .field("inner_store", &self.inner_store)
            .field("compression_level", &self.compression_level)
            .finish_non_exhaustive()
    }
}

impl ZstdStore {
    pub fn new(spec: &ZstdConfig, inner_store: Store) -> Result<Arc<Self>, Error> {
        if let Some(level) = spec.compression_level
            && !(1..=19).contains(&level)
        {
            return Err(make_input_err!(
                "ZstdStore compression_level must be in [1, 19], got {level}"
            ));
        }
        if spec.temp_path.is_empty() {
            return Err(make_input_err!("ZstdStore requires a non-empty temp_path"));
        }
        if spec.max_compressed_upload_size == 0 {
            return Err(make_input_err!(
                "ZstdStore requires a positive max_compressed_upload_size"
            ));
        }
        // Recompression re-encodes at `compression_level`, so a ceiling without
        // a level would silently do nothing. Reject it instead.
        if spec.max_recompression_size > 0 && spec.compression_level.is_none() {
            return Err(make_input_err!(
                "ZstdStore max_recompression_size requires compression_level to be set"
            ));
        }
        let nonzero_or = |configured: usize, default: usize| match configured {
            0 => default,
            n => n,
        };
        Ok(Arc::new(Self {
            inner_store,
            temp_path: spec.temp_path.clone(),
            compression_level: spec.compression_level,
            max_compressed_upload_size: spec.max_compressed_upload_size,
            max_recompression_size: spec.max_recompression_size,
            max_inline_commit_size: match spec.max_inline_commit_size {
                0 => DEFAULT_MAX_INLINE_COMMIT_SIZE,
                n => n,
            },
            staged_upload_semaphore: Arc::new(Semaphore::new(nonzero_or(
                spec.max_concurrent_staged_uploads,
                DEFAULT_MAX_CONCURRENT_STAGED_UPLOADS,
            ))),
            identity_semaphore: Arc::new(Semaphore::new(nonzero_or(
                spec.max_concurrent_identity_ops,
                DEFAULT_MAX_CONCURRENT_IDENTITY_OPS,
            ))),
            recompression_semaphore: Arc::new(Semaphore::new(nonzero_or(
                spec.max_concurrent_recompressions,
                DEFAULT_MAX_CONCURRENT_RECOMPRESSIONS,
            ))),
            stage_timeout: Duration::from_secs(match spec.stage_timeout_s {
                0 => DEFAULT_STAGE_TIMEOUT_S,
                n => n,
            }),
            commit_timeout: Duration::from_secs(match spec.commit_timeout_s {
                0 => DEFAULT_COMMIT_TIMEOUT_S,
                n => n,
            }),
            wire_uploads: AtomicU64::new(0),
            wire_upload_bytes: AtomicU64::new(0),
            wire_downloads: AtomicU64::new(0),
            batch_zstd_passthroughs: AtomicU64::new(0),
            batch_identity_decodes: AtomicU64::new(0),
            inline_commits: AtomicU64::new(0),
            staged_commits: AtomicU64::new(0),
            staged_uploads_inflight: AtomicU64::new(0),
            identity_ops_inflight: AtomicU64::new(0),
            recompressions_applied: AtomicU64::new(0),
            recompressions_rejected: AtomicU64::new(0),
            recompressions_skipped_busy: AtomicU64::new(0),
            stage_timeouts: AtomicU64::new(0),
            commit_timeouts: AtomicU64::new(0),
        }))
    }

    #[inline]
    fn encode_level(&self) -> i32 {
        self.compression_level.unwrap_or(DEFAULT_ENCODE_LEVEL)
    }

    /// `Some(limit)` when an upload's decoded bytes should be buffered for a
    /// recompression attempt. `new` guarantees a level is configured whenever
    /// the ceiling is positive.
    #[inline]
    const fn recompression_limit(&self) -> Option<u64> {
        if self.max_recompression_size > 0 {
            Some(self.max_recompression_size)
        } else {
            None
        }
    }

    async fn acquire_staging_permit(&self) -> Result<OwnedSemaphorePermit, Error> {
        self.staged_upload_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| make_err!(Code::Internal, "Staged upload semaphore closed: {e}"))
    }

    async fn acquire_identity_permit(&self) -> Result<OwnedSemaphorePermit, Error> {
        self.identity_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| make_err!(Code::Internal, "Identity semaphore closed: {e}"))
    }

    async fn update_identity(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
    ) -> Result<u64, Error> {
        let StoreKey::Digest(digest) = key else {
            return Err(make_input_err!(
                "ZstdStore only supports digest keys, got {key:?}"
            ));
        };

        if is_zero_digest(digest) {
            return reader.recv().await.and_then(|chunk| {
                if chunk.is_empty() {
                    Ok(0)
                } else {
                    Err(make_err!(Code::Internal, "Zero byte hash was not empty"))
                }
            });
        }

        // The encode below parks a blocking thread for the whole upload, so it
        // must be admitted rather than allowed to exhaust the shared pool.
        let _permit = self.acquire_identity_permit().await?;
        let _inflight = Inflight::enter(&self.identity_ops_inflight);

        let (tx, rx) = make_buf_channel_pair();
        let inner_store = self.inner_store.clone();
        let max_output_size = zstd_compress_bound(digest.size_bytes());
        let update_fut = spawn!("zstd_store_update_spawn", async move {
            inner_store
                .update(digest, rx, UploadSizeInfo::MaxSize(max_output_size))
                .await
                .err_tip(|| "Inner store update in zstd store failed")
        })
        .map(|joined| flatten_join(joined, "Failed to run zstd store update spawn"));

        let level = self.encode_level();
        let hasher_func = digest_hasher_func_from_context();
        let encode_fut = spawn_blocking!("zstd_store_encode_identity", move || {
            encode_identity(reader, tx, level, hasher_func, digest)
        })
        .map(|joined| flatten_join(joined, "Failed to run zstd store encode task"));

        let (encode_res, update_res) = tokio::join!(encode_fut, update_fut);
        match (encode_res, update_res) {
            (Ok(size), Ok(_)) => Ok(size),
            (Err(e), _) | (_, Err(e)) => Err(e),
        }
    }

    async fn get_part_identity(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        let StoreKey::Digest(digest) = key else {
            return Err(make_input_err!(
                "ZstdStore only supports digest keys, got {key:?}"
            ));
        };

        if is_zero_digest(digest) {
            writer
                .send_eof()
                .err_tip(|| "Failed to send zero-digest EOF in zstd store get_part")?;
            return Ok(());
        }

        // The decode below parks a blocking thread until the client has consumed
        // the whole blob, so it must be admitted like an upload.
        let _permit = self.acquire_identity_permit().await?;
        let _inflight = Inflight::enter(&self.identity_ops_inflight);

        let (physical_tx, physical_rx) = make_buf_channel_pair();
        let inner_store = self.inner_store.clone();
        let get_fut = spawn!("zstd_store_get_part_spawn", async move {
            inner_store
                .get_part(digest, physical_tx, 0, None)
                .await
                .err_tip(|| "Inner store get in zstd store failed")
        })
        .map(|joined| flatten_join(joined, "Failed to run zstd store get spawn"));

        let (raw_tx, mut raw_rx) = make_buf_channel_pair();
        let decode_fut = spawn_blocking!("zstd_store_decode_identity", move || {
            decode_identity(physical_rx, raw_tx, offset, length)
        })
        .map(|joined| flatten_join(joined, "Failed to run zstd store decode task"));

        let pump_fut = async move {
            loop {
                let chunk = raw_rx
                    .recv()
                    .await
                    .err_tip(|| "Failed to read decoded chunk in zstd store get_part")?;
                if chunk.is_empty() {
                    break;
                }
                writer
                    .send(chunk)
                    .await
                    .err_tip(|| "Failed to send chunk in zstd store get_part")?;
            }
            writer
                .send_eof()
                .err_tip(|| "Failed to send EOF in zstd store get_part")?;
            Result::<(), Error>::Ok(())
        };

        let (get_res, decode_res, pump_res) = tokio::join!(get_fut, decode_fut, pump_fut);
        // Prioritize the inner get error (e.g. NotFound) as the root cause, then
        // the decode error (DataLoss on corrupt data), then the pump error.
        get_res?;
        decode_res?;
        pump_res?;
        Ok(())
    }

    /// Byte-for-byte passthrough of the stored zstd stream. Always emits zstd:
    /// zero digests are answered with the canonical empty encoding.
    pub async fn get_zstd(
        &self,
        digest: DigestInfo,
        mut writer: DropCloserWriteHalf,
    ) -> Result<(), Error> {
        if is_zero_digest(digest) {
            writer
                .send(empty_zstd_frame())
                .await
                .err_tip(|| "Failed to send empty zstd in zstd store get_zstd")?;
            writer
                .send_eof()
                .err_tip(|| "Failed to send EOF in zstd store get_zstd")?;
            return Ok(());
        }
        // The physical bytes are already zstd; pipe them straight through.
        self.inner_store
            .get_part(digest, writer, 0, None)
            .await
            .err_tip(|| "Inner store get_part in zstd store get_zstd failed")?;
        self.wire_downloads.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Accept a client-supplied compressed (zstd) stream: validate, stage,
    /// optionally re-compress, then commit. Returns the number of compressed
    /// wire bytes consumed.
    pub async fn update_zstd(
        &self,
        digest: DigestInfo,
        digest_function: DigestHasherFunc,
        reader: DropCloserReadHalf,
    ) -> Result<u64, Error> {
        if is_zero_digest(digest) {
            return self.validate_empty_zstd(digest_function, reader).await;
        }
        let wire_bytes = self
            .update_staged_zstd(digest, digest_function, reader)
            .await?;
        self.record_wire_upload(wire_bytes);
        Ok(wire_bytes)
    }

    /// Whole-`Bytes` variant of [`ZstdStore::update_zstd`] for `BatchUpdate`.
    pub async fn update_zstd_oneshot(
        &self,
        digest: DigestInfo,
        digest_function: DigestHasherFunc,
        data: Bytes,
    ) -> Result<u64, Error> {
        // Zero digests must not be whole-buffer decoded by an allocating
        // decoder, so they go through the same bounded decoder as everything
        // else — just fed from memory instead of a channel.
        if is_zero_digest(digest) {
            return self
                .validate_empty_zstd(digest_function, OneChunk(Some(data)))
                .await;
        }

        let wire_bytes = if data.len() as u64 <= self.max_inline_commit_size {
            self.update_inline_zstd(digest, digest_function, data)
                .await?
        } else {
            self.update_large_oneshot_zstd(digest, digest_function, data)
                .await?
        };
        self.record_wire_upload(wire_bytes);
        Ok(wire_bytes)
    }

    fn record_wire_upload(&self, wire_bytes: u64) {
        self.wire_uploads.fetch_add(1, Ordering::Relaxed);
        self.wire_upload_bytes
            .fetch_add(wire_bytes, Ordering::Relaxed);
    }

    /// Batch read selection: return `(data, is_zstd)`. Prefers the physical
    /// zstd bytes when the client accepts zstd and compression actually helped;
    /// otherwise decodes to raw.
    pub async fn get_for_batch(
        &self,
        digest: DigestInfo,
        client_accepts_zstd: bool,
    ) -> Result<(Bytes, bool), Error> {
        if is_zero_digest(digest) {
            return Ok((Bytes::new(), false));
        }
        let physical = self
            .inner_store
            .get_part_unchunked(digest, 0, None)
            .await
            .err_tip(|| "Inner store get in zstd store get_for_batch failed")?;
        if client_accepts_zstd && (physical.len() as u64) < digest.size_bytes() {
            self.batch_zstd_passthroughs.fetch_add(1, Ordering::Relaxed);
            return Ok((physical, true));
        }
        let expected_size = usize::try_from(digest.size_bytes())
            .map_err(|_| make_err!(Code::Internal, "Digest size too large for this platform"))?;
        let raw = flatten_join(
            spawn_blocking!("zstd_store_batch_decode", move || {
                // Stored data was validated at upload time, and
                // `bulk::decompress` bounds its output by `expected_size`, so a
                // failure here is corruption of already-stored bytes.
                zstd::bulk::decompress(&physical, expected_size)
                    .map_err(|e| make_err!(Code::DataLoss, "Zstd decode failed in zstd store: {e}"))
            })
            .await,
            "Failed to run zstd store batch decode task",
        )?;
        self.batch_identity_decodes.fetch_add(1, Ordering::Relaxed);
        Ok((Bytes::from(raw), false))
    }

    /// Validate that a zero-digest client stream decodes to empty without
    /// touching the inner store or allocating the decoded output. Takes a
    /// staging permit and the staging deadline, so a flood of zero-digest
    /// streams cannot spawn unbounded blocking decode jobs or park them.
    /// Returns the compressed wire bytes consumed.
    async fn validate_empty_zstd<S: ChunkSource + Send + 'static>(
        &self,
        digest_function: DigestHasherFunc,
        source: S,
    ) -> Result<u64, Error> {
        let _permit = self.acquire_staging_permit().await?;
        let _inflight = Inflight::enter(&self.staged_uploads_inflight);
        let max_compressed_upload_size = self.max_compressed_upload_size;
        let validate_fut = spawn_blocking!("zstd_store_validate_empty", move || {
            validate_bounded_zstd_blocking(source, digest_function, 0, max_compressed_upload_size)
        });
        match tokio::time::timeout(self.stage_timeout, validate_fut).await {
            Ok(joined) => flatten_join(joined, "Failed to run zstd store empty validation task"),
            Err(_elapsed) => Err(self.stage_timeout_err()),
        }
    }

    fn stage_timeout_err(&self) -> Error {
        self.stage_timeouts.fetch_add(1, Ordering::Relaxed);
        make_err!(
            Code::DeadlineExceeded,
            "zstd store validation and staging exceeded stage_timeout_s ({}s); cleaning up",
            self.stage_timeout.as_secs()
        )
    }

    /// Bound the post-validation recompression and commit of an already-staged
    /// or already-validated upload.
    async fn with_commit_timeout<F>(&self, commit: F) -> Result<u64, Error>
    where
        F: Future<Output = Result<u64, Error>>,
    {
        match tokio::time::timeout(self.commit_timeout, commit).await {
            Ok(result) => result,
            Err(_elapsed) => {
                self.commit_timeouts.fetch_add(1, Ordering::Relaxed);
                Err(make_err!(
                    Code::DeadlineExceeded,
                    "zstd store recompression or commit timed out after {}s; cleaning up staged files",
                    self.commit_timeout.as_secs()
                ))
            }
        }
    }

    /// Validate and stage a non-zero-digest compressed stream, then commit it.
    /// The blocking staging task owns its cleanup guard, so cancellation while
    /// it is detached still removes its temp file once it stops.
    async fn update_staged_zstd(
        &self,
        digest: DigestInfo,
        digest_function: DigestHasherFunc,
        reader: DropCloserReadHalf,
    ) -> Result<u64, Error> {
        let mut staged = self
            .stage_compressed(digest, digest_function, reader)
            .await?;
        let wire_bytes_consumed = staged.wire_bytes_consumed;
        self.with_commit_timeout(async {
            self.recompress_staged(&mut staged).await?;
            self.commit_staged(digest, staged).await?;
            Ok(wire_bytes_consumed)
        })
        .await
    }

    /// Oneshot payload too large to hold a second copy of in memory: feed it
    /// through the same bounded streaming staging path as a client stream.
    async fn update_large_oneshot_zstd(
        &self,
        digest: DigestInfo,
        digest_function: DigestHasherFunc,
        data: Bytes,
    ) -> Result<u64, Error> {
        let (mut tx, rx) = make_buf_channel_pair();
        let feed_fut = spawn!("zstd_store_oneshot_feed", async move {
            if !data.is_empty() {
                tx.send(data)
                    .await
                    .err_tip(|| "Failed to feed oneshot data in zstd store")?;
            }
            tx.send_eof()
                .err_tip(|| "Failed to send oneshot EOF in zstd store")?;
            Result::<(), Error>::Ok(())
        });

        let stage_res = self.update_staged_zstd(digest, digest_function, rx).await;
        let feed_res = flatten_join(feed_fut.await, "Failed to run zstd store oneshot feed task");
        // Staging owns the meaningful error: rejecting an upload drops the
        // reader, which makes the feeder fail too. Only surface the feeder's
        // error when staging itself succeeded.
        let wire_bytes = stage_res?;
        feed_res?;
        Ok(wire_bytes)
    }

    /// Validate and commit a small compressed upload straight from memory: no
    /// staging file and no `fsync`. `BatchUpdateBlobs` payloads are small and
    /// numerous, so a per-blob disk round trip would dominate their cost.
    async fn update_inline_zstd(
        &self,
        digest: DigestInfo,
        digest_function: DigestHasherFunc,
        data: Bytes,
    ) -> Result<u64, Error> {
        let _permit = self.acquire_staging_permit().await?;
        let _inflight = Inflight::enter(&self.staged_uploads_inflight);

        let max_compressed_upload_size = self.max_compressed_upload_size;
        let collect_limit = self.recompression_limit();
        // `Bytes` is refcounted, so the validation copy is free.
        let to_validate = data.clone();
        let validate_fut = spawn_blocking!("zstd_store_validate_inline", move || {
            validate_zstd_buffer(
                to_validate,
                digest,
                digest_function,
                max_compressed_upload_size,
                collect_limit,
            )
        });
        let (wire_bytes_consumed, collected) =
            match tokio::time::timeout(self.stage_timeout, validate_fut).await {
                Ok(joined) => flatten_join(joined, "Failed to run zstd store inline validation")?,
                Err(_elapsed) => return Err(self.stage_timeout_err()),
            };

        self.with_commit_timeout(async {
            let mut payload = data;
            if let Some(collected) = collected
                && let Some(smaller) = self
                    .maybe_recompress(collected, payload.len() as u64)
                    .await?
            {
                payload = Bytes::from(smaller);
            }
            self.inner_store
                .update_oneshot(digest, payload)
                .await
                .err_tip(|| "Failed to commit inline zstd upload to inner store")?;
            self.inline_commits.fetch_add(1, Ordering::Relaxed);
            Ok(wire_bytes_consumed)
        })
        .await
    }

    /// Stage a validated compressed upload to a temp file. The returned
    /// [`StagedStream`] owns the cleanup guard and staging permit.
    async fn stage_compressed(
        &self,
        digest: DigestInfo,
        digest_function: DigestHasherFunc,
        reader: DropCloserReadHalf,
    ) -> Result<StagedStream, Error> {
        let permit = self.acquire_staging_permit().await?;
        let _inflight = Inflight::enter(&self.staged_uploads_inflight);

        let stage_path = format!("{}/zstd-stage-{}", self.temp_path, uuid::Uuid::new_v4());
        // Transfer an already-armed guard to the detached blocking task. If this
        // async future is cancelled (including by `stage_timeout`) while that
        // task is still creating or validating the file, the task keeps owning
        // cleanup until it finishes; the guard comes back only on success.
        let guard = TempFileGuard::arm(stage_path.clone());

        let fs_permit = fs::get_permit().await?;
        let max_compressed_upload_size = self.max_compressed_upload_size;
        let collect_limit = self.recompression_limit();
        let blocking_path = stage_path.clone();
        let stage_fut = spawn_blocking!("zstd_store_stage", move || {
            stage_compressed_blocking(StageCompressedInput {
                reader,
                path: blocking_path,
                digest,
                hasher_func: digest_function,
                max_compressed_upload_size,
                collect_limit,
                fs_permit,
                guard,
            })
        });
        let output = match tokio::time::timeout(self.stage_timeout, stage_fut).await {
            Ok(joined) => flatten_join(joined, "Failed to run zstd store staging task")?,
            Err(_elapsed) => return Err(self.stage_timeout_err()),
        };

        Ok(StagedStream {
            file: Some(output.file),
            path: stage_path,
            compressed_size: output.wire_bytes_consumed,
            wire_bytes_consumed: output.wire_bytes_consumed,
            collected: output.collected,
            _permit: permit,
            _guard: output.guard,
        })
    }

    /// Re-encode `collected` at the configured level, returning it only if it is
    /// smaller than `current_size`.
    ///
    /// Best-effort by design: an upload that finds every recompression slot busy
    /// returns `None` rather than queueing, because it is holding a staging
    /// permit and queueing here would let a small recompression pool throttle
    /// the whole upload path.
    async fn maybe_recompress(
        &self,
        collected: Vec<u8>,
        current_size: u64,
    ) -> Result<Option<Vec<u8>>, Error> {
        let Ok(_rec_permit) = self.recompression_semaphore.clone().try_acquire_owned() else {
            self.recompressions_skipped_busy
                .fetch_add(1, Ordering::Relaxed);
            return Ok(None);
        };
        let level = self.encode_level();
        let recompressed = flatten_join(
            spawn_blocking!("zstd_store_recompress", move || {
                zstd::bulk::compress(&collected, level).map_err(|e| {
                    make_err!(
                        Code::Internal,
                        "Zstd re-compression failed in zstd store: {e}"
                    )
                })
            })
            .await,
            "Failed to run zstd store recompress task",
        )?;
        if (recompressed.len() as u64) < current_size {
            self.recompressions_applied.fetch_add(1, Ordering::Relaxed);
            Ok(Some(recompressed))
        } else {
            self.recompressions_rejected.fetch_add(1, Ordering::Relaxed);
            Ok(None)
        }
    }

    /// Overwrite the staged file with a smaller re-encoding when one is
    /// available. The validated descriptor stays open and is reused, so this
    /// needs neither a second file permit (which could deadlock under
    /// exhaustion) nor a reopen of the staged pathname.
    async fn recompress_staged(&self, staged: &mut StagedStream) -> Result<(), Error> {
        let Some(collected) = staged.collected.take() else {
            return Ok(());
        };
        let Some(recompressed) = self
            .maybe_recompress(collected, staged.compressed_size)
            .await?
        else {
            return Ok(());
        };

        let file = staged
            .file
            .as_mut()
            .ok_or_else(|| make_err!(Code::Internal, "Staged zstd file already consumed"))?;
        file.as_ref()
            .set_len(0)
            .await
            .map_err(stage_file_err("truncate"))?;
        file.seek(SeekFrom::Start(0))
            .await
            .map_err(stage_file_err("rewind"))?;
        file.write_all(&recompressed)
            .await
            .map_err(stage_file_err("write"))?;
        file.flush().await.map_err(stage_file_err("flush"))?;
        file.as_ref()
            .sync_all()
            .await
            .map_err(stage_file_err("sync"))?;
        file.seek(SeekFrom::Start(0))
            .await
            .map_err(stage_file_err("rewind"))?;
        staged.compressed_size = recompressed.len() as u64;
        Ok(())
    }

    /// Commit a validated staged upload to the inner store. Only called after
    /// validation succeeded, so the inner store never sees corrupt data.
    /// Consumes `staged`, so the staging permit and temp file are released and
    /// removed once the commit resolves.
    ///
    /// Both the validated descriptor and its pathname are handed over, and which
    /// one the backend uses is backend-specific: stores that stream the bytes
    /// (the default `StoreDriver::update_with_whole_file`, so `memory`, S3, …)
    /// read the exact descriptor that was validated, while `filesystem` drops it
    /// and commits by `rename(2)` on the path. For the latter, the guarantee
    /// against an observe-and-replace race comes from `O_EXCL` creation of an
    /// unguessable name inside an operator-private `temp_path` — enforced by
    /// `post_init` — not from descriptor pinning. That backend also requires
    /// `temp_path` and its `content_path` to share a filesystem; a cross-device
    /// `rename` fails with `EXDEV` and the upload is rejected.
    async fn commit_staged(
        &self,
        digest: DigestInfo,
        mut staged: StagedStream,
    ) -> Result<(), Error> {
        let file = staged
            .file
            .take()
            .ok_or_else(|| make_err!(Code::Internal, "Staged zstd file already consumed"))?;
        let compressed_size = staged.compressed_size;
        let path = OsString::from(staged.path.clone());
        self.inner_store
            .update_with_whole_file(
                digest,
                path,
                file,
                UploadSizeInfo::ExactSize(compressed_size),
            )
            .await
            .err_tip(|| "Failed to commit staged zstd upload to inner store")?;
        self.staged_commits.fetch_add(1, Ordering::Relaxed);
        Ok(())
        // `staged` (permit + cleanup guard) drops here on every path.
    }
}

#[async_trait]
impl WireCompressionStore for ZstdStore {
    async fn update_compressed(
        self: Arc<Self>,
        digest: DigestInfo,
        digest_function: DigestHasherFunc,
        compressor: WireCompressor,
        reader: DropCloserReadHalf,
    ) -> Result<u64, Error> {
        match compressor {
            WireCompressor::Zstd => self.update_zstd(digest, digest_function, reader).await,
        }
    }

    async fn get_compressed(
        self: Arc<Self>,
        digest: DigestInfo,
        compressor: WireCompressor,
        writer: DropCloserWriteHalf,
    ) -> Result<(), Error> {
        match compressor {
            WireCompressor::Zstd => self.get_zstd(digest, writer).await,
        }
    }

    async fn update_compressed_oneshot(
        self: Arc<Self>,
        digest: DigestInfo,
        digest_function: DigestHasherFunc,
        compressor: WireCompressor,
        data: Bytes,
    ) -> Result<(), Error> {
        match compressor {
            WireCompressor::Zstd => self
                .update_zstd_oneshot(digest, digest_function, data)
                .await
                .map(|_| ()),
        }
    }

    async fn get_for_batch(
        self: Arc<Self>,
        digest: DigestInfo,
        acceptable_compressors: &[WireCompressor],
    ) -> Result<(Bytes, Option<WireCompressor>), Error> {
        let accepts_zstd = acceptable_compressors.contains(&WireCompressor::Zstd);
        let (data, is_zstd) = Self::get_for_batch(self.as_ref(), digest, accepts_zstd).await?;
        Ok((data, is_zstd.then_some(WireCompressor::Zstd)))
    }
}

#[async_trait]
impl StoreDriver for ZstdStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        tokio::fs::create_dir_all(&self.temp_path)
            .await
            .map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Failed to create ZstdStore temp_path {}: {e}",
                    self.temp_path
                )
            })?;
        let meta = tokio::fs::metadata(&self.temp_path).await.map_err(|e| {
            make_err!(
                Code::Internal,
                "Failed to stat ZstdStore temp_path {}: {e}",
                self.temp_path
            )
        })?;
        if !meta.is_dir() {
            return Err(make_input_err!(
                "ZstdStore temp_path {} is not a directory",
                self.temp_path
            ));
        }
        #[cfg(unix)]
        {
            // Staged files hold validated-but-uncommitted blob contents, and a
            // `filesystem` backend commits them by pathname, so the directory
            // itself must not be writable by untrusted local users. A sticky
            // world-writable dir (like /tmp) restricts deletes/renames to the
            // owner, so it is tolerated.
            let mode = meta.permissions().mode();
            if mode & 0o002 != 0 && mode & 0o1000 == 0 {
                return Err(make_input_err!(
                    "ZstdStore temp_path {} is world-writable without the sticky bit (mode {:o}); \
                     use an operator-private directory not writable by untrusted users",
                    self.temp_path,
                    mode & 0o7777
                ));
            }
        }
        // Probe writability at startup rather than discovering it on the first
        // upload.
        let probe_path = format!(
            "{}/.zstd-store-probe-{}",
            self.temp_path,
            uuid::Uuid::new_v4()
        );
        tokio::fs::write(&probe_path, [0u8]).await.map_err(|e| {
            make_err!(
                Code::Internal,
                "ZstdStore temp_path {} is not writable: {e}",
                self.temp_path
            )
        })?;
        tokio::fs::remove_file(&probe_path).await.map_err(|e| {
            make_err!(
                Code::Internal,
                "Failed to remove ZstdStore write-probe file {probe_path}: {e}"
            )
        })?;
        self.inner_store.clone().into_inner().post_init().await
    }

    fn wire_compression_store(self: Arc<Self>) -> Option<Arc<dyn WireCompressionStore>> {
        Some(self)
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        // ZstdStore is CAS-only.
        for key in keys {
            if let StoreKey::Str(_) = key {
                return Err(make_input_err!("ZstdStore only supports digest keys"));
            }
        }

        // Invariant: zero digests never touch the inner store. Satisfy them
        // directly and only forward the non-zero keys to the inner store.
        for (key, result) in keys.iter().zip(results.iter_mut()) {
            if is_zero_digest(key.borrow()) {
                *result = Some(0);
            }
        }

        let nonzero_keys = keys
            .iter()
            .filter(|key| !is_zero_digest(key.borrow()))
            .map(StoreKey::borrow)
            .collect::<Vec<_>>();
        if nonzero_keys.is_empty() {
            return Ok(());
        }

        let mut nonzero_results = vec![None; nonzero_keys.len()];
        self.inner_store
            .as_store_driver_pin()
            .has_with_results(&nonzero_keys, &mut nonzero_results)
            .await?;

        // Presence comes from the inner store, but the reported size is always
        // the digest's uncompressed size (the physical zstd size is meaningless
        // to clients).
        let nonzero_slots = keys
            .iter()
            .zip(results.iter_mut())
            .filter_map(|(key, result)| (!is_zero_digest(key.borrow())).then_some((key, result)));
        for ((key, result), inner_result) in nonzero_slots.zip(nonzero_results) {
            *result = match (inner_result, key) {
                (Some(_), StoreKey::Digest(digest)) => Some(digest.size_bytes()),
                (other, _) => other,
            };
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        _upload_size: UploadSizeInfo,
    ) -> Result<u64, Error> {
        self.update_identity(key, reader).await
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        self.get_part_identity(key, writer, offset, length).await
    }

    fn inner_store(&self, _key: Option<StoreKey>) -> &dyn StoreDriver {
        // Representation-changing store => terminal.
        self
    }

    fn as_any(&self) -> &(dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_remove_callback(self: Arc<Self>, callback: RemoveCallback) -> Result<(), Error> {
        self.inner_store.register_remove_callback(callback)
    }
}

default_health_status_indicator!(ZstdStore);

#[cfg(test)]
mod tests {
    use std::io::Write;

    use nativelink_util::digest_hasher::DigestHasherFunc;

    use super::{DecodeSink, create_temp_exclusive};
    use crate::cas_utils::is_zero_digest;

    fn unique_temp_path(tag: &str) -> std::path::PathBuf {
        // Process- and call-unique name under the system temp dir. `Math`-style
        // randomness is unavailable in some sandboxes, so combine the pid with a
        // monotonic counter instead of a clock.
        use core::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "nativelink-zstd-unit-{tag}-{}-{n}",
            std::process::id()
        ))
    }

    #[test]
    fn create_temp_exclusive_rejects_existing_path() {
        let path = unique_temp_path("exists");
        let path_str = path.to_str().unwrap();
        let first = create_temp_exclusive(path_str).expect("first exclusive create must succeed");
        drop(first);
        let err = create_temp_exclusive(path_str)
            .expect_err("second exclusive create on an existing path must fail");
        assert!(
            err.to_string().contains("exclusively create"),
            "unexpected error: {err}"
        );
        drop(std::fs::remove_file(path_str));
    }

    #[cfg(unix)]
    #[test]
    fn create_temp_exclusive_rejects_symlink() {
        let target = unique_temp_path("symlink-target");
        let link = unique_temp_path("symlink-link");
        let link_str = link.to_str().unwrap();
        // A dangling symlink at the path: O_EXCL must refuse to create through it.
        std::os::unix::fs::symlink(&target, &link).expect("symlink must be created");
        let err = create_temp_exclusive(link_str)
            .expect_err("exclusive create through a symlink must fail");
        assert!(
            err.to_string().contains("exclusively create"),
            "unexpected error: {err}"
        );
        // The symlink target must never have been created (no follow).
        assert!(!target.exists(), "O_EXCL must not follow the symlink");
        drop(std::fs::remove_file(link_str));
    }

    #[cfg(unix)]
    #[test]
    fn create_temp_exclusive_sets_owner_only_mode() {
        use std::os::unix::fs::PermissionsExt;
        let path = unique_temp_path("mode");
        let path_str = path.to_str().unwrap();
        let file = create_temp_exclusive(path_str).expect("exclusive create must succeed");
        let mode = file.metadata().unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "staging file must be created 0o600, got {mode:o}"
        );
        drop(file);
        drop(std::fs::remove_file(path_str));
    }

    #[test]
    fn decode_sink_rejects_output_past_max_and_never_overcounts() {
        // Cap at 4 bytes; a write that would exceed it is rejected wholesale and
        // must not advance the counter or hash any of the offending bytes.
        let mut sink = DecodeSink::new(DigestHasherFunc::Sha256, 4, None);
        assert_eq!(
            sink.write(b"abcd").unwrap(),
            4,
            "exact-fit write is accepted"
        );
        assert_eq!(sink.decoded_len, 4);
        let err = sink
            .write(b"e")
            .expect_err("a byte past the cap must be rejected at the sink");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(sink.decoded_len, 4, "rejected bytes must not be counted");
    }

    #[test]
    fn decode_sink_zero_cap_rejects_first_byte() {
        // The zero-digest configuration: any decoded byte is rejected.
        let mut sink = DecodeSink::new(DigestHasherFunc::Sha256, 0, None);
        assert!(
            sink.write(b"x").is_err(),
            "a zero-capacity sink must reject any output"
        );
        assert_eq!(sink.decoded_len, 0);
        // An empty write is a no-op success (decoders may flush zero bytes).
        assert_eq!(sink.write(b"").unwrap(), 0);
    }

    #[test]
    fn zero_byte_digests_are_recognized() {
        // Guards the invariant the zero-digest fast paths rely on.
        for digest in crate::cas_utils::ZERO_BYTE_DIGESTS {
            assert!(is_zero_digest(digest));
        }
    }
}
