// Copyright 2026 Trace Machina, Inc. All rights reserved.
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

use core::time::Duration;
use std::sync::Arc;

use nativelink_sim::Run;
use nativelink_sim::fleet::{Counters, Distribution};

fn distribution(seed: u64) -> Distribution {
    Distribution {
        seed,
        run_ms: (100, 5_000),
        fail_percent: 20,
        counters: Arc::new(Counters::default()),
    }
}

/// RFC-0061 invariant 3: the same seed gives every operation the same
/// duration and outcome, on any worker that is handed it.
#[test]
fn distribution_repeats_per_operation() {
    let one = distribution(7);
    let other = distribution(7);
    for i in 0..1_000 {
        let operation = format!("op-{i}");
        assert_eq!(one.draw(&operation), other.draw(&operation));
    }
}

#[test]
fn distribution_stays_in_range_and_fails_about_as_asked() {
    let d = distribution(11);
    let mut failures = 0;
    for i in 0..10_000 {
        let (run_for, run) = d.draw(&format!("op-{i}"));
        assert!(run_for >= Duration::from_millis(100) && run_for <= Duration::from_secs(5));
        if run != Run::Succeed {
            failures += 1;
        }
    }
    assert!(
        (1_500..2_500).contains(&failures),
        "{failures} of 10,000 failed"
    );
}

#[test]
fn seeds_differ() {
    let a = distribution(1);
    let b = distribution(2);
    let same = (0..100)
        .filter(|i| a.draw(&format!("op-{i}")) == b.draw(&format!("op-{i}")))
        .count();
    assert!(same < 10, "{same} of 100 draws matched across seeds");
}

/// The generator streams a blob block by block; its digest must be the
/// digest of the whole, across a block boundary and a short last block.
#[test]
fn generated_blob_digest_matches_its_bytes() {
    use nativelink_sim::cas_load::{generated_bytes, generated_digest_of};
    let size = 2 * 1024 * 1024 + 12_345;
    let bytes = generated_bytes(9, 3, size);
    assert_eq!(bytes.len(), size);
    let mut hasher = nativelink_util::digest_hasher::DigestHasherFunc::Sha256.hasher();
    nativelink_util::digest_hasher::DigestHasher::update(&mut hasher, &bytes);
    let whole = nativelink_util::digest_hasher::DigestHasher::finalize_digest(&mut hasher);
    assert_eq!(generated_digest_of(9, 3, size), whole);
    assert_ne!(generated_bytes(9, 4, 64), generated_bytes(9, 3, 64));
}
