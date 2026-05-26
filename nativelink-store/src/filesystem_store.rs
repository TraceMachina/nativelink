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

use core::fmt::{Debug, Formatter};
use core::pin::Pin;
use core::sync::atomic::{AtomicU64, Ordering};
use core::time::Duration;
use std::borrow::Cow;
#[cfg(unix)]
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::sync::{Arc, Weak};
use std::time::SystemTime;

#[cfg(unix)]
use async_lock::Mutex;
use async_lock::RwLock;
use async_trait::async_trait;
use bytes::{Bytes, BytesMut};
use futures::stream::{StreamExt, TryStreamExt};
use futures::{Future, TryFutureExt};
use nativelink_config::stores::FilesystemSpec;
use nativelink_error::{Code, Error, ResultExt, make_err};
use nativelink_metric::MetricsComponent;
use nativelink_util::background_spawn;
use nativelink_util::buf_channel::{
    DropCloserReadHalf, DropCloserWriteHalf, make_buf_channel_pair,
};
use nativelink_util::common::{DigestInfo, fs};
use nativelink_util::evicting_map::{EvictingMap, LenEntry};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
#[cfg(unix)]
use nativelink_util::spawn_blocking;
use nativelink_util::store_trait::{
    RemoveItemCallback, StoreDriver, StoreKey, StoreKeyBorrow, StoreOptimizations, UploadSizeInfo,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, Take};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_stream::wrappers::ReadDirStream;
use tracing::{debug, error, info, trace, warn};

use crate::callback_utils::RemoveItemCallbackHolder;
use crate::cas_utils::is_zero_digest;

// Default size to allocate memory of the buffer when reading files.
const DEFAULT_BUFF_SIZE: usize = 32 * 1024;
// Default block size of all major filesystems is 4KB
const DEFAULT_BLOCK_SIZE: u64 = 4 * 1024;

pub const STR_FOLDER: &str = "s";
pub const DIGEST_FOLDER: &str = "d";

/// Suffix for the sibling directory that holds per-digest read-only
/// **executable** (0o555) variants of CAS blobs (see
/// [`FilesystemStore::get_executable_hardlink_source`]). It is a sibling of
/// `content_path` rather than a child so the normal content/temp scan and prune
/// logic never touches it. Cleared on startup; entries are regenerable.
#[cfg(unix)]
const EXECUTABLE_DIR_SUFFIX: &str = ".exec";

#[derive(Clone, Copy, Debug)]
pub enum FileType {
    Digest,
    String,
}

#[derive(Debug, MetricsComponent)]
pub struct SharedContext {
    // Used in testing to know how many active drop() spawns are running.
    // TODO(palfrey) It is probably a good idea to use a spin lock during
    // destruction of the store to ensure that all files are actually
    // deleted (similar to how it is done in tests).
    #[metric(help = "Number of active drop spawns")]
    pub active_drop_spawns: AtomicU64,
    #[metric(help = "Path to the configured temp path")]
    temp_path: String,
    #[metric(help = "Path to the configured content path")]
    content_path: String,
}

#[derive(Eq, PartialEq, Debug)]
enum PathType {
    Content,
    Temp,
    Custom(OsString),
}

/// [`EncodedFilePath`] stores the path to the file
/// including the context, path type and key to the file.
/// The whole [`StoreKey`] is stored as opposed to solely
/// the [`DigestInfo`] so that it is more usable for things
/// such as BEP -see Issue #1108
#[derive(Debug)]
pub struct EncodedFilePath {
    shared_context: Arc<SharedContext>,
    path_type: PathType,
    key: StoreKey<'static>,
}

impl EncodedFilePath {
    #[inline]
    fn get_file_path(&self) -> Cow<'_, OsStr> {
        get_file_path_raw(&self.path_type, self.shared_context.as_ref(), &self.key)
    }
}

#[inline]
fn get_file_path_raw<'a>(
    path_type: &'a PathType,
    shared_context: &SharedContext,
    key: &StoreKey<'a>,
) -> Cow<'a, OsStr> {
    let folder = match path_type {
        PathType::Content => &shared_context.content_path,
        PathType::Temp => &shared_context.temp_path,
        PathType::Custom(path) => return Cow::Borrowed(path),
    };
    Cow::Owned(to_full_path_from_key(folder, key))
}

impl Drop for EncodedFilePath {
    fn drop(&mut self) {
        // `drop()` can be called during shutdown, so we use `path_type` flag to know if the
        // file actually needs to be deleted.
        if self.path_type == PathType::Content {
            return;
        }

        let file_path = self.get_file_path().to_os_string();
        let shared_context = self.shared_context.clone();
        // .fetch_add returns previous value, so we add one to get approximate current value
        let current_active_drop_spawns = shared_context
            .active_drop_spawns
            .fetch_add(1, Ordering::Relaxed)
            + 1;
        debug!(
            %current_active_drop_spawns,
            ?file_path,
            "Spawned a filesystem_delete_file"
        );
        background_spawn!("filesystem_delete_file", async move {
            let result = fs::remove_file(&file_path)
                .await
                .err_tip(|| format!("Failed to remove file {}", file_path.display()));
            if let Err(err) = result {
                error!(?file_path, ?err, "Failed to delete file",);
            } else {
                debug!(?file_path, "File deleted",);
            }
            // .fetch_sub returns previous value, so we subtract one to get approximate current value
            let current_active_drop_spawns = shared_context
                .active_drop_spawns
                .fetch_sub(1, Ordering::Relaxed)
                - 1;
            debug!(
                ?current_active_drop_spawns,
                ?file_path,
                "Dropped a filesystem_delete_file"
            );
        });
    }
}

/// This creates the file path from the [`StoreKey`]. If
/// it is a string, the string, prefixed with [`STR_PREFIX`]
/// for backwards compatibility, is stored.
///
/// If it is a [`DigestInfo`], it is prefixed by [`DIGEST_PREFIX`]
/// followed by the string representation of a digest - the hash in hex,
/// a hyphen then the size in bytes
///
/// Previously, only the string representation of the [`DigestInfo`] was
/// used with no prefix
#[inline]
fn to_full_path_from_key(folder: &str, key: &StoreKey<'_>) -> OsString {
    match key {
        StoreKey::Str(str) => format!("{folder}/{STR_FOLDER}/{str}"),
        StoreKey::Digest(digest_info) => format!("{folder}/{DIGEST_FOLDER}/{digest_info}"),
    }
    .into()
}

pub trait FileEntry: LenEntry + Send + Sync + Debug + 'static {
    /// Responsible for creating the underlying `FileEntry`.
    fn create(data_size: u64, block_size: u64, encoded_file_path: RwLock<EncodedFilePath>) -> Self;

    /// Creates a (usually) temp file, opens it and returns the path to the temp file.
    fn make_and_open_file(
        block_size: u64,
        encoded_file_path: EncodedFilePath,
    ) -> impl Future<Output = Result<(Self, fs::FileSlot, OsString), Error>> + Send
    where
        Self: Sized;

    /// Returns the underlying reference to the size of the data in bytes
    fn data_size_mut(&mut self) -> &mut u64;

    /// Returns the actual size of the underlying file on the disk after accounting for filesystem block size.
    fn size_on_disk(&self) -> u64;

    /// Gets the underlying `EncodedfilePath`.
    fn get_encoded_file_path(&self) -> &RwLock<EncodedFilePath>;

    /// Returns a reader that will read part of the underlying file.
    fn read_file_part(
        &self,
        offset: u64,
        length: u64,
    ) -> impl Future<Output = Result<Take<fs::FileSlot>, Error>> + Send;

    /// This function is a safe way to extract the file name of the underlying file. To protect users from
    /// accidentally creating undefined behavior we encourage users to do the logic they need to do with
    /// the filename inside this function instead of extracting the filename and doing the logic outside.
    /// This is because the filename is not guaranteed to exist after this function returns, however inside
    /// the callback the file is always guaranteed to exist and immutable.
    /// DO NOT USE THIS FUNCTION TO EXTRACT THE FILENAME AND STORE IT FOR LATER USE.
    fn get_file_path_locked<
        T,
        Fut: Future<Output = Result<T, Error>> + Send,
        F: FnOnce(OsString) -> Fut + Send,
    >(
        &self,
        handler: F,
    ) -> impl Future<Output = Result<T, Error>> + Send;
}

pub struct FileEntryImpl {
    data_size: u64,
    block_size: u64,
    // We lock around this as it gets rewritten when we move between temp and content types
    encoded_file_path: RwLock<EncodedFilePath>,
}

impl FileEntryImpl {
    pub fn get_shared_context_for_test(&mut self) -> Arc<SharedContext> {
        self.encoded_file_path.get_mut().shared_context.clone()
    }
}

impl FileEntry for FileEntryImpl {
    fn create(data_size: u64, block_size: u64, encoded_file_path: RwLock<EncodedFilePath>) -> Self {
        Self {
            data_size,
            block_size,
            encoded_file_path,
        }
    }

    /// This encapsulates the logic for the edge case of if the file fails to create
    /// the cleanup of the file is handled without creating a `FileEntry`, which would
    /// try to cleanup the file as well during `drop()`.
    async fn make_and_open_file(
        block_size: u64,
        encoded_file_path: EncodedFilePath,
    ) -> Result<(Self, fs::FileSlot, OsString), Error> {
        let temp_full_path = encoded_file_path.get_file_path().to_os_string();
        let temp_file_result = fs::create_file(temp_full_path.clone())
            .or_else(|mut err| async {
                let remove_result = fs::remove_file(&temp_full_path).await.err_tip(|| {
                    format!(
                        "Failed to remove file {} in filesystem store",
                        temp_full_path.display()
                    )
                });
                if let Err(remove_err) = remove_result {
                    err = err.merge(remove_err);
                }
                warn!(?err, ?block_size, ?temp_full_path, "Failed to create file",);
                Err(err).err_tip(|| {
                    format!(
                        "Failed to create {} in filesystem store",
                        temp_full_path.display()
                    )
                })
            })
            .await?;

        Ok((
            <Self as FileEntry>::create(
                0, /* Unknown yet, we will fill it in later */
                block_size,
                RwLock::new(encoded_file_path),
            ),
            temp_file_result,
            temp_full_path,
        ))
    }

    fn data_size_mut(&mut self) -> &mut u64 {
        &mut self.data_size
    }

    fn size_on_disk(&self) -> u64 {
        self.data_size.div_ceil(self.block_size) * self.block_size
    }

    fn get_encoded_file_path(&self) -> &RwLock<EncodedFilePath> {
        &self.encoded_file_path
    }

    fn read_file_part(
        &self,
        offset: u64,
        length: u64,
    ) -> impl Future<Output = Result<Take<fs::FileSlot>, Error>> + Send {
        self.get_file_path_locked(move |full_content_path| async move {
            let file = fs::open_file(&full_content_path, offset, length)
                .await
                .err_tip(|| {
                    format!(
                        "Failed to open file in filesystem store {}",
                        full_content_path.display()
                    )
                })?;
            Ok(file)
        })
    }

    async fn get_file_path_locked<
        T,
        Fut: Future<Output = Result<T, Error>> + Send,
        F: FnOnce(OsString) -> Fut + Send,
    >(
        &self,
        handler: F,
    ) -> Result<T, Error> {
        let encoded_file_path = self.get_encoded_file_path().read().await;
        handler(encoded_file_path.get_file_path().to_os_string()).await
    }
}

impl Debug for FileEntryImpl {
    fn fmt(&self, f: &mut Formatter<'_>) -> Result<(), core::fmt::Error> {
        f.debug_struct("FileEntryImpl")
            .field("data_size", &self.data_size)
            .field("encoded_file_path", &"<behind mutex>")
            .finish()
    }
}

fn make_temp_digest(mut digest: DigestInfo) -> DigestInfo {
    static DELETE_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);
    let mut hash = *digest.packed_hash();
    hash[24..].clone_from_slice(
        &DELETE_FILE_COUNTER
            .fetch_add(1, Ordering::Relaxed)
            .to_le_bytes(),
    );
    digest.set_packed_hash(*hash);
    digest
}

fn make_temp_key(key: &StoreKey) -> StoreKey<'static> {
    StoreKey::Digest(make_temp_digest(key.borrow().into_digest()))
}

impl LenEntry for FileEntryImpl {
    #[inline]
    fn len(&self) -> u64 {
        self.size_on_disk()
    }

    fn is_empty(&self) -> bool {
        self.data_size == 0
    }

    // unref() only triggers when an item is removed from the eviction_map. It is possible
    // that another place in code has a reference to `FileEntryImpl` and may later read the
    // file. To support this edge case, we first move the file to a temp file and point
    // target file location to the new temp file. `unref()` should only ever be called once.
    #[inline]
    async fn unref(&self) {
        let mut encoded_file_path = self.encoded_file_path.write().await;
        if encoded_file_path.path_type == PathType::Temp {
            // We are already a temp file that is now marked for deletion on drop.
            // This is very rare, but most likely the rename into the content path failed.
            warn!(
                key = ?encoded_file_path.key,
                "File is already a temp file",
            );
            return;
        }
        let from_path = encoded_file_path.get_file_path();
        let new_key = make_temp_key(&encoded_file_path.key);

        let to_path = to_full_path_from_key(&encoded_file_path.shared_context.temp_path, &new_key);

        if let Err(err) = fs::rename(&from_path, &to_path).await {
            // ENOENT here means the file we expected at `from_path`
            // was already gone — typically because another thread's
            // eviction beat us to the unref, or because the entry
            // ended up in our map without its file ever landing on
            // disk (the "phantom-map" case the runtime recovers from
            // via `FastSlowStore::get_part`'s slow-store fallback).
            // It is benign at this site — there is no file to move
            // — and historically dominates the log volume of this
            // store under heavy write+evict concurrency, fast enough
            // to drown the runtime under sustained pressure. Demote
            // to `debug` and drop the per-emission path fields so it
            // stops costing serialization in the hot path.
            //
            // Other rename failures (EACCES, EXDEV, EBUSY, …) are
            // genuinely unexpected and stay at `warn` with full
            // context.
            if err.code == Code::NotFound {
                debug!(
                    key = ?encoded_file_path.key,
                    "Failed to rename file (already gone, treating as benign)",
                );
            } else {
                warn!(
                    key = ?encoded_file_path.key,
                    ?from_path,
                    ?to_path,
                    ?err,
                    "Failed to rename file",
                );
            }
        } else {
            debug!(
                key = ?encoded_file_path.key,
                ?from_path,
                ?to_path,
                "Renamed file (unref)",
            );
            encoded_file_path.path_type = PathType::Temp;
            encoded_file_path.key = new_key;
        }
    }
}

#[inline]
fn digest_from_filename(file_name: &str) -> Result<DigestInfo, Error> {
    let (hash, size) = file_name.split_once('-').err_tip(|| "")?;
    let size = size.parse::<i64>()?;
    DigestInfo::try_new(hash, size)
}

pub fn key_from_file(file_name: &str, file_type: FileType) -> Result<StoreKey<'_>, Error> {
    match file_type {
        FileType::String => Ok(StoreKey::new_str(file_name)),
        FileType::Digest => digest_from_filename(file_name).map(StoreKey::Digest),
    }
}

/// The number of files to read the metadata for at the same time when running
/// `add_files_to_cache`.
const SIMULTANEOUS_METADATA_READS: usize = 200;

type FsEvictingMap<'a, Fe> =
    EvictingMap<StoreKeyBorrow, StoreKey<'a>, Arc<Fe>, SystemTime, RemoveItemCallbackHolder>;

async fn add_files_to_cache<Fe: FileEntry>(
    evicting_map: &FsEvictingMap<'_, Fe>,
    anchor_time: &SystemTime,
    shared_context: &Arc<SharedContext>,
    block_size: u64,
    rename_fn: fn(&OsStr, &OsStr) -> Result<(), std::io::Error>,
) -> Result<(), Error> {
    #[expect(clippy::too_many_arguments)]
    async fn process_entry<Fe: FileEntry>(
        evicting_map: &FsEvictingMap<'_, Fe>,
        file_name: &str,
        file_type: FileType,
        atime: SystemTime,
        data_size: u64,
        block_size: u64,
        anchor_time: &SystemTime,
        shared_context: &Arc<SharedContext>,
    ) -> Result<(), Error> {
        let key = key_from_file(file_name, file_type)?;

        let file_entry = Fe::create(
            data_size,
            block_size,
            RwLock::new(EncodedFilePath {
                shared_context: shared_context.clone(),
                path_type: PathType::Content,
                key: key.borrow().into_owned(),
            }),
        );
        let time_since_anchor = if let Ok(d) = anchor_time.duration_since(atime) {
            d
        } else {
            warn!(
                %file_name,
                atime = %humantime::format_rfc3339(atime),
                anchor_time = %humantime::format_rfc3339(*anchor_time),
                "File access time newer than FilesystemStore start time",
            );
            Duration::ZERO
        };
        evicting_map
            .insert_with_time(
                key.into_owned().into(),
                Arc::new(file_entry),
                i32::try_from(time_since_anchor.as_secs()).unwrap_or(i32::MAX),
            )
            .await;
        Ok(())
    }

    async fn read_files(
        folder: Option<&str>,
        shared_context: &SharedContext,
    ) -> Result<Vec<(String, SystemTime, u64, bool)>, Error> {
        // Note: In Dec 2024 this is for backwards compatibility with the old
        // way files were stored on disk. Previously all files were in a single
        // folder regardless of the StoreKey type. This allows old versions of
        // nativelink file layout to be upgraded at startup time.
        // This logic can be removed once more time has passed.
        let read_dir = folder.map_or_else(
            || format!("{}/", shared_context.content_path),
            |folder| format!("{}/{folder}/", shared_context.content_path),
        );

        let (_permit, dir_handle) = fs::read_dir(read_dir)
            .await
            .err_tip(|| "Failed opening content directory for iterating in filesystem store")?
            .into_inner();

        let read_dir_stream = ReadDirStream::new(dir_handle);
        read_dir_stream
            .map(|dir_entry| async move {
                let dir_entry = dir_entry.unwrap();
                let file_name = dir_entry.file_name().into_string().unwrap();
                let metadata = dir_entry
                    .metadata()
                    .await
                    .err_tip(|| "Failed to get metadata in filesystem store")?;
                // We need to filter out folders - we do not want to try to cache the s and d folders.
                let is_file =
                    metadata.is_file() || !(file_name == STR_FOLDER || file_name == DIGEST_FOLDER);
                // Using access time is not perfect, but better than random. We do not update the
                // atime when a file is actually "touched", we rely on whatever the filesystem does
                // when we read the file (usually update on read).
                let atime = metadata
                    .accessed()
                    .or_else(|_| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                Result::<(String, SystemTime, u64, bool), Error>::Ok((
                    file_name,
                    atime,
                    metadata.len(),
                    is_file,
                ))
            })
            .buffer_unordered(SIMULTANEOUS_METADATA_READS)
            .try_collect()
            .await
    }

    /// Note: In Dec 2024 this is for backwards compatibility with the old
    /// way files were stored on disk. Previously all files were in a single
    /// folder regardless of the [`StoreKey`] type. This moves files from the old cache
    /// location to the new cache location, under [`DIGEST_FOLDER`].
    async fn move_old_cache(
        shared_context: &Arc<SharedContext>,
        rename_fn: fn(&OsStr, &OsStr) -> Result<(), std::io::Error>,
    ) -> Result<(), Error> {
        let file_infos = read_files(None, shared_context).await?;

        let from_path = &shared_context.content_path;

        let to_path = format!("{}/{DIGEST_FOLDER}", shared_context.content_path);

        for (file_name, _, _, _) in file_infos.into_iter().filter(|x| x.3) {
            let from_file: OsString = format!("{from_path}/{file_name}").into();
            let to_file: OsString = format!("{to_path}/{file_name}").into();

            if let Err(err) = rename_fn(&from_file, &to_file) {
                warn!(?from_file, ?to_file, ?err, "Failed to rename file",);
            } else {
                debug!(?from_file, ?to_file, "Renamed file (old cache)",);
            }
        }
        Ok(())
    }

    async fn add_files_to_cache<Fe: FileEntry>(
        evicting_map: &FsEvictingMap<'_, Fe>,
        anchor_time: &SystemTime,
        shared_context: &Arc<SharedContext>,
        block_size: u64,
        folder: &str,
    ) -> Result<(), Error> {
        let file_infos = read_files(Some(folder), shared_context).await?;
        let file_type = match folder {
            STR_FOLDER => FileType::String,
            DIGEST_FOLDER => FileType::Digest,
            _ => panic!("Invalid folder type"),
        };

        let path_root = format!("{}/{folder}", shared_context.content_path);

        for (file_name, atime, data_size, _) in file_infos.into_iter().filter(|x| x.3) {
            let result = process_entry(
                evicting_map,
                &file_name,
                file_type,
                atime,
                data_size,
                block_size,
                anchor_time,
                shared_context,
            )
            .await;
            if let Err(err) = result {
                warn!(?file_name, ?err, "Failed to add file to eviction cache",);
                // Ignore result.
                drop(fs::remove_file(format!("{path_root}/{file_name}")).await);
            }
        }
        Ok(())
    }

    move_old_cache(shared_context, rename_fn).await?;

    add_files_to_cache(
        evicting_map,
        anchor_time,
        shared_context,
        block_size,
        DIGEST_FOLDER,
    )
    .await?;

    add_files_to_cache(
        evicting_map,
        anchor_time,
        shared_context,
        block_size,
        STR_FOLDER,
    )
    .await?;
    Ok(())
}

async fn prune_temp_path(temp_path: &str) -> Result<(), Error> {
    async fn prune_temp_inner(temp_path: &str, subpath: &str) -> Result<(), Error> {
        let (_permit, dir_handle) = fs::read_dir(format!("{temp_path}/{subpath}"))
            .await
            .err_tip(
                || "Failed opening temp directory to prune partial downloads in filesystem store",
            )?
            .into_inner();

        let mut read_dir_stream = ReadDirStream::new(dir_handle);
        while let Some(dir_entry) = read_dir_stream.next().await {
            let path = dir_entry?.path();
            if let Err(err) = fs::remove_file(&path).await {
                warn!(?path, ?err, "Failed to delete file",);
            }
        }
        Ok(())
    }

    prune_temp_inner(temp_path, STR_FOLDER).await?;
    prune_temp_inner(temp_path, DIGEST_FOLDER).await?;
    Ok(())
}

#[derive(Debug, MetricsComponent)]
pub struct FilesystemStore<Fe: FileEntry = FileEntryImpl> {
    #[metric]
    shared_context: Arc<SharedContext>,
    #[metric(group = "evicting_map")]
    evicting_map: Arc<FsEvictingMap<'static, Fe>>,
    #[metric(help = "Block size of the configured filesystem")]
    block_size: u64,
    #[metric(help = "Size of the configured read buffer size")]
    read_buffer_size: usize,
    weak_self: Weak<Self>,
    rename_fn: fn(&OsStr, &OsStr) -> Result<(), std::io::Error>,
    /// Limits concurrent write operations to prevent disk I/O saturation.
    write_semaphore: Option<Semaphore>,
    /// Per-digest single-flight locks guarding creation of the executable
    /// variant in `{content_path}.exec`, so each variant's writable fd is
    /// opened exactly once. The outer lock is sync and only ever held to
    /// get/insert/remove the per-digest async lock — never across I/O.
    #[cfg(unix)]
    executable_locks: std::sync::Mutex<HashMap<DigestInfo, Arc<Mutex<()>>>>,
}

impl<Fe: FileEntry> FilesystemStore<Fe> {
    pub async fn new(spec: &FilesystemSpec) -> Result<Arc<Self>, Error> {
        Self::new_with_timeout_and_rename_fn(spec, |from, to| std::fs::rename(from, to)).await
    }

    pub async fn new_with_timeout_and_rename_fn(
        spec: &FilesystemSpec,
        rename_fn: fn(&OsStr, &OsStr) -> Result<(), std::io::Error>,
    ) -> Result<Arc<Self>, Error> {
        async fn create_subdirs(path: &str) -> Result<(), Error> {
            fs::create_dir_all(format!("{path}/{STR_FOLDER}"))
                .await
                .err_tip(|| format!("Failed to create directory {path}/{STR_FOLDER}"))?;
            fs::create_dir_all(format!("{path}/{DIGEST_FOLDER}"))
                .await
                .err_tip(|| format!("Failed to create directory {path}/{DIGEST_FOLDER}"))
        }

        let now = SystemTime::now();

        let empty_policy = nativelink_config::stores::EvictionPolicy::default();
        let eviction_policy = spec.eviction_policy.as_ref().unwrap_or(&empty_policy);
        let evicting_map = Arc::new(EvictingMap::new(eviction_policy, now));

        // Create temp and content directories and the s and d subdirectories.

        create_subdirs(&spec.temp_path).await?;
        create_subdirs(&spec.content_path).await?;

        // Executable-variant directory: a sibling of `content_path` holding
        // per-digest 0o555 copies used as hardlink sources for executable
        // inputs (see `get_executable_hardlink_source`). Cleared on startup —
        // the variants are regenerable and we never want a stale one to leak
        // across runs. Unix-only: the executable bit (and the ETXTBSY race it
        // guards against) does not apply on Windows.
        #[cfg(unix)]
        {
            let executable_dir = format!("{}{EXECUTABLE_DIR_SUFFIX}", spec.content_path);
            drop(fs::remove_dir_all(&executable_dir).await);
            fs::create_dir_all(format!("{executable_dir}/{DIGEST_FOLDER}"))
                .await
                .err_tip(|| format!("Failed to create executable dir {executable_dir}"))?;
        }

        let shared_context = Arc::new(SharedContext {
            active_drop_spawns: AtomicU64::new(0),
            temp_path: spec.temp_path.clone(),
            content_path: spec.content_path.clone(),
        });

        let block_size = if spec.block_size == 0 {
            DEFAULT_BLOCK_SIZE
        } else {
            spec.block_size
        };
        add_files_to_cache(
            evicting_map.as_ref(),
            &now,
            &shared_context,
            block_size,
            rename_fn,
        )
        .await?;
        prune_temp_path(&shared_context.temp_path).await?;

        let read_buffer_size = if spec.read_buffer_size == 0 {
            DEFAULT_BUFF_SIZE
        } else {
            spec.read_buffer_size as usize
        };
        let write_semaphore = if spec.max_concurrent_writes > 0 {
            Some(Semaphore::new(spec.max_concurrent_writes))
        } else {
            None
        };
        Ok(Arc::new_cyclic(|weak_self| Self {
            shared_context,
            evicting_map,
            block_size,
            read_buffer_size,
            weak_self: weak_self.clone(),
            rename_fn,
            write_semaphore,
            #[cfg(unix)]
            executable_locks: std::sync::Mutex::new(HashMap::new()),
        }))
    }

    pub fn get_arc(&self) -> Option<Arc<Self>> {
        self.weak_self.upgrade()
    }

    /// Path of the read-only executable (0o555) variant for `digest`.
    #[cfg(unix)]
    fn executable_variant_path(&self, digest: &DigestInfo) -> OsString {
        format!(
            "{}{EXECUTABLE_DIR_SUFFIX}/{DIGEST_FOLDER}/{digest}",
            self.shared_context.content_path
        )
        .into()
    }

    /// Returns the path to a private, read-only **executable** (0o555) copy of
    /// the blob for `digest`, creating it at most once. Callers **hardlink**
    /// the returned path into action input trees instead of copying the
    /// executable per action.
    ///
    /// Why this exists: a CAS blob is stored read-only **0o444** and shared
    /// across actions by hardlink, so it cannot carry the executable bit and
    /// must never be `chmod`'d (that mutates the shared inode — the #2347
    /// corruption class). Materializing an executable input therefore needs a
    /// separate 0o555 inode. Doing that copy *per action* opens a writable fd
    /// in the worker's hot path; under fork-heavy concurrency a child can
    /// inherit that fd and a concurrent `execve` of the executable then fails
    /// with `ETXTBSY` ("Text file busy", os error 26). Creating the 0o555 inode
    /// **once** — writer fd fsync'd and closed, then atomically renamed into
    /// place before the inode is ever hardlinked or executed — and hardlinking
    /// it thereafter keeps the per-action path hardlink-only.
    #[cfg(unix)]
    pub async fn get_executable_hardlink_source(
        &self,
        digest: &DigestInfo,
    ) -> Result<OsString, Error> {
        let variant_path = self.executable_variant_path(digest);

        // Fast path: the variant already exists, so the caller can hardlink it
        // with no writable fd anywhere in sight.
        if fs::metadata(&variant_path).await.is_ok() {
            return Ok(variant_path);
        }

        // Single-flight: exactly one task ever opens a writable fd for this
        // variant. Without this, a cold-cache burst of concurrent actions
        // would each open a writer for the same executable — the very window
        // ETXTBSY exploits.
        let lock = {
            let mut locks = self
                .executable_locks
                .lock()
                .expect("executable_locks poisoned");
            locks
                .entry(*digest)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _guard = lock.lock().await;

        // Re-check: another task may have constructed it while we waited.
        if fs::metadata(&variant_path).await.is_ok() {
            self.forget_executable_lock(digest);
            return Ok(variant_path);
        }

        let result = self.create_executable_variant(digest, &variant_path).await;
        // Drop the per-digest lock entry regardless of outcome so the map
        // cannot grow unbounded; a concurrent waiter already cloned the Arc.
        self.forget_executable_lock(digest);
        result.map(|()| variant_path)
    }

    /// Non-unix has no executable bit and no `ETXTBSY`, so just hardlink the
    /// CAS blob directly.
    #[cfg(not(unix))]
    pub async fn get_executable_hardlink_source(
        &self,
        digest: &DigestInfo,
    ) -> Result<OsString, Error> {
        let file_entry = self.get_file_entry_for_digest(digest).await?;
        file_entry
            .get_file_path_locked(|p| async move { Ok(p) })
            .await
    }

    #[cfg(unix)]
    fn forget_executable_lock(&self, digest: &DigestInfo) {
        self.executable_locks
            .lock()
            .expect("executable_locks poisoned")
            .remove(digest);
    }

    /// Materializes the 0o555 executable variant for `digest`. Must be called
    /// under the per-digest single-flight guard.
    #[cfg(unix)]
    async fn create_executable_variant(
        &self,
        digest: &DigestInfo,
        variant_path: &OsStr,
    ) -> Result<(), Error> {
        // Resolve the on-disk CAS blob (0o444) to copy from. Must be present in
        // this tier; callers populate the fast store first.
        let file_entry = self
            .get_file_entry_for_digest(digest)
            .await
            .err_tip(|| "Resolving CAS blob for executable variant")?;
        let src_path = file_entry
            .get_file_path_locked(|p| async move { Ok(p) })
            .await?;

        let variant_owned = variant_path.to_os_string();
        let mut temp_owned = variant_path.to_os_string();
        temp_owned.push(".tmp");
        let rename_fn = self.rename_fn;

        // All of this is blocking std::fs; run it off the async runtime. The
        // writable fd opened by `copy` is fully closed before the `rename`
        // publishes the inode, so no reachable hardlink of the variant ever has
        // an open writer.
        spawn_blocking!(
            "filesystem_store_executable_variant",
            move || -> Result<(), Error> {
                use std::os::unix::fs::PermissionsExt;
                std::fs::copy(&src_path, &temp_owned).map_err(|e| {
                    make_err!(Code::Internal, "executable-variant copy failed: {e:?}")
                })?;
                std::fs::set_permissions(&temp_owned, std::fs::Permissions::from_mode(0o555))
                    .map_err(|e| {
                        make_err!(
                            Code::Internal,
                            "executable-variant chmod 0o555 failed: {e:?}"
                        )
                    })?;
                // Reopen read-only purely to fsync the bytes durable before publish.
                let f = std::fs::File::open(&temp_owned)
                    .map_err(|e| make_err!(Code::Internal, "executable-variant reopen: {e:?}"))?;
                f.sync_all()
                    .map_err(|e| make_err!(Code::Internal, "executable-variant fsync: {e:?}"))?;
                drop(f);
                rename_fn(temp_owned.as_os_str(), variant_owned.as_os_str()).map_err(|e| {
                    make_err!(Code::Internal, "executable-variant rename failed: {e:?}")
                })?;
                Ok(())
            }
        )
        .await
        .err_tip(|| "executable-variant spawn_blocking join failed")?
    }

    pub async fn get_file_entry_for_digest(&self, digest: &DigestInfo) -> Result<Arc<Fe>, Error> {
        // Zero-digest blobs have no backing file on disk (FilesystemStore
        // never persists zero-byte content). The previous implementation
        // returned a synthetic FileEntry whose content_path did not exist,
        // which downstream callers would then try to hard_link from,
        // silently producing missing or empty output files in worker
        // execution directories. Return NotFound so callers are forced to
        // take the explicit zero-digest path (e.g. fs::create_file).
        if is_zero_digest(digest) {
            return Err(make_err!(
                Code::NotFound,
                "{digest} is a zero-digest; FilesystemStore does not persist zero-byte files. \
                 Callers must materialise empty files directly rather than going through get_file_entry_for_digest."
            ));
        }
        self.evicting_map
            .get(&digest.into())
            .await
            .ok_or_else(|| make_err!(Code::NotFound, "{digest} not found in filesystem store. This may indicate the file was evicted due to cache pressure. Consider increasing 'max_bytes' in your filesystem store's eviction_policy configuration."))
    }

    async fn update_file(
        self: Pin<&Self>,
        mut entry: Fe,
        mut temp_file: fs::FileSlot,
        final_key: StoreKey<'static>,
        mut reader: DropCloserReadHalf,
    ) -> Result<(), Error> {
        let mut data_size = 0;
        loop {
            let mut data = reader
                .recv()
                .await
                .err_tip(|| "Failed to receive data in filesystem store")?;
            let data_len = data.len();
            if data_len == 0 {
                break; // EOF.
            }
            temp_file
                .write_all_buf(&mut data)
                .await
                .err_tip(|| "Failed to write data into filesystem store")?;
            data_size += data_len as u64;
        }

        let permit = if let Some(sem) = &self.write_semaphore {
            Some(sem.acquire().await.map_err(|err| {
                Error::from_std_err(Code::Internal, &err).append("Write semaphore closed")
            })?)
        } else {
            None
        };

        temp_file
            .as_ref()
            .sync_all()
            .await
            .err_tip(|| "Failed to sync_data in filesystem store")?;

        drop(permit);

        temp_file.advise_dontneed();
        trace!(?temp_file, "Dropping file to update_file");
        drop(temp_file);

        *entry.data_size_mut() = data_size;
        self.emplace_file(final_key, Arc::new(entry)).await
    }

    async fn emplace_file(&self, key: StoreKey<'static>, entry: Arc<Fe>) -> Result<(), Error> {
        // This sequence of events is quite tricky to understand due to the amount of triggers that
        // happen, async'ness of it and the locking. So here is a breakdown of what happens:
        // 1. Here will hold a write lock on any file operations of this FileEntry.
        // 2. Then insert the entry into the evicting map. This may trigger an eviction of other
        //    entries.
        // 3. Eviction triggers `unref()`, which grabs a write lock on the evicted FileEntry
        //    during the rename.
        // 4. It should be impossible for items to be added while eviction is happening, so there
        //    should not be a deadlock possibility. However, it is possible for the new FileEntry
        //    to be evicted before the file is moved into place. Eviction of the newly inserted
        //    item is not possible within the `insert()` call because the write lock inside the
        //    eviction map. If an eviction of new item happens after `insert()` but before
        //    `rename()` then we get to finish our operation because the `unref()` of the new item
        //    will be blocked on us because we currently have the lock.
        // 5. Move the file into place. Since we hold a write lock still anyone that gets our new
        //    FileEntry (which has not yet been placed on disk) will not be able to read the file's
        //    contents until we release the lock.
        let evicting_map = self.evicting_map.clone();
        let rename_fn = self.rename_fn;

        // We need to guarantee that this will get to the end even if the parent future is dropped.
        // See: https://github.com/TraceMachina/nativelink/issues/495
        background_spawn!("filesystem_store_emplace_file", async move {
            evicting_map
                .insert(key.borrow().into_owned().into(), entry.clone())
                .await;

            // The insert might have resulted in an eviction/unref so we need to check
            // it still exists in there. But first, get the lock...
            let mut encoded_file_path = entry.get_encoded_file_path().write().await;
            // Then check it's still in there...
            if evicting_map.get(&key).await.is_none() {
                info!(%key, "Got eviction while emplacing, dropping");
                return Ok(());
            }

            let final_path = get_file_path_raw(
                &PathType::Content,
                encoded_file_path.shared_context.as_ref(),
                &key,
            );

            let from_path = encoded_file_path.get_file_path();

            // Lock the blob down as read-only *before* it lands at its final
            // content path, so every hardlink of it (the worker directory cache
            // and `download_to_directory`) inherits an immutable, read-only
            // inode. This is what preserves the input-tree hermeticity contract
            // (actions cannot mutate their inputs) without a per-materialization
            // chmod walk, and it means callers must never `chmod` a hardlinked
            // blob — anything needing a different mode (e.g. an executable's +x
            // bit) must take a private copy. Content-addressed blobs are
            // write-once and replaced by `rename` rather than in-place writes,
            // and unlink only needs the *parent directory* to be writable, so a
            // read-only file mode is safe for both overwrite-by-rename and
            // eviction. Best-effort: a failure here (e.g. the temp file was
            // already evicted out from under us) must not abort the emplace.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if let Err(err) =
                    fs::set_permissions(&from_path, std::fs::Permissions::from_mode(0o444)).await
                {
                    warn!(?err, ?from_path, "Failed to set CAS blob read-only");
                }
            }
            // Internally tokio spawns fs commands onto a blocking thread anyways.
            // Since we are already on a blocking thread, we just need the `fs` wrapper to manage
            // an open-file permit (ensure we don't open too many files at once).
            let result = (rename_fn)(&from_path, &final_path).err_tip(|| {
                format!(
                    "Failed to rename temp file to final path {}",
                    final_path.display()
                )
            });

            // In the event our move from temp file to final file fails we need to ensure we remove
            // the entry from our map.
            // Remember: At this point it is possible for another thread to have a reference to
            // `entry`, so we can't delete the file, only drop() should ever delete files.
            if let Err(err) = result {
                error!(?err, ?from_path, ?final_path, "Failed to rename file",);
                // Warning: To prevent deadlock we need to release our lock or during `remove_if()`
                // it will call `unref()`, which triggers a write-lock on `encoded_file_path`.
                drop(encoded_file_path);
                // It is possible that the item in our map is no longer the item we inserted,
                // So, we need to conditionally remove it only if the pointers are the same.

                evicting_map
                    .remove_if(&key, |map_entry| Arc::<Fe>::ptr_eq(map_entry, &entry))
                    .await;
                return Err(err);
            }
            encoded_file_path.path_type = PathType::Content;
            encoded_file_path.key = key;
            Ok(())
        })
        .await
        .err_tip(|| "Failed to create spawn in filesystem store update_file")?
    }
}

#[async_trait]
impl<Fe: FileEntry> StoreDriver for FilesystemStore<Fe> {
    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> {
        let own_keys = keys
            .iter()
            .map(|sk| sk.borrow().into_owned())
            .collect::<Vec<_>>();
        self.evicting_map
            .sizes_for_keys(own_keys.iter(), results, false /* peek */)
            .await;
        // We need to do a special pass to ensure our zero files exist.
        // If our results failed and the result was a zero file, we need to
        // create the file by spec.
        for (key, result) in keys.iter().zip(results.iter_mut()) {
            if result.is_some() || !is_zero_digest(key.borrow()) {
                continue;
            }
            let (mut tx, rx) = make_buf_channel_pair();
            let send_eof_result = tx.send_eof();
            self.update(key.borrow(), rx, UploadSizeInfo::ExactSize(0))
                .await
                .err_tip(|| format!("Failed to create zero file for key {}", key.as_str()))
                .merge(
                    send_eof_result
                        .err_tip(|| "Failed to send zero file EOF in filesystem store has"),
                )?;

            *result = Some(0);
        }
        Ok(())
    }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        mut reader: DropCloserReadHalf,
        _upload_size: UploadSizeInfo,
    ) -> Result<(), Error> {
        if is_zero_digest(key.borrow()) {
            // don't need to add, because zero length files are just assumed to exist
            return Ok(());
        }

        let temp_key = make_temp_key(&key);

        // There's a possibility of deadlock here where we take all of the
        // file semaphores with make_and_open_file and the semaphores for
        // whatever is populating reader is exhasted on the threads that
        // have the FileSlots and not on those which can't.  To work around
        // this we don't take the FileSlot until there's something on the
        // reader available to know that the populator is active.
        reader.peek().await?;

        let (entry, temp_file, temp_full_path) = Fe::make_and_open_file(
            self.block_size,
            EncodedFilePath {
                shared_context: self.shared_context.clone(),
                path_type: PathType::Temp,
                key: temp_key,
            },
        )
        .await?;

        self.update_file(entry, temp_file, key.into_owned(), reader)
            .await
            .err_tip(|| {
                format!(
                    "While processing with temp file {}",
                    temp_full_path.display()
                )
            })
    }

    fn optimized_for(&self, optimization: StoreOptimizations) -> bool {
        matches!(
            optimization,
            StoreOptimizations::FileUpdates | StoreOptimizations::SubscribesToUpdateOneshot
        )
    }

    async fn update_oneshot(self: Pin<&Self>, key: StoreKey<'_>, data: Bytes) -> Result<(), Error> {
        if is_zero_digest(key.borrow()) {
            return Ok(());
        }

        let temp_key = make_temp_key(&key);
        let (mut entry, mut temp_file, temp_full_path) = Fe::make_and_open_file(
            self.block_size,
            EncodedFilePath {
                shared_context: self.shared_context.clone(),
                path_type: PathType::Temp,
                key: temp_key,
            },
        )
        .await
        .err_tip(|| "Failed to create temp file in filesystem store update_oneshot")?;

        // Write directly without channel overhead
        if !data.is_empty() {
            temp_file
                .write_all(&data)
                .await
                .err_tip(|| format!("Failed to write data to {}", temp_full_path.display()))?;
        }

        let _permit = if let Some(sem) = &self.write_semaphore {
            Some(sem.acquire().await.map_err(|err| {
                Error::from_std_err(Code::Internal, &err).append("Write semaphore closed")
            })?)
        } else {
            None
        };

        temp_file
            .as_ref()
            .sync_all()
            .await
            .err_tip(|| "Failed to sync_data in filesystem store update_oneshot")?;

        drop(_permit);

        temp_file.advise_dontneed();
        drop(temp_file);

        *entry.data_size_mut() = data.len() as u64;
        self.emplace_file(key.into_owned(), Arc::new(entry)).await
    }

    async fn update_with_whole_file(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        path: OsString,
        file: fs::FileSlot,
        upload_size: UploadSizeInfo,
    ) -> Result<Option<fs::FileSlot>, Error> {
        let file_size = match upload_size {
            UploadSizeInfo::ExactSize(size) => size,
            UploadSizeInfo::MaxSize(_) => file
                .as_ref()
                .metadata()
                .await
                .err_tip(|| format!("While reading metadata for {}", path.display()))?
                .len(),
        };
        if file_size == 0 {
            // don't need to add, because zero length files are just assumed to exist
            return Ok(None);
        }
        let entry = Fe::create(
            file_size,
            self.block_size,
            RwLock::new(EncodedFilePath {
                shared_context: self.shared_context.clone(),
                path_type: PathType::Custom(path),
                key: key.borrow().into_owned(),
            }),
        );
        // We are done with the file, if we hold a reference to the file here, it could
        // result in a deadlock if `emplace_file()` also needs file descriptors.
        trace!(?file, "Dropping file to to update_with_whole_file");
        file.advise_dontneed();
        drop(file);
        self.emplace_file(key.into_owned(), Arc::new(entry))
            .await
            .err_tip(|| "Could not move file into store in upload_file_to_store, maybe dest is on different volume?")?;
        return Ok(None);
    }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> {
        if is_zero_digest(key.borrow()) {
            self.has(key.borrow())
                .await
                .err_tip(|| "Failed to check if zero digest exists in filesystem store")?;
            writer
                .send_eof()
                .err_tip(|| "Failed to send zero EOF in filesystem store get_part")?;
            return Ok(());
        }
        let owned_key = key.into_owned();
        let entry = self.evicting_map.get(&owned_key).await.ok_or_else(|| {
            make_err!(
                Code::NotFound,
                "{} not found in filesystem store here",
                owned_key.as_str()
            )
        })?;
        let read_limit = length.unwrap_or(u64::MAX);
        let mut temp_file = entry.read_file_part(offset, read_limit).or_else(|err| async move {
            // If the file is not found, we need to remove it from the eviction map.
            if err.code == Code::NotFound {
                error!(
                    ?err,
                    key = ?owned_key,
                    "Entry was in our map, but not found on disk. Removing from map as a precaution, but process probably need restarted."
                );
                self.evicting_map.remove(&owned_key).await;
            }
            Err(err)
        }).await?;

        loop {
            let mut buf = BytesMut::with_capacity(self.read_buffer_size);
            temp_file
                .read_buf(&mut buf)
                .await
                .err_tip(|| "Failed to read data in filesystem store")?;
            if buf.is_empty() {
                break; // EOF.
            }
            writer
                .send(buf.freeze())
                .await
                .err_tip(|| "Failed to send chunk in filesystem store get_part")?;
        }
        temp_file.get_ref().advise_dontneed();
        writer
            .send_eof()
            .err_tip(|| "Filed to send EOF in filesystem store get_part")?;

        Ok(())
    }

    fn inner_store(&self, _digest: Option<StoreKey>) -> &dyn StoreDriver {
        self
    }

    fn as_any<'a>(&'a self) -> &'a (dyn core::any::Any + Sync + Send + 'static) {
        self
    }

    fn as_any_arc(self: Arc<Self>) -> Arc<dyn core::any::Any + Sync + Send + 'static> {
        self
    }

    fn register_health(self: Arc<Self>, registry: &mut HealthRegistryBuilder) {
        registry.register_indicator(self);
    }

    fn register_remove_callback(
        self: Arc<Self>,
        callback: Arc<dyn RemoveItemCallback>,
    ) -> Result<(), Error> {
        self.evicting_map
            .add_remove_callback(RemoveItemCallbackHolder::new(callback));
        Ok(())
    }
}

#[async_trait]
impl<Fe: FileEntry> HealthStatusIndicator for FilesystemStore<Fe> {
    fn get_name(&self) -> &'static str {
        "FilesystemStore"
    }

    /// Lightweight probe: `stat()` the `content_path` directory. No
    /// write-semaphore / eviction-map contention with production
    /// traffic, and bounded so a hung NFS / EBS mount can't wedge the
    /// indicator.
    async fn check_health(&self, _namespace: Cow<'static, str>) -> HealthStatus {
        const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

        let content_path = &self.shared_context.content_path;
        let stat = tokio::fs::metadata(&content_path);
        match timeout(HEALTH_PROBE_TIMEOUT, stat).await {
            Ok(Ok(meta)) if meta.is_dir() => {
                HealthStatus::new_ok(self, "FilesystemStore::check_health: ok".into())
            }
            Ok(Ok(_)) => HealthStatus::new_failed(
                self,
                format!(
                    "FilesystemStore::check_health: content_path {content_path} is not a directory"
                )
                .into(),
            ),
            Ok(Err(e)) => {
                warn!(
                    ?e,
                    %content_path,
                    "FilesystemStore::check_health: stat errored",
                );
                HealthStatus::new_failed(
                    self,
                    format!("FilesystemStore::check_health: stat errored: {e}").into(),
                )
            }
            Err(_) => {
                warn!(
                    %content_path,
                    timeout_secs = HEALTH_PROBE_TIMEOUT.as_secs(),
                    "FilesystemStore::check_health: stat timed out",
                );
                HealthStatus::Timeout {
                    struct_name: self.struct_name(),
                }
            }
        }
    }
}
