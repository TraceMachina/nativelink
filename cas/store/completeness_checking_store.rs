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

use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::Mutex;

use ac_utils::get_and_decode_digest;
use action_messages::ActionResult;
use async_trait::async_trait;
use futures::future::BoxFuture;
use futures::stream::{FuturesUnordered, StreamExt};
use futures::FutureExt;
use proto::build::bazel::remote::execution::v2::{ActionResult as ProtoActionResult, Directory as ProtoDirectory};

use buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use common::DigestInfo;
use error::{make_err, Code, Error, ResultExt};
use traits::{StoreTrait, UploadSizeInfo};

/// Aggressively check if the digests of files exist in the cas. This function
/// will spawn unbounded number of futures check all of the files. The store itself
/// should be rate limited if spawning too many requests at once is an issue.
/// Sadly we cannot use `async fn` here because the rust compiler cannot determine the auto traits
/// of the future. So we need to force this function to return a dynamic future instead.
/// see: https://github.com/rust-lang/rust/issues/78649
pub fn check_files_and_directories_in_cas<'a>(
    cas_store: Pin<&'a dyn StoreTrait>,
    ac_store: Pin<&'a dyn StoreTrait>,
    file_digests: &'a [DigestInfo],
    directory_digests: &'a [DigestInfo],
    results_arc_mut: Arc<Mutex<Vec<Option<usize>>>>,
    res_index: usize,
) -> BoxFuture<'a, Result<(), Error>> {
    async move {
        let mut futures = FuturesUnordered::new();
        let results_arc_mut_clone = results_arc_mut.clone();

        if !file_digests.is_empty() {
            futures.push(
                async move {
                    let cas_lookup_results = cas_store.has_many(file_digests).await;

                    let mut results = results_arc_mut.lock().await;
                    if cas_lookup_results.iter().any(|x| x.iter().any(|n| n.is_none())) {
                        results[res_index] = None;
                        Err(make_err!(Code::NotFound, "Files not found found in CAS"))
                    } else {
                        results[res_index] = Some(cas_lookup_results.into_iter().len());
                        Ok(())
                    }
                }
                .left_future(),
            );
        }

        for directory_digest in directory_digests {
            let directory = get_and_decode_digest::<ProtoDirectory>(ac_store, directory_digest)
                .await
                .err_tip(|| "Converting digest to Directory")?;

            let directory_digests: Vec<_> = Result::from_iter(directory.directories.into_iter().map(|dir_node| {
                dir_node
                    .digest
                    .err_tip(|| "Expected Digest to exist in Directory::directory::digest")?
                    .try_into()
                    .err_tip(|| "in completeness_checking_store::check_files_and_directories_in_cas")
            }))?;

            let dir_file_digests: Vec<_> = Result::from_iter(directory.files.into_iter().map(|file| {
                file.digest
                    .err_tip(|| "Expected Digest to exist in Directory::file::digest")?
                    .try_into()
                    .err_tip(|| "in completeness_checking_store::check_files_and_directories_in_cas")
            }))?;

            let res_clone = results_arc_mut_clone.clone();

            futures.push(
                async move {
                    let _ = check_files_and_directories_in_cas(
                        cas_store,
                        ac_store,
                        &dir_file_digests,
                        &directory_digests,
                        res_clone,
                        res_index,
                    )
                    .await
                    .err_tip(|| "Could not recursively traverse");
                    Ok(())
                }
                .right_future(),
            );
        }

        while let Some(result) = futures.next().await {
            result?
        }

        Ok(())
    }
    .boxed()
}

pub struct CompletenessCheckingStore {
    cas_store: Arc<dyn StoreTrait>,
    ac_store: Arc<dyn StoreTrait>,
}

impl CompletenessCheckingStore {
    pub fn new(ac_store: Arc<dyn StoreTrait>, cas_store: Arc<dyn StoreTrait>) -> Self {
        CompletenessCheckingStore { cas_store, ac_store }
    }

    fn pin_cas(&self) -> Pin<&dyn StoreTrait> {
        Pin::new(self.cas_store.as_ref())
    }

    fn pin_ac(&self) -> Pin<&dyn StoreTrait> {
        Pin::new(self.ac_store.as_ref())
    }
}

#[async_trait]
impl StoreTrait for CompletenessCheckingStore {
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
                    let action_result: ActionResult =
                        get_and_decode_digest::<ProtoActionResult>(pin_ac, action_res_digest)
                            .await?
                            .try_into()
                            .err_tip(|| "Action result could not be converted in completeness checking store")?;

                    let file_digests: Vec<_> = action_result.output_files.into_iter().map(|file| file.digest).collect();

                    let directories = action_result.output_folders;

                    let directories: Vec<_> = directories.into_iter().map(|directory| directory.tree_digest).collect();

                    check_files_and_directories_in_cas(
                        pin_cas,
                        pin_ac,
                        &file_digests,
                        &directories,
                        results_vec_clone,
                        i,
                    )
                    .await
                    .err_tip(|| "Could not traverse recursively")
                }
                .boxed(),
            );
        }

        while let Some(result) = futures.next().await {
            match result {
                Ok(_) => results[0] = Some(1),
                Err(err) => return Err(err),
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
        pin_ac.has(digest).await.err_tip(|| "Items not found in CAS")?;
        pin_ac.get_part_ref(digest, writer, offset, length).await
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
