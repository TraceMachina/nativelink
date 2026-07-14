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
use std::io::{Read, Write};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::future::FutureExt;
use nativelink_config::stores::ZstdStoreSpec;
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{
    DigestHasher, DigestHasherFunc, digest_hasher_func_from_context,
};
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    RemoveItemCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use nativelink_util::{spawn, spawn_blocking};
use tokio::sync::Semaphore;

use crate::cas_utils::is_zero_digest;

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

/// A `std::io::Read` adapter over a [`DropCloserReadHalf`] that blocks the
/// current (blocking) thread while waiting for the next chunk. Only safe to use
/// from within `spawn_blocking!`.
struct BufChannelReader {
    rx: DropCloserReadHalf,
    chunk: Bytes,
    chunk_offset: usize,
}

impl BufChannelReader {
    const fn new(rx: DropCloserReadHalf) -> Self {
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

/// A `std::io::Write` adapter over a [`DropCloserWriteHalf`] that blocks the
/// current (blocking) thread while forwarding chunks. Only safe to use from
/// within `spawn_blocking!`. EOF is intentionally NOT sent on drop; the caller
/// must call [`BufChannelWriter::send_eof`] explicitly once the upload is
/// validated so a failed upload never commits to the inner store.
struct BufChannelWriter {
    tx: DropCloserWriteHalf,
}

impl BufChannelWriter {
    const fn new(tx: DropCloserWriteHalf) -> Self {
        Self { tx }
    }

    fn send_eof(&mut self) -> Result<(), Error> {
        self.tx.send_eof()
    }
}

impl Write for BufChannelWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        self.tx
            .blocking_send(Bytes::copy_from_slice(buf))
            .map_err(Error::to_std_err)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
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

    while remaining > 0 {
        let read = decoder
            .read(&mut buffer)
            .map_err(|e| make_err!(Code::DataLoss, "Zstd decode failed in zstd store: {e}"))?;
        if read == 0 {
            break; // EOF.
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
    // TODO(task 4): used by zstd fast path.
    #[allow(dead_code)]
    max_compressed_upload_size: u64,
    // TODO(task 4): used by zstd fast path.
    #[allow(dead_code)]
    max_recompression_size: u64,
    // TODO(task 4): used by zstd fast path.
    #[allow(dead_code)]
    staged_upload_semaphore: Arc<Semaphore>,
    // TODO(task 4): used by zstd fast path.
    #[allow(dead_code)]
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

        // Presence comes from the inner store, but the reported size is always
        // the digest's uncompressed size (the physical zstd size is meaningless
        // to clients).
        self.inner_store
            .as_store_driver_pin()
            .has_with_results(keys, results)
            .await?;
        for (key, result) in keys.iter().zip(results.iter_mut()) {
            if is_zero_digest(key.borrow()) {
                *result = Some(0);
            } else if result.is_some()
                && let StoreKey::Digest(digest) = key
            {
                *result = Some(digest.size_bytes());
            }
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
