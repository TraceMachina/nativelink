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

use core::sync::atomic::{AtomicUsize, Ordering};
use std::fs::{Metadata, Permissions};
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use bytes::{Bytes, BytesMut};
use nativelink_error::{Code, Error, ResultExt, make_err};
use rlimit::increase_nofile_limit;
/// We wrap all `tokio::fs` items in our own wrapper so we can limit the number of outstanding
/// open files at any given time. This will greatly reduce the chance we'll hit open file limit
/// issues.
pub use tokio::fs::DirEntry;
use tokio::io::SeekFrom;
use tokio::sync::{Semaphore, SemaphorePermit};
use tracing::{error, info, trace, warn};

use crate::buf_channel::{DropCloserReadHalf, DropCloserWriteHalf};
use crate::spawn_blocking;

/// Default read buffer size when reading to/from disk.
pub const DEFAULT_READ_BUFF_SIZE: usize = 64 * 1024;

#[derive(Debug)]
pub struct FileSlot {
    // We hold the permit because once it is dropped it goes back into the queue.
    _permit: SemaphorePermit<'static>,
    inner: std::fs::File,
}

impl FileSlot {
    /// Returns a reference to the underlying `std::fs::File`.
    #[inline]
    pub fn as_std(&self) -> &std::fs::File {
        &self.inner
    }

    /// Returns a mutable reference to the underlying `std::fs::File`.
    #[inline]
    pub fn as_std_mut(&mut self) -> &mut std::fs::File {
        &mut self.inner
    }

    /// Advise the kernel to drop page cache for this file's contents.
    /// Only available on Linux;
    #[cfg(target_os = "linux")]
    pub fn advise_dontneed(&self) {
        use std::os::unix::io::AsRawFd;
        let fd = self.inner.as_raw_fd();
        let ret = unsafe { libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_DONTNEED) };
        if ret != 0 {
            tracing::debug!(
                fd,
                ret,
                "posix_fadvise(DONTNEED) returned non-zero (best-effort, ignoring)",
            );
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub const fn advise_dontneed(&self) {
        // No-op: posix_fadvise is not available on Mac or Windows.
    }

    /// Advise the kernel that this file will be read sequentially,
    /// enabling more aggressive readahead (typically 2-4x default).
    #[cfg(target_os = "linux")]
    pub fn advise_sequential(&self) {
        use std::os::unix::io::AsRawFd;
        let fd = self.inner.as_raw_fd();
        let ret = unsafe { libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_SEQUENTIAL) };
        if ret != 0 {
            tracing::debug!(
                fd,
                ret,
                "posix_fadvise(SEQUENTIAL) returned non-zero (best-effort, ignoring)",
            );
        }
    }

    #[cfg(target_os = "macos")]
    pub fn advise_sequential(&self) {
        // F_RDADVISE hints that we'll read a range soon — use a 4MB initial
        // window to kick off readahead similar to Linux POSIX_FADV_SEQUENTIAL.
        self.advise_willneed(0, 4 * 1024 * 1024);
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub const fn advise_sequential(&self) {}

    /// Advise the kernel that we will soon need data at [offset, offset+len).
    /// Best-effort: errors are silently ignored.
    #[cfg(target_os = "linux")]
    pub fn advise_willneed(&self, offset: u64, len: usize) {
        use std::os::unix::io::AsRawFd;
        let fd = self.inner.as_raw_fd();
        unsafe {
            libc::posix_fadvise(fd, offset as i64, len as i64, libc::POSIX_FADV_WILLNEED);
        }
    }

    #[cfg(target_os = "macos")]
    pub fn advise_willneed(&self, offset: u64, len: usize) {
        use std::os::unix::io::AsRawFd;
        const F_RDADVISE: libc::c_int = 44;
        #[repr(C)]
        struct radvisory {
            ra_offset: libc::off_t,  // i64
            ra_count: libc::c_int,   // i32
        }
        let ra = radvisory {
            ra_offset: offset as libc::off_t,
            ra_count: len.min(i32::MAX as usize) as libc::c_int,
        };
        let fd = self.inner.as_raw_fd();
        unsafe {
            libc::fcntl(fd, F_RDADVISE, &ra);
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub const fn advise_willneed(&self, _offset: u64, _len: usize) {}
}

// Note: If the default changes make sure you update the documentation in
// `config/cas_server.rs`.
pub const DEFAULT_OPEN_FILE_LIMIT: usize = 24 * 1024; // 24k.
static OPEN_FILE_LIMIT: AtomicUsize = AtomicUsize::new(DEFAULT_OPEN_FILE_LIMIT);
pub static OPEN_FILE_SEMAPHORE: Semaphore = Semaphore::const_new(DEFAULT_OPEN_FILE_LIMIT);

/// Try to acquire a permit from the open file semaphore.
#[inline]
pub async fn get_permit() -> Result<SemaphorePermit<'static>, Error> {
    trace!(
        available_permits = OPEN_FILE_SEMAPHORE.available_permits(),
        "getting FS permit"
    );
    OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))
}
/// Acquire a permit from the open file semaphore and call a raw function.
#[inline]
pub async fn call_with_permit<F, T>(f: F) -> Result<T, Error>
where
    F: FnOnce(SemaphorePermit<'static>) -> Result<T, Error> + Send + 'static,
    T: Send + 'static,
{
    let permit = get_permit().await?;
    spawn_blocking!("fs_call_with_permit", move || f(permit))
        .await
        .unwrap_or_else(|e| Err(make_err!(Code::Internal, "background task failed: {e:?}")))
}

/// Sets the soft nofile limit to `desired_open_file_limit` and adjusts
/// `OPEN_FILE_SEMAPHORE` accordingly.
///
/// # Panics
///
/// If any type conversion fails. This can't happen if `usize` is smaller than
/// `u64`.
pub fn set_open_file_limit(desired_open_file_limit: usize) {
    // Tokio semaphores have a max of 2^61 - 1 permits. On some platforms
    // (e.g. macOS) the kernel reports an "unlimited" file limit that exceeds
    // this, causing a panic when we try to add permits. Cap at a generous but
    // safe value.
    const MAX_SAFE_LIMIT: usize = 1 << 30; // ~1 billion

    let new_open_file_limit = {
        match increase_nofile_limit(
            u64::try_from(desired_open_file_limit)
                .expect("desired_open_file_limit is too large to convert to u64."),
        ) {
            Ok(open_file_limit) => {
                info!("set_open_file_limit() assigns new open file limit {open_file_limit}.",);
                usize::try_from(open_file_limit)
                    .expect("open_file_limit is too large to convert to usize.")
                    .min(MAX_SAFE_LIMIT)
            }
            Err(e) => {
                error!(
                    "set_open_file_limit() failed to assign open file limit. Maybe system does not have ulimits, continuing anyway. - {e:?}",
                );
                DEFAULT_OPEN_FILE_LIMIT
            }
        }
    };
    // TODO(jaroeichler): Can we give a better estimate?
    if new_open_file_limit < DEFAULT_OPEN_FILE_LIMIT {
        warn!(
            "The new open file limit ({new_open_file_limit}) is below the recommended value of {DEFAULT_OPEN_FILE_LIMIT}. Consider raising max_open_files.",
        );
    }

    // Use only 80% of the open file limit for permits from OPEN_FILE_SEMAPHORE
    // to give extra room for other file descriptors like sockets, pipes, and
    // other things.
    let reduced_open_file_limit = new_open_file_limit.saturating_sub(new_open_file_limit / 5);
    let previous_open_file_limit = OPEN_FILE_LIMIT.load(Ordering::Acquire);
    // No permit should be acquired yet, so this warning should not occur.
    if (OPEN_FILE_SEMAPHORE.available_permits() + reduced_open_file_limit)
        < previous_open_file_limit
    {
        warn!(
            "There are not enough available permits to remove {previous_open_file_limit} - {reduced_open_file_limit} permits.",
        );
    }
    if previous_open_file_limit <= reduced_open_file_limit {
        OPEN_FILE_LIMIT.fetch_add(
            reduced_open_file_limit - previous_open_file_limit,
            Ordering::Release,
        );
        OPEN_FILE_SEMAPHORE.add_permits(reduced_open_file_limit - previous_open_file_limit);
    } else {
        OPEN_FILE_LIMIT.fetch_sub(
            previous_open_file_limit - reduced_open_file_limit,
            Ordering::Release,
        );
        OPEN_FILE_SEMAPHORE.forget_permits(previous_open_file_limit - reduced_open_file_limit);
    }
}

pub fn get_open_files_for_test() -> usize {
    OPEN_FILE_LIMIT.load(Ordering::Acquire) - OPEN_FILE_SEMAPHORE.available_permits()
}

pub async fn open_file(path: impl AsRef<Path>, start: u64) -> Result<FileSlot, Error> {
    let path = path.as_ref().to_owned();
    let (permit, os_file) = call_with_permit(move |permit| {
        let mut os_file =
            std::fs::File::open(&path).err_tip(|| format!("Could not open {}", path.display()))?;
        if start > 0 {
            os_file
                .seek(SeekFrom::Start(start))
                .err_tip(|| format!("Could not seek to {start} in {}", path.display()))?;
        }
        Ok((permit, os_file))
    })
    .await?;
    Ok(FileSlot {
        _permit: permit,
        inner: os_file,
    })
}

pub async fn create_file(path: impl AsRef<Path>) -> Result<FileSlot, Error> {
    let path = path.as_ref().to_owned();
    let (permit, os_file) = call_with_permit(move |permit| {
        Ok((
            permit,
            std::fs::File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .open(&path)
                .err_tip(|| format!("Could not open {}", path.display()))?,
        ))
    })
    .await?;
    Ok(FileSlot {
        _permit: permit,
        inner: os_file,
    })
}

/// Read from `file` in a blocking thread, sending chunks to `writer`.
/// Reads up to `limit` bytes starting from `start_offset`.
/// `read_buffer_size` controls the chunk size (typically 256 KiB).
/// After each read, prefetches the next 2 chunks via `advise_willneed`.
/// Returns the `FileSlot` so the caller can reuse or drop it.
pub async fn read_file_to_channel(
    file: FileSlot,
    writer: &mut DropCloserWriteHalf,
    limit: u64,
    read_buffer_size: usize,
    start_offset: u64,
) -> Result<FileSlot, Error> {
    let (sync_tx, mut async_rx) = tokio::sync::mpsc::channel::<Result<Bytes, Error>>(4);

    let read_task = spawn_blocking!("fs_read_file", move || {
        let mut f = file;
        let mut remaining = limit;
        let mut current_offset = start_offset;
        loop {
            let to_read = read_buffer_size.min(remaining as usize);
            if to_read == 0 {
                break;
            }
            let mut buf = BytesMut::zeroed(to_read);
            match f.as_std_mut().read(&mut buf[..]) {
                Ok(0) => break,
                Ok(n) => {
                    buf.truncate(n);
                    current_offset += n as u64;
                    remaining -= n as u64;
                    // Prefetch next 2 chunks while this one travels over the network.
                    f.advise_willneed(current_offset, read_buffer_size * 2);
                    if sync_tx.blocking_send(Ok(buf.freeze())).is_err() {
                        break; // reader dropped
                    }
                }
                Err(e) => {
                    drop(sync_tx.blocking_send(Err(e.into())));
                    break;
                }
            }
        }
        f
    });

    // Receive chunks and forward to the async writer.
    while let Some(result) = async_rx.recv().await {
        let chunk = result?;
        writer
            .send(chunk)
            .await
            .err_tip(|| "Failed to send chunk from file reader")?;
    }
    // Ensure the blocking task completed successfully.
    read_task
        .await
        .map_err(|e| make_err!(Code::Internal, "read task join failed: {e:?}"))
}

/// Write to `file` from a blocking thread, receiving chunks from `reader`.
/// Returns total bytes written and the `FileSlot`.
pub async fn write_file_from_channel(
    file: FileSlot,
    reader: &mut DropCloserReadHalf,
) -> Result<(u64, FileSlot), Error> {
    let (async_tx, mut sync_rx) = tokio::sync::mpsc::channel::<Bytes>(4);

    let write_task = spawn_blocking!("fs_write_file", move || {
        let mut f = file;
        let mut total: u64 = 0;
        while let Some(data) = sync_rx.blocking_recv() {
            f.as_std_mut()
                .write_all(&data)
                .map_err(|e| Into::<Error>::into(e))?;
            total += data.len() as u64;
        }
        Ok::<_, Error>((total, f))
    });

    // Async side: recv from channel, send to blocking writer.
    let send_result: Result<(), Error> = async {
        loop {
            let data = reader
                .recv()
                .await
                .err_tip(|| "Failed to recv in write_file_from_channel")?;
            if data.is_empty() {
                break; // EOF
            }
            if async_tx.send(data).await.is_err() {
                // Writer task died — we'll get the error from write_task.
                break;
            }
        }
        Ok(())
    }
    .await;
    drop(async_tx); // Signal EOF to writer.

    let (total, file) = write_task
        .await
        .map_err(|e| make_err!(Code::Internal, "write task join failed: {e:?}"))??;

    send_result?;
    Ok((total, file))
}

pub async fn hard_link(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
    let src = src.as_ref().to_owned();
    let dst = dst.as_ref().to_owned();
    call_with_permit(move |_| std::fs::hard_link(src, dst).map_err(Into::<Error>::into)).await
}

pub async fn set_permissions(src: impl AsRef<Path>, perm: Permissions) -> Result<(), Error> {
    let src = src.as_ref().to_owned();
    call_with_permit(move |_| std::fs::set_permissions(src, perm).map_err(Into::<Error>::into))
        .await
}

pub async fn create_dir(path: impl AsRef<Path>) -> Result<(), Error> {
    let path = path.as_ref().to_owned();
    call_with_permit(move |_| std::fs::create_dir(path).map_err(Into::<Error>::into)).await
}

pub async fn create_dir_all(path: impl AsRef<Path>) -> Result<(), Error> {
    let path = path.as_ref().to_owned();
    call_with_permit(move |_| std::fs::create_dir_all(path).map_err(Into::<Error>::into)).await
}

#[cfg(target_family = "unix")]
pub async fn symlink(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
    // TODO: add a test for #2051: deadlock with large number of files
    let _permit = get_permit().await?;
    tokio::fs::symlink(src, dst).await.map_err(Into::into)
}

pub async fn read_link(path: impl AsRef<Path>) -> Result<PathBuf, Error> {
    let path = path.as_ref().to_owned();
    call_with_permit(move |_| std::fs::read_link(path).map_err(Into::<Error>::into)).await
}

#[derive(Debug)]
pub struct ReadDir {
    // We hold the permit because once it is dropped it goes back into the queue.
    permit: SemaphorePermit<'static>,
    inner: tokio::fs::ReadDir,
}

impl ReadDir {
    pub fn into_inner(self) -> (SemaphorePermit<'static>, tokio::fs::ReadDir) {
        (self.permit, self.inner)
    }
}

impl AsRef<tokio::fs::ReadDir> for ReadDir {
    fn as_ref(&self) -> &tokio::fs::ReadDir {
        &self.inner
    }
}

impl AsMut<tokio::fs::ReadDir> for ReadDir {
    fn as_mut(&mut self) -> &mut tokio::fs::ReadDir {
        &mut self.inner
    }
}

pub async fn read_dir(path: impl AsRef<Path>) -> Result<ReadDir, Error> {
    let path = path.as_ref().to_owned();
    let (permit, inner) = call_with_permit(move |permit| {
        Ok((
            permit,
            tokio::runtime::Handle::current()
                .block_on(tokio::fs::read_dir(path))
                .map_err(Into::<Error>::into)?,
        ))
    })
    .await?;
    Ok(ReadDir { permit, inner })
}

pub async fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<(), Error> {
    let from = from.as_ref().to_owned();
    let to = to.as_ref().to_owned();
    call_with_permit(move |_| std::fs::rename(from, to).map_err(Into::<Error>::into)).await
}

pub async fn remove_file(path: impl AsRef<Path>) -> Result<(), Error> {
    let path = path.as_ref().to_owned();
    call_with_permit(move |_| std::fs::remove_file(path).map_err(Into::<Error>::into)).await
}

pub async fn canonicalize(path: impl AsRef<Path>) -> Result<PathBuf, Error> {
    let path = path.as_ref().to_owned();
    call_with_permit(move |_| std::fs::canonicalize(path).map_err(Into::<Error>::into)).await
}

pub async fn metadata(path: impl AsRef<Path>) -> Result<Metadata, Error> {
    let path = path.as_ref().to_owned();
    call_with_permit(move |_| std::fs::metadata(path).map_err(Into::<Error>::into)).await
}

pub async fn read(path: impl AsRef<Path>) -> Result<Vec<u8>, Error> {
    let path = path.as_ref().to_owned();
    call_with_permit(move |_| std::fs::read(path).map_err(Into::<Error>::into)).await
}

pub async fn symlink_metadata(path: impl AsRef<Path>) -> Result<Metadata, Error> {
    let path = path.as_ref().to_owned();
    call_with_permit(move |_| std::fs::symlink_metadata(path).map_err(Into::<Error>::into)).await
}

// We can't just use the stock remove_dir_all as it falls over if someone's set readonly
// permissions. This version walks the directories and fixes the permissions where needed
// before deleting everything.
#[cfg(not(target_family = "windows"))]
fn internal_remove_dir_all(path: impl AsRef<Path>) -> Result<(), Error> {
    // Because otherwise Windows builds complain about these things not being used
    use std::io::ErrorKind;
    use std::os::unix::fs::PermissionsExt;

    use tracing::debug;
    use walkdir::WalkDir;

    for entry in WalkDir::new(&path) {
        let Ok(entry) = &entry else {
            debug!("Can't get into {entry:?}, assuming already deleted");
            continue;
        };
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            match std::fs::remove_dir_all(entry.path()) {
                Ok(()) => {}
                Err(e) if e.kind() == ErrorKind::PermissionDenied => {
                    std::fs::set_permissions(entry.path(), Permissions::from_mode(0o700)).err_tip(
                        || format!("Setting permissions for {}", entry.path().display()),
                    )?;
                }
                e @ Err(_) => e.err_tip(|| format!("Removing {}", entry.path().display()))?,
            }
        } else if metadata.is_file() {
            std::fs::set_permissions(entry.path(), Permissions::from_mode(0o600))
                .err_tip(|| format!("Setting permissions for {}", entry.path().display()))?;
        }
    }

    // should now be safe to delete after we fixed all the permissions in the walk loop
    match std::fs::remove_dir_all(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        e @ Err(_) => e.err_tip(|| {
            format!(
                "Removing {} after permissions fixes",
                path.as_ref().display()
            )
        })?,
    }
    Ok(())
}

// We can't set the permissions easily in Windows, so just fallback to
// the stock Rust remove_dir_all
#[cfg(target_family = "windows")]
fn internal_remove_dir_all(path: impl AsRef<Path>) -> Result<(), Error> {
    std::fs::remove_dir_all(&path)?;
    Ok(())
}

pub async fn remove_dir_all(path: impl AsRef<Path>) -> Result<(), Error> {
    let path = path.as_ref().to_owned();
    call_with_permit(move |_| internal_remove_dir_all(path)).await
}
