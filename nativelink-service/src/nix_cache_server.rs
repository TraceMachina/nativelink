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

//! The Nix binary-cache (substituter) HTTP service.
//!
//! Serves the Nix HTTP binary-cache protocol (`nix-cache-info`,
//! `<hash>.narinfo`, `<hash>.ls`, `nar/<name>`, and `log/<drv>` endpoints)
//! directly from `NativeLink` stores, so `nix` clients can list a
//! `NativeLink` deployment in their `substituters` and `nix copy` can push
//! to it.
//!
//! Store layout (see `nativelink-config/examples/nix_cache.json5`):
//!
//! - `cas_store`: digest-keyed uncompressed NAR blobs under
//!   `DigestInfo(sha256(nar), nar_size)`, plus — when `serve_compression`
//!   is `zstd` — transcoded `.nar.zst` blobs under
//!   `DigestInfo(sha256(zstd(nar)), zstd_size)`.
//! - `path_info_store`: string-keyed `narinfo` records under the
//!   32-character nix32 store-path hash.
//! - `alias_store`: string-keyed map from client-chosen NAR URL basenames
//!   to the `(digest, size)` of the NAR blob in `cas_store`, plus the
//!   auxiliary documents: NAR listings under `ls:{hash}` and build logs
//!   under `log:{drv}` / `log-enc:{drv}`.

use core::pin::Pin;
use core::task::{Context as TaskContext, Poll};
use core::time::Duration;
use std::borrow::Cow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Instant;

use async_compression::Level;
use async_compression::tokio::bufread::{
    BzDecoder, GzipDecoder, XzDecoder, ZstdDecoder, ZstdEncoder,
};
use axum::Router;
use axum::body::Body;
use axum::extract::{Path as UrlPath, State};
use axum::http::header::{
    ACCEPT_RANGES, AUTHORIZATION, CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE,
    RANGE, WWW_AUTHENTICATE,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::Response;
use axum::routing::get;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use bytes::{Bytes, BytesMut};
use futures::{Stream, StreamExt, TryStreamExt};
use nativelink_config::cas_server::{NixCacheConfig, WithInstanceName};
use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_metric::MetricsComponent;
use nativelink_nix::nar_url::{
    GZIP_MAGIC, NarCodec, alias_key, canonical_nar_name, canonical_nar_zst_name, codec_for_name,
    format_alias, listing_key, log_encoding_key, log_key, parse_alias, parse_canonical_any,
    parse_canonical_nar_name,
};
use nativelink_nix::narinfo::is_store_path_hash;
use nativelink_nix::path_info::NixPathInfo;
use nativelink_nix::signing::{NixPublicKey, NixSigningKey};
use nativelink_nix::{narinfo, nixbase32};
use nativelink_store::store_manager::StoreManager;
use nativelink_util::buf_channel::{DropCloserReadHalf, make_buf_channel_pair};
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc, make_ctx_for_hash_func};
use nativelink_util::metrics_utils::{Counter, CounterWithTime};
use nativelink_util::store_trait::{
    Store, StoreKey, StoreLike, UploadSizeInfo, slow_update_store_with_file,
};
use nativelink_util::task::JoinHandleDropGuard;
use nativelink_util::{fs, spawn};
use opentelemetry::context::{Context as OtelContext, FutureExt};
use reqwest::Client as HttpClient;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio::time::timeout;
use tokio_util::io::StreamReader;
use tracing::{debug, error, warn};
use uuid::Uuid;

/// Content type of `nix-cache-info` responses.
const NIX_CACHE_INFO_CONTENT_TYPE: &str = "text/x-nix-cache-info";
/// Content type of `.narinfo` responses.
const NARINFO_CONTENT_TYPE: &str = "text/x-nix-narinfo";
/// Content type of NAR responses.
const NAR_CONTENT_TYPE: &str = "application/x-nix-nar";
/// Content type of `.ls` (NAR listing) responses.
const LISTING_CONTENT_TYPE: &str = "application/json";
/// Content type of build-log responses.
const LOG_CONTENT_TYPE: &str = "text/plain; charset=utf-8";
/// File extension of `narinfo` documents.
const NARINFO_EXTENSION: &str = ".narinfo";
/// File extension of NAR-listing documents.
const LISTING_EXTENSION: &str = ".ls";
/// Upper bound on an uploaded `narinfo` body. Real documents are a few
/// hundred bytes; anything beyond this is rejected with `413`.
const MAX_NARINFO_BODY_BYTES: usize = 1024 * 1024;
/// Upper bound on an uploaded `.ls` (NAR listing) body; beyond it: `413`.
const MAX_LISTING_BODY_BYTES: usize = 8 * 1024 * 1024;
/// Upper bound on an uploaded build-log body; beyond it: `413`.
const MAX_LOG_BODY_BYTES: usize = 32 * 1024 * 1024;
/// Upper bound on a build-log `.drv` basename length. Nix store-path names
/// are at most 211 bytes; capping here keeps the derived filesystem keys
/// (`log:{drv}` / `log-enc:{drv}`, small prefixes) inside every real
/// filesystem's per-name limit, so an over-long name is a clean `400`
/// instead of a rename `ENAMETOOLONG` surfaced as `500`.
const MAX_LOG_NAME_LEN: usize = 240;
/// Directory name under the system temp dir used when `spool_path` is not
/// configured.
const DEFAULT_SPOOL_DIR_NAME: &str = "nativelink-nix-spool";
/// Filename prefix for every NAR spool file. Startup pruning removes ONLY
/// files carrying this prefix, so pointing `spool_path` at a shared or
/// already-populated directory can never delete unrelated files.
const SPOOL_FILE_PREFIX: &str = "nativelink-nix-spool-";
/// The `Compression`/`file_compression` name of the zstd codec.
const ZSTD_COMPRESSION_NAME: &str = "zstd";
/// Default zstd level when `serve_compression` is `"zstd"` and no
/// `compression_level` is configured (zstd's own default).
const DEFAULT_ZSTD_LEVEL: i32 = 3;
/// zstd's valid compression-level range.
const ZSTD_MIN_LEVEL: i32 = 1;
const ZSTD_MAX_LEVEL: i32 = 22;
/// The challenge attached to every `401`: nix/curl answer a `Basic`
/// challenge from their netrc, and nix treats a `401` on a `narinfo`
/// fetch as a clean miss — exactly the hiding a private cache wants.
const WWW_AUTHENTICATE_CHALLENGE: &str = "Basic realm=\"nix-cache\"";

/// Builds an [`opentelemetry`] context that pins the digest function to
/// SHA256. This is the single choke point for digest-function hygiene:
/// every store call in this module runs under this context. NAR digests
/// are always `sha256(nar)`, and with `require_explicit_digest_function`
/// plus a BLAKE3-leaning global default, a missed context would be a
/// silent-wrong-algorithm bug.
fn sha256_hasher_ctx() -> Result<OtelContext, Error> {
    make_ctx_for_hash_func(DigestHasherFunc::Sha256)
        .err_tip(|| "Making SHA256 hasher context in NixCacheServer")
}

/// Runs a store future under the SHA256 hasher context; see
/// [`sha256_hasher_ctx`].
async fn with_sha256_ctx<F, T>(fut: F) -> Result<T, Error>
where
    F: Future<Output = Result<T, Error>> + Send,
{
    fut.with_context(sha256_hasher_ctx()?).await
}

/// The single error-to-HTTP-status mapping for this service.
///
/// Absent content maps to `404` and never to a `5xx`: any `5xx` makes nix
/// retry five times and then disable the substituter for 60 seconds.
/// Backend failures must never map to `404` either — honesty both ways.
fn error_response(context: &'static str, err: &Error) -> Response {
    match err.code {
        Code::NotFound => empty_response(StatusCode::NOT_FOUND),
        // Client-caused statuses return a concise, stable message and keep
        // the full error — store-layer breadcrumbs, internal digests, and
        // the `Error { .. }` Debug wrapper — in the server log instead of
        // echoing it into the response body.
        Code::InvalidArgument => {
            debug!(?err, context, "Nix cache client error");
            text_response(StatusCode::BAD_REQUEST, "bad request")
        }
        Code::ResourceExhausted => {
            debug!(?err, context, "Nix cache payload too large");
            text_response(StatusCode::PAYLOAD_TOO_LARGE, "payload too large")
        }
        Code::DeadlineExceeded => {
            debug!(?err, context, "Nix cache request timed out");
            text_response(StatusCode::REQUEST_TIMEOUT, "request timeout")
        }
        _ => {
            error!(?err, context, "Nix cache request failed");
            text_response(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        }
    }
}

fn empty_response(status: StatusCode) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    response
}

fn text_response(status: StatusCode, body: &str) -> Response {
    let mut response = Response::new(Body::from(body.to_owned()));
    *response.status_mut() = status;
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response
}

/// Validates a `<32-char nix32 hash>.narinfo` request file name, returning
/// the store-path hash. Anything else is not a `narinfo` route.
fn narinfo_name_hash(file: &str) -> Option<&str> {
    let hash = file.strip_suffix(NARINFO_EXTENSION)?;
    is_store_path_hash(hash).then_some(hash)
}

/// Validates a `<32-char nix32 hash>.ls` request file name, returning the
/// store-path hash. Anything else is not a listing route.
fn listing_name_hash(file: &str) -> Option<&str> {
    let hash = file.strip_suffix(LISTING_EXTENSION)?;
    is_store_path_hash(hash).then_some(hash)
}

/// Validates a build-log `.drv` basename: non-empty, at most
/// [`MAX_LOG_NAME_LEN`] bytes, printable ASCII, no `/`, and no `..`. The
/// axum capture is percent-decoded, so a crafted `%2F`/`%2E%2E` would
/// otherwise smuggle separators into store keys, and an unbounded length
/// would overflow the derived filesystem key.
fn is_valid_log_name(drv: &str) -> bool {
    !drv.is_empty()
        && drv.len() <= MAX_LOG_NAME_LEN
        && !drv.contains("..")
        && drv.bytes().all(|b| b.is_ascii_graphic() && b != b'/')
}

/// Whether a client-supplied build-log `Content-Encoding` is one this cache
/// will store and replay verbatim to later readers. Restricting to a known
/// set stops an uploader from injecting an arbitrary `Content-Encoding`
/// response header into a shared cached log that every reader then tries to
/// decode. The value is a (possibly comma-separated) list of codec tokens.
fn is_allowed_log_encoding(encoding: &str) -> bool {
    encoding.split(',').map(str::trim).all(|token| {
        matches!(
            token.to_ascii_lowercase().as_str(),
            "" | "identity" | "gzip" | "x-gzip" | "deflate" | "br" | "zstd" | "xz" | "bzip2"
        )
    })
}

/// Returns `sha256(data)`. Token comparisons happen in hashed space, so
/// both sides of every comparison have the same length by construction.
fn sha256_of(data: &[u8]) -> [u8; 32] {
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(data);
    let digest = hasher.finalize_digest();
    let hash: &[u8; 32] = digest.packed_hash();
    *hash
}

/// Constant-time equality: after the length gate, every byte pair's XOR
/// is folded into a single accumulator with `|=`, so the cost never
/// depends on where (or whether) the slices differ. Callers compare
/// sha256 digests, which makes the length gate vacuous.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc = 0_u8;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}

/// Whether a presented token hash matches any candidate hash. Every
/// candidate is compared — no early exit — via [`constant_time_eq`].
fn token_matches_any(presented: &[u8; 32], candidates: &[[u8; 32]]) -> bool {
    let mut found = false;
    for candidate in candidates {
        found |= constant_time_eq(presented, candidate);
    }
    found
}

/// Extracts the presented token from the `Authorization` header:
/// `Bearer <token>`, or `Basic <base64(user:password)>` where the token
/// is the PASSWORD and the username is ignored — the latter is how stock
/// nix authenticates against a binary cache via `netrc`.
fn extract_token(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, rest) = value.trim().split_once(' ')?;
    let rest = rest.trim();
    if scheme.eq_ignore_ascii_case("bearer") {
        return (!rest.is_empty()).then(|| rest.to_string());
    }
    if scheme.eq_ignore_ascii_case("basic") {
        let decoded = BASE64.decode(rest).ok()?;
        let text = String::from_utf8(decoded).ok()?;
        let (_user, password) = text.split_once(':')?;
        return Some(password.to_string());
    }
    None
}

/// Loads token files — each holds exactly ONE token, trimmed; multiple
/// files enable rotation — failing fast on unreadable files or empty
/// tokens, and returns the sha256 of each token (see [`sha256_of`]).
fn load_token_hashes(files: &[String], what: &str) -> Result<Vec<[u8; 32]>, Error> {
    let mut hashes = Vec::with_capacity(files.len());
    for file in files {
        let contents = std::fs::read_to_string(file)
            .err_tip(|| format!("Failed to read Nix cache {what} token file '{file}'"))?;
        let token = contents.trim();
        if token.is_empty() {
            return Err(make_input_err!(
                "Nix cache {what} token file '{file}' holds no token"
            ));
        }
        hashes.push(sha256_of(token.as_bytes()));
    }
    Ok(hashes)
}

/// The `401` served for missing or invalid tokens, carrying the `Basic`
/// challenge from [`WWW_AUTHENTICATE_CHALLENGE`].
fn unauthorized_response() -> Response {
    let mut response = empty_response(StatusCode::UNAUTHORIZED);
    response.headers_mut().insert(
        WWW_AUTHENTICATE,
        HeaderValue::from_static(WWW_AUTHENTICATE_CHALLENGE),
    );
    response
}

/// A parsed `Range` request header, relative to a body of `total` bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RangeRequest {
    /// No `Range` header, or one this service ignores (multi-range,
    /// suffix-range, or malformed): serve `200` with the full body.
    Full,
    /// Serve `start..=end`, guaranteed within `0..total`.
    Range { start: u64, end: u64 },
    /// `start >= total`: serve `416`.
    Unsatisfiable,
}

/// Parses a `Range` header, accepting only the single-range forms
/// `bytes=start-` and `bytes=start-end` (`end` inclusive, clamped to the
/// body). Anything else falls back to serving the full body.
fn parse_range(header: Option<&str>, total: u64) -> RangeRequest {
    let Some(header) = header else {
        return RangeRequest::Full;
    };
    let Some(spec) = header.strip_prefix("bytes=") else {
        return RangeRequest::Full;
    };
    // Multi-range requests are ignored: serve the full body instead.
    if spec.contains(',') {
        return RangeRequest::Full;
    }
    let Some((start_str, end_str)) = spec.trim().split_once('-') else {
        return RangeRequest::Full;
    };
    // Suffix ranges ("bytes=-N") are ignored.
    if start_str.is_empty() {
        return RangeRequest::Full;
    }
    let Ok(start) = start_str.parse::<u64>() else {
        return RangeRequest::Full;
    };
    let end = if end_str.is_empty() {
        total.saturating_sub(1)
    } else {
        match end_str.parse::<u64>() {
            Ok(end) => end.min(total.saturating_sub(1)),
            Err(_) => return RangeRequest::Full,
        }
    };
    if start >= total {
        return RangeRequest::Unsatisfiable;
    }
    if end < start {
        return RangeRequest::Full;
    }
    RangeRequest::Range { start, end }
}

/// Returns the value of the `Content-Length` header, if present and valid.
fn content_length_of(headers: &HeaderMap) -> Option<u64> {
    headers.get(CONTENT_LENGTH)?.to_str().ok()?.parse().ok()
}

/// Accepts only NAR upload names nix's uploader can produce: a stem of 52
/// nix32 characters (the client-side file hash) or the canonical
/// `{nix32(hash)}-{size}` form, followed by a supported extension.
/// Anything else — including traversal-shaped names — is rejected before
/// any body bytes are read and before anything is written.
fn is_valid_nar_upload_name(name: &str) -> bool {
    let Some(stem) = name
        .strip_suffix(".nar")
        .or_else(|| name.strip_suffix(".nar.xz"))
        .or_else(|| name.strip_suffix(".nar.zst"))
        .or_else(|| name.strip_suffix(".nar.bz2"))
    else {
        return false;
    };
    let hash_part = match stem.split_once('-') {
        Some((hash_part, size_str)) => {
            if size_str.is_empty() || !size_str.bytes().all(|b| b.is_ascii_digit()) {
                return false;
            }
            hash_part
        }
        None => stem,
    };
    hash_part.len() == 52 && nixbase32::decode(hash_part).is_ok()
}

/// Derives the CAS digest of a bare `.nar` upload from its client-chosen
/// name.
///
/// Canonical `{nix32(hash)}-{size}.nar` names carry both halves of the
/// digest. Nix's own uploader names identity-compressed NARs
/// `{nix32(sha256(payload))}.nar` — with identity compression that
/// 52-character file hash IS `sha256(nar)`, so together with the request's
/// `Content-Length` the digest is known before reading the body.
fn bare_nar_digest_from_name(name: &str, content_length: Option<u64>) -> Option<([u8; 32], u64)> {
    if let Some(resolved) = parse_canonical_nar_name(name) {
        return Some(resolved);
    }
    let stem = name.strip_suffix(".nar")?;
    if stem.len() != 52 {
        return None;
    }
    let nar_sha256: [u8; 32] = nixbase32::decode(stem).ok()?.try_into().ok()?;
    Some((nar_sha256, content_length?))
}

/// Returns the 32-byte digest a bare `{52 nix32}.nar` name commits to in
/// its stem, or `None` for any other name (a canonical
/// `{hash}-{size}.nar`, a compressed name, ...). Unlike
/// [`bare_nar_digest_from_name`] this needs no `Content-Length` and never
/// matches canonical or compressed names, so it isolates exactly the
/// bare-identity case where the stem must equal `sha256(content)`.
fn bare_nar_stem_sha256(name: &str) -> Option<[u8; 32]> {
    let stem = name.strip_suffix(".nar")?;
    if stem.len() != 52 {
        return None;
    }
    nixbase32::decode(stem).ok()?.try_into().ok()
}

/// Derives the `(sha256, size)` of the ORIGINAL compressed blob a
/// `.nar.{ext}` upload names, for preserving it verbatim.
///
/// The 52-nix32 stem hash is `sha256(compressed bytes)` — the client's
/// `FileHash`, which nix names the compressed NAR after. The size is the
/// name's `-{size}` when present (a canonical compressed name), else the
/// request `Content-Length`. Returns `None` when the size is unknown (a
/// bare name with no `Content-Length`), so the caller falls back to the
/// non-preserving spooled path rather than buffering to learn it.
fn compressed_upload_digest(
    name: &str,
    name_codec: NarCodec,
    content_length: Option<u64>,
) -> Option<([u8; 32], u64)> {
    let ext = match name_codec {
        NarCodec::Xz => ".nar.xz",
        NarCodec::Zstd => ".nar.zst",
        NarCodec::Bzip2 => ".nar.bz2",
        NarCodec::None | NarCodec::Gzip => return None,
    };
    let stem = name.strip_suffix(ext)?;
    let (hash_part, size) = match stem.split_once('-') {
        Some((hash_part, size_str)) => {
            // Canonical decimal only: no empty, no leading zeros, in range.
            if size_str.is_empty()
                || !size_str.bytes().all(|b| b.is_ascii_digit())
                || (size_str.len() > 1 && size_str.starts_with('0'))
            {
                return None;
            }
            (hash_part, size_str.parse::<u64>().ok()?)
        }
        None => (stem, content_length?),
    };
    if hash_part.len() != 52 {
        return None;
    }
    let sha256: [u8; 32] = nixbase32::decode(hash_part).ok()?.try_into().ok()?;
    Some((sha256, size))
}

/// Reads a request body to completion, buffering at most `limit` bytes.
/// Returns `None` when the body exceeds the limit.
async fn read_body_limited(body: Body, limit: usize) -> Result<Option<Bytes>, Error> {
    let mut stream = body.into_data_stream();
    let mut buffer = BytesMut::new();
    while let Some(chunk) = stream.next().await {
        // A truncated or malformed client body is a client error (400),
        // not a backend failure (500).
        let chunk = chunk.map_err(|e| make_input_err!("Failed to read request body: {e}"))?;
        if buffer.len() + chunk.len() > limit {
            return Ok(None);
        }
        buffer.extend_from_slice(&chunk);
    }
    Ok(Some(buffer.freeze()))
}

/// Adapter from the buf-channel read half to an HTTP response body stream.
///
/// The read half's [`Stream`] impl already carries the protocol this
/// service needs: an empty-`Bytes` recv is a clean EOF (stream ends), and
/// a writer dropped without `send_eof` surfaces as an `Err`, which makes
/// hyper abort the response mid-body — so a client retries instead of
/// trusting truncated bytes. Holding the [`JoinHandleDropGuard`] keeps the
/// producer task alive exactly as long as the body, and aborts it if the
/// client disconnects early.
#[derive(Debug)]
struct NarBodyStream {
    rx: DropCloserReadHalf,
    _task: JoinHandleDropGuard<()>,
    /// Held for the body's entire lifetime so the concurrent-NAR-stream
    /// permit is released only once the response body is fully drained or
    /// the client disconnects (which drops this stream and aborts the
    /// producer task).
    _permit: OwnedSemaphorePermit,
}

impl Stream for NarBodyStream {
    type Item = Result<Bytes, std::io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.rx).poll_next(cx)
    }
}

/// Removes a NAR spool file on drop unless disarmed via
/// [`SpoolFileGuard::cleanup`], so no spool files leak on error paths.
#[derive(Debug)]
struct SpoolFileGuard {
    path: Option<PathBuf>,
}

impl SpoolFileGuard {
    const fn new(path: PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// Removes the spool file eagerly and disarms the drop cleanup.
    async fn cleanup(mut self) {
        if let Some(path) = self.path.take()
            && let Err(err) = tokio::fs::remove_file(&path).await
            && err.kind() != std::io::ErrorKind::NotFound
        {
            warn!(?err, path = %path.display(), "Failed to remove NAR spool file");
        }
    }
}

impl Drop for SpoolFileGuard {
    fn drop(&mut self) {
        if let Some(path) = self.path.take()
            && let Err(err) = std::fs::remove_file(&path)
            && err.kind() != std::io::ErrorKind::NotFound
        {
            warn!(?err, path = %path.display(), "Failed to remove NAR spool file");
        }
    }
}

/// Creates the spool directory and prunes files left over from a previous
/// run (for example after a crash mid-upload).
fn prepare_spool_dir(spool_dir: &Path) -> Result<(), Error> {
    std::fs::create_dir_all(spool_dir)
        .err_tip(|| format!("Failed to create spool dir {}", spool_dir.display()))?;
    let entries = std::fs::read_dir(spool_dir)
        .err_tip(|| format!("Failed to read spool dir {}", spool_dir.display()))?;
    for entry in entries {
        let entry =
            entry.err_tip(|| format!("Failed to list spool dir {}", spool_dir.display()))?;
        let file_type = entry
            .file_type()
            .err_tip(|| format!("Failed to stat spool entry {}", entry.path().display()))?;
        // Only prune files we know we created: a `spool_path` misconfigured
        // to a shared/populated directory must never cost unrelated data.
        let is_spool_file = entry.file_name().to_str().is_some_and(|name| {
            name.starts_with(SPOOL_FILE_PREFIX)
                && Path::new(name)
                    .extension()
                    .and_then(std::ffi::OsStr::to_str)
                    == Some("nar")
        });
        if file_type.is_file()
            && is_spool_file
            && let Err(err) = std::fs::remove_file(entry.path())
        {
            warn!(?err, path = %entry.path().display(), "Failed to prune stale NAR spool file");
        }
    }
    Ok(())
}

/// The compression this instance serves NARs with, parsed once at
/// startup from the `serve_compression` config string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServeCompression {
    /// Phase-1 behavior: serve the uncompressed NAR under its canonical
    /// `.nar` name.
    None,
    /// Transcode once at `narinfo` upload time and serve the `.nar.zst`
    /// artifact, falling back to the uncompressed NAR if it is evicted.
    Zstd,
}

/// The result of a NAR-to-zstd transcode: `(sha256(zstd(nar)), zstd_size)`.
type TranscodeResult = Result<([u8; 32], u64), Error>;

/// A single in-flight zstd transcode that concurrent narinfo PUTs for the
/// same NAR coalesce onto: the leader runs the transcode, stores the
/// result, and wakes every follower waiting on `notify`. Keyed on the NAR
/// digest, so N distinct store paths that serialize to one NAR trigger
/// exactly one transcode instead of N.
#[derive(Debug)]
struct TranscodeInflight {
    /// Set once by the leader before `notify.notify_waiters()`.
    result: StdMutex<Option<TranscodeResult>>,
    notify: Notify,
}

/// One resolved upstream Nix binary cache for the read-through feature:
/// the normalized base URL (no trailing slash) and the parsed public keys
/// a fetched `narinfo` must verify against before it is cached.
#[derive(Debug)]
struct UpstreamCache {
    base_url: String,
    public_keys: Vec<NixPublicKey>,
}

/// The outcome of a read-through attempt: `Some(record)` when an upstream
/// had the path (and it was verified and ingested), `None` when no
/// upstream had it. `Error` is a hard local failure (e.g. a store write
/// failing); transport/upstream faults are folded into `Ok(None)` so a
/// flaky upstream never turns a lookup into a 5xx.
type ReadThroughResult = Result<Option<Bytes>, Error>;

/// The aggregate outcome of probing every upstream for one store-path
/// hash. Distinguishes a proven absence from an unproven one so only the
/// former is negative-cached: caching an [`Indeterminate`] miss would turn
/// a transient upstream outage into a `404` served for the whole
/// `upstream_negative_ttl`, hiding a path that actually exists upstream.
///
/// [`Indeterminate`]: UpstreamOutcome::Indeterminate
enum UpstreamOutcome {
    /// An upstream had the path; carries the stored narinfo record bytes.
    Hit(Bytes),
    /// Every upstream was reachable and definitively lacked the path
    /// (e.g. a `404`). Safe to negative-cache.
    DefinitiveMiss,
    /// No upstream returned the path, but at least one failed transiently
    /// (timeout, 5xx, transport error, a post-download NAR hash/size
    /// mismatch, or a local ingest error), so absence is unproven — served
    /// as a miss but never negative-cached.
    Indeterminate,
}

/// A single in-flight read-through that concurrent `narinfo` GETs for the
/// same store-path hash coalesce onto — the leader fetches from upstream,
/// stores the record, and wakes every follower with the result. Mirrors
/// [`TranscodeInflight`]; keyed on the 32-char store-path hash so a burst
/// of identical misses triggers exactly one upstream fetch.
#[derive(Debug)]
struct ReadThroughInflight {
    result: StdMutex<Option<ReadThroughResult>>,
    notify: Notify,
}

/// Maps a `narinfo` `Compression:` value to the decoder used to ingest the
/// upstream NAR. `None`/`"none"` is an uncompressed NAR. Returns `None`
/// for codecs this service cannot decode (e.g. `br`), so such a path is
/// treated as an upstream miss rather than a hard error.
fn codec_for_compression(compression: &str) -> Option<NarCodec> {
    match compression {
        "none" | "" => Some(NarCodec::None),
        "xz" => Some(NarCodec::Xz),
        "zstd" => Some(NarCodec::Zstd),
        "bzip2" => Some(NarCodec::Bzip2),
        "gzip" => Some(NarCodec::Gzip),
        _ => None,
    }
}

/// Joins an upstream base URL with the `URL:` field of a fetched `narinfo`.
/// Nix `narinfo` URLs are relative (`nar/<name>.nar.xz`), but tolerate an
/// absolute URL just in case.
fn join_upstream_url(base_url: &str, narinfo_url: &str) -> String {
    if narinfo_url.starts_with("http://") || narinfo_url.starts_with("https://") {
        narinfo_url.to_string()
    } else {
        format!("{base_url}/{}", narinfo_url.trim_start_matches('/'))
    }
}

/// A read-through negative cache never grows past this many entries: once
/// reached, expired entries are swept before the next insert. Bounds
/// memory when a client probes a very large space of non-existent paths.
const NEGATIVE_CACHE_MAX_ENTRIES: usize = 50_000;

/// One configured Nix cache instance: resolved stores, identity, and
/// counters. Shared with every request handler through the router state.
#[derive(Debug, MetricsComponent)]
pub struct NixCacheInstance {
    #[metric(help = "The configured instance name of this Nix cache")]
    instance_name: String,
    mount_path: String,
    cas_store: Store,
    path_info_store: Store,
    alias_store: Store,
    signing_keys: Vec<NixSigningKey>,
    store_dir: String,
    priority: u32,
    want_mass_query: bool,
    read_only: bool,
    spool_dir: PathBuf,
    /// sha256 of each configured read token; empty means anonymous reads.
    read_token_hashes: Vec<[u8; 32]>,
    /// sha256 of each configured write token; empty means writes are
    /// gated only by `read_only`.
    write_token_hashes: Vec<[u8; 32]>,
    serve_compression: ServeCompression,
    compression_level: i32,
    /// When true, a compressed NAR upload's original bytes are preserved
    /// as a CAS blob and served verbatim under the client's URL (see
    /// `preserve_upload_compression` in the config); when false, uploads
    /// are decompressed to the canonical uncompressed NAR.
    preserve_upload_compression: bool,
    /// Cap on the decompressed size of a single ingested NAR (see the
    /// `max_nar_size_bytes` config field); bounds the spool a compressed
    /// upload can write.
    max_nar_size_bytes: u64,
    /// Idle timeout on a NAR upload body; a stall past this is aborted with
    /// `408` and the spool file/descriptor released.
    nar_upload_idle_timeout: Duration,
    /// Bounds concurrent NAR GET response streams; see
    /// [`Self::acquire_nar_stream_permit`].
    nar_stream_semaphore: Arc<Semaphore>,
    /// Bounds concurrent zstd transcodes.
    transcode_semaphore: Arc<Semaphore>,
    /// Coalesces concurrent transcodes of the same NAR into one, keyed on
    /// the NAR digest.
    transcode_inflight: StdMutex<HashMap<DigestInfo, Arc<TranscodeInflight>>>,

    /// Upstream caches to read through on a local `narinfo` miss; empty
    /// disables read-through. See [`Self::read_through`].
    upstreams: Vec<UpstreamCache>,
    /// Shared outbound HTTP client for upstream fetches.
    http_client: HttpClient,
    /// Timeout for a single upstream `narinfo` probe.
    upstream_timeout: Duration,
    /// How long a "not found in any upstream" result is remembered.
    upstream_negative_ttl: Duration,
    /// store-path hash -> expiry instant for negatively-cached misses.
    negative_cache: StdMutex<HashMap<String, Instant>>,
    /// Coalesces concurrent read-throughs of the same store-path hash.
    readthrough_inflight: StdMutex<HashMap<String, Arc<ReadThroughInflight>>>,

    #[metric(help = "Number of narinfo GET requests")]
    narinfo_gets: CounterWithTime,
    #[metric(help = "Number of accepted narinfo PUT requests")]
    narinfo_puts: CounterWithTime,
    #[metric(help = "Number of NAR GET requests")]
    nar_gets: CounterWithTime,
    #[metric(help = "Number of accepted NAR PUT requests")]
    nar_puts: CounterWithTime,
    #[metric(help = "Number of NAR bytes served to clients")]
    nar_bytes_served: Counter,
    #[metric(help = "Number of uncompressed NAR bytes ingested into the CAS")]
    nar_bytes_ingested: Counter,
    #[metric(help = "Number of listing (.ls) and build-log GET requests")]
    aux_gets: CounterWithTime,
    #[metric(help = "Number of accepted listing (.ls) and build-log PUT requests")]
    aux_puts: CounterWithTime,
    #[metric(help = "Number of rejected PUT requests (narinfo, NAR, listing, and log)")]
    rejected_puts: CounterWithTime,
    #[metric(help = "Number of requests rejected with 401 for missing or invalid tokens")]
    unauthorized_requests: CounterWithTime,
    #[metric(help = "Number of narinfo misses served by fetching from an upstream cache")]
    upstream_hits: CounterWithTime,
    #[metric(help = "Number of narinfo misses no configured upstream cache had")]
    upstream_misses: CounterWithTime,
    #[metric(
        help = "Number of upstream read-through attempts that failed with a transport or upstream error"
    )]
    upstream_errors: CounterWithTime,
    #[metric(
        help = "Number of upstream narinfo documents refused for failing signature verification"
    )]
    upstream_rejected_unsigned: CounterWithTime,
    #[metric(help = "Number of NAR bytes ingested from upstream caches via read-through")]
    upstream_nar_bytes: Counter,
}

impl NixCacheInstance {
    fn new(
        config: &WithInstanceName<NixCacheConfig>,
        store_manager: &StoreManager,
    ) -> Result<Self, Error> {
        let cas_store = store_manager
            .get_store(&config.cas_store)
            .ok_or_else(|| make_input_err!("'cas_store': '{}' does not exist", config.cas_store))?;
        let path_info_store = store_manager
            .get_store(&config.path_info_store)
            .ok_or_else(|| {
                make_input_err!(
                    "'path_info_store': '{}' does not exist",
                    config.path_info_store
                )
            })?;
        let alias_store = store_manager
            .get_store(&config.alias_store)
            .ok_or_else(|| {
                make_input_err!("'alias_store': '{}' does not exist", config.alias_store)
            })?;

        let mut signing_keys = Vec::with_capacity(config.signing_key_files.len());
        for key_file in &config.signing_key_files {
            let contents = std::fs::read_to_string(key_file)
                .err_tip(|| format!("Failed to read Nix signing key file '{key_file}'"))?;
            let key = NixSigningKey::from_secret_string(contents.trim())
                .err_tip(|| format!("In Nix signing key file '{key_file}'"))?;
            signing_keys.push(key);
        }

        let mut mount_path = config
            .path
            .clone()
            .unwrap_or_else(|| format!("/nix/{}", config.instance_name));
        while mount_path.len() > 1 && mount_path.ends_with('/') {
            mount_path.pop();
        }
        if !mount_path.starts_with('/') || mount_path.len() < 2 {
            return Err(make_input_err!(
                "'path' must start with '/' and must not be the root, got '{mount_path}'"
            ));
        }

        let spool_dir = config.spool_path.as_ref().map_or_else(
            || {
                std::env::temp_dir()
                    .join(DEFAULT_SPOOL_DIR_NAME)
                    .join(&config.instance_name)
            },
            PathBuf::from,
        );
        prepare_spool_dir(&spool_dir).err_tip(|| {
            format!(
                "Preparing NAR spool dir for nix_cache instance '{}'",
                config.instance_name
            )
        })?;

        let read_token_hashes = load_token_hashes(&config.read_token_files, "read")?;
        let write_token_hashes = load_token_hashes(&config.write_token_files, "write")?;

        let serve_compression = match config.serve_compression.as_deref() {
            None | Some("none") => ServeCompression::None,
            Some(ZSTD_COMPRESSION_NAME) => ServeCompression::Zstd,
            Some(other) => {
                return Err(make_input_err!(
                    "'serve_compression' must be \"none\" or \"zstd\", got '{other}'"
                ));
            }
        };

        // zstd silently clamps an out-of-range level; refuse it at boot so a
        // typo in `compression_level` is a visible configuration error.
        if serve_compression == ServeCompression::Zstd
            && let Some(level) = config.compression_level
            && !(ZSTD_MIN_LEVEL..=ZSTD_MAX_LEVEL).contains(&level)
        {
            return Err(make_input_err!(
                "'compression_level' {level} is outside zstd's valid range {ZSTD_MIN_LEVEL}..={ZSTD_MAX_LEVEL}"
            ));
        }

        // Clamp the concurrency ceilings to at least one permit: a zero
        // would deadlock every stream/transcode, a self-inflicted DoS.
        let max_concurrent_nar_streams = config.max_concurrent_nar_streams.max(1);
        let max_concurrent_transcodes = config.max_concurrent_transcodes.max(1);

        // Resolve upstream caches for read-through. Each upstream must
        // list at least one trusted public key: read-through only caches a
        // path whose upstream narinfo verifies, so an upstream with no keys
        // could never contribute and is a configuration error caught here.
        let mut upstreams = Vec::with_capacity(config.upstream_caches.len());
        for upstream in &config.upstream_caches {
            let base_url = upstream.url.trim_end_matches('/').to_string();
            if base_url.is_empty() {
                return Err(make_input_err!(
                    "'upstream_caches' entry has an empty 'url'"
                ));
            }
            if upstream.trusted_public_keys.is_empty() {
                return Err(make_input_err!(
                    "'upstream_caches' entry '{base_url}' has no 'trusted_public_keys'; \
                     read-through requires at least one key to verify fetched narinfo against"
                ));
            }
            let mut public_keys = Vec::with_capacity(upstream.trusted_public_keys.len());
            for key in &upstream.trusted_public_keys {
                public_keys
                    .push(NixPublicKey::from_string(key).err_tip(|| {
                        format!("In 'trusted_public_keys' of upstream '{base_url}'")
                    })?);
            }
            upstreams.push(UpstreamCache {
                base_url,
                public_keys,
            });
        }
        // reqwest is built with `rustls-no-provider`, like the object-store
        // backends, so a crypto provider has to be installed before the first
        // client exists. Installing twice is harmless and the result is
        // deliberately ignored.
        drop(rustls::crypto::ring::default_provider().install_default());
        let http_client = HttpClient::builder()
            .user_agent(concat!("nativelink-nix-cache/", env!("CARGO_PKG_VERSION")))
            // Cap connection establishment and, crucially, the idle time
            // between response data frames. The read (idle) timeout stops a
            // stalled upstream NAR download from hanging the read-through
            // leader — and every follower coalesced onto it — forever,
            // without capping the total duration of a large but healthy
            // download the way a per-request total `.timeout()` would.
            .connect_timeout(Duration::from_secs(config.upstream_timeout_s))
            .read_timeout(Duration::from_secs(config.upstream_timeout_s))
            .build()
            .map_err(|e| make_err!(Code::Internal, "Failed to build upstream HTTP client: {e}"))?;

        Ok(Self {
            instance_name: config.instance_name.clone(),
            mount_path,
            cas_store,
            path_info_store,
            alias_store,
            signing_keys,
            store_dir: config.store_dir.clone(),
            priority: config.priority,
            want_mass_query: config.want_mass_query,
            read_only: config.read_only,
            spool_dir,
            read_token_hashes,
            write_token_hashes,
            serve_compression,
            compression_level: config.compression_level.unwrap_or(DEFAULT_ZSTD_LEVEL),
            preserve_upload_compression: config.preserve_upload_compression,
            max_nar_size_bytes: config.max_nar_size_bytes,
            nar_upload_idle_timeout: Duration::from_secs(config.nar_upload_idle_timeout_s),
            nar_stream_semaphore: Arc::new(Semaphore::new(max_concurrent_nar_streams)),
            transcode_semaphore: Arc::new(Semaphore::new(max_concurrent_transcodes)),
            transcode_inflight: StdMutex::new(HashMap::new()),
            upstreams,
            http_client,
            upstream_timeout: Duration::from_secs(config.upstream_timeout_s),
            upstream_negative_ttl: Duration::from_secs(config.upstream_negative_ttl_s),
            negative_cache: StdMutex::new(HashMap::new()),
            readthrough_inflight: StdMutex::new(HashMap::new()),
            narinfo_gets: CounterWithTime::default(),
            narinfo_puts: CounterWithTime::default(),
            nar_gets: CounterWithTime::default(),
            nar_puts: CounterWithTime::default(),
            nar_bytes_served: Counter::default(),
            nar_bytes_ingested: Counter::default(),
            aux_gets: CounterWithTime::default(),
            aux_puts: CounterWithTime::default(),
            rejected_puts: CounterWithTime::default(),
            unauthorized_requests: CounterWithTime::default(),
            upstream_hits: CounterWithTime::default(),
            upstream_misses: CounterWithTime::default(),
            upstream_errors: CounterWithTime::default(),
            upstream_rejected_unsigned: CounterWithTime::default(),
            upstream_nar_bytes: Counter::default(),
        })
    }

    /// Gates reads. When `read_token_files` is configured, EVERY request
    /// on the instance (including `nix-cache-info`) requires a valid
    /// read OR write token; returns the `401` (with the `Basic`
    /// challenge) to serve otherwise.
    fn authorize_read(&self, headers: &HeaderMap) -> Option<Response> {
        if self.read_token_hashes.is_empty() {
            return None;
        }
        let presented = extract_token(headers).map(|token| sha256_of(token.as_bytes()));
        let authorized = presented.is_some_and(|hash| {
            token_matches_any(&hash, &self.read_token_hashes)
                || token_matches_any(&hash, &self.write_token_hashes)
        });
        if authorized {
            None
        } else {
            self.unauthorized_requests.inc();
            Some(unauthorized_response())
        }
    }

    /// Gates writes. Read authorization runs first. Then, whenever ANY
    /// token auth is configured on the instance, a write requires a valid
    /// WRITE token — a read token alone is never sufficient. In particular
    /// if reads are gated but `write_token_files` is empty, writes are
    /// denied outright (fail closed) rather than silently granting write
    /// access to every read-token holder; point `write_token_files` at the
    /// same files as `read_token_files` to intentionally allow that. Only a
    /// cache with NO tokens at all leaves writes open (trusted-network
    /// mode), gated solely by `read_only`, which the PUT handlers enforce
    /// separately and which wins over any valid write token.
    fn authorize_write(&self, headers: &HeaderMap) -> Option<Response> {
        if let Some(denied) = self.authorize_read(headers) {
            return Some(denied);
        }
        // No token auth configured at all: open cache for trusted networks.
        if self.read_token_hashes.is_empty() && self.write_token_hashes.is_empty() {
            return None;
        }
        // Otherwise a valid write token is mandatory. When no write tokens
        // are configured, `token_matches_any` against an empty set is always
        // false, so writes fail closed until write tokens are provisioned.
        let presented = extract_token(headers).map(|token| sha256_of(token.as_bytes()));
        let authorized =
            presented.is_some_and(|hash| token_matches_any(&hash, &self.write_token_hashes));
        if authorized {
            None
        } else {
            self.unauthorized_requests.inc();
            Some(unauthorized_response())
        }
    }

    /// Looks up an alias record for `url_basename`, returning the
    /// `(sha256, size)` of the NAR it names, or `None` when no alias
    /// exists. Corrupt alias records are internal errors, not 404s.
    async fn lookup_alias(&self, url_basename: &str) -> Result<Option<([u8; 32], u64)>, Error> {
        let key = alias_key(url_basename);
        let lookup = with_sha256_ctx(self.alias_store.get_part_unchunked(
            StoreKey::Str(Cow::Owned(key)),
            0,
            None,
        ))
        .await;
        let raw = match lookup {
            Ok(raw) => raw,
            Err(err) if err.code == Code::NotFound => return Ok(None),
            Err(err) => {
                return Err(err).err_tip(|| format!("Looking up NAR alias for '{url_basename}'"));
            }
        };
        let value = core::str::from_utf8(&raw).map_err(|e| {
            make_err!(
                Code::Internal,
                "Corrupt NAR alias record for '{url_basename}': {e}"
            )
        })?;
        match parse_alias(value) {
            Ok(resolved) => Ok(Some(resolved)),
            Err(err) => Err(make_err!(
                Code::Internal,
                "Corrupt NAR alias record for '{url_basename}': {err}"
            )),
        }
    }

    /// Tries to reserve one of the bounded concurrent-NAR-stream permits,
    /// returning `None` when the instance is already at
    /// `max_concurrent_nar_streams` in-flight bodies. The permit is moved
    /// into the response body via [`Self::stream_nar`] and released only
    /// when that body is fully drained or dropped.
    fn acquire_nar_stream_permit(&self) -> Option<OwnedSemaphorePermit> {
        Arc::clone(&self.nar_stream_semaphore)
            .try_acquire_owned()
            .ok()
    }

    /// Spawns a task that streams `digest` from the CAS into a response
    /// body without buffering the whole NAR. See [`NarBodyStream`] for the
    /// EOF/abort semantics. `permit` bounds the number of simultaneous
    /// streams and lives for the body's lifetime.
    fn stream_nar(
        &self,
        digest: DigestInfo,
        offset: u64,
        length: Option<u64>,
        permit: OwnedSemaphorePermit,
    ) -> Result<Body, Error> {
        let (tx, rx) = make_buf_channel_pair();
        let ctx = sha256_hasher_ctx()?;
        let cas_store = self.cas_store.clone();
        let task = spawn!(
            "nix_cache_nar_stream",
            async move {
                if let Err(err) = cas_store.get_part(digest, tx, offset, length).await {
                    // Dropping `tx` without an EOF aborts the response body,
                    // which is exactly what a mid-body failure must do.
                    warn!(?err, ?digest, "Failed streaming NAR from CAS");
                }
            }
            .with_context(ctx)
        );
        Ok(Body::from_stream(NarBodyStream {
            rx,
            _task: task,
            _permit: permit,
        }))
    }

    /// Whether `hash` is currently remembered as absent from every
    /// upstream. Expired entries are dropped on access.
    fn is_negatively_cached(&self, hash: &str) -> bool {
        if self.upstream_negative_ttl.is_zero() {
            return false;
        }
        let mut cache = self
            .negative_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match cache.get(hash) {
            Some(&expiry) if expiry > Instant::now() => true,
            Some(_) => {
                cache.remove(hash);
                false
            }
            None => false,
        }
    }

    /// Remembers that no upstream had `hash`, so repeated probes (Nix
    /// issues many while computing a closure) do not re-query upstream for
    /// `upstream_negative_ttl`. Sweeps expired entries when the map is
    /// large so it can never grow without bound.
    fn mark_negative(&self, hash: &str) {
        if self.upstream_negative_ttl.is_zero() {
            return;
        }
        let now = Instant::now();
        let mut cache = self
            .negative_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if cache.len() >= NEGATIVE_CACHE_MAX_ENTRIES {
            cache.retain(|_, &mut expiry| expiry > now);
        }
        cache.insert(hash.to_string(), now + self.upstream_negative_ttl);
    }

    /// Attempts to satisfy a local `narinfo` miss from the configured
    /// upstream caches, storing the path (`narinfo` and NAR) locally on a
    /// verified hit so it is durable from then on. Returns the freshly
    /// stored record, or `None` when read-through is disabled or no
    /// upstream has the path. Concurrent identical misses coalesce onto a
    /// single fetch.
    async fn read_through(&self, hash: &str) -> ReadThroughResult {
        if self.upstreams.is_empty() || self.is_negatively_cached(hash) {
            return Ok(None);
        }

        // Coalesce concurrent read-throughs of the same hash, mirroring the
        // transcode-coalescing pattern: the first caller becomes the leader
        // and fetches; the rest await its published result.
        let (slot, is_leader) = {
            let mut inflight = self
                .readthrough_inflight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(existing) = inflight.get(hash) {
                (Arc::clone(existing), false)
            } else {
                let slot = Arc::new(ReadThroughInflight {
                    result: StdMutex::new(None),
                    notify: Notify::new(),
                });
                inflight.insert(hash.to_string(), Arc::clone(&slot));
                (slot, true)
            }
        };

        if is_leader {
            let outcome = self.fetch_across_upstreams(hash).await;
            // Only a proven-absent result is safe to negative-cache. An
            // indeterminate miss (a transient upstream failure) is returned
            // as a plain miss but must be retried on the next probe, not
            // remembered as a 404 for `upstream_negative_ttl`.
            if matches!(outcome, UpstreamOutcome::DefinitiveMiss) {
                self.mark_negative(hash);
            }
            let result: ReadThroughResult = match outcome {
                UpstreamOutcome::Hit(record) => Ok(Some(record)),
                UpstreamOutcome::DefinitiveMiss | UpstreamOutcome::Indeterminate => Ok(None),
            };
            // Publish and wake followers BEFORE removing the slot, so a
            // follower already holding this slot always observes the result.
            {
                let mut cell = slot
                    .result
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                *cell = Some(result.clone());
            }
            slot.notify.notify_waiters();
            self.readthrough_inflight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(hash);
            result
        } else {
            // Follower: register the waiter (`enable`) BEFORE reading the
            // result cell so a publish that races with the read is never
            // missed, then re-check after each wake.
            loop {
                let notified = slot.notify.notified();
                tokio::pin!(notified);
                notified.as_mut().enable();
                let maybe_result = slot
                    .result
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone();
                if let Some(result) = maybe_result {
                    return result;
                }
                notified.await;
            }
        }
    }

    /// Tries each configured upstream in order, returning the first
    /// verified hit. A per-upstream transport or protocol error is logged
    /// and treated as an INDETERMINATE result for that upstream so the next
    /// one is still tried and a flaky upstream never becomes a
    /// client-facing 5xx — but a miss produced (even partly) by such an
    /// error is [`UpstreamOutcome::Indeterminate`], never negative-cached,
    /// so a transient outage cannot turn an existing path into a cached
    /// 404. Only when every upstream was reachable and definitively lacked
    /// the path is the result [`UpstreamOutcome::DefinitiveMiss`].
    async fn fetch_across_upstreams(&self, hash: &str) -> UpstreamOutcome {
        let mut had_error = false;
        for upstream in &self.upstreams {
            match self.fetch_one_upstream(upstream, hash).await {
                Ok(Some(record)) => {
                    self.upstream_hits.inc();
                    return UpstreamOutcome::Hit(record);
                }
                Ok(None) => {}
                Err(err) => {
                    had_error = true;
                    self.upstream_errors.inc();
                    warn!(
                        ?err,
                        %hash,
                        url = %upstream.base_url,
                        "Upstream read-through failed; trying next upstream",
                    );
                }
            }
        }
        self.upstream_misses.inc();
        if had_error {
            UpstreamOutcome::Indeterminate
        } else {
            UpstreamOutcome::DefinitiveMiss
        }
    }

    /// Fetches one store path from a single upstream: probes its
    /// `narinfo`, verifies the signature against the upstream's trusted
    /// keys, downloads and verifies the NAR against the signed
    /// `NarHash`/`NarSize`, ingests the uncompressed NAR into the CAS, and
    /// stores the record (preserving upstream signatures and adding this
    /// instance's own). Returns `None` for a clean upstream miss (404/401/
    /// 403, hash mismatch, unsupported compression, failed verification).
    async fn fetch_one_upstream(&self, upstream: &UpstreamCache, hash: &str) -> ReadThroughResult {
        // 1. narinfo probe.
        let narinfo_url = format!("{}/{hash}.narinfo", upstream.base_url);
        let resp = self
            .http_client
            .get(&narinfo_url)
            .timeout(self.upstream_timeout)
            .send()
            .await
            .err_tip(|| format!("Requesting upstream narinfo {narinfo_url}"))?;
        let status = resp.status();
        if matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ) {
            return Ok(None);
        }
        if !status.is_success() {
            return Err(make_err!(
                Code::Unavailable,
                "upstream narinfo {narinfo_url} returned {status}"
            ));
        }
        let text = resp
            .text()
            .await
            .err_tip(|| "Reading upstream narinfo body")?;
        let info = narinfo::parse(&text).err_tip(|| "Parsing upstream narinfo")?;

        // 2. The returned narinfo must be for the store path we asked for,
        //    and for our store dir; otherwise it is not ours to cache.
        let store_hash = info
            .store_path_hash()
            .err_tip(|| "Validating StorePath in upstream narinfo")?;
        if store_hash != hash {
            warn!(%hash, %store_hash, "upstream narinfo StorePath hash mismatch; ignoring");
            return Ok(None);
        }
        let upstream_store_dir = info
            .store_path
            .rfind('/')
            .map_or("", |idx| &info.store_path[..idx]);
        if upstream_store_dir != self.store_dir {
            warn!(
                %hash,
                upstream_store_dir,
                our_store_dir = %self.store_dir,
                "upstream narinfo is for a different store dir; ignoring",
            );
            return Ok(None);
        }

        // 3. Require a valid signature from a trusted key before caching:
        //    the fingerprint covers StorePath, NarHash, NarSize, and
        //    References, so a verified narinfo pins exactly the NAR bytes
        //    fetched in step 4.
        let fingerprint = info.fingerprint();
        let verified = info.sigs.iter().any(|sig| {
            upstream
                .public_keys
                .iter()
                .any(|key| key.verify(&fingerprint, sig))
        });
        if !verified {
            self.upstream_rejected_unsigned.inc();
            warn!(
                %hash,
                url = %upstream.base_url,
                "upstream narinfo failed signature verification; refusing to cache",
            );
            return Ok(None);
        }

        // 4. Fetch and ingest the NAR. Unsupported compression is a clean
        //    miss rather than an error.
        let Some(codec) = codec_for_compression(&info.compression) else {
            warn!(
                %hash,
                compression = %info.compression,
                "upstream NAR uses a compression this cache cannot decode; ignoring",
            );
            return Ok(None);
        };
        let nar_url = join_upstream_url(&upstream.base_url, &info.url);
        let nar_resp = self
            .http_client
            .get(&nar_url)
            .send()
            .await
            .err_tip(|| format!("Requesting upstream NAR {nar_url}"))?;
        if !nar_resp.status().is_success() {
            return Err(make_err!(
                Code::Unavailable,
                "upstream NAR {nar_url} returned {}",
                nar_resp.status()
            ));
        }
        let nar_stream = nar_resp.bytes_stream().map_err(axum::Error::new).boxed();
        let (nar_sha256, nar_size) = ingest_nar_spooled(self, codec, nar_stream)
            .await
            .err_tip(|| "Ingesting upstream NAR into cas_store")?;

        // 5. The ingested NAR must match the SIGNED NarHash/NarSize. A
        //    mismatch means the upstream served bytes that do not match the
        //    signature it also served: refuse, leaving the ingested blob
        //    unreferenced (it is under its own true digest and will be
        //    evicted) and writing no record.
        if nar_sha256 != info.nar_hash || nar_size != info.nar_size {
            // A post-download hash/size mismatch means truncation, corruption,
            // or tampering in transit — a transient/indeterminate condition,
            // NOT proof the path is absent. Return an error (folded into
            // `Indeterminate` upstream) so it is retried next probe rather
            // than negative-cached as a 404. The ingested blob is left
            // unreferenced under its own true digest and will be evicted.
            return Err(make_err!(
                Code::Unavailable,
                "upstream NAR hash/size does not match its signed narinfo for {hash} at {}; refusing",
                upstream.base_url
            ));
        }
        self.upstream_nar_bytes.add(nar_size);

        // 6. Build and store the record: preserve the upstream signatures
        //    verbatim and add this instance's own for every key that has
        //    not already signed this fingerprint. The served NAR is our
        //    canonical uncompressed blob (file_* left unset), so the
        //    re-rendered narinfo advertises Compression: none.
        let mut path_info = NixPathInfo::from_nar_info(&info);
        // Dedup on the signature bytes, NOT the key name. Ed25519 (RFC 8032)
        // signatures over the same fingerprint are deterministic, so a key
        // that already signed reproduces its exact existing signature and is
        // naturally deduped — while a foreign signature that merely shares our
        // key NAME never matches, so it can no longer suppress our own
        // authoritative signature. (Name-based dedup let a same-named foreign
        // key poison the narinfo, leaving it signed only by an untrusted key.)
        let new_sigs: Vec<String> = self
            .signing_keys
            .iter()
            .map(|key| key.sign(&fingerprint))
            .filter(|our_sig| !path_info.signatures.contains(our_sig))
            .collect();
        path_info.signatures.extend(new_sigs);
        let record = path_info
            .encode_record()
            .err_tip(|| "Encoding read-through narinfo record")?;
        let record_bytes = Bytes::from(record);
        with_sha256_ctx(
            self.path_info_store
                .update_oneshot(StoreKey::new_str(hash), record_bytes.clone()),
        )
        .await
        .err_tip(|| "Storing read-through narinfo record in path_info_store")?;
        debug!(%hash, url = %upstream.base_url, nar_size, "read-through cached upstream path");
        Ok(Some(record_bytes))
    }
}

/// The retryable `503` served when the instance is at its
/// `max_concurrent_nar_streams` ceiling: a bounded, non-hanging refusal
/// rather than committing a `200` and buffering unboundedly.
fn too_many_streams_response() -> Response {
    text_response(
        StatusCode::SERVICE_UNAVAILABLE,
        "too many concurrent NAR streams",
    )
}

/// The Nix binary-cache HTTP service: one router per configured instance.
#[derive(Debug, MetricsComponent)]
pub struct NixCacheServer {
    #[metric(group = "instances")]
    instances: Vec<Arc<NixCacheInstance>>,
}

impl NixCacheServer {
    /// Creates the service, resolving all store references and signing
    /// keys up front so misconfiguration fails at startup rather than on
    /// first request.
    pub fn new(
        configs: &[WithInstanceName<NixCacheConfig>],
        store_manager: &StoreManager,
    ) -> Result<Self, Error> {
        let mut instances = Vec::with_capacity(configs.len());
        for config in configs {
            instances.push(Arc::new(
                NixCacheInstance::new(config, store_manager).err_tip(|| {
                    format!(
                        "Failed to create nix_cache instance '{}'",
                        config.instance_name
                    )
                })?,
            ));
        }
        Ok(Self { instances })
    }

    /// Returns one `(mount_path, Router)` pair per instance, to be nested
    /// into the server's axum router.
    ///
    /// matchit (axum 0.8's router) rejects dynamic-suffix routes like
    /// `/{hash}.narinfo` at router-build time, so only whole-segment
    /// captures are used and the `.narinfo` suffix is stripped in-handler.
    /// HEAD handlers are registered explicitly: axum's `get()` would
    /// otherwise answer HEAD by running the GET body, which must never
    /// happen for multi-gigabyte NARs.
    #[must_use]
    pub fn routers(&self) -> Vec<(String, Router)> {
        self.instances
            .iter()
            .map(|instance| {
                let router = Router::new()
                    .route("/nix-cache-info", get(get_cache_info).put(put_cache_info))
                    .route("/nar/{name}", get(get_nar).head(head_nar).put(put_nar))
                    .route(
                        "/log/{drv}",
                        get(get_build_log).head(head_build_log).put(put_build_log),
                    )
                    .route(
                        "/{file}",
                        get(get_narinfo).head(head_narinfo).put(put_narinfo),
                    )
                    .with_state(instance.clone());
                (instance.mount_path.clone(), router)
            })
            .collect()
    }
}

/// `GET /nix-cache-info`: `200` (or `401` on a token-gated instance) —
/// a `404` here bricks the client's store open.
async fn get_cache_info(
    State(instance): State<Arc<NixCacheInstance>>,
    headers: HeaderMap,
) -> Response {
    if let Some(denied) = instance.authorize_read(&headers) {
        return denied;
    }
    let body = format!(
        "StoreDir: {}\nWantMassQuery: {}\nPriority: {}\n",
        instance.store_dir,
        u8::from(instance.want_mass_query),
        instance.priority,
    );
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        CONTENT_TYPE,
        HeaderValue::from_static(NIX_CACHE_INFO_CONTENT_TYPE),
    );
    response
}

/// `PUT /nix-cache-info`: `nix copy` uploads this file on first push;
/// accept and discard (the served document always comes from config).
async fn put_cache_info(
    State(instance): State<Arc<NixCacheInstance>>,
    headers: HeaderMap,
) -> Response {
    if let Some(denied) = instance.authorize_write(&headers) {
        instance.rejected_puts.inc();
        return denied;
    }
    if instance.read_only {
        instance.rejected_puts.inc();
        return text_response(StatusCode::METHOD_NOT_ALLOWED, "cache is read-only");
    }
    empty_response(StatusCode::OK)
}

/// `HEAD /{hash}.narinfo` and `HEAD /{hash}.ls`.
async fn head_narinfo(
    State(instance): State<Arc<NixCacheInstance>>,
    UrlPath(file): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(denied) = instance.authorize_read(&headers) {
        return denied;
    }
    if let Some(hash) = listing_name_hash(&file) {
        return head_listing_inner(&instance, hash)
            .await
            .unwrap_or_else(|err| error_response("HEAD listing", &err));
    }
    head_narinfo_inner(&instance, &file)
        .await
        .unwrap_or_else(|err| error_response("HEAD narinfo", &err))
}

async fn head_narinfo_inner(instance: &NixCacheInstance, file: &str) -> Result<Response, Error> {
    let Some(hash) = narinfo_name_hash(file) else {
        return Ok(empty_response(StatusCode::NOT_FOUND));
    };
    // The completeness-checking wrapper around `path_info_store` makes a
    // narinfo whose NAR was evicted read as absent here — by design.
    let found = with_sha256_ctx(instance.path_info_store.has(StoreKey::new_str(hash)))
        .await
        .err_tip(|| "Checking narinfo existence in path_info_store")?;
    if found.is_some() {
        return Ok(empty_response(StatusCode::OK));
    }
    // A local miss reads through, exactly as the GET handler does — nix's
    // `isValidPath` probes availability with a HEAD before copying, so
    // read-through must answer HEAD too or `nix copy --from` reports the
    // path missing without ever issuing the GET.
    if instance.read_through(hash).await?.is_some() {
        Ok(empty_response(StatusCode::OK))
    } else {
        Ok(empty_response(StatusCode::NOT_FOUND))
    }
}

/// `GET /{hash}.narinfo` and `GET /{hash}.ls`.
async fn get_narinfo(
    State(instance): State<Arc<NixCacheInstance>>,
    UrlPath(file): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(denied) = instance.authorize_read(&headers) {
        return denied;
    }
    if let Some(hash) = listing_name_hash(&file) {
        return get_listing_inner(&instance, hash)
            .await
            .unwrap_or_else(|err| error_response("GET listing", &err));
    }
    get_narinfo_inner(&instance, &file)
        .await
        .unwrap_or_else(|err| error_response("GET narinfo", &err))
}

async fn get_narinfo_inner(instance: &NixCacheInstance, file: &str) -> Result<Response, Error> {
    instance.narinfo_gets.inc();
    let Some(hash) = narinfo_name_hash(file) else {
        return Ok(empty_response(StatusCode::NOT_FOUND));
    };
    // A local hit serves directly. A local miss (`Code::NotFound`) falls
    // through to read-through when upstreams are configured; that fetches,
    // verifies, and stores the path so this and later requests are served
    // locally. With no upstreams (or an upstream miss) it stays a 404.
    let record = match with_sha256_ctx(instance.path_info_store.get_part_unchunked(
        StoreKey::new_str(hash),
        0,
        None,
    ))
    .await
    {
        Ok(record) => record,
        Err(err) if err.code == Code::NotFound => match instance.read_through(hash).await? {
            Some(record) => record,
            None => return Ok(empty_response(StatusCode::NOT_FOUND)),
        },
        Err(err) => {
            return Err(err).err_tip(|| "Fetching narinfo record from path_info_store");
        }
    };
    // A record that exists but does not decode is a server-side problem
    // (500), not a client error: never let it surface as a 400 or 404.
    let mut path_info = NixPathInfo::decode_record(&record)
        .map_err(|err| make_err!(Code::Internal, "Corrupt narinfo record for '{hash}': {err}"))?;
    let nar_sha256: [u8; 32] = path_info.nar_sha256.as_slice().try_into().map_err(|_| {
        make_err!(
            Code::Internal,
            "Corrupt narinfo record for '{hash}': NAR hash is not 32 bytes"
        )
    })?;
    // Stamp our authoritative signature on serve, so records stored before the
    // dedup fix (signed only by a foreign same-named key) self-heal on the next
    // fetch rather than needing a re-push. Dedup on signature bytes: a key that
    // already signed reproduces its exact signature and is skipped. The
    // fingerprint covers path/NarHash/NarSize/References, which the URL and
    // Compression switch below never touch, so this stays valid for every
    // rendered variant. A fingerprint failure is non-fatal — serve as stored.
    if !instance.signing_keys.is_empty()
        && let Ok(fingerprint) = path_info.fingerprint()
    {
        let new_sigs: Vec<String> = instance
            .signing_keys
            .iter()
            .map(|key| key.sign(&fingerprint))
            .filter(|our_sig| !path_info.signatures.contains(our_sig))
            .collect();
        path_info.signatures.extend(new_sigs);
    }
    // Choose the served URL/Compression/FileHash/FileSize by precedence,
    // each guarded by "the referenced blob still exists in CAS" (else fall
    // through). The ed25519 fingerprint covers only path/NarHash/NarSize/
    // references, so this URL/Compression switch never invalidates stored
    // signatures.
    //   1. A preserved ORIGINAL compressed push: advertise the client's
    //      verbatim URL and its original codec/FileHash/FileSize — an exact
    //      round trip of the push.
    //   2. A zstd transcode (phase 2): the canonical `.nar.zst` URL.
    //   3. The canonical uncompressed NAR: `Compression: none`, no
    //      FileHash/FileSize.
    let info = if let Some((file_sha256, file_size, compression, url)) =
        preserved_original_serving_fields(instance, &path_info).await?
    {
        let mut info = path_info.to_nar_info(url, compression).map_err(|err| {
            make_err!(Code::Internal, "Corrupt narinfo record for '{hash}': {err}")
        })?;
        info.file_hash = Some(file_sha256);
        info.file_size = Some(file_size);
        info
    } else if let Some((file_sha256, file_size)) = zstd_serving_fields(instance, &path_info).await?
    {
        let url = canonical_nar_zst_name(&file_sha256, file_size);
        let mut info = path_info
            .to_nar_info(format!("nar/{url}"), ZSTD_COMPRESSION_NAME.to_string())
            .map_err(|err| {
                make_err!(Code::Internal, "Corrupt narinfo record for '{hash}': {err}")
            })?;
        info.file_hash = Some(file_sha256);
        info.file_size = Some(file_size);
        info
    } else {
        let url = canonical_nar_name(&nar_sha256, path_info.nar_size);
        path_info
            .to_nar_info(format!("nar/{url}"), "none".to_string())
            .map_err(|err| {
                make_err!(Code::Internal, "Corrupt narinfo record for '{hash}': {err}")
            })?
    };
    let rendered = info.render();
    let mut response = Response::new(Body::from(rendered));
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(NARINFO_CONTENT_TYPE));
    Ok(response)
}

/// Returns the `(file_sha256, file_size)` a record's zstd artifact is
/// served under, or `None` when the record carries no `file_*` trio or
/// the compressed blob has been evicted. Eviction degrades gracefully to
/// the uncompressed rendering — the reason `file_*` stays out of the
/// record's `ActionResult` output files.
async fn zstd_serving_fields(
    instance: &NixCacheInstance,
    path_info: &NixPathInfo,
) -> Result<Option<([u8; 32], u64)>, Error> {
    if path_info.file_compression != ZSTD_COMPRESSION_NAME || path_info.file_size == 0 {
        return Ok(None);
    }
    // `decode_record` enforces the all-or-none file_* invariant; treat
    // any residual shape surprise as "no compressed artifact" rather
    // than failing the narinfo.
    let Ok(file_sha256) = <[u8; 32]>::try_from(path_info.file_sha256.as_slice()) else {
        return Ok(None);
    };
    let file_digest = DigestInfo::new(file_sha256, path_info.file_size);
    let present = with_sha256_ctx(instance.cas_store.has(file_digest))
        .await
        .err_tip(|| "Checking transcoded NAR existence in cas_store")?;
    Ok(present.map(|_| (file_sha256, path_info.file_size)))
}

/// Returns the `(FileHash, FileSize)` of a preserved ORIGINAL compressed
/// upload at narinfo-PUT time: the client's `FileHash`/`FileSize` when
/// both are present and the compressed blob is still in the CAS. Returns
/// `None` when the client sent no compressed file fields, or the blob is
/// absent (preserve was off when the NAR was PUT, or it was later
/// evicted) — the caller then falls back to canonical or zstd serving.
async fn preserved_compressed_fields(
    instance: &NixCacheInstance,
    info: &narinfo::NarInfo,
) -> Result<Option<([u8; 32], u64)>, Error> {
    let (Some(file_sha256), Some(file_size)) = (info.file_hash, info.file_size) else {
        return Ok(None);
    };
    if file_size == 0 {
        return Ok(None);
    }
    let file_digest = DigestInfo::new(file_sha256, file_size);
    let present = with_sha256_ctx(instance.cas_store.has(file_digest))
        .await
        .err_tip(|| "Checking preserved compressed NAR existence in cas_store")?;
    Ok(present.map(|_| (file_sha256, file_size)))
}

/// Returns the `(file_sha256, file_size, compression, url)` a record's
/// preserved ORIGINAL compressed artifact is served under (narinfo-GET
/// case 1), or `None` when the record carries no preserved original or its
/// blob has been evicted.
///
/// A preserved original is distinguished from a zstd transcode by a
/// non-empty `file_url` (the verbatim client URL) and a `file_compression`
/// that is not `zstd`: a preserved zstd push is served by the canonical
/// zstd path ([`zstd_serving_fields`]) instead, so both the client's
/// zstd URL (via the alias) and the canonical `.nar.zst` name resolve.
/// Eviction of the compressed blob degrades gracefully to the uncompressed
/// rendering — the reason `file_*` stays out of the record's
/// `ActionResult` output files.
async fn preserved_original_serving_fields(
    instance: &NixCacheInstance,
    path_info: &NixPathInfo,
) -> Result<Option<([u8; 32], u64, String, String)>, Error> {
    if path_info.file_url.is_empty()
        || path_info.file_size == 0
        || path_info.file_compression.is_empty()
        || path_info.file_compression == ZSTD_COMPRESSION_NAME
    {
        return Ok(None);
    }
    let Ok(file_sha256) = <[u8; 32]>::try_from(path_info.file_sha256.as_slice()) else {
        return Ok(None);
    };
    let file_digest = DigestInfo::new(file_sha256, path_info.file_size);
    let present = with_sha256_ctx(instance.cas_store.has(file_digest))
        .await
        .err_tip(|| "Checking preserved compressed NAR existence in cas_store")?;
    Ok(present.map(|_| {
        (
            file_sha256,
            path_info.file_size,
            path_info.file_compression.clone(),
            path_info.file_url.clone(),
        )
    }))
}

/// `PUT /{hash}.narinfo` and `PUT /{hash}.ls`.
async fn put_narinfo(
    State(instance): State<Arc<NixCacheInstance>>,
    UrlPath(file): UrlPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if let Some(denied) = instance.authorize_write(&headers) {
        instance.rejected_puts.inc();
        return denied;
    }
    if let Some(hash) = listing_name_hash(&file) {
        let response = put_listing_inner(&instance, hash, body)
            .await
            .unwrap_or_else(|err| error_response("PUT listing", &err));
        if response.status() == StatusCode::CREATED {
            instance.aux_puts.inc();
        } else {
            instance.rejected_puts.inc();
        }
        return response;
    }
    let response = put_narinfo_inner(&instance, &file, body)
        .await
        .unwrap_or_else(|err| error_response("PUT narinfo", &err));
    if response.status() == StatusCode::CREATED {
        instance.narinfo_puts.inc();
    } else {
        instance.rejected_puts.inc();
    }
    response
}

async fn put_narinfo_inner(
    instance: &NixCacheInstance,
    file: &str,
    body: Body,
) -> Result<Response, Error> {
    if instance.read_only {
        return Ok(text_response(
            StatusCode::METHOD_NOT_ALLOWED,
            "cache is read-only",
        ));
    }
    let Some(hash) = narinfo_name_hash(file) else {
        return Ok(empty_response(StatusCode::NOT_FOUND));
    };
    let Some(body_bytes) = read_body_limited(body, MAX_NARINFO_BODY_BYTES).await? else {
        return Ok(text_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "narinfo body exceeds 1 MiB",
        ));
    };
    let text = core::str::from_utf8(&body_bytes)
        .map_err(|e| make_input_err!("narinfo body is not UTF-8: {e}"))?;
    let info = narinfo::parse(text).err_tip(|| "Parsing uploaded narinfo")?;
    let store_hash = info
        .store_path_hash()
        .err_tip(|| "Validating StorePath in uploaded narinfo")?;
    if store_hash != hash {
        return Err(make_input_err!(
            "narinfo file name hash '{hash}' does not match StorePath hash '{store_hash}'"
        ));
    }

    // The canonical uncompressed NAR is always keyed by its NarHash/
    // NarSize. Verify it directly rather than resolving the narinfo's URL
    // through the alias: with `preserve_upload_compression` the alias now
    // points at the client's ORIGINAL (possibly compressed) blob, so it is
    // no longer a NarHash oracle. Action-cache completeness keys on this
    // uncompressed NAR, so a narinfo whose NAR was never uploaded is a 409.
    let digest = DigestInfo::new(info.nar_hash, info.nar_size);
    let nar_present = with_sha256_ctx(instance.cas_store.has(digest))
        .await
        .err_tip(|| "Checking NAR existence in cas_store")?;
    if nar_present.is_none() {
        return Ok(text_response(
            StatusCode::CONFLICT,
            "NAR must be uploaded before its narinfo",
        ));
    }

    // Preserve the client's signatures verbatim and add ours for any
    // configured key that has not signed this fingerprint yet.
    let mut path_info = NixPathInfo::from_nar_info(&info);
    let fingerprint = path_info
        .fingerprint()
        .err_tip(|| "Computing narinfo fingerprint for signing")?;
    // Dedup on signature bytes, not key name — see the note at the upstream
    // ingest site. A same-named foreign signature must not suppress ours.
    let new_sigs: Vec<String> = instance
        .signing_keys
        .iter()
        .map(|key| key.sign(&fingerprint))
        .filter(|our_sig| !path_info.signatures.contains(our_sig))
        .collect();
    path_info.signatures.extend(new_sigs);

    // Choose the served compressed artifact, in precedence order.
    //
    // 1. A preserved ORIGINAL: the client pushed a compressed NAR
    //    (`Compression != none` with `FileHash`/`FileSize`) and, with
    //    preserve on, its blob is still in the CAS. Record the original
    //    compression, FileHash/FileSize, and the exact client URL so a GET
    //    round-trips the push byte-for-byte. This takes precedence over
    //    zstd transcoding: the client's own codec is served and the
    //    re-encode is skipped.
    // 2. Otherwise, on a zstd instance, transcode the uncompressed NAR to
    //    zstd on ingest so GET stays a pure blob read.
    //
    // Either way a transcode/lookup failure fails the whole PUT; no
    // half-record is written. `file_url` stays empty for the zstd path
    // (its URL is derived canonically on GET) and for the plain path.
    let preserved = if instance.preserve_upload_compression && info.compression != "none" {
        preserved_compressed_fields(instance, &info).await?
    } else {
        None
    };
    if let Some((file_sha256, file_size)) = preserved {
        path_info.file_sha256 = file_sha256.to_vec();
        path_info.file_size = file_size;
        path_info.file_compression = info.compression.clone();
        path_info.file_url = info.url.clone();
    } else if instance.serve_compression == ServeCompression::Zstd {
        let (file_sha256, file_size) =
            resolve_or_transcode_zstd(instance, hash, &path_info, digest).await?;
        path_info.file_sha256 = file_sha256.to_vec();
        path_info.file_size = file_size;
        path_info.file_compression = ZSTD_COMPRESSION_NAME.to_string();
    }

    // `encode_record` runs the same reference/signature/store-path checks
    // `to_nar_info` runs on GET, so a record that could never be rendered
    // is rejected here as a client error (400) instead of being stored and
    // then 500ing forever on GET. Preserve its `InvalidArgument` code
    // rather than masking it as an internal error.
    let record = path_info
        .encode_record()
        .err_tip(|| "Encoding narinfo record for storage")?;
    with_sha256_ctx(
        instance
            .path_info_store
            .update_oneshot(StoreKey::new_str(hash), record.into()),
    )
    .await
    .err_tip(|| "Storing narinfo record in path_info_store")?;
    Ok(empty_response(StatusCode::CREATED))
}

/// `HEAD /{hash}.ls` (dispatched from [`head_narinfo`]).
async fn head_listing_inner(instance: &NixCacheInstance, hash: &str) -> Result<Response, Error> {
    let found = with_sha256_ctx(
        instance
            .alias_store
            .has(StoreKey::Str(Cow::Owned(listing_key(hash)))),
    )
    .await
    .err_tip(|| "Checking NAR listing existence in alias_store")?;
    if found.is_some() {
        Ok(empty_response(StatusCode::OK))
    } else {
        Ok(empty_response(StatusCode::NOT_FOUND))
    }
}

/// `GET /{hash}.ls` (dispatched from [`get_narinfo`]).
async fn get_listing_inner(instance: &NixCacheInstance, hash: &str) -> Result<Response, Error> {
    instance.aux_gets.inc();
    // `Code::NotFound` propagates to a 404 through `error_response`.
    let listing = with_sha256_ctx(instance.alias_store.get_part_unchunked(
        StoreKey::Str(Cow::Owned(listing_key(hash))),
        0,
        None,
    ))
    .await
    .err_tip(|| "Fetching NAR listing from alias_store")?;
    let mut response = Response::new(Body::from(listing));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(LISTING_CONTENT_TYPE));
    Ok(response)
}

/// `PUT /{hash}.ls` (dispatched from [`put_narinfo`]). Stored verbatim:
/// nix pushes listings (with `?write-nar-listing=1`) BEFORE the NAR, so
/// there is deliberately no cross-check against path-info or CAS state.
async fn put_listing_inner(
    instance: &NixCacheInstance,
    hash: &str,
    body: Body,
) -> Result<Response, Error> {
    if instance.read_only {
        return Ok(text_response(
            StatusCode::METHOD_NOT_ALLOWED,
            "cache is read-only",
        ));
    }
    let Some(body_bytes) = read_body_limited(body, MAX_LISTING_BODY_BYTES).await? else {
        return Ok(text_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "listing body exceeds 8 MiB",
        ));
    };
    with_sha256_ctx(
        instance
            .alias_store
            .update_oneshot(StoreKey::Str(Cow::Owned(listing_key(hash))), body_bytes),
    )
    .await
    .err_tip(|| "Storing NAR listing in alias_store")?;
    Ok(empty_response(StatusCode::CREATED))
}

/// Resolves a NAR request name to the `(sha256, size)` of the stored
/// blob: canonical names (`.nar` for the uncompressed NAR, `.nar.zst`
/// for a transcoded artifact) directly, anything else through the alias
/// store. Either way the digest addresses exactly the bytes the name
/// promises.
async fn resolve_nar_name(
    instance: &NixCacheInstance,
    name: &str,
) -> Result<Option<([u8; 32], u64)>, Error> {
    if let Some((sha256, size, _codec)) = parse_canonical_any(name) {
        return Ok(Some((sha256, size)));
    }
    instance.lookup_alias(name).await
}

/// `HEAD /nar/{name}`.
async fn head_nar(
    State(instance): State<Arc<NixCacheInstance>>,
    UrlPath(name): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(denied) = instance.authorize_read(&headers) {
        return denied;
    }
    head_nar_inner(&instance, &name)
        .await
        .unwrap_or_else(|err| error_response("HEAD nar", &err))
}

async fn head_nar_inner(instance: &NixCacheInstance, name: &str) -> Result<Response, Error> {
    let Some((nar_sha256, nar_size)) = resolve_nar_name(instance, name).await? else {
        return Ok(empty_response(StatusCode::NOT_FOUND));
    };
    // The alias alone must not answer: an alias whose NAR was evicted must
    // 404 so `nix copy` re-uploads the NAR. A 200 here followed by a
    // rejected narinfo PUT would brick the push.
    let digest = DigestInfo::new(nar_sha256, nar_size);
    let nar_present = with_sha256_ctx(instance.cas_store.has(digest))
        .await
        .err_tip(|| "Checking NAR existence in cas_store")?;
    if nar_present.is_none() {
        return Ok(empty_response(StatusCode::NOT_FOUND));
    }
    // Content-Length comes from the name/alias, never from has(): store
    // compositions (compression, fast_slow, ...) report physical sizes.
    let mut response = empty_response(StatusCode::OK);
    let headers = response.headers_mut();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static(NAR_CONTENT_TYPE));
    headers.insert(CONTENT_LENGTH, HeaderValue::from(nar_size));
    headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    Ok(response)
}

/// `GET /nar/{name}`.
async fn get_nar(
    State(instance): State<Arc<NixCacheInstance>>,
    UrlPath(name): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(denied) = instance.authorize_read(&headers) {
        return denied;
    }
    get_nar_inner(&instance, &name, &headers)
        .await
        .unwrap_or_else(|err| error_response("GET nar", &err))
}

async fn get_nar_inner(
    instance: &NixCacheInstance,
    name: &str,
    headers: &HeaderMap,
) -> Result<Response, Error> {
    instance.nar_gets.inc();
    // Resolve exactly as HEAD does (see `resolve_nar_name`): canonical
    // names address the blob directly (`.nar` = uncompressed NAR,
    // `.nar.zst` = transcoded artifact), and any other name goes through
    // the alias store. A compressed alias name (`.nar.xz`, ...) resolves
    // to the ORIGINAL compressed blob preserved at upload time
    // (`preserve_upload_compression`), so serving its bytes verbatim under
    // that name is correct — this is what makes a client's warm pull of
    // its own `.nar.xz` URL resolve instead of 404ing. Either way the
    // digest addresses exactly the bytes the name promises, so the range,
    // length, and streaming below are identical.
    let Some((nar_sha256, nar_size)) = resolve_nar_name(instance, name).await? else {
        return Ok(empty_response(StatusCode::NOT_FOUND));
    };
    let digest = DigestInfo::new(nar_sha256, nar_size);
    // Check existence before committing a 200: a missing NAR must be an
    // honest 404, not a committed status followed by an aborted body.
    let nar_present = with_sha256_ctx(instance.cas_store.has(digest))
        .await
        .err_tip(|| "Checking NAR existence in cas_store")?;
    if nar_present.is_none() {
        return Ok(empty_response(StatusCode::NOT_FOUND));
    }

    let range_header = headers.get(RANGE).and_then(|value| value.to_str().ok());
    match parse_range(range_header, nar_size) {
        RangeRequest::Unsatisfiable => {
            let mut response = empty_response(StatusCode::RANGE_NOT_SATISFIABLE);
            response.headers_mut().insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes */{nar_size}"))
                    .map_err(|e| make_err!(Code::Internal, "Invalid Content-Range header: {e}"))?,
            );
            Ok(response)
        }
        RangeRequest::Full => {
            let Some(permit) = instance.acquire_nar_stream_permit() else {
                return Ok(too_many_streams_response());
            };
            let body = instance.stream_nar(digest, 0, None, permit)?;
            instance.nar_bytes_served.add(nar_size);
            let mut response = Response::new(body);
            let response_headers = response.headers_mut();
            response_headers.insert(CONTENT_TYPE, HeaderValue::from_static(NAR_CONTENT_TYPE));
            response_headers.insert(CONTENT_LENGTH, HeaderValue::from(nar_size));
            response_headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            Ok(response)
        }
        RangeRequest::Range { start, end } => {
            let Some(permit) = instance.acquire_nar_stream_permit() else {
                return Ok(too_many_streams_response());
            };
            let length = end - start + 1;
            let body = instance.stream_nar(digest, start, Some(length), permit)?;
            instance.nar_bytes_served.add(length);
            let mut response = Response::new(body);
            *response.status_mut() = StatusCode::PARTIAL_CONTENT;
            let response_headers = response.headers_mut();
            response_headers.insert(CONTENT_TYPE, HeaderValue::from_static(NAR_CONTENT_TYPE));
            response_headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
            response_headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
            response_headers.insert(
                CONTENT_RANGE,
                HeaderValue::from_str(&format!("bytes {start}-{end}/{nar_size}"))
                    .map_err(|e| make_err!(Code::Internal, "Invalid Content-Range header: {e}"))?,
            );
            Ok(response)
        }
    }
}

/// `PUT /nar/{name}`.
async fn put_nar(
    State(instance): State<Arc<NixCacheInstance>>,
    UrlPath(name): UrlPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if let Some(denied) = instance.authorize_write(&headers) {
        instance.rejected_puts.inc();
        return denied;
    }
    let response = put_nar_inner(&instance, &name, &headers, body)
        .await
        .unwrap_or_else(|err| error_response("PUT nar", &err));
    if response.status() == StatusCode::CREATED {
        instance.nar_puts.inc();
    } else {
        instance.rejected_puts.inc();
    }
    response
}

async fn put_nar_inner(
    instance: &NixCacheInstance,
    name: &str,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, Error> {
    if instance.read_only {
        return Ok(text_response(
            StatusCode::METHOD_NOT_ALLOWED,
            "cache is read-only",
        ));
    }
    let Some(name_codec) = codec_for_name(name) else {
        return Err(make_input_err!(
            "unsupported NAR extension in '{name}': expected .nar, .nar.xz, .nar.zst, or .nar.bz2"
        ));
    };
    if !is_valid_nar_upload_name(name) {
        return Err(make_input_err!(
            "invalid NAR name '{name}': expected '{{52 nix32 chars}}' or \
             '{{52 nix32 chars}}-{{size}}' before the extension"
        ));
    }
    let content_length = content_length_of(headers);
    // Fast-fail before reading any body bytes when a declared size already
    // exceeds the cap: the uncompressed size embedded in a canonical
    // `{hash}-{size}.nar` name, or otherwise the wire `Content-Length` (an
    // upload that is already over the cap on the wire is over it
    // decompressed too, since compression only shrinks).
    let declared_size = bare_nar_digest_from_name(name, content_length)
        .map(|(_, size)| size)
        .or(content_length);
    if let Some(size) = declared_size
        && size > instance.max_nar_size_bytes
    {
        return Err(make_err!(
            Code::ResourceExhausted,
            "declared NAR size {size} exceeds the {}-byte limit",
            instance.max_nar_size_bytes
        ));
    }
    let mut data_stream = body.into_data_stream();

    // Nix's uploader maps BOTH gzip and identity compression to a bare
    // `.nar` name, so sniff the first two body bytes for the gzip magic.
    let mut prefix_chunks: Vec<Bytes> = Vec::new();
    let codec = if name_codec == NarCodec::None {
        let mut prefix_len = 0_usize;
        while prefix_len < GZIP_MAGIC.len() {
            match data_stream.next().await {
                Some(Ok(chunk)) => {
                    if !chunk.is_empty() {
                        prefix_len += chunk.len();
                        prefix_chunks.push(chunk);
                    }
                }
                Some(Err(err)) => {
                    return Err(make_input_err!("Failed to read NAR upload body: {err}"));
                }
                None => break,
            }
        }
        let mut magic_bytes = prefix_chunks.iter().flat_map(|chunk| chunk.iter().copied());
        if (magic_bytes.next(), magic_bytes.next()) == (Some(GZIP_MAGIC[0]), Some(GZIP_MAGIC[1])) {
            NarCodec::Gzip
        } else {
            NarCodec::None
        }
    } else {
        name_codec
    };
    let full_stream = futures::stream::iter(prefix_chunks.into_iter().map(Ok)).chain(data_stream);

    // Preserve the client's ORIGINAL compressed bytes when configured and
    // the upload carries an explicit compressed extension. Gzip-sniffed
    // bare `.nar` uploads are deliberately excluded: their name promises
    // `.nar` (uncompressed), so serving the gzip bytes back under it would
    // corrupt a client, and gzip is not a codec nix produces anyway.
    let preserved_digest = if instance.preserve_upload_compression
        && matches!(name_codec, NarCodec::Xz | NarCodec::Zstd | NarCodec::Bzip2)
    {
        compressed_upload_digest(name, name_codec, content_length)
    } else {
        None
    };
    if let Some((_, comp_size)) = preserved_digest
        && comp_size > instance.max_nar_size_bytes
    {
        return Err(make_err!(
            Code::ResourceExhausted,
            "declared compressed NAR size {comp_size} exceeds the {}-byte limit",
            instance.max_nar_size_bytes
        ));
    }

    // `nar_sha256`/`nar_size` always name the canonical UNCOMPRESSED NAR;
    // `preserved` is `Some(compressed digest)` only when the client's
    // original compressed blob was stored too.
    let (nar_sha256, nar_size, preserved) = if let Some((file_sha256, file_size)) = preserved_digest
    {
        // Store the original compressed bytes verbatim — `ingest_nar_direct`
        // self-verifies sha256(compressed) == the name hash before it
        // commits — then decompress that blob into the canonical
        // uncompressed NAR. Storing the compressed blob is 1:1 with the wire
        // bytes, so it adds no amplification; the decompression-bomb cap
        // still applies to the DECOMPRESSED output inside `decompress_cas_blob`.
        ingest_nar_direct(instance, file_sha256, file_size, full_stream)
            .await
            .err_tip(|| "Storing the original compressed NAR blob")?;
        let comp_digest = DigestInfo::new(file_sha256, file_size);
        let (nar_sha256, nar_size) = decompress_cas_blob(instance, comp_digest, codec).await?;
        (nar_sha256, nar_size, Some((file_sha256, file_size)))
    } else if codec == NarCodec::None
        && let Some((digest_sha256, digest_size)) = bare_nar_digest_from_name(name, content_length)
    {
        // Fast path: the digest is known before the body is read, so the
        // bytes stream straight into the CAS. `ingest_nar_direct`
        // self-verifies sha256(body) == the named digest before committing,
        // so a mismatch is rejected even without a `verify{}` store. No
        // spool file involved.
        ingest_nar_direct(instance, digest_sha256, digest_size, full_stream).await?;
        (digest_sha256, digest_size, None)
    } else {
        let (nar_sha256, nar_size) = ingest_nar_spooled(instance, codec, full_stream).await?;
        // Defense in depth: a bare `{52 nix32}.nar` name commits to
        // sha256(content) in its stem. The direct path enforces that via
        // the `verify{}` wrapper; the spooled path (reached when the body
        // is chunked, so there is no Content-Length) must enforce it too.
        // Scoped to the identity codec only: a gzip-sniffed or
        // `.nar.xz`/`.nar.zst`/`.nar.bz2` stem is the COMPRESSED file hash
        // and intentionally differs from the decompressed NAR digest.
        if codec == NarCodec::None
            && let Some(expected) = bare_nar_stem_sha256(name)
            && expected != nar_sha256
        {
            return Err(make_input_err!(
                "bare NAR name stem does not match the uploaded content hash"
            ));
        }
        (nar_sha256, nar_size, None)
    };

    // Record the alias only after the CAS write(s) succeeded (an alias's
    // existence implies its target blob is, or was, present):
    // - a `.nar` name (bare identity or gzip-sniffed) aliases to the
    //   UNCOMPRESSED NAR, so a client's bare `nar/{hash}.nar` warm pull
    //   resolves;
    // - a preserved compressed name aliases to the ORIGINAL COMPRESSED
    //   blob, so the client's `nar/{hash}.nar.{ext}` warm pull resolves;
    // - a compressed name that was NOT preserved gets no alias — it would
    //   be a dead pointer to the uncompressed NAR under a compressed name,
    //   and the served narinfo advertises the canonical `.nar` instead.
    if name_codec == NarCodec::None || preserved.is_some() {
        let (alias_sha256, alias_size) = preserved.unwrap_or((nar_sha256, nar_size));
        with_sha256_ctx(instance.alias_store.update_oneshot(
            StoreKey::Str(Cow::Owned(alias_key(name))),
            format_alias(&alias_sha256, alias_size).into(),
        ))
        .await
        .err_tip(|| "Recording NAR alias")?;
    }
    instance.nar_bytes_ingested.add(nar_size);
    Ok(empty_response(StatusCode::CREATED))
}

/// Looks up the `Content-Encoding` recorded for a build log, or `None`
/// for identity (no record, or the empty record written by an
/// unencoded PUT to clear a stale value).
async fn lookup_log_encoding(
    instance: &NixCacheInstance,
    drv: &str,
) -> Result<Option<String>, Error> {
    let lookup = with_sha256_ctx(instance.alias_store.get_part_unchunked(
        StoreKey::Str(Cow::Owned(log_encoding_key(drv))),
        0,
        None,
    ))
    .await;
    let raw = match lookup {
        Ok(raw) => raw,
        Err(err) if err.code == Code::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).err_tip(|| format!("Looking up build-log encoding for '{drv}'"));
        }
    };
    if raw.is_empty() {
        return Ok(None);
    }
    let value = core::str::from_utf8(&raw).map_err(|e| {
        make_err!(
            Code::Internal,
            "Corrupt build-log encoding record for '{drv}': {e}"
        )
    })?;
    Ok(Some(value.to_string()))
}

/// `HEAD /log/{drv}`.
async fn head_build_log(
    State(instance): State<Arc<NixCacheInstance>>,
    UrlPath(drv): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(denied) = instance.authorize_read(&headers) {
        return denied;
    }
    head_build_log_inner(&instance, &drv)
        .await
        .unwrap_or_else(|err| error_response("HEAD build log", &err))
}

async fn head_build_log_inner(instance: &NixCacheInstance, drv: &str) -> Result<Response, Error> {
    if !is_valid_log_name(drv) {
        return Ok(empty_response(StatusCode::NOT_FOUND));
    }
    let found = with_sha256_ctx(
        instance
            .alias_store
            .has(StoreKey::Str(Cow::Owned(log_key(drv)))),
    )
    .await
    .err_tip(|| "Checking build-log existence in alias_store")?;
    if found.is_some() {
        Ok(empty_response(StatusCode::OK))
    } else {
        Ok(empty_response(StatusCode::NOT_FOUND))
    }
}

/// `GET /log/{drv}`.
async fn get_build_log(
    State(instance): State<Arc<NixCacheInstance>>,
    UrlPath(drv): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(denied) = instance.authorize_read(&headers) {
        return denied;
    }
    get_build_log_inner(&instance, &drv)
        .await
        .unwrap_or_else(|err| error_response("GET build log", &err))
}

async fn get_build_log_inner(instance: &NixCacheInstance, drv: &str) -> Result<Response, Error> {
    instance.aux_gets.inc();
    if !is_valid_log_name(drv) {
        return Ok(empty_response(StatusCode::NOT_FOUND));
    }
    // `Code::NotFound` propagates to a 404 through `error_response`.
    let log = with_sha256_ctx(instance.alias_store.get_part_unchunked(
        StoreKey::Str(Cow::Owned(log_key(drv))),
        0,
        None,
    ))
    .await
    .err_tip(|| "Fetching build log from alias_store")?;
    let encoding = lookup_log_encoding(instance, drv).await?;
    let mut response = Response::new(Body::from(log));
    let response_headers = response.headers_mut();
    response_headers.insert(CONTENT_TYPE, HeaderValue::from_static(LOG_CONTENT_TYPE));
    // Replay the Content-Encoding the log was uploaded with (nix's
    // `?log-compression=br` sets it). Every nix client sends
    // `Accept-Encoding: br, zstd, gzip, ...`, so the replay is safe.
    if let Some(encoding) = encoding {
        response_headers.insert(
            CONTENT_ENCODING,
            HeaderValue::from_str(&encoding).map_err(|e| {
                make_err!(
                    Code::Internal,
                    "Corrupt build-log encoding record for '{drv}': {e}"
                )
            })?,
        );
    }
    Ok(response)
}

/// `PUT /log/{drv}`.
async fn put_build_log(
    State(instance): State<Arc<NixCacheInstance>>,
    UrlPath(drv): UrlPath<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    if let Some(denied) = instance.authorize_write(&headers) {
        instance.rejected_puts.inc();
        return denied;
    }
    let response = put_build_log_inner(&instance, &drv, &headers, body)
        .await
        .unwrap_or_else(|err| error_response("PUT build log", &err));
    if response.status() == StatusCode::CREATED {
        instance.aux_puts.inc();
    } else {
        instance.rejected_puts.inc();
    }
    response
}

async fn put_build_log_inner(
    instance: &NixCacheInstance,
    drv: &str,
    headers: &HeaderMap,
    body: Body,
) -> Result<Response, Error> {
    if instance.read_only {
        return Ok(text_response(
            StatusCode::METHOD_NOT_ALLOWED,
            "cache is read-only",
        ));
    }
    if !is_valid_log_name(drv) {
        return Err(make_input_err!(
            "invalid build-log name '{drv}': expected a printable, slash-free derivation basename"
        ));
    }
    // nix's `?log-compression=br` uploads brotli bytes with a
    // `Content-Encoding` header: keep the encoded bytes verbatim and
    // remember the encoding for GET to replay.
    let content_encoding = match headers.get(CONTENT_ENCODING) {
        Some(value) => {
            let encoding = value
                .to_str()
                .map_err(|e| make_input_err!("Content-Encoding header is not ASCII: {e}"))?
                .trim()
                .to_string();
            // This value is stored and later replayed verbatim as the
            // response `Content-Encoding` for every reader of this shared
            // log, so only accept a curated set of codec tokens — never an
            // arbitrary attacker-chosen header value.
            if !is_allowed_log_encoding(&encoding) {
                return Err(make_input_err!(
                    "unsupported build-log Content-Encoding '{encoding}': expected some \
                     combination of identity, gzip, deflate, br, zstd, xz, or bzip2"
                ));
            }
            Some(encoding)
        }
        None => None,
    };
    let Some(body_bytes) = read_body_limited(body, MAX_LOG_BODY_BYTES).await? else {
        return Ok(text_response(
            StatusCode::PAYLOAD_TOO_LARGE,
            "build log exceeds 32 MiB",
        ));
    };
    // The encoding record is written first (empty means identity, which
    // also clears a stale encoding on overwrite), so a concurrent reader
    // of a brand-new log never sees bytes without their encoding.
    with_sha256_ctx(instance.alias_store.update_oneshot(
        StoreKey::Str(Cow::Owned(log_encoding_key(drv))),
        content_encoding.unwrap_or_default().into(),
    ))
    .await
    .err_tip(|| "Storing build-log encoding in alias_store")?;
    with_sha256_ctx(
        instance
            .alias_store
            .update_oneshot(StoreKey::Str(Cow::Owned(log_key(drv))), body_bytes),
    )
    .await
    .err_tip(|| "Storing build log in alias_store")?;
    Ok(empty_response(StatusCode::CREATED))
}

/// Streams an identity-compressed NAR body directly into the CAS under a
/// digest that is already known from the request.
async fn ingest_nar_direct<S>(
    instance: &NixCacheInstance,
    nar_sha256: [u8; 32],
    nar_size: u64,
    mut stream: S,
) -> Result<(), Error>
where
    S: Stream<Item = Result<Bytes, axum::Error>> + Send + Unpin,
{
    let digest = DigestInfo::new(nar_sha256, nar_size);
    let idle_timeout = instance.nar_upload_idle_timeout;
    let (mut tx, rx) = make_buf_channel_pair();
    let update_fut = with_sha256_ctx(instance.cas_store.update(
        digest,
        rx,
        UploadSizeInfo::ExactSize(nar_size),
    ));
    let pump_fut = async move {
        // Self-certify the upload: hash every byte and confirm it matches
        // the digest named in the request BEFORE committing (before EOF).
        // This makes CAS integrity independent of whether the operator
        // wrapped `cas_store` in a `verify{}` store — on a hash/size
        // mismatch we return without sending EOF, which drops `tx` and makes
        // `cas_store.update` fail, so the wrong bytes never commit under the
        // claimed digest (no silent cache poisoning even on a plain store).
        let mut hasher = DigestHasherFunc::Sha256.hasher();
        let mut written: u64 = 0;
        loop {
            // Bound each body read by the idle timeout so a stalled client
            // cannot pin the upload (and the CAS write half) indefinitely.
            let next = timeout(idle_timeout, stream.next()).await.map_err(|_| {
                make_err!(
                    Code::DeadlineExceeded,
                    "NAR upload stalled for more than {}s",
                    idle_timeout.as_secs()
                )
            })?;
            let Some(chunk) = next else { break };
            // A truncated/aborted client body is a client error (400), not
            // a backend failure (500) — matching the spooled path.
            let chunk =
                chunk.map_err(|e| make_input_err!("Failed to read NAR upload body: {e}"))?;
            if chunk.is_empty() {
                continue;
            }
            written = written.saturating_add(chunk.len() as u64);
            // Reject an over-long body before over-sending to the CAS write
            // half (whose `ExactSize` would also reject it, but failing here
            // keeps the error a clean 400).
            if written > nar_size {
                return Err(make_input_err!(
                    "NAR upload exceeds its declared size of {nar_size} bytes"
                ));
            }
            hasher.update(&chunk);
            tx.send(chunk)
                .await
                .err_tip(|| "Sending NAR bytes to cas_store")?;
        }
        let finalized = hasher.finalize_digest();
        let computed: &[u8; 32] = finalized.packed_hash();
        if written != nar_size || *computed != nar_sha256 {
            // Return WITHOUT sending EOF: dropping `tx` aborts the CAS write
            // so the mismatched bytes are never committed under the digest.
            return Err(make_input_err!(
                "uploaded NAR does not match its named digest \
                 (declared {nar_size} bytes, received {written})"
            ));
        }
        tx.send_eof().err_tip(|| "Sending EOF to cas_store")
    };
    let (update_res, pump_res) = futures::join!(update_fut, pump_fut);
    pump_res
        .merge(update_res)
        .err_tip(|| "Uploading NAR to cas_store")?;
    Ok(())
}

/// Stream-decompresses a NAR body while hashing, spooling the decompressed
/// bytes to a temp file; once the digest is known the spool file is
/// streamed into the CAS and removed (also on every error path, via
/// [`SpoolFileGuard`]).
async fn ingest_nar_spooled<S>(
    instance: &NixCacheInstance,
    codec: NarCodec,
    stream: S,
) -> Result<([u8; 32], u64), Error>
where
    S: Stream<Item = Result<Bytes, axum::Error>> + Send + Unpin,
{
    let reader = StreamReader::new(
        stream.map_err(|e| std::io::Error::other(format!("Failed to read NAR upload body: {e}"))),
    );
    let mut decoder: Box<dyn AsyncRead + Send + Unpin> = match codec {
        NarCodec::None => Box::new(reader),
        NarCodec::Gzip => Box::new(GzipDecoder::new(reader)),
        NarCodec::Xz => Box::new(XzDecoder::new(reader)),
        NarCodec::Zstd => Box::new(ZstdDecoder::new(reader)),
        NarCodec::Bzip2 => Box::new(BzDecoder::new(reader)),
    };

    let spool_path = instance
        .spool_dir
        .join(format!("{SPOOL_FILE_PREFIX}{}.nar", Uuid::new_v4()));
    let spool_guard = SpoolFileGuard::new(spool_path.clone());
    let mut spool_file = fs::create_file(&spool_path)
        .await
        .err_tip(|| format!("Creating NAR spool file {}", spool_path.display()))?;

    let max_nar_size = instance.max_nar_size_bytes;
    let idle_timeout = instance.nar_upload_idle_timeout;
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    let mut chunk = BytesMut::with_capacity(fs::DEFAULT_READ_BUFF_SIZE);
    let mut written: u64 = 0;
    loop {
        chunk.clear();
        // A stalled/slowloris upload must not pin the spool file and its
        // descriptor forever: bound each read by the idle timeout, which
        // resets on every chunk actually received.
        let read = timeout(idle_timeout, decoder.read_buf(&mut chunk))
            .await
            .map_err(|_| {
                make_err!(
                    Code::DeadlineExceeded,
                    "NAR upload stalled for more than {}s",
                    idle_timeout.as_secs()
                )
            })?
            // Decompression failures are client-data problems (400), not
            // internal errors.
            .map_err(|e| make_input_err!("Failed to decompress NAR upload: {e}"))?;
        if read == 0 {
            break;
        }
        // Abort a decompression bomb before the spool can exceed the cap:
        // the SpoolFileGuard deletes the partial file on this error return.
        written = written.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if written > max_nar_size {
            return Err(make_err!(
                Code::ResourceExhausted,
                "uploaded NAR decompresses past the {max_nar_size}-byte limit"
            ));
        }
        hasher.update(&chunk);
        spool_file
            .write_all(&chunk)
            .await
            .err_tip(|| "Writing decompressed NAR to spool file")?;
    }
    spool_file
        .flush()
        .await
        .err_tip(|| "Flushing NAR spool file")?;

    let digest = hasher.finalize_digest();
    let nar_size = digest.size_bytes();
    with_sha256_ctx(slow_update_store_with_file(
        instance.cas_store.as_store_driver_pin(),
        digest,
        &mut spool_file,
        UploadSizeInfo::ExactSize(nar_size),
    ))
    .await
    .err_tip(|| "Uploading spooled NAR to cas_store")?;
    drop(spool_file);
    spool_guard.cleanup().await;

    let nar_sha256_ref: &[u8; 32] = digest.packed_hash();
    Ok((*nar_sha256_ref, nar_size))
}

/// Streams the compressed blob at `comp_digest` out of the CAS through the
/// matching decoder, sha256-hashing and counting the DECOMPRESSED output
/// while spooling it, then writes the canonical uncompressed NAR into the
/// CAS under `DigestInfo(nar_sha256, nar_size)` and returns that pair.
///
/// Used by the `preserve_upload_compression` ingest path after the
/// client's original compressed blob has been stored: the uncompressed
/// NAR is what the record's `ActionResult` (and therefore action-cache
/// completeness) keys on. The decompressed size is capped at
/// `max_nar_size_bytes` — the same decompression-bomb guard the spooled
/// path enforces — and spool files are removed on every path via
/// [`SpoolFileGuard`]. The compressed bytes are client-supplied, so a
/// decode failure is a client-data error (400), matching
/// [`ingest_nar_spooled`].
async fn decompress_cas_blob(
    instance: &NixCacheInstance,
    comp_digest: DigestInfo,
    codec: NarCodec,
) -> Result<([u8; 32], u64), Error> {
    let (tx, rx) = make_buf_channel_pair();
    let ctx = sha256_hasher_ctx()?;
    let cas_store = instance.cas_store.clone();
    let read_task = spawn!(
        "nix_cache_nar_decompress_read",
        async move {
            if let Err(err) = cas_store.get_part(comp_digest, tx, 0, None).await {
                // Dropping `tx` without an EOF surfaces the failure as a
                // read error in the decoder loop below.
                warn!(
                    ?err,
                    ?comp_digest,
                    "Failed streaming compressed NAR from CAS for decompression"
                );
            }
        }
        .with_context(ctx)
    );
    let reader = StreamReader::new(rx);
    let mut decoder: Box<dyn AsyncRead + Send + Unpin> = match codec {
        NarCodec::None => Box::new(reader),
        NarCodec::Gzip => Box::new(GzipDecoder::new(reader)),
        NarCodec::Xz => Box::new(XzDecoder::new(reader)),
        NarCodec::Zstd => Box::new(ZstdDecoder::new(reader)),
        NarCodec::Bzip2 => Box::new(BzDecoder::new(reader)),
    };

    let spool_path = instance
        .spool_dir
        .join(format!("{SPOOL_FILE_PREFIX}{}.nar", Uuid::new_v4()));
    let spool_guard = SpoolFileGuard::new(spool_path.clone());
    let mut spool_file = fs::create_file(&spool_path)
        .await
        .err_tip(|| format!("Creating NAR spool file {}", spool_path.display()))?;

    let max_nar_size = instance.max_nar_size_bytes;
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    let mut chunk = BytesMut::with_capacity(fs::DEFAULT_READ_BUFF_SIZE);
    let mut written: u64 = 0;
    loop {
        chunk.clear();
        // Decompression failures are client-data problems (400): the
        // compressed bytes came from the client's upload.
        let read = decoder
            .read_buf(&mut chunk)
            .await
            .map_err(|e| make_input_err!("Failed to decompress preserved NAR upload: {e}"))?;
        if read == 0 {
            break;
        }
        // Abort a decompression bomb before the spool can exceed the cap:
        // the SpoolFileGuard deletes the partial file on this error return.
        written = written.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if written > max_nar_size {
            return Err(make_err!(
                Code::ResourceExhausted,
                "uploaded NAR decompresses past the {max_nar_size}-byte limit"
            ));
        }
        hasher.update(&chunk);
        spool_file
            .write_all(&chunk)
            .await
            .err_tip(|| "Writing decompressed NAR to spool file")?;
    }
    spool_file
        .flush()
        .await
        .err_tip(|| "Flushing NAR spool file")?;
    drop(read_task);

    let digest = hasher.finalize_digest();
    let nar_size = digest.size_bytes();
    with_sha256_ctx(slow_update_store_with_file(
        instance.cas_store.as_store_driver_pin(),
        digest,
        &mut spool_file,
        UploadSizeInfo::ExactSize(nar_size),
    ))
    .await
    .err_tip(|| "Uploading decompressed NAR to cas_store")?;
    drop(spool_file);
    spool_guard.cleanup().await;

    let nar_sha256_ref: &[u8; 32] = digest.packed_hash();
    Ok((*nar_sha256_ref, nar_size))
}

/// Returns the `(file_sha256, file_size)` of the zstd artifact for the
/// NAR at `nar_digest`, reusing the previous record's transcode when it
/// still describes this exact NAR and its blob is still in the CAS, and
/// transcoding once otherwise.
async fn resolve_or_transcode_zstd(
    instance: &NixCacheInstance,
    hash: &str,
    path_info: &NixPathInfo,
    nar_digest: DigestInfo,
) -> Result<([u8; 32], u64), Error> {
    if let Some(reused) = reusable_zstd_fields(instance, hash, path_info).await? {
        return Ok(reused);
    }

    // Coalesce concurrent transcodes of the SAME NAR: exactly one PUT
    // becomes the leader and transcodes; the rest await its result. This
    // kills the transcode storm and the redundant re-encode of one NAR
    // shared by many distinct store paths (whose store-path hashes differ,
    // so the reuse fast path above never finds them).
    let (slot, is_leader) = {
        let mut inflight = instance
            .transcode_inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(existing) = inflight.get(&nar_digest) {
            (Arc::clone(existing), false)
        } else {
            let slot = Arc::new(TranscodeInflight {
                result: StdMutex::new(None),
                notify: Notify::new(),
            });
            inflight.insert(nar_digest, Arc::clone(&slot));
            (slot, true)
        }
    };

    if is_leader {
        // Bound concurrent CPU/spool-heavy transcodes; queue (do not
        // load-shed) when saturated so a burst of distinct NARs is
        // serialized rather than rejected.
        let result = match Arc::clone(&instance.transcode_semaphore)
            .acquire_owned()
            .await
        {
            Ok(_permit) => transcode_nar_to_zstd(instance, nar_digest).await,
            Err(err) => Err(make_err!(
                Code::Internal,
                "transcode semaphore closed: {err}"
            )),
        };
        // Publish the result and wake every follower BEFORE removing the
        // slot, so a follower already holding this slot always observes it.
        {
            let mut cell = slot
                .result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *cell = Some(result.clone());
        }
        slot.notify.notify_waiters();
        instance
            .transcode_inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&nar_digest);
        result
    } else {
        // Follower: register the waiter (`enable`) BEFORE reading the
        // result cell so a publish that races with the read is never
        // missed, then re-check after each wake.
        loop {
            let notified = slot.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            // Clone the result out of the guard into an owned value so the
            // lock is released before the `if let` (avoids holding it across
            // the branch / the await below).
            let maybe_result = slot
                .result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if let Some(result) = maybe_result {
                return result;
            }
            notified.await;
        }
    }
}

/// Looks for a previous record of the same store path whose `file_*`
/// trio can be reused: same uncompressed NAR, zstd compression, and the
/// compressed blob still present in the CAS (if it was evicted, the
/// caller re-transcodes so freshly pushed paths self-heal). A missing,
/// corrupt, or mismatched previous record just means "transcode again";
/// it never fails the PUT.
async fn reusable_zstd_fields(
    instance: &NixCacheInstance,
    hash: &str,
    path_info: &NixPathInfo,
) -> Result<Option<([u8; 32], u64)>, Error> {
    let lookup = with_sha256_ctx(instance.path_info_store.get_part_unchunked(
        StoreKey::new_str(hash),
        0,
        None,
    ))
    .await;
    let record = match lookup {
        Ok(record) => record,
        Err(err) if err.code == Code::NotFound => return Ok(None),
        Err(err) => {
            return Err(err)
                .err_tip(|| "Checking the previous narinfo record for a reusable transcode");
        }
    };
    let Ok(previous) = NixPathInfo::decode_record(&record) else {
        return Ok(None);
    };
    if previous.nar_sha256 != path_info.nar_sha256
        || previous.nar_size != path_info.nar_size
        || previous.file_compression != ZSTD_COMPRESSION_NAME
        || previous.file_size == 0
    {
        return Ok(None);
    }
    let Ok(file_sha256) = <[u8; 32]>::try_from(previous.file_sha256.as_slice()) else {
        return Ok(None);
    };
    let file_digest = DigestInfo::new(file_sha256, previous.file_size);
    let present = with_sha256_ctx(instance.cas_store.has(file_digest))
        .await
        .err_tip(|| "Checking transcoded NAR existence in cas_store")?;
    Ok(present.map(|_| (file_sha256, previous.file_size)))
}

/// Streams the uncompressed NAR at `nar_digest` out of the CAS through a
/// zstd encoder at the instance's configured level, sha256-hashing and
/// counting the COMPRESSED output while spooling it, then writes the
/// compressed blob into the CAS under `DigestInfo(file_sha256,
/// file_size)` and returns that pair. Spool files are removed on every
/// path via [`SpoolFileGuard`].
async fn transcode_nar_to_zstd(
    instance: &NixCacheInstance,
    nar_digest: DigestInfo,
) -> Result<([u8; 32], u64), Error> {
    let (tx, rx) = make_buf_channel_pair();
    let ctx = sha256_hasher_ctx()?;
    let cas_store = instance.cas_store.clone();
    let read_task = spawn!(
        "nix_cache_nar_transcode_read",
        async move {
            if let Err(err) = cas_store.get_part(nar_digest, tx, 0, None).await {
                // Dropping `tx` without an EOF surfaces the failure as a
                // read error in the encoder loop below.
                warn!(
                    ?err,
                    ?nar_digest,
                    "Failed streaming NAR from CAS for zstd transcode"
                );
            }
        }
        .with_context(ctx)
    );
    let reader = StreamReader::new(rx);
    let mut encoder = ZstdEncoder::with_quality(reader, Level::Precise(instance.compression_level));

    let spool_path = instance
        .spool_dir
        .join(format!("{}.nar.zst", Uuid::new_v4()));
    let spool_guard = SpoolFileGuard::new(spool_path.clone());
    let mut spool_file = fs::create_file(&spool_path)
        .await
        .err_tip(|| format!("Creating zstd spool file {}", spool_path.display()))?;

    let max_nar_size = instance.max_nar_size_bytes;
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    let mut chunk = BytesMut::with_capacity(fs::DEFAULT_READ_BUFF_SIZE);
    let mut written: u64 = 0;
    loop {
        chunk.clear();
        // The input is our own CAS: any failure here is a server-side
        // problem (500 through `error_response`), never a client error.
        let read = encoder.read_buf(&mut chunk).await.map_err(|e| {
            make_err!(
                Code::Internal,
                "Failed to zstd-transcode NAR {nar_digest:?}: {e}"
            )
        })?;
        if read == 0 {
            break;
        }
        // Defense in depth: the input NAR is already capped at ingest and
        // compression only shrinks, so this never trips for real data, but
        // it bounds the spool against any pathological encoder blow-up.
        written = written.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if written > max_nar_size {
            return Err(make_err!(
                Code::ResourceExhausted,
                "zstd-transcoded NAR exceeds the {max_nar_size}-byte limit"
            ));
        }
        hasher.update(&chunk);
        spool_file
            .write_all(&chunk)
            .await
            .err_tip(|| "Writing zstd-transcoded NAR to spool file")?;
    }
    spool_file
        .flush()
        .await
        .err_tip(|| "Flushing zstd spool file")?;
    drop(read_task);

    let file_digest = hasher.finalize_digest();
    let file_size = file_digest.size_bytes();
    with_sha256_ctx(slow_update_store_with_file(
        instance.cas_store.as_store_driver_pin(),
        file_digest,
        &mut spool_file,
        UploadSizeInfo::ExactSize(file_size),
    ))
    .await
    .err_tip(|| "Uploading zstd-transcoded NAR to cas_store")?;
    drop(spool_file);
    spool_guard.cleanup().await;

    let file_sha256: &[u8; 32] = file_digest.packed_hash();
    Ok((*file_sha256, file_size))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use base64::Engine as _;
    use bytes::Bytes;
    use nativelink_config::stores::MemorySpec;
    use nativelink_error::{Code, Error, make_err, make_input_err};
    use nativelink_macro::nativelink_test;
    use nativelink_store::memory_store::MemoryStore;
    use pretty_assertions::assert_eq;
    use tokio::io::{AsyncRead, AsyncReadExt};
    use uuid::Uuid;

    use super::{
        Counter, CounterWithTime, DEFAULT_SPOOL_DIR_NAME, DEFAULT_ZSTD_LEVEL, DigestHasher,
        DigestHasherFunc, DigestInfo, HeaderMap, HeaderValue, NarCodec, NixCacheInstance,
        NixPathInfo, RangeRequest, SPOOL_FILE_PREFIX, ServeCompression, StatusCode, Store,
        StoreLike, bare_nar_digest_from_name, bare_nar_stem_sha256, canonical_nar_name,
        canonical_nar_zst_name, codec_for_name, compressed_upload_digest, constant_time_eq,
        error_response, extract_token, ingest_nar_direct, ingest_nar_spooled,
        is_allowed_log_encoding, is_valid_log_name, is_valid_nar_upload_name, listing_name_hash,
        narinfo_name_hash, nixbase32, parse_canonical_any, parse_canonical_nar_name, parse_range,
        prepare_spool_dir, resolve_or_transcode_zstd, sha256_of, token_matches_any,
        transcode_nar_to_zstd, with_sha256_ctx,
    };

    fn test_instance() -> Arc<NixCacheInstance> {
        build_test_instance(32 * 1024 * 1024 * 1024, 256)
    }

    fn build_test_instance(
        max_nar_size_bytes: u64,
        max_nar_streams: usize,
    ) -> Arc<NixCacheInstance> {
        // A fresh directory directly under the system temp dir: the shared
        // `<tmp>/nativelink-nix-spool` parent may belong to another user.
        let spool_dir = std::env::temp_dir().join(format!(
            "{DEFAULT_SPOOL_DIR_NAME}-unit-test-{}",
            Uuid::new_v4()
        ));
        prepare_spool_dir(&spool_dir).expect("prepare spool dir");
        Arc::new(NixCacheInstance {
            instance_name: "unit-test".to_string(),
            mount_path: "/nix/unit-test".to_string(),
            cas_store: Store::new(MemoryStore::new(&MemorySpec::default())),
            path_info_store: Store::new(MemoryStore::new(&MemorySpec::default())),
            alias_store: Store::new(MemoryStore::new(&MemorySpec::default())),
            signing_keys: vec![],
            store_dir: "/nix/store".to_string(),
            priority: 40,
            want_mass_query: true,
            read_only: false,
            spool_dir,
            read_token_hashes: vec![],
            write_token_hashes: vec![],
            serve_compression: ServeCompression::None,
            compression_level: DEFAULT_ZSTD_LEVEL,
            preserve_upload_compression: true,
            max_nar_size_bytes,
            nar_upload_idle_timeout: core::time::Duration::from_mins(1),
            nar_stream_semaphore: Arc::new(tokio::sync::Semaphore::new(max_nar_streams)),
            transcode_semaphore: Arc::new(tokio::sync::Semaphore::new(8)),
            transcode_inflight: std::sync::Mutex::new(std::collections::HashMap::new()),
            upstreams: vec![],
            http_client: {
                drop(rustls::crypto::ring::default_provider().install_default());
                reqwest::Client::new()
            },
            upstream_timeout: core::time::Duration::from_secs(30),
            upstream_negative_ttl: core::time::Duration::from_mins(1),
            negative_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
            readthrough_inflight: std::sync::Mutex::new(std::collections::HashMap::new()),
            narinfo_gets: CounterWithTime::default(),
            narinfo_puts: CounterWithTime::default(),
            nar_gets: CounterWithTime::default(),
            nar_puts: CounterWithTime::default(),
            nar_bytes_served: Counter::default(),
            nar_bytes_ingested: Counter::default(),
            aux_gets: CounterWithTime::default(),
            aux_puts: CounterWithTime::default(),
            rejected_puts: CounterWithTime::default(),
            unauthorized_requests: CounterWithTime::default(),
            upstream_hits: CounterWithTime::default(),
            upstream_misses: CounterWithTime::default(),
            upstream_errors: CounterWithTime::default(),
            upstream_rejected_unsigned: CounterWithTime::default(),
            upstream_nar_bytes: Counter::default(),
        })
    }

    async fn compress(codec: NarCodec, payload: &[u8]) -> Vec<u8> {
        use async_compression::tokio::bufread::{BzEncoder, GzipEncoder, XzEncoder, ZstdEncoder};
        let mut encoder: Box<dyn AsyncRead + Send + Unpin + '_> = match codec {
            NarCodec::None => Box::new(payload),
            NarCodec::Gzip => Box::new(GzipEncoder::new(payload)),
            NarCodec::Xz => Box::new(XzEncoder::new(payload)),
            NarCodec::Zstd => Box::new(ZstdEncoder::new(payload)),
            NarCodec::Bzip2 => Box::new(BzEncoder::new(payload)),
        };
        let mut out = Vec::new();
        encoder.read_to_end(&mut out).await.expect("compress");
        out
    }

    /// Proves every compression backend links and functions at runtime
    /// (notably xz's liblzma), that the spooled path recovers the exact
    /// digest of the decompressed bytes, and that no spool files leak.
    #[nativelink_test]
    async fn spooled_ingest_decompresses_every_codec() -> Result<(), Error> {
        let instance = test_instance();
        let payload = b"nar payload for spooled codec round trips ".repeat(64);
        let expected_size = u64::try_from(payload.len()).expect("size fits in u64");
        let expected_sha256: [u8; 32] = {
            let mut hasher = DigestHasherFunc::Sha256.hasher();
            hasher.update(&payload);
            let digest = hasher.finalize_digest();
            let hash_ref: &[u8; 32] = digest.packed_hash();
            *hash_ref
        };
        for codec in [
            NarCodec::None,
            NarCodec::Gzip,
            NarCodec::Xz,
            NarCodec::Zstd,
            NarCodec::Bzip2,
        ] {
            let compressed = compress(codec, &payload).await;
            let stream = futures::stream::iter([Ok::<_, axum::Error>(Bytes::from(compressed))]);
            let (nar_sha256, nar_size) = ingest_nar_spooled(&instance, codec, stream)
                .await
                .unwrap_or_else(|e| panic!("ingest for {codec:?}: {e:?}"));
            assert_eq!(nar_size, expected_size, "size for {codec:?}");
            assert_eq!(nar_sha256, expected_sha256, "hash for {codec:?}");
            let stored = with_sha256_ctx(instance.cas_store.get_part_unchunked(
                DigestInfo::new(nar_sha256, nar_size),
                0,
                None,
            ))
            .await
            .expect("stored NAR readable");
            assert_eq!(stored.as_ref(), payload.as_slice(), "content for {codec:?}");
        }
        let leftovers = std::fs::read_dir(&instance.spool_dir)
            .expect("read spool dir")
            .count();
        assert_eq!(leftovers, 0, "spool files leaked");
        Ok(())
    }

    /// Corrupt compressed data must be a client error (400 via the error
    /// mapping) and must not leak spool files.
    #[nativelink_test]
    async fn spooled_ingest_rejects_corrupt_compressed_data() -> Result<(), Error> {
        let instance = test_instance();
        for codec in [
            NarCodec::Gzip,
            NarCodec::Xz,
            NarCodec::Zstd,
            NarCodec::Bzip2,
        ] {
            let stream = futures::stream::iter([Ok::<_, axum::Error>(Bytes::from_static(
                b"definitely not compressed data",
            ))]);
            let err = ingest_nar_spooled(&instance, codec, stream)
                .await
                .expect_err("corrupt data must fail");
            assert_eq!(err.code, Code::InvalidArgument, "for {codec:?}");
        }
        let leftovers = std::fs::read_dir(&instance.spool_dir)
            .expect("read spool dir")
            .count();
        assert_eq!(leftovers, 0, "spool files leaked");
        Ok(())
    }

    #[test]
    fn parse_range_accepts_single_ranges() {
        assert_eq!(
            parse_range(Some("bytes=0-99"), 1000),
            RangeRequest::Range { start: 0, end: 99 }
        );
        assert_eq!(
            parse_range(Some("bytes=500-"), 1000),
            RangeRequest::Range {
                start: 500,
                end: 999
            }
        );
        // End is clamped to the body.
        assert_eq!(
            parse_range(Some("bytes=500-100000"), 1000),
            RangeRequest::Range {
                start: 500,
                end: 999
            }
        );
        assert_eq!(
            parse_range(Some("bytes=999-999"), 1000),
            RangeRequest::Range {
                start: 999,
                end: 999
            }
        );
    }

    #[test]
    fn parse_range_ignores_invalid_and_multi_ranges() {
        for header in [
            "bytes=0-99,200-299", // multi-range
            "bytes=-500",         // suffix range
            "bytes=abc-",         // non-numeric start
            "bytes=0-abc",        // non-numeric end
            "bytes=5-4",          // end < start
            "items=0-99",         // unknown unit
            "bytes=",             // empty spec
        ] {
            assert_eq!(
                parse_range(Some(header), 1000),
                RangeRequest::Full,
                "for '{header}'"
            );
        }
        assert_eq!(parse_range(None, 1000), RangeRequest::Full);
    }

    #[test]
    fn parse_range_unsatisfiable_when_start_at_or_past_end() {
        assert_eq!(
            parse_range(Some("bytes=1000-"), 1000),
            RangeRequest::Unsatisfiable
        );
        assert_eq!(
            parse_range(Some("bytes=5000-6000"), 1000),
            RangeRequest::Unsatisfiable
        );
        // Zero-length body: any start is past the end.
        assert_eq!(
            parse_range(Some("bytes=0-"), 0),
            RangeRequest::Unsatisfiable
        );
    }

    #[test]
    fn error_response_maps_codes_to_statuses() {
        assert_eq!(
            error_response("test", &make_err!(Code::NotFound, "missing")).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            error_response("test", &make_input_err!("bad input")).status(),
            StatusCode::BAD_REQUEST
        );
        // Backend failure must never map to 404 — and absent must never map
        // to a 5xx.
        assert_eq!(
            error_response("test", &make_err!(Code::Internal, "backend broke")).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            error_response("test", &make_err!(Code::Unavailable, "backend down")).status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        // Resource-exhaustion and deadline map to client statuses.
        assert_eq!(
            error_response("test", &make_err!(Code::ResourceExhausted, "too big")).status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            error_response("test", &make_err!(Code::DeadlineExceeded, "stalled")).status(),
            StatusCode::REQUEST_TIMEOUT
        );
    }

    #[test]
    fn narinfo_name_hash_validates_shape() {
        assert_eq!(
            narinfo_name_hash("p4pclmv1gyja5kzc26npqpia1qqxrf0l.narinfo"),
            Some("p4pclmv1gyja5kzc26npqpia1qqxrf0l")
        );
        // Wrong extension, wrong length, and non-nix32 characters.
        assert_eq!(
            narinfo_name_hash("p4pclmv1gyja5kzc26npqpia1qqxrf0l.nar"),
            None
        );
        assert_eq!(
            narinfo_name_hash("p4pclmv1gyja5kzc26npqpia1qqxrf0.narinfo"),
            None
        );
        assert_eq!(
            narinfo_name_hash("e4pclmv1gyja5kzc26npqpia1qqxrf0l.narinfo"),
            None
        );
        assert_eq!(narinfo_name_hash("nix-cache-info"), None);
        assert_eq!(narinfo_name_hash(".narinfo"), None);
    }

    #[test]
    fn nar_upload_names_must_be_hash_shaped() {
        const HELLO_NIX32: &str = "094qif9n4cq4fdg459qzbhg1c6wywawwaaivx0k0x8xhbyx4vwic";
        for good in [
            format!("{HELLO_NIX32}.nar"),
            format!("{HELLO_NIX32}.nar.xz"),
            format!("{HELLO_NIX32}.nar.zst"),
            format!("{HELLO_NIX32}.nar.bz2"),
            format!("{HELLO_NIX32}-42.nar"),
        ] {
            assert!(is_valid_nar_upload_name(&good), "for '{good}'");
        }
        for bad in [
            "../../escape.nar".to_string(),
            "random.nar".to_string(),
            format!("{}.nar", "z".repeat(53)),
            format!("{}.nar", "z".repeat(31)),
            // 'e' is outside the nix32 alphabet.
            format!("e{}.nar", &HELLO_NIX32[1..]),
            format!("{HELLO_NIX32}-.nar"),
            format!("{HELLO_NIX32}-4x2.nar"),
            format!("{HELLO_NIX32}.tar"),
            format!("{HELLO_NIX32}.nar.gz"),
            String::new(),
        ] {
            assert!(!is_valid_nar_upload_name(&bad), "for '{bad}'");
        }
    }

    #[test]
    fn bare_nar_digest_requires_hash_shaped_name() {
        // sha256("hello") in nix32 (52 chars).
        const HELLO_NIX32: &str = "094qif9n4cq4fdg459qzbhg1c6wywawwaaivx0k0x8xhbyx4vwic";
        const HELLO_HEX: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let hello_hash: [u8; 32] = hex::decode(HELLO_HEX)
            .expect("valid hex")
            .try_into()
            .expect("32 bytes");

        // 52-char file-hash name + Content-Length.
        assert_eq!(
            bare_nar_digest_from_name(&format!("{HELLO_NIX32}.nar"), Some(123)),
            Some((hello_hash, 123))
        );
        // Same name without Content-Length: digest unknown, spool path.
        assert_eq!(
            bare_nar_digest_from_name(&format!("{HELLO_NIX32}.nar"), None),
            None
        );
        // Canonical names need no Content-Length.
        assert_eq!(
            bare_nar_digest_from_name(&format!("{HELLO_NIX32}-42.nar"), None),
            Some((hello_hash, 42))
        );
        // Non-hash-shaped names never resolve.
        assert_eq!(bare_nar_digest_from_name("random.nar", Some(123)), None);
        assert_eq!(
            bare_nar_digest_from_name(&format!("{HELLO_NIX32}.nar.xz"), Some(123)),
            None
        );
    }

    fn auth_headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            super::AUTHORIZATION,
            HeaderValue::from_str(value).expect("header value"),
        );
        headers
    }

    #[test]
    fn constant_time_eq_gates_length_and_content() {
        assert!(constant_time_eq(b"", b""));
        assert!(constant_time_eq(b"same bytes", b"same bytes"));
        // Same length, different content.
        assert!(!constant_time_eq(b"same length!", b"same lengthX"));
        // Length mismatch fails at the gate, including shared prefixes.
        assert!(!constant_time_eq(b"short", b"a longer slice"));
        assert!(!constant_time_eq(b"prefix", b"prefix-and-more"));
        assert!(!constant_time_eq(b"", b"x"));
    }

    #[test]
    fn token_matches_any_compares_every_candidate() {
        let a = sha256_of(b"token-a");
        let b = sha256_of(b"token-b");
        let c = sha256_of(b"token-c");
        assert!(token_matches_any(&a, &[a]));
        assert!(token_matches_any(&b, &[a, b, c]));
        assert!(!token_matches_any(&sha256_of(b"other"), &[a, b, c]));
        assert!(!token_matches_any(&a, &[]));
    }

    #[test]
    fn extract_token_accepts_bearer_and_basic() {
        assert_eq!(
            extract_token(&auth_headers("Bearer sekrit")).as_deref(),
            Some("sekrit")
        );
        // Scheme matching is case-insensitive; padding is trimmed.
        assert_eq!(
            extract_token(&auth_headers("bearer  sekrit ")).as_deref(),
            Some("sekrit")
        );
        // Basic auth: the token is the PASSWORD and the username is
        // ignored — this is how stock nix authenticates via netrc.
        let basic = format!("Basic {}", super::BASE64.encode("ignored-user:sekrit"));
        assert_eq!(
            extract_token(&auth_headers(&basic)).as_deref(),
            Some("sekrit")
        );
        // Passwords may contain ':' — only the first one splits.
        let colons = format!("basic {}", super::BASE64.encode("u:pa:ss"));
        assert_eq!(
            extract_token(&auth_headers(&colons)).as_deref(),
            Some("pa:ss")
        );
    }

    #[test]
    fn extract_token_rejects_garbage() {
        assert_eq!(extract_token(&HeaderMap::new()), None);
        for bad in [
            "Bearer",                 // no token at all
            "Bearer ",                // empty token
            "Basic",                  // no payload
            "Basic !!!not-base64!!!", // invalid base64
            "Digest abc",             // unsupported scheme
            "sekrit",                 // no scheme
        ] {
            assert_eq!(extract_token(&auth_headers(bad)), None, "for '{bad}'");
        }
        // A Basic payload without ':' has no password.
        let no_colon = format!("Basic {}", super::BASE64.encode("just-a-user"));
        assert_eq!(extract_token(&auth_headers(&no_colon)), None);
        // A Basic payload that is not UTF-8.
        let not_utf8 = format!("Basic {}", super::BASE64.encode([0xff, 0xfe, b':', 0xff]));
        assert_eq!(extract_token(&auth_headers(&not_utf8)), None);
    }

    #[test]
    fn log_names_must_be_printable_and_slash_free() {
        for good in [
            "q0hpd8s75g1h17yr8zqp1yf8sc9g4gp2-nix-2.34.7.drv",
            "x.drv",
            "log-with_odd~chars!.drv",
        ] {
            assert!(is_valid_log_name(good), "for '{good}'");
        }
        for bad in [
            "",
            "a/b.drv",
            "../escape.drv",
            "a..b.drv",
            "has space.drv",
            "has\ttab.drv",
            "non-ascii-\u{e9}.drv",
        ] {
            assert!(!is_valid_log_name(bad), "for '{bad}'");
        }
        // An unbounded name would overflow the derived filesystem key
        // (`log-enc:{drv}`), so lengths past the cap are rejected.
        assert!(is_valid_log_name(&"a".repeat(240)));
        assert!(!is_valid_log_name(&"a".repeat(241)));
    }

    #[test]
    fn listing_name_hash_validates_shape() {
        assert_eq!(
            listing_name_hash("p4pclmv1gyja5kzc26npqpia1qqxrf0l.ls"),
            Some("p4pclmv1gyja5kzc26npqpia1qqxrf0l")
        );
        // Wrong extension, wrong length, and non-nix32 characters.
        assert_eq!(
            listing_name_hash("p4pclmv1gyja5kzc26npqpia1qqxrf0l.narinfo"),
            None
        );
        assert_eq!(
            listing_name_hash("p4pclmv1gyja5kzc26npqpia1qqxrf0.ls"),
            None
        );
        assert_eq!(
            listing_name_hash("e4pclmv1gyja5kzc26npqpia1qqxrf0l.ls"),
            None
        );
        assert_eq!(listing_name_hash(".ls"), None);
    }

    /// Transcode round trip: the compressed blob lands in the CAS under
    /// `DigestInfo(sha256(zstd(nar)), zstd_size)` and decodes back to
    /// the original NAR bytes; no spool files leak.
    #[nativelink_test]
    async fn transcode_produces_addressable_zstd_artifact() -> Result<(), Error> {
        use async_compression::tokio::bufread::ZstdDecoder;
        let instance = test_instance();
        let payload = b"nar payload for the zstd transcode round trip ".repeat(64);
        let nar_size = u64::try_from(payload.len()).expect("size fits in u64");
        let nar_digest = DigestInfo::new(sha256_of(&payload), nar_size);
        with_sha256_ctx(
            instance
                .cas_store
                .update_oneshot(nar_digest, Bytes::from(payload.clone())),
        )
        .await
        .expect("seed NAR");

        let (file_sha256, file_size) = transcode_nar_to_zstd(&instance, nar_digest)
            .await
            .expect("transcode");
        let stored = with_sha256_ctx(instance.cas_store.get_part_unchunked(
            DigestInfo::new(file_sha256, file_size),
            0,
            None,
        ))
        .await
        .expect("compressed blob readable");
        assert_eq!(u64::try_from(stored.len()).expect("stored len"), file_size);
        assert_eq!(sha256_of(&stored), file_sha256);

        let mut decoder = ZstdDecoder::new(stored.as_ref());
        let mut decoded = Vec::new();
        decoder
            .read_to_end(&mut decoded)
            .await
            .expect("zstd decodes");
        assert_eq!(decoded, payload);
        let leftovers = std::fs::read_dir(&instance.spool_dir)
            .expect("read spool dir")
            .count();
        assert_eq!(leftovers, 0, "spool files leaked");
        Ok(())
    }

    /// Transcoding a NAR that is not in the CAS must surface as a
    /// server-side error (a 500 through the error mapping), never hang
    /// or turn into a client error.
    #[nativelink_test]
    async fn transcode_of_missing_nar_is_internal_error() -> Result<(), Error> {
        let instance = test_instance();
        let missing = DigestInfo::new(sha256_of(b"never uploaded"), 42);
        let err = transcode_nar_to_zstd(&instance, missing)
            .await
            .expect_err("missing NAR must fail");
        assert_eq!(err.code, Code::Internal);
        let leftovers = std::fs::read_dir(&instance.spool_dir)
            .expect("read spool dir")
            .count();
        assert_eq!(leftovers, 0, "spool files leaked");
        Ok(())
    }

    /// A truncated/aborted client body on the direct (Content-Length)
    /// upload path is a client error (400 via `InvalidArgument`), matching
    /// the spooled path — not a `500`.
    #[nativelink_test]
    async fn direct_ingest_maps_body_error_to_client_error() -> Result<(), Error> {
        let instance = test_instance();
        let payload = b"partial nar bytes before the stream errors";
        let digest = sha256_of(payload);
        // Yields some bytes, then errors mid-stream (a truncated body).
        let err_item: Result<Bytes, axum::Error> = Err(axum::Error::new(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "peer went away",
        )));
        let stream = futures::stream::iter(vec![Ok(Bytes::copy_from_slice(payload)), err_item]);
        // Declare a larger size so the write is genuinely incomplete.
        let err = ingest_nar_direct(&instance, digest, 4096, stream)
            .await
            .expect_err("truncated body must fail");
        assert_eq!(
            err.code,
            Code::InvalidArgument,
            "a client-caused truncated body must be a client error"
        );
        Ok(())
    }

    /// A decompressed NAR that grows past `max_nar_size_bytes` must abort
    /// as `ResourceExhausted` (413), for every codec, without leaking the
    /// partial spool file.
    #[nativelink_test]
    async fn spooled_ingest_enforces_max_nar_size() -> Result<(), Error> {
        let instance = build_test_instance(64, 256);
        let payload = b"decompression bomb payload byte ".repeat(256);
        for codec in [
            NarCodec::None,
            NarCodec::Gzip,
            NarCodec::Xz,
            NarCodec::Zstd,
            NarCodec::Bzip2,
        ] {
            let compressed = compress(codec, &payload).await;
            let stream = futures::stream::iter([Ok::<_, axum::Error>(Bytes::from(compressed))]);
            let err = ingest_nar_spooled(&instance, codec, stream)
                .await
                .expect_err("cap must abort the ingest");
            assert_eq!(err.code, Code::ResourceExhausted, "for {codec:?}");
        }
        let leftovers = std::fs::read_dir(&instance.spool_dir)
            .expect("read spool dir")
            .count();
        assert_eq!(leftovers, 0, "spool files leaked");
        Ok(())
    }

    /// The concurrent-NAR-stream ceiling refuses new streams once the
    /// permits are exhausted, and a released permit is reusable.
    #[test]
    fn acquire_nar_stream_permit_bounds_concurrency() {
        let instance = build_test_instance(1024, 2);
        let p1 = instance.acquire_nar_stream_permit().expect("first permit");
        let p2 = instance.acquire_nar_stream_permit().expect("second permit");
        assert!(
            instance.acquire_nar_stream_permit().is_none(),
            "a third concurrent stream must be refused at the ceiling"
        );
        drop(p1);
        assert!(
            instance.acquire_nar_stream_permit().is_some(),
            "a freed permit is reusable"
        );
        drop(p2);
    }

    #[test]
    fn bare_nar_stem_sha256_matches_only_bare_identity_names() {
        const HELLO_NIX32: &str = "094qif9n4cq4fdg459qzbhg1c6wywawwaaivx0k0x8xhbyx4vwic";
        const HELLO_HEX: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        let hello: [u8; 32] = hex::decode(HELLO_HEX)
            .expect("hex")
            .try_into()
            .expect("32 bytes");
        assert_eq!(
            bare_nar_stem_sha256(&format!("{HELLO_NIX32}.nar")),
            Some(hello)
        );
        // Canonical, compressed, and non-hash-shaped names never match.
        assert_eq!(bare_nar_stem_sha256(&format!("{HELLO_NIX32}-42.nar")), None);
        assert_eq!(bare_nar_stem_sha256(&format!("{HELLO_NIX32}.nar.xz")), None);
        assert_eq!(bare_nar_stem_sha256("random.nar"), None);
    }

    /// Concurrent narinfo PUTs for the SAME NAR under DISTINCT store-path
    /// hashes coalesce into one transcode via the single-flight map and
    /// must agree on the (content-addressed) result; the in-flight map is
    /// drained afterward.
    #[nativelink_test]
    async fn transcode_single_flights_concurrent_puts_for_one_nar() -> Result<(), Error> {
        let instance = build_test_instance(32 * 1024 * 1024 * 1024, 256);
        let payload = b"single-flight transcode payload 0123456789 ".repeat(64);
        let nar_size = u64::try_from(payload.len()).expect("size fits in u64");
        let nar_digest = DigestInfo::new(sha256_of(&payload), nar_size);
        with_sha256_ctx(
            instance
                .cas_store
                .update_oneshot(nar_digest, Bytes::from(payload.clone())),
        )
        .await
        .expect("seed NAR");

        let path_info = NixPathInfo {
            nar_sha256: sha256_of(&payload).to_vec(),
            nar_size,
            ..Default::default()
        };
        let (a, b) = tokio::join!(
            resolve_or_transcode_zstd(
                &instance,
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                &path_info,
                nar_digest,
            ),
            resolve_or_transcode_zstd(
                &instance,
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                &path_info,
                nar_digest,
            ),
        );
        assert_eq!(
            a.expect("transcode a"),
            b.expect("transcode b"),
            "concurrent transcodes of one NAR must agree"
        );
        assert!(
            instance
                .transcode_inflight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .is_empty(),
            "the in-flight map must be drained after completion"
        );
        Ok(())
    }

    /// Spool-directory pruning is scoped strictly to files this service
    /// created: an UNRELATED file dropped into a misconfigured shared
    /// `spool_path` must survive `prepare_spool_dir`, while a genuine leftover
    /// spool file (carrying [`SPOOL_FILE_PREFIX`] and the `.nar` suffix) is
    /// removed. Losing unrelated operator data here would be a data-loss bug.
    #[test]
    fn prepare_spool_dir_prunes_only_spool_files() {
        let dir = std::env::temp_dir().join(format!(
            "{DEFAULT_SPOOL_DIR_NAME}-prune-test-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("create dir");

        // An unrelated file that must be preserved.
        let unrelated = dir.join("important.txt");
        std::fs::write(&unrelated, b"operator data that must not be deleted").expect("write");
        // A genuine leftover spool file that must be pruned.
        let stale_spool = dir.join(format!("{SPOOL_FILE_PREFIX}{}.nar", Uuid::new_v4()));
        std::fs::write(&stale_spool, b"stale partial NAR").expect("write");
        // A file that merely ends in `.nar` but lacks the prefix must survive:
        // pruning keys on BOTH the prefix and the suffix.
        let foreign_nar = dir.join("someones-artifact.nar");
        std::fs::write(&foreign_nar, b"not ours").expect("write");
        // A file with the prefix but the wrong suffix must also survive.
        let prefixed_other = dir.join(format!("{SPOOL_FILE_PREFIX}{}.tmp", Uuid::new_v4()));
        std::fs::write(&prefixed_other, b"not a spool nar").expect("write");

        prepare_spool_dir(&dir).expect("prepare spool dir");

        assert!(unrelated.exists(), "unrelated file must survive pruning");
        assert!(
            foreign_nar.exists(),
            "a .nar without the spool prefix must survive"
        );
        assert!(
            prefixed_other.exists(),
            "a prefixed file without the .nar suffix must survive"
        );
        assert!(
            !stale_spool.exists(),
            "a genuine leftover spool file must be pruned"
        );

        drop(std::fs::remove_dir_all(&dir));
    }

    /// The build-log `Content-Encoding` allowlist accepts every known codec
    /// token (and their comma-combinations, case-insensitively, with
    /// surrounding whitespace) while rejecting anything else.
    #[test]
    fn is_allowed_log_encoding_accepts_known_and_rejects_unknown() {
        for good in [
            "",
            "identity",
            "gzip",
            "x-gzip",
            "deflate",
            "br",
            "zstd",
            "xz",
            "bzip2",
            "BR",
            "GZip",
            "gzip, br",
            " br , zstd ",
            "identity,identity",
        ] {
            assert!(is_allowed_log_encoding(good), "must accept '{good}'");
        }
        for bad in [
            "totally-bogus",
            "gzip, totally-bogus",
            "br;q=1.0",
            "chunked",
            "compress",
            "gzip br", // space-separated, not comma
            "../etc",
        ] {
            assert!(!is_allowed_log_encoding(bad), "must reject '{bad}'");
        }
    }

    mod proptests {
        use proptest::prelude::*;
        use proptest::test_runner::{Config as ProptestConfig, FileFailurePersistence};

        use super::super::BASE64;
        use super::*;

        fn no_persist() -> ProptestConfig {
            ProptestConfig {
                // Never write a `proptest-regressions/` file into the tree.
                failure_persistence: Some(Box::new(FileFailurePersistence::Off)),
                ..ProptestConfig::default()
            }
        }

        /// Builds an `Authorization` header map from an arbitrary ASCII string
        /// (header values must be visible ASCII / bytes hyper accepts).
        fn auth_map(value: &str) -> HeaderMap {
            let mut headers = HeaderMap::new();
            if let Ok(v) = HeaderValue::from_str(value) {
                headers.insert(super::super::AUTHORIZATION, v);
            }
            headers
        }

        proptest! {
            #![proptest_config(no_persist())]

            /// `constant_time_eq` must agree with `==` on every input,
            /// including empty and length-mismatched slices.
            #[test]
            fn constant_time_eq_matches_plain_eq(a: Vec<u8>, b: Vec<u8>) {
                prop_assert_eq!(constant_time_eq(&a, &b), a == b);
            }

            /// On equal-length pairs it still exactly matches `==` (the branch
            /// past the length gate).
            #[test]
            fn constant_time_eq_matches_on_equal_length(pair in
                (0usize..64).prop_flat_map(|len| {
                    (proptest::collection::vec(any::<u8>(), len),
                     proptest::collection::vec(any::<u8>(), len))
                })
            ) {
                let (a, b) = pair;
                prop_assert_eq!(a.len(), b.len());
                prop_assert_eq!(constant_time_eq(&a, &b), a == b);
            }

            /// It is reflexive for any slice.
            #[test]
            fn constant_time_eq_is_reflexive(a: Vec<u8>) {
                prop_assert!(constant_time_eq(&a, &a));
            }

            /// `extract_token` must never panic on ANY header value bytes, and
            /// must never return an empty token.
            #[test]
            fn extract_token_never_panics(value in ".*") {
                if let Some(token) = extract_token(&auth_map(&value)) {
                    prop_assert!(!token.is_empty());
                }
            }

            /// A well-formed `Bearer <token>` extracts exactly that token, for
            /// any non-empty visible-ASCII token containing no spaces.
            #[test]
            fn extract_token_reads_bearer(token in "[!-~&&[^ ]]{1,64}") {
                let headers = auth_map(&format!("Bearer {token}"));
                let extracted = extract_token(&headers);
                prop_assert_eq!(extracted.as_deref(), Some(token.as_str()));
            }

            /// A well-formed `Basic base64(user:password)` extracts the
            /// password (the token nix authenticates with via netrc), for any
            /// user/password drawn from a byte set with no ':' and no NUL.
            #[test]
            fn extract_token_reads_basic_password(
                user in "[a-zA-Z0-9._-]{0,32}",
                password in "[a-zA-Z0-9._!@#$%^&*()+-]{1,48}",
            ) {
                let encoded = BASE64.encode(format!("{user}:{password}"));
                let headers = auth_map(&format!("Basic {encoded}"));
                let extracted = extract_token(&headers);
                prop_assert_eq!(extracted.as_deref(), Some(password.as_str()));
            }

            /// The NAR name parsers must never panic on arbitrary strings.
            #[test]
            fn nar_name_parsers_never_panic(name in ".*", len in any::<u64>()) {
                let _ = bare_nar_digest_from_name(&name, Some(len));
                let _ = bare_nar_digest_from_name(&name, None);
                let _ = bare_nar_stem_sha256(&name);
                let _ = parse_canonical_any(&name);
                let _ = parse_canonical_nar_name(&name);
                for codec in [NarCodec::None, NarCodec::Gzip, NarCodec::Xz, NarCodec::Zstd, NarCodec::Bzip2] {
                    let _ = compressed_upload_digest(&name, codec, Some(len));
                    let _ = compressed_upload_digest(&name, codec, None);
                }
                let _ = is_valid_nar_upload_name(&name);
                let _ = codec_for_name(&name);
            }

            /// Any name `canonical_nar_name` produces round-trips through
            /// `parse_canonical_nar_name` and `parse_canonical_any` back to the
            /// same `(hash, size)` — and re-rendering equals the input.
            #[test]
            fn canonical_nar_name_round_trips(hash in any::<[u8; 32]>(), size in any::<u64>()) {
                let name = canonical_nar_name(&hash, size);
                prop_assert_eq!(parse_canonical_nar_name(&name), Some((hash, size)));
                prop_assert_eq!(parse_canonical_any(&name), Some((hash, size, NarCodec::None)));
                // The renderer is a true inverse of the parser on its own output.
                let (rt_hash, rt_size) = parse_canonical_nar_name(&name).expect("parses");
                prop_assert_eq!(canonical_nar_name(&rt_hash, rt_size), name);
            }

            /// Likewise for the canonical `.nar.zst` name.
            #[test]
            fn canonical_nar_zst_name_round_trips(hash in any::<[u8; 32]>(), size in any::<u64>()) {
                let name = canonical_nar_zst_name(&hash, size);
                prop_assert_eq!(parse_canonical_any(&name), Some((hash, size, NarCodec::Zstd)));
                let (_, _, codec) = parse_canonical_any(&name).expect("parses");
                prop_assert_eq!(codec, NarCodec::Zstd);
            }

            /// A bare `{52 nix32}.nar` name round-trips through
            /// `bare_nar_stem_sha256` (stem == sha256(content)) and, with a
            /// Content-Length, through `bare_nar_digest_from_name`.
            #[test]
            fn bare_nar_stem_round_trips(hash in any::<[u8; 32]>(), len in any::<u64>()) {
                let name = format!("{}.nar", nixbase32::encode(&hash));
                prop_assert_eq!(bare_nar_stem_sha256(&name), Some(hash));
                prop_assert_eq!(bare_nar_digest_from_name(&name, Some(len)), Some((hash, len)));
            }

            /// `is_allowed_log_encoding` never panics on arbitrary strings, and
            /// a single random non-codec token is always rejected.
            #[test]
            fn is_allowed_log_encoding_never_panics(s in ".*") {
                let _ = is_allowed_log_encoding(&s);
            }

            /// Any token outside the known codec set is rejected (alone and
            /// when appended to a valid list).
            #[test]
            fn is_allowed_log_encoding_rejects_non_codecs(
                token in "[a-z][a-z0-9-]{2,20}"
            ) {
                const KNOWN: &[&str] =
                    &["identity", "gzip", "x-gzip", "deflate", "br", "zstd", "xz", "bzip2"];
                prop_assume!(!KNOWN.contains(&token.as_str()));
                prop_assert!(!is_allowed_log_encoding(&token), "rejects '{}'", token);
                prop_assert!(
                    !is_allowed_log_encoding(&format!("gzip, {token}")),
                    "rejects a list containing '{}'",
                    token
                );
            }
        }
    }
}
