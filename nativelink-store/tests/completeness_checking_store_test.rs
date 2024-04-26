// Copyright 2023 The NativeLink Authors. All rights reserved.
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

use nativelink_config::stores::MemoryStore as MemoryStoreConfig;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::{
    ActionResult as ProtoActionResult, Directory, DirectoryNode, FileNode, OutputDirectory,
    OutputFile, Tree,
};
use nativelink_store::ac_utils::serialize_and_upload_message;
use nativelink_store::completeness_checking_store::CompletenessCheckingStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::store_trait::Store;

#[cfg(test)]
mod completeness_checking_store_tests {
    use super::*;

    const ROOT_FILE: DigestInfo = DigestInfo::new([0u8; 32], 0);
    const ROOT_DIRECTORY: DigestInfo = DigestInfo::new([1u8; 32], 0);
    const CHILD_FILE: DigestInfo = DigestInfo::new([2u8; 32], 0);
    const OUTPUT_FILE: DigestInfo = DigestInfo::new([4u8; 32], 0);
    const STDOUT: DigestInfo = DigestInfo::new([5u8; 32], 0);
    const STDERR: DigestInfo = DigestInfo::new([6u8; 32], 0);

    async fn setup() -> Result<(Arc<CompletenessCheckingStore>, Arc<MemoryStore>, DigestInfo), Error>
    {
        let backend_store = Arc::new(MemoryStore::new(&MemoryStoreConfig::default()));
        let cas_store = Arc::new(MemoryStore::new(&MemoryStoreConfig::default()));
        let ac_owned = Arc::new(CompletenessCheckingStore::new(
            backend_store.clone(),
            cas_store.clone(),
        ));
        let pinned_ac: Pin<&dyn Store> = Pin::new(backend_store.as_ref());
        let pinned_cas: Pin<&dyn Store> = Pin::new(cas_store.as_ref());

        pinned_cas.update_oneshot(ROOT_FILE, "".into()).await?;
        // Note: Explicitly not uploading `ROOT_DIRECTORY`. See: TraceMachina/nativelink#747.
        pinned_cas.update_oneshot(CHILD_FILE, "".into()).await?;
        pinned_cas.update_oneshot(OUTPUT_FILE, "".into()).await?;
        pinned_cas.update_oneshot(STDOUT, "".into()).await?;
        pinned_cas.update_oneshot(STDERR, "".into()).await?;

        let tree = Tree {
            root: Some(Directory {
                files: vec![FileNode {
                    digest: Some(ROOT_FILE.into()),
                    ..Default::default()
                }],
                directories: vec![DirectoryNode {
                    digest: Some(ROOT_DIRECTORY.into()),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            children: vec![Directory {
                files: vec![FileNode {
                    digest: Some(CHILD_FILE.into()),
                    ..Default::default()
                }],
                ..Default::default()
            }],
        };

        let tree_digest =
            serialize_and_upload_message(&tree, pinned_cas, &mut DigestHasherFunc::Blake3.hasher())
                .await?;

        let output_directory = OutputDirectory {
            tree_digest: Some(tree_digest.into()),
            ..Default::default()
        };

        serialize_and_upload_message(
            &output_directory,
            pinned_cas,
            &mut DigestHasherFunc::Blake3.hasher(),
        )
        .await?;

        let action_result = ProtoActionResult {
            output_files: vec![OutputFile {
                digest: Some(OUTPUT_FILE.into()),
                ..Default::default()
            }],
            output_directories: vec![output_directory],
            stdout_digest: Some(STDOUT.into()),
            stderr_digest: Some(STDERR.into()),
            ..Default::default()
        };

        // The structure of the action result is not following the spec, but is simplified for testing purposes.
        let action_result_digest = serialize_and_upload_message(
            &action_result,
            pinned_ac,
            &mut DigestHasherFunc::Blake3.hasher(),
        )
        .await?;

        Ok((ac_owned, cas_store, action_result_digest))
    }

    #[nativelink_test]
    async fn verify_has_function_call_checks_cas() -> Result<(), Error> {
        {
            // Completeness check should succeed when all digests exist in CAS.

            let (ac_store, _cas_store, action_result_digest) = setup().await?;

            let pinned_store: Pin<&dyn Store> = Pin::new(ac_store.as_ref());

            let res = pinned_store
                .has_many(&[action_result_digest])
                .await
                .unwrap();
            assert!(
                res[0].is_some(),
                "Results should be some with all items in CAS."
            );
        }

        {
            // Completeness check should fail when root file digest is missing.

            let (ac_store, cas_store, action_result_digest) = setup().await?;

            let pinned_store: Pin<&dyn Store> = Pin::new(ac_store.as_ref());

            cas_store.remove_entry(&ROOT_FILE).await;

            let res = pinned_store
                .has_many(&[action_result_digest])
                .await
                .unwrap();
            assert!(
                res[0].is_none(),
                "Results should be none with missing root file."
            );
        }

        {
            // Completeness check should fail when child file digest is missing.

            let (ac_store, cas_store, action_result_digest) = setup().await?;

            let pinned_store: Pin<&dyn Store> = Pin::new(ac_store.as_ref());

            cas_store.remove_entry(&CHILD_FILE).await;
            let res = pinned_store
                .has_many(&[action_result_digest])
                .await
                .unwrap();
            assert!(
                res[0].is_none(),
                "Results should be none with missing root file."
            );
        }

        {
            // Completeness check should fail when output file digest is missing.

            let (ac_store, cas_store, action_result_digest) = setup().await?;

            let pinned_store: Pin<&dyn Store> = Pin::new(ac_store.as_ref());

            cas_store.remove_entry(&OUTPUT_FILE).await;
            let res = pinned_store
                .has_many(&[action_result_digest])
                .await
                .unwrap();
            assert!(
                res[0].is_none(),
                "Results should be none with missing root file."
            );
        }

        {
            // Completeness check should fail when stdout digest is missing.

            let (ac_store, cas_store, action_result_digest) = setup().await?;

            let pinned_store: Pin<&dyn Store> = Pin::new(ac_store.as_ref());

            cas_store.remove_entry(&STDOUT).await;
            let res = pinned_store
                .has_many(&[action_result_digest])
                .await
                .unwrap();
            assert!(
                res[0].is_none(),
                "Results should be none with missing root file."
            );
        }

        {
            // Completeness check should fail when stderr digest is missing.

            let (ac_store, cas_store, action_result_digest) = setup().await?;

            let pinned_store: Pin<&dyn Store> = Pin::new(ac_store.as_ref());

            cas_store.remove_entry(&STDERR).await;
            let res = pinned_store
                .has_many(&[action_result_digest])
                .await
                .unwrap();
            assert!(
                res[0].is_none(),
                "Results should be none with missing root file."
            );
        }

        Ok(())
    }

    #[nativelink_test]
    async fn verify_completeness_get() -> Result<(), Error> {
        {
            // Completeness check in get call should succeed when all digests exist in CAS.

            let (ac_store, _cas_store, action_result_digest) = setup().await?;

            let pinned_store: Pin<&dyn Store> = Pin::new(ac_store.as_ref());

            assert!(
                pinned_store
                    .get_part_unchunked(action_result_digest, 0, None)
                    .await
                    .is_ok(),
                ".get() should succeed with all items in CAS",
            );
        }

        {
            // Completeness check in get call should fail when digest is missing in CAS.

            let (ac_store, cas_store, action_result_digest) = setup().await?;

            let pinned_store: Pin<&dyn Store> = Pin::new(ac_store.as_ref());

            cas_store.remove_entry(&OUTPUT_FILE).await;

            assert!(
                pinned_store
                    .get_part_unchunked(action_result_digest, 0, None)
                    .await
                    .is_err(),
                ".get() should fail with item missing in CAS",
            );
        }

        Ok(())
    }
}
