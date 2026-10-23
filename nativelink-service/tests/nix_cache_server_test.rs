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

//! Integration tests for the Nix binary-cache HTTP facade
//! (`NixCacheServer`). These tests are the behavioral specification of
//! the service: they drive the axum router returned by
//! `NixCacheServer::routers()` in-process (no TCP listener) against
//! programmatically constructed `NativeLink` stores, exactly as the
//! production binary would wire them:
//!
//! - the NAR store is a `MemoryStore` wrapped in `VerifyStore` with both
//!   `verify_size` and `verify_hash` enabled, and
//! - the path-info store is a `MemoryStore` wrapped in
//!   `CompletenessCheckingStore` referencing the NAR store, so a
//!   `narinfo` whose NAR is missing is invisible to clients.
//!
//! Phase 2 extends the coverage to bearer/netrc token auth
//! (`read_token_files`/`write_token_files`), `.ls` listings, build logs
//! under `log/{drvBasename}`, and zstd-compressed serving
//! (`serve_compression = "zstd"`), all against the same store wiring.

use core::sync::atomic::{AtomicUsize, Ordering};
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode, header};
use axum::routing::get;
use bytes::Bytes;
use http_body_util::BodyExt;
use nativelink_config::cas_server::{NixCacheConfig, WithInstanceName};
use nativelink_config::stores::{MemorySpec, StoreSpec, VerifySpec};
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_nix::narinfo::{self, NarInfo};
use nativelink_nix::path_info::NixPathInfo;
use nativelink_nix::signing::{NixPublicKey, NixSigningKey};
use nativelink_nix::{nar_url, nixbase32};
use nativelink_service::nix_cache_server::NixCacheServer;
use nativelink_store::completeness_checking_store::CompletenessCheckingStore;
use nativelink_store::memory_store::MemoryStore;
use nativelink_store::store_manager::StoreManager;
use nativelink_store::verify_store::VerifyStore;
use nativelink_util::common::DigestInfo;
use nativelink_util::store_trait::{Store, StoreKey, StoreLike};
use pretty_assertions::assert_eq;
use sha2::{Digest as _, Sha256};
use tower::ServiceExt;

const NAR_STORE_NAME: &str = "nix_cache_nar_store";
const PATH_INFO_STORE_NAME: &str = "nix_cache_path_info_store";
const ALIAS_STORE_NAME: &str = "nix_cache_alias_store";

/// Throwaway ed25519 signing key for the SERVER side, generated once
/// with the local Nix CLI (nix (Nix) 2.34.7) and hardcoded here:
///
/// ```text
/// $ nix key generate-secret --key-name test-int-1
/// ```
///
/// This key exists only for this test suite; it protects nothing.
const SERVER_SECRET_KEY: &str = "test-int-1:59Az40GGu2M3sM6rQ6T/+c61OMLw1r+qx9xMe/SzkIkjJupCgoxM63utsPNUImg3vc6stSSfAlXYYgmmUSZ5Ig==";

/// Throwaway CLIENT-side signing key, used to attach a genuine `Sig`
/// line to uploaded `narinfo` documents so signature preservation can
/// be asserted. Generated the same way:
///
/// ```text
/// $ nix key generate-secret --key-name test-client-1
/// ```
const CLIENT_SECRET_KEY: &str = "test-client-1:1ZNgkOBfktY+KpNRdPjnmm/P5c4Fqb4PmwMv/FyCqI4BfpImUj8jSSISa1lTOX8E+fPRIJpvjeXLcHh5dblOjQ==";

/// The exact `nix-cache-info` document for the default configuration
/// (`store_dir` "/nix/store", `want_mass_query` true, `priority` 40).
const NIX_CACHE_INFO_BODY: &str = "StoreDir: /nix/store\nWantMassQuery: 1\nPriority: 40\n";

/// Write token used by the auth tests; the server reads it from a
/// one-token file referenced by `write_token_files`.
const WRITE_TOKEN: &str = "nlwrite-51a09be2c47d";

/// Read token used by the auth tests, via `read_token_files`.
const READ_TOKEN: &str = "nlread-9f3e7d10c216";

/// The netrc-style `Authorization` value nix sends for [`WRITE_TOKEN`]
/// after a Basic challenge: `Basic base64("ignored:<token>")`. Hardcoded
/// because base64 is not a dev-dependency of this crate; generated with:
///
/// ```text
/// $ printf 'ignored:nlwrite-51a09be2c47d' | base64
/// ```
const WRITE_TOKEN_BASIC: &str = "Basic aWdub3JlZDpubHdyaXRlLTUxYTA5YmUyYzQ3ZA==";

/// The Basic form of a token that matches nothing:
///
/// ```text
/// $ printf 'ignored:not-the-write-token' | base64
/// ```
const WRONG_TOKEN_BASIC: &str = "Basic aWdub3JlZDpub3QtdGhlLXdyaXRlLXRva2Vu";

/// The first four bytes of every zstd frame (RFC 8878): the magic number
/// `0xFD2FB528`, little-endian on the wire.
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];

/// Decompressed payload of [`GZIP_NAR_BLOB_HEX`]. NAR bodies are opaque
/// to the service, so this does not need real NAR framing.
const GZIP_NAR_PAYLOAD: &[u8] =
    b"nativelink nix-cache gzip fixture: not a real NAR, deterministic payload 0123456789";

/// A gzip blob whose decompressed bytes are [`GZIP_NAR_PAYLOAD`].
/// Hardcoded because `flate2` is not a dev-dependency of this crate.
/// Generated locally (gzip 1.14) with:
///
/// ```text
/// $ printf '<GZIP_NAR_PAYLOAD>' > payload.bin
/// $ gzip -n -9 -c payload.bin | xxd -p | tr -d '\n'
/// ```
const GZIP_NAR_BLOB_HEX: &str = concat!(
    "1f8b080000000000020305c1c10d83300c05d055fe00ad4429b4c08d0538b081",
    "9518b0080e4a0d0a4cdff7944c4e0ea22b54f2d3915b18f32d3b26c97624eea0",
    "d140484c01433f3ee0d9386da2f23371d8e90a913c8a57f9aeeacfb769ff23af",
    "b60553000000"
);

/// One 93-byte unit of the xz round-trip fixture's decompressed payload;
/// the fixture NAR is this repeated four times (372 bytes). NAR bodies are
/// opaque to the service, so this need not be real NAR framing.
const XZ_NAR_PAYLOAD_UNIT: &[u8] =
    b"nativelink nix-cache xz round-trip fixture: not a real NAR, deterministic payload 0123456789 ";

/// An xz blob whose decompressed bytes are [`XZ_NAR_PAYLOAD_UNIT`] repeated
/// four times. Hardcoded because `xz`/`liblzma` is not a dev-dependency of
/// this crate. Generated locally (xz 5.8.3) with:
///
/// ```text
/// $ printf '<XZ_NAR_PAYLOAD_UNIT * 4>' | xz -9 -e -c | xxd -p | tr -d '\n'
/// ```
const XZ_NAR_BLOB_HEX: &str = concat!(
    "fd377a585a000004e6d6b44604c068f40221011c0000000000000000dd3a4ad7",
    "e0017300605d0037184aef3f626b21bed045fb84015d5a29c73889ea84539fdc",
    "2701dd07cf5f9ff6d023a1e79d754ddbad2e405b2c97157b2176e63779f6871a",
    "5cd9831f279f6a23e8da37f624703b2981e06c2cd1cfcfc33b63c9333c281807",
    "99b2ccb5e0000000982658240112729500018401f40200003dfe9471b1c467fb",
    "020000000004595a"
);

/// One self-contained store/server fixture. Every test builds its own,
/// so tests can run concurrently and assert exact store contents.
struct CacheFixture {
    store_manager: Arc<StoreManager>,
    /// The `MemoryStore` underneath the `VerifyStore` NAR wrapper.
    nar_memory: Arc<MemoryStore>,
    /// The `MemoryStore` underneath the `CompletenessCheckingStore`.
    path_info_memory: Arc<MemoryStore>,
    alias_memory: Arc<MemoryStore>,
    signing_key_path: PathBuf,
}

impl CacheFixture {
    fn new(tag: &str) -> Self {
        let nar_memory = MemoryStore::new(&MemorySpec::default());
        let nar_store = VerifyStore::new(
            &VerifySpec {
                backend: StoreSpec::Memory(MemorySpec::default()),
                verify_size: true,
                verify_hash: true,
            },
            Store::new(nar_memory.clone()),
        );
        let path_info_memory = MemoryStore::new(&MemorySpec::default());
        let path_info_store = CompletenessCheckingStore::new(
            Store::new(path_info_memory.clone()),
            Store::new(nar_store.clone()),
        );
        let alias_memory = MemoryStore::new(&MemorySpec::default());

        let store_manager = Arc::new(StoreManager::new());
        store_manager
            .add_store(NAR_STORE_NAME, Store::new(nar_store))
            .expect("store registers");
        store_manager
            .add_store(PATH_INFO_STORE_NAME, Store::new(path_info_store))
            .expect("store registers");
        store_manager
            .add_store(ALIAS_STORE_NAME, Store::new(alias_memory.clone()))
            .expect("store registers");

        // The config takes key *files* (as produced by
        // `nix key generate-secret > file`), so write the hardcoded key
        // out with the trailing newline a real key file has.
        let signing_key_path = std::env::temp_dir().join(format!(
            "nix_cache_server_test-{}-{tag}.secret",
            std::process::id()
        ));
        std::fs::write(&signing_key_path, format!("{SERVER_SECRET_KEY}\n"))
            .expect("write signing key file");

        Self {
            store_manager,
            nar_memory,
            path_info_memory,
            alias_memory,
            signing_key_path,
        }
    }

    /// A `NixCacheConfig` with everything serde-defaulted except the
    /// store references and the signing key.
    fn config(&self, read_only: bool) -> NixCacheConfig {
        let mut config = self.config_with_extra("");
        config.read_only = read_only;
        config
    }

    /// Like [`Self::config`], with `extra_json5` spliced verbatim into
    /// the config object — the hook for the phase-2 knobs (token files,
    /// `serve_compression`, ...) without disturbing the base fixture.
    fn config_with_extra(&self, extra_json5: &str) -> NixCacheConfig {
        serde_json5::from_str(&format!(
            r#"{{
                cas_store: "{NAR_STORE_NAME}",
                path_info_store: "{PATH_INFO_STORE_NAME}",
                alias_store: "{ALIAS_STORE_NAME}",
                signing_key_files: ["{}"],
                // Spool under the fixture's own directory: the serde default
                // (`<system temp>/nativelink-nix-spool`) may belong to another
                // user, and is unwritable inside the Bazel test sandbox.
                spool_path: "{}",
                {extra_json5}
            }}"#,
            self.signing_key_path.display(),
            self.signing_key_path.with_extension("spool").display()
        ))
        .expect("NixCacheConfig json5 parses")
    }

    /// Builds a `NixCacheServer` for one instance and returns its mount
    /// path plus a root router with the instance router nested at that
    /// mount — exactly how the production binary consumes
    /// `NixCacheServer::routers()`. Tests drive the root router with
    /// full-path URIs (`{mount}/...`).
    fn server(&self, instance_name: &str, read_only: bool) -> (String, Router) {
        self.server_with_config(instance_name, self.config(read_only))
    }

    /// [`Self::server`] with extra JSON5 config fields; see
    /// [`Self::config_with_extra`].
    fn server_with_extra(&self, instance_name: &str, extra_json5: &str) -> (String, Router) {
        self.server_with_config(instance_name, self.config_with_extra(extra_json5))
    }

    fn server_with_config(&self, instance_name: &str, config: NixCacheConfig) -> (String, Router) {
        let server = NixCacheServer::new(
            &[WithInstanceName {
                instance_name: instance_name.to_string(),
                config,
            }],
            &self.store_manager,
        )
        .expect("NixCacheServer::new");
        let mut routers = server.routers();
        assert_eq!(routers.len(), 1, "one router per configured instance");
        let (mount, instance_router) = routers.pop().expect("router present");
        let root_router = Router::new().nest(&mount, instance_router);
        (mount, root_router)
    }
}

async fn call(router: &Router, request: Request<Body>) -> (StatusCode, HeaderMap, Bytes) {
    let response = router
        .clone()
        .oneshot(request)
        .await
        .expect("axum router is infallible");
    let (parts, body) = response.into_parts();
    let collected = body.collect().await.expect("collect response body");
    (parts.status, parts.headers, collected.to_bytes())
}

fn get_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .expect("valid GET request")
}

fn head_request(uri: &str) -> Request<Body> {
    Request::builder()
        .method(Method::HEAD)
        .uri(uri)
        .body(Body::empty())
        .expect("valid HEAD request")
}

fn put_request(uri: &str, payload: &[u8]) -> Request<Body> {
    Request::builder()
        .method(Method::PUT)
        .uri(uri)
        .header(header::CONTENT_LENGTH, payload.len())
        .body(Body::from(Bytes::copy_from_slice(payload)))
        .expect("valid PUT request")
}

fn range_request(uri: &str, range: &str) -> Request<Body> {
    Request::builder()
        .method(Method::GET)
        .uri(uri)
        .header(header::RANGE, range)
        .body(Body::empty())
        .expect("valid ranged GET request")
}

/// Writes a one-token file — the format `read_token_files` and
/// `write_token_files` reference — into the temp dir and returns its
/// path. Like real token files, it ends with a trailing newline.
fn token_file(tag: &str, token: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "nix_cache_server_test-{}-{tag}.token",
        std::process::id()
    ));
    std::fs::write(&path, format!("{token}\n")).expect("write token file");
    path
}

fn bearer(token: &str) -> String {
    format!("Bearer {token}")
}

/// Attaches an `Authorization` header to a request.
fn authed(mut request: Request<Body>, authorization: &str) -> Request<Body> {
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(authorization).expect("Authorization value is ASCII"),
    );
    request
}

/// PUTs `payload` at `uri` with the given `Authorization` value and
/// returns the response status.
async fn put_with_auth(
    router: &Router,
    uri: &str,
    payload: &[u8],
    authorization: &str,
) -> StatusCode {
    let (status, _, _) = call(router, authed(put_request(uri, payload), authorization)).await;
    status
}

fn header_text(headers: &HeaderMap, name: &HeaderName) -> String {
    headers
        .get(name)
        .unwrap_or_else(|| panic!("missing header {name}"))
        .to_str()
        .expect("header value is ASCII")
        .to_string()
}

fn sha256(data: &[u8]) -> [u8; 32] {
    Sha256::digest(data).into()
}

/// A deterministic, alphabet-valid `/nix/store/<hash>-<name>` path: the
/// 32-character nix32 hash is derived from `seed` (20 bytes of its
/// sha256, exactly the width of a real store-path hash).
fn test_store_path(seed: &str, name: &str) -> String {
    let digest = sha256(seed.as_bytes());
    let path_hash = nixbase32::encode(&digest[..20]);
    assert!(
        narinfo::is_store_path_hash(&path_hash),
        "generated hash '{path_hash}' must be a valid store path hash"
    );
    format!("/nix/store/{path_hash}-{name}")
}

fn store_path_basename(store_path: &str) -> &str {
    store_path.rsplit('/').next().expect("store path basename")
}

fn store_path_hash(store_path: &str) -> &str {
    &store_path_basename(store_path)[..32]
}

/// The `narinfo` a well-behaved client would upload for an uncompressed
/// (`Compression: none`) NAR: `FileHash`/`FileSize` equal
/// `NarHash`/`NarSize` and the URL is the client-chosen NAR location.
fn narinfo_for_payload(store_path: &str, url: String, payload: &[u8]) -> NarInfo {
    let nar_hash = sha256(payload);
    NarInfo {
        store_path: store_path.to_string(),
        url,
        compression: "none".to_string(),
        file_hash: Some(nar_hash),
        file_size: Some(payload.len() as u64),
        nar_hash,
        nar_size: payload.len() as u64,
        references: vec![],
        deriver: None,
        system: None,
        sigs: vec![],
        ca: None,
    }
}

/// The basename a real `nix copy` client PUTs an uncompressed NAR
/// under: `{nix32(sha256(payload))}.nar`.
fn client_nar_basename(payload: &[u8]) -> String {
    format!("{}.nar", nixbase32::encode(&sha256(payload)))
}

/// The basename a real `nix copy --to ...?compression=xz` client PUTs a
/// compressed NAR under: `{nix32(sha256(compressed))}.nar.{ext}` — the
/// hash is of the COMPRESSED bytes (nix's `FileHash`).
fn client_compressed_basename(compressed: &[u8], ext: &str) -> String {
    format!("{}.nar.{ext}", nixbase32::encode(&sha256(compressed)))
}

/// The `narinfo` a well-behaved client uploads for a COMPRESSED NAR:
/// `FileHash`/`FileSize` describe the compressed bytes at `url`, while
/// `NarHash`/`NarSize` describe the uncompressed NAR (`payload`).
fn compressed_narinfo_for(
    store_path: &str,
    url: String,
    compression: &str,
    compressed: &[u8],
    payload: &[u8],
) -> NarInfo {
    NarInfo {
        store_path: store_path.to_string(),
        url,
        compression: compression.to_string(),
        file_hash: Some(sha256(compressed)),
        file_size: Some(compressed.len() as u64),
        nar_hash: sha256(payload),
        nar_size: payload.len() as u64,
        references: vec![],
        deriver: None,
        system: None,
        sigs: vec![],
        ca: None,
    }
}

/// Uploads `payload` as a bare `.nar` under the client-chosen basename
/// and asserts the 201. Returns the basename.
async fn put_nar(router: &Router, mount: &str, payload: &[u8]) -> String {
    let basename = client_nar_basename(payload);
    let (status, _, body) = call(
        router,
        put_request(&format!("{mount}/nar/{basename}"), payload),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "PUT nar/{basename}: {}",
        String::from_utf8_lossy(&body)
    );
    basename
}

/// Uploads `compressed` as a `.nar.{ext}` under the client-chosen
/// compressed basename and asserts the 201. Returns the basename.
async fn put_compressed_nar(router: &Router, mount: &str, compressed: &[u8], ext: &str) -> String {
    let basename = client_compressed_basename(compressed, ext);
    let (status, _, body) = call(
        router,
        put_request(&format!("{mount}/nar/{basename}"), compressed),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "PUT nar/{basename}: {}",
        String::from_utf8_lossy(&body)
    );
    basename
}

/// PUTs `info` at `{mount}/{store_path_hash}.narinfo` and returns the
/// response status.
async fn put_narinfo(router: &Router, mount: &str, info: &NarInfo) -> StatusCode {
    let path_hash = store_path_hash(&info.store_path);
    let (status, _, _) = call(
        router,
        put_request(
            &format!("{mount}/{path_hash}.narinfo"),
            info.render().as_bytes(),
        ),
    )
    .await;
    status
}

/// Full client upload flow: NAR first, then its `narinfo`; asserts both
/// return 201.
async fn publish(router: &Router, mount: &str, info: &NarInfo, payload: &[u8]) {
    put_nar(router, mount, payload).await;
    let status = put_narinfo(router, mount, info).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "PUT narinfo for {}",
        info.store_path
    );
}

/// GETs and parses `{mount}/{hash}.narinfo`, asserting the 200.
async fn get_narinfo(router: &Router, mount: &str, path_hash: &str) -> NarInfo {
    let (status, _, body) =
        call(router, get_request(&format!("{mount}/{path_hash}.narinfo"))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET {path_hash}.narinfo: {}",
        String::from_utf8_lossy(&body)
    );
    let text = core::str::from_utf8(&body).expect("narinfo body is UTF-8");
    narinfo::parse(text).expect("served narinfo parses")
}

#[nativelink_test]
async fn nix_cache_info_serves_default_document() -> Result<(), Error> {
    let fixture = CacheFixture::new("cache-info");
    let (mount, router) = fixture.server("main", false);
    // The default mount is `/nix/<instance_name>`.
    assert_eq!(mount, "/nix/main");

    let (status, headers, body) =
        call(&router, get_request(&format!("{mount}/nix-cache-info"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        header_text(&headers, &header::CONTENT_TYPE),
        "text/x-nix-cache-info"
    );
    assert_eq!(
        core::str::from_utf8(&body).expect("body is UTF-8"),
        NIX_CACHE_INFO_BODY
    );
    Ok(())
}

#[nativelink_test]
async fn put_nar_then_head_by_client_and_canonical_name() -> Result<(), Error> {
    const PAYLOAD: &[u8] = b"opaque nar payload for the upload/head round trip 0123456789";
    let fixture = CacheFixture::new("put-head");
    let (mount, router) = fixture.server("main", false);

    let basename = put_nar(&router, &mount, PAYLOAD).await;

    // HEAD of the exact URL the client uploaded to.
    let (status, headers, _) =
        call(&router, head_request(&format!("{mount}/nar/{basename}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header_text(&headers, &header::ACCEPT_RANGES), "bytes");

    // HEAD of the canonical `{nix32(sha256)}-{size}.nar` name.
    let canonical = nar_url::canonical_nar_name(&sha256(PAYLOAD), PAYLOAD.len() as u64);
    let (status, headers, _) =
        call(&router, head_request(&format!("{mount}/nar/{canonical}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        header_text(&headers, &header::CONTENT_LENGTH),
        PAYLOAD.len().to_string()
    );

    // The client-chosen URL also serves the exact bytes back.
    let (status, _, body) = call(&router, get_request(&format!("{mount}/nar/{basename}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), PAYLOAD);
    Ok(())
}

#[nativelink_test]
async fn put_narinfo_before_nar_is_conflict() -> Result<(), Error> {
    let fixture = CacheFixture::new("narinfo-before-nar");
    let (mount, router) = fixture.server("main", false);

    let store_path = test_store_path("conflict-seed", "ghost-1.0");
    let payload = b"nar bytes that were never uploaded";
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(payload)),
        payload,
    );

    assert_eq!(
        put_narinfo(&router, &mount, &info).await,
        StatusCode::CONFLICT,
        "narinfo whose NAR was never uploaded must 409"
    );
    // The rejected narinfo must not be visible or stored.
    let path_hash = store_path_hash(&store_path);
    let (status, _, _) = call(
        &router,
        get_request(&format!("{mount}/{path_hash}.narinfo")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(fixture.path_info_memory.len_for_test(), 0);
    Ok(())
}

#[nativelink_test]
async fn put_narinfo_with_mismatched_nar_hash_is_rejected() -> Result<(), Error> {
    let fixture = CacheFixture::new("narhash-mismatch");
    let (mount, router) = fixture.server("main", false);

    let uploaded_payload = b"the nar bytes that really got uploaded!!";
    let basename = put_nar(&router, &mount, uploaded_payload).await;

    // Same length, different bytes: NarHash disagrees with the upload.
    let other_payload = b"completely different nar bytes, same len";
    assert_eq!(uploaded_payload.len(), other_payload.len());
    let store_path = test_store_path("mismatch-seed", "liar-1.0");
    let info = narinfo_for_payload(&store_path, format!("nar/{basename}"), other_payload);

    // The narinfo is validated against the uncompressed NAR keyed by its
    // own NarHash/NarSize (no longer via the URL alias), so a NarHash whose
    // NAR was never uploaded is a 409 ("upload the NAR first"), not a 400.
    assert_eq!(
        put_narinfo(&router, &mount, &info).await,
        StatusCode::CONFLICT,
        "narinfo whose NarHash names an un-uploaded NAR must 409"
    );
    assert_eq!(fixture.path_info_memory.len_for_test(), 0);
    Ok(())
}

#[nativelink_test]
async fn put_narinfo_with_filename_store_path_mismatch_is_rejected() -> Result<(), Error> {
    let fixture = CacheFixture::new("filename-mismatch");
    let (mount, router) = fixture.server("main", false);

    let payload = b"nar payload for the filename mismatch case";
    let basename = put_nar(&router, &mount, payload).await;

    let store_path = test_store_path("body-seed", "inner-1.0");
    let info = narinfo_for_payload(&store_path, format!("nar/{basename}"), payload);
    // PUT under the hash of a DIFFERENT store path than the body claims.
    let wrong_path = test_store_path("filename-seed", "outer-1.0");
    let wrong_hash = store_path_hash(&wrong_path);
    let (status, _, _) = call(
        &router,
        put_request(
            &format!("{mount}/{wrong_hash}.narinfo"),
            info.render().as_bytes(),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "filename hash != StorePath hash must 400"
    );
    assert_eq!(fixture.path_info_memory.len_for_test(), 0);
    Ok(())
}

#[nativelink_test]
async fn served_narinfo_is_canonical_and_signed() -> Result<(), Error> {
    let fixture = CacheFixture::new("served-narinfo");
    let (mount, router) = fixture.server("main", false);

    // A dependency path, so the signed fingerprint covers a non-trivial
    // References list.
    let dep_payload = b"dependency nar payload";
    let dep_path = test_store_path("dep-seed", "libdep-1.2");
    let dep_info = narinfo_for_payload(
        &dep_path,
        format!("nar/{}", client_nar_basename(dep_payload)),
        dep_payload,
    );
    publish(&router, &mount, &dep_info, dep_payload).await;

    // The main path: references (dep + self), a deriver, and a genuine
    // client signature over the client-computed fingerprint.
    let main_payload = b"main nar payload with enough bytes to be interesting";
    let main_path = test_store_path("main-seed", "app-3.4");
    let mut main_info = narinfo_for_payload(
        &main_path,
        format!("nar/{}", client_nar_basename(main_payload)),
        main_payload,
    );
    main_info.references = vec![
        store_path_basename(&dep_path).to_string(),
        store_path_basename(&main_path).to_string(),
    ];
    main_info.deriver =
        Some(store_path_basename(&test_store_path("drv-seed", "app-3.4.drv")).to_string());
    let client_key = NixSigningKey::from_secret_string(CLIENT_SECRET_KEY).expect("client key");
    let client_sig = client_key.sign(&main_info.fingerprint());
    main_info.sigs = vec![client_sig.clone()];
    publish(&router, &mount, &main_info, main_payload).await;

    let served = get_narinfo(&router, &mount, store_path_hash(&main_path)).await;

    // Identity fields survive the round trip.
    assert_eq!(served.store_path, main_path);
    assert_eq!(served.nar_hash, sha256(main_payload));
    assert_eq!(served.nar_size, main_payload.len() as u64);
    assert_eq!(served.deriver, main_info.deriver);
    let mut expected_references = main_info.references.clone();
    expected_references.sort_unstable();
    assert_eq!(served.references, expected_references);

    // The served URL is the canonical NAR name and the compression is
    // `none`, regardless of what the client uploaded.
    let canonical = nar_url::canonical_nar_name(&sha256(main_payload), main_payload.len() as u64);
    assert_eq!(served.url, format!("nar/{canonical}"));
    assert_eq!(served.compression, "none");

    // The fingerprint is unchanged, the client signature is preserved
    // verbatim, and the server added a signature from the configured
    // key that verifies over the served document's fingerprint.
    assert_eq!(served.fingerprint(), main_info.fingerprint());
    assert!(
        served.sigs.contains(&client_sig),
        "client Sig line must be preserved, got: {}",
        served.sigs.join(" | ")
    );
    let server_key = NixSigningKey::from_secret_string(SERVER_SECRET_KEY).expect("server key");
    let public_key = NixPublicKey::from_string(&server_key.public_key_string()).expect("pub key");
    let server_sig = served
        .sigs
        .iter()
        .find(|sig: &&String| sig.starts_with("test-int-1:"))
        .expect("server Sig line present");
    assert!(
        public_key.verify(&served.fingerprint(), server_sig),
        "server signature must verify over the narinfo fingerprint"
    );

    // The advertised URL is fetchable and returns the exact NAR bytes.
    let (status, _, body) = call(&router, get_request(&format!("{mount}/{}", served.url))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), main_payload);
    Ok(())
}

#[nativelink_test]
async fn narinfo_head_unknown_and_malformed_names() -> Result<(), Error> {
    let fixture = CacheFixture::new("narinfo-head");
    let (mount, router) = fixture.server("main", false);

    let payload = b"nar payload for the HEAD narinfo case";
    let store_path = test_store_path("head-seed", "headful-1.0");
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(payload)),
        payload,
    );
    publish(&router, &mount, &info, payload).await;

    let path_hash = store_path_hash(&store_path);
    let (status, _, _) = call(
        &router,
        head_request(&format!("{mount}/{path_hash}.narinfo")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "HEAD of an uploaded narinfo");

    // A well-formed hash that was never uploaded is a clean 404 on both
    // GET and HEAD.
    let unknown_path = test_store_path("unknown-seed", "missing-1.0");
    let unknown_hash = store_path_hash(&unknown_path);
    let (status, _, _) = call(
        &router,
        get_request(&format!("{mount}/{unknown_hash}.narinfo")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _, _) = call(
        &router,
        head_request(&format!("{mount}/{unknown_hash}.narinfo")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // Malformed names must be 400 or 404, never a 5xx: 31 characters,
    // and 32 characters outside the nix32 alphabet ('e' is excluded).
    let malformed = [
        path_hash[..31].to_string(),
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_string(),
    ];
    for name in &malformed {
        for request in [
            get_request(&format!("{mount}/{name}.narinfo")),
            head_request(&format!("{mount}/{name}.narinfo")),
        ] {
            let (status, _, _) = call(&router, request).await;
            assert!(
                matches!(status, StatusCode::NOT_FOUND | StatusCode::BAD_REQUEST),
                "malformed name '{name}' must be 400/404, got {status}"
            );
        }
    }
    Ok(())
}

#[nativelink_test]
async fn nar_get_full_and_range_requests() -> Result<(), Error> {
    const PAYLOAD: &[u8] = b"0123456789abcdefghij:nar-range-payload!";
    let fixture = CacheFixture::new("nar-range");
    let (mount, router) = fixture.server("main", false);

    let basename = put_nar(&router, &mount, PAYLOAD).await;
    let nar_uri = format!("{mount}/nar/{basename}");

    // Full GET: exact bytes, length, and media type.
    let (status, headers, body) = call(&router, get_request(&nar_uri)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        header_text(&headers, &header::CONTENT_TYPE),
        "application/x-nix-nar"
    );
    assert_eq!(
        header_text(&headers, &header::CONTENT_LENGTH),
        PAYLOAD.len().to_string()
    );
    assert_eq!(body.as_ref(), PAYLOAD);

    // A satisfiable byte range: inclusive on both ends.
    let (status, headers, body) = call(&router, range_request(&nar_uri, "bytes=3-9")).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        header_text(&headers, &header::CONTENT_RANGE),
        format!("bytes 3-9/{}", PAYLOAD.len())
    );
    assert_eq!(body.as_ref(), &PAYLOAD[3..=9]);

    // Ranges starting at or past the end are unsatisfiable.
    let at_end = format!("bytes={}-{}", PAYLOAD.len(), PAYLOAD.len() + 5);
    for range in [at_end.as_str(), "bytes=100000-"] {
        let (status, _, _) = call(&router, range_request(&nar_uri, range)).await;
        assert_eq!(
            status,
            StatusCode::RANGE_NOT_SATISFIABLE,
            "range '{range}' must 416"
        );
    }
    Ok(())
}

#[nativelink_test]
async fn narinfo_of_evicted_nar_is_not_found() -> Result<(), Error> {
    let fixture = CacheFixture::new("evicted-nar");
    let (mount, router) = fixture.server("main", false);

    let payload = b"nar payload that will be evicted from the store";
    let store_path = test_store_path("evict-seed", "shortlived-1.0");
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(payload)),
        payload,
    );
    publish(&router, &mount, &info, payload).await;

    let path_hash = store_path_hash(&store_path);
    let narinfo_uri = format!("{mount}/{path_hash}.narinfo");
    let (status, _, _) = call(&router, get_request(&narinfo_uri)).await;
    assert_eq!(status, StatusCode::OK, "narinfo visible before eviction");

    // Evict the NAR out from under the path-info record. NARs are keyed
    // by `DigestInfo(sha256(nar), nar_size)` in the NAR store.
    let removed = fixture
        .nar_memory
        .remove_entry(DigestInfo::new(sha256(payload), payload.len() as u64).into())
        .await;
    assert!(removed, "NAR entry must exist before eviction");

    // The completeness_checking wrapper now hides the record on both
    // GET and HEAD.
    let (status, _, _) = call(&router, get_request(&narinfo_uri)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "GET after eviction");
    let (status, _, _) = call(&router, head_request(&narinfo_uri)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "HEAD after eviction");
    Ok(())
}

#[nativelink_test]
async fn path_info_record_without_nar_is_not_found() -> Result<(), Error> {
    let fixture = CacheFixture::new("phantom-record");
    let (mount, router) = fixture.server("main", false);

    // Write a well-formed path-info record directly into the backing
    // store, referencing a NAR digest that was never uploaded.
    let phantom_payload = b"phantom nar bytes that never reached the store";
    let store_path = test_store_path("phantom-seed", "phantom-2.0");
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(phantom_payload)),
        phantom_payload,
    );
    let record = NixPathInfo::from_nar_info(&info).encode_record()?;
    let path_hash = store_path_hash(&store_path);
    fixture
        .path_info_memory
        .update_oneshot(StoreKey::new_str(path_hash), Bytes::from(record))
        .await?;

    let narinfo_uri = format!("{mount}/{path_hash}.narinfo");
    let (status, _, _) = call(&router, get_request(&narinfo_uri)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "GET of a NAR-less record");
    let (status, _, _) = call(&router, head_request(&narinfo_uri)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "HEAD of a NAR-less record");
    Ok(())
}

#[nativelink_test]
async fn read_only_instance_rejects_uploads_but_serves_reads() -> Result<(), Error> {
    let fixture = CacheFixture::new("read-only");
    let (writable_mount, writable_router) = fixture.server("main", false);
    let (mirror_mount, mirror_router) = fixture.server("mirror", true);
    assert_eq!(mirror_mount, "/nix/mirror");

    // Populate through the writable instance.
    let payload = b"nar payload shared by the writable and read-only mounts";
    let store_path = test_store_path("ro-seed", "shared-1.0");
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(payload)),
        payload,
    );
    publish(&writable_router, &writable_mount, &info, payload).await;

    // All reads work on the read-only instance.
    let (status, _, body) = call(
        &mirror_router,
        get_request(&format!("{mirror_mount}/nix-cache-info")),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        core::str::from_utf8(&body).expect("UTF-8"),
        NIX_CACHE_INFO_BODY
    );
    let served = get_narinfo(&mirror_router, &mirror_mount, store_path_hash(&store_path)).await;
    let (status, _, body) = call(
        &mirror_router,
        get_request(&format!("{mirror_mount}/{}", served.url)),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), payload);

    // Every PUT is 405 and leaves no trace in any store.
    let nar_len = fixture.nar_memory.len_for_test();
    let path_info_len = fixture.path_info_memory.len_for_test();
    let alias_len = fixture.alias_memory.len_for_test();

    let new_payload = b"bytes the read-only mount must refuse";
    let (status, _, _) = call(
        &mirror_router,
        put_request(
            &format!("{mirror_mount}/nar/{}", client_nar_basename(new_payload)),
            new_payload,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "PUT nar on read-only"
    );

    let new_path = test_store_path("ro-put-seed", "refused-1.0");
    let new_info = narinfo_for_payload(
        &new_path,
        format!("nar/{}", client_nar_basename(new_payload)),
        new_payload,
    );
    assert_eq!(
        put_narinfo(&mirror_router, &mirror_mount, &new_info).await,
        StatusCode::METHOD_NOT_ALLOWED,
        "PUT narinfo on read-only"
    );

    assert_eq!(fixture.nar_memory.len_for_test(), nar_len);
    assert_eq!(fixture.path_info_memory.len_for_test(), path_info_len);
    assert_eq!(fixture.alias_memory.len_for_test(), alias_len);
    Ok(())
}

#[nativelink_test]
async fn gzip_nar_upload_is_sniffed_and_stored_decompressed() -> Result<(), Error> {
    let fixture = CacheFixture::new("gzip-sniff");
    let (mount, router) = fixture.server("main", false);

    let gzip_blob = hex::decode(GZIP_NAR_BLOB_HEX).expect("fixture hex decodes");
    assert_eq!(gzip_blob[..2], nar_url::GZIP_MAGIC, "fixture is gzip");

    // A client that gzipped its NAR but still named it `.nar`: the name
    // is derived from the *compressed* bytes, as `nix copy` would.
    let (status, _, body) = call(
        &router,
        put_request(
            &format!("{mount}/nar/{}", client_nar_basename(&gzip_blob)),
            &gzip_blob,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "PUT gzipped .nar: {}",
        String::from_utf8_lossy(&body)
    );

    // The stored NAR is the DECOMPRESSED stream: fetching the canonical
    // name derived from the decompressed sha256 returns those bytes.
    let canonical =
        nar_url::canonical_nar_name(&sha256(GZIP_NAR_PAYLOAD), GZIP_NAR_PAYLOAD.len() as u64);
    let (status, _, body) = call(&router, get_request(&format!("{mount}/nar/{canonical}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), GZIP_NAR_PAYLOAD);
    Ok(())
}

#[nativelink_test]
async fn put_nar_with_wrong_name_digest_is_client_error() -> Result<(), Error> {
    let fixture = CacheFixture::new("wrong-digest");
    let (mount, router) = fixture.server("main", false);

    // Name computed from DIFFERENT bytes: the verify wrapper must
    // reject the write, surfaced as a 4xx, and store nothing.
    let payload = b"actual bytes of the corrupt upload";
    let wrong_name = client_nar_basename(b"bytes the name was computed from instead");
    let (status, _, _) = call(
        &router,
        put_request(&format!("{mount}/nar/{wrong_name}"), payload),
    )
    .await;
    assert!(
        status.is_client_error(),
        "hash-mismatched NAR upload must be 4xx, got {status}"
    );
    assert_eq!(fixture.nar_memory.len_for_test(), 0);
    Ok(())
}

#[nativelink_test]
async fn traversal_and_garbage_names_are_client_errors_without_writes() -> Result<(), Error> {
    let fixture = CacheFixture::new("traversal");
    let (mount, router) = fixture.server("main", false);

    let garbage_uris = [
        format!("{mount}/..%2Fetc"),
        format!("{mount}/..%2F..%2Fetc%2Fpasswd.narinfo"),
        format!("{mount}/nar/..%2F..%2Fescape.nar"),
        format!("{mount}/nar/../x"),
        format!("{mount}/{}.narinfo", "z".repeat(53)),
        format!("{mount}/nar/{}.nar", "z".repeat(53)),
        format!("{mount}/nar/{}.nar", "z".repeat(31)),
    ];
    for uri in &garbage_uris {
        let (status, _, _) = call(&router, get_request(uri)).await;
        assert!(
            status.is_client_error(),
            "GET {uri} must be 4xx, got {status}"
        );
        let (status, _, _) = call(&router, put_request(uri, b"garbage payload")).await;
        assert!(
            status.is_client_error(),
            "PUT {uri} must be 4xx, got {status}"
        );
    }

    // None of it may have written anything anywhere.
    assert_eq!(fixture.nar_memory.len_for_test(), 0);
    assert_eq!(fixture.path_info_memory.len_for_test(), 0);
    assert_eq!(fixture.alias_memory.len_for_test(), 0);
    Ok(())
}

#[nativelink_test]
async fn write_tokens_gate_puts_with_bearer_and_basic() -> Result<(), Error> {
    let fixture = CacheFixture::new("write-auth");
    let write_token_path = token_file("write-auth", WRITE_TOKEN);
    let (mount, router) = fixture.server_with_extra(
        "main",
        &format!(r#"write_token_files: ["{}"],"#, write_token_path.display()),
    );

    let payload = b"nar payload behind write auth";
    let store_path = test_store_path("write-auth-seed", "guarded-1.0");
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(payload)),
        payload,
    );
    let nar_uri = format!("{mount}/nar/{}", client_nar_basename(payload));

    // Only write tokens are configured, so reads stay anonymous.
    let (status, _, _) = call(&router, get_request(&format!("{mount}/nix-cache-info"))).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "anonymous read with write-only auth"
    );

    // Anonymous PUT: 401 challenging for Basic credentials (nix only
    // sends netrc credentials after a Basic challenge).
    let (status, headers, _) = call(&router, put_request(&nar_uri, payload)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "anonymous PUT");
    assert!(
        header_text(&headers, &header::WWW_AUTHENTICATE).starts_with("Basic"),
        "401 must challenge with WWW-Authenticate: Basic"
    );

    // Wrong tokens: 401 in both the Bearer and the Basic form.
    for authorization in ["Bearer not-the-write-token", WRONG_TOKEN_BASIC] {
        assert_eq!(
            put_with_auth(&router, &nar_uri, payload, authorization).await,
            StatusCode::UNAUTHORIZED,
            "PUT with '{authorization}'"
        );
    }
    // None of the rejected PUTs may have written anything.
    assert_eq!(fixture.nar_memory.len_for_test(), 0);
    assert_eq!(fixture.alias_memory.len_for_test(), 0);

    // The Bearer form and the netrc Basic form both restore normal
    // behavior: the usual NAR-then-narinfo flow succeeds end to end.
    assert_eq!(
        put_with_auth(&router, &nar_uri, payload, &bearer(WRITE_TOKEN)).await,
        StatusCode::CREATED,
        "PUT nar with the Bearer write token"
    );
    let path_hash = store_path_hash(&store_path);
    assert_eq!(
        put_with_auth(
            &router,
            &format!("{mount}/{path_hash}.narinfo"),
            info.render().as_bytes(),
            WRITE_TOKEN_BASIC,
        )
        .await,
        StatusCode::CREATED,
        "PUT narinfo with the Basic write token"
    );
    let served = get_narinfo(&router, &mount, path_hash).await;
    assert_eq!(served.store_path, store_path);
    Ok(())
}

#[nativelink_test]
async fn read_tokens_gate_reads_including_cache_info() -> Result<(), Error> {
    let fixture = CacheFixture::new("read-auth");
    let read_token_path = token_file("read-auth-read", READ_TOKEN);
    let write_token_path = token_file("read-auth-write", WRITE_TOKEN);
    let (mount, router) = fixture.server_with_extra(
        "main",
        &format!(
            r#"read_token_files: ["{}"], write_token_files: ["{}"],"#,
            read_token_path.display(),
            write_token_path.display()
        ),
    );

    // Publish one path with the write token.
    let payload = b"nar payload behind read auth";
    let store_path = test_store_path("read-auth-seed", "private-1.0");
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(payload)),
        payload,
    );
    let nar_uri = format!("{mount}/nar/{}", client_nar_basename(payload));
    assert_eq!(
        put_with_auth(&router, &nar_uri, payload, &bearer(WRITE_TOKEN)).await,
        StatusCode::CREATED
    );
    let path_hash = store_path_hash(&store_path);
    let narinfo_uri = format!("{mount}/{path_hash}.narinfo");
    let put_status = put_with_auth(
        &router,
        &narinfo_uri,
        info.render().as_bytes(),
        &bearer(WRITE_TOKEN),
    )
    .await;
    assert_eq!(put_status, StatusCode::CREATED);

    // Every anonymous read is 401, including nix-cache-info.
    let cache_info_uri = format!("{mount}/nix-cache-info");
    for request in [
        get_request(&cache_info_uri),
        get_request(&narinfo_uri),
        head_request(&narinfo_uri),
        get_request(&nar_uri),
    ] {
        let uri = request.uri().clone();
        let (status, _, _) = call(&router, request).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "anonymous {uri}");
    }

    // Both the read token and the write token open every read.
    for authorization in [bearer(READ_TOKEN), bearer(WRITE_TOKEN)] {
        for uri in [&cache_info_uri, &narinfo_uri, &nar_uri] {
            let (status, _, _) = call(&router, authed(get_request(uri), &authorization)).await;
            assert_eq!(status, StatusCode::OK, "GET {uri} with '{authorization}'");
        }
        let (status, _, _) =
            call(&router, authed(head_request(&narinfo_uri), &authorization)).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "HEAD narinfo with '{authorization}'"
        );
    }

    // A read token opens reads only: it must not authorize writes.
    let other_payload = b"bytes a read token may not push";
    let put_status = put_with_auth(
        &router,
        &format!("{mount}/nar/{}", client_nar_basename(other_payload)),
        other_payload,
        &bearer(READ_TOKEN),
    )
    .await;
    assert_eq!(
        put_status,
        StatusCode::UNAUTHORIZED,
        "PUT with a read token"
    );
    Ok(())
}

#[nativelink_test]
async fn read_only_wins_over_valid_write_token() -> Result<(), Error> {
    let fixture = CacheFixture::new("ro-auth");
    let write_token_path = token_file("ro-auth", WRITE_TOKEN);
    let (mount, router) = fixture.server_with_extra(
        "mirror",
        &format!(
            r#"read_only: true, write_token_files: ["{}"],"#,
            write_token_path.display()
        ),
    );

    let payload = b"bytes no token can push to a read-only mount";
    let nar_uri = format!("{mount}/nar/{}", client_nar_basename(payload));
    for authorization in [bearer(WRITE_TOKEN), WRITE_TOKEN_BASIC.to_string()] {
        assert_eq!(
            put_with_auth(&router, &nar_uri, payload, &authorization).await,
            StatusCode::METHOD_NOT_ALLOWED,
            "read_only must win over a valid write token ('{authorization}')"
        );
    }
    assert_eq!(fixture.nar_memory.len_for_test(), 0);
    assert_eq!(fixture.alias_memory.len_for_test(), 0);
    Ok(())
}

#[nativelink_test]
async fn ls_round_trip_unknown_and_oversized() -> Result<(), Error> {
    /// A miniature but structurally plausible NAR listing document.
    const LS_DOC: &[u8] = br#"{"root":{"type":"regular","size":123,"narOffset":168},"version":1}"#;
    let fixture = CacheFixture::new("ls-round-trip");
    let (mount, router) = fixture.server("main", false);

    let payload = b"nar payload whose listing gets uploaded";
    let store_path = test_store_path("ls-seed", "listed-1.0");
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(payload)),
        payload,
    );
    publish(&router, &mount, &info, payload).await;

    let path_hash = store_path_hash(&store_path);
    let ls_uri = format!("{mount}/{path_hash}.ls");
    let (status, _, body) = call(&router, put_request(&ls_uri, LS_DOC)).await;
    assert!(
        status.is_success(),
        "PUT {path_hash}.ls: {status} {}",
        String::from_utf8_lossy(&body)
    );

    // Served back verbatim as JSON.
    let (status, headers, body) = call(&router, get_request(&ls_uri)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        header_text(&headers, &header::CONTENT_TYPE),
        "application/json"
    );
    assert_eq!(body.as_ref(), LS_DOC);

    // Shared-contract storage location: the listing lives in the alias
    // store under the slash-free key `ls:{storePathHash}`.
    let ls_key = format!("ls:{path_hash}");
    assert!(
        fixture
            .alias_memory
            .has(StoreKey::new_str(&ls_key))
            .await?
            .is_some(),
        "listing must be recorded under '{ls_key}'"
    );

    // A listing that was never uploaded is a clean 404.
    let unknown_path = test_store_path("ls-unknown-seed", "nolisting-1.0");
    let unknown_hash = store_path_hash(&unknown_path);
    let (status, _, _) = call(&router, get_request(&format!("{mount}/{unknown_hash}.ls"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // One byte past the 8 MiB listing limit: 413, and nothing written.
    let alias_len = fixture.alias_memory.len_for_test();
    let path_info_len = fixture.path_info_memory.len_for_test();
    let nar_len = fixture.nar_memory.len_for_test();
    let oversized = vec![b'{'; 8 * 1024 * 1024 + 1];
    let (status, _, _) = call(
        &router,
        put_request(&format!("{mount}/{unknown_hash}.ls"), &oversized),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE, "oversized .ls PUT");
    assert_eq!(fixture.alias_memory.len_for_test(), alias_len);
    assert_eq!(fixture.path_info_memory.len_for_test(), path_info_len);
    assert_eq!(fixture.nar_memory.len_for_test(), nar_len);
    let (status, _, _) = call(&router, get_request(&format!("{mount}/{unknown_hash}.ls"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "rejected .ls must not serve");
    Ok(())
}

#[nativelink_test]
async fn log_put_get_round_trip() -> Result<(), Error> {
    const LOG_BODY: &[u8] = b"these 2 derivations will be built:\nbuilding app-3.4...\ndone\n";
    let fixture = CacheFixture::new("log-round-trip");
    let (mount, router) = fixture.server("main", false);

    let drv_path = test_store_path("log-seed", "app-3.4.drv");
    let drv = store_path_basename(&drv_path);
    let log_uri = format!("{mount}/log/{drv}");
    let (status, _, body) = call(&router, put_request(&log_uri, LOG_BODY)).await;
    assert!(
        status.is_success(),
        "PUT log/{drv}: {status} {}",
        String::from_utf8_lossy(&body)
    );

    let (status, headers, body) = call(&router, get_request(&log_uri)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        header_text(&headers, &header::CONTENT_TYPE),
        "text/plain; charset=utf-8"
    );
    assert_eq!(body.as_ref(), LOG_BODY);

    // Shared-contract storage location: the log lives in the alias store
    // under the slash-free key `log:{drvBasename}`.
    let log_key = format!("log:{drv}");
    assert!(
        fixture
            .alias_memory
            .has(StoreKey::new_str(&log_key))
            .await?
            .is_some(),
        "log must be recorded under '{log_key}'"
    );

    // A log that was never uploaded is a clean 404.
    let other_path = test_store_path("log-unknown-seed", "other-1.0.drv");
    let other_uri = format!("{mount}/log/{}", store_path_basename(&other_path));
    let (status, _, _) = call(&router, get_request(&other_uri)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[nativelink_test]
async fn log_content_encoding_br_is_replayed_verbatim() -> Result<(), Error> {
    // Not real brotli: the server must treat the body as opaque and
    // replay it byte for byte with the same Content-Encoding.
    const BR_BODY: &[u8] = b"\x0b\x02\x80pretend-brotli log bytes, stored verbatim\x03";
    let fixture = CacheFixture::new("log-br");
    let (mount, router) = fixture.server("main", false);

    let drv_path = test_store_path("log-br-seed", "compressed-log-1.0.drv");
    let drv = store_path_basename(&drv_path);
    let log_uri = format!("{mount}/log/{drv}");
    let mut request = put_request(&log_uri, BR_BODY);
    request
        .headers_mut()
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static("br"));
    let (status, _, body) = call(&router, request).await;
    assert!(
        status.is_success(),
        "PUT br log: {status} {}",
        String::from_utf8_lossy(&body)
    );

    let (status, headers, body) = call(&router, get_request(&log_uri)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header_text(&headers, &header::CONTENT_ENCODING), "br");
    assert_eq!(
        body.as_ref(),
        BR_BODY,
        "still-compressed bytes must replay verbatim"
    );

    // Shared-contract storage location of the encoding: the alias store
    // holds the slash-free key `log-enc:{drvBasename}`.
    let enc_key = format!("log-enc:{drv}");
    assert!(
        fixture
            .alias_memory
            .has(StoreKey::new_str(&enc_key))
            .await?
            .is_some(),
        "log encoding must be recorded under '{enc_key}'"
    );
    Ok(())
}

#[nativelink_test]
async fn log_bad_names_are_client_errors_without_writes() -> Result<(), Error> {
    let fixture = CacheFixture::new("log-bad-names");
    let (mount, router) = fixture.server("main", false);

    let bad_uris = [
        format!("{mount}/log/.."),
        format!("{mount}/log/..%2F..%2Fetc%2Fpasswd"),
        format!("{mount}/log/a%2Fb.drv"),
        format!("{mount}/log/"),
    ];
    for uri in &bad_uris {
        let (status, _, _) = call(&router, put_request(uri, b"log bytes")).await;
        assert!(
            status.is_client_error(),
            "PUT {uri} must be 4xx, got {status}"
        );
        let (status, _, _) = call(&router, get_request(uri)).await;
        assert!(
            status.is_client_error(),
            "GET {uri} must be 4xx, got {status}"
        );
    }
    assert_eq!(fixture.nar_memory.len_for_test(), 0);
    assert_eq!(fixture.path_info_memory.len_for_test(), 0);
    assert_eq!(fixture.alias_memory.len_for_test(), 0);
    Ok(())
}

#[nativelink_test]
async fn zstd_instance_serves_compressed_nar() -> Result<(), Error> {
    let fixture = CacheFixture::new("zstd-serve");
    let (mount, router) = fixture.server_with_extra("zstd", r#"serve_compression: "zstd","#);

    let payload = b"nativelink nix-cache zstd fixture line 0123456789\n".repeat(64);
    let store_path = test_store_path("zstd-seed", "compressed-1.0");
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(&payload)),
        &payload,
    );
    publish(&router, &mount, &info, &payload).await;

    // The narinfo advertises the compressed file under its canonical
    // `.nar.zst` name; NarHash/NarSize still describe the uncompressed
    // NAR.
    let served = get_narinfo(&router, &mount, store_path_hash(&store_path)).await;
    assert_eq!(served.compression, "zstd");
    let file_hash = served.file_hash.expect("FileHash line present");
    let file_size = served.file_size.expect("FileSize line present");
    assert_eq!(
        served.url,
        format!(
            "nar/{}",
            nar_url::canonical_nar_zst_name(&file_hash, file_size)
        )
    );
    assert_eq!(served.nar_hash, sha256(&payload));
    assert_eq!(served.nar_size, payload.len() as u64);

    // The advertised URL serves exactly the bytes FileHash/FileSize
    // describe.
    let zst_uri = format!("{mount}/{}", served.url);
    let (status, headers, blob) = call(&router, get_request(&zst_uri)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        header_text(&headers, &header::CONTENT_LENGTH),
        file_size.to_string()
    );
    assert_eq!(blob.len() as u64, file_size);
    assert_eq!(sha256(&blob), file_hash);

    // No zstd decoder is available as a dev-dependency, so prove the
    // blob is a genuine, distinct zstd rendition instead: it starts with
    // the zstd frame magic, it is smaller than the (highly compressible)
    // payload, and the uncompressed canonical URL still serves the
    // original bytes untouched.
    assert_eq!(blob[..4], ZSTD_MAGIC, "zstd frame magic");
    assert!(
        blob.len() < payload.len(),
        "compressible payload must shrink: {} vs {}",
        blob.len(),
        payload.len()
    );
    let canonical = nar_url::canonical_nar_name(&sha256(&payload), payload.len() as u64);
    let (status, _, body) = call(&router, get_request(&format!("{mount}/nar/{canonical}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), payload.as_slice());

    // A Range request on the `.nar.zst` URL returns the exact slice.
    let (status, headers, slice) = call(&router, range_request(&zst_uri, "bytes=4-9")).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        header_text(&headers, &header::CONTENT_RANGE),
        format!("bytes 4-9/{file_size}")
    );
    assert_eq!(slice.as_ref(), &blob[4..=9]);
    Ok(())
}

#[nativelink_test]
async fn zstd_narinfo_signatures_survive_compression() -> Result<(), Error> {
    let fixture = CacheFixture::new("zstd-sigs");
    let (mount, router) = fixture.server_with_extra("zstd", r#"serve_compression: "zstd","#);

    let payload = b"nativelink nix-cache zstd signature fixture 0123456789\n".repeat(64);
    let store_path = test_store_path("zstd-sig-seed", "signed-1.0");
    let mut info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(&payload)),
        &payload,
    );
    let client_key = NixSigningKey::from_secret_string(CLIENT_SECRET_KEY).expect("client key");
    let client_sig = client_key.sign(&info.fingerprint());
    info.sigs = vec![client_sig.clone()];
    publish(&router, &mount, &info, &payload).await;

    let served = get_narinfo(&router, &mount, store_path_hash(&store_path)).await;
    assert_eq!(served.compression, "zstd");

    // The URL/Compression rewrite must not break signatures: the
    // fingerprint covers only the uncompressed NAR identity, so it is
    // unchanged, the client Sig is preserved verbatim, and the server
    // Sig verifies over the served document's fingerprint.
    assert_eq!(served.fingerprint(), info.fingerprint());
    assert!(
        served.sigs.contains(&client_sig),
        "client Sig line must be preserved, got: {}",
        served.sigs.join(" | ")
    );
    let server_key = NixSigningKey::from_secret_string(SERVER_SECRET_KEY).expect("server key");
    let public_key = NixPublicKey::from_string(&server_key.public_key_string()).expect("pub key");
    let server_sig = served
        .sigs
        .iter()
        .find(|sig: &&String| sig.starts_with("test-int-1:"))
        .expect("server Sig line present");
    assert!(
        public_key.verify(&served.fingerprint(), server_sig),
        "server signature must verify over the served fingerprint"
    );
    Ok(())
}

#[nativelink_test]
async fn zstd_blob_eviction_falls_back_and_nar_eviction_hides_narinfo() -> Result<(), Error> {
    let fixture = CacheFixture::new("zstd-evict");
    let (mount, router) = fixture.server_with_extra("zstd", r#"serve_compression: "zstd","#);

    let payload = b"nativelink nix-cache zstd eviction fixture 0123456789\n".repeat(64);
    let store_path = test_store_path("zstd-evict-seed", "degradable-1.0");
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(&payload)),
        &payload,
    );
    publish(&router, &mount, &info, &payload).await;

    let path_hash = store_path_hash(&store_path);
    let served = get_narinfo(&router, &mount, path_hash).await;
    assert_eq!(served.compression, "zstd");
    let file_hash = served.file_hash.expect("FileHash line present");
    let file_size = served.file_size.expect("FileSize line present");
    let zst_uri = format!("{mount}/{}", served.url);

    // Evict ONLY the compressed blob out from under the record.
    let removed = fixture
        .nar_memory
        .remove_entry(DigestInfo::new(file_hash, file_size).into())
        .await;
    assert!(removed, "zst blob must exist before eviction");

    // The handler degrades gracefully: the narinfo still 200s and falls
    // back to the uncompressed canonical URL. Completeness keys on the
    // uncompressed NAR only, so losing the zst blob must not 404 it.
    let degraded = get_narinfo(&router, &mount, path_hash).await;
    assert_eq!(degraded.compression, "none");
    let canonical = nar_url::canonical_nar_name(&sha256(&payload), payload.len() as u64);
    assert_eq!(degraded.url, format!("nar/{canonical}"));
    let (status, _, _) = call(
        &router,
        head_request(&format!("{mount}/{path_hash}.narinfo")),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "HEAD narinfo after zst eviction");
    // The fallback URL serves the original bytes; the evicted zst URL
    // is an honest 404.
    let (status, _, body) = call(&router, get_request(&format!("{mount}/nar/{canonical}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), payload.as_slice());
    let (status, _, _) = call(&router, get_request(&zst_uri)).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "evicted zst URL");

    // Evicting the UNCOMPRESSED NAR hides the narinfo entirely.
    let removed = fixture
        .nar_memory
        .remove_entry(DigestInfo::new(sha256(&payload), payload.len() as u64).into())
        .await;
    assert!(removed, "uncompressed NAR must exist before eviction");
    let (status, _, _) = call(
        &router,
        get_request(&format!("{mount}/{path_hash}.narinfo")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "narinfo after NAR eviction");
    Ok(())
}

/// A NAR upload whose DECLARED size (Content-Length) already exceeds
/// `max_nar_size_bytes` is fast-failed with `413` before the body is read,
/// and nothing is written.
#[nativelink_test]
async fn oversized_declared_nar_upload_is_rejected() -> Result<(), Error> {
    let fixture = CacheFixture::new("oversized-declared");
    let (mount, router) = fixture.server_with_extra("main", "max_nar_size_bytes: 100,");

    let payload = vec![b'x'; 200]; // 200 bytes, cap is 100
    let basename = client_nar_basename(&payload);
    let (status, _, _) = call(
        &router,
        put_request(&format!("{mount}/nar/{basename}"), &payload),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "declared size over the cap must 413"
    );
    assert_eq!(fixture.nar_memory.len_for_test(), 0);
    Ok(())
}

/// A NAR upload with no Content-Length (chunked) whose bytes grow past
/// `max_nar_size_bytes` while spooling is aborted with `413` by the
/// running cap, and nothing is written.
#[nativelink_test]
async fn chunked_nar_over_cap_is_rejected() -> Result<(), Error> {
    let fixture = CacheFixture::new("chunked-over-cap");
    let (mount, router) = fixture.server_with_extra("main", "max_nar_size_bytes: 100,");

    let payload = vec![b'x'; 500];
    let basename = client_nar_basename(&payload);
    // No Content-Length header, so this takes the spooled path guarded by
    // the running decompression cap.
    let request = Request::builder()
        .method(Method::PUT)
        .uri(format!("{mount}/nar/{basename}"))
        .body(Body::from(Bytes::from(payload)))
        .expect("valid PUT request");
    let (status, _, _) = call(&router, request).await;
    assert_eq!(
        status,
        StatusCode::PAYLOAD_TOO_LARGE,
        "a body past the running cap must 413"
    );
    assert_eq!(fixture.nar_memory.len_for_test(), 0);
    Ok(())
}

/// A narinfo with a malformed reference is rejected at PUT with `400`
/// (not stored), closing the PUT/GET validation asymmetry: previously it
/// was accepted (201) and then 500'd forever on GET.
#[nativelink_test]
async fn narinfo_with_malformed_reference_is_rejected_at_put() -> Result<(), Error> {
    let fixture = CacheFixture::new("malformed-reference");
    let (mount, router) = fixture.server("main", false);

    let payload = b"nar payload for the malformed-reference narinfo";
    let store_path = test_store_path("malformed-ref-seed", "app-1.0");
    let mut info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(payload)),
        payload,
    );
    put_nar(&router, &mount, payload).await;
    // A valid-length hash, but a NUL byte in the reference name.
    info.references = vec!["00000000000000000000000000000000-na\0me".to_string()];

    assert_eq!(
        put_narinfo(&router, &mount, &info).await,
        StatusCode::BAD_REQUEST,
        "a malformed reference must be rejected at PUT"
    );
    // Nothing persisted, so a GET is a clean 404 rather than a permanent
    // 500 — the symmetry the fix guarantees.
    let path_hash = store_path_hash(&store_path);
    let (get_status, _, _) = call(
        &router,
        get_request(&format!("{mount}/{path_hash}.narinfo")),
    )
    .await;
    assert_eq!(get_status, StatusCode::NOT_FOUND);
    assert_eq!(fixture.path_info_memory.len_for_test(), 0);
    Ok(())
}

/// A full store path in `Deriver` is invalid binary-cache metadata. Nix parses
/// that slash as part of a store hash and reports "illegal base-32 character
/// '/'"; reject it at admission so it can never poison substitution.
#[nativelink_test]
async fn narinfo_with_full_path_deriver_is_rejected_at_put() -> Result<(), Error> {
    let fixture = CacheFixture::new("malformed-deriver");
    let (mount, router) = fixture.server("main", false);
    let payload = b"nar payload for malformed deriver";
    let store_path = test_store_path("malformed-deriver-seed", "app-1.0");
    let mut info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(payload)),
        payload,
    );
    info.deriver = Some("/nix/store/00000000000000000000000000000000-builder.drv".to_string());
    put_nar(&router, &mount, payload).await;

    assert_eq!(
        put_narinfo(&router, &mount, &info).await,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(fixture.path_info_memory.len_for_test(), 0);
    Ok(())
}

/// A narinfo whose `StorePath` name carries a NUL byte is rejected at PUT
/// with `400` and never stored.
#[nativelink_test]
async fn nul_byte_in_store_path_name_is_rejected() -> Result<(), Error> {
    let fixture = CacheFixture::new("nul-store-path-name");
    let (mount, router) = fixture.server("main", false);

    let payload = b"nar payload for the NUL store-path-name case";
    put_nar(&router, &mount, payload).await;
    let hash = "00000000000000000000000000000000";
    let store_path = format!("/nix/store/{hash}-na\0me");
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(payload)),
        payload,
    );

    let (status, _, _) = call(
        &router,
        put_request(&format!("{mount}/{hash}.narinfo"), info.render().as_bytes()),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a NUL in the store-path name must 400"
    );
    let (get_status, _, _) = call(&router, get_request(&format!("{mount}/{hash}.narinfo"))).await;
    assert_eq!(get_status, StatusCode::NOT_FOUND);
    assert_eq!(fixture.path_info_memory.len_for_test(), 0);
    Ok(())
}

/// An over-long build-log name is a clean `4xx` (`400` on PUT, `404` on
/// GET) rather than a `500` from an oversized filesystem key.
#[nativelink_test]
async fn long_log_name_is_client_error() -> Result<(), Error> {
    let fixture = CacheFixture::new("long-log-name");
    let (mount, router) = fixture.server("main", false);

    let long = "a".repeat(300);
    let uri = format!("{mount}/log/{long}");
    let (put_status, _, _) = call(&router, put_request(&uri, b"log bytes")).await;
    assert_eq!(
        put_status,
        StatusCode::BAD_REQUEST,
        "over-long log name must 400 on PUT"
    );
    let (get_status, _, _) = call(&router, get_request(&uri)).await;
    assert!(
        get_status.is_client_error(),
        "over-long log name must be 4xx on GET, got {get_status}"
    );
    assert_eq!(fixture.alias_memory.len_for_test(), 0);
    Ok(())
}

/// With `preserve_upload_compression` on (the default), a compressed
/// (`.nar.xz`) push is served back byte-for-byte under the client's exact
/// URL, and the served narinfo advertises the ORIGINAL compression plus
/// `FileHash`/`FileSize` of the compressed blob (the attic-parity warm
/// pull). The uncompressed canonical URL still serves the decompressed
/// NAR.
#[nativelink_test]
async fn preserved_compressed_push_round_trips_original_bytes() -> Result<(), Error> {
    let fixture = CacheFixture::new("preserve-xz");
    let (mount, router) = fixture.server("main", false);

    let payload = XZ_NAR_PAYLOAD_UNIT.repeat(4);
    let xz_blob = hex::decode(XZ_NAR_BLOB_HEX).expect("xz fixture hex decodes");
    assert!(
        xz_blob.len() < payload.len(),
        "fixture must actually compress: {} vs {}",
        xz_blob.len(),
        payload.len()
    );

    // The client PUTs the compressed NAR under nar/{fileHash}.nar.xz.
    let basename = put_compressed_nar(&router, &mount, &xz_blob, "xz").await;
    let store_path = test_store_path("preserve-xz-seed", "compressed-1.0");
    let info = compressed_narinfo_for(
        &store_path,
        format!("nar/{basename}"),
        "xz",
        &xz_blob,
        &payload,
    );
    assert_eq!(
        put_narinfo(&router, &mount, &info).await,
        StatusCode::CREATED,
        "PUT compressed narinfo"
    );

    // (b) The served narinfo advertises the original compression, the
    // client URL, and the compressed blob's FileHash/FileSize; NarHash/
    // NarSize still describe the uncompressed NAR.
    let served = get_narinfo(&router, &mount, store_path_hash(&store_path)).await;
    assert_eq!(served.compression, "xz");
    assert_eq!(served.url, format!("nar/{basename}"));
    assert_eq!(served.file_hash, Some(sha256(&xz_blob)));
    assert_eq!(served.file_size, Some(xz_blob.len() as u64));
    assert_eq!(served.nar_hash, sha256(&payload));
    assert_eq!(served.nar_size, payload.len() as u64);

    // (a) GET the client's exact URL returns byte-identical original bytes.
    let (status, headers, body) =
        call(&router, get_request(&format!("{mount}/nar/{basename}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        header_text(&headers, &header::CONTENT_TYPE),
        "application/x-nix-nar"
    );
    assert_eq!(
        header_text(&headers, &header::CONTENT_LENGTH),
        xz_blob.len().to_string()
    );
    assert_eq!(
        body.as_ref(),
        xz_blob.as_slice(),
        "original xz bytes verbatim"
    );

    // The uncompressed canonical URL still serves the decompressed NAR.
    let canonical = nar_url::canonical_nar_name(&sha256(&payload), payload.len() as u64);
    let (status, _, body) = call(&router, get_request(&format!("{mount}/nar/{canonical}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), payload.as_slice());
    Ok(())
}

/// A Range request on the preserved compressed URL returns a 206 with the
/// exact slice of the compressed blob.
#[nativelink_test]
async fn range_on_preserved_compressed_url_is_partial() -> Result<(), Error> {
    let fixture = CacheFixture::new("preserve-range");
    let (mount, router) = fixture.server("main", false);

    let xz_blob = hex::decode(XZ_NAR_BLOB_HEX).expect("xz fixture hex decodes");
    let basename = put_compressed_nar(&router, &mount, &xz_blob, "xz").await;
    let uri = format!("{mount}/nar/{basename}");

    let (status, headers, slice) = call(&router, range_request(&uri, "bytes=4-9")).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        header_text(&headers, &header::CONTENT_RANGE),
        format!("bytes 4-9/{}", xz_blob.len())
    );
    assert_eq!(slice.as_ref(), &xz_blob[4..=9]);
    Ok(())
}

/// With `preserve_upload_compression = false`, a compressed push is
/// decompressed to the canonical uncompressed NAR and served as
/// `Compression: none` (the pre-preserve behavior); the client's
/// compressed URL 404s (no preserved blob, no alias).
#[nativelink_test]
async fn preserve_off_compressed_push_serves_canonical_none() -> Result<(), Error> {
    let fixture = CacheFixture::new("preserve-off");
    let (mount, router) = fixture.server_with_extra("main", "preserve_upload_compression: false,");

    let payload = XZ_NAR_PAYLOAD_UNIT.repeat(4);
    let xz_blob = hex::decode(XZ_NAR_BLOB_HEX).expect("xz fixture hex decodes");

    let basename = put_compressed_nar(&router, &mount, &xz_blob, "xz").await;
    let store_path = test_store_path("preserve-off-seed", "compressed-1.0");
    let info = compressed_narinfo_for(
        &store_path,
        format!("nar/{basename}"),
        "xz",
        &xz_blob,
        &payload,
    );
    assert_eq!(
        put_narinfo(&router, &mount, &info).await,
        StatusCode::CREATED
    );

    // The served narinfo is canonical `none` (old behavior): no FileHash/
    // FileSize, canonical `.nar` URL.
    let served = get_narinfo(&router, &mount, store_path_hash(&store_path)).await;
    assert_eq!(served.compression, "none");
    assert_eq!(served.file_hash, None);
    assert_eq!(served.file_size, None);
    let canonical = nar_url::canonical_nar_name(&sha256(&payload), payload.len() as u64);
    assert_eq!(served.url, format!("nar/{canonical}"));

    // The client's compressed URL 404s; the canonical URL serves the
    // decompressed bytes.
    let (status, _, _) = call(&router, get_request(&format!("{mount}/nar/{basename}"))).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "compressed URL with preserve off"
    );
    let (status, _, body) = call(&router, get_request(&format!("{mount}/nar/{canonical}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), payload.as_slice());
    Ok(())
}

/// Evicting ONLY the preserved compressed blob degrades the narinfo
/// gracefully to `Compression: none` (still 200 — completeness keys on the
/// uncompressed NAR, which is intact); evicting the uncompressed NAR then
/// hides the narinfo entirely (404).
#[nativelink_test]
async fn preserved_blob_eviction_falls_back_and_nar_eviction_hides_narinfo() -> Result<(), Error> {
    let fixture = CacheFixture::new("preserve-evict");
    let (mount, router) = fixture.server("main", false);

    let payload = XZ_NAR_PAYLOAD_UNIT.repeat(4);
    let xz_blob = hex::decode(XZ_NAR_BLOB_HEX).expect("xz fixture hex decodes");
    let basename = put_compressed_nar(&router, &mount, &xz_blob, "xz").await;
    let store_path = test_store_path("preserve-evict-seed", "degradable-1.0");
    let info = compressed_narinfo_for(
        &store_path,
        format!("nar/{basename}"),
        "xz",
        &xz_blob,
        &payload,
    );
    assert_eq!(
        put_narinfo(&router, &mount, &info).await,
        StatusCode::CREATED
    );

    let path_hash = store_path_hash(&store_path);
    let served = get_narinfo(&router, &mount, path_hash).await;
    assert_eq!(served.compression, "xz");

    // Evict ONLY the compressed blob out from under the record.
    let removed = fixture
        .nar_memory
        .remove_entry(DigestInfo::new(sha256(&xz_blob), xz_blob.len() as u64).into())
        .await;
    assert!(removed, "compressed blob must exist before eviction");

    // The narinfo still 200s and falls back to the uncompressed canonical
    // URL; losing the compressed blob must not 404 it.
    let degraded = get_narinfo(&router, &mount, path_hash).await;
    assert_eq!(degraded.compression, "none");
    assert_eq!(degraded.file_hash, None);
    let canonical = nar_url::canonical_nar_name(&sha256(&payload), payload.len() as u64);
    assert_eq!(degraded.url, format!("nar/{canonical}"));
    let (status, _, _) = call(
        &router,
        head_request(&format!("{mount}/{path_hash}.narinfo")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "HEAD narinfo after compressed eviction"
    );
    // The evicted compressed URL is an honest 404; the fallback URL serves.
    let (status, _, _) = call(&router, get_request(&format!("{mount}/nar/{basename}"))).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "evicted compressed URL");
    let (status, _, body) = call(&router, get_request(&format!("{mount}/nar/{canonical}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), payload.as_slice());

    // Evicting the UNCOMPRESSED NAR hides the narinfo entirely.
    let removed = fixture
        .nar_memory
        .remove_entry(DigestInfo::new(sha256(&payload), payload.len() as u64).into())
        .await;
    assert!(removed, "uncompressed NAR must exist before eviction");
    let (status, _, _) = call(
        &router,
        get_request(&format!("{mount}/{path_hash}.narinfo")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "narinfo after NAR eviction");
    Ok(())
}

/// On a `serve_compression = "zstd"` instance, a preserved original takes
/// precedence over transcoding: a compressed (`.nar.xz`) push is served
/// back in the client's ORIGINAL codec (no zstd re-encode), while
/// `Compression: none` pushes are still transcoded (covered by
/// `zstd_instance_serves_compressed_nar`).
#[nativelink_test]
async fn zstd_instance_preserves_original_codec_over_transcoding() -> Result<(), Error> {
    let fixture = CacheFixture::new("zstd-preserve");
    let (mount, router) = fixture.server_with_extra("zstd", r#"serve_compression: "zstd","#);

    let payload = XZ_NAR_PAYLOAD_UNIT.repeat(4);
    let xz_blob = hex::decode(XZ_NAR_BLOB_HEX).expect("xz fixture hex decodes");
    let basename = put_compressed_nar(&router, &mount, &xz_blob, "xz").await;
    let store_path = test_store_path("zstd-preserve-seed", "compressed-1.0");
    let info = compressed_narinfo_for(
        &store_path,
        format!("nar/{basename}"),
        "xz",
        &xz_blob,
        &payload,
    );
    assert_eq!(
        put_narinfo(&router, &mount, &info).await,
        StatusCode::CREATED
    );

    // The served narinfo advertises xz (the client's codec), NOT zstd, and
    // the client's URL serves the original xz bytes verbatim.
    let served = get_narinfo(&router, &mount, store_path_hash(&store_path)).await;
    assert_eq!(served.compression, "xz");
    assert_eq!(served.url, format!("nar/{basename}"));
    assert_eq!(served.file_hash, Some(sha256(&xz_blob)));
    let (status, _, body) = call(&router, get_request(&format!("{mount}/nar/{basename}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), xz_blob.as_slice());
    Ok(())
}

// ---------------------------------------------------------------------------
// Upstream read-through
// ---------------------------------------------------------------------------

/// Serves `router` on an ephemeral loopback port over real HTTP so a
/// read-through front cache can reach it with its reqwest client. Returns
/// the base URL (`http://127.0.0.1:<port>`) and the serving task's guard
/// (drop it to simulate the upstream going away).
async fn spawn_http(router: Router) -> (String, nativelink_util::task::JoinHandleDropGuard<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    let handle = nativelink_util::spawn!("test_upstream_http", async move {
        axum::serve(listener, router.into_make_service())
            .await
            .expect("axum serve");
    });
    (format!("http://{addr}"), handle)
}

/// The public key string (`test-int-1:...`) matching [`SERVER_SECRET_KEY`],
/// which every [`CacheFixture`] signs served narinfo with.
fn server_public_key() -> String {
    NixSigningKey::from_secret_string(SERVER_SECRET_KEY)
        .expect("server secret key parses")
        .public_key_string()
}

#[nativelink_test]
async fn read_through_fetches_verifies_and_caches_from_upstream() -> Result<(), Error> {
    // An upstream cache holding one signed path.
    let upstream = CacheFixture::new("rt-up");
    let (up_mount, up_router) = upstream.server("main", false);
    let payload = b"read-through upstream NAR payload: opaque bytes 0123456789";
    let store_path = test_store_path("read-through-happy", "hello-1.0");
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(payload)),
        payload,
    );
    publish(&up_router, &up_mount, &info, payload).await;
    let (base, handle) = spawn_http(up_router).await;
    let upstream_url = format!("{base}{up_mount}");
    let upstream_pubkey = server_public_key();

    // A front cache configured to read through to that upstream.
    let front = CacheFixture::new("rt-front");
    let (mount, router) = front.server_with_extra(
        "main",
        &format!(
            r#"upstream_caches: [{{ url: "{upstream_url}", trusted_public_keys: ["{upstream_pubkey}"] }}]"#
        ),
    );
    let path_hash = store_path_hash(&store_path);

    // A local miss reads through, verifies, ingests, and serves.
    let served = get_narinfo(&router, &mount, path_hash).await;
    assert_eq!(served.store_path, store_path);
    assert_eq!(served.nar_hash, sha256(payload));
    assert_eq!(served.nar_size, payload.len() as u64);
    // We serve our own canonical uncompressed NAR.
    assert_eq!(served.compression, "none");
    let canonical = nar_url::canonical_nar_name(&sha256(payload), payload.len() as u64);
    assert_eq!(served.url, format!("nar/{canonical}"));
    // The served narinfo carries a valid signature.
    let fingerprint = narinfo::fingerprint(
        &served.store_path,
        &served.nar_hash,
        served.nar_size,
        &served.references,
    );
    let key = NixPublicKey::from_string(&upstream_pubkey).expect("pubkey parses");
    assert!(
        served.sigs.iter().any(|sig| key.verify(&fingerprint, sig)),
        "served narinfo must carry a valid signature: {:?}",
        served.sigs
    );

    // The NAR is now local: a NAR GET returns the exact bytes.
    let (status, _, body) = call(&router, get_request(&format!("{mount}/nar/{canonical}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), payload);

    // Durable: with the upstream gone, the front still serves from its own
    // stores (proving the first fetch persisted, not proxied).
    drop(handle);
    let served_again = get_narinfo(&router, &mount, path_hash).await;
    assert_eq!(served_again.store_path, store_path);
    Ok(())
}

#[nativelink_test]
async fn read_through_refuses_untrusted_upstream_signature() -> Result<(), Error> {
    let upstream = CacheFixture::new("rt-badsig-up");
    let (up_mount, up_router) = upstream.server("main", false);
    let payload = b"untrusted upstream payload that must never be cached";
    let store_path = test_store_path("read-through-badsig", "evil-1.0");
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(payload)),
        payload,
    );
    publish(&up_router, &up_mount, &info, payload).await;
    let (base, _handle) = spawn_http(up_router).await;
    let upstream_url = format!("{base}{up_mount}");

    // The front trusts a DIFFERENT key than the upstream signs with, so
    // the fetched narinfo fails verification and must not be cached.
    let untrusted_key = "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=";
    let front = CacheFixture::new("rt-badsig-front");
    let (mount, router) = front.server_with_extra(
        "main",
        &format!(
            r#"upstream_caches: [{{ url: "{upstream_url}", trusted_public_keys: ["{untrusted_key}"] }}]"#
        ),
    );
    let path_hash = store_path_hash(&store_path);

    let (status, _, _) = call(
        &router,
        get_request(&format!("{mount}/{path_hash}.narinfo")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "an unverifiable upstream narinfo must be refused, not cached"
    );
    // Nothing was ingested: signature verification precedes the NAR fetch,
    // so the NAR blob never entered the front's store either.
    let digest = DigestInfo::new(sha256(payload), payload.len() as u64);
    assert!(
        front
            .nar_memory
            .get_part_unchunked(digest, 0, None)
            .await
            .is_err(),
        "no NAR should have been ingested for an unverified path"
    );
    Ok(())
}

#[nativelink_test]
async fn read_through_missing_upstream_path_is_404() -> Result<(), Error> {
    // An empty upstream: it has nothing, so every narinfo probe 404s.
    let upstream = CacheFixture::new("rt-empty-up");
    let (up_mount, up_router) = upstream.server("main", false);
    let (base, _handle) = spawn_http(up_router).await;
    let upstream_url = format!("{base}{up_mount}");
    let upstream_pubkey = server_public_key();

    let front = CacheFixture::new("rt-empty-front");
    let (mount, router) = front.server_with_extra(
        "main",
        &format!(
            r#"upstream_caches: [{{ url: "{upstream_url}", trusted_public_keys: ["{upstream_pubkey}"] }}]"#
        ),
    );
    let store_path = test_store_path("read-through-missing", "absent-1.0");
    let path_hash = store_path_hash(&store_path);

    let (status, _, _) = call(
        &router,
        get_request(&format!("{mount}/{path_hash}.narinfo")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    Ok(())
}

#[nativelink_test]
async fn read_through_answers_head_probe() -> Result<(), Error> {
    // `nix copy --from` (and `isValidPath`) probes availability with a HEAD
    // on the narinfo before issuing the GET, so read-through must answer HEAD
    // too or the client reports the path missing without ever fetching it.
    let upstream = CacheFixture::new("rt-head-up");
    let (up_mount, up_router) = upstream.server("main", false);
    let payload = b"read-through HEAD probe payload: opaque bytes";
    let store_path = test_store_path("read-through-head", "hi-1.0");
    let info = narinfo_for_payload(
        &store_path,
        format!("nar/{}", client_nar_basename(payload)),
        payload,
    );
    publish(&up_router, &up_mount, &info, payload).await;
    let (base, _handle) = spawn_http(up_router).await;
    let upstream_url = format!("{base}{up_mount}");
    let upstream_pubkey = server_public_key();

    let front = CacheFixture::new("rt-head-front");
    let (mount, router) = front.server_with_extra(
        "main",
        &format!(
            r#"upstream_caches: [{{ url: "{upstream_url}", trusted_public_keys: ["{upstream_pubkey}"] }}]"#
        ),
    );
    let path_hash = store_path_hash(&store_path);

    let (status, _, _) = call(
        &router,
        head_request(&format!("{mount}/{path_hash}.narinfo")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a HEAD on a locally-absent path must read-through and answer 200"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Negative-cache poisoning (transient vs definitive upstream miss)
// ---------------------------------------------------------------------------

/// A programmable upstream mock, richer than a `CacheFixture`: it counts
/// every `{hash}.narinfo` probe and can be flipped between three modes so a
/// single upstream can move from "transiently broken" to "serving". The NAR
/// GET returns the payload bytes verbatim.
///
/// This is what lets us assert the negative-cache poisoning fix: a 5xx probe
/// must be served as a miss WITHOUT being negative-cached (the next probe
/// re-queries upstream), whereas a genuine 404 IS negative-cached (a second
/// probe short-circuits without a new upstream request).
#[derive(Clone, Copy, PartialEq, Eq)]
enum UpstreamMode {
    /// Every narinfo probe returns `503` (a transient failure).
    ServerError,
    /// Every narinfo probe returns `404` (a definitive absence).
    NotFound,
    /// Serve the (signed) narinfo and the NAR bytes.
    Healthy,
}

struct MockUpstream {
    mode: Arc<StdMutex<UpstreamMode>>,
    narinfo_probes: Arc<AtomicUsize>,
}

/// Spawns a programmable upstream serving one signed store path. Returns the
/// base URL, the mode handle (to flip behavior between requests), the probe
/// counter, and the serving task guard.
async fn spawn_mock_upstream(
    store_path: &str,
    payload: &'static [u8],
    initial_mode: UpstreamMode,
) -> (
    String,
    Arc<StdMutex<UpstreamMode>>,
    Arc<AtomicUsize>,
    nativelink_util::task::JoinHandleDropGuard<()>,
) {
    let path_hash = store_path_hash(store_path).to_string();
    let nar_basename = client_nar_basename(payload);

    // A genuinely-signed narinfo for the server key the front trusts: the
    // front verifies the upstream signature before caching, so an unsigned
    // document would be refused regardless of the transient/definitive logic
    // under test.
    let mut info = narinfo_for_payload(store_path, format!("nar/{nar_basename}"), payload);
    let server_key = NixSigningKey::from_secret_string(SERVER_SECRET_KEY).expect("server key");
    info.sigs = vec![server_key.sign(&info.fingerprint())];
    let narinfo_text = info.render();

    let mode = Arc::new(StdMutex::new(initial_mode));
    let narinfo_probes = Arc::new(AtomicUsize::new(0));
    let state = MockUpstream {
        mode: Arc::clone(&mode),
        narinfo_probes: Arc::clone(&narinfo_probes),
    };
    let state = Arc::new(state);

    // The front's `upstream_url` ends in the mount path `/nix/main`, so the
    // probe it issues is `GET /nix/main/{hash}.narinfo`; register there.
    let narinfo_route = format!("/nix/main/{path_hash}.narinfo");
    let nar_route = format!("/nix/main/nar/{nar_basename}");

    let narinfo_state = Arc::clone(&state);
    let narinfo_body = narinfo_text.clone();
    let nar_state = Arc::clone(&state);

    let router = Router::new()
        .route(
            &narinfo_route,
            get(move || {
                let state = Arc::clone(&narinfo_state);
                let body = narinfo_body.clone();
                async move {
                    state.narinfo_probes.fetch_add(1, Ordering::SeqCst);
                    let mode = *state
                        .mode
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match mode {
                        UpstreamMode::ServerError => (
                            StatusCode::SERVICE_UNAVAILABLE,
                            "upstream is transiently unavailable".to_string(),
                        ),
                        UpstreamMode::NotFound => {
                            (StatusCode::NOT_FOUND, "no such path".to_string())
                        }
                        UpstreamMode::Healthy => (StatusCode::OK, body),
                    }
                }
            }),
        )
        .route(
            &nar_route,
            get(move || {
                let state = Arc::clone(&nar_state);
                async move {
                    let mode = *state
                        .mode
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match mode {
                        UpstreamMode::Healthy => (StatusCode::OK, Bytes::from_static(payload)),
                        _ => (StatusCode::NOT_FOUND, Bytes::from_static(b"")),
                    }
                }
            }),
        );
    let (base, handle) = spawn_http(router).await;
    (base, mode, narinfo_probes, handle)
}

/// Builds a front cache that reads through to `upstream_url`, trusting the
/// server key. `negative_ttl_s` controls how long a definitive miss is
/// remembered (set high so the second definitive probe reliably short-
/// circuits within the test).
fn front_for_upstream(
    fixture: &CacheFixture,
    upstream_url: &str,
    negative_ttl_s: u64,
) -> (String, Router) {
    let upstream_pubkey = server_public_key();
    fixture.server_with_extra(
        "main",
        &format!(
            r#"upstream_negative_ttl_s: {negative_ttl_s},
               upstream_caches: [{{ url: "{upstream_url}", trusted_public_keys: ["{upstream_pubkey}"] }}]"#
        ),
    )
}

/// THE negative-cache poisoning test. A transient upstream failure (`503`)
/// is served as a miss but must NOT be negative-cached: the moment the SAME
/// upstream recovers, the very next probe re-queries it and the path is
/// fetched, verified, and cached. A cached `404` here would hide a path that
/// actually exists upstream for the whole `upstream_negative_ttl`.
#[nativelink_test]
async fn transient_upstream_failure_is_not_negative_cached_and_recovers() -> Result<(), Error> {
    const PAYLOAD: &[u8] = b"read-through recovery payload: opaque bytes 0123456789";
    let store_path = test_store_path("rt-transient-seed", "recover-1.0");
    let (base, mode, probes, _handle) =
        spawn_mock_upstream(&store_path, PAYLOAD, UpstreamMode::ServerError).await;

    let front = CacheFixture::new("rt-transient-front");
    // A long negative TTL: if the transient miss were (wrongly) cached, the
    // recovery probe below would be short-circuited and the assertion would
    // fail — exactly the regression this locks in.
    let (mount, router) = front_for_upstream(&front, &format!("{base}/nix/main"), 3600);
    let path_hash = store_path_hash(&store_path);
    let narinfo_uri = format!("{mount}/{path_hash}.narinfo");

    // Probe 1: upstream 5xx -> the front serves a miss (404), ingests nothing.
    let (status, _, _) = call(&router, get_request(&narinfo_uri)).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a transient upstream 5xx is served as a miss"
    );
    assert_eq!(
        front.path_info_memory.len_for_test(),
        0,
        "nothing cached yet"
    );
    let after_first = probes.load(Ordering::SeqCst);
    assert!(after_first >= 1, "the upstream must have been probed once");

    // Flip the SAME upstream to healthy. Because the transient miss was NOT
    // negative-cached, the next probe must re-query upstream.
    *mode
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = UpstreamMode::Healthy;

    // Probe 2: re-queries upstream, fetches + verifies + caches, serves 200.
    let served = get_narinfo(&router, &mount, path_hash).await;
    assert_eq!(served.store_path, store_path);
    assert_eq!(served.nar_hash, sha256(PAYLOAD));
    assert!(
        probes.load(Ordering::SeqCst) > after_first,
        "recovery must re-probe upstream (transient miss must not be cached)"
    );

    // The NAR is now local and durable.
    let canonical = nar_url::canonical_nar_name(&sha256(PAYLOAD), PAYLOAD.len() as u64);
    let (status, _, body) = call(&router, get_request(&format!("{mount}/nar/{canonical}"))).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), PAYLOAD);
    Ok(())
}

/// The contrast case: a genuine `404` upstream IS negative-cached, so a
/// second probe within the TTL short-circuits and does NOT hit upstream
/// again (asserted via the probe counter on the mock).
#[nativelink_test]
async fn definitive_upstream_404_is_negative_cached_and_short_circuits() -> Result<(), Error> {
    const PAYLOAD: &[u8] = b"definitely-absent payload never served by the upstream";
    let store_path = test_store_path("rt-definitive-seed", "absent-1.0");
    let (base, _mode, probes, _handle) =
        spawn_mock_upstream(&store_path, PAYLOAD, UpstreamMode::NotFound).await;

    let front = CacheFixture::new("rt-definitive-front");
    let (mount, router) = front_for_upstream(&front, &format!("{base}/nix/main"), 3600);
    let path_hash = store_path_hash(&store_path);
    let narinfo_uri = format!("{mount}/{path_hash}.narinfo");

    // Probe 1: upstream 404 -> miss, and the absence is negative-cached.
    let (status, _, _) = call(&router, get_request(&narinfo_uri)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let after_first = probes.load(Ordering::SeqCst);
    assert!(after_first >= 1, "the first miss must probe upstream");

    // Probe 2: the negative cache short-circuits; no new upstream request.
    let (status, _, _) = call(&router, get_request(&narinfo_uri)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        probes.load(Ordering::SeqCst),
        after_first,
        "a negatively-cached 404 must not re-probe upstream within the TTL"
    );
    Ok(())
}

/// An unreachable upstream (connection refused) is a transport error — the
/// transient/indeterminate case. It is served as a miss but must NOT be
/// negative-cached, so the moment the SAME address starts accepting, the next
/// probe re-queries and the path resolves. A single front and a single stable
/// upstream address make this deterministic: the address is bound (so the
/// negative-cache key is fixed) but not served until after the first probe.
#[nativelink_test]
async fn unreachable_upstream_is_not_negative_cached() -> Result<(), Error> {
    const PAYLOAD: &[u8] = b"unreachable-then-reachable payload 0123456789";
    let store_path = test_store_path("rt-unreachable-seed", "flaky-1.0");
    let path_hash = store_path_hash(&store_path).to_string();
    let nar_basename = client_nar_basename(PAYLOAD);

    // Bind a stable loopback address, but do NOT serve on it yet: connections
    // are refused, which the read-through surfaces as a transport error.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind upstream");
    let addr = listener.local_addr().expect("addr");
    let upstream_url = format!("http://{addr}/nix/main");

    let front = CacheFixture::new("rt-unreachable-front");
    let (mount, router) = front_for_upstream(&front, &upstream_url, 3600);
    let narinfo_uri = format!("{mount}/{path_hash}.narinfo");

    // Probe 1: nothing is accepting on `addr` -> connection error -> miss,
    // nothing cached, and (critically) NOT negative-cached.
    let (status, _, _) = call(&router, get_request(&narinfo_uri)).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a connection error is a miss"
    );
    assert_eq!(front.path_info_memory.len_for_test(), 0);

    // Now start serving a validly-signed path on the SAME address.
    let mut info = narinfo_for_payload(&store_path, format!("nar/{nar_basename}"), PAYLOAD);
    let server_key = NixSigningKey::from_secret_string(SERVER_SECRET_KEY).expect("server key");
    info.sigs = vec![server_key.sign(&info.fingerprint())];
    let narinfo_text = info.render();
    let up_router = Router::new()
        .route(
            &format!("/nix/main/{path_hash}.narinfo"),
            get(move || {
                let body = narinfo_text.clone();
                async move { (StatusCode::OK, body) }
            }),
        )
        .route(
            &format!("/nix/main/nar/{nar_basename}"),
            get(move || async move { (StatusCode::OK, Bytes::from_static(PAYLOAD)) }),
        );
    let _handle = nativelink_util::spawn!("test_late_upstream", async move {
        axum::serve(listener, up_router.into_make_service())
            .await
            .expect("serve");
    });

    // Probe 2: because the connection error was not negative-cached, this
    // re-queries the now-live upstream, verifies, ingests, and serves 200.
    let served = get_narinfo(&router, &mount, &path_hash).await;
    assert_eq!(served.store_path, store_path);
    assert_eq!(served.nar_hash, sha256(PAYLOAD));
    Ok(())
}

/// A post-download NAR hash/size mismatch is a transient/indeterminate
/// condition (corruption or tampering in transit), NOT proof of absence: it
/// must be served as a miss and NOT negative-cached, so the next probe of a
/// now-honest upstream re-fetches. The mock serves a validly-signed narinfo
/// but NAR bytes that disagree with the signed `NarHash`.
#[nativelink_test]
async fn upstream_nar_hash_mismatch_is_not_negative_cached() -> Result<(), Error> {
    const PAYLOAD: &[u8] = b"the true NAR bytes the signed narinfo commits to 01234";
    const WRONG: &[u8] = b"tampered NAR bytes of the very same byte length !!!!!!";
    assert_eq!(PAYLOAD.len(), WRONG.len());
    let store_path = test_store_path("rt-mismatch-seed", "tampered-1.0");
    let path_hash = store_path_hash(&store_path).to_string();
    let nar_basename = client_nar_basename(PAYLOAD);

    // Signed narinfo commits to sha256(PAYLOAD); the NAR route serves WRONG.
    let mut info = narinfo_for_payload(&store_path, format!("nar/{nar_basename}"), PAYLOAD);
    let server_key = NixSigningKey::from_secret_string(SERVER_SECRET_KEY).expect("server key");
    info.sigs = vec![server_key.sign(&info.fingerprint())];
    let narinfo_text = info.render();

    let probes = Arc::new(AtomicUsize::new(0));
    let probes_route = Arc::clone(&probes);
    let router = Router::new()
        .route(
            &format!("/nix/main/{path_hash}.narinfo"),
            get(move || {
                let probes = Arc::clone(&probes_route);
                let body = narinfo_text.clone();
                async move {
                    probes.fetch_add(1, Ordering::SeqCst);
                    (StatusCode::OK, body)
                }
            }),
        )
        .route(
            &format!("/nix/main/nar/{nar_basename}"),
            get(move || async move { (StatusCode::OK, Bytes::from_static(WRONG)) }),
        );
    let (base, _handle) = spawn_http(router).await;

    let front = CacheFixture::new("rt-mismatch-front");
    let (mount, router) = front_for_upstream(&front, &format!("{base}/nix/main"), 3600);

    // Probe 1: the NAR fails to match its signed NarHash -> indeterminate ->
    // served as a miss, nothing cached, NOT negative-cached.
    let (status, _, _) = call(
        &router,
        get_request(&format!("{mount}/{path_hash}.narinfo")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a NAR/narinfo hash mismatch is served as a miss"
    );
    assert_eq!(
        front.path_info_memory.len_for_test(),
        0,
        "a tampered path must not be cached"
    );

    // Probe 2 must re-query upstream (the mismatch was not negative-cached).
    let after_first = probes.load(Ordering::SeqCst);
    let (status, _, _) = call(
        &router,
        get_request(&format!("{mount}/{path_hash}.narinfo")),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(
        probes.load(Ordering::SeqCst) > after_first,
        "a hash-mismatch miss must not be negative-cached; the next probe re-queries"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Write-token fail-closed
// ---------------------------------------------------------------------------

/// With `read_token_files` set but `write_token_files` NOT set, a PUT with a
/// valid READ token must fail closed (`401`) — a read token is never a write
/// token. Previously this silently granted write access to every read-token
/// holder. Reads with the token still succeed, and nothing is written.
#[nativelink_test]
async fn read_token_alone_cannot_write_without_write_tokens() -> Result<(), Error> {
    let fixture = CacheFixture::new("read-only-token");
    let read_token_path = token_file("read-only-token", READ_TOKEN);
    let (mount, router) = fixture.server_with_extra(
        "main",
        &format!(r#"read_token_files: ["{}"],"#, read_token_path.display()),
    );

    // The read token opens reads, including nix-cache-info.
    let (status, _, _) = call(
        &router,
        authed(
            get_request(&format!("{mount}/nix-cache-info")),
            &bearer(READ_TOKEN),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "read token opens reads");

    // A PUT carrying that same valid read token must be 401: with reads gated
    // and no write tokens configured, writes fail closed.
    let payload = b"bytes a read token must never be able to push";
    let nar_uri = format!("{mount}/nar/{}", client_nar_basename(payload));
    let (status, headers, _) = call(
        &router,
        authed(put_request(&nar_uri, payload), &bearer(READ_TOKEN)),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a read token must not authorize writes when no write tokens are set"
    );
    assert!(
        header_text(&headers, &header::WWW_AUTHENTICATE).starts_with("Basic"),
        "the 401 must carry the Basic challenge"
    );

    // An anonymous PUT is likewise 401 (read auth denies it first).
    assert_eq!(
        put_with_auth(&router, &nar_uri, payload, "Bearer not-a-token").await,
        StatusCode::UNAUTHORIZED,
        "an invalid token cannot write either"
    );

    // Nothing was written by any rejected PUT.
    assert_eq!(fixture.nar_memory.len_for_test(), 0);
    assert_eq!(fixture.path_info_memory.len_for_test(), 0);
    assert_eq!(fixture.alias_memory.len_for_test(), 0);
    Ok(())
}

/// The fully-open case (NO tokens at all) still allows anonymous writes:
/// fail-closed applies only once ANY token auth is configured. This guards
/// against the fail-closed fix over-reaching into trusted-network mode.
#[nativelink_test]
async fn no_tokens_configured_still_allows_anonymous_write() -> Result<(), Error> {
    let fixture = CacheFixture::new("open-write");
    let (mount, router) = fixture.server("main", false);

    let payload = b"anonymous push on a fully-open cache";
    // The whole NAR-then-narinfo flow succeeds with no Authorization header.
    let basename = put_nar(&router, &mount, payload).await;
    let store_path = test_store_path("open-write-seed", "open-1.0");
    let info = narinfo_for_payload(&store_path, format!("nar/{basename}"), payload);
    assert_eq!(
        put_narinfo(&router, &mount, &info).await,
        StatusCode::CREATED,
        "anonymous narinfo PUT must succeed with no tokens configured"
    );
    assert!(fixture.nar_memory.len_for_test() > 0, "the NAR was written");
    Ok(())
}

// ---------------------------------------------------------------------------
// Self-verifying direct ingest (no verify{} wrapper)
// ---------------------------------------------------------------------------

/// A bare-name NAR PUT whose body does NOT match the digest named in the URL
/// is rejected (`4xx`) and nothing is committed — even though this fixture's
/// NAR store is a plain in-memory store WITHOUT a `verify{}` wrapper. The
/// self-certification inside `ingest_nar_direct` is what stops the cache
/// poisoning on a plain store.
#[nativelink_test]
async fn put_nar_with_body_not_matching_named_digest_is_rejected() -> Result<(), Error> {
    // The URL names the digest of PAYLOAD_A, with a matching Content-Length,
    // but the body is PAYLOAD_B (same length, different bytes).
    const PAYLOAD_A: &[u8] = b"the bytes whose sha256 the URL name commits to !!";
    const PAYLOAD_B: &[u8] = b"entirely different bytes of the very same length.";

    // A plain MemoryStore NAR store (no VerifyStore), so only the in-handler
    // self-check can reject a mismatched body.
    let nar_memory = MemoryStore::new(&MemorySpec::default());
    let path_info_memory = MemoryStore::new(&MemorySpec::default());
    let path_info_store = CompletenessCheckingStore::new(
        Store::new(path_info_memory.clone()),
        Store::new(nar_memory.clone()),
    );
    let alias_memory = MemoryStore::new(&MemorySpec::default());
    let store_manager = Arc::new(StoreManager::new());
    store_manager
        .add_store(NAR_STORE_NAME, Store::new(nar_memory.clone()))
        .expect("store registers");
    store_manager
        .add_store(PATH_INFO_STORE_NAME, Store::new(path_info_store))
        .expect("store registers");
    store_manager
        .add_store(ALIAS_STORE_NAME, Store::new(alias_memory.clone()))
        .expect("store registers");

    let signing_key_path = std::env::temp_dir().join(format!(
        "nix_cache_server_test-{}-plain-nar.secret",
        std::process::id()
    ));
    std::fs::write(&signing_key_path, format!("{SERVER_SECRET_KEY}\n")).expect("write key");
    let config: NixCacheConfig = serde_json5::from_str(&format!(
        r#"{{
            cas_store: "{NAR_STORE_NAME}",
            path_info_store: "{PATH_INFO_STORE_NAME}",
            alias_store: "{ALIAS_STORE_NAME}",
            signing_key_files: ["{}"],
            spool_path: "{}",
        }}"#,
        signing_key_path.display(),
        signing_key_path.with_extension("spool").display()
    ))
    .expect("config parses");
    let server = NixCacheServer::new(
        &[WithInstanceName {
            instance_name: "main".to_string(),
            config,
        }],
        &store_manager,
    )
    .expect("server");
    let (mount, instance_router) = server.routers().pop().expect("router");
    let router = Router::new().nest(&mount, instance_router);

    assert_eq!(PAYLOAD_A.len(), PAYLOAD_B.len());
    let named = client_nar_basename(PAYLOAD_A);
    let (status, _, _) = call(
        &router,
        put_request(&format!("{mount}/nar/{named}"), PAYLOAD_B),
    )
    .await;
    assert!(
        status.is_client_error(),
        "a body that does not match its named digest must be 4xx, got {status}"
    );
    // The plain store must hold nothing: the mismatched bytes never committed.
    assert_eq!(
        nar_memory.len_for_test(),
        0,
        "no NAR must be committed on a digest mismatch, even without verify{{}}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Build-log Content-Encoding allowlist
// ---------------------------------------------------------------------------

/// A build-log PUT with a bogus `Content-Encoding` is a `400` (the value
/// would otherwise be replayed verbatim as a response header to every reader
/// of the shared log). An allowed encoding (`br`) is accepted and replayed on
/// a later GET.
#[nativelink_test]
async fn build_log_content_encoding_is_allowlisted() -> Result<(), Error> {
    // Not real brotli: the cache treats the body as opaque and only gates the
    // (allowlisted) Content-Encoding it will replay to later readers.
    const BR_BODY: &[u8] = b"\x0b\x02\x80pretend-brotli allowlisted log bytes\x03";

    let fixture = CacheFixture::new("log-enc-allowlist");
    let (mount, router) = fixture.server("main", false);

    let drv_path = test_store_path("log-enc-seed", "encoded-1.0.drv");
    let drv = store_path_basename(&drv_path);
    let log_uri = format!("{mount}/log/{drv}");

    // A bogus Content-Encoding is rejected with 400 and stores nothing.
    let mut bogus = put_request(&log_uri, b"some log bytes");
    bogus.headers_mut().insert(
        header::CONTENT_ENCODING,
        HeaderValue::from_static("totally-bogus"),
    );
    let (status, _, _) = call(&router, bogus).await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "an unlisted Content-Encoding must 400"
    );
    let (status, _, _) = call(&router, get_request(&log_uri)).await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "a rejected log PUT must not have stored anything"
    );

    // `br` is on the allowlist: accepted and replayed verbatim.
    let mut good = put_request(&log_uri, BR_BODY);
    good.headers_mut()
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static("br"));
    let (status, _, body) = call(&router, good).await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "an allowlisted encoding must be accepted: {}",
        String::from_utf8_lossy(&body)
    );
    let (status, headers, body) = call(&router, get_request(&log_uri)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(header_text(&headers, &header::CONTENT_ENCODING), "br");
    assert_eq!(body.as_ref(), BR_BODY);
    Ok(())
}
