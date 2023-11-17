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

use ac_utils::get_and_decode_digest;
use action_messages::ActionResult;
use async_trait::async_trait;
use futures::future::{try_join_all, BoxFuture};
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
pub fn check_directory_files_in_cas<'a>(
    cas_store: Pin<&'a dyn StoreTrait>,
    ac_store: Pin<&'a dyn StoreTrait>,
    digest: &'a DigestInfo,
) -> BoxFuture<'a, Result<(), Error>> {
    async move {
        let directory = get_and_decode_digest::<ProtoDirectory>(ac_store, digest)
            .await
            .err_tip(|| "Converting digest to Directory")?;
        let mut futures = vec![];

        let mut file_digests = vec![];
        for file in directory.files {
            let digest: DigestInfo = file
                .digest
                .err_tip(|| "Expected Digest to exist in Directory::file::digest")?
                .try_into()
                .err_tip(|| "In Directory::file::digest")?;
            file_digests.push(digest);
        }

        if !file_digests.is_empty() {
            futures.push(
                async move {
                    cas_store.has_many(&file_digests).await.and_then(|exists_vec| {
                        if exists_vec.iter().all(|result| {
                            println!("res: {:?}", result);
                            result.is_some()
                        }) {
                            Ok(())
                        } else {
                            Err(make_err!(Code::NotFound, "One or more files not found in CAS"))
                        }
                    })
                }
                .left_future(),
            );
        }

        for directory in directory.directories {
            let digest: DigestInfo = directory
                .digest
                .err_tip(|| "Expected Digest to exist in Directory::directories::digest")?
                .try_into()
                .err_tip(|| "In Directory::file::digest")?;
            futures.push(
                async move {
                    check_directory_files_in_cas(cas_store, ac_store, &digest)
                        .await
                        .err_tip(|| "Could not recursively traverse")?;
                    Ok(())
                }
                .right_future(),
            );
        }

        let result = try_join_all(futures).await;
        match result {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
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
        _results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        // The proto promises that all results will exist in the CAS when
        // requested using the associated actions. However we currently allow
        // stores to prune themselves which violates this condition. Therefore we need this completeness checking
        // store to check that all results exist in the CAS before we allow the the action result
        // to be returned to the client.
        // * Take the root digest which is the serialzied action proto hash
        // * Deserialize the action proto
        // * Check files in the root tree exist in the CAS but there's directories that needs
        // to be traversed as the directories can have directories and files in them and esure all
        // of them exist in the CAS.

        let pin_cas = self.pin_cas();
        let pin_ac = self.pin_ac();
        let mut futures = vec![];

        for digest in digests {
            let action_result: ActionResult = get_and_decode_digest::<ProtoActionResult>(pin_ac, digest)
                .await?
                .try_into()
                .err_tip(|| "Action result could not be converted in completeness checking store")?;

            for output_file in &action_result.output_files {
                let file_digest: DigestInfo = output_file.digest;

                futures.push(
                    async move {
                        check_directory_files_in_cas(pin_cas, pin_ac, &file_digest)
                            .await
                            .err_tip(|| "Couldn't recursively call directory check")
                    }
                    .left_future(),
                );
            }

            for output_directory in action_result.output_folders {
                let tree_digest = output_directory.tree_digest;
                futures.push(
                    async move {
                        check_directory_files_in_cas(pin_cas, pin_ac, &tree_digest)
                            .await
                            .err_tip(|| "Couldn't recursively call directory check")
                    }
                    .right_future(),
                );
            }
        }

        let results = try_join_all(futures).await;

        match results {
            Ok(_) => Ok(()),
            Err(e) => Err(e),
        }
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
        match pin_ac.has_with_results(&[digest], &mut []).await {
            Ok(_) => pin_ac.get_part_ref(digest, writer, offset, length).await,
            Err(e) => Err(e),
        }
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
