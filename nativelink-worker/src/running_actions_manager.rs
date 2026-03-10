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

use core::cmp::min;
use core::convert::Into;
use core::fmt::Debug;
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering};
use core::time::Duration;
use std::borrow::Cow;
use std::collections::vec_deque::VecDeque;
use std::collections::{HashMap, HashSet};
use std::env;
use std::ffi::{OsStr, OsString};
#[cfg(target_family = "unix")]
use std::fs::Permissions;
#[cfg(target_family = "unix")]
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Weak};
use std::time::SystemTime;

use bytes::{Bytes, BytesMut};
use filetime::{FileTime, set_file_mtime};
use formatx::Template;
use futures::future::{
    BoxFuture, Future, FutureExt, TryFutureExt, try_join, try_join_all, try_join3,
};
use futures::stream::{FuturesUnordered, StreamExt, TryStreamExt};
use nativelink_config::cas_server::{
    EnvironmentSource, UploadActionResultConfig, UploadCacheResultsStrategy,
};
use nativelink_config::stores::StoreDirection;
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_metric::MetricsComponent;
use nativelink_proto::build::bazel::remote::execution::v2::{
    Action, ActionResult as ProtoActionResult, BatchReadBlobsRequest, Command as ProtoCommand,
    Directory as ProtoDirectory, Directory, DirectoryNode, ExecuteResponse, FileNode,
    GetTreeRequest, SymlinkNode, Tree as ProtoTree, UpdateActionResultRequest,
};
use nativelink_proto::com::github::trace_machina::nativelink::remote_execution::{
    HistoricalExecuteResponse, StartExecute,
};
use nativelink_store::ac_utils::{
    ESTIMATED_DIGEST_SIZE, compute_buf_digest, get_and_decode_digest, serialize_and_upload_message,
};
use nativelink_store::cas_utils::is_zero_digest;
use nativelink_store::fast_slow_store::FastSlowStore;
use nativelink_store::filesystem_store::{FileEntry, FilesystemStore};
use nativelink_store::grpc_store::GrpcStore;
use nativelink_util::action_messages::{
    ActionInfo, ActionResult, DirectoryInfo, ExecutionMetadata, FileInfo, NameOrPath, OperationId,
    SymlinkInfo, to_execute_response,
};
use nativelink_util::common::{DigestInfo, fs};
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc, default_digest_hasher_func};
use nativelink_util::metrics_utils::{AsyncCounterWrapper, CounterWithTime};
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::store_trait::{Store, StoreKey, StoreLike, StoreOptimizations, UploadSizeInfo};
use nativelink_util::log_utils::throughput_mbps;
use nativelink_util::{background_spawn, spawn, spawn_blocking};
use parking_lot::Mutex;
use prost::Message;
use scopeguard::{ScopeGuard, guard};
use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::process;
use tokio::sync::{Notify, oneshot, watch};
use tokio::time::Instant;
use tokio_stream::wrappers::ReadDirStream;
use opentelemetry::context::Context;
use tonic::Request;
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

/// For simplicity we use a fixed exit code for cases when our program is terminated
/// due to a signal.
const EXIT_CODE_FOR_SIGNAL: i32 = 9;

/// Default strategy for uploading historical results.
/// Note: If this value changes the config documentation
/// should reflect it.
const DEFAULT_HISTORICAL_RESULTS_STRATEGY: UploadCacheResultsStrategy =
    UploadCacheResultsStrategy::FailuresOnly;

/// Valid string reasons for a failure.
/// Note: If these change, the documentation should be updated.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SideChannelFailureReason {
    /// Task should be considered timed out.
    Timeout,
}

/// This represents the json data that can be passed from the running process
/// to the parent via the `SideChannelFile`. See:
/// `config::EnvironmentSource::sidechannelfile` for more details.
/// Note: Any fields added here must be added to the documentation.
#[derive(Debug, Deserialize, Default)]
struct SideChannelInfo {
    /// If the task should be considered a failure and why.
    failure: Option<SideChannelFailureReason>,
}

#[derive(prost::Message)]
struct PreconditionFailure {
    #[prost(message, repeated, tag = "1")]
    violations: Vec<Violation>,
}

#[derive(prost::Message)]
struct Violation {
    #[prost(string, tag = "1")]
    r#type: String,
    #[prost(string, tag = "2")]
    subject: String,
    #[prost(string, tag = "3")]
    description: String,
}

fn make_precondition_failure_any(digest: DigestInfo) -> prost_types::Any {
    let failure = PreconditionFailure {
        violations: vec![Violation {
            r#type: "MISSING".into(),
            subject: format!("blobs/{}/{}", digest.packed_hash(), digest.size_bytes()),
            description: String::new(),
        }],
    };
    prost_types::Any {
        type_url: "type.googleapis.com/google.rpc.PreconditionFailure".into(),
        value: failure.encode_to_vec(),
    }
}

/// Metadata about a file to be materialized from CAS to disk.
struct FileToMaterialize {
    digest: DigestInfo,
    dest: String,
    #[cfg(target_family = "unix")]
    unix_mode: Option<u32>,
    mtime: Option<prost_types::Timestamp>,
}

/// Maximum size for a blob to be eligible for BatchReadBlobs (1 MiB).
/// Blobs larger than this use the existing ByteStream path.
const BATCH_READ_MAX_BLOB_SIZE: u64 = 1024 * 1024;

/// Maximum total payload per BatchReadBlobs request (4 MiB), per REAPI recommendation.
const BATCH_READ_MAX_REQUEST_SIZE: u64 = 4 * 1024 * 1024;

/// Resolve the full directory tree starting from `root_digest`.
///
/// Tries the `GetTree` RPC (single streaming call) if the slow store is a `GrpcStore`.
/// Falls back to recursive `get_and_decode_digest` calls otherwise.
///
/// Returns a map from digest to Directory proto for every directory in the tree.
async fn resolve_directory_tree(
    cas_store: &FastSlowStore,
    root_digest: &DigestInfo,
) -> Result<HashMap<DigestInfo, ProtoDirectory>, Error> {
    let tree_start = std::time::Instant::now();
    // Try the fast path: GetTree RPC via the underlying GrpcStore.
    if let Some(grpc_store) = cas_store.slow_store().downcast_ref::<GrpcStore>(None) {
        let request = GetTreeRequest {
            instance_name: String::new(), // GrpcStore fills this in
            root_digest: Some((*root_digest).into()),
            page_size: 0, // server decides
            page_token: String::new(),
            digest_function: Context::current()
                .get::<DigestHasherFunc>()
                .map_or_else(default_digest_hasher_func, |v| *v)
                .proto_digest_func()
                .into(),
        };

        match grpc_store.get_tree(Request::new(request)).await {
            Ok(response) => {
                let mut stream = response.into_inner();
                let mut tree = HashMap::new();
                let hasher_func = Context::current()
                    .get::<DigestHasherFunc>()
                    .copied()
                    .unwrap_or_else(default_digest_hasher_func);
                while let Some(resp) = stream.message().await.err_tip(|| "In GetTree stream")? {
                    for dir in resp.directories {
                        let encoded = dir.encode_to_vec();
                        let dir_digest =
                            compute_buf_digest(&encoded, &mut hasher_func.hasher());
                        tree.insert(dir_digest, dir);
                    }
                }
                // Validate that the root and ALL referenced child digests
                // are present in the map. Protobuf serialization is not
                // guaranteed deterministic across implementations, so the
                // recomputed digest may differ from the server's stored
                // digest for non-nativelink servers.
                let tree_valid = tree.contains_key(root_digest) && {
                    tree.values().all(|dir| {
                        dir.directories.iter().all(|node| {
                            node.digest
                                .as_ref()
                                .and_then(|d| DigestInfo::try_from(d).ok())
                                .is_some_and(|d| tree.contains_key(&d))
                        })
                    })
                };
                if tree_valid {
                    let elapsed = tree_start.elapsed();
                    let total_bytes: u64 = tree.keys().map(|d| d.size_bytes()).sum();
                    info!(
                        root = ?root_digest,
                        dir_count = tree.len(),
                        total_bytes,
                        elapsed_ms = elapsed.as_millis() as u64,
                        "Resolved directory tree via GetTree RPC"
                    );
                    return Ok(tree);
                }
                // Server returned an incomplete or digest-mismatched tree; fall through.
                warn!(
                    root = ?root_digest,
                    tree_has_root = tree.contains_key(root_digest),
                    tree_size = tree.len(),
                    "GetTree response failed validation, falling back to recursive fetch"
                );
            }
            Err(e) => {
                debug!(
                    root = ?root_digest,
                    err = ?e,
                    "GetTree RPC failed, falling back to recursive fetch"
                );
            }
        }
    }

    // Fallback: recursive fetch (original behavior).
    let mut tree = HashMap::new();
    resolve_directory_tree_recursive(cas_store, root_digest, &mut tree).await?;
    let elapsed = tree_start.elapsed();
    let total_bytes: u64 = tree.keys().map(|d| d.size_bytes()).sum();
    info!(
        root = ?root_digest,
        dir_count = tree.len(),
        total_bytes,
        elapsed_ms = elapsed.as_millis() as u64,
        "Resolved directory tree via recursive fetch"
    );
    Ok(tree)
}

/// Recursively fetch directories via individual `get_and_decode_digest` calls.
fn resolve_directory_tree_recursive<'a>(
    cas_store: &'a FastSlowStore,
    digest: &'a DigestInfo,
    tree: &'a mut HashMap<DigestInfo, ProtoDirectory>,
) -> BoxFuture<'a, Result<(), Error>> {
    async move {
        if tree.contains_key(digest) {
            return Ok(());
        }
        let directory = get_and_decode_digest::<ProtoDirectory>(cas_store, digest.into())
            .await
            .err_tip(|| "Converting digest to Directory in recursive tree fetch")?;
        let child_digests: Vec<DigestInfo> = directory
            .directories
            .iter()
            .map(|d| {
                d.digest
                    .as_ref()
                    .err_tip(|| "Expected Digest in DirectoryNode")?
                    .try_into()
                    .err_tip(|| "Parsing child directory digest in recursive tree fetch")
            })
            .collect::<Result<Vec<_>, _>>()?;
        tree.insert(*digest, directory);
        for child in &child_digests {
            resolve_directory_tree_recursive(cas_store, child, tree).await?;
        }
        Ok(())
    }
    .boxed()
}

/// Walk the resolved directory tree, creating all directories and collecting
/// all files that need to be materialized. Returns the flat list of files.
fn collect_files_from_tree(
    tree: &HashMap<DigestInfo, ProtoDirectory>,
    root_digest: &DigestInfo,
    root_path: &str,
) -> Result<(Vec<FileToMaterialize>, Vec<(String, String)>), Error> {
    let mut files = Vec::new();
    // (symlink_target, dest_path)
    let mut symlinks: Vec<(String, String)> = Vec::new();
    // BFS to create directories in order and collect files.
    let mut queue = VecDeque::new();
    queue.push_back((*root_digest, root_path.to_string()));

    while let Some((dir_digest, dir_path)) = queue.pop_front() {
        let directory = tree.get(&dir_digest).ok_or_else(|| {
            make_err!(
                Code::Internal,
                "Directory {dir_digest:?} not found in resolved tree"
            )
        })?;

        for file in &directory.files {
            let digest: DigestInfo = file
                .digest
                .as_ref()
                .err_tip(|| "Expected Digest in Directory::file::digest")?
                .try_into()
                .err_tip(|| "In Directory::file::digest")?;
            let dest = format!("{}/{}", dir_path, file.name);

            #[cfg(target_family = "unix")]
            let unix_mode = {
                let (_, mut mode) = match &file.node_properties {
                    Some(properties) => (properties.mtime.clone(), properties.unix_mode),
                    None => (None, None),
                };
                if file.is_executable {
                    mode = Some(mode.unwrap_or(0o444) | 0o111);
                }
                mode
            };

            let mtime = file.node_properties.as_ref().and_then(|p| p.mtime.clone());

            files.push(FileToMaterialize {
                digest,
                dest,
                #[cfg(target_family = "unix")]
                unix_mode,
                mtime,
            });
        }

        for subdir in &directory.directories {
            let child_digest: DigestInfo = subdir
                .digest
                .as_ref()
                .err_tip(|| "Expected Digest in Directory::directories::digest")?
                .try_into()
                .err_tip(|| "In Directory::directories::digest")?;
            let child_path = format!("{}/{}", dir_path, subdir.name);
            queue.push_back((child_digest, child_path));
        }

        #[cfg(target_family = "unix")]
        for symlink_node in &directory.symlinks {
            let dest = format!("{}/{}", dir_path, symlink_node.name);
            symlinks.push((symlink_node.target.clone(), dest));
        }
    }

    Ok((files, symlinks))
}

/// Maximum number of concurrent BatchReadBlobs RPCs in flight.
const BATCH_READ_CONCURRENCY: usize = 16;

/// Maximum number of concurrent ByteStream fetches in flight.
const BYTESTREAM_CONCURRENCY: usize = 16;

/// Batch-download small blobs via `BatchReadBlobs` and write them into the fast store.
/// Returns the set of digests that were successfully fetched.
///
/// Batches are sent concurrently (up to `BATCH_READ_CONCURRENCY`) to pipeline
/// RPCs and hide per-batch round-trip latency.
async fn batch_read_small_blobs(
    cas_store: &FastSlowStore,
    small_digests: &[DigestInfo],
) -> Result<HashSet<DigestInfo>, Error> {
    let grpc_store = match cas_store.slow_store().downcast_ref::<GrpcStore>(None) {
        Some(store) => store,
        None => return Ok(HashSet::new()), // Can't batch, caller will use populate_fast_store
    };

    // Partition digests into 4 MiB batches.
    let mut batches: Vec<Vec<DigestInfo>> = Vec::new();
    let mut current_batch: Vec<DigestInfo> = Vec::new();
    let mut current_size: u64 = 0;

    for &digest in small_digests {
        let blob_size = digest.size_bytes();
        if !current_batch.is_empty() && current_size + blob_size > BATCH_READ_MAX_REQUEST_SIZE {
            batches.push(std::mem::take(&mut current_batch));
            current_size = 0;
        }
        current_batch.push(digest);
        current_size += blob_size;
    }
    if !current_batch.is_empty() {
        batches.push(current_batch);
    }

    // Execute batches concurrently with bounded concurrency.
    let fetched: HashSet<DigestInfo> = futures::stream::iter(batches)
        .map(|batch| async move {
            execute_batch_read(grpc_store, cas_store, &batch).await
        })
        .buffer_unordered(BATCH_READ_CONCURRENCY)
        .try_fold(HashSet::new(), |mut acc, completed| async move {
            acc.extend(completed);
            Ok(acc)
        })
        .await?;

    Ok(fetched)
}

/// Execute a single BatchReadBlobs request and write results to fast store.
async fn execute_batch_read(
    grpc_store: &GrpcStore,
    cas_store: &FastSlowStore,
    digests: &[DigestInfo],
) -> Result<Vec<DigestInfo>, Error> {
    let request = BatchReadBlobsRequest {
        instance_name: String::new(), // GrpcStore fills this in
        digests: digests.iter().map(|d| (*d).into()).collect(),
        acceptable_compressors: vec![],
        digest_function: Context::current()
            .get::<DigestHasherFunc>()
            .map_or_else(default_digest_hasher_func, |v| *v)
            .proto_digest_func()
            .into(),
    };

    let response = grpc_store
        .batch_read_blobs(Request::new(request))
        .await
        .err_tip(|| "In execute_batch_read")?
        .into_inner();

    let fast_store = cas_store.fast_store();

    // Parse all valid responses first, then write to fast store concurrently.
    let valid_blobs: Vec<(DigestInfo, Bytes)> = response
        .responses
        .into_iter()
        .filter_map(|blob_resp| {
            let status_code = blob_resp.status.as_ref().map_or(0, |s| s.code);
            if status_code != 0 {
                return None;
            }
            let proto_digest = blob_resp.digest?;
            let digest = DigestInfo::try_from(proto_digest).ok()?;
            Some((digest, Bytes::from(blob_resp.data)))
        })
        .collect();

    // Write all blobs to fast store concurrently.
    let write_futures: FuturesUnordered<_> = valid_blobs
        .into_iter()
        .map(|(digest, data)| {
            let data_len = data.len() as u64;
            async move {
                let (mut tx, rx) = make_buf_channel_pair();
                let store_key: StoreKey<'_> = digest.into();
                let update_fut = fast_store.update(
                    store_key,
                    rx,
                    UploadSizeInfo::ExactSize(data_len),
                );
                let send_fut = async {
                    tx.send(data)
                        .await
                        .err_tip(|| "Sending batch blob to fast store")?;
                    tx.send_eof().err_tip(|| "Sending EOF for batch blob")?;
                    Ok::<_, Error>(())
                };
                let (update_res, send_res) = futures::join!(update_fut, send_fut);
                update_res
                    .merge(send_res)
                    .err_tip(|| format!("Writing batch-read blob {digest:?} to fast store"))?;
                Ok::<DigestInfo, Error>(digest)
            }
        })
        .collect();

    let completed: Vec<DigestInfo> = write_futures.try_collect().await?;

    Ok(completed)
}

/// Populate the fast store for a single digest and hardlink it to `dest`.
/// Contains the retry loop for cache eviction races.
async fn populate_and_hardlink(
    cas_store: &FastSlowStore,
    filesystem_store: Pin<&FilesystemStore>,
    digest: DigestInfo,
    dest: &str,
) -> Result<(), Error> {
    if is_zero_digest(digest) {
        cas_store.populate_fast_store(digest.into()).await?;
        let mut file_slot = fs::create_file(dest).await?;
        std::io::Write::write_all(file_slot.as_std_mut(), &[])
            .err_tip(|| "Could not write to file")?;
        return Ok(());
    }

    const MAX_RETRIES: u32 = 3;
    let mut last_err = None;
    for attempt in 0..MAX_RETRIES {
        if attempt > 0 {
            filesystem_store.remove_entry_for_digest(&digest).await;
        }
        cas_store.populate_fast_store(digest.into()).await?;

        let result = async {
            let file_entry = filesystem_store
                .get_file_entry_for_digest(&digest)
                .await
                .err_tip(|| "Getting file entry for hardlink")?;
            let dest_clone = dest.to_string();
            file_entry
                .get_file_path_locked(move |src| async move {
                    let src_exists = Path::new(&src).exists();
                    let result = fs::hard_link(&src, &dest_clone).await;
                    if result.is_err() {
                        warn!(
                            src = %src.to_string_lossy(),
                            src_exists = src_exists,
                            dest = %dest_clone,
                            "hard_link failed while holding read lock"
                        );
                    }
                    result
                })
                .await
        }
        .await;

        match result {
            Ok(()) => {
                last_err = None;
                break;
            }
            Err(e) if e.code == Code::NotFound => {
                warn!(
                    attempt = attempt + 1,
                    max_retries = MAX_RETRIES,
                    ?digest,
                    dest = %dest,
                    err = ?e,
                    "File evicted from cache during hardlink. Retrying."
                );
                last_err = Some(e);
            }
            Err(e) => {
                return Err(make_err!(
                    Code::Internal,
                    "Could not make hardlink, {e:?} : {dest}"
                ));
            }
        }
    }
    if let Some(e) = last_err {
        return Err(make_err!(
            Code::Internal,
            "Could not make hardlink after {MAX_RETRIES} attempts, \
            file was repeatedly evicted from cache. {e:?} : {dest}\n\
            This error often occurs when the filesystem store's max_bytes is too small for your workload.\n\
            To fix this issue:\n\
            1. Increase the 'max_bytes' value in your filesystem store configuration\n\
            2. Example: Change 'max_bytes: 10000000000' to 'max_bytes: 50000000000' (or higher)\n\
            3. The setting is typically found in your nativelink.json config under:\n\
            stores -> [your_filesystem_store] -> filesystem -> eviction_policy -> max_bytes\n\
            4. Restart NativeLink after making the change\n\n\
            If this error persists after increasing max_bytes several times, please report at:\n\
            https://github.com/TraceMachina/nativelink/issues\n\
            Include your config file and both server and client logs to help us assist you."
        ));
    }
    Ok(())
}

/// Hardlink a file from the filesystem store to the destination, then apply
/// permissions and mtime.
async fn hardlink_and_set_metadata(
    cas_store: &FastSlowStore,
    filesystem_store: Pin<&FilesystemStore>,
    file: FileToMaterialize,
    already_in_cache: bool,
) -> Result<(), Error> {
    let digest = file.digest;
    let dest = file.dest;

    if already_in_cache && !is_zero_digest(digest) {
        // Already in fast store — just hardlink directly (with retry for eviction).
        const MAX_RETRIES: u32 = 3;
        let mut last_err = None;
        for attempt in 0..MAX_RETRIES {
            if attempt > 0 {
                // Re-populate if evicted between cache check and hardlink.
                filesystem_store.remove_entry_for_digest(&digest).await;
                cas_store.populate_fast_store(digest.into()).await?;
            }
            let result = async {
                let file_entry = filesystem_store
                    .get_file_entry_for_digest(&digest)
                    .await
                    .err_tip(|| "Getting file entry for hardlink (cached)")?;
                let dest_clone = dest.clone();
                file_entry
                    .get_file_path_locked(move |src| async move {
                        fs::hard_link(&src, &dest_clone).await
                    })
                    .await
            }
            .await;
            match result {
                Ok(()) => {
                    last_err = None;
                    break;
                }
                Err(e) if e.code == Code::NotFound => {
                    last_err = Some(e);
                }
                Err(e) => {
                    return Err(make_err!(
                        Code::Internal,
                        "Could not make hardlink (cached), {e:?} : {dest}"
                    ));
                }
            }
        }
        if let Some(_e) = last_err {
            // Fall back to full populate+hardlink.
            populate_and_hardlink(cas_store, filesystem_store, digest, &dest).await?;
        }
    } else {
        populate_and_hardlink(cas_store, filesystem_store, digest, &dest).await?;
    }

    // Default CAS file permissions — files in the CAS store are pre-set to 0o555
    // (read+execute for all). Skip chmod when the requested mode matches.
    #[cfg(target_family = "unix")]
    if let Some(unix_mode) = file.unix_mode {
        if unix_mode != 0o555 {
            fs::set_permissions(&dest, Permissions::from_mode(unix_mode))
                .await
                .err_tip(|| format!("Could not set unix mode in download_to_directory {dest}"))?;
        }
    }

    // Apply mtime.
    if let Some(mtime) = file.mtime {
        let dest_owned = dest.clone();
        spawn_blocking!("download_to_directory_set_mtime", move || {
            set_file_mtime(
                &dest_owned,
                FileTime::from_unix_time(mtime.seconds, mtime.nanos as u32),
            )
            .err_tip(|| format!("Failed to set mtime in download_to_directory {dest_owned}"))
        })
        .await
        .err_tip(|| "Failed to launch spawn_blocking in download_to_directory")??;
    }

    Ok(())
}

/// Aggressively download the digests of files and make a local folder from it.
///
/// This optimized version:
/// 1. Resolves the full directory tree via `GetTree` RPC (single streaming call)
///    instead of issuing recursive individual `get_and_decode_digest` calls.
/// 2. Batch-checks which blobs are already in the fast store via `has_with_results`
///    (maps to `FindMissingBlobs` on GrpcStore), avoiding per-file existence RPCs.
/// 3. Fetches small missing blobs (<1 MiB) via `BatchReadBlobs` in 4 MiB batches,
///    with large blobs using the existing ByteStream path.
///
/// We require the `FilesystemStore` to be the `fast` store of `FastSlowStore`.
/// We will request the `FastSlowStore` to populate the entry then we will
/// assume the `FilesystemStore` has the file available immediately after and hardlink the file
/// to a new location.
pub fn download_to_directory<'a>(
    cas_store: &'a FastSlowStore,
    filesystem_store: Pin<&'a FilesystemStore>,
    digest: &'a DigestInfo,
    current_directory: &'a str,
) -> BoxFuture<'a, Result<(), Error>> {
    async move {
        let phase_start = std::time::Instant::now();

        // Step 1: Resolve the full directory tree.
        let tree = resolve_directory_tree(cas_store, digest).await?;
        let tree_resolve_ms = phase_start.elapsed().as_millis();

        // Step 2: Walk the tree, creating all directories and collecting files.
        let (files, symlinks) = collect_files_from_tree(&tree, digest, current_directory)?;

        // Create all subdirectories (BFS order ensures parents are created first).
        {
            let mut dir_queue = VecDeque::new();
            dir_queue.push_back((*digest, current_directory.to_string()));
            while let Some((dir_digest, dir_path)) = dir_queue.pop_front() {
                if let Some(directory) = tree.get(&dir_digest) {
                    for subdir in &directory.directories {
                        let child_digest: DigestInfo = subdir
                            .digest
                            .as_ref()
                            .err_tip(|| "Expected Digest")?
                            .try_into()
                            .err_tip(|| "In Directory::directories::digest")?;
                        let child_path = format!("{}/{}", dir_path, subdir.name);
                        fs::create_dir(&child_path)
                            .await
                            .err_tip(|| format!("Could not create directory {child_path}"))?;
                        dir_queue.push_back((child_digest, child_path));
                    }
                }
            }
        }

        // Create symlinks concurrently.
        #[cfg(target_family = "unix")]
        {
            let symlink_futures: FuturesUnordered<_> = symlinks
                .iter()
                .map(|(target, dest)| async move {
                    fs::symlink(target, dest)
                        .await
                        .err_tip(|| format!("Could not create symlink {target} -> {dest}"))
                })
                .collect();
            symlink_futures
                .try_for_each(|()| futures::future::ready(Ok(())))
                .await?;
        }

        if files.is_empty() {
            return Ok(());
        }

        // Step 3: Batch-check which blobs are already in the fast store.
        // Deduplicate digests first to avoid redundant checks.
        let unique_digests: Vec<DigestInfo> = {
            let mut seen = HashSet::with_capacity(files.len());
            files
                .iter()
                .filter_map(|f| {
                    if seen.insert(f.digest) {
                        Some(f.digest)
                    } else {
                        None
                    }
                })
                .collect()
        };

        let store_keys: Vec<StoreKey<'_>> =
            unique_digests.iter().map(|d| (*d).into()).collect();
        let mut has_results = vec![None; store_keys.len()];
        Pin::new(cas_store.fast_store())
            .has_with_results(&store_keys, &mut has_results)
            .await
            .err_tip(|| "Batch has_with_results on fast store")?;

        let cached_set: HashSet<DigestInfo> = unique_digests
            .iter()
            .zip(has_results.iter())
            .filter_map(|(digest, result)| result.map(|_| *digest))
            .collect();

        let missing_digests: Vec<DigestInfo> = unique_digests
            .iter()
            .zip(has_results.iter())
            .filter_map(|(digest, result)| if result.is_none() { Some(*digest) } else { None })
            .collect();

        let has_check_ms = phase_start.elapsed().as_millis();

        info!(
            total_files = files.len(),
            unique_digests = unique_digests.len(),
            cached = cached_set.len(),
            missing = missing_digests.len(),
            "Batch existence check complete"
        );

        // Step 4: Fetch missing blobs.
        // Partition into small (BatchReadBlobs-eligible) and large (ByteStream),
        // then fetch BOTH concurrently — BatchReadBlobs batches (16 concurrent)
        // and ByteStream fetches (16 concurrent) run in parallel.
        let mut small_missing = Vec::new();
        let mut large_missing = Vec::new();
        for &digest in &missing_digests {
            if is_zero_digest(digest) {
                continue;
            }
            if digest.size_bytes() <= BATCH_READ_MAX_BLOB_SIZE {
                small_missing.push(digest);
            } else {
                large_missing.push(digest);
            }
        }

        debug!(
            small = small_missing.len(),
            large = large_missing.len(),
            "Fetching missing blobs (BatchReadBlobs + ByteStream concurrent)"
        );

        // Launch BatchReadBlobs for small blobs (bounded at BATCH_READ_CONCURRENCY).
        let batch_fut = async {
            if small_missing.is_empty() {
                return Ok::<HashSet<DigestInfo>, Error>(HashSet::new());
            }
            batch_read_small_blobs(cas_store, &small_missing).await
        };

        // Launch ByteStream for large blobs (bounded at BYTESTREAM_CONCURRENCY).
        let bytestream_fut = async {
            if large_missing.is_empty() {
                return Ok::<(), Error>(());
            }
            futures::stream::iter(large_missing.iter().map(Ok::<_, Error>))
                .try_for_each_concurrent(BYTESTREAM_CONCURRENCY, |&digest| async move {
                    cas_store
                        .populate_fast_store_unchecked(digest.into())
                        .await
                        .err_tip(|| format!("Populating fast store for {digest:?}"))
                })
                .await
        };

        // Run both concurrently.
        let (batch_result, bytestream_result) =
            futures::future::join(batch_fut, bytestream_fut).await;
        let batch_fetched = batch_result?;
        bytestream_result?;

        // Any small blobs that BatchReadBlobs failed to fetch — fall back to
        // ByteStream (still bounded at BYTESTREAM_CONCURRENCY).
        let batch_fallback: Vec<DigestInfo> = small_missing
            .iter()
            .filter(|d| !batch_fetched.contains(d))
            .copied()
            .collect();
        if !batch_fallback.is_empty() {
            debug!(count = batch_fallback.len(), "Fetching BatchReadBlobs fallback via ByteStream");
            futures::stream::iter(batch_fallback.iter().map(Ok::<_, Error>))
                .try_for_each_concurrent(BYTESTREAM_CONCURRENCY, |&digest| async move {
                    cas_store
                        .populate_fast_store_unchecked(digest.into())
                        .await
                        .err_tip(|| format!("Populating fast store (fallback) for {digest:?}"))
                })
                .await?;
        }

        let fetch_ms = phase_start.elapsed().as_millis();

        // Step 5: Hardlink all files from the fast store to the work directory.
        // By this point, all non-zero digests have been populated into the fast
        // store (via cache hit, BatchReadBlobs, or ByteStream). Pass
        // already_in_cache=true so hardlink_and_set_metadata skips the redundant
        // populate_fast_store call on the first attempt.
        let hardlink_futures: FuturesUnordered<_> = files
            .into_iter()
            .map(|file| {
                let in_cache = !is_zero_digest(file.digest);
                async move {
                    let digest = file.digest;
                    hardlink_and_set_metadata(cas_store, filesystem_store, file, in_cache)
                        .await
                        .map_err(move |e| {
                            let mut e = e.append(format!("for digest {digest}"));
                            if e.code == Code::NotFound {
                                e.details.push(make_precondition_failure_any(digest));
                            }
                            e
                        })
                }
            })
            .collect();
        hardlink_futures
            .try_for_each(|()| futures::future::ready(Ok(())))
            .await?;

        let total_bytes: u64 = unique_digests.iter().map(|d| d.size_bytes()).sum();
        let total_ms = phase_start.elapsed().as_millis();
        info!(
            tree_resolve_ms,
            has_check_ms = has_check_ms - tree_resolve_ms,
            fetch_ms = fetch_ms - has_check_ms,
            hardlink_ms = total_ms - fetch_ms,
            total_ms,
            num_files = unique_digests.len(),
            total_bytes,
            throughput_mbps = format!("{:.1}", throughput_mbps(total_bytes, phase_start.elapsed())),
            "download_to_directory completed",
        );

        Ok(())
    }
    .boxed()
}

/// Prepares action inputs by first trying the directory cache (if available),
/// then falling back to traditional `download_to_directory`.
///
/// This provides a significant performance improvement for repeated builds
/// with the same input directories.
pub async fn prepare_action_inputs(
    directory_cache: &Option<Arc<crate::directory_cache::DirectoryCache>>,
    cas_store: &FastSlowStore,
    filesystem_store: Pin<&FilesystemStore>,
    digest: &DigestInfo,
    work_directory: &str,
) -> Result<(), Error> {
    // Try cache first if available
    if let Some(cache) = directory_cache {
        match cache
            .get_or_create(*digest, Path::new(work_directory))
            .await
        {
            Ok(cache_hit) => {
                trace!(
                    ?digest,
                    work_directory, cache_hit, "Successfully prepared inputs via directory cache"
                );
                return Ok(());
            }
            Err(e) => {
                warn!(
                    ?digest,
                    ?e,
                    "Directory cache failed, falling back to traditional download"
                );
                // Fall through to traditional path
            }
        }
    }

    // Traditional path (cache disabled or failed)
    download_to_directory(cas_store, filesystem_store, digest, work_directory).await
}

#[cfg(target_family = "windows")]
fn is_executable(_metadata: &std::fs::Metadata, full_path: &impl AsRef<Path>) -> bool {
    static EXECUTABLE_EXTENSIONS: &[&str] = &["exe", "bat", "com"];
    EXECUTABLE_EXTENSIONS
        .iter()
        .any(|ext| full_path.as_ref().extension().map_or(false, |v| v == *ext))
}

#[cfg(target_family = "unix")]
fn is_executable(metadata: &std::fs::Metadata, _full_path: &impl AsRef<Path>) -> bool {
    (metadata.mode() & 0o111) != 0
}

type DigestUploader = Arc<tokio::sync::OnceCell<()>>;

async fn upload_file(
    cas_store: Pin<&impl StoreLike>,
    full_path: impl AsRef<Path> + Debug + Send + Sync,
    hasher: DigestHasherFunc,
    metadata: std::fs::Metadata,
    digest_uploaders: Arc<Mutex<HashMap<DigestInfo, DigestUploader>>>,
) -> Result<FileInfo, Error> {
    let is_executable = is_executable(&metadata, &full_path);
    let file_size = metadata.len();
    let file = fs::open_file(&full_path, 0)
        .await
        .err_tip(|| format!("Could not open file {full_path:?}"))?;

    let (digest, mut file) = hasher
        .hasher()
        .digest_for_file(&full_path, file, Some(file_size))
        .await
        .err_tip(|| format!("Failed to hash file in digest_for_file failed for {full_path:?}"))?;

    let digest_uploader = match digest_uploaders.lock().entry(digest) {
        std::collections::hash_map::Entry::Occupied(occupied_entry) => occupied_entry.get().clone(),
        std::collections::hash_map::Entry::Vacant(vacant_entry) => vacant_entry
            .insert(Arc::new(tokio::sync::OnceCell::new()))
            .clone(),
    };

    // Only upload a file with a given hash once.  The file may exist multiple
    // times in the output with different names.
    digest_uploader
        .get_or_try_init(async || {
            // Only upload if the digest doesn't already exist, this should be
            // a much cheaper operation than an upload.
            let cas_store = cas_store.as_store_driver_pin();
            let store_key: StoreKey<'_> = digest.into();
            let has_start = std::time::Instant::now();
            if cas_store
                .has(store_key.borrow())
                .await
                .is_ok_and(|result| result.is_some())
            {
                trace!(
                    ?digest,
                    has_elapsed_ms = has_start.elapsed().as_millis(),
                    "upload_file: digest already exists in CAS, skipping upload",
                );
                return Ok(());
            }
            trace!(
                ?digest,
                has_elapsed_ms = has_start.elapsed().as_millis(),
                file_size = digest.size_bytes(),
                "upload_file: digest not in CAS, starting upload",
            );

            std::io::Seek::seek(file.as_std_mut(), std::io::SeekFrom::Start(0))
                .err_tip(|| "Could not rewind file")?;

            // Note: For unknown reasons we appear to be hitting:
            // https://github.com/rust-lang/rust/issues/92096
            // or a similar issue if we try to use the non-store driver function, so we
            // are using the store driver function here.
            let store_key_for_upload = store_key.clone();
            let file_upload_start = std::time::Instant::now();
            let upload_result = cas_store
                .update_with_whole_file(
                    store_key_for_upload,
                    full_path.as_ref().into(),
                    file,
                    UploadSizeInfo::ExactSize(digest.size_bytes()),
                )
                .await
                .map(|_slot| ());
            let upload_elapsed = file_upload_start.elapsed();

            match &upload_result {
                Ok(()) => {
                    info!(
                        ?digest,
                        size_bytes = digest.size_bytes(),
                        elapsed_ms = upload_elapsed.as_millis() as u64,
                        throughput_mbps = format!("{:.1}", throughput_mbps(digest.size_bytes(), upload_elapsed)),
                        "upload_file: CAS write completed",
                    );
                }
                Err(e) => {
                    error!(
                        ?digest,
                        size_bytes = digest.size_bytes(),
                        elapsed_ms = upload_elapsed.as_millis() as u64,
                        ?e,
                        "upload_file: CAS write failed",
                    );
                }
            }

            match upload_result {
                Ok(()) => Ok(()),
                Err(err) => {
                    // Output uploads run concurrently and may overlap (e.g. a file is listed
                    // both as an output file and inside an output directory). When another
                    // upload has already moved the file into CAS, this update can fail with
                    // NotFound even though the digest is now present. Per the RE spec, missing
                    // outputs should be ignored, so treat this as success if the digest exists.
                    if err.code == Code::NotFound
                        && cas_store
                            .has(store_key.borrow())
                            .await
                            .is_ok_and(|result| result.is_some())
                    {
                        Ok(())
                    } else {
                        Err(err)
                    }
                }
            }
        })
        .await
        .err_tip(|| format!("for {full_path:?}"))?;

    let name = full_path
        .as_ref()
        .file_name()
        .err_tip(|| format!("Expected file_name to exist on {full_path:?}"))?
        .to_str()
        .err_tip(|| {
            make_err!(
                Code::Internal,
                "Could not convert {:?} to string",
                full_path
            )
        })?
        .to_string();

    Ok(FileInfo {
        name_or_path: NameOrPath::Name(name),
        digest,
        is_executable,
    })
}

async fn upload_symlink(
    full_path: impl AsRef<Path> + Debug,
    full_work_directory_path: impl AsRef<Path>,
) -> Result<SymlinkInfo, Error> {
    let full_target_path = fs::read_link(full_path.as_ref())
        .await
        .err_tip(|| format!("Could not get read_link path of {full_path:?}"))?;

    // Detect if our symlink is inside our work directory, if it is find the
    // relative path otherwise use the absolute path.
    let target = if full_target_path.starts_with(full_work_directory_path.as_ref()) {
        full_target_path
            .strip_prefix(full_work_directory_path.as_ref())
            .map_err(|e| make_err!(Code::Internal, "Could not strip work dir prefix: {}", e))?
            .to_str()
            .err_tip(|| {
                make_err!(
                    Code::Internal,
                    "Could not convert '{:?}' to string",
                    full_target_path
                )
            })?
            .to_string()
    } else {
        full_target_path
            .to_str()
            .err_tip(|| {
                make_err!(
                    Code::Internal,
                    "Could not convert '{:?}' to string",
                    full_target_path
                )
            })?
            .to_string()
    };

    let name = full_path
        .as_ref()
        .file_name()
        .err_tip(|| format!("Expected file_name to exist on {full_path:?}"))?
        .to_str()
        .err_tip(|| {
            make_err!(
                Code::Internal,
                "Could not convert {:?} to string",
                full_path
            )
        })?
        .to_string();

    Ok(SymlinkInfo {
        name_or_path: NameOrPath::Name(name),
        target,
    })
}

fn upload_directory<'a, P: AsRef<Path> + Debug + Send + Sync + Clone + 'a>(
    cas_store: Pin<&'a impl StoreLike>,
    full_dir_path: P,
    full_work_directory: &'a str,
    hasher: DigestHasherFunc,
    digest_uploaders: Arc<Mutex<HashMap<DigestInfo, DigestUploader>>>,
) -> BoxFuture<'a, Result<(Directory, VecDeque<ProtoDirectory>), Error>> {
    Box::pin(async move {
        let file_futures = FuturesUnordered::new();
        let dir_futures = FuturesUnordered::new();
        let symlink_futures = FuturesUnordered::new();
        {
            let (_permit, dir_handle) = fs::read_dir(&full_dir_path)
                .await
                .err_tip(|| format!("Error reading dir for reading {full_dir_path:?}"))?
                .into_inner();
            let mut dir_stream = ReadDirStream::new(dir_handle);
            // Note: Try very hard to not leave file descriptors open. Try to keep them as short
            // lived as possible. This is why we iterate the directory and then build a bunch of
            // futures with all the work we are wanting to do then execute it. It allows us to
            // close the directory iterator file descriptor, then open the child files/folders.
            while let Some(entry_result) = dir_stream.next().await {
                let entry = entry_result.err_tip(|| "Error while iterating directory")?;
                let file_type = entry
                    .file_type()
                    .await
                    .err_tip(|| format!("Error running file_type() on {entry:?}"))?;
                let full_path = full_dir_path.as_ref().join(entry.path());
                if file_type.is_dir() {
                    let full_dir_path = full_dir_path.clone();
                    dir_futures.push(
                        upload_directory(
                            cas_store,
                            full_path.clone(),
                            full_work_directory,
                            hasher,
                            digest_uploaders.clone(),
                        )
                        .and_then(|(dir, all_dirs)| async move {
                            let directory_name = full_path
                                .file_name()
                                .err_tip(|| {
                                    format!("Expected file_name to exist on {full_dir_path:?}")
                                })?
                                .to_str()
                                .err_tip(|| {
                                    make_err!(
                                        Code::Internal,
                                        "Could not convert {:?} to string",
                                        full_dir_path
                                    )
                                })?
                                .to_string();

                            let digest =
                                serialize_and_upload_message(&dir, cas_store, &mut hasher.hasher())
                                    .await
                                    .err_tip(|| format!("for {}", full_path.display()))?;

                            Result::<(DirectoryNode, VecDeque<Directory>), Error>::Ok((
                                DirectoryNode {
                                    name: directory_name,
                                    digest: Some(digest.into()),
                                },
                                all_dirs,
                            ))
                        })
                        .boxed(),
                    );
                } else if file_type.is_file() {
                    let digest_uploaders = digest_uploaders.clone();
                    file_futures.push(async move {
                        let metadata = fs::metadata(&full_path)
                            .await
                            .err_tip(|| format!("Could not open file {}", full_path.display()))?;
                        upload_file(cas_store, &full_path, hasher, metadata, digest_uploaders)
                            .map_ok(TryInto::try_into)
                            .await?
                    });
                } else if file_type.is_symlink() {
                    symlink_futures.push(
                        upload_symlink(full_path, &full_work_directory)
                            .map(|symlink| symlink?.try_into()),
                    );
                }
            }
        }

        let (mut file_nodes, dir_entries, mut symlinks) = try_join3(
            file_futures.try_collect::<Vec<FileNode>>(),
            dir_futures.try_collect::<Vec<(DirectoryNode, VecDeque<Directory>)>>(),
            symlink_futures.try_collect::<Vec<SymlinkNode>>(),
        )
        .await?;

        let mut directory_nodes = Vec::with_capacity(dir_entries.len());
        // For efficiency we use a deque because it allows cheap concat of Vecs.
        // We make the assumption here that when performance is important it is because
        // our directory is quite large. This allows us to cheaply merge large amounts of
        // directories into one VecDeque. Then after we are done we need to collapse it
        // down into a single Vec.
        let mut all_child_directories = VecDeque::with_capacity(dir_entries.len());
        for (directory_node, mut recursive_child_directories) in dir_entries {
            directory_nodes.push(directory_node);
            all_child_directories.append(&mut recursive_child_directories);
        }

        file_nodes.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        directory_nodes.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        symlinks.sort_unstable_by(|a, b| a.name.cmp(&b.name));

        let directory = Directory {
            files: file_nodes,
            directories: directory_nodes,
            symlinks,
            node_properties: None, // We don't support file properties.
        };
        all_child_directories.push_back(directory.clone());

        Ok((directory, all_child_directories))
    })
}

async fn process_side_channel_file(
    side_channel_file: Cow<'_, OsStr>,
    args: &[&OsStr],
    timeout: Duration,
) -> Result<Option<Error>, Error> {
    let mut json_contents = String::new();
    {
        // Note: Scoping `file_slot` allows the file_slot semaphore to be released faster.
        let mut file_slot = match fs::open_file(side_channel_file, 0).await {
            Ok(file_slot) => file_slot,
            Err(e) => {
                if e.code != Code::NotFound {
                    return Err(e).err_tip(|| "Error opening side channel file");
                }
                // Note: If file does not exist, it's ok. Users are not required to create this file.
                return Ok(None);
            }
        };
        std::io::Read::read_to_string(file_slot.as_std_mut(), &mut json_contents)
            .err_tip(|| "Error reading side channel file")?;
    }

    let side_channel_info: SideChannelInfo =
        serde_json5::from_str(&json_contents).map_err(|e| {
            make_input_err!(
                "Could not convert contents of side channel file (json) to SideChannelInfo : {e:?}"
            )
        })?;
    Ok(side_channel_info.failure.map(|failure| match failure {
        SideChannelFailureReason::Timeout => Error::new(
            Code::DeadlineExceeded,
            format!(
                "Command '{}' timed out after {} seconds",
                args.join(OsStr::new(" ")).to_string_lossy(),
                timeout.as_secs_f32()
            ),
        ),
    }))
}

async fn do_cleanup(
    running_actions_manager: &Arc<RunningActionsManagerImpl>,
    operation_id: &OperationId,
    action_directory: &str,
) -> Result<(), Error> {
    // Mark this operation as being cleaned up
    let Some(_cleaning_guard) = running_actions_manager.perform_cleanup(operation_id.clone())
    else {
        // Cleanup is already happening elsewhere.
        return Ok(());
    };

    debug!("Worker cleaning up");
    // Note: We need to be careful to keep trying to cleanup even if one of the steps fails.
    let remove_dir_result = match fs::remove_dir_all(action_directory).await {
        Ok(()) => Ok(()),
        Err(_) => {
            // On macOS, Spotlight/Finder can momentarily recreate files
            // (e.g. .DS_Store) during deletion, causing ENOTEMPTY. A
            // short delay and single retry is sufficient.
            tokio::time::sleep(Duration::from_millis(100)).await;
            fs::remove_dir_all(action_directory).await
        }
    }
    .err_tip(|| format!("Could not remove working directory {action_directory}"));

    if let Err(err) = running_actions_manager.cleanup_action(operation_id) {
        error!(%operation_id, ?err, "Error cleaning up action");
        Result::<(), Error>::Err(err).merge(remove_dir_result)
    } else if let Err(err) = remove_dir_result {
        error!(%operation_id, ?err, "Error removing working directory");
        Err(err)
    } else {
        Ok(())
    }
}

pub trait RunningAction: Sync + Send + Sized + Unpin + 'static {
    /// Returns the action id of the action.
    fn get_operation_id(&self) -> &OperationId;

    /// Anything that needs to execute before the actions is actually executed should happen here.
    fn prepare_action(self: Arc<Self>) -> impl Future<Output = Result<Arc<Self>, Error>> + Send;

    /// Actually perform the execution of the action.
    fn execute(self: Arc<Self>) -> impl Future<Output = Result<Arc<Self>, Error>> + Send;

    /// Any uploading, processing or analyzing of the results should happen here.
    fn upload_results(self: Arc<Self>) -> impl Future<Output = Result<Arc<Self>, Error>> + Send;

    /// Cleanup any residual files, handles or other junk resulting from running the action.
    fn cleanup(self: Arc<Self>) -> impl Future<Output = Result<Arc<Self>, Error>> + Send;

    /// Returns the final result. As a general rule this action should be thought of as
    /// a consumption of `self`, meaning once a return happens here the lifetime of `Self`
    /// is over and any action performed on it after this call is undefined behavior.
    fn get_finished_result(
        self: Arc<Self>,
    ) -> impl Future<Output = Result<ActionResult, Error>> + Send;

    /// Returns the work directory of the action.
    fn get_work_directory(&self) -> &String;
}

#[derive(Debug)]
struct RunningActionImplExecutionResult {
    stdout: Bytes,
    stderr: Bytes,
    exit_code: i32,
}

#[derive(Debug)]
struct RunningActionImplState {
    command_proto: Option<ProtoCommand>,
    // TODO(palfrey) Kill is not implemented yet, but is instrumented.
    // However, it is used if the worker disconnects to destroy current jobs.
    kill_channel_tx: Option<oneshot::Sender<()>>,
    kill_channel_rx: Option<oneshot::Receiver<()>>,
    execution_result: Option<RunningActionImplExecutionResult>,
    action_result: Option<ActionResult>,
    execution_metadata: ExecutionMetadata,
    // If there was an internal error, this will be set.
    // This should NOT be set if everything was fine, but the process had a
    // non-zero exit code. Instead this should be used for internal errors
    // that prevented the action from running, upload failures, timeouts, exc...
    // but we have (or could have) the action results (like stderr/stdout).
    error: Option<Error>,
}

#[derive(Debug)]
pub struct RunningActionImpl {
    operation_id: OperationId,
    action_directory: String,
    work_directory: String,
    action_info: ActionInfo,
    timeout: Duration,
    running_actions_manager: Arc<RunningActionsManagerImpl>,
    state: Mutex<RunningActionImplState>,
    has_manager_entry: AtomicBool,
    did_cleanup: AtomicBool,
}

impl RunningActionImpl {
    pub fn new(
        execution_metadata: ExecutionMetadata,
        operation_id: OperationId,
        action_directory: String,
        action_info: ActionInfo,
        timeout: Duration,
        running_actions_manager: Arc<RunningActionsManagerImpl>,
    ) -> Self {
        let work_directory = format!("{}/{}", action_directory, "work");
        let (kill_channel_tx, kill_channel_rx) = oneshot::channel();
        Self {
            operation_id,
            action_directory,
            work_directory,
            action_info,
            timeout,
            running_actions_manager,
            state: Mutex::new(RunningActionImplState {
                command_proto: None,
                kill_channel_rx: Some(kill_channel_rx),
                kill_channel_tx: Some(kill_channel_tx),
                execution_result: None,
                action_result: None,
                execution_metadata,
                error: None,
            }),
            // Always need to ensure that we're removed from the manager on Drop.
            has_manager_entry: AtomicBool::new(true),
            // Only needs to be cleaned up after a prepare_action call, set there.
            did_cleanup: AtomicBool::new(true),
        }
    }

    #[allow(
        clippy::missing_const_for_fn,
        reason = "False positive on stable, but not on nightly"
    )]
    fn metrics(&self) -> &Arc<Metrics> {
        &self.running_actions_manager.metrics
    }

    /// Prepares any actions needed to execute this action. This action will do the following:
    ///
    /// * Download any files needed to execute the action
    /// * Build a folder with all files needed to execute the action.
    ///
    /// This function will aggressively download and spawn potentially thousands of futures. It is
    /// up to the stores to rate limit if needed.
    async fn inner_prepare_action(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        {
            let mut state = self.state.lock();
            state.execution_metadata.input_fetch_start_timestamp =
                (self.running_actions_manager.callbacks.now_fn)();
        }
        let command = {
            // Download and build out our input files/folders. Also fetch and decode our Command.
            let command_fut = self.metrics().get_proto_command_from_store.wrap(async {
                get_and_decode_digest::<ProtoCommand>(
                    self.running_actions_manager.cas_store.as_ref(),
                    self.action_info.command_digest.into(),
                )
                .await
                .err_tip(|| "Converting command_digest to Command")
            });
            let filesystem_store_pin =
                Pin::new(self.running_actions_manager.filesystem_store.as_ref());
            let (command, ()) = try_join(command_fut, async {
                fs::create_dir(&self.work_directory)
                    .await
                    .err_tip(|| format!("Error creating work directory {}", self.work_directory))?;
                // Now the work directory has been created, we have to clean up.
                self.did_cleanup.store(false, Ordering::Release);
                // Download the input files/folder and place them into the temp directory.
                // Use directory cache if available for better performance.
                self.metrics()
                    .download_to_directory
                    .wrap(prepare_action_inputs(
                        &self.running_actions_manager.directory_cache,
                        &self.running_actions_manager.cas_store,
                        filesystem_store_pin,
                        &self.action_info.input_root_digest,
                        &self.work_directory,
                    ))
                    .await
            })
            .await?;
            command
        };
        {
            // Create all directories needed for our output paths. This is required by the bazel spec.
            let prepare_output_directories = |output_file| {
                let full_output_path = if command.working_directory.is_empty() {
                    format!("{}/{}", self.work_directory, output_file)
                } else {
                    format!(
                        "{}/{}/{}",
                        self.work_directory, command.working_directory, output_file
                    )
                };
                async move {
                    let full_parent_path = Path::new(&full_output_path)
                        .parent()
                        .err_tip(|| format!("Parent path for {full_output_path} has no parent"))?;
                    fs::create_dir_all(full_parent_path).await.err_tip(|| {
                        format!(
                            "Error creating output directory {} (file)",
                            full_parent_path.display()
                        )
                    })?;
                    Result::<(), Error>::Ok(())
                }
            };
            self.metrics()
                .prepare_output_files
                .wrap(try_join_all(
                    command.output_files.iter().map(prepare_output_directories),
                ))
                .await?;
            self.metrics()
                .prepare_output_paths
                .wrap(try_join_all(
                    command.output_paths.iter().map(prepare_output_directories),
                ))
                .await?;
        }
        debug!(?command, "Worker received command");
        {
            let mut state = self.state.lock();
            state.command_proto = Some(command);
            state.execution_metadata.input_fetch_completed_timestamp =
                (self.running_actions_manager.callbacks.now_fn)();
        }
        Ok(self)
    }

    async fn inner_execute(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        let (command_proto, mut kill_channel_rx) = {
            let mut state = self.state.lock();
            state.execution_metadata.execution_start_timestamp =
                (self.running_actions_manager.callbacks.now_fn)();
            (
                state
                    .command_proto
                    .take()
                    .err_tip(|| "Expected state to have command_proto in execute()")?,
                state
                    .kill_channel_rx
                    .take()
                    .err_tip(|| "Expected state to have kill_channel_rx in execute()")?
                    // This is important as we may be killed at any point.
                    .fuse(),
            )
        };
        if command_proto.arguments.is_empty() {
            return Err(make_input_err!("No arguments provided in Command proto"));
        }
        let args: Vec<&OsStr> = if let Some(entrypoint) = &self
            .running_actions_manager
            .execution_configuration
            .entrypoint
        {
            core::iter::once(entrypoint.as_ref())
                .chain(command_proto.arguments.iter().map(AsRef::as_ref))
                .collect()
        } else {
            command_proto.arguments.iter().map(AsRef::as_ref).collect()
        };
        // TODO(palfrey): This should probably be in debug, but currently
        //                    that's too busy and we often rely on this to
        //                    figure out toolchain misconfiguration issues.
        //                    De-bloat the `debug` level by using the `trace`
        //                    level more effectively and adjust this.
        info!(?args, "Executing command",);
        let mut command_builder = process::Command::new(args[0]);
        command_builder
            .args(&args[1..])
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .current_dir(format!(
                "{}/{}",
                self.work_directory, command_proto.working_directory
            ))
            .env_clear();

        let requested_timeout = if self.action_info.timeout.is_zero() {
            self.running_actions_manager.max_action_timeout
        } else {
            self.action_info.timeout
        };

        let mut maybe_side_channel_file: Option<Cow<'_, OsStr>> = None;
        if let Some(additional_environment) = &self
            .running_actions_manager
            .execution_configuration
            .additional_environment
        {
            for (name, source) in additional_environment {
                let value = match source {
                    EnvironmentSource::Property(property) => self
                        .action_info
                        .platform_properties
                        .get(property)
                        .map_or_else(|| Cow::Borrowed(""), |v| Cow::Borrowed(v.as_str())),
                    EnvironmentSource::Value(value) => Cow::Borrowed(value.as_str()),
                    EnvironmentSource::FromEnvironment => {
                        Cow::Owned(env::var(name).unwrap_or_default())
                    }
                    EnvironmentSource::TimeoutMillis => {
                        Cow::Owned(requested_timeout.as_millis().to_string())
                    }
                    EnvironmentSource::SideChannelFile => {
                        let file_cow =
                            format!("{}/{}", self.action_directory, Uuid::new_v4().simple());
                        maybe_side_channel_file = Some(Cow::Owned(file_cow.clone().into()));
                        Cow::Owned(file_cow)
                    }
                    EnvironmentSource::ActionDirectory => {
                        Cow::Borrowed(self.action_directory.as_str())
                    }
                };
                command_builder.env(name, value.as_ref());
            }
        }

        #[cfg(target_family = "unix")]
        let envs = &command_proto.environment_variables;
        // If SystemRoot is not set on windows we set it to default. Failing to do
        // this causes all commands to fail.
        #[cfg(target_family = "windows")]
        let envs = {
            let mut envs = command_proto.environment_variables.clone();
            if !envs.iter().any(|v| v.name.to_uppercase() == "SYSTEMROOT") {
                envs.push(
                    nativelink_proto::build::bazel::remote::execution::v2::command::EnvironmentVariable {
                        name: "SystemRoot".to_string(),
                        value: "C:\\Windows".to_string(),
                    },
                );
            }
            if !envs.iter().any(|v| v.name.to_uppercase() == "PATH") {
                envs.push(
                    nativelink_proto::build::bazel::remote::execution::v2::command::EnvironmentVariable {
                        name: "PATH".to_string(),
                        value: "C:\\Windows\\System32".to_string(),
                    },
                );
            }
            envs
        };
        for environment_variable in envs {
            command_builder.env(&environment_variable.name, &environment_variable.value);
        }

        let mut child_process = command_builder
            .spawn()
            .err_tip(|| format!("Could not execute command {args:?}"))?;
        let mut stdout_reader = child_process
            .stdout
            .take()
            .err_tip(|| "Expected stdout to exist on command this should never happen")?;
        let mut stderr_reader = child_process
            .stderr
            .take()
            .err_tip(|| "Expected stderr to exist on command this should never happen")?;

        let mut child_process_guard = guard(child_process, |mut child_process| {
            let result: Result<Option<std::process::ExitStatus>, std::io::Error> =
                child_process.try_wait();
            match result {
                Ok(res) if res.is_some() => {
                    // The child already exited, probably a timeout or kill operation
                }
                result => {
                    error!(
                        ?result,
                        "Child process was not cleaned up before dropping the call to execute(), killing in background spawn."
                    );
                    background_spawn!("running_actions_manager_kill_child_process", async move {
                        child_process.kill().await
                    });
                }
            }
        });

        let all_stdout_fut = spawn!("stdout_reader", async move {
            let mut all_stdout = BytesMut::new();
            loop {
                let sz = stdout_reader
                    .read_buf(&mut all_stdout)
                    .await
                    .err_tip(|| "Error reading stdout stream")?;
                if sz == 0 {
                    break; // EOF.
                }
            }
            Result::<Bytes, Error>::Ok(all_stdout.freeze())
        });
        let all_stderr_fut = spawn!("stderr_reader", async move {
            let mut all_stderr = BytesMut::new();
            loop {
                let sz = stderr_reader
                    .read_buf(&mut all_stderr)
                    .await
                    .err_tip(|| "Error reading stderr stream")?;
                if sz == 0 {
                    break; // EOF.
                }
            }
            Result::<Bytes, Error>::Ok(all_stderr.freeze())
        });
        let mut killed_action = false;

        let timer = self.metrics().child_process.begin_timer();
        let mut sleep_fut = (self.running_actions_manager.callbacks.sleep_fn)(self.timeout).fuse();
        loop {
            tokio::select! {
                () = &mut sleep_fut => {
                    self.running_actions_manager.metrics.task_timeouts.inc();
                    killed_action = true;
                    if let Err(err) = child_process_guard.kill().await {
                        error!(
                            ?err,
                            "Could not kill process in RunningActionsManager for action timeout",
                        );
                    }
                    {
                        let joined_command = args.join(OsStr::new(" "));
                        let command = joined_command.to_string_lossy();
                        info!(
                            seconds = self.action_info.timeout.as_secs_f32(),
                            %command,
                            "Command timed out"
                        );
                        let mut state = self.state.lock();
                        state.error = Error::merge_option(state.error.take(), Some(Error::new(
                            Code::DeadlineExceeded,
                            format!(
                                "Command '{}' timed out after {} seconds",
                                command,
                                self.action_info.timeout.as_secs_f32()
                            )
                        )));
                    }
                },
                maybe_exit_status = child_process_guard.wait() => {
                    // Defuse our guard so it does not try to cleanup and make senseless logs.
                    drop(ScopeGuard::<_, _>::into_inner(child_process_guard));
                    let exit_status = maybe_exit_status.err_tip(|| "Failed to collect exit code of process")?;
                    // TODO(palfrey) We should implement stderr/stdout streaming to client here.
                    // If we get killed before the stream is started, then these will lock up.
                    // TODO(palfrey) There is a significant bug here. If we kill the action and the action creates
                    // child processes, it can create zombies. See: https://github.com/tracemachina/nativelink/issues/225
                    let (stdout, stderr) = if killed_action {
                        drop(timer);
                        (Bytes::new(), Bytes::new())
                    } else {
                        timer.measure();
                        let (maybe_all_stdout, maybe_all_stderr) = tokio::join!(all_stdout_fut, all_stderr_fut);
                        (
                            maybe_all_stdout.err_tip(|| "Internal error reading from stdout of worker task")??,
                            maybe_all_stderr.err_tip(|| "Internal error reading from stderr of worker task")??
                        )
                    };

                    let exit_code = exit_status.code().map_or(EXIT_CODE_FOR_SIGNAL, |exit_code| {
                        if exit_code == 0 {
                            self.metrics().child_process_success_error_code.inc();
                        } else {
                            self.metrics().child_process_failure_error_code.inc();
                        }
                        exit_code
                    });

                    info!(?args, "Command complete");

                    let maybe_error_override = if let Some(side_channel_file) = maybe_side_channel_file {
                        process_side_channel_file(side_channel_file.clone(), &args, requested_timeout).await
                        .err_tip(|| format!("Error processing side channel file: {}", side_channel_file.display()))?
                    } else {
                        None
                    };
                    {
                        let mut state = self.state.lock();
                        state.error = Error::merge_option(state.error.take(), maybe_error_override);

                        state.command_proto = Some(command_proto);
                        state.execution_result = Some(RunningActionImplExecutionResult{
                            stdout,
                            stderr,
                            exit_code,
                        });
                        state.execution_metadata.execution_completed_timestamp = (self.running_actions_manager.callbacks.now_fn)();
                    }
                    return Ok(self);
                },
                _ = &mut kill_channel_rx => {
                    killed_action = true;
                    if let Err(err) = child_process_guard.kill().await {
                        error!(
                            operation_id = ?self.operation_id,
                            ?err,
                            "Could not kill process",
                        );
                    }
                    {
                        let mut state = self.state.lock();
                        state.error = Error::merge_option(state.error.take(), Some(Error::new(
                            Code::Aborted,
                            format!(
                                "Command '{}' was killed by scheduler",
                                args.join(OsStr::new(" ")).to_string_lossy()
                            )
                        )));
                    }
                },
            }
        }
        // Unreachable.
    }

    async fn inner_upload_results(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        enum OutputType {
            None,
            File(FileInfo),
            Directory(DirectoryInfo),
            FileSymlink(SymlinkInfo),
            DirectorySymlink(SymlinkInfo),
        }

        let upload_start = std::time::Instant::now();
        debug!(
            operation_id = ?self.operation_id,
            "Worker uploading results - starting",
        );
        let (mut command_proto, execution_result, mut execution_metadata) = {
            let mut state = self.state.lock();
            state.execution_metadata.output_upload_start_timestamp =
                (self.running_actions_manager.callbacks.now_fn)();
            (
                state
                    .command_proto
                    .take()
                    .err_tip(|| "Expected state to have command_proto in execute()")?,
                state
                    .execution_result
                    .take()
                    .err_tip(|| "Execution result does not exist at upload_results stage")?,
                state.execution_metadata.clone(),
            )
        };
        // Upload outputs to the fast store (local FilesystemStore) only.
        // The slow store (remote CAS) upload is deferred to the background
        // after the execution result is reported, reducing latency.
        let cas_store = self.running_actions_manager.cas_store.fast_store();
        let hasher = self.action_info.unique_qualifier.digest_function();

        let mut output_path_futures = FuturesUnordered::new();
        let mut output_paths = command_proto.output_paths;
        if output_paths.is_empty() {
            output_paths
                .reserve(command_proto.output_files.len() + command_proto.output_directories.len());
            output_paths.append(&mut command_proto.output_files);
            output_paths.append(&mut command_proto.output_directories);
        }
        let digest_uploaders = Arc::new(Mutex::new(HashMap::new()));
        for entry in output_paths {
            let full_path = OsString::from(if command_proto.working_directory.is_empty() {
                format!("{}/{}", self.work_directory, entry)
            } else {
                format!(
                    "{}/{}/{}",
                    self.work_directory, command_proto.working_directory, entry
                )
            });
            let work_directory = &self.work_directory;
            let digest_uploaders = digest_uploaders.clone();
            output_path_futures.push(async move {
                let metadata = {
                    let metadata = match fs::symlink_metadata(&full_path).await {
                        Ok(file) => file,
                        Err(e) => {
                            if e.code == Code::NotFound {
                                // In the event our output does not exist, according to the bazel remote
                                // execution spec, we simply ignore it continue.
                                return Result::<OutputType, Error>::Ok(OutputType::None);
                            }
                            return Err(e).err_tip(|| {
                                format!("Could not open file {}", full_path.display())
                            });
                        }
                    };

                    if metadata.is_file() {
                        return Ok(OutputType::File(
                            upload_file(
                                cas_store.as_pin(),
                                &full_path,
                                hasher,
                                metadata,
                                digest_uploaders,
                            )
                            .await
                            .map(|mut file_info| {
                                file_info.name_or_path = NameOrPath::Path(entry);
                                file_info
                            })
                            .err_tip(|| format!("Uploading file {}", full_path.display()))?,
                        ));
                    }
                    metadata
                };
                if metadata.is_dir() {
                    Ok(OutputType::Directory(
                        upload_directory(
                            cas_store.as_pin(),
                            &full_path,
                            work_directory,
                            hasher,
                            digest_uploaders,
                        )
                        .and_then(|(root_dir, children)| async move {
                            let tree = ProtoTree {
                                root: Some(root_dir),
                                children: children.into(),
                            };
                            let tree_digest = serialize_and_upload_message(
                                &tree,
                                cas_store.as_pin(),
                                &mut hasher.hasher(),
                            )
                            .await
                            .err_tip(|| format!("While processing {entry}"))?;
                            Ok(DirectoryInfo {
                                path: entry,
                                tree_digest,
                            })
                        })
                        .await
                        .err_tip(|| format!("Uploading directory {}", full_path.display()))?,
                    ))
                } else if metadata.is_symlink() {
                    let output_symlink = upload_symlink(&full_path, work_directory)
                        .await
                        .map(|mut symlink_info| {
                            symlink_info.name_or_path = NameOrPath::Path(entry);
                            symlink_info
                        })
                        .err_tip(|| format!("Uploading symlink {}", full_path.display()))?;
                    match fs::metadata(&full_path).await {
                        Ok(metadata) => {
                            if metadata.is_dir() {
                                Ok(OutputType::DirectorySymlink(output_symlink))
                            } else {
                                // Note: If it's anything but directory we put it as a file symlink.
                                Ok(OutputType::FileSymlink(output_symlink))
                            }
                        }
                        Err(e) => {
                            if e.code != Code::NotFound {
                                return Err(e).err_tip(|| {
                                    format!(
                                        "While querying target symlink metadata for {}",
                                        full_path.display()
                                    )
                                });
                            }
                            // If the file doesn't exist, we consider it a file. Even though the
                            // file doesn't exist we still need to populate an entry.
                            Ok(OutputType::FileSymlink(output_symlink))
                        }
                    }
                } else {
                    Err(make_err!(
                        Code::Internal,
                        "{full_path:?} was not a file, folder or symlink. Must be one.",
                    ))
                }
            });
        }
        let mut output_files = vec![];
        let mut output_folders = vec![];
        let mut output_directory_symlinks = vec![];
        let mut output_file_symlinks = vec![];

        if execution_result.exit_code != 0 {
            let stdout = core::str::from_utf8(&execution_result.stdout).unwrap_or("<no-utf8>");
            let stderr = core::str::from_utf8(&execution_result.stderr).unwrap_or("<no-utf8>");
            error!(
                exit_code = ?execution_result.exit_code,
                stdout = ?stdout[..min(stdout.len(), 1000)],
                stderr = ?stderr[..min(stderr.len(), 1000)],
                "Command returned non-zero exit code",
            );
        }

        let stdout_digest_fut = self.metrics().upload_stdout.wrap(async {
            let start = std::time::Instant::now();
            let data = execution_result.stdout;
            let data_len = data.len();
            let digest = compute_buf_digest(&data, &mut hasher.hasher());
            cas_store
                .update_oneshot(digest, data)
                .await
                .err_tip(|| "Uploading stdout")?;
            let elapsed = start.elapsed();
            info!(
                ?digest,
                size_bytes = data_len,
                elapsed_ms = elapsed.as_millis() as u64,
                throughput_mbps = format!("{:.1}", throughput_mbps(data_len as u64, elapsed)),
                "upload_results: stdout upload completed",
            );
            Result::<DigestInfo, Error>::Ok(digest)
        });
        let stderr_digest_fut = self.metrics().upload_stderr.wrap(async {
            let start = std::time::Instant::now();
            let data = execution_result.stderr;
            let data_len = data.len();
            let digest = compute_buf_digest(&data, &mut hasher.hasher());
            cas_store
                .update_oneshot(digest, data)
                .await
                .err_tip(|| "Uploading  stderr")?;
            let elapsed = start.elapsed();
            info!(
                ?digest,
                size_bytes = data_len,
                elapsed_ms = elapsed.as_millis() as u64,
                throughput_mbps = format!("{:.1}", throughput_mbps(data_len as u64, elapsed)),
                "upload_results: stderr upload completed",
            );
            Result::<DigestInfo, Error>::Ok(digest)
        });

        debug!(
            operation_id = ?self.operation_id,
            num_output_paths = output_path_futures.len(),
            "upload_results: starting stdout/stderr/output_paths uploads",
        );
        let join_start = std::time::Instant::now();
        let upload_result = futures::try_join!(stdout_digest_fut, stderr_digest_fut, async {
            while let Some(output_type) = output_path_futures.try_next().await? {
                match output_type {
                    OutputType::File(output_file) => output_files.push(output_file),
                    OutputType::Directory(output_folder) => output_folders.push(output_folder),
                    OutputType::FileSymlink(output_symlink) => {
                        output_file_symlinks.push(output_symlink);
                    }
                    OutputType::DirectorySymlink(output_symlink) => {
                        output_directory_symlinks.push(output_symlink);
                    }
                    OutputType::None => { /* Safe to ignore */ }
                }
            }
            Ok(())
        });
        drop(output_path_futures);
        debug!(
            operation_id = ?self.operation_id,
            elapsed_ms = join_start.elapsed().as_millis(),
            success = upload_result.is_ok(),
            "upload_results: all uploads completed",
        );
        let (stdout_digest, stderr_digest) = match upload_result {
            Ok((stdout_digest, stderr_digest, ())) => (stdout_digest, stderr_digest),
            Err(e) => return Err(e).err_tip(|| "Error while uploading results"),
        };

        execution_metadata.output_upload_completed_timestamp =
            (self.running_actions_manager.callbacks.now_fn)();
        output_files.sort_unstable_by(|a, b| a.name_or_path.cmp(&b.name_or_path));
        output_folders.sort_unstable_by(|a, b| a.path.cmp(&b.path));
        output_file_symlinks.sort_unstable_by(|a, b| a.name_or_path.cmp(&b.name_or_path));
        output_directory_symlinks.sort_unstable_by(|a, b| a.name_or_path.cmp(&b.name_or_path));
        let num_output_files = output_files.len();
        let num_output_folders = output_folders.len();
        {
            let mut state = self.state.lock();
            execution_metadata.worker_completed_timestamp =
                (self.running_actions_manager.callbacks.now_fn)();

            // Log phase durations for every action so we can diagnose latency.
            let duration_ms = |start: SystemTime, end: SystemTime| -> i64 {
                end.duration_since(start)
                    .map(|d| d.as_millis() as i64)
                    .unwrap_or_else(|e| -(e.duration().as_millis() as i64))
            };
            let em = &execution_metadata;
            info!(
                operation_id = ?self.operation_id,
                queue_ms = duration_ms(em.queued_timestamp, em.worker_start_timestamp),
                input_fetch_ms = duration_ms(em.input_fetch_start_timestamp, em.input_fetch_completed_timestamp),
                execution_ms = duration_ms(em.execution_start_timestamp, em.execution_completed_timestamp),
                output_upload_ms = duration_ms(em.output_upload_start_timestamp, em.output_upload_completed_timestamp),
                worker_overhead_ms = duration_ms(em.worker_start_timestamp, em.input_fetch_start_timestamp),
                total_worker_ms = duration_ms(em.worker_start_timestamp, em.worker_completed_timestamp),
                "Action phase timing",
            );

            state.action_result = Some(ActionResult {
                output_files,
                output_folders,
                output_directory_symlinks,
                output_file_symlinks,
                exit_code: execution_result.exit_code,
                stdout_digest,
                stderr_digest,
                execution_metadata,
                server_logs: HashMap::default(), // TODO(palfrey) Not implemented.
                error: state.error.clone(),
                message: String::new(), // Will be filled in on cache_action_result if needed.
            });
        }
        debug!(
            operation_id = ?self.operation_id,
            total_elapsed_ms = upload_start.elapsed().as_millis(),
            num_output_files,
            num_output_folders,
            "upload_results: inner_upload_results completed successfully",
        );
        Ok(self)
    }

    async fn inner_get_finished_result(self: Arc<Self>) -> Result<ActionResult, Error> {
        let mut state = self.state.lock();
        state
            .action_result
            .take()
            .err_tip(|| "Expected action_result to exist in get_finished_result")
    }
}

impl Drop for RunningActionImpl {
    fn drop(&mut self) {
        if self.did_cleanup.load(Ordering::Acquire) {
            if self.has_manager_entry.load(Ordering::Acquire) {
                drop(
                    self.running_actions_manager
                        .cleanup_action(&self.operation_id),
                );
            }
            return;
        }
        let operation_id = self.operation_id.clone();
        error!(
            %operation_id,
            "RunningActionImpl did not cleanup. This is a violation of the requirements, will attempt to do it in the background."
        );
        let running_actions_manager = self.running_actions_manager.clone();
        let action_directory = self.action_directory.clone();
        background_spawn!("running_action_impl_drop", async move {
            let Err(err) =
                do_cleanup(&running_actions_manager, &operation_id, &action_directory).await
            else {
                return;
            };
            error!(
                %operation_id,
                ?action_directory,
                ?err,
                "Error cleaning up action"
            );
        });
    }
}

impl RunningAction for RunningActionImpl {
    fn get_operation_id(&self) -> &OperationId {
        &self.operation_id
    }

    async fn prepare_action(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        let res = self
            .metrics()
            .clone()
            .prepare_action
            .wrap(Self::inner_prepare_action(self))
            .await;
        if let Err(ref e) = res {
            warn!(?e, "Error during prepare_action");
        }
        res
    }

    async fn execute(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        let res = self
            .metrics()
            .clone()
            .execute
            .wrap(Self::inner_execute(self))
            .await;
        if let Err(ref e) = res {
            warn!(?e, "Error during prepare_action");
        }
        res
    }

    async fn upload_results(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        let upload_timeout = self.running_actions_manager.max_upload_timeout;
        let operation_id = self.operation_id.clone();
        info!(
            ?operation_id,
            upload_timeout_s = upload_timeout.as_secs(),
            "upload_results: starting with timeout",
        );
        let metrics = self.metrics().clone();
        let upload_fut = metrics
            .upload_results
            .wrap(Self::inner_upload_results(self));

        let stall_warned = AtomicBool::new(false);
        let stall_warn_fut = async {
            let mut elapsed_secs = 0u64;
            loop {
                tokio::time::sleep(Duration::from_secs(60)).await;
                elapsed_secs += 60;
                stall_warned.store(true, Ordering::Relaxed);
                warn!(
                    ?operation_id,
                    elapsed_s = elapsed_secs,
                    timeout_s = upload_timeout.as_secs(),
                    "upload_results: still in progress — possible stall",
                );
            }
        };

        let upload_start = Instant::now();
        let res = tokio::time::timeout(upload_timeout, async {
            tokio::pin!(upload_fut);
            tokio::pin!(stall_warn_fut);
            tokio::select! {
                result = &mut upload_fut => result,
                () = &mut stall_warn_fut => unreachable!(),
            }
        })
        .await
        .map_err(|_| {
            make_err!(
                Code::DeadlineExceeded,
                "Upload results timed out after {}s for operation {:?}",
                upload_timeout.as_secs(),
                operation_id,
            )
        })?;
        match &res {
            Ok(_) if stall_warned.load(Ordering::Relaxed) => {
                info!(
                    ?operation_id,
                    elapsed_s = upload_start.elapsed().as_secs(),
                    "upload_results: completed after stall",
                );
            }
            Err(e) => {
                warn!(?operation_id, ?e, "Error during upload_results");
            }
            _ => {}
        }
        res
    }

    async fn cleanup(self: Arc<Self>) -> Result<Arc<Self>, Error> {
        let res = self
            .metrics()
            .clone()
            .cleanup
            .wrap(async move {
                let result = do_cleanup(
                    &self.running_actions_manager,
                    &self.operation_id,
                    &self.action_directory,
                )
                .await;
                self.has_manager_entry.store(false, Ordering::Release);
                self.did_cleanup.store(true, Ordering::Release);
                result.map(move |()| self)
            })
            .await;
        if let Err(ref e) = res {
            warn!(?e, "Error during cleanup");
        }
        res
    }

    async fn get_finished_result(self: Arc<Self>) -> Result<ActionResult, Error> {
        self.metrics()
            .clone()
            .get_finished_result
            .wrap(Self::inner_get_finished_result(self))
            .await
    }

    fn get_work_directory(&self) -> &String {
        &self.work_directory
    }
}

pub trait RunningActionsManager: Sync + Send + Sized + Unpin + 'static {
    type RunningAction: RunningAction;

    fn create_and_add_action(
        self: &Arc<Self>,
        worker_id: String,
        start_execute: StartExecute,
    ) -> impl Future<Output = Result<Arc<Self::RunningAction>, Error>> + Send;

    fn cache_action_result(
        &self,
        action_digest: DigestInfo,
        action_result: &mut ActionResult,
        hasher: DigestHasherFunc,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    fn kill_all(&self) -> impl Future<Output = ()> + Send;

    fn kill_operation(
        &self,
        operation_id: &OperationId,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    /// Spawn a background task to upload action output blobs from the local
    /// fast store to the remote slow store. No-op by default.
    fn spawn_upload_to_remote(self: &Arc<Self>, _action_result: &ActionResult) {}

    fn metrics(&self) -> &Arc<Metrics>;
}

/// A function to get the current system time, used to allow mocking for tests
type NowFn = fn() -> SystemTime;
type SleepFn = fn(Duration) -> BoxFuture<'static, ()>;

/// Functions that may be injected for testing purposes, during standard control
/// flows these are specified by the new function.
#[derive(Clone, Copy)]
pub struct Callbacks {
    /// A function that gets the current time.
    pub now_fn: NowFn,
    /// A function that sleeps for a given Duration.
    pub sleep_fn: SleepFn,
}

impl Debug for Callbacks {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Callbacks").finish_non_exhaustive()
    }
}

/// The set of additional information for executing an action over and above
/// those given in the `ActionInfo` passed to the worker.  This allows
/// modification of the action for execution on this particular worker.  This
/// may be used to run the action with a particular set of additional
/// environment variables, or perhaps configure it to execute within a
/// container.
#[derive(Debug, Default)]
pub struct ExecutionConfiguration {
    /// If set, will be executed instead of the first argument passed in the
    /// `ActionInfo` with all of the arguments in the `ActionInfo` passed as
    /// arguments to this command.
    pub entrypoint: Option<String>,
    /// The only environment variables that will be specified when the command
    /// executes other than those in the `ActionInfo`.  On Windows, `SystemRoot`
    /// and PATH are also assigned (see `inner_execute`).
    pub additional_environment: Option<HashMap<String, EnvironmentSource>>,
}

#[derive(Debug)]
struct UploadActionResults {
    upload_ac_results_strategy: UploadCacheResultsStrategy,
    upload_historical_results_strategy: UploadCacheResultsStrategy,
    ac_store: Option<Store>,
    historical_store: Store,
    success_message_template: Template,
    failure_message_template: Template,
}

impl UploadActionResults {
    fn new(
        config: &UploadActionResultConfig,
        ac_store: Option<Store>,
        historical_store: Store,
    ) -> Result<Self, Error> {
        let upload_historical_results_strategy = config
            .upload_historical_results_strategy
            .unwrap_or(DEFAULT_HISTORICAL_RESULTS_STRATEGY);
        if !matches!(
            config.upload_ac_results_strategy,
            UploadCacheResultsStrategy::Never
        ) && ac_store.is_none()
        {
            return Err(make_input_err!(
                "upload_ac_results_strategy is set, but no ac_store is configured"
            ));
        }
        Ok(Self {
            upload_ac_results_strategy: config.upload_ac_results_strategy,
            upload_historical_results_strategy,
            ac_store,
            historical_store,
            success_message_template: Template::new(&config.success_message_template).map_err(
                |e| {
                    make_input_err!(
                        "Could not convert success_message_template to rust template: {} : {e:?}",
                        config.success_message_template
                    )
                },
            )?,
            failure_message_template: Template::new(&config.failure_message_template).map_err(
                |e| {
                    make_input_err!(
                        "Could not convert failure_message_template to rust template: {} : {e:?}",
                        config.success_message_template
                    )
                },
            )?,
        })
    }

    const fn should_cache_result(
        strategy: UploadCacheResultsStrategy,
        action_result: &ActionResult,
        treat_infra_error_as_failure: bool,
    ) -> bool {
        let did_fail = action_result.exit_code != 0
            || (treat_infra_error_as_failure && action_result.error.is_some());
        match strategy {
            UploadCacheResultsStrategy::SuccessOnly => !did_fail,
            UploadCacheResultsStrategy::Never => false,
            // Never cache internal errors or timeouts.
            UploadCacheResultsStrategy::Everything => {
                treat_infra_error_as_failure || action_result.error.is_none()
            }
            UploadCacheResultsStrategy::FailuresOnly => did_fail,
        }
    }

    /// Formats the message field in `ExecuteResponse` from the `success_message_template`
    /// or `failure_message_template` config templates.
    fn format_execute_response_message(
        mut template_str: Template,
        action_digest_info: DigestInfo,
        maybe_historical_digest_info: Option<DigestInfo>,
        hasher: DigestHasherFunc,
    ) -> Result<String, Error> {
        template_str.replace(
            "digest_function",
            hasher.proto_digest_func().as_str_name().to_lowercase(),
        );
        template_str.replace(
            "action_digest_hash",
            action_digest_info.packed_hash().to_string(),
        );
        template_str.replace("action_digest_size", action_digest_info.size_bytes());
        if let Some(historical_digest_info) = maybe_historical_digest_info {
            template_str.replace(
                "historical_results_hash",
                format!("{}", historical_digest_info.packed_hash()),
            );
            template_str.replace(
                "historical_results_size",
                historical_digest_info.size_bytes(),
            );
        } else {
            template_str.replace("historical_results_hash", "");
            template_str.replace("historical_results_size", "");
        }
        template_str
            .text()
            .map_err(|e| make_input_err!("Could not convert template to text: {e:?}"))
    }

    async fn upload_ac_results(
        &self,
        action_digest: DigestInfo,
        action_result: ProtoActionResult,
        hasher: DigestHasherFunc,
    ) -> Result<(), Error> {
        let Some(ac_store) = self.ac_store.as_ref() else {
            return Ok(());
        };
        // If we are a GrpcStore we shortcut here, as this is a special store.
        if let Some(grpc_store) = ac_store.downcast_ref::<GrpcStore>(Some(action_digest.into())) {
            let update_action_request = UpdateActionResultRequest {
                // This is populated by `update_action_result`.
                instance_name: String::new(),
                action_digest: Some(action_digest.into()),
                action_result: Some(action_result),
                results_cache_policy: None,
                digest_function: hasher.proto_digest_func().into(),
            };
            let size_bytes = update_action_request.encoded_len() as u64;
            let start = std::time::Instant::now();
            grpc_store
                .update_action_result(Request::new(update_action_request))
                .await
                .map(|_| ())
                .err_tip(|| "Caching ActionResult")?;
            let elapsed = start.elapsed();
            info!(
                ?action_digest,
                size_bytes,
                elapsed_ms = elapsed.as_millis() as u64,
                throughput_mbps = format!("{:.1}", throughput_mbps(size_bytes, elapsed)),
                "AC write completed (grpc)",
            );
            return Ok(());
        }

        let mut store_data = BytesMut::with_capacity(ESTIMATED_DIGEST_SIZE);
        action_result
            .encode(&mut store_data)
            .err_tip(|| "Encoding ActionResult for caching")?;

        let size_bytes = store_data.len() as u64;
        let start = std::time::Instant::now();
        ac_store
            .update_oneshot(action_digest, store_data.split().freeze())
            .await
            .err_tip(|| "Caching ActionResult")?;
        let elapsed = start.elapsed();
        info!(
            ?action_digest,
            size_bytes,
            elapsed_ms = elapsed.as_millis() as u64,
            throughput_mbps = format!("{:.1}", throughput_mbps(size_bytes, elapsed)),
            "AC write completed",
        );
        Ok(())
    }

    async fn upload_historical_results_with_message(
        &self,
        action_digest: DigestInfo,
        execute_response: ExecuteResponse,
        message_template: Template,
        hasher: DigestHasherFunc,
    ) -> Result<String, Error> {
        let historical_digest_info = serialize_and_upload_message(
            &HistoricalExecuteResponse {
                action_digest: Some(action_digest.into()),
                execute_response: Some(execute_response.clone()),
            },
            self.historical_store.as_pin(),
            &mut hasher.hasher(),
        )
        .await
        .err_tip(|| format!("Caching HistoricalExecuteResponse for digest: {action_digest}"))?;

        Self::format_execute_response_message(
            message_template,
            action_digest,
            Some(historical_digest_info),
            hasher,
        )
        .err_tip(|| "Could not format message in upload_historical_results_with_message")
    }

    async fn cache_action_result(
        &self,
        action_info: DigestInfo,
        action_result: &mut ActionResult,
        hasher: DigestHasherFunc,
    ) -> Result<(), Error> {
        let should_upload_historical_results =
            Self::should_cache_result(self.upload_historical_results_strategy, action_result, true);
        let should_upload_ac_results =
            Self::should_cache_result(self.upload_ac_results_strategy, action_result, false);
        // Shortcut so we don't need to convert to proto if not needed.
        if !should_upload_ac_results && !should_upload_historical_results {
            return Ok(());
        }

        let execute_response = to_execute_response(action_result.clone());

        // In theory exit code should always be != 0 if there's an error, but for safety we
        // catch both.
        let message_template = if action_result.exit_code == 0 && action_result.error.is_none() {
            self.success_message_template.clone()
        } else {
            self.failure_message_template.clone()
        };

        // Extract AC result proto before concurrent uploads (independent of message).
        let ac_result_proto = if should_upload_ac_results {
            Some(
                execute_response
                    .result
                    .clone()
                    .err_tip(|| "No result set in cache_action_result")?,
            )
        } else {
            None
        };

        // Run historical + AC uploads concurrently — they are independent.
        let historical_fut = async {
            if should_upload_historical_results {
                match self
                    .upload_historical_results_with_message(
                        action_info,
                        execute_response,
                        message_template,
                        hasher,
                    )
                    .await
                {
                    Ok(message) => Ok(Some(message)),
                    Err(e) => Err(e),
                }
            } else {
                match Self::format_execute_response_message(
                    message_template,
                    action_info,
                    None,
                    hasher,
                ) {
                    Ok(message) => Ok(Some(message)),
                    Err(e) => {
                        Err(e).err_tip(|| "Could not format message in cache_action_result")
                    }
                }
            }
        };

        let ac_fut = async {
            if let Some(proto) = ac_result_proto {
                self.upload_ac_results(action_info, proto, hasher).await
            } else {
                Ok(())
            }
        };

        let (historical_result, ac_result) = futures::future::join(historical_fut, ac_fut).await;

        // Apply message from historical upload.
        if let Ok(Some(message)) = &historical_result {
            action_result.message.clone_from(message);
        }

        historical_result
            .map(|_| ())
            .merge(ac_result)
    }
}

#[derive(Debug)]
pub struct RunningActionsManagerArgs<'a> {
    pub root_action_directory: String,
    pub execution_configuration: ExecutionConfiguration,
    pub cas_store: Arc<FastSlowStore>,
    pub ac_store: Option<Store>,
    pub historical_store: Store,
    pub upload_action_result_config: &'a UploadActionResultConfig,
    pub max_action_timeout: Duration,
    pub max_upload_timeout: Duration,
    pub timeout_handled_externally: bool,
    pub directory_cache: Option<Arc<crate::directory_cache::DirectoryCache>>,
    /// Worker-local locality map for registering peer hints from StartExecute.
    /// When present, peer_hints from the scheduler are registered here so that
    /// WorkerProxyStore can fetch blobs from peer workers.
    pub peer_locality_map: Option<nativelink_util::blob_locality_map::SharedBlobLocalityMap>,
}

struct CleanupGuard {
    manager: Weak<RunningActionsManagerImpl>,
    operation_id: OperationId,
}

impl Drop for CleanupGuard {
    fn drop(&mut self) {
        let Some(manager) = self.manager.upgrade() else {
            return;
        };
        let mut cleaning = manager.cleaning_up_operations.lock();
        cleaning.remove(&self.operation_id);
        manager.cleanup_complete_notify.notify_waiters();
    }
}

/// Holds state info about what is being executed and the interface for interacting
/// with actions while they are running.
#[derive(Debug)]
pub struct RunningActionsManagerImpl {
    root_action_directory: String,
    execution_configuration: ExecutionConfiguration,
    cas_store: Arc<FastSlowStore>,
    filesystem_store: Arc<FilesystemStore>,
    upload_action_results: UploadActionResults,
    max_action_timeout: Duration,
    max_upload_timeout: Duration,
    timeout_handled_externally: bool,
    running_actions: Mutex<HashMap<OperationId, Weak<RunningActionImpl>>>,
    // Note: We don't use Notify because we need to support a .wait_for()-like function, which
    // Notify does not support.
    action_done_tx: watch::Sender<()>,
    callbacks: Callbacks,
    metrics: Arc<Metrics>,
    /// Track operations being cleaned up to avoid directory collisions during action retries.
    /// When an action fails and is retried on the same worker, we need to ensure the previous
    /// attempt's directory is fully cleaned up before creating a new one.
    /// See: <https://github.com/TraceMachina/nativelink/issues/1859>
    cleaning_up_operations: Mutex<HashSet<OperationId>>,
    /// Notify waiters when a cleanup operation completes. This is used in conjunction with
    /// `cleaning_up_operations` to coordinate directory cleanup and creation.
    cleanup_complete_notify: Arc<Notify>,
    /// Optional directory cache for improving performance by caching reconstructed
    /// input directories and using hardlinks.
    directory_cache: Option<Arc<crate::directory_cache::DirectoryCache>>,
    /// Worker-local locality map for registering peer hints from StartExecute.
    peer_locality_map: Option<nativelink_util::blob_locality_map::SharedBlobLocalityMap>,
}

impl RunningActionsManagerImpl {
    /// Maximum time to wait for a cleanup operation to complete before timing out.
    /// TODO(marcussorealheis): Consider making cleanup wait timeout configurable in the future
    const MAX_WAIT: Duration = Duration::from_secs(30);
    /// Maximum backoff duration for exponential backoff when waiting for cleanup.
    const MAX_BACKOFF: Duration = Duration::from_millis(500);
    pub fn new_with_callbacks(
        args: RunningActionsManagerArgs<'_>,
        callbacks: Callbacks,
    ) -> Result<Self, Error> {
        // Sadly because of some limitations of how Any works we need to clone more times than optimal.
        let filesystem_store = args
            .cas_store
            .fast_store()
            .downcast_ref::<FilesystemStore>(None)
            .err_tip(
                || "Expected FilesystemStore store for .fast_store() in RunningActionsManagerImpl",
            )?
            .get_arc()
            .err_tip(|| "FilesystemStore's internal Arc was lost")?;
        let (action_done_tx, _) = watch::channel(());
        Ok(Self {
            root_action_directory: args.root_action_directory,
            execution_configuration: args.execution_configuration,
            cas_store: args.cas_store,
            filesystem_store,
            upload_action_results: UploadActionResults::new(
                args.upload_action_result_config,
                args.ac_store,
                args.historical_store,
            )
            .err_tip(|| "During RunningActionsManagerImpl construction")?,
            max_action_timeout: args.max_action_timeout,
            max_upload_timeout: args.max_upload_timeout,
            timeout_handled_externally: args.timeout_handled_externally,
            running_actions: Mutex::new(HashMap::new()),
            action_done_tx,
            callbacks,
            metrics: Arc::new(Metrics::default()),
            cleaning_up_operations: Mutex::new(HashSet::new()),
            cleanup_complete_notify: Arc::new(Notify::new()),
            directory_cache: args.directory_cache,
            peer_locality_map: args.peer_locality_map,
        })
    }

    pub fn new(args: RunningActionsManagerArgs<'_>) -> Result<Self, Error> {
        Self::new_with_callbacks(
            args,
            Callbacks {
                now_fn: SystemTime::now,
                sleep_fn: |duration| Box::pin(tokio::time::sleep(duration)),
            },
        )
    }

    /// Spawn a background task that uploads all action output blobs from the
    /// fast store (local FilesystemStore) to the slow store (remote CAS).
    /// This is called after the execution result has been reported to the
    /// scheduler, so it does not block action completion latency.
    pub fn spawn_upload_to_remote(self: &Arc<Self>, action_result: &ActionResult) {
        let slow_store = self.cas_store.slow_store();
        if slow_store
            .inner_store(None::<StoreKey<'_>>)
            .optimized_for(StoreOptimizations::NoopUpdates)
        {
            return;
        }
        // Respect slow_direction config — when set to Get or ReadOnly,
        // the slow store should not receive writes (same check as
        // FastSlowStore::update).
        let dir = self.cas_store.slow_direction();
        if dir == StoreDirection::Get || dir == StoreDirection::ReadOnly {
            return;
        }

        let mut digests = Vec::new();
        for file in &action_result.output_files {
            if file.digest.size_bytes() > 0 {
                digests.push(file.digest);
            }
        }
        for folder in &action_result.output_folders {
            if folder.tree_digest.size_bytes() > 0 {
                digests.push(folder.tree_digest);
            }
        }
        if action_result.stdout_digest.size_bytes() > 0 {
            digests.push(action_result.stdout_digest);
        }
        if action_result.stderr_digest.size_bytes() > 0 {
            digests.push(action_result.stderr_digest);
        }
        if digests.is_empty() {
            return;
        }

        let cas_store = self.cas_store.clone();
        let total = digests.len();
        tokio::spawn(async move {
            let fast_store = cas_store.fast_store();
            let slow_store = cas_store.slow_store();
            let start = std::time::Instant::now();
            info!(
                total_digests = total,
                "upload_to_remote: starting background CAS upload",
            );

            // Small blobs use update_oneshot which routes through
            // BatchUpdateBlobs for efficient coalescing. Large blobs
            // stream through a channel to avoid loading into memory.
            const BATCH_THRESHOLD: u64 = 1024 * 1024; // 1 MiB

            let mut uploads = FuturesUnordered::new();
            for digest in digests {
                uploads.push(async move {
                    let result = if digest.size_bytes() <= BATCH_THRESHOLD {
                        match fast_store.get_part_unchunked(digest, 0, None).await {
                            Ok(data) => slow_store.update_oneshot(digest, data).await,
                            Err(e) => Err(e),
                        }
                    } else {
                        let (tx, rx) = make_buf_channel_pair();
                        let read_fut = fast_store.get(digest, tx);
                        let write_fut = slow_store.update(
                            digest,
                            rx,
                            UploadSizeInfo::ExactSize(digest.size_bytes()),
                        );
                        let (read_res, write_res) = tokio::join!(read_fut, write_fut);
                        read_res.merge(write_res)
                    };
                    if let Err(e) = result {
                        warn!(
                            ?digest,
                            ?e,
                            "upload_to_remote: failed to upload digest",
                        );
                    }
                });
            }
            while uploads.next().await.is_some() {}

            info!(
                total_digests = total,
                elapsed_ms = start.elapsed().as_millis() as u64,
                "upload_to_remote: background CAS upload completed",
            );
        });
    }

    /// Fixes a race condition that occurs when an action fails to execute on a worker, and the same worker
    /// attempts to re-execute the same action before the physical cleanup (file is removed) completes.
    /// See this issue for additional details: <https://github.com/TraceMachina/nativelink/issues/1859>
    async fn wait_for_cleanup_if_needed(&self, operation_id: &OperationId) -> Result<(), Error> {
        let start = Instant::now();
        let mut backoff = Duration::from_millis(10);
        let mut has_waited = false;

        loop {
            let should_wait = {
                let cleaning = self.cleaning_up_operations.lock();
                cleaning.contains(operation_id)
            };

            if !should_wait {
                let dir_path =
                    PathBuf::from(&self.root_action_directory).join(operation_id.to_string());

                if !dir_path.exists() {
                    return Ok(());
                }

                // Safety check: ensure we're only removing directories under root_action_directory
                let root_path = Path::new(&self.root_action_directory);
                let canonical_root = root_path.canonicalize().err_tip(|| {
                    format!(
                        "Failed to canonicalize root directory: {}",
                        self.root_action_directory
                    )
                })?;
                let canonical_dir = dir_path.canonicalize().err_tip(|| {
                    format!("Failed to canonicalize directory: {}", dir_path.display())
                })?;

                if !canonical_dir.starts_with(&canonical_root) {
                    return Err(make_err!(
                        Code::Internal,
                        "Attempted to remove directory outside of root_action_directory: {}",
                        dir_path.display()
                    ));
                }

                // Directory exists but not being cleaned - remove it
                warn!(
                    "Removing stale directory for {}: {}",
                    operation_id,
                    dir_path.display()
                );
                self.metrics.stale_removals.inc();

                // Try to remove the directory, with one retry on failure
                let remove_result = fs::remove_dir_all(&dir_path).await;
                if let Err(e) = remove_result {
                    // Retry once after a short delay in case the directory is temporarily locked
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    fs::remove_dir_all(&dir_path).await.err_tip(|| {
                        format!(
                            "Failed to remove stale directory {} for retry of {} after retry (original error: {})",
                            dir_path.display(),
                            operation_id,
                            e
                        )
                    })?;
                }
                return Ok(());
            }

            if start.elapsed() > Self::MAX_WAIT {
                self.metrics.cleanup_wait_timeouts.inc();
                return Err(make_err!(
                    Code::DeadlineExceeded,
                    "Timeout waiting for previous operation cleanup: {} (waited {:?})",
                    operation_id,
                    start.elapsed()
                ));
            }

            if !has_waited {
                self.metrics.cleanup_waits.inc();
                has_waited = true;
            }

            trace!(
                "Waiting for cleanup of {} (elapsed: {:?}, backoff: {:?})",
                operation_id,
                start.elapsed(),
                backoff
            );

            tokio::select! {
                () = self.cleanup_complete_notify.notified() => {},
                () = tokio::time::sleep(backoff) => {
                    // Exponential backoff
                    backoff = (backoff * 2).min(Self::MAX_BACKOFF);
                },
            }
        }
    }

    fn make_action_directory<'a>(
        &'a self,
        operation_id: &'a OperationId,
    ) -> impl Future<Output = Result<String, Error>> + 'a {
        self.metrics.make_action_directory.wrap(async move {
            let action_directory = format!("{}/{}", self.root_action_directory, operation_id);
            fs::create_dir(&action_directory)
                .await
                .err_tip(|| format!("Error creating action directory {action_directory}"))?;
            Ok(action_directory)
        })
    }

    fn create_action_info(
        &self,
        start_execute: StartExecute,
        queued_timestamp: SystemTime,
    ) -> impl Future<Output = Result<ActionInfo, Error>> + '_ {
        self.metrics.create_action_info.wrap(async move {
            let execute_request = start_execute
                .execute_request
                .err_tip(|| "Expected execute_request to exist in StartExecute")?;
            let action_digest: DigestInfo = execute_request
                .action_digest
                .clone()
                .err_tip(|| "Expected action_digest to exist on StartExecute")?
                .try_into()?;
            let load_start_timestamp = (self.callbacks.now_fn)();
            let action =
                get_and_decode_digest::<Action>(self.cas_store.as_ref(), action_digest.into())
                    .await
                    .err_tip(|| "During start_action")?;
            let action_info = ActionInfo::try_from_action_and_execute_request(
                execute_request,
                action,
                load_start_timestamp,
                queued_timestamp,
            )
            .err_tip(|| "Could not create ActionInfo in create_and_add_action()")?;
            Ok(action_info)
        })
    }

    fn cleanup_action(&self, operation_id: &OperationId) -> Result<(), Error> {
        let mut running_actions = self.running_actions.lock();
        let result = running_actions.remove(operation_id).err_tip(|| {
            format!("Expected operation id '{operation_id}' to exist in RunningActionsManagerImpl")
        });
        // No need to copy anything, we just are telling the receivers an event happened.
        self.action_done_tx.send_modify(|()| {});
        result.map(|_| ())
    }

    // Note: We do not capture metrics on this call, only `.kill_all()`.
    // Important: When the future returns the process may still be running.
    async fn kill_operation(action: Arc<RunningActionImpl>) {
        warn!(
            operation_id = ?action.operation_id,
            "Sending kill to running operation",
        );
        let kill_channel_tx = {
            let mut action_state = action.state.lock();
            action_state.kill_channel_tx.take()
        };
        if let Some(kill_channel_tx) = kill_channel_tx {
            if kill_channel_tx.send(()).is_err() {
                error!(
                    operation_id = ?action.operation_id,
                    "Error sending kill to running operation",
                );
            }
        }
    }

    fn perform_cleanup(self: &Arc<Self>, operation_id: OperationId) -> Option<CleanupGuard> {
        let mut cleaning = self.cleaning_up_operations.lock();
        cleaning
            .insert(operation_id.clone())
            .then_some(CleanupGuard {
                manager: Arc::downgrade(self),
                operation_id,
            })
    }
}

impl RunningActionsManager for RunningActionsManagerImpl {
    type RunningAction = RunningActionImpl;

    async fn create_and_add_action(
        self: &Arc<Self>,
        worker_id: String,
        start_execute: StartExecute,
    ) -> Result<Arc<RunningActionImpl>, Error> {
        self.metrics
            .create_and_add_action
            .wrap(async move {
                // Extract peer hints BEFORE consuming start_execute.
                let peer_hints = start_execute.peer_hints.clone();
                if !peer_hints.is_empty() {
                    if let Some(ref locality_map) = self.peer_locality_map {
                        let mut map = locality_map.write();
                        let mut total_registered = 0usize;
                        for hint in &peer_hints {
                            if let Some(ref digest_proto) = hint.digest {
                                if let Ok(digest) = DigestInfo::try_from(digest_proto) {
                                    for endpoint in &hint.peer_endpoints {
                                        map.register_blobs(endpoint, &[digest]);
                                        total_registered += 1;
                                    }
                                }
                            }
                        }
                        info!(
                            hints = peer_hints.len(),
                            registrations = total_registered,
                            "Registered peer hints from scheduler into worker locality map"
                        );
                    }
                }

                let queued_timestamp = start_execute
                    .queued_timestamp
                    .and_then(|time| time.try_into().ok())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                let operation_id = start_execute
                    .operation_id.as_str().into();
                let action_info = self.create_action_info(start_execute, queued_timestamp).await?;
                debug!(
                    ?action_info,
                    "Worker received action",
                );
                // Wait for any previous cleanup to complete before creating directory
                self.wait_for_cleanup_if_needed(&operation_id).await?;
                let action_directory = self.make_action_directory(&operation_id).await?;
                let execution_metadata = ExecutionMetadata {
                    worker: worker_id,
                    queued_timestamp: action_info.insert_timestamp,
                    worker_start_timestamp: action_info.load_timestamp,
                    worker_completed_timestamp: SystemTime::UNIX_EPOCH,
                    input_fetch_start_timestamp: SystemTime::UNIX_EPOCH,
                    input_fetch_completed_timestamp: SystemTime::UNIX_EPOCH,
                    execution_start_timestamp: SystemTime::UNIX_EPOCH,
                    execution_completed_timestamp: SystemTime::UNIX_EPOCH,
                    output_upload_start_timestamp: SystemTime::UNIX_EPOCH,
                    output_upload_completed_timestamp: SystemTime::UNIX_EPOCH,
                };
                let timeout = if action_info.timeout.is_zero() || self.timeout_handled_externally {
                    self.max_action_timeout
                } else {
                    action_info.timeout
                };
                if timeout > self.max_action_timeout {
                    return Err(make_err!(
                        Code::InvalidArgument,
                        "Action timeout of {} seconds is greater than the maximum allowed timeout of {} seconds",
                        timeout.as_secs_f32(),
                        self.max_action_timeout.as_secs_f32()
                    ));
                }
                let running_action = Arc::new(RunningActionImpl::new(
                    execution_metadata,
                    operation_id.clone(),
                    action_directory,
                    action_info,
                    timeout,
                    self.clone(),
                ));
                {
                    let mut running_actions = self.running_actions.lock();
                    // Check if action already exists and is still alive
                    if let Some(existing_weak) = running_actions.get(&operation_id) {
                        if let Some(_existing_action) = existing_weak.upgrade() {
                            return Err(make_err!(
                                Code::AlreadyExists,
                                "Action with operation_id {} is already running",
                                operation_id
                            ));
                        }
                    }
                    running_actions.insert(operation_id, Arc::downgrade(&running_action));
                }
                Ok(running_action)
            })
            .await
    }

    async fn cache_action_result(
        &self,
        action_info: DigestInfo,
        action_result: &mut ActionResult,
        hasher: DigestHasherFunc,
    ) -> Result<(), Error> {
        self.metrics
            .cache_action_result
            .wrap(self.upload_action_results.cache_action_result(
                action_info,
                action_result,
                hasher,
            ))
            .await
    }

    async fn kill_operation(&self, operation_id: &OperationId) -> Result<(), Error> {
        let running_action = {
            let running_actions = self.running_actions.lock();
            running_actions
                .get(operation_id)
                .and_then(Weak::upgrade)
                .ok_or_else(|| make_input_err!("Failed to get running action {operation_id}"))?
        };
        Self::kill_operation(running_action).await;
        Ok(())
    }

    // Note: When the future returns the process should be fully killed and cleaned up.
    async fn kill_all(&self) {
        self.metrics
            .kill_all
            .wrap_no_capture_result(async move {
                let kill_operations: Vec<Arc<RunningActionImpl>> = {
                    let running_actions = self.running_actions.lock();
                    running_actions
                        .iter()
                        .filter_map(|(_operation_id, action)| action.upgrade())
                        .collect()
                };
                let mut kill_futures: FuturesUnordered<_> = kill_operations
                    .into_iter()
                    .map(Self::kill_operation)
                    .collect();
                while kill_futures.next().await.is_some() {}
            })
            .await;
        // Ignore error. If error happens it means there's no sender, which is not a problem.
        // Note: Sanity check this API will always check current value then future values:
        // https://play.rust-lang.org/?version=stable&edition=2021&gist=23103652cc1276a97e5f9938da87fdb2
        drop(
            self.action_done_tx
                .subscribe()
                .wait_for(|()| self.running_actions.lock().is_empty())
                .await,
        );
    }

    fn spawn_upload_to_remote(self: &Arc<Self>, action_result: &ActionResult) {
        RunningActionsManagerImpl::spawn_upload_to_remote(self, action_result);
    }

    #[inline]
    fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
    }
}

#[derive(Debug, Default, MetricsComponent)]
pub struct Metrics {
    #[metric(help = "Stats about the create_and_add_action command.")]
    create_and_add_action: AsyncCounterWrapper,
    #[metric(help = "Stats about the cache_action_result command.")]
    cache_action_result: AsyncCounterWrapper,
    #[metric(help = "Stats about the kill_all command.")]
    kill_all: AsyncCounterWrapper,
    #[metric(help = "Stats about the create_action_info command.")]
    create_action_info: AsyncCounterWrapper,
    #[metric(help = "Stats about the make_work_directory command.")]
    make_action_directory: AsyncCounterWrapper,
    #[metric(help = "Stats about the prepare_action command.")]
    prepare_action: AsyncCounterWrapper,
    #[metric(help = "Stats about the execute command.")]
    execute: AsyncCounterWrapper,
    #[metric(help = "Stats about the upload_results command.")]
    upload_results: AsyncCounterWrapper,
    #[metric(help = "Stats about the cleanup command.")]
    cleanup: AsyncCounterWrapper,
    #[metric(help = "Stats about the get_finished_result command.")]
    get_finished_result: AsyncCounterWrapper,
    #[metric(help = "Number of times an action waited for cleanup to complete.")]
    cleanup_waits: CounterWithTime,
    #[metric(help = "Number of stale directories removed during action retries.")]
    stale_removals: CounterWithTime,
    #[metric(help = "Number of timeouts while waiting for cleanup to complete.")]
    cleanup_wait_timeouts: CounterWithTime,
    #[metric(help = "Stats about the get_proto_command_from_store command.")]
    get_proto_command_from_store: AsyncCounterWrapper,
    #[metric(help = "Stats about the download_to_directory command.")]
    download_to_directory: AsyncCounterWrapper,
    #[metric(help = "Stats about the prepare_output_files command.")]
    prepare_output_files: AsyncCounterWrapper,
    #[metric(help = "Stats about the prepare_output_paths command.")]
    prepare_output_paths: AsyncCounterWrapper,
    #[metric(help = "Stats about the child_process command.")]
    child_process: AsyncCounterWrapper,
    #[metric(help = "Stats about the child_process_success_error_code command.")]
    child_process_success_error_code: CounterWithTime,
    #[metric(help = "Stats about the child_process_failure_error_code command.")]
    child_process_failure_error_code: CounterWithTime,
    #[metric(help = "Total time spent uploading stdout.")]
    upload_stdout: AsyncCounterWrapper,
    #[metric(help = "Total time spent uploading stderr.")]
    upload_stderr: AsyncCounterWrapper,
    #[metric(help = "Total number of task timeouts.")]
    task_timeouts: CounterWithTime,
}
