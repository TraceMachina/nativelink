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

use common::DigestInfo;
use completeness_checking_store::CompletenessCheckingStore;
use error::Error;
use memory_store::MemoryStore;
use prost::Message;
use proto::build::bazel::remote::execution::v2::{
    ActionResult as ProtoActionResult, Directory, DirectoryNode, ExecutedActionMetadata, FileNode, OutputDirectory,
    OutputFile,
};
use sha2::{Digest, Sha256};
use std::{
    pin::Pin,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use store::Store;

#[cfg(test)]
mod completeness_checking_store_tests {
    use super::*;

    const FILE1_NAME: &str = "file1.txt";

    const FILE1_CONTENT: &str = "HELLOFILE1";
    const FILE2_CONTENT: &str = "HELLOFILE2";
    const FILE3_CONTENT: &str = "HELLOFILE3";

    const DIRECTORY1_NAME: &str = "folder1";
    const DIRECTORY2_NAME: &str = "folder2";

    const HASH1: &str = "0125456789abcdef000000000000000000000000000000000123456789abcdef";
    const HASH2: &str = "0123f56789abcdef000000000000000000000000000000000123456789abcdef";
    const HASH3: &str = "0126456789abcdef000000000000000000000000000000000123456789abcdef";

    fn make_system_time(time: u64) -> SystemTime {
        UNIX_EPOCH.checked_add(Duration::from_secs(time)).unwrap()
    }

    #[tokio::test]
    async fn verify_completeness_check() -> Result<(), Error> {
        let ac_store = Arc::new(MemoryStore::new(&config::stores::MemoryStore::default()));
        let cas_store = Arc::new(MemoryStore::new(&config::stores::MemoryStore::default()));
        let store_owned = CompletenessCheckingStore::new(ac_store.clone(), cas_store.clone());
        let pinned_store: Pin<&dyn Store> = Pin::new(&store_owned);
        let pinned_cas: Pin<&dyn Store> = Pin::new(cas_store.as_ref());
        let pinned_ac: Pin<&dyn Store> = Pin::new(ac_store.as_ref());

        let test_file = OutputFile {
            path: "some path1".to_string(),
            digest: Some(DigestInfo::new([8u8; 32], 124).into()),
            is_executable: true,
            contents: Default::default(),
            node_properties: None,
        };

        let file1_digest = DigestInfo::try_new(HASH1, FILE1_CONTENT.len())?;
        let file2_digest = DigestInfo::try_new(HASH2, FILE2_CONTENT.len())?;
        let file3_digest = DigestInfo::try_new(HASH3, FILE3_CONTENT.len())?;

        pinned_cas.update_oneshot(file1_digest, FILE1_CONTENT.into()).await?;
        pinned_cas.update_oneshot(file2_digest, FILE2_CONTENT.into()).await?;
        pinned_cas.update_oneshot(file3_digest, FILE3_CONTENT.into()).await?;

        // Upload file to cas
        pinned_cas
            .update_oneshot(test_file.digest.unwrap().try_into().unwrap(), test_file.contents)
            .await?;

        // Create a directory and add files to it
        let root_directory_digest = {
            // Make and insert (into store) our digest info needed to create our directory & files.
            let directory1_digest = DigestInfo::new([1u8; 32], 32);
            {
                let file1_content_digest = DigestInfo::new([2u8; 32], 32);
                pinned_cas
                    .update_oneshot(file1_content_digest, FILE1_CONTENT.into())
                    .await?;
                pinned_ac
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
                pinned_ac
                    .update_oneshot(directory1_digest, directory1.encode_to_vec().into())
                    .await?;
            }
            let directory2_digest = DigestInfo::new([3u8; 32], 32);
            {
                // Now upload an empty directory.
                pinned_cas
                    .update_oneshot(directory2_digest, Directory::default().encode_to_vec().into())
                    .await?;
                pinned_ac
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
                pinned_ac
                    .update_oneshot(root_directory_digest, root_directory.encode_to_vec().into())
                    .await?;
            }
            root_directory_digest
        };

        let output_directory = OutputDirectory {
            path: "".to_string(), // set the path as needed
            tree_digest: Some(root_directory_digest.into()),
            ..Default::default()
        };

        let action_result = ProtoActionResult {
            output_files: vec![OutputFile {
                path: "some path1".to_string(),
                digest: Some(DigestInfo::new([8u8; 32], 124).into()),
                is_executable: true,
                contents: Default::default(), // We don't implement this.
                node_properties: None,
            }],
            output_file_symlinks: Default::default(),
            output_symlinks: Default::default(),
            output_directories: vec![output_directory],
            output_directory_symlinks: Default::default(), // Bazel deprecated this.
            exit_code: 5,
            stdout_raw: Default::default(), // We don't implement this.
            stdout_digest: Some(DigestInfo::new([10u8; 32], 124).into()),
            stderr_raw: Default::default(), // We don't implement this.
            stderr_digest: Some(DigestInfo::new([11u8; 32], 124).into()),
            execution_metadata: Some(ExecutedActionMetadata {
                worker: "".to_string(),
                queued_timestamp: Some(make_system_time(1).into()),
                worker_start_timestamp: Some(make_system_time(2).into()),
                worker_completed_timestamp: Some(make_system_time(3).into()),
                input_fetch_start_timestamp: Some(make_system_time(4).into()),
                input_fetch_completed_timestamp: Some(make_system_time(5).into()),
                execution_start_timestamp: Some(make_system_time(6).into()),
                execution_completed_timestamp: Some(make_system_time(7).into()),
                output_upload_start_timestamp: Some(make_system_time(8).into()),
                output_upload_completed_timestamp: Some(make_system_time(9).into()),
                virtual_execution_duration: Default::default(),
                auxiliary_metadata: vec![],
            }),
        };

        let mut bytes = vec![];
        action_result.encode(&mut bytes)?;

        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let result = hasher.finalize();

        let digest = DigestInfo::new(result.into(), bytes.len() as i64);

        let digests = vec![digest];

        pinned_ac.update_oneshot(digest, bytes.into()).await?;

        let res = pinned_store.has_many(&digests).await;
        println!("RES_SUCC: {:?}", res);
        assert!(res.is_ok(), "Expected has_many to succeed with items in cas.");

        Ok(())
    }

    #[tokio::test]
    async fn verify_completeness_check_fails_with_missing_cas_items() -> Result<(), Error> {
        let ac_store = Arc::new(MemoryStore::new(&config::stores::MemoryStore::default()));
        let cas_store = Arc::new(MemoryStore::new(&config::stores::MemoryStore::default()));
        let store_owned = CompletenessCheckingStore::new(ac_store.clone(), cas_store.clone());
        let pinned_store: Pin<&dyn Store> = Pin::new(&store_owned);
        //let pinned_cas: Pin<&dyn Store> = Pin::new(cas_store.as_ref());
        let pinned_ac: Pin<&dyn Store> = Pin::new(ac_store.as_ref());

        // let test_file = OutputFile {
        //     path: "some path1".to_string(),
        //     digest: Some(DigestInfo::new([8u8; 32], 124).into()),
        //     is_executable: true,
        //     contents: Default::default(), // We don't implement this.
        //     node_properties: None,
        // };

        // Create a directory and add files to it
        let root_directory_digest = {
            // Make and insert (into store) our digest info needed to create our directory & files.
            let directory1_digest = DigestInfo::new([1u8; 32], 32);
            {
                let file1_content_digest = DigestInfo::new([2u8; 32], 32);
                pinned_ac
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
                pinned_ac
                    .update_oneshot(directory1_digest, directory1.encode_to_vec().into())
                    .await?;
            }
            let directory2_digest = DigestInfo::new([3u8; 32], 32);
            {
                // Now upload an empty directory.
                pinned_ac
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
                pinned_ac
                    .update_oneshot(root_directory_digest, root_directory.encode_to_vec().into())
                    .await?;
            }
            root_directory_digest
        };

        let output_directory = OutputDirectory {
            path: "".to_string(), // set the path as needed
            tree_digest: Some(root_directory_digest.into()),
            ..Default::default()
        };

        let action_result = ProtoActionResult {
            output_files: vec![OutputFile {
                path: "some path1".to_string(),
                digest: Some(DigestInfo::new([8u8; 32], 124).into()),
                is_executable: true,
                contents: Default::default(), // We don't implement this.
                node_properties: None,
            }],
            output_file_symlinks: Default::default(),
            output_symlinks: Default::default(),
            output_directories: vec![output_directory],
            output_directory_symlinks: Default::default(), // Bazel deprecated this.
            exit_code: 5,
            stdout_raw: Default::default(), // We don't implement this.
            stdout_digest: Some(DigestInfo::new([10u8; 32], 124).into()),
            stderr_raw: Default::default(), // We don't implement this.
            stderr_digest: Some(DigestInfo::new([11u8; 32], 124).into()),
            execution_metadata: Some(ExecutedActionMetadata {
                worker: "".to_string(),
                queued_timestamp: Some(make_system_time(1).into()),
                worker_start_timestamp: Some(make_system_time(2).into()),
                worker_completed_timestamp: Some(make_system_time(3).into()),
                input_fetch_start_timestamp: Some(make_system_time(4).into()),
                input_fetch_completed_timestamp: Some(make_system_time(5).into()),
                execution_start_timestamp: Some(make_system_time(6).into()),
                execution_completed_timestamp: Some(make_system_time(7).into()),
                output_upload_start_timestamp: Some(make_system_time(8).into()),
                output_upload_completed_timestamp: Some(make_system_time(9).into()),
                virtual_execution_duration: Default::default(),
                auxiliary_metadata: vec![],
            }),
        };

        let mut bytes = vec![];
        action_result.encode(&mut bytes)?;

        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let result = hasher.finalize();

        let digest = DigestInfo::new(result.into(), bytes.len() as i64);

        let digests = vec![digest];

        pinned_ac.update_oneshot(digest, bytes.into()).await?;

        let res = pinned_store.has_many(&digests).await;
        println!("RES_FAIL: {:?}", res);
        assert!(res.is_err(), "Expected has_many to fail with files missing in cas.");

        Ok(())
    }
}
