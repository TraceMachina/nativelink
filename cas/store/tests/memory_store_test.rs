// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

#[cfg(test)]
mod memory_store_tests {
    use pretty_assertions::assert_eq; // Must be declared in every module.

    use error::Error;
    use std::io::Cursor;

    use common::DigestInfo;
    use memory_store::MemoryStore;
    use traits::StoreTrait;

    const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

    #[tokio::test]
    async fn insert_one_item_then_update() -> Result<(), Error> {
        let store = MemoryStore::new();

        {
            // Insert dummy value into store.
            const VALUE1: &str = "1";
            store
                .update(
                    &DigestInfo::try_new(&VALID_HASH1, VALUE1.len())?,
                    Box::new(Cursor::new(VALUE1)),
                )
                .await?;
            assert!(
                store
                    .has(&DigestInfo::try_new(&VALID_HASH1, VALID_HASH1.len())?)
                    .await?,
                "Expected memory store to have hash: {}",
                VALID_HASH1
            );
        }

        const VALUE2: &str = "23";
        let mut store_data = Vec::new();
        {
            // Now change value we just inserted.
            store
                .update(
                    &DigestInfo::try_new(&VALID_HASH1, VALUE2.len())?,
                    Box::new(Cursor::new(VALUE2)),
                )
                .await?;
            store
                .get(
                    &DigestInfo::try_new(&VALID_HASH1, VALID_HASH1.len())?,
                    &mut Cursor::new(&mut store_data),
                )
                .await?;
        }

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

    const TOO_LONG_HASH: &str =
        "0123456789abcdef000000000000000000010000000000000123456789abcdefff";
    const TOO_SHORT_HASH: &str = "100000000000000000000000000000000000000000000000000000000000001";
    const INVALID_HASH: &str = "g111111111111111111111111111111111111111111111111111111111111111";

    #[tokio::test]
    async fn errors_with_invalid_inputs() -> Result<(), Error> {
        let mut store = MemoryStore::new();
        const VALUE1: &str = "123";
        {
            // .has() tests.
            async fn has_should_fail(store: &MemoryStore, hash: &str, expected_size: usize) {
                let digest = DigestInfo::try_new(&hash, expected_size);
                assert!(
                    digest.is_err() || store.has(&digest.unwrap()).await.is_err(),
                    ".has() should have failed: {} {}",
                    hash,
                    expected_size
                );
            };
            has_should_fail(&store, &TOO_LONG_HASH, VALUE1.len()).await;
            has_should_fail(&store, &TOO_SHORT_HASH, VALUE1.len()).await;
            has_should_fail(&store, &INVALID_HASH, VALUE1.len()).await;
        }
        {
            // .update() tests.
            async fn update_should_fail<'a, 'b>(
                store: &'a mut MemoryStore,
                hash: &'a str,
                expected_size: usize,
                value: &'b str,
            ) {
                let digest = DigestInfo::try_new(&hash, expected_size);
                assert!(
                    digest.is_err()
                        || store
                            .update(&digest.unwrap(), Box::new(Cursor::new(&value)))
                            .await
                            .is_err(),
                    ".has() should have failed: {} {} {}",
                    hash,
                    expected_size,
                    value
                );
            };
            update_should_fail(&mut store, &TOO_LONG_HASH, VALUE1.len(), &VALUE1).await;
            update_should_fail(&mut store, &TOO_SHORT_HASH, VALUE1.len(), &VALUE1).await;
            update_should_fail(&mut store, &INVALID_HASH, VALUE1.len(), &VALUE1).await;
            update_should_fail(&mut store, &VALID_HASH1, VALUE1.len() + 1, &VALUE1).await;
            update_should_fail(&mut store, &VALID_HASH1, VALUE1.len() - 1, &VALUE1).await;
        }
        {
            // .update() tests.
            async fn get_should_fail<'a>(
                store: &'a mut MemoryStore,
                hash: &'a str,
                expected_size: usize,
                out_data: &'a mut Vec<u8>,
            ) {
                let digest = DigestInfo::try_new(&hash, expected_size);
                assert!(
                    digest.is_err()
                        || store
                            .get(&digest.unwrap(), &mut Cursor::new(out_data))
                            .await
                            .is_err(),
                    ".get() should have failed: {} {}",
                    hash,
                    expected_size
                );
            };
            let mut out_data: Vec<u8> = Vec::new();

            get_should_fail(&mut store, &TOO_LONG_HASH, 1, &mut out_data).await;
            get_should_fail(&mut store, &TOO_SHORT_HASH, 1, &mut out_data).await;
            get_should_fail(&mut store, &INVALID_HASH, 1, &mut out_data).await;
            // With an empty store .get() should fail too.
            get_should_fail(&mut store, &VALID_HASH1, 1, &mut out_data).await;
        }
        Ok(())
    }
}
