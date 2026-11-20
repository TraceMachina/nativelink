// Copyright 2026 Trace Machina, Inc. All rights reserved.
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

use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use nativelink_config::stores::{
    FastSlowSpec, FilesystemSpec, MemorySpec, StoreDirection, StoreSpec,
};
use nativelink_error::{Error, make_input_err};
use nativelink_proto::build::bazel::remote::execution::v2::{Directory, DirectoryNode, FileNode};
use nativelink_store::ac_utils::{compute_buf_digest, serialize_and_upload_message};
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::filesystem_store::FilesystemStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::DigestHasherFunc;
use nativelink_util::store_trait::{Store, StoreLike};
use nativelink_worker::directory_cache::{DirectoryCache, DirectoryCacheConfig};
use nativelink_worker::running_actions_manager::download_to_directory_bounded;
use tokio::sync::Semaphore;

const CHILD_ENV: &str = "NL_INPUT_STACK_CASE";
const CONTENTS: &[u8] = b"deep input stack probe";

struct InputTree {
    fast: Arc<FilesystemStore>,
    cas: Arc<FastSlowStore>,
    cache: Option<DirectoryCache>,
    digest: DigestInfo,
    depth: usize,
    downloads: Semaphore,
    // Keep the filesystem alive until the stores have been dropped.
    _root: tempfile::TempDir,
}

impl InputTree {
    async fn new(mode: &str, depth: usize, fetches: usize) -> Result<Self, Error> {
        let root = tempfile::tempdir()?;
        let fast_spec = FilesystemSpec {
            content_path: root.path().join("content").to_string_lossy().into_owned(),
            temp_path: root.path().join("temp").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let fast = FilesystemStore::new(&fast_spec).await?;
        let slow = MemoryStore::new(&MemorySpec::default());
        let cas = FastSlowStore::new(
            &FastSlowSpec {
                fast: StoreSpec::Filesystem(fast_spec),
                slow: StoreSpec::Memory(MemorySpec::default()),
                fast_direction: StoreDirection::default(),
                slow_direction: StoreDirection::default(),
                bypass_dedup_threshold_bytes: 0,
                has_slow_write_wait_timeout_s: 0,
                leader_inactivity_timeout_s: 0,
            },
            Store::new(fast.clone()),
            Store::new(slow.clone()),
        );
        let file_digest = compute_buf_digest(CONTENTS, &mut DigestHasherFunc::Sha256.hasher());
        slow.update_oneshot(file_digest, Bytes::from_static(CONTENTS))
            .await?;
        let mut digest = serialize_and_upload_message(
            &Directory {
                files: vec![FileNode {
                    name: "leaf".to_owned(),
                    digest: Some(file_digest.into()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            slow.as_pin(),
            &mut DigestHasherFunc::Sha256.hasher(),
        )
        .await?;
        for _ in 0..depth {
            digest = serialize_and_upload_message(
                &Directory {
                    directories: vec![DirectoryNode {
                        name: "d".to_owned(),
                        digest: Some(digest.into()),
                    }],
                    ..Default::default()
                },
                slow.as_pin(),
                &mut DigestHasherFunc::Sha256.hasher(),
            )
            .await?;
        }
        let cache = if mode == "none" {
            None
        } else {
            Some(
                DirectoryCache::new(
                    DirectoryCacheConfig {
                        cache_root: root.path().join("cache"),
                        experimental_subtree_caching: mode == "subtree",
                        max_concurrent_fetches: fetches,
                        ..Default::default()
                    },
                    cas.clone(),
                )
                .await?,
            )
        };
        Ok(Self {
            _root: root,
            fast,
            cas,
            cache,
            digest,
            depth,
            downloads: Semaphore::new(fetches),
        })
    }

    async fn materialize(&self, destination: &Path, hot: bool) -> Result<(), Error> {
        if let Some(cache) = &self.cache {
            assert_eq!(cache.get_or_create(self.digest, destination).await?, hot);
        } else {
            tokio::fs::create_dir_all(destination).await?;
            download_to_directory_bounded(
                self.cas.as_ref(),
                self.fast.as_pin(),
                &self.digest,
                destination
                    .to_str()
                    .ok_or_else(|| make_input_err!("non-UTF8 test path"))?,
                &self.downloads,
            )
            .await?;
        }
        assert_eq!(
            tokio::fs::read(destination.join("d/".repeat(self.depth)).join("leaf")).await?,
            CONTENTS,
        );
        Ok(())
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "explicit setup and poll stack sizes are the subject of this test"
)]
fn run_case(case: String) -> Result<(), Error> {
    std::thread::Builder::new()
        .name("input-stack-setup".to_owned())
        .stack_size(32 * 1024 * 1024)
        .spawn(move || -> Result<(), Error> {
            let parts: Vec<_> = case.split(':').collect();
            let mode = parts[0];
            let hot = parts[1] == "hot";
            let depth = parts[2].parse::<usize>()?;
            let stack_bytes = parts[3].parse::<usize>()?;
            let fetches = parts[4].parse::<usize>()?;
            eprintln!("CASE {case} SETUP_BEGIN");
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            let tree = runtime.block_on(Box::pin(InputTree::new(mode, depth, fetches)))?;
            let destinations = tempfile::tempdir()?;
            if hot {
                eprintln!("CASE {case} PREWARM_BEGIN stack_bytes=33554432");
                runtime.block_on(Box::pin(async {
                    tokio::time::timeout(
                        Duration::from_secs(30),
                        tree.materialize(&destinations.path().join("warm"), false),
                    )
                    .await
                    .map_err(|_| make_input_err!("prewarm timed out: {case}"))?
                }))?;
                eprintln!("CASE {case} PREWARM_OK");
            }
            eprintln!("CASE {case} SETUP_OK");
            std::thread::Builder::new()
                .name("input-stack-poll".to_owned())
                .stack_size(stack_bytes)
                .spawn(move || -> Result<(), Error> {
                    eprintln!("CASE {case} MATERIALIZE_BEGIN");
                    runtime.block_on(Box::pin(async move {
                        tokio::time::timeout(
                            Duration::from_secs(30),
                            tree.materialize(&destinations.path().join("probe"), hot),
                        )
                        .await
                        .map_err(|_| make_input_err!("materialization timed out: {case}"))??;
                        eprintln!("CASE {case} MATERIALIZE_OK");
                        drop(tree);
                        drop(destinations);
                        Ok(())
                    }))
                })?
                .join()
                .map_err(|_| make_input_err!("poll thread panicked"))?
        })?
        .join()
        .map_err(|_| make_input_err!("setup thread panicked"))?
}

// A stack overflow aborts, so each probe needs its own process. Continue after
// failures to collect all modes rather than losing the rest of the matrix.
#[test]
fn isolated_input_stack() -> Result<(), Error> {
    if let Ok(case) = std::env::var(CHILD_ENV) {
        return run_case(case);
    }
    let mut failures = Vec::new();
    for depth in [8, 128] {
        for stack_bytes in [2 * 1024 * 1024, 16 * 1024 * 1024] {
            for (mode, fetches) in ["none", "default", "subtree"]
                .into_iter()
                .flat_map(|mode| [1, 2, 64].into_iter().map(move |fetches| (mode, fetches)))
            {
                for temperature in ["cold", "hot"] {
                    let case = format!("{mode}:{temperature}:{depth}:{stack_bytes}:{fetches}");
                    eprintln!("CASE {case} BEGIN");
                    let status = Command::new(std::env::current_exe()?)
                        .args(["--exact", "isolated_input_stack", "--nocapture"])
                        .env(CHILD_ENV, &case)
                        .status()?;
                    eprintln!("CASE {case} EXIT {status}");
                    if !status.success() {
                        failures.push(case);
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "failed cases: {failures:?}");
    Ok(())
}
