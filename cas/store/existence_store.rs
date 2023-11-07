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

use hashbrown::HashSet;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;

use async_trait::async_trait;

use buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use common::DigestInfo;
use error::Error;
use traits::{StoreTrait, UploadSizeInfo};

pub struct ExistenceStore {
    inner_store: Arc<dyn StoreTrait>,
    existence_cache: Arc<Mutex<HashSet<DigestInfo>>>,
}

impl ExistenceStore {
    pub fn new(inner_store: Arc<dyn StoreTrait>) -> Self {
        ExistenceStore {
            inner_store,
            existence_cache: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub async fn digest_in_existence_cache(&self, digest: &DigestInfo) -> bool {
        let cache = self.existence_cache.lock().await;
        cache.contains(digest)
    }

    async fn cache_existence(&self, digest: &DigestInfo) -> bool {
        let mut cache = self.existence_cache.lock().await;
        cache.insert(*digest)
    }

    fn pin_inner(&self) -> Pin<&dyn StoreTrait> {
        Pin::new(self.inner_store.as_ref())
    }
}

#[async_trait]
impl StoreTrait for ExistenceStore {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        let mut not_in_cache = Vec::new();
        for digest in digests {
            if !self.digest_in_existence_cache(digest).await {
                not_in_cache.push(digest);
                self.cache_existence(digest).await;
            }
        }

        let not_in_cache: Vec<DigestInfo> = not_in_cache.into_iter().cloned().collect();
        self.pin_inner().has_with_results(&not_in_cache, results).await
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        self.pin_inner().update(digest, reader, size_info).await
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        self.pin_inner().get_part_ref(digest, writer, offset, length).await
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
