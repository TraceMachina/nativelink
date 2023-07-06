// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

#[cfg(test)]
mod memory_store_tests {
    use super::*;
    use pretty_assertions::assert_eq; // Must be declared in every module.

    use error::Error;

    use common::DigestInfo;
    use config;
    use memory_store::MemoryStore;
    use traits::StoreTrait;

    const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

    #[tokio::test]
    async fn insert_one_item_then_update() -> Result<(), Error> {
        let store_owned = MemoryStore::new(&config::stores::MemoryStore::default());
        let store = Pin::new(&store_owned);

        {
            // Insert dummy value into store.
            const VALUE1: &str = "13";
            store
                .update_oneshot(DigestInfo::try_new(&VALID_HASH1, VALUE1.len())?, VALUE1.into())
                .await?;
            assert_eq!(
                store.has(DigestInfo::try_new(&VALID_HASH1, VALUE1.len())?).await,
                Ok(Some(VALUE1.len())),
                "Expected memory store to have hash: {}",
                VALID_HASH1
            );
        }

        const VALUE2: &str = "23";
        let store_data = {
            // Now change value we just inserted.
            store
                .update_oneshot(DigestInfo::try_new(&VALID_HASH1, VALUE2.len())?, VALUE2.into())
                .await?;
            store
                .get_part_unchunked(DigestInfo::try_new(&VALID_HASH1, VALUE2.len())?, 0, None, None)
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

    const TOO_LONG_HASH: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdefff";
    const TOO_SHORT_HASH: &str = "100000000000000000000000000000000000000000000000000000000000001";
    const INVALID_HASH: &str = "g111111111111111111111111111111111111111111111111111111111111111";

    #[tokio::test]
    async fn read_partial() -> Result<(), Error> {
        let store_owned = MemoryStore::new(&config::stores::MemoryStore::default());
        let store = Pin::new(&store_owned);

        const VALUE1: &str = "1234";
        let digest = DigestInfo::try_new(&VALID_HASH1, 4).unwrap();
        store.update_oneshot(digest.clone(), VALUE1.into()).await?;

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
        let store_owned = MemoryStore::new(&config::stores::MemoryStore::default());
        let store = Pin::new(&store_owned);

        // Insert dummy value into store.
        const VALUE: &str = "";
        store
            .update_oneshot(DigestInfo::try_new(&VALID_HASH1, VALUE.len())?, VALUE.into())
            .await?;
        assert_eq!(
            store
                .get_part_unchunked(DigestInfo::try_new(&VALID_HASH1, VALUE.len())?, 0, None, None)
                .await,
            Ok("".into()),
            "Expected memory store to have empty value"
        );
        Ok(())
    }

    #[tokio::test]
    async fn errors_with_invalid_inputs() -> Result<(), Error> {
        let store_owned = MemoryStore::new(&config::stores::MemoryStore::default());
        let store = Pin::new(&store_owned);
        const VALUE1: &str = "123";
        {
            // .has() tests.
            async fn has_should_fail(store: Pin<&MemoryStore>, hash: &str, expected_size: usize) {
                let digest = DigestInfo::try_new(&hash, expected_size);
                assert!(
                    digest.is_err() || store.has(digest.unwrap()).await.is_err(),
                    ".has() should have failed: {} {}",
                    hash,
                    expected_size
                );
            }
            has_should_fail(store, &TOO_LONG_HASH, VALUE1.len()).await;
            has_should_fail(store, &TOO_SHORT_HASH, VALUE1.len()).await;
            has_should_fail(store, &INVALID_HASH, VALUE1.len()).await;
        }
        {
            // .update() tests.
            async fn update_should_fail<'a>(
                store: Pin<&'a MemoryStore>,
                hash: &'a str,
                expected_size: usize,
                value: &'static str,
            ) {
                let digest = DigestInfo::try_new(&hash, expected_size);
                assert!(
                    digest.is_err() || store.update_oneshot(digest.unwrap(), value.into(),).await.is_err(),
                    ".has() should have failed: {} {} {}",
                    hash,
                    expected_size,
                    value
                );
            }
            update_should_fail(store, &TOO_LONG_HASH, VALUE1.len(), &VALUE1).await;
            update_should_fail(store, &TOO_SHORT_HASH, VALUE1.len(), &VALUE1).await;
            update_should_fail(store, &INVALID_HASH, VALUE1.len(), &VALUE1).await;
        }
        {
            // .update() tests.
            async fn get_should_fail<'a>(store: Pin<&'a MemoryStore>, hash: &'a str, expected_size: usize) {
                let digest = DigestInfo::try_new(&hash, expected_size);
                assert!(
                    digest.is_err() || store.get_part_unchunked(digest.unwrap(), 0, None, None).await.is_err(),
                    ".get() should have failed: {} {}",
                    hash,
                    expected_size
                );
            }

            get_should_fail(store, &TOO_LONG_HASH, 1).await;
            get_should_fail(store, &TOO_SHORT_HASH, 1).await;
            get_should_fail(store, &INVALID_HASH, 1).await;
            // With an empty store .get() should fail too.
            get_should_fail(store, &VALID_HASH1, 1).await;
        }
        Ok(())
    }
}
