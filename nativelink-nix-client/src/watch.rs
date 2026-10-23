// Copyright 2026 The NativeLink Authors. All rights reserved.
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

//! Watches the local store for newly-committed paths.
//!
//! The primary mechanism is `fanotify` with a directory mark on the store and
//! `FAN_REPORT_DFID_NAME` (Linux ≥5.9): it catches the atomic `rename()` that
//! commits a store path (`FAN_MOVED_TO`) and directory creation (`FAN_CREATE`),
//! and reports the parent directory + entry name so the path resolves without
//! racing. It needs `CAP_SYS_ADMIN` (a non-issue for a root systemd unit). An
//! `inotify` fallback (`watch_store(.., true)`) covers unprivileged hosts.
//!
//! Both backends yield a [`StoreEvent`] only once the entry is a fully-committed
//! store path (`StorePath::from_full_path` accepts it) — never a temp build dir.

use std::ffi::CString;
use std::os::fd::{FromRawFd as _, OwnedFd, RawFd};
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use nativelink_error::{Code, Error, make_err};
use tokio::io::unix::AsyncFd;
use tokio::sync::mpsc;

use crate::store::StorePath;

/// A newly-observed store path.
#[derive(Clone, Debug)]
pub struct StoreEvent {
    /// The store path that appeared.
    pub path: StorePath,
}

/// The [`Stream`] of newly-committed store paths returned by [`watch_store`].
pub struct EventStream {
    rx: mpsc::Receiver<StoreEvent>,
}

impl core::fmt::Debug for EventStream {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EventStream").finish_non_exhaustive()
    }
}

impl Stream for EventStream {
    type Item = StoreEvent;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<StoreEvent>> {
        self.rx.poll_recv(cx)
    }
}

// --- fanotify ABI (stable kernel constants; not all are in every libc) -------
const FAN_CLOEXEC: u32 = 0x0000_0001;
const FAN_NONBLOCK: u32 = 0x0000_0002;
const FAN_CLASS_NOTIF: u32 = 0x0000_0000;
/// Do not cap the kernel notification queue at 16384 events (needs `CAP_SYS_ADMIN`,
/// already held). Without this a commit burst overflows the queue and the kernel
/// drops events, signalling `FAN_Q_OVERFLOW`.
const FAN_UNLIMITED_QUEUE: u32 = 0x0000_0010;
/// Set on a synthetic event's `mask` when the kernel dropped events on overflow.
const FAN_Q_OVERFLOW: u64 = 0x0000_4000;
const FAN_REPORT_DIR_FID: u32 = 0x0000_0400;
const FAN_REPORT_NAME: u32 = 0x0000_0800;
const FAN_REPORT_DFID_NAME: u32 = FAN_REPORT_DIR_FID | FAN_REPORT_NAME;
const FAN_MARK_ADD: u32 = 0x0000_0001;
const FAN_CREATE: u64 = 0x0000_0100;
const FAN_MOVED_TO: u64 = 0x0000_0080;
const FAN_ONDIR: u64 = 0x4000_0000;
const FAN_EVENT_INFO_TYPE_DFID_NAME: u8 = 2;
const FANOTIFY_METADATA_VERSION: u8 = 3;

#[repr(C)]
#[derive(Clone, Copy)]
struct FanotifyEventMetadata {
    event_len: u32,
    vers: u8,
    reserved: u8,
    metadata_len: u16,
    mask: u64,
    fd: i32,
    pid: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FanotifyEventInfoHeader {
    info_type: u8,
    pad: u8,
    len: u16,
}

const METADATA_SIZE: usize = size_of::<FanotifyEventMetadata>();
const INFO_HEADER_SIZE: usize = size_of::<FanotifyEventInfoHeader>();
/// `__kernel_fsid_t` (two `int`s) that precedes the `file_handle` in an info-fid record.
const FSID_SIZE: usize = 8;
/// The fixed prefix of `struct file_handle`: `handle_bytes: u32, handle_type: i32`.
const FILE_HANDLE_PREFIX: usize = 8;

/// Watches `store_dir`, yielding a [`StoreEvent`] for each newly-committed
/// top-level store path. When `use_inotify` is set (or fanotify is
/// unavailable/unprivileged) it uses the inotify backend.
pub fn watch_store(
    store_dir: &str,
    use_inotify: bool,
) -> Result<impl Stream<Item = StoreEvent> + Send + 'static, Error> {
    let backend = if use_inotify {
        Backend::inotify(store_dir)?
    } else {
        match Backend::fanotify(store_dir) {
            Ok(b) => b,
            Err(err) if err.code == Code::PermissionDenied => {
                tracing::warn!(
                    ?err,
                    "fanotify unavailable (needs CAP_SYS_ADMIN); falling back to inotify"
                );
                Backend::inotify(store_dir)?
            }
            Err(err) => return Err(err),
        }
    };

    let async_fd = AsyncFd::new(backend.fd)
        .map_err(|e| make_err!(Code::Internal, "registering watch fd with the runtime: {e}"))?;
    let store_dir = store_dir.trim_end_matches('/').to_string();
    let kind = backend.kind;
    let (tx, rx) = mpsc::channel::<StoreEvent>(4096);

    drop(nativelink_util::background_spawn!(
        "store_watch",
        async move {
            if let Err(err) = watch_loop(&async_fd, kind, &store_dir, &tx).await {
                tracing::error!(?err, "store watch loop stopped");
            }
        }
    ));
    Ok(EventStream { rx })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WatchKind {
    Fanotify,
    Inotify,
}

struct Backend {
    fd: OwnedFd,
    kind: WatchKind,
}

impl Backend {
    fn fanotify(store_dir: &str) -> Result<Self, Error> {
        // SAFETY: `fanotify_init` is a plain syscall wrapper; on success it
        // returns an owned fd, on failure -1 with `errno` set.
        let raw = unsafe {
            libc::fanotify_init(
                FAN_CLASS_NOTIF
                    | FAN_UNLIMITED_QUEUE
                    | FAN_REPORT_DFID_NAME
                    | FAN_CLOEXEC
                    | FAN_NONBLOCK,
                libc::O_RDONLY as u32,
            )
        };
        if raw < 0 {
            return Err(errno_err("fanotify_init"));
        }
        // SAFETY: `raw` is a fresh, owned fd from a successful `fanotify_init`.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let path = CString::new(store_dir)
            .map_err(|_| make_err!(Code::InvalidArgument, "store dir contains a NUL byte"))?;
        // SAFETY: `fd` is a valid fanotify fd; `path` is a valid C string. A
        // directory mark for create/moved-into events on the store directory.
        let rc = unsafe {
            libc::fanotify_mark(
                as_raw(&fd),
                FAN_MARK_ADD,
                FAN_CREATE | FAN_MOVED_TO | FAN_ONDIR,
                libc::AT_FDCWD,
                path.as_ptr(),
            )
        };
        if rc < 0 {
            return Err(errno_err("fanotify_mark"));
        }
        Ok(Self {
            fd,
            kind: WatchKind::Fanotify,
        })
    }

    fn inotify(store_dir: &str) -> Result<Self, Error> {
        // SAFETY: `inotify_init1` is a plain syscall wrapper.
        let raw = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if raw < 0 {
            return Err(errno_err("inotify_init1"));
        }
        // SAFETY: `raw` is a fresh, owned fd from a successful `inotify_init1`.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let path = CString::new(store_dir)
            .map_err(|_| make_err!(Code::InvalidArgument, "store dir contains a NUL byte"))?;
        // SAFETY: `fd` is a valid inotify fd; `path` is a valid C string.
        let rc = unsafe {
            libc::inotify_add_watch(
                as_raw(&fd),
                path.as_ptr(),
                libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_ONLYDIR,
            )
        };
        if rc < 0 {
            return Err(errno_err("inotify_add_watch"));
        }
        Ok(Self {
            fd,
            kind: WatchKind::Inotify,
        })
    }
}

async fn watch_loop(
    async_fd: &AsyncFd<OwnedFd>,
    kind: WatchKind,
    store_dir: &str,
    tx: &mpsc::Sender<StoreEvent>,
) -> Result<(), Error> {
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let mut guard = async_fd
            .readable()
            .await
            .map_err(|e| make_err!(Code::Internal, "awaiting watch fd: {e}"))?;
        // SAFETY: `buf` is a valid, sized buffer; `read` fills at most its len.
        let n = unsafe {
            libc::read(
                as_raw(async_fd.get_ref()),
                buf.as_mut_ptr().cast(),
                buf.len(),
            )
        };
        if n < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::WouldBlock {
                guard.clear_ready();
                continue;
            }
            return Err(make_err!(Code::Internal, "reading watch events: {err}"));
        }
        if n == 0 {
            return Ok(());
        }
        let filled = &buf[..n as usize];
        match kind {
            WatchKind::Fanotify => parse_fanotify(filled, store_dir, tx).await,
            WatchKind::Inotify => parse_inotify(filled, store_dir, tx).await,
        }
    }
}

/// Emits `store_dir/name` as a `StoreEvent` if it is a valid, committed store
/// path (silently ignores temp dirs, lock files, and `.links`).
async fn emit(store_dir: &str, name: &[u8], tx: &mpsc::Sender<StoreEvent>) {
    let Ok(name) = std::str::from_utf8(name) else {
        return;
    };
    let full = format!("{store_dir}/{name}");
    if let Ok(path) = StorePath::from_full_path(store_dir, &full) {
        // Backpressure, never drop: `send().await` waits for channel capacity
        // when the consumer is behind. The previous `try_send` dropped events
        // silently on a full channel (not just a closed one), which is how
        // large paths committed at the tail of a build burst went missing from
        // the mirror. An `Err` here means the receiver is closed — the daemon
        // is shutting down.
        if tx.send(StoreEvent { path }).await.is_err() {
            tracing::debug!("event receiver closed; store watch shutting down");
        }
    }
}

/// Parses a buffer of concatenated `fanotify` events, extracting the entry name
/// from each `DFID_NAME` info record.
async fn parse_fanotify(mut buf: &[u8], store_dir: &str, tx: &mpsc::Sender<StoreEvent>) {
    while buf.len() >= METADATA_SIZE {
        // SAFETY: `buf` has at least `METADATA_SIZE` bytes; the struct is `repr(C)`
        // and `Copy`, so a byte-read of the header is sound.
        let meta =
            unsafe { std::ptr::read_unaligned(buf.as_ptr().cast::<FanotifyEventMetadata>()) };
        let event_len = meta.event_len as usize;
        if event_len < METADATA_SIZE
            || event_len > buf.len()
            || meta.vers != FANOTIFY_METADATA_VERSION
        {
            return; // malformed / version mismatch — stop rather than misparse.
        }
        // A queue overflow is a synthetic event (no fd, no info record): the
        // kernel dropped store-commit events. With FAN_UNLIMITED_QUEUE this
        // should not happen, but if it does, say so loudly — the mirror is now
        // incomplete and needs a reconciliation pass, not a silent gap.
        if meta.mask & FAN_Q_OVERFLOW != 0 {
            tracing::warn!(
                "fanotify queue overflow: the kernel dropped store-commit events; \
                 mirrored cache is incomplete until a reconciliation sweep runs"
            );
            buf = &buf[event_len..];
            continue;
        }
        if let Some(name) = fanotify_name(&buf[METADATA_SIZE..event_len]) {
            emit(store_dir, name, tx).await;
        }
        buf = &buf[event_len..];
    }
}

/// Walks the info records after the metadata to find the `DFID_NAME` entry
/// name (the bytes after the `file_handle`).
fn fanotify_name(mut info: &[u8]) -> Option<&[u8]> {
    while info.len() >= INFO_HEADER_SIZE {
        // SAFETY: at least `INFO_HEADER_SIZE` bytes remain; `repr(C)` `Copy` header.
        let hdr =
            unsafe { std::ptr::read_unaligned(info.as_ptr().cast::<FanotifyEventInfoHeader>()) };
        let len = hdr.len as usize;
        if len < INFO_HEADER_SIZE || len > info.len() {
            return None;
        }
        if hdr.info_type == FAN_EVENT_INFO_TYPE_DFID_NAME {
            let body = &info[INFO_HEADER_SIZE..len];
            // body = fsid(8) + file_handle{ handle_bytes:u32, handle_type:i32, f_handle[handle_bytes] } + name\0
            if body.len() >= FSID_SIZE + FILE_HANDLE_PREFIX {
                let hb =
                    u32::from_ne_bytes(body[FSID_SIZE..FSID_SIZE + 4].try_into().ok()?) as usize;
                let name_start = FSID_SIZE + FILE_HANDLE_PREFIX + hb;
                if name_start <= body.len() {
                    let name = &body[name_start..];
                    // Trim trailing NUL padding.
                    let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
                    return Some(&name[..end]);
                }
            }
            return None;
        }
        info = &info[len..];
    }
    None
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InotifyEvent {
    wd: i32,
    mask: u32,
    cookie: u32,
    len: u32,
}

const INOTIFY_EVENT_SIZE: usize = size_of::<InotifyEvent>();

/// Parses a buffer of concatenated `inotify` events.
async fn parse_inotify(mut buf: &[u8], store_dir: &str, tx: &mpsc::Sender<StoreEvent>) {
    while buf.len() >= INOTIFY_EVENT_SIZE {
        // SAFETY: at least `INOTIFY_EVENT_SIZE` bytes remain; `repr(C)` `Copy`.
        let ev = unsafe { std::ptr::read_unaligned(buf.as_ptr().cast::<InotifyEvent>()) };
        let name_len = ev.len as usize;
        let total = INOTIFY_EVENT_SIZE + name_len;
        if total > buf.len() {
            return;
        }
        if name_len > 0 {
            let raw = &buf[INOTIFY_EVENT_SIZE..total];
            let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
            emit(store_dir, &raw[..end], tx).await;
        }
        buf = &buf[total..];
    }
}

fn as_raw<F: std::os::fd::AsRawFd>(fd: &F) -> RawFd {
    fd.as_raw_fd()
}

fn errno_err(what: &str) -> Error {
    let err = std::io::Error::last_os_error();
    let code = if err.kind() == std::io::ErrorKind::PermissionDenied {
        Code::PermissionDenied
    } else {
        Code::Internal
    };
    make_err!(code, "{what}: {err}")
}
