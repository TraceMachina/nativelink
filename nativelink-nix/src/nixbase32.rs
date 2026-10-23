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

//! Nix's nonstandard base32 encoding (LSB-first, alphabet omits `e o u t`).
//!
//! Reference implementation: `src/libutil/base-nix-32.cc` (and its header
//! `include/nix/util/base-nix-32.hh`) in Nix 2.34. Character `i` of the
//! encoding covers 5 bits starting at bit `5 * i` counted from the *low*
//! end of the byte string, and Nix emits characters from the highest index
//! down. The encoded length of `n` bytes is `(n * 8 - 1) / 5 + 1`
//! characters (52 characters for a 32-byte `sha256` digest).

use nativelink_error::{Error, make_input_err};

/// The nix32 digit alphabet. Deliberately omits `e`, `o`, `u`, and `t` to
/// reduce the chance of hashes spelling recognizable words.
const ALPHABET: &[u8; 32] = b"0123456789abcdfghijklmnpqrsvwxyz";

/// Sentinel for bytes that are not nix32 digits in [`REVERSE`].
const INVALID: u8 = 0xff;

/// Maps an ASCII byte to its 5-bit nix32 digit value, or [`INVALID`].
const REVERSE: [u8; 256] = {
    let mut map = [INVALID; 256];
    let mut digit: u8 = 0;
    while digit < 32 {
        map[ALPHABET[digit as usize] as usize] = digit;
        digit += 1;
    }
    map
};

/// Returns true if `byte` is a valid nix32 digit.
pub(crate) const fn is_valid_char(byte: u8) -> bool {
    REVERSE[byte as usize] != INVALID
}

/// Returns the nix32-encoded length of `input_len` bytes.
///
/// Mirrors `BaseNix32::encodedLength` in Nix: `(n * 8 - 1) / 5 + 1`,
/// with zero bytes encoding to zero characters.
const fn encoded_length(input_len: usize) -> usize {
    if input_len == 0 {
        0
    } else {
        (input_len * 8 - 1) / 5 + 1
    }
}

/// Encodes `input` in Nix's base32 variant.
///
/// Bit-for-bit equivalent to `BaseNix32::encode` in Nix 2.34: character
/// `i` (counting from the end of the returned string) covers bits
/// `5 * i ..= 5 * i + 4` of the little-endian bit string of `input`.
#[must_use]
pub fn encode(input: &[u8]) -> String {
    let len = encoded_length(input.len());
    let mut out = String::with_capacity(len);
    for n in (0..len).rev() {
        let bit = n * 5;
        let byte = bit / 8;
        let shift = bit % 8;
        // `byte < input.len()` because `5 * (len - 1) < 8 * input.len()`
        // holds for every non-zero `len = encoded_length(input.len())`.
        let low = u16::from(input[byte]) >> shift;
        let high = input
            .get(byte + 1)
            .map_or(0_u16, |&next| u16::from(next) << (8 - shift));
        let digit = usize::from((low | high) & 0x1f);
        out.push(char::from(ALPHABET[digit]));
    }
    out
}

/// Decodes a nix32 string produced by [`encode`].
///
/// # Errors
///
/// Returns an `InvalidArgument` error if `input` contains characters
/// outside the nix32 alphabet, has a length that no byte string encodes
/// to, or carries nonzero padding bits in its leading character.
pub fn decode(input: &str) -> Result<Vec<u8>, Error> {
    let decoded_len = input.len() * 5 / 8;
    if encoded_length(decoded_len) != input.len() {
        return Err(make_input_err!(
            "invalid nix32 length {}: no byte string encodes to it",
            input.len()
        ));
    }
    let mut out = vec![0_u8; decoded_len];
    for (n, &c) in input.as_bytes().iter().rev().enumerate() {
        let digit = REVERSE[usize::from(c)];
        if digit == INVALID {
            return Err(make_input_err!(
                "invalid character '{}' in nix32 string '{input}'",
                char::from(c)
            ));
        }
        let bit = n * 5;
        let byte = bit / 8;
        let shift = bit % 8;
        // `byte < decoded_len` for every character index of a valid
        // length (checked above), so this never panics.
        out[byte] |= digit << shift;
        // Bits shifted off the top of `out[byte]` carry into the next
        // byte; for the leading character they are padding and must be
        // zero. The `shift > 3` guard both avoids an overflowing
        // `8 - shift` shift when `shift == 0` and skips cases where the
        // 5-bit digit cannot cross a byte boundary.
        if shift > 3 {
            let carry = digit >> (8 - shift);
            if carry != 0 {
                if let Some(slot) = out.get_mut(byte + 1) {
                    *slot |= carry;
                } else {
                    return Err(make_input_err!(
                        "invalid nix32 string '{input}': leading character '{}' has nonzero padding bits",
                        char::from(c)
                    ));
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{decode, encode, encoded_length, is_valid_char};

    // Golden vectors generated with the local Nix CLI (nix 2.34.7).
    //
    // 32-byte sha256 vector #1:
    //   $ echo -n hello | sha256sum
    //   2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
    //   $ nix hash convert --hash-algo sha256 --to nix32 \
    //       2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
    //   094qif9n4cq4fdg459qzbhg1c6wywawwaaivx0k0x8xhbyx4vwic
    const SHA256_HELLO_HEX: &str =
        "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
    const SHA256_HELLO_NIX32: &str = "094qif9n4cq4fdg459qzbhg1c6wywawwaaivx0k0x8xhbyx4vwic";

    // 32-byte sha256 vector #2:
    //   $ echo -n "" | sha256sum
    //   e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
    //   $ nix hash convert --hash-algo sha256 --to nix32 \
    //       e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
    //   0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73
    const SHA256_EMPTY_HEX: &str =
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const SHA256_EMPTY_NIX32: &str = "0mdqa9w1p6cmli6976v4wi0sw9r4p5prkj7lzfd1877wk11c9c73";

    // Short asymmetric input (20 bytes -> 32 characters):
    //   $ echo -n hello | sha1sum
    //   aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d
    //   $ nix hash convert --hash-algo sha1 --to nix32 \
    //       aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d
    //   9m1skbnr5i43n3yypvda5s65vhfwdx5a
    const SHA1_HELLO_HEX: &str = "aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d";
    const SHA1_HELLO_NIX32: &str = "9m1skbnr5i43n3yypvda5s65vhfwdx5a";

    // Short asymmetric input (16 bytes -> 26 characters):
    //   $ echo -n hello | md5sum
    //   5d41402abc4b2a76b9719d911017c592
    //   $ nix hash convert --hash-algo md5 --to nix32 \
    //       5d41402abc4b2a76b9719d911017c592
    //   4jqlbi14cxf6wpcajbphm40hax
    const MD5_HELLO_HEX: &str = "5d41402abc4b2a76b9719d911017c592";
    const MD5_HELLO_NIX32: &str = "4jqlbi14cxf6wpcajbphm40hax";

    fn unhex(s: &str) -> Vec<u8> {
        hex::decode(s).expect("valid hex in test vector")
    }

    #[test]
    fn empty_input_round_trips() {
        assert_eq!(encode(&[]), "");
        assert_eq!(decode("").expect("empty decode"), Vec::<u8>::new());
    }

    #[test]
    fn encoded_length_matches_nix_formula() {
        assert_eq!(encoded_length(0), 0);
        assert_eq!(encoded_length(1), 2);
        assert_eq!(encoded_length(16), 26);
        assert_eq!(encoded_length(20), 32);
        assert_eq!(encoded_length(32), 52);
    }

    #[test]
    fn golden_vectors_encode_like_nix() {
        for (hex_str, nix32) in [
            (SHA256_HELLO_HEX, SHA256_HELLO_NIX32),
            (SHA256_EMPTY_HEX, SHA256_EMPTY_NIX32),
            (SHA1_HELLO_HEX, SHA1_HELLO_NIX32),
            (MD5_HELLO_HEX, MD5_HELLO_NIX32),
        ] {
            assert_eq!(encode(&unhex(hex_str)), nix32, "encode of {hex_str}");
        }
    }

    #[test]
    fn golden_vectors_round_trip() {
        for hex_str in [
            SHA256_HELLO_HEX,
            SHA256_EMPTY_HEX,
            SHA1_HELLO_HEX,
            MD5_HELLO_HEX,
        ] {
            let bytes = unhex(hex_str);
            assert_eq!(
                decode(&encode(&bytes)).expect("round trip decode"),
                bytes,
                "round trip of {hex_str}"
            );
        }
    }

    #[test]
    fn golden_vectors_decode_like_nix() {
        for (hex_str, nix32) in [
            (SHA256_HELLO_HEX, SHA256_HELLO_NIX32),
            (SHA256_EMPTY_HEX, SHA256_EMPTY_NIX32),
        ] {
            assert_eq!(decode(nix32).expect("decode"), unhex(hex_str));
        }
    }

    #[test]
    fn all_single_bytes_round_trip() {
        for byte in 0..=u8::MAX {
            let encoded = encode(&[byte]);
            assert_eq!(encoded.len(), 2);
            assert_eq!(decode(&encoded).expect("decode"), vec![byte]);
        }
    }

    #[test]
    fn rejects_characters_outside_the_alphabet() {
        // 'e', 'o', 'u', 't' are deliberately absent from the alphabet, as
        // are uppercase letters and punctuation.
        for bad in ["7e", "7o", "7u", "7t", "7A", "7!", "7="] {
            assert!(decode(bad).is_err(), "expected error for '{bad}'");
        }
    }

    #[test]
    fn rejects_impossible_lengths() {
        // No byte count encodes to 1, 3, 6, or 9 characters.
        for bad in ["0", "000", "000000", "000000000"] {
            assert!(
                decode(bad).is_err(),
                "expected error for length {}",
                bad.len()
            );
        }
    }

    #[test]
    fn rejects_nonzero_padding_bits() {
        // Two characters hold 10 bits but decode to a single byte; the top
        // two bits must be zero, so the leading digit must be < 8.
        assert!(decode("7z").is_ok());
        assert!(decode("8z").is_err());
        assert!(decode("zz").is_err());
    }

    #[test]
    fn alphabet_membership() {
        assert!(is_valid_char(b'0'));
        assert!(is_valid_char(b'z'));
        assert!(!is_valid_char(b'e'));
        assert!(!is_valid_char(b'o'));
        assert!(!is_valid_char(b'u'));
        assert!(!is_valid_char(b't'));
        assert!(!is_valid_char(b'E'));
    }
}

#[cfg(test)]
mod proptests {
    use proptest::prelude::*;

    use super::{ALPHABET, decode, encode, is_valid_char};

    proptest! {
        #![proptest_config(ProptestConfig {
            cases: 4096,
            failure_persistence: Some(Box::new(
                proptest::test_runner::FileFailurePersistence::Off,
            )),
            ..ProptestConfig::default()
        })]

        /// `decode(encode(bytes)) == bytes` for arbitrary byte strings.
        #[test]
        fn decode_of_encode_round_trips(bytes in proptest::collection::vec(any::<u8>(), 0..=64)) {
            let encoded = encode(&bytes);
            prop_assert_eq!(encoded.len(), if bytes.is_empty() { 0 } else { (bytes.len() * 8 - 1) / 5 + 1 });
            prop_assert_eq!(decode(&encoded).expect("round-trip decode"), bytes);
        }

        /// `decode` never panics on arbitrary UTF-8 strings.
        #[test]
        fn decode_never_panics_on_arbitrary_string(s in ".*") {
            // Reaching here without unwinding is the property; consume the
            // result so no `let _` / must-use lint fires.
            // Determinism doubles as a no-panic assertion (both calls run).
            prop_assert_eq!(decode(&s).is_ok(), decode(&s).is_ok());
        }

        /// `decode` never panics on arbitrary bytes viewed as a lossy string.
        #[test]
        fn decode_never_panics_on_lossy_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..=80)) {
            let s = String::from_utf8_lossy(&bytes);
            // Determinism doubles as a no-panic assertion (both calls run).
            prop_assert_eq!(decode(&s).is_ok(), decode(&s).is_ok());
        }

        /// Canonicality: any string that decodes Ok re-encodes to itself.
        /// This is what rejects non-canonical encodings (nonzero trailing
        /// padding bits): they must never decode successfully.
        #[test]
        fn decodable_strings_are_canonical(
            s in proptest::collection::vec(0usize..32, 0..=60)
                .prop_map(|digits| digits.into_iter().map(|d| char::from(ALPHABET[d])).collect::<String>())
        ) {
            if let Ok(decoded) = decode(&s) {
                prop_assert_eq!(encode(&decoded), s);
            }
        }

        /// Encoding arbitrary bytes always yields a canonical string that
        /// decodes back and re-encodes to itself (a fixed point).
        #[test]
        fn encode_output_is_a_decode_fixed_point(bytes in proptest::collection::vec(any::<u8>(), 0..=48)) {
            let encoded = encode(&bytes);
            let decoded = decode(&encoded).expect("encode output must decode");
            prop_assert_eq!(&decoded, &bytes);
            prop_assert_eq!(encode(&decoded), encoded);
        }

        /// Every character outside the alphabet causes rejection: splice a
        /// non-alphabet byte into an otherwise valid encoding.
        #[test]
        fn non_alphabet_character_is_rejected(
            bytes in proptest::collection::vec(any::<u8>(), 1..=32),
            pos in any::<prop::sample::Index>(),
            bad in any::<char>().prop_filter(
                "must be a non-alphabet ASCII-representable char",
                |c| !c.is_ascii() || !is_valid_char(*c as u8),
            ),
        ) {
            let encoded = encode(&bytes);
            let idx = pos.index(encoded.len());
            let mut chars: Vec<char> = encoded.chars().collect();
            chars[idx] = bad;
            let mutated: String = chars.into_iter().collect();
            // The only way a mutation can still decode is if `bad` is
            // itself an alphabet character AND the result stays canonical;
            // by construction `bad` is not in the alphabet, so decode must
            // fail on that character.
            prop_assert!(decode(&mutated).is_err(), "expected rejection of {:?}", mutated);
        }
    }
}
