// Copyright 2022 The Turbo Cache Authors. All rights reserved.
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

use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
#[cfg(target_family = "unix")]
use std::fs::Permissions;
use std::io::Cursor;
#[cfg(target_family = "unix")]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::pin::Pin;
use std::str::from_utf8;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use futures::{FutureExt, TryFutureExt};
use lazy_static::lazy_static;
use prost::Message;
use rand::{thread_rng, Rng};
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use ac_utils::{compute_digest, get_and_decode_digest, serialize_and_upload_message};
#[cfg_attr(target_family = "windows", allow(unused_imports))]
use action_messages::{ActionResult, DirectoryInfo, ExecutionMetadata, FileInfo, NameOrPath, SymlinkInfo};
use common::{fs, DigestInfo};
use error::{Code, Error, ResultExt};
use fast_slow_store::FastSlowStore;
use filesystem_store::FilesystemStore;
use memory_store::MemoryStore;
#[cfg_attr(target_family = "windows", allow(unused_imports))]
use proto::build::bazel::remote::execution::v2::{
    platform::Property, Action, ActionResult as ProtoActionResult, Command, Directory, DirectoryNode, ExecuteRequest,
    FileNode, NodeProperties, Platform, SymlinkNode, Tree,
};
use proto::com::github::allada::turbo_cache::remote_execution::StartExecute;
use running_actions_manager::{
    download_to_directory, Callbacks, RunningAction, RunningActionImpl, RunningActionsManager,
    RunningActionsManagerImpl,
};
use store::Store;

/// Get temporary path from either `TEST_TMPDIR` or best effort temp directory if
/// not set.
fn make_temp_path(data: &str) -> String {
    #[cfg(target_family = "unix")]
    return format!(
        "{}/{}/{}",
        env::var("TEST_TMPDIR").unwrap_or(env::temp_dir().to_str().unwrap().to_string()),
        thread_rng().gen::<u64>(),
        data
    );
    #[cfg(target_family = "windows")]
    return format!(
        "{}\\{}\\{}",
        env::var("TEST_TMPDIR").unwrap_or(env::temp_dir().to_str().unwrap().to_string()),
        thread_rng().gen::<u64>(),
        data
    );
}

async fn setup_stores() -> Result<
    (
        Pin<Arc<FilesystemStore>>,
        Pin<Arc<MemoryStore>>,
        Pin<Arc<FastSlowStore>>,
        Pin<Arc<MemoryStore>>,
    ),
    Error,
> {
    let fast_config = config::stores::FilesystemStore {
        content_path: make_temp_path("content_path"),
        temp_path: make_temp_path("temp_path"),
        eviction_policy: None,
        ..Default::default()
    };
    let slow_config = config::stores::MemoryStore::default();
    let fast_store = Pin::new(Arc::new(FilesystemStore::new(&fast_config).await?));
    let slow_store = Pin::new(Arc::new(MemoryStore::new(&slow_config)));
    let ac_store = Pin::new(Arc::new(MemoryStore::new(&slow_config)));
    let cas_store = Pin::new(Arc::new(FastSlowStore::new(
        &config::stores::FastSlowStore {
            fast: config::stores::StoreConfig::filesystem(fast_config),
            slow: config::stores::StoreConfig::memory(slow_config),
        },
        Pin::into_inner(fast_store.clone()),
        Pin::into_inner(slow_store.clone()),
    )));
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

#[cfg(test)]
mod running_actions_manager_tests {
    use super::*;
    use pretty_assertions::assert_eq; // Must be declared in every module.

    #[tokio::test]
    async fn download_to_directory_file_download_test() -> Result<(), Box<dyn std::error::Error>> {
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
                fast_store.as_ref(),
                &root_directory_digest,
                &download_dir,
            )
            .await?;
            download_dir
        };
        {
            // Now ensure that our download_dir has the files.
            let file1_content = fs::read(format!("{download_dir}/{FILE1_NAME}")).await?;
            assert_eq!(std::str::from_utf8(&file1_content)?, FILE1_CONTENT);

            let file2_path = format!("{download_dir}/{FILE2_NAME}");
            let file2_content = fs::read(&file2_path).await?;
            assert_eq!(std::str::from_utf8(&file2_content)?, FILE2_CONTENT);

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

    #[tokio::test]
    async fn download_to_directory_folder_download_test() -> Result<(), Box<dyn std::error::Error>> {
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
                fast_store.as_ref(),
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
            assert_eq!(std::str::from_utf8(&file1_content)?, FILE1_CONTENT);

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
    #[tokio::test]
    async fn download_to_directory_symlink_download_test() -> Result<(), Box<dyn std::error::Error>> {
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
                fast_store.as_ref(),
                &root_directory_digest,
                &download_dir,
            )
            .await?;
            download_dir
        };
        {
            // Now ensure that our download_dir has the files.
            let symlink_path = format!("{download_dir}/{SYMLINK_NAME}");
            let symlink_content = fs::read(&symlink_path).await.err_tip(|| "On symlink read")?;
            assert_eq!(std::str::from_utf8(&symlink_content)?, FILE_CONTENT);

            let symlink_metadata = fs::symlink_metadata(&symlink_path)
                .await
                .err_tip(|| "On symlink symlink_metadata")?;
            assert_eq!(symlink_metadata.is_symlink(), true);
        }
        Ok(())
    }

    #[tokio::test]
    async fn ensure_output_files_full_directories_are_created_test() -> Result<(), Box<dyn std::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_work_directory = make_temp_path("root_work_directory");
        fs::create_dir_all(&root_work_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            root_work_directory,
            None,
            Pin::into_inner(cas_store.clone()),
            Pin::into_inner(ac_store.clone()),
            config::cas_server::UploadCacheResultsStrategy::Never,
            Duration::MAX,
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(futures::future::pending()),
            },
        )?);
        {
            const SALT: u64 = 55;
            let command = Command {
                arguments: vec!["touch".to_string(), "./some/path/test.txt".to_string()],
                output_files: vec!["some/path/test.txt".to_string()],
                working_directory: "some_cwd".to_string(),
                ..Default::default()
            };
            let command_digest = serialize_and_upload_message(&command, cas_store.as_ref()).await?;
            let input_root_digest = serialize_and_upload_message(
                &Directory {
                    directories: vec![DirectoryNode {
                        name: "some_cwd".to_string(),
                        digest: Some(
                            serialize_and_upload_message(&Directory::default(), cas_store.as_ref())
                                .await?
                                .into(),
                        ),
                    }],
                    ..Default::default()
                },
                cas_store.as_ref(),
            )
            .await?;
            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(&action, cas_store.as_ref()).await?;

            let running_action = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(ExecuteRequest {
                            action_digest: Some(action_digest.into()),
                            ..Default::default()
                        }),
                        salt: SALT,
                        queued_timestamp: None,
                    },
                )
                .await?;

            let running_action = running_action.clone().prepare_action().await?;

            // The folder should have been created for our output file.
            assert_eq!(
                fs::metadata(format!("{}/{}", running_action.get_work_directory(), "some/path"))
                    .await
                    .is_ok(),
                true,
                "Expected path to exist"
            );

            running_action.cleanup().await?;
        };
        Ok(())
    }

    #[tokio::test]
    async fn upload_files_from_above_cwd_test() -> Result<(), Box<dyn std::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, slow_store, cas_store, ac_store) = setup_stores().await?;
        let root_work_directory = make_temp_path("root_work_directory");
        fs::create_dir_all(&root_work_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            root_work_directory,
            None,
            Pin::into_inner(cas_store.clone()),
            Pin::into_inner(ac_store.clone()),
            config::cas_server::UploadCacheResultsStrategy::Never,
            Duration::MAX,
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(futures::future::pending()),
            },
        )?);
        let action_result = {
            const SALT: u64 = 55;
            #[cfg(target_family = "unix")]
            let arguments = vec![
                "sh".to_string(),
                "-c".to_string(),
                "echo -n \"123 \" > ../test.txt; echo -n \"foo-stdout \"; >&2 echo -n \"bar-stderr  \"".to_string(),
            ];
            #[cfg(target_family = "windows")]
            let arguments = vec![
                "cmd".to_string(),
                "/C".to_string(),
                // Note: Windows adds two spaces after 'set /p=XXX'.
                "echo | set /p=123> ../test.txt & echo | set /p=foo-stdout & echo | set /p=bar-stderr 1>&2 & exit 0"
                    .to_string(),
            ];
            let command = Command {
                arguments,
                output_paths: vec!["test.txt".to_string()],
                working_directory: "some_cwd".to_string(),
                ..Default::default()
            };
            let command_digest = serialize_and_upload_message(&command, cas_store.as_ref()).await?;
            let input_root_digest = serialize_and_upload_message(
                &Directory {
                    directories: vec![DirectoryNode {
                        name: "some_cwd".to_string(),
                        digest: Some(
                            serialize_and_upload_message(&Directory::default(), cas_store.as_ref())
                                .await?
                                .into(),
                        ),
                    }],
                    ..Default::default()
                },
                cas_store.as_ref(),
            )
            .await?;
            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(&action, cas_store.as_ref()).await?;

            let running_action_impl = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(ExecuteRequest {
                            action_digest: Some(action_digest.into()),
                            ..Default::default()
                        }),
                        salt: SALT,
                        queued_timestamp: None,
                    },
                )
                .await?;

            run_action(running_action_impl.clone()).await?
        };
        let file_content = slow_store
            .as_ref()
            .get_part_unchunked(action_result.output_files[0].digest, 0, None, None)
            .await?;
        assert_eq!(from_utf8(&file_content)?, "123 ");
        let stdout_content = slow_store
            .as_ref()
            .get_part_unchunked(action_result.stdout_digest, 0, None, None)
            .await?;
        assert_eq!(from_utf8(&stdout_content)?, "foo-stdout ");
        let stderr_content = slow_store
            .as_ref()
            .get_part_unchunked(action_result.stderr_digest, 0, None, None)
            .await?;
        assert_eq!(from_utf8(&stderr_content)?, "bar-stderr  ");
        let mut clock_time = make_system_time(0);
        assert_eq!(
            action_result,
            ActionResult {
                output_files: vec![FileInfo {
                    name_or_path: NameOrPath::Path("test.txt".to_string()),
                    digest: DigestInfo::try_new("c69e10a5f54f4e28e33897fbd4f8701595443fa8c3004aeaa20dd4d9a463483b", 4)?,
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
            }
        );
        Ok(())
    }

    // Windows does not support symlinks.
    #[cfg(not(target_family = "windows"))]
    #[tokio::test]
    async fn upload_dir_and_symlink_test() -> Result<(), Box<dyn std::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, slow_store, cas_store, ac_store) = setup_stores().await?;
        let root_work_directory = make_temp_path("root_work_directory");
        fs::create_dir_all(&root_work_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            root_work_directory,
            None,
            Pin::into_inner(cas_store.clone()),
            Pin::into_inner(ac_store.clone()),
            config::cas_server::UploadCacheResultsStrategy::Never,
            Duration::MAX,
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(futures::future::pending()),
            },
        )?);
        let queued_timestamp = make_system_time(1000);
        let action_result = {
            const SALT: u64 = 55;
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
                ..Default::default()
            };
            let command_digest = serialize_and_upload_message(&command, cas_store.as_ref()).await?;
            let input_root_digest = serialize_and_upload_message(&Directory::default(), cas_store.as_ref()).await?;
            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(&action, cas_store.as_ref()).await?;

            let running_action_impl = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(ExecuteRequest {
                            action_digest: Some(action_digest.into()),
                            ..Default::default()
                        }),
                        salt: SALT,
                        queued_timestamp: Some(queued_timestamp.into()),
                    },
                )
                .await?;

            run_action(running_action_impl.clone()).await?
        };
        let tree =
            get_and_decode_digest::<Tree>(slow_store.as_ref(), &action_result.output_folders[0].tree_digest).await?;
        let root_directory = Directory {
            files: vec![
                FileNode {
                    name: "file".to_string(),
                    digest: Some(
                        DigestInfo::try_new("b5bb9d8014a0f9b1d61e21e796d78dccdf1352f23cd32812f4850b878ae4944c", 4)?
                            .into(),
                    ),
                    ..Default::default()
                },
                FileNode {
                    name: "file2".to_string(),
                    digest: Some(
                        DigestInfo::try_new("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", 0)?
                            .into(),
                    ),
                    ..Default::default()
                },
            ],
            directories: vec![DirectoryNode {
                name: "dir2".to_string(),
                digest: Some(
                    DigestInfo::try_new("cce0098e0b0f1d785edb0da50beedb13e27dcd459b091b2f8f82543cb7cd0527", 16)?.into(),
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
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn cleanup_happens_on_job_failure() -> Result<(), Box<dyn std::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_work_directory = make_temp_path("root_work_directory");
        fs::create_dir_all(&root_work_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            root_work_directory.clone(),
            None,
            Pin::into_inner(cas_store.clone()),
            Pin::into_inner(ac_store.clone()),
            config::cas_server::UploadCacheResultsStrategy::Never,
            Duration::MAX,
            Callbacks {
                now_fn: test_monotonic_clock,
                sleep_fn: |_duration| Box::pin(futures::future::pending()),
            },
        )?);
        let queued_timestamp = make_system_time(1000);

        #[cfg(target_family = "unix")]
        let arguments = vec!["sh".to_string(), "-c".to_string(), "exit 33".to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec!["cmd".to_string(), "/C".to_string(), "exit 33".to_string()];

        let action_result = {
            const SALT: u64 = 55;
            let command = Command {
                arguments,
                output_paths: vec![],
                working_directory: ".".to_string(),
                ..Default::default()
            };
            let command_digest = serialize_and_upload_message(&command, cas_store.as_ref()).await?;
            let input_root_digest = serialize_and_upload_message(&Directory::default(), cas_store.as_ref()).await?;
            let action = Action {
                command_digest: Some(command_digest.into()),
                input_root_digest: Some(input_root_digest.into()),
                ..Default::default()
            };
            let action_digest = serialize_and_upload_message(&action, cas_store.as_ref()).await?;

            let running_action_impl = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(ExecuteRequest {
                            action_digest: Some(action_digest.into()),
                            ..Default::default()
                        }),
                        salt: SALT,
                        queued_timestamp: Some(queued_timestamp.into()),
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
            }
        );
        let mut dir_stream = fs::read_dir(&root_work_directory).await?;
        assert!(
            dir_stream.as_mut().next_entry().await?.is_none(),
            "Expected empty directory at {root_work_directory}"
        );
        Ok(())
    }

    #[tokio::test]
    async fn kill_ends_action() -> Result<(), Box<dyn std::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";
        const SALT: u64 = 55;

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_work_directory = make_temp_path("root_work_directory");
        fs::create_dir_all(&root_work_directory).await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new(
            root_work_directory.clone(),
            None,
            Pin::into_inner(cas_store.clone()),
            Pin::into_inner(ac_store.clone()),
            config::cas_server::UploadCacheResultsStrategy::Never,
            Duration::MAX,
        )?);

        #[cfg(target_family = "unix")]
        let arguments = vec!["sh".to_string(), "-c".to_string(), "sleep infinity".to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec!["cmd".to_string(), "/C".to_string(), "timeout 99999".to_string()];

        let command = Command {
            arguments,
            output_paths: vec![],
            working_directory: ".".to_string(),
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(&command, cas_store.as_ref()).await?;
        let input_root_digest = serialize_and_upload_message(&Directory::default(), cas_store.as_ref()).await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(&action, cas_store.as_ref()).await?;

        let running_action_impl = running_actions_manager
            .clone()
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(ExecuteRequest {
                        action_digest: Some(action_digest.into()),
                        ..Default::default()
                    }),
                    salt: SALT,
                    queued_timestamp: Some(make_system_time(1000).into()),
                },
            )
            .await?;

        // Start the action and kill it at the same time.
        let result = futures::join!(run_action(running_action_impl), running_actions_manager.kill_all()).0?;

        // Check that the action was killed.
        #[cfg(target_family = "unix")]
        assert_eq!(9, result.exit_code);
        // Note: Windows kill command returns exit code 1.
        #[cfg(target_family = "windows")]
        assert_eq!(1, result.exit_code);

        Ok(())
    }

    // This script runs a command under a wrapper script set in a config.
    // The wrapper script will print a constant string to stderr, and the test itself will
    // print to stdout. We then check the results of both to make sure the shell script was
    // invoked and the actual command was invoked under the shell script.
    #[tokio::test]
    async fn entrypoint_cmd_does_invoke_if_set() -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(target_family = "unix")]
        const TEST_WRAPPER_SCRIPT_CONTENT: &str = "\
#!/bin/bash
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
        const SALT: u64 = 66;
        const EXPECTED_STDOUT: &str = "Action did run";

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_work_directory = make_temp_path("root_work_directory");
        fs::create_dir_all(&root_work_directory).await?;

        let test_wrapper_script = {
            let test_wrapper_dir = make_temp_path("wrapper_dir");
            fs::create_dir_all(&test_wrapper_dir).await?;
            #[cfg(target_family = "unix")]
            let test_wrapper_script = OsString::from(test_wrapper_dir + "/test_wrapper_script.sh");
            #[cfg(target_family = "windows")]
            let test_wrapper_script = OsString::from(test_wrapper_dir + "\\test_wrapper_script.bat");
            let mut test_wrapper_script_handle = fs::create_file(&test_wrapper_script).await?;
            test_wrapper_script_handle
                .as_writer()
                .await?
                .write_all(TEST_WRAPPER_SCRIPT_CONTENT.as_bytes())
                .await?;
            #[cfg(target_family = "unix")]
            fs::set_permissions(&test_wrapper_script, Permissions::from_mode(0o755)).await?;
            test_wrapper_script
        };

        let mut full_wrapper_script_path = env::current_dir()?;
        full_wrapper_script_path.push(test_wrapper_script);
        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new(
            root_work_directory.clone(),
            Some(Arc::new(
                full_wrapper_script_path.into_os_string().into_string().unwrap(),
            )),
            Pin::into_inner(cas_store.clone()),
            Pin::into_inner(ac_store.clone()),
            config::cas_server::UploadCacheResultsStrategy::Never,
            Duration::MAX,
        )?);
        #[cfg(target_family = "unix")]
        let arguments = vec!["printf".to_string(), EXPECTED_STDOUT.to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec!["echo".to_string(), EXPECTED_STDOUT.to_string()];
        let command = Command {
            arguments,
            working_directory: ".".to_string(),
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(&command, cas_store.as_ref()).await?;
        let input_root_digest = serialize_and_upload_message(&Directory::default(), cas_store.as_ref()).await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(&action, cas_store.as_ref()).await?;

        let running_action_impl = running_actions_manager
            .clone()
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(ExecuteRequest {
                        action_digest: Some(action_digest.into()),
                        ..Default::default()
                    }),
                    salt: SALT,
                    queued_timestamp: Some(make_system_time(1000).into()),
                },
            )
            .await?;

        let result = run_action(running_action_impl).await?;
        assert_eq!(result.exit_code, 0, "Exit code should be 0");

        let expected_stdout = compute_digest(Cursor::new(EXPECTED_STDOUT)).await?.0;
        // Note: This string should match what is in worker_for_test.sh
        let expected_stderr = compute_digest(Cursor::new("Wrapper script did run")).await?.0;
        assert_eq!(expected_stdout, result.stdout_digest);
        assert_eq!(expected_stderr, result.stderr_digest);

        Ok(())
    }

    #[tokio::test]
    async fn entrypoint_cmd_injects_properties() -> Result<(), Box<dyn std::error::Error>> {
        #[cfg(target_family = "unix")]
        const TEST_WRAPPER_SCRIPT_CONTENT: &str = "\
#!/bin/bash
# Print some static text to stderr. This is what the test uses to
# make sure the script did run.
>&2 printf \"Wrapper script did run with argument $1\"

# Now run the real command.
shift 1
exec \"$@\"
";
        #[cfg(target_family = "windows")]
        const TEST_WRAPPER_SCRIPT_CONTENT: &str = "\
@echo off
:: Print some static text to stderr. This is what the test uses to
:: make sure the script did run.
echo | set /p=\"Wrapper script did run with argument %1\" 1>&2

:: Run command, but morph the echo to ensure it doesn't
:: add a new line to the end of the output.
%2 | set /p=%3
exit 0
";
        const WORKER_ID: &str = "foo_worker_id";
        const SALT: u64 = 66;
        const EXPECTED_STDOUT: &str = "Action did run";

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let root_work_directory = make_temp_path("root_work_directory");
        fs::create_dir_all(&root_work_directory).await?;

        let test_wrapper_script = {
            let test_wrapper_dir = make_temp_path("wrapper_dir");
            fs::create_dir_all(&test_wrapper_dir).await?;
            #[cfg(target_family = "unix")]
            let test_wrapper_script = OsString::from(test_wrapper_dir + "/test_wrapper_script.sh");
            #[cfg(target_family = "windows")]
            let test_wrapper_script = OsString::from(test_wrapper_dir + "\\test_wrapper_script.bat");
            let mut test_wrapper_script_handle = fs::create_file(&test_wrapper_script).await?;
            test_wrapper_script_handle
                .as_writer()
                .await?
                .write_all(TEST_WRAPPER_SCRIPT_CONTENT.as_bytes())
                .await?;
            #[cfg(target_family = "unix")]
            fs::set_permissions(&test_wrapper_script, Permissions::from_mode(0o755)).await?;
            test_wrapper_script
        };

        let mut full_wrapper_script_path = env::current_dir()?;
        full_wrapper_script_path.push(test_wrapper_script);
        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new(
            root_work_directory.clone(),
            Some(Arc::new(
                full_wrapper_script_path.into_os_string().into_string().unwrap() + " {property_name}",
            )),
            Pin::into_inner(cas_store.clone()),
            Pin::into_inner(ac_store.clone()),
            config::cas_server::UploadCacheResultsStrategy::Never,
            Duration::MAX,
        )?);
        #[cfg(target_family = "unix")]
        let arguments = vec!["printf".to_string(), EXPECTED_STDOUT.to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec!["echo".to_string(), EXPECTED_STDOUT.to_string()];
        let command = Command {
            arguments,
            working_directory: ".".to_string(),
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(&command, cas_store.as_ref()).await?;
        let input_root_digest = serialize_and_upload_message(&Directory::default(), cas_store.as_ref()).await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            platform: Some(Platform {
                properties: vec![Property {
                    name: "property_name".into(),
                    value: "test value".into(),
                }],
            }),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(&action, cas_store.as_ref()).await?;

        let running_action_impl = running_actions_manager
            .clone()
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(ExecuteRequest {
                        action_digest: Some(action_digest.into()),
                        ..Default::default()
                    }),
                    salt: SALT,
                    queued_timestamp: Some(make_system_time(1000).into()),
                },
            )
            .await?;

        let result = run_action(running_action_impl).await?;
        assert_eq!(result.exit_code, 0, "Exit code should be 0");

        let expected_stdout = compute_digest(Cursor::new(EXPECTED_STDOUT)).await?.0;
        // Note: This string should match what is in worker_for_test.sh
        let expected_stderr = "Wrapper script did run with argument test value";
        let expected_stderr_digest = compute_digest(Cursor::new(expected_stderr)).await?.0;

        let actual_stderr: prost::bytes::Bytes = cas_store
            .as_ref()
            .get_part_unchunked(result.stderr_digest, 0, None, Some(expected_stderr.len()))
            .await?;
        let actual_stderr_decoded = std::str::from_utf8(&actual_stderr)?;
        assert_eq!(expected_stderr, actual_stderr_decoded);
        assert_eq!(expected_stdout, result.stdout_digest);
        assert_eq!(expected_stderr_digest, result.stderr_digest);

        Ok(())
    }

    #[tokio::test]
    async fn caches_results_in_action_cache_store() -> Result<(), Box<dyn std::error::Error>> {
        let (_, _, cas_store, ac_store) = setup_stores().await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new(
            String::new(),
            None,
            Pin::into_inner(cas_store.clone()),
            Pin::into_inner(ac_store.clone()),
            config::cas_server::UploadCacheResultsStrategy::SuccessOnly,
            Duration::MAX,
        )?);

        let action_digest = DigestInfo::new([2u8; 32], 32);
        let action_result = ActionResult {
            output_files: vec![FileInfo {
                name_or_path: NameOrPath::Path("test.txt".to_string()),
                digest: DigestInfo::try_new("a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3", 3)?,
                is_executable: false,
            }],
            stdout_digest: DigestInfo::try_new("426afaf613d8cfdd9fa8addcc030ae6c95a7950ae0301164af1d5851012081d5", 10)?,
            stderr_digest: DigestInfo::try_new("7b2e400d08b8e334e3172d105be308b506c6036c62a9bde5c509d7808b28b213", 10)?,
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
        };
        running_actions_manager
            .cache_action_result(action_digest, action_result.clone())
            .await?;

        let retrieved_result = get_and_decode_digest::<ProtoActionResult>(ac_store.as_ref(), &action_digest).await?;

        let proto_result: ProtoActionResult = action_result.into();
        assert_eq!(proto_result, retrieved_result);

        Ok(())
    }

    #[tokio::test]
    async fn failed_action_does_not_cache_in_action_cache() -> Result<(), Box<dyn std::error::Error>> {
        let (_, _, cas_store, ac_store) = setup_stores().await?;

        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new(
            String::new(),
            None,
            Pin::into_inner(cas_store.clone()),
            Pin::into_inner(ac_store.clone()),
            config::cas_server::UploadCacheResultsStrategy::Everything,
            Duration::MAX,
        )?);

        let action_digest = DigestInfo::new([2u8; 32], 32);
        let action_result = ActionResult {
            output_files: vec![FileInfo {
                name_or_path: NameOrPath::Path("test.txt".to_string()),
                digest: DigestInfo::try_new("a665a45920422f9d417e4867efdc4fb8a04a1f3fff1fa07e998e86f7f7a27ae3", 3)?,
                is_executable: false,
            }],
            stdout_digest: DigestInfo::try_new("426afaf613d8cfdd9fa8addcc030ae6c95a7950ae0301164af1d5851012081d5", 10)?,
            stderr_digest: DigestInfo::try_new("7b2e400d08b8e334e3172d105be308b506c6036c62a9bde5c509d7808b28b213", 10)?,
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
        };
        running_actions_manager
            .cache_action_result(action_digest, action_result.clone())
            .await?;

        let retrieved_result = get_and_decode_digest::<ProtoActionResult>(ac_store.as_ref(), &action_digest).await?;

        let proto_result: ProtoActionResult = action_result.into();
        assert_eq!(proto_result, retrieved_result);

        Ok(())
    }

    #[tokio::test]
    async fn ensure_worker_timeout_chooses_correct_values() -> Result<(), Box<dyn std::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        let root_work_directory = make_temp_path("root_work_directory");
        fs::create_dir_all(&root_work_directory).await?;

        let (_, _, cas_store, ac_store) = setup_stores().await?;

        #[cfg(target_family = "unix")]
        let arguments = vec!["true".to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec!["cmd".to_string(), "/C".to_string(), "exit".to_string(), "0".to_string()];

        let command = Command {
            arguments,
            output_paths: vec![],
            working_directory: ".".to_string(),
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(&command, cas_store.as_ref()).await?;
        let input_root_digest = serialize_and_upload_message(&Directory::default(), cas_store.as_ref()).await?;

        {
            // Test to ensure that the task timeout is choosen if it is less than the max timeout.
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
            let action_digest = serialize_and_upload_message(&action, cas_store.as_ref()).await?;

            let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
                root_work_directory.clone(),
                None,
                Pin::into_inner(cas_store.clone()),
                Pin::into_inner(ac_store.clone()),
                config::cas_server::UploadCacheResultsStrategy::Never,
                MAX_TIMEOUT_DURATION,
                Callbacks {
                    now_fn: test_monotonic_clock,
                    sleep_fn: |duration| {
                        SENT_TIMEOUT.store(duration.as_millis() as i64, Ordering::Relaxed);
                        Box::pin(futures::future::pending())
                    },
                },
            )?);

            running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(ExecuteRequest {
                            action_digest: Some(action_digest.into()),
                            ..Default::default()
                        }),
                        salt: 0,
                        queued_timestamp: Some(make_system_time(1000).into()),
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
            assert_eq!(SENT_TIMEOUT.load(Ordering::Relaxed), TASK_TIMEOUT.as_millis() as i64);
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
            let action_digest = serialize_and_upload_message(&action, cas_store.as_ref()).await?;

            let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
                root_work_directory.clone(),
                None,
                Pin::into_inner(cas_store.clone()),
                Pin::into_inner(ac_store.clone()),
                config::cas_server::UploadCacheResultsStrategy::Never,
                MAX_TIMEOUT_DURATION,
                Callbacks {
                    now_fn: test_monotonic_clock,
                    sleep_fn: |duration| {
                        SENT_TIMEOUT.store(duration.as_millis() as i64, Ordering::Relaxed);
                        Box::pin(futures::future::pending())
                    },
                },
            )?);

            running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(ExecuteRequest {
                            action_digest: Some(action_digest.into()),
                            ..Default::default()
                        }),
                        salt: 0,
                        queued_timestamp: Some(make_system_time(1000).into()),
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
                MAX_TIMEOUT_DURATION.as_millis() as i64
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
            let action_digest = serialize_and_upload_message(&action, cas_store.as_ref()).await?;

            let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
                root_work_directory.clone(),
                None,
                Pin::into_inner(cas_store.clone()),
                Pin::into_inner(ac_store.clone()),
                config::cas_server::UploadCacheResultsStrategy::Never,
                MAX_TIMEOUT_DURATION,
                Callbacks {
                    now_fn: test_monotonic_clock,
                    sleep_fn: |duration| {
                        SENT_TIMEOUT.store(duration.as_millis() as i64, Ordering::Relaxed);
                        Box::pin(futures::future::pending())
                    },
                },
            )?);

            let result = running_actions_manager
                .create_and_add_action(
                    WORKER_ID.to_string(),
                    StartExecute {
                        execute_request: Some(ExecuteRequest {
                            action_digest: Some(action_digest.into()),
                            ..Default::default()
                        }),
                        salt: 0,
                        queued_timestamp: Some(make_system_time(1000).into()),
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

    #[tokio::test]
    async fn worker_times_out() -> Result<(), Box<dyn std::error::Error>> {
        const WORKER_ID: &str = "foo_worker_id";

        fn test_monotonic_clock() -> SystemTime {
            static CLOCK: AtomicU64 = AtomicU64::new(0);
            monotonic_clock(&CLOCK)
        }

        type StaticOneshotTuple = Mutex<(Option<oneshot::Sender<()>>, Option<oneshot::Receiver<()>>)>;
        lazy_static! {
            static ref TIMEOUT_ONESHOT: StaticOneshotTuple = {
                let (tx, rx) = oneshot::channel();
                Mutex::new((Some(tx), Some(rx)))
            };
        }
        let root_work_directory = make_temp_path("root_work_directory");
        fs::create_dir_all(&root_work_directory).await?;

        let (_, _, cas_store, ac_store) = setup_stores().await?;
        let running_actions_manager = Arc::new(RunningActionsManagerImpl::new_with_callbacks(
            root_work_directory.clone(),
            None,
            Pin::into_inner(cas_store.clone()),
            Pin::into_inner(ac_store.clone()),
            config::cas_server::UploadCacheResultsStrategy::Never,
            Duration::MAX,
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
        let arguments = vec!["sh".to_string(), "-c".to_string(), "sleep infinity".to_string()];
        #[cfg(target_family = "windows")]
        let arguments = vec!["cmd".to_string(), "/C".to_string(), "timeout 99999".to_string()];

        let command = Command {
            arguments,
            output_paths: vec![],
            working_directory: ".".to_string(),
            ..Default::default()
        };
        let command_digest = serialize_and_upload_message(&command, cas_store.as_ref()).await?;
        let input_root_digest = serialize_and_upload_message(&Directory::default(), cas_store.as_ref()).await?;
        let action = Action {
            command_digest: Some(command_digest.into()),
            input_root_digest: Some(input_root_digest.into()),
            ..Default::default()
        };
        let action_digest = serialize_and_upload_message(&action, cas_store.as_ref()).await?;

        let execute_results_fut = running_actions_manager
            .create_and_add_action(
                WORKER_ID.to_string(),
                StartExecute {
                    execute_request: Some(ExecuteRequest {
                        action_digest: Some(action_digest.into()),
                        ..Default::default()
                    }),
                    salt: 0,
                    queued_timestamp: Some(make_system_time(1000).into()),
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

        let (results, _) = tokio::join!(execute_results_fut, async move {
            tokio::task::yield_now().await;
            let tx = TIMEOUT_ONESHOT.lock().unwrap().0.take().unwrap();
            tx.send(()).expect("Could not send timeout signal");
        });
        assert_eq!(results?.error.unwrap().code, Code::DeadlineExceeded);

        Ok(())
    }
}
