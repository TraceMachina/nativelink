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
use futures::stream::{self, StreamExt, TryStreamExt};
use proto::build::bazel::remote::execution::v2::{ActionResult as ProtoActionResult, Tree};

use buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use common::DigestInfo;
use error::{Error, ResultExt};
use traits::{StoreTrait, UploadSizeInfo};

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

        for digest in digests {
            let action_result: ActionResult = get_and_decode_digest::<ProtoActionResult>(self.pin_cas(), digest)
                .await?
                .try_into()
                .err_tip(|| "Action result could not be converted in completeness checking store has")?;

            // Something that takes a stream of iterators and flattens it into a stream of items
            // 'temp' is a stream of digests obtained from the output folders of the action result
            let _temp =
                stream::iter(action_result.output_folders)
                    .then(|directory| async move {
                        get_and_decode_digest::<Tree>(self.pin_cas(), &directory.tree_digest).await
                    })
                    .map_ok(|tree| async {
                        let trees =
                            tree.children
                                .into_iter()
                                .chain(tree.root.into_iter())
                                .flat_map(|child_directory| {
                                    child_directory.files.into_iter().filter_map(|file| file.digest)
                                });

                        let item_stream = stream::iter(trees);
                        item_stream
                            .map(|file_digest| async move {
                                let digest_info =
                                    DigestInfo::try_new(&file_digest.hash, file_digest.size_bytes as usize)?;
                                self.pin_cas().has(digest_info).await
                            })
                            .buffer_unordered(10)
                            .try_collect::<Vec<_>>()
                            .await
                    });
        }

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
