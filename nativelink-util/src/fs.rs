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
#[cfg(all(feature = "io-uring", target_os = "linux"))]
use std::sync::OnceLock;

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

/// Runtime probe for io_uring availability. On first call, attempts to
/// launch a `tokio_epoll_uring::System`. If the kernel does not support
/// io_uring (old kernel, container with seccomp, etc.), the flag is set
/// to false and all subsequent calls fall back to the spawn_blocking path
/// for the rest of the process lifetime.
#[cfg(all(feature = "io-uring", target_os = "linux"))]
static IO_URING_AVAILABLE: OnceLock<bool> = OnceLock::new();

/// Check whether io_uring is available on this system. On first call,
/// probes by launching a `tokio_epoll_uring::System`. The result is
/// cached for the lifetime of the process.
#[cfg(all(feature = "io-uring", target_os = "linux"))]
async fn is_io_uring_available() -> bool {
    if let Some(&available) = IO_URING_AVAILABLE.get() {
        return available;
    }
    // First call — probe by actually launching a System (which calls
    // io_uring_setup internally). This is the same code path that
    // thread_local_system() uses, but we handle the error instead
    // of panicking.
    let available = match tokio_epoll_uring::System::launch().await {
        Ok(_handle) => {
            info!("io_uring runtime probe succeeded, using io_uring for filesystem ops");
            true
        }
        Err(e) => {
            warn!(
                error = %e,
                "io_uring runtime probe failed, falling back to spawn_blocking for all filesystem ops"
            );
            false
        }
    };
    // Another thread may have raced us; that's fine, the value is
    // deterministic (same kernel on both probes).
    let _ = IO_URING_AVAILABLE.set(available);
    available
}

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

    /// Decompose into the semaphore permit and raw `std::fs::File`.
    /// Used by the io_uring path which needs ownership transfer.
    #[inline]
    pub fn into_inner(self) -> (SemaphorePermit<'static>, std::fs::File) {
        (self._permit, self.inner)
    }

    /// Reconstitute from a permit and file returned by io_uring.
    #[inline]
    pub fn from_parts(permit: SemaphorePermit<'static>, file: std::fs::File) -> Self {
        Self {
            _permit: permit,
            inner: file,
        }
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

/// Open a file for reading.
///
/// On io_uring: uses `openat` via io_uring (no spawn_blocking, no seek).
/// The io_uring read path uses `pread` with explicit offsets so file
/// position doesn't matter. The `start` parameter is stored for fallback
/// paths that use sequential `read()` calls.
///
/// On non-io_uring: delegates to `open_file_std` (spawn_blocking + seek).
#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn open_file(path: impl AsRef<Path>, start: u64) -> Result<FileSlot, Error> {
    if !is_io_uring_available().await {
        return open_file_std(path, start).await;
    }
    let path = path.as_ref().to_owned();
    let permit = get_permit().await?;
    let system = tokio_epoll_uring::thread_local_system().await;
    let mut opts = tokio_epoll_uring::ops::open_at::OpenOptions::new();
    opts.read(true);
    let owned_fd = system
        .open(&path, &opts)
        .await
        .map_err(|e| uring_err(e, &format!("open {}", path.display())))?;
    let _ = start; // pread uses explicit offsets; no seek needed
    Ok(FileSlot::from_parts(permit, owned_fd.into()))
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
pub async fn open_file(path: impl AsRef<Path>, start: u64) -> Result<FileSlot, Error> {
    open_file_std(path, start).await
}

async fn open_file_std(path: impl AsRef<Path>, start: u64) -> Result<FileSlot, Error> {
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

/// Create a file for read+write via io_uring openat with O_CREAT|O_TRUNC.
/// Falls back to spawn_blocking if io_uring is unavailable at runtime.
#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn create_file(path: impl AsRef<Path>) -> Result<FileSlot, Error> {
    if !is_io_uring_available().await {
        return create_file_std(path).await;
    }
    let path = path.as_ref().to_owned();
    let create_start = std::time::Instant::now();
    let permit = get_permit().await?;
    let system = tokio_epoll_uring::thread_local_system().await;
    let mut opts = tokio_epoll_uring::ops::open_at::OpenOptions::new();
    opts.read(true).write(true).create(true).truncate(true);
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let owned_fd = system
        .open(&path, &opts)
        .await
        .map_err(|e| uring_err(e, &format!("create {}", path.display())))?;
    let create_ms = create_start.elapsed().as_millis();
    if create_ms > 100 {
        warn!(
            create_ms,
            "create_file: slow io_uring file creation (>100ms)"
        );
    }
    Ok(FileSlot::from_parts(permit, owned_fd.into()))
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
pub async fn create_file(path: impl AsRef<Path>) -> Result<FileSlot, Error> {
    create_file_std(path).await
}

async fn create_file_std(path: impl AsRef<Path>) -> Result<FileSlot, Error> {
    let path = path.as_ref().to_owned();
    let create_start = std::time::Instant::now();
    let (permit, os_file) = call_with_permit(move |permit| {
        use std::os::unix::fs::OpenOptionsExt;
        Ok((
            permit,
            std::fs::File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&path)
                .err_tip(|| format!("Could not open {}", path.display()))?,
        ))
    })
    .await?;
    let create_ms = create_start.elapsed().as_millis();
    if create_ms > 100 {
        warn!(
            create_ms,
            "create_file: slow file creation (>100ms), may indicate semaphore contention or disk latency"
        );
    }
    Ok(FileSlot {
        _permit: permit,
        inner: os_file,
    })
}

/// Convert a `tokio_epoll_uring` operation error into a NativeLink `Error`.
/// Maps `io::ErrorKind::NotFound` to `Code::NotFound` so upper layers
/// can distinguish missing files from internal failures.
#[cfg(all(feature = "io-uring", target_os = "linux"))]
fn uring_err(e: tokio_epoll_uring::Error<std::io::Error>, ctx: &str) -> Error {
    match e {
        tokio_epoll_uring::Error::Op(io_err) => {
            let code = match io_err.kind() {
                std::io::ErrorKind::NotFound => Code::NotFound,
                std::io::ErrorKind::PermissionDenied => Code::PermissionDenied,
                std::io::ErrorKind::AlreadyExists => Code::AlreadyExists,
                _ => Code::Internal,
            };
            make_err!(code, "io_uring {ctx}: {io_err:?}")
        }
        tokio_epoll_uring::Error::System(sys_err) => {
            make_err!(Code::Internal, "io_uring system error in {ctx}: {sys_err:?}")
        }
    }
}

/// Read from `file` via io_uring or spawn_blocking, sending chunks to `writer`.
///
/// Strategy by file size:
/// - **Single-chunk files** (limit <= read_buffer_size): synchronous `pread()`
///   on the async thread. For page-cache warm small blobs (~73% of production
///   traffic), this is ~500ns — no io_uring round-trip, no thread pool.
/// - **Multi-chunk files**: spawn_blocking sequential read loop. Benchmarks
///   show this is 2-5x faster than io_uring batch pread for >=1MB files due
///   to lower per-chunk coordination overhead.
///
/// Falls back to spawn_blocking for all reads if io_uring is unavailable.
#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn read_file_to_channel(
    file: FileSlot,
    writer: &mut DropCloserWriteHalf,
    limit: u64,
    read_buffer_size: usize,
    start_offset: u64,
) -> Result<FileSlot, Error> {
    if !is_io_uring_available().await {
        return read_file_to_channel_std(file, writer, limit, read_buffer_size, start_offset).await;
    }

    if limit == 0 || read_buffer_size == 0 {
        return Ok(file);
    }

    // --- Single-chunk synchronous pread fast path ---
    // For small blobs (≤64KB), a direct pread() syscall on the async
    // thread is faster than an io_uring round-trip or spawn_blocking.
    // 16KB threshold: p50 blob is 8KB, cold 16KB SSD read is max ~100μs.
    // Higher thresholds risk 1-5ms stalls on cold ZFS reads under txg sync.
    const SYNC_PREAD_THRESHOLD: u64 = 16 * 1024; // 16 KiB
    if limit <= read_buffer_size as u64 && limit <= SYNC_PREAD_THRESHOLD {
        use std::os::unix::io::AsRawFd;

        let fd = file.as_std().as_raw_fd();
        let mut buf = vec![0u8; limit as usize];
        let n = loop {
            let ret = unsafe {
                libc::pread(
                    fd,
                    buf.as_mut_ptr() as *mut libc::c_void,
                    buf.len(),
                    start_offset as libc::off_t,
                )
            };
            if ret >= 0 {
                break ret;
            }
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue; // retry on EINTR
            }
            return Err(make_err!(
                Code::Internal,
                "pread failed: {:?}",
                err
            ));
        };
        if n > 0 {
            buf.truncate(n as usize);
            writer
                .send(Bytes::from(buf))
                .await
                .err_tip(|| "failed to send chunk from file reader")?;
        }
        return Ok(file);
    }

    // Multi-chunk: spawn_blocking sequential read loop.
    // Benchmarks show this is 2-5x faster than io_uring batch pread
    // for >=1MB files due to lower per-chunk coordination overhead.
    read_file_to_channel_std(file, writer, limit, read_buffer_size, start_offset).await
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
pub async fn read_file_to_channel(
    file: FileSlot,
    writer: &mut DropCloserWriteHalf,
    limit: u64,
    read_buffer_size: usize,
    start_offset: u64,
) -> Result<FileSlot, Error> {
    read_file_to_channel_std(file, writer, limit, read_buffer_size, start_offset).await
}

/// Read from `file` in a blocking thread, sending chunks to `writer`.
/// Reads up to `limit` bytes starting from `start_offset`.
/// `read_buffer_size` controls the chunk size (typically 256 KiB).
/// After each read, prefetches the next 2 chunks via `advise_willneed`.
/// Returns the `FileSlot` so the caller can reuse or drop it.
async fn read_file_to_channel_std(
    file: FileSlot,
    writer: &mut DropCloserWriteHalf,
    limit: u64,
    read_buffer_size: usize,
    start_offset: u64,
) -> Result<FileSlot, Error> {
    let (sync_tx, mut async_rx) = tokio::sync::mpsc::channel::<Result<Bytes, Error>>(8);

    let read_task = spawn_blocking!("fs_read_file", move || {
        let mut f = file;
        // Ensure file position matches start_offset. On the io_uring open
        // path, the file is opened without seeking (pread uses explicit
        // offsets). The sequential read() loop below needs correct position.
        if start_offset > 0 {
            if let Err(e) = f.as_std_mut().seek(SeekFrom::Start(start_offset)) {
                drop(sync_tx.blocking_send(Err(e.into())));
                return f;
            }
        }
        let mut remaining = limit;
        let mut current_offset = start_offset;
        loop {
            let to_read = read_buffer_size.min(remaining as usize);
            if to_read == 0 {
                break;
            }
            let mut buf = BytesMut::zeroed(to_read);
            let read_start = std::time::Instant::now();
            match f.as_std_mut().read(&mut buf[..]) {
                Ok(0) => break,
                Ok(n) => {
                    let read_ms = read_start.elapsed().as_millis();
                    if read_ms > 100 {
                        warn!(
                            read_ms,
                            bytes_read = n,
                            current_offset,
                            "read_file_to_channel: slow read syscall (>100ms)"
                        );
                    }
                    buf.truncate(n);
                    current_offset += n as u64;
                    remaining = remaining.saturating_sub(n as u64);
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

/// Read via mmap + memcpy in a blocking thread.
/// Maps the entire read region with a single `mmap()` call, then copies
/// chunks to the writer channel. Avoids per-chunk `read()` syscalls —
/// after the initial mapping, data access is pure memcpy from page cache.
///
/// Uses `MAP_POPULATE` to pre-fault pages and `MADV_SEQUENTIAL` for
/// aggressive kernel readahead.
#[cfg(target_os = "linux")]
pub async fn read_file_to_channel_mmap(
    file: FileSlot,
    writer: &mut DropCloserWriteHalf,
    limit: u64,
    read_buffer_size: usize,
    start_offset: u64,
) -> Result<FileSlot, Error> {
    if limit == 0 || read_buffer_size == 0 {
        return Ok(file);
    }

    let (sync_tx, mut async_rx) = tokio::sync::mpsc::channel::<Result<Bytes, Error>>(8);

    let read_task = spawn_blocking!("fs_read_mmap", move || {
        use std::os::unix::io::AsRawFd;

        let fd = file.as_std().as_raw_fd();

        // Page-align the mmap offset (mmap requires page-aligned offset).
        let page_size = 4096u64;
        let mmap_offset = start_offset & !(page_size - 1);
        let offset_in_page = (start_offset - mmap_offset) as usize;
        let mmap_len = (limit as usize) + offset_in_page;

        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                mmap_len,
                libc::PROT_READ,
                libc::MAP_PRIVATE | libc::MAP_POPULATE,
                fd,
                mmap_offset as libc::off_t,
            )
        };
        if ptr == libc::MAP_FAILED {
            let e = std::io::Error::last_os_error();
            drop(sync_tx.blocking_send(Err(make_err!(
                Code::Internal,
                "mmap failed: {e:?}"
            ))));
            return file;
        }

        unsafe {
            libc::madvise(ptr, mmap_len, libc::MADV_SEQUENTIAL);
        }

        let base = ptr as *const u8;
        let mut pos = offset_in_page;
        let end = offset_in_page + limit as usize;

        while pos < end {
            let chunk_size = read_buffer_size.min(end - pos);
            let chunk = Bytes::copy_from_slice(unsafe {
                std::slice::from_raw_parts(base.add(pos), chunk_size)
            });
            pos += chunk_size;
            if sync_tx.blocking_send(Ok(chunk)).is_err() {
                break;
            }
        }

        unsafe {
            libc::munmap(ptr, mmap_len);
        }
        file
    });

    while let Some(result) = async_rx.recv().await {
        let chunk = result?;
        writer
            .send(chunk)
            .await
            .err_tip(|| "Failed to send mmap chunk")?;
    }

    read_task
        .await
        .map_err(|e| make_err!(Code::Internal, "mmap read task join failed: {e:?}"))
}

/// Explicitly use the spawn_blocking read path, bypassing io_uring.
/// Exposed for benchmarking backend comparisons.
pub async fn read_file_to_channel_blocking(
    file: FileSlot,
    writer: &mut DropCloserWriteHalf,
    limit: u64,
    read_buffer_size: usize,
    start_offset: u64,
) -> Result<FileSlot, Error> {
    read_file_to_channel_std(file, writer, limit, read_buffer_size, start_offset).await
}

/// Write to `file` via coalesced io_uring pwritev, receiving chunks from
/// `reader`. Small incoming chunks (typically 16 KiB from gRPC h2 framing)
/// are accumulated until we have at least `COALESCE_TARGET` bytes or a
/// timeout expires, then submitted as a single `IORING_OP_WRITEV` SQE
/// with an iovec array pointing to all accumulated `Bytes` buffers.
/// This is zero-copy — the kernel reads directly from the original gRPC
/// frame allocations.
///
/// Up to `WRITE_PIPELINE_DEPTH` writev ops are kept in-flight
/// simultaneously, overlapping ZFS/kernel processing with coalescing
/// and submission of the next batch.
///
/// The fd is wrapped in `Arc<std::fs::File>` so each in-flight write
/// can hold its own `Arc` handle (required by `IoFd` ownership semantics
/// in `tokio_epoll_uring::SystemHandle::writev`). Since all writes use
/// pwritev with explicit offsets, concurrent writes to the same fd are
/// safe — the kernel handles per-write positioning independently of the
/// file cursor.
///
/// Falls back to spawn_blocking if io_uring is unavailable at runtime.
#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn write_file_from_channel(
    file: FileSlot,
    reader: &mut DropCloserReadHalf,
) -> Result<(u64, FileSlot), Error> {
    use std::sync::Arc;
    use std::time::Duration;

    use futures::FutureExt;
    use futures::stream::{FuturesUnordered, StreamExt};

    /// Maximum number of io_uring writev futures in flight simultaneously.
    /// Matched to RING_SIZE (1024) and buf_channel capacity (1024) so
    /// the full pipeline can be utilized without artificial bottlenecks.
    const WRITE_PIPELINE_DEPTH: usize = 1024;

    /// Coalescing target size. Incoming chunks are accumulated until at
    /// least this many bytes are pending, then submitted as one writev.
    /// Matches ZFS recordsize (128 KiB) so each writev fills exactly
    /// one ZFS record instead of creating many small records.
    const COALESCE_TARGET: usize = 128 * 1024;

    /// Maximum time to wait for more chunks before submitting what we
    /// have. Prevents indefinite buffering when the sender is slow.
    const COALESCE_TIMEOUT: Duration = Duration::from_millis(100);

    if !is_io_uring_available().await {
        return write_file_from_channel_std(file, reader).await;
    }
    let system = tokio_epoll_uring::thread_local_system().await;
    let (permit, std_file) = file.into_inner();

    // Set FADV_SEQUENTIAL before the loop while we own the fd.
    {
        use std::os::unix::io::AsRawFd;
        let raw_fd = std_file.as_raw_fd();
        // Safety: raw_fd is valid, fadvise is best-effort.
        unsafe {
            libc::posix_fadvise(raw_fd, 0, 0, libc::POSIX_FADV_SEQUENTIAL);
        }
    }

    // Wrap fd in Arc so multiple in-flight writes can each hold a handle.
    // IoFd is implemented for Arc<T> where T: IoFd, so this works with
    // system.writev() which takes the fd by ownership.
    let fd_arc = Arc::new(std_file);
    let mut write_offset: u64 = 0;
    let mut completed_bytes: u64 = 0;
    let mut max_write_ms: u128 = 0;
    let mut slow_write_count: u32 = 0;
    let task_start = std::time::Instant::now();

    // Completion result carries the meta alongside the io_uring result
    // so FuturesUnordered can deliver completions in any order.
    struct WriteCompletion {
        chunk_len: usize,
        enqueue_time: std::time::Instant,
        submit_time: std::time::Instant,
        result: Result<usize, tokio_epoll_uring::Error<std::io::Error>>,
    }
    let mut in_flight: FuturesUnordered<
        std::pin::Pin<Box<dyn std::future::Future<Output = WriteCompletion> + Send>>,
    > = FuturesUnordered::new();

    #[inline]
    fn process_completion(
        wc: WriteCompletion,
        completed_bytes: &mut u64,
        max_write_ms: &mut u128,
        slow_write_count: &mut u32,
    ) -> Result<(), Error> {
        let n = match wc.result {
            Ok(n) => n,
            Err(e) => return Err(uring_err(e, "write_file_from_channel")),
        };

        if n < wc.chunk_len {
            return Err(make_err!(
                Code::Internal,
                "io_uring partial writev: {n}/{} bytes",
                wc.chunk_len,
            ));
        }

        let total_ms = wc.enqueue_time.elapsed().as_millis();
        let queue_ms = wc.submit_time.duration_since(wc.enqueue_time).as_millis();
        let io_ms = wc.submit_time.elapsed().as_millis();
        if total_ms > *max_write_ms {
            *max_write_ms = total_ms;
        }
        if total_ms > 100 {
            *slow_write_count += 1;
            warn!(
                total_ms,
                queue_ms,
                io_ms,
                chunk_len = wc.chunk_len,
                total_so_far = *completed_bytes,
                "write_file_from_channel: slow io_uring writev (>100ms)"
            );
        }
        *completed_bytes += wc.chunk_len as u64;
        Ok(())
    }

    loop {
        // Drain all ready completions without blocking, then start
        // coalescing the next batch. This keeps the pipeline moving —
        // completions are processed as soon as they arrive.
        loop {
            match in_flight.next().now_or_never() {
                Some(Some(wc)) => process_completion(
                    wc,
                    &mut completed_bytes,
                    &mut max_write_ms,
                    &mut slow_write_count,
                )?,
                _ => break,
            }
        }

        // If pipeline is full, block until at least one completes.
        if in_flight.len() >= WRITE_PIPELINE_DEPTH {
            let wc = in_flight
                .next()
                .await
                .ok_or_else(|| make_err!(Code::Internal, "pipeline unexpectedly empty"))?;
            process_completion(
                wc,
                &mut completed_bytes,
                &mut max_write_ms,
                &mut slow_write_count,
            )?;
        }

        // --- Coalescing phase: accumulate chunks into a batch ---
        let mut pending_chunks: Vec<Bytes> = Vec::new();
        let mut pending_bytes: usize = 0;
        let mut hit_eof = false;

        // Get at least one chunk (blocking recv).
        let first = reader
            .recv()
            .await
            .err_tip(|| "Failed to recv in write_file_from_channel")?;
        if first.is_empty() {
            break; // EOF
        }
        pending_bytes += first.len();
        pending_chunks.push(first);

        // Accumulate more chunks until we hit the target size or timeout.
        if pending_bytes < COALESCE_TARGET {
            let deadline = tokio::time::Instant::now() + COALESCE_TIMEOUT;
            loop {
                match tokio::time::timeout_at(deadline, reader.recv()).await {
                    Ok(Ok(chunk)) => {
                        if chunk.is_empty() {
                            hit_eof = true;
                            break;
                        }
                        pending_bytes += chunk.len();
                        pending_chunks.push(chunk);
                        if pending_bytes >= COALESCE_TARGET {
                            break;
                        }
                    }
                    Ok(Err(e)) => {
                        return Err(e)
                            .err_tip(|| "Failed to recv during coalescing in write_file_from_channel");
                    }
                    Err(_timeout) => break,
                }
            }
        }

        // --- Submit coalesced writev ---
        let total_len = pending_bytes;
        let offset = write_offset;
        write_offset += total_len as u64;

        // Build iovec array pointing into the Bytes buffers. The
        // iovecs and buffers are moved into WritevOp which keeps them
        // alive until the kernel CQE arrives.
        let iovecs: Vec<libc::iovec> = pending_chunks
            .iter()
            .map(|b| libc::iovec {
                iov_base: b.as_ptr() as *mut libc::c_void,
                iov_len: b.len(),
            })
            .collect();

        let enqueue_time = std::time::Instant::now();
        let write_fut = system.writev(
            Arc::clone(&fd_arc),
            offset,
            iovecs,
            pending_chunks,
        );

        in_flight.push(Box::pin(async move {
            let submit_time = std::time::Instant::now();
            let (_fd, result) = write_fut.await;
            WriteCompletion {
                chunk_len: total_len,
                enqueue_time,
                submit_time,
                result,
            }
        }));

        if hit_eof {
            break;
        }
    }

    // Drain all remaining in-flight writes.
    while let Some(wc) = in_flight.next().await {
        process_completion(
            wc,
            &mut completed_bytes,
            &mut max_write_ms,
            &mut slow_write_count,
        )?;
    }

    let task_total_ms = task_start.elapsed().as_millis();
    if task_total_ms > 100 {
        warn!(
            task_total_ms,
            total_bytes = completed_bytes,
            max_write_ms,
            slow_write_count,
            "write_file_from_channel: slow total write (>100ms)"
        );
    }

    // Extract the std::fs::File from the Arc. All in-flight writes
    // have completed and returned their Arc handles, so we should be
    // the sole owner.
    let std_file = Arc::try_unwrap(fd_arc).map_err(|arc| {
        make_err!(
            Code::Internal,
            "fd_arc has {} strong refs after all writes completed, expected 1",
            Arc::strong_count(&arc)
        )
    })?;

    Ok((completed_bytes, FileSlot::from_parts(permit, std_file)))
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
pub async fn write_file_from_channel(
    file: FileSlot,
    reader: &mut DropCloserReadHalf,
) -> Result<(u64, FileSlot), Error> {
    write_file_from_channel_std(file, reader).await
}

/// Write to `file` from a blocking thread, receiving chunks from `reader`.
/// Returns total bytes written and the `FileSlot`.
async fn write_file_from_channel_std(
    file: FileSlot,
    reader: &mut DropCloserReadHalf,
) -> Result<(u64, FileSlot), Error> {
    let (async_tx, mut sync_rx) = tokio::sync::mpsc::channel::<Bytes>(8);

    let write_task = spawn_blocking!("fs_write_file", move || {
        let mut f = file;
        f.advise_sequential();
        let mut total: u64 = 0;
        let mut max_write_ms: u128 = 0;
        let mut slow_write_count: u32 = 0;
        let task_start = std::time::Instant::now();
        while let Some(data) = sync_rx.blocking_recv() {
            let chunk_len = data.len();
            let write_start = std::time::Instant::now();
            f.as_std_mut()
                .write_all(&data)
                .map_err(|e| Into::<Error>::into(e))?;
            let write_ms = write_start.elapsed().as_millis();
            if write_ms > max_write_ms {
                max_write_ms = write_ms;
            }
            if write_ms > 100 {
                slow_write_count += 1;
                warn!(
                    write_ms,
                    chunk_len,
                    total_so_far = total,
                    "write_file_from_channel: slow write_all syscall (>100ms)"
                );
            }
            total += chunk_len as u64;
        }
        let task_total_ms = task_start.elapsed().as_millis();
        if task_total_ms > 100 {
            warn!(
                task_total_ms,
                total_bytes = total,
                max_write_ms,
                slow_write_count,
                "write_file_from_channel: slow total write (>100ms)"
            );
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

/// Write `data` to `file` at offset 0 in a single operation.
/// On io_uring: zero-copy pwrite (Bytes passed directly to kernel).
/// On fallback: spawn_blocking + write_all.
///
/// Falls back to spawn_blocking if io_uring is unavailable at runtime.
/// Synchronous pwrite threshold. For writes at or below this size, use a
/// direct `pwrite()` syscall on the async thread instead of io_uring or
/// spawn_blocking. For page-cache-backed filesystems this is a ~1μs memcpy.
const SYNC_PWRITE_THRESHOLD: usize = 16 * 1024; // 16 KiB

#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn write_all_to_file(file: FileSlot, data: Bytes) -> Result<FileSlot, Error> {
    if data.is_empty() {
        return Ok(file);
    }

    // Synchronous pwrite fast path for small data.
    // 16KB threshold matches pread to avoid cold-cache stalls on tokio workers.
    if data.len() <= SYNC_PWRITE_THRESHOLD {
        use std::os::unix::io::AsRawFd;
        let fd = file.as_std().as_raw_fd();
        let n = loop {
            let ret = unsafe {
                libc::pwrite(
                    fd,
                    data.as_ptr() as *const libc::c_void,
                    data.len(),
                    0,
                )
            };
            if ret >= 0 {
                break ret;
            }
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                continue; // retry on EINTR
            }
            return Err(make_err!(
                Code::Internal,
                "pwrite failed: {:?}",
                err
            ));
        };
        if (n as usize) < data.len() {
            return Err(make_err!(
                Code::Internal,
                "partial pwrite: {n}/{} bytes",
                data.len()
            ));
        }
        return Ok(file);
    }

    if !is_io_uring_available().await {
        return write_all_to_file_std(file, data).await;
    }
    let expected = data.len();
    let system = tokio_epoll_uring::thread_local_system().await;
    let (permit, std_file) = file.into_inner();
    let ((returned_fd, _), result) = system.write(std_file, 0, data).await;
    let n = result.map_err(|e| uring_err(e, "write_all_to_file"))?;
    if n < expected {
        return Err(make_err!(
            Code::Internal,
            "io_uring partial write in write_all_to_file: {n}/{expected} bytes"
        ));
    }
    Ok(FileSlot::from_parts(permit, returned_fd))
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
pub async fn write_all_to_file(file: FileSlot, data: Bytes) -> Result<FileSlot, Error> {
    if data.is_empty() {
        return Ok(file);
    }
    write_all_to_file_std(file, data).await
}

async fn write_all_to_file_std(mut file: FileSlot, data: Bytes) -> Result<FileSlot, Error> {
    file = spawn_blocking!("fs_write_all", move || {
        file.as_std_mut()
            .write_all(&data)
            .map_err(|e| Into::<Error>::into(e))?;
        Ok::<_, Error>(file)
    })
    .await
    .map_err(|e| make_err!(Code::Internal, "write_all join failed: {e:?}"))??;
    Ok(file)
}

/// Write data to file via mmap. Truncates the file to the data length,
/// maps it with `MAP_SHARED`, copies data into the mapping, and unmaps.
/// The kernel handles writeback of dirty pages asynchronously.
#[cfg(target_os = "linux")]
pub async fn write_all_to_file_mmap(file: FileSlot, data: Bytes) -> Result<FileSlot, Error> {
    if data.is_empty() {
        return Ok(file);
    }

    spawn_blocking!("fs_write_all_mmap", move || {
        use std::os::unix::io::AsRawFd;

        let fd = file.as_std().as_raw_fd();
        let size = data.len();

        let ret = unsafe { libc::ftruncate(fd, size as libc::off_t) };
        if ret != 0 {
            return Err(make_err!(
                Code::Internal,
                "ftruncate failed: {:?}",
                std::io::Error::last_os_error()
            ));
        }

        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            return Err(make_err!(
                Code::Internal,
                "mmap write failed: {:?}",
                std::io::Error::last_os_error()
            ));
        }

        unsafe {
            std::ptr::copy_nonoverlapping(data.as_ptr(), ptr as *mut u8, size);
            libc::munmap(ptr, size);
        }

        Ok(file)
    })
    .await
    .map_err(|e| make_err!(Code::Internal, "mmap write join failed: {e:?}"))?
}

/// Explicitly use the spawn_blocking write path, bypassing io_uring.
/// Exposed for benchmarking backend comparisons.
pub async fn write_all_to_file_blocking(file: FileSlot, data: Bytes) -> Result<FileSlot, Error> {
    if data.is_empty() {
        return Ok(file);
    }
    write_all_to_file_std(file, data).await
}

#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn hard_link(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
    if !is_io_uring_available().await {
        return hard_link_std(src, dst).await;
    }
    let system = tokio_epoll_uring::thread_local_system().await;
    system
        .link_at(src.as_ref(), dst.as_ref(), 0)
        .await
        .map_err(|e| uring_err(e, "hard_link"))
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
pub async fn hard_link(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
    hard_link_std(src, dst).await
}

async fn hard_link_std(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
    let src = src.as_ref().to_owned();
    let dst = dst.as_ref().to_owned();
    call_with_permit(move |_| std::fs::hard_link(src, dst).map_err(Into::<Error>::into)).await
}

/// Batch hard link: submit all linkat SQEs with a single `io_uring_enter` syscall.
/// Falls back to sequential `hard_link` calls if io_uring is unavailable.
#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn hard_link_batch(entries: &[(&Path, &Path)]) -> Vec<Result<(), Error>> {
    if entries.is_empty() {
        return Vec::new();
    }
    if !is_io_uring_available().await {
        let mut results = Vec::with_capacity(entries.len());
        for (src, dst) in entries {
            results.push(hard_link_std(src, dst).await);
        }
        return results;
    }
    let system = tokio_epoll_uring::thread_local_system().await;
    let batch: Vec<(&Path, &Path, i32)> = entries.iter().map(|(s, d)| (*s, *d, 0)).collect();
    system
        .link_at_batch(batch)
        .await
        .into_iter()
        .map(|r| r.map_err(|e| uring_err(e, "hard_link_batch")))
        .collect()
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
pub async fn hard_link_batch(entries: &[(&Path, &Path)]) -> Vec<Result<(), Error>> {
    let mut results = Vec::with_capacity(entries.len());
    for (src, dst) in entries {
        results.push(hard_link_std(src, dst).await);
    }
    results
}

pub async fn set_permissions(src: impl AsRef<Path>, perm: Permissions) -> Result<(), Error> {
    let src = src.as_ref().to_owned();
    call_with_permit(move |_| std::fs::set_permissions(src, perm).map_err(Into::<Error>::into))
        .await
}

/// Batch symlink: submit all symlinkat SQEs with a single `io_uring_enter` syscall.
/// Falls back to sequential `symlink` calls if io_uring is unavailable.
#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn symlink_batch(entries: &[(&Path, &Path)]) -> Vec<Result<(), Error>> {
    if entries.is_empty() {
        return Vec::new();
    }
    if !is_io_uring_available().await {
        let mut results = Vec::with_capacity(entries.len());
        for (target, linkpath) in entries {
            results.push(symlink_std(target, linkpath).await);
        }
        return results;
    }
    let system = tokio_epoll_uring::thread_local_system().await;
    let batch: Vec<(&Path, &Path)> = entries.iter().copied().collect();
    system
        .symlink_at_batch(batch)
        .await
        .into_iter()
        .map(|r| r.map_err(|e| uring_err(e, "symlink_batch")))
        .collect()
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
pub async fn symlink_batch(entries: &[(&Path, &Path)]) -> Vec<Result<(), Error>> {
    let mut results = Vec::with_capacity(entries.len());
    for (target, linkpath) in entries {
        results.push(symlink_std(target, linkpath).await);
    }
    results
}

#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn create_dir(path: impl AsRef<Path>) -> Result<(), Error> {
    if !is_io_uring_available().await {
        return create_dir_std(path).await;
    }
    let system = tokio_epoll_uring::thread_local_system().await;
    system
        .mkdir_at(path.as_ref(), 0o777)
        .await
        .map_err(|e| uring_err(e, "create_dir"))
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
pub async fn create_dir(path: impl AsRef<Path>) -> Result<(), Error> {
    create_dir_std(path).await
}

async fn create_dir_std(path: impl AsRef<Path>) -> Result<(), Error> {
    let path = path.as_ref().to_owned();
    call_with_permit(move |_| std::fs::create_dir(path).map_err(Into::<Error>::into)).await
}

pub async fn create_dir_all(path: impl AsRef<Path>) -> Result<(), Error> {
    let path = path.as_ref().to_owned();
    call_with_permit(move |_| std::fs::create_dir_all(path).map_err(Into::<Error>::into)).await
}

#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn symlink(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
    if !is_io_uring_available().await {
        return symlink_std(src, dst).await;
    }
    let system = tokio_epoll_uring::thread_local_system().await;
    system
        .symlink_at(src.as_ref(), dst.as_ref())
        .await
        .map_err(|e| uring_err(e, "symlink"))
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
pub async fn symlink(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
    symlink_std(src, dst).await
}

#[cfg(target_family = "unix")]
async fn symlink_std(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
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
    let permit = get_permit().await?;
    let inner = tokio::fs::read_dir(path)
        .await
        .map_err(Into::<Error>::into)?;
    Ok(ReadDir { permit, inner })
}

#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<(), Error> {
    if !is_io_uring_available().await {
        return rename_std(from, to).await;
    }
    let rename_start = std::time::Instant::now();
    let system = tokio_epoll_uring::thread_local_system().await;
    let result = system
        .rename_at(from.as_ref(), to.as_ref(), 0)
        .await
        .map_err(|e| uring_err(e, "rename"));
    let rename_ms = rename_start.elapsed().as_millis();
    if rename_ms > 100 {
        warn!(rename_ms, "fs::rename: slow io_uring rename (>100ms)");
    }
    result
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
pub async fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<(), Error> {
    rename_std(from, to).await
}

async fn rename_std(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<(), Error> {
    let from = from.as_ref().to_owned();
    let to = to.as_ref().to_owned();
    let rename_start = std::time::Instant::now();
    let result =
        call_with_permit(move |_| std::fs::rename(from, to).map_err(Into::<Error>::into)).await;
    let rename_ms = rename_start.elapsed().as_millis();
    if rename_ms > 100 {
        warn!(rename_ms, "fs::rename: slow rename syscall (>100ms)");
    }
    result
}

#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn remove_file(path: impl AsRef<Path>) -> Result<(), Error> {
    if !is_io_uring_available().await {
        return remove_file_std(path).await;
    }
    let system = tokio_epoll_uring::thread_local_system().await;
    system
        .unlink_at(path.as_ref(), 0)
        .await
        .map_err(|e| uring_err(e, "remove_file"))
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
pub async fn remove_file(path: impl AsRef<Path>) -> Result<(), Error> {
    remove_file_std(path).await
}

async fn remove_file_std(path: impl AsRef<Path>) -> Result<(), Error> {
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
