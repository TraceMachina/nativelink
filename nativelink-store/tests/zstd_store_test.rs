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
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use nativelink_config::stores::{MemorySpec, StoreSpec, ZstdStoreSpec};
use nativelink_error::Code;
use nativelink_error::{Error, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_store::cas_utils::ZERO_BYTE_DIGESTS;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::zstd_store::ZstdStore;
use nativelink_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::store_trait::{
    RemoveItemCallback, Store, StoreDriver, StoreKey, StoreLike, UploadSizeInfo,
};
use pretty_assertions::assert_eq;
use sha2::{Digest, Sha256};

#[derive(Default, MetricsComponent)]
struct RecordingStore {
    update_count: AtomicUsize,
}

#[async_trait]
impl StoreDriver for RecordingStore {
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
