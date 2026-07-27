// Copyright 2024 The NativeLink Authors. All rights reserved.
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
use core::str::from_utf8;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::io::Cursor;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use bytes::Bytes;
use nativelink_config::stores::{CompressionSpec, MemorySpec, StoreSpec};
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_macro::nativelink_test;
use nativelink_metric::MetricsComponent;
use nativelink_store::cas_utils::ZERO_BYTE_DIGESTS;
use nativelink_store::compression_store::{
    CURRENT_STREAM_FORMAT_VERSION, CompressionStore, DEFAULT_BLOCK_SIZE, FOOTER_FRAME_TYPE, Footer,
    Lz4Config, SliceIndex, WincodeConfig,
};
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair,
};
use nativelink_util::common::DigestInfo;
use nativelink_util::health_utils::{HealthStatusIndicator, default_health_status_indicator};
use nativelink_util::spawn;
use nativelink_util::store_trait::{
    RemoveItemCallback, Store, StoreKey, StoreLike, UploadSizeInfo,
};
use pretty_assertions::assert_eq;
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use tokio::io::AsyncReadExt;

/// Utility function that will build a Footer object from the input.
fn extract_footer(data: &[u8]) -> Result<Footer, Error> {
    let mut pos = data.len() - 1; // Skip version byte(u8)
    pos -= 4; // Skip block_size(u32).
    pos -= 8; // Skip uncompressed_data_size(u64).
    let index_count = u32::from_le_bytes(data[pos - 4..pos].try_into().unwrap());
    pos -= 4; // Skip index_count(u32).
    pos -= (index_count * 4) as usize; // Skip indexes(u32 * index_count).
    pos -= 8; // Skip bincode_index_count(u64).
    let footer_len = u32::from_le_bytes(data[pos - 4..pos].try_into().unwrap());
    assert_eq!(
        footer_len as usize,
        data.len() - pos,
        "Expected footer_len to match"
    );
    assert_eq!(
        data[pos - 4 - 1],
        FOOTER_FRAME_TYPE,
        "Expected frame_type to be footer"
    );

    let footer = wincode::config::deserialize::<Footer, _>(&data[pos..], WincodeConfig::new())
        .map_err(|e| make_err!(Code::Internal, "Failed to deserialize header : {:?}", e))?;
    Ok(footer)
}

const VALID_HASH: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";
const DUMMY_DATA_SIZE: usize = 100; // Some dummy size to populate DigestInfo with.
const MEGABYTE_SZ: usize = 1024 * 1024;

mod recording_store {
    use nativelink_util::store_trait::StoreDriver;

    use super::*;

    #[derive(MetricsComponent)]
    pub(super) struct RecordingStore {
        pub(super) has_call_count: AtomicUsize,
        pub(super) has_key_count: AtomicUsize,
        pub(super) has_keys: Mutex<Vec<StoreKey<'static>>>,
        pub(super) update_count: AtomicUsize,
    }

    #[async_trait]
    impl StoreDriver for RecordingStore {
        async fn post_init(self: Arc<Self>) -> Result<(), Error> {
            Ok(())
        }

        async fn has_with_results(
            self: Pin<&Self>,
            keys: &[StoreKey<'_>],
            results: &mut [Option<u64>],
        ) -> Result<(), Error> {
            self.has_call_count.fetch_add(1, Ordering::Relaxed);
            self.has_key_count.fetch_add(keys.len(), Ordering::Relaxed);
            *self.has_keys.lock().unwrap() =
                keys.iter().map(|key| key.borrow().into_owned()).collect();
            for (size, result) in (100..).zip(results.iter_mut()) {
                *result = Some(size);
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
}

use recording_store::RecordingStore;

fn make_recording_compression_store() -> Result<(Arc<CompressionStore>, Arc<RecordingStore>), Error>
{
    let inner_store = Arc::new(RecordingStore {
        has_call_count: AtomicUsize::new(0),
        has_key_count: AtomicUsize::new(0),
        has_keys: Mutex::new(Vec::new()),
        update_count: AtomicUsize::new(0),
    });
    let store = CompressionStore::new(
        &CompressionSpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            compression_algorithm: nativelink_config::stores::CompressionAlgorithm::Lz4(
                nativelink_config::stores::Lz4Config::default(),
            ),
        },
        Store::new(inner_store.clone()),
    )?;
    Ok((store, inner_store))
}

#[nativelink_test]
async fn simple_smoke_test() -> Result<(), Error> {
    const RAW_INPUT: &str = "123";

    let store = CompressionStore::new(
        &CompressionSpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            compression_algorithm: nativelink_config::stores::CompressionAlgorithm::Lz4(
                nativelink_config::stores::Lz4Config::default(),
            ),
        },
        Store::new(MemoryStore::new(&MemorySpec::default())),
    )
    .err_tip(|| "Failed to create compression store")?;

    let digest = DigestInfo::try_new(VALID_HASH, DUMMY_DATA_SIZE).unwrap();
    store.update_oneshot(digest, RAW_INPUT.into()).await?;

    let store_data = store
        .get_part_unchunked(digest, 0, None)
        .await
        .err_tip(|| "Failed to get from inner store")?;

    assert_eq!(
        from_utf8(&store_data[..]).unwrap(),
        RAW_INPUT,
        "Expected data to match"
    );
    Ok(())
}

#[nativelink_test]
async fn partial_reads_test() -> Result<(), Error> {
    const RAW_DATA: [u8; 30] = [
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, // BR.
        10, 11, 12, 13, 14, 15, 16, 17, 18, 19, // BR.
        20, 21, 22, 23, 24, 25, 26, 27, 28, 29, // BR.
    ];

    let store_owned = CompressionStore::new(
        &CompressionSpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            compression_algorithm: nativelink_config::stores::CompressionAlgorithm::Lz4(
                nativelink_config::stores::Lz4Config {
                    block_size: 10,
                    ..Default::default()
                },
            ),
        },
        Store::new(MemoryStore::new(&MemorySpec::default())),
    )
    .err_tip(|| "Failed to create compression store")?;
    let store = Pin::new(&store_owned);

    let digest = DigestInfo::try_new(VALID_HASH, DUMMY_DATA_SIZE).unwrap();
    store
        .update_oneshot(digest, RAW_DATA.as_ref().into())
        .await?;

    // Read through the store forcing lots of decompression steps at different offsets
    // and different window sizes. This will ensure we get most edge cases for when
    // we go across block boundaries inclusive, on the fence and exclusive.
    for read_slice_size in 0..(RAW_DATA.len() + 5) {
        for offset in 0..(RAW_DATA.len() + 5) {
            let store_data = store
                .get_part_unchunked(digest, offset as u64, Some(read_slice_size as u64))
                .await
                .err_tip(|| {
                    format!("Failed to get from inner store at {offset} - {read_slice_size}")
                })?;

            let start_pos = cmp::min(RAW_DATA.len(), offset);
            let end_pos = cmp::min(RAW_DATA.len(), offset + read_slice_size);
            assert_eq!(
                &store_data,
                &RAW_DATA[start_pos..end_pos],
                "Expected data to match at {} - {}",
                offset,
                read_slice_size,
            );
        }
    }

    Ok(())
}

#[nativelink_test]
async fn rand_5mb_smoke_test() -> Result<(), Error> {
    let store_owned = CompressionStore::new(
        &CompressionSpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            compression_algorithm: nativelink_config::stores::CompressionAlgorithm::Lz4(
                nativelink_config::stores::Lz4Config::default(),
            ),
        },
        Store::new(MemoryStore::new(&MemorySpec::default())),
    )
    .err_tip(|| "Failed to create compression store")?;
    let store = Pin::new(&store_owned);

    let mut value = vec![0u8; 5 * MEGABYTE_SZ];
    let mut rng = SmallRng::seed_from_u64(1);
    rng.fill(&mut value[..]);

    let digest = DigestInfo::try_new(VALID_HASH, DUMMY_DATA_SIZE).unwrap();
    store.update_oneshot(digest, value.clone().into()).await?;

    let store_data = store
        .get_part_unchunked(digest, 0, None)
        .await
        .err_tip(|| "Failed to get from inner store")?;

    assert_eq!(&store_data, &value, "Expected data to match");
    Ok(())
}

#[nativelink_test]
async fn sanity_check_zero_bytes_test() -> Result<(), Error> {
    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store_owned = CompressionStore::new(
        &CompressionSpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            compression_algorithm: nativelink_config::stores::CompressionAlgorithm::Lz4(
                nativelink_config::stores::Lz4Config::default(),
            ),
        },
        Store::new(inner_store.clone()),
    )
    .err_tip(|| "Failed to create compression store")?;
    let store = Pin::new(&store_owned);

    let digest = DigestInfo::try_new(VALID_HASH, DUMMY_DATA_SIZE).unwrap();
    store.update_oneshot(digest, vec![].into()).await?;

    let store_data = store
        .get_part_unchunked(digest, 0, None)
        .await
        .err_tip(|| "Failed to get from inner store")?;

    assert_eq!(
        store_data.len(),
        0,
        "Expected store data to have no data in it"
    );

    let compressed_data = Pin::new(inner_store.as_ref())
        .get_part_unchunked(digest, 0, None)
        .await
        .err_tip(|| "Failed to get from inner store")?;
    assert_eq!(
        extract_footer(&compressed_data)?,
        Footer {
            indexes: vec![],
            index_count: 0,
            uncompressed_data_size: 0,
            config: Lz4Config {
                block_size: DEFAULT_BLOCK_SIZE,
            },
            version: CURRENT_STREAM_FORMAT_VERSION,
        },
        "Expected footers to match"
    );
    Ok(())
}

#[nativelink_test]
async fn check_header_test() -> Result<(), Error> {
    const BLOCK_SIZE: u32 = 150;
    const MAX_SIZE_INPUT: u64 = 1024 * 1024; // 1MB.
    const RAW_INPUT: &str = "123";

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store_owned = CompressionStore::new(
        &CompressionSpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            compression_algorithm: nativelink_config::stores::CompressionAlgorithm::Lz4(
                nativelink_config::stores::Lz4Config {
                    block_size: BLOCK_SIZE,
                    ..Default::default()
                },
            ),
        },
        Store::new(inner_store.clone()),
    )
    .err_tip(|| "Failed to create compression store")?;
    let store = Pin::new(&store_owned);

    let digest = DigestInfo::try_new(VALID_HASH, DUMMY_DATA_SIZE).unwrap();

    let (mut tx, rx) = make_buf_channel_pair();
    let send_fut = async move {
        tx.send(RAW_INPUT.into()).await?;
        tx.send_eof()
    };
    let (res1, res2) = futures::join!(
        send_fut,
        store.update(digest, rx, UploadSizeInfo::MaxSize(MAX_SIZE_INPUT))
    );
    res1.merge(res2)?;

    let compressed_data = Pin::new(inner_store.as_ref())
        .get_part_unchunked(digest, 0, None)
        .await
        .err_tip(|| "Failed to get from inner store")?;

    let mut reader = Cursor::new(&compressed_data);
    {
        // Check version in header.
        let version = reader.read_u8().await?;
        assert_eq!(
            version, CURRENT_STREAM_FORMAT_VERSION,
            "Expected header version to match current version"
        );
    }
    {
        // Check block size.
        let block_size = reader.read_u32_le().await?;
        assert_eq!(block_size, BLOCK_SIZE, "Expected block size to match");
    }
    {
        // Check upload_type and upload_size.
        const MAX_SIZE_OPT_CODE: u32 = 1;
        let upload_type = reader.read_u32_le().await?;
        assert_eq!(
            upload_type, MAX_SIZE_OPT_CODE,
            "Expected enum size type to match"
        );
        let upload_size = reader.read_u32_le().await?;
        assert_eq!(
            u64::from(upload_size),
            MAX_SIZE_INPUT,
            "Expected upload size to match"
        );
    }

    // As a sanity check lets check our footer.
    assert_eq!(
        extract_footer(&compressed_data)?,
        Footer {
            indexes: vec![],
            index_count: 0,
            uncompressed_data_size: RAW_INPUT.len() as u64,
            config: Lz4Config {
                block_size: BLOCK_SIZE
            },
            version: CURRENT_STREAM_FORMAT_VERSION,
        },
        "Expected footers to match"
    );

    Ok(())
}

#[nativelink_test]
async fn check_footer_test() -> Result<(), Error> {
    const BLOCK_SIZE: u32 = 32 * 1024;
    const EXPECTED_INDEXES: [u32; 7] = [32898, 32898, 32898, 32898, 140, 140, 140];

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store_owned = CompressionStore::new(
        &CompressionSpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            compression_algorithm: nativelink_config::stores::CompressionAlgorithm::Lz4(
                nativelink_config::stores::Lz4Config {
                    block_size: BLOCK_SIZE,
                    ..Default::default()
                },
            ),
        },
        Store::new(inner_store.clone()),
    )
    .err_tip(|| "Failed to create compression store")?;
    let store = Pin::new(&store_owned);

    let mut value = vec![0u8; MEGABYTE_SZ / 4];
    let data_len = value.len();

    let mut rng = SmallRng::seed_from_u64(1);
    // Fill first half of data with random data that is not compressible.
    rng.fill(&mut value[..(data_len / 2)]);

    let digest = DigestInfo::try_new(VALID_HASH, DUMMY_DATA_SIZE).unwrap();
    store.update_oneshot(digest, value.clone().into()).await?;

    let compressed_data = Pin::new(inner_store.as_ref())
        .get_part_unchunked(digest, 0, None)
        .await
        .err_tip(|| "Failed to get from inner store")?;

    let mut pos = compressed_data.len();
    {
        // Check version in footer.
        let version = compressed_data[pos - 1];
        pos -= 1;
        assert_eq!(
            version, CURRENT_STREAM_FORMAT_VERSION,
            "Expected footer version to match current version"
        );
    }
    {
        // Check block size in footer.
        let block_size = u32::from_le_bytes(compressed_data[pos - 4..pos].try_into().unwrap());
        pos -= 4;
        assert_eq!(
            block_size, BLOCK_SIZE,
            "Expected uncompressed_data_size to match original data size"
        );
    }
    {
        // Check data size in footer.
        let uncompressed_data_size =
            u64::from_le_bytes(compressed_data[pos - 8..pos].try_into().unwrap());
        pos -= 8;
        assert_eq!(
            uncompressed_data_size,
            value.len() as u64,
            "Expected uncompressed_data_size to match original data size"
        );
    }
    let index_count = {
        // Check index count in footer.
        let index_count = u32::from_le_bytes(compressed_data[pos - 4..pos].try_into().unwrap());
        pos -= 4;
        assert_eq!(
            index_count as usize,
            EXPECTED_INDEXES.len(),
            "Expected index_count to match"
        );
        index_count
    };
    {
        // Check indexes in footer.
        let byte_count = (index_count * 4) as usize;
        let index_vec_raw = &compressed_data[pos - byte_count..pos];
        pos -= byte_count;
        let mut cursor = Cursor::new(index_vec_raw);
        let mut i = 0;
        while let Ok(index_pos) = cursor.read_u32_le().await {
            assert_eq!(
                index_pos, EXPECTED_INDEXES[i],
                "Expected index to equal at position {}",
                i
            );
            i += 1;
        }
    }
    {
        // `wincode` adds the size again as a u64 before our index vector so check it too.
        let bincode_index_count =
            u64::from_le_bytes(compressed_data[pos - 8..pos].try_into().unwrap());
        pos -= 8;
        assert_eq!(
            bincode_index_count,
            u64::from(index_count),
            "Expected index_count and bincode_index_count to match"
        );
    }
    {
        // Check our footer length.
        let footer_len = u32::from_le_bytes(compressed_data[pos - 4..pos].try_into().unwrap());
        pos -= 4;
        assert_eq!(
            footer_len,
            1 + 4 + 8 + 4 + (index_count * 4) + 8,
            "Expected frame type to be footer"
        );
    }
    {
        // Check our frame type.
        let frame_type = u8::from_le_bytes(compressed_data[pos - 1..pos].try_into().unwrap());
        assert_eq!(
            frame_type, FOOTER_FRAME_TYPE,
            "Expected frame type to be footer"
        );
    }

    // Just as one last sanity check lets check our deserialized footer.
    assert_eq!(
        extract_footer(&compressed_data)?,
        Footer {
            indexes: EXPECTED_INDEXES
                .map(|v| SliceIndex {
                    position_from_prev_index: v
                })
                .to_vec(),
            index_count: u32::try_from(EXPECTED_INDEXES.len()).unwrap_or(u32::MAX),
            uncompressed_data_size: data_len as u64,
            config: Lz4Config {
                block_size: BLOCK_SIZE
            },
            version: CURRENT_STREAM_FORMAT_VERSION,
        },
        "Expected footers to match"
    );

    Ok(())
}

#[nativelink_test]
async fn get_part_is_zero_digest() -> Result<(), Error> {
    const BLOCK_SIZE: u32 = 32 * 1024;

    let digest = ZERO_BYTE_DIGESTS[0];

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store_owned = CompressionStore::new(
        &CompressionSpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            compression_algorithm: nativelink_config::stores::CompressionAlgorithm::Lz4(
                nativelink_config::stores::Lz4Config {
                    block_size: BLOCK_SIZE,
                    ..Default::default()
                },
            ),
        },
        Store::new(inner_store.clone()),
    )
    .err_tip(|| "Failed to create compression store")?;
    let store = Pin::new(Arc::new(store_owned));

    let (mut writer, mut reader) = make_buf_channel_pair();

    let _drop_guard = spawn!("get_part_is_zero_digest", async move {
        drop(
            store
                .as_ref()
                .get_part(digest, &mut writer, 0, None)
                .await
                .err_tip(|| "Failed to get_part"),
        );
    });

    let file_data = reader
        .consume(Some(1024))
        .await
        .err_tip(|| "Error reading bytes")?;

    let empty_bytes = Bytes::new();
    assert_eq!(&file_data, &empty_bytes, "Expected file content to match");

    Ok(())
}

#[nativelink_test]
async fn update_zero_digest_skips_inner_store() -> Result<(), Error> {
    let (store, inner_store) = make_recording_compression_store()?;
    for digest in ZERO_BYTE_DIGESTS {
        store.update_oneshot(digest, Bytes::new()).await?;
    }
    assert_eq!(inner_store.update_count.load(Ordering::Relaxed), 0);
    Ok(())
}

#[nativelink_test]
async fn update_zero_digest_rejects_nonempty_data() -> Result<(), Error> {
    let (store, inner_store) = make_recording_compression_store()?;
    for digest in ZERO_BYTE_DIGESTS {
        let err = store
            .update_oneshot(digest, Bytes::from_static(b"not empty"))
            .await
            .expect_err("Expected nonempty data for a zero digest to fail");
        assert!(
            err.to_string().contains("Zero byte hash not empty"),
            "Unexpected error: {err}"
        );
    }
    assert_eq!(inner_store.update_count.load(Ordering::Relaxed), 0);
    Ok(())
}

#[nativelink_test]
async fn has_zero_digests_skips_inner_store() -> Result<(), Error> {
    let (store, inner_store) = make_recording_compression_store()?;
    let keys = ZERO_BYTE_DIGESTS.map(StoreKey::from);
    let mut results = [None; ZERO_BYTE_DIGESTS.len()];

    store.has_with_results(&keys, &mut results).await?;
    let expected_keys: &[StoreKey<'static>] = &[];
    assert_eq!(
        (
            results,
            inner_store.has_call_count.load(Ordering::Relaxed),
            inner_store.has_key_count.load(Ordering::Relaxed),
            inner_store.has_keys.lock().unwrap().as_slice(),
        ),
        ([Some(0); ZERO_BYTE_DIGESTS.len()], 0, 0, expected_keys,)
    );
    Ok(())
}

#[nativelink_test]
async fn has_mixed_digests_preserves_result_order() -> Result<(), Error> {
    let (store, inner_store) = make_recording_compression_store()?;
    let keys = [
        StoreKey::from(ZERO_BYTE_DIGESTS[0]),
        StoreKey::new_str("first"),
        StoreKey::from(ZERO_BYTE_DIGESTS[1]),
        StoreKey::new_str("second"),
    ];
    let mut results = [None; 4];

    store.has_with_results(&keys, &mut results).await?;
    let expected_keys = [
        StoreKey::new_str("first").into_owned(),
        StoreKey::new_str("second").into_owned(),
    ];
    assert_eq!(
        (
            results,
            inner_store.has_call_count.load(Ordering::Relaxed),
            inner_store.has_key_count.load(Ordering::Relaxed),
            inner_store.has_keys.lock().unwrap().as_slice(),
        ),
        (
            [Some(0), Some(100), Some(0), Some(101)],
            1,
            2,
            expected_keys.as_slice(),
        )
    );
    Ok(())
}

#[nativelink_test]
async fn has_nonzero_digests_delegates_single_batch() -> Result<(), Error> {
    let (store, inner_store) = make_recording_compression_store()?;
    let keys = [StoreKey::new_str("first"), StoreKey::new_str("second")];
    let mut results = [None; 2];

    store.has_with_results(&keys, &mut results).await?;
    let expected_keys = [
        StoreKey::new_str("first").into_owned(),
        StoreKey::new_str("second").into_owned(),
    ];
    assert_eq!(
        (
            results,
            inner_store.has_call_count.load(Ordering::Relaxed),
            inner_store.has_key_count.load(Ordering::Relaxed),
            inner_store.has_keys.lock().unwrap().as_slice(),
        ),
        ([Some(100), Some(101)], 1, 2, expected_keys.as_slice())
    );
    Ok(())
}

// Regression test for the bug where start_pos > end_pos in the slice operation
#[nativelink_test]
async fn regression_test_range_start_not_greater_than_end() -> Result<(), Error> {
    // Create a store with a small block size to trigger multiple blocks
    const BLOCK_SIZE: u32 = 64 * 1024; // 64KB, same as DEFAULT_BLOCK_SIZE

    let inner_store = MemoryStore::new(&MemorySpec::default());
    let store_owned = CompressionStore::new(
        &CompressionSpec {
            backend: StoreSpec::Memory(MemorySpec::default()),
            compression_algorithm: nativelink_config::stores::CompressionAlgorithm::Lz4(
                nativelink_config::stores::Lz4Config {
                    block_size: BLOCK_SIZE,
                    ..Default::default()
                },
            ),
        },
        Store::new(inner_store.clone()),
    )
    .err_tip(|| "Failed to create compression store")?;
    let store = Pin::new(&store_owned);

    // Create a large buffer that spans multiple blocks
    let data_size = BLOCK_SIZE as usize * 3; // 3 blocks
    let mut data = vec![0u8; data_size];
    let mut rng = SmallRng::seed_from_u64(42);
    rng.fill(&mut data[..]);

    let digest = DigestInfo::try_new(VALID_HASH, data_size).unwrap();
    store.update_oneshot(digest, data.clone().into()).await?;

    // Try to read exactly at block boundaries with various offsets
    let boundary = u64::from(BLOCK_SIZE);

    // These specific offsets test the case in the bug report where
    // start_pos was 65536 and end_pos was 65535
    for (offset, length) in &[
        (boundary - 1, Some(2u64)),  // Read across block boundary
        (boundary, Some(1u64)),      // Read exactly at block boundary
        (boundary + 1, Some(10u64)), // Read just after block boundary
        // Specifically test the case where offset >= block size
        (u64::from(BLOCK_SIZE), Some(20u64)),
        // Specifically test the case that caused the bug (65536 and 65535)
        (u64::from(BLOCK_SIZE), Some(u64::from(BLOCK_SIZE) - 1)),
        // More edge cases around the block boundary to thoroughly test the issue
        (u64::from(BLOCK_SIZE) - 1, Some(1u64)), // Just before boundary
        (u64::from(BLOCK_SIZE), Some(0u64)),     // Zero length at boundary
        (u64::from(BLOCK_SIZE), Some(u64::MAX)), // Unlimited length at boundary
        (u64::from(BLOCK_SIZE) * 2, Some(u64::from(BLOCK_SIZE) - 1)), // Same issue at next block
    ] {
        // First test with get_part_unchunked
        let result = store.get_part_unchunked(digest, *offset, *length).await;

        // The bug was causing a panic, so just checking that it doesn't panic
        // means the fix is working
        assert!(
            result.is_ok(),
            "Reading with get_part_unchunked at offset {offset} with length {length:?} should not fail"
        );

        let store_data = result.unwrap();

        // Verify the data matches what we expect
        let expected_len = cmp::min(
            usize::try_from(length.unwrap_or(u64::MAX))?,
            data.len().saturating_sub(usize::try_from(*offset)?),
        );
        assert_eq!(
            store_data.len(),
            expected_len,
            "Expected data length to match when reading at offset {} with length {:?}",
            offset,
            length
        );

        if expected_len > 0 {
            let start = usize::try_from(*offset)?;
            let end = start + expected_len;
            assert_eq!(
                &store_data[..],
                &data[start..end],
                "Expected data content to match when reading at offset {} with length {:?}",
                offset,
                length
            );
        }

        // Now also test with the lower-level get_part method to ensure it doesn't panic
        // This is closer to what the bytestream server would call
        let (mut tx, mut rx) = make_buf_channel_pair();

        // The error was happening in this method call
        let get_part_result = store.get_part(digest, &mut tx, *offset, *length).await;
        assert!(
            get_part_result.is_ok(),
            "Reading with get_part at offset {offset} with length {length:?} should not fail"
        );

        // Just to consume the stream and ensure it behaves as expected
        let mut received_data = Vec::new();
        while let Ok(chunk) = rx.consume(Some(1024)).await {
            if chunk.is_empty() {
                break;
            }
            received_data.extend_from_slice(&chunk);
        }

        assert_eq!(
            received_data.len(),
            expected_len,
            "Expected get_part received data length to match when reading at offset {} with length {:?}",
            offset,
            length
        );

        if expected_len > 0 {
            let start = usize::try_from(*offset)?;
            let end = start + expected_len;
            assert_eq!(
                &received_data[..],
                &data[start..end],
                "Expected get_part data content to match when reading at offset {} with length {:?}",
                offset,
                length
            );
        }
    }

    Ok(())
}
