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

//! The Nix binary-cache HTTP client: the same wire protocol as `nix copy`,
//! co-designed with the NativeLink `nix_cache` server.
//!
//! A push serializes the store path to a NAR ([`crate::nar::dump_path`]), tees
//! it through a SHA-256 hasher for the `NarHash`, zstd-compresses it (learning
//! the `FileHash` of the compressed bytes and spooling them to a temp file),
//! then — after a validated `GET {hash}.narinfo` dedup probe — `PUT`s
//! `nar/{FileHash}.nar.zst` followed by the signed `{hash}.narinfo`. A pull is
//! the inverse: fetch + verify the narinfo signature, download + decompress the
//! NAR, and restore it, checking the bytes against the signed `NarHash`.

use std::io;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Instant;

use async_compression::Level;
use async_compression::tokio::write::ZstdEncoder;
use futures::StreamExt as _;
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_nix::path_info::NixPathInfo;
use nativelink_nix::signing::{NixPublicKey, NixSigningKey};
use nativelink_nix::{nar_url, narinfo};
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncWrite, AsyncWriteExt as _};
use tokio_util::io::ReaderStream;

use crate::config::{Auth, CacheConfig};
use crate::metrics::Metrics;
use crate::nar;
use crate::store::PathMeta;

/// The outcome of a single path push.
#[derive(Clone, Copy, Debug)]
pub enum PushOutcome {
    /// The NAR + narinfo were uploaded.
    Uploaded {
        /// Uncompressed NAR bytes.
        nar_size: u64,
        /// Compressed bytes sent on the wire.
        wire_bytes: u64,
    },
    /// The cache already had a structurally valid narinfo; nothing sent.
    AlreadyPresent,
}

/// A cache client bound to one cache URL, its credentials, and its signing keys.
#[derive(Debug)]
pub struct CacheClient {
    base_url: String,
    auth: Option<Auth>,
    compress: bool,
    compression_level: i32,
    dedup: bool,
    metrics: Arc<Metrics>,
    http: reqwest::Client,
    signing_keys: Vec<NixSigningKey>,
}

impl CacheClient {
    /// Builds a client from `config`, loading and validating any signing keys.
    pub fn new(config: CacheConfig, metrics: Arc<Metrics>) -> Result<Self, Error> {
        let http = reqwest::Client::builder()
            .user_agent(concat!("nl-nix/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| make_err!(Code::Internal, "building HTTP client: {e}"))?;
        let mut signing_keys = Vec::with_capacity(config.signing_key_files.len());
        for file in &config.signing_key_files {
            let contents = std::fs::read_to_string(file)
                .err_tip(|| format!("reading signing key '{file}'"))?;
            signing_keys.push(
                NixSigningKey::from_secret_string(contents.trim())
                    .err_tip(|| format!("parsing signing key '{file}'"))?,
            );
        }
        Ok(Self {
            base_url: config.base_url.trim_end_matches('/').to_string(),
            auth: config.auth,
            compress: config.compress,
            compression_level: config.compression_level,
            dedup: config.dedup,
            metrics,
            http,
            signing_keys,
        })
    }

    /// Applies the configured `Authorization` header to a request builder.
    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.auth {
            None => req,
            Some(Auth::Bearer(token)) => req.bearer_auth(token),
            Some(Auth::Basic { user, password }) => req.basic_auth(user, Some(password)),
        }
    }

    /// `GET /nix-cache-info` — returns the raw body (for `info`/probing).
    pub async fn cache_info(&self) -> Result<String, Error> {
        let url = format!("{}/nix-cache-info", self.base_url);
        let resp = self
            .authed(self.http.get(&url))
            .send()
            .await
            .err_tip(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            return Err(make_err!(
                Code::Unavailable,
                "GET {url} returned {}",
                resp.status()
            ));
        }
        resp.text().await.err_tip(|| "reading nix-cache-info body")
    }

    /// Whether the cache has a structurally valid narinfo for this path.
    /// A poisoned historical record must be treated as absent so a subsequent
    /// push overwrites it instead of deduplicating against unusable metadata.
    pub async fn has_path(&self, hash: &str) -> Result<bool, Error> {
        let url = format!("{}/{hash}.narinfo", self.base_url);
        let resp = self
            .authed(self.http.get(&url))
            .send()
            .await
            .err_tip(|| format!("GET {url}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(false);
        }
        if !resp.status().is_success() {
            return Ok(false);
        }
        let text = resp.text().await.err_tip(|| format!("reading {url}"))?;
        Ok(narinfo_is_usable(&text))
    }

    /// Pushes one store path: dedup probe, then dump + hash + compress + spool +
    /// `PUT nar` + signed `PUT narinfo`.
    pub async fn push_path(&self, meta: &PathMeta) -> Result<PushOutcome, Error> {
        let started = Instant::now();
        let result = self.push_path_inner(meta).await;
        match &result {
            Ok(PushOutcome::Uploaded {
                nar_size,
                wire_bytes,
            }) => self
                .metrics
                .push_ok(*nar_size, *wire_bytes, started.elapsed().as_secs_f64()),
            Ok(PushOutcome::AlreadyPresent) => self.metrics.push_deduped(),
            Err(_) => self.metrics.push_error(),
        }
        result
    }

    async fn push_path_inner(&self, meta: &PathMeta) -> Result<PushOutcome, Error> {
        let hash = meta.path.hash_part();
        if self.dedup && self.has_path(hash).await? {
            return Ok(PushOutcome::AlreadyPresent);
        }

        // Dump → NarHash → zstd → FileHash → spool, in one streaming pass.
        let spool = SpoolFile::new(hash);
        let dumped = compress_to_spool(
            PathBuf::from(meta.path.full_path()),
            spool.path().to_path_buf(),
            self.compress,
            self.compression_level,
        )
        .await?;

        // Build the narinfo the server will store and re-serve.
        let (url_name, compression, file_hash, file_size) = if self.compress {
            (
                nar_url::canonical_nar_zst_name(&dumped.file_hash, dumped.file_size),
                "zstd".to_string(),
                Some(dumped.file_hash),
                Some(dumped.file_size),
            )
        } else {
            (
                nar_url::canonical_nar_name(&dumped.nar_hash, dumped.nar_size),
                "none".to_string(),
                None,
                None,
            )
        };
        let mut info = narinfo::NarInfo {
            store_path: meta.path.full_path().to_string(),
            url: format!("nar/{url_name}"),
            compression,
            file_hash,
            file_size,
            nar_hash: dumped.nar_hash,
            nar_size: dumped.nar_size,
            references: meta
                .references
                .iter()
                .map(|r| r.base_name().to_string())
                .collect(),
            deriver: meta.deriver.clone(),
            system: None,
            sigs: meta.sigs.clone(),
            ca: meta.ca.clone(),
        };
        let fingerprint = info.fingerprint();
        for key in &self.signing_keys {
            let sig = key.sign(&fingerprint);
            if !info.sigs.contains(&sig) {
                info.sigs.push(sig);
            }
        }

        // PUT the NAR (streamed from the spool), then the narinfo.
        let nar_url = format!("{}/{}", self.base_url, info.url);
        let file = tokio::fs::File::open(spool.path())
            .await
            .err_tip(|| "opening NAR spool for upload")?;
        let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
        let resp = self
            .authed(self.http.put(&nar_url))
            .body(body)
            .send()
            .await
            .err_tip(|| format!("PUT {nar_url}"))?;
        if !resp.status().is_success() {
            return Err(make_err!(
                Code::Unavailable,
                "PUT {nar_url} returned {}",
                resp.status()
            ));
        }

        let narinfo_url = format!("{}/{hash}.narinfo", self.base_url);
        let resp = self
            .authed(self.http.put(&narinfo_url))
            .body(info.render())
            .send()
            .await
            .err_tip(|| format!("PUT {narinfo_url}"))?;
        if !resp.status().is_success() {
            return Err(make_err!(
                Code::Unavailable,
                "PUT {narinfo_url} returned {}",
                resp.status()
            ));
        }

        Ok(PushOutcome::Uploaded {
            nar_size: dumped.nar_size,
            wire_bytes: dumped.file_size,
        })
    }

    /// Pulls one store path by its 32-char hash: fetch the narinfo, verify its
    /// signature against `trusted_keys`, download + decompress the NAR, and
    /// restore it into `dest`, checking the bytes against the signed `NarHash`.
    ///
    /// v1 restores the tree but does not register it as a valid store path
    /// (that requires the nix daemon); ordinary substitution is nix's job.
    pub async fn pull_path(
        &self,
        hash: &str,
        trusted_keys: &[NixPublicKey],
        dest: &std::path::Path,
    ) -> Result<u64, Error> {
        let result = self.pull_path_inner(hash, trusted_keys, dest).await;
        match &result {
            Ok(nar_size) => self.metrics.pull_ok(*nar_size),
            Err(_) => self.metrics.pull_error(),
        }
        result
    }

    async fn pull_path_inner(
        &self,
        hash: &str,
        trusted_keys: &[NixPublicKey],
        dest: &std::path::Path,
    ) -> Result<u64, Error> {
        let narinfo_url = format!("{}/{hash}.narinfo", self.base_url);
        let resp = self
            .authed(self.http.get(&narinfo_url))
            .send()
            .await
            .err_tip(|| format!("GET {narinfo_url}"))?;
        if !resp.status().is_success() {
            return Err(make_err!(
                Code::NotFound,
                "GET {narinfo_url} returned {}",
                resp.status()
            ));
        }
        let text = resp.text().await.err_tip(|| "reading narinfo body")?;
        let info = narinfo::parse(&text).err_tip(|| "parsing narinfo")?;

        if !trusted_keys.is_empty() {
            let fingerprint = info.fingerprint();
            let verified = info
                .sigs
                .iter()
                .any(|sig| trusted_keys.iter().any(|k| k.verify(&fingerprint, sig)));
            if !verified {
                return Err(make_input_err!(
                    "narinfo for {hash} has no signature from a trusted key"
                ));
            }
        }

        let nar_url = if info.url.starts_with("http://") || info.url.starts_with("https://") {
            info.url.clone()
        } else {
            format!("{}/{}", self.base_url, info.url.trim_start_matches('/'))
        };
        let resp = self
            .authed(self.http.get(&nar_url))
            .send()
            .await
            .err_tip(|| format!("GET {nar_url}"))?;
        if !resp.status().is_success() {
            return Err(make_err!(
                Code::Unavailable,
                "GET {nar_url} returned {}",
                resp.status()
            ));
        }

        // Decompress (if compressed) → hash the uncompressed NAR → restore.
        let byte_stream = resp
            .bytes_stream()
            .map(|r| r.map_err(|e| io::Error::other(e.to_string())));
        let reader = tokio_util::io::StreamReader::new(byte_stream);
        let decoded: Pin<Box<dyn tokio::io::AsyncRead + Send>> = match info.compression.as_str() {
            "" | "none" => Box::pin(reader),
            "zstd" => Box::pin(async_compression::tokio::bufread::ZstdDecoder::new(reader)),
            "xz" => Box::pin(async_compression::tokio::bufread::XzDecoder::new(reader)),
            "bzip2" => Box::pin(async_compression::tokio::bufread::BzDecoder::new(reader)),
            "gzip" => Box::pin(async_compression::tokio::bufread::GzipDecoder::new(reader)),
            other => {
                return Err(make_input_err!("unsupported NAR compression '{other}'"));
            }
        };
        let cell: HashCell = Arc::new(std::sync::Mutex::new((Sha256::new(), 0)));
        let hashing = HashingReader::new(decoded, cell.clone());
        nar::restore_path(hashing, dest)
            .await
            .err_tip(|| "restoring NAR to destination")?;
        let (nar_hash, nar_size) = {
            let guard = cell
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            (<[u8; 32]>::from(guard.0.clone().finalize()), guard.1)
        };
        if nar_hash != info.nar_hash || nar_size != info.nar_size {
            return Err(make_input_err!(
                "downloaded NAR for {hash} does not match its signed NarHash/NarSize"
            ));
        }
        Ok(nar_size)
    }
}

fn narinfo_is_usable(text: &str) -> bool {
    let Ok(info) = narinfo::parse(text) else {
        return false;
    };
    NixPathInfo::from_nar_info(&info).encode_record().is_ok()
}

#[cfg(test)]
mod tests {
    use super::narinfo_is_usable;

    const BASE: &str = concat!(
        "StorePath: /nix/store/00000000000000000000000000000000-output\n",
        "URL: nar/output.nar\n",
        "Compression: none\n",
        "NarHash: sha256:0000000000000000000000000000000000000000000000000000\n",
        "NarSize: 1\n",
    );

    #[test]
    fn dedup_rejects_poisoned_full_path_deriver() {
        assert!(!narinfo_is_usable(&format!(
            "{BASE}Deriver: /nix/store/11111111111111111111111111111111-builder.drv\n"
        )));
        assert!(narinfo_is_usable(&format!(
            "{BASE}Deriver: 11111111111111111111111111111111-builder.drv\n"
        )));
    }
}

/// The result of dumping + compressing a store path to a spool file.
struct Dumped {
    nar_hash: [u8; 32],
    nar_size: u64,
    file_hash: [u8; 32],
    file_size: u64,
}

/// Dumps the NAR of `path`, tees the uncompressed bytes through a SHA-256
/// hasher (`NarHash`/`NarSize`), optionally zstd-compresses, and writes the
/// on-wire bytes to `spool` while hashing them (`FileHash`/`FileSize`).
async fn compress_to_spool(
    path: PathBuf,
    spool: PathBuf,
    compress: bool,
    level: i32,
) -> Result<Dumped, Error> {
    let file = tokio::fs::File::create(&spool)
        .await
        .err_tip(|| format!("creating NAR spool '{}'", spool.display()))?;
    let mut nar_hasher = Sha256::new();
    let mut nar_size: u64 = 0;
    let mut dump = nar::dump_path(path);

    let hashing = if compress {
        let mut enc = ZstdEncoder::with_quality(HashingWriter::new(file), Level::Precise(level));
        while let Some(chunk) = dump.next().await {
            let chunk = chunk?;
            nar_hasher.update(&chunk);
            nar_size += chunk.len() as u64;
            enc.write_all(&chunk)
                .await
                .err_tip(|| "compressing NAR to spool")?;
        }
        enc.shutdown().await.err_tip(|| "finalizing zstd stream")?;
        enc.into_inner()
    } else {
        let mut hw = HashingWriter::new(file);
        while let Some(chunk) = dump.next().await {
            let chunk = chunk?;
            nar_hasher.update(&chunk);
            nar_size += chunk.len() as u64;
            hw.write_all(&chunk)
                .await
                .err_tip(|| "writing NAR to spool")?;
        }
        hw.shutdown().await.err_tip(|| "flushing NAR spool")?;
        hw
    };
    let (file_hash, file_size) = hashing.finalize();
    Ok(Dumped {
        nar_hash: nar_hasher.finalize().into(),
        nar_size,
        file_hash,
        file_size,
    })
}

/// An `AsyncWrite` that SHA-256-hashes and counts everything written through it.
struct HashingWriter<W> {
    inner: W,
    hasher: Sha256,
    len: u64,
}

impl<W> HashingWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            hasher: Sha256::new(),
            len: 0,
        }
    }

    fn finalize(self) -> ([u8; 32], u64) {
        (self.hasher.finalize().into(), self.len)
    }
}

impl<W: AsyncWrite + Unpin> AsyncWrite for HashingWriter<W> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let this = self.get_mut();
        match Pin::new(&mut this.inner).poll_write(cx, buf) {
            Poll::Ready(Ok(n)) => {
                this.hasher.update(&buf[..n]);
                this.len += n as u64;
                Poll::Ready(Ok(n))
            }
            other => other,
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// Shared running SHA-256 + byte count for a NAR being read.
type HashCell = Arc<std::sync::Mutex<(Sha256, u64)>>;

/// An `AsyncRead` that folds everything read through it into a shared
/// [`HashCell`], so the caller can read `(NarHash, NarSize)` after
/// `restore_path` has consumed the reader by value.
struct HashingReader<R> {
    inner: R,
    cell: HashCell,
}

impl<R> HashingReader<R> {
    fn new(inner: R, cell: HashCell) -> Self {
        Self { inner, cell }
    }
}

impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for HashingReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let res = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &res {
            let new = &buf.filled()[before..];
            let mut guard = this
                .cell
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.0.update(new);
            guard.1 += new.len() as u64;
        }
        res
    }
}

/// A spool file under the system temp dir, removed on drop.
struct SpoolFile {
    path: PathBuf,
}

impl SpoolFile {
    fn new(hash: &str) -> Self {
        let name = format!("nl-nix-spool-{}-{hash}.nar", std::process::id());
        Self {
            path: std::env::temp_dir().join(name),
        }
    }

    fn path(&self) -> &std::path::Path {
        &self.path
    }
}

impl Drop for SpoolFile {
    fn drop(&mut self) {
        drop(std::fs::remove_file(&self.path));
    }
}
