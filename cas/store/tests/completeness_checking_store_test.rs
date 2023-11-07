// Copyright 2023 The Turbo Cache Authors. All rights reserved.
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
mod completeness_checking_store_tests {
    use super::*;

    use common::DigestInfo;
    use completeness_checking_store::CompletenessCheckingStore;
    use error::Error;
    use memory_store::MemoryStore;
    use traits::StoreTrait;

    const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";

    #[tokio::test]
    async fn verify_has_with_results_and_find_in_cas() -> Result<(), Error> {
        let inner_store = Arc::new(MemoryStore::new(&config::stores::MemoryStore::default()));
        let store_owned = CompletenessCheckingStore::new(inner_store.clone());
        let pinned_store = Pin::new(&store_owned);

        const VALUE1: &str = "123";
        let digest = DigestInfo::try_new(VALID_HASH1, 100).unwrap();
        let result = pinned_store.update_oneshot(digest, VALUE1.into()).await;
        assert_eq!(result, Ok(()), "Expected success, got: {:?}", result);

        let digest = DigestInfo::try_new(VALID_HASH1, 0).unwrap();
        let _res = pinned_store.find_in_cas(digest).await;
        assert!(_res.is_ok(), "Expected find_in_cas to succeed");

        let mut results = vec![Some(0); 1];
        let _res = pinned_store.has_with_results(&[digest], &mut results).await;
        assert!(_res.is_ok(), "Expected has_with_results to succeed");
        assert_eq!(results[0], None, "Expected digest to be pruned from results");

        Ok(())
    }
}
