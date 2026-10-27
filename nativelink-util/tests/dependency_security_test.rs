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

//! Regression coverage for RUSTSEC-2026-0391, RUSTSEC-2026-0395 through
//! RUSTSEC-2026-0406 and RUSTSEC-2026-0410: memory-safety defects in the
//! zstd stack before zstd 0.14.3, zstd-safe 8.2.0 and zstd-sys 2.1.2.
//!
//! Bumping the direct dependency is not enough. A transitive dependency
//! that requires the older line, such as a crate enabling tonic's `zstd`
//! feature, resolves a second, vulnerable copy beside the fixed one, and
//! that is how the first bump shipped while every advisory stayed open.
//! This test reads the resolved lock and fails when any copy of these
//! crates, not merely the newest, is below its fixed release.

const FIXED_FLOORS: &[(&str, [u64; 3])] = &[
    ("zstd", [0, 14, 3]),
    ("zstd-safe", [8, 2, 0]),
    ("zstd-sys", [2, 1, 2]),
];

/// The lock to audit: the runner's environment names it under Bazel, and a
/// cargo invocation finds it by walking up from its working directory.
fn cargo_lock_contents() -> Option<String> {
    if let Ok(path) = std::env::var("CARGO_LOCK_FOR_AUDIT") {
        return std::fs::read_to_string(path).ok();
    }
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join("Cargo.lock");
        if candidate.is_file() {
            return std::fs::read_to_string(candidate).ok();
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Parses `1.2.3`, tolerating build or pre-release suffixes like
/// `2.1.2+zstd.1.5.7`.
fn parse_version(version: &str) -> Option<[u64; 3]> {
    let bare = version.split(['+', '-']).next()?;
    let mut parts = bare.splitn(3, '.');
    Some([
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ])
}

fn resolved_versions(lock: &str, package: &str) -> Vec<String> {
    let mut versions = Vec::new();
    for block in lock.split("[[package]]") {
        let mut name = None;
        let mut version = None;
        for line in block.lines() {
            if let Some(value) = line.strip_prefix("name = \"") {
                name = value.strip_suffix('"');
            } else if let Some(value) = line.strip_prefix("version = \"") {
                version = value.strip_suffix('"');
            }
        }
        if name == Some(package)
            && let Some(version) = version
        {
            versions.push(version.to_owned());
        }
    }
    versions
}

#[test]
fn every_resolved_zstd_crate_is_at_or_above_its_fixed_release() {
    let Some(lock) = cargo_lock_contents() else {
        eprintln!("Cargo.lock is not reachable from this environment; nothing to audit");
        return;
    };
    for (package, floor) in FIXED_FLOORS {
        let versions = resolved_versions(&lock, package);
        assert!(
            !versions.is_empty(),
            "{package} disappeared from Cargo.lock; update this audit alongside the dependency"
        );
        for version in &versions {
            let parsed = parse_version(version)
                .unwrap_or_else(|| panic!("unparsable {package} version {version}"));
            assert!(
                parsed >= *floor,
                "{package} {version} resolves below the fixed release \
                 {}.{}.{}; a dependency reintroduced the vulnerable line",
                floor[0],
                floor[1],
                floor[2]
            );
        }
    }
}
