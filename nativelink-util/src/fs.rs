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
/// **Important**: the io_uring path ignores `start` because `read_file_to_channel`
/// uses pread with explicit offsets. Callers MUST pass the same offset to
/// `read_file_to_channel`'s `start_offset` parameter. Do NOT use the returned
/// `FileSlot` for direct sequential reads at a non-zero offset — use pread or
/// the spawn_blocking fallback instead.
///
/// Falls back to spawn_blocking (with seek) if io_uring is unavailable.
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

/// Read from `file` via io_uring pread, sending chunks to `writer`.
/// Eliminates the spawn_blocking thread pool and mpsc channel bridge —
/// reads are submitted directly to the kernel via io_uring and awaited
/// on the current tokio task.
///
/// Uses double-buffering to overlap disk I/O with network transmission:
/// while one chunk is being sent to the writer channel, the next read
/// is already submitted to io_uring. Buffers are reused across iterations
/// to avoid per-read allocation and zeroing overhead.
///
/// Falls back to spawn_blocking if io_uring is unavailable at runtime.
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
    let system = tokio_epoll_uring::thread_local_system().await;
    let (permit, std_file) = file.into_inner();

    use std::os::unix::io::AsRawFd;
    let raw_fd = std_file.as_raw_fd();

    // Advise the kernel we will read sequentially — enables aggressive
    // readahead (typically 2-4x default window).
    unsafe {
        // len=0 means "to end of file" per POSIX, which is correct when
        // limit is u64::MAX (casting u64::MAX to i64 would produce -1).
        let fadvise_len = if limit == u64::MAX { 0 } else { limit as i64 };
        libc::posix_fadvise(raw_fd, start_offset as i64, fadvise_len, libc::POSIX_FADV_SEQUENTIAL);
    }

    let mut remaining = limit;
    let mut current_offset = start_offset;
    let mut fd = std_file;

    // --- First read (priming the pipeline) ---
    let first_to_read = read_buffer_size.min(remaining as usize);
    if first_to_read == 0 {
        return Ok(FileSlot::from_parts(permit, fd));
    }

    let read_start = std::time::Instant::now();
    // Safety: IoBufMut for Vec<u8> uses capacity as the writable region.
    // The kernel fills bytes via pread; set_init(n) is called on completion.
    // No need to zero-initialize — the kernel overwrites the buffer.
    let ((returned_fd, returned_buf), result) =
        system.read(fd, current_offset, Vec::with_capacity(first_to_read)).await;
    fd = returned_fd;

    let n = match result {
        Ok(0) => return Ok(FileSlot::from_parts(permit, fd)),
        Ok(n) => n,
        Err(e) => return Err(uring_err(e, "read_file_to_channel")),
    };

    let read_ms = read_start.elapsed().as_millis();
    if read_ms > 100 {
        warn!(
            read_ms,
            bytes_read = n,
            current_offset,
            "read_file_to_channel: slow io_uring read (>100ms)"
        );
    }

    // Zero-copy: Vec heap transfers directly to Bytes.
    let mut vec_buf = returned_buf;
    vec_buf.truncate(n);
    let mut pending_chunk = Bytes::from(vec_buf);
    current_offset += n as u64;
    remaining = remaining.saturating_sub(n as u64);

    // --- Steady-state loop: overlap channel send with next io_uring read ---
    // While the previous chunk travels over the network, the next chunk
    // is being read from disk via io_uring. This hides disk latency
    // behind network transmission.
    loop {
        let to_read = read_buffer_size.min(remaining as usize);
        if to_read == 0 {
            // No more data to read — just send the last pending chunk.
            writer
                .send(pending_chunk)
                .await
                .err_tip(|| "failed to send final chunk from file reader")?;
            break;
        }

        // Submit next read and send previous chunk concurrently.
        // Each iteration allocates a fresh Vec for the read buffer.
        // Bytes::from(vec) transfers ownership zero-copy, so the Vec
        // can't be reused — but mimalloc's thread-local free lists
        // recycle the same pages, making this effectively free.
        // No zero-init: kernel overwrites via pread, IoBufMut uses capacity.
        let read_fut = system.read(fd, current_offset, Vec::with_capacity(to_read));
        let send_fut = writer.send(pending_chunk);

        let (send_result, ((returned_fd, returned_buf), read_result)) =
            tokio::join!(send_fut, read_fut);

        send_result.err_tip(|| "failed to send chunk from file reader")?;

        fd = returned_fd;

        let n = match read_result {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => return Err(uring_err(e, "read_file_to_channel")),
        };

        // Zero-copy: transfer Vec heap to Bytes.
        let mut vec_buf = returned_buf;
        vec_buf.truncate(n);
        pending_chunk = Bytes::from(vec_buf);
        current_offset += n as u64;
        remaining = remaining.saturating_sub(n as u64);
    }

    Ok(FileSlot::from_parts(permit, fd))
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

/// Write to `file` via io_uring pwrite, receiving chunks from `reader`.
/// Eliminates the spawn_blocking thread pool and mpsc channel bridge —
/// writes are submitted directly to the kernel via io_uring. `Bytes`
/// buffers are passed by ownership (zero-copy to kernel).
///
/// Falls back to spawn_blocking if io_uring is unavailable at runtime.
#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn write_file_from_channel(
    file: FileSlot,
    reader: &mut DropCloserReadHalf,
) -> Result<(u64, FileSlot), Error> {
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

    let mut fd = std_file;
    let mut total: u64 = 0;
    let mut max_write_ms: u128 = 0;
    let mut slow_write_count: u32 = 0;
    let task_start = std::time::Instant::now();

    loop {
        let data = reader
            .recv()
            .await
            .err_tip(|| "Failed to recv in write_file_from_channel")?;
        if data.is_empty() {
            break; // EOF
        }
        let chunk_len = data.len();
        let write_start = std::time::Instant::now();

        // Pass Bytes directly — avoids the spawn_blocking + mpsc copy.
        // The kernel reads from the Bytes heap pointer.
        let ((returned_fd, _), result) = system.write(fd, total, data).await;
        fd = returned_fd;

        let n = match result {
            Ok(n) => n,
            Err(e) => return Err(uring_err(e, "write_file_from_channel")),
        };

        // For regular files, pwrite writes the full amount unless the
        // disk is full. Handle partial writes defensively.
        if n < chunk_len {
            return Err(make_err!(
                Code::Internal,
                "io_uring partial write: {n}/{chunk_len} bytes at offset {total}"
            ));
        }

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
                "write_file_from_channel: slow io_uring write (>100ms)"
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

    Ok((total, FileSlot::from_parts(permit, fd)))
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
    let (async_tx, mut sync_rx) = tokio::sync::mpsc::channel::<Bytes>(4);

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
#[cfg(all(feature = "io-uring", target_os = "linux"))]
pub async fn write_all_to_file(file: FileSlot, data: Bytes) -> Result<FileSlot, Error> {
    if data.is_empty() {
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
