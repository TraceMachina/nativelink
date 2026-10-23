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

//! The `.narinfo` wire format: parsing, rendering, and the signing
//! fingerprint.
//!
//! Reference implementation: `src/libstore/nar-info.cc` in Nix 2.34; the
//! signing fingerprint lives in `src/libstore/path-info.cc`
//! (`ValidPathInfo::fingerprint`). Wire facts this module implements
//! exactly:
//!
//! - Lines are `Name: value\n`; Nix takes the value starting at
//!   `colon + 2`, so the separator is exactly colon-space. An empty
//!   `References` field still renders as `"References: \n"` (trailing
//!   space before the newline).
//! - Unknown field names are silently ignored on parse. Duplicate
//!   `References` or `CA` lines are errors; `Sig` may repeat.
//! - A missing `Compression` field means `bzip2` to Nix, so the field is
//!   non-optional here and always rendered explicitly.
//! - `Deriver: unknown-deriver` (and an empty `Deriver` value) parses to
//!   `None` and is never rendered.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use nativelink_error::{Error, ResultExt, make_input_err};

use crate::nixbase32;

/// A parsed `.narinfo` document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NarInfo {
    /// Full store path, e.g. `/nix/store/<hash>-<name>`.
    pub store_path: String,
    /// URL of the NAR, usually relative to the cache root.
    pub url: String,
    /// Compression of the NAR at `url`, verbatim: `none`, `xz`, `zstd`,
    /// `bzip2`, `br`, ... Never empty: an absent `Compression` line means
    /// `bzip2` to Nix, so rendering must always be explicit.
    pub compression: String,
    /// `sha256` of the compressed NAR at `url`.
    pub file_hash: Option<[u8; 32]>,
    /// Size in bytes of the compressed NAR at `url`.
    pub file_size: Option<u64>,
    /// `sha256` of the uncompressed NAR.
    pub nar_hash: [u8; 32],
    /// Size in bytes of the uncompressed NAR.
    pub nar_size: u64,
    /// References as store-path basenames only.
    pub references: Vec<String>,
    /// Basename of the deriving `.drv`, if known.
    pub deriver: Option<String>,
    /// Platform string, e.g. `x86_64-linux` (attic compatibility; Nix
    /// itself ignores this field).
    pub system: Option<String>,
    /// Signatures, verbatim `name:base64`, in the order encountered.
    pub sigs: Vec<String>,
    /// Content-address, verbatim, for fixed-output paths.
    pub ca: Option<String>,
}

/// Parses a `sha256` hash field value (`NarHash`/`FileHash`) into raw
/// digest bytes.
///
/// Accepts `sha256:` followed by base16 (64 chars), nix32 (52 chars), or
/// base64 (44 chars), as well as the SRI form `sha256-<base64>`; this is
/// the set accepted by `Hash::parseAnyPrefixed` in Nix.
///
/// # Errors
///
/// Returns an `InvalidArgument` error for non-`sha256` algorithms, for
/// encodings of the wrong length, and for undecodable digests.
fn parse_hash_field(value: &str) -> Result<[u8; 32], Error> {
    let digest = if let Some(rest) = value.strip_prefix("sha256:") {
        match rest.len() {
            64 => hex::decode(rest)
                .map_err(|e| make_input_err!("invalid base16 sha256 hash '{rest}': {e}"))?,
            52 => nixbase32::decode(rest).err_tip(|| format!("in nix32 sha256 hash '{rest}'"))?,
            44 => BASE64
                .decode(rest)
                .map_err(|e| make_input_err!("invalid base64 sha256 hash '{rest}': {e}"))?,
            len => {
                return Err(make_input_err!(
                    "invalid sha256 hash '{value}': length {len} is not base16 (64), nix32 (52), or base64 (44)"
                ));
            }
        }
    } else if let Some(rest) = value.strip_prefix("sha256-") {
        BASE64
            .decode(rest)
            .map_err(|e| make_input_err!("invalid SRI sha256 hash '{value}': {e}"))?
    } else {
        return Err(make_input_err!(
            "unsupported hash algorithm in '{value}': only sha256 is supported"
        ));
    };
    digest.try_into().map_err(|bytes: Vec<u8>| {
        make_input_err!(
            "sha256 hash '{value}' decodes to {} bytes, expected 32",
            bytes.len()
        )
    })
}

/// Parses a strict canonical decimal `u64` the way Nix's `string2Int`
/// does, rejecting the leading `+` and leading zeros that Rust's
/// `str::parse::<u64>` silently accepts.
///
/// A value is accepted only when it is non-empty, all ASCII digits, has
/// no leading `+`, and has no leading zero unless it is the single digit
/// `"0"`. This mirrors the canonical-decimal check in
/// `nix_cache_server`'s `compressed_upload_digest`, so `FileSize`/`NarSize`
/// on the wire are held to exactly the size grammar the rest of the facade
/// uses.
///
/// # Errors
///
/// Returns an `InvalidArgument` error for an empty string, any non-digit
/// byte (including a leading `+` or `-`), a leading zero on a multi-digit
/// value, or a value that overflows `u64`.
fn parse_strict_u64(value: &str, field: &str) -> Result<u64, Error> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return Err(make_input_err!(
            "corrupt narinfo: {field} '{value}' is not a decimal number"
        ));
    }
    if value.len() > 1 && value.starts_with('0') {
        return Err(make_input_err!(
            "corrupt narinfo: {field} '{value}' has leading zeros"
        ));
    }
    value
        .parse::<u64>()
        .map_err(|e| make_input_err!("corrupt narinfo: {field} '{value}' does not fit in u64: {e}"))
}

/// Parses a `.narinfo` document.
///
/// Follows `NarInfo::NarInfo(...)` in Nix's `src/libstore/nar-info.cc`:
/// unknown fields are ignored, duplicate `References`/`CA` lines are
/// rejected, `Sig` accumulates, `Deriver: unknown-deriver` becomes
/// `None`, a missing `Compression` defaults to `bzip2`, and a missing
/// `StorePath`/`URL`/`NarHash` or a missing/zero `NarSize` makes the
/// document corrupt.
///
/// # Errors
///
/// Returns an `InvalidArgument` error for malformed lines, invalid field
/// values, duplicate `References`/`CA` lines, or missing required fields.
pub fn parse(text: &str) -> Result<NarInfo, Error> {
    let mut store_path: Option<String> = None;
    let mut url: Option<String> = None;
    let mut compression: Option<String> = None;
    let mut file_hash: Option<[u8; 32]> = None;
    let mut file_size: Option<u64> = None;
    let mut nar_hash: Option<[u8; 32]> = None;
    let mut nar_size: Option<u64> = None;
    let mut references: Option<Vec<String>> = None;
    let mut deriver: Option<String> = None;
    let mut system: Option<String> = None;
    let mut sigs: Vec<String> = Vec::new();
    let mut ca: Option<String> = None;

    for raw_line in text.split('\n') {
        // Strip a single trailing '\r' so a CRLF-served narinfo parses
        // identically to an LF one; otherwise every value would carry a
        // '\r' suffix (`Compression: xz\r` looks unsupported, a hash or
        // size gains a byte and fails to parse).
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() {
            continue;
        }
        let colon = line
            .find(':')
            .ok_or_else(|| make_input_err!("corrupt narinfo: expecting ':' in line '{line}'"))?;
        let name = &line[..colon];
        // Nix takes the value starting at colon + 2: the separator is
        // exactly colon-space. Out-of-range means an empty value.
        let value = line.get(colon + 2..).unwrap_or("");
        match name {
            "StorePath" => store_path = Some(value.to_string()),
            "URL" => url = Some(value.to_string()),
            "Compression" => compression = Some(value.to_string()),
            "FileHash" => {
                file_hash = Some(parse_hash_field(value).err_tip(|| "in narinfo FileHash field")?);
            }
            "FileSize" => {
                file_size = Some(parse_strict_u64(value, "FileSize")?);
            }
            "NarHash" => {
                nar_hash = Some(parse_hash_field(value).err_tip(|| "in narinfo NarHash field")?);
            }
            "NarSize" => {
                nar_size = Some(parse_strict_u64(value, "NarSize")?);
            }
            "References" => {
                if references.is_some() {
                    return Err(make_input_err!("corrupt narinfo: extra References line"));
                }
                // Nix-faithful: references are ASCII-space-delimited (Nix
                // splits the value on `' '`). Empty tokens from double or
                // trailing spaces are dropped so they never become empty
                // references (which would fail later store-path validation
                // and corrupt the fingerprint).
                references = Some(
                    value
                        .split(' ')
                        .filter(|part| !part.is_empty())
                        .map(str::to_string)
                        .collect(),
                );
            }
            "Deriver" => {
                // `unknown-deriver` is Nix's explicit sentinel for "no
                // deriver"; an empty value means the same thing, and
                // treating it as absent keeps `parse` consistent with
                // `path_info::{from,to}_nar_info` (which map empty <-> None).
                if !value.is_empty() && value != "unknown-deriver" {
                    deriver = Some(value.to_string());
                }
            }
            "System" => system = Some(value.to_string()),
            "Sig" => sigs.push(value.to_string()),
            "CA" => {
                if ca.is_some() {
                    return Err(make_input_err!("corrupt narinfo: extra CA line"));
                }
                if !value.is_empty() {
                    ca = Some(value.to_string());
                }
            }
            // Unknown field names are silently ignored, matching Nix.
            _ => {}
        }
    }

    let store_path =
        store_path.ok_or_else(|| make_input_err!("corrupt narinfo: StorePath missing"))?;
    let nar_hash = nar_hash.ok_or_else(|| make_input_err!("corrupt narinfo: NarHash missing"))?;
    let url = url
        .filter(|u| !u.is_empty())
        .ok_or_else(|| make_input_err!("corrupt narinfo: URL missing"))?;
    let nar_size = nar_size
        .filter(|&n| n != 0)
        .ok_or_else(|| make_input_err!("corrupt narinfo: NarSize missing or zero"))?;
    // An absent (or empty) Compression field means bzip2 to Nix.
    let compression = compression
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| "bzip2".to_string());

    Ok(NarInfo {
        store_path,
        url,
        compression,
        file_hash,
        file_size,
        nar_hash,
        nar_size,
        references: references.unwrap_or_default(),
        deriver,
        system,
        sigs,
        ca,
    })
}

/// Returns true if `s` is a store-path hash: exactly 32 characters, all
/// in the nix32 alphabet (the `<hash>` in `/nix/store/<hash>-<name>`).
#[must_use]
pub fn is_store_path_hash(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(nixbase32::is_valid_char)
}

/// Returns true if `name` is a valid store-path name component — the
/// `<name>` in `/nix/store/<hash>-<name>` and the tail of every
/// reference — per Nix's `checkName` (`src/libstore/path.cc`):
/// non-empty, at most 211 bytes, every byte in `[0-9a-zA-Z+._?=-]`, and
/// not starting with `.` (which would allow `.`/`..`).
///
/// This is stricter than a bare `/`-and-whitespace check: it rejects NUL
/// and every other control or out-of-charset byte, so a crafted narinfo
/// cannot smuggle them into a stored/served store path or reference.
#[must_use]
pub fn is_valid_store_path_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 211
        && !name.starts_with('.')
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.' | b'_' | b'?' | b'=')
        })
}

/// Computes the exact byte string Nix signs for a store path, per
/// `ValidPathInfo::fingerprint` in `src/libstore/path-info.cc` (lines
/// 43-50 of Nix 2.34, empirically verified against `cache.nixos.org`
/// signatures):
///
/// ```text
/// 1;<store_path>;sha256:<nix32(nar_hash)>;<nar_size>;<refs>
/// ```
///
/// where `<refs>` is the comma-joined list of *full* store paths of the
/// references, sorted byte-lexicographically. `references` holds
/// basenames; each is prefixed with the store directory derived from
/// `store_path` (everything before its final `/`). Empty references yield
/// an empty string after the final semicolon.
#[must_use]
pub fn fingerprint(
    store_path: &str,
    nar_hash: &[u8; 32],
    nar_size: u64,
    references: &[String],
) -> String {
    let store_dir = store_path.rfind('/').map_or("", |idx| &store_path[..idx]);
    let mut sorted: Vec<&str> = references.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    let mut joined = String::new();
    for (idx, reference) in sorted.iter().enumerate() {
        if idx > 0 {
            joined.push(',');
        }
        joined.push_str(store_dir);
        joined.push('/');
        joined.push_str(reference);
    }
    format!(
        "1;{store_path};sha256:{};{nar_size};{joined}",
        nixbase32::encode(nar_hash)
    )
}

impl NarInfo {
    /// Renders the document in Nix's serializer order, byte-compatible
    /// with `NarInfo::to_string` in `src/libstore/nar-info.cc` (plus the
    /// optional `System` field for attic compatibility).
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        let mut line = |name: &str, value: &str| {
            out.push_str(name);
            out.push_str(": ");
            out.push_str(value);
            out.push('\n');
        };
        line("StorePath", &self.store_path);
        line("URL", &self.url);
        line("Compression", &self.compression);
        if let Some(file_hash) = &self.file_hash {
            line(
                "FileHash",
                &format!("sha256:{}", nixbase32::encode(file_hash)),
            );
        }
        if let Some(file_size) = self.file_size {
            line("FileSize", &file_size.to_string());
        }
        line(
            "NarHash",
            &format!("sha256:{}", nixbase32::encode(&self.nar_hash)),
        );
        line("NarSize", &self.nar_size.to_string());
        // Always emitted; when empty this is "References: \n" with a
        // trailing space, exactly as Nix serializes it.
        line("References", &self.references.join(" "));
        if let Some(deriver) = &self.deriver {
            line("Deriver", deriver);
        }
        if let Some(system) = &self.system {
            line("System", system);
        }
        for sig in &self.sigs {
            line("Sig", sig);
        }
        if let Some(ca) = &self.ca {
            line("CA", ca);
        }
        out
    }

    /// Returns the 32-character nix32 hash prefix of the store path's
    /// basename (the `<hash>` in `/nix/store/<hash>-<name>`), validated.
    ///
    /// # Errors
    ///
    /// Returns an `InvalidArgument` error if the basename is shorter than
    /// 33 characters, is not followed by `-` after the hash, or contains
    /// characters outside the nix32 alphabet.
    pub fn store_path_hash(&self) -> Result<&str, Error> {
        let basename = self
            .store_path
            .rsplit('/')
            .next()
            .ok_or_else(|| make_input_err!("invalid store path '{}'", self.store_path))?;
        let (hash, rest) = basename.split_at_checked(32).ok_or_else(|| {
            make_input_err!(
                "store path basename '{basename}' is too short for a 32-character nix32 hash"
            )
        })?;
        if !rest.starts_with('-') {
            return Err(make_input_err!(
                "store path basename '{basename}' lacks a '-' after the 32-character hash"
            ));
        }
        if !hash.bytes().all(nixbase32::is_valid_char) {
            return Err(make_input_err!(
                "store path hash '{hash}' contains characters outside the nix32 alphabet"
            ));
        }
        Ok(hash)
    }

    /// The signing fingerprint of this document; see the free function
    /// [`fingerprint`](self::fingerprint).
    #[must_use]
    pub fn fingerprint(&self) -> String {
        fingerprint(
            &self.store_path,
            &self.nar_hash,
            self.nar_size,
            &self.references,
        )
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    use super::{NarInfo, fingerprint, is_store_path_hash, is_valid_store_path_name, parse};
    use crate::nixbase32;

    /// The real cache.nixos.org narinfo quoted at the top of
    /// straylight-attic's `server/src/narinfo/mod.rs`, from
    /// <https://cache.nixos.org/p4pclmv1gyja5kzc26npqpia1qqxrf0l.narinfo>.
    const RUBY_NARINFO: &str = concat!(
        "StorePath: /nix/store/p4pclmv1gyja5kzc26npqpia1qqxrf0l-ruby-2.7.3\n",
        "URL: nar/1w1fff338fvdw53sqgamddn1b2xgds473pv6y13gizdbqjv4i5p3.nar.xz\n",
        "Compression: xz\n",
        "FileHash: sha256:1w1fff338fvdw53sqgamddn1b2xgds473pv6y13gizdbqjv4i5p3\n",
        "FileSize: 4029176\n",
        "NarHash: sha256:1impfw8zdgisxkghq9a3q7cn7jb9zyzgxdydiamp8z2nlyyl0h5h\n",
        "NarSize: 18735072\n",
        "References: 0d71ygfwbmy1xjlbj1v027dfmy9cqavy-libffi-3.3",
        " 0dbbrvlw2rahvzi69bmpqy1z9mvzg62s-gdbm-1.19",
        " 0i6vphc3vnr8mg0gxjr61564hnp0s2md-gnugrep-3.6",
        " 0vkw1m51q34dr64z5i87dy99an4hfmyg-coreutils-8.32",
        " 64ylsrpd025kcyi608w3dqckzyz57mdc-libyaml-0.2.5",
        " 65ys3k6gn2s27apky0a0la7wryg3az9q-zlib-1.2.11",
        " 9m4hy7cy70w6v2rqjmhvd7ympqkj6yxk-ncurses-6.2",
        " a4yw1svqqk4d8lhwinn9xp847zz9gfma-bash-4.4-p23",
        " hbm0951q7xrl4qd0ccradp6bhjayfi4b-openssl-1.1.1k",
        " hjwjf3bj86gswmxva9k40nqx6jrb5qvl-readline-6.3p08",
        " p4pclmv1gyja5kzc26npqpia1qqxrf0l-ruby-2.7.3",
        " sbbifs2ykc05inws26203h0xwcadnf0l-glibc-2.32-46\n",
        "Deriver: bidkcs01mww363s4s7akdhbl6ws66b0z-ruby-2.7.3.drv\n",
        "Sig: cache.nixos.org-1:GrGV/Ls10TzoOaCnrcAqmPbKXFLLSBDeGNh5EQGKyuGA4K1wv1LcRVb6/",
        "sU+NAPK8lDiam8XcdJzUngmdhfTBQ==\n",
    );

    /// The narinfo from straylight-attic's `test_fingerprint` (its
    /// `server/src/narinfo/tests.rs`, around lines 89-127), with the
    /// `NarHash` in base16 form.
    const ATTIC_HELLO_NARINFO: &str = concat!(
        "StorePath: /nix/store/xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10\n",
        "URL: nar/0nqgf15qfiacfxrgm2wkw0gwwncjqqzzalj8rs14w9srkydkjsk9.nar.xz\n",
        "Compression: xz\n",
        "FileHash: sha256:0nqgf15qfiacfxrgm2wkw0gwwncjqqzzalj8rs14w9srkydkjsk9\n",
        "FileSize: 41104\n",
        "NarHash: sha256:91e129ac1959d062ad093d2b1f8b65afae0f712056fe3eac78ec530ff6a1bb9a\n",
        "NarSize: 206104\n",
        "References: 563528481rvhc5kxwipjmg6rqrl95mdx-glibc-2.33-56",
        " xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10\n",
        "Deriver: vvb4wxmnjixmrkhmj2xb75z62hrr41i7-hello-2.10.drv\n",
        "Sig: cache.nixos.org-1:lo9EfNIL4eGRuNh7DTbAAffWPpI2SlYC/8uP7JnhgmfRIUNGhSbFe8qEa",
        "KN0mFS02TuhPpXFPNtRkFcCp0hGAQ==\n",
    );

    /// Attic's expected fingerprint bytes for [`ATTIC_HELLO_NARINFO`]
    /// (its `server/src/narinfo/tests.rs` line 104).
    const ATTIC_HELLO_FINGERPRINT: &str = concat!(
        "1;/nix/store/xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10;",
        "sha256:16mvl7v0ylzcg2n3xzjn41qhzbmgcn5iyarx16nn5l2r36n2kqci;206104;",
        "/nix/store/563528481rvhc5kxwipjmg6rqrl95mdx-glibc-2.33-56,",
        "/nix/store/xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10"
    );

    /// A REAL narinfo generated with the local Nix CLI (nix 2.34.7):
    ///
    /// ```text
    /// $ nix copy --to "file:///tmp/nixnar-test-$$" \
    ///     /nix/store/bvkx110ylicifcgl0xiid5f100hx3ar7-nix-2.34.7
    /// $ cat /tmp/nixnar-test-*/bvkx110ylicifcgl0xiid5f100hx3ar7.narinfo
    /// ```
    ///
    /// The `Sig` line was propagated from the local store database, where
    /// it was recorded when the path was substituted from `cache.nixos.org`.
    const NIX_CLI_NARINFO: &str = concat!(
        "StorePath: /nix/store/bvkx110ylicifcgl0xiid5f100hx3ar7-nix-2.34.7\n",
        "URL: nar/11by0lzb61psglb8810552qb8v569x7y8q9gcj9p1dadpv9q0jqw.nar.xz\n",
        "Compression: xz\n",
        "FileHash: sha256:11by0lzb61psglb8810552qb8v569x7y8q9gcj9p1dadpv9q0jqw\n",
        "FileSize: 772\n",
        "NarHash: sha256:1ip47yiybcjh26ijapy13qzg8mzvypr2qci2ixx5jkm3b76r0bk9\n",
        "NarSize: 8848\n",
        "References: 97zxp9j00zcjmkn3zv9karhwj86q7x5w-nix-nswrapper-2.34.7",
        " hqwkw2nala59avjximpdmn1yi474n4h7-nix-2.34.7\n",
        "Deriver: q0hpd8s75g1h17yr8zqp1yf8sc9g4gp2-nix-2.34.7.drv\n",
        "Sig: cache.nixos.org-1:RPgjMEzWvBICkTl8Usg+ppxnHbBNqCtp5iIy5FFPZzxn2reRAb3ffPwpP",
        "zrARfK1NWO2g4nIQkfMyzsd2J0pAw==\n",
    );

    /// A REAL narinfo with EMPTY References, from the same `nix copy`
    /// invocation as [`NIX_CLI_NARINFO`]:
    ///
    /// ```text
    /// $ cat /tmp/nixnar-test-*/093bbkmklv9vznvdpg1dmdmirdf3acjg.narinfo
    /// ```
    ///
    /// Note the trailing space in the `References: ` line, confirmed on
    /// the wire with `od -c` (`R e f e r e n c e s :  space \n`).
    const EMPTY_REFS_NARINFO: &str = concat!(
        "StorePath: /nix/store/093bbkmklv9vznvdpg1dmdmirdf3acjg-publicsuffix-list-0-unstable-2026-03-26\n",
        "URL: nar/0gm1bi2h8dxy535wwjabybxb3bx3ckdn5sij105m0413g79nwr5q.nar.xz\n",
        "Compression: xz\n",
        "FileHash: sha256:0gm1bi2h8dxy535wwjabybxb3bx3ckdn5sij105m0413g79nwr5q\n",
        "FileSize: 75868\n",
        "NarHash: sha256:0r997jpgvk679xm8jin667j0ah50hfb251gpzs7bad1195dz7s4w\n",
        "NarSize: 336128\n",
        "References: \n",
        "Deriver: abcxfh2spvzd9smisi40sabzhcy5rlr8-publicsuffix-list-0-unstable-2026-03-26.drv\n",
        "Sig: cache.nixos.org-1:jqPehuLtEKxWEB9qoicMvBcLhGK+OzxmDVGz5dvJneOFwJeSSv1izNdHB",
        "ic49w65/qLLKmh/deWAbf4N1cnBBw==\n",
    );

    /// Verifies a `cache.nixos.org-1` signature over our computed
    /// fingerprint, using the well-known public key from
    /// <https://nixos.org> — an end-to-end check that both the parser
    /// and the fingerprint are byte-exact.
    fn verify_cache_nixos_org_sig(info: &NarInfo) {
        const KEY_B64: &str = "6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=";
        let key_bytes: [u8; 32] = BASE64
            .decode(KEY_B64)
            .expect("valid base64 key")
            .try_into()
            .expect("32-byte key");
        let key = VerifyingKey::from_bytes(&key_bytes).expect("valid ed25519 key");
        let sig_b64 = info
            .sigs
            .iter()
            .find_map(|s| s.strip_prefix("cache.nixos.org-1:"))
            .expect("cache.nixos.org signature present");
        let sig_bytes: [u8; 64] = BASE64
            .decode(sig_b64)
            .expect("valid base64 signature")
            .try_into()
            .expect("64-byte signature");
        let signature = Signature::from_bytes(&sig_bytes);
        key.verify(info.fingerprint().as_bytes(), &signature)
            .expect("signature verifies over computed fingerprint");
    }

    #[test]
    fn parses_cache_nixos_org_ruby_narinfo() {
        let info = parse(RUBY_NARINFO).expect("parse");
        assert_eq!(
            info.store_path,
            "/nix/store/p4pclmv1gyja5kzc26npqpia1qqxrf0l-ruby-2.7.3"
        );
        assert_eq!(
            info.url,
            "nar/1w1fff338fvdw53sqgamddn1b2xgds473pv6y13gizdbqjv4i5p3.nar.xz"
        );
        assert_eq!(info.compression, "xz");
        assert_eq!(
            nixbase32::encode(&info.file_hash.expect("file hash")),
            "1w1fff338fvdw53sqgamddn1b2xgds473pv6y13gizdbqjv4i5p3"
        );
        assert_eq!(info.file_size, Some(4_029_176));
        assert_eq!(
            nixbase32::encode(&info.nar_hash),
            "1impfw8zdgisxkghq9a3q7cn7jb9zyzgxdydiamp8z2nlyyl0h5h"
        );
        assert_eq!(info.nar_size, 18_735_072);
        assert_eq!(info.references.len(), 12);
        assert_eq!(
            info.references[0],
            "0d71ygfwbmy1xjlbj1v027dfmy9cqavy-libffi-3.3"
        );
        assert_eq!(
            info.references[11],
            "sbbifs2ykc05inws26203h0xwcadnf0l-glibc-2.32-46"
        );
        assert_eq!(
            info.deriver.as_deref(),
            Some("bidkcs01mww363s4s7akdhbl6ws66b0z-ruby-2.7.3.drv")
        );
        assert_eq!(info.system, None);
        assert_eq!(info.sigs.len(), 1);
        assert_eq!(info.ca, None);
        assert_eq!(
            info.store_path_hash().expect("store path hash"),
            "p4pclmv1gyja5kzc26npqpia1qqxrf0l"
        );
        verify_cache_nixos_org_sig(&info);
    }

    #[test]
    fn attic_fingerprint_vector() {
        let info = parse(ATTIC_HELLO_NARINFO).expect("parse");
        assert_eq!(info.fingerprint(), ATTIC_HELLO_FINGERPRINT);
        // The same fingerprint must come out of the free function, and
        // reference order on input must not matter (byte-lexicographic
        // sort before joining).
        let mut reversed = info.references.clone();
        reversed.reverse();
        assert_eq!(
            fingerprint(&info.store_path, &info.nar_hash, info.nar_size, &reversed),
            ATTIC_HELLO_FINGERPRINT
        );
        verify_cache_nixos_org_sig(&info);
    }

    #[test]
    fn parse_render_parse_round_trip_is_stable() {
        for text in [
            RUBY_NARINFO,
            ATTIC_HELLO_NARINFO,
            NIX_CLI_NARINFO,
            EMPTY_REFS_NARINFO,
        ] {
            let first = parse(text).expect("first parse");
            let rendered = first.render();
            let second = parse(&rendered).expect("second parse");
            assert_eq!(first, second);
            assert_eq!(second.render(), rendered);
        }
        // These captures are already in Nix's serializer order with
        // nix32 hashes, so rendering reproduces them byte-for-byte.
        for text in [RUBY_NARINFO, NIX_CLI_NARINFO, EMPTY_REFS_NARINFO] {
            assert_eq!(parse(text).expect("parse").render(), text);
        }
    }

    #[test]
    fn real_nix_cli_narinfo_field_expectations() {
        let info = parse(NIX_CLI_NARINFO).expect("parse");
        assert_eq!(
            info.store_path,
            "/nix/store/bvkx110ylicifcgl0xiid5f100hx3ar7-nix-2.34.7"
        );
        assert_eq!(
            info.url,
            "nar/11by0lzb61psglb8810552qb8v569x7y8q9gcj9p1dadpv9q0jqw.nar.xz"
        );
        assert_eq!(info.compression, "xz");
        assert_eq!(
            nixbase32::encode(&info.file_hash.expect("file hash")),
            "11by0lzb61psglb8810552qb8v569x7y8q9gcj9p1dadpv9q0jqw"
        );
        assert_eq!(info.file_size, Some(772));
        // $ nix hash convert --hash-algo sha256 --from nix32 --to base16 \
        //     1ip47yiybcjh26ijapy13qzg8mzvypr2qci2ixx5jkm3b76r0bk9
        // 692e90cd59a34e597a8f22322cf2f5fb57f43e1ec15f25a31150b2e5a33fe4c6
        assert_eq!(
            hex::encode(info.nar_hash),
            "692e90cd59a34e597a8f22322cf2f5fb57f43e1ec15f25a31150b2e5a33fe4c6"
        );
        assert_eq!(info.nar_size, 8848);
        assert_eq!(
            info.references,
            vec![
                "97zxp9j00zcjmkn3zv9karhwj86q7x5w-nix-nswrapper-2.34.7".to_string(),
                "hqwkw2nala59avjximpdmn1yi474n4h7-nix-2.34.7".to_string(),
            ]
        );
        assert_eq!(
            info.deriver.as_deref(),
            Some("q0hpd8s75g1h17yr8zqp1yf8sc9g4gp2-nix-2.34.7.drv")
        );
        assert_eq!(info.system, None);
        assert_eq!(info.sigs.len(), 1);
        assert_eq!(info.ca, None);
        assert_eq!(
            info.store_path_hash().expect("store path hash"),
            "bvkx110ylicifcgl0xiid5f100hx3ar7"
        );
        verify_cache_nixos_org_sig(&info);
    }

    #[test]
    fn empty_references_renders_with_trailing_space() {
        let info = parse(EMPTY_REFS_NARINFO).expect("parse");
        assert!(info.references.is_empty());
        let rendered = info.render();
        assert!(
            rendered.contains("\nReferences: \n"),
            "expected trailing space on empty References line in:\n{rendered}"
        );
        // Empty references => empty string after the fingerprint's final
        // semicolon. The real cache.nixos.org signature over this
        // fingerprint proves the trailing-';' form is what Nix signs.
        assert!(info.fingerprint().ends_with(";336128;"));
        verify_cache_nixos_org_sig(&info);
    }

    #[test]
    fn sri_and_nix32_nar_hash_parse_equivalence() {
        // All four accepted encodings of the same sha256 digest,
        // generated with the local Nix CLI (nix 2.34.7):
        //   $ nix hash convert --hash-algo sha256 --to nix32 \
        //       91e129ac1959d062ad093d2b1f8b65afae0f712056fe3eac78ec530ff6a1bb9a
        //   16mvl7v0ylzcg2n3xzjn41qhzbmgcn5iyarx16nn5l2r36n2kqci
        //   $ nix hash convert --hash-algo sha256 --to base64 \
        //       91e129ac1959d062ad093d2b1f8b65afae0f712056fe3eac78ec530ff6a1bb9a
        //   keEprBlZ0GKtCT0rH4tlr64PcSBW/j6seOxTD/ahu5o=
        //   $ nix hash convert --hash-algo sha256 --to sri \
        //       91e129ac1959d062ad093d2b1f8b65afae0f712056fe3eac78ec530ff6a1bb9a
        //   sha256-keEprBlZ0GKtCT0rH4tlr64PcSBW/j6seOxTD/ahu5o=
        let forms = [
            "sha256:16mvl7v0ylzcg2n3xzjn41qhzbmgcn5iyarx16nn5l2r36n2kqci",
            "sha256:91e129ac1959d062ad093d2b1f8b65afae0f712056fe3eac78ec530ff6a1bb9a",
            "sha256:keEprBlZ0GKtCT0rH4tlr64PcSBW/j6seOxTD/ahu5o=",
            "sha256-keEprBlZ0GKtCT0rH4tlr64PcSBW/j6seOxTD/ahu5o=",
        ];
        let expected =
            hex::decode("91e129ac1959d062ad093d2b1f8b65afae0f712056fe3eac78ec530ff6a1bb9a")
                .expect("valid hex");
        for form in forms {
            let text = format!(
                "StorePath: /nix/store/xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10\n\
                 URL: nar/x.nar.xz\n\
                 Compression: xz\n\
                 NarHash: {form}\n\
                 NarSize: 206104\n\
                 References: \n"
            );
            let info = parse(&text).expect("parse");
            assert_eq!(info.nar_hash.as_slice(), expected, "for form {form}");
        }
    }

    #[test]
    fn rejects_non_sha256_hash_algorithms() {
        for form in [
            "sha1:9m1skbnr5i43n3yypvda5s65vhfwdx5a",
            "md5:5d41402abc4b2a76b9719d911017c592",
            "sha512-MJ7MSJwS1utMxA9QyQLytNDtd+5RGnx6m808qG1M2G+YndNbxf9JlnDaNCVbRbDP2DDoH2Bdz33FVC6TrpzXbw==",
            "16mvl7v0ylzcg2n3xzjn41qhzbmgcn5iyarx16nn5l2r36n2kqci",
        ] {
            let text = format!(
                "StorePath: /nix/store/xcp9cav49dmsjbwdjlmkjxj10gkpx553-hello-2.10\n\
                 URL: nar/x.nar.xz\n\
                 Compression: xz\n\
                 NarHash: {form}\n\
                 NarSize: 206104\n\
                 References: \n"
            );
            assert!(parse(&text).is_err(), "expected rejection of '{form}'");
        }
    }

    #[test]
    fn multi_sig_accumulates_in_order() {
        let text = format!(
            "{NIX_CLI_NARINFO}Sig: example.org-1:aGVsbG8gd29ybGQK\nSig: example.org-2:Zm9vYmFyCg==\n"
        );
        let info = parse(&text).expect("parse");
        assert_eq!(info.sigs.len(), 3);
        assert!(info.sigs[0].starts_with("cache.nixos.org-1:"));
        assert_eq!(info.sigs[1], "example.org-1:aGVsbG8gd29ybGQK");
        assert_eq!(info.sigs[2], "example.org-2:Zm9vYmFyCg==");
        // Rendering emits one Sig line per signature, in order.
        let rendered = info.render();
        assert!(
            rendered.ends_with(
                "Sig: example.org-1:aGVsbG8gd29ybGQK\nSig: example.org-2:Zm9vYmFyCg==\n"
            )
        );
    }

    #[test]
    fn duplicate_references_and_ca_are_rejected() {
        let dup_refs = format!("{EMPTY_REFS_NARINFO}References: \n");
        assert!(parse(&dup_refs).is_err());
        let one_ca = format!(
            "{EMPTY_REFS_NARINFO}CA: fixed:sha256:0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73\n"
        );
        let info = parse(&one_ca).expect("parse");
        assert_eq!(
            info.ca.as_deref(),
            Some("fixed:sha256:0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73")
        );
        let dup_ca = format!(
            "{one_ca}CA: text:sha256:0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73\n"
        );
        assert!(parse(&dup_ca).is_err());
    }

    #[test]
    fn unknown_deriver_parses_to_none_and_is_never_rendered() {
        let text = EMPTY_REFS_NARINFO.replace(
            "Deriver: abcxfh2spvzd9smisi40sabzhcy5rlr8-publicsuffix-list-0-unstable-2026-03-26.drv",
            "Deriver: unknown-deriver",
        );
        let info = parse(&text).expect("parse");
        assert_eq!(info.deriver, None);
        assert!(!info.render().contains("Deriver"));
    }

    #[test]
    fn unknown_fields_are_silently_ignored() {
        let text = format!("{NIX_CLI_NARINFO}Frobnicator: yes\nX-Custom: value\n");
        let info = parse(&text).expect("parse");
        assert_eq!(info, parse(NIX_CLI_NARINFO).expect("parse"));
    }

    #[test]
    fn missing_required_fields_are_corrupt() {
        let remove_line = |field: &str| -> String {
            NIX_CLI_NARINFO
                .split_inclusive('\n')
                .filter(|line| !line.starts_with(field))
                .collect()
        };
        for field in ["StorePath", "URL", "NarHash", "NarSize"] {
            let err = parse(&remove_line(field)).expect_err(field);
            assert!(
                err.to_string().contains("corrupt"),
                "expected corrupt error for missing {field}, got: {err}"
            );
        }
        // NarSize of zero is as corrupt as a missing one.
        let zero_size = NIX_CLI_NARINFO.replace("NarSize: 8848", "NarSize: 0");
        assert!(parse(&zero_size).is_err());
    }

    #[test]
    fn missing_compression_means_bzip2() {
        let text = NIX_CLI_NARINFO
            .split_inclusive('\n')
            .filter(|line| !line.starts_with("Compression"))
            .collect::<String>();
        let info = parse(&text).expect("parse");
        assert_eq!(info.compression, "bzip2");
        // None must never render as absent: the field is always emitted.
        assert!(info.render().contains("\nCompression: bzip2\n"));
    }

    /// Cross-module end-to-end test over a REAL nix-produced artifact,
    /// tying `narinfo`, `nixbase32`, and `signing` together.
    ///
    /// Fixture generation (nix (Nix) 2.34.7, 2026-07-02):
    ///
    /// ```text
    /// $ nix key generate-secret --key-name nl-test-1 > /tmp/nl-sk
    /// $ cat /tmp/nl-sk       # the secret key hardcoded below
    /// $ nix copy --to "file:///tmp/nl-sig-test?secret-key=/tmp/nl-sk" \
    ///     /nix/store/lw117lsr8d585xs63kx5k233impyrq7q-bash-5.3p3
    /// $ cat /tmp/nl-sig-test/lw117lsr8d585xs63kx5k233impyrq7q.narinfo
    /// ```
    ///
    /// The narinfo below is that file, byte-for-byte. The same fixture
    /// backs the `signing` module's golden vectors; here the fingerprint
    /// is COMPUTED by [`NarInfo::fingerprint`] rather than constructed by
    /// hand, so the two modules cross-check each other.
    #[test]
    fn end_to_end_nix_produced_narinfo_signature() {
        use crate::signing::{NixPublicKey, NixSigningKey};

        const SECRET_KEY: &str = "nl-test-1:JopCSwJGgB13rkPagDgcbo9/UOQluENPBbkpBHWwt/ErsdQpil4SypWcBm2XcowcywRpwwqBsv+BdkaBMKxm3g==";
        const BASH_NARINFO: &str = concat!(
            "StorePath: /nix/store/lw117lsr8d585xs63kx5k233impyrq7q-bash-5.3p3\n",
            "URL: nar/14hirjxgs38nvvfpqdzd2z7ilvnyhx3g2md4vcrfh383cfzxfnp3.nar.xz\n",
            "Compression: xz\n",
            "FileHash: sha256:14hirjxgs38nvvfpqdzd2z7ilvnyhx3g2md4vcrfh383cfzxfnp3\n",
            "FileSize: 502428\n",
            "NarHash: sha256:1b89r1vlfiv6immkhq8aqxhy1jrzh2araqsn6rvwhrjgpy3pd52h\n",
            "NarSize: 1856888\n",
            "References: j193mfi0f921y0kfs8vjc1znnr45ispv-glibc-2.40-66",
            " lw117lsr8d585xs63kx5k233impyrq7q-bash-5.3p3\n",
            "Deriver: i0lswaixfnfr6j3qr9xrij8nq93rp9b5-bash-5.3p3.drv\n",
            "Sig: cache.nixos.org-1:jY7ctGkzGM+K70V4BKcxhSwmSq3xr8zXETZ7z7wupsHjxdjbF",
            "xGZ4idhDD6Q6hxDol48Hxk7za5O4A0Salt8CA==\n",
            "Sig: nl-test-1:fVj4XST/j2Yz1xgPY0pjDjmGMUbMjVRC84PzQJKNfOWth+poDayvMYGa1",
            "LX+JlY+TcT0Gdk6U4RCk50sEduTBA==\n",
        );

        // Parse the nix-written narinfo.
        let info = parse(BASH_NARINFO).expect("parse nix-produced narinfo");
        assert_eq!(
            info.store_path_hash().expect("store path hash"),
            "lw117lsr8d585xs63kx5k233impyrq7q"
        );

        // Derive the public key from the secret key, exactly as
        // `nix key convert-secret-to-public` would.
        let signing_key = NixSigningKey::from_secret_string(SECRET_KEY).expect("secret key");
        let public_key =
            NixPublicKey::from_string(&signing_key.public_key_string()).expect("public key");
        assert_eq!(public_key.name(), "nl-test-1");

        // The fingerprint computed by this module verifies against the
        // Sig line nix wrote with our key...
        let fingerprint = info.fingerprint();
        let our_sig = info
            .sigs
            .iter()
            .find(|s| s.starts_with("nl-test-1:"))
            .expect("nl-test-1 signature present");
        assert!(public_key.verify(&fingerprint, our_sig));
        // ...and signing it ourselves reproduces nix's Sig line
        // byte-identically (ed25519 is deterministic).
        assert_eq!(&signing_key.sign(&fingerprint), our_sig);
        // The upstream cache.nixos.org signature (preserved by nix copy)
        // verifies over the same computed fingerprint.
        let upstream = NixPublicKey::from_string(
            "cache.nixos.org-1:6NCHdD59X431o0gWypbMrAURkbJ16ZPMQFGspcDShjY=",
        )
        .expect("well-known key");
        let upstream_sig = info
            .sigs
            .iter()
            .find(|s| s.starts_with("cache.nixos.org-1:"))
            .expect("cache.nixos.org signature present");
        assert!(upstream.verify(&fingerprint, upstream_sig));
        // A mutated fingerprint must not verify.
        assert!(!public_key.verify(&format!("{fingerprint};"), our_sig));

        // Render/parse stability: the capture is in Nix's serializer
        // order, so rendering reproduces the wire bytes exactly, and
        // re-parsing is a fixed point.
        let rendered = info.render();
        assert_eq!(rendered, BASH_NARINFO);
        let reparsed = parse(&rendered).expect("re-parse");
        assert_eq!(reparsed, info);
        assert_eq!(reparsed.fingerprint(), fingerprint);
    }

    #[test]
    fn is_store_path_hash_checks_length_and_alphabet() {
        assert!(is_store_path_hash("bvkx110ylicifcgl0xiid5f100hx3ar7"));
        assert!(is_store_path_hash("p4pclmv1gyja5kzc26npqpia1qqxrf0l"));
        assert!(is_store_path_hash("00000000000000000000000000000000"));
        // Wrong lengths: 31 and 33 characters.
        assert!(!is_store_path_hash("bvkx110ylicifcgl0xiid5f100hx3ar"));
        assert!(!is_store_path_hash("bvkx110ylicifcgl0xiid5f100hx3ar77"));
        assert!(!is_store_path_hash(""));
        // 'e', 'o', 'u', 't' and uppercase are outside the alphabet.
        assert!(!is_store_path_hash("evkx110ylicifcgl0xiid5f100hx3ar7"));
        assert!(!is_store_path_hash("BVKX110YLICIFCGL0XIID5F100HX3AR7"));
        // Multi-byte characters must not pass on char count alone.
        assert!(!is_store_path_hash("bvkx110ylicifcgl0xiid5f100hx3aré"));
    }

    #[test]
    fn is_valid_store_path_name_matches_nix_check_name() {
        for good in [
            "nix-2.34.7",
            "hello-2.10",
            "publicsuffix-list-0-unstable-2026-03-26",
            "glibc-2.40-66",
            "app-3.4.drv",
            "a",
            "with+plus_and?query=eq",
        ] {
            assert!(is_valid_store_path_name(good), "for '{good}'");
        }
        for bad in [
            "",            // empty
            ".hidden",     // leading dot
            "..",          // leading dot (and traversal)
            "na\0me",      // NUL byte
            "has space",   // whitespace
            "has/slash",   // path separator
            "tab\tinside", // control byte
            "star*",       // out-of-charset
            "unïcode",     // non-ascii
        ] {
            assert!(!is_valid_store_path_name(bad), "for '{bad}'");
        }
        // 211 bytes is allowed; 212 is not.
        assert!(is_valid_store_path_name(&"a".repeat(211)));
        assert!(!is_valid_store_path_name(&"a".repeat(212)));
    }

    #[test]
    fn store_path_hash_validates() {
        let mut info = parse(NIX_CLI_NARINFO).expect("parse");
        info.store_path = "/nix/store/short-x".to_string();
        assert!(info.store_path_hash().is_err());
        // 'e' is outside the nix32 alphabet.
        info.store_path = "/nix/store/eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee-x".to_string();
        assert!(info.store_path_hash().is_err());
        // No '-' after the 32-character hash.
        info.store_path = "/nix/store/bvkx110ylicifcgl0xiid5f100hx3ar7nix".to_string();
        assert!(info.store_path_hash().is_err());
        info.store_path = "/nix/store/bvkx110ylicifcgl0xiid5f100hx3ar7-nix-2.34.7".to_string();
        assert_eq!(
            info.store_path_hash().expect("valid"),
            "bvkx110ylicifcgl0xiid5f100hx3ar7"
        );
    }
}

#[cfg(test)]
mod proptests {
    use proptest::prelude::*;

    use super::{NarInfo, fingerprint, parse};

    /// A store path with a real 32-char nix32 hash and a safe name.
    fn store_path_strategy() -> impl Strategy<Value = String> {
        (
            proptest::collection::vec(0u8..32, 32),
            "[0-9a-zA-Z+._?=-]{1,20}",
        )
            .prop_map(|(hash_digits, name)| {
                let hash: String = hash_digits
                    .into_iter()
                    .map(|d| char::from(b"0123456789abcdfghijklmnpqrsvwxyz"[usize::from(d)]))
                    .collect();
                // Keep the name Nix-legal (no leading '.') so the store path
                // survives the path_info validators used elsewhere.
                format!("/nix/store/{hash}-n{name}")
            })
    }

    /// A field value with no line-structure bytes: safe to place after
    /// `Name: ` and render/parse back verbatim. Never empty.
    fn safe_value() -> impl Strategy<Value = String> {
        "[!-~ ]{1,40}".prop_filter("no CR/LF", |s| !s.contains(['\n', '\r']))
    }

    /// A reference basename: nonempty and free of the space delimiter and
    /// line bytes, so `References` round-trips exactly.
    fn reference_strategy() -> impl Strategy<Value = String> {
        "[!-~]{1,30}".prop_filter("no space/CR/LF", |s| !s.contains([' ', '\n', '\r']))
    }

    /// A verbatim `name:base64` signature string free of line bytes.
    fn sig_strategy() -> impl Strategy<Value = String> {
        "[a-z0-9.-]{1,12}:[A-Za-z0-9+/=]{1,90}"
            .prop_filter("no CR/LF", |s| !s.contains(['\n', '\r']))
    }

    /// Builds a structurally-valid, render/parse-stable [`NarInfo`]. Every
    /// invariant that `parse` normalizes on is pre-satisfied so that
    /// `parse(render(x)) == x`:
    /// - `url`/`compression` nonempty (empty would be re-defaulted),
    /// - `nar_size` nonzero (zero is corrupt),
    /// - references nonempty and space-free (the split delimiter),
    /// - a `Some` `deriver` is nonempty and not `unknown-deriver`,
    /// - a `Some` `ca` is nonempty,
    /// - no field value carries a `\n`/`\r`.
    fn narinfo_strategy() -> impl Strategy<Value = NarInfo> {
        (
            store_path_strategy(),
            safe_value(),                            // url
            "[a-z0-9]{1,8}",                         // compression
            proptest::option::of(any::<[u8; 32]>()), // file_hash
            proptest::option::of(1u64..=u64::MAX),   // file_size
            any::<[u8; 32]>(),                       // nar_hash
            1u64..=u64::MAX,                         // nar_size
            proptest::collection::vec(reference_strategy(), 0..6),
            proptest::option::of(
                "[!-~]{1,20}".prop_filter("nonempty, not the sentinel, no CR/LF", |s: &String| {
                    s != "unknown-deriver" && !s.contains(['\n', '\r'])
                }),
            ),
            proptest::option::of(
                "[!-~ ]{1,15}".prop_filter("no CR/LF", |s: &String| !s.contains(['\n', '\r'])),
            ),
            proptest::collection::vec(sig_strategy(), 0..4),
            proptest::option::of(safe_value()),
        )
            .prop_map(
                |(
                    store_path,
                    url,
                    compression,
                    file_hash,
                    file_size,
                    nar_hash,
                    nar_size,
                    references,
                    deriver,
                    system,
                    sigs,
                    ca,
                )| NarInfo {
                    store_path,
                    url,
                    compression,
                    file_hash,
                    file_size,
                    nar_hash,
                    nar_size,
                    references,
                    deriver,
                    system,
                    sigs,
                    ca,
                },
            )
    }

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 2048,
            failure_persistence: Some(Box::new(
                proptest::test_runner::FileFailurePersistence::Off,
            )),
            ..ProptestConfig::default()
        })]

        /// THE key fuzz property: `parse` never panics on arbitrary UTF-8.
        #[test]
        fn parse_never_panics_on_arbitrary_string(s in ".*") {
            // Determinism doubles as a no-panic assertion (both calls run).
            prop_assert_eq!(parse(&s).is_ok(), parse(&s).is_ok());
        }

        /// `parse` never panics on arbitrary bytes viewed as a lossy string.
        #[test]
        fn parse_never_panics_on_lossy_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..=256)) {
            let s = String::from_utf8_lossy(&bytes);
            // Determinism doubles as a no-panic assertion (both calls run).
            prop_assert_eq!(parse(&s).is_ok(), parse(&s).is_ok());
        }

        /// `parse` never panics on a structured-but-mutated narinfo: take a
        /// rendered valid document, flip/insert bytes, and parse.
        #[test]
        fn parse_never_panics_on_mutated_narinfo(
            info in narinfo_strategy(),
            byte in any::<u8>(),
            pos in any::<prop::sample::Index>(),
            insert in any::<bool>(),
        ) {
            let mut bytes = info.render().into_bytes();
            if bytes.is_empty() {
                return Ok(());
            }
            let idx = pos.index(bytes.len());
            if insert {
                bytes.insert(idx, byte);
            } else {
                bytes[idx] = byte;
            }
            let s = String::from_utf8_lossy(&bytes);
            // Determinism doubles as a no-panic assertion (both calls run).
            prop_assert_eq!(parse(&s).is_ok(), parse(&s).is_ok());
        }

        /// Idempotent round-trip: a structurally-valid narinfo parses back
        /// to itself, and re-rendering is a fixed point.
        #[test]
        fn render_parse_round_trips(info in narinfo_strategy()) {
            let rendered = info.render();
            let reparsed = parse(&rendered).expect("valid narinfo must parse");
            prop_assert_eq!(&reparsed, &info);
            prop_assert_eq!(reparsed.render(), rendered);
        }

        /// `parse(render(parse(t)?)) == parse(t)?` for any text that parses:
        /// parsing is idempotent through a render.
        #[test]
        fn parse_render_parse_is_idempotent(s in ".*") {
            if let Ok(first) = parse(&s) {
                let second = parse(&first.render()).expect("re-parse of rendered narinfo");
                prop_assert_eq!(second, first);
            }
        }

        /// The fingerprint is invariant under reference reordering (it sorts
        /// byte-lexicographically before joining).
        #[test]
        fn fingerprint_invariant_under_reference_reordering(
            info in narinfo_strategy(),
            seed in any::<u64>(),
        ) {
            let base = info.fingerprint();
            // A deterministic Fisher-Yates shuffle of the references, so the
            // fingerprint must be invariant across every reachable ordering.
            let mut shuffled = info.references.clone();
            let mut state = seed | 1;
            for i in (1..shuffled.len()).rev() {
                // SplitMix64-style step; the exact PRNG is irrelevant, only
                // that it reaches many permutations across proptest cases.
                state = state.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
                let bound = u64::try_from(i).expect("len fits u64") + 1;
                let j = usize::try_from(state % bound).expect("bounded index");
                shuffled.swap(i, j);
            }
            let reordered = fingerprint(
                &info.store_path,
                &info.nar_hash,
                info.nar_size,
                &shuffled,
            );
            prop_assert_eq!(reordered, base);
        }

        /// The fingerprint is invariant under CRLF-vs-LF line endings: a
        /// document served with CRLF parses to the same fingerprint as its
        /// LF form.
        #[test]
        fn fingerprint_invariant_under_crlf(info in narinfo_strategy()) {
            let lf = info.render();
            let crlf = lf.replace('\n', "\r\n");
            let from_lf = parse(&lf).expect("LF parse");
            let from_crlf = parse(&crlf).expect("CRLF parse");
            prop_assert_eq!(from_lf.fingerprint(), from_crlf.fingerprint());
            // And the whole document is identical after CRLF stripping.
            prop_assert_eq!(from_lf, from_crlf);
        }

        /// Field-value INJECTION defense: a `\n`/`\r` smuggled into a field
        /// value cannot survive a parse round trip to forge a later field.
        ///
        /// `render` does not escape, so a hand-built `NarInfo` whose `url`
        /// contains `\nSig: forged...` DOES render a line a naive reader
        /// would treat as a signature. The invariant that makes this safe
        /// in practice is that a `NarInfo` produced by `parse` can never
        /// carry a line-structure byte in any field (parse splits on
        /// `\n` and strips `\r`). We assert exactly that: whatever a
        /// forged, re-parsed document yields, none of its parsed fields
        /// contains `\n` or `\r`, so a THIRD parse observes no new field
        /// and the round trip is a fixed point (no forged field is gained
        /// or lost on further passes).
        #[test]
        fn newline_injection_cannot_survive_a_parse_round_trip(mut info in narinfo_strategy()) {
            info.url = format!("{}\nSig: forged-key:QUJDRA==", info.url);
            let rendered = info.render();
            if let Ok(parsed) = parse(&rendered) {
                // No field a parse produced may carry a line byte.
                let has_line_byte = |s: &str| s.contains(['\n', '\r']);
                prop_assert!(!has_line_byte(&parsed.store_path));
                prop_assert!(!has_line_byte(&parsed.url));
                prop_assert!(!has_line_byte(&parsed.compression));
                prop_assert!(parsed.references.iter().all(|r| !has_line_byte(r)));
                prop_assert!(parsed.sigs.iter().all(|s| !has_line_byte(s)));
                prop_assert!(parsed.deriver.as_deref().is_none_or(|d| !has_line_byte(d)));
                prop_assert!(parsed.system.as_deref().is_none_or(|s| !has_line_byte(s)));
                prop_assert!(parsed.ca.as_deref().is_none_or(|c| !has_line_byte(c)));
                // A parse produced from parse output is a fixed point: no
                // forged field materializes or vanishes on the next pass.
                let reparsed = parse(&parsed.render()).expect("re-parse");
                prop_assert_eq!(reparsed, parsed);
            }
        }

        /// Strict integers: a `FileSize`/`NarSize` value with a leading `+`
        /// or leading zeros is rejected, even though Rust's `parse::<u64>`
        /// would accept the former's leading zeros.
        #[test]
        fn strict_integer_fields_reject_leading_plus_and_zeros(
            info in narinfo_strategy(),
            digits in "[0-9]{1,6}",
            which in 0u8..3,
        ) {
            let lf = info.render();
            // Build a non-canonical size token.
            let bad = match which {
                0 => format!("0{digits}"),   // leading zero
                1 => format!("+{digits}"),   // leading plus
                _ => format!("00{digits}"),  // multiple leading zeros
            };
            // NarSize is always present; replace its value.
            let nar_line = format!("NarSize: {}\n", info.nar_size);
            let bad_nar = format!("NarSize: {bad}\n");
            prop_assert!(lf.contains(&nar_line));
            let mutated = lf.replace(&nar_line, &bad_nar);
            prop_assert!(
                parse(&mutated).is_err(),
                "NarSize '{}' must be rejected", bad
            );
        }
    }
}
