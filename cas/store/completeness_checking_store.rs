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
use futures::stream::FuturesUnordered;
use futures::FutureExt;
use futures::{
    future::BoxFuture,
    stream::{StreamExt, TryStreamExt},
};
use proto::build::bazel::remote::execution::v2::{ActionResult as ProtoActionResult, Directory as ProtoDirectory};

use buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use common::DigestInfo;
use error::{
    //make_err, Code,
    Error,
    ResultExt,
};
use traits::{StoreTrait, UploadSizeInfo};

/// Aggressively check if the digests of files exist in the cas. This function
/// will spawn unbounded number of futures check all of the files. The store itself
/// should be rate limited if spawning too many requests at once is an issue.
// Sadly we cannot use `async fn` here because the rust compiler cannot determine the auto traits
// of the future. So we need to force this function to return a dynamic future instead.
// see: https://github.com/rust-lang/rust/issues/78649
pub fn check_directory_files_in_cas<'a>(
    cas_store: Pin<&'a dyn StoreTrait>,
    digest: &'a DigestInfo,
    current_directory: &'a str,
) -> BoxFuture<'a, Result<(), Error>> {
    async move {
        let directory = get_and_decode_digest::<ProtoDirectory>(cas_store, digest)
            .await
            .err_tip(|| "Converting digest to Directory")?;
        let mut futures = FuturesUnordered::new();

        for file in directory.files {
            let digest: DigestInfo = file
                .digest
                .err_tip(|| "Expected Digest to exist in Directory::file::digest")?
                .try_into()
                .err_tip(|| "In Directory::file::digest")?;
            // Maybe could be made more efficient
            futures.push(cas_store.has(digest).boxed());
        }

        for directory in directory.directories {
            let digest: DigestInfo = directory
                .digest
                .err_tip(|| "Expected Digest to exist in Directory::directories::digest")?
                .try_into()
                .err_tip(|| "In Directory::file::digest")?;
            let new_directory_path = format!("{}/{}", current_directory, directory.name);
            futures.push(
                async move {
                    check_directory_files_in_cas(cas_store, &digest, &new_directory_path)
                        .await
                        .err_tip(|| format!("in traverse_ : {new_directory_path}"))?;
                    Ok(Some(1))
                }
                .boxed(),
            );
        }

        while futures.try_next().await?.is_some() {}
        Ok(())
    }
    .boxed()
}

pub struct CompletenessCheckingStore {
    cas_store: Arc<dyn StoreTrait>,
}

impl CompletenessCheckingStore {
    pub fn new(_ac_store: Arc<dyn StoreTrait>, cas_store: Arc<dyn StoreTrait>) -> Self {
        CompletenessCheckingStore { cas_store }
    }

    fn pin_cas(&self) -> Pin<&dyn StoreTrait> {
        Pin::new(self.cas_store.as_ref())
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
        // stores to prune themselves which violates this condition, because the
        // action cache and CAS are different. Therefore we need this completeness checking
        // store to check that all results exist in the CAS before we allow the the action result
        // to be returned to the client.
        // * Take the root digest which is the serialzied action proto hash
        // * Deserialize the action proto
        // * Check files in the root tree exist in the CAS but there's directories that needs
        // to be traversed as the directories can have directories and files in them and esure all
        // of them exist in the CAS.

        let pin_cas = self.pin_cas();
        let mut futures = FuturesUnordered::new();

        for digest in digests {
            let action_result: ActionResult = get_and_decode_digest::<ProtoActionResult>(pin_cas, digest)
                .await?
                .try_into()
                .err_tip(|| "Action result could not be converted in completeness checking store")?;

            for output_file in &action_result.output_files {
                let file = output_file.digest;
                let file_digest = DigestInfo::try_new(&file.hash_str(), file.size_bytes as usize)?;
                futures.push(
                    async move {
                        if let Err(e) = check_directory_files_in_cas(pin_cas, &file_digest, "").await {
                            eprintln!("Error: {:?}", e);
                        }
                    }
                    .boxed(),
                );
            }

            for output_directory in action_result.output_folders {
                let path = output_directory.path;
                let tree_digest = output_directory.tree_digest;
                futures.push(
                    async move {
                        if let Err(e) = check_directory_files_in_cas(pin_cas, &tree_digest, &path).await {
                            eprintln!("Error: {:?}", e);
                        }
                    }
                    .boxed(),
                );
            }
        }

        while (futures.next().await).is_some() {}

        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        digest: DigestInfo,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<(), Error> {
        self.pin_cas().update(digest, reader, size_info).await
    }

    async fn get_part_ref(
        self: Pin<&Self>,
        digest: DigestInfo,
        writer: &mut DropCloserWriteHalf,
        offset: usize,
        length: Option<usize>,
    ) -> Result<(), Error> {
        self.pin_cas().get_part_ref(digest, writer, offset, length).await
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
