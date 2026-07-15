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
use core::pin::Pin;
use std::ffi::OsString;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use bytes::Bytes;
use futures::future::FutureExt;
use nativelink_config::stores::ZstdStoreSpec;
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
use nativelink_util::fs;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    RemoveItemCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use nativelink_util::{spawn, spawn_blocking};
use tokio::sync::{OwnedSemaphorePermit, Semaphore, SemaphorePermit};

use crate::cas_utils::is_zero_digest;

/// The canonical zstd encoding of empty input at [`DEFAULT_ENCODE_LEVEL`],
/// computed once. Emitted verbatim for zero-digest `get_zstd` responses so the
/// client always receives a valid zstd stream that decodes to nothing.
fn empty_zstd_level3() -> Bytes {
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

/// Default number of concurrent staged uploads when the config value is `0`.
const DEFAULT_MAX_CONCURRENT_STAGED_UPLOADS: usize = 4;
/// Default number of concurrent recompressions when the config value is `0`.
const DEFAULT_MAX_CONCURRENT_RECOMPRESSIONS: usize = 1;
/// Level used when no `compression_level` is configured, matching the service
/// wire codec (`nativelink-service` `wire_compression::ZSTD_COMPRESSION_LEVEL`).
const DEFAULT_ENCODE_LEVEL: i32 = 3;

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

/// Whole-buffer zstd decode shared by the non-streaming paths. When `size_hint`
/// is `Some`, the single-frame bulk decoder is used with that value as a
/// capacity hint (the digest's uncompressed size); otherwise the multi-frame
/// streaming decoder is used with no size hint. A decode failure is mapped to an
/// [`Error`] by `on_err` so each caller keeps its own error code and message.
fn decode_all_zstd<F>(data: &[u8], size_hint: Option<usize>, on_err: F) -> Result<Vec<u8>, Error>
where
    F: FnOnce(std::io::Error) -> Error,
{
    match size_hint {
        Some(capacity) => zstd::bulk::decompress(data, capacity),
        None => zstd::stream::decode_all(data),
    }
    .map_err(on_err)
}

/// Streaming zstd encode of an identity (raw) upload.
///
/// Raw chunks are read from `reader`, hashed and counted, and fed through a
/// streaming zstd encoder whose compressed output is forwarded to the inner
/// store via `tx`. The inner store EOF is withheld until the finalized hash and
/// uncompressed size match the request `digest`; only then is the encoder
/// finished and EOF sent so the inner upload commits. On any mismatch an
/// `InvalidArgument` error is returned and EOF is never sent, so the inner
/// upload never commits.
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

    // Validate BEFORE committing to the inner store.
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

    // Validation passed: flush the final frame and commit the inner upload.
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

/// Streaming zstd decode of stored (physical) data back to raw bytes.
///
/// Compressed chunks are read from `physical_rx`, decoded, then `offset`
/// decoded bytes are discarded and at most `length` bytes are forwarded to
/// `raw_tx`. Any decode failure of already-stored data is a `DataLoss` error
/// (the data was accepted at upload time, so a failure here is corruption).
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

    // We must keep draining the decoder (and therefore the physical reader) all
    // the way to EOF even after the requested byte budget is satisfied. Stopping
    // early would drop `physical_rx` while the spawned inner `get` future is still
    // streaming compressed bytes into the buffered channel; its next `send` would
    // then fail with "receiver disconnected" and the inner get would resolve to an
    // error even though the requested bytes were produced correctly. This mirrors
    // `compression_store::get_part`, which likewise consumes the inner stream to
    // EOF and merely stops *sending* to the writer once the budget is met.
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

/// RAII guard that best-effort removes staged temp files when dropped. Because
/// staged files are pure scratch (the inner store copies the bytes on commit),
/// they are always removed — on success, error, or cancellation — so nothing
/// ever lingers in `temp_path`.
#[derive(Default)]
struct TempFileGuard {
    paths: Vec<String>,
}

impl TempFileGuard {
    fn add(&mut self, path: String) {
        self.paths.push(path);
    }
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        for path in &self.paths {
            // Best-effort: the file may already have been removed (e.g. the
            // losing recompression candidate). Ignore failures.
            drop(std::fs::remove_file(path));
        }
    }
}

/// A `std::io::Write` sink for the decoded output of a staged upload. It never
/// stores the full decoded stream unless re-compression is enabled; instead it
/// hashes and counts every decoded byte. When `collect` is `Some`, decoded
/// bytes are additionally buffered up to `collect_limit`; if that limit is
/// exceeded the buffer is dropped (the upload is too large to re-compress).
struct DecodeSink {
    hasher: DigestHasherImpl,
    decoded_len: u64,
    collect: Option<Vec<u8>>,
    collect_limit: u64,
}

impl Write for DecodeSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.hasher.update(buf);
        self.decoded_len = self.decoded_len.saturating_add(buf.len() as u64);
        if let Some(collected) = self.collect.as_mut() {
            if self.decoded_len <= self.collect_limit {
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

/// Result of the blocking validation pass over a staged compressed upload.
struct StageOutput {
    /// Physical (compressed) bytes written to the temp file.
    compressed_size: u64,
    /// Compressed wire bytes consumed from the client.
    wire_bytes_consumed: u64,
    /// Decoded content, present only when re-compression is eligible (decoded
    /// length stayed within the configured `max_recompression_size`).
    collected: Option<Vec<u8>>,
}

/// A validated, committed-ready staged upload. Dropping it removes the temp
/// file(s) and releases the staging permit.
struct StagedStream {
    /// Path of the chosen temp file to commit from.
    path: String,
    /// Physical (compressed) size of the chosen stream.
    compressed_size: u64,
    /// Compressed wire bytes actually consumed from the client.
    wire_bytes_consumed: u64,
    _permit: OwnedSemaphorePermit,
    _guard: TempFileGuard,
}

/// Creates an owner-only (0o600) temp file at `path` and closes it, leaving an
/// empty file on disk. Callers reopen it inside `spawn_blocking!` for the
/// actual streaming write; opening without `create` there means a concurrent
/// cancellation (which removes the file via the guard) is observed as an open
/// error rather than silently recreating a leftover file.
async fn create_empty_temp_file(path: &str) -> Result<(), Error> {
    let slot = fs::create_file(path)
        .await
        .err_tip(|| format!("Failed to create zstd staging file {path}"))?;
    drop(slot);
    // Owner-only permissions are enforced on unix only (NativeLink's supported
    // platform); on non-unix the file inherits whatever default permissions
    // the platform applies to newly created files.
    #[cfg(unix)]
    fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .await
        .err_tip(|| format!("Failed to set permissions on zstd staging file {path}"))?;
    Ok(())
}

/// Blocking validation pass: stream compressed chunks from `reader`, writing the
/// raw compressed bytes to the temp file at `path` while decoding them to
/// recompute the uncompressed length and hash. Enforces the compressed-size cap
/// (`ResourceExhausted`) and, at EOF, that the decoded length and hash match the
/// requested `digest` (`InvalidArgument`). Only runs from within
/// `spawn_blocking!` because it blocks on channel receives and the zstd codec.
fn stage_compressed_blocking(
    mut reader: DropCloserReadHalf,
    path: &str,
    digest: DigestInfo,
    hasher_func: DigestHasherFunc,
    max_compressed_upload_size: u64,
    collect_limit: Option<u64>,
    _fs_permit: SemaphorePermit<'static>,
) -> Result<StageOutput, Error> {
    let expected_size = digest.size_bytes();
    // Open the pre-created file without `create`: if it was removed by the
    // cleanup guard (cancellation) this fails instead of recreating a leftover.
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| make_err!(Code::Internal, "Failed to open zstd staging file: {e}"))?;

    let sink = DecodeSink {
        hasher: hasher_func.hasher(),
        decoded_len: 0,
        collect: collect_limit.map(|_| Vec::new()),
        collect_limit: collect_limit.unwrap_or(0),
    };
    let mut decoder = zstd::stream::write::Decoder::new(sink).map_err(|e| {
        make_err!(
            Code::Internal,
            "Zstd decoder init failed in zstd store: {e}"
        )
    })?;

    let mut wire_bytes_consumed: u64 = 0;
    loop {
        let chunk = reader
            .blocking_recv()
            .err_tip(|| "Failed to read compressed chunk in zstd store staging")?;
        if chunk.is_empty() {
            break; // EOF.
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
        file.write_all(&chunk)
            .map_err(|e| make_err!(Code::Internal, "Failed to write zstd staging file: {e}"))?;
        // A decode failure here is bad *client* input, not stored corruption.
        decoder
            .write_all(&chunk)
            .map_err(|e| make_input_err!("Zstd decode failed in zstd store staging: {e}"))?;
        if decoder.get_ref().decoded_len > expected_size {
            return Err(make_input_err!(
                "Decoded more than digest size in zstd store staging, digest says {expected_size}"
            ));
        }
    }

    decoder
        .flush()
        .map_err(|e| make_input_err!("Zstd decode flush failed in zstd store staging: {e}"))?;
    let mut sink = decoder.into_inner();
    let decoded_len = sink.decoded_len;
    let collected = sink.collect.take();

    // Validate BEFORE the upload is allowed to commit.
    if decoded_len != expected_size {
        return Err(make_input_err!(
            "Decoded size {decoded_len} does not match digest size {expected_size} in zstd store"
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

    file.flush()
        .map_err(|e| make_err!(Code::Internal, "Failed to flush zstd staging file: {e}"))?;
    file.sync_all()
        .map_err(|e| make_err!(Code::Internal, "Failed to sync zstd staging file: {e}"))?;

    Ok(StageOutput {
        compressed_size: wire_bytes_consumed,
        wire_bytes_consumed,
        collected,
    })
}

/// A pass-through store that stores blobs zstd-compressed in the inner CAS
/// store while presenting the raw (uncompressed) view to clients.
///
/// This is CAS-only: only digest keys are supported. Zero-byte digests are
/// never forwarded to the inner store.
#[derive(MetricsComponent)]
pub struct ZstdStore {
    #[metric(group = "inner_store")]
    inner_store: Store,
    /// Operator-controlled staging directory. Created/permission-checked in
    /// `post_init`; used by the zstd fast path.
    temp_path: String,
    /// Configured encode level, already validated to `1..=19`; `None` uses
    /// [`DEFAULT_ENCODE_LEVEL`].
    compression_level: Option<i32>,
    /// Hard cap on the number of compressed wire bytes accepted for a single
    /// staged upload before it is rejected with `ResourceExhausted`.
    max_compressed_upload_size: u64,
    /// Uncompressed-size ceiling below which a staged upload is eligible for
    /// re-compression at `compression_level`. `0` disables re-compression.
    max_recompression_size: u64,
    /// Bounds concurrent staged uploads (temp files being validated at once).
    staged_upload_semaphore: Arc<Semaphore>,
    /// Bounds concurrent re-compression passes.
    recompression_semaphore: Arc<Semaphore>,
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
    pub fn new(spec: &ZstdStoreSpec, inner_store: Store) -> Result<Arc<Self>, Error> {
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
        let staged = match spec.max_concurrent_staged_uploads {
            0 => DEFAULT_MAX_CONCURRENT_STAGED_UPLOADS,
            n => n,
        };
        let recompressions = match spec.max_concurrent_recompressions {
            0 => DEFAULT_MAX_CONCURRENT_RECOMPRESSIONS,
            n => n,
        };
        Ok(Arc::new(Self {
            inner_store,
            temp_path: spec.temp_path.clone(),
            compression_level: spec.compression_level,
            max_compressed_upload_size: spec.max_compressed_upload_size,
            max_recompression_size: spec.max_recompression_size,
            staged_upload_semaphore: Arc::new(Semaphore::new(staged)),
            recompression_semaphore: Arc::new(Semaphore::new(recompressions)),
        }))
    }

    #[inline]
    fn encode_level(&self) -> i32 {
        self.compression_level.unwrap_or(DEFAULT_ENCODE_LEVEL)
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

        let (tx, rx) = make_buf_channel_pair();
        let inner_store = self.inner_store.clone();
        let max_output_size = zstd_compress_bound(digest.size_bytes());
        let update_fut = spawn!("zstd_store_update_spawn", async move {
            inner_store
                .update(digest, rx, UploadSizeInfo::MaxSize(max_output_size))
                .await
                .err_tip(|| "Inner store update in zstd store failed")
        })
        .map(
            |result| match result.err_tip(|| "Failed to run zstd store update spawn") {
                Ok(inner_result) => inner_result,
                Err(e) => Err(e),
            },
        );

        let level = self.encode_level();
        let hasher_func = digest_hasher_func_from_context();
        let encode_fut = spawn_blocking!("zstd_store_encode_identity", move || {
            encode_identity(reader, tx, level, hasher_func, digest)
        })
        .map(
            |result| match result.err_tip(|| "Failed to run zstd store encode task") {
                Ok(encode_result) => encode_result,
                Err(e) => Err(e),
            },
        );

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

        let (physical_tx, physical_rx) = make_buf_channel_pair();
        let inner_store = self.inner_store.clone();
        let get_fut = spawn!("zstd_store_get_part_spawn", async move {
            inner_store
                .get_part(digest, physical_tx, 0, None)
                .await
                .err_tip(|| "Inner store get in zstd store failed")
        })
        .map(
            |result| match result.err_tip(|| "Failed to run zstd store get spawn") {
                Ok(inner_result) => inner_result,
                Err(e) => Err(e),
            },
        );

        let (raw_tx, mut raw_rx) = make_buf_channel_pair();
        let decode_fut = spawn_blocking!("zstd_store_decode_identity", move || {
            decode_identity(physical_rx, raw_tx, offset, length)
        })
        .map(
            |result| match result.err_tip(|| "Failed to run zstd store decode task") {
                Ok(decode_result) => decode_result,
                Err(e) => Err(e),
            },
        );

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
                .send(empty_zstd_level3())
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
            .err_tip(|| "Inner store get_part in zstd store get_zstd failed")
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
            return self.validate_empty_zstd_stream(reader).await;
        }
        let staged = self
            .stage_compressed(digest, digest_function, reader)
            .await?;
        let wire_bytes_consumed = staged.wire_bytes_consumed;
        self.commit_staged(digest, &staged).await?;
        Ok(wire_bytes_consumed)
    }

    /// Whole-`Bytes` variant of [`ZstdStore::update_zstd`] for `BatchUpdate`.
    pub async fn update_zstd_oneshot(
        &self,
        digest: DigestInfo,
        digest_function: DigestHasherFunc,
        data: Bytes,
    ) -> Result<u64, Error> {
        if is_zero_digest(digest) {
            let wire_bytes_consumed = data.len() as u64;
            if wire_bytes_consumed > self.max_compressed_upload_size {
                return Err(make_err!(
                    Code::ResourceExhausted,
                    "Compressed upload exceeded max_compressed_upload_size ({} bytes) in zstd store",
                    self.max_compressed_upload_size
                ));
            }
            let decoded = spawn_blocking!("zstd_store_validate_empty_oneshot", move || {
                decode_all_zstd(&data, None, |e| {
                    make_input_err!("Zstd decode failed for zero digest: {e}")
                })
            })
            .await
            .err_tip(|| "Failed to run zstd store empty validation task")??;
            if !decoded.is_empty() {
                return Err(make_input_err!(
                    "Zero-digest zstd upload did not decode to empty in zstd store"
                ));
            }
            return Ok(wire_bytes_consumed);
        }

        // Feed `data` through a channel so the shared staging path handles it.
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

        let stage_res = self.stage_compressed(digest, digest_function, rx).await;
        let feed_res = feed_fut
            .await
            .err_tip(|| "Failed to run zstd store oneshot feed task")?;
        let staged = stage_res?;
        feed_res?;
        let wire_bytes_consumed = staged.wire_bytes_consumed;
        self.commit_staged(digest, &staged).await?;
        Ok(wire_bytes_consumed)
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
            return Ok((physical, true));
        }
        let expected_size = usize::try_from(digest.size_bytes())
            .map_err(|_| make_err!(Code::Internal, "Digest size too large for this platform"))?;
        let raw = spawn_blocking!("zstd_store_batch_decode", move || {
            // Stored data was validated at upload time; a decode failure here is
            // corruption of already-stored bytes.
            decode_all_zstd(&physical, Some(expected_size), |e| {
                make_err!(Code::DataLoss, "Zstd decode failed in zstd store: {e}")
            })
        })
        .await
        .err_tip(|| "Failed to run zstd store batch decode task")??;
        Ok((Bytes::from(raw), false))
    }

    /// Validate that a zero-digest client stream decodes to empty without
    /// touching the inner store. Returns the compressed wire bytes consumed.
    async fn validate_empty_zstd_stream(&self, reader: DropCloserReadHalf) -> Result<u64, Error> {
        let max_compressed_upload_size = self.max_compressed_upload_size;
        spawn_blocking!("zstd_store_validate_empty", move || {
            let mut reader = reader;
            let mut compressed = Vec::new();
            let mut wire_bytes_consumed: u64 = 0;
            loop {
                let chunk = reader
                    .blocking_recv()
                    .err_tip(|| "Failed to read chunk validating zero-digest zstd")?;
                if chunk.is_empty() {
                    break;
                }
                wire_bytes_consumed += chunk.len() as u64;
                if wire_bytes_consumed > max_compressed_upload_size {
                    return Err(make_err!(
                        Code::ResourceExhausted,
                        "Compressed upload exceeded max_compressed_upload_size ({max_compressed_upload_size} bytes) in zstd store"
                    ));
                }
                compressed.extend_from_slice(&chunk);
            }
            let decoded = decode_all_zstd(&compressed, None, |e| {
                make_input_err!("Zstd decode failed for zero digest: {e}")
            })?;
            if !decoded.is_empty() {
                return Err(make_input_err!(
                    "Zero-digest zstd upload did not decode to empty in zstd store"
                ));
            }
            Ok(wire_bytes_consumed)
        })
        .await
        .err_tip(|| "Failed to run zstd store empty validation task")?
    }

    /// Stage a validated compressed upload to a temp file, optionally producing
    /// a smaller re-compressed candidate. The returned [`StagedStream`] owns the
    /// cleanup guard and staging permit.
    async fn stage_compressed(
        &self,
        digest: DigestInfo,
        digest_function: DigestHasherFunc,
        reader: DropCloserReadHalf,
    ) -> Result<StagedStream, Error> {
        let permit = self
            .staged_upload_semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| make_err!(Code::Internal, "Staged upload semaphore closed: {e}"))?;

        let mut guard = TempFileGuard::default();
        let primary_path = format!("{}/zstd-stage-{}", self.temp_path, uuid::Uuid::new_v4());
        // Arm the guard BEFORE creating the file: if `create_empty_temp_file`
        // creates the file but then fails (e.g. `set_permissions` errors) or
        // this `.await` is cancelled, the file must still be tracked for
        // removal. Removing a path that was never created is a harmless
        // ENOENT no-op, so arming first is always safe.
        guard.add(primary_path.clone());
        create_empty_temp_file(&primary_path).await?;

        let recompress_eligible =
            self.max_recompression_size > 0 && self.compression_level.is_some();
        let collect_limit = recompress_eligible.then_some(self.max_recompression_size);

        let fs_permit = fs::get_permit().await?;
        let max_compressed_upload_size = self.max_compressed_upload_size;
        let blocking_path = primary_path.clone();
        let output = spawn_blocking!("zstd_store_stage", move || {
            stage_compressed_blocking(
                reader,
                &blocking_path,
                digest,
                digest_function,
                max_compressed_upload_size,
                collect_limit,
                fs_permit,
            )
        })
        .await
        .err_tip(|| "Failed to run zstd store staging task")??;

        let mut chosen_path = primary_path.clone();
        let mut compressed_size = output.compressed_size;

        // Re-compression only runs when the decoded content stayed within the
        // configured ceiling (so `collected` is present).
        if let Some(collected) = output.collected {
            let level = self.encode_level();
            let _rec_permit = self
                .recompression_semaphore
                .clone()
                .acquire_owned()
                .await
                .map_err(|e| make_err!(Code::Internal, "Recompression semaphore closed: {e}"))?;
            let secondary_path = format!("{}/zstd-stage-{}", self.temp_path, uuid::Uuid::new_v4());
            // Arm the guard before creation for the same reason as the primary
            // path above; track both candidates while they coexist on disk.
            guard.add(secondary_path.clone());
            create_empty_temp_file(&secondary_path).await?;

            let rec_fs_permit = fs::get_permit().await?;
            let blocking_secondary = secondary_path.clone();
            let recompressed_size = spawn_blocking!("zstd_store_recompress", move || {
                let recompressed = zstd::bulk::compress(&collected, level).map_err(|e| {
                    make_err!(
                        Code::Internal,
                        "Zstd re-compression failed in zstd store: {e}"
                    )
                })?;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&blocking_secondary)
                    .map_err(|e| {
                        make_err!(Code::Internal, "Failed to open zstd recompress file: {e}")
                    })?;
                file.write_all(&recompressed).map_err(|e| {
                    make_err!(Code::Internal, "Failed to write zstd recompress file: {e}")
                })?;
                file.flush().map_err(|e| {
                    make_err!(Code::Internal, "Failed to flush zstd recompress file: {e}")
                })?;
                file.sync_all().map_err(|e| {
                    make_err!(Code::Internal, "Failed to sync zstd recompress file: {e}")
                })?;
                drop(rec_fs_permit);
                Result::<u64, Error>::Ok(recompressed.len() as u64)
            })
            .await
            .err_tip(|| "Failed to run zstd store recompress task")??;

            // Keep the smaller candidate; remove the larger one now to bound
            // disk use (the guard still covers it if removal fails).
            if recompressed_size < compressed_size {
                chosen_path = secondary_path;
                compressed_size = recompressed_size;
                drop(fs::remove_file(&primary_path).await);
            } else {
                drop(fs::remove_file(&secondary_path).await);
            }
        }

        Ok(StagedStream {
            path: chosen_path,
            compressed_size,
            wire_bytes_consumed: output.wire_bytes_consumed,
            _permit: permit,
            _guard: guard,
        })
    }

    /// Stream a validated staged temp file into the inner store. Only called
    /// after validation succeeded, so the inner store never sees corrupt data.
    async fn commit_staged(&self, digest: DigestInfo, staged: &StagedStream) -> Result<(), Error> {
        let file = fs::open_file(&staged.path, 0, u64::MAX)
            .await
            .err_tip(|| "Failed to open staged zstd file for commit")?
            .into_inner();
        self.inner_store
            .update_with_whole_file(
                digest,
                OsString::from(staged.path.clone()),
                file,
                UploadSizeInfo::ExactSize(staged.compressed_size),
            )
            .await
            .err_tip(|| "Failed to commit staged zstd upload to inner store")?;
        Ok(())
    }
}

#[async_trait]
impl StoreDriver for ZstdStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        // Ensure the staging directory exists and is writable for the zstd fast
        // path (which stages validation/recompression files there).
        tokio::fs::create_dir_all(&self.temp_path)
            .await
            .map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Failed to create ZstdStore temp_path {}: {e}",
                    self.temp_path
                )
            })?;
        // Probe that temp_path is actually writable: a read-only or
        // mispermissioned directory would otherwise only surface as a failure
        // on the first upload, long after startup.
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

    fn register_remove_callback(
        self: Arc<Self>,
        callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        self.inner_store.register_remove_callback(callback)
    }
}

default_health_status_indicator!(ZstdStore);
