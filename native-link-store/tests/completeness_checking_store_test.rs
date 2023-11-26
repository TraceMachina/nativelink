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

use error::Error;
use native_link_config::stores::MemoryStore as MemoryStoreConfig;
use native_link_store::ac_utils::serialize_and_upload_message;
use native_link_store::completeness_checking_store::CompletenessCheckingStore;
use native_link_store::memory_store::MemoryStore;
use native_link_util::common::DigestInfo;
use native_link_util::digest_hasher::DigestHasherFunc::Blake3;
use native_link_util::store_trait::Store;
use proto::build::bazel::remote::execution::v2::{
    ActionResult as ProtoActionResult, Directory, DirectoryNode, FileNode, OutputDirectory, OutputFile, Tree,
};

#[cfg(test)]
mod completeness_checking_store_tests {
    use super::*;

    const DIGEST1: DigestInfo = DigestInfo::new([0u8; 32], 0);
    const DIGEST2: DigestInfo = DigestInfo::new([1u8; 32], 0);
    const DIGEST3: DigestInfo = DigestInfo::new([2u8; 32], 0);
    const DIGEST4: DigestInfo = DigestInfo::new([3u8; 32], 0);
    const DIGEST5: DigestInfo = DigestInfo::new([4u8; 32], 0);
    const DIGEST6: DigestInfo = DigestInfo::new([5u8; 32], 0);
    const DIGEST7: DigestInfo = DigestInfo::new([6u8; 32], 0);

    async fn setup() -> Result<(Arc<CompletenessCheckingStore>, Arc<MemoryStore>, Vec<DigestInfo>), Error> {
        let ac_store = Arc::new(MemoryStore::new(&MemoryStoreConfig::default()));
        let cas_store = Arc::new(MemoryStore::new(&MemoryStoreConfig::default()));
        let store_owned = Arc::new(CompletenessCheckingStore::new(ac_store.clone(), cas_store.clone()));
        let pinned_ac: Pin<&dyn Store> = Pin::new(ac_store.as_ref());
        let pinned_cas: Pin<&dyn Store> = Pin::new(cas_store.as_ref());

        pinned_cas.update_oneshot(DIGEST1, "".into()).await?;
        pinned_cas.update_oneshot(DIGEST2, "".into()).await?;
        pinned_cas.update_oneshot(DIGEST3, "".into()).await?;
        pinned_cas.update_oneshot(DIGEST4, "".into()).await?;
        pinned_cas.update_oneshot(DIGEST5, "".into()).await?;
        pinned_cas.update_oneshot(DIGEST6, "".into()).await?;
        pinned_cas.update_oneshot(DIGEST7, "".into()).await?;

        let tree = Tree {
            root: Some(Directory {
                files: vec![FileNode {
                    name: "bar".to_string(),
                    digest: Some(DIGEST1.into()),
                    ..Default::default()
                }],
                directories: vec![DirectoryNode {
                    name: "foo".to_string(),
                    digest: Some(DIGEST2.into()),
                }],
                ..Default::default()
            }),
            children: vec![Directory {
                files: vec![
                    FileNode {
                        name: "baz".to_string(),
                        digest: Some(DIGEST3.into()),
                        is_executable: true,
                        ..Default::default()
                    },
                    FileNode {
                        name: "buz".to_string(),
                        digest: Some(DIGEST4.into()),
                        is_executable: true,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
        };

        let tree_digest = serialize_and_upload_message(&tree, pinned_cas, &mut Blake3.into()).await?;

        let output_directory = OutputDirectory {
            path: "".to_string(),
            tree_digest: Some(tree_digest.into()),
            ..Default::default()
        };

        serialize_and_upload_message(&output_directory, pinned_cas, &mut Blake3.into()).await?;

        let action_result = ProtoActionResult {
            output_files: vec![OutputFile {
                digest: Some(DIGEST5.into()),
                ..Default::default()
            }],
            output_directories: vec![output_directory],
            stdout_digest: Some(DIGEST6.into()),
            stderr_digest: Some(DIGEST7.into()),
            ..Default::default()
        };

        let digest = serialize_and_upload_message(&action_result, pinned_cas, &mut Blake3.into()).await?;
        serialize_and_upload_message(&action_result, pinned_ac, &mut Blake3.into()).await?;

        let digests = vec![digest];
        Ok((store_owned, cas_store, digests))
    }

    #[tokio::test]
    async fn verify_completeness_check() -> Result<(), Error> {
        let (store_owned, cas_store, digests) = setup().await?;

        let pinned_store: Pin<&dyn Store> = Pin::new(&*store_owned);
        let pinned_cas: Pin<&dyn Store> = Pin::new(&*cas_store);

        {
            let res = pinned_store.has_many(&digests).await;
            assert!(res.is_ok(), "Expected has_many to succeed.");
            assert!(
                res.unwrap()[0].is_some(),
                "Results should be some with all items in CAS."
            );
        }

        {
            cas_store.remove_entry(&DIGEST1).await;
            let res = pinned_store.has_many(&digests).await;
            assert!(res.is_ok(), "Expected has_many to succeed.");
            assert!(
                res.unwrap()[0].is_none(),
                "Results should be none with missing root file."
            );
        }

        pinned_cas.update_oneshot(DIGEST1, "".into()).await?;

        {
            cas_store.remove_entry(&DIGEST3).await;
            let res = pinned_store.has_many(&digests).await;
            assert!(res.is_ok(), "Expected has_many to succeed.");
            assert!(res.unwrap()[0].is_none(), "All items should show as existing in CAS.");
        }

        pinned_cas.update_oneshot(DIGEST3, "".into()).await?;

        {
            cas_store.remove_entry(&DIGEST5).await;
            let res = pinned_store.has_many(&digests).await;
            assert!(res.is_ok(), "Expected has_many to succeed.");
            assert!(res.unwrap()[0].is_none(), "All items should show as existing in CAS.");
        }

        pinned_cas.update_oneshot(DIGEST5, "".into()).await?;

        {
            cas_store.remove_entry(&DIGEST6).await;
            let res = pinned_store.has_many(&digests).await;
            assert!(res.is_ok(), "Expected has_many to succeed.");
            assert!(res.unwrap()[0].is_none(), "All items should show as existing in CAS.");
        }

        pinned_cas.update_oneshot(DIGEST6, "".into()).await?;

        {
            cas_store.remove_entry(&DIGEST7).await;
            let res = pinned_store.has_many(&digests).await;
            assert!(res.is_ok(), "Expected has_many to succeed.");
            assert!(res.unwrap()[0].is_none(), "All items should show as existing in CAS.");
        }

        Ok(())
    }

    #[tokio::test]
    async fn verify_completeness_get() -> Result<(), Error> {
        let (store_owned, cas_store, digests) = setup().await?;

        let pinned_store: Pin<&dyn Store> = Pin::new(&*store_owned);

        {
            assert!(
                pinned_store.get_part_unchunked(digests[0], 0, None, None).await.is_ok(),
                ".get() should have succeeded with all items in CAS",
            );
        }

        {
            cas_store.remove_entry(&DIGEST5).await;
            assert!(
                pinned_store
                    .get_part_unchunked(digests[0], 0, None, None)
                    .await
                    .is_err(),
                ".get() should have failed with item missing in CAS",
            );
        }

        Ok(())
    }
}
