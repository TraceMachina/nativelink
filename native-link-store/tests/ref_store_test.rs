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
use std::sync::Arc;

#[cfg(test)]
mod ref_store_tests {
    use error::Error;
    use native_link_store::memory_store::MemoryStore;
    use native_link_store::ref_store::RefStore;
    use native_link_store::store_manager::StoreManager;
    use native_link_util::common::DigestInfo;
    use native_link_util::store_trait::Store;
    use pretty_assertions::assert_eq; // Must be declared in every module.

    use super::*;

    const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

    fn setup_stores() -> (Arc<StoreManager>, Arc<MemoryStore>, Arc<RefStore>) {
        let store_manager = Arc::new(StoreManager::new());

        let memory_store_owned = Arc::new(MemoryStore::new(&native_link_config::stores::MemoryStore::default()));
        store_manager.add_store("foo", memory_store_owned.clone());

        let ref_store_owned = Arc::new(RefStore::new(
            &native_link_config::stores::RefStore {
                name: "foo".to_string(),
            },
            Arc::downgrade(&store_manager),
        ));
        store_manager.add_store("bar", ref_store_owned.clone());
        (store_manager, memory_store_owned, ref_store_owned)
    }

    #[tokio::test]
    async fn has_test() -> Result<(), Error> {
        let (_store_manager, memory_store_owned, ref_store_owned) = setup_stores();

        const VALUE1: &str = "13";
        {
            // Insert data into memory store.
            Pin::new(memory_store_owned.as_ref())
                .update_oneshot(DigestInfo::try_new(VALID_HASH1, VALUE1.len())?, VALUE1.into())
                .await?;
        }
        {
            // Now check if we check of ref_store has the data.
            let has_result = Pin::new(ref_store_owned.as_ref())
                .has(DigestInfo::try_new(VALID_HASH1, VALUE1.len())?)
                .await;
            assert_eq!(
                has_result,
                Ok(Some(VALUE1.len())),
                "Expected ref store to have data in ref store : {}",
                VALID_HASH1
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn get_test() -> Result<(), Error> {
        let (_store_manager, memory_store_owned, ref_store_owned) = setup_stores();

        const VALUE1: &str = "13";
        {
            // Insert data into memory store.
            Pin::new(memory_store_owned.as_ref())
                .update_oneshot(DigestInfo::try_new(VALID_HASH1, VALUE1.len())?, VALUE1.into())
                .await?;
        }
        {
            // Now check if we read it from ref_store it has same data.
            let data = Pin::new(ref_store_owned.as_ref())
                .get_part_unchunked(DigestInfo::try_new(VALID_HASH1, VALUE1.len())?, 0, None, None)
                .await
                .expect("Get should have succeeded");
            assert_eq!(
                data,
                VALUE1.as_bytes(),
                "Expected ref store to have data in ref store : {}",
                VALID_HASH1
            );
        }
        Ok(())
    }

    #[tokio::test]
    async fn update_test() -> Result<(), Error> {
        let (_store_manager, memory_store_owned, ref_store_owned) = setup_stores();

        const VALUE1: &str = "13";
        {
            // Insert data into ref_store.
            Pin::new(ref_store_owned.as_ref())
                .update_oneshot(DigestInfo::try_new(VALID_HASH1, VALUE1.len())?, VALUE1.into())
                .await?;
        }
        {
            // Now check if we read it from memory_store it has same data.
            let data = Pin::new(memory_store_owned.as_ref())
                .get_part_unchunked(DigestInfo::try_new(VALID_HASH1, VALUE1.len())?, 0, None, None)
                .await
                .expect("Get should have succeeded");
            assert_eq!(
                data,
                VALUE1.as_bytes(),
                "Expected ref store to have data in memory store : {}",
                VALID_HASH1
            );
        }
        Ok(())
    }
}
