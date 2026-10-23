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

//! Local Nix store access: store-path identity and a read-only reader over the
//! Nix `SQLite` database (`<store_dir>/../var/nix/db/db.sqlite`).

use nativelink_error::{Code, Error, ResultExt, make_err, make_input_err};
use nativelink_nix::nixbase32;
use rusqlite::{Connection, OpenFlags};

/// The default Nix store directory.
pub const DEFAULT_STORE_DIR: &str = "/nix/store";

/// The 32-character length of the nixbase32 store-path hash.
const STORE_PATH_HASH_LEN: usize = 32;

/// The nixbase32-encoded length of a 32-byte `sha256` digest.
const SHA256_NIXBASE32_LEN: usize = 52;

/// The hex-encoded (base16) length of a 32-byte `sha256` digest.
const SHA256_HEX_LEN: usize = 64;

/// The number of bytes in a `sha256` digest (a NAR hash).
const SHA256_LEN: usize = 32;

/// A validated absolute Nix store path of the form
/// `<store_dir>/<32-char nixbase32 hash>-<name>`.
///
/// The hash part is the store path's identity (a 20-byte truncated digest,
/// nixbase32-encoded); the name is a human label. Both are validated on
/// construction, so a `StorePath` can never carry a traversal segment or a
/// malformed hash into a URL or a database query.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct StorePath {
    full_path: String,
    store_dir_len: usize,
}

impl StorePath {
    /// Parses and validates a full store path against `store_dir`.
    pub fn from_full_path(store_dir: &str, full_path: &str) -> Result<Self, Error> {
        let store_dir = store_dir.trim_end_matches('/');
        let rest = full_path
            .strip_prefix(store_dir)
            .and_then(|r| r.strip_prefix('/'))
            .ok_or_else(|| {
                make_input_err!("'{full_path}' is not under the store directory '{store_dir}'")
            })?;
        if rest.contains('/') {
            return Err(make_input_err!(
                "store path '{full_path}' has a nested path component"
            ));
        }
        let (hash, name) = rest.split_at_checked(STORE_PATH_HASH_LEN).ok_or_else(|| {
            make_input_err!("store path base name '{rest}' is shorter than a 32-char hash")
        })?;
        if nixbase32::decode(hash).is_err() {
            return Err(make_input_err!(
                "store path hash '{hash}' is not valid nixbase32"
            ));
        }
        let name = name.strip_prefix('-').ok_or_else(|| {
            make_input_err!("store path '{rest}' is missing the '-' name separator")
        })?;
        if !is_valid_store_path_name(name) {
            return Err(make_input_err!("store path name '{name}' is invalid"));
        }
        Ok(Self {
            full_path: format!("{store_dir}/{rest}"),
            store_dir_len: store_dir.len(),
        })
    }

    /// The full path, e.g. `/nix/store/<hash>-<name>`.
    #[must_use]
    pub fn full_path(&self) -> &str {
        &self.full_path
    }

    /// The 32-character nixbase32 hash part (the store path's identity).
    #[must_use]
    pub fn hash_part(&self) -> &str {
        let base = &self.full_path[self.store_dir_len + 1..];
        &base[..STORE_PATH_HASH_LEN]
    }

    /// The human-readable name after the hash.
    #[must_use]
    pub fn name(&self) -> &str {
        let base = &self.full_path[self.store_dir_len + 1..];
        &base[STORE_PATH_HASH_LEN + 1..]
    }

    /// The store-path basename `<hash>-<name>` (no store directory), as a
    /// `.narinfo` `References`/`Deriver` line requires — those are basenames,
    /// only the `StorePath` line is a full path.
    #[must_use]
    pub fn base_name(&self) -> &str {
        &self.full_path[self.store_dir_len + 1..]
    }
}

/// Whether `name` matches Nix's store-path name charset (`checkName`): non-empty,
/// no leading `.`, and only `[0-9a-zA-Z+._?=-]`.
fn is_valid_store_path_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.' | b'_' | b'?' | b'=')
        })
}

/// Metadata for a valid store path, drawn from the Nix database plus the NAR
/// serialization. This is exactly the information a `.narinfo` carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathMeta {
    /// The store path this describes.
    pub path: StorePath,
    /// `sha256` of the (uncompressed) NAR — the `NarHash`.
    pub nar_hash: [u8; 32],
    /// Length in bytes of the (uncompressed) NAR — the `NarSize`.
    pub nar_size: u64,
    /// Direct references, in Nix's canonical (sorted) order.
    pub references: Vec<StorePath>,
    /// The `.drv` that produced this path, if recorded.
    pub deriver: Option<String>,
    /// Existing signatures (`name:base64sig`), if any.
    pub sigs: Vec<String>,
    /// The content-address field (`ca`), if this is a fixed-output path.
    pub ca: Option<String>,
}

/// A read-only handle to the local Nix store's `SQLite` database.
///
/// Opened `SQLITE_OPEN_READ_ONLY` so a running `nix-daemon` (WAL mode) is never
/// blocked. Metadata (`references`, `deriver`, `sigs`, `ca`, `nar_hash`,
/// `nar_size`) is authoritative here; the NAR bytes are produced separately by
/// [`crate::nar::dump_path`].
pub struct NixStore {
    store_dir: String,
    /// A read-only handle to `db.sqlite`. Held for the reader's lifetime.
    db: Connection,
}

impl core::fmt::Debug for NixStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // The `rusqlite::Connection` is not meaningfully printable and holds
        // an open file handle; expose only the store directory.
        f.debug_struct("NixStore")
            .field("store_dir", &self.store_dir)
            .finish_non_exhaustive()
    }
}

impl NixStore {
    /// Opens the store rooted at `store_dir` (its DB is at
    /// `<store_dir>/../var/nix/db/db.sqlite`, unless `NIX_STATE_DIR` overrides
    /// the state directory).
    ///
    /// The database is opened for live, query-only reads that see a running
    /// `nix-daemon`'s WAL commits (see [`open_nix_db`]); it never writes nix's
    /// data and never blocks the daemon. On a read-only DB directory it degrades
    /// to a frozen read-only/`immutable=1` snapshot.
    pub fn open(store_dir: &str) -> Result<Self, Error> {
        let store_dir = store_dir.trim_end_matches('/').to_string();
        let db_path = db_path_for_store(&store_dir);
        let db = open_nix_db(&db_path)?;
        Ok(Self { store_dir, db })
    }

    /// The store directory this reader is rooted at.
    #[must_use]
    pub fn store_dir(&self) -> &str {
        &self.store_dir
    }

    /// Reads the full metadata for `path`, or a `NotFound` error if it is not
    /// a currently-valid path in the store.
    pub fn query_path_info(&self, path: &StorePath) -> Result<PathMeta, Error> {
        let full_path = path.full_path();
        // The `path` column is `UNIQUE`, so this is a single index probe. We
        // read `narSize` as its canonical decimal text (`CAST(... AS TEXT)`)
        // and parse it with the same strict parser used at the narinfo
        // boundary, so an out-of-range or non-canonical value is rejected the
        // same way regardless of whether it arrived from the DB or a narinfo.
        let (id, hash, nar_size, deriver, sigs, ca) = self
            .db
            .query_row(
                "SELECT id, hash, CAST(narSize AS TEXT), deriver, sigs, ca \
                 FROM ValidPaths WHERE path = ?1",
                [full_path],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Option<String>>(5)?,
                    ))
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => make_err!(
                    Code::NotFound,
                    "store path '{full_path}' is not valid in '{}'",
                    self.store_dir
                ),
                other => make_err!(
                    Code::Internal,
                    "querying path info for '{full_path}': {other}"
                ),
            })?;

        let nar_hash = decode_nar_hash(&hash)
            .err_tip(|| format!("decoding NAR hash for store path '{full_path}'"))?;
        let nar_size = nar_size
            .ok_or_else(|| {
                make_err!(
                    Code::Internal,
                    "store path '{full_path}' has a NULL narSize"
                )
            })
            .and_then(|value| {
                parse_strict_u64(&value)
                    .err_tip(|| format!("store path '{full_path}' has an invalid narSize"))
            })?;
        let references = self.references_for(id)?;

        Ok(PathMeta {
            path: path.clone(),
            nar_hash,
            nar_size,
            references,
            // The DB stores `deriver` as a full path; a narinfo `Deriver` is a
            // basename, so strip the store dir.
            deriver: non_empty(deriver).map(|d| store_basename(&self.store_dir, d)),
            sigs: split_sigs(sigs.as_deref()),
            ca: non_empty(ca),
        })
    }

    /// Whether `path` is a currently-valid path in the store.
    pub fn is_valid_path(&self, path: &StorePath) -> Result<bool, Error> {
        let full_path = path.full_path();
        self.db
            .query_row(
                "SELECT 1 FROM ValidPaths WHERE path = ?1 LIMIT 1",
                [full_path],
                |_row| Ok(()),
            )
            .map(|()| true)
            .or_else(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => Ok(false),
                other => Err(make_err!(
                    Code::Internal,
                    "checking validity of '{full_path}': {other}"
                )),
            })
    }

    /// Every currently-valid store path. Paths that fail to parse against
    /// `store_dir` are logged and skipped rather than aborting the whole scan,
    /// so one malformed row cannot hide the rest of the store.
    pub fn all_valid_paths(&self) -> Result<Vec<StorePath>, Error> {
        let mut stmt = self
            .db
            .prepare("SELECT path FROM ValidPaths ORDER BY path")
            .map_err(|e| make_err!(Code::Internal, "preparing all-valid-paths query: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| make_err!(Code::Internal, "scanning all valid paths: {e}"))?;

        let mut paths = Vec::new();
        for row in rows {
            let full_path =
                row.map_err(|e| make_err!(Code::Internal, "reading a valid-path row: {e}"))?;
            match StorePath::from_full_path(&self.store_dir, &full_path) {
                Ok(store_path) => paths.push(store_path),
                Err(err) => {
                    tracing::warn!(
                        path = %full_path,
                        error = ?err,
                        "skipping unparsable store path in ValidPaths",
                    );
                }
            }
        }
        Ok(paths)
    }

    /// The direct references of the `ValidPaths` row with the given id, parsed
    /// into [`StorePath`]s and kept in Nix's canonical (path-sorted) order.
    fn references_for(&self, id: i64) -> Result<Vec<StorePath>, Error> {
        let mut stmt = self
            .db
            .prepare(
                "SELECT v.path FROM Refs r \
                 JOIN ValidPaths v ON v.id = r.reference \
                 WHERE r.referrer = ?1 ORDER BY v.path",
            )
            .map_err(|e| make_err!(Code::Internal, "preparing references query: {e}"))?;
        let rows = stmt
            .query_map([id], |row| row.get::<_, String>(0))
            .map_err(|e| make_err!(Code::Internal, "querying references for id {id}: {e}"))?;

        let mut references = Vec::new();
        for row in rows {
            let full_path =
                row.map_err(|e| make_err!(Code::Internal, "reading a reference row: {e}"))?;
            let store_path = StorePath::from_full_path(&self.store_dir, &full_path)
                .err_tip(|| format!("parsing reference '{full_path}'"))?;
            references.push(store_path);
        }
        Ok(references)
    }
}

/// Derives the `db.sqlite` path for `store_dir`. Honours `NIX_STATE_DIR`
/// (Nix's own override) when set, otherwise uses `<store_dir>/../var/nix`.
fn db_path_for_store(store_dir: &str) -> std::path::PathBuf {
    let state_dir = std::env::var_os("NIX_STATE_DIR").map_or_else(
        || std::path::Path::new(store_dir).join("../var/nix"),
        std::path::PathBuf::from,
    );
    state_dir.join("db/db.sqlite")
}

/// Opens the Nix DB for **live** reads that track a running `nix-daemon`'s WAL
/// commits.
///
/// The subtlety that makes this necessary: a plain `SQLITE_OPEN_READ_ONLY`
/// opener that cannot write the WAL `-shm` shared-memory index only ever sees a
/// frozen snapshot as of the last checkpoint, and never observes commits made
/// after it opened. For a long-lived `nl-watch-store` daemon that means every
/// path built after startup is invisible — the watcher fires but the DB lookup
/// reports the path as not valid. So we prefer a `READ_WRITE` handle (which lets
/// SQLite maintain the wal-index, so each query sees the latest commit),
/// immediately clamped with `PRAGMA query_only = ON` so this connection can
/// never modify nix's database — a safe, ordinary WAL reader. Only if the
/// read-write open fails (a genuinely read-only DB directory) do we fall back to
/// read-only, then the `immutable=1` snapshot, trading live freshness for access.
fn open_nix_db(db_path: &std::path::Path) -> Result<Connection, Error> {
    // Preferred: a read-write handle demoted to query-only. This is what lets a
    // persistent reader see paths committed after it opened.
    if let Ok(conn) = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE) {
        match conn.execute_batch("PRAGMA query_only = ON;") {
            Ok(()) => return Ok(conn),
            Err(err) => tracing::debug!(
                db = %db_path.display(),
                error = %err,
                "could not clamp read-write handle to query_only; falling back to read-only",
            ),
        }
    }
    match Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(conn) => Ok(conn),
        Err(err) if is_wal_access_error(&err) => {
            let uri = format!("file:{}?immutable=1", db_path.display());
            tracing::debug!(
                db = %db_path.display(),
                error = %err,
                "read-only open hit a WAL/locking error; retrying with immutable=1",
            );
            Connection::open_with_flags(
                &uri,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )
            .map_err(|e| {
                make_err!(
                    Code::Internal,
                    "opening Nix DB '{}' read-only (immutable fallback): {e}",
                    db_path.display()
                )
            })
        }
        Err(err) => Err(make_err!(
            Code::Internal,
            "opening Nix DB '{}' read-only: {err}",
            db_path.display()
        )),
    }
}

/// Whether a `rusqlite` open error looks like the WAL-sidecar access problem
/// (the DB dir is not writable), for which the `immutable=1` URI is the fix.
/// `SQLite` reports these as `SQLITE_CANTOPEN`, `SQLITE_IOERR`, `SQLITE_BUSY`,
/// or `SQLITE_READONLY` primary result codes.
const fn is_wal_access_error(err: &rusqlite::Error) -> bool {
    use rusqlite::ErrorCode::{
        CannotOpen, DatabaseBusy, DatabaseLocked, ReadOnly, SystemIoFailure,
    };
    matches!(
        err,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: CannotOpen | SystemIoFailure | DatabaseBusy | DatabaseLocked | ReadOnly,
                ..
            },
            _,
        )
    )
}

/// Decodes a `ValidPaths.hash` value (`sha256:<encoded>`) into a 32-byte
/// digest. The encoding is nixbase32 (52 chars) in modern stores or base16 (64
/// chars) in older ones; detect by length. Anything that is not a 32-byte
/// `sha256` digest is rejected.
fn decode_nar_hash(hash: &str) -> Result<[u8; SHA256_LEN], Error> {
    let body = hash
        .strip_prefix("sha256:")
        .ok_or_else(|| make_input_err!("NAR hash '{hash}' is not a sha256 hash"))?;
    let bytes = match body.len() {
        SHA256_NIXBASE32_LEN => nixbase32::decode(body)
            .err_tip(|| format!("NAR hash '{hash}' has an invalid nixbase32 body"))?,
        SHA256_HEX_LEN => hex::decode(body)
            .map_err(|e| make_input_err!("NAR hash '{hash}' has an invalid base16 body: {e}"))?,
        other => {
            return Err(make_input_err!(
                "NAR hash '{hash}' has an unexpected body length {other} \
                 (want {SHA256_NIXBASE32_LEN} nixbase32 or {SHA256_HEX_LEN} base16 chars)"
            ));
        }
    };
    <[u8; SHA256_LEN]>::try_from(bytes.as_slice())
        .map_err(|_| make_input_err!("NAR hash '{hash}' did not decode to {SHA256_LEN} bytes"))
}

/// Maps a NULL-or-empty database text field to `None`.
fn non_empty(value: Option<String>) -> Option<String> {
    value.filter(|s| !s.is_empty())
}

/// Strips the store directory from a full path to its `<hash>-<name>` basename
/// (leaving a value not under the store dir untouched).
fn store_basename(store_dir: &str, path: String) -> String {
    match path
        .strip_prefix(store_dir)
        .and_then(|rest| rest.strip_prefix('/'))
    {
        Some(base) if !base.is_empty() => base.to_string(),
        _ => path,
    }
}

/// Splits a space-separated `sigs` field into individual signatures, dropping
/// empty fragments (as Nix does when tokenizing on whitespace).
fn split_sigs(sigs: Option<&str>) -> Vec<String> {
    sigs.map(|s| {
        s.split(' ')
            .filter(|part| !part.is_empty())
            .map(String::from)
            .collect()
    })
    .unwrap_or_default()
}

/// Parses a `NAR_SIZE`-style unsigned decimal exactly as Nix does (no `+`, no
/// leading zeros), used across the DB and narinfo boundaries.
pub(crate) fn parse_strict_u64(value: &str) -> Result<u64, Error> {
    if value.is_empty()
        || !value.bytes().all(|b| b.is_ascii_digit())
        || (value.len() > 1 && value.starts_with('0'))
    {
        return Err(make_input_err!(
            "'{value}' is not a canonical unsigned integer"
        ));
    }
    value
        .parse::<u64>()
        .err_tip(|| format!("integer '{value}' out of range"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_path_parses_hash_and_name() {
        let p = StorePath::from_full_path(
            "/nix/store",
            "/nix/store/00000000000000000000000000000000-hello-2.12.1",
        )
        .expect("valid store path");
        assert_eq!(p.hash_part(), "00000000000000000000000000000000");
        assert_eq!(p.name(), "hello-2.12.1");
        assert_eq!(
            p.full_path(),
            "/nix/store/00000000000000000000000000000000-hello-2.12.1"
        );
    }

    #[test]
    fn store_path_rejects_traversal_and_bad_hash() {
        // Nested path component.
        assert!(StorePath::from_full_path("/nix/store", "/nix/store/aaaa-x/../../etc").is_err());
        // 'e' is not in the nixbase32 alphabet, and the hash is too short.
        assert!(StorePath::from_full_path("/nix/store", "/nix/store/eee-x").is_err());
        // Not under the store dir.
        assert!(StorePath::from_full_path("/nix/store", "/etc/passwd").is_err());
    }

    // sha256("hello") in both encodings, from the nixbase32 golden vectors.
    const HELLO_HEX: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    const HELLO_NIXBASE32: &str = "094qif9n4cq4fdg459qzbhg1c6wywawwaaivx0k0x8xhbyx4vwic";

    #[test]
    fn decode_nar_hash_accepts_both_encodings() {
        let expected: [u8; 32] = hex::decode(HELLO_HEX)
            .expect("valid hex")
            .try_into()
            .expect("32 bytes");
        // Modern stores: nixbase32 (52 chars). Older stores: base16 (64 chars).
        assert_eq!(
            decode_nar_hash(&format!("sha256:{HELLO_NIXBASE32}")).expect("nixbase32 decodes"),
            expected
        );
        assert_eq!(
            decode_nar_hash(&format!("sha256:{HELLO_HEX}")).expect("base16 decodes"),
            expected
        );
    }

    #[test]
    fn decode_nar_hash_rejects_bad_input() {
        // Wrong algorithm.
        assert!(decode_nar_hash(&format!("sha1:{HELLO_HEX}")).is_err());
        // No algorithm prefix.
        assert!(decode_nar_hash(HELLO_HEX).is_err());
        // Body length that is neither 52 (nixbase32) nor 64 (base16).
        assert!(decode_nar_hash("sha256:deadbeef").is_err());
        // Right length, invalid hex.
        assert!(decode_nar_hash(&format!("sha256:{}", "z".repeat(64))).is_err());
    }

    #[test]
    fn references_and_deriver_are_basenames_not_full_paths() {
        // A narinfo `References`/`Deriver` line holds BASENAMES (`<hash>-<name>`),
        // never full paths — the server rejects a `/nix/store/...` reference with
        // a 400. Guard both the StorePath accessor and the deriver stripping.
        let p = StorePath::from_full_path(
            "/nix/store",
            "/nix/store/bxlz3hzl8k02rnxpgml6krqkrnpird91-stdenv-linux.drv",
        )
        .expect("valid store path");
        assert_eq!(
            p.base_name(),
            "bxlz3hzl8k02rnxpgml6krqkrnpird91-stdenv-linux.drv"
        );
        assert!(!p.base_name().contains('/'));

        assert_eq!(
            store_basename(
                "/nix/store",
                "/nix/store/bxlz3hzl8k02rnxpgml6krqkrnpird91-stdenv-linux.drv".to_string()
            ),
            "bxlz3hzl8k02rnxpgml6krqkrnpird91-stdenv-linux.drv"
        );
        // A value not under the store dir is left untouched, never truncated.
        assert_eq!(
            store_basename("/nix/store", "elsewhere".to_string()),
            "elsewhere"
        );
    }

    #[test]
    fn reconciliation_enumerates_every_multi_output_row() {
        let db = Connection::open_in_memory().expect("in-memory Nix DB");
        db.execute("CREATE TABLE ValidPaths (path TEXT NOT NULL)", [])
            .expect("schema");
        for path in [
            "/nix/store/00000000000000000000000000000000-package",
            "/nix/store/11111111111111111111111111111111-package-dev",
        ] {
            db.execute("INSERT INTO ValidPaths(path) VALUES (?1)", [path])
                .expect("valid path row");
        }
        let store = NixStore {
            store_dir: "/nix/store".to_string(),
            db,
        };
        let paths = store.all_valid_paths().expect("reconciliation snapshot");
        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].name(), "package");
        assert_eq!(paths[1].name(), "package-dev");
    }

    #[test]
    fn non_empty_and_split_sigs_behave() {
        assert_eq!(non_empty(None), None);
        assert_eq!(non_empty(Some(String::new())), None);
        assert_eq!(non_empty(Some("x".to_string())), Some("x".to_string()));

        assert_eq!(split_sigs(None), Vec::<String>::new());
        assert_eq!(split_sigs(Some("")), Vec::<String>::new());
        assert_eq!(
            split_sigs(Some("cache.nixos.org-1:AAA  cache2:BBB ")),
            vec![
                "cache.nixos.org-1:AAA".to_string(),
                "cache2:BBB".to_string()
            ]
        );
    }

    #[test]
    fn open_on_bogus_store_dir_errors_without_panic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bogus = dir.path().join("no-such-store");
        let err = NixStore::open(bogus.to_str().expect("utf8 path"))
            .expect_err("opening a store with no db.sqlite must be an Err");
        assert_eq!(err.code, Code::Internal);
    }

    /// The world-readable Nix DB used for the integration test below. Present
    /// on real NixOS/Nix hosts; the test skips cleanly when it is absent.
    const REAL_DB: &str = "/nix/var/nix/db/db.sqlite";

    #[test]
    fn reads_real_nix_db_when_present() {
        if !std::path::Path::new(REAL_DB).exists() {
            eprintln!("skipping: {REAL_DB} not present");
            return;
        }
        let store = NixStore::open(DEFAULT_STORE_DIR).expect("open real Nix store");

        // Pick a real path directly from the DB (independent of our reader).
        let sample: String = store
            .db
            .query_row("SELECT path FROM ValidPaths LIMIT 1", [], |row| row.get(0))
            .expect("at least one valid path");
        let path =
            StorePath::from_full_path(DEFAULT_STORE_DIR, &sample).expect("sample path parses");

        // Full metadata round-trips and is internally consistent.
        let meta = store.query_path_info(&path).expect("query_path_info");
        assert_eq!(meta.path, path);
        assert_eq!(meta.nar_hash.len(), 32);
        assert!(meta.nar_size > 0, "nar_size should be positive");
        for reference in &meta.references {
            // Every reference is itself a valid, well-formed store path.
            assert!(reference.full_path().starts_with(DEFAULT_STORE_DIR));
            assert!(
                store.is_valid_path(reference).expect("is_valid_path(ref)"),
                "reference {} should be valid",
                reference.full_path()
            );
        }

        // Validity: true for the sample, false for a syntactically-valid but
        // absent hash (all-zero hash with an unlikely name).
        assert!(store.is_valid_path(&path).expect("is_valid_path(sample)"));
        let absent = StorePath::from_full_path(
            DEFAULT_STORE_DIR,
            "/nix/store/00000000000000000000000000000000-definitely-not-real",
        )
        .expect("absent path parses");
        assert!(
            !store.is_valid_path(&absent).expect("is_valid_path(absent)"),
            "an all-zero-hash path should not be valid"
        );
        assert!(
            store.query_path_info(&absent).unwrap_err().code == Code::NotFound,
            "query_path_info on an absent path must be NotFound"
        );

        // all_valid_paths returns a non-empty set that includes the sample.
        let all = store.all_valid_paths().expect("all_valid_paths");
        assert!(!all.is_empty(), "the store has at least one valid path");
        assert!(
            all.contains(&path),
            "the sample path is among all valid paths"
        );
    }
}
