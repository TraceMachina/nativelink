// Copyright 2020 Nathan (Blaise) Bruer.  All rights reserved.

#[cfg(test)]
mod memory_store_tests {
    use std::io::Cursor;

    use memory_store::MemoryStore;
    use traits::StoreTrait;

    const HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

    #[tokio::test]
    async fn insert_one_item_then_update() -> Result<(), Box<dyn std::error::Error>> {
        let mut store = MemoryStore::new();

        {
            // Insert dummy value into store.
            const VALUE1: &str = "1";
            store
                .update(&HASH1, VALUE1.len(), Box::new(Cursor::new(VALUE1)))
                .await?;
            assert!(
                store.has(&HASH1, HASH1.len()).await?,
                "Expected memory store to have hash: {}",
                HASH1
            );
        }

        const VALUE2: &str = "23";
        let mut store_data = Vec::new();
        {
            // Now change value we just inserted.
            store
                .update(&HASH1, VALUE2.len(), Box::new(Cursor::new(VALUE2)))
                .await?;
            store
                .get(&HASH1, HASH1.len(), &mut Cursor::new(&mut store_data))
                .await?;
        }

        assert_eq!(
            store_data,
            VALUE2.as_bytes(),
            "Hash for key: {} did not update. Expected: {:#x?}, but got: {:#x?}",
            HASH1,
            VALUE2,
            store_data
        );
        Ok(())
    }
}
