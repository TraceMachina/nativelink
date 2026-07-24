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

use core::pin::Pin;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use core::time::Duration;
use std::ffi::OsString;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use nativelink_config::stores::{
    CacheMetricsSpec, CompressionAlgorithm, CompressionSpec, DedupSpec, ExistenceCacheSpec,
    FastSlowSpec, MemorySpec, NoopSpec, RefSpec, ShardConfig, ShardSpec, SizePartitioningSpec,
    StoreDirection, StoreSpec, ZstdConfig,
};
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_store::cache_metrics_store::CacheMetricsStore;
use nativelink_store::cas_utils::{ZERO_BYTE_DIGESTS, is_zero_digest};
use nativelink_store::dedup_store::DedupStore;
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::existence_cache_store::ExistenceCacheStore;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::ref_store::RefStore;
use nativelink_store::shard_store::ShardStore;
use nativelink_store::size_partitioning_store::SizePartitioningStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_store::zstd_store::ZstdStore;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair,
};
use nativelink_util::common::{DigestInfo, make_temp_path};
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc, make_ctx_for_hash_func};
use nativelink_util::fs::FileSlot;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    RemoveCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo, WireCompressor,
};
use nativelink_util::{background_spawn, spawn};
use opentelemetry::context::FutureExt;
use pretty_assertions::assert_eq;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

#[derive(Default, MetricsComponent)]
struct RecordingStore {
    update_count: AtomicUsize,
    /// Total number of keys the inner store observed via `has_with_results`.
    has_key_count: AtomicUsize,
    /// Set if any zero-byte digest was ever forwarded to the inner store.
    has_saw_zero: AtomicBool,
}

#[async_trait]
impl StoreDriver for RecordingStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        _results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        self.has_key_count.fetch_add(keys.len(), Ordering::Relaxed);
        if keys.iter().any(|key| is_zero_digest(key.borrow())) {
            self.has_saw_zero.store(true, Ordering::Relaxed);
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        _reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        self.update_count.fetch_add(1, Ordering::Relaxed);
        Ok(0)
    }

    async fn get_part(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        _writer: &mut DropCloserWriteHalf,
        _offset: u64,
        _length: Option<u64>,
    ) -> Result<(), Error> {
        Err(make_err!(Code::NotFound, "Not found"))
    }

    fn inner_store(&self, _key: Option<StoreKey>) -> &dyn StoreDriver {
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
        _callback: RemoveCallback,
    ) -> Result<(), Error> {
        Ok(())
    }
}

default_health_status_indicator!(RecordingStore);

/// A store wrapper that stores data in an inner [`MemoryStore`] but, on
/// `get_part`, replays the physical bytes in many small `send` calls. This forces
/// the `ZstdStore` physical channel (a 2-slot buffer) to back up so the drain
/// behavior of the decode path is actually exercised. A real streaming backend
/// (S3, filesystem) chunks its output the same way.
#[derive(MetricsComponent)]
struct ChunkingStore {
    #[metric(group = "inner")]
    inner: Store,
    chunk_size: usize,
}

#[async_trait]
impl StoreDriver for ChunkingStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        self.inner.has_with_results(keys, results).await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        self.inner
            .as_store_driver_pin()
            .update(key, reader, size_info)
            .await
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        // Ignore offset/length: ZstdStore always requests the full physical blob.
        let _ = (offset, length);
        let full = self.inner.get_part_unchunked(key.borrow(), 0, None).await?;
        for chunk in full.chunks(self.chunk_size) {
            writer.send(Bytes::copy_from_slice(chunk)).await?;
        }
        writer.send_eof()?;
        Ok(())
    }

    fn inner_store(&self, _key: Option<StoreKey>) -> &dyn StoreDriver {
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
        _callback: RemoveCallback,
    ) -> Result<(), Error> {
        Ok(())
    }
}

default_health_status_indicator!(ChunkingStore);

const TEMP_PATH: &str = "/tmp/nativelink-zstd-store-test";

fn spec() -> ZstdConfig {
    ZstdConfig {
        temp_path: TEMP_PATH.to_string(),
        max_compressed_upload_size: 512 * 1024 * 1024,
        max_concurrent_staged_uploads: 0,
        compression_level: None,
        max_recompression_size: 0,
        max_concurrent_recompressions: 0,
        commit_timeout_s: 0,
    }
}

fn digest_for(data: &[u8]) -> DigestInfo {
    let hash: [u8; 32] = Sha256::digest(data).into();
    DigestInfo::new(hash, data.len() as u64)
}

#[nativelink_test]
async fn identity_round_trip() -> Result<(), Error> {
    const DATA: &[u8] = b"hello zstd store, this compresses a bit aaaaaaaaaaaaaaaaaaaa";

    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = Store::new(ZstdStore::new(&spec(), inner)?);

    let digest = digest_for(DATA);

    store.update_oneshot(digest, DATA.into()).await?;
    let got = store.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(&got[..], DATA, "Expected round-tripped data to match");
    Ok(())
}

#[nativelink_test]
async fn identity_round_trip_partial_read() -> Result<(), Error> {
    const DATA: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz0123456789";

    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = Store::new(ZstdStore::new(&spec(), inner)?);

    let digest = digest_for(DATA);

    store.update_oneshot(digest, DATA.into()).await?;
    let got = store.get_part_unchunked(digest, 5, Some(10)).await?;
    assert_eq!(&got[..], &DATA[5..15], "Expected partial read to match");
    Ok(())
}

#[nativelink_test]
async fn zero_digest_read_returns_empty() -> Result<(), Error> {
    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = Store::new(ZstdStore::new(&spec(), inner)?);

    for digest in ZERO_BYTE_DIGESTS {
        let got = store.get_part_unchunked(digest, 0, None).await?;
        assert_eq!(got.len(), 0, "Expected zero-digest read to be empty");
    }
    Ok(())
}

#[nativelink_test]
async fn zero_digest_write_empty_skips_inner() -> Result<(), Error> {
    let recording = Arc::new(RecordingStore::default());
    let store = Store::new(ZstdStore::new(&spec(), Store::new(recording.clone()))?);

    for digest in ZERO_BYTE_DIGESTS {
        store.update_oneshot(digest, Bytes::new()).await?;
    }
    // The inner store's update must never be called for zero digests.
    assert_eq!(
        recording.update_count.load(Ordering::Relaxed),
        0,
        "Zero digest must not reach the inner store"
    );
    Ok(())
}

#[nativelink_test]
async fn str_key_is_rejected() -> Result<(), Error> {
    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = Store::new(ZstdStore::new(&spec(), inner)?);

    let keys = [StoreKey::new_str("some-string-key")];
    let mut results = [None];
    let err = store
        .has_with_results(&keys, &mut results)
        .await
        .expect_err("Expected a Str key to be rejected");
    assert!(
        err.to_string().contains("only supports digest keys"),
        "Unexpected error: {err}"
    );
    Ok(())
}

#[nativelink_test]
async fn has_reports_uncompressed_digest_size() -> Result<(), Error> {
    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = Store::new(ZstdStore::new(&spec(), inner)?);

    // Highly compressible payload so the physical zstd size differs from the
    // uncompressed digest size.
    let data = vec![0u8; 4096];
    let digest = digest_for(&data);

    store.update_oneshot(digest, data.clone().into()).await?;
    let reported = store.has(digest).await?;
    assert_eq!(
        reported,
        Some(data.len() as u64),
        "has must report the uncompressed digest size, not the physical zstd size"
    );
    Ok(())
}

#[nativelink_test]
async fn ranged_read_large_blob_drains_inner_stream() -> Result<(), Error> {
    // Regression for the partial-read drain bug: a ranged read on a blob whose
    // physical (compressed) stream is larger than the 2-slot channel buffer must
    // still return the requested bytes. Before the fix the decode loop returned
    // as soon as the requested `length` was produced, dropping the physical
    // reader while the spawned inner `get` was still streaming compressed bytes;
    // its next `send` then failed with "receiver disconnected" and the whole
    // get_part surfaced that error instead of the requested bytes.
    //
    // Use a repeating-but-nontrivial pattern (not all-zero) so the compressed
    // stream is large enough to span multiple channel sends.
    let mut data = Vec::with_capacity(1024 * 1024);
    let mut counter: u32 = 0x1234_5678;
    while data.len() < 1024 * 1024 {
        counter = counter.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        data.extend_from_slice(&counter.to_le_bytes());
    }

    // A chunking inner store forces the physical stream to span many channel
    // sends (16 KiB each) so the 2-slot channel backs up while the decoder is
    // still consuming; this is what makes an early return drop the reader and
    // fail the inner get.
    let inner = Store::new(Arc::new(ChunkingStore {
        inner: Store::new(MemoryStore::new(&MemorySpec::default())),
        chunk_size: 16 * 1024,
    }));
    let store = Store::new(ZstdStore::new(&spec(), inner)?);

    let digest = digest_for(&data);
    store.update_oneshot(digest, data.clone().into()).await?;

    let offset = 300 * 1024;
    let length = 100 * 1024;
    let got = store
        .get_part_unchunked(digest, offset as u64, Some(length as u64))
        .await?;
    assert_eq!(
        &got[..],
        &data[offset..offset + length],
        "Ranged read of a large blob must return the requested sub-range"
    );
    Ok(())
}

#[nativelink_test]
async fn corrupt_upload_hash_mismatch_is_rejected_and_not_committed() -> Result<(), Error> {
    const DATA: &[u8] = b"the quick brown fox jumps over the lazy dog";

    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = Store::new(ZstdStore::new(&spec(), inner.clone())?);

    // Claim a digest whose hash does NOT match the data (but the size matches).
    let mut wrong = DATA.to_vec();
    wrong[0] ^= 0xFF;
    let bad_digest = digest_for(&wrong);
    assert_eq!(bad_digest.size_bytes(), DATA.len() as u64);

    let err = store
        .update_oneshot(bad_digest, DATA.into())
        .await
        .expect_err("Expected a hash mismatch to be rejected");
    assert_eq!(
        err.code,
        Code::InvalidArgument,
        "Hash mismatch must be InvalidArgument, got: {err}"
    );

    // The blob must NOT have committed to the inner store.
    assert_eq!(
        store.has(bad_digest).await?,
        None,
        "A rejected upload must not be visible via the zstd store"
    );
    assert_eq!(
        inner.has(bad_digest).await?,
        None,
        "A rejected upload must not commit to the inner store"
    );
    Ok(())
}

#[nativelink_test]
async fn corrupt_upload_size_mismatch_is_rejected_and_not_committed() -> Result<(), Error> {
    const DATA: &[u8] = b"the quick brown fox jumps over the lazy dog";

    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = Store::new(ZstdStore::new(&spec(), inner.clone())?);

    // Correct hash, but claim a size larger than the actual data length.
    let hash: [u8; 32] = Sha256::digest(DATA).into();
    let bad_digest = DigestInfo::new(hash, DATA.len() as u64 + 10);

    let err = store
        .update_oneshot(bad_digest, DATA.into())
        .await
        .expect_err("Expected a size mismatch to be rejected");
    assert_eq!(
        err.code,
        Code::InvalidArgument,
        "Size mismatch must be InvalidArgument, got: {err}"
    );

    assert_eq!(
        store.has(bad_digest).await?,
        None,
        "A rejected upload must not be visible via the zstd store"
    );
    assert_eq!(
        inner.has(bad_digest).await?,
        None,
        "A rejected upload must not commit to the inner store"
    );
    Ok(())
}

#[nativelink_test]
async fn new_rejects_out_of_range_compression_levels() -> Result<(), Error> {
    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));

    for level in [0, 20] {
        let mut bad_spec = spec();
        bad_spec.compression_level = Some(level);
        assert!(
            ZstdStore::new(&bad_spec, inner.clone()).is_err(),
            "compression_level {level} must be rejected"
        );
    }

    for level in [Some(1), Some(19), None] {
        let mut ok_spec = spec();
        ok_spec.compression_level = level;
        assert!(
            ZstdStore::new(&ok_spec, inner.clone()).is_ok(),
            "compression_level {level:?} must be accepted"
        );
    }
    Ok(())
}

#[nativelink_test]
async fn identity_round_trip_blake3_context() -> Result<(), Error> {
    const DATA: &[u8] = b"blake3 round trip payload aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = Store::new(ZstdStore::new(&spec(), inner)?);

    // Compute the BLAKE3 digest of the payload using the context hasher.
    let mut hasher = DigestHasherFunc::Blake3.hasher();
    hasher.update(DATA);
    let digest = hasher.finalize_digest();

    // Run both the update and get under a BLAKE3 request-digest context, proving
    // the context-hasher mechanism (not the SHA-256 default) is honored.
    let ctx = make_ctx_for_hash_func(DigestHasherFunc::Blake3)?;
    store
        .update_oneshot(digest, DATA.into())
        .with_context(ctx.clone())
        .await?;
    let got = store
        .get_part_unchunked(digest, 0, None)
        .with_context(ctx)
        .await?;
    assert_eq!(&got[..], DATA, "Expected BLAKE3 round-trip to match");
    Ok(())
}

#[nativelink_test]
async fn has_zero_digest_never_touches_inner_store() -> Result<(), Error> {
    let recording = Arc::new(RecordingStore::default());
    let store = Store::new(ZstdStore::new(&spec(), Store::new(recording.clone()))?);

    // Mixed batch: two zero digests and one non-zero (absent) digest.
    let nonzero = digest_for(b"not present");
    let keys = [
        StoreKey::Digest(ZERO_BYTE_DIGESTS[0]),
        StoreKey::Digest(nonzero),
        StoreKey::Digest(ZERO_BYTE_DIGESTS[1]),
    ];
    let mut results = [None, None, None];
    store.has_with_results(&keys, &mut results).await?;

    assert_eq!(
        results,
        [Some(0), None, Some(0)],
        "Zero digests must report Some(0); absent non-zero must be None"
    );
    assert!(
        !recording.has_saw_zero.load(Ordering::Relaxed),
        "Zero digests must never be forwarded to the inner store"
    );
    assert_eq!(
        recording.has_key_count.load(Ordering::Relaxed),
        1,
        "Only the single non-zero key may reach the inner store"
    );
    Ok(())
}

#[nativelink_test]
async fn has_mixed_batch_reports_correct_sizes() -> Result<(), Error> {
    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = Store::new(ZstdStore::new(&spec(), inner)?);

    let present = vec![7u8; 4096];
    let present_digest = digest_for(&present);
    store
        .update_oneshot(present_digest, present.clone().into())
        .await?;
    let absent_digest = digest_for(b"definitely absent");

    let keys = [
        StoreKey::Digest(ZERO_BYTE_DIGESTS[0]),
        StoreKey::Digest(present_digest),
        StoreKey::Digest(absent_digest),
    ];
    let mut results = [None, None, None];
    store.has_with_results(&keys, &mut results).await?;
    assert_eq!(
        results,
        [Some(0), Some(present.len() as u64), None],
        "Mixed batch must report zero=Some(0), present=uncompressed size, absent=None"
    );
    Ok(())
}

#[nativelink_test]
async fn factory_selects_zstd_wire_capability() -> Result<(), Error> {
    let store_spec = StoreSpec::Compression(Box::new(CompressionSpec {
        backend: StoreSpec::Memory(MemorySpec::default()),
        compression_algorithm: CompressionAlgorithm::Zstd(spec()),
    }));
    let store_manager = Arc::new(StoreManager::new());
    let store = store_factory(&store_spec, &store_manager, None).await?;
    assert!(
        store.wire_compression_store().is_some(),
        "Expected CompressionAlgorithm::Zstd to expose the wire-compression capability"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// zstd fast-path (get_zstd / update_zstd / update_zstd_oneshot / get_for_batch)
// ---------------------------------------------------------------------------

/// A spec pointing at a specific temp dir (for tests that assert temp-dir
/// emptiness and therefore must not share the global staging directory).
const fn spec_for(temp_path: String) -> ZstdConfig {
    ZstdConfig {
        temp_path,
        max_compressed_upload_size: 512 * 1024 * 1024,
        max_concurrent_staged_uploads: 0,
        compression_level: None,
        max_recompression_size: 0,
        max_concurrent_recompressions: 0,
        commit_timeout_s: 0,
    }
}

/// Builds a `ZstdStore` over a fresh `MemoryStore`, ensuring the staging dir
/// exists. Returns the concrete store (for the fast-path methods), a `Store`
/// wrapper (for the identity round-trip view), and the inner store (for
/// physical inspection).
async fn build(spec: &ZstdConfig) -> Result<(Arc<ZstdStore>, Store, Store), Error> {
    std::fs::create_dir_all(&spec.temp_path)
        .map_err(|e| make_err!(Code::Internal, "Failed to create test temp dir: {e}"))?;
    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let zstd = ZstdStore::new(spec, inner.clone())?;
    let store = Store::new(zstd.clone());
    Ok((zstd, store, inner))
}

/// Drive `get_zstd` and collect the full emitted (physical zstd) stream.
async fn collect_zstd(store: &ZstdStore, digest: DigestInfo) -> Result<Bytes, Error> {
    let (tx, mut rx) = make_buf_channel_pair();
    let (get_res, collected) = tokio::join!(store.get_zstd(digest, tx), rx.consume(None));
    get_res?;
    collected
}

/// A `DropCloserReadHalf` that yields the given chunks then EOF, fed by a task.
fn reader_from(chunks: Vec<Bytes>) -> DropCloserReadHalf {
    let (mut tx, rx) = make_buf_channel_pair();
    background_spawn!("zstd_test_reader_feed", async move {
        for chunk in chunks {
            if tx.send(chunk).await.is_err() {
                return;
            }
        }
        drop(tx.send_eof());
    });
    rx
}

/// Highly compressible data (a repeated pseudo-random block) whose compressed
/// size is sensitive to the zstd level.
fn compressible_data(len: usize) -> Vec<u8> {
    let mut block = [0u8; 251];
    let mut state: u32 = 0x9E37_79B1;
    for b in &mut block {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *b = (state >> 24) as u8;
    }
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let take = (len - out.len()).min(block.len());
        out.extend_from_slice(&block[..take]);
    }
    out
}

fn random_bytes(len: usize) -> Vec<u8> {
    use rand::RngCore;
    let mut out = vec![0u8; len];
    rand::rng().fill_bytes(&mut out);
    out
}

/// Data whose compressed size is strongly level-sensitive: two copies of a
/// large incompressible block. Only a large-window (high level) encoder can
/// dedup the second copy, so a low level produces a much larger stream.
fn level_sensitive_data() -> Vec<u8> {
    let block = random_bytes(512 * 1024);
    let mut out = Vec::with_capacity(block.len() * 2);
    out.extend_from_slice(&block);
    out.extend_from_slice(&block);
    out
}

fn dir_entry_count(path: &str) -> usize {
    std::fs::read_dir(path).map_or(0, Iterator::count)
}

#[nativelink_test]
async fn get_zstd_is_byte_for_byte_passthrough() -> Result<(), Error> {
    const DATA: &[u8] = b"passthrough payload aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    let (zstd, _store, _inner) = build(&spec()).await?;
    let compressed = zstd::bulk::compress(DATA, 3).unwrap();
    let digest = digest_for(DATA);

    let wire = zstd
        .update_zstd_oneshot(
            digest,
            DigestHasherFunc::Sha256,
            Bytes::from(compressed.clone()),
        )
        .await?;
    assert_eq!(wire, compressed.len() as u64, "wire bytes must match input");

    let got = collect_zstd(&zstd, digest).await?;
    assert_eq!(
        &got[..],
        &compressed[..],
        "get_zstd must return the stored zstd stream byte-for-byte"
    );
    let decoded = zstd::stream::decode_all(&got[..]).unwrap();
    assert_eq!(
        &decoded[..],
        DATA,
        "the passthrough stream must decode back"
    );
    Ok(())
}

#[nativelink_test]
async fn get_zstd_preserves_concatenated_frames() -> Result<(), Error> {
    // Two independently-compressed zstd frames concatenated into one stream.
    const PART_A: &[u8] = b"first frame content aaaaaaaaaaaaaaaaaaaaaaaa";
    const PART_B: &[u8] = b"second frame content bbbbbbbbbbbbbbbbbbbbbbbb";

    let frame_a = zstd::bulk::compress(PART_A, 3).unwrap();
    let frame_b = zstd::bulk::compress(PART_B, 3).unwrap();
    let mut concatenated = frame_a.clone();
    concatenated.extend_from_slice(&frame_b);

    let mut raw = PART_A.to_vec();
    raw.extend_from_slice(PART_B);
    let digest = digest_for(&raw);

    let (zstd, _store, _inner) = build(&spec()).await?;
    // Upload the two frames as separate stream chunks to exercise the streaming
    // path; passthrough must not re-frame them.
    let wire = zstd
        .update_zstd(
            digest,
            DigestHasherFunc::Sha256,
            reader_from(vec![Bytes::from(frame_a), Bytes::from(frame_b)]),
        )
        .await?;
    assert_eq!(wire, concatenated.len() as u64);

    let got = collect_zstd(&zstd, digest).await?;
    assert_eq!(
        &got[..],
        &concatenated[..],
        "passthrough must preserve the exact concatenated-frame bytes"
    );
    assert_eq!(
        zstd::stream::decode_all(&got[..]).unwrap(),
        raw,
        "concatenated frames must decode to the concatenated raw content"
    );
    Ok(())
}

#[nativelink_test]
async fn update_zstd_round_trips_through_identity() -> Result<(), Error> {
    const DATA: &[u8] = b"round trip payload cccccccccccccccccccccccccccccccc";

    let (zstd, store, _inner) = build(&spec()).await?;
    let compressed = zstd::bulk::compress(DATA, 3).unwrap();
    let digest = digest_for(DATA);

    let wire = zstd
        .update_zstd_oneshot(
            digest,
            DigestHasherFunc::Sha256,
            Bytes::from(compressed.clone()),
        )
        .await?;
    assert_eq!(wire, compressed.len() as u64);

    let got = store.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(
        &got[..],
        DATA,
        "identity read must return the original raw bytes"
    );
    Ok(())
}

#[nativelink_test]
async fn update_zstd_round_trips_with_blake3() -> Result<(), Error> {
    const DATA: &[u8] = b"blake3 compressed upload dddddddddddddddddddddddddddd";

    let (zstd, store, _inner) = build(&spec()).await?;
    let mut hasher = DigestHasherFunc::Blake3.hasher();
    hasher.update(DATA);
    let digest = hasher.finalize_digest();
    let compressed = zstd::bulk::compress(DATA, 3).unwrap();

    zstd.update_zstd_oneshot(digest, DigestHasherFunc::Blake3, Bytes::from(compressed))
        .await?;
    let got = store.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(&got[..], DATA, "BLAKE3 compressed round-trip must match");
    Ok(())
}

#[nativelink_test]
async fn update_zstd_rejects_hash_mismatch() -> Result<(), Error> {
    const DATA: &[u8] = b"the quick brown fox jumps over the lazy dog";

    let temp = make_temp_path("zstd-hash-mismatch");
    let (zstd, _store, inner) = build(&spec_for(temp.clone())).await?;

    // Compress DATA but claim the digest of a different blob of the same size.
    let compressed = zstd::bulk::compress(DATA, 3).unwrap();
    let mut other = DATA.to_vec();
    other[0] ^= 0xFF;
    let bad_digest = digest_for(&other);
    assert_eq!(bad_digest.size_bytes(), DATA.len() as u64);

    let err = zstd
        .update_zstd_oneshot(
            bad_digest,
            DigestHasherFunc::Sha256,
            Bytes::from(compressed),
        )
        .await
        .expect_err("hash mismatch must be rejected");
    assert_eq!(
        err.code,
        Code::InvalidArgument,
        "hash mismatch must be InvalidArgument, got: {err}"
    );
    assert_eq!(
        inner.has(bad_digest).await?,
        None,
        "a rejected compressed upload must not commit to the inner store"
    );
    assert_eq!(
        dir_entry_count(&temp),
        0,
        "a rejected upload must not leave staged temp files"
    );
    Ok(())
}

/// A complete frame followed by the beginning of a second frame is not a
/// complete concatenated zstd stream. `flush` alone accepted this shape, so it
/// is important that both the staged and zero-digest validation paths finalize
/// the decoder at EOF.
#[nativelink_test]
async fn update_zstd_rejects_incomplete_trailing_frame() -> Result<(), Error> {
    const DATA: &[u8] = b"complete first frame followed by a truncated second frame";

    let temp = make_temp_path("zstd-incomplete-trailing-frame");
    let (zstd, _store, inner) = build(&spec_for(temp.clone())).await?;
    let digest = digest_for(DATA);
    let mut trailing = zstd::bulk::compress(DATA, 3).unwrap();
    let next_frame = zstd::bulk::compress(b"second frame", 3).unwrap();
    trailing.extend_from_slice(&next_frame[..4]); // zstd magic, but no full frame.

    let err = zstd
        .update_zstd_oneshot(digest, DigestHasherFunc::Sha256, Bytes::from(trailing))
        .await
        .expect_err("a partial trailing zstd frame must be rejected");
    assert_eq!(err.code, Code::InvalidArgument, "got: {err}");
    assert_eq!(
        inner.has(digest).await?,
        None,
        "a stream with an incomplete trailing frame must not commit"
    );
    assert_eq!(
        dir_entry_count(&temp),
        0,
        "a rejected trailing frame must not leave a staging file"
    );

    let zero = ZERO_BYTE_DIGESTS[0];
    let mut empty_then_partial = zstd::bulk::compress(&[], 3).unwrap();
    empty_then_partial.extend_from_slice(&next_frame[..4]);
    let err = zstd
        .update_zstd_oneshot(
            zero,
            DigestHasherFunc::Sha256,
            Bytes::from(empty_then_partial),
        )
        .await
        .expect_err("zero-digest validation must reject a partial trailing frame too");
    assert_eq!(err.code, Code::InvalidArgument, "got: {err}");
    Ok(())
}

#[nativelink_test]
async fn update_zstd_rejects_oversize_and_cleans_up() -> Result<(), Error> {
    let temp = make_temp_path("zstd-oversize");
    let mut spec = spec_for(temp.clone());
    spec.max_compressed_upload_size = 32; // Very small cap.
    let (zstd, _store, inner) = build(&spec).await?;

    // Incompressible data so the compressed stream comfortably exceeds 32 bytes.
    let data = random_bytes(4096);
    let compressed = zstd::bulk::compress(&data, 3).unwrap();
    assert!(compressed.len() as u64 > spec.max_compressed_upload_size);
    let digest = digest_for(&data);

    let err = zstd
        .update_zstd_oneshot(digest, DigestHasherFunc::Sha256, Bytes::from(compressed))
        .await
        .expect_err("oversize upload must be rejected");
    assert_eq!(
        err.code,
        Code::ResourceExhausted,
        "oversize upload must be ResourceExhausted, got: {err}"
    );
    assert_eq!(
        inner.has(digest).await?,
        None,
        "an oversize upload must not commit to the inner store"
    );
    assert_eq!(
        dir_entry_count(&temp),
        0,
        "an oversize upload must not leave staged temp files"
    );
    Ok(())
}

/// Cancelling the async request after the blocking stage creates its file must
/// not leak it. The guard is moved into the detached blocking task before that
/// task can create the path; after EOF lets it finish, dropping its unobserved
/// result closes the descriptor before removing the file.
#[nativelink_test]
async fn cancelled_staging_task_eventually_cleans_its_temp_file() -> Result<(), Error> {
    const DATA: &[u8] = b"cancelled staging cleanup payload";

    let temp = make_temp_path("zstd-cancelled-stage");
    let (zstd, _store, _inner) = build(&spec_for(temp.clone())).await?;
    let digest = digest_for(DATA);
    let compressed = zstd::bulk::compress(DATA, 3).unwrap();
    let (mut tx, rx) = make_buf_channel_pair();
    let task_store = zstd.clone();
    let task = tokio::spawn(async move {
        task_store
            .update_zstd(digest, DigestHasherFunc::Sha256, rx)
            .await
    });

    tx.send(Bytes::from(compressed))
        .await
        .map_err(|e| make_err!(Code::Internal, "failed to feed staging task: {e}"))?;
    tokio::time::timeout(Duration::from_secs(1), async {
        while dir_entry_count(&temp) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the blocking stage must create its temp file before EOF");

    task.abort();
    assert!(task.await.is_err(), "the request task must be cancelled");
    tx.send_eof()
        .map_err(|e| make_err!(Code::Internal, "failed to finish staging input: {e}"))?;

    tokio::time::timeout(Duration::from_secs(1), async {
        while dir_entry_count(&temp) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the detached blocking stage must eventually remove its temp file");
    Ok(())
}

#[nativelink_test]
async fn update_zstd_recompresses_to_smaller_stream() -> Result<(), Error> {
    let data = level_sensitive_data();
    let digest = digest_for(&data);

    // Upload a poorly-compressed (level 1) stream; store re-compresses at 19.
    let mut spec = spec_for(make_temp_path("zstd-recompress"));
    spec.compression_level = Some(19);
    spec.max_recompression_size = 16 * 1024 * 1024;
    let (zstd, _store, inner) = build(&spec).await?;

    let poorly = zstd::bulk::compress(&data, 1).unwrap();
    zstd.update_zstd_oneshot(
        digest,
        DigestHasherFunc::Sha256,
        Bytes::from(poorly.clone()),
    )
    .await?;
    let physical = inner.get_part_unchunked(digest, 0, None).await?;
    assert!(
        physical.len() < poorly.len(),
        "re-compression must shrink the stored stream ({} !< {})",
        physical.len(),
        poorly.len()
    );
    assert_eq!(
        zstd::stream::decode_all(&physical[..]).unwrap(),
        data,
        "re-compressed stream must still decode to the original content"
    );

    // Control: an already-well-compressed upload is kept as-is (never enlarged).
    let (zstd2, _store2, inner2) = build(&spec).await?;
    let well = zstd::bulk::compress(&data, 19).unwrap();
    zstd2
        .update_zstd_oneshot(digest, DigestHasherFunc::Sha256, Bytes::from(well.clone()))
        .await?;
    let physical2 = inner2.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(
        &physical2[..],
        &well[..],
        "a well-compressed upload must be kept byte-for-byte, not enlarged"
    );
    Ok(())
}

#[nativelink_test]
async fn update_zstd_skips_recompression_above_threshold() -> Result<(), Error> {
    let data = compressible_data(64 * 1024);
    let digest = digest_for(&data);

    let mut spec = spec_for(make_temp_path("zstd-above-threshold"));
    spec.compression_level = Some(19);
    spec.max_recompression_size = 16; // Uncompressed size far exceeds this.
    let (zstd, _store, inner) = build(&spec).await?;

    let poorly = zstd::bulk::compress(&data, 1).unwrap();
    zstd.update_zstd_oneshot(
        digest,
        DigestHasherFunc::Sha256,
        Bytes::from(poorly.clone()),
    )
    .await?;
    let physical = inner.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(
        &physical[..],
        &poorly[..],
        "above max_recompression_size the stored stream must equal the upload byte-for-byte"
    );
    Ok(())
}

#[nativelink_test]
async fn get_for_batch_selects_zstd_or_raw() -> Result<(), Error> {
    let (zstd, store, _inner) = build(&spec()).await?;
    let wire_store = Store::new(zstd.clone())
        .wire_compression_store()
        .expect("ZstdStore must expose the wire-compression capability");

    // Compressible blob (stored via the identity path so inner holds zstd).
    let compressible = vec![0u8; 4096];
    let comp_digest = digest_for(&compressible);
    store
        .update_oneshot(comp_digest, compressible.clone().into())
        .await?;

    let (payload, compressor) = wire_store
        .clone()
        .get_for_batch(comp_digest, &[WireCompressor::Zstd])
        .await?;
    assert_eq!(
        compressor,
        Some(WireCompressor::Zstd),
        "compressible blob with accepts_zstd must return zstd"
    );
    assert!(
        (payload.len() as u64) < comp_digest.size_bytes(),
        "physical zstd must be smaller than the uncompressed size"
    );
    assert_eq!(
        zstd::stream::decode_all(&payload[..]).unwrap(),
        compressible,
        "returned zstd must decode to the original content"
    );

    // Same blob, but the client does not accept zstd => raw.
    let (raw, compressor) = wire_store.clone().get_for_batch(comp_digest, &[]).await?;
    assert_eq!(compressor, None);
    assert_eq!(
        &raw[..],
        &compressible[..],
        "must return raw when zstd not accepted"
    );

    // Incompressible blob: physical zstd is not smaller => always raw.
    let incompressible = random_bytes(2048);
    let inc_digest = digest_for(&incompressible);
    store
        .update_oneshot(inc_digest, incompressible.clone().into())
        .await?;
    let (raw, compressor) = wire_store
        .get_for_batch(inc_digest, &[WireCompressor::Zstd])
        .await?;
    assert_eq!(
        compressor, None,
        "incompressible blob must not be served as zstd"
    );
    assert_eq!(&raw[..], &incompressible[..]);
    Ok(())
}

#[nativelink_test]
async fn get_for_batch_decodes_concatenated_frames() -> Result<(), Error> {
    // Regression for `get_for_batch`'s raw-decode path: the physical stream
    // stored for a blob can be two independently-compressed zstd frames
    // concatenated together (see `get_zstd_preserves_concatenated_frames`).
    // `zstd::bulk::decompress` must handle that multi-frame input, not just a
    // single-frame stream, when the client does not accept zstd.
    const PART_A: &[u8] = b"first frame content aaaaaaaaaaaaaaaaaaaaaaaa";
    const PART_B: &[u8] = b"second frame content bbbbbbbbbbbbbbbbbbbbbbbb";

    let frame_a = zstd::bulk::compress(PART_A, 3).unwrap();
    let frame_b = zstd::bulk::compress(PART_B, 3).unwrap();

    let mut raw = PART_A.to_vec();
    raw.extend_from_slice(PART_B);
    let digest = digest_for(&raw);

    let (zstd, _store, _inner) = build(&spec()).await?;
    zstd.update_zstd(
        digest,
        DigestHasherFunc::Sha256,
        reader_from(vec![Bytes::from(frame_a), Bytes::from(frame_b)]),
    )
    .await?;

    // Client does not accept zstd => `get_for_batch` must decode the stored
    // (concatenated-frame) physical bytes to the full raw content.
    let (payload, is_zstd) = zstd.get_for_batch(digest, false).await?;
    assert!(
        !is_zstd,
        "client that does not accept zstd must get raw bytes"
    );
    assert_eq!(
        &payload[..],
        &raw[..],
        "get_for_batch must decode a concatenated-frame physical stream to the full raw content"
    );
    Ok(())
}

#[nativelink_test]
async fn zero_digest_zstd_fast_path() -> Result<(), Error> {
    let recording = Arc::new(RecordingStore::default());
    std::fs::create_dir_all(TEMP_PATH).map_err(|e| make_err!(Code::Internal, "temp dir: {e}"))?;
    let zstd = ZstdStore::new(&spec(), Store::new(recording.clone()))?;

    let zero = ZERO_BYTE_DIGESTS[0];

    // get_zstd yields a valid zstd stream that decodes to empty.
    let got = collect_zstd(&zstd, zero).await?;
    assert!(
        !got.is_empty(),
        "zero-digest get_zstd must emit a real zstd frame"
    );
    assert_eq!(
        zstd::stream::decode_all(&got[..]).unwrap().len(),
        0,
        "zero-digest zstd stream must decode to empty"
    );

    // update_zstd of an empty-decoding stream succeeds without touching inner.
    let empty = zstd::bulk::compress(&[], 3).unwrap();
    let wire = zstd
        .update_zstd_oneshot(zero, DigestHasherFunc::Sha256, Bytes::from(empty.clone()))
        .await?;
    assert_eq!(wire, empty.len() as u64);
    assert_eq!(
        recording.update_count.load(Ordering::Relaxed),
        0,
        "zero digest must never reach the inner store"
    );

    // A stream that decodes to non-empty content is rejected for a zero digest.
    let non_empty = zstd::bulk::compress(b"not empty", 3).unwrap();
    let err = zstd
        .update_zstd_oneshot(zero, DigestHasherFunc::Sha256, Bytes::from(non_empty))
        .await
        .expect_err("non-empty content under a zero digest must be rejected");
    assert_eq!(err.code, Code::InvalidArgument);
    Ok(())
}

#[nativelink_test]
async fn cancelled_update_zstd_leaves_no_temp_files() -> Result<(), Error> {
    let temp = make_temp_path("zstd-cancel");
    let (zstd, _store, _inner) = build(&spec_for(temp.clone())).await?;

    let data = compressible_data(256 * 1024);
    let compressed = zstd::bulk::compress(&data, 3).unwrap();
    let digest = digest_for(&data);

    let (mut tx, rx) = make_buf_channel_pair();
    let zstd_clone = zstd.clone();
    // `spawn!` returns a guard that aborts the task when it is dropped.
    let handle = spawn!("zstd_test_cancel", async move {
        zstd_clone
            .update_zstd(digest, DigestHasherFunc::Sha256, rx)
            .await
    });

    // Send only the first half of the compressed stream, never EOF.
    let half = compressed.len() / 2;
    tx.send(Bytes::copy_from_slice(&compressed[..half]))
        .await
        .ok();
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Cancel the upload mid-stream (dropping the guard aborts it), then
    // release the sender.
    drop(handle);
    drop(tx);
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert_eq!(
        dir_entry_count(&temp),
        0,
        "a cancelled upload must not leave staged temp files"
    );
    Ok(())
}

#[nativelink_test]
async fn concurrent_uploads_respect_staging_semaphore() -> Result<(), Error> {
    const DATA_A: &[u8] = b"concurrent upload alpha aaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const DATA_B: &[u8] = b"concurrent upload beta bbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    let mut spec = spec_for(make_temp_path("zstd-semaphore"));
    spec.max_concurrent_staged_uploads = 1; // Serialize staging.
    let (zstd, store, _inner) = build(&spec).await?;

    let digest_a = digest_for(DATA_A);
    let digest_b = digest_for(DATA_B);
    let comp_a = zstd::bulk::compress(DATA_A, 3).unwrap();
    let comp_b = zstd::bulk::compress(DATA_B, 3).unwrap();

    let za = zstd.clone();
    let zb = zstd.clone();
    let (ra, rb) = tokio::join!(
        za.update_zstd_oneshot(digest_a, DigestHasherFunc::Sha256, Bytes::from(comp_a)),
        zb.update_zstd_oneshot(digest_b, DigestHasherFunc::Sha256, Bytes::from(comp_b)),
    );
    ra?;
    rb?;

    assert_eq!(
        &store.get_part_unchunked(digest_a, 0, None).await?[..],
        DATA_A,
        "first concurrent upload must round-trip"
    );
    assert_eq!(
        &store.get_part_unchunked(digest_b, 0, None).await?[..],
        DATA_B,
        "second concurrent upload must round-trip"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Cross-wrapper integration: ZstdStore composed with other stores.
//
// These prove ZstdStore is a well-behaved participant in a store stack. Every
// test does a full identity round-trip (write raw via `update_oneshot`, read raw
// back via `get_part_unchunked`) through the composed stack. Where a wrapper
// lives INSIDE ZstdStore (so it observes the physical zstd stream) the test also
// asserts that wrapper's side effect still fires.
//
// Identity-path tests do not touch `temp_path` (only the zstd fast path stages
// files), so they reuse the shared `spec()` without creating a staging dir.
// ---------------------------------------------------------------------------

// 1. ZstdStore over `fast_slow` (fast=memory, slow=memory): round-trip, then
//    prove a slow-hit read repopulates the fast tier with the physical zstd blob.
#[nativelink_test]
async fn over_fast_slow_populates_fast_tier() -> Result<(), Error> {
    let fast_mem = MemoryStore::new(&MemorySpec::default());
    let slow_mem = MemoryStore::new(&MemorySpec::default());
    let fast_slow = Store::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
            bypass_dedup_threshold_bytes: 0,
        },
        Store::new(fast_mem.clone()),
        Store::new(slow_mem.clone()),
    ));
    let store = Store::new(ZstdStore::new(&spec(), fast_slow)?);

    let data = compressible_data(64 * 1024);
    let digest = digest_for(&data);
    store.update_oneshot(digest, data.clone().into()).await?;
    assert_eq!(
        &store.get_part_unchunked(digest, 0, None).await?[..],
        &data[..],
        "round-trip through zstd-over-fast_slow must return the raw bytes"
    );

    // The physical (compressed) bytes landed in both tiers; capture the slow copy.
    let physical = slow_mem.get_part_unchunked(digest, 0, None).await?;
    assert!(
        physical.len() < data.len(),
        "inner store must hold the compressed physical stream ({} !< {})",
        physical.len(),
        data.len()
    );

    // Evict the fast tier, leaving only the slow tier populated.
    assert!(
        fast_mem.remove_entry(digest.into()).await,
        "fast tier should have held the blob before eviction"
    );
    assert_eq!(
        fast_mem.has(digest).await?,
        None,
        "fast tier must be empty after eviction"
    );

    // A read through the stack is a slow-hit that must repopulate the fast tier.
    assert_eq!(
        &store.get_part_unchunked(digest, 0, None).await?[..],
        &data[..],
        "slow-hit read-through must still return the raw bytes"
    );
    assert_eq!(
        fast_mem.has(digest).await?,
        Some(physical.len() as u64),
        "fast tier must be repopulated with the physical zstd blob"
    );
    assert_eq!(
        &fast_mem.get_part_unchunked(digest, 0, None).await?[..],
        &physical[..],
        "the repopulated fast-tier bytes must be the physical zstd stream"
    );
    Ok(())
}

// 2. ZstdStore over `dedup`: a blob whose physical stream exceeds the dedup block
//    size must split across multiple content chunks yet round-trip byte-for-byte.
#[nativelink_test]
async fn over_dedup_splits_and_round_trips() -> Result<(), Error> {
    let content_mem = MemoryStore::new(&MemorySpec::default());
    let dedup = Store::new(DedupStore::new(
        &DedupSpec {
            index_store: StoreSpec::Memory(MemorySpec::default()),
            content_store: StoreSpec::Memory(MemorySpec::default()),
            min_size: 8 * 1024,
            normal_size: 32 * 1024,
            max_size: 128 * 1024,
            max_concurrent_fetch_per_get: 10,
        },
        Store::new(MemoryStore::new(&MemorySpec::default())),
        Store::new(content_mem.clone()),
    )?);
    let store = Store::new(ZstdStore::new(&spec(), dedup)?);

    // Incompressible data so the physical zstd stream stays ~256 KiB, comfortably
    // above the 128 KiB max block size and therefore split into several chunks.
    let data = random_bytes(256 * 1024);
    let digest = digest_for(&data);
    store.update_oneshot(digest, data.clone().into()).await?;
    assert_eq!(
        &store.get_part_unchunked(digest, 0, None).await?[..],
        &data[..],
        "round-trip through zstd-over-dedup must return the raw bytes"
    );
    assert!(
        content_mem.len_for_test() > 1,
        "dedup must split the physical zstd stream into multiple content chunks, got {}",
        content_mem.len_for_test()
    );
    Ok(())
}

// 3. ZstdStore over `existence_cache`: round-trip and prove the existence cache
//    side effect (population on write) still fires for the forwarded digest.
#[nativelink_test]
async fn over_existence_cache_fires_side_effect() -> Result<(), Error> {
    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let existence = ExistenceCacheStore::new(
        &ExistenceCacheSpec {
            backend: StoreSpec::Noop(NoopSpec::default()), // Unused: inner is passed directly.
            eviction_policy: None,
        },
        inner,
    );
    let store = Store::new(ZstdStore::new(&spec(), Store::new(existence.clone()))?);

    let data = compressible_data(4096);
    let digest = digest_for(&data);
    assert!(
        !existence.exists_in_cache(&digest).await,
        "digest must not be cached before the write"
    );

    store.update_oneshot(digest, data.clone().into()).await?;
    assert!(
        existence.exists_in_cache(&digest).await,
        "write through zstd must populate the inner existence cache"
    );
    assert_eq!(
        &store.get_part_unchunked(digest, 0, None).await?[..],
        &data[..],
        "round-trip through zstd-over-existence_cache must return the raw bytes"
    );
    // `has` is served from the now-populated existence cache and reports the
    // uncompressed digest size (not the physical zstd size).
    assert_eq!(
        store.has(digest).await?,
        Some(data.len() as u64),
        "has must report the uncompressed size via the existence cache"
    );
    Ok(())
}

// 4. ZstdStore over `cache_metrics`: round-trip through the metrics wrapper. The
//    metrics themselves are process-global OpenTelemetry counters (not unit
//    assertable without a metrics-reader harness, mirroring the existing
//    `cache_metrics_store_test.rs`), so we assert that operations route through
//    the wrapper and that zstd compression is still applied behind it.
#[nativelink_test]
async fn over_cache_metrics_round_trips() -> Result<(), Error> {
    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let metrics = CacheMetricsStore::new(
        &CacheMetricsSpec {
            cache_type: "cas".to_string(),
            backend: StoreSpec::Memory(MemorySpec::default()), // Unused: inner is passed directly.
        },
        inner.clone(),
    );
    let store = Store::new(ZstdStore::new(&spec(), Store::new(metrics))?);

    let data = compressible_data(4096);
    let digest = digest_for(&data);
    store.update_oneshot(digest, data.clone().into()).await?;
    assert_eq!(
        &store.get_part_unchunked(digest, 0, None).await?[..],
        &data[..],
        "round-trip through zstd-over-cache_metrics must return the raw bytes"
    );
    assert_eq!(store.has(digest).await?, Some(data.len() as u64));
    let physical = inner.get_part_unchunked(digest, 0, None).await?;
    assert!(
        physical.len() < data.len(),
        "zstd compression must still be applied behind the metrics wrapper"
    );
    Ok(())
}

// 5a. ZstdStore over `size_partitioning`: identity round-trip; the physical zstd
//     stream is routed by the (uncompressed) digest size.
#[nativelink_test]
async fn over_size_partitioning_round_trips() -> Result<(), Error> {
    let lower = MemoryStore::new(&MemorySpec::default());
    let upper = MemoryStore::new(&MemorySpec::default());
    let size_part = Store::new(SizePartitioningStore::new(
        &SizePartitioningSpec {
            size: 100,
            lower_store: StoreSpec::Memory(MemorySpec::default()),
            upper_store: StoreSpec::Memory(MemorySpec::default()),
        },
        Store::new(lower.clone()),
        Store::new(upper.clone()),
    ));
    let store = Store::new(ZstdStore::new(&spec(), size_part)?);

    // Uncompressed size (4096) >= partition threshold (100) => routed to `upper`.
    let data = compressible_data(4096);
    let digest = digest_for(&data);
    store.update_oneshot(digest, data.clone().into()).await?;
    assert_eq!(
        &store.get_part_unchunked(digest, 0, None).await?[..],
        &data[..],
        "round-trip through zstd-over-size_partitioning must return the raw bytes"
    );
    assert!(
        upper.has(digest).await?.is_some(),
        "a blob above the partition threshold must be stored in the upper partition"
    );
    assert_eq!(
        lower.has(digest).await?,
        None,
        "the lower partition must not hold the blob"
    );
    Ok(())
}

// 5b. ZstdStore over `shard`: identity round-trip through a two-way shard.
#[nativelink_test]
async fn over_shard_round_trips() -> Result<(), Error> {
    let shard = Store::new(ShardStore::new(
        &ShardSpec {
            stores: vec![
                ShardConfig {
                    store: StoreSpec::Memory(MemorySpec::default()),
                    weight: Some(1),
                },
                ShardConfig {
                    store: StoreSpec::Memory(MemorySpec::default()),
                    weight: Some(1),
                },
            ],
        },
        vec![
            Store::new(MemoryStore::new(&MemorySpec::default())),
            Store::new(MemoryStore::new(&MemorySpec::default())),
        ],
    )?);
    let store = Store::new(ZstdStore::new(&spec(), shard)?);

    let data = compressible_data(4096);
    let digest = digest_for(&data);
    store.update_oneshot(digest, data.clone().into()).await?;
    assert_eq!(
        &store.get_part_unchunked(digest, 0, None).await?[..],
        &data[..],
        "round-trip through zstd-over-shard must return the raw bytes"
    );
    assert_eq!(store.has(digest).await?, Some(data.len() as u64));
    Ok(())
}

// 5c. ZstdStore over `ref`: identity round-trip through a ref store that resolves
//     to a named memory backend via the StoreManager.
#[nativelink_test]
async fn over_ref_round_trips() -> Result<(), Error> {
    let store_manager = Arc::new(StoreManager::new());
    let backing = Store::new(MemoryStore::new(&MemorySpec::default()));
    store_manager.add_store("backing", backing.clone())?;
    let ref_store = Store::new(RefStore::new(
        &RefSpec {
            name: "backing".to_string(),
        },
        Arc::downgrade(&store_manager),
    ));
    store_manager.add_store("ref", ref_store.clone())?;
    store_manager.run_post_init().await.unwrap();

    let store = Store::new(ZstdStore::new(&spec(), ref_store)?);

    let data = compressible_data(4096);
    let digest = digest_for(&data);
    store.update_oneshot(digest, data.clone().into()).await?;
    assert_eq!(
        &store.get_part_unchunked(digest, 0, None).await?[..],
        &data[..],
        "round-trip through zstd-over-ref must return the raw bytes"
    );
    // The resolved backing store must physically hold the compressed stream.
    let physical = backing.get_part_unchunked(digest, 0, None).await?;
    assert!(
        physical.len() < data.len(),
        "the ref-resolved backend must hold the compressed physical stream"
    );
    Ok(())
}

// 6. A wrapper OUTSIDE ZstdStore (cache_metrics -> zstd_store -> memory). This
//    exercises only the StoreDriver identity path (the service zstd fast path is
//    unreachable through an outer wrapper), proving that placing a wrapper
//    outside ZstdStore preserves identity correctness and does not bypass the
//    outer wrapper's operations while zstd compression stays active underneath.
#[nativelink_test]
async fn wrapper_outside_zstd_preserves_identity() -> Result<(), Error> {
    let backing = Store::new(MemoryStore::new(&MemorySpec::default()));
    let zstd = Store::new(ZstdStore::new(&spec(), backing.clone())?);
    let outer = Store::new(CacheMetricsStore::new(
        &CacheMetricsSpec {
            cache_type: "cas".to_string(),
            backend: StoreSpec::Memory(MemorySpec::default()), // Unused: zstd is passed directly.
        },
        zstd,
    ));

    let data = compressible_data(4096);
    let digest = digest_for(&data);
    outer.update_oneshot(digest, data.clone().into()).await?;
    assert_eq!(
        &outer.get_part_unchunked(digest, 0, None).await?[..],
        &data[..],
        "identity round-trip through an outer wrapper must return the raw bytes"
    );
    assert_eq!(
        outer.has(digest).await?,
        Some(data.len() as u64),
        "the outer wrapper must report the uncompressed size"
    );
    let physical = backing.get_part_unchunked(digest, 0, None).await?;
    assert!(
        physical.len() < data.len(),
        "zstd compression must still be active behind the outer wrapper"
    );
    Ok(())
}

// 7. Concatenated-frame integrity through a wrapper: upload two concatenated zstd
//    frames (fast path) into a ZstdStore-over-fast_slow, read back via `get_zstd`
//    byte-for-byte, and via the identity `get_part` decode to the full content.
#[nativelink_test]
async fn concatenated_frames_through_fast_slow() -> Result<(), Error> {
    const PART_A: &[u8] = b"first frame content aaaaaaaaaaaaaaaaaaaaaaaa";
    const PART_B: &[u8] = b"second frame content bbbbbbbbbbbbbbbbbbbbbbbb";

    let frame_a = zstd::bulk::compress(PART_A, 3).unwrap();
    let frame_b = zstd::bulk::compress(PART_B, 3).unwrap();
    let mut concatenated = frame_a.clone();
    concatenated.extend_from_slice(&frame_b);

    let mut raw = PART_A.to_vec();
    raw.extend_from_slice(PART_B);
    let digest = digest_for(&raw);

    // The fast path stages files, so a real writable temp dir is required.
    let temp = make_temp_path("zstd-concat-fast-slow");
    std::fs::create_dir_all(&temp)
        .map_err(|e| make_err!(Code::Internal, "Failed to create test temp dir: {e}"))?;
    let fast_slow = Store::new(FastSlowStore::new(
        &FastSlowSpec {
            fast: StoreSpec::Memory(MemorySpec::default()),
            slow: StoreSpec::Memory(MemorySpec::default()),
            fast_direction: StoreDirection::default(),
            slow_direction: StoreDirection::default(),
            bypass_dedup_threshold_bytes: 0,
        },
        Store::new(MemoryStore::new(&MemorySpec::default())),
        Store::new(MemoryStore::new(&MemorySpec::default())),
    ));
    let zstd = ZstdStore::new(&spec_for(temp), fast_slow)?;
    let store = Store::new(zstd.clone());

    let wire = zstd
        .update_zstd(
            digest,
            DigestHasherFunc::Sha256,
            reader_from(vec![Bytes::from(frame_a), Bytes::from(frame_b)]),
        )
        .await?;
    assert_eq!(wire, concatenated.len() as u64);

    let got = collect_zstd(&zstd, digest).await?;
    assert_eq!(
        &got[..],
        &concatenated[..],
        "passthrough through fast_slow must preserve the exact concatenated-frame bytes"
    );

    // The identity view decodes the concatenated frames to the full content.
    let decoded = store.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(
        &decoded[..],
        &raw[..],
        "the identity read must decode the concatenated frames to the full raw content"
    );
    Ok(())
}

// 8. Rollout from an empty namespace: a fresh memory backend, a full write->read
//    cycle, confirming the digest is absent before the write and present after.
#[nativelink_test]
async fn rollout_from_empty_namespace() -> Result<(), Error> {
    const DATA: &[u8] = b"rollout payload from an empty dedicated namespace aaaaaaaaaa";

    let inner = Store::new(MemoryStore::new(&MemorySpec::default()));
    let store = Store::new(ZstdStore::new(&spec(), inner.clone())?);
    let digest = digest_for(DATA);

    assert_eq!(
        store.has(digest).await?,
        None,
        "an empty namespace must report the digest as absent"
    );
    assert_eq!(
        inner.has(digest).await?,
        None,
        "the inner backend must start empty"
    );

    store.update_oneshot(digest, DATA.into()).await?;
    assert_eq!(
        &store.get_part_unchunked(digest, 0, None).await?[..],
        DATA,
        "the rollout write->read cycle must return the raw bytes"
    );
    assert_eq!(
        store.has(digest).await?,
        Some(DATA.len() as u64),
        "after the write the digest must be present at its uncompressed size"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Security & starvation hardening (bounded decode, descriptor pinning, commit
// deadline). See the PR's issues 1-3.
// ---------------------------------------------------------------------------

/// Inner store that, on `update_with_whole_file`, first *replaces* the staged
/// pathname's contents (simulating an observe-and-replace attacker) and then
/// reads the retained descriptor it was handed, storing whatever the descriptor
/// yields into an inner `MemoryStore`. If the zstd store commits from the
/// validated descriptor (as it must), the stored bytes are the validated stream
/// regardless of what happened to the pathname.
#[derive(MetricsComponent)]
struct ClobberingStore {
    #[metric(group = "inner")]
    inner: Store,
}

#[async_trait]
impl StoreDriver for ClobberingStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        self.inner.has_with_results(keys, results).await
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        self.inner
            .as_store_driver_pin()
            .update(key, reader, size_info)
            .await
    }

    async fn update_with_whole_file(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        path: OsString,
        mut file: FileSlot,
        _upload_size: UploadSizeInfo,
    ) -> Result<(u64, Option<FileSlot>), Error> {
        // Replace the pathname with attacker-controlled content *after* the zstd
        // store validated the descriptor. Unlink first so the retained fd points
        // at a now-orphaned inode, then create a fresh file at the same path.
        drop(std::fs::remove_file(&path));
        std::fs::write(&path, b"CLOBBERED-BY-ATTACKER").expect("clobber write");

        // Read the *descriptor* the store handed us (the validated bytes).
        file.rewind()
            .await
            .map_err(|e| make_err!(Code::Internal, "rewind clobber fd: {e}"))?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)
            .await
            .map_err(|e| make_err!(Code::Internal, "read clobber fd: {e}"))?;
        let size = buf.len() as u64;
        self.inner
            .update_oneshot(key.into_digest(), Bytes::from(buf))
            .await?;
        Ok((size, None))
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        self.inner
            .as_store_driver_pin()
            .get_part(key, writer, offset, length)
            .await
    }

    fn inner_store(&self, _key: Option<StoreKey>) -> &dyn StoreDriver {
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
        _callback: RemoveCallback,
    ) -> Result<(), Error> {
        Ok(())
    }
}

default_health_status_indicator!(ClobberingStore);

/// Inner store whose whole-file commit never resolves, used to prove the zstd
/// store bounds a stalled backend commit with its `commit_timeout` and releases
/// the staging permit afterwards.
#[derive(Default, MetricsComponent)]
struct StallStore {}

#[async_trait]
impl StoreDriver for StallStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

    async fn has_with_results(
        self: Pin<&Self>,
        _keys: &[StoreKey<'_>],
        _results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        _reader: DropCloserReadHalf,
        _size_info: UploadSizeInfo,
    ) -> Result<u64, Error> {
        // Never completes.
        core::future::pending::<()>().await;
        unreachable!("StallStore::update never resolves")
    }

    async fn update_with_whole_file(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        _path: OsString,
        _file: FileSlot,
        _upload_size: UploadSizeInfo,
    ) -> Result<(u64, Option<FileSlot>), Error> {
        // Simulate a permanently stalled backend commit.
        core::future::pending::<()>().await;
        unreachable!("StallStore::update_with_whole_file never resolves")
    }

    async fn get_part(
        self: Pin<&Self>,
        _key: StoreKey<'_>,
        _writer: &mut DropCloserWriteHalf,
        _offset: u64,
        _length: Option<u64>,
    ) -> Result<(), Error> {
        Err(make_err!(Code::NotFound, "Not found"))
    }

    fn inner_store(&self, _key: Option<StoreKey>) -> &dyn StoreDriver {
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
        _callback: RemoveCallback,
    ) -> Result<(), Error> {
        Ok(())
    }
}

default_health_status_indicator!(StallStore);

/// A compressed stream that decodes to non-empty content is rejected under a
/// zero digest — via both the streaming and one-shot entry points — and stops
/// at the output sink without materializing the (large) decoded output.
#[nativelink_test]
async fn zero_digest_decoding_nonempty_is_rejected_at_sink() -> Result<(), Error> {
    let temp = make_temp_path("zstd-zero-bomb");
    let (zstd, _store, _inner) = build(&spec_for(temp.clone())).await?;

    // 16 MiB of zeros compresses to a few KiB. Under a zero digest the decoded
    // output cap is 0, so the first decoded block is rejected immediately — the
    // 16 MiB is never materialized (the test would be far slower / OOM if it
    // were).
    let bomb = zstd::bulk::compress(&vec![0u8; 16 * 1024 * 1024], 3).unwrap();
    assert!(
        bomb.len() < 64 * 1024,
        "bomb should be tiny compressed ({} bytes)",
        bomb.len()
    );
    let zero = ZERO_BYTE_DIGESTS[0];

    // One-shot entry point.
    let err = zstd
        .update_zstd_oneshot(zero, DigestHasherFunc::Sha256, Bytes::from(bomb.clone()))
        .await
        .expect_err("a non-empty-decoding zero-digest upload must be rejected");
    assert_eq!(err.code, Code::InvalidArgument, "got: {err}");

    // Streaming entry point.
    let err = zstd
        .update_zstd(
            zero,
            DigestHasherFunc::Sha256,
            reader_from(vec![Bytes::from(bomb)]),
        )
        .await
        .expect_err("a non-empty-decoding zero-digest stream must be rejected");
    assert_eq!(err.code, Code::InvalidArgument, "got: {err}");

    assert_eq!(
        dir_entry_count(&temp),
        0,
        "zero-digest validation must never stage a temp file"
    );
    Ok(())
}

/// A non-zero upload whose decoded output exceeds the declared digest size is
/// stopped at the decoder sink (`InvalidArgument`), commits nothing, and leaves
/// no staging file. The decoded output (256 KiB) dwarfs the declared size (100
/// bytes), so the sink rejects the very first over-limit block.
#[nativelink_test]
async fn decoded_output_exceeding_digest_size_is_stopped_at_sink() -> Result<(), Error> {
    let temp = make_temp_path("zstd-decode-overflow");
    let (zstd, _store, inner) = build(&spec_for(temp.clone())).await?;

    let data = compressible_data(256 * 1024);
    let compressed = zstd::bulk::compress(&data, 3).unwrap();
    // Claim a digest for only the first 100 decoded bytes: the actual decoded
    // stream is far larger than the declared size.
    let bad_digest = digest_for(&data[..100]);
    assert_eq!(bad_digest.size_bytes(), 100);

    let err = zstd
        .update_zstd_oneshot(
            bad_digest,
            DigestHasherFunc::Sha256,
            Bytes::from(compressed),
        )
        .await
        .expect_err("decoded output larger than the digest size must be rejected");
    assert_eq!(err.code, Code::InvalidArgument, "got: {err}");

    assert_eq!(
        inner.has(bad_digest).await?,
        None,
        "an over-decoding upload must not commit to the inner store"
    );
    assert_eq!(
        dir_entry_count(&temp),
        0,
        "an over-decoding upload must not leave staged temp files"
    );
    Ok(())
}

/// The commit streams the exact validated descriptor, not a reopened pathname:
/// replacing the pathname's contents after validation cannot change what is
/// committed.
#[nativelink_test]
async fn commit_uses_validated_descriptor_not_reopened_path() -> Result<(), Error> {
    const DATA: &[u8] = b"descriptor-pinned commit payload aaaaaaaaaaaaaaaaaaaaaaaa";

    let temp = make_temp_path("zstd-fd-pin");
    std::fs::create_dir_all(&temp).map_err(|e| make_err!(Code::Internal, "temp dir: {e}"))?;
    let inner_mem = Store::new(MemoryStore::new(&MemorySpec::default()));
    let clobber = Store::new(Arc::new(ClobberingStore {
        inner: inner_mem.clone(),
    }));
    let zstd = ZstdStore::new(&spec_for(temp.clone()), clobber)?;

    let compressed = zstd::bulk::compress(DATA, 3).unwrap();
    let digest = digest_for(DATA);

    zstd.update_zstd_oneshot(
        digest,
        DigestHasherFunc::Sha256,
        Bytes::from(compressed.clone()),
    )
    .await?;

    // The bytes the inner store committed came from the retained descriptor, so
    // they equal the validated stream — NOT the "CLOBBERED-BY-ATTACKER" content
    // that replaced the pathname during the commit.
    let committed = inner_mem.get_part_unchunked(digest, 0, None).await?;
    assert_eq!(
        &committed[..],
        &compressed[..],
        "committed bytes must be the validated descriptor's contents, not the replaced pathname's"
    );
    assert_eq!(
        zstd::stream::decode_all(&committed[..]).unwrap(),
        DATA,
        "the descriptor-committed stream must still decode to the original content"
    );
    assert_eq!(
        dir_entry_count(&temp),
        0,
        "a successful commit must leave no staged temp files"
    );
    Ok(())
}

/// A permanently stalled backend commit is bounded by `commit_timeout`: the
/// upload fails with `DeadlineExceeded`, the staged file is removed, and the
/// staging permit is released — so a second upload behind a `max_concurrent
/// staged_uploads = 1` bound is admitted rather than blocked forever.
// Uses a short real-time `commit_timeout_s = 1` rather than a paused clock:
// the `nativelink-store` test crate does not enable tokio's `test-util`
// feature, so `start_paused` is unavailable here.
#[nativelink_test]
async fn stalled_commit_times_out_and_releases_staging_slot() -> Result<(), Error> {
    const DATA: &[u8] = b"payload whose commit stalls forever bbbbbbbbbbbbbbbbbbbb";

    let temp = make_temp_path("zstd-commit-stall");
    let mut spec = spec_for(temp.clone());
    spec.max_concurrent_staged_uploads = 1;
    spec.commit_timeout_s = 1;
    std::fs::create_dir_all(&temp).map_err(|e| make_err!(Code::Internal, "temp dir: {e}"))?;
    let zstd = ZstdStore::new(&spec, Store::new(Arc::new(StallStore {})))?;

    let compressed = zstd::bulk::compress(DATA, 3).unwrap();
    let digest = digest_for(DATA);

    let err = zstd
        .update_zstd_oneshot(
            digest,
            DigestHasherFunc::Sha256,
            Bytes::from(compressed.clone()),
        )
        .await
        .expect_err("a stalled commit must fail rather than hang");
    assert_eq!(
        err.code,
        Code::DeadlineExceeded,
        "a stalled commit must surface DeadlineExceeded, got: {err}"
    );
    assert_eq!(
        dir_entry_count(&temp),
        0,
        "a timed-out commit must remove its staged temp file"
    );

    // If the first upload had not released the staging permit, this second
    // upload (staging bound = 1) would block on the semaphore forever and the
    // test would hang. It resolving at all proves the slot was freed.
    let err2 = zstd
        .update_zstd_oneshot(digest, DigestHasherFunc::Sha256, Bytes::from(compressed))
        .await
        .expect_err("second stalled commit must also time out, not hang");
    assert_eq!(err2.code, Code::DeadlineExceeded, "got: {err2}");
    assert_eq!(
        dir_entry_count(&temp),
        0,
        "the second timed-out commit must also clean up"
    );
    Ok(())
}
