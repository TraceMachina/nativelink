// Copyright 2024 The NativeLink Authors. All rights reserved.
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

//! Conformance tests for the `FastCDC` 2020 implementation used by the
//! `SplitBlob` on-demand chunking path against the official REAPI test
//! vectors from the remote-apis repository:
//! <https://github.com/bazelbuild/remote-apis/blob/main/build/bazel/remote/execution/v2/fastcdc2020_test_vectors.txt>
//!
//! The chunk boundaries produced by the server MUST match these vectors
//! byte-for-byte; otherwise chunks produced by clients (e.g. Bazel with
//! `--experimental_remote_cache_chunking`) never deduplicate against chunks
//! produced by the server and the feature silently loses its value.

use fastcdc::v2020::{AsyncStreamCDC, FastCDC, Normalization};
use futures::StreamExt;
use nativelink_macro::nativelink_test;
use pretty_assertions::assert_eq;
use sha2::{Digest as _, Sha256};

/// The canonical test input named in the vectors file header
/// (SHA256 d9e749d9367fc908876749d6502eb212fee88c9a94892fb07da5ef3ba8bc39ed).
const TEST_INPUT: &[u8] = include_bytes!("data/SekienAkashita.jpg");
const TEST_VECTORS: &str = include_str!("data/fastcdc2020_test_vectors.txt");

/// Parameters stated in the vectors file header.
const MIN_SIZE: u32 = 4096;
const AVG_SIZE: u32 = 16384;
const MAX_SIZE: u32 = 65535;

struct ExpectedChunk {
    offset: u64,
    length: usize,
    sha256_hex: String,
    fingerprint: u64,
}

/// Parses the `# Seed: <n>` sections of the vectors file into
/// (seed, expected chunks) pairs.
fn parse_test_vectors() -> Vec<(u64, Vec<ExpectedChunk>)> {
    let mut sections = Vec::new();
    for line in TEST_VECTORS.lines() {
        let line = line.trim();
        if let Some(seed) = line.strip_prefix("# Seed: ") {
            sections.push((seed.parse::<u64>().unwrap(), Vec::new()));
            continue;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        let chunk = ExpectedChunk {
            offset: fields.next().unwrap().parse().unwrap(),
            length: fields.next().unwrap().parse().unwrap(),
            sha256_hex: fields.next().unwrap().to_string(),
            fingerprint: fields.next().unwrap().parse().unwrap(),
        };
        sections
            .last_mut()
            .expect("chunk line before any '# Seed:' section")
            .1
            .push(chunk);
    }
    assert!(!sections.is_empty(), "no seed sections parsed");
    sections
}

#[nativelink_test]
async fn fastcdc2020_matches_reapi_test_vectors() -> Result<(), Box<dyn core::error::Error>> {
    assert_eq!(
        hex::encode(Sha256::digest(TEST_INPUT)),
        "d9e749d9367fc908876749d6502eb212fee88c9a94892fb07da5ef3ba8bc39ed",
        "test fixture does not match the input named in the vectors file"
    );

    for (seed, expected_chunks) in parse_test_vectors() {
        let chunks: Vec<_> = FastCDC::with_level_and_seed(
            TEST_INPUT,
            MIN_SIZE,
            AVG_SIZE,
            MAX_SIZE,
            Normalization::Level2,
            seed,
        )
        .collect();
        assert_eq!(
            chunks.len(),
            expected_chunks.len(),
            "chunk count mismatch for seed {seed}"
        );
        for (chunk, expected) in chunks.iter().zip(&expected_chunks) {
            assert_eq!(chunk.offset as u64, expected.offset, "offset, seed {seed}");
            assert_eq!(chunk.length, expected.length, "length, seed {seed}");
            assert_eq!(chunk.hash, expected.fingerprint, "fingerprint, seed {seed}");
            let data = &TEST_INPUT[chunk.offset..chunk.offset + chunk.length];
            assert_eq!(
                hex::encode(Sha256::digest(data)),
                expected.sha256_hex,
                "chunk content sha256, seed {seed}"
            );
        }
    }
    Ok(())
}

/// The streaming chunker (the variant `SplitBlob` actually uses) must
/// produce the same boundaries as the in-memory reference.
#[nativelink_test]
async fn fastcdc2020_streaming_matches_reapi_test_vectors()
-> Result<(), Box<dyn core::error::Error>> {
    let sections = parse_test_vectors();
    let (_, expected_chunks) = sections
        .iter()
        .find(|(seed, _)| *seed == 0)
        .expect("seed 0 section missing");

    let mut cdc = AsyncStreamCDC::with_level(
        TEST_INPUT,
        MIN_SIZE,
        AVG_SIZE,
        MAX_SIZE,
        Normalization::Level2,
    );
    let stream = cdc.as_stream();
    let mut stream = core::pin::pin!(stream);
    let mut chunks = Vec::new();
    while let Some(chunk) = stream.next().await {
        chunks.push(chunk.expect("chunking the test input failed"));
    }
    assert_eq!(chunks.len(), expected_chunks.len());
    for (chunk, expected) in chunks.iter().zip(expected_chunks) {
        assert_eq!(chunk.offset, expected.offset);
        assert_eq!(chunk.length, expected.length);
        assert_eq!(chunk.hash, expected.fingerprint);
        assert_eq!(
            hex::encode(Sha256::digest(&chunk.data)),
            expected.sha256_hex
        );
    }
    Ok(())
}
