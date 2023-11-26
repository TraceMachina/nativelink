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
use std::env::var;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use error::{make_err, make_input_err, Code, Error, ResultExt};
use futures::future::BoxFuture;
use futures::stream::{futures_unordered, FuturesUnordered, StreamExt};
use futures::{FutureExt, TryFutureExt, TryStreamExt};
use native_link_util::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use native_link_util::common::DigestInfo;
use native_link_util::store_trait::{Store, UploadSizeInfo};
use parking_lot::Mutex;
use proto::build::bazel::remote::execution::v2::{ActionResult as ProtoActionResult, Tree as ProtoTree};

use crate::ac_utils::{get_and_decode_digest, get_size_and_decode_digest};

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
        action_result_digests: &[DigestInfo],
        results: &mut [Option<usize>],
    ) -> Result<(), Error> {
        enum FutureResult<'a> {
            AddFuturesAndDigests((Vec<BoxFuture<'a, FutureResult<'a>>>, Vec<(DigestInfo, usize)>)),
            Err(usize),
        }

        let mut futures = FuturesUnordered::new();

        for (i, digest) in action_result_digests.iter().enumerate() {
            // let digest_queue = digest_queue.clone();
            futures.push(
                get_size_and_decode_digest::<ProtoActionResult>(self.pin_cas(), digest)
                    .and_then(move |(action_result, size)| async move {
                        // We need to add 2 because stderr and stdout digests.
                        const NUM_ADDTIONAL_DIGESTS: usize = 2;
                        let mut digest_infos =
                            Vec::with_capacity(action_result.output_files.len() + NUM_ADDTIONAL_DIGESTS);
                        let maybe_digests = action_result
                            .stderr_digest
                            .map(DigestInfo::try_from)
                            .into_iter()
                            .chain(action_result.stdout_digest.map(DigestInfo::try_from).into_iter())
                            .chain(
                                action_result
                                    .output_files
                                    .into_iter()
                                    .filter_map(|file| file.digest.map(DigestInfo::try_from)),
                            );
                        for maybe_digest in maybe_digests {
                            match maybe_digest {
                                Ok(digest) => digest_infos.push((digest, i)),
                                Err(_) => return Err(make_input_err!("")),
                            }
                        }

                        let v = action_result
                            .output_directories
                            .into_iter()
                            .map(move |output_directory| {
                                {
                                    // let digest_queue = digest_queue.clone();
                                    async move {
                                        let Ok(tree_digest) = output_directory
                                            .tree_digest
                                            .err_tip(|| "")
                                            .and_then(DigestInfo::try_from)
                                        else {
                                            // results[i] = None;
                                            return FutureResult::Err(i);
                                        };
                                        get_and_decode_digest::<ProtoTree>(self.pin_cas(), &tree_digest)
                                            .map_ok(|tree| {
                                                let digest_count =
                                                    tree.children.iter().chain(&tree.root).fold(0, |acc, directory| {
                                                        acc + directory.files.len() + directory.directories.len()
                                                    });
                                                let mut digest_infos = Vec::with_capacity(digest_count);
                                                for directory in tree.children.into_iter().chain(tree.root) {
                                                    let maybe_digests = directory
                                                        .files
                                                        .into_iter()
                                                        .filter_map(|file| file.digest.map(DigestInfo::try_from))
                                                        .chain(directory.directories.into_iter().filter_map(
                                                            |directory| directory.digest.map(DigestInfo::try_from),
                                                        ))
                                                        .collect::<Vec<_>>();
                                                    for maybe_digest in maybe_digests {
                                                        match maybe_digest {
                                                            Ok(digest) => digest_infos.push((digest, i)),
                                                            Err(_) => return FutureResult::Err(i),
                                                        }
                                                    }
                                                }
                                                FutureResult::AddFuturesAndDigests((vec![], digest_infos))
                                            })
                                            .map(move |result| match result {
                                                Ok(v) => v,
                                                Err(_) => FutureResult::Err(i),
                                            })
                                            .await
                                    }
                                }
                                .boxed()
                            })
                            .collect::<Vec<_>>();

                        Ok(FutureResult::AddFuturesAndDigests((v, digest_infos)))
                    })
                    .map(move |v| v.unwrap_or_else(|_| FutureResult::Err(i)))
                    .boxed(),
            );
        }

        use std::sync::atomic::{AtomicBool, Ordering};
        let mut digest_deque = VecDeque::new();
        let has_request_outstanding = Arc::new(AtomicBool::new(false));

        while let Some(future_result) = futures.next().await {
            match future_result {
                FutureResult::Err(i) => {
                    results[i] = None;
                }
                FutureResult::AddFuturesAndDigests((futures_to_add, digest_infos)) => {
                    futures.extend(futures_to_add.into_iter());
                    digest_deque.extend(digest_infos.into_iter());
                    if !digest_deque.is_empty() && !has_request_outstanding.load(Ordering::Acquire) {
                        let has_request_outstanding = has_request_outstanding.clone();
                        has_request_outstanding.store(true, Ordering::Release);

                        let (digests, indexes): (Vec<_>, Vec<_>) = digest_deque.drain(..).unzip();

                        let res = self.pin_cas().has_many(&digests).await?;

                        for (i, result) in res.iter().enumerate() {
                            if result.is_none() {
                                results[indexes[i]] = None;
                            }
                        }

                        futures.push(
                            async move {
                                has_request_outstanding.store(false, Ordering::Release);
                                println!("Results {:?}", results);
                                FutureResult::Err(0)
                                //FutureResult::AddFuturesAndDigests
                            }
                            .boxed(),
                        );
                    }
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
        // TODO(blake) Fix this to use private.has().
        // // TODO (BlakeHatch) This pair is calling .has() which calls get_and_decode_digest() which calls .get(). Fix this so that
        // // it is making only one call to get and is not redundant. Maybe can be done by storing result of get() in results of has_with_results.
        // pin_ac.has(digest).await.err_tip(|| "Items not found in CAS")?;
        // self.has(digest).
        pin_ac.get_part_ref(digest, writer, offset, length).await
    }

    fn as_any(self: Arc<Self>) -> Box<dyn std::any::Any + Send> {
        Box::new(self)
    }
}
