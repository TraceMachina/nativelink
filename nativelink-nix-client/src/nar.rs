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

//! Pure-Rust NAR (Nix ARchive) serialization and restoration.
//!
//! [`dump_path`] streams the exact bytes `nix-store --dump PATH` produces —
//! `str("nix-archive-1")` followed by a recursively-serialized node, where each
//! `str(s)` is a little-endian `u64` length followed by `s` zero-padded to an
//! 8-byte boundary — without buffering the whole archive. [`restore_path`] is
//! the inverse, for pull/import. [`HashingNar`] tees a NAR byte stream through
//! a SHA-256 hasher and a byte counter so an uploader learns the `NarHash` and
//! `NarSize` as the bytes flow past.
//!
//! The grammar (matching Nix's `libutil`/`libstore` serialisers byte-for-byte):
//!
//! ```text
//! nar     = str("nix-archive-1") node
//! node    = str("(") str("type") (
//!             str("regular")   [ str("executable") str("") ] str("contents") bytes(file)
//!           | str("symlink")   str("target") str(linktarget)
//!           | str("directory") entry* )
//!           str(")")
//! entry   = str("entry") str("(") str("name") str(name) str("node") node str(")")
//! ```
//!
//! Directory entries are emitted sorted by name as raw bytes (ascending). The
//! `executable`/`""` pair is emitted only when the file's owner-execute bit is
//! set.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::task::{Context, Poll};

use bytes::{Bytes, BytesMut};
use futures::Stream;
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_util::buf_channel::{DropCloserWriteHalf, make_buf_channel_pair};
use nativelink_util::spawn;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

/// The NAR format magic string, the first token of every archive.
const NAR_MAGIC: &str = "nix-archive-1";

/// Chunk size for streaming regular-file contents so a multi-GB file never
/// sits fully in memory.
const FILE_CHUNK_SIZE: usize = 64 * 1024;

/// Upper bound on any length-prefixed token accepted by [`restore_path`] that
/// is *not* raw file contents (type tags, entry names, symlink targets). Nix
/// itself caps these; the bound keeps a hostile `u64` length from triggering a
/// huge allocation. File contents are streamed in [`FILE_CHUNK_SIZE`] pieces
/// and are not subject to this cap.
const MAX_TOKEN_LEN: u64 = 64 * 1024;

/// Streams the NAR serialization of the filesystem tree at `path`, chunk by
/// chunk, without holding the whole archive in memory. The byte sequence is
/// identical to `nix-store --dump path`, so its SHA-256 is the store path's
/// `NarHash`.
///
/// A producer task walks the tree and writes framed chunks into a
/// [`make_buf_channel_pair`] write half; the returned stream is the read half,
/// adapted so its error type is [`Error`]. Regular-file contents are read in
/// [`FILE_CHUNK_SIZE`] pieces.
pub fn dump_path(path: PathBuf) -> impl Stream<Item = Result<Bytes, Error>> + Send + 'static {
    let (writer, reader) = make_buf_channel_pair();
    let _producer = spawn!("nar_dump_path", async move {
        let mut nar = NarWriter { writer };
        let result = async {
            nar.emit_str(NAR_MAGIC.as_bytes()).await?;
            nar.dump_node(&path).await?;
            nar.writer
                .send_eof()
                .err_tip(|| "In dump_path: sending NAR EOF")
        }
        .await;
        if let Err(err) = result {
            // Surface the failure to the reader instead of a silent truncation:
            // dropping the write half *without* an EOF makes `recv` yield an
            // "Sender dropped before sending EOF" error, which the adapter maps
            // through. We log for observability.
            tracing::warn!(?err, "NAR dump_path producer failed");
        }
    });

    NarByteStream { reader, _producer }
}

/// Adapts a [`nativelink_util::buf_channel`] read half (whose `Stream` yields
/// `Result<Bytes, std::io::Error>`) into the `Result<Bytes, Error>` stream the
/// callers require, and keeps the producer task alive for the stream's lifetime.
struct NarByteStream {
    reader: nativelink_util::buf_channel::DropCloserReadHalf,
    // Held so the producer is aborted if the consumer drops the stream early.
    _producer: nativelink_util::task::JoinHandleDropGuard<()>,
}

impl core::fmt::Debug for NarByteStream {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NarByteStream").finish_non_exhaustive()
    }
}

impl Stream for NarByteStream {
    type Item = Result<Bytes, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.reader)
            .poll_next(cx)
            .map(|opt| opt.map(|res| res.map_err(Error::from)))
    }
}

/// Writes NAR tokens into a [`DropCloserWriteHalf`].
struct NarWriter {
    writer: DropCloserWriteHalf,
}

impl core::fmt::Debug for NarWriter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NarWriter").finish_non_exhaustive()
    }
}

impl NarWriter {
    /// Emits `str(s)`: an LE `u64` length, the bytes of `s`, then zero-padding
    /// to the next 8-byte boundary.
    async fn emit_str(&mut self, s: &[u8]) -> Result<(), Error> {
        let len = u64::try_from(s.len()).err_tip(|| "NAR token length overflows u64")?;
        // Length prefix + payload + padding, in as few sends as fits.
        let pad = padding(s.len());
        let mut header = BytesMut::with_capacity(8);
        header.extend_from_slice(&len.to_le_bytes());
        self.send(header.freeze()).await?;
        if !s.is_empty() {
            self.send(Bytes::copy_from_slice(s)).await?;
        }
        if pad != 0 {
            self.send(Bytes::copy_from_slice(&ZERO_PAD[..pad])).await?;
        }
        Ok(())
    }

    /// Emits `bytes(contents)` where `contents` is read from `file` in chunks:
    /// the LE `u64` length, then the raw bytes streamed [`FILE_CHUNK_SIZE`] at a
    /// time, then padding to the next 8-byte boundary.
    async fn emit_file_contents(
        &mut self,
        file: &mut tokio::fs::File,
        len: u64,
    ) -> Result<(), Error> {
        let mut header = BytesMut::with_capacity(8);
        header.extend_from_slice(&len.to_le_bytes());
        self.send(header.freeze()).await?;

        let mut remaining = len;
        let mut buf = vec![0u8; FILE_CHUNK_SIZE];
        while remaining > 0 {
            let want = usize::try_from(remaining.min(FILE_CHUNK_SIZE as u64))
                .err_tip(|| "NAR file chunk length overflow")?;
            let n = file
                .read(&mut buf[..want])
                .await
                .err_tip(|| "In dump_path: reading file contents")?;
            if n == 0 {
                // The file was truncated under us relative to its stat size.
                return Err(make_err!(
                    Code::Internal,
                    "file shrank while being dumped to NAR (short read)"
                ));
            }
            let n64 = u64::try_from(n).err_tip(|| "read length overflows u64")?;
            self.send(Bytes::copy_from_slice(&buf[..n])).await?;
            remaining -= n64;
        }

        let pad = padding_u64(len);
        if pad != 0 {
            self.send(Bytes::copy_from_slice(&ZERO_PAD[..pad])).await?;
        }
        Ok(())
    }

    /// Serializes the node at `path` (a regular file, symlink, or directory).
    ///
    /// Async recursion is boxed because the directory case re-enters `dump_node`.
    fn dump_node<'a>(
        &'a mut self,
        path: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            let meta = tokio::fs::symlink_metadata(path)
                .await
                .err_tip(|| format!("In dump_path: stat of {}", path.display()))?;
            let file_type = meta.file_type();

            self.emit_str(b"(").await?;
            self.emit_str(b"type").await?;

            if file_type.is_file() {
                self.emit_str(b"regular").await?;
                // Owner-execute bit → emit `executable` `""`.
                if meta.permissions().mode() & 0o100 != 0 {
                    self.emit_str(b"executable").await?;
                    self.emit_str(b"").await?;
                }
                self.emit_str(b"contents").await?;
                let mut file = tokio::fs::File::open(path)
                    .await
                    .err_tip(|| format!("In dump_path: open {}", path.display()))?;
                self.emit_file_contents(&mut file, meta.len()).await?;
            } else if file_type.is_symlink() {
                let target = tokio::fs::read_link(path)
                    .await
                    .err_tip(|| format!("In dump_path: readlink {}", path.display()))?;
                self.emit_str(b"symlink").await?;
                self.emit_str(b"target").await?;
                self.emit_str(target.as_os_str().as_bytes()).await?;
            } else if file_type.is_dir() {
                self.emit_str(b"directory").await?;
                // Collect + sort entries by raw byte name (Nix's canonical order).
                let mut entries: BTreeMap<Vec<u8>, OsString> = BTreeMap::new();
                let mut rd = tokio::fs::read_dir(path)
                    .await
                    .err_tip(|| format!("In dump_path: opendir {}", path.display()))?;
                while let Some(entry) = rd
                    .next_entry()
                    .await
                    .err_tip(|| format!("In dump_path: readdir {}", path.display()))?
                {
                    let name = entry.file_name();
                    entries.insert(name.as_bytes().to_vec(), name);
                }
                for (name_bytes, name) in entries {
                    self.emit_str(b"entry").await?;
                    self.emit_str(b"(").await?;
                    self.emit_str(b"name").await?;
                    self.emit_str(&name_bytes).await?;
                    self.emit_str(b"node").await?;
                    self.dump_node(&path.join(&name)).await?;
                    self.emit_str(b")").await?;
                }
            } else {
                return Err(make_err!(
                    Code::InvalidArgument,
                    "cannot serialize {} to NAR: unsupported file type",
                    path.display()
                ));
            }

            self.emit_str(b")").await?;
            Ok(())
        })
    }

    async fn send(&mut self, chunk: Bytes) -> Result<(), Error> {
        self.writer
            .send(chunk)
            .await
            .err_tip(|| "In dump_path: writing to NAR channel")
    }
}

/// Restores a NAR byte stream to `dest`, recreating the file tree (regular
/// files with their executable bit, symlinks, and directories). Used by the
/// client's pull/import path.
///
/// `dest` must not already exist (the top-level node materializes *as* `dest`).
/// Entry names are validated so a NAR can never escape `dest`: no `/`, no `.`
/// or `..`, no embedded NUL, and non-empty. Malformed or truncated input is
/// rejected with an [`Error`] rather than a panic.
pub async fn restore_path<R: AsyncRead + Unpin + Send>(
    reader: R,
    dest: &Path,
) -> Result<(), Error> {
    let mut nar = NarReader { reader };
    let magic = nar.read_bytes_token().await?;
    if magic != NAR_MAGIC.as_bytes() {
        return Err(make_input_err!("bad NAR magic: expected '{NAR_MAGIC}'"));
    }
    nar.restore_node(dest).await?;
    Ok(())
}

/// Parses NAR tokens from an [`AsyncRead`].
struct NarReader<R> {
    reader: R,
}

impl<R> core::fmt::Debug for NarReader<R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("NarReader").finish_non_exhaustive()
    }
}

impl<R: AsyncRead + Unpin + Send> NarReader<R> {
    /// Reads a little-endian `u64`.
    async fn read_u64(&mut self) -> Result<u64, Error> {
        let mut buf = [0u8; 8];
        self.reader
            .read_exact(&mut buf)
            .await
            .map_err(|e| make_input_err!("truncated NAR: expected u64 length: {e}"))?;
        Ok(u64::from_le_bytes(buf))
    }

    /// Skips `pad` zero-padding bytes and verifies they are actually zero.
    async fn skip_padding(&mut self, pad: usize) -> Result<(), Error> {
        if pad == 0 {
            return Ok(());
        }
        let mut buf = [0u8; 8];
        self.reader
            .read_exact(&mut buf[..pad])
            .await
            .map_err(|e| make_input_err!("truncated NAR: expected padding: {e}"))?;
        if buf[..pad].iter().any(|&b| b != 0) {
            return Err(make_input_err!("NAR padding was not zero"));
        }
        Ok(())
    }

    /// Reads a length-prefixed, 8-byte-padded token as owned bytes, capping the
    /// length at [`MAX_TOKEN_LEN`]. Used for type tags, entry names, and symlink
    /// targets — not for file contents.
    async fn read_bytes_token(&mut self) -> Result<Vec<u8>, Error> {
        let len = self.read_u64().await?;
        if len > MAX_TOKEN_LEN {
            return Err(make_input_err!(
                "NAR token length {len} exceeds maximum {MAX_TOKEN_LEN}"
            ));
        }
        let len_usize = usize::try_from(len).err_tip(|| "NAR token length overflow")?;
        let mut buf = vec![0u8; len_usize];
        self.reader
            .read_exact(&mut buf)
            .await
            .map_err(|e| make_input_err!("truncated NAR: expected {len} token bytes: {e}"))?;
        self.skip_padding(padding(len_usize)).await?;
        Ok(buf)
    }

    /// Reads a token expected to equal `expected`, erroring otherwise.
    async fn expect_token(&mut self, expected: &[u8]) -> Result<(), Error> {
        let got = self.read_bytes_token().await?;
        if got != expected {
            return Err(make_input_err!(
                "malformed NAR: expected '{}', got '{}'",
                String::from_utf8_lossy(expected),
                String::from_utf8_lossy(&got)
            ));
        }
        Ok(())
    }

    /// Restores the node currently at the read cursor into `dest`.
    ///
    /// Async recursion is boxed for the directory case.
    fn restore_node<'a>(
        &'a mut self,
        dest: &'a Path,
    ) -> Pin<Box<dyn Future<Output = Result<(), Error>> + Send + 'a>> {
        Box::pin(async move {
            self.expect_token(b"(").await?;
            self.expect_token(b"type").await?;
            let node_type = self.read_bytes_token().await?;
            match node_type.as_slice() {
                b"regular" => self.restore_regular(dest).await?,
                b"symlink" => self.restore_symlink(dest).await?,
                b"directory" => self.restore_directory(dest).await?,
                other => {
                    return Err(make_input_err!(
                        "malformed NAR: unknown node type '{}'",
                        String::from_utf8_lossy(other)
                    ));
                }
            }
            Ok(())
        })
    }

    async fn restore_regular(&mut self, dest: &Path) -> Result<(), Error> {
        // Optional `executable` `""`, then `contents` <bytes>.
        let mut executable = false;
        let mut tag = self.read_bytes_token().await?;
        if tag == b"executable" {
            // The paired empty string.
            let empty = self.read_bytes_token().await?;
            if !empty.is_empty() {
                return Err(make_input_err!(
                    "malformed NAR: 'executable' must be followed by an empty string"
                ));
            }
            executable = true;
            tag = self.read_bytes_token().await?;
        }
        if tag != b"contents" {
            return Err(make_input_err!(
                "malformed NAR: expected 'contents', got '{}'",
                String::from_utf8_lossy(&tag)
            ));
        }

        let len = self.read_u64().await?;
        let mut file = tokio::fs::File::create(dest)
            .await
            .err_tip(|| format!("In restore_path: create {}", dest.display()))?;
        self.copy_contents_to(&mut file, len).await?;
        file.flush()
            .await
            .err_tip(|| format!("In restore_path: flush {}", dest.display()))?;

        let mode = if executable { 0o755 } else { 0o644 };
        tokio::fs::set_permissions(dest, std::fs::Permissions::from_mode(mode))
            .await
            .err_tip(|| format!("In restore_path: chmod {}", dest.display()))?;
        // Skip the content padding, then consume the node's closing `)`.
        self.skip_padding(padding_u64(len)).await?;
        self.expect_token(b")").await?;
        Ok(())
    }

    /// Streams `len` content bytes from the reader into `file` in
    /// [`FILE_CHUNK_SIZE`] pieces (padding is handled by the caller).
    async fn copy_contents_to(
        &mut self,
        file: &mut tokio::fs::File,
        len: u64,
    ) -> Result<(), Error> {
        let mut remaining = len;
        let mut buf = vec![0u8; FILE_CHUNK_SIZE];
        while remaining > 0 {
            let want = usize::try_from(remaining.min(FILE_CHUNK_SIZE as u64))
                .err_tip(|| "NAR content chunk overflow")?;
            let n = self.reader.read(&mut buf[..want]).await.map_err(|e| {
                make_input_err!("truncated NAR: expected {remaining} more content bytes: {e}")
            })?;
            if n == 0 {
                return Err(make_input_err!(
                    "truncated NAR: expected {remaining} more content bytes, got EOF"
                ));
            }
            file.write_all(&buf[..n])
                .await
                .err_tip(|| "In restore_path: writing file contents")?;
            let n64 = u64::try_from(n).err_tip(|| "read length overflows u64")?;
            remaining -= n64;
        }
        Ok(())
    }

    async fn restore_symlink(&mut self, dest: &Path) -> Result<(), Error> {
        self.expect_token(b"target").await?;
        let target = self.read_bytes_token().await?;
        if target.contains(&0) {
            return Err(make_input_err!("NAR symlink target contains a NUL byte"));
        }
        let target_os: OsString = std::os::unix::ffi::OsStringExt::from_vec(target);
        tokio::fs::symlink(&target_os, dest)
            .await
            .err_tip(|| format!("In restore_path: symlink {}", dest.display()))?;
        // Consume the node's closing `)`.
        self.expect_token(b")").await?;
        Ok(())
    }

    async fn restore_directory(&mut self, dest: &Path) -> Result<(), Error> {
        tokio::fs::create_dir(dest)
            .await
            .err_tip(|| format!("In restore_path: mkdir {}", dest.display()))?;
        // Directories default to 0755 (create_dir applies the umask; Nix
        // canonicalises modes on import, and this matches the read side's
        // expectations for a freshly-restored tree).
        let mut prev_name: Option<Vec<u8>> = None;
        loop {
            // Either another `entry` or the closing `)`.
            let tag = self.read_bytes_token().await?;
            match tag.as_slice() {
                b")" => break,
                b"entry" => {}
                other => {
                    return Err(make_input_err!(
                        "malformed NAR: expected 'entry' or ')', got '{}'",
                        String::from_utf8_lossy(other)
                    ));
                }
            }
            self.expect_token(b"(").await?;
            self.expect_token(b"name").await?;
            let name = self.read_bytes_token().await?;
            validate_entry_name(&name)?;
            // Enforce strictly-ascending byte order, as Nix does on import.
            if let Some(prev) = &prev_name
                && name.as_slice() <= prev.as_slice()
            {
                return Err(make_input_err!(
                    "malformed NAR: directory entries out of order or duplicated"
                ));
            }
            self.expect_token(b"node").await?;
            let name_os: OsString = std::os::unix::ffi::OsStringExt::from_vec(name.clone());
            let child = dest.join(&name_os);
            self.restore_node(&child).await?;
            self.expect_token(b")").await?;
            prev_name = Some(name);
        }
        Ok(())
    }
}

/// Rejects entry names that could escape `dest`: empty, `.`, `..`, or names
/// containing `/` or a NUL byte.
fn validate_entry_name(name: &[u8]) -> Result<(), Error> {
    if name.is_empty() {
        return Err(make_input_err!("NAR directory entry has an empty name"));
    }
    if name == b"." || name == b".." {
        return Err(make_input_err!(
            "NAR directory entry name '.'/'..' is not allowed"
        ));
    }
    if name.contains(&b'/') {
        return Err(make_input_err!("NAR directory entry name contains a '/'"));
    }
    if name.contains(&0) {
        return Err(make_input_err!(
            "NAR directory entry name contains a NUL byte"
        ));
    }
    Ok(())
}

/// Zero bytes for emitting padding (at most 7 are ever used).
const ZERO_PAD: [u8; 8] = [0u8; 8];

/// Number of zero-padding bytes needed to round `len` up to a multiple of 8.
const fn padding(len: usize) -> usize {
    (8 - (len % 8)) % 8
}

/// Number of zero-padding bytes needed to round a `u64` `len` up to a multiple
/// of 8.
const fn padding_u64(len: u64) -> usize {
    ((8 - (len % 8)) % 8) as usize
}

/// Wraps a NAR byte stream, folding every byte through a SHA-256 hasher and a
/// length counter so [`HashingNar::finalize`] yields `(NarHash, NarSize)` once
/// the wrapped stream is exhausted.
pub struct HashingNar<S> {
    inner: S,
    hasher: sha2::Sha256,
    len: u64,
}

impl<S> core::fmt::Debug for HashingNar<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("HashingNar")
            .field("len", &self.len)
            .finish_non_exhaustive()
    }
}

impl<S> HashingNar<S> {
    /// Wraps `inner`.
    pub fn new(inner: S) -> Self {
        use sha2::Digest as _;
        Self {
            inner,
            hasher: sha2::Sha256::new(),
            len: 0,
        }
    }

    /// The `(sha256, byte length)` seen so far — call after the stream ends.
    #[must_use]
    pub fn finalize(self) -> ([u8; 32], u64) {
        use sha2::Digest as _;
        (self.hasher.finalize().into(), self.len)
    }
}

impl<S: Stream<Item = Result<Bytes, Error>> + Unpin> Stream for HashingNar<S> {
    type Item = Result<Bytes, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        use sha2::Digest as _;
        match Pin::new(&mut self.inner).poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                self.hasher.update(&chunk);
                self.len += chunk.len() as u64;
                Poll::Ready(Some(Ok(chunk)))
            }
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use futures::StreamExt;

    use super::*;

    /// Collects the whole NAR stream into one contiguous buffer (tests only).
    async fn collect_nar(path: &Path) -> Vec<u8> {
        let mut out = Vec::new();
        let mut stream = Box::pin(dump_path(path.to_path_buf()));
        while let Some(chunk) = stream.next().await {
            out.extend_from_slice(&chunk.expect("dump_path chunk"));
        }
        out
    }

    /// Hand-frames `str(s)` for the golden test: LE u64 length, bytes, padding.
    fn framed(s: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(&(s.len() as u64).to_le_bytes());
        v.extend_from_slice(s);
        v.extend(std::iter::repeat_n(0u8, padding(s.len())));
        v
    }

    #[tokio::test]
    async fn golden_magic_prefix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("f");
        tokio::fs::write(&file, b"hi").await.expect("write");
        let nar = collect_nar(&file).await;
        // The archive always starts with str("nix-archive-1").
        assert_eq!(
            &nar[..framed(b"nix-archive-1").len()],
            framed(b"nix-archive-1").as_slice()
        );
    }

    #[tokio::test]
    async fn golden_regular_file_exact_bytes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("f");
        tokio::fs::write(&file, b"hi").await.expect("write");

        let nar = collect_nar(&file).await;

        // Hand-computed: nar = str("nix-archive-1") node
        //   node = str("(") str("type") str("regular") str("contents") bytes("hi") str(")")
        let mut expected = Vec::new();
        expected.extend(framed(b"nix-archive-1"));
        expected.extend(framed(b"("));
        expected.extend(framed(b"type"));
        expected.extend(framed(b"regular"));
        expected.extend(framed(b"contents"));
        expected.extend(framed(b"hi")); // 2 bytes -> 6 padding
        expected.extend(framed(b")"));

        assert_eq!(nar, expected, "regular-file NAR framing must be exact");
    }

    #[tokio::test]
    async fn golden_executable_and_symlink_and_empty() {
        // Executable file.
        let dir = tempfile::tempdir().expect("tempdir");
        let exe = dir.path().join("x");
        tokio::fs::write(&exe, b"X").await.expect("write");
        tokio::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))
            .await
            .expect("chmod");
        let nar = collect_nar(&exe).await;
        let mut expected = Vec::new();
        expected.extend(framed(b"nix-archive-1"));
        expected.extend(framed(b"("));
        expected.extend(framed(b"type"));
        expected.extend(framed(b"regular"));
        expected.extend(framed(b"executable"));
        expected.extend(framed(b"")); // empty string == 8 zero bytes
        expected.extend(framed(b"contents"));
        expected.extend(framed(b"X"));
        expected.extend(framed(b")"));
        assert_eq!(nar, expected, "executable NAR framing must be exact");

        // Symlink.
        let link = dir.path().join("l");
        tokio::fs::symlink("/some/target", &link)
            .await
            .expect("symlink");
        let nar = collect_nar(&link).await;
        let mut expected = Vec::new();
        expected.extend(framed(b"nix-archive-1"));
        expected.extend(framed(b"("));
        expected.extend(framed(b"type"));
        expected.extend(framed(b"symlink"));
        expected.extend(framed(b"target"));
        expected.extend(framed(b"/some/target"));
        expected.extend(framed(b")"));
        assert_eq!(nar, expected, "symlink NAR framing must be exact");

        // Empty regular file.
        let empty = dir.path().join("e");
        tokio::fs::write(&empty, b"").await.expect("write");
        let nar = collect_nar(&empty).await;
        let mut expected = Vec::new();
        expected.extend(framed(b"nix-archive-1"));
        expected.extend(framed(b"("));
        expected.extend(framed(b"type"));
        expected.extend(framed(b"regular"));
        expected.extend(framed(b"contents"));
        expected.extend(framed(b"")); // zero-length contents
        expected.extend(framed(b")"));
        assert_eq!(nar, expected, "empty-file NAR framing must be exact");
    }

    /// Builds a representative tree and returns its root.
    async fn build_fixture_tree(root: &Path) {
        // regular (non-multiple of 8 length) + executable + empty file
        tokio::fs::write(root.join("regular.txt"), b"hello, nar")
            .await
            .expect("write regular");
        let exe = root.join("run.sh");
        tokio::fs::write(&exe, b"#!/bin/sh\necho hi\n")
            .await
            .expect("write exe");
        tokio::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755))
            .await
            .expect("chmod exe");
        tokio::fs::write(root.join("empty"), b"")
            .await
            .expect("write empty");
        // File whose size is an exact multiple of 8 (16 bytes).
        tokio::fs::write(root.join("aligned"), b"0123456789abcdef")
            .await
            .expect("write aligned");
        // Symlink.
        tokio::fs::symlink("regular.txt", root.join("link"))
            .await
            .expect("symlink");
        // Nested dir with a file, plus an empty dir.
        tokio::fs::create_dir(root.join("sub"))
            .await
            .expect("mkdir sub");
        tokio::fs::write(root.join("sub").join("nested"), b"deep")
            .await
            .expect("write nested");
        tokio::fs::create_dir(root.join("emptydir"))
            .await
            .expect("mkdir emptydir");
    }

    /// Recursively compares two trees for structure, contents, exec bits, and
    /// symlink targets.
    fn assert_trees_equal(a: &Path, b: &Path) {
        let am = std::fs::symlink_metadata(a).expect("stat a");
        let bm = std::fs::symlink_metadata(b).expect("stat b");
        let at = am.file_type();
        let bt = bm.file_type();
        assert_eq!(at.is_dir(), bt.is_dir(), "dir mismatch at {a:?}");
        assert_eq!(at.is_file(), bt.is_file(), "file mismatch at {a:?}");
        assert_eq!(
            at.is_symlink(),
            bt.is_symlink(),
            "symlink mismatch at {a:?}"
        );
        if at.is_symlink() {
            let ta = std::fs::read_link(a).expect("readlink a");
            let tb = std::fs::read_link(b).expect("readlink b");
            assert_eq!(ta, tb, "symlink target mismatch at {a:?}");
        } else if at.is_file() {
            let ca = std::fs::read(a).expect("read a");
            let cb = std::fs::read(b).expect("read b");
            assert_eq!(ca, cb, "content mismatch at {a:?}");
            let xa = am.permissions().mode() & 0o100 != 0;
            let xb = bm.permissions().mode() & 0o100 != 0;
            assert_eq!(xa, xb, "exec-bit mismatch at {a:?}");
        } else if at.is_dir() {
            let mut ea: Vec<_> = std::fs::read_dir(a)
                .expect("readdir a")
                .map(|e| e.expect("entry").file_name())
                .collect();
            let mut eb: Vec<_> = std::fs::read_dir(b)
                .expect("readdir b")
                .map(|e| e.expect("entry").file_name())
                .collect();
            ea.sort();
            eb.sort();
            assert_eq!(ea, eb, "dir entries mismatch at {a:?}");
            for name in ea {
                assert_trees_equal(&a.join(&name), &b.join(&name));
            }
        }
    }

    #[tokio::test]
    async fn round_trip_tree() {
        let src = tempfile::tempdir().expect("src tempdir");
        let src_root = src.path().join("root");
        tokio::fs::create_dir(&src_root).await.expect("mkdir root");
        build_fixture_tree(&src_root).await;

        let nar = collect_nar(&src_root).await;

        let dst = tempfile::tempdir().expect("dst tempdir");
        let dst_root = dst.path().join("restored");
        restore_path(std::io::Cursor::new(nar), &dst_root)
            .await
            .expect("restore_path");

        assert_trees_equal(&src_root, &dst_root);
    }

    #[tokio::test]
    async fn round_trip_single_symlink() {
        let src = tempfile::tempdir().expect("src tempdir");
        let link = src.path().join("l");
        tokio::fs::symlink("/nix/store/whatever", &link)
            .await
            .expect("symlink");
        let nar = collect_nar(&link).await;

        let dst = tempfile::tempdir().expect("dst tempdir");
        let out = dst.path().join("l");
        restore_path(std::io::Cursor::new(nar), &out)
            .await
            .expect("restore_path");
        assert_eq!(
            std::fs::read_link(&out).expect("readlink"),
            Path::new("/nix/store/whatever")
        );
    }

    #[tokio::test]
    async fn restore_rejects_bad_magic() {
        let bogus = b"not-a-nar-at-all-really".to_vec();
        let dst = tempfile::tempdir().expect("dst tempdir");
        let out = dst.path().join("x");
        let err = restore_path(std::io::Cursor::new(bogus), &out)
            .await
            .expect_err("bad magic must error");
        assert_eq!(err.code, Code::InvalidArgument);
    }

    #[tokio::test]
    async fn restore_rejects_traversal_name() {
        // Build a directory NAR whose single entry is named "..".
        let mut nar = Vec::new();
        nar.extend(framed(b"nix-archive-1"));
        nar.extend(framed(b"("));
        nar.extend(framed(b"type"));
        nar.extend(framed(b"directory"));
        nar.extend(framed(b"entry"));
        nar.extend(framed(b"("));
        nar.extend(framed(b"name"));
        nar.extend(framed(b".."));
        nar.extend(framed(b"node"));
        nar.extend(framed(b"("));
        nar.extend(framed(b"type"));
        nar.extend(framed(b"regular"));
        nar.extend(framed(b"contents"));
        nar.extend(framed(b"pwned"));
        nar.extend(framed(b")"));
        nar.extend(framed(b")"));
        nar.extend(framed(b")"));

        let dst = tempfile::tempdir().expect("dst tempdir");
        let out = dst.path().join("d");
        let err = restore_path(std::io::Cursor::new(nar), &out)
            .await
            .expect_err("'..' entry must error");
        assert_eq!(err.code, Code::InvalidArgument);
    }

    /// Cross-checks `dump_path` against the real `nix-store --dump` when the
    /// binary and a store are available. Skips (with a message) otherwise, so
    /// CI without Nix still passes.
    #[tokio::test]
    async fn cross_check_against_nix_store() {
        // Locate `nix-store` on PATH.
        let which = std::process::Command::new("sh")
            .args(["-c", "command -v nix-store"])
            .output();
        let Ok(out) = which else {
            eprintln!("SKIP cross_check: could not run `command -v nix-store`");
            return;
        };
        if !out.status.success() {
            eprintln!("SKIP cross_check: nix-store not found on PATH");
            return;
        }

        // Pick both a small regular-file store path and a small directory store
        // path, so the comparison exercises leaf nodes, entry sorting, and
        // recursion. If there is no store, skip.
        let Ok(rd) = std::fs::read_dir("/nix/store") else {
            eprintln!("SKIP cross_check: /nix/store not readable");
            return;
        };
        let mut a_file: Option<PathBuf> = None;
        let mut a_dir: Option<PathBuf> = None;
        for entry in rd.flatten() {
            let p = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if a_file.is_none() && meta.is_file() && meta.len() > 0 && meta.len() < 8192 {
                a_file = Some(p.clone());
            } else if a_dir.is_none() && meta.is_dir() {
                // Keep small directories cheap: bound the NAR we will materialise.
                if let Ok(ref_out) = std::process::Command::new("nix-store")
                    .arg("--dump")
                    .arg(&p)
                    .output()
                    && ref_out.status.success()
                    && ref_out.stdout.len() < 64 * 1024
                {
                    a_dir = Some(p.clone());
                }
            }
            if a_file.is_some() && a_dir.is_some() {
                break;
            }
        }

        let mut checked = 0usize;
        for path in [a_file, a_dir].into_iter().flatten() {
            let reference = std::process::Command::new("nix-store")
                .arg("--dump")
                .arg(&path)
                .output()
                .expect("run nix-store --dump");
            assert!(
                reference.status.success(),
                "nix-store --dump failed for {}",
                path.display()
            );
            let ours = collect_nar(&path).await;
            assert_eq!(
                ours,
                reference.stdout,
                "dump_path must be byte-identical to `nix-store --dump {}`",
                path.display()
            );
            eprintln!(
                "cross_check OK: dump_path matched nix-store --dump for {} ({} bytes)",
                path.display(),
                ours.len()
            );
            checked += 1;
        }
        if checked == 0 {
            eprintln!("SKIP cross_check: no suitable small store path found");
        }
    }

    proptest::proptest! {
        // Arbitrary bytes fed to restore_path must never panic — only Ok/Err.
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(512))]
        #[test]
        fn restore_never_panics(data in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..4096)) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            rt.block_on(async {
                let dst = tempfile::tempdir().expect("dst tempdir");
                let out = dst.path().join("out");
                // Result is intentionally ignored: the property is "does not panic".
                drop(restore_path(std::io::Cursor::new(data), &out).await);
            });
        }
    }
}
