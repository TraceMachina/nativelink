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
use nativelink_util::evicting_map::{EvictingMap, EvictionSnapshot, LenEntry};
use nativelink_util::fs::FileSlot;
use nativelink_util::health_utils::{HealthRegistryBuilder, HealthStatus, HealthStatusIndicator};
#[cfg(unix)]
use nativelink_util::spawn_blocking;
#[cfg(unix)]
use nativelink_util::store_trait::RemoveItemCallback;
use nativelink_util::store_trait::{
    RemoveCallback, StoreDriver, StoreKey, StoreKeyBorrow, StoreOptimizations, UploadSizeInfo,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt, Take};
use tokio::sync::Semaphore;
use tokio::time::timeout;
use tokio_stream::wrappers::ReadDirStream;
use tracing::{debug, error, info, trace, warn};

use crate::callback_utils::RemoveCallbackHolder;
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
/// logic never touches it. Cleared on writable startup; entries are
/// regenerable. If the wipe is blocked by a read-only filesystem, executable
/// variants are disabled so surviving files are never trusted.
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
            match fs::remove_file(&file_path).await {
                Ok(()) => debug!(?file_path, "File deleted"),
                // The file already being gone is the desired end state of a
                // delete, not a failure — e.g. an entry marked Temp after an
                // already-gone unref points at a path that was never created.
                Err(err) if err.code == Code::NotFound => {
                    debug!(?file_path, "File already gone, nothing to delete");
                }
                Err(err) => error!(?file_path, ?err, "Failed to delete file"),
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
    ) -> impl Future<Output = Result<(Self, FileSlot, OsString), Error>> + Send
    where
        Self: Sized;

    /// Returns the underlying size of the data in bytes
    fn data_size(&self) -> u64;

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
    ) -> impl Future<Output = Result<Take<FileSlot>, Error>> + Send;

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
    ) -> Result<(Self, FileSlot, OsString), Error> {
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

    fn data_size(&self) -> u64 {
        self.data_size
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
    ) -> impl Future<Output = Result<Take<FileSlot>, Error>> + Send {
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

pub fn make_temp_key(key: &StoreKey) -> StoreKey<'static> {
    StoreKey::Digest(make_temp_digest(key.borrow().into_digest()))
}

/// Group-commit flush coalescer (macOS only).
///
/// On macOS, `File::sync_all` issues `fcntl(F_FULLFSYNC)` — a full
/// device-cache flush — once per call. The flush is serialized at the
/// device, costs multiple milliseconds, and dominates uploads of many
/// small blobs (a 4,400-tiny-file action input tree spends ~12s of its
/// ~14.5s materialization in these flushes). Linux does not have this
/// problem because concurrent `fsync` calls coalesce inside the
/// filesystem journal's group commit.
///
/// This applies the same group-commit idea in userspace, exploiting an
/// asymmetry in `F_FULLFSYNC`: writing a file's pages to the storage
/// device is per-file work (plain `fsync(2)`, cheap), while the expensive
/// device-cache drain is device-wide by nature. Each writer first pushes
/// its own data to the device with `fsync(2)`, then joins the current
/// commit round; a single long-lived flusher task per DEVICE issues one
/// `F_FULLFSYNC` (on a dedicated sentinel file) per round, covering every
/// previously-`fsync`ed blob at once. Rounds self-batch exactly like a
/// journal: while one flush is running, later writers accumulate into
/// the next round.
///
/// The coalescer (and its flusher task) is per store instance. Sharing
/// one coalescer across all stores on a device would amortize further —
/// the flush is device-wide — but a process-global flusher task cannot
/// safely outlive the tokio runtime that spawned it (multiple runtimes
/// coexist in one process, e.g. one per test); doing this correctly
/// needs a runtime-agnostic flusher (a dedicated OS thread) and is left
/// as a follow-up.
///
/// Durability is identical to per-file `F_FULLFSYNC`: a writer only
/// proceeds (and only renames its blob into the content directory) after
/// a device-cache flush that started after its own `fsync(2)` completed.
#[cfg(target_os = "macos")]
#[derive(Debug)]
struct FlushCoalescer {
    /// The round currently accepting waiters. Swapped out by the flusher
    /// right before it flushes, so writers whose `fsync(2)` finished
    /// after the flush began land in the next round.
    current_round: parking_lot::Mutex<Arc<FlushRound>>,
    /// Wakes the flusher task. Writers notify after subscribing to their
    /// round, so a wakeup can never be observed before its waiter.
    wake: Arc<tokio::sync::Notify>,
    /// Dedicated file the `F_FULLFSYNC` is issued on. A SIBLING of the
    /// store's content path (like the `.exec` variant directory), so
    /// neither the startup scan nor `move_old_cache`'s legacy root sweep
    /// ever sees it.
    sentinel: Arc<std::fs::File>,
    /// Number of device-wide barriers issued. Kept separately from the
    /// per-file `fsync(2)` count so tests can pin the coalescing invariant.
    full_flush_count: Arc<AtomicU64>,
}

#[cfg(target_os = "macos")]
#[derive(Debug)]
struct FlushRound {
    /// Broadcasts the flush outcome to every writer in the round.
    result_tx: tokio::sync::watch::Sender<Option<Result<(), Error>>>,
}

#[cfg(target_os = "macos")]
impl FlushRound {
    fn new() -> Arc<Self> {
        let (result_tx, _) = tokio::sync::watch::channel(None);
        Arc::new(Self { result_tx })
    }
}

/// Guarantees a [`FlushRound`]'s waiters always receive a result: if the
/// flusher task is cancelled at an await point (runtime shutdown), waiters
/// must get an error rather than pend forever — their own `Arc<FlushRound>`
/// keeps the channel alive, so a dropped-sender wakeup can never happen.
#[cfg(target_os = "macos")]
struct SendOnDrop(Option<Arc<FlushRound>>);

#[cfg(target_os = "macos")]
impl SendOnDrop {
    fn finish(mut self, result: Result<(), Error>) {
        if let Some(round) = self.0.take() {
            // Ignore send errors: every waiter may have been cancelled.
            drop(round.result_tx.send(Some(result)));
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for SendOnDrop {
    fn drop(&mut self) {
        if let Some(round) = self.0.take() {
            drop(round.result_tx.send(Some(Err(make_err!(
                Code::Internal,
                "Flush coalescer round task cancelled before completing"
            )))));
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for FlushCoalescer {
    fn drop(&mut self) {
        // Wake the flusher so it observes the dead Weak and exits instead
        // of parking forever.
        self.wake.notify_one();
    }
}

#[cfg(target_os = "macos")]
impl FlushCoalescer {
    /// Creates the coalescer for a store and spawns its flusher task on
    /// the current runtime (the same runtime the store's uploads run on).
    async fn for_content_path(content_path: &str) -> Result<Arc<Self>, Error> {
        // Sibling of `content_path` (never inside it): the startup scan
        // and `move_old_cache` sweep everything under the content root.
        let sentinel_path = format!("{content_path}.flush_sentinel");
        let sentinel = spawn_blocking!("filesystem_store_flush_sentinel_open", move || {
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&sentinel_path)
                .map_err(|e| {
                    make_err!(
                        Code::Internal,
                        "Failed to create flush sentinel {sentinel_path}: {e:?}"
                    )
                })
        })
        .await
        .err_tip(|| "Failed to join flush sentinel open task")??;

        let coalescer = Arc::new(Self {
            current_round: parking_lot::Mutex::new(FlushRound::new()),
            wake: Arc::new(tokio::sync::Notify::new()),
            sentinel: Arc::new(sentinel),
            full_flush_count: Arc::new(AtomicU64::new(0)),
        });

        let weak = Arc::downgrade(&coalescer);
        let wake = coalescer.wake.clone();
        background_spawn!("filesystem_store_flush_coalescer", async move {
            loop {
                wake.notified().await;
                let Some(coalescer) = weak.upgrade() else {
                    return;
                };
                coalescer.flush_one_round().await;
                // Drop the strong ref before parking so the store can be
                // torn down while the flusher is idle.
            }
        });
        Ok(coalescer)
    }

    /// Waits until a device-cache flush that started after this call has
    /// completed. Callers must have already `fsync(2)`ed their own file.
    async fn commit(&self) -> Result<(), Error> {
        let round = self.current_round.lock().clone();
        let mut result_rx = round.result_tx.subscribe();
        // Notify strictly after subscribing: the flusher skips rounds
        // with no receivers, so this ordering makes lost wakeups
        // impossible (a permit is stored even while the flusher is busy).
        self.wake.notify_one();
        let result_ref = result_rx
            .wait_for(Option::is_some)
            .await
            .map_err(|_| make_err!(Code::Internal, "Flush coalescer round dropped"))?;
        result_ref
            .clone()
            .unwrap_or_else(|| Err(make_err!(Code::Internal, "Flush round result missing")))
    }

    async fn flush_one_round(&self) {
        let round = self.current_round.lock().clone();
        if round.result_tx.receiver_count() == 0 {
            // Spurious wakeup (e.g. a permit left over from a round that
            // a previous iteration already served).
            return;
        }
        let send_guard = SendOnDrop(Some(round.clone()));

        // Brief accumulation window, only when this round actually has
        // multiple waiters: lets a burst pack more writers into the round
        // before it closes, trading ~3ms of publish latency (about the
        // cost of one device flush) for fewer device flushes. A solo
        // writer on an idle store skips it and pays only its own flush.
        if round.result_tx.receiver_count() > 1 {
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
        // Close the round: writers arriving from here on cannot assume
        // this flush covers them, so they must get a fresh round.
        {
            let mut current = self.current_round.lock();
            if Arc::ptr_eq(&*current, &round) {
                *current = FlushRound::new();
            }
        }
        let sentinel = self.sentinel.clone();
        let full_flush_count = self.full_flush_count.clone();
        let flush_result = spawn_blocking!("filesystem_store_full_flush", move || {
            use std::os::fd::AsRawFd;
            use std::os::unix::fs::FileExt;
            // Keep the sentinel dirty so the flush is never a no-op.
            // Best-effort: a failed write (e.g. ENOSPC on a full CoW
            // volume) must not fail the whole round — the fcntl below is
            // what provides the device-cache drain.
            if let Err(err) = sentinel.write_at(b"f", 0) {
                warn!(?err, "Flush sentinel write failed; issuing flush anyway");
            }
            // Count the actual blocking operation, rather than merely counting
            // rounds scheduled by the async task.
            full_flush_count.fetch_add(1, Ordering::Relaxed);
            loop {
                if unsafe { libc::fcntl(sentinel.as_raw_fd(), libc::F_FULLFSYNC) } != -1 {
                    return Ok(());
                }
                let err = std::io::Error::last_os_error();
                // std's sync_all retries EINTR (cvt_r); match it.
                if err.kind() != std::io::ErrorKind::Interrupted {
                    return Err(Error::from(err).append("F_FULLFSYNC failed in flush coalescer"));
                }
            }
        })
        .await
        .unwrap_or_else(|e| {
            Err(Error::from_std_err(Code::Internal, &e).append("Flush coalescer task failed"))
        });
        send_guard.finish(flush_result);
    }
}

#[cfg(unix)]
async fn prepare_executable_dir(content_path: &str) -> Result<bool, Error> {
    fn is_non_writable_error(err: &std::io::Error) -> bool {
        matches!(
            err.kind(),
            std::io::ErrorKind::PermissionDenied | std::io::ErrorKind::ReadOnlyFilesystem
        )
    }

    let executable_dir = format!("{content_path}{EXECUTABLE_DIR_SUFFIX}");
    let executable_digest_dir = format!("{executable_dir}/{DIGEST_FOLDER}");
    spawn_blocking!("filesystem_store_prepare_executable_dir", move || {
        match std::fs::remove_dir_all(&executable_dir) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            // A read-only cache must remain available for ordinary reads. Do
            // not reuse the directory, though: a surviving executable variant
            // may be stale or torn after an unclean shutdown.
            Err(err) if is_non_writable_error(&err) => return Ok(false),
            Err(err) => {
                return Err(Error::from(err)
                    .append(format!("Failed to clear executable dir {executable_dir}")));
            }
        }

        match std::fs::create_dir_all(&executable_digest_dir) {
            Ok(()) => Ok(true),
            Err(err) if is_non_writable_error(&err) => Ok(false),
            Err(err) => Err(Error::from(err).append(format!(
                "Failed to create executable dir {executable_digest_dir}"
            ))),
        }
    })
    .await
    .err_tip(|| "Failed to join executable-dir preparation task")?
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
            // ENOENT from rename is ambiguous: the source may be gone, or
            // a directory component of the destination (the temp dir) may
            // be missing. Confirm the source is genuinely gone before
            // treating it as benign — otherwise a removed temp dir would
            // flip an intact content file to Temp and orphan it on disk.
            let source_gone = err.code == Code::NotFound
                && matches!(
                    fs::metadata(&from_path).await,
                    Err(meta_err) if meta_err.code == Code::NotFound
                );
            if source_gone {
                // The file is already gone — typically another thread's
                // eviction beat us, or the entry never got its file on
                // disk. Benign here, and it dominates log volume under
                // heavy write+evict concurrency, so keep it at `debug`.
                // Mark the entry Temp (as a successful rename would) so a
                // repeat unref is a no-op and drop stops claiming the
                // content path.
                debug!(
                    key = ?encoded_file_path.key,
                    "Failed to rename file (already gone, treating as benign)",
                );
                encoded_file_path.path_type = PathType::Temp;
                encoded_file_path.key = new_key;
            } else {
                // Either a non-ENOENT failure (EACCES, EXDEV, EBUSY, …) or
                // ENOENT with the source still present (missing temp dir).
                // The content file is intact; leave the entry as Content.
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
    EvictingMap<StoreKeyBorrow, StoreKey<'a>, Arc<Fe>, SystemTime, RemoveCallbackHolder>;

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

// Sometimes we get files to emplace that are identical to the existing files
// Due to the evict/remove/replace cycle taking some amount of time, we actually
// want to drop these
// Return value is "is duplicate"
pub async fn check_duplicate_files<Fe>(
    evicting_map: &Arc<FsEvictingMap<'_, Fe>>,
    key: &StoreKey<'static>,
    entry: &Arc<Fe>,
) -> Result<bool, Error>
where
    Fe: FileEntry,
{
    let temp_file_encoded_file_path = entry.get_encoded_file_path().write().await;
    let maybe_existing_item = evicting_map.get(&key.borrow().into_owned()).await;
    if let Some(existing_item) = maybe_existing_item {
        if Arc::ptr_eq(entry, &existing_item) {
            warn!("Tried to check duplicate of an entry we already have!");
            return Ok(true);
        }
        let existing_item_encoded_file_path = existing_item.get_encoded_file_path().write().await;
        if entry.data_size() == existing_item.data_size() {
            const CHUNK_SIZE: usize = 16 * 1024; // 16kb chunks, kinda picked out of the air
            let file_length = entry.data_size();
            let existing_path = existing_item_encoded_file_path.get_file_path();
            let temp_path = temp_file_encoded_file_path.get_file_path();
            trace!(?existing_path, ?temp_path, "Checking duplicate files");
            let mut temp_file = fs::open_file(&temp_path, 0, file_length).await?;
            let mut existing_file = fs::open_file(&existing_path, 0, file_length).await?;

            let mut temp_buffer: [u8; CHUNK_SIZE] = [0; CHUNK_SIZE];
            let mut existing_buffer: [u8; CHUNK_SIZE] = [0; CHUNK_SIZE];
            // in a file_length file, there are 0 to file_length-1 entries
            // not file_length. It's counting all the bytes, starting from 0
            for offset in (0..file_length - 1).step_by(CHUNK_SIZE) {
                let buffer_size = if offset + (CHUNK_SIZE as u64) <= file_length {
                    CHUNK_SIZE
                } else if file_length < CHUNK_SIZE as u64 {
                    usize::try_from(file_length)
                        .expect("Always succeeds because file_length < 16384")
                } else {
                    usize::try_from(file_length - offset).expect("Always succeeds because offset < file_length, and offset-file_length must be < 16384")
                };
                if let Err(err) = temp_file.read_exact(&mut temp_buffer[0..buffer_size]).await {
                    warn!(
                        ?err,
                        ?temp_path,
                        file_length,
                        offset,
                        buffer_size,
                        "Failed to read temp file, skipping duplicate check"
                    );
                    return Ok(false);
                }
                if let Err(err) = existing_file
                    .read_exact(&mut existing_buffer[0..buffer_size])
                    .await
                {
                    warn!(
                        ?err,
                        ?existing_path,
                        file_length,
                        offset,
                        buffer_size,
                        "Failed to read existing, skipping duplicate check"
                    );
                    return Ok(false);
                }
                if temp_buffer.ne(&existing_buffer) {
                    trace!(
                        ?existing_path,
                        ?temp_path,
                        "Files are different, so non-duplicate"
                    );
                    return Ok(false);
                }
            }
            trace!(
                ?existing_path,
                ?temp_path,
                "Identical files, so don't need to edit, skipping emplace"
            );
            return Ok(true);
        }
        trace!(
            entry_data_size = entry.data_size(),
            existing_data_size = existing_item.data_size(),
            existing_path = ?existing_item_encoded_file_path.get_file_path(),
            "Different data sizes, so non-duplicate"
        );
    } else {
        trace!(
            temp_file = ?temp_file_encoded_file_path.get_file_path(),
            "No existing entry, so not duplicate"
        );
    }
    Ok(false)
}

/// Deletes a digest's `.exec` variant (see
/// [`FilesystemStore::get_executable_hardlink_source`]) when that digest is
/// evicted or replaced in the primary CAS `evicting_map`. Without this, the
/// `.exec` directory is invisible to `max_bytes` and is only ever cleared by
/// the startup `remove_dir_all`, so it grows without bound at runtime (#2474).
/// Tying its lifetime to the primary entry instead bounds total disk use to
/// roughly `2 * max_bytes` in the worst case (every blob also executable).
#[cfg(unix)]
#[derive(Debug)]
struct ExecutableVariantRemover {
    content_path: String,
}

#[cfg(unix)]
impl RemoveItemCallback for ExecutableVariantRemover {
    fn callback<'a>(
        &'a self,
        store_key: StoreKey<'a>,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            let StoreKey::Digest(digest) = store_key else {
                return;
            };
            let variant_path = format!(
                "{}{EXECUTABLE_DIR_SUFFIX}/{DIGEST_FOLDER}/{digest}",
                self.content_path
            );
            match fs::remove_file(&variant_path).await {
                Ok(()) => debug!(
                    ?variant_path,
                    "Deleted executable variant for evicted digest"
                ),
                // Common case: no variant was ever materialized for this digest.
                Err(err) if err.code == Code::NotFound => {}
                Err(err) => warn!(
                    ?variant_path,
                    ?err,
                    "Failed to delete executable variant for evicted digest"
                ),
            }
        })
    }
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
    /// See [`FlushCoalescer`]: amortizes the per-blob `F_FULLFSYNC` cost
    /// across concurrent uploads without weakening durability. `None` when
    /// the sentinel could not be created (e.g. read-only content volume);
    /// uploads then fall back to per-file `sync_all`, matching the old
    /// behavior (and such stores fail at write time anyway).
    #[cfg(target_os = "macos")]
    flush_coalescer: Option<Arc<FlushCoalescer>>,
    /// Per-digest single-flight locks guarding creation of the executable
    /// variant in `{content_path}.exec`, so each variant's writable fd is
    /// opened exactly once. The outer lock is sync and only ever held to
    /// get/insert/remove the per-digest async lock — never across I/O.
    #[cfg(unix)]
    executable_locks: std::sync::Mutex<HashMap<DigestInfo, Arc<Mutex<()>>>>,
    /// False when the startup wipe could not safely remove old executable
    /// variants from a read-only filesystem. Ordinary CAS reads remain
    /// available, but executable hardlink sources must not reuse stale files.
    #[cfg(unix)]
    executable_variants_enabled: bool,
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
        // inputs (see `get_executable_hardlink_source`). Cleared on writable
        // startup — the variants are regenerable and we never want a stale one
        // to leak across runs. If a read-only filesystem prevents the wipe,
        // ordinary CAS reads remain enabled but executable variants do not.
        // Unix-only: the executable bit (and the ETXTBSY race it guards
        // against) does not apply on Windows.
        #[cfg(unix)]
        let executable_variants_enabled = {
            let enabled = prepare_executable_dir(&spec.content_path).await?;
            if !enabled {
                warn!(
                    executable_dir = %format!("{}{EXECUTABLE_DIR_SUFFIX}", spec.content_path),
                    "Executable directory is not writable; serving CAS reads with executable variants disabled"
                );
            }
            if enabled {
                // Only register cleanup when executable variants are enabled.
                // Otherwise surviving variants are deliberately quarantined,
                // and repeated deletion warnings would obscure the fallback.
                evicting_map.add_remove_callback(RemoveCallbackHolder::new(Arc::new(
                    ExecutableVariantRemover {
                        content_path: spec.content_path.clone(),
                    },
                )));
            }
            enabled
        };

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
        // Never make construction fail over the durability optimization:
        // a read-only content volume must still serve reads, exactly as
        // it did before the coalescer existed.
        #[cfg(target_os = "macos")]
        let flush_coalescer = FlushCoalescer::for_content_path(&shared_context.content_path)
            .await
            .inspect_err(|err| {
                warn!(
                    ?err,
                    "Failed to create flush coalescer; falling back to per-file sync_all"
                );
            })
            .ok();

        Ok(Arc::new_cyclic(|weak_self| Self {
            shared_context,
            evicting_map,
            block_size,
            read_buffer_size,
            weak_self: weak_self.clone(),
            rename_fn,
            write_semaphore,
            #[cfg(target_os = "macos")]
            flush_coalescer,
            #[cfg(unix)]
            executable_locks: std::sync::Mutex::new(HashMap::new()),
            #[cfg(unix)]
            executable_variants_enabled,
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
        if !self.executable_variants_enabled {
            return Err(make_err!(
                Code::FailedPrecondition,
                "Executable hardlink sources are disabled because the startup wipe could not safely clear the read-only executable directory"
            ));
        }
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

        // The digest may have been evicted mid-copy: its eviction callback ran
        // before the rename published the variant, so nothing owns the file
        // anymore. This orphans the variant from eviction accounting, but the
        // race is rare enough (needs an eviction to land in the narrow window
        // between rename and this check, on a digest's first-ever variant
        // materialization) that it's a self-limiting leak, not a systemic one
        // — cheaper to log and let this action succeed with the still-valid
        // file than to fail an otherwise-successful action over it.
        if result.is_ok() && self.evicting_map.get(&digest.into()).await.is_none() {
            warn!(
                %digest,
                ?variant_path,
                "Digest evicted while materializing its executable variant; \
                 variant is now untracked by eviction accounting"
            );
        }

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
                // Belt-and-suspenders flush before publish. The `.exec`
                // directory is cleared before executable variants are enabled,
                // so durability is not needed across process restarts. The
                // flush still ensures a published variant is complete for the
                // current process. Non-macOS keeps the prior `sync_all`
                // behavior; macOS uses plain `fsync(2)` (data handed to the
                // device, no multi-ms `F_FULLFSYNC` device-cache drain), which
                // covers kernel panics — the next writable startup wipe covers
                // power loss.
                let f = std::fs::File::open(&temp_owned)
                    .map_err(|e| make_err!(Code::Internal, "executable-variant reopen: {e:?}"))?;
                #[cfg(target_os = "macos")]
                let flush_result = {
                    use std::os::fd::AsRawFd;
                    loop {
                        if unsafe { libc::fsync(f.as_raw_fd()) } == 0 {
                            break Ok(());
                        }
                        let err = std::io::Error::last_os_error();
                        if err.kind() != std::io::ErrorKind::Interrupted {
                            break Err(err);
                        }
                    }
                };
                #[cfg(not(target_os = "macos"))]
                let flush_result = f.sync_all();
                flush_result
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
        mut temp_file: FileSlot,
        final_key: StoreKey<'static>,
        mut reader: DropCloserReadHalf,
    ) -> Result<u64, Error> {
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

        // tokio defers write errors to the next write or flush call, so without an explicit flush
        // the final write's failure is silently swallowed by `sync_all`, and a truncated file would
        // be renamed into the content path.
        temp_file
            .flush()
            .await
            .err_tip(|| "Failed to flush in filesystem store")?;
        self.flush_durably(&temp_file)
            .await
            .err_tip(|| "Failed to sync in filesystem store")?;

        drop(permit);

        temp_file.advise_dontneed();
        trace!(?temp_file, "Dropping file to update_file");
        drop(temp_file);

        *entry.data_size_mut() = data_size;
        self.emplace_file(final_key, Arc::new(entry)).await?;
        Ok(data_size)
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
            // Sometimes we get files to emplace that are identical to the existing files
            // Due to the evict/remove/replace cycle taking some amount of time, we actually
            // want to drop these
            if check_duplicate_files(&evicting_map, &key, &entry).await? {
                return Ok(());
            }

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
            trace!(?key, "Finished emplace file");
            encoded_file_path.path_type = PathType::Content;
            encoded_file_path.key = key;
            Ok(())
        })
        .await
        .err_tip(|| "Failed to create spawn in filesystem store update_file")?
    }

    /// Makes `file`'s contents durable with `sync_all` semantics.
    ///
    /// On macOS the naive equivalent (`fcntl(F_FULLFSYNC)` per file) is a
    /// full device-cache flush that serializes at the device and dominates
    /// many-small-blob uploads, so the flush is split into the cheap
    /// per-file part (`fsync(2)`: push this file's pages to the device)
    /// and a device-wide cache drain shared with every concurrent upload
    /// via [`FlushCoalescer`]. Other platforms get their journal's group
    /// commit for free and keep plain `sync_all`.
    async fn flush_durably(&self, file: &FileSlot) -> Result<(), Error> {
        #[cfg(target_os = "macos")]
        {
            use std::os::fd::AsRawFd;
            let Some(flush_coalescer) = &self.flush_coalescer else {
                return file.as_ref().sync_all().await.map_err(Into::into);
            };
            // Dup the handle so the blocking task owns its own fd: if
            // this future is cancelled mid-await, the temp file's fd can
            // close and be reused, and an already-started blocking task
            // must never fsync an unrelated descriptor.
            let file_dup = file
                .as_ref()
                .try_clone()
                .await
                .err_tip(|| "Failed to dup fd for fsync in filesystem store")?
                .into_std()
                .await;
            // Deliberately NOT via `fs::call_with_permit`: the caller's
            // `FileSlot` already holds an open-file permit, so requiring a
            // second one here deadlocks when the semaphore is drained (the
            // exact scenario the #2051 regression test pins at one
            // permit). Concurrency is still bounded: every in-flight fsync
            // belongs to an upload holding a `FileSlot` permit.
            spawn_blocking!("filesystem_store_fsync", move || {
                loop {
                    if unsafe { libc::fsync(file_dup.as_raw_fd()) } == 0 {
                        return Ok(());
                    }
                    let err = std::io::Error::last_os_error();
                    // std's sync_all retries EINTR (cvt_r); match it.
                    if err.kind() != std::io::ErrorKind::Interrupted {
                        return Err(Error::from(err).append("fsync failed in filesystem store"));
                    }
                }
            })
            .await
            .err_tip(|| "Failed to join fsync task in filesystem store")??;
            flush_coalescer.commit().await
        }
        #[cfg(not(target_os = "macos"))]
        {
            file.as_ref().sync_all().await.map_err(Into::into)
        }
    }

    pub fn get_eviction_snapshot(&self) -> EvictionSnapshot {
        self.evicting_map.get_snapshot()
    }

    // Only for tests, so we can run check_duplicate_files
    pub fn get_evicting_map(&self) -> Arc<FsEvictingMap<'static, Fe>> {
        self.evicting_map.clone()
    }

    /// Returns the number of device-wide durability barriers issued by this
    /// store's macOS flush coalescer. Exposed so the integration test can pin
    /// the batching behavior rather than only checking data correctness.
    #[cfg(target_os = "macos")]
    pub fn full_flush_count_for_test(&self) -> Option<u64> {
        self.flush_coalescer
            .as_ref()
            .map(|coalescer| coalescer.full_flush_count.load(Ordering::Relaxed))
    }

    // Separated out so tests can use this
    pub async fn make_temp_file(
        &self,
        temp_key: StoreKey<'static>,
    ) -> Result<(Fe, FileSlot, OsString), Error> {
        Fe::make_and_open_file(
            self.block_size,
            EncodedFilePath {
                shared_context: self.shared_context.clone(),
                path_type: PathType::Temp,
                key: temp_key,
            },
        )
        .await
    }
}

#[async_trait]
impl<Fe: FileEntry> StoreDriver for FilesystemStore<Fe> {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())
    }

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
    ) -> Result<u64, Error> {
        if is_zero_digest(key.borrow()) {
            // don't need to add, because zero length files are just assumed to exist.
            return Ok(0);
        }

        let temp_key = make_temp_key(&key);

        // There's a possibility of deadlock here where we take all of the
        // file semaphores with make_and_open_file and the semaphores for
        // whatever is populating reader is exhasted on the threads that
        // have the FileSlots and not on those which can't.  To work around
        // this we don't take the FileSlot until there's something on the
        // reader available to know that the populator is active.
        reader.peek().await?;

        let (entry, temp_file, temp_full_path) = self.make_temp_file(temp_key).await?;

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
        let (mut entry, mut temp_file, temp_full_path) = self
            .make_temp_file(temp_key)
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

        // See comment in `update_file` above.
        temp_file
            .flush()
            .await
            .err_tip(|| "Failed to flush in filesystem store update_oneshot")?;
        self.flush_durably(&temp_file)
            .await
            .err_tip(|| "Failed to sync in filesystem store update_oneshot")?;

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
        file: FileSlot,
        upload_size: UploadSizeInfo,
    ) -> Result<(u64, Option<FileSlot>), Error> {
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
            return Ok((0, None));
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
        return Ok((file_size, None));
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
                // Map said the file was present but `open()` hit ENOENT.
                // Self-heals: we remove the stale entry below and a
                // fast/slow caller re-populates from the slow store, so
                // this is a recoverable warn, not a fatal error.
                warn!(
                    ?err,
                    key = ?owned_key,
                    "Filesystem store map/disk divergence: removing entry; reader will fall through to slow store",
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

    fn register_remove_callback(self: Arc<Self>, callback: RemoveCallback) -> Result<(), Error> {
        self.evicting_map
            .add_remove_callback(RemoveCallbackHolder::new(callback));
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
