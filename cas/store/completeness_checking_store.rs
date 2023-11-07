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
use std::sync::{Arc, Mutex};

use ac_utils::get_and_decode_digest;
use async_trait::async_trait;
use proto::build::bazel::remote::execution::v2::Directory;

use buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use common::DigestInfo;
use error::{Code, Error, ResultExt};
use traits::{StoreTrait, UploadSizeInfo};

pub struct CompletenessCheckingStore {
    inner_store: Arc<dyn StoreTrait>,
    completeness_cache: Arc<Mutex<HashSet<DigestInfo>>>,
}

impl CompletenessCheckingStore {
    pub fn new(inner_store: Arc<dyn StoreTrait>) -> Self {
        CompletenessCheckingStore {
            inner_store,
            completeness_cache: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub async fn find_in_cas(self: Pin<&Self>, digest: DigestInfo) -> Result<(), Error> {
        let inner_store = self.pin_inner();
        let mut stack = vec![digest];
        while let Some(digest) = stack.pop() {
            let directory = get_and_decode_digest::<Directory>(inner_store, &digest).await?;

            for file in &directory.files {
                let file_digest = DigestInfo::try_new(
                    file.digest.as_ref().map_or("", |d| &d.hash),
                    file.digest.as_ref().map_or(0, |d| d.size_bytes),
                )?;
                let exists_in_cache = {
                    let lock = self.completeness_cache.lock().unwrap();
                    lock.contains(&file_digest)
                };
                if !exists_in_cache {
                    let exists = self
                        .pin_inner()
                        .has(file_digest)
                        .await
                        .err_tip(|| "Failed to check file existence in CAS")?;
                    if let Some(_) = exists {
                        let mut lock = self.completeness_cache.lock().unwrap();
                        lock.insert(file_digest);
                    }
                }
            }

            for child_directory in &directory.directories {
                let child_digest = child_directory.digest.as_ref().map_or_else(
                    || Err(Error::new(Code::Internal, "Digest is None".to_string())),
                    |digest| DigestInfo::try_new(&digest.hash.clone(), digest.size_bytes as usize).map_err(Error::from),
                )?;
                stack.push(child_digest);
            }
        }
        Ok(())
    }

    pub async fn has_in_cache(&self, digest: &DigestInfo) -> bool {
        match self.completeness_cache.lock() {
            Ok(cache) => cache.contains(digest),
            Err(_) => false,
        }
    }

    fn pin_inner(&self) -> Pin<&dyn StoreTrait> {
        Pin::new(self.inner_store.as_ref())
    }
}

#[async_trait]
impl StoreTrait for CompletenessCheckingStore {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        for (i, digest) in digests.iter().enumerate() {
            if self.find_in_cas(*digest).await.is_err() {
                results[i] = None;
            }
        }

        self.pin_inner().has_with_results(digests, results).await
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
