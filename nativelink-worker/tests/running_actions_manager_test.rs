// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Functional Source License, Version 1.1, Apache 2.0 Future License (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    See LICENSE file for details
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use serial_test::serial;

#[serial]
mod tests {
    use core::str::from_utf8;
    use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
    #[cfg(target_family = "unix")]
    use core::task::Poll;
    use core::time::Duration;
    use std::collections::HashMap;
    use std::env;
    use std::ffi::OsString;
    use std::io::{Cursor, Write};
    #[cfg(target_family = "unix")]
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    use std::sync::{Arc, LazyLock, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use bytes::Bytes;
    use futures::prelude::*;
    use nativelink_config::cas_server::EnvironmentSource;
    use nativelink_config::stores::{
        FastSlowSpec, FilesystemSpec, MemorySpec, StoreDirection, StoreSpec,
    };
    use nativelink_error::{Code, Error, ResultExt, make_input_err};
    use nativelink_macro::nativelink_test;
    use nativelink_proto::build::bazel::remote::execution::v2::command::EnvironmentVariable;
    #[cfg_attr(target_family = "windows", allow(unused_imports))]
    use nativelink_proto::build::bazel::remote::execution::v2::{
        Action, ActionResult as ProtoActionResult, Command, Digest, Directory, DirectoryNode,
        ExecuteRequest, ExecuteResponse, FileNode, NodeProperties, Platform, SymlinkNode, Tree,
        digest_function::Value as ProtoDigestFunction, platform::Property,
    };
    use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
        HistoricalExecuteResponse, PeerHint, StartExecute,
    };
    use nativelink_proto::google::rpc::Status;
    use nativelink_store::ac_utils::{get_and_decode_digest, serialize_and_upload_message};
    use nativelink_store::fast_slow_store::FastSlowStore;
    use nativelink_store::filesystem_store::FilesystemStore;
    use nativelink_store::memory_store::MemoryStore;
    #[cfg(target_family = "unix")]
    use nativelink_util::action_messages::DirectoryInfo;
    #[cfg_attr(target_family = "windows", allow(unused_imports))]
    use nativelink_util::action_messages::SymlinkInfo;
    use nativelink_util::action_messages::{
        ActionResult, ExecutionMetadata, FileInfo, NameOrPath, OperationId,
    };
    use nativelink_util::blob_locality_map::new_shared_blob_locality_map;
    use nativelink_util::common::{DigestInfo, fs};
    use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
    use nativelink_util::store_trait::{Store, StoreLike};
    use nativelink_worker::running_actions_manager::{
        Callbacks, ExecutionConfiguration, RunningAction, RunningActionImpl, RunningActionsManager,
        RunningActionsManagerArgs, RunningActionsManagerImpl, download_to_directory,
    };
    use pretty_assertions::assert_eq;
    use prost::Message;
    use rand::Rng;
    use tokio::sync::oneshot;

    const DEFAULT_MAX_UPLOAD_TIMEOUT: u64 = 600;

    /// Get temporary path from either `TEST_TMPDIR` or best effort temp directory if
    /// not set.
    fn make_temp_path(data: &str) -> String {
        #[cfg(target_family = "unix")]
        return format!(
            "{}/{}/{}",
            env::var("TEST_TMPDIR")
                .unwrap_or_else(|_| env::temp_dir().to_str().unwrap().to_string()),
            rand::rng().random::<u64>(),
            data
        );
        #[cfg(target_family = "windows")]
        return format!(
            "{}\\{}\\{}",
            env::var("TEST_TMPDIR")
                .unwrap_or_else(|_| env::temp_dir().to_str().unwrap().to_string()),
            rand::rng().random::<u64>(),
            data
        );
    }

    async fn setup_stores() -> Result<
        (
            Arc<FilesystemStore>,
            Arc<MemoryStore>,
            Arc<FastSlowStore>,
            Arc<MemoryStore>,
        ),
        Error,
    > {
        let fast_config = FilesystemSpec {
            content_path: make_temp_path("content_path"),
            temp_path: make_temp_path("temp_path"),
            eviction_policy: None,
            ..Default::default()
        };
        let slow_config = MemorySpec::default();
        let fast_store = FilesystemStore::new(&fast_config).await?;
        let slow_store = MemoryStore::new(&slow_config);
        let ac_store = MemoryStore::new(&slow_config);
        let cas_store = FastSlowStore::new(
            &FastSlowSpec {
                fast: StoreSpec::Filesystem(fast_config),
                slow: StoreSpec::Memory(slow_config),
                fast_direction: StoreDirection::default(),
                slow_direction: StoreDirection::default(),
            },
            Store::new(fast_store.clone()),
            Store::new(slow_store.clone()),
        );
        Ok((fast_store, slow_store, cas_store, ac_store))
    }

    async fn run_action(action: Arc<RunningActionImpl>) -> Result<ActionResult, Error> {
        action
            .clone()
            .prepare_action()
            .and_then(RunningAction::execute)
            .and_then(RunningAction::upload_results)
            .and_then(RunningAction::get_finished_result)
            .then(|result| async move {
                action.cleanup().await?;
                result
            })
            .await
    }

    const NOW_TIME: u64 = 10000;

    fn make_system_time(add_time: u64) -> SystemTime {
        UNIX_EPOCH
            .checked_add(Duration::from_secs(NOW_TIME + add_time))
            .unwrap()
    }

    fn monotonic_clock(counter: &AtomicU64) -> SystemTime {
        let count = counter.fetch_add(1, Ordering::Relaxed);
        make_system_time(count)
    }

    fn increment_clock(time: &mut SystemTime) -> SystemTime {
        let previous_time = *time;
        *time = previous_time.checked_add(Duration::from_secs(1)).unwrap();
        previous_time
    }

    #[nativelink_test]
    async fn download_to_directory_file_download_test() -> Result<(), Box<dyn core::error::Error>> {
        const FILE1_NAME: &str = "file1.txt";
        const FILE1_CONTENT: &str = "HELLOFILE1";
        const FILE2_NAME: &str = "file2.exec";
        const FILE2_CONTENT: &str = "HELLOFILE2";
        const FILE2_MODE: u32 = 0o710;
        const FILE2_MTIME: u64 = 5;

        let (fast_store, slow_store, cas_store, _ac_store) = setup_stores().await?;

        let root_directory_digest = {
            // Make and insert (into store) our digest info needed to create our directory & files.
            let file1_content_digest = DigestInfo::new([2u8; 32], 32);
            slow_store
                .as_ref()
                .update_oneshot(file1_content_digest, FILE1_CONTENT.into())
                .await?;
            let file2_content_digest = DigestInfo::new([3u8; 32], 32);
            slow_store
                .as_ref()
                .update_oneshot(file2_content_digest, FILE2_CONTENT.into())
                .await?;

            let root_directory_digest = DigestInfo::new([1u8; 32], 32);
            let root_directory = Directory {
                files: vec![
                    FileNode {
                        name: FILE1_NAME.to_string(),
                        digest: Some(file1_content_digest.into()),
                        is_executable: false,
                        node_properties: None,
                    },
                    FileNode {
                        name: FILE2_NAME.to_string(),
                        digest: Some(file2_content_digest.into()),
                        is_executable: true,
                        node_properties: Some(NodeProperties {
                            properties: vec![],
                            mtime: Some(
                                SystemTime::UNIX_EPOCH
                                    .checked_add(Duration::from_secs(FILE2_MTIME))
                                    .unwrap()
                                    .into(),
                            ),
                            unix_mode: Some(FILE2_MODE),
                        }),
                    },
                ],
                ..Default::default()
            };

            slow_store
                .as_ref()
                .update_oneshot(root_directory_digest, root_directory.encode_to_vec().into())
                .await?;
            root_directory_digest
        };

        let download_dir = {
            // Tell it to download the digest info to a directory.
            let download_dir = make_temp_path("download_dir");
            fs::create_dir_all(&download_dir)
                .await
                .err_tip(|| format!("Could not make download_dir : {download_dir}"))?;
            download_to_directory(
                cas_store.as_ref(),
                fast_store.as_pin(),
                &root_directory_digest,
                &download_dir,
                None,
            )
            .await?;
            download_dir
        };
        {
            // Now ensure that our download_dir has the files.
            let file1_content = fs::read(format!("{download_dir}/{FILE1_NAME}")).await?;
            assert_eq!(from_utf8(&file1_content)?, FILE1_CONTENT);

            let file2_path = format!("{download_dir}/{FILE2_NAME}");
            let file2_content = fs::read(&file2_path).await?;
            assert_eq!(from_utf8(&file2_content)?, FILE2_CONTENT);

            let file2_metadata = fs::metadata(&file2_path).await?;
            // Note: We sent 0o710, but because is_executable was set it turns into 0o711.
            #[cfg(target_family = "unix")]
            assert_eq!(file2_metadata.mode() & 0o777, FILE2_MODE | 0o111);
            assert_eq!(
                file2_metadata
                    .modified()?
                    .duration_since(SystemTime::UNIX_EPOCH)?
                    .as_secs(),
                FILE2_MTIME
            );
        }
        Ok(())
    }

    #[nativelink_test]
    async fn download_to_directory_folder_download_test() -> Result<(), Box<dyn core::error::Error>>
    {
        const DIRECTORY1_NAME: &str = "folder1";
        const FILE1_NAME: &str = "file1.txt";
        const FILE1_CONTENT: &str = "HELLOFILE1";
        const DIRECTORY2_NAME: &str = "folder2";

        let (fast_store, slow_store, cas_store, _ac_store) = setup_stores().await?;

        let root_directory_digest = {
            // Make and insert (into store) our digest info needed to create our directory & files.
            let directory1_digest = DigestInfo::new([1u8; 32], 32);
            {
                let file1_content_digest = DigestInfo::new([2u8; 32], 32);
                slow_store
                    .as_ref()
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
                slow_store
                    .as_ref()
                    .update_oneshot(directory1_digest, directory1.encode_to_vec().into())
                    .await?;
            }
            let directory2_digest = DigestInfo::new([3u8; 32], 32);
            {
                // Now upload an empty directory.
                slow_store
                    .as_ref()
                    .update_oneshot(
                        directory2_digest,
                        Directory::default().encode_to_vec().into(),
                    )
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
                slow_store
                    .as_ref()
                    .update_oneshot(root_directory_digest, root_directory.encode_to_vec().into())
                    .await?;
            }
            root_directory_digest
        };

        let download_dir = {
            // Tell it to download the digest info to a directory.
            let download_dir = make_temp_path("download_dir");
            fs::create_dir_all(&download_dir)
                .await
                .err_tip(|| format!("Could not make download_dir : {download_dir}"))?;
            download_to_directory(
                cas_store.as_ref(),
                fast_store.as_pin(),
                &root_directory_digest,
                &download_dir,
                None,
            )
            .await?;
            download_dir
        };
        {
            // Now ensure that our download_dir has the files.
            let file1_content = fs::read(format!("{download_dir}/{DIRECTORY1_NAME}/{FILE1_NAME}"))
                .await
                .err_tip(|| "On file_1 read")?;
            assert_eq!(from_utf8(&file1_content)?, FILE1_CONTENT);

            let folder2_path = format!("{download_dir}/{DIRECTORY2_NAME}");
            let folder2_metadata = fs::metadata(&folder2_path)
                .await
                .err_tip(|| "On folder2_metadata metadata")?;
            assert_eq!(folder2_metadata.is_dir(), true);
        }
        Ok(())
    }

    // Windows does not support symlinks.
    #[cfg(not(target_family = "windows"))]
    #[nativelink_test]
    async fn download_to_directory_symlink_download_test() -> Result<(), Box<dyn core::error::Error>>
    {
        const FILE_NAME: &str = "file.txt";
        const FILE_CONTENT: &str = "HELLOFILE";
        const SYMLINK_NAME: &str = "symlink_file.txt";
        const SYMLINK_TARGET: &str = "file.txt";

        let (fast_store, slow_store, cas_store, _ac_store) = setup_stores().await?;

        let root_directory_digest = {
            // Make and insert (into store) our digest info needed to create our directory & files.
            let file_content_digest = DigestInfo::new([1u8; 32], 32);
            slow_store
                .as_ref()
                .update_oneshot(file_content_digest, FILE_CONTENT.into())
                .await?;

            let root_directory_digest = DigestInfo::new([2u8; 32], 32);
            let root_directory = Directory {
                files: vec![FileNode {
                    name: FILE_NAME.to_string(),
                    digest: Some(file_content_digest.into()),
                    is_executable: false,
                    node_properties: None,
                }],
                symlinks: vec![SymlinkNode {
                    name: SYMLINK_NAME.to_string(),
                    target: SYMLINK_TARGET.to_string(),
                    node_properties: None,
                }],
                ..Default::default()
            };

            slow_store
                .as_ref()
                .update_oneshot(root_directory_digest, root_directory.encode_to_vec().into())
                .await?;
            root_directory_digest
        };

        let download_dir = {
            // Tell it to download the digest info to a directory.
            let download_dir = make_temp_path("download_dir");
            fs::create_dir_all(&download_dir)
                .await
                .err_tip(|| format!("Could not make download_dir : {download_dir}"))?;
            download_to_directory(
                cas_store.as_ref(),
                fast_store.as_pin(),
                &root_directory_digest,
                &download_dir,
                None,
            )
            .await?;
            download_dir
        };
        {
            // Now ensure that our download_dir has the files.
            let symlink_path = format!("{download_dir}/{SYMLINK_NAME}");
            let symlink_content = fs::read(&symlink_path)
                .await
                .err_tip(|| "On symlink read")?;
            assert_eq!(from_utf8(&symlink_content)?, FILE_CONTENT);

            let symlink_metadata = fs::symlink_metadata(&symlink_path)
                .await
                .err_tip(|| "On symlink symlink_metadata")?;
            assert_eq!(symlink_metadata.is_symlink(), true);
        }
        Ok(())
    }

    #[nativelink_test]
    async fn download_to_directory_batch_existence_check_test(
    ) -> Result<(), Box<dyn core::error::Error>> {
        // Verifies that files already in the fast store are hardlinked
        // without being re-fetched from the slow store.
        const FILE1_NAME: &str = "cached_file.txt";
        const FILE1_CONTENT: &str = "ALREADY_IN_FAST";
        const FILE2_NAME: &str = "uncached_file.txt";
        const FILE2_CONTENT: &str = "ONLY_IN_SLOW";

        let (fast_store, slow_store, cas_store, _ac_store) = setup_stores().await?;

        let root_directory_digest = {
            let file1_content_digest = DigestInfo::new([10u8; 32], FILE1_CONTENT.len() as u64);
            let file2_content_digest = DigestInfo::new([11u8; 32], FILE2_CONTENT.len() as u64);

            // Put file1 in BOTH slow and fast store (simulates a cached blob).
            slow_store
                .as_ref()
                .update_oneshot(file1_content_digest, FILE1_CONTENT.into())
                .await?;
            fast_store
                .as_ref()
                .update_oneshot(file1_content_digest, FILE1_CONTENT.into())
                .await?;

            // Put file2 ONLY in slow store (simulates a cache miss).
            slow_store
                .as_ref()
                .update_oneshot(file2_content_digest, FILE2_CONTENT.into())
                .await?;

            let root_directory_digest = DigestInfo::new([12u8; 32], 32);
            let root_directory = Directory {
                files: vec![
                    FileNode {
                        name: FILE1_NAME.to_string(),
                        digest: Some(file1_content_digest.into()),
                        ..Default::default()
                    },
                    FileNode {
                        name: FILE2_NAME.to_string(),
                        digest: Some(file2_content_digest.into()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };

            slow_store
                .as_ref()
                .update_oneshot(root_directory_digest, root_directory.encode_to_vec().into())
                .await?;
            root_directory_digest
        };

        let download_dir = make_temp_path("download_dir_batch_check");
        fs::create_dir_all(&download_dir).await?;
        download_to_directory(
            cas_store.as_ref(),
            fast_store.as_pin(),
            &root_directory_digest,
            &download_dir,
            None,
        )
        .await?;

        // Both files should be present with correct content.
        let file1_content = fs::read(format!("{download_dir}/{FILE1_NAME}")).await?;
        assert_eq!(from_utf8(&file1_content)?, FILE1_CONTENT);

        let file2_content = fs::read(format!("{download_dir}/{FILE2_NAME}")).await?;
        assert_eq!(from_utf8(&file2_content)?, FILE2_CONTENT);

        Ok(())
    }

    #[nativelink_test]
    async fn download_to_directory_dedup_digests_test(
    ) -> Result<(), Box<dyn core::error::Error>> {
        // Verifies that multiple files sharing the same digest content
        // are all materialized correctly (the digest is only downloaded once
        // but hardlinked to multiple destinations).
        const SHARED_CONTENT: &str = "SHARED_CONTENT_DATA";
        const FILE_A_NAME: &str = "file_a.txt";
        const FILE_B_NAME: &str = "file_b.txt";
        const FILE_C_NAME: &str = "file_c.txt";

        let (fast_store, slow_store, cas_store, _ac_store) = setup_stores().await?;

        let root_directory_digest = {
            let shared_digest = DigestInfo::new([20u8; 32], SHARED_CONTENT.len() as u64);
            slow_store
                .as_ref()
                .update_oneshot(shared_digest, SHARED_CONTENT.into())
                .await?;

            let root_directory_digest = DigestInfo::new([21u8; 32], 32);
            let root_directory = Directory {
                files: vec![
                    FileNode {
                        name: FILE_A_NAME.to_string(),
                        digest: Some(shared_digest.into()),
                        ..Default::default()
                    },
                    FileNode {
                        name: FILE_B_NAME.to_string(),
                        digest: Some(shared_digest.into()),
                        ..Default::default()
                    },
                    FileNode {
                        name: FILE_C_NAME.to_string(),
                        digest: Some(shared_digest.into()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };

            slow_store
                .as_ref()
                .update_oneshot(root_directory_digest, root_directory.encode_to_vec().into())
                .await?;
            root_directory_digest
        };

        let download_dir = make_temp_path("download_dir_dedup");
        fs::create_dir_all(&download_dir).await?;
        download_to_directory(
            cas_store.as_ref(),
            fast_store.as_pin(),
            &root_directory_digest,
            &download_dir,
            None,
        )
        .await?;

        // All three files should exist with the same content.
        for name in &[FILE_A_NAME, FILE_B_NAME, FILE_C_NAME] {
            let content = fs::read(format!("{download_dir}/{name}")).await?;
            assert_eq!(from_utf8(&content)?, SHARED_CONTENT, "Mismatch for {name}");
        }

        Ok(())
    }

    #[nativelink_test]
    async fn download_to_directory_deep_nested_tree_test(
    ) -> Result<(), Box<dyn core::error::Error>> {
        // Verifies that deeply nested directory trees (3 levels) are resolved
        // correctly via the recursive fallback path (MemoryStore).
        const LEAF_FILE_NAME: &str = "leaf.txt";
        const LEAF_CONTENT: &str = "DEEP_LEAF_DATA";

        let (fast_store, slow_store, cas_store, _ac_store) = setup_stores().await?;

        let root_directory_digest = {
            let leaf_content_digest = DigestInfo::new([30u8; 32], LEAF_CONTENT.len() as u64);
            slow_store
                .as_ref()
                .update_oneshot(leaf_content_digest, LEAF_CONTENT.into())
                .await?;

            // Level 3 (deepest): directory containing a file
            let level3_digest = DigestInfo::new([31u8; 32], 32);
            let level3_dir = Directory {
                files: vec![FileNode {
                    name: LEAF_FILE_NAME.to_string(),
                    digest: Some(leaf_content_digest.into()),
                    ..Default::default()
                }],
                ..Default::default()
            };
            slow_store
                .as_ref()
                .update_oneshot(level3_digest, level3_dir.encode_to_vec().into())
                .await?;

            // Level 2: directory containing level3
            let level2_digest = DigestInfo::new([32u8; 32], 32);
            let level2_dir = Directory {
                directories: vec![DirectoryNode {
                    name: "level3".to_string(),
                    digest: Some(level3_digest.into()),
                }],
                ..Default::default()
            };
            slow_store
                .as_ref()
                .update_oneshot(level2_digest, level2_dir.encode_to_vec().into())
                .await?;

            // Level 1 (root): directory containing level2
            let root_digest = DigestInfo::new([33u8; 32], 32);
            let root_dir = Directory {
                directories: vec![DirectoryNode {
                    name: "level2".to_string(),
                    digest: Some(level2_digest.into()),
                }],
                ..Default::default()
            };
            slow_store
                .as_ref()
                .update_oneshot(root_digest, root_dir.encode_to_vec().into())
                .await?;
            root_digest
        };

        let download_dir = make_temp_path("download_dir_deep");
        fs::create_dir_all(&download_dir).await?;
        download_to_directory(
            cas_store.as_ref(),
            fast_store.as_pin(),
            &root_directory_digest,
            &download_dir,
            None,
        )
        .await?;

        // Verify the deeply nested file exists with correct content.
        let leaf_path = format!("{download_dir}/level2/level3/{LEAF_FILE_NAME}");
        let leaf_content = fs::read(&leaf_path).await?;
        assert_eq!(from_utf8(&leaf_content)?, LEAF_CONTENT);

        // Verify intermediate directories exist.
        let level2_meta = fs::metadata(format!("{download_dir}/level2")).await?;
        assert!(level2_meta.is_dir());
        let level3_meta = fs::metadata(format!("{download_dir}/level2/level3")).await?;
        assert!(level3_meta.is_dir());

        Ok(())
    }

    #[nativelink_test]
    async fn download_to_directory_empty_directory_test(
    ) -> Result<(), Box<dyn core::error::Error>> {
        // Verifies that an empty root directory is handled correctly.
        let (fast_store, slow_store, cas_store, _ac_store) = setup_stores().await?;

        let root_directory_digest = {
            let root_digest = DigestInfo::new([40u8; 32], 32);
            let root_dir = Directory::default();
            slow_store
                .as_ref()
                .update_oneshot(root_digest, root_dir.encode_to_vec().into())
                .await?;
            root_digest
        };

        let download_dir = make_temp_path("download_dir_empty");
        fs::create_dir_all(&download_dir).await?;
        download_to_directory(
            cas_store.as_ref(),
            fast_store.as_pin(),
            &root_directory_digest,
            &download_dir,
            None,
        )
        .await?;

        // Directory should exist and be empty.
        let meta = fs::metadata(&download_dir).await?;
        assert!(meta.is_dir());

        Ok(())
    }

    #[nativelink_test]
    async fn download_to_directory_many_files_test(
    ) -> Result<(), Box<dyn core::error::Error>> {
        // Verifies that a directory with many files (simulating a real build
        // with many inputs) is handled correctly by the batch existence check
        // and parallel download paths.
        const FILE_COUNT: usize = 50;

        let (fast_store, slow_store, cas_store, _ac_store) = setup_stores().await?;

        let root_directory_digest = {
            let mut file_nodes = Vec::with_capacity(FILE_COUNT);
            for i in 0..FILE_COUNT {
                let content = format!("content_of_file_{i}");
                // Create unique digests using the index.
                let mut hash = [0u8; 32];
                hash[0] = 50;
                hash[1] = (i >> 8) as u8;
                hash[2] = (i & 0xff) as u8;
                let digest = DigestInfo::new(hash, content.len() as u64);

                slow_store
                    .as_ref()
                    .update_oneshot(digest, content.into())
                    .await?;

                // Pre-populate every 3rd file in the fast store to test
                // the mixed cached/uncached path.
                if i % 3 == 0 {
                    let content_again = format!("content_of_file_{i}");
                    fast_store
                        .as_ref()
                        .update_oneshot(digest, content_again.into())
                        .await?;
                }

                file_nodes.push(FileNode {
                    name: format!("file_{i:04}.txt"),
                    digest: Some(digest.into()),
                    ..Default::default()
                });
            }

            let root_digest = DigestInfo::new([51u8; 32], 32);
            let root_dir = Directory {
                files: file_nodes,
                ..Default::default()
            };
            slow_store
                .as_ref()
                .update_oneshot(root_digest, root_dir.encode_to_vec().into())
                .await?;
            root_digest
        };

        let download_dir = make_temp_path("download_dir_many");
        fs::create_dir_all(&download_dir).await?;
        download_to_directory(
            cas_store.as_ref(),
            fast_store.as_pin(),
            &root_directory_digest,
            &download_dir,
            None,
        )
        .await?;

        // Verify all files.
        for i in 0..FILE_COUNT {
            let expected = format!("content_of_file_{i}");
            let path = format!("{download_dir}/file_{i:04}.txt");
            let content = fs::read(&path).await?;
            assert_eq!(
                from_utf8(&content)?,
                expected,
                "Content mismatch for file {i}"
            );
        }

        Ok(())
    }

    #[nativelink_test]
    async fn download_to_directory_missing_blob_returns_error_test(
    ) -> Result<(), Box<dyn core::error::Error>> {
        // Verifies that a reference to a missing blob in the slow store
        // propagates an error (not silently ignored).
        const FILE_NAME: &str = "missing.txt";

        let (fast_store, slow_store, cas_store, _ac_store) = setup_stores().await?;

        let root_directory_digest = {
            // Reference a file content digest that does NOT exist in any store.
            let missing_content_digest = DigestInfo::new([60u8; 32], 100);

            let root_digest = DigestInfo::new([61u8; 32], 32);
            let root_directory = Directory {
                files: vec![FileNode {
                    name: FILE_NAME.to_string(),
                    digest: Some(missing_content_digest.into()),
                    ..Default::default()
                }],
                ..Default::default()
            };

            slow_store
                .as_ref()
                .update_oneshot(root_digest, root_directory.encode_to_vec().into())
                .await?;
            root_digest
        };

        let download_dir = make_temp_path("download_dir_missing_blob");
        fs::create_dir_all(&download_dir).await?;
        let result = download_to_directory(
            cas_store.as_ref(),
            fast_store.as_pin(),
            &root_directory_digest,
            &download_dir,
            None,
        )
        .await;

        assert!(result.is_err(), "Expected error for missing blob");
        Ok(())
    }

    #[nativelink_test]
    async fn download_to_directory_missing_directory_digest_returns_error_test(
    ) -> Result<(), Box<dyn core::error::Error>> {
        // Verifies that a DirectoryNode referencing a non-existent directory
        // digest propagates an error during tree resolution.
        let (fast_store, slow_store, cas_store, _ac_store) = setup_stores().await?;

        let root_directory_digest = {
            // Reference a child directory digest that does NOT exist.
            let missing_child_digest = DigestInfo::new([70u8; 32], 32);

            let root_digest = DigestInfo::new([71u8; 32], 32);
            let root_directory = Directory {
                directories: vec![DirectoryNode {
                    name: "missing_dir".to_string(),
                    digest: Some(missing_child_digest.into()),
                }],
                ..Default::default()
            };

            slow_store
                .as_ref()
                .update_oneshot(root_digest, root_directory.encode_to_vec().into())
                .await?;
            root_digest
        };

        let download_dir = make_temp_path("download_dir_missing_dir");
        fs::create_dir_all(&download_dir).await?;
        let result = download_to_directory(
            cas_store.as_ref(),
            fast_store.as_pin(),
            &root_directory_digest,
            &download_dir,
            None,
        )
        .await;

        assert!(result.is_err(), "Expected error for missing directory digest");
        Ok(())
    }

    #[nativelink_test]
    async fn download_to_directory_zero_digest_file_test(
    ) -> Result<(), Box<dyn core::error::Error>> {
        // Verifies that zero-digest (empty) files are created correctly.
        // Zero-digest files have special handling and skip batch existence checks.
        const EMPTY_FILE_NAME: &str = "empty.txt";
        const NORMAL_FILE_NAME: &str = "normal.txt";
        const NORMAL_CONTENT: &str = "NORMAL_DATA";

        // SHA-256 of zero bytes.
        const ZERO_HASH: [u8; 32] = [
            0xe3, 0xb0, 0xc4, 0x42, 0x98, 0xfc, 0x1c, 0x14, 0x9a, 0xfb, 0xf4, 0xc8, 0x99, 0x6f,
            0xb9, 0x24, 0x27, 0xae, 0x41, 0xe4, 0x64, 0x9b, 0x93, 0x4c, 0xa4, 0x95, 0x99, 0x1b,
            0x78, 0x52, 0xb8, 0x55,
        ];

        let (fast_store, slow_store, cas_store, _ac_store) = setup_stores().await?;

        let root_directory_digest = {
            let zero_digest = DigestInfo::new(ZERO_HASH, 0);
            let normal_digest = DigestInfo::new([80u8; 32], NORMAL_CONTENT.len() as u64);
            slow_store
                .as_ref()
                .update_oneshot(normal_digest, NORMAL_CONTENT.into())
                .await?;

            let root_digest = DigestInfo::new([81u8; 32], 32);
            let root_directory = Directory {
                files: vec![
                    FileNode {
                        name: EMPTY_FILE_NAME.to_string(),
                        digest: Some(zero_digest.into()),
                        ..Default::default()
                    },
                    FileNode {
                        name: NORMAL_FILE_NAME.to_string(),
                        digest: Some(normal_digest.into()),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            };

            slow_store
                .as_ref()
                .update_oneshot(root_digest, root_directory.encode_to_vec().into())
                .await?;
            root_digest
        };

        let download_dir = make_temp_path("download_dir_zero");
        fs::create_dir_all(&download_dir).await?;
        download_to_directory(
            cas_store.as_ref(),
            fast_store.as_pin(),
            &root_directory_digest,
            &download_dir,
            None,
        )
        .await?;

        // Zero-digest file should exist and be empty.
        let empty_path = format!("{download_dir}/{EMPTY_FILE_NAME}");
        let empty_content = fs::read(&empty_path).await?;
        assert_eq!(empty_content.len(), 0, "Zero-digest file should be empty");

        // Normal file should also exist.
        let normal_content = fs::read(format!("{download_dir}/{NORMAL_FILE_NAME}")).await?;
        assert_eq!(from_utf8(&normal_content)?, NORMAL_CONTENT);

        Ok(())
    }

    #[nativelink_test]
    async fn ensure_output_files_full_directories_are_created_no_working_directory_test()
    -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);
        {
            let command = Command {
                arguments: vec!["touch".to_string(), "./some/path/test.txt".to_string()],
                output_files: vec!["some/path/test.txt".to_string()],
                environment_variables: vec![EnvironmentVariable {
                    name: "PATH".to_string(),
                    value: env::var("PATH").unwrap(),
                }],
                ..Default::default()
            };
            let command_digest = serialize_and_upload_message(
                &command,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let input_root_digest = serialize_and_upload_message(
                &Directory {
                    directories: vec![DirectoryNode {
                        name: "some_cwd".to_string(),
                        digest: Some(
                            serialize_and_upload_message(
                                &Directory::default(),
                                cas_store.as_pin(),
                                &mut DigestHasherFunc::Sha256.hasher(),
                            )
                            .await?
                            .into(),
                        ),
                    }],
                    ..Default::default()
                },
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(
                &action,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;

            let execute_request = ExecuteRequest {
                action_digest: Some(action_digest.into()),
                ..Default::default()
            };
            let operation_id = OperationId::default().to_string();

            let running_action = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(execute_request),
                        operation_id,
                        queued_timestamp: None,
                        platform: action.platform.clone(),
                        worker_id: WORKER_ID.to_string(),
                        peer_hints: Vec::new(),
                        resolved_directories: Vec::new(),
                        resolved_directory_digests: Vec::new(),
                    },
                )
                .await?;

            let running_action = running_action.clone().prepare_action().await?;

            // The folder should have been created for our output file.
            assert_eq!(
                fs::metadata(format!(
                    "{}/{}",
                    running_action.get_work_directory(),
                    "some/path"
                ))
                .await
                .is_ok(),
                true,
                "Expected path to exist"
            );

            running_action.cleanup().await?;
        };
        Ok(())
    }

    #[nativelink_test]
    async fn ensure_output_files_full_directories_are_created_test()
    -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);
        {
            let working_directory = "some_cwd";
            let command = Command {
                arguments: vec!["touch".to_string(), "./some/path/test.txt".to_string()],
                output_files: vec!["some/path/test.txt".to_string()],
                working_directory: working_directory.to_string(),
                environment_variables: vec![EnvironmentVariable {
                    name: "PATH".to_string(),
                    value: env::var("PATH").unwrap(),
                }],
                ..Default::default()
            };
            let command_digest = serialize_and_upload_message(
                &command,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let input_root_digest = serialize_and_upload_message(
                &Directory {
                    directories: vec![DirectoryNode {
                        name: "some_cwd".to_string(),
                        digest: Some(
                            serialize_and_upload_message(
                                &Directory::default(),
                                cas_store.as_pin(),
                                &mut DigestHasherFunc::Sha256.hasher(),
                            )
                            .await?
                            .into(),
                        ),
                    }],
                    ..Default::default()
                },
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(
                &action,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;

            let execute_request = ExecuteRequest {
                action_digest: Some(action_digest.into()),
                ..Default::default()
            };
            let operation_id = OperationId::default().to_string();

            let running_action = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(execute_request),
                        operation_id,
                        queued_timestamp: None,
                        platform: action.platform.clone(),
                        worker_id: WORKER_ID.to_string(),
                        peer_hints: Vec::new(),
                        resolved_directories: Vec::new(),
                        resolved_directory_digests: Vec::new(),
                    },
                )
                .await?;

            let running_action = running_action.clone().prepare_action().await?;

            // The folder should have been created for our output file.
            assert_eq!(
                fs::metadata(format!(
                    "{}/{}/{}",
                    running_action.get_work_directory(),
                    working_directory,
                    "some/path"
                ))
                .await
                .is_ok(),
                true,
                "Expected path to exist"
            );

            running_action.cleanup().await?;
        };
        Ok(())
    }

    #[nativelink_test]
    async fn blake3_upload_files() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, _slow_store, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);
        let action_result = {
            #[cfg(target_family = "unix")]
            let arguments = vec![
                "sh".to_string(),
                "-c".to_string(),
                "printf '123 ' > ./test.txt; printf 'foo-stdout '; >&2 printf 'bar-stderr  '"
                    .to_string(),
            ];
            #[cfg(target_family = "windows")]
            let arguments = vec![
                "cmd".to_string(),
                "/C".to_string(),
                // Note: Windows adds two spaces after 'set /p=XXX'.
                "echo | set /p=123> ./test.txt & echo | set /p=foo-stdout & echo | set /p=bar-stderr 1>&2 & exit 0"
                    .to_string(),
            ];
            let working_directory = "some_cwd";
            let command = Command {
                arguments,
                output_paths: vec!["test.txt".to_string()],
                working_directory: working_directory.to_string(),
                environment_variables: vec![EnvironmentVariable {
                    name: "PATH".to_string(),
                    value: env::var("PATH").unwrap(),
                }],
                ..Default::default()
            };
            let command_digest = serialize_and_upload_message(
                &command,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Blake3.hasher(),
            )
            .await?;
            let input_root_digest = serialize_and_upload_message(
                &Directory {
                    directories: vec![DirectoryNode {
                        name: working_directory.to_string(),
                        digest: Some(
                            serialize_and_upload_message(
                                &Directory::default(),
                                cas_store.as_pin(),
                                &mut DigestHasherFunc::Blake3.hasher(),
                            )
                            .await?
                            .into(),
                        ),
                    }],
                    ..Default::default()
                },
                cas_store.as_pin(),
                &mut DigestHasherFunc::Blake3.hasher(),
            )
            .await?;
            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(
                &action,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Blake3.hasher(),
            )
            .await?;

            let execute_request = ExecuteRequest {
                action_digest: Some(action_digest.into()),
                digest_function: ProtoDigestFunction::Blake3.into(),
                ..Default::default()
            };
            let operation_id = OperationId::default().to_string();

            let running_action_impl = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(execute_request),
                        operation_id,
                        queued_timestamp: None,
                        platform: action.platform.clone(),
                        worker_id: WORKER_ID.to_string(),
                        peer_hints: Vec::new(),
                        resolved_directories: Vec::new(),
                        resolved_directory_digests: Vec::new(),
                    },
                )
                .await?;

            run_action(running_action_impl.clone()).await?
        };
        let file_content = cas_store
            .as_ref()
            .get_part_unchunked(action_result.output_files[0].digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&file_content)?, "123 ");
        let stdout_content = cas_store
            .as_ref()
            .get_part_unchunked(action_result.stdout_digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&stdout_content)?, "foo-stdout ");
        let stderr_content = cas_store
            .as_ref()
            .get_part_unchunked(action_result.stderr_digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&stderr_content)?, "bar-stderr  ");
        let mut clock_time = make_system_time(0);
        assert_eq!(
            action_result,
            ActionResult {
                output_files: vec![FileInfo {
                    name_or_path: NameOrPath::Path("test.txt".to_string()),
                    digest: DigestInfo::try_new(
                        "3f488ba478fc6716c756922c9f34ebd7e84b85c3e03e33e22e7a3736cafdc6d8",
                        4
                    )?,
                    is_executable: false,
                }],
                stdout_digest: DigestInfo::try_new(
                    "af1720193ae81515067a3ef39f0dfda3ad54a1a9d216e55d32fe5c1e178c6a7d",
                    11
                )?,
                stderr_digest: DigestInfo::try_new(
                    "65e0abbae32a3aedaf040b654c6f02ace03c7690c17a8415a90fc2ec9c809a16",
                    12
                )?,
                exit_code: 0,
                output_folders: vec![],
                output_file_symlinks: vec![],
                output_directory_symlinks: vec![],
                server_logs: HashMap::new(),
                execution_metadata: ExecutionMetadata {
                    worker: WORKER_ID.to_string(),
                    queued_timestamp: SystemTime::UNIX_EPOCH,
                    worker_start_timestamp: increment_clock(&mut clock_time),
                    input_fetch_start_timestamp: increment_clock(&mut clock_time),
                    input_fetch_completed_timestamp: increment_clock(&mut clock_time),
                    execution_start_timestamp: increment_clock(&mut clock_time),
                    execution_completed_timestamp: increment_clock(&mut clock_time),
                    output_upload_start_timestamp: increment_clock(&mut clock_time),
                    output_upload_completed_timestamp: increment_clock(&mut clock_time),
                    worker_completed_timestamp: increment_clock(&mut clock_time),
                },
                error: None,
                message: String::new(),
            }
        );
        Ok(())
    }

    #[nativelink_test]
    async fn upload_files_from_above_cwd_test() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, _slow_store, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);
        let action_result = {
            #[cfg(target_family = "unix")]
            let arguments = vec![
                "sh".to_string(),
                "-c".to_string(),
                "printf '123 ' > ./test.txt; printf 'foo-stdout '; >&2 printf 'bar-stderr  '"
                    .to_string(),
            ];
            #[cfg(target_family = "windows")]
            let arguments = vec![
                "cmd".to_string(),
                "/C".to_string(),
                // Note: Windows adds two spaces after 'set /p=XXX'.
                "echo | set /p=123> ./test.txt & echo | set /p=foo-stdout & echo | set /p=bar-stderr 1>&2 & exit 0"
                    .to_string(),
            ];
            let working_directory = "some_cwd";
            let command = Command {
                arguments,
                output_paths: vec!["test.txt".to_string()],
                working_directory: working_directory.to_string(),
                environment_variables: vec![EnvironmentVariable {
                    name: "PATH".to_string(),
                    value: env::var("PATH").unwrap(),
                }],
                ..Default::default()
            };
            let command_digest = serialize_and_upload_message(
                &command,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let input_root_digest = serialize_and_upload_message(
                &Directory {
                    directories: vec![DirectoryNode {
                        name: working_directory.to_string(),
                        digest: Some(
                            serialize_and_upload_message(
                                &Directory::default(),
                                cas_store.as_pin(),
                                &mut DigestHasherFunc::Sha256.hasher(),
                            )
                            .await?
                            .into(),
                        ),
                    }],
                    ..Default::default()
                },
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(
                &action,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;

            let execute_request = ExecuteRequest {
                action_digest: Some(action_digest.into()),
                ..Default::default()
            };
            let operation_id = OperationId::default().to_string();

            let running_action_impl = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(execute_request),
                        operation_id,
                        queued_timestamp: None,
                        platform: action.platform.clone(),
                        worker_id: WORKER_ID.to_string(),
                        peer_hints: Vec::new(),
                        resolved_directories: Vec::new(),
                        resolved_directory_digests: Vec::new(),
                    },
                )
                .await?;

            run_action(running_action_impl.clone()).await?
        };
        let file_content = cas_store
            .as_ref()
            .get_part_unchunked(action_result.output_files[0].digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&file_content)?, "123 ");
        let stdout_content = cas_store
            .as_ref()
            .get_part_unchunked(action_result.stdout_digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&stdout_content)?, "foo-stdout ");
        let stderr_content = cas_store
            .as_ref()
            .get_part_unchunked(action_result.stderr_digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&stderr_content)?, "bar-stderr  ");
        let mut clock_time = make_system_time(0);
        assert_eq!(
            action_result,
            ActionResult {
                output_files: vec![FileInfo {
                    name_or_path: NameOrPath::Path("test.txt".to_string()),
                    digest: DigestInfo::try_new(
                        "c69e10a5f54f4e28e33897fbd4f8701595443fa8c3004aeaa20dd4d9a463483b",
                        4
                    )?,
                    is_executable: false,
                }],
                stdout_digest: DigestInfo::try_new(
                    "15019a676f057d97d1ad3af86f3cc1e623cb33b18ff28422bbe3248d2471cc94",
                    11
                )?,
                stderr_digest: DigestInfo::try_new(
                    "2375ab8a01ca11e1ea7606dfb58756c153d49733cde1dbfb5a1e00f39afacf06",
                    12
                )?,
                exit_code: 0,
                output_folders: vec![],
                output_file_symlinks: vec![],
                output_directory_symlinks: vec![],
                server_logs: HashMap::new(),
                execution_metadata: ExecutionMetadata {
                    worker: WORKER_ID.to_string(),
                    queued_timestamp: SystemTime::UNIX_EPOCH,
                    worker_start_timestamp: increment_clock(&mut clock_time),
                    input_fetch_start_timestamp: increment_clock(&mut clock_time),
                    input_fetch_completed_timestamp: increment_clock(&mut clock_time),
                    execution_start_timestamp: increment_clock(&mut clock_time),
                    execution_completed_timestamp: increment_clock(&mut clock_time),
                    output_upload_start_timestamp: increment_clock(&mut clock_time),
                    output_upload_completed_timestamp: increment_clock(&mut clock_time),
                    worker_completed_timestamp: increment_clock(&mut clock_time),
                },
                error: None,
                message: String::new(),
            }
        );
        Ok(())
    }

    // Windows does not support symlinks.
    #[cfg(not(target_family = "windows"))]
    #[nativelink_test]
    async fn upload_dir_and_symlink_test() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, _slow_store, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);
        let queued_timestamp = make_system_time(1000);
        let action_result = {
            let command = Command {
                arguments: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    concat!(
                        "mkdir -p dir1/dir2 && ",
                        "echo foo > dir1/file && ",
                        "touch dir1/file2 && ",
                        "ln -s ../file dir1/dir2/sym &&",
                        "ln -s /dev/null empty_sym",
                    )
                    .to_string(),
                ],
                output_paths: vec!["dir1".to_string(), "empty_sym".to_string()],
                working_directory: ".".to_string(),
                environment_variables: vec![EnvironmentVariable {
                    name: "PATH".to_string(),
                    value: env::var("PATH").unwrap(),
                }],
                ..Default::default()
            };
            let command_digest = serialize_and_upload_message(
                &command,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let input_root_digest = serialize_and_upload_message(
                &Directory::default(),
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(
                &action,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;

            let execute_request = ExecuteRequest {
                action_digest: Some(action_digest.into()),
                ..Default::default()
            };
            let operation_id = OperationId::default().to_string();

            let running_action_impl = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(execute_request),
                        operation_id,
                        queued_timestamp: Some(queued_timestamp.into()),
                        platform: action.platform.clone(),
                        worker_id: WORKER_ID.to_string(),
                        peer_hints: Vec::new(),
                        resolved_directories: Vec::new(),
                        resolved_directory_digests: Vec::new(),
                    },
                )
                .await?;

            run_action(running_action_impl.clone()).await?
        };
        let tree = get_and_decode_digest::<Tree>(
            cas_store.as_ref(),
            action_result.output_folders[0].tree_digest.into(),
        )
        .await?;
        let root_directory = Directory {
            files: vec![
                FileNode {
                    name: "file".to_string(),
                    digest: Some(
                        DigestInfo::try_new(
                            "b5bb9d8014a0f9b1d61e21e796d78dccdf1352f23cd32812f4850b878ae4944c",
                            4,
                        )?
                        .into(),
                    ),
                    ..Default::default()
                },
                FileNode {
                    name: "file2".to_string(),
                    digest: Some(
                        DigestInfo::try_new(
                            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                            0,
                        )?
                        .into(),
                    ),
                    ..Default::default()
                },
            ],
            directories: vec![DirectoryNode {
                name: "dir2".to_string(),
                digest: Some(
                    DigestInfo::try_new(
                        "cce0098e0b0f1d785edb0da50beedb13e27dcd459b091b2f8f82543cb7cd0527",
                        16,
                    )?
                    .into(),
                ),
            }],
            ..Default::default()
        };
        assert_eq!(
            tree,
            Tree {
                root: Some(root_directory.clone()),
                children: vec![
                    Directory {
                        symlinks: vec![SymlinkNode {
                            name: "sym".to_string(),
                            target: "../file".to_string(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    root_directory
                ],
            }
        );
        let mut clock_time = make_system_time(0);
        assert_eq!(
            action_result,
            ActionResult {
                output_files: vec![],
                stdout_digest: DigestInfo::try_new(
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    0
                )?,
                stderr_digest: DigestInfo::try_new(
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    0
                )?,
                exit_code: 0,
                output_folders: vec![DirectoryInfo {
                    path: "dir1".to_string(),
                    tree_digest: DigestInfo::try_new(
                        "adbb04fa6e166e663c1310bbf8ba494e468b1b6c33e1e5346e2216b6904c9917",
                        490
                    )?,
                }],
                output_file_symlinks: vec![SymlinkInfo {
                    name_or_path: NameOrPath::Path("empty_sym".to_string()),
                    target: "/dev/null".to_string(),
                }],
                output_directory_symlinks: vec![],
                server_logs: HashMap::new(),
                execution_metadata: ExecutionMetadata {
                    worker: WORKER_ID.to_string(),
                    queued_timestamp,
                    worker_start_timestamp: increment_clock(&mut clock_time),
                    input_fetch_start_timestamp: increment_clock(&mut clock_time),
                    input_fetch_completed_timestamp: increment_clock(&mut clock_time),
                    execution_start_timestamp: increment_clock(&mut clock_time),
                    execution_completed_timestamp: increment_clock(&mut clock_time),
                    output_upload_start_timestamp: increment_clock(&mut clock_time),
                    output_upload_completed_timestamp: increment_clock(&mut clock_time),
                    worker_completed_timestamp: increment_clock(&mut clock_time),
                },
                error: None,
                message: String::new(),
            }
        );
        Ok(())
    }

    #[nativelink_test]
    async fn cleanup_happens_on_job_failure() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);
        let queued_timestamp = make_system_time(1000);

        #[cfg(target_family = "unix")]
        let arguments = vec!["sh".to_string(), "-c".to_string(), "exit 33".to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec!["cmd".to_string(), "/C".to_string(), "exit 33".to_string()];

        let action_result = {
            let command = Command {
                arguments,
                output_paths: vec![],
                working_directory: ".".to_string(),
                environment_variables: vec![EnvironmentVariable {
                    name: "PATH".to_string(),
                    value: env::var("PATH").unwrap(),
                }],
                ..Default::default()
            };
            let command_digest = serialize_and_upload_message(
                &command,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let input_root_digest = serialize_and_upload_message(
                &Directory::default(),
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(
                &action,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;

            let execute_request = ExecuteRequest {
                action_digest: Some(action_digest.into()),
                ..Default::default()
            };
            let operation_id = OperationId::default().to_string();

            let running_action_impl = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(execute_request),
                        operation_id,
                        queued_timestamp: Some(queued_timestamp.into()),
                        platform: action.platform.clone(),
                        worker_id: WORKER_ID.to_string(),
                        peer_hints: Vec::new(),
                        resolved_directories: Vec::new(),
                        resolved_directory_digests: Vec::new(),
                    },
                )
                .await?;

            run_action(running_action_impl.clone()).await?
        };
        let mut clock_time = make_system_time(0);
        assert_eq!(
            action_result,
            ActionResult {
                output_files: vec![],
                stdout_digest: DigestInfo::try_new(
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    0
                )?,
                stderr_digest: DigestInfo::try_new(
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                    0
                )?,
                exit_code: 33,
                output_folders: vec![],
                output_file_symlinks: vec![],
                output_directory_symlinks: vec![],
                server_logs: HashMap::new(),
                execution_metadata: ExecutionMetadata {
                    worker: WORKER_ID.to_string(),
                    queued_timestamp,
                    worker_start_timestamp: increment_clock(&mut clock_time),
                    input_fetch_start_timestamp: increment_clock(&mut clock_time),
                    input_fetch_completed_timestamp: increment_clock(&mut clock_time),
                    execution_start_timestamp: increment_clock(&mut clock_time),
                    execution_completed_timestamp: increment_clock(&mut clock_time),
                    output_upload_start_timestamp: increment_clock(&mut clock_time),
                    output_upload_completed_timestamp: increment_clock(&mut clock_time),
                    worker_completed_timestamp: increment_clock(&mut clock_time),
                },
                error: None,
                message: String::new(),
            }
        );
        let mut dir_stream = fs::read_dir(&root_action_directory).await?;
        assert!(
            dir_stream.as_mut().next_entry().await?.is_none(),
            "Expected empty directory at {root_action_directory}"
        );
        Ok(())
    }

    #[nativelink_test]
    async fn kill_ends_action() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            })?);

        #[cfg(target_family = "unix")]
        let (arguments, process_started_file) = {
            let process_started_file = {
                let tmp_dir = make_temp_path("root_action_directory");
                fs::create_dir_all(&tmp_dir).await.unwrap();
                format!("{tmp_dir}/process_started")
            };
            (
                vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    format!("touch {process_started_file} && sleep infinity"),
                ],
                process_started_file,
            )
        };
        #[cfg(target_family = "windows")]
        // Windows is weird with timeout, so we use ping. See:
        // https://www.ibm.com/support/pages/timeout-command-run-batch-job-exits-immediately-and-returns-error-input-redirection-not-supported-exiting-process-immediately
        let arguments = vec![
            "cmd".to_string(),
            "/C".to_string(),
            "ping -n 99999 127.0.0.1".to_string(),
        ];

        let command = Command {
            arguments,
            output_paths: vec![],
            working_directory: ".".to_string(),
            environment_variables: vec![EnvironmentVariable {
                name: "PATH".to_string(),
                value: env::var("PATH").unwrap(),
            }],
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(
            &command,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let input_root_digest = serialize_and_upload_message(
            &Directory::default(),
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(
            &action,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let execute_request = ExecuteRequest {
            action_digest: Some(action_digest.into()),
            ..Default::default()
        };
        let operation_id = OperationId::default().to_string();

        let running_action_impl = running_actions_manager
            .clone()
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id,
                    queued_timestamp: Some(make_system_time(1000).into()),
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: Vec::new(),
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await?;

        let run_action_fut = run_action(running_action_impl);
        tokio::pin!(run_action_fut);

        #[cfg(target_family = "unix")]
        loop {
            assert_eq!(futures::poll!(&mut run_action_fut), Poll::Pending);
            tokio::task::yield_now().await;
            match fs::metadata(&process_started_file).await {
                Ok(_) => break,
                Err(err) => {
                    assert_eq!(err.code, Code::NotFound, "Unknown error {err:?}");
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
        }

        let result = futures::join!(run_action_fut, running_actions_manager.kill_all())
            .0
            .unwrap();

        // Check that the action was killed.
        #[cfg(all(target_family = "unix", not(target_os = "macos")))]
        assert_eq!(9, result.exit_code, "Wrong exit_code - {result:?}");
        // Mac for some reason sometimes returns 1 and 9.
        #[cfg(all(target_family = "unix", target_os = "macos"))]
        assert!(
            9 == result.exit_code || 1 == result.exit_code,
            "Wrong exit_code - {result:?}"
        );
        // Note: Windows kill command returns exit code 1.
        #[cfg(target_family = "windows")]
        assert_eq!(1, result.exit_code);

        Ok(())
    }

    // This script runs a command under a wrapper script set in a config.
    // The wrapper script will print a constant string to stderr, and the test itself will
    // print to stdout. We then check the results of both to make sure the shell script was
    // invoked and the actual command was invoked under the shell script.
    #[cfg_attr(feature = "nix", ignore)]
    #[nativelink_test]
    async fn entrypoint_does_invoke_if_set() -> Result<(), Box<dyn core::error::Error>> {
        #[cfg(target_family = "unix")]
        const TEST_WRAPPER_SCRIPT_CONTENT: &str = "\
#!/usr/bin/env bash
# Print some static text to stderr. This is what the test uses to
# make sure the script did run.
>&2 printf \"Wrapper script did run\"

# Now run the real command.
exec \"$@\"
";
        #[cfg(target_family = "windows")]
        const TEST_WRAPPER_SCRIPT_CONTENT: &str = "\
@echo off
:: Print some static text to stderr. This is what the test uses to
:: make sure the script did run.
echo | set /p=\"Wrapper script did run\" 1>&2

:: Run command, but morph the echo to ensure it doesn't
:: add a new line to the end of the output.
%1 | set /p=%2
exit 0
";
        const WORKER_ID: &str = "foo_worker_id";
        const EXPECTED_STDOUT: &str = "Action did run";

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let test_wrapper_script = {
            let test_wrapper_dir = make_temp_path("wrapper_dir");
            fs::create_dir_all(&test_wrapper_dir).await?;
            #[cfg(target_family = "unix")]
            let test_wrapper_script = OsString::from(test_wrapper_dir + "/test_wrapper_script.sh");
            #[cfg(target_family = "windows")]
            let test_wrapper_script =
                OsString::from(test_wrapper_dir + "\\test_wrapper_script.bat");
            {
                let mut file_options = std::fs::OpenOptions::new();
                file_options.create(true);
                file_options.truncate(true);
                file_options.write(true);
                #[cfg(target_family = "unix")]
                file_options.mode(0o777);
                let mut test_wrapper_script_handle = file_options
                    .open(OsString::from(&test_wrapper_script))
                    .unwrap();
                test_wrapper_script_handle
                    .write_all(TEST_WRAPPER_SCRIPT_CONTENT.as_bytes())
                    .unwrap();
                test_wrapper_script_handle.sync_all().unwrap();
                // Note: Github runners appear to use some kind of filesystem driver
                // that does not sync data as expected. This is the easiest solution.
                // See: https://github.com/pantsbuild/pants/issues/10507
                // See: https://github.com/moby/moby/issues/9547
                std::process::Command::new("sync").output().unwrap();
            }
            test_wrapper_script
        };

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration {
                    entrypoint: Some(test_wrapper_script.into_string().unwrap()),
                    additional_environment: None,
                },
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            })?);
        #[cfg(target_family = "unix")]
        let arguments = vec!["printf".to_string(), EXPECTED_STDOUT.to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec!["echo".to_string(), EXPECTED_STDOUT.to_string()];
        let command = Command {
            arguments,
            working_directory: ".".to_string(),
            environment_variables: vec![EnvironmentVariable {
                name: "PATH".to_string(),
                value: env::var("PATH").unwrap(),
            }],
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(
            &command,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let input_root_digest = serialize_and_upload_message(
            &Directory::default(),
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(
            &action,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let execute_request = ExecuteRequest {
            action_digest: Some(action_digest.into()),
            ..Default::default()
        };
        let operation_id = OperationId::default().to_string();

        let running_action_impl = running_actions_manager
            .clone()
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id,
                    queued_timestamp: Some(make_system_time(1000).into()),
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: Vec::new(),
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await?;

        let result = run_action(running_action_impl).await?;
        assert_eq!(result.exit_code, 0, "Exit code should be 0");

        let expected_stdout = DigestHasherFunc::Sha256
            .hasher()
            .compute_from_reader(Cursor::new(EXPECTED_STDOUT))
            .await?;
        // Note: This string should match what is in worker_for_test.sh
        let expected_stderr = DigestHasherFunc::Sha256
            .hasher()
            .compute_from_reader(Cursor::new("Wrapper script did run"))
            .await?;
        assert_eq!(expected_stdout, result.stdout_digest);
        assert_eq!(expected_stderr, result.stderr_digest);

        Ok(())
    }

    #[cfg_attr(feature = "nix", ignore)]
    #[nativelink_test]
    async fn entrypoint_injects_properties() -> Result<(), Box<dyn core::error::Error>> {
        #[cfg(target_family = "unix")]
        const TEST_WRAPPER_SCRIPT_CONTENT: &str = "\
#!/usr/bin/env bash
# Print some static text to stderr. This is what the test uses to
# make sure the script did run.
>&2 printf \"Wrapper script did run with property $PROPERTY $VALUE $INNER_TIMEOUT\"

# Now run the real command.
exec \"$@\"
";
        #[cfg(target_family = "windows")]
        const TEST_WRAPPER_SCRIPT_CONTENT: &str = "\
@echo off
:: Print some static text to stderr. This is what the test uses to
:: make sure the script did run.
echo | set /p=\"Wrapper script did run with property %PROPERTY% %VALUE% %INNER_TIMEOUT%\" 1>&2

:: Run command, but morph the echo to ensure it doesn't
:: add a new line to the end of the output.
%1 | set /p=%2
exit 0
";
        const WORKER_ID: &str = "foo_worker_id";
        const EXPECTED_STDOUT: &str = "Action did run";
        const TASK_TIMEOUT: Duration = Duration::from_secs(122);

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let test_wrapper_script = {
            let test_wrapper_dir = make_temp_path("wrapper_dir");
            fs::create_dir_all(&test_wrapper_dir).await?;
            #[cfg(target_family = "unix")]
            let test_wrapper_script = OsString::from(test_wrapper_dir + "/test_wrapper_script.sh");
            #[cfg(target_family = "windows")]
            let test_wrapper_script =
                OsString::from(test_wrapper_dir + "\\test_wrapper_script.bat");
            {
                let mut file_options = std::fs::OpenOptions::new();
                file_options.create(true);
                file_options.truncate(true);
                file_options.write(true);
                #[cfg(target_family = "unix")]
                file_options.mode(0o777);
                let mut test_wrapper_script_handle = file_options
                    .open(OsString::from(&test_wrapper_script))
                    .unwrap();
                test_wrapper_script_handle
                    .write_all(TEST_WRAPPER_SCRIPT_CONTENT.as_bytes())
                    .unwrap();
                test_wrapper_script_handle.sync_all().unwrap();
                // Note: Github runners appear to use some kind of filesystem driver
                // that does not sync data as expected. This is the easiest solution.
                // See: https://github.com/pantsbuild/pants/issues/10507
                // See: https://github.com/moby/moby/issues/9547
                std::process::Command::new("sync").output().unwrap();
            }
            test_wrapper_script
        };

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration {
                    entrypoint: Some(test_wrapper_script.into_string().unwrap()),
                    additional_environment: Some(HashMap::from([
                        (
                            "PROPERTY".to_string(),
                            EnvironmentSource::Property("property_name".to_string()),
                        ),
                        (
                            "VALUE".to_string(),
                            EnvironmentSource::Value("raw_value".to_string()),
                        ),
                        (
                            "INNER_TIMEOUT".to_string(),
                            EnvironmentSource::TimeoutMillis,
                        ),
                        (
                            "PATH".to_string(),
                            EnvironmentSource::Value(env::var("PATH").unwrap()),
                        ),
                    ])),
                },
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            })?);
        #[cfg(target_family = "unix")]
        let arguments = vec!["printf".to_string(), EXPECTED_STDOUT.to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec!["echo".to_string(), EXPECTED_STDOUT.to_string()];
        let command = Command {
            arguments,
            working_directory: ".".to_string(),
            environment_variables: vec![EnvironmentVariable {
                name: "PATH".to_string(),
                value: env::var("PATH").unwrap(),
            }],
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(
            &command,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let input_root_digest = serialize_and_upload_message(
            &Directory::default(),
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            platform: Some(Platform {
                properties: vec![Property {
                    name: "property_name".into(),
                    value: "property_value".into(),
                }],
            }),
            timeout: Some(prost_types::Duration {
                seconds: TASK_TIMEOUT.as_secs() as i64,
                nanos: 0,
            }),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(
            &action,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let execute_request = ExecuteRequest {
            action_digest: Some(action_digest.into()),
            ..Default::default()
        };
        let operation_id = OperationId::default().to_string();

        let running_action_impl = running_actions_manager
            .clone()
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id,
                    queued_timestamp: Some(make_system_time(1000).into()),
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: Vec::new(),
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await?;

        let result = run_action(running_action_impl).await?;
        assert_eq!(result.exit_code, 0, "Exit code should be 0");

        let expected_stdout = DigestHasherFunc::Sha256
            .hasher()
            .compute_from_reader(Cursor::new(EXPECTED_STDOUT))
            .await?;
        // Note: This string should match what is in worker_for_test.sh
        let expected_stderr =
            "Wrapper script did run with property property_value raw_value 122000";
        let expected_stderr_digest = DigestHasherFunc::Sha256
            .hasher()
            .compute_from_reader(Cursor::new(expected_stderr))
            .await?;

        let actual_stderr: Bytes = cas_store
            .as_ref()
            .get_part_unchunked(result.stderr_digest, 0, None)
            .await?;
        let actual_stderr_decoded = from_utf8(&actual_stderr)?;
        assert_eq!(expected_stderr, actual_stderr_decoded);
        assert_eq!(expected_stdout, result.stdout_digest);
        assert_eq!(expected_stderr_digest, result.stderr_digest);

        Ok(())
    }

    #[cfg_attr(feature = "nix", ignore)]
    #[nativelink_test]
    async fn entrypoint_sends_timeout_via_side_channel() -> Result<(), Box<dyn core::error::Error>>
    {
        #[cfg(target_family = "unix")]
        const TEST_WRAPPER_SCRIPT_CONTENT: &str = "\
#!/bin/bash
echo '{\"failure\":\"timeout\"}' > \"$SIDE_CHANNEL_FILE\"
exit 1
";
        #[cfg(target_family = "windows")]
        const TEST_WRAPPER_SCRIPT_CONTENT: &str = "\
@echo off
echo | set /p={\"failure\":\"timeout\"} 1>&2 > %SIDE_CHANNEL_FILE%
exit 1
";
        const WORKER_ID: &str = "foo_worker_id";

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let test_wrapper_script = {
            let test_wrapper_dir = make_temp_path("wrapper_dir");
            fs::create_dir_all(&test_wrapper_dir).await?;
            #[cfg(target_family = "unix")]
            let test_wrapper_script = OsString::from(test_wrapper_dir + "/test_wrapper_script.sh");
            #[cfg(target_family = "windows")]
            let test_wrapper_script =
                OsString::from(test_wrapper_dir + "\\test_wrapper_script.bat");
            {
                let mut file_options = std::fs::OpenOptions::new();
                file_options.create(true);
                file_options.truncate(true);
                file_options.write(true);
                #[cfg(target_family = "unix")]
                file_options.mode(0o777);
                let mut test_wrapper_script_handle = file_options
                    .open(OsString::from(&test_wrapper_script))
                    .unwrap();
                test_wrapper_script_handle
                    .write_all(TEST_WRAPPER_SCRIPT_CONTENT.as_bytes())
                    .unwrap();
                test_wrapper_script_handle.sync_all().unwrap();
                // Note: Github runners appear to use some kind of filesystem driver
                // that does not sync data as expected. This is the easiest solution.
                // See: https://github.com/pantsbuild/pants/issues/10507
                // See: https://github.com/moby/moby/issues/9547
                std::process::Command::new("sync").output().unwrap();
            }
            test_wrapper_script
        };

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration {
                    entrypoint: Some(test_wrapper_script.into_string().unwrap()),
                    additional_environment: Some(HashMap::from([(
                        "SIDE_CHANNEL_FILE".to_string(),
                        EnvironmentSource::SideChannelFile,
                    )])),
                },
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            })?);
        let arguments = vec!["true".to_string()];
        let command = Command {
            arguments,
            working_directory: ".".to_string(),
            environment_variables: vec![EnvironmentVariable {
                name: "PATH".to_string(),
                value: env::var("PATH").unwrap(),
            }],
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(
            &command,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let input_root_digest = serialize_and_upload_message(
            &Directory::default(),
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(
            &action,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let execute_request = ExecuteRequest {
            action_digest: Some(action_digest.into()),
            ..Default::default()
        };
        let operation_id = OperationId::default().to_string();

        let running_action_impl = running_actions_manager
            .clone()
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id,
                    queued_timestamp: Some(make_system_time(1000).into()),
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: Vec::new(),
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await?;

        let result = run_action(running_action_impl).await?;
        assert_eq!(result.exit_code, 1, "Exit code should be 1");
        assert_eq!(
            result.error.err_tip(|| "Error should exist")?.code,
            Code::DeadlineExceeded
        );
        Ok(())
    }

    #[nativelink_test]
    async fn caches_results_in_action_cache_store() -> Result<(), Box<dyn core::error::Error>> {
        let (_, _, cas_store, ac_store) = setup_stores().await?;

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: String::new(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::SuccessOnly,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            })?);

        let action_digest = DigestInfo::new([2u8; 32], 32);
        let mut action_result = ActionResult {
            output_files: vec![FileInfo {
                name_or_path: NameOrPath::Path("test.txt".to_string()),
                digest: DigestInfo::try_new(
                    "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3",
                    3,
                )?,
                is_executable: false,
            }],
            stdout_digest: DigestInfo::try_new(
                "426afaf613d8cfdd9fa8addcc030ae6c95a7950ae0301164af1d5851012081d5",
                10,
            )?,
            stderr_digest: DigestInfo::try_new(
                "7b2e400d08b8e334e3172d105be308b506c6036c62a9bde5c509d7808b28b213",
                10,
            )?,
            exit_code: 0,
            output_folders: vec![],
            output_file_symlinks: vec![],
            output_directory_symlinks: vec![],
            server_logs: HashMap::new(),
            execution_metadata: ExecutionMetadata {
                worker: "WORKER_ID".to_string(),
                queued_timestamp: SystemTime::UNIX_EPOCH,
                worker_start_timestamp: make_system_time(0),
                input_fetch_start_timestamp: make_system_time(1),
                input_fetch_completed_timestamp: make_system_time(2),
                execution_start_timestamp: make_system_time(3),
                execution_completed_timestamp: make_system_time(4),
                output_upload_start_timestamp: make_system_time(5),
                output_upload_completed_timestamp: make_system_time(6),
                worker_completed_timestamp: make_system_time(7),
            },
            error: None,
            message: String::new(),
        };
        running_actions_manager
            .cache_action_result(action_digest, &mut action_result, DigestHasherFunc::Sha256)
            .await?;

        let retrieved_result =
            get_and_decode_digest::<ProtoActionResult>(ac_store.as_ref(), action_digest.into())
                .await?;

        let proto_result: ProtoActionResult = action_result.try_into()?;
        assert_eq!(proto_result, retrieved_result);

        Ok(())
    }

    #[nativelink_test]
    async fn failed_action_does_not_cache_in_action_cache()
    -> Result<(), Box<dyn core::error::Error>> {
        let (_, _, cas_store, ac_store) = setup_stores().await?;

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: String::new(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Everything,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            })?);

        let action_digest = DigestInfo::new([2u8; 32], 32);
        let mut action_result = ActionResult {
            output_files: vec![FileInfo {
                name_or_path: NameOrPath::Path("test.txt".to_string()),
                digest: DigestInfo::try_new(
                    "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3",
                    3,
                )?,
                is_executable: false,
            }],
            stdout_digest: DigestInfo::try_new(
                "426afaf613d8cfdd9fa8addcc030ae6c95a7950ae0301164af1d5851012081d5",
                10,
            )?,
            stderr_digest: DigestInfo::try_new(
                "7b2e400d08b8e334e3172d105be308b506c6036c62a9bde5c509d7808b28b213",
                10,
            )?,
            exit_code: 1,
            output_folders: vec![],
            output_file_symlinks: vec![],
            output_directory_symlinks: vec![],
            server_logs: HashMap::new(),
            execution_metadata: ExecutionMetadata {
                worker: "WORKER_ID".to_string(),
                queued_timestamp: SystemTime::UNIX_EPOCH,
                worker_start_timestamp: make_system_time(0),
                input_fetch_start_timestamp: make_system_time(1),
                input_fetch_completed_timestamp: make_system_time(2),
                execution_start_timestamp: make_system_time(3),
                execution_completed_timestamp: make_system_time(4),
                output_upload_start_timestamp: make_system_time(5),
                output_upload_completed_timestamp: make_system_time(6),
                worker_completed_timestamp: make_system_time(7),
            },
            error: None,
            message: String::new(),
        };
        running_actions_manager
            .cache_action_result(action_digest, &mut action_result, DigestHasherFunc::Sha256)
            .await?;

        let retrieved_result =
            get_and_decode_digest::<ProtoActionResult>(ac_store.as_ref(), action_digest.into())
                .await?;

        let proto_result: ProtoActionResult = action_result.try_into()?;
        assert_eq!(proto_result, retrieved_result);

        Ok(())
    }

    #[nativelink_test]
    async fn success_does_cache_in_historical_results() -> Result<(), Box<dyn core::error::Error>> {
        let (_, _, cas_store, ac_store) = setup_stores().await?;

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: String::new(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_historical_results_strategy: Some(
                            nativelink_config::cas_server::UploadCacheResultsStrategy::SuccessOnly,
                        ),
                        #[expect(
                            clippy::literal_string_with_formatting_args,
                            reason = "passed to `formatx` crate for runtime interpretation"
                        )]
                        success_message_template:
                            "{historical_results_hash}-{historical_results_size}".to_string(),
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            })?);

        let action_digest = DigestInfo::new([2u8; 32], 32);
        let mut action_result = ActionResult {
            output_files: vec![FileInfo {
                name_or_path: NameOrPath::Path("test.txt".to_string()),
                digest: DigestInfo::try_new(
                    "a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3",
                    3,
                )?,
                is_executable: false,
            }],
            stdout_digest: DigestInfo::try_new(
                "426afaf613d8cfdd9fa8addcc030ae6c95a7950ae0301164af1d5851012081d5",
                10,
            )?,
            stderr_digest: DigestInfo::try_new(
                "7b2e400d08b8e334e3172d105be308b506c6036c62a9bde5c509d7808b28b213",
                10,
            )?,
            exit_code: 0,
            output_folders: vec![],
            output_file_symlinks: vec![],
            output_directory_symlinks: vec![],
            server_logs: HashMap::new(),
            execution_metadata: ExecutionMetadata {
                worker: "WORKER_ID".to_string(),
                queued_timestamp: SystemTime::UNIX_EPOCH,
                worker_start_timestamp: make_system_time(0),
                input_fetch_start_timestamp: make_system_time(1),
                input_fetch_completed_timestamp: make_system_time(2),
                execution_start_timestamp: make_system_time(3),
                execution_completed_timestamp: make_system_time(4),
                output_upload_start_timestamp: make_system_time(5),
                output_upload_completed_timestamp: make_system_time(6),
                worker_completed_timestamp: make_system_time(7),
            },
            error: None,
            message: String::new(),
        };
        running_actions_manager
            .cache_action_result(action_digest, &mut action_result, DigestHasherFunc::Sha256)
            .await?;

        assert!(!action_result.message.is_empty(), "Message should be set");

        let historical_digest = {
            let (historical_results_hash, historical_results_size) = action_result
                .message
                .split_once('-')
                .expect("Message should be in format {hash}-{size}");

            DigestInfo::try_new(
                historical_results_hash,
                historical_results_size.parse::<i64>()?,
            )?
        };
        let retrieved_result = get_and_decode_digest::<HistoricalExecuteResponse>(
            cas_store.as_ref(),
            historical_digest.into(),
        )
        .await?;

        assert_eq!(
            HistoricalExecuteResponse {
                action_digest: Some(action_digest.into()),
                execute_response: Some(ExecuteResponse {
                    result: Some(action_result.try_into()?),
                    status: Some(Status::default()),
                    ..Default::default()
                }),
            },
            retrieved_result
        );

        Ok(())
    }

    #[nativelink_test]
    async fn failure_does_not_cache_in_historical_results()
    -> Result<(), Box<dyn core::error::Error>> {
        let (_, _, cas_store, ac_store) = setup_stores().await?;

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: String::new(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_historical_results_strategy: Some(
                            nativelink_config::cas_server::UploadCacheResultsStrategy::SuccessOnly,
                        ),
                        success_message_template:
                            "{historical_results_hash}-{historical_results_size}".to_string(),
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            })?);

        let action_digest = DigestInfo::new([2u8; 32], 32);
        let mut action_result = ActionResult {
            exit_code: 1,
            ..Default::default()
        };
        running_actions_manager
            .cache_action_result(action_digest, &mut action_result, DigestHasherFunc::Sha256)
            .await?;

        assert!(
            action_result.message.is_empty(),
            "Message should not be set"
        );
        Ok(())
    }

    #[nativelink_test]
    async fn infra_failure_does_cache_in_historical_results()
    -> Result<(), Box<dyn core::error::Error>> {
        let (_, _, cas_store, ac_store) = setup_stores().await?;

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: String::new(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_historical_results_strategy: Some(
                            nativelink_config::cas_server::UploadCacheResultsStrategy::FailuresOnly,
                        ),
                        #[expect(
                            clippy::literal_string_with_formatting_args,
                            reason = "passed to `formatx` crate for runtime interpretation"
                        )]
                        failure_message_template:
                            "{historical_results_hash}-{historical_results_size}".to_string(),
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            })?);

        let action_digest = DigestInfo::new([2u8; 32], 32);
        let mut action_result = ActionResult {
            exit_code: 0,
            error: Some(make_input_err!("test error")),
            ..Default::default()
        };
        running_actions_manager
            .cache_action_result(action_digest, &mut action_result, DigestHasherFunc::Sha256)
            .await?;

        assert!(!action_result.message.is_empty(), "Message should be set");

        let historical_digest = {
            let (historical_results_hash, historical_results_size) = action_result
                .message
                .split_once('-')
                .expect("Message should be in format {hash}-{size}");

            DigestInfo::try_new(
                historical_results_hash,
                historical_results_size.parse::<i64>()?,
            )?
        };

        let retrieved_result = get_and_decode_digest::<HistoricalExecuteResponse>(
            cas_store.as_ref(),
            historical_digest.into(),
        )
        .await?;

        assert_eq!(
            HistoricalExecuteResponse {
                action_digest: Some(action_digest.into()),
                execute_response: Some(ExecuteResponse {
                    result: Some(action_result.try_into()?),
                    status: Some(make_input_err!("test error").into()),
                    ..Default::default()
                }),
            },
            retrieved_result
        );
        Ok(())
    }

    #[nativelink_test]
    async fn action_result_has_used_in_message() -> Result<(), Box<dyn core::error::Error>> {
        let (_, _, cas_store, ac_store) = setup_stores().await?;

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: String::new(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::SuccessOnly,
                        success_message_template: "{action_digest_hash}-{action_digest_size}"
                            .to_string(),
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            })?);

        let action_digest = DigestInfo::new([2u8; 32], 32);
        let mut action_result = ActionResult {
            exit_code: 0,
            ..Default::default()
        };
        running_actions_manager
            .cache_action_result(action_digest, &mut action_result, DigestHasherFunc::Sha256)
            .await?;

        assert!(!action_result.message.is_empty(), "Message should be set");

        let action_result_digest = {
            let (action_result_hash, action_result_size) = action_result
                .message
                .split_once('-')
                .expect("Message should be in format {hash}-{size}");

            DigestInfo::try_new(action_result_hash, action_result_size.parse::<i64>()?)?
        };

        let retrieved_result = get_and_decode_digest::<ProtoActionResult>(
            ac_store.as_ref(),
            action_result_digest.into(),
        )
        .await?;

        let proto_result: ProtoActionResult = action_result.try_into()?;
        assert_eq!(proto_result, retrieved_result);
        Ok(())
    }

    #[nativelink_test]
    async fn ensure_worker_timeout_chooses_correct_values()
    -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let (_, _, cas_store, ac_store) = setup_stores().await?;

        #[cfg(target_family = "unix")]
        let arguments = vec!["true".to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec![
            "cmd".to_string(),
            "/C".to_string(),
            "exit".to_string(),
            "0".to_string(),
        ];

        let command = Command {
            arguments,
            output_paths: vec![],
            working_directory: ".".to_string(),
            environment_variables: vec![EnvironmentVariable {
                name: "PATH".to_string(),
                value: env::var("PATH").unwrap(),
            }],
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(
            &command,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let input_root_digest = serialize_and_upload_message(
            &Directory::default(),
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        {
            // Test to ensure that the task timeout is chosen if it is less than the max timeout.
            static SENT_TIMEOUT: AtomicI64 = AtomicI64::new(-1);
            const MAX_TIMEOUT_DURATION: Duration = Duration::from_secs(100);
            const TASK_TIMEOUT: Duration = Duration::from_secs(10);

            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                timeout: Some(prost_types::Duration {
                    seconds: TASK_TIMEOUT.as_secs() as i64,
                    nanos: 0,
                }),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(
                &action,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;

            let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
                RunningActionsManagerArgs {
                    root_action_directory: root_action_directory.clone(),
                    execution_configuration: ExecutionConfiguration::default(),
                    cas_store: cas_store.clone(),
                    ac_store: Some(Store::new(ac_store.clone())),
                    historical_store: Store::new(cas_store.clone()),
                    upload_action_result_config:
                        &nativelink_config::cas_server::UploadActionResultConfig {
                            upload_ac_results_strategy:
                                nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                            ..Default::default()
                        },
                    max_action_timeout: MAX_TIMEOUT_DURATION,
                    max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                    timeout_handled_externally: false,
                    directory_cache: None,
                    peer_locality_map: None,
                },
                Callbacks {
                    now_fn: test_monotonic_clock,
                    sleep_fn: |duration| {
                        SENT_TIMEOUT.store(
                            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
                            Ordering::Relaxed,
                        );
                        Box::pin(future::pending())
                    },
                },
            )?);

            let execute_request = ExecuteRequest {
                action_digest: Some(action_digest.into()),
                ..Default::default()
            };
            let operation_id = OperationId::default().to_string();

            running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(execute_request),
                        operation_id,
                        queued_timestamp: Some(make_system_time(1000).into()),
                        platform: action.platform.clone(),
                        worker_id: WORKER_ID.to_string(),
                        peer_hints: Vec::new(),
                        resolved_directories: Vec::new(),
                        resolved_directory_digests: Vec::new(),
                    },
                )
                .and_then(|action| {
                    action
                        .clone()
                        .prepare_action()
                        .and_then(RunningAction::execute)
                        .then(|result| async move {
                            if let Err(e) = action.cleanup().await {
                                return Result::<ActionResult, Error>::Err(e).merge(result);
                            }
                            result
                        })
                })
                .await?;
            assert_eq!(
                SENT_TIMEOUT.load(Ordering::Relaxed),
                i64::try_from(TASK_TIMEOUT.as_millis())
                    .expect("TASK_TIMEOUT.as_millis() exceeds i64::MAX")
            );
        }
        {
            // Ensure if no timeout is set use max timeout.
            static SENT_TIMEOUT: AtomicI64 = AtomicI64::new(-1);
            const MAX_TIMEOUT_DURATION: Duration = Duration::from_secs(100);
            const TASK_TIMEOUT: Duration = Duration::from_secs(0);

            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                timeout: Some(prost_types::Duration {
                    seconds: TASK_TIMEOUT.as_secs() as i64,
                    nanos: 0,
                }),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(
                &action,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;

            let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
                RunningActionsManagerArgs {
                    root_action_directory: root_action_directory.clone(),
                    execution_configuration: ExecutionConfiguration::default(),
                    cas_store: cas_store.clone(),
                    ac_store: Some(Store::new(ac_store.clone())),
                    historical_store: Store::new(cas_store.clone()),
                    upload_action_result_config:
                        &nativelink_config::cas_server::UploadActionResultConfig {
                            upload_ac_results_strategy:
                                nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                            ..Default::default()
                        },
                    max_action_timeout: MAX_TIMEOUT_DURATION,
                    max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                    timeout_handled_externally: false,
                    directory_cache: None,
                    peer_locality_map: None,
                },
                Callbacks {
                    now_fn: test_monotonic_clock,
                    sleep_fn: |duration| {
                        SENT_TIMEOUT.store(
                            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
                            Ordering::Relaxed,
                        );
                        Box::pin(future::pending())
                    },
                },
            )?);

            let execute_request = ExecuteRequest {
                action_digest: Some(action_digest.into()),
                ..Default::default()
            };
            let operation_id = OperationId::default().to_string();

            running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(execute_request),
                        operation_id,
                        queued_timestamp: Some(make_system_time(1000).into()),
                        platform: action.platform.clone(),
                        worker_id: WORKER_ID.to_string(),
                        peer_hints: Vec::new(),
                        resolved_directories: Vec::new(),
                        resolved_directory_digests: Vec::new(),
                    },
                )
                .and_then(|action| {
                    action
                        .clone()
                        .prepare_action()
                        .and_then(RunningAction::execute)
                        .then(|result| async move {
                            if let Err(e) = action.cleanup().await {
                                return Result::<ActionResult, Error>::Err(e).merge(result);
                            }
                            result
                        })
                })
                .await?;
            assert_eq!(
                SENT_TIMEOUT.load(Ordering::Relaxed),
                i64::try_from(MAX_TIMEOUT_DURATION.as_millis())
                    .expect("MAX_TIMEOUT_DURATION.as_millis() exceeds i64::MAX")
            );
        }
        {
            // Ensure we reject tasks that have a timeout set too high.
            static SENT_TIMEOUT: AtomicI64 = AtomicI64::new(-1);
            const MAX_TIMEOUT_DURATION: Duration = Duration::from_secs(100);
            const TASK_TIMEOUT: Duration = Duration::from_secs(200);

            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                timeout: Some(prost_types::Duration {
                    seconds: TASK_TIMEOUT.as_secs() as i64,
                    nanos: 0,
                }),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(
                &action,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;

            let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
                RunningActionsManagerArgs {
                    root_action_directory: root_action_directory.clone(),
                    execution_configuration: ExecutionConfiguration::default(),
                    cas_store: cas_store.clone(),
                    ac_store: Some(Store::new(ac_store.clone())),
                    historical_store: Store::new(cas_store.clone()),
                    upload_action_result_config:
                        &nativelink_config::cas_server::UploadActionResultConfig {
                            upload_ac_results_strategy:
                                nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                            ..Default::default()
                        },
                    max_action_timeout: MAX_TIMEOUT_DURATION,
                    max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                    timeout_handled_externally: false,
                    directory_cache: None,
                    peer_locality_map: None,
                },
                Callbacks {
                    now_fn: test_monotonic_clock,
                    sleep_fn: |duration| {
                        SENT_TIMEOUT.store(
                            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX),
                            Ordering::Relaxed,
                        );
                        Box::pin(future::pending())
                    },
                },
            )?);

            let execute_request = ExecuteRequest {
                action_digest: Some(action_digest.into()),
                ..Default::default()
            };
            let operation_id = OperationId::default().to_string();

            let result = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(execute_request),
                        operation_id,
                        queued_timestamp: Some(make_system_time(1000).into()),
                        platform: action.platform.clone(),
                        worker_id: WORKER_ID.to_string(),
                        peer_hints: Vec::new(),
                        resolved_directories: Vec::new(),
                        resolved_directory_digests: Vec::new(),
                    },
                )
                .and_then(|action| {
                    action
                        .clone()
                        .prepare_action()
                        .and_then(RunningAction::execute)
                        .then(|result| async move {
                            if let Err(e) = action.cleanup().await {
                                return Result::<ActionResult, Error>::Err(e).merge(result);
                            }
                            result
                        })
                })
                .await;
            assert_eq!(SENT_TIMEOUT.load(Ordering::Relaxed), -1);
            assert_eq!(result.err().unwrap().code, Code::InvalidArgument);
        }
        Ok(())
    }

    #[nativelink_test]
    async fn worker_times_out() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        type StaticOneshotTuple =
            Mutex<(Option<oneshot::Sender<()>>, Option<oneshot::Receiver<()>>)>;
        static TIMEOUT_ONESHOT: LazyLock<StaticOneshotTuple> = LazyLock::new(|| {
            let (tx, rx) = oneshot::channel();
            Mutex::new((Some(tx), Some(rx)))
        });
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| {
                    Box::pin(async move {
                        let rx = TIMEOUT_ONESHOT.lock().unwrap().1.take().unwrap();
                        rx.await.expect("Could not receive timeout signal");
                    })
                },
            },
        )?);

        #[cfg(target_family = "unix")]
        let arguments = vec![
            "sh".to_string(),
            "-c".to_string(),
            "sleep infinity".to_string(),
        ];
        #[cfg(target_family = "windows")]
        // Windows is weird with timeout, so we use ping. See:
        // https://www.ibm.com/support/pages/timeout-command-run-batch-job-exits-immediately-and-returns-error-input-redirection-not-supported-exiting-process-immediately
        let arguments = vec![
            "cmd".to_string(),
            "/C".to_string(),
            "ping -n 99999 127.0.0.1".to_string(),
        ];

        let command = Command {
            arguments,
            output_paths: vec![],
            working_directory: ".".to_string(),
            environment_variables: vec![EnvironmentVariable {
                name: "PATH".to_string(),
                value: env::var("PATH").unwrap(),
            }],
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(
            &command,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let input_root_digest = serialize_and_upload_message(
            &Directory::default(),
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(
            &action,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let execute_request = ExecuteRequest {
            action_digest: Some(action_digest.into()),
            ..Default::default()
        };
        let operation_id = OperationId::default().to_string();

        let execute_results_fut = running_actions_manager
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id,
                    queued_timestamp: Some(make_system_time(1000).into()),
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: Vec::new(),
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .and_then(|action| {
                action
                    .clone()
                    .prepare_action()
                    .and_then(RunningAction::execute)
                    .and_then(RunningAction::upload_results)
                    .and_then(RunningAction::get_finished_result)
                    .then(|result| async move {
                        if let Err(e) = action.cleanup().await {
                            return Result::<ActionResult, Error>::Err(e).merge(result);
                        }
                        result
                    })
            });

        let (results, ()) = tokio::join!(execute_results_fut, async move {
            tokio::task::yield_now().await;
            let tx = TIMEOUT_ONESHOT.lock().unwrap().0.take().unwrap();
            tx.send(()).expect("Could not send timeout signal");
        });
        assert_eq!(results?.error.unwrap().code, Code::DeadlineExceeded);

        #[cfg(target_family = "unix")]
        let command = "[\"sh\", \"-c\", \"sleep infinity\"]";
        #[cfg(target_family = "windows")]
        let command = "[\"cmd\", \"/C\", \"ping -n 99999 127.0.0.1\"]";

        assert!(logs_contain(&format!("Executing command args={command}")));
        assert!(logs_contain(&format!("Command complete args={command}")));

        assert!(!logs_contain(
            "Child process was not cleaned up before dropping the call to execute(), killing in background spawn"
        ));
        #[cfg(target_family = "unix")]
        assert!(logs_contain(
            "Command timed out seconds=0.0 command=sh -c sleep infinity"
        ));
        #[cfg(target_family = "windows")]
        assert!(logs_contain(
            "Command timed out seconds=0.0 command=cmd /C ping -n 99999 127.0.0.1"
        ));

        Ok(())
    }

    #[nativelink_test]
    async fn kill_all_waits_for_all_tasks_to_finish() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);

        #[cfg(target_family = "unix")]
        let arguments = vec![
            "sh".to_string(),
            "-c".to_string(),
            "sleep infinity".to_string(),
        ];
        #[cfg(target_family = "windows")]
        // Windows is weird with timeout, so we use ping. See:
        // https://www.ibm.com/support/pages/timeout-command-run-batch-job-exits-immediately-and-returns-error-input-redirection-not-supported-exiting-process-immediately
        let arguments = vec![
            "cmd".to_string(),
            "/C".to_string(),
            "ping -n 99999 127.0.0.1".to_string(),
        ];

        let command = Command {
            arguments,
            output_paths: vec![],
            working_directory: ".".to_string(),
            environment_variables: vec![EnvironmentVariable {
                name: "PATH".to_string(),
                value: env::var("PATH").unwrap(),
            }],
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(
            &command,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let input_root_digest = serialize_and_upload_message(
            &Directory::default(),
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(
            &action,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let execute_request = ExecuteRequest {
            action_digest: Some(action_digest.into()),
            ..Default::default()
        };
        let operation_id = OperationId::default().to_string();

        let (cleanup_tx, cleanup_rx) = oneshot::channel();
        let cleanup_was_requested = AtomicBool::new(false);
        let action = running_actions_manager
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id,
                    queued_timestamp: Some(make_system_time(1000).into()),
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: Vec::new(),
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await?;
        let execute_results_fut = action
            .clone()
            .prepare_action()
            .and_then(RunningAction::execute)
            .and_then(RunningAction::upload_results)
            .and_then(RunningAction::get_finished_result)
            .then(|result| async {
                cleanup_was_requested.store(true, Ordering::Release);
                cleanup_rx.await.expect("Could not receive cleanup signal");
                if let Err(e) = action.cleanup().await {
                    return Result::<ActionResult, Error>::Err(e).merge(result);
                }
                result
            });

        tokio::pin!(execute_results_fut);
        {
            // Advance the action as far as possible and ensure we are not waiting on cleanup.
            for _ in 0..100 {
                assert!(futures::poll!(&mut execute_results_fut).is_pending());
                tokio::task::yield_now().await;
            }
            assert_eq!(cleanup_was_requested.load(Ordering::Acquire), false);
        }

        let kill_all_fut = running_actions_manager.kill_all();
        tokio::pin!(kill_all_fut);

        {
            // * Advance the action as far as possible.
            // * Ensure we are now waiting on cleanup.
            // * Ensure our kill_action is still pending.
            while !cleanup_was_requested.load(Ordering::Acquire) {
                // Wait for cleanup to be triggered.
                tokio::task::yield_now().await;
                assert!(futures::poll!(&mut execute_results_fut).is_pending());
                assert!(futures::poll!(&mut kill_all_fut).is_pending());
            }
        }
        // Allow cleanup, which allows execute_results_fut to advance.
        cleanup_tx.send(()).expect("Could not send cleanup signal");
        // Advance our two futures to completion now.
        let result = execute_results_fut.await;
        kill_all_fut.await;
        {
            // Ensure our results are correct.
            let action_result = result?;
            let err = action_result
                .error
                .as_ref()
                .err_tip(|| format!("No error exists in result : {action_result:?}"))?;
            assert_eq!(
                err.code,
                Code::Aborted,
                "Expected Aborted : {action_result:?}"
            );
        }

        Ok(())
    }

    /// Regression Test for Issue #675
    #[cfg(target_family = "unix")]
    #[nativelink_test]
    async fn unix_executable_file_test() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";
        const FILE_1_NAME: &str = "file1";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                execution_configuration: ExecutionConfiguration::default(),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);
        // Create and run an action which
        // creates a file with owner executable permissions.
        let action_result = {
            let command = Command {
                arguments: vec![
                    "sh".to_string(),
                    "-c".to_string(),
                    format!("touch {FILE_1_NAME} && chmod 700 {FILE_1_NAME}"),
                ],
                output_paths: vec![FILE_1_NAME.to_string()],
                working_directory: ".".to_string(),
                environment_variables: vec![EnvironmentVariable {
                    name: "PATH".to_string(),
                    value: env::var("PATH").unwrap(),
                }],
                ..Default::default()
            };
            let command_digest = serialize_and_upload_message(
                &command,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let input_root_digest = serialize_and_upload_message(
                &Directory::default(),
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(
                &action,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;

            let execute_request = ExecuteRequest {
                action_digest: Some(action_digest.into()),
                ..Default::default()
            };
            let operation_id = OperationId::default().to_string();

            let running_action_impl = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(execute_request),
                        operation_id,
                        ..Default::default()
                    },
                )
                .await?;

            run_action(running_action_impl.clone()).await?
        };
        // Ensure the file copied from worker to CAS is executable.
        assert!(
            action_result.output_files[0].is_executable,
            "Expected output file to be executable"
        );
        Ok(())
    }

    #[nativelink_test]
    async fn action_directory_contents_are_cleaned() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;
        let temp_action_directory = make_temp_path("root_action_directory/temp");
        fs::create_dir_all(&temp_action_directory).await?;

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            })?);
        let queued_timestamp = make_system_time(1000);

        #[cfg(target_family = "unix")]
        let arguments = vec!["sh".to_string(), "-c".to_string(), "exit 0".to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec!["cmd".to_string(), "/C".to_string(), "exit 0".to_string()];

        let command = Command {
            arguments,
            output_paths: vec![],
            working_directory: ".".to_string(),
            environment_variables: vec![EnvironmentVariable {
                name: "PATH".to_string(),
                value: env::var("PATH").unwrap(),
            }],
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(
            &command,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let input_root_digest = serialize_and_upload_message(
            &Directory::default(),
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(
            &action,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let execute_request = ExecuteRequest {
            action_digest: Some(action_digest.into()),
            ..Default::default()
        };
        let operation_id = OperationId::default().to_string();

        let running_action_impl = running_actions_manager
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id,
                    queued_timestamp: Some(queued_timestamp.into()),
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: Vec::new(),
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await?;

        run_action(running_action_impl.clone()).await?;

        let mut dir_stream = fs::read_dir(&root_action_directory).await?;
        assert!(
            dir_stream.as_mut().next_entry().await?.is_none(),
            "Expected empty directory at {root_action_directory}"
        );
        Ok(())
    }

    // We've experienced deadlocks when uploading, so make only a single permit available and
    // check it's able to handle uploading some directories with some files in.

    // TODO(palfrey) This is unix only only because I was lazy and didn't spend the time to
    // build the bash-like commands in windows as well.

    #[nativelink_test]
    #[cfg(target_family = "unix")]
    async fn upload_with_single_permit() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, _slow_store, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        // Take all but one FD permit away.
        let _permits = stream::iter(1..fs::OPEN_FILE_SEMAPHORE.available_permits())
            .then(|_| fs::OPEN_FILE_SEMAPHORE.acquire())
            .try_collect::<Vec<_>>()
            .await?;
        assert_eq!(1, fs::OPEN_FILE_SEMAPHORE.available_permits());

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);
        let action_result = {
            let arguments = vec![
            "sh".to_string(),
            "-c".to_string(),
            "printf '123 ' > ./test.txt; mkdir ./tst; printf '456 ' > ./tst/tst.txt; printf 'foo-stdout '; >&2 printf 'bar-stderr  '"
                .to_string(),
        ];
            let working_directory = "some_cwd";
            let command = Command {
                arguments,
                output_paths: vec!["test.txt".to_string(), "tst".to_string()],
                working_directory: working_directory.to_string(),
                environment_variables: vec![EnvironmentVariable {
                    name: "PATH".to_string(),
                    value: env::var("PATH").unwrap(),
                }],
                ..Default::default()
            };
            let command_digest = serialize_and_upload_message(
                &command,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let input_root_digest = serialize_and_upload_message(
                &Directory {
                    directories: vec![DirectoryNode {
                        name: working_directory.to_string(),
                        digest: Some(
                            serialize_and_upload_message(
                                &Directory::default(),
                                cas_store.as_pin(),
                                &mut DigestHasherFunc::Sha256.hasher(),
                            )
                            .await?
                            .into(),
                        ),
                    }],
                    ..Default::default()
                },
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(
                &action,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;

            let execute_request = ExecuteRequest {
                action_digest: Some(action_digest.into()),
                ..Default::default()
            };
            let operation_id = OperationId::default().to_string();

            let running_action_impl = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(execute_request),
                        operation_id,
                        queued_timestamp: None,
                        platform: action.platform.clone(),
                        worker_id: WORKER_ID.to_string(),
                        peer_hints: Vec::new(),
                        resolved_directories: Vec::new(),
                        resolved_directory_digests: Vec::new(),
                    },
                )
                .await?;

            run_action(running_action_impl.clone()).await?
        };
        let file_content = cas_store
            .as_ref()
            .get_part_unchunked(action_result.output_files[0].digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&file_content)?, "123 ");
        let stdout_content = cas_store
            .as_ref()
            .get_part_unchunked(action_result.stdout_digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&stdout_content)?, "foo-stdout ");
        let stderr_content = cas_store
            .as_ref()
            .get_part_unchunked(action_result.stderr_digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&stderr_content)?, "bar-stderr  ");
        let mut clock_time = make_system_time(0);
        assert_eq!(
            action_result,
            ActionResult {
                output_files: vec![FileInfo {
                    name_or_path: NameOrPath::Path("test.txt".to_string()),
                    digest: DigestInfo::try_new(
                        "c69e10a5f54f4e28e33897fbd4f8701595443fa8c3004aeaa20dd4d9a463483b",
                        4
                    )?,
                    is_executable: false,
                }],
                stdout_digest: DigestInfo::try_new(
                    "15019a676f057d97d1ad3af86f3cc1e623cb33b18ff28422bbe3248d2471cc94",
                    11
                )?,
                stderr_digest: DigestInfo::try_new(
                    "2375ab8a01ca11e1ea7606dfb58756c153d49733cde1dbfb5a1e00f39afacf06",
                    12
                )?,
                exit_code: 0,
                output_folders: vec![DirectoryInfo {
                    path: "tst".to_string(),
                    tree_digest: DigestInfo::try_new(
                        "95711c1905d4898a70209dd6e98241dcafb479c00241a1ea4ed8415710d706f3",
                        166,
                    )?,
                },],
                output_file_symlinks: vec![],
                output_directory_symlinks: vec![],
                server_logs: HashMap::new(),
                execution_metadata: ExecutionMetadata {
                    worker: WORKER_ID.to_string(),
                    queued_timestamp: SystemTime::UNIX_EPOCH,
                    worker_start_timestamp: increment_clock(&mut clock_time),
                    input_fetch_start_timestamp: increment_clock(&mut clock_time),
                    input_fetch_completed_timestamp: increment_clock(&mut clock_time),
                    execution_start_timestamp: increment_clock(&mut clock_time),
                    execution_completed_timestamp: increment_clock(&mut clock_time),
                    output_upload_start_timestamp: increment_clock(&mut clock_time),
                    output_upload_completed_timestamp: increment_clock(&mut clock_time),
                    worker_completed_timestamp: increment_clock(&mut clock_time),
                },
                error: None,
                message: String::new(),
            }
        );
        Ok(())
    }

    #[nativelink_test]
    async fn running_actions_manager_respects_action_timeout()
    -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        // Ignore the sleep and immediately timeout.
        static ACTION_TIMEOUT: i64 = 1;
        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_work_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                // If action_timeout is the passed duration then return immediately,
                // which will cause the action to be killed and pass the test,
                // otherwise return pending and fail the test.
                sleep_fn: |duration| {
                    assert_eq!(duration.as_secs(), ACTION_TIMEOUT as u64);
                    Box::pin(future::ready(()))
                },
            },
        )?);
        #[cfg(target_family = "unix")]
        let arguments = vec!["sh".to_string(), "-c".to_string(), "sleep 2".to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec![
            "cmd".to_string(),
            "/C".to_string(),
            "ping -n 99999 127.0.0.1".to_string(),
        ];
        let command = Command {
            arguments,
            working_directory: ".".to_string(),
            environment_variables: vec![EnvironmentVariable {
                name: "PATH".to_string(),
                value: env::var("PATH").unwrap(),
            }],
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(
            &command,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let input_root_digest = serialize_and_upload_message(
            &Directory::default(),
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            platform: Some(Platform {
                properties: vec![Property {
                    name: "property_name".into(),
                    value: "property_value".into(),
                }],
            }),
            timeout: Some(prost_types::Duration {
                seconds: ACTION_TIMEOUT,
                nanos: 0,
            }),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(
            &action,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let execute_request = ExecuteRequest {
            action_digest: Some(action_digest.into()),
            ..Default::default()
        };
        let operation_id = OperationId::default().to_string();

        let running_action_impl = running_actions_manager
            .clone()
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id,
                    queued_timestamp: Some(make_system_time(1000).into()),
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: Vec::new(),
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await?;

        let result = run_action(running_action_impl).await?;

        #[cfg(target_family = "unix")]
        assert_eq!(result.exit_code, 9, "Action process should be been killed");
        #[cfg(target_family = "windows")]
        assert_eq!(result.exit_code, 1, "Action process should be been killed");
        Ok(())
    }

    #[nativelink_test]
    async fn test_handles_stale_directory_on_retry() -> Result<(), Error> {
        const WORKER_ID: &str = "foo_worker_id";
        let (_, ac_store, cas_store, _) = setup_stores().await?;
        let root_action_directory = make_temp_path("retry_work_directory");

        // Ensure root directory exists
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration {
                    entrypoint: None,
                    additional_environment: None,
                },
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            })?);

        // Create a simple action
        let command = Command {
            arguments: vec!["echo".to_string(), "test".to_string()],
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(
            &command,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let input_root_digest = serialize_and_upload_message(
            &Directory::default(),
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(
            &action,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let execute_request = ExecuteRequest {
            action_digest: Some(action_digest.into()),
            ..Default::default()
        };

        // Use a fixed operation ID to simulate retry with same ID
        let operation_id = "test-retry-operation-fixed-id".to_string();

        // Create the directory manually to simulate a previous failed action
        let action_directory = format!("{root_action_directory}/{operation_id}");
        eprintln!("Creating directory: {action_directory}");
        fs::create_dir_all(&action_directory).await?;

        // Also create the work subdirectory to ensure conflict
        let work_directory = format!("{action_directory}/work");
        fs::create_dir_all(&work_directory).await?;

        // Add a marker file to detect if directory is deleted and recreated
        let marker_file = format!("{action_directory}/marker.txt");
        tokio::fs::write(&marker_file, "test").await?;

        // Verify the directory was created
        assert!(
            tokio::fs::metadata(&action_directory).await.is_ok(),
            "Directory should exist"
        );
        assert!(
            tokio::fs::metadata(&work_directory).await.is_ok(),
            "Work directory should exist"
        );
        assert!(
            tokio::fs::metadata(&marker_file).await.is_ok(),
            "Marker file should exist"
        );

        // Now try to create an action with the same operation ID
        // This should fail with "File exists" error
        eprintln!("Attempting to create action with existing directory...");
        let result = running_actions_manager
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id: operation_id.clone(),
                    queued_timestamp: Some(SystemTime::now().into()),
                    platform: None,
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: Vec::new(),
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await;

        // Verify the behavior - with the fix, it should succeed after removing stale directory
        match result {
            Ok(_) => {
                // Check if the directory still exists and if marker file is gone
                let dir_exists = tokio::fs::metadata(&action_directory).await.is_ok();
                let marker_exists = tokio::fs::metadata(&marker_file).await.is_ok();
                eprintln!(
                    "SUCCESS: Directory collision handled gracefully. Directory exists: {dir_exists}, Marker exists: {marker_exists}"
                );
                assert!(
                    dir_exists,
                    "Directory should exist after successful creation"
                );
                assert!(
                    !marker_exists,
                    "Marker file should be gone - stale directory was cleaned up"
                );
                eprintln!(
                    "PASSED: The fix is working - stale directory was removed and action proceeded"
                );
            }
            Err(err) => {
                panic!("Expected success after fix, but got error: {err}");
            }
        }

        // Clean up
        fs::remove_dir_all(&root_action_directory).await?;
        Ok(())
    }

    #[nativelink_test]
    async fn test_retry_after_cleanup_succeeds() -> Result<(), Error> {
        const WORKER_ID: &str = "foo_worker_id";
        let (_, ac_store, cas_store, _) = setup_stores().await?;
        let root_action_directory = make_temp_path("retry_after_cleanup_work_directory");

        // Ensure root directory exists
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration {
                    entrypoint: None,
                    additional_environment: None,
                },
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map: None,
            })?);

        // Create a simple action
        let command = Command {
            arguments: vec!["echo".to_string(), "test".to_string()],
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(
            &command,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let input_root_digest = serialize_and_upload_message(
            &Directory::default(),
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(
            &action,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let execute_request = ExecuteRequest {
            action_digest: Some(action_digest.into()),
            ..Default::default()
        };

        let operation_id = "test-retry-after-cleanup-fixed-id".to_string();

        // First, create and execute an action
        let action1 = running_actions_manager
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request.clone()),
                    operation_id: operation_id.clone(),
                    queued_timestamp: Some(SystemTime::now().into()),
                    platform: None,
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: Vec::new(),
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await?;

        // Clean up the action
        action1.cleanup().await?;

        // Give cleanup a moment to complete
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Now try to create another action with the same operation ID
        // This should succeed because the directory has been cleaned up
        let result = running_actions_manager
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id: operation_id.clone(),
                    queued_timestamp: Some(SystemTime::now().into()),
                    platform: None,
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: Vec::new(),
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await;

        assert!(
            result.is_ok(),
            "Expected success when creating action after cleanup, got: {:?}",
            result.err()
        );

        // Clean up
        if let Ok(action2) = result {
            action2.cleanup().await?;
        }
        fs::remove_dir_all(&root_action_directory).await?;
        Ok(())
    }

    /// Helper: set up a RunningActionsManagerImpl with stores, a root directory,
    /// and a minimal action (empty command + empty input root) uploaded to the CAS.
    /// Returns (manager, execute_request, action) for use in peer hint tests.
    async fn setup_peer_hint_test(
        peer_locality_map: Option<nativelink_util::blob_locality_map::SharedBlobLocalityMap>,
    ) -> Result<
        (
            Arc<RunningActionsManagerImpl>,
            ExecuteRequest,
            Action,
            String,
        ),
        Box<dyn core::error::Error>,
    > {
        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config:
                    &nativelink_config::cas_server::UploadActionResultConfig {
                        upload_ac_results_strategy:
                            nativelink_config::cas_server::UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                peer_locality_map,
            })?);

        // Upload a minimal command + empty input root + action to CAS.
        #[cfg(target_family = "unix")]
        let arguments = vec![
            "sh".to_string(),
            "-c".to_string(),
            "true".to_string(),
        ];
        #[cfg(target_family = "windows")]
        let arguments = vec![
            "cmd".to_string(),
            "/C".to_string(),
            "echo ok".to_string(),
        ];

        let command = Command {
            arguments,
            output_paths: vec![],
            working_directory: ".".to_string(),
            environment_variables: vec![EnvironmentVariable {
                name: "PATH".to_string(),
                value: env::var("PATH").unwrap(),
            }],
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(
            &command,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let input_root_digest = serialize_and_upload_message(
            &Directory::default(),
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(
            &action,
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let execute_request = ExecuteRequest {
            action_digest: Some(action_digest.into()),
            ..Default::default()
        };

        Ok((
            running_actions_manager,
            execute_request,
            action,
            root_action_directory,
        ))
    }

    #[nativelink_test]
    async fn test_peer_hints_registered_in_locality_map(
    ) -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "peer_hint_worker";

        let locality_map = new_shared_blob_locality_map();
        let (running_actions_manager, execute_request, action, root_action_directory) =
            setup_peer_hint_test(Some(locality_map.clone())).await?;

        let d1 = DigestInfo::new([0xAA; 32], 1000);
        let d1_proto: Digest = d1.into();

        let running_action = running_actions_manager
            .clone()
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id: OperationId::default().to_string(),
                    queued_timestamp: Some(make_system_time(1000).into()),
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: vec![PeerHint {
                        digest: Some(d1_proto),
                        peer_endpoints: vec!["worker-a:50081".to_string()],
                    }],
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await?;

        // Verify the locality map was populated.
        {
            let map = locality_map.read();
            let workers = map.lookup_workers(&d1);
            assert_eq!(workers.len(), 1, "Expected 1 endpoint for d1");
            assert_eq!(&*workers[0], "worker-a:50081");
        }

        // Clean up.
        running_action.cleanup().await?;
        fs::remove_dir_all(&root_action_directory).await?;
        Ok(())
    }

    #[nativelink_test]
    async fn test_empty_peer_hints_no_error() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "empty_hints_worker";

        let locality_map = new_shared_blob_locality_map();
        let (running_actions_manager, execute_request, action, root_action_directory) =
            setup_peer_hint_test(Some(locality_map.clone())).await?;

        let running_action = running_actions_manager
            .clone()
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id: OperationId::default().to_string(),
                    queued_timestamp: Some(make_system_time(1000).into()),
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: Vec::new(),
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await?;

        // Locality map should be empty.
        {
            let map = locality_map.read();
            assert_eq!(map.digest_count(), 0, "Expected no digests in locality map");
            assert_eq!(
                map.endpoint_count(),
                0,
                "Expected no endpoints in locality map"
            );
        }

        running_action.cleanup().await?;
        fs::remove_dir_all(&root_action_directory).await?;
        Ok(())
    }

    #[nativelink_test]
    async fn test_peer_hints_without_locality_map() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "no_map_worker";

        // Pass None for peer_locality_map.
        let (running_actions_manager, execute_request, action, root_action_directory) =
            setup_peer_hint_test(None).await?;

        let d1 = DigestInfo::new([0xBB; 32], 500);
        let d1_proto: Digest = d1.into();

        // Should not panic or error even though peer_hints are provided.
        let running_action = running_actions_manager
            .clone()
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id: OperationId::default().to_string(),
                    queued_timestamp: Some(make_system_time(1000).into()),
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: vec![PeerHint {
                        digest: Some(d1_proto),
                        peer_endpoints: vec!["worker-x:50081".to_string()],
                    }],
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await?;

        running_action.cleanup().await?;
        fs::remove_dir_all(&root_action_directory).await?;
        Ok(())
    }

    #[nativelink_test]
    async fn test_multiple_endpoints_per_hint() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "multi_endpoint_worker";

        let locality_map = new_shared_blob_locality_map();
        let (running_actions_manager, execute_request, action, root_action_directory) =
            setup_peer_hint_test(Some(locality_map.clone())).await?;

        let d1 = DigestInfo::new([0xCC; 32], 2000);
        let d1_proto: Digest = d1.into();

        let running_action = running_actions_manager
            .clone()
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id: OperationId::default().to_string(),
                    queued_timestamp: Some(make_system_time(1000).into()),
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                    peer_hints: vec![PeerHint {
                        digest: Some(d1_proto),
                        peer_endpoints: vec![
                            "worker-a:50081".to_string(),
                            "worker-b:50081".to_string(),
                        ],
                    }],
                    resolved_directories: Vec::new(),
                    resolved_directory_digests: Vec::new(),
                },
            )
            .await?;

        // Both endpoints should be registered for d1.
        {
            let map = locality_map.read();
            let workers = map.lookup_workers(&d1);
            assert_eq!(workers.len(), 2, "Expected 2 endpoints for d1");
            assert!(
                workers.iter().any(|w| &**w == "worker-a:50081"),
                "Expected worker-a:50081 in endpoints"
            );
            assert!(
                workers.iter().any(|w| &**w == "worker-b:50081"),
                "Expected worker-b:50081 in endpoints"
            );
        }

        running_action.cleanup().await?;
        fs::remove_dir_all(&root_action_directory).await?;
        Ok(())
    }
}
