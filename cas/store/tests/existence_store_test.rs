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
mod verify_store_tests {
    use super::*;

    use common::DigestInfo;
    use error::Error;
    use existence_store::ExistenceStore;
    use memory_store::MemoryStore;

    use traits::StoreTrait;

    const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

    async fn has_in_cache(store: Pin<&ExistenceStore>, digest: &DigestInfo) -> bool {
        match store.existence_cache.lock() {
            Ok(cache) => cache.contains(digest),
            Err(_) => false,
        }
    }

    #[tokio::test]
    async fn verify_existence_caching() -> Result<(), Error> {
        const VALUE: &str = "123";
        let inner_store = Arc::new(MemoryStore::new(&config::stores::MemoryStore::default()));
        let store_owned = ExistenceStore::new(inner_store.clone());
        let store = Pin::new(&store_owned);

        let digest = DigestInfo::try_new(VALID_HASH1, 3).unwrap();
        let _result = store.update_oneshot(digest, VALUE.into()).await;

        assert!(
            !has_in_cache(store, &digest).await,
            "Expected digest to not exist in cache before has call"
        );

        let _result = store.has(digest).await;

        assert!(
            has_in_cache(store, &digest).await,
            "Expected digest to exist in cache after has call"
        );
        Ok(())
    }
}
