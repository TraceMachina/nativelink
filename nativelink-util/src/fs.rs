// Copyright 2023 The Native Link Authors. All rights reserved.
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

use std::borrow::Cow;
use std::ffi::OsStr;
use std::fs::Metadata;
use std::io::IoSlice;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::BytesMut;
use futures::Future;
use nativelink_error::{make_err, Code, Error, ResultExt};
/// We wrap all tokio::fs items in our own wrapper so we can limit the number of outstanding
/// open files at any given time. This will greatly reduce the chance we'll hit open file limit
/// issues.
pub use tokio::fs::DirEntry;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWrite, ReadBuf, SeekFrom, Take};
use tokio::sync::{Semaphore, SemaphorePermit};
use tokio::time::timeout;
use tracing::error;

type StreamPosition = u64;
type BytesRemaining = u64;

#[derive(Debug)]
enum MaybeFileSlot {
    Open(Take<FileSlot>),
    Closed((StreamPosition, BytesRemaining)),
}

/// A wrapper around a generic FileSlot. This gives us the ability to
/// close a file and then resume it later. Specifically useful for cases
/// piping data from one location to another and one side is slow at
/// reading or writing the data, we can have a timeout, close the file
/// and then reopen it later.
///
/// Note: This wraps both files opened for read and write, so we always
/// need to know how the original file was opened and the location of
/// the file. To simplify the code significantly we always require the
/// file to be a `Take<FileSlot>`.
#[derive(Debug)]
pub struct ResumeableFileSlot<'a> {
    maybe_file_slot: MaybeFileSlot,
    path: Cow<'a, OsStr>,
    is_write: bool,
}

impl<'a> ResumeableFileSlot<'a> {
    pub fn new(file: FileSlot, path: impl Into<Cow<'a, OsStr>>, is_write: bool) -> Self {
        Self {
            maybe_file_slot: MaybeFileSlot::Open(file.take(u64::MAX)),
            path: path.into(),
            is_write,
        }
    }

    pub fn new_with_take(file: Take<FileSlot>, path: impl Into<Cow<'a, OsStr>>, is_write: bool) -> Self {
        Self {
            maybe_file_slot: MaybeFileSlot::Open(file),
            path: path.into(),
            is_write,
        }
    }

    pub async fn close_file(&mut self) -> Result<(), Error> {
        let MaybeFileSlot::Open(file_slot) = &mut self.maybe_file_slot else {
            return Ok(());
        };
        let position = file_slot
            .get_mut()
            .inner
            .stream_position()
            .await
            .err_tip(|| format!("Failed to get file position {:?}", self.path))?;
        self.maybe_file_slot = MaybeFileSlot::Closed((position, file_slot.limit()));
        Ok(())
    }

    #[inline]
    pub async fn as_reader(&mut self) -> Result<&mut Take<FileSlot>, Error> {
        let (stream_position, bytes_remaining) = match self.maybe_file_slot {
            MaybeFileSlot::Open(ref mut file_slot) => return Ok(file_slot),
            MaybeFileSlot::Closed(pos) => pos,
        };
        let permit = OPEN_FILE_SEMAPHORE
            .acquire()
            .await
            .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
        let inner = tokio::fs::OpenOptions::new()
            .write(self.is_write)
            .read(!self.is_write)
            .open(&self.path)
            .await
            .err_tip(|| format!("Could not open after resume {:?}", self.path))?;
        let mut file_slot = FileSlot { _permit: permit, inner };
        file_slot
            .inner
            .seek(SeekFrom::Start(stream_position))
            .await
            .err_tip(|| format!("Failed to seek to position {:?} {:?}", stream_position, self.path))?;

        self.maybe_file_slot = MaybeFileSlot::Open(file_slot.take(bytes_remaining));
        match &mut self.maybe_file_slot {
            MaybeFileSlot::Open(file_slot) => Ok(file_slot),
            MaybeFileSlot::Closed(_) => unreachable!(),
        }
    }

    #[inline]
    pub async fn as_writer(&mut self) -> Result<&mut FileSlot, Error> {
        Ok(self.as_reader().await?.get_mut())
    }

    /// Utility function to read data from a handler and handles file descriptor
    /// timeouts. Chunk size is based on the `buf`'s capacity.
    /// Note: If the `handler` changes `buf`s capcity, it is responsible for reserving
    /// more before returning.
    pub async fn read_buf_cb<'b, T, F, Fut>(
        &'b mut self,
        (mut buf, mut state): (BytesMut, T),
        mut handler: F,
    ) -> Result<(BytesMut, T), Error>
    where
        F: (FnMut((BytesMut, T)) -> Fut) + 'b,
        Fut: Future<Output = Result<(BytesMut, T), Error>> + 'b,
    {
        loop {
            buf.clear();
            self.as_reader()
                .await
                .err_tip(|| "Could not get reader from file slot in read_buf_cb")?
                .read_buf(&mut buf)
                .await
                .err_tip(|| "Could not read chunk during read_buf_cb")?;
            if buf.is_empty() {
                return Ok((buf, state));
            }
            let handler_fut = handler((buf, state));
            tokio::pin!(handler_fut);
            loop {
                match timeout(idle_file_descriptor_timeout(), &mut handler_fut).await {
                    Ok(Ok(output)) => {
                        (buf, state) = output;
                        break;
                    }
                    Ok(Err(err)) => return Err(err).err_tip(|| "read_buf_cb's handler returned an error"),
                    Err(_) => {
                        self.close_file()
                            .await
                            .err_tip(|| "Could not close file due to timeout in read_buf_cb")?;
                        continue;
                    }
                }
            }
        }
    }
}

#[derive(Debug)]
pub struct FileSlot {
    // We hold the permit because once it is dropped it goes back into the queue.
    _permit: SemaphorePermit<'static>,
    inner: tokio::fs::File,
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

    fn poll_complete(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<u64, tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_complete(cx)
    }
}

impl AsyncWrite for FileSlot {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<Result<usize, tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), tokio::io::Error>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), tokio::io::Error>> {
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

const DEFAULT_OPEN_FILE_PERMITS: usize = 10;
static TOTAL_FILE_SEMAPHORES: AtomicUsize = AtomicUsize::new(DEFAULT_OPEN_FILE_PERMITS);
pub static OPEN_FILE_SEMAPHORE: Semaphore = Semaphore::const_new(DEFAULT_OPEN_FILE_PERMITS);

/// Acquire a permit from the open file semaphore and call a raw function.
#[inline]
pub async fn call_with_permit<R>(func: impl FnOnce() -> Result<R, Error>) -> Result<R, Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    (func)()
}

pub fn set_open_file_limit(limit: usize) {
    let current_total = TOTAL_FILE_SEMAPHORES.load(Ordering::Acquire);
    if limit < current_total {
        error!("set_open_file_limit({}) must be greater than {}", limit, current_total);
        return;
    }
    TOTAL_FILE_SEMAPHORES.fetch_add(limit - current_total, Ordering::Release);
    OPEN_FILE_SEMAPHORE.add_permits(limit - current_total);
}

pub fn get_open_files_for_test() -> usize {
    TOTAL_FILE_SEMAPHORES.load(Ordering::Acquire) - OPEN_FILE_SEMAPHORE.available_permits()
}

/// How long a file descriptor can be open without being used before it is closed.
static IDLE_FILE_DESCRIPTOR_TIMEOUT: OnceLock<Duration> = OnceLock::new();

pub fn idle_file_descriptor_timeout() -> Duration {
    *IDLE_FILE_DESCRIPTOR_TIMEOUT.get_or_init(|| Duration::MAX)
}

/// Set the idle file descriptor timeout. This is the amount of time
/// a file descriptor can be open without being used before it is closed.
pub fn set_idle_file_descriptor_timeout(timeout: Duration) -> Result<(), Error> {
    IDLE_FILE_DESCRIPTOR_TIMEOUT
        .set(timeout)
        .map_err(|_| make_err!(Code::Internal, "idle_file_descriptor_timeout already set"))
}

pub async fn open_file<'a>(path: impl Into<Cow<'a, OsStr>>, limit: u64) -> Result<ResumeableFileSlot<'a>, Error> {
    let permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    let path = path.into();
    Ok(ResumeableFileSlot::new_with_take(
        FileSlot {
            _permit: permit,
            inner: tokio::fs::File::open(&path)
                .await
                .err_tip(|| format!("Could not open {:?}", path))?,
        }
        .take(limit),
        path,
        false, /* is_write */
    ))
}

pub async fn create_file<'a>(path: impl Into<Cow<'a, OsStr>>) -> Result<ResumeableFileSlot<'a>, Error> {
    let permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    let path = path.into();
    Ok(ResumeableFileSlot::new(
        FileSlot {
            _permit: permit,
            inner: tokio::fs::File::create(&path)
                .await
                .err_tip(|| format!("Could not open {:?}", path))?,
        },
        path,
        true, /* is_write */
    ))
}

pub async fn hard_link(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    tokio::fs::hard_link(src, dst).await.map_err(|e| e.into())
}

pub async fn set_permissions(src: impl AsRef<Path>, perm: std::fs::Permissions) -> Result<(), Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    tokio::fs::set_permissions(src, perm).await.map_err(|e| e.into())
}

pub async fn create_dir(path: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    tokio::fs::create_dir(path).await.map_err(|e| e.into())
}

pub async fn create_dir_all(path: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    tokio::fs::create_dir_all(path).await.map_err(|e| e.into())
}

#[cfg(target_family = "unix")]
pub async fn symlink(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    tokio::fs::symlink(src, dst).await.map_err(|e| e.into())
}

pub async fn read_link(path: impl AsRef<Path>) -> Result<std::path::PathBuf, Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    tokio::fs::read_link(path).await.map_err(|e| e.into())
}

pub struct ReadDir<'a> {
    // We hold the permit because once it is dropped it goes back into the queue.
    permit: SemaphorePermit<'a>,
    inner: tokio::fs::ReadDir,
}

impl<'a> ReadDir<'a> {
    pub fn into_inner(self) -> (SemaphorePermit<'a>, tokio::fs::ReadDir) {
        (self.permit, self.inner)
    }
}

impl<'a> AsRef<tokio::fs::ReadDir> for ReadDir<'a> {
    fn as_ref(&self) -> &tokio::fs::ReadDir {
        &self.inner
    }
}

impl<'a> AsMut<tokio::fs::ReadDir> for ReadDir<'a> {
    fn as_mut(&mut self) -> &mut tokio::fs::ReadDir {
        &mut self.inner
    }
}

pub async fn read_dir(path: impl AsRef<Path>) -> Result<ReadDir<'static>, Error> {
    let permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    Ok(ReadDir {
        permit,
        inner: tokio::fs::read_dir(path).await.map_err(Into::<Error>::into)?,
    })
}

pub async fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    tokio::fs::rename(from, to).await.map_err(|e| e.into())
}

pub async fn remove_file(path: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    tokio::fs::remove_file(path).await.map_err(|e| e.into())
}

pub async fn canonicalize(path: impl AsRef<Path>) -> Result<PathBuf, Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    tokio::fs::canonicalize(path).await.map_err(|e| e.into())
}

pub async fn metadata(path: impl AsRef<Path>) -> Result<Metadata, Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    tokio::fs::metadata(path).await.map_err(|e| e.into())
}

pub async fn read(path: impl AsRef<Path>) -> Result<Vec<u8>, Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    tokio::fs::read(path).await.map_err(|e| e.into())
}

pub async fn symlink_metadata(path: impl AsRef<Path>) -> Result<Metadata, Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    tokio::fs::symlink_metadata(path).await.map_err(|e| e.into())
}

pub async fn remove_dir_all(path: impl AsRef<Path>) -> Result<(), Error> {
    let _permit = OPEN_FILE_SEMAPHORE
        .acquire()
        .await
        .map_err(|e| make_err!(Code::Internal, "Open file semaphore closed {:?}", e))?;
    tokio::fs::remove_dir_all(path).await.map_err(|e| e.into())
}
