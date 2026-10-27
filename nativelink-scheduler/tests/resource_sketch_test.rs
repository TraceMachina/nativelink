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

use core::time::Duration;

use nativelink_config::schedulers::SizeClassSpec;
use nativelink_scheduler::resource_sketch::{
    Budget, Entry, Family, Hist, Observation, Policy, ResourceSketch, SAMPLER_GENERATION,
    SketchKey, Snapshot,
};
use pretty_assertions::assert_eq;

fn class(name: &str, cpu_count: u64, memory_kb: u64, disk_kb: u64) -> SizeClassSpec {
    SizeClassSpec {
        name: name.to_string(),
        cpu_count,
        memory_kb,
        disk_kb,
    }
}

const fn policy(headroom_percent: u64, classes: Vec<SizeClassSpec>) -> Policy {
    Policy {
        headroom_percent,
        classes,
        reserve_cpu: true,
        reserve_memory: true,
        reserve_disk: true,
    }
}

fn sketch(policy: Policy, budget: Budget) -> ResourceSketch {
    ResourceSketch::new(policy, budget, Duration::from_hours(7 * 24))
}

const fn small_budget() -> Budget {
    Budget {
        target: 3,
        mnemonic: 2,
        command: 2,
        probation: 4,
    }
}

fn target(name: &str, mnemonic: &str) -> SketchKey {
    SketchKey::for_attempt(Some(name), Some(mnemonic), "unused")
}

fn command(digest: &str) -> SketchKey {
    SketchKey::for_attempt(None, None, digest)
}

fn memory(peak_kb: u64, now_s: u64) -> Observation {
    Observation {
        peak_memory_kb: peak_kb,
        now_s,
        ..Observation::default()
    }
}

/// The rollup's `hist_bucket`: 0 under 1000, then floor(log2(v / 1000)) + 1.
#[test]
fn histogram_buckets_follow_the_rollup() {
    for (value, bucket) in [
        (0u64, 0usize),
        (999, 0),
        (1000, 1),
        (1999, 1),
        (2000, 2),
        (3999, 2),
        (4000, 3),
        (1_048_576_000, 21),
        (u64::MAX, 24),
    ] {
        assert_eq!(Hist::bucket(value), bucket, "{value}");
    }
}

/// The p95 is the upper bound of the bucket the 95th sample falls in.
#[test]
fn p95_is_the_upper_bound_of_the_95th_samples_bucket() {
    let mut hist = Hist::default();
    assert_eq!(hist.p95_upper(), 0, "empty");
    hist.add(1500);
    assert_eq!(hist.p95_upper(), 2000, "one sample in [1000, 2000)");
    // 95 small samples and 5 large: the 95th of 100 is still small.
    let mut hist = Hist::default();
    for _ in 0..95 {
        hist.add(1500);
    }
    for _ in 0..5 {
        hist.add(100_000);
    }
    assert_eq!(hist.p95_upper(), 2000);
    hist.add(100_000);
    assert_eq!(hist.p95_upper(), 128_000, "the 96th of 101 is large");
}

/// One sample reserves exactly what it used, plus headroom; the bucket's
/// upper bound only applies once the maximum is above it.
#[test]
fn shape_caps_the_bucket_by_the_exact_maximum() {
    let sketch = sketch(policy(20, vec![]), small_budget());
    let key = target("//a", "Genrule");
    sketch.observe(key.clone(), &memory(3_000_000, 1));
    let shaped = sketch.shape(&sketch.lookup(&key).unwrap());
    assert_eq!(shaped.memory_kb, 3_600_000);
    assert_eq!(shaped.cpu_count, 0, "no cpu measured");
    assert!(shaped.class.is_none());
    // A thousand samples at 3M: the bucket's bound is 4M, the max 3M.
    for _ in 0..1000 {
        sketch.observe(key.clone(), &memory(3_000_000, 1));
    }
    assert_eq!(
        sketch.shape(&sketch.lookup(&key).unwrap()).memory_kb,
        3_600_000
    );
    // One at 3.9M moves the max, not the bucket.
    sketch.observe(key.clone(), &memory(3_900_000, 1));
    assert_eq!(
        sketch.shape(&sketch.lookup(&key).unwrap()).memory_kb,
        4_680_000
    );
}

/// The kill floor: a hint never sits at or below a reservation the action
/// was killed at, and a class whose memory would be killed again is passed.
#[test]
fn shape_sits_above_the_kill() {
    let ladder = vec![
        class("s", 1000, 2_000_000, 0),
        class("m", 2000, 6_000_000, 0),
        class("l", 4000, 12_000_000, 0),
    ];
    let sketch = sketch(policy(0, ladder), small_budget());
    let key = target("//a", "Test");
    sketch.observe(
        key.clone(),
        &Observation {
            peak_memory_kb: 1_500_000,
            killed_at_kb: Some(2_000_000),
            killed: true,
            now_s: 1,
            ..Observation::default()
        },
    );
    let shaped = sketch.shape(&sketch.lookup(&key).unwrap());
    assert_eq!(shaped.class.as_deref(), Some("m"), "s was killed at");
    assert_eq!(shaped.memory_kb, 6_000_000);
    // Killed at the top: no class sits above it, the raw number stands.
    sketch.observe(
        key.clone(),
        &Observation {
            peak_memory_kb: 11_000_000,
            killed_at_kb: Some(12_000_000),
            killed: true,
            now_s: 2,
            ..Observation::default()
        },
    );
    let shaped = sketch.shape(&sketch.lookup(&key).unwrap());
    assert!(shaped.class.is_none(), "{shaped:?}");
    assert_eq!(shaped.memory_kb, 12_000_001);
}

/// A dimension the ladder does not reserve is dropped before anything is
/// decided from it: a disk-only measurement is no hint.
#[test]
fn shape_drops_a_dimension_the_fleet_does_not_reserve() {
    let mut policy = policy(0, vec![class("s", 1000, 2_000_000, 0)]);
    policy.reserve_disk = false;
    let sketch = sketch(policy, small_budget());
    let key = target("//a", "Link");
    sketch.observe(
        key.clone(),
        &Observation {
            peak_disk_kb: 9000,
            now_s: 1,
            ..Observation::default()
        },
    );
    let shaped = sketch.shape(&sketch.lookup(&key).unwrap());
    assert!(shaped.is_empty(), "{shaped:?}");
    sketch.observe(key.clone(), &memory(500_000, 2));
    let shaped = sketch.shape(&sketch.lookup(&key).unwrap());
    assert_eq!(shaped.class.as_deref(), Some("s"));
    assert_eq!(shaped.disk_kb, 0);
}

/// A command key earns an entry on its second sighting or on a kill; a
/// digest that ran once and was not killed is an action-cache hit next time.
#[test]
fn a_command_key_is_on_probation_until_seen_twice_or_killed() {
    let sketch = sketch(policy(0, vec![]), small_budget());
    assert!(!sketch.observe(command("aa-1"), &memory(1000, 1)));
    assert!(sketch.lookup(&command("aa-1")).is_none());
    assert_eq!(sketch.stats().probation, 1);
    assert!(sketch.observe(command("aa-1"), &memory(1000, 2)));
    assert_eq!(sketch.lookup(&command("aa-1")).unwrap().samples, 1);
    assert_eq!(sketch.stats().probation, 0);
    assert!(sketch.observe(
        command("bb-1"),
        &Observation {
            peak_memory_kb: 1000,
            killed: true,
            now_s: 3,
            ..Observation::default()
        }
    ));
    assert!(
        sketch.lookup(&command("bb-1")).is_some(),
        "a kill admits at once"
    );
    // Target keys are admitted on first sight.
    assert!(sketch.observe(target("//a", "X"), &memory(1000, 4)));
}

/// A full family evicts its least recently seen key; a lookup counts as
/// use; another family is untouched.
#[test]
fn a_full_family_evicts_the_least_recently_seen() {
    let sketch = sketch(policy(0, vec![]), small_budget());
    for name in ["//a", "//b", "//c"] {
        sketch.observe(target(name, "X"), &memory(1000, 1));
    }
    assert!(sketch.lookup(&target("//a", "X")).is_some(), "promotes //a");
    sketch.observe(target("//d", "X"), &memory(1000, 2));
    assert!(
        sketch.lookup(&target("//b", "X")).is_none(),
        "//b was the oldest"
    );
    assert!(sketch.lookup(&target("//a", "X")).is_some());
    let stats = sketch.stats();
    assert_eq!(stats.entries, [3, 0, 0]);
    assert_eq!(stats.evictions, [1, 0, 0]);
    assert_eq!(stats.samples, [4, 0, 0]);
}

/// A snapshot round-trips every field, drops keys outside the window, and
/// comes back in the same recency order.
#[test]
fn a_snapshot_round_trips_and_ages_out() {
    let sketch = sketch(policy(0, vec![]), small_budget());
    let old = 1_000_000u64;
    let now = old + 10 * 86_400;
    sketch.observe(target("//old", "X"), &memory(1000, old));
    sketch.observe(
        target("//new", "Y"),
        &Observation {
            peak_memory_kb: 123_456,
            cpu_millicores: Some(2500),
            peak_disk_kb: 77,
            killed_at_kb: Some(100_000),
            killed: true,
            now_s: now,
        },
    );
    sketch.observe(command("cc-9"), &memory(5000, now));
    sketch.observe(command("cc-9"), &memory(6000, now));
    let snapshot = sketch.snapshot(now);
    assert_eq!(snapshot.entries.len(), 2, "//old aged out");
    assert_eq!(sketch.stats().entries, [1, 0, 1], "and was dropped");
    let bytes = snapshot.encode();
    let decoded = Snapshot::decode(&bytes).unwrap();
    assert_eq!(decoded, snapshot);
    let (_, entry) = decoded
        .entries
        .iter()
        .find(|(key, _)| *key == target("//new", "Y"))
        .unwrap();
    assert_eq!(entry.samples, 1);
    assert_eq!(entry.max_memory_kb, 123_456);
    assert_eq!(entry.max_cpu_millicores, 2500);
    assert_eq!(entry.max_disk_kb, 77);
    assert_eq!(entry.killed_at_kb, 100_000);
    assert_eq!(entry.last_seen_s, now);
    assert!(!entry.seeded);

    let fresh = self::sketch(policy(0, vec![]), small_budget());
    assert_eq!(fresh.load(decoded).unwrap(), 2);
    // The command key's first sighting was probation; one sample counted.
    assert_eq!(fresh.lookup(&command("cc-9")).unwrap().samples, 1);

    // Truncated is an error, not a partial load.
    assert!(Snapshot::decode(&bytes[..bytes.len() - 3]).is_err());
    // Another sampler generation is refused.
    let mut other = sketch.snapshot(now);
    other.sampler_generation = SAMPLER_GENERATION + 1;
    let fresh = self::sketch(policy(0, vec![]), small_budget());
    assert!(fresh.load(other).is_err());
    assert_eq!(fresh.stats().entries, [0, 0, 0]);
}

/// A seed stands until the first real sample of its key, and never
/// replaces one.
#[test]
fn a_seed_yields_to_the_first_sample_and_never_replaces_one() {
    let sketch = sketch(policy(0, vec![]), small_budget());
    let key = target("//a", "X");
    assert!(sketch.seed(key.clone(), Entry::seeded(1000, 500_000, 0, 1, 1)));
    let entry = sketch.lookup(&key).unwrap();
    assert!(entry.seeded);
    assert_eq!(sketch.shape(&entry).memory_kb, 500_000);
    assert_eq!(sketch.shape(&entry).cpu_count, 1000);
    sketch.observe(key.clone(), &memory(2_000_000, 2));
    let entry = sketch.lookup(&key).unwrap();
    assert!(!entry.seeded);
    assert_eq!(entry.samples, 2, "the seed's sample count carries");
    assert_eq!(sketch.shape(&entry).memory_kb, 2_000_000);
    assert!(
        !sketch.seed(key, Entry::seeded(1, 1, 1, 1, 1)),
        "a measured key is never reseeded"
    );
}

/// The budget divides into families and the admin listing shapes every
/// entry, most recently used first.
#[test]
fn the_budget_and_the_listing() {
    let budget = Budget::from_bytes(128 << 20);
    assert_eq!(budget.target + budget.mnemonic + budget.command, 335_544);
    assert_eq!(budget.target, 268_436);
    assert_eq!(budget.probation, 335_540);
    let sketch = sketch(
        policy(0, vec![class("s", 1000, 2_000_000, 0)]),
        small_budget(),
    );
    sketch.observe(target("//a", "X"), &memory(1000, 1));
    sketch.observe(
        SketchKey::for_attempt(None, Some("Link"), "unused"),
        &memory(2000, 2),
    );
    let rows = sketch.hints(None, 10);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].key, target("//a", "X"));
    assert_eq!(rows[0].shaped.class.as_deref(), Some("s"));
    assert_eq!(rows[0].shaped.memory_kb, 2_000_000);
    assert_eq!(sketch.hints(Some(Family::Mnemonic), 10).len(), 1);
    assert_eq!(sketch.hints(Some(Family::Command), 10).len(), 0);
    assert_eq!(sketch.hints(None, 1).len(), 1);
}
