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
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use error::Error;
use native_link_config::stores::MemoryStore as MemoryStoreConfig;
use native_link_store::completeness_checking_store::CompletenessCheckingStore;
use native_link_store::memory_store::MemoryStore;
use native_link_util::common::DigestInfo;
use native_link_util::store_trait::Store;
use prost::Message;
use proto::build::bazel::remote::execution::v2::{
    ActionResult as ProtoActionResult, Directory, DirectoryNode, ExecutedActionMetadata, FileNode, OutputDirectory,
    OutputFile, Tree,
};

#[cfg(test)]
mod completeness_checking_store_tests {
    use super::*;

    const FILE1_CONTENT: &str = "HELLOFILE1";
    const FILE2_CONTENT: &str = "HELLOFILE2";
    const FILE3_CONTENT: &str = "HELLOFILE3";

    const HASH1: &str = "0125456789abcdef000000000000000000000000000000000123456789abcdef";
    const HASH2: &str = "0123f56789abcdef000000000000000000000000000000000123456789abcdef";
    const HASH3: &str = "0126456789abcdef000000000000000000000000000000000123456789abcdef";

    fn make_system_time(time: u64) -> SystemTime {
        UNIX_EPOCH.checked_add(Duration::from_secs(time)).unwrap()
    }

    #[tokio::test]
    async fn verify_completeness_check_contains_no_nones() -> Result<(), Error> {
        let ac_store = Arc::new(MemoryStore::new(&MemoryStoreConfig::default()));
        let cas_store = Arc::new(MemoryStore::new(&MemoryStoreConfig::default()));
        let store_owned = CompletenessCheckingStore::new(ac_store.clone(), cas_store.clone());
        let pinned_store: Pin<&dyn Store> = Pin::new(&store_owned);
        let pinned_cas: Pin<&dyn Store> = Pin::new(cas_store.as_ref());
        let pinned_ac: Pin<&dyn Store> = Pin::new(ac_store.as_ref());

        let file1_digest = DigestInfo::try_new(HASH1, FILE1_CONTENT.len())?;
        let file2_digest = DigestInfo::try_new(HASH2, FILE2_CONTENT.len())?;
        let file3_digest = DigestInfo::try_new(HASH3, FILE3_CONTENT.len())?;
        let file4_digest = DigestInfo::try_new(HASH1, FILE3_CONTENT.len())?;
        let output_file_digest = DigestInfo::try_new(HASH2, FILE3_CONTENT.len())?;

        pinned_cas.update_oneshot(file1_digest, FILE1_CONTENT.into()).await?;
        pinned_cas.update_oneshot(file2_digest, FILE2_CONTENT.into()).await?;
        pinned_cas.update_oneshot(file3_digest, FILE3_CONTENT.into()).await?;
        pinned_cas.update_oneshot(file4_digest, FILE3_CONTENT.into()).await?;
        pinned_cas
            .update_oneshot(output_file_digest, FILE3_CONTENT.into())
            .await?;

        let tree = Tree {
            root: Some(Directory {
                files: vec![FileNode {
                    name: "bar".to_string(),
                    digest: Some(file1_digest.into()),
                    ..Default::default()
                }],
                directories: vec![DirectoryNode {
                    name: "foo".to_string(),
                    digest: Some(file2_digest.into()),
                }],
                ..Default::default()
            }),
            children: vec![Directory {
                files: vec![
                    FileNode {
                        name: "baz".to_string(),
                        digest: Some(file3_digest.into()),
                        is_executable: true,
                        ..Default::default()
                    },
                    FileNode {
                        name: "baz".to_string(),
                        digest: Some(file4_digest.into()),
                        is_executable: true,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
        };

        let tree_bytes = tree.encode_to_vec();
        let tree_digest = DigestInfo::new([6u8; 32], tree_bytes.len() as i64);

        pinned_ac.update_oneshot(tree_digest, tree_bytes.clone().into()).await?;

        let output_directory = OutputDirectory {
            path: "".to_string(), // set the path as needed
            tree_digest: Some(tree_digest.into()),
            ..Default::default()
        };

        let action_result = ProtoActionResult {
            output_files: vec![OutputFile {
                path: "some path1".to_string(),
                digest: Some(output_file_digest.into()),
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
            stderr_digest: Some(DigestInfo::new([11u8; 32], 123).into()),
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

        let dummy_hash = [0u8; 32]; // Using a dummy hash value

        let digest = DigestInfo::new(dummy_hash, bytes.len() as i64);

        let digests = vec![digest];

        pinned_ac.update_oneshot(digest, bytes.into()).await?;

        let res = pinned_store.has_many(&digests).await;
        println!("RES_SUCC: {:?}", res);
        assert!(res.is_ok(), "Expected has_many to succeed with items in cas.");

        Ok(())
    }

    #[tokio::test]
    async fn verify_completeness_check_associates_none_with_missing_cas_items() -> Result<(), Error> {
        let ac_store = Arc::new(MemoryStore::new(&MemoryStoreConfig::default()));
        let cas_store = Arc::new(MemoryStore::new(&MemoryStoreConfig::default()));
        let store_owned = CompletenessCheckingStore::new(ac_store.clone(), cas_store.clone());
        let pinned_store: Pin<&dyn Store> = Pin::new(&store_owned);
        let pinned_cas: Pin<&dyn Store> = Pin::new(cas_store.as_ref());
        let pinned_ac: Pin<&dyn Store> = Pin::new(ac_store.as_ref());

        let file1_digest = DigestInfo::try_new(HASH1, FILE1_CONTENT.len())?;
        let file2_digest = DigestInfo::try_new(HASH2, FILE2_CONTENT.len())?;
        let file3_digest = DigestInfo::try_new(HASH3, FILE3_CONTENT.len())?;
        let file4_digest = DigestInfo::try_new(HASH1, FILE3_CONTENT.len())?;
        let output_file_digest = DigestInfo::try_new(HASH2, FILE3_CONTENT.len())?;

        pinned_cas.update_oneshot(file1_digest, FILE1_CONTENT.into()).await?;
        pinned_cas.update_oneshot(file2_digest, FILE2_CONTENT.into()).await?;
        pinned_cas.update_oneshot(file4_digest, FILE3_CONTENT.into()).await?;
        pinned_cas
            .update_oneshot(output_file_digest, FILE3_CONTENT.into())
            .await?;

        let tree = Tree {
            root: Some(Directory {
                files: vec![FileNode {
                    name: "bar".to_string(),
                    digest: Some(file1_digest.into()),
                    ..Default::default()
                }],
                directories: vec![DirectoryNode {
                    name: "foo".to_string(),
                    digest: Some(file2_digest.into()),
                }],
                ..Default::default()
            }),
            children: vec![Directory {
                files: vec![
                    FileNode {
                        name: "baz".to_string(),
                        digest: Some(file3_digest.into()),
                        is_executable: true,
                        ..Default::default()
                    },
                    FileNode {
                        name: "baz".to_string(),
                        digest: Some(file4_digest.into()),
                        is_executable: true,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
        };

        let tree_alt = Tree {
            root: Some(Directory {
                files: vec![FileNode {
                    name: "foo".to_string(),
                    digest: Some(file2_digest.into()),
                    ..Default::default()
                }],
                directories: vec![DirectoryNode {
                    name: "bar".to_string(),
                    digest: Some(file1_digest.into()),
                }],
                ..Default::default()
            }),
            children: vec![Directory {
                files: vec![
                    FileNode {
                        name: "baz".to_string(),
                        digest: Some(file3_digest.into()),
                        is_executable: true,
                        ..Default::default()
                    },
                    FileNode {
                        name: "baz".to_string(),
                        digest: Some(file4_digest.into()),
                        is_executable: true,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
        };

        let tree_bytes = tree.encode_to_vec();
        let tree_digest = DigestInfo::new([6u8; 32], tree_bytes.len() as i64);

        let tree_alt_bytes = tree_alt.encode_to_vec();
        let tree_alt_digest = DigestInfo::new([6u8; 32], tree_alt_bytes.len() as i64);

        pinned_ac.update_oneshot(tree_digest, tree_bytes.clone().into()).await?;

        let output_directory = OutputDirectory {
            path: "".to_string(), // set the path as needed
            tree_digest: Some(tree_digest.into()),
            ..Default::default()
        };

        let output_directory_1 = OutputDirectory {
            path: "".to_string(), // set the path as needed
            tree_digest: Some(tree_alt_digest.into()),
            ..Default::default()
        };

        let action_result = ProtoActionResult {
            output_files: vec![OutputFile {
                path: "some path1".to_string(),
                digest: Some(output_file_digest.into()),
                is_executable: true,
                contents: Default::default(), // We don't implement this.
                node_properties: None,
            }],
            output_file_symlinks: Default::default(),
            output_symlinks: Default::default(),
            output_directories: vec![output_directory, output_directory_1],
            output_directory_symlinks: Default::default(), // Bazel deprecated this.
            exit_code: 5,
            stdout_raw: Default::default(), // We don't implement this.
            stdout_digest: Some(DigestInfo::new([10u8; 32], 124).into()),
            stderr_raw: Default::default(), // We don't implement this.
            stderr_digest: Some(DigestInfo::new([11u8; 32], 123).into()),
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

        let dummy_hash = [0u8; 32]; // Using a dummy hash value

        let digest = DigestInfo::new(dummy_hash, bytes.len() as i64);

        let digests = vec![digest];

        pinned_ac.update_oneshot(digest, bytes.into()).await?;

        let res = pinned_store.has_many(&digests).await;
        println!("RES_ERR: {:?}", res);
        assert!(
            res.is_err(),
            "Expected has_many to fail immediately once item missing from cas was checked."
        );

        Ok(())
    }
}
