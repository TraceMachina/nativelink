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
    use std::path::PathBuf;
    use std::sync::{Arc, LazyLock, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use bytes::Bytes;
    use futures::future::join_all;
    use futures::prelude::*;
    use nativelink_config::cas_server::{
        EnvironmentSource, UploadActionResultConfig, UploadCacheResultsStrategy,
    };
    use nativelink_config::stores::{
        FastSlowSpec, FilesystemSpec, MemorySpec, StoreDirection, StoreSpec,
    };
    use nativelink_error::{Code, Error, ResultExt, make_input_err};
    use nativelink_macro::nativelink_test;
    use nativelink_proto::build::bazel::remote::execution::v2::command::EnvironmentVariable;
    #[cfg_attr(target_family = "windows", allow(unused_imports))]
    use nativelink_proto::build::bazel::remote::execution::v2::{
        Action, ActionResult as ProtoActionResult, Command, Directory, DirectoryNode,
        ExecuteRequest, ExecuteResponse, FileNode, NodeProperties, Platform, SymlinkNode, Tree,
        digest_function::Value as ProtoDigestFunction, platform::Property,
    };
    use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
        HistoricalExecuteResponse, StartExecute,
    };
    use nativelink_proto::google::rpc::Status;
    #[cfg(target_family = "unix")]
    use nativelink_store::ac_utils::compute_buf_digest;
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
    use nativelink_util::common::{DigestInfo, fs, make_temp_path};
    use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
    use nativelink_util::spawn;
    use nativelink_util::store_trait::{Store, StoreLike};
    #[cfg(target_os = "linux")]
    use nativelink_worker::namespace_utils;
    use nativelink_worker::running_actions_manager::{
        Callbacks, ExecutionConfiguration, RunningAction, RunningActionImpl, RunningActionsManager,
        RunningActionsManagerArgs, RunningActionsManagerImpl, download_to_directory,
    };
    use pretty_assertions::assert_eq;
    use prost::Message;
    use tokio::sync::oneshot;
    use tracing::info;

    const DEFAULT_MAX_UPLOAD_TIMEOUT: u64 = 600;

    #[cfg(target_os = "linux")]
    fn use_namespaces() -> nativelink_worker::running_actions_manager::UseNamespaces {
        if namespace_utils::namespaces_supported(true) {
            nativelink_worker::running_actions_manager::UseNamespaces::YesAndMount
        } else if namespace_utils::namespaces_supported(false) {
            nativelink_worker::running_actions_manager::UseNamespaces::Yes
        } else {
            nativelink_worker::running_actions_manager::UseNamespaces::No
        }
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
                bypass_dedup_threshold_bytes: 0,
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

    /// Waits for a background cleanup to remove `path`. The removal happens on
    /// the blocking pool, so yielding to the scheduler does not order against
    /// it and the wait has to be a real one.
    async fn wait_for_removal(path: &str) {
        for _ in 0..1000 {
            if tokio::fs::metadata(path).await.is_err() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("{path} was never removed");
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

    #[nativelink_test]
    async fn download_to_directory_zero_digest_empty_file_test()
    -> Result<(), Box<dyn core::error::Error>> {
        // Regression test: zero-digest files used to fall through a
        // synthetic FileEntry path that pointed at a non-existent content
        // path on disk. The worker's prefetched-hardlink path silently
        // failed to materialise the empty file. Verify that an empty file
        // declared as part of an input directory now lands on disk at the
        // expected location with zero bytes — exercising the
        // FilesystemStore + running_actions_manager output-materialisation
        // path. Complements PR #2338's DirectoryCache zero-byte test
        // which covers a different code path (the input directory cache
        // short-circuit).
        const EMPTY_FILE_NAME: &str = "empty.txt";
        const SECOND_EMPTY_FILE_NAME: &str = "also_empty.log";
        const NESTED_EMPTY_FILE_NAME: &str = "nested_empty";
        const NESTED_DIR_NAME: &str = "subdir";
        const NON_EMPTY_FILE_NAME: &str = "non_empty.txt";
        const NON_EMPTY_CONTENT: &str = "non-empty";

        let (fast_store, slow_store, cas_store, _ac_store) = setup_stores().await?;

        // SHA-256 of empty content.
        let zero_digest = DigestInfo::try_new(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            0,
        )?;
        let non_empty_digest = DigestInfo::new([7u8; 32], NON_EMPTY_CONTENT.len() as u64);
        slow_store
            .as_ref()
            .update_oneshot(non_empty_digest, NON_EMPTY_CONTENT.into())
            .await?;

        // A nested subdirectory containing yet another zero-digest file —
        // confirms the recursive download path also takes the empty-file
        // branch (i.e. NotFound from get_file_entry_for_digest is handled
        // by every caller, not just the root). The cas_store is a
        // FastSlowStore, so the subdir Directory proto must live in slow.
        let nested_dir_digest = DigestInfo::new([9u8; 32], 96);
        let nested_dir = Directory {
            files: vec![FileNode {
                name: NESTED_EMPTY_FILE_NAME.to_string(),
                digest: Some(zero_digest.into()),
                ..Default::default()
            }],
            ..Default::default()
        };
        slow_store
            .as_ref()
            .update_oneshot(nested_dir_digest, nested_dir.encode_to_vec().into())
            .await?;

        let root_directory_digest = DigestInfo::new([8u8; 32], 64);
        let root_directory = Directory {
            files: vec![
                FileNode {
                    name: EMPTY_FILE_NAME.to_string(),
                    digest: Some(zero_digest.into()),
                    ..Default::default()
                },
                // Second zero-digest at the same level — proves the path
                // is not single-use / not dependent on any one filename.
                FileNode {
                    name: SECOND_EMPTY_FILE_NAME.to_string(),
                    digest: Some(zero_digest.into()),
                    ..Default::default()
                },
                FileNode {
                    name: NON_EMPTY_FILE_NAME.to_string(),
                    digest: Some(non_empty_digest.into()),
                    ..Default::default()
                },
            ],
            directories: vec![DirectoryNode {
                name: NESTED_DIR_NAME.to_string(),
                digest: Some(nested_dir_digest.into()),
            }],
            ..Default::default()
        };
        slow_store
            .as_ref()
            .update_oneshot(root_directory_digest, root_directory.encode_to_vec().into())
            .await?;

        let download_dir = make_temp_path("download_dir");
        fs::create_dir_all(&download_dir).await?;
        // The whole download succeeding is itself the strongest assertion
        // that NotFound from get_file_entry_for_digest is HANDLED by the
        // download_to_directory caller — if NotFound propagated it would
        // surface as an Err here.
        download_to_directory(
            cas_store.as_ref(),
            fast_store.as_pin(),
            &root_directory_digest,
            &download_dir,
        )
        .await?;

        // All three zero-digest files must exist on disk as regular files
        // with exactly zero bytes — strict assertions so a silent
        // regression (missing file, wrong type, non-zero length) is
        // impossible.
        for relative in [
            EMPTY_FILE_NAME.to_string(),
            SECOND_EMPTY_FILE_NAME.to_string(),
            format!("{NESTED_DIR_NAME}/{NESTED_EMPTY_FILE_NAME}"),
        ] {
            let path = format!("{download_dir}/{relative}");
            let meta = fs::metadata(&path)
                .await
                .err_tip(|| format!("Expected zero-digest file to be materialised at {path}"))?;
            assert!(meta.is_file(), "{path} must be a regular file");
            assert!(!meta.is_symlink(), "{path} must not be a symlink");
            assert_eq!(meta.len(), 0, "{path} must be exactly zero bytes");
            // Read back to confirm it is actually readable (not a phantom
            // dirent) and truly empty.
            let bytes = fs::read(&path).await?;
            assert!(bytes.is_empty(), "{path} must read back as empty");
        }

        // Sanity-check the non-zero-digest path still works.
        let non_empty_path = format!("{download_dir}/{NON_EMPTY_FILE_NAME}");
        let non_empty_bytes = fs::read(&non_empty_path).await?;
        assert_eq!(from_utf8(&non_empty_bytes)?, NON_EMPTY_CONTENT);
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                digest_function: ProtoDigestFunction::Sha256.into(),
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                digest_function: ProtoDigestFunction::Sha256.into(),
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

        let (_, slow_store, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                    },
                )
                .await?;

            run_action(running_action_impl.clone()).await?
        };
        let file_content = slow_store
            .as_ref()
            .get_part_unchunked(action_result.output_files[0].digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&file_content)?, "123 ");
        let stdout_content = slow_store
            .as_ref()
            .get_part_unchunked(action_result.stdout_digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&stdout_content)?, "foo-stdout ");
        let stderr_content = slow_store
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

        let (_, slow_store, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                digest_function: ProtoDigestFunction::Sha256.into(),
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
                    },
                )
                .await?;

            run_action(running_action_impl.clone()).await?
        };
        let file_content = slow_store
            .as_ref()
            .get_part_unchunked(action_result.output_files[0].digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&file_content)?, "123 ");
        let stdout_content = slow_store
            .as_ref()
            .get_part_unchunked(action_result.stdout_digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&stdout_content)?, "foo-stdout ");
        let stderr_content = slow_store
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

        let (_, slow_store, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                    "mkdir -p dir1/dir2 && \
                         echo foo > dir1/file && \
                         touch dir1/file2 && \
                         ln -s ../file dir1/dir2/sym && \
                         ln -s dir1/file rel_sym && \
                         ln -s /dev/null empty_sym"
                        .to_string(),
                ],
                // `dir1` exercises the directory upload path,
                // `rel_sym` exercises the relative-symlink-preserved path,
                // `empty_sym` exercises the absolute-symlink-resolved path
                // against `/dev/null`. Pre-fix this test asserted `empty_sym`
                // was kept as a `SymlinkInfo` with target `/dev/null`; that
                // behavior is now incorrect because absolute symlinks are
                // worker-local and must be resolved before upload. Reading
                // `/dev/null` returns 0 bytes immediately by its character-
                // device contract, so the worker produces an empty-file
                // output with the canonical sha256 empty digest.
                output_paths: vec![
                    "dir1".to_string(),
                    "empty_sym".to_string(),
                    "rel_sym".to_string(),
                ],
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
                digest_function: ProtoDigestFunction::Sha256.into(),
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
                    },
                )
                .await?;

            run_action(running_action_impl.clone()).await?
        };
        let tree = get_and_decode_digest::<Tree>(
            slow_store.as_ref(),
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
                // `empty_sym` was an absolute symlink — the worker resolves
                // it and uploads the underlying (empty) file. The resulting
                // digest is the well-known sha256 of zero bytes.
                output_files: vec![FileInfo {
                    name_or_path: NameOrPath::Path("empty_sym".to_string()),
                    digest: DigestInfo::try_new(
                        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
                        0,
                    )?,
                    is_executable: false,
                }],
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
                    name_or_path: NameOrPath::Path("rel_sym".to_string()),
                    target: "dir1/file".to_string(),
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

    // The fixture is built with the `ln -s` shell builtin (matching the
    // convention used by `upload_dir_and_symlink_test` above), so this test
    // only runs on Unix-like platforms. Windows itself does support symlinks
    // via `std::os::windows::fs::{symlink_file, symlink_dir}`, but creating
    // them from a shell isn't portable.
    #[cfg(not(target_family = "windows"))]
    #[nativelink_test]
    async fn upload_absolute_symlink_resolves_contents() -> Result<(), Box<dyn core::error::Error>>
    {
        // Regression test: when an action produces an absolute symlink as one
        // of its declared outputs (for example because the work directory was
        // populated via DirectoryCache and contains absolute symlinks into the
        // cache), uploading the literal symlink target yields a path that is
        // meaningless on the client. The worker must resolve absolute
        // symlinks and upload the underlying file/directory contents.
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, slow_store, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        // Create an out-of-tree file whose absolute path the action will
        // symlink into the work directory.
        let external_root = make_temp_path("external_payload");
        fs::create_dir_all(&external_root).await?;
        let external_file = format!("{external_root}/payload.txt");
        tokio::fs::write(&external_file, b"hello-from-outside").await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                    format!("ln -s {external_file} resolved_output"),
                ],
                output_paths: vec!["resolved_output".to_string()],
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
                digest_function: ProtoDigestFunction::Sha256.into(),
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
                    },
                )
                .await?;

            run_action(running_action_impl.clone()).await?
        };

        // With the fix, the absolute symlink should be resolved and the
        // payload uploaded as a regular file with the payload's content
        // hash. The output_file_symlinks list must be empty.
        assert_eq!(
            action_result.output_file_symlinks.len(),
            0,
            "absolute symlink should be resolved to file, not uploaded as symlink"
        );
        assert_eq!(
            action_result.output_directory_symlinks.len(),
            0,
            "no directory symlinks expected"
        );
        assert_eq!(
            action_result.output_files.len(),
            1,
            "absolute symlink should appear as an uploaded file"
        );
        let uploaded = &action_result.output_files[0];
        assert_eq!(
            uploaded.name_or_path,
            NameOrPath::Path("resolved_output".to_string())
        );
        assert_eq!(
            usize::try_from(uploaded.digest.size_bytes())?,
            b"hello-from-outside".len()
        );
        // Verify the blob actually landed in CAS by re-reading it.
        let key: nativelink_util::store_trait::StoreKey<'_> = uploaded.digest.into();
        let blob = slow_store.as_ref().get_part_unchunked(key, 0, None).await?;
        assert_eq!(blob.as_ref(), b"hello-from-outside");
        Ok(())
    }

    // The fixture is built with the `ln -s` shell builtin (matching the
    // convention used by `upload_dir_and_symlink_test` above), so this test
    // only runs on Unix-like platforms. Windows itself does support symlinks
    // via `std::os::windows::fs::{symlink_file, symlink_dir}`, but creating
    // them from a shell isn't portable.
    #[cfg(not(target_family = "windows"))]
    #[nativelink_test]
    async fn upload_absolute_symlink_to_directory_uploads_tree()
    -> Result<(), Box<dyn core::error::Error>> {
        // Regression test (companion to upload_absolute_symlink_resolves_contents):
        // exercises the directory branch. When an absolute symlink points
        // at a directory, the worker must walk it and upload a Tree proto
        // — NOT preserve the symlink. The previous implementation produced
        // an OutputType::DirectorySymlink with a worker-local absolute
        // target that is meaningless on the client.
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, slow_store, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        // Out-of-tree directory the action will absolute-symlink to.
        let external_root = make_temp_path("external_dir_payload");
        fs::create_dir_all(&external_root).await?;
        tokio::fs::write(format!("{external_root}/inner.txt"), b"inner-payload").await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                    format!("ln -s {external_root} resolved_dir"),
                ],
                output_paths: vec!["resolved_dir".to_string()],
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
                digest_function: ProtoDigestFunction::Sha256.into(),
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
                    },
                )
                .await?;
            run_action(running_action_impl.clone()).await?
        };

        // The absolute directory symlink must be resolved into a Tree, not
        // preserved as DirectorySymlink/FileSymlink.
        assert_eq!(
            action_result.output_directory_symlinks.len(),
            0,
            "absolute dir symlink must be resolved, not uploaded as DirectorySymlink"
        );
        assert_eq!(
            action_result.output_file_symlinks.len(),
            0,
            "absolute dir symlink must not appear as FileSymlink either"
        );
        assert_eq!(
            action_result.output_files.len(),
            0,
            "directory target should not surface as an output file"
        );
        assert_eq!(
            action_result.output_folders.len(),
            1,
            "absolute dir symlink should produce a single output_folders entry"
        );
        let folder = &action_result.output_folders[0];
        assert_eq!(folder.path, "resolved_dir");
        // Walk the uploaded Tree and confirm inner.txt is present with
        // correct content. This proves the directory was actually walked
        // and uploaded — not a stub.
        let tree =
            get_and_decode_digest::<Tree>(slow_store.as_ref(), folder.tree_digest.into()).await?;
        let root = tree.root.expect("Tree must have a root Directory");
        let inner = root
            .files
            .iter()
            .find(|f| f.name == "inner.txt")
            .expect("inner.txt must be in uploaded Tree root");
        let inner_digest: DigestInfo = inner
            .digest
            .clone()
            .expect("inner.txt must have a digest")
            .try_into()?;
        let key: nativelink_util::store_trait::StoreKey<'_> = inner_digest.into();
        let blob = slow_store.as_ref().get_part_unchunked(key, 0, None).await?;
        assert_eq!(blob.as_ref(), b"inner-payload");
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                digest_function: ProtoDigestFunction::Sha256.into(),
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                    format!("touch {process_started_file} && sleep 24h"),
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
            digest_function: ProtoDigestFunction::Sha256.into(),
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
            digest_function: ProtoDigestFunction::Sha256.into(),
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                seconds: TASK_TIMEOUT.as_secs().try_into().unwrap_or(i64::MAX),
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
            digest_function: ProtoDigestFunction::Sha256.into(),
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
            digest_function: ProtoDigestFunction::Sha256.into(),
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
                },
            )
            .await?;

        let result = run_action(running_action_impl).await?;
        assert_eq!(result.exit_code, 1, "Exit code should be 1");
        assert_eq!(
            result.error.err_tip(|| "Error should exist")?.code,
            Code::DeadlineExceeded
        );
        assert!(logs_contain(
            "Command returned non-zero exit code exit_code=1 stdout=\"\" stderr=\"\" command=[\"true\"]"
        ));
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::SuccessOnly,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Everything,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_historical_results_strategy: Some(
                        UploadCacheResultsStrategy::SuccessOnly,
                    ),
                    #[expect(
                        clippy::literal_string_with_formatting_args,
                        reason = "passed to `formatx` crate for runtime interpretation"
                    )]
                    success_message_template: "{historical_results_hash}-{historical_results_size}"
                        .to_string(),
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_historical_results_strategy: Some(
                        UploadCacheResultsStrategy::SuccessOnly,
                    ),
                    success_message_template: "{historical_results_hash}-{historical_results_size}"
                        .to_string(),
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_historical_results_strategy: Some(
                        UploadCacheResultsStrategy::FailuresOnly,
                    ),
                    #[expect(
                        clippy::literal_string_with_formatting_args,
                        reason = "passed to `formatx` crate for runtime interpretation"
                    )]
                    failure_message_template: "{historical_results_hash}-{historical_results_size}"
                        .to_string(),
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::SuccessOnly,
                    success_message_template: "{action_digest_hash}-{action_digest_size}"
                        .to_string(),
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                    seconds: TASK_TIMEOUT.as_secs().try_into().unwrap_or(i64::MAX),
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
                    upload_action_result_config: &UploadActionResultConfig {
                        upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                    max_action_timeout: MAX_TIMEOUT_DURATION,
                    max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                    timeout_handled_externally: false,
                    directory_cache: None,
                    #[cfg(target_os = "linux")]
                    use_namespaces: use_namespaces(),
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
                digest_function: ProtoDigestFunction::Sha256.into(),
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
                    seconds: TASK_TIMEOUT.as_secs().try_into().unwrap_or(i64::MAX),
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
                    upload_action_result_config: &UploadActionResultConfig {
                        upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                    max_action_timeout: MAX_TIMEOUT_DURATION,
                    max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                    timeout_handled_externally: false,
                    directory_cache: None,
                    #[cfg(target_os = "linux")]
                    use_namespaces: use_namespaces(),
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
                digest_function: ProtoDigestFunction::Sha256.into(),
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
                    seconds: TASK_TIMEOUT.as_secs().try_into().unwrap_or(i64::MAX),
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
                    upload_action_result_config: &UploadActionResultConfig {
                        upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                        ..Default::default()
                    },
                    max_action_timeout: MAX_TIMEOUT_DURATION,
                    max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                    timeout_handled_externally: false,
                    directory_cache: None,
                    #[cfg(target_os = "linux")]
                    use_namespaces: use_namespaces(),
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
                digest_function: ProtoDigestFunction::Sha256.into(),
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
        let arguments = vec!["sh".to_string(), "-c".to_string(), "sleep 24h".to_string()];
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
            digest_function: ProtoDigestFunction::Sha256.into(),
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
        let command = "[\"sh\", \"-c\", \"sleep 24h\"]";
        #[cfg(target_family = "windows")]
        let command = "[\"cmd\", \"/C\", \"ping -n 99999 127.0.0.1\"]";

        assert!(logs_contain(&format!("Executing command args={command}")));
        assert!(logs_contain(&format!("Command complete args={command}")));

        assert!(!logs_contain(
            "Child process was not cleaned up before dropping the call to execute(), killing in background spawn"
        ));
        #[cfg(target_family = "unix")]
        assert!(logs_contain(
            "Command timed out seconds=0.0 command=sh -c sleep 24h"
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);

        #[cfg(target_family = "unix")]
        let arguments = vec!["sh".to_string(), "-c".to_string(), "sleep 24h".to_string()];
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
            digest_function: ProtoDigestFunction::Sha256.into(),
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                digest_function: ProtoDigestFunction::Sha256.into(),
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

    /// Regression for skipping the `pre_exec` hook when namespaces are off.
    /// With namespaces disabled (the default) the action spawn goes through
    /// `posix_spawn` instead of `fork`, and `process_group(0)` must still make
    /// the child its own process-group leader (pgid == pid).
    #[cfg(target_os = "linux")]
    #[nativelink_test]
    async fn no_namespace_action_is_process_group_leader() -> Result<(), Box<dyn core::error::Error>>
    {
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
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                execution_configuration: ExecutionConfiguration::default(),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                // Pin namespaces off so this exercises the no-pre_exec/posix_spawn
                // path regardless of what the host kernel supports.
                use_namespaces: nativelink_worker::running_actions_manager::UseNamespaces::No,
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);

        // Print the shell's own pid (field 1) and process-group id (field 5)
        // from its /proc stat line; process_group(0) makes them equal.
        let command = Command {
            arguments: vec![
                "sh".to_string(),
                "-c".to_string(),
                "read -r pid _ _ _ pgrp _ < /proc/$$/stat; printf '%s %s' \"$pid\" \"$pgrp\""
                    .to_string(),
            ],
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
            digest_function: ProtoDigestFunction::Sha256.into(),
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

        let action_result = run_action(running_action_impl.clone()).await?;
        assert_eq!(
            action_result.exit_code, 0,
            "action should run to completion via posix_spawn"
        );

        let stdout = cas_store
            .as_ref()
            .get_part_unchunked(action_result.stdout_digest, 0, None)
            .await?;
        let stdout = from_utf8(&stdout)?;
        let (pid, pgrp) = stdout.split_once(' ').expect("expected 'pid pgrp' output");
        assert_eq!(
            pid, pgrp,
            "spawned process should be its own process-group leader (pid={pid} pgrp={pgrp})"
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
            digest_function: ProtoDigestFunction::Sha256.into(),
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

    #[nativelink_test]
    #[cfg(target_family = "unix")]
    async fn persistent_worker_reuses_json_worker_between_actions()
    -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";
        const WORKER_SCRIPT_NAME: &str = "worker.sh";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        async fn create_action(
            cas_store: &Arc<FastSlowStore>,
            worker_script_digest: DigestInfo,
        ) -> Result<(Action, DigestInfo), Error> {
            let command = Command {
                arguments: vec![format!("./{WORKER_SCRIPT_NAME}"), "@args.txt".to_string()],
                output_paths: vec!["count.txt".to_string()],
                platform: Some(Platform {
                    properties: vec![
                        Property {
                            name: "supports-workers".to_string(),
                            value: "1".to_string(),
                        },
                        Property {
                            name: "requires-worker-protocol".to_string(),
                            value: "json".to_string(),
                        },
                    ],
                }),
                ..Default::default()
            };
            let command_digest = serialize_and_upload_message(
                &command,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let args_digest = DigestInfo::new([9u8; 32], 0);
            cas_store.update_oneshot(args_digest, Bytes::new()).await?;
            let input_root_digest = serialize_and_upload_message(
                &Directory {
                    files: vec![
                        FileNode {
                            name: WORKER_SCRIPT_NAME.to_string(),
                            digest: Some(worker_script_digest.into()),
                            is_executable: true,
                            node_properties: None,
                        },
                        FileNode {
                            name: "args.txt".to_string(),
                            digest: Some(args_digest.into()),
                            is_executable: false,
                            node_properties: None,
                        },
                    ],
                    ..Default::default()
                },
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                platform: command.platform.clone(),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(
                &action,
                cas_store.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
            Ok((action, action_digest))
        }

        async fn run_persistent_action(
            running_actions_manager: &Arc<RunningActionsManagerImpl>,
            slow_store: &Arc<MemoryStore>,
            action: &Action,
            action_digest: DigestInfo,
            operation_id: &str,
        ) -> Result<String, Error> {
            let running_action_impl = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(ExecuteRequest {
                            action_digest: Some(action_digest.into()),
                            digest_function: ProtoDigestFunction::Sha256.into(),
                            ..Default::default()
                        }),
                        operation_id: operation_id.to_string(),
                        queued_timestamp: None,
                        platform: action.platform.clone(),
                        worker_id: WORKER_ID.to_string(),
                    },
                )
                .await?;
            let action_result = run_action(running_action_impl).await?;
            slow_store
                .as_ref()
                .get_part_unchunked(action_result.output_files[0].digest, 0, None)
                .await
                .and_then(|content| {
                    String::from_utf8(content.to_vec()).map_err(|err| {
                        Error::from_std_err(Code::Internal, &err)
                            .append("Decoding persistent worker count.txt")
                    })
                })
        }

        let (_, slow_store, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::from_secs(30),
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);

        let worker_script = r#"#!/bin/sh
count=0
while IFS= read -r request; do
  if ! pwd -P >/dev/null 2>&1; then
    printf '{"exitCode":1,"output":"worker cwd was removed"}\n'
    continue
  fi
  count=$((count + 1))
  sandbox_dir=$(printf '%s' "$request" | sed -n 's/.*"sandboxDir":"\([^"]*\)".*/\1/p')
  printf '%s' "$count" > "$sandbox_dir/count.txt"
  printf '{"exitCode":0,"output":"count=%s"}\n' "$count"
done
"#;
        let worker_script_bytes = Bytes::from(worker_script);
        let worker_script_digest =
            compute_buf_digest(&worker_script_bytes, &mut DigestHasherFunc::Sha256.hasher());
        cas_store
            .update_oneshot(worker_script_digest, worker_script_bytes)
            .await?;
        let (action, action_digest) = create_action(&cas_store, worker_script_digest).await?;

        let first = run_persistent_action(
            &running_actions_manager,
            &slow_store,
            &action,
            action_digest,
            "persistent_worker_first",
        )
        .await?;
        let second = run_persistent_action(
            &running_actions_manager,
            &slow_store,
            &action,
            action_digest,
            "persistent_worker_second",
        )
        .await?;

        assert_eq!(first, "1");
        assert_eq!(
            second, "2",
            "second action must reuse the already-running persistent worker"
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

        let (_, slow_store, cas_store, ac_store) = setup_stores().await?;
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
                digest_function: ProtoDigestFunction::Sha256.into(),
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
                    },
                )
                .await?;

            run_action(running_action_impl.clone()).await?
        };
        let file_content = slow_store
            .as_ref()
            .get_part_unchunked(action_result.output_files[0].digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&file_content)?, "123 ");
        let stdout_content = slow_store
            .as_ref()
            .get_part_unchunked(action_result.stdout_digest, 0, None)
            .await?;
        assert_eq!(from_utf8(&stdout_content)?, "foo-stdout ");
        let stderr_content = slow_store
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
            digest_function: ProtoDigestFunction::Sha256.into(),
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
    async fn leftover_directory_does_not_collide_with_retry_test() -> Result<(), Error> {
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
            digest_function: ProtoDigestFunction::Sha256.into(),
            ..Default::default()
        };

        let operation_id = "test-retry-operation-fixed-id".to_string();

        // A directory left behind by a previous attempt of this operation,
        // named the way older versions named it (the bare operation id).
        let leftover_directory = format!("{root_action_directory}/{operation_id}");
        fs::create_dir_all(format!("{leftover_directory}/work")).await?;
        let marker_file = format!("{leftover_directory}/marker.txt");
        tokio::fs::write(&marker_file, "test").await?;

        // The attempt gets a directory of its own, so a leftover directory is
        // not a collision to resolve: nothing has to be removed to make room,
        // and nothing the previous attempt may still be using gets touched.
        let running_action = running_actions_manager
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id: operation_id.clone(),
                    queued_timestamp: Some(SystemTime::now().into()),
                    platform: None,
                    worker_id: WORKER_ID.to_string(),
                },
            )
            .await?;

        let action_directory = running_action
            .get_work_directory()
            .strip_suffix("/work")
            .expect("work directory should sit under the action directory")
            .to_string();
        assert_ne!(
            action_directory, leftover_directory,
            "attempt must not reuse a leftover attempt's directory"
        );
        assert!(
            tokio::fs::metadata(&action_directory).await.is_ok(),
            "action directory should exist after successful creation"
        );
        assert!(
            tokio::fs::metadata(&marker_file).await.is_ok(),
            "leftover directory should be left alone, not deleted out from under its owner"
        );

        running_action.cleanup().await?;
        assert!(
            tokio::fs::metadata(&action_directory).await.is_err(),
            "cleanup should remove this attempt's directory"
        );
        assert!(
            tokio::fs::metadata(&marker_file).await.is_ok(),
            "cleanup must not reach outside this attempt's directory"
        );

        fs::remove_dir_all(&root_action_directory).await?;
        Ok(())
    }

    /// A duplicate `StartAction` for an operation already running here must be
    /// rejected before it touches the filesystem. It used to be rejected only
    /// after `create_and_add_action` had already decided the live attempt's
    /// directory was stale and removed it, which deleted the running action's
    /// files out from under it.
    #[nativelink_test]
    async fn duplicate_start_does_not_disturb_live_action_test() -> Result<(), Error> {
        const WORKER_ID: &str = "foo_worker_id";
        let (_, ac_store, cas_store, _) = setup_stores().await?;
        let root_action_directory = make_temp_path("duplicate_start_work_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
            })?);

        let command = Command {
            arguments: vec!["true".to_string()],
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

        let operation_id = "duplicate-start-operation-id".to_string();
        let start_execute = StartExecute {
            execute_request: Some(ExecuteRequest {
                action_digest: Some(action_digest.into()),
                digest_function: ProtoDigestFunction::Sha256.into(),
                ..Default::default()
            }),
            operation_id: operation_id.clone(),
            queued_timestamp: Some(SystemTime::now().into()),
            platform: None,
            worker_id: WORKER_ID.to_string(),
        };

        // A live attempt, mid-run, with files it is relying on.
        let running_action = running_actions_manager
            .create_and_add_action(WORKER_ID.to_string(), start_execute.clone())
            .await?
            .prepare_action()
            .await?;
        let work_directory = running_action.get_work_directory().clone();
        let in_use_file = format!("{work_directory}/in_use.txt");
        tokio::fs::write(&in_use_file, "in use").await?;

        let err = running_actions_manager
            .create_and_add_action(WORKER_ID.to_string(), start_execute)
            .await
            .expect_err("duplicate StartAction should be rejected");
        assert_eq!(err.code, Code::AlreadyExists, "{err}");

        assert!(
            tokio::fs::metadata(&in_use_file).await.is_ok(),
            "duplicate StartAction deleted the live action's files"
        );

        running_action.cleanup().await?;
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
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
            digest_function: ProtoDigestFunction::Sha256.into(),
            ..Default::default()
        };

        let operation_id = "test-retry-after-cleanup-fixed-id".to_string();
        let execute_request2 = execute_request.clone();

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

        // An action abandoned before `prepare_action` still has a directory,
        // and nothing else ever sweeps action directories up, so dropping it
        // must clean up after itself.
        let action3 = running_actions_manager
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request2),
                    operation_id: operation_id.clone(),
                    queued_timestamp: Some(SystemTime::now().into()),
                    platform: None,
                    worker_id: WORKER_ID.to_string(),
                },
            )
            .await?;
        let action3_directory = action3
            .get_work_directory()
            .strip_suffix("/work")
            .expect("work directory should sit under the action directory")
            .to_string();
        assert!(tokio::fs::metadata(&action3_directory).await.is_ok());
        drop(action3);
        // Panics if it is never removed: abandoning an action before
        // `prepare_action` must not leak its directory.
        wait_for_removal(&action3_directory).await;

        fs::remove_dir_all(&root_action_directory).await?;
        Ok(())
    }

    #[nativelink_test]
    async fn test_canonical_path() -> Result<(), Error> {
        let (_fast_store, slow_store, cas_store, _ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;
        let running_actions_manager =
            Arc::new(RunningActionsManagerImpl::new(RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: None,
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
            })?);
        let operation_id = OperationId::default().to_string();
        let file_content_digest = DigestInfo::new([2u8; 32], 32);
        slow_store
            .as_ref()
            .update_oneshot(file_content_digest, "hello".into())
            .await?;
        let command = Command {
            arguments: vec!["touch".to_string(), "./some/path/test.txt".to_string()],
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
        let running_action_impl = running_actions_manager
            .create_and_add_action(
                "foo_worker_id".to_string(),
                StartExecute {
                    execute_request: Some(ExecuteRequest {
                        action_digest: Some(action_digest.into()),
                        digest_function: ProtoDigestFunction::Sha256.into(),
                        ..Default::default()
                    }),
                    operation_id,
                    queued_timestamp: None,
                    platform: action.platform.clone(),
                    worker_id: "foo_worker_id".to_string(),
                },
            )
            .await?;

        let mut bash_path = which::which("bash")
            .map_err(|e| Error::from_std_err(Code::Internal, &e))
            .err_tip(|| "Getting bash path")?;
        while let Ok(ref new_path) = std::fs::read_link(&bash_path) {
            bash_path = new_path
                .to_owned()
                .canonicalize()
                .err_tip(|| format!("Canonicalising {}", new_path.display()))?;
        }
        let cwd: PathBuf = env::current_dir()?;
        let relative_bash = pathdiff::diff_paths(&bash_path, &cwd).ok_or_else(|| {
            Error::new(
                Code::Internal,
                format!(
                    "Getting diff for {} and {}",
                    bash_path.display(),
                    cwd.display()
                ),
            )
        })?;
        info!(?relative_bash, ?cwd, "Canonicalise input");
        let canonical_path = running_action_impl
            .canonicalise_path(relative_bash.as_os_str(), &cwd.to_string_lossy().into())
            .err_tip(|| {
                format!(
                    "Canonicalising with {} and {}",
                    relative_bash.display(),
                    cwd.display()
                )
            })?;

        // Because of how pathdiff works, we get a relative_bash starting with
        // ../../../bin/bash in many test cases, so /usr/bin/bash is a good answer
        // But the initial bash path may well be /bin/bash, so permit this case
        // Note the canonicalisation like this only happens if both paths exist
        if canonical_path.to_str().unwrap() != "/usr/bin/bash"
            || bash_path.to_str().unwrap() != "/bin/bash"
        {
            // If it's anything else, check
            assert_eq!(canonical_path, bash_path);
        }
        Ok(())
    }

    #[nativelink_test]
    #[cfg(target_family = "unix")]
    async fn test_arg0_is_relative_path() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, slow_store, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory,
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);

        // Can't just use /bin/sh because of Nix paths
        let sh_path = which::which("sh")
            .map_err(|e| Error::from_std_err(Code::Internal, &e))
            .err_tip(|| "Getting sh_path path")?
            .to_string_lossy()
            .to_string();

        let input_root_digest = serialize_and_upload_message(
            &Directory {
                symlinks: vec![SymlinkNode {
                    name: "my_sh".to_string(),
                    target: sh_path,
                    node_properties: None,
                }],
                ..Default::default()
            },
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let command = Command {
            arguments: vec![
                "./my_sh".to_string(),
                "-c".to_string(),
                "printf \"%s\" \"$0\" > out.txt".to_string(),
            ],
            output_paths: vec!["out.txt".to_string()],
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

        let running_action_impl = running_actions_manager
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(ExecuteRequest {
                        action_digest: Some(action_digest.into()),
                        digest_function: ProtoDigestFunction::Sha256.into(),
                        ..Default::default()
                    }),
                    operation_id: OperationId::default().to_string(),
                    queued_timestamp: None,
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                },
            )
            .await?;

        let action_result = run_action(running_action_impl).await?;

        let stderr_content = slow_store
            .as_ref()
            .get_part_unchunked(action_result.stderr_digest, 0, None)
            .await?;
        assert_eq!(
            action_result.exit_code,
            0,
            "Action should succeed. stderr: {}",
            from_utf8(&stderr_content).unwrap_or("unreadable stderr")
        );

        assert_eq!(action_result.output_files.len(), 1);
        assert_eq!(
            action_result.output_files[0].name_or_path,
            NameOrPath::Path("out.txt".to_string())
        );

        let out_digest = action_result.output_files[0].digest;
        let out_content = slow_store
            .as_ref()
            .get_part_unchunked(out_digest, 0, None)
            .await?;

        assert_eq!(from_utf8(&out_content)?, "./my_sh");

        Ok(())
    }

    #[nativelink_test]
    async fn canonicalisation_failure() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        let (_, _, cas_store, ac_store) = setup_stores().await?;

        let arguments = vec![
            "garbage/to-canonicalise".to_string(),
            "arguments".to_string(),
            "to test".to_string(),
        ];
        let command = Command {
            arguments,
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

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            RunningActionsManagerArgs {
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::MAX,
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);

        let execute_request = ExecuteRequest {
            action_digest: Some(action_digest.into()),
            digest_function: ProtoDigestFunction::Sha256.into(),
            ..Default::default()
        };
        let operation_id = OperationId::default().to_string();

        let res = running_actions_manager
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(execute_request),
                    operation_id,
                    queued_timestamp: Some(make_system_time(1000).into()),
                    platform: action.platform.clone(),
                    worker_id: WORKER_ID.to_string(),
                },
            )
            .and_then(|action| action.prepare_action().and_then(RunningAction::execute))
            .await;
        assert!(res.is_err(), "{res:#?}");
        assert_eq!(res.unwrap_err(), Error::new_with_messages(Code::NotFound, vec![
            if cfg!(target_family = "windows") { "The system cannot find the path specified. (os error 3)" } else { "No such file or directory (os error 2)" },
            "Could not canonicalize path for command root garbage/to-canonicalise.",
            "Canonicalisation failure. Command=[\n    \"garbage/to-canonicalise\",\n    \"arguments\",\n    \"to test\",\n]"
            ].into_iter().map(String::from).collect()

        ));
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_pgid_from_stat_extracts_field_after_comm() {
        use nativelink_worker::running_actions_manager::parse_pgid_from_stat;
        // /proc/<pid>/stat layout: pid (comm) state ppid pgrp ...
        assert_eq!(
            parse_pgid_from_stat("315 (perl) S 1 60 60 0 -1 0"),
            Some(60)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_pgid_from_stat_handles_comm_with_spaces_and_parens() {
        use nativelink_worker::running_actions_manager::parse_pgid_from_stat;
        // `comm` may contain spaces and parentheses; parsing must be relative
        // to the final ')'. Here pgrp == 777.
        assert_eq!(
            parse_pgid_from_stat("1234 (weird )( name) R 1 777 777 0 -1 0"),
            Some(777)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parse_pgid_from_stat_rejects_malformed_input() {
        use nativelink_worker::running_actions_manager::parse_pgid_from_stat;
        assert_eq!(parse_pgid_from_stat("no parenthesis here"), None);
        assert_eq!(parse_pgid_from_stat("123 (only) S"), None); // too few fields
        assert_eq!(parse_pgid_from_stat(""), None);
    }

    // Regression test for #2636: deeply nested directories with a single
    // semaphore permit previously deadlocked because dir_futures and
    // file_futures competed for the same permit inside try_join3.
    // The fix awaits dir_futures first so permits are released before
    // file/symlink uploads begin.

    #[nativelink_test]
    #[cfg(target_family = "unix")]
    async fn upload_with_single_permit_nested_dirs() -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_action_directory = make_temp_path("root_action_directory");
        fs::create_dir_all(&root_action_directory).await?;

        // Take all but one FD permit away to trigger the deadlock scenario.
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);

        // Create 3-level nested dirs: out/a/b/ with files at each level.
        // This exercises recursive upload_directory under permit starvation.
        let arguments = vec![
            "sh".to_string(),
            "-c".to_string(),
            concat!(
                "mkdir -p ./out/a/b && ",
                "printf 'root ' > ./out/root.txt && ",
                "printf 'mid ' > ./out/a/mid.txt && ",
                "printf 'leaf ' > ./out/a/b/leaf.txt && ",
                "printf 'ok-stdout '; >&2 printf 'ok-stderr '"
            )
            .to_string(),
        ];
        let working_directory = "some_cwd";
        let command = Command {
            arguments,
            output_paths: vec!["out".to_string()],
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
            digest_function: ProtoDigestFunction::Sha256.into(),
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
                },
            )
            .await?;

        // This would deadlock before the fix in #2636 because nested
        // dir_futures and file_futures would compete for the single permit.
        let action_result = run_action(running_action_impl.clone()).await?;

        assert_eq!(action_result.exit_code, 0, "action should succeed");
        // Verify the nested directory tree was uploaded.
        assert_eq!(
            action_result.output_folders.len(),
            1,
            "expected one output directory"
        );
        assert_eq!(action_result.output_folders[0].path, "out");
        Ok(())
    }

    /// Regression test for #1859 / #1868: an aborted action's cleanup runs in
    /// the background, concurrently with a retry of the same operation. The
    /// retry's files must survive it. Each attempt therefore gets its own
    /// directory, which is what makes the two independent — no coordination
    /// between the cleanup and the retry is involved, and none should be
    /// needed.
    #[nativelink_test]
    async fn dropped_action_registers_cleanup_before_yielding_test()
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
                root_action_directory: root_action_directory.clone(),
                execution_configuration: ExecutionConfiguration::default(),
                cas_store: cas_store.clone(),
                ac_store: Some(Store::new(ac_store.clone())),
                historical_store: Store::new(cas_store.clone()),
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
            },
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(future::pending()),
            },
        )?);

        let command = Command {
            arguments: vec!["true".to_string()],
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

        let operation_id = OperationId::default();
        let start_execute = StartExecute {
            execute_request: Some(ExecuteRequest {
                action_digest: Some(action_digest.into()),
                ..Default::default()
            }),
            operation_id: operation_id.to_string(),
            queued_timestamp: None,
            platform: action.platform.clone(),
            worker_id: WORKER_ID.to_string(),
        };

        // First attempt: create and prepare, then get aborted (dropped
        // without cleanup), as happens when the worker disconnects or the
        // operation is cancelled.
        let running_action = running_actions_manager
            .create_and_add_action(WORKER_ID.to_string(), start_execute.clone())
            .await?
            .prepare_action()
            .await?;
        let work_directory = running_action.get_work_directory().clone();
        assert!(fs::metadata(&work_directory).await.is_ok());

        // Dropping without cleanup spawns the cleanup in the background; it has
        // not run yet, so the directory is still there when the retry starts.
        drop(running_action);
        assert!(
            fs::metadata(&work_directory).await.is_ok(),
            "background cleanup should not have run yet"
        );

        // Retry of the same operation, started while that cleanup is still
        // pending. It must get a directory of its own.
        let retry_action = running_actions_manager
            .create_and_add_action(WORKER_ID.to_string(), start_execute)
            .await?
            .prepare_action()
            .await?;
        let retry_work_directory = retry_action.get_work_directory().clone();
        assert_ne!(
            retry_work_directory, work_directory,
            "retry must not share the aborted attempt's directory"
        );

        // Let the aborted attempt's cleanup run to completion.
        wait_for_removal(&work_directory).await;
        assert!(
            fs::metadata(&retry_work_directory).await.is_ok(),
            "retry working directory was deleted by the previous attempt's cleanup"
        );

        // The retry still owns the operation: the late cleanup must not have
        // evicted its manager entry, or this fails.
        running_actions_manager
            .kill_operation(&operation_id)
            .await?;
        retry_action.cleanup().await?;
        Ok(())
    }

    /// #2001: many *identical* actions running at once on one worker locked it
    /// up. "Identical" there means the same action/command digest and the same
    /// input root, arriving as distinct operations — not the same operation
    /// twice — so all of them legitimately run side by side, sharing the input
    /// files they hardlink out of the `FilesystemStore`.
    ///
    /// This drives the whole lifecycle for each of them concurrently, the way
    /// `local_worker` does (one spawned task per `StartAction`), and fails on a
    /// timeout rather than hanging so a deadlock is a test failure.
    #[nativelink_test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_identical_actions_all_complete_test()
    -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";
        const CONCURRENT_ACTIONS: usize = 24;
        const INPUT_FILES: usize = 8;

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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
            })?);

        // One input root shared by every action, so they all contend for the
        // same `FilesystemStore` entries when hardlinking their inputs in.
        let mut input_files = Vec::with_capacity(INPUT_FILES);
        for i in 0..INPUT_FILES {
            let content = Bytes::from(format!("shared input {i}"));
            let digest = compute_buf_digest(&content, &mut DigestHasherFunc::Sha256.hasher());
            cas_store.update_oneshot(digest, content).await?;
            input_files.push(FileNode {
                name: format!("input_{i}.txt"),
                digest: Some(digest.into()),
                is_executable: false,
                node_properties: None,
            });
        }
        let input_root_digest = serialize_and_upload_message(
            &Directory {
                files: input_files,
                ..Default::default()
            },
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        #[cfg(target_family = "unix")]
        let arguments = vec!["true".to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec!["cmd".to_string(), "/C".to_string(), "exit 0".to_string()];
        let command_digest = serialize_and_upload_message(
            &Command {
                arguments,
                ..Default::default()
            },
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        let action_digest = serialize_and_upload_message(
            &Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            },
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        // Distinct operations, identical work.
        let handles: Vec<_> = (0..CONCURRENT_ACTIONS)
            .map(|i| {
                let running_actions_manager = running_actions_manager.clone();
                let start_execute = StartExecute {
                    execute_request: Some(ExecuteRequest {
                        action_digest: Some(action_digest.into()),
                        digest_function: ProtoDigestFunction::Sha256.into(),
                        ..Default::default()
                    }),
                    operation_id: format!("concurrent-identical-{i}"),
                    queued_timestamp: None,
                    platform: None,
                    worker_id: WORKER_ID.to_string(),
                };
                spawn!("concurrent_identical_action", async move {
                    let action = running_actions_manager
                        .create_and_add_action(WORKER_ID.to_string(), start_execute)
                        .await?;
                    run_action(action).await
                })
            })
            .collect();

        let results = tokio::time::timeout(Duration::from_secs(60), join_all(handles))
            .await
            .expect("worker locked up running concurrent identical actions");

        for (i, result) in results.into_iter().enumerate() {
            let action_result = result
                .unwrap_or_else(|e| panic!("action {i} task panicked: {e}"))
                .unwrap_or_else(|e| panic!("action {i} failed: {e}"));
            assert_eq!(action_result.exit_code, 0, "action {i} exit code");
        }

        // Every attempt cleaned up after itself; nothing is left behind.
        let mut remaining = tokio::fs::read_dir(&root_action_directory).await?;
        let mut leftovers = Vec::new();
        while let Some(entry) = remaining.next_entry().await? {
            leftovers.push(entry.file_name().to_string_lossy().into_owned());
        }
        assert!(
            leftovers.is_empty(),
            "action directories left behind: {leftovers:?}"
        );
        Ok(())
    }

    /// The same operation arriving twice at once must be rejected exactly once,
    /// not raced. `local_worker` handles each `Update::StartAction` in its own
    /// `spawn!`, so duplicates really can be in `create_and_add_action`
    /// simultaneously; the sequential
    /// `duplicate_start_does_not_disturb_live_action_test` does not cover that.
    #[nativelink_test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_duplicate_starts_admit_exactly_one_test()
    -> Result<(), Box<dyn core::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";
        const DUPLICATES: usize = 8;

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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
            })?);

        let command_digest = serialize_and_upload_message(
            &Command {
                arguments: vec!["true".to_string()],
                ..Default::default()
            },
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
        let action_digest = serialize_and_upload_message(
            &Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            },
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let operation_id = "concurrent-duplicate-operation-id".to_string();
        let handles: Vec<_> = (0..DUPLICATES)
            .map(|_| {
                let running_actions_manager = running_actions_manager.clone();
                let start_execute = StartExecute {
                    execute_request: Some(ExecuteRequest {
                        action_digest: Some(action_digest.into()),
                        digest_function: ProtoDigestFunction::Sha256.into(),
                        ..Default::default()
                    }),
                    operation_id: operation_id.clone(),
                    queued_timestamp: None,
                    platform: None,
                    worker_id: WORKER_ID.to_string(),
                };
                spawn!("concurrent_duplicate_start", async move {
                    running_actions_manager
                        .create_and_add_action(WORKER_ID.to_string(), start_execute)
                        .await
                })
            })
            .collect();

        let results = tokio::time::timeout(Duration::from_secs(60), join_all(handles))
            .await
            .expect("worker locked up running concurrent duplicate starts");

        // Every admitted action is still held here, so the winner's claim was
        // live for the whole window: a loser cannot have been admitted just
        // because the winner already finished.
        let mut admitted = Vec::new();
        for result in results {
            match result.expect("task panicked") {
                Ok(action) => admitted.push(action),
                Err(err) => assert_eq!(
                    err.code,
                    Code::AlreadyExists,
                    "duplicate should be rejected with AlreadyExists, got: {err}"
                ),
            }
        }
        assert_eq!(
            admitted.len(),
            1,
            "exactly one of {DUPLICATES} duplicate starts should have been admitted"
        );
        admitted.pop().unwrap().cleanup().await?;
        Ok(())
    }

    /// Rejecting a duplicate `StartAction` must not deregister the action that
    /// won. This is what lets a worker end up running one operation twice
    /// (#2001), and it needs no concurrency to reproduce:
    ///
    /// The duplicate check is the last thing `create_and_add_action` does, so a
    /// duplicate has already built a `RunningActionImpl` by the time it is
    /// rejected. Returning `AlreadyExists` drops that action, and `Drop` — with
    /// nothing to clean up and `has_manager_entry` still set — calls
    /// `cleanup_action()`, which removes the *winner's* entry. The operation is
    /// then unregistered while still running, so the next `StartAction` for it
    /// is admitted and the worker prepares the same operation a second time.
    #[nativelink_test]
    async fn rejected_duplicate_does_not_unregister_live_action_test()
    -> Result<(), Box<dyn core::error::Error>> {
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
            })?);

        let command_digest = serialize_and_upload_message(
            &Command {
                arguments: vec!["true".to_string()],
                ..Default::default()
            },
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
        let action_digest = serialize_and_upload_message(
            &Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            },
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let operation_id: OperationId = "rejected-duplicate-operation-id".into();
        let start_execute = StartExecute {
            execute_request: Some(ExecuteRequest {
                action_digest: Some(action_digest.into()),
                digest_function: ProtoDigestFunction::Sha256.into(),
                ..Default::default()
            }),
            operation_id: operation_id.to_string(),
            queued_timestamp: Some(SystemTime::now().into()),
            platform: None,
            worker_id: WORKER_ID.to_string(),
        };

        let running_action = running_actions_manager
            .create_and_add_action(WORKER_ID.to_string(), start_execute.clone())
            .await?;

        let err = running_actions_manager
            .create_and_add_action(WORKER_ID.to_string(), start_execute.clone())
            .await
            .map(|_| ())
            .expect_err("duplicate StartAction should be rejected");
        assert_eq!(err.code, Code::AlreadyExists, "{err}");

        // The rejection above must have left the winner registered. If it
        // deregistered it, this third `StartAction` is admitted and the worker
        // is now running the same operation twice.
        let err = running_actions_manager
            .create_and_add_action(WORKER_ID.to_string(), start_execute)
            .await
            .map(|_| ())
            .expect_err(
                "operation was deregistered by a rejected duplicate and is now running twice",
            );
        assert_eq!(err.code, Code::AlreadyExists, "{err}");

        // Still reachable by operation id, so a disconnect or cancellation can
        // still kill it.
        running_actions_manager
            .kill_operation(&operation_id)
            .await
            .err_tip(|| "operation was deregistered and can no longer be killed")?;

        // And it can still retire itself.
        running_action
            .cleanup()
            .await
            .err_tip(|| "operation was deregistered and could not clean up")?;
        Ok(())
    }

    /// The flip side of the same defect: an operation deregistered by a
    /// rejected duplicate is invisible to `kill_all()`, which is what a
    /// scheduler disconnect runs. `kill_all` reports every action drained while
    /// one is still executing, so the worker reconnects with an orphaned
    /// process still holding its resources.
    #[nativelink_test]
    async fn kill_all_still_sees_action_after_rejected_duplicate_test()
    -> Result<(), Box<dyn core::error::Error>> {
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
                upload_action_result_config: &UploadActionResultConfig {
                    upload_ac_results_strategy: UploadCacheResultsStrategy::Never,
                    ..Default::default()
                },
                max_action_timeout: Duration::MAX,
                max_upload_timeout: Duration::from_secs(DEFAULT_MAX_UPLOAD_TIMEOUT),
                timeout_handled_externally: false,
                directory_cache: None,
                #[cfg(target_os = "linux")]
                use_namespaces: use_namespaces(),
            })?);

        #[cfg(target_family = "unix")]
        let arguments = vec!["sh".to_string(), "-c".to_string(), "sleep 24h".to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec![
            "cmd".to_string(),
            "/C".to_string(),
            "ping -n 99999 127.0.0.1".to_string(),
        ];
        let command_digest = serialize_and_upload_message(
            &Command {
                arguments,
                ..Default::default()
            },
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
        let action_digest = serialize_and_upload_message(
            &Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            },
            cas_store.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;

        let operation_id = "kill-all-after-duplicate-operation-id".to_string();
        let start_execute = StartExecute {
            execute_request: Some(ExecuteRequest {
                action_digest: Some(action_digest.into()),
                digest_function: ProtoDigestFunction::Sha256.into(),
                ..Default::default()
            }),
            operation_id: operation_id.clone(),
            queued_timestamp: Some(SystemTime::now().into()),
            platform: None,
            worker_id: WORKER_ID.to_string(),
        };

        let running_action = running_actions_manager
            .create_and_add_action(WORKER_ID.to_string(), start_execute.clone())
            .await?
            .prepare_action()
            .await?;

        // Run the action the way the worker does: execute to completion, then
        // retire it. `kill_all` only returns once every action it knows about
        // has retired, so this has to be in flight alongside it.
        let action_task = spawn!("kill_all_after_duplicate", {
            let running_action = running_action.clone();
            async move {
                let running_action = running_action.execute().await?;
                running_action.cleanup().await?;
                Ok::<_, Error>(())
            }
        });
        // Let the child process actually start.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let err = running_actions_manager
            .create_and_add_action(WORKER_ID.to_string(), start_execute)
            .await
            .map(|_| ())
            .expect_err("duplicate StartAction should be rejected");
        assert_eq!(err.code, Code::AlreadyExists, "{err}");
        drop(running_action);

        // The disconnect path.
        tokio::time::timeout(Duration::from_secs(30), running_actions_manager.kill_all())
            .await
            .expect("kill_all hung");

        // `kill_all` claims every action has drained. If the rejected duplicate
        // deregistered this one, it saw an empty set, killed nothing and
        // returned anyway -- and the action is still running.
        tokio::time::timeout(Duration::from_secs(10), action_task)
            .await
            .expect("kill_all reported all actions drained while one was still running")??;
        Ok(())
    }
}
