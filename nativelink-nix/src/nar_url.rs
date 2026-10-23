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

//! Pure helpers for NAR URL names: compression-codec selection by file
//! extension, the canonical `{nix32(hash)}-{size}.nar` (and `.nar.zst`)
//! object names used for CAS-backed NARs, the alias-key scheme that maps
//! arbitrary upstream NAR URL basenames onto canonical names, and the
//! auxiliary-metadata keys for listings and build logs.

use nativelink_error::{Error, make_input_err};

use crate::nixbase32;

/// Compression codec of a NAR, as determined by its URL name (or, for
/// [`NarCodec::Gzip`], by content sniffing — see [`codec_for_name`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NarCodec {
    /// Uncompressed NAR bytes.
    None,
    /// gzip (only ever detected by sniffing [`GZIP_MAGIC`], never by name).
    Gzip,
    /// xz / LZMA2.
    Xz,
    /// Zstandard.
    Zstd,
    /// bzip2.
    Bzip2,
}

/// The first two bytes of any gzip stream (RFC 1952). Some caches serve
/// gzip-compressed bytes under a bare `.nar` name; callers that get
/// [`NarCodec::None`] from [`codec_for_name`] may sniff for this magic.
pub const GZIP_MAGIC: [u8; 2] = [0x1f, 0x8b];

/// Maps a NAR file name to its compression codec by extension.
///
/// - `.nar` maps to `Some(NarCodec::None)` — but the bytes may still be
///   gzip on the wire; the caller may sniff [`GZIP_MAGIC`].
/// - `.nar.xz`, `.nar.zst`, and `.nar.bz2` map to their codecs.
/// - Anything else (including `.nar.gz`, which real caches do not emit)
///   is unsupported and maps to `Option::None`.
#[must_use]
pub fn codec_for_name(name: &str) -> Option<NarCodec> {
    // Deliberately case-sensitive: NAR URL names are lowercase on the
    // wire, and `.NAR` is not a name any real cache emits.
    const SUFFIX_CODECS: [(&str, NarCodec); 4] = [
        (".nar.xz", NarCodec::Xz),
        (".nar.zst", NarCodec::Zstd),
        (".nar.bz2", NarCodec::Bzip2),
        (".nar", NarCodec::None),
    ];
    SUFFIX_CODECS
        .iter()
        .find(|(suffix, _)| name.ends_with(suffix))
        .map(|&(_, codec)| codec)
}

/// Formats the canonical name for an uncompressed NAR:
/// `{nix32(nar_sha256)}-{nar_size}.nar` (52 nix32 characters, `-`, the
/// decimal size, `.nar`).
#[must_use]
pub fn canonical_nar_name(nar_sha256: &[u8; 32], nar_size: u64) -> String {
    format!("{}-{nar_size}.nar", nixbase32::encode(nar_sha256))
}

/// Formats the canonical name for a zstd-compressed NAR:
/// `{nix32(file_sha256)}-{file_size}.nar.zst`, where `file_sha256` and
/// `file_size` describe the COMPRESSED bytes (matching the `file_*`
/// fields of [`crate::path_info::NixPathInfo`]).
#[must_use]
pub fn canonical_nar_zst_name(file_sha256: &[u8; 32], file_size: u64) -> String {
    format!("{}-{file_size}.nar.zst", nixbase32::encode(file_sha256))
}

/// Parses the `{nix32}-{size}` stem shared by [`canonical_nar_name`] and
/// [`canonical_nar_zst_name`].
///
/// Returns `None` unless `stem` is exactly 52 nix32 characters, `-`, and
/// a canonical decimal `u64` (no leading zeros, no sign, in range).
fn parse_canonical_stem(stem: &str) -> Option<([u8; 32], u64)> {
    let (hash_str, size_part) = stem.split_at_checked(52)?;
    let size_str = size_part.strip_prefix('-')?;
    if size_str.is_empty() || !size_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // Reject non-canonical decimals: "042" round-trips to "42".
    if size_str.len() > 1 && size_str.starts_with('0') {
        return None;
    }
    let size = size_str.parse::<u64>().ok()?;
    // `decode` also rejects nonzero padding bits in the leading
    // character, so accepted hashes re-encode to `hash_str` exactly.
    let sha256: [u8; 32] = nixbase32::decode(hash_str).ok()?.try_into().ok()?;
    Some((sha256, size))
}

/// Parses a name produced by [`canonical_nar_name`].
///
/// Returns `None` unless `name` is exactly 52 nix32 characters, `-`, a
/// canonical decimal `u64` (no leading zeros, no sign, in range), and
/// `.nar` — i.e. unless `canonical_nar_name` reproduces `name` exactly.
#[must_use]
pub fn parse_canonical_nar_name(name: &str) -> Option<([u8; 32], u64)> {
    parse_canonical_stem(name.strip_suffix(".nar")?)
}

/// Parses a canonical NAR name of either form: a
/// [`canonical_nar_name`] maps to [`NarCodec::None`] and a
/// [`canonical_nar_zst_name`] to [`NarCodec::Zstd`].
///
/// The stem is held to the same strictness as
/// [`parse_canonical_nar_name`]: exactly 52 nix32 characters, `-`, and a
/// canonical decimal `u64`. For `.nar` names the pair is
/// `(nar_sha256, nar_size)` of the uncompressed NAR; for `.nar.zst`
/// names it is `(file_sha256, file_size)` of the compressed bytes.
#[must_use]
pub fn parse_canonical_any(name: &str) -> Option<([u8; 32], u64, NarCodec)> {
    let (stem, codec) = if let Some(stem) = name.strip_suffix(".nar.zst") {
        (stem, NarCodec::Zstd)
    } else {
        (name.strip_suffix(".nar")?, NarCodec::None)
    };
    let (sha256, size) = parse_canonical_stem(stem)?;
    Some((sha256, size, codec))
}

/// Builds the store key under which the alias for an upstream NAR URL
/// basename lives: `nar-alias:{basename}`.
///
/// Only the final `/`-separated segment of `url_basename` is used, so the
/// returned key never contains a `/`.
#[must_use]
pub fn alias_key(url_basename: &str) -> String {
    let basename = url_basename.rsplit('/').next().unwrap_or(url_basename);
    format!("nar-alias:{basename}")
}

/// Formats the alias value for a NAR: `{lowercase hex(nar_sha256)}-{nar_size}`.
#[must_use]
pub fn format_alias(nar_sha256: &[u8; 32], nar_size: u64) -> String {
    format!("{}-{nar_size}", hex::encode(nar_sha256))
}

/// Parses an alias value produced by [`format_alias`].
///
/// # Errors
///
/// Returns an `InvalidArgument` error unless `s` is exactly 64 lowercase
/// hex characters, `-`, and a canonical decimal `u64` (no leading zeros,
/// no sign, in range).
pub fn parse_alias(s: &str) -> Result<([u8; 32], u64), Error> {
    let (hex_str, size_part) = s
        .split_at_checked(64)
        .ok_or_else(|| make_input_err!("nar alias '{s}' is shorter than a 64-character hash"))?;
    if !hex_str
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(make_input_err!(
            "nar alias hash '{hex_str}' is not lowercase hex"
        ));
    }
    let size_str = size_part.strip_prefix('-').ok_or_else(|| {
        make_input_err!("nar alias '{s}' lacks a '-' after the 64-character hash")
    })?;
    if size_str.is_empty() || !size_str.bytes().all(|b| b.is_ascii_digit()) {
        return Err(make_input_err!(
            "nar alias size '{size_str}' is not a decimal number"
        ));
    }
    if size_str.len() > 1 && size_str.starts_with('0') {
        return Err(make_input_err!(
            "nar alias size '{size_str}' has leading zeros"
        ));
    }
    let nar_size = size_str
        .parse::<u64>()
        .map_err(|e| make_input_err!("nar alias size '{size_str}' does not fit in u64: {e}"))?;
    let mut nar_sha256 = [0_u8; 32];
    hex::decode_to_slice(hex_str, &mut nar_sha256)
        .map_err(|e| make_input_err!("invalid hex in nar alias hash '{hex_str}': {e}"))?;
    Ok((nar_sha256, nar_size))
}

/// Builds the auxiliary-metadata store key under which the directory
/// listing (`.ls`) for a store path lives: `ls:{storePathHash}`, where
/// `store_path_hash` is the 32-character nix32 store-path hash.
///
/// Only the final `/`-separated segment of `store_path_hash` is used, so
/// the returned key never contains a `/`.
#[must_use]
pub fn listing_key(store_path_hash: &str) -> String {
    let hash = store_path_hash
        .rsplit('/')
        .next()
        .unwrap_or(store_path_hash);
    format!("ls:{hash}")
}

/// Builds the auxiliary-metadata store key under which the build log for
/// a derivation lives: `log:{drvBasename}`, where `drv_basename` is the
/// `.drv` store-path basename (as served under `log/{drvBasename}`).
///
/// Only the final `/`-separated segment of `drv_basename` is used, so
/// the returned key never contains a `/`.
#[must_use]
pub fn log_key(drv_basename: &str) -> String {
    let basename = drv_basename.rsplit('/').next().unwrap_or(drv_basename);
    format!("log:{basename}")
}

/// Builds the auxiliary-metadata store key under which the content
/// encoding of a stored build log lives: `log-enc:{drvBasename}`.
///
/// Only the final `/`-separated segment of `drv_basename` is used, so
/// the returned key never contains a `/`.
#[must_use]
pub fn log_encoding_key(drv_basename: &str) -> String {
    let basename = drv_basename.rsplit('/').next().unwrap_or(drv_basename);
    format!("log-enc:{basename}")
}

#[cfg(test)]
mod tests {
    use super::{
        GZIP_MAGIC, NarCodec, alias_key, canonical_nar_name, canonical_nar_zst_name,
        codec_for_name, format_alias, listing_key, log_encoding_key, log_key, parse_alias,
        parse_canonical_any, parse_canonical_nar_name,
    };

    /// sha256("hello"); its nix32 form is a golden vector from the
    /// `nixbase32` module (generated with nix 2.34.7).
    const HELLO_HEX: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    const HELLO_NIX32: &str = "094qif9n4cq4fdg459qzbhg1c6wywawwaaivx0k0x8xhbyx4vwic";

    fn hello_hash() -> [u8; 32] {
        let bytes = hex::decode(HELLO_HEX).expect("valid hex");
        bytes.try_into().expect("32 bytes")
    }

    #[test]
    fn codec_for_name_maps_known_extensions() {
        assert_eq!(codec_for_name("x.nar"), Some(NarCodec::None));
        assert_eq!(codec_for_name("x.nar.xz"), Some(NarCodec::Xz));
        assert_eq!(codec_for_name("x.nar.zst"), Some(NarCodec::Zstd));
        assert_eq!(codec_for_name("x.nar.bz2"), Some(NarCodec::Bzip2));
        // Full URL-ish names work too: only the suffix matters.
        assert_eq!(
            codec_for_name("nar/1w1fff338fvdw53sqgamddn1b2xgds473pv6y13gizdbqjv4i5p3.nar.xz"),
            Some(NarCodec::Xz)
        );
        assert_eq!(codec_for_name(".nar"), Some(NarCodec::None));
    }

    #[test]
    fn codec_for_name_rejects_everything_else() {
        // gzip is never selected by name — only by sniffing GZIP_MAGIC.
        for unsupported in [
            "x.nar.gz",
            "x.nar.br",
            "x.narinfo",
            "x.tar",
            "x.nar.xz.bak",
            "nar",
            "",
        ] {
            assert_eq!(codec_for_name(unsupported), None, "for '{unsupported}'");
        }
    }

    #[test]
    fn gzip_magic_is_rfc_1952() {
        assert_eq!(GZIP_MAGIC, [0x1f, 0x8b]);
    }

    #[test]
    fn canonical_nar_name_round_trips() {
        for size in [1_u64, 42, 206_104, u64::MAX] {
            let name = canonical_nar_name(&hello_hash(), size);
            assert_eq!(name, format!("{HELLO_NIX32}-{size}.nar"));
            assert_eq!(parse_canonical_nar_name(&name), Some((hello_hash(), size)));
        }
        // Zero is a canonical decimal with no leading zeros.
        assert_eq!(
            parse_canonical_nar_name(&canonical_nar_name(&hello_hash(), 0)),
            Some((hello_hash(), 0))
        );
    }

    #[test]
    fn parse_canonical_nar_name_rejects_malformed_names() {
        let hash51 = &HELLO_NIX32[..51];
        let hash53 = format!("{HELLO_NIX32}0");
        let bad_alphabet = format!("e{}", &HELLO_NIX32[1..]);
        for bad in [
            // 51- and 53-character hashes.
            format!("{hash51}-42.nar"),
            format!("{hash53}-42.nar"),
            // 'e' is outside the nix32 alphabet.
            format!("{bad_alphabet}-42.nar"),
            // Leading zeros / sign / non-decimal sizes.
            format!("{HELLO_NIX32}-042.nar"),
            format!("{HELLO_NIX32}-00.nar"),
            format!("{HELLO_NIX32}-+42.nar"),
            format!("{HELLO_NIX32}--42.nar"),
            format!("{HELLO_NIX32}-4x2.nar"),
            format!("{HELLO_NIX32}-.nar"),
            // u64::MAX + 1 does not fit.
            format!("{HELLO_NIX32}-18446744073709551616.nar"),
            // Missing '-' or missing/wrong extension.
            format!("{HELLO_NIX32}42.nar"),
            format!("{HELLO_NIX32}-42"),
            format!("{HELLO_NIX32}-42.nar.xz"),
            format!("{HELLO_NIX32}-42.narx"),
            String::new(),
        ] {
            assert_eq!(parse_canonical_nar_name(&bad), None, "for '{bad}'");
        }
        // A 52-character string whose leading character carries nonzero
        // padding bits is not the encoding of any 32-byte digest.
        let bad_padding = format!("z{}", &HELLO_NIX32[1..]);
        assert_eq!(
            parse_canonical_nar_name(&format!("{bad_padding}-42.nar")),
            None
        );
    }

    #[test]
    fn canonical_nar_zst_name_round_trips_via_parse_canonical_any() {
        for size in [1_u64, 42, 206_104, u64::MAX] {
            let name = canonical_nar_zst_name(&hello_hash(), size);
            assert_eq!(name, format!("{HELLO_NIX32}-{size}.nar.zst"));
            assert_eq!(
                parse_canonical_any(&name),
                Some((hello_hash(), size, NarCodec::Zstd))
            );
            // A .nar.zst name is not an uncompressed canonical name.
            assert_eq!(parse_canonical_nar_name(&name), None);
        }
        // Zero is a canonical decimal with no leading zeros.
        assert_eq!(
            parse_canonical_any(&canonical_nar_zst_name(&hello_hash(), 0)),
            Some((hello_hash(), 0, NarCodec::Zstd))
        );
    }

    #[test]
    fn parse_canonical_any_accepts_both_forms() {
        let plain = canonical_nar_name(&hello_hash(), 42);
        assert_eq!(
            parse_canonical_any(&plain),
            Some((hello_hash(), 42, NarCodec::None))
        );
        let zst = canonical_nar_zst_name(&hello_hash(), 42);
        assert_eq!(
            parse_canonical_any(&zst),
            Some((hello_hash(), 42, NarCodec::Zstd))
        );
        // It stays in lockstep with parse_canonical_nar_name on .nar.
        assert_eq!(parse_canonical_nar_name(&plain), Some((hello_hash(), 42)));
    }

    #[test]
    fn parse_canonical_any_rejects_malformed_names() {
        let hash51 = &HELLO_NIX32[..51];
        let hash53 = format!("{HELLO_NIX32}0");
        let bad_alphabet = format!("e{}", &HELLO_NIX32[1..]);
        for bad in [
            // Extensions in the wrong order or unsupported.
            format!("{HELLO_NIX32}-42.zst.nar"),
            format!("{HELLO_NIX32}-42.zst"),
            format!("{HELLO_NIX32}-42.nar.xz"),
            format!("{HELLO_NIX32}-42.nar.zst.zst"),
            format!("{HELLO_NIX32}-42.nar.zstd"),
            // 51- and 53-character hashes, in both forms.
            format!("{hash51}-42.nar"),
            format!("{hash51}-42.nar.zst"),
            format!("{hash53}-42.nar"),
            format!("{hash53}-42.nar.zst"),
            // 'e' is outside the nix32 alphabet.
            format!("{bad_alphabet}-42.nar.zst"),
            // Leading zeros / sign / non-decimal sizes.
            format!("{HELLO_NIX32}-042.nar"),
            format!("{HELLO_NIX32}-042.nar.zst"),
            format!("{HELLO_NIX32}-00.nar.zst"),
            format!("{HELLO_NIX32}-+42.nar.zst"),
            format!("{HELLO_NIX32}--42.nar.zst"),
            format!("{HELLO_NIX32}-4x2.nar.zst"),
            format!("{HELLO_NIX32}-.nar.zst"),
            // u64::MAX + 1 does not fit.
            format!("{HELLO_NIX32}-18446744073709551616.nar.zst"),
            // Missing '-' or missing extension.
            format!("{HELLO_NIX32}42.nar.zst"),
            format!("{HELLO_NIX32}-42"),
            String::new(),
        ] {
            assert_eq!(parse_canonical_any(&bad), None, "for '{bad}'");
        }
        // Nonzero padding bits in the leading nix32 character are
        // rejected in the .nar.zst form too.
        let bad_padding = format!("z{}", &HELLO_NIX32[1..]);
        assert_eq!(
            parse_canonical_any(&format!("{bad_padding}-42.nar.zst")),
            None
        );
    }

    #[test]
    fn alias_key_is_prefixed_and_slash_free() {
        assert_eq!(alias_key("abc.nar.xz"), "nar-alias:abc.nar.xz");
        // Only the final path segment is used, so keys never contain '/'.
        assert_eq!(alias_key("nar/abc.nar.xz"), "nar-alias:abc.nar.xz");
        assert_eq!(alias_key("a/b/c.nar"), "nar-alias:c.nar");
        assert_eq!(alias_key(""), "nar-alias:");
        assert!(!alias_key("nar/deep/path.nar").contains('/'));
    }

    #[test]
    fn alias_round_trips() {
        for size in [0_u64, 1, 336_128, u64::MAX] {
            let alias = format_alias(&hello_hash(), size);
            assert_eq!(alias, format!("{HELLO_HEX}-{size}"));
            let (hash, parsed_size) = parse_alias(&alias).expect("parse alias");
            assert_eq!(hash, hello_hash());
            assert_eq!(parsed_size, size);
        }
    }

    #[test]
    fn auxiliary_metadata_keys_have_the_documented_shapes() {
        assert_eq!(
            listing_key("bvkx110ylicifcgl0xiid5f100hx3ar7"),
            "ls:bvkx110ylicifcgl0xiid5f100hx3ar7"
        );
        assert_eq!(
            log_key("q0hpd8s75g1h17yr8zqp1yf8sc9g4gp2-nix-2.34.7.drv"),
            "log:q0hpd8s75g1h17yr8zqp1yf8sc9g4gp2-nix-2.34.7.drv"
        );
        assert_eq!(
            log_encoding_key("q0hpd8s75g1h17yr8zqp1yf8sc9g4gp2-nix-2.34.7.drv"),
            "log-enc:q0hpd8s75g1h17yr8zqp1yf8sc9g4gp2-nix-2.34.7.drv"
        );
    }

    #[test]
    fn auxiliary_metadata_keys_are_slash_free() {
        // Only the final path segment is used, so keys never contain '/'.
        assert_eq!(
            log_key("log/q0hpd8s75g1h17yr8zqp1yf8sc9g4gp2-nix-2.34.7.drv"),
            "log:q0hpd8s75g1h17yr8zqp1yf8sc9g4gp2-nix-2.34.7.drv"
        );
        for pathy in ["a/b/c", "/leading", "trailing/", ""] {
            assert!(!listing_key(pathy).contains('/'), "for '{pathy}'");
            assert!(!log_key(pathy).contains('/'), "for '{pathy}'");
            assert!(!log_encoding_key(pathy).contains('/'), "for '{pathy}'");
        }
    }

    #[test]
    fn parse_alias_rejects_malformed_values() {
        let upper_hex = HELLO_HEX.to_uppercase();
        for bad in [
            // Too short for the 64-character hash / truncated hash.
            String::new(),
            "abc-42".to_string(),
            HELLO_HEX[..63].to_string(),
            // Uppercase and non-hex hashes.
            format!("{upper_hex}-42"),
            format!("g{}-42", &HELLO_HEX[1..]),
            // Missing '-', missing size, non-decimal sizes.
            HELLO_HEX.to_string(),
            format!("{HELLO_HEX}42"),
            format!("{HELLO_HEX}-"),
            format!("{HELLO_HEX}-042"),
            format!("{HELLO_HEX}-+42"),
            format!("{HELLO_HEX}-4x2"),
            format!("{HELLO_HEX}-18446744073709551616"),
        ] {
            assert!(parse_alias(&bad).is_err(), "expected rejection of '{bad}'");
        }
    }
}

#[cfg(test)]
mod proptests {
    use proptest::prelude::*;

    use super::{
        NarCodec, canonical_nar_name, canonical_nar_zst_name, format_alias, parse_alias,
        parse_canonical_any, parse_canonical_nar_name,
    };
    use crate::nixbase32;

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 4096,
            failure_persistence: Some(Box::new(
                proptest::test_runner::FileFailurePersistence::Off,
            )),
            ..ProptestConfig::default()
        })]

        /// Round-trip: a canonical `.nar` name parses back to its exact
        /// `(hash, size)`.
        #[test]
        fn canonical_nar_name_round_trips(hash in any::<[u8; 32]>(), size in any::<u64>()) {
            let name = canonical_nar_name(&hash, size);
            prop_assert_eq!(parse_canonical_nar_name(&name), Some((hash, size)));
            prop_assert_eq!(
                parse_canonical_any(&name),
                Some((hash, size, NarCodec::None))
            );
        }

        /// Round-trip: a canonical `.nar.zst` name parses back via
        /// `parse_canonical_any` and is NOT an uncompressed `.nar` name.
        #[test]
        fn canonical_nar_zst_name_round_trips(hash in any::<[u8; 32]>(), size in any::<u64>()) {
            let name = canonical_nar_zst_name(&hash, size);
            prop_assert_eq!(
                parse_canonical_any(&name),
                Some((hash, size, NarCodec::Zstd))
            );
            prop_assert_eq!(parse_canonical_nar_name(&name), None);
        }

        /// Round-trip: a formatted alias parses back to its `(hash, size)`.
        #[test]
        fn alias_round_trips(hash in any::<[u8; 32]>(), size in any::<u64>()) {
            let alias = format_alias(&hash, size);
            let (parsed_hash, parsed_size) = parse_alias(&alias).expect("round-trip parse");
            prop_assert_eq!(parsed_hash, hash);
            prop_assert_eq!(parsed_size, size);
        }

        /// Parse never panics on arbitrary strings (all four parsers).
        #[test]
        fn parsers_never_panic_on_arbitrary_string(s in ".*") {
            let _a = parse_canonical_nar_name(&s);
            let _b = parse_canonical_any(&s);
            let _c = parse_alias(&s);
        }

        /// Parse never panics on arbitrary bytes viewed as a lossy string.
        #[test]
        fn parsers_never_panic_on_lossy_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..=96)) {
            let s = String::from_utf8_lossy(&bytes);
            let _a = parse_canonical_nar_name(&s);
            let _b = parse_canonical_any(&s);
            let _c = parse_alias(&s);
        }

        /// Canonicality: any string `parse_canonical_nar_name` accepts
        /// re-renders to the identical string.
        #[test]
        fn accepted_nar_names_re_render_identically(s in ".*") {
            if let Some((hash, size)) = parse_canonical_nar_name(&s) {
                prop_assert_eq!(canonical_nar_name(&hash, size), s);
            }
        }

        /// Canonicality: any string `parse_canonical_any` accepts
        /// re-renders (per codec) to the identical string.
        #[test]
        fn accepted_any_names_re_render_identically(s in ".*") {
            if let Some((hash, size, codec)) = parse_canonical_any(&s) {
                let rendered = match codec {
                    NarCodec::None => canonical_nar_name(&hash, size),
                    NarCodec::Zstd => canonical_nar_zst_name(&hash, size),
                    other => panic!("parse_canonical_any yielded unexpected codec {other:?}"),
                };
                prop_assert_eq!(rendered, s);
            }
        }

        /// Canonicality: any alias `parse_alias` accepts re-renders to the
        /// identical string.
        #[test]
        fn accepted_aliases_re_render_identically(s in ".*") {
            if let Ok((hash, size)) = parse_alias(&s) {
                prop_assert_eq!(format_alias(&hash, size), s);
            }
        }

        /// A structured generator that hits the canonical grammar far more
        /// often than `.*`: a genuinely valid 52-char nix32 hash (obtained
        /// by encoding 32 random bytes), `-`, a decimal that may carry
        /// leading zeros or overflow, and a varied suffix. This exercises
        /// the re-render property on genuinely-accepted inputs and on the
        /// near-misses (leading-zero size, overflow, wrong extension).
        #[test]
        fn structured_nar_names_re_render_when_accepted(
            hash_bytes in any::<[u8; 32]>(),
            size_str in "0?[0-9]{1,21}",
            ext in prop::sample::select(vec![".nar", ".nar.zst", ".nar.xz", ""]),
        ) {
            let hash = nixbase32::encode(&hash_bytes);
            let name = format!("{hash}-{size_str}{ext}");
            if let Some((h, sz, codec)) = parse_canonical_any(&name) {
                let rendered = match codec {
                    NarCodec::None => canonical_nar_name(&h, sz),
                    NarCodec::Zstd => canonical_nar_zst_name(&h, sz),
                    other => panic!("unexpected codec {other:?}"),
                };
                prop_assert_eq!(rendered, name);
            }
        }
    }
}
