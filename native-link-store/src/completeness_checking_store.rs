// Copyright 2023 The Native Link Authors. All rights reserved.
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

use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use error::{make_err, Code, Error, ResultExt};
use futures::future::BoxFuture;
use futures::stream::FuturesUnordered;
use futures::{FutureExt, TryStreamExt};
use native_link_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use native_link_util::common::DigestInfo;
use native_link_util::store_trait::{Store, UploadSizeInfo};
use proto::build::bazel::remote::execution::v2::{ActionResult as ProtoActionResult, Tree};
use tokio::sync::Mutex;

use crate::ac_utils::get_and_decode_digest;

/// Aggressively check if the digests of files exist in the cas. This function
/// will spawn unbounded number of futures check all of the files. The store itself
/// should be rate limited if spawning too many requests at once is an issue.
/// Sadly we cannot use `async fn` here because the rust compiler cannot determine the auto traits
/// of the future. So we need to force this function to return a dynamic future instead.
/// see: https://github.com/rust-lang/rust/issues/78649
fn check_files_and_directories_in_cas<'a>(
    cas_store: Pin<&'a dyn Store>,
    ac_store: Pin<&'a dyn Store>,
    file_digests: &'a mut [DigestInfo],
    tree_digests: &'a mut [DigestInfo],
    results_arc_mut: Arc<Mutex<Vec<Option<usize>>>>,
    res_index: usize,
) -> BoxFuture<'a, Result<(), Error>> {
    Box::pin(async move {
        let mut futures = FuturesUnordered::new();

        let mut file_vec: Vec<_> = file_digests.to_vec();
        let mut directory_queue: VecDeque<_> = tree_digests.iter().collect();

        while let Some(directory_digest) = directory_queue.pop_front() {
            let tree = get_and_decode_digest::<Tree>(ac_store, directory_digest)
                .await
                .err_tip(|| "Converting digest to Directory")?;

            let root_files: Result<Vec<DigestInfo>, _> = tree
                .root
                .ok_or_else(|| make_err!(Code::Aborted, "Could not get root from tree"))
                .err_tip(|| "Expected Root to exist in Tree")?
                .files
                .into_iter()
                .map(|file| {
                    file.digest
                        .ok_or_else(|| make_err!(Code::NotFound, "Expected Digest to exist in Directory::file::digest"))
                        .map_err(|_| make_err!(Code::NotFound, "Expected Digest to exist in Directory::file::digest"))
                        .and_then(|digest| {
                            digest
                                .try_into()
                                .map_err(|_| make_err!(Code::NotFound, "Failed to convert digest"))
                        })
                })
                .collect();

            file_vec.extend(root_files?);

            for child in tree.children {
                futures.push({
                    futures::future::ready({
                        let child_files: Vec<DigestInfo> = child
                            .files
                            .into_iter()
                            .map(|file| {
                                file.digest
                                    .err_tip(|| "Expected Digest to exist in Directory::file::digest")?
                                    .try_into()
                                    .err_tip(|| "in completeness_checking_store::check_files_and_directories_in_cas")
                            })
                            .collect::<Result<Vec<_>, Error>>()?;

                        file_vec.extend(child_files);
                        Ok::<(), Error>(())
                    })
                    .left_future()
                })
            }

            let results_arc_mut_clone = results_arc_mut.clone();
            let file_vec_clone = file_vec.clone();

            futures.push(
                async move {
                    let file_digests: &[DigestInfo] = &file_vec_clone;
                    let cas_lookup_results = cas_store.has_many(file_digests).await;

                    let mut results = results_arc_mut_clone.lock().await;
                    if cas_lookup_results.iter().any(|x| x.iter().any(|n| n.is_none())) {
                        results[res_index] = None;
                        Err(make_err!(Code::NotFound, "Files not found found in CAS"))
                    } else {
                        results[res_index] = Some(cas_lookup_results.into_iter().len());
                        Ok(())
                    }
                }
                .right_future(),
            );
            file_vec.clear();
        }

        while let Some(()) = futures.try_next().await? {}

        Ok(())
    })
}

pub struct CompletenessCheckingStore {
    cas_store: Arc<dyn Store>,
    ac_store: Arc<dyn Store>,
}

impl CompletenessCheckingStore {
    pub fn new(ac_store: Arc<dyn Store>, cas_store: Arc<dyn Store>) -> Self {
        CompletenessCheckingStore { cas_store, ac_store }
    }

    fn pin_cas(&self) -> Pin<&dyn Store> {
        Pin::new(self.cas_store.as_ref())
    }

    fn pin_ac(&self) -> Pin<&dyn Store> {
        Pin::new(self.ac_store.as_ref())
    }
}

#[async_trait]
impl Store for CompletenessCheckingStore {
    async fn has_with_results(
        self: Pin<&Self>,
        digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        let pin_cas = self.pin_cas();
        let pin_ac = self.pin_ac();
        let mut futures = FuturesUnordered::new();
        let results_vec = Arc::new(Mutex::new(results.to_vec()));

        for (i, action_res_digest) in digests.iter().enumerate() {
            let results_vec_clone = results_vec.clone();
            futures.push(
                async move {
                    let action_result: ProtoActionResult =
                        get_and_decode_digest::<ProtoActionResult>(pin_ac, action_res_digest).await?;

                    let mut file_digests: Vec<_> = action_result
                        .output_files
                        .into_iter()
                        .filter_map(|file| {
                            file.digest.and_then(|digest| match digest.try_into() {
                                Ok(digest_info) => Some(digest_info),
                                Err(_) => None,
                            })
                        })
                        .collect();

                    let directories = action_result.output_directories;

                    let mut tree_digests: Vec<_> = directories
                        .into_iter()
                        .filter_map(|directory| {
                            directory
                                .tree_digest
                                .and_then(|tree_digest| tree_digest.try_into().ok())
                        })
                        .collect();

                    check_files_and_directories_in_cas(
                        pin_cas,
                        pin_ac,
                        &mut file_digests,
                        &mut tree_digests,
                        results_vec_clone,
                        i,
                    )
                    .await
                    .err_tip(|| "Could not traverse files and directories in trees")
                }
                .boxed(),
            );
        }

        while let Some(()) = futures.try_next().await? {}

        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        self.pin_ac().update(digest, reader, size_info).await
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        let pin_ac = self.pin_ac();
        // TODO (BlakeHatch) This pair is calling .has() which calls get_and_decode_digest() which calls .get(). Fix this so that
        // it is making only one call to get and is not redundant. Maybe can be done by storing result of get() in results of has_with_results.
        pin_ac.has(digest).await.err_tip(|| "Items not found in CAS")?;
        pin_ac.get_part_ref(digest, writer, offset, length).await
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
