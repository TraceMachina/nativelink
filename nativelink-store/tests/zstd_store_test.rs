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
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use nativelink_config::stores::{MemorySpec, StoreSpec, ZstdStoreSpec};
use nativelink_error::{Code, Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_store::cas_utils::{ZERO_BYTE_DIGESTS, is_zero_digest};
use nativelink_store::default_store_factory::store_factory;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_store::zstd_store::ZstdStore;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc, make_ctx_for_hash_func};
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    RemoveItemCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use opentelemetry::context::FutureExt;
use pretty_assertions::assert_eq;
use sha2::{Digest, Sha256};

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
        _callback: Arc<dyn RemoveItemCallback>,
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
        _callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        Ok(())
    }
}

default_health_status_indicator!(ChunkingStore);

const TEMP_PATH: &str = "/tmp/nativelink-zstd-store-test";

fn spec() -> ZstdStoreSpec {
    ZstdStoreSpec {
        backend: StoreSpec::Memory(MemorySpec::default()),
        temp_path: TEMP_PATH.to_string(),
        max_compressed_upload_size: 512 * 1024 * 1024,
        max_concurrent_staged_uploads: 0,
        compression_level: None,
        max_recompression_size: 0,
        max_concurrent_recompressions: 0,
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
async fn factory_builds_zstd_store() -> Result<(), Error> {
    let store_spec = StoreSpec::ZstdStore(Box::new(spec()));
    let store_manager = Arc::new(StoreManager::new());
    let store = store_factory(&store_spec, &store_manager, None).await?;
    assert!(
        store.downcast_ref_immediate::<ZstdStore>().is_some(),
        "Expected store_factory to build a ZstdStore for StoreSpec::ZstdStore"
    );
    Ok(())
}
