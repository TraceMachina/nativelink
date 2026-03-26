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
use std::borrow::Cow;
use std::ffi::{OsStr, OsString};
use std::sync::{Arc, Weak};
use std::time::SystemTime;

use async_lock::RwLock;
use async_trait::async_trait;
use bytes::Bytes;
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
use nativelink_util::evicting_map::{LenEntry, ShardedEvictingMap};
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
use nativelink_util::store_trait::{
    ItemCallback, StoreDriver, StoreKey, StoreKeyBorrow, StoreOptimizations, UploadSizeInfo,
};
use tokio::sync::Semaphore;
use tokio_stream::wrappers::ReadDirStream;
use tracing::{debug, error, info, trace, warn};

use crate::callback_utils::ItemCallbackHolder;
use crate::cas_utils::is_zero_digest;

// Default size to allocate memory of the buffer when reading files.
// 256 KiB reduces syscalls by 4x compared to 64 KiB. At 10Gbps, 64 KiB reads
// cause ~19,500 syscalls/sec/stream; 256 KiB brings this down to ~4,900.
// Modern NVMe SSDs perform significantly better with larger read sizes.
/// Default read buffer size. Matches the default ByteStream
/// `max_bytes_per_stream` (3 MiB) so that each disk read produces
/// exactly one chunk, avoiding BytesMut concatenation copies in
/// `buf_channel::consume()`.
const DEFAULT_BUFF_SIZE: usize = 3 * 1024 * 1024;
// Default block size of all major filesystems is 4KB
const DEFAULT_BLOCK_SIZE: u64 = 4 * 1024;

pub const STR_FOLDER: &str = "s";
pub const DIGEST_FOLDER: &str = "d";

/// Returns the expected on-disk path for a digest file under the given
/// content path. This is useful for tests and external tooling that need
/// to construct or verify file paths.
///
/// The path layout is: `{content_path}/d/{hash[0..2]}/{hash}-{size}`
pub fn digest_content_path(content_path: &str, digest: &DigestInfo) -> OsString {
    let key: StoreKey<'_> = (*digest).into();
    to_full_path_from_key(content_path, &key)
}

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
                if err.code == Code::NotFound {
                    // File already deleted (e.g. race between eviction paths).
                    debug!(?file_path, "File already deleted, ignoring");
                } else {
                    error!(?file_path, ?err, "Failed to delete file");
                }
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

/// Returns the 2-character hex shard prefix for a digest, derived from
/// the first byte of the packed hash. This gives 256 subdirectories
/// (00-ff), reducing per-directory file count from hundreds of thousands
/// to ~1,500 on typical deployments.
#[inline]
fn digest_shard_prefix(digest_info: &DigestInfo) -> [u8; 2] {
    const HEX_LUT: &[u8; 16] = b"0123456789abcdef";
    let first_byte = digest_info.packed_hash()[0];
    [
        HEX_LUT[(first_byte >> 4) as usize],
        HEX_LUT[(first_byte & 0x0f) as usize],
    ]
}

/// This creates the file path from the [`StoreKey`]. If
/// it is a string, the string, prefixed with [`STR_PREFIX`]
/// for backwards compatibility, is stored.
///
/// If it is a [`DigestInfo`], it is prefixed by [`DIGEST_PREFIX`]
/// followed by a 2-char hex shard directory (first byte of hash),
/// then the string representation of a digest - the hash in hex,
/// a hyphen then the size in bytes.
///
/// Layout: `{folder}/d/{hash[0..2]}/{hash}-{size}`
#[inline]
fn to_full_path_from_key(folder: &str, key: &StoreKey<'_>) -> OsString {
    match key {
        StoreKey::Str(str) => format!("{folder}/{STR_FOLDER}/{str}"),
        StoreKey::Digest(digest_info) => {
            let shard = digest_shard_prefix(digest_info);
            // SAFETY: shard is always valid ASCII hex chars.
            let shard_str = unsafe { core::str::from_utf8_unchecked(&shard) };
            format!("{folder}/{DIGEST_FOLDER}/{shard_str}/{digest_info}")
        }
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
    ) -> impl Future<Output = Result<fs::FileSlot, Error>> + Send;

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
    ) -> impl Future<Output = Result<fs::FileSlot, Error>> + Send {
        self.get_file_path_locked(move |full_content_path| async move {
            let file = fs::open_file(&full_content_path, offset)
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
            // Already a temp file marked for deletion on drop. This happens
            // when the entry is evicted from the map before emplace_file
            // renames it into the content path — expected under cache pressure.
            debug!(
                key = ?encoded_file_path.key,
                "File is already a temp file",
            );
            return;
        }
        let from_path = encoded_file_path.get_file_path();
        let new_key = make_temp_key(&encoded_file_path.key);

        let to_path = to_full_path_from_key(&encoded_file_path.shared_context.temp_path, &new_key);

        if let Err(err) = fs::rename(&from_path, &to_path).await {
            warn!(
                key = ?encoded_file_path.key,
                ?from_path,
                ?to_path,
                ?err,
                "Failed to rename file",
            );
        } else {
            debug!(
                key = ?encoded_file_path.key,
                ?from_path,
                ?to_path,
                "Evicted blob from filesystem cache (unref)",
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
    ShardedEvictingMap<StoreKeyBorrow, StoreKey<'a>, Arc<Fe>, SystemTime, ItemCallbackHolder>;

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
        // Use a negative seconds_since_anchor for files that existed before
        // the anchor time (startup). This correctly represents them as "older
        // than anything inserted during runtime" in the EvictingMap timeline.
        // Files with atime closer to startup get values closer to 0 (newer),
        // while files not accessed for days get large negative values (older).
        let seconds_since_anchor = if let Ok(before) = anchor_time.duration_since(atime) {
            let secs = before.as_secs();
            if secs > i32::MAX as u64 {
                i32::MIN
            } else {
                -(secs as i32)
            }
        } else {
            // atime is after anchor_time (file touched between capturing
            // `now` and reading metadata) — treat as most-recently-used.
            0
        };
        evicting_map
            .insert_with_time(
                key.into_owned().into(),
                Arc::new(file_entry),
                seconds_since_anchor,
            )
            .await;
        Ok(())
    }

    /// Reads directory entries from a single directory, returning
    /// (file_name, atime, size, is_file) tuples.
    async fn read_dir_entries(
        dir_path: &str,
    ) -> Result<Vec<(String, SystemTime, u64, bool)>, Error> {
        let (_permit, dir_handle) = fs::read_dir(dir_path)
            .await
            .err_tip(|| {
                format!("Failed opening directory {dir_path} for iterating in filesystem store")
            })?
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
                let is_file = metadata.is_file();
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

        read_dir_entries(&read_dir).await
    }

    /// Reads files from the digest folder, scanning both shard
    /// subdirectories (d/XX/) and legacy flat files (d/HASH-SIZE).
    async fn read_digest_files_sharded(
        shared_context: &SharedContext,
    ) -> Result<Vec<(String, SystemTime, u64, bool)>, Error> {
        let digest_dir = format!("{}/{DIGEST_FOLDER}", shared_context.content_path);
        let top_entries = read_dir_entries(&digest_dir).await?;

        let mut all_files = Vec::new();

        for (name, atime, size, is_file) in top_entries {
            if is_file {
                // Legacy flat file directly in d/ — include it.
                all_files.push((name, atime, size, true));
            } else if name.len() == 2 {
                // Shard subdirectory (00-ff) — scan its contents.
                let shard_path = format!("{digest_dir}/{name}");
                match read_dir_entries(&shard_path).await {
                    Ok(shard_entries) => {
                        for entry in shard_entries {
                            if entry.3 {
                                all_files.push(entry);
                            }
                        }
                    }
                    Err(err) => {
                        warn!(?err, shard = %name, "failed to read shard directory during startup scan");
                    }
                }
            }
            // Skip other directories (s/, d/ — shouldn't be here but just in case).
        }

        Ok(all_files)
    }

    /// Note: In Dec 2024 this is for backwards compatibility with the old
    /// way files were stored on disk. Previously all files were in a single
    /// folder regardless of the [`StoreKey`] type. This moves files from the old cache
    /// location to the new cache location, under [`DIGEST_FOLDER`] with shard prefix.
    async fn move_old_cache(
        shared_context: &Arc<SharedContext>,
        rename_fn: fn(&OsStr, &OsStr) -> Result<(), std::io::Error>,
    ) -> Result<(), Error> {
        let file_infos = read_files(None, shared_context).await?;

        let from_path = shared_context.content_path.to_string();

        let digest_path = format!("{}/{DIGEST_FOLDER}", shared_context.content_path);

        for (file_name, _, _, _) in file_infos.into_iter().filter(|x| x.3) {
            let from_file: OsString = format!("{from_path}/{file_name}").into();
            // Place into the shard subdirectory based on first 2 hex chars.
            let to_file: OsString = if file_name.len() >= 2 {
                let shard = &file_name[..2];
                format!("{digest_path}/{shard}/{file_name}").into()
            } else {
                format!("{digest_path}/{file_name}").into()
            };

            if let Err(err) = rename_fn(&from_file, &to_file) {
                warn!(?from_file, ?to_file, ?err, "Failed to rename file",);
            } else {
                debug!(?from_file, ?to_file, "Renamed file (old cache)",);
            }
        }
        Ok(())
    }

    /// Migrates legacy flat files from `d/HASH-SIZE` to the sharded
    /// layout `d/XX/HASH-SIZE`. Files already in shard subdirectories
    /// are left alone.
    async fn migrate_flat_to_sharded(
        shared_context: &Arc<SharedContext>,
        rename_fn: fn(&OsStr, &OsStr) -> Result<(), std::io::Error>,
    ) -> Result<(), Error> {
        let digest_dir = format!("{}/{DIGEST_FOLDER}", shared_context.content_path);
        let top_entries = read_dir_entries(&digest_dir).await?;
        let mut migrated = 0u64;

        for (file_name, _, _, is_file) in &top_entries {
            if !is_file || file_name.len() < 2 {
                continue;
            }
            let shard = &file_name[..2];
            let from_file: OsString = format!("{digest_dir}/{file_name}").into();
            let to_file: OsString = format!("{digest_dir}/{shard}/{file_name}").into();

            if let Err(err) = rename_fn(&from_file, &to_file) {
                warn!(?from_file, ?to_file, ?err, "failed to migrate flat file to shard");
            } else {
                migrated += 1;
            }
        }
        if migrated > 0 {
            info!(migrated, "migrated legacy flat CAS files to sharded layout");
        }
        Ok(())
    }

    async fn add_files_for_folder<Fe: FileEntry>(
        evicting_map: &FsEvictingMap<'_, Fe>,
        anchor_time: &SystemTime,
        shared_context: &Arc<SharedContext>,
        block_size: u64,
        folder: &str,
    ) -> Result<(), Error> {
        let file_type = match folder {
            STR_FOLDER => FileType::String,
            DIGEST_FOLDER => FileType::Digest,
            _ => panic!("Invalid folder type"),
        };

        let mut file_infos = if folder == DIGEST_FOLDER {
            read_digest_files_sharded(shared_context).await?
        } else {
            read_files(Some(folder), shared_context).await?
        };

        // Sort by atime oldest-first so that the LRU cache ordering matches
        // actual file access recency. Without this, items are inserted in
        // directory-iteration order (random), causing recently-used files to
        // be evicted while cold files survive.
        file_infos.sort_by(|a, b| a.1.cmp(&b.1));

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
                // Derive full path: for digests, use shard subdir; for strings, flat.
                let full_path = if folder == DIGEST_FOLDER && file_name.len() >= 2 {
                    let shard = &file_name[..2];
                    format!("{path_root}/{shard}/{file_name}")
                } else {
                    format!("{path_root}/{file_name}")
                };
                // Ignore result.
                drop(fs::remove_file(full_path).await);
            }
        }
        Ok(())
    }

    move_old_cache(shared_context, rename_fn).await?;
    migrate_flat_to_sharded(shared_context, rename_fn).await?;

    add_files_for_folder(
        evicting_map,
        anchor_time,
        shared_context,
        block_size,
        DIGEST_FOLDER,
    )
    .await?;

    add_files_for_folder(
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
    async fn prune_files_in_dir(dir_path: &str) -> Result<(), Error> {
        let (_permit, dir_handle) = fs::read_dir(dir_path)
            .await
            .err_tip(
                || "Failed opening temp directory to prune partial downloads in filesystem store",
            )?
            .into_inner();

        let mut read_dir_stream = ReadDirStream::new(dir_handle);
        while let Some(dir_entry) = read_dir_stream.next().await {
            let dir_entry = dir_entry?;
            let path = dir_entry.path();
            let metadata = dir_entry.metadata().await.ok();
            if metadata.as_ref().map_or(true, |m| m.is_file()) {
                if let Err(err) = fs::remove_file(&path).await {
                    warn!(?path, ?err, "Failed to delete temp file",);
                }
            }
        }
        Ok(())
    }

    prune_files_in_dir(&format!("{temp_path}/{STR_FOLDER}")).await?;
    // Prune both flat files in d/ and files in d/XX/ shard subdirectories.
    let digest_dir = format!("{temp_path}/{DIGEST_FOLDER}");
    prune_files_in_dir(&digest_dir).await?;
    for byte in 0u8..=255 {
        let shard_dir = format!("{digest_dir}/{byte:02x}");
        // Shard dirs may not exist yet (first startup before create_subdirs).
        if let Ok(()) = prune_files_in_dir(&shard_dir).await {
            // ok
        }
    }
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
    /// Skip writes when a blob with the same key already exists (CAS dedup).
    content_is_immutable: bool,
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
                .err_tip(|| format!("Failed to create directory {path}/{DIGEST_FOLDER}"))?;
            // Create all 256 shard subdirectories (00-ff) under the digest
            // folder. This avoids create_dir_all on every write and reduces
            // per-directory file count from hundreds of thousands to ~1,500.
            for byte in 0u8..=255 {
                let shard = format!("{byte:02x}");
                fs::create_dir_all(format!("{path}/{DIGEST_FOLDER}/{shard}"))
                    .await
                    .err_tip(|| {
                        format!("Failed to create shard directory {path}/{DIGEST_FOLDER}/{shard}")
                    })?;
            }
            Ok(())
        }

        let now = SystemTime::now();

        let empty_policy = nativelink_config::stores::EvictionPolicy::default();
        let eviction_policy = spec.eviction_policy.as_ref().unwrap_or(&empty_policy);
        let evicting_map = Arc::new(ShardedEvictingMap::new(eviction_policy, now));

        // Create temp and content directories and the s and d subdirectories.

        create_subdirs(&spec.temp_path).await?;
        create_subdirs(&spec.content_path).await?;

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
        evicting_map.start_background_eviction();
        Ok(Arc::new_cyclic(|weak_self| Self {
            shared_context,
            evicting_map,
            block_size,
            read_buffer_size,
            weak_self: weak_self.clone(),
            rename_fn,
            write_semaphore,
            content_is_immutable: spec.content_is_immutable,
        }))
    }

    pub fn get_arc(&self) -> Option<Arc<Self>> {
        self.weak_self.upgrade()
    }

    /// Pin a digest to prevent eviction during background upload.
    pub fn pin_digest(&self, digest: &DigestInfo) {
        let key: StoreKey<'static> = (*digest).into();
        self.evicting_map.pin_key(StoreKeyBorrow::from(key));
    }

    /// Unpin a digest, allowing eviction again.
    pub fn unpin_digest(&self, digest: &DigestInfo) {
        let key: StoreKey<'static> = (*digest).into();
        self.evicting_map.unpin_key(&key);
    }

    /// Returns all digest entries in the cache with their absolute last-access
    /// timestamps (seconds since UNIX epoch). String-keyed entries are skipped.
    /// This is a peek-only operation and does NOT promote entries in the LRU.
    pub fn get_all_digests_with_timestamps(&self) -> Vec<(DigestInfo, i64)> {
        self.evicting_map
            .get_all_entries_with_timestamps()
            .into_iter()
            .filter_map(|(key_borrow, abs_timestamp)| {
                match StoreKey::from(key_borrow) {
                    StoreKey::Digest(digest) => Some((digest, abs_timestamp)),
                    _ => None,
                }
            })
            .collect()
    }

    /// Remove a digest's entry from the evicting map so the next
    /// `populate_fast_store` is forced to re-download from the slow store.
    pub async fn remove_entry_for_digest(&self, digest: &DigestInfo) {
        self.evicting_map.remove(&digest.into()).await;
    }

    pub async fn get_file_entry_for_digest(&self, digest: &DigestInfo) -> Result<Arc<Fe>, Error> {
        if is_zero_digest(digest) {
            return Ok(Arc::new(Fe::create(
                0,
                0,
                RwLock::new(EncodedFilePath {
                    shared_context: self.shared_context.clone(),
                    path_type: PathType::Content,
                    key: digest.into(),
                }),
            )));
        }
        self.evicting_map
            .get(&digest.into())
            .await
            .ok_or_else(|| make_err!(Code::NotFound, "{digest} not found in filesystem store. This may indicate the file was evicted due to cache pressure. Consider increasing 'max_bytes' in your filesystem store's eviction_policy configuration."))
    }

    /// Batch-retrieves file entries for multiple digests in a single lock
    /// acquisition on the EvictingMap, reducing contention compared to
    /// calling `get_file_entry_for_digest()` individually for each digest.
    pub async fn get_file_entries_batch(
        &self,
        digests: &[DigestInfo],
    ) -> Vec<Option<Arc<Fe>>> {
        // Separate zero digests (which don't go through evicting_map).
        let store_keys: Vec<StoreKey<'static>> = digests
            .iter()
            .filter(|d| !is_zero_digest(**d))
            .map(|d| (*d).into())
            .collect();

        let batch_results = self.evicting_map.get_many(store_keys.iter()).await;

        // Reassemble results, inserting zero-digest entries where needed.
        // Zero-digest files have no backing file on disk, so we return None
        // to let the caller fall back to creating an empty file directly.
        let mut batch_iter = batch_results.into_iter();
        digests
            .iter()
            .map(|digest| {
                if is_zero_digest(*digest) {
                    None
                } else {
                    batch_iter.next().flatten()
                }
            })
            .collect()
    }

    async fn update_file(
        self: Pin<&Self>,
        mut entry: Fe,
        temp_file: fs::FileSlot,
        final_key: StoreKey<'static>,
        mut reader: DropCloserReadHalf,
    ) -> Result<(), Error> {
        let write_start = std::time::Instant::now();
        let (data_size, temp_file) = fs::write_file_from_channel(temp_file, &mut reader)
            .await
            .err_tip(|| "Failed to write data into filesystem store")?;
        let write_ms = write_start.elapsed().as_millis();

        let permit = if let Some(sem) = &self.write_semaphore {
            Some(
                sem.acquire()
                    .await
                    .map_err(|_| make_err!(Code::Internal, "Write semaphore closed"))?,
            )
        } else {
            None
        };

        drop(permit);

        trace!(?temp_file, "Dropping file to update_file");
        drop(temp_file);

        *entry.data_size_mut() = data_size;
        let emplace_start = std::time::Instant::now();
        let result = self.emplace_file(final_key.borrow().into_owned(), Arc::new(entry)).await;
        let emplace_ms = emplace_start.elapsed().as_millis();

        let total_ms = write_ms + emplace_ms;
        if total_ms > 100 {
            warn!(
                key = %final_key.as_str(),
                total_ms,
                write_ms,
                emplace_ms,
                data_size,
                "update_file slow phases (>100ms)"
            );
        }
        result
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
        let content_is_immutable = self.content_is_immutable;

        // We need to guarantee that this will get to the end even if the parent future is dropped.
        // See: https://github.com/TraceMachina/nativelink/issues/495
        background_spawn!("filesystem_store_emplace_file", async move {
            let emplace_timer = std::time::Instant::now();

            // CAS optimization: if the key already exists and the store is
            // content-addressable (immutable), just promote it in the LRU
            // instead of replacing it. Same digest = same content, so
            // replacing triggers an unnecessary unref (filesystem rename).
            // Skip for mutable stores (AC) where the same key can map to
            // different values.
            if content_is_immutable {
                let owned_key = key.borrow().into_owned();
                if evicting_map.size_for_key(&owned_key).await.is_some() {
                    // Key exists, content identical — skip insert+unref cycle.
                    return Ok(());
                }
            }

            evicting_map
                .insert(key.borrow().into_owned().into(), entry.clone())
                .await;
            let map_insert_ms = emplace_timer.elapsed().as_millis();

            // The insert might have resulted in an eviction/unref so we need to check
            // it still exists in there. But first, get the lock...
            let mut encoded_file_path = entry.get_encoded_file_path().write().await;
            let lock_acquire_ms = emplace_timer.elapsed().as_millis() - map_insert_ms;

            // Check that OUR specific entry is still in the map. A concurrent
            // write for the same key may have replaced our entry (calling
            // unref which deletes our temp file). Checking just the key
            // would pass if the replacement entry exists, but our temp file
            // would already be deleted → ENOENT on rename.
            let still_ours = match evicting_map.get(&key).await {
                Some(map_entry) => Arc::ptr_eq(&map_entry, &entry),
                None => false,
            };
            if !still_ours {
                info!(%key, "Got eviction or replacement while emplacing, dropping");
                return Ok(());
            }

            let final_path = get_file_path_raw(
                &PathType::Content,
                encoded_file_path.shared_context.as_ref(),
                &key,
            );

            let from_path: OsString = encoded_file_path.get_file_path().into_owned();
            let final_path_owned: OsString = final_path.into_owned();
            // Run rename + set_permissions on a blocking thread to avoid
            // stalling the async runtime with syscalls.
            let from_clone = from_path.clone();
            let to_clone = final_path_owned.clone();
            let rename_start = std::time::Instant::now();
            let result = tokio::task::spawn_blocking(move || -> Result<(u128, u128), Error> {
                let rename_syscall_start = std::time::Instant::now();
                (rename_fn)(&from_clone, &to_clone)?;
                let rename_syscall_ms = rename_syscall_start.elapsed().as_millis();

                // Pre-set CAS file permissions to read+execute (0o555) so that
                // hardlinked copies already have correct permissions without
                // needing a per-file chmod during input materialization.
                let chmod_ms;
                #[cfg(target_family = "unix")]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let chmod_start = std::time::Instant::now();
                    let perms = std::fs::Permissions::from_mode(0o555);
                    if let Err(err) = std::fs::set_permissions(&to_clone, perms) {
                        tracing::warn!(?err, path = ?to_clone, "Failed to set CAS file permissions to 0o555");
                    }
                    chmod_ms = chmod_start.elapsed().as_millis();
                }
                #[cfg(not(target_family = "unix"))]
                {
                    chmod_ms = 0;
                }
                Ok((rename_syscall_ms, chmod_ms))
            })
            .await
            .map_err(|e| make_err!(Code::Internal, "Rename task join error: {e:?}"))
            .and_then(|r| r.err_tip(|| "Failed to rename temp file to final path"));
            let rename_total_ms = rename_start.elapsed().as_millis();

            match &result {
                Ok((rename_syscall_ms, chmod_ms)) => {
                    let emplace_total_ms = emplace_timer.elapsed().as_millis();
                    if emplace_total_ms > 100 {
                        warn!(
                            %key,
                            emplace_total_ms,
                            map_insert_ms,
                            lock_acquire_ms,
                            rename_total_ms,
                            rename_syscall_ms,
                            chmod_ms,
                            "emplace_file slow (>100ms)"
                        );
                    }
                    encoded_file_path.path_type = PathType::Content;
                    encoded_file_path.key = key;
                    Ok(())
                }
                Err(err) => {
                    // In the event our move from temp file to final file fails we need to ensure
                    // we remove the entry from our map.
                    // Remember: At this point it is possible for another thread to have a reference
                    // to `entry`, so we can't delete the file, only drop() should ever delete files.
                    error!(?err, ?from_path, ?final_path_owned, "Failed to rename file",);
                    // Warning: To prevent deadlock we need to release our lock or during
                    // `remove_if()` it will call `unref()`, which triggers a write-lock on
                    // `encoded_file_path`.
                    drop(encoded_file_path);
                    // It is possible that the item in our map is no longer the item we inserted,
                    // So, we need to conditionally remove it only if the pointers are the same.

                    evicting_map
                        .remove_if(&key, |map_entry| Arc::<Fe>::ptr_eq(map_entry, &entry))
                        .await;
                    Err(make_err!(
                        Code::Internal,
                        "Failed to rename temp file to final path: {err:?}"
                    ))
                }
            }
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

        // CAS dedup: skip write if blob already exists (same digest = same content).
        // sizes_for_keys with peek=false promotes the key in the LRU, updating
        // its access time so it won't be evicted prematurely.
        if self.content_is_immutable {
            let owned_key = key.borrow().into_owned();
            let mut exists = [None];
            self.evicting_map
                .sizes_for_keys(core::iter::once(&owned_key), &mut exists, false)
                .await;
            if exists[0].is_some() {
                reader
                    .drain()
                    .await
                    .err_tip(|| "Failed to drain reader for existing blob")?;
                return Ok(());
            }
        }

        let temp_key = make_temp_key(&key);
        let update_total_start = std::time::Instant::now();

        // There's a possibility of deadlock here where we take all of the
        // file semaphores with make_and_open_file and the semaphores for
        // whatever is populating reader is exhasted on the threads that
        // have the FileSlots and not on those which can't.  To work around
        // this we don't take the FileSlot until there's something on the
        // reader available to know that the populator is active.
        reader.peek().await?;

        let temp_create_start = std::time::Instant::now();
        let (entry, temp_file, temp_full_path) = Fe::make_and_open_file(
            self.block_size,
            EncodedFilePath {
                shared_context: self.shared_context.clone(),
                path_type: PathType::Temp,
                key: temp_key,
            },
        )
        .await?;
        let temp_create_ms = temp_create_start.elapsed().as_millis();

        let result = self.update_file(entry, temp_file, key.borrow().into_owned(), reader)
            .await
            .err_tip(|| {
                format!(
                    "While processing with temp file {}",
                    temp_full_path.display()
                )
            });

        let total_ms = update_total_start.elapsed().as_millis();
        if total_ms > 100 {
            warn!(
                key = %key.as_str(),
                total_ms,
                temp_create_ms,
                write_and_emplace_ms = total_ms.saturating_sub(temp_create_ms),
                "update slow write (>100ms)"
            );
        }
        result
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

        // CAS dedup: skip write if blob already exists (same digest = same content).
        if self.content_is_immutable {
            let owned_key = key.borrow().into_owned();
            let mut exists = [None];
            self.evicting_map
                .sizes_for_keys(core::iter::once(&owned_key), &mut exists, false)
                .await;
            if exists[0].is_some() {
                return Ok(());
            }
        }

        let oneshot_total_start = std::time::Instant::now();
        let temp_key = make_temp_key(&key);
        let temp_create_start = std::time::Instant::now();
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
        let temp_create_ms = temp_create_start.elapsed().as_millis();

        // Write directly without channel overhead
        let data_len = data.len() as u64;
        let write_ms;
        if !data.is_empty() {
            let write_start = std::time::Instant::now();
            let temp_full_path_clone = temp_full_path.clone();
            temp_file = nativelink_util::spawn_blocking!("fs_write_oneshot", move || {
                use std::io::Write;
                temp_file.advise_sequential();
                temp_file
                    .as_std_mut()
                    .write_all(&data)
                    .map_err(|e| Into::<Error>::into(e))
                    .err_tip(|| {
                        format!("Failed to write data to {}", temp_full_path_clone.display())
                    })?;
                Ok::<_, Error>(temp_file)
            })
            .await
            .map_err(|e| make_err!(Code::Internal, "write oneshot join failed: {e:?}"))??;
            write_ms = write_start.elapsed().as_millis();
        } else {
            write_ms = 0;
        }

        let _permit = if let Some(sem) = &self.write_semaphore {
            Some(
                sem.acquire()
                    .await
                    .map_err(|_| make_err!(Code::Internal, "Write semaphore closed"))?,
            )
        } else {
            None
        };

        drop(_permit);

        drop(temp_file);

        *entry.data_size_mut() = data_len;
        let emplace_start = std::time::Instant::now();
        let result = self.emplace_file(key.borrow().into_owned(), Arc::new(entry)).await;
        let emplace_ms = emplace_start.elapsed().as_millis();

        let total_ms = oneshot_total_start.elapsed().as_millis();
        if total_ms > 100 {
            warn!(
                key = %key.as_str(),
                total_ms,
                temp_create_ms,
                write_ms,
                emplace_ms,
                data_len,
                "update_oneshot slow write (>100ms)"
            );
        }
        result
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
                .as_std()
                .metadata()
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
        let temp_file = entry.read_file_part(offset).or_else(|err| async move {
            // If the file is not found, we need to remove it from the eviction map.
            if err.code == Code::NotFound {
                warn!(
                    ?err,
                    key = ?owned_key,
                    "Stale filesystem cache entry: file not found on disk. \
                     Removed from map; upper store layer will re-fetch from remote."
                );
                self.evicting_map.remove(&owned_key).await;
            }
            Err(err)
        }).await?;

        // Hint to the kernel that we'll read sequentially — enables more
        // aggressive readahead (typically 2-4x the default 128 KiB).
        temp_file.advise_sequential();

        // NOTE: We intentionally do NOT call advise_dontneed() after reading.
        // The same blobs are frequently read by multiple workers within
        // seconds of each other — keeping them in page cache avoids
        // redundant disk I/O (measured: 76% of read I/O is re-reads).
        fs::read_file_to_channel(temp_file, writer, read_limit, self.read_buffer_size, offset)
            .await
            .err_tip(|| "Failed to read data in filesystem store")?;
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

    fn register_item_callback(
        self: Arc<Self>,
        callback: Arc<dyn ItemCallback>,
    ) -> Result<(), Error> {
        self.evicting_map
            .add_item_callback(ItemCallbackHolder::new(callback));
        Ok(())
    }

    fn pin_digests(&self, digests: &[DigestInfo]) {
        let keys: Vec<StoreKeyBorrow> = digests
            .iter()
            .map(|d| StoreKeyBorrow::from(StoreKey::from(*d)))
            .collect();
        self.evicting_map.pin_keys(&keys);
    }
}

#[async_trait]
impl<Fe: FileEntry> HealthStatusIndicator for FilesystemStore<Fe> {
    fn get_name(&self) -> &'static str {
        "FilesystemStore"
    }

    async fn check_health(&self, namespace: Cow<'static, str>) -> HealthStatus {
        StoreDriver::check_health(Pin::new(self), namespace).await
    }
}
