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

use hashbrown::HashSet;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use common::DigestInfo;
use error::Error;
use traits::{StoreTrait, UploadSizeInfo};

pub struct ExistenceStore {
    inner_store: Arc<dyn StoreTrait>,
    pub existence_cache: Mutex<HashSet<DigestInfo>>,
}

impl ExistenceStore {
    pub fn new(inner_store: Arc<dyn StoreTrait>) -> Self {
        Self {
            inner_store,
            // TODO (BlakeHatch):
            // Consider using RwLock in a future commit.
            // Since HashSet implements Send and Sync this should
            // be a drop-in replacement in theory.
            // Make sure benchmark is done to justify.
            existence_cache: Mutex::new(HashSet::new()),
        }
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
        let mut pruned_digests = Vec::new();

        for (i, digest) in digests.iter().enumerate() {
            if self.existence_cache.lock().unwrap().contains(digest) {
                results[i] = Some(1);
            } else {
                pruned_digests.push(*digest);
            }
        }

        if !pruned_digests.is_empty() {
            let mut inner_results = vec![None; pruned_digests.len()];
            self.pin_inner()
                .has_with_results(&pruned_digests, &mut inner_results)
                .await?;

            for (i, result) in inner_results.iter().enumerate() {
                if result.is_some() {
                    self.existence_cache.lock().unwrap().insert(pruned_digests[i]);
                    results[i] = Some(1);
                }
            }
        }

        Ok(())
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
