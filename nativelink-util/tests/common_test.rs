// Copyright 2024 The NativeLink Authors. All rights reserved.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//    http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use nativelink_error::{Error, make_input_err};
use nativelink_macro::nativelink_test;
use nativelink_util::common::DigestInfo;
use pretty_assertions::assert_eq;

const MIN_DIGEST: &str = "0000000000000000000000000000000000000000000000000000000000000000-0";
const MAX_SAFE_DIGEST: &str =
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff-9223372036854775807";
const MAX_UNSAFE_DIGEST: &str =
    "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff-18446744073709551615";

#[nativelink_test]
async fn digest_info_min_max_test() -> Result<(), Error> {
    {
        const DIGEST: DigestInfo = DigestInfo::new([0u8; 32], u64::MIN);
        assert_eq!(&format!("{DIGEST}"), MIN_DIGEST);
    }
    {
        const DIGEST: DigestInfo = DigestInfo::new([255u8; 32], u64::MAX);
        assert_eq!(&format!("{DIGEST}"), MAX_UNSAFE_DIGEST);
    }
    Ok(())
}

#[nativelink_test]
async fn digest_info_try_new_min_max_test() -> Result<(), Error> {
    {
        let digest_parts: (&str, u64) = MIN_DIGEST
            .split_once('-')
            .map(|(s, n)| (s, n.parse().unwrap()))
            .unwrap();
        let digest = DigestInfo::try_new(digest_parts.0, digest_parts.1).unwrap();
        assert_eq!(&format!("{digest}"), MIN_DIGEST);
    }
    {
        let digest_parts: (&str, u64) = MAX_SAFE_DIGEST
            .split_once('-')
            .map(|(s, n)| (s, n.parse().unwrap()))
            .unwrap();
        let digest = DigestInfo::try_new(digest_parts.0, digest_parts.1).unwrap();
        assert_eq!(&format!("{digest}"), MAX_SAFE_DIGEST);
    }
    {
        let digest_parts: (&str, u64) = MAX_UNSAFE_DIGEST
            .split_once('-')
            .map(|(s, n)| (s, n.parse().unwrap()))
            .unwrap();
        let digest_res = DigestInfo::try_new(digest_parts.0, digest_parts.1);
        assert_eq!(
            digest_res,
            Err(make_input_err!(
                "Size bytes is too large: 18446744073709551615 - max: 9223372036854775807"
            ))
        );
    }
    {
        // Sanity check to ensure non-repeating digests print properly
        const DIGEST: &str =
            "123456789abcdeffedcba987654321089abcde0123456789ffedcba987654321-123456789";
        let digest_parts: (&str, u64) = DIGEST
            .split_once('-')
            .map(|(s, n)| (s, n.parse().unwrap()))
            .unwrap();
        let digest = DigestInfo::try_new(digest_parts.0, digest_parts.1).unwrap();
        assert_eq!(&format!("{digest}"), DIGEST);
    }
    Ok(())
}

#[nativelink_test]
async fn digest_info_serialize_test() -> Result<(), Error> {
    {
        const DIGEST: DigestInfo = DigestInfo::new([0u8; 32], u64::MIN);
        assert_eq!(
            serde_json::to_string(&DIGEST).unwrap(),
            format!("\"{MIN_DIGEST}\"")
        );
    }
    {
        const DIGEST: DigestInfo = DigestInfo::new([255u8; 32], i64::MAX as u64);
        assert_eq!(
            serde_json::to_string(&DIGEST).unwrap(),
            format!("\"{MAX_SAFE_DIGEST}\"")
        );
    }
    {
        const DIGEST: DigestInfo = DigestInfo::new([255u8; 32], u64::MAX);
        assert_eq!(
            serde_json::to_string(&DIGEST).unwrap(),
            format!("\"{MAX_UNSAFE_DIGEST}\"")
        );
    }
    Ok(())
}

#[nativelink_test]
async fn digest_info_deserialize_test() -> Result<(), Error> {
    {
        assert_eq!(
            serde_json::from_str::<DigestInfo>(&format!("\"{MIN_DIGEST}\"")).unwrap(),
            DigestInfo::new([0u8; 32], u64::MIN)
        );
    }
    {
        assert_eq!(
            serde_json::from_str::<DigestInfo>(&format!("\"{MAX_SAFE_DIGEST}\"")).unwrap(),
            DigestInfo::new([255u8; 32], i64::MAX as u64)
        );
    }
    {
        let digest_res = serde_json::from_str::<DigestInfo>(&format!("\"{MAX_UNSAFE_DIGEST}\""));
        assert_eq!(
            format!("{}", digest_res.err().unwrap()),
            "Could not create DigestInfo: Error { code: InvalidArgument, messages: [\"Size bytes is too large: 18446744073709551615 - max: 9223372036854775807\"] } at line 1 column 87",
        );
    }
    Ok(())
}
