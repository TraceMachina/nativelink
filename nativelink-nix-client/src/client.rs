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
//! co-designed with the `NativeLink` `nix_cache` server.
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
use reqwest::Url;
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
/// How a pull decides whether to trust the metadata a cache returns.
///
/// A `narinfo` is the only thing that tells the client which bytes to expect,
/// so checking the downloaded NAR against it proves consistency, not
/// authenticity. Authenticity comes from the `Sig` lines, verified against
/// keys the operator chose to trust, exactly as `nix` does with
/// `trusted-public-keys`.
#[derive(Clone, Debug)]
pub enum TrustPolicy {
    /// Accept a `narinfo` only when one of its signatures verifies against one
    /// of these keys. The list must not be empty.
    Keys(Vec<NixPublicKey>),
    /// Skip signature verification. Only for caches reached over a channel
    /// that is trusted some other way; the caller opts in explicitly.
    Unverified,
}

/// Whether a NAR request may carry the cache's credentials.
///
/// The `URL` field of a `narinfo` is not covered by its signature, so a cache
/// (or anyone who can rewrite its responses) can point it at any host. The
/// credentials belong to the cache's origin alone: same scheme, host and port.
/// A downgrade from an `https` cache to an `http` NAR host is refused outright,
/// because it would move the download, and any credentials with it, onto a
/// cleartext connection the operator never agreed to.
fn credentials_allowed(cache: &Url, nar: &Url) -> Result<bool, Error> {
    if cache.scheme() == "https" && nar.scheme() == "http" {
        return Err(make_input_err!(
            "refusing to download NAR over cleartext http from '{}' for a cache served over https",
            nar.host_str().unwrap_or("?")
        ));
    }
    Ok(cache.origin() == nar.origin())
}

#[derive(Debug)]
pub struct CacheClient {
    base_url: String,
    base: Url,
    store_dir: String,
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
        let base_url = config.base_url.trim_end_matches('/').to_string();
        let base = Url::parse(&base_url)
            .map_err(|e| make_input_err!("cache URL '{base_url}' is not a valid URL: {e}"))?;
        if !matches!(base.scheme(), "http" | "https") {
            return Err(make_input_err!(
                "cache URL '{base_url}' must use http or https"
            ));
        }
        Ok(Self {
            base_url,
            base,
            store_dir: config.store_dir.trim_end_matches('/').to_string(),
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
    /// signature under `trust`, download + decompress the NAR, and
    /// restore it into `dest`, checking the bytes against the signed `NarHash`.
    ///
    /// v1 restores the tree but does not register it as a valid store path
    /// (that requires the nix daemon); ordinary substitution is nix's job.
    pub async fn pull_path(
        &self,
        hash: &str,
        trust: &TrustPolicy,
        dest: &std::path::Path,
    ) -> Result<u64, Error> {
        let result = self.pull_path_inner(hash, trust, dest).await;
        match &result {
            Ok(nar_size) => self.metrics.pull_ok(*nar_size),
            Err(_) => self.metrics.pull_error(),
        }
        result
    }

    async fn pull_path_inner(
        &self,
        hash: &str,
        trust: &TrustPolicy,
        dest: &std::path::Path,
    ) -> Result<u64, Error> {
        if let TrustPolicy::Keys(keys) = trust
            && keys.is_empty()
        {
            return Err(make_input_err!(
                "pull needs at least one trusted public key, or an explicit choice to skip verification"
            ));
        }
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

        // Bind the answer to the question. A signature authenticates the path
        // the narinfo describes, not the request that produced it, so a cache
        // could otherwise answer a request for A with a perfectly valid B and
        // have B restored under A's name.
        let returned_hash = info
            .store_path_hash()
            .err_tip(|| format!("narinfo for {hash} names an invalid StorePath"))?;
        if returned_hash != hash {
            return Err(make_input_err!(
                "narinfo for {hash} describes a different store path ({})",
                info.store_path
            ));
        }
        let returned_dir = info.store_path.rsplit_once('/').map_or("", |(dir, _)| dir);
        if returned_dir != self.store_dir {
            return Err(make_input_err!(
                "narinfo for {hash} is for store directory '{returned_dir}', not '{}'",
                self.store_dir
            ));
        }

        if let TrustPolicy::Keys(keys) = trust {
            let fingerprint = info.fingerprint();
            let verified = info
                .sigs
                .iter()
                .any(|sig| keys.iter().any(|k| k.verify(&fingerprint, sig)));
            if !verified {
                return Err(make_input_err!(
                    "narinfo for {hash} has no signature from a trusted key"
                ));
            }
        }

        let nar_url = if info.url.starts_with("http://") || info.url.starts_with("https://") {
            Url::parse(&info.url)
                .map_err(|e| make_input_err!("narinfo for {hash} has an invalid URL: {e}"))?
        } else {
            Url::parse(&format!(
                "{}/{}",
                self.base_url,
                info.url.trim_start_matches('/')
            ))
            .map_err(|e| make_input_err!("narinfo for {hash} has an invalid URL: {e}"))?
        };
        let request = self.http.get(nar_url.clone());
        let request = if credentials_allowed(&self.base, &nar_url)? {
            self.authed(request)
        } else {
            request
        };
        let resp = request.send().await.err_tip(|| format!("GET {nar_url}"))?;
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
    const fn new(inner: R, cell: HashCell) -> Self {
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
        if matches!(res, Poll::Ready(Ok(()))) {
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

/// Pull-side behaviour against local HTTP fixtures: credential scoping, the
/// binding of the returned `StorePath` to the requested hash, and the trust
/// policy. The fixture is a minimal HTTP/1.1 responder that records every
/// request's path and headers.
#[cfg(test)]
mod pull_tests {
    use std::collections::HashMap;
    use std::fmt::Write as _;
    use std::sync::{Arc, Mutex};

    use futures::StreamExt as _;
    use nativelink_macro::nativelink_test;
    use nativelink_nix::signing::NixSigningKey;
    use nativelink_nix::{narinfo, nixbase32};
    use nativelink_util::spawn;
    use nativelink_util::task::JoinHandleDropGuard;
    use sha2::{Digest as _, Sha256};
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::TcpListener;

    use super::{Auth, CacheClient, CacheConfig, Metrics, TrustPolicy, Url, credentials_allowed};

    /// A throwaway ed25519 key (`nix key generate-secret --key-name test-int-1`);
    /// it protects nothing and is shared with the server's integration suite.
    const SECRET_KEY: &str = "test-int-1:59Az40GGu2M3sM6rQ6T/+c61OMLw1r+qx9xMe/SzkIkjJupCgoxM63utsPNUImg3vc6stSSfAlXYYgmmUSZ5Ig==";
    const HASH_A: &str = "00000000000000000000000000000000";
    const HASH_B: &str = "11111111111111111111111111111111";

    type Routes = HashMap<String, (u16, Vec<u8>)>;
    type Seen = Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>;

    struct Fixture {
        base: String,
        seen: Seen,
        _server: JoinHandleDropGuard<()>,
    }

    impl Fixture {
        fn paths(&self) -> Vec<String> {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .map(|(p, _)| p.clone())
                .collect()
        }

        fn authorization_for(&self, path: &str) -> Option<String> {
            self.seen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .iter()
                .find(|(p, _)| p == path)
                .and_then(|(_, headers)| {
                    headers
                        .iter()
                        .find(|(k, _)| k == "authorization")
                        .map(|(_, v)| v.clone())
                })
        }
    }

    async fn serve(routes: Routes) -> Fixture {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let seen: Seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        let server = spawn!("pull_test_fixture", async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&buf).into_owned();
                let mut lines = text.split("\r\n");
                let path = lines
                    .next()
                    .and_then(|l| l.split(' ').nth(1))
                    .unwrap_or("")
                    .to_string();
                let headers = lines
                    .take_while(|l| !l.is_empty())
                    .filter_map(|l| l.split_once(':'))
                    .map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string()))
                    .collect();
                recorder
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push((path.clone(), headers));
                let (status, body) = routes
                    .get(&path)
                    .cloned()
                    .unwrap_or_else(|| (404, b"not found".to_vec()));
                let head = format!(
                    "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                drop(sock.write_all(head.as_bytes()).await);
                drop(sock.write_all(&body).await);
                drop(sock.shutdown().await);
            }
        });
        Fixture {
            base,
            seen,
            _server: server,
        }
    }

    /// A real NAR of a one-file tree, with its hash and size.
    async fn fixture_nar() -> (Vec<u8>, [u8; 32], u64) {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("hello"), b"hello from the fixture\n").expect("write");
        let mut nar = Vec::new();
        let mut stream = Box::pin(super::nar::dump_path(dir.path().to_path_buf()));
        while let Some(chunk) = stream.next().await {
            nar.extend_from_slice(&chunk.expect("nar chunk"));
        }
        let hash = <[u8; 32]>::from(Sha256::digest(&nar));
        let size = nar.len() as u64;
        (nar, hash, size)
    }

    fn narinfo_text(
        hash: &str,
        url: &str,
        nar_hash: &[u8; 32],
        nar_size: u64,
        sign: bool,
    ) -> String {
        let mut text = format!(
            "StorePath: /nix/store/{hash}-fixture\nURL: {url}\nCompression: none\nNarHash: sha256:{}\nNarSize: {nar_size}\nReferences: \n",
            nixbase32::encode(nar_hash)
        );
        if sign {
            let info = narinfo::parse(&text).expect("fixture narinfo parses");
            let key = NixSigningKey::from_secret_string(SECRET_KEY).expect("test key");
            let _ = writeln!(text, "Sig: {}", key.sign(&info.fingerprint()));
        }
        text
    }

    fn client_for(base: &str) -> CacheClient {
        CacheClient::new(
            CacheConfig {
                base_url: format!("{base}/nix/main"),
                auth: Some(Auth::Bearer("cache-token".to_string())),
                ..CacheConfig::default()
            },
            Arc::new(Metrics::new("pull-tests")),
        )
        .expect("client")
    }

    #[test]
    fn credentials_follow_the_cache_origin_only() {
        let cache = Url::parse("https://cache.example.org:8443/nix/main").expect("url");
        let same = Url::parse("https://cache.example.org:8443/nar/x.nar").expect("url");
        let other_host = Url::parse("https://cdn.example.net/nar/x.nar").expect("url");
        let other_port = Url::parse("https://cache.example.org/nar/x.nar").expect("url");
        let downgrade = Url::parse("http://cache.example.org:8443/nar/x.nar").expect("url");
        assert!(credentials_allowed(&cache, &same).expect("same origin"));
        assert!(!credentials_allowed(&cache, &other_host).expect("other host"));
        assert!(!credentials_allowed(&cache, &other_port).expect("other port"));
        assert!(
            credentials_allowed(&cache, &downgrade).is_err(),
            "an https cache must not send its NAR download over cleartext"
        );
        let plain = Url::parse("http://cache.example.org/nix/main").expect("url");
        let upgrade = Url::parse("https://cdn.example.net/nar/x.nar").expect("url");
        assert!(
            !credentials_allowed(&plain, &upgrade).expect("upgrade is fine, just unauthenticated")
        );
    }

    #[nativelink_test]
    async fn nar_on_another_origin_is_fetched_without_the_cache_credentials() {
        let (nar, nar_hash, nar_size) = fixture_nar().await;
        let nar_host = serve(HashMap::from([("/blob.nar".to_string(), (200, nar))])).await;
        let nar_url = format!("{}/blob.nar", nar_host.base);
        let cache = serve(HashMap::from([(
            format!("/nix/main/{HASH_A}.narinfo"),
            (
                200,
                narinfo_text(HASH_A, &nar_url, &nar_hash, nar_size, false).into_bytes(),
            ),
        )]))
        .await;
        let dest = tempfile::tempdir().expect("tempdir");
        let size = client_for(&cache.base)
            .pull_path(HASH_A, &TrustPolicy::Unverified, &dest.path().join(HASH_A))
            .await
            .expect("pull succeeds");
        assert_eq!(size, nar_size);
        assert_eq!(
            cache.authorization_for(&format!("/nix/main/{HASH_A}.narinfo")),
            Some("Bearer cache-token".to_string()),
            "the cache itself gets the credentials"
        );
        assert_eq!(
            nar_host.paths(),
            vec!["/blob.nar".to_string()],
            "the NAR host was asked for the NAR"
        );
        assert_eq!(
            nar_host.authorization_for("/blob.nar"),
            None,
            "a NAR host on another origin must never see the cache token"
        );
    }

    #[nativelink_test]
    async fn narinfo_for_another_store_path_is_rejected_before_any_nar_fetch() {
        let (nar, nar_hash, nar_size) = fixture_nar().await;
        let cache = serve(HashMap::from([
            (
                format!("/nix/main/{HASH_A}.narinfo"),
                (
                    200,
                    narinfo_text(HASH_B, "nar/b.nar", &nar_hash, nar_size, false).into_bytes(),
                ),
            ),
            ("/nix/main/nar/b.nar".to_string(), (200, nar)),
        ]))
        .await;
        let dest = tempfile::tempdir().expect("tempdir");
        let err = client_for(&cache.base)
            .pull_path(HASH_A, &TrustPolicy::Unverified, &dest.path().join(HASH_A))
            .await
            .expect_err("a narinfo for B must not satisfy a request for A");
        assert!(
            err.to_string().contains("different store path"),
            "unexpected error: {err}"
        );
        assert_eq!(
            cache.paths(),
            vec![format!("/nix/main/{HASH_A}.narinfo")],
            "the NAR must not be fetched for a mismatched narinfo"
        );
    }

    #[nativelink_test]
    async fn unsigned_narinfo_is_rejected_unless_verification_is_waived() {
        let (nar, nar_hash, nar_size) = fixture_nar().await;
        let cache = serve(HashMap::from([
            (
                format!("/nix/main/{HASH_A}.narinfo"),
                (
                    200,
                    narinfo_text(HASH_A, "nar/a.nar", &nar_hash, nar_size, false).into_bytes(),
                ),
            ),
            ("/nix/main/nar/a.nar".to_string(), (200, nar)),
        ]))
        .await;
        let key = NixSigningKey::from_secret_string(SECRET_KEY).expect("test key");
        let trusted = TrustPolicy::Keys(vec![
            super::NixPublicKey::from_string(&key.public_key_string()).expect("public key"),
        ]);
        let dest = tempfile::tempdir().expect("tempdir");
        let client = client_for(&cache.base);
        let err = client
            .pull_path(HASH_A, &trusted, &dest.path().join("trusted"))
            .await
            .expect_err("an unsigned narinfo must not pass a key policy");
        assert!(
            err.to_string().contains("no signature from a trusted key"),
            "unexpected error: {err}"
        );
        let err = client
            .pull_path(
                HASH_A,
                &TrustPolicy::Keys(vec![]),
                &dest.path().join("empty"),
            )
            .await
            .expect_err("an empty key list is not a policy");
        assert!(
            err.to_string().contains("at least one trusted public key"),
            "unexpected error: {err}"
        );
        client
            .pull_path(
                HASH_A,
                &TrustPolicy::Unverified,
                &dest.path().join("waived"),
            )
            .await
            .expect("an explicit waiver restores the path");
    }

    #[nativelink_test]
    async fn signed_narinfo_pulls_under_its_key() {
        let (nar, nar_hash, nar_size) = fixture_nar().await;
        let cache = serve(HashMap::from([
            (
                format!("/nix/main/{HASH_A}.narinfo"),
                (
                    200,
                    narinfo_text(HASH_A, "nar/a.nar", &nar_hash, nar_size, true).into_bytes(),
                ),
            ),
            ("/nix/main/nar/a.nar".to_string(), (200, nar)),
        ]))
        .await;
        let key = NixSigningKey::from_secret_string(SECRET_KEY).expect("test key");
        let trusted = TrustPolicy::Keys(vec![
            super::NixPublicKey::from_string(&key.public_key_string()).expect("public key"),
        ]);
        let dest = tempfile::tempdir().expect("tempdir");
        let size = client_for(&cache.base)
            .pull_path(HASH_A, &trusted, &dest.path().join(HASH_A))
            .await
            .expect("a correctly signed narinfo pulls");
        assert_eq!(size, nar_size);
        assert!(dest.path().join(HASH_A).join("hello").is_file());
    }
}
