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
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use ac_utils::get_and_decode_digest;
use async_trait::async_trait;
use proto::build::bazel::remote::execution::v2::Directory;

use buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use common::{log, DigestInfo};
use error::{Code, Error, ResultExt};
use traits::{StoreTrait, UploadSizeInfo};

pub struct ExistenceStore {
    inner_store: Arc<dyn StoreTrait>,
    existence_cache: Mutex<HashSet<DigestInfo>>,
}

impl ExistenceStore {
    pub fn new(inner_store: Arc<dyn StoreTrait>) -> Self {
        ExistenceStore {
            inner_store,
            existence_cache: Mutex::new(HashSet::new()),
        }
    }

    pub async fn record_existence_directory_walk(
        self: Arc<Self>,
        digest: DigestInfo,
    ) -> Box<dyn Future<Output = Result<(), Error>>> {
        Box::new(async move {
            // Fetch the Directory object from the CAS using get_and_decode_digest
            let directory = get_and_decode_digest::<Directory>(self.pin_inner(), &digest).await?;

            // Iterate over the files and directories in this Directory
            for file in &directory.files {
                // For each file, check its existence in the CAS
                let file_digest = DigestInfo::try_new(
                    &file.digest.as_ref().map_or("", |d| &d.hash),
                    file.digest.as_ref().map_or(0, |d| d.size_bytes),
                )?;
                let cache_lock = self.existence_cache.lock();
                match cache_lock {
                    Ok(mut cache) => {
                        if cache.contains(&file_digest) {
                            continue;
                        }
                        let exists = self
                            .pin_inner()
                            .has(file_digest)
                            .await
                            .err_tip(|| "Failed to check file existence in CAS")?;
                        if let Some(_) = exists {
                            cache.insert(file_digest);
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to acquire lock on existence_cache: {}", e);
                    }
                }
            }

            for child_directory in &directory.directories {
                // For each child directory, recursively walk it
                let child_digest = child_directory.digest.as_ref().map_or_else(
                    || Err(Error::new(Code::Internal, "Digest is None".to_string())),
                    |digest| DigestInfo::try_new(&digest.hash.clone(), digest.size_bytes as usize).map_err(Error::from),
                )?;

                let _child_result = self.clone().record_existence_directory_walk(child_digest).await;
            }

            Ok(())
        })
    }

    pub async fn has_in_cache(&self, digest: &DigestInfo) -> bool {
        match self.existence_cache.lock() {
            Ok(cache) => cache.contains(digest),
            Err(_) => false,
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
                pruned_digests.push(digest.clone());
            }
        }

        if !pruned_digests.is_empty() {
            let mut inner_results = vec![None; pruned_digests.len()];
            self
                .pin_inner()
                .has_with_results(&pruned_digests, &mut inner_results)
                .await?;

            for (i, result) in inner_results.iter().enumerate() {
                if let Some(_) = result {
                    self.existence_cache.lock().unwrap().insert(pruned_digests[i].clone());
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
