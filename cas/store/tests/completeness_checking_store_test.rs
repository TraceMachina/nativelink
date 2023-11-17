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

// use action_messages::{ActionResult, ExecutionMetadata};
// use proto::build::bazel::remote::execution::v2::ActionResult as ProtoActionResult;
use prost::Message;
use proto::build::bazel::remote::execution::v2::{Directory, DirectoryNode, FileNode};
use std::pin::Pin;
use std::sync::Arc;
// use std::time::{Duration, UNIX_EPOCH};

#[cfg(test)]
mod completeness_checking_store_tests {
    use super::*;

    use common::DigestInfo;
    use completeness_checking_store::CompletenessCheckingStore;
    use error::Error;
    use memory_store::MemoryStore;
    // use traits::StoreTrait;
    // use prost::Message;
    use store::Store;

    //const VALID_HASH1: &str = "0123456789abcdef000000000000000000010000000000000123456789abcdef";
    const FILE1_NAME: &str = "file1.txt";
    const _FILE2_NAME: &str = "file2.txt";
    const FILE3_NAME: &str = "file3.txt";
    const FILE1_CONTENT: &str = "HELLOFILE1";
    const FILE2_CONTENT: &str = "HELLOFILE2";
    const FILE3_CONTENT: &str = "HELLOFILE3";

    const DIRECTORY1_NAME: &str = "folder1";
    const DIRECTORY2_NAME: &str = "folder2";
    const DIRECTORY3_NAME: &str = "folder3";

    const HASH1: &str = "0125456789abcdef000000000000000000000000000000000123456789abcdef";
    const HASH2: &str = "0123f56789abcdef000000000000000000000000000000000123456789abcdef";
    const HASH3: &str = "0126456789abcdef000000000000000000000000000000000123456789abcdef";

    #[tokio::test]
    async fn verify_completeness_check() -> Result<(), Error> {
        let ac_store = Arc::new(MemoryStore::new(&config::stores::MemoryStore::default()));
        let cas_store = Arc::new(MemoryStore::new(&config::stores::MemoryStore::default()));
        let store_owned = CompletenessCheckingStore::new(ac_store.clone(), cas_store.clone());
        let pinned_store: Pin<&dyn Store> = Pin::new(&store_owned);
        let pinned_cas: Pin<&dyn Store> = Pin::new(cas_store.as_ref());

        let file1_digest = DigestInfo::try_new(HASH1, FILE1_CONTENT.len())?;
        let file2_digest = DigestInfo::try_new(HASH2, FILE2_CONTENT.len())?;
        let file3_digest = DigestInfo::try_new(HASH3, FILE3_CONTENT.len())?;

        pinned_cas.update_oneshot(file1_digest, FILE1_CONTENT.into()).await?;
        pinned_cas.update_oneshot(file2_digest, FILE2_CONTENT.into()).await?;
        pinned_cas.update_oneshot(file3_digest, FILE3_CONTENT.into()).await?;

        // Create a directory and add files to it
        let root_directory_digest = {
            // Make and insert (into store) our digest info needed to create our directory & files.
            let directory1_digest = DigestInfo::new([1u8; 32], 32);
            {
                let file1_content_digest = DigestInfo::new([2u8; 32], 32);
                pinned_cas
                    .update_oneshot(file1_content_digest, FILE1_CONTENT.into())
                    .await?;
                let directory1 = Directory {
                    files: vec![FileNode {
                        name: FILE1_NAME.to_string(),
                        digest: Some(file1_content_digest.into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                };
                pinned_cas
                    .update_oneshot(directory1_digest, directory1.encode_to_vec().into())
                    .await?;
            }
            let directory2_digest = DigestInfo::new([3u8; 32], 32);
            {
                // Now upload an empty directory.
                pinned_cas
                    .update_oneshot(directory2_digest, Directory::default().encode_to_vec().into())
                    .await?;
            }
            let root_directory_digest = DigestInfo::new([5u8; 32], 32);
            {
                let root_directory = Directory {
                    directories: vec![
                        DirectoryNode {
                            name: DIRECTORY1_NAME.to_string(),
                            digest: Some(directory1_digest.into()),
                        },
                        DirectoryNode {
                            name: DIRECTORY2_NAME.to_string(),
                            digest: Some(directory2_digest.into()),
                        },
                    ],
                    ..Default::default()
                };
                pinned_cas
                    .update_oneshot(root_directory_digest, root_directory.encode_to_vec().into())
                    .await?;
            }
            root_directory_digest
        };

        let digests = vec![root_directory_digest];
        let mut results = vec![None; digests.len()];

        let res = pinned_store.has_with_results(&digests, &mut results).await;
        assert!(res.is_ok(), "Expected has_with_results to succeed");

        Ok(())
    }

    #[tokio::test]
    async fn verify_completeness_check_fails_with_missing_cas_item() -> Result<(), Error> {
        let ac_store = Arc::new(MemoryStore::new(&config::stores::MemoryStore::default()));
        let cas_store = Arc::new(MemoryStore::new(&config::stores::MemoryStore::default()));
        let store_owned = CompletenessCheckingStore::new(ac_store.clone(), cas_store.clone());
        let pinned_store: Pin<&dyn Store> = Pin::new(&store_owned);
        let pinned_cas: Pin<&dyn Store> = Pin::new(cas_store.as_ref());

        let file1_digest = DigestInfo::try_new(HASH1, FILE1_CONTENT.len())?;
        let file2_digest = DigestInfo::try_new(HASH2, FILE2_CONTENT.len())?;
        let file3_digest = DigestInfo::try_new(HASH3, FILE3_CONTENT.len())?;

        pinned_cas.update_oneshot(file1_digest, FILE1_CONTENT.into()).await?;
        pinned_cas.update_oneshot(file2_digest, FILE2_CONTENT.into()).await?;
        pinned_cas.update_oneshot(file3_digest, FILE3_CONTENT.into()).await?;

        // Create a directory and add files to it
        let root_directory_digest = {
            // Make and insert (into store) our digest info needed to create our directory & files.
            let directory1_digest = DigestInfo::new([1u8; 32], 32);
            {
                let file1_content_digest = DigestInfo::new([2u8; 32], 32);
                pinned_cas
                    .update_oneshot(file1_content_digest, FILE1_CONTENT.into())
                    .await?;
                let directory1 = Directory {
                    files: vec![FileNode {
                        name: FILE1_NAME.to_string(),
                        digest: Some(file1_content_digest.into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                };
                pinned_cas
                    .update_oneshot(directory1_digest, directory1.encode_to_vec().into())
                    .await?;
            }
            let directory2_digest = DigestInfo::new([3u8; 32], 32);
            {
                // Now upload an empty directory.
                pinned_cas
                    .update_oneshot(directory2_digest, Directory::default().encode_to_vec().into())
                    .await?;
            }
            let directory3_digest = DigestInfo::new([1u8; 32], 32);
            {
                let file1_content_digest = DigestInfo::new([2u8; 32], 32);
                pinned_cas
                    .update_oneshot(file1_content_digest, FILE3_CONTENT.into())
                    .await?;
                let _directory3 = Directory {
                    files: vec![FileNode {
                        name: FILE3_NAME.to_string(),
                        digest: Some(file1_content_digest.into()),
                        ..Default::default()
                    }],
                    ..Default::default()
                };
            }
            let root_directory_digest = DigestInfo::new([5u8; 32], 32);
            {
                let _root_directory = Directory {
                    directories: vec![
                        DirectoryNode {
                            name: DIRECTORY1_NAME.to_string(),
                            digest: Some(directory1_digest.into()),
                        },
                        DirectoryNode {
                            name: DIRECTORY2_NAME.to_string(),
                            digest: Some(directory2_digest.into()),
                        },
                        DirectoryNode {
                            name: DIRECTORY3_NAME.to_string(),
                            digest: Some(directory3_digest.into()),
                        },
                    ],
                    ..Default::default()
                };
            }
            root_directory_digest
        };

        let digests = vec![root_directory_digest];
        let mut results = vec![None; digests.len()];

        let res = pinned_store.has_with_results(&digests, &mut results).await;
        assert!(res.is_err(), "Expected has_with_results to succeed");

        Ok(())
    }
}
