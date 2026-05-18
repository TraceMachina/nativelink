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

use core::pin::Pin;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::task::{Context, Poll};
use std::fs::{Metadata, Permissions};
use std::io::{IoSlice, Seek};
use std::path::{Path, PathBuf};

use nativelink_error::{Code, Error, ResultExt, make_err};
use rlimit::increase_nofile_limit;
/// We wrap all `tokio::fs` items in our own wrapper so we can limit the number of outstanding
/// open files at any given time. This will greatly reduce the chance we'll hit open file limit
/// issues.
pub use tokio::fs::DirEntry;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncWrite, ReadBuf, SeekFrom, Take};
use tokio::sync::{Semaphore, SemaphorePermit};
use tracing::{error, info, trace, warn};

use crate::spawn_blocking;

/// Default read buffer size when reading to/from disk.
pub const DEFAULT_READ_BUFF_SIZE: usize = 0x4000;

#[derive(Debug)]
pub struct FileSlot {
    // We hold the permit because once it is dropped it goes back into the queue.
    _permit: SemaphorePermit<'static>,
    inner: tokio::fs::File,
}

impl FileSlot {
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

    /// Advise the kernel that we will read this file sequentially.
    ///
    /// On Linux this issues `posix_fadvise(POSIX_FADV_SEQUENTIAL)` so the
    /// kernel performs aggressive read-ahead. On macOS there is no direct
    /// equivalent, so we issue an `F_RDADVISE` over an initial 4 MiB window —
    /// this primes the unified buffer cache with the same effect for the
    /// first reads of a sequential scan. Best-effort: errors are ignored.
    #[cfg(target_os = "linux")]
    pub fn advise_sequential(&self) {
        use std::os::unix::io::AsRawFd;
        let fd = self.inner.as_raw_fd();
        // Length 0 = "apply to the entire file from offset".
        let ret = unsafe { libc::posix_fadvise(fd, 0, 0, libc::POSIX_FADV_SEQUENTIAL) };
        if ret != 0 {
            tracing::debug!(
                fd,
                ret,
                "posix_fadvise(SEQUENTIAL) returned non-zero (best-effort, ignoring)",
            );
        }
    }

    /// macOS analogue of `POSIX_FADV_SEQUENTIAL`: kick off readahead for
    /// the first 4 MiB via `F_RDADVISE`.
    #[cfg(target_os = "macos")]
    pub fn advise_sequential(&self) {
        self.advise_willneed(0, 4 * 1024 * 1024);
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub const fn advise_sequential(&self) {
        // No-op: no equivalent advisory on this platform.
    }

    /// Advise the kernel that we will soon need data at `[offset, offset+len)`.
    ///
    /// Linux uses `posix_fadvise(POSIX_FADV_WILLNEED)`; macOS uses
    /// `fcntl(F_RDADVISE)` with a `radvisory` struct (the documented
    /// equivalent — see `<sys/fcntl.h>`). Best-effort: errors are silently
    /// ignored. No-op on other platforms.
    #[cfg(target_os = "linux")]
    pub fn advise_willneed(&self, offset: u64, len: usize) {
        use std::os::unix::io::AsRawFd;
        let fd = self.inner.as_raw_fd();
        // posix_fadvise length is a signed off_t — clamp to i64::MAX to avoid
        // an overflow on 32-bit platforms (unlikely for our targets, but cheap).
        let len_i64 = i64::try_from(len).unwrap_or(i64::MAX);
        let offset_i64 = i64::try_from(offset).unwrap_or(i64::MAX);
        unsafe {
            libc::posix_fadvise(fd, offset_i64, len_i64, libc::POSIX_FADV_WILLNEED);
        }
    }

    /// macOS implementation of `advise_willneed`. The `F_RDADVISE` fcntl takes
    /// a `radvisory` struct; the kernel uses this hint to start fetching the
    /// requested byte range into the unified buffer cache.
    ///
    /// `F_RDADVISE` is defined as `44` in macOS's `<sys/fcntl.h>` but the
    /// `libc` crate does not currently expose it, so we declare the constant
    /// locally. The `radvisory` layout matches `struct radvisory` exactly:
    /// `off_t ra_offset` followed by `int ra_count`.
    #[cfg(target_os = "macos")]
    pub fn advise_willneed(&self, offset: u64, len: usize) {
        use std::os::unix::io::AsRawFd;
        const F_RDADVISE: libc::c_int = 44;
        #[repr(C)]
        struct Radvisory {
            ra_offset: libc::off_t, // i64 on Darwin
            ra_count: libc::c_int,  // i32
        }
        let ra = Radvisory {
            ra_offset: i64::try_from(offset).unwrap_or(i64::MAX),
            ra_count: i32::try_from(len.min(i32::MAX as usize)).unwrap_or(i32::MAX),
        };
        let fd = self.inner.as_raw_fd();
        // SAFETY: `&ra` is a valid pointer to a properly-aligned `Radvisory`
        // for the lifetime of this call; the kernel reads but does not retain it.
        unsafe {
            libc::fcntl(fd, F_RDADVISE, &ra);
        }
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    pub const fn advise_willneed(&self, _offset: u64, _len: usize) {
        // No-op: no equivalent advisory on this platform.
    }
}

impl AsRef<tokio::fs::File> for FileSlot {
    fn as_ref(&self) -> &tokio::fs::File {
        &self.inner
    }
}

impl AsMut<tokio::fs::File> for FileSlot {
    fn as_mut(&mut self) -> &mut tokio::fs::File {
        &mut self.inner
    }
}

impl AsyncRead for FileSlot {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl AsyncSeek for FileSlot {
    fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> Result<(), tokio::io::Error> {
        Pin::new(&mut self.inner).start_seek(position)
    }

    fn poll_complete(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<u64, tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_complete(cx)
    }
}

impl AsyncWrite for FileSlot {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<Result<usize, tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
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
        .unwrap_or_else(|e| {
            Err(Error::from_std_err(Code::Internal, &e).append("background task failed"))
        })
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

pub async fn open_file(
    path: impl AsRef<Path>,
    start: u64,
    limit: u64,
) -> Result<Take<FileSlot>, Error> {
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
        inner: tokio::fs::File::from_std(os_file),
    }
    .take(limit))
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
        inner: tokio::fs::File::from_std(os_file),
    })
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

#[cfg(test)]
mod advise_tests {
    //! Tests for the kernel page-cache advisory helpers (`advise_sequential` /
    //! `advise_willneed`). These exercise the per-platform code path on macOS
    //! and Linux, and verify that the no-op fallbacks compile and are callable
    //! on every other target.

    use std::env;
    use std::io::Write;

    use nativelink_macro::nativelink_test;

    use super::open_file;

    // F_RDADVISE constant used by the macOS direct-syscall test below.
    // Declared at module scope to satisfy `clippy::items_after_statements`.
    #[cfg(target_os = "macos")]
    const TEST_F_RDADVISE: libc::c_int = 44;

    /// Matches the layout of macOS `struct radvisory` from `<sys/fcntl.h>`.
    /// Used by the direct-syscall test below.
    #[cfg(target_os = "macos")]
    #[repr(C)]
    struct TestRadvisory {
        ra_offset: libc::off_t,
        ra_count: libc::c_int,
    }

    /// Create a temp file with `size` bytes of zeros and return its path.
    fn make_temp_file(name: &str, size: usize) -> std::path::PathBuf {
        let dir = env::temp_dir().join("nativelink_advise_tests");
        // Best-effort: ignore if already exists.
        drop(std::fs::create_dir_all(&dir));
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).expect("create temp file");
        if size > 0 {
            // Write in 64 KiB chunks so we don't allocate the whole buffer at once.
            #[allow(clippy::decimal_literal_representation)]
            let chunk_size: usize = 65_536;
            let chunk = vec![0u8; chunk_size.min(size)];
            let mut remaining = size;
            while remaining > 0 {
                let n = remaining.min(chunk.len());
                f.write_all(&chunk[..n]).expect("write");
                remaining -= n;
            }
            f.flush().expect("flush");
        }
        path
    }

    /// Cross-platform smoke test: the helpers must be safely callable on every
    /// platform we build for. On Linux/macOS this hits the real syscall path;
    /// on other targets it must compile to a no-op.
    #[nativelink_test("crate")]
    async fn advise_helpers_are_callable_everywhere() -> Result<(), Box<dyn core::error::Error>> {
        let path = make_temp_file("advise_xplat.bin", 1024);
        let file = open_file(&path, 0, u64::MAX)
            .await
            .expect("open_file should succeed");
        // Both helpers are best-effort and must never panic, even on
        // platforms where they are no-ops.
        file.get_ref().advise_sequential();
        file.get_ref().advise_willneed(0, 4096);
        // Out-of-range offsets / sizes must also be safe.
        file.get_ref().advise_willneed(u64::MAX, usize::MAX);
        drop(std::fs::remove_file(&path));
        Ok(())
    }

    /// macOS-only test: directly invoke the `F_RDADVISE` syscall path and
    /// confirm it returns `0` (success). This is the real path we ship for
    /// Mac-Mini RBE workers.
    #[cfg(target_os = "macos")]
    #[nativelink_test("crate")]
    async fn macos_f_rdadvise_returns_success() -> Result<(), Box<dyn core::error::Error>> {
        use std::os::unix::io::AsRawFd;
        // Allocate something large enough for the 4 MiB sequential window to
        // be meaningful: 8 MiB.
        let path = make_temp_file("advise_macos.bin", 8 * 1024 * 1024);
        let file = open_file(&path, 0, u64::MAX)
            .await
            .expect("open_file should succeed");

        let ra = TestRadvisory {
            ra_offset: 0,
            ra_count: 4 * 1024 * 1024,
        };
        // SAFETY: fd is open for the duration of this call; `&ra` is valid
        // and the kernel does not retain the pointer.
        let ret = unsafe { libc::fcntl(file.get_ref().inner.as_raw_fd(), TEST_F_RDADVISE, &ra) };
        assert_eq!(ret, 0, "F_RDADVISE should succeed (errno set otherwise)");

        // And via our wrapper, which must be equally happy.
        file.get_ref().advise_sequential();
        file.get_ref().advise_willneed(0, 4 * 1024 * 1024);
        drop(std::fs::remove_file(&path));
        Ok(())
    }
}
