// Copyright 2023 The Native Link Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use bytes::{BufMut, BytesMut};
use memory_stats::memory_stats;
use nativelink_error::{Error, ResultExt};
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::buf_channel::{make_buf_channel_pair, DropCloserReadHalf};
use nativelink_util::common::{DigestInfo, JoinHandleDropGuard};
use nativelink_util::store_trait::Store;
use sha2::{Digest, Sha256};

const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";
const VALID_HASH2: &str = "0123456789abcdef000000000000000000020000000000000123456789abcdef";
const VALID_HASH3: &str = "0123456789abcdef000000000000000000030000000000000123456789abcdef";
const VALID_HASH4: &str = "0123456789abcdef000000000000000000040000000000000123456789abcdef";
const TOO_LONG_HASH: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdefff";
const TOO_SHORT_HASH: &str = "100000000000000000000000000000000000000000000000000000000000001";
const INVALID_HASH: &str = "g111111111111111111111111111111111111111111111111111111111111111";

#[cfg(test)]
mod memory_store_tests {

    use pretty_assertions::assert_eq;

    use super::*; // Must be declared in every module.

    #[tokio::test]
    async fn insert_one_item_then_update() -> Result<(), Error> {
        const VALUE1: &str = "13";
        const VALUE2: &str = "23";
        let store_owned = MemoryStore::new(&nativelink_config::stores::MemoryStore::default());
        let store = Pin::new(&store_owned);

        // Insert dummy value into store.
        store
            .update_oneshot(DigestInfo::try_new(VALID_HASH1, VALUE1.len())?, VALUE1.into())
            .await?;
        assert_eq!(
            store.has(DigestInfo::try_new(VALID_HASH1, VALUE1.len())?).await,
            Ok(Some(VALUE1.len())),
            "Expected memory store to have hash: {}",
            VALID_HASH1
        );

        let store_data = {
            // Now change value we just inserted.
            store
                .update_oneshot(DigestInfo::try_new(VALID_HASH1, VALUE2.len())?, VALUE2.into())
                .await?;
            store
                .get_part_unchunked(DigestInfo::try_new(VALID_HASH1, VALUE2.len())?, 0, None, None)
                .await?
        };

        assert_eq!(
            store_data,
            VALUE2.as_bytes(),
            "Hash for key: {} did not update. Expected: {:#x?}, but got: {:#x?}",
            VALID_HASH1,
            VALUE2,
            store_data
        );
        Ok(())
    }

    // Regression test for: https://github.com/TraceMachina/turbo-cache/issues/289.
    #[tokio::test]
    async fn ensure_full_copy_of_bytes_is_made_test() -> Result<(), Error> {
        // Arbitrary value, this may be increased if we find out that this is
        // too low for some kernels/operating systems.
        const MAXIMUM_MEMORY_USAGE_INCREASE_PERC: f64 = 1.3; // 30% increase.

        let store_owned = MemoryStore::new(&nativelink_config::stores::MemoryStore::default());
        let store = Pin::new(&store_owned);

        let initial_virtual_mem = memory_stats().err_tip(|| "Failed to read memory.")?.physical_mem;
        for (i, hash) in [VALID_HASH1, VALID_HASH2, VALID_HASH3, VALID_HASH4]
            .into_iter()
            .enumerate()
        {
            // User a variety of sizes increasing up to 10MB each iteration.
            // We do this to reduce the chance of memory page size masking the potential bug.
            let reserved_size = 10_usize.pow(u32::try_from(i).expect("Cast failed") + 4);
            let mut mut_data = BytesMut::with_capacity(reserved_size);
            mut_data.put_bytes(u8::try_from(i).expect("Cast failed"), 1);
            let data = mut_data.freeze();

            let digest = DigestInfo::try_new(hash, data.len())?;
            store
                .update_oneshot(digest, data)
                .await
                .err_tip(|| "Could not update store")?;
        }

        let new_virtual_mem = memory_stats().err_tip(|| "Failed to read memory.")?.physical_mem;
        let memory_usage_increase_perc = new_virtual_mem as f64 / initial_virtual_mem as f64;
        assert!(
            memory_usage_increase_perc < MAXIMUM_MEMORY_USAGE_INCREASE_PERC,
            "Memory usage increased by {memory_usage_increase_perc} perc, which is more than {MAXIMUM_MEMORY_USAGE_INCREASE_PERC} perc",
        );
        Ok(())
    }

    #[tokio::test]
    async fn read_partial() -> Result<(), Error> {
        const VALUE1: &str = "1234";
        let store_owned = MemoryStore::new(&nativelink_config::stores::MemoryStore::default());
        let store = Pin::new(&store_owned);

        let digest = DigestInfo::try_new(VALID_HASH1, 4).unwrap();
        store.update_oneshot(digest, VALUE1.into()).await?;

        let store_data = store.get_part_unchunked(digest, 1, Some(2), None).await?;

        assert_eq!(
            VALUE1[1..3].as_bytes(),
            store_data,
            "Expected partial data to match, expected '{:#x?}' got: {:#x?}'",
            VALUE1[1..3].as_bytes(),
            store_data,
        );
        Ok(())
    }

    // A bug was found where reading an empty value from memory store would result in an error
    // due to internal EOF handling. This is an edge case test.
    #[tokio::test]
    async fn read_zero_size_item_test() -> Result<(), Error> {
        const VALUE: &str = "";
        let store_owned = MemoryStore::new(&nativelink_config::stores::MemoryStore::default());
        let store = Pin::new(&store_owned);

        // Insert dummy value into store.
        store
            .update_oneshot(DigestInfo::try_new(VALID_HASH1, VALUE.len())?, VALUE.into())
            .await?;
        assert_eq!(
            store
                .get_part_unchunked(DigestInfo::try_new(VALID_HASH1, VALUE.len())?, 0, None, None)
                .await,
            Ok("".into()),
            "Expected memory store to have empty value"
        );
        Ok(())
    }

    #[tokio::test]
    async fn errors_with_invalid_inputs() -> Result<(), Error> {
        const VALUE1: &str = "123";
        let store_owned = MemoryStore::new(&nativelink_config::stores::MemoryStore::default());
        let store = Pin::new(&store_owned);
        {
            // .has() tests.
            async fn has_should_fail(store: Pin<&MemoryStore>, hash: &str, expected_size: usize) {
                let digest = DigestInfo::try_new(hash, expected_size);
                assert!(
                    digest.is_err() || store.has(digest.unwrap()).await.is_err(),
                    ".has() should have failed: {hash} {expected_size}",
                );
            }
            has_should_fail(store, TOO_LONG_HASH, VALUE1.len()).await;
            has_should_fail(store, TOO_SHORT_HASH, VALUE1.len()).await;
            has_should_fail(store, INVALID_HASH, VALUE1.len()).await;
        }
        {
            // .update() tests.
            async fn update_should_fail<'a>(
                store: Pin<&'a MemoryStore>,
                hash: &'a str,
                expected_size: usize,
                value: &'static str,
            ) {
                let digest = DigestInfo::try_new(hash, expected_size);
                assert!(
                    digest.is_err() || store.update_oneshot(digest.unwrap(), value.into(),).await.is_err(),
                    ".has() should have failed: {hash} {expected_size} {value}",
                );
            }
            update_should_fail(store, TOO_LONG_HASH, VALUE1.len(), VALUE1).await;
            update_should_fail(store, TOO_SHORT_HASH, VALUE1.len(), VALUE1).await;
            update_should_fail(store, INVALID_HASH, VALUE1.len(), VALUE1).await;
        }
        {
            // .update() tests.
            async fn get_should_fail<'a>(store: Pin<&'a MemoryStore>, hash: &'a str, expected_size: usize) {
                let digest = DigestInfo::try_new(hash, expected_size);
                assert!(
                    digest.is_err() || store.get_part_unchunked(digest.unwrap(), 0, None, None).await.is_err(),
                    ".get() should have failed: {hash} {expected_size}",
                );
            }

            get_should_fail(store, TOO_LONG_HASH, 1).await;
            get_should_fail(store, TOO_SHORT_HASH, 1).await;
            get_should_fail(store, INVALID_HASH, 1).await;
            // With an empty store .get() should fail too.
            get_should_fail(store, VALID_HASH1, 1).await;
        }
        Ok(())
    }

    #[tokio::test]
    async fn get_part_is_zero_digest() -> Result<(), Error> {
        let digest = DigestInfo {
            packed_hash: Sha256::new().finalize().into(),
            size_bytes: 0,
        };

        let store = Arc::new(MemoryStore::new(&nativelink_config::stores::MemoryStore::default()));
        let store_clone = store.clone();
        let (mut writer, mut reader) = make_buf_channel_pair();

        let _drop_guard = JoinHandleDropGuard::new(tokio::spawn(async move {
            let _ = Pin::new(store_clone.as_ref())
                .get_part_ref(digest, &mut writer, 0, None)
                .await
                .err_tip(|| "Failed to get_part_ref");
        }));

        let file_data = DropCloserReadHalf::take(&mut reader, 1024)
            .await
            .err_tip(|| "Error reading bytes")?;

        let empty_bytes = Bytes::new();
        assert_eq!(&file_data, &empty_bytes, "Expected file content to match");

        Ok(())
    }

    #[tokio::test]
    async fn has_with_results_on_zero_digests() -> Result<(), Error> {
        let digest = DigestInfo {
            packed_hash: Sha256::new().finalize().into(),
            size_bytes: 0,
        };
        let mut digests = vec![digest];
        let mut results = vec![None];

        let store_owned = MemoryStore::new(&nativelink_config::stores::MemoryStore::default());
        let store = Pin::new(&store_owned);

        let _ = store
            .as_ref()
            .has_with_results(&mut digests, &mut results)
            .await
            .err_tip(|| "Failed to get_part_ref");
        assert_eq!(results, vec!(Some(0)));

        Ok(())
    }
}
