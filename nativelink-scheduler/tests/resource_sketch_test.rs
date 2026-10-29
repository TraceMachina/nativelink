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
    Budget, ENTRY_BYTES, Entry, Family, Half, Hist, Observation, Policy, ResourceSketch,
    SAMPLER_GENERATION, SketchKey, Snapshot,
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

/// The clock the tests look up at: a week in, so every sample in the tests
/// (seconds 1 to a few thousand, or NOW itself) sits inside the window.
const NOW: u64 = 7 * 86_400;

/// One shard, so the eviction order is the one LRU order of each family;
/// the scheduler uses `SHARDS`.
fn sketch(policy: Policy, budget: Budget) -> ResourceSketch {
    ResourceSketch::with_shards(policy, budget, Duration::from_hours(7 * 24), 1)
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
    let shaped = sketch.shape(&sketch.lookup(&key, NOW).unwrap());
    assert_eq!(shaped.memory_kb, 3_600_000);
    assert_eq!(shaped.cpu_count, 0, "no cpu measured");
    assert!(shaped.class.is_none());
    // A thousand samples at 3M: the bucket's bound is 4M, the max 3M.
    for _ in 0..1000 {
        sketch.observe(key.clone(), &memory(3_000_000, 1));
    }
    assert_eq!(
        sketch.shape(&sketch.lookup(&key, NOW).unwrap()).memory_kb,
        3_600_000
    );
    // One at 3.9M moves the max, not the bucket.
    sketch.observe(key.clone(), &memory(3_900_000, 1));
    assert_eq!(
        sketch.shape(&sketch.lookup(&key, NOW).unwrap()).memory_kb,
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
    let shaped = sketch.shape(&sketch.lookup(&key, NOW).unwrap());
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
    let shaped = sketch.shape(&sketch.lookup(&key, NOW).unwrap());
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
    let shaped = sketch.shape(&sketch.lookup(&key, NOW).unwrap());
    assert!(shaped.is_empty(), "{shaped:?}");
    sketch.observe(key.clone(), &memory(500_000, 2));
    let shaped = sketch.shape(&sketch.lookup(&key, NOW).unwrap());
    assert_eq!(shaped.class.as_deref(), Some("s"));
    assert_eq!(shaped.disk_kb, 0);
}

/// A command key earns an entry on its second sighting or on a kill; a
/// digest that ran once and was not killed is an action-cache hit next time.
#[test]
fn a_command_key_is_on_probation_until_seen_twice_or_killed() {
    let sketch = sketch(policy(0, vec![]), small_budget());
    assert!(!sketch.observe(command("aa-1"), &memory(1000, 1)));
    assert!(sketch.lookup(&command("aa-1"), NOW).is_none());
    assert_eq!(sketch.stats().probation, 1);
    assert!(sketch.observe(command("aa-1"), &memory(1000, 2)));
    assert_eq!(sketch.lookup(&command("aa-1"), NOW).unwrap().samples, 1);
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
        sketch.lookup(&command("bb-1"), NOW).is_some(),
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
    assert!(
        sketch.lookup(&target("//a", "X"), NOW).is_some(),
        "promotes //a"
    );
    sketch.observe(target("//d", "X"), &memory(1000, 2));
    assert!(
        sketch.lookup(&target("//b", "X"), NOW).is_none(),
        "//b was the oldest"
    );
    assert!(sketch.lookup(&target("//a", "X"), NOW).is_some());
    let stats = sketch.stats();
    // The four attempts named mnemonic X, so the mnemonic family holds it.
    assert_eq!(stats.entries, [3, 1, 0]);
    assert_eq!(stats.evictions, [1, 0, 0]);
    assert_eq!(stats.samples, [4, 4, 0]);
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
    assert_eq!(
        snapshot.entries.len(),
        3,
        "//old left out, the mnemonics in"
    );
    assert_eq!(sketch.stats().entries, [2, 2, 1], "nothing dropped yet");
    assert_eq!(sketch.age_out(now), 2, "//old and its mnemonic X dropped");
    assert_eq!(sketch.stats().entries, [1, 1, 1]);
    assert_eq!(sketch.snapshot(now), snapshot);
    let bytes = snapshot.encode();
    let decoded = Snapshot::decode(&bytes, usize::MAX).unwrap();
    assert_eq!(decoded, snapshot);
    let (_, entry) = decoded
        .entries
        .iter()
        .find(|(key, _)| *key == target("//new", "Y"))
        .unwrap();
    assert_eq!(entry.samples, 1);
    assert_eq!(entry.current.max_memory_kb, 123_456);
    assert_eq!(entry.current.max_cpu_millicores, 2500);
    assert_eq!(entry.current.max_disk_kb, 77);
    assert_eq!(entry.previous, Half::default());
    assert_eq!(entry.killed_at_kb, 100_000);
    assert_eq!(entry.killed_at_s, now);
    assert_eq!(entry.last_seen_s, now);
    assert_eq!(entry.period, now / (7 * 86_400 / 2));
    assert!(!entry.seeded);

    let fresh = self::sketch(policy(0, vec![]), small_budget());
    assert_eq!(fresh.load(decoded.clone(), now).unwrap(), 3);
    // The command key's first sighting was probation; one sample counted.
    assert_eq!(fresh.lookup(&command("cc-9"), now).unwrap().samples, 1);
    // Replaying the same owned snapshot does not count samples again.
    assert_eq!(fresh.load(decoded, now).unwrap(), 0);
    assert_eq!(fresh.lookup(&command("cc-9"), now).unwrap().samples, 1);
    assert_eq!(
        fresh
            .lookup(&target("//new", "Y"), now)
            .unwrap()
            .memory_samples(),
        1
    );

    // Truncated is an error, not a partial load.
    assert!(Snapshot::decode(&bytes[..bytes.len() - 3], usize::MAX).is_err());
    // Another sampler generation is refused.
    let mut other = sketch.snapshot(now);
    other.sampler_generation = SAMPLER_GENERATION + 1;
    let fresh = self::sketch(policy(0, vec![]), small_budget());
    assert!(fresh.load(other, now).is_err());
    assert_eq!(fresh.stats().entries, [0, 0, 0]);
}

/// A seed stands until the first real sample of its key, which replaces it
/// whole (an oversized seed must neither cap nor lift what is measured),
/// and a seed never replaces a measurement.
#[test]
fn a_seed_yields_to_the_first_sample_and_never_replaces_one() {
    let sketch = sketch(policy(0, vec![]), small_budget());
    let key = target("//a", "X");
    assert!(sketch.seed(key.clone(), Entry::seeded(1000, 500_000_000, 0, 1, 1)));
    let entry = sketch.lookup(&key, NOW).unwrap();
    assert!(entry.seeded);
    assert_eq!(sketch.shape(&entry).memory_kb, 500_000_000);
    assert_eq!(sketch.shape(&entry).cpu_count, 1000);
    // A seed does not age out: it stands until its first sample.
    assert_eq!(sketch.age_out(NOW + 30 * 86_400), 0);
    sketch.observe(key.clone(), &memory(2_000_000, 2));
    let entry = sketch.lookup(&key, NOW).unwrap();
    assert!(!entry.seeded);
    assert_eq!(entry.samples, 1, "the seed is gone, not merged");
    assert_eq!(
        entry.max_memory_kb(),
        2_000_000,
        "the seed's maximum with it"
    );
    assert_eq!(sketch.shape(&entry).memory_kb, 2_000_000);
    assert_eq!(sketch.shape(&entry).cpu_count, 0, "the seed's cpu with it");
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
    assert!(budget.target > budget.mnemonic);
    assert_eq!(budget.mnemonic, budget.command);
    let empty = ResourceSketch::new(policy(0, vec![]), budget, Duration::from_hours(1));
    assert!(empty.charged_bytes() + empty.snapshot_byte_limit() * 3 <= 128 << 20);
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
    // The target, its mnemonic X, and Link; most recently seen first.
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows[0].key,
        SketchKey::for_attempt(None, Some("Link"), "unused")
    );
    let a = rows
        .iter()
        .find(|row| row.key == target("//a", "X"))
        .unwrap();
    assert_eq!(a.shaped.class.as_deref(), Some("s"));
    assert_eq!(a.shaped.memory_kb, 2_000_000);
    assert_eq!(sketch.hints(Some(Family::Mnemonic), 10).len(), 2);
    assert_eq!(sketch.hints(Some(Family::Command), 10).len(), 0);
    assert_eq!(sketch.hints(None, 1).len(), 1);
    // The scheduler's own shard count spreads a family's capacity.
    let sharded = ResourceSketch::new(policy(0, vec![]), small_budget(), Duration::from_hours(1));
    assert_eq!(sharded.stats().capacity, [3, 2, 2]);
}

/// A sample counts for between half a window and a whole one, never
/// longer: what a key used last week stops shaping it however often it
/// runs, a kill floor expires with the window, and a lookup past the window
/// finds nothing.
#[test]
fn samples_leave_with_the_window_and_the_kill_floor_expires() {
    let window = Duration::from_hours(7 * 24);
    let sketch = ResourceSketch::with_shards(policy(0, vec![]), small_budget(), window, 1);
    let key = target("//a", "X");
    let day = 86_400u64;
    // Day 100 is half-window 28 (3.5 days each).
    let start = 100 * day;
    for _ in 0..100 {
        sketch.observe(key.clone(), &memory(7_000_000, start));
    }
    sketch.observe(
        key.clone(),
        &Observation {
            peak_memory_kb: 7_500_000,
            killed_at_kb: Some(8_000_000),
            killed: true,
            now_s: start,
            ..Observation::default()
        },
    );
    let entry = sketch.lookup(&key, start).unwrap();
    assert_eq!(sketch.shape(&entry).memory_kb, 8_000_001, "above the kill");
    // Three days on, the next half-window: a few small samples, and the
    // big ones still count.
    for i in 0..3u64 {
        sketch.observe(key.clone(), &memory(1_000_000, start + 3 * day + i));
    }
    let entry = sketch.lookup(&key, start + 3 * day).unwrap();
    assert_eq!(entry.current.memory.total(), 3);
    assert_eq!(entry.previous.memory.total(), 101);
    assert_eq!(
        sketch.shape(&entry).memory_kb,
        8_000_001,
        "the kill still holds"
    );
    // Eight days on: two half-windows past the big samples and a window
    // past the kill. The tick ages the key; the small samples are what is
    // left, and the maximum left with the big ones.
    assert_eq!(sketch.age_out(start + 8 * day), 0);
    let entry = sketch.lookup(&key, start + 8 * day).unwrap();
    assert_eq!(entry.killed_at_kb, 0, "the floor expired");
    assert_eq!(entry.previous.memory.total(), 3);
    assert_eq!(entry.current, Half::default());
    assert_eq!(entry.max_memory_kb(), 1_000_000);
    assert_eq!(
        sketch.shape(&entry).memory_kb,
        1_000_000,
        "capped by the exact maximum"
    );
    // A sample a week old is gone whatever came after it: a key seen once
    // a week sizes from its latest sample.
    let rare = target("//rare", "X");
    sketch.observe(rare.clone(), &memory(9_000_000, start));
    sketch.observe(rare.clone(), &memory(1_000, start + 7 * day + 1));
    assert_eq!(
        sketch
            .shape(&sketch.lookup(&rare, start + 7 * day + 1).unwrap())
            .memory_kb,
        1_000
    );
    // Past the window the key is dropped by a lookup, not sized from.
    assert!(sketch.lookup(&key, start + 16 * day).is_none());
    assert_eq!(sketch.stats().entries[0], 1, "only //rare is left");
}

/// An attempt that names a target and a mnemonic teaches the mnemonic
/// family too, so a target never seen is sized from its kind of action.
#[test]
fn a_named_attempt_teaches_its_mnemonic() {
    let sketch = sketch(policy(0, vec![]), small_budget());
    sketch.observe(target("//a", "Javac"), &memory(3_000_000, 1));
    sketch.observe(target("//b", "Javac"), &memory(1_000_000, 2));
    let javac = SketchKey::for_attempt(None, Some("Javac"), "unused");
    let entry = sketch.lookup(&javac, NOW).unwrap();
    assert_eq!(entry.samples, 2);
    assert_eq!(entry.max_memory_kb(), 3_000_000);
    assert_eq!(sketch.stats().samples, [2, 2, 0]);
    // A key without a mnemonic teaches no mnemonic; a mnemonic-only key
    // counts once.
    sketch.observe(
        SketchKey::for_attempt(Some("//c"), None, "unused"),
        &memory(1_000, 3),
    );
    sketch.observe(javac.clone(), &memory(1_000, 4));
    assert_eq!(sketch.stats().samples, [3, 3, 0]);
    assert_eq!(sketch.lookup(&javac, NOW).unwrap().samples, 3);
}

/// Independent u16 saturation previously turned a 0.4% tail into the p95.
#[test]
fn busy_histogram_preserves_the_distribution_and_snapshot_counts() {
    let sketch = sketch(policy(0, vec![]), small_budget());
    let key = target("//busy", "X");
    for _ in 0..1_000_000 {
        sketch.observe(key.clone(), &memory(1000, NOW));
    }
    for _ in 0..4000 {
        sketch.observe(key.clone(), &memory(8_000_000, NOW));
    }
    let entry = sketch.lookup(&key, NOW).unwrap();
    assert_eq!(entry.memory_samples(), 1_004_000);
    assert_eq!(entry.memory_p95_upper(), 2000);
    assert_eq!(sketch.shape(&entry).memory_kb, 2000);
    let bytes = sketch.encode_snapshot(NOW);
    let restored = self::sketch(policy(0, vec![]), small_budget());
    restored
        .load(Snapshot::decode(&bytes, usize::MAX).unwrap(), NOW)
        .unwrap();
    assert_eq!(restored.lookup(&key, NOW).unwrap(), entry);
}

/// Charge long keys and probation as well as entries. Encoding must stay
/// within its preallocated allowance rather than cloning the full sketch.
#[test]
fn memory_budget_covers_long_keys_probation_and_snapshot_buffer() {
    let limit = 1 << 20;
    let sketch = ResourceSketch::new(
        policy(0, vec![]),
        Budget::from_bytes(limit),
        Duration::from_hours(24),
    );
    assert!(usize::try_from(ENTRY_BYTES).unwrap() > size_of::<Entry>() + size_of::<SketchKey>());
    for i in 0..5000 {
        sketch.observe(
            target(&format!("//{}:{i}", "x".repeat(2048)), "X"),
            &memory(1000, NOW),
        );
        sketch.observe(
            command(&format!("{i}-{}", "c".repeat(64))),
            &memory(1000, NOW),
        );
    }
    assert!(sketch.stats().evictions[0] > 0);
    assert!(sketch.stats().probation > 0);
    let bytes = sketch.encode_snapshot(NOW);
    assert!(bytes.len() <= bytes.capacity());
    assert_eq!(bytes.capacity() as u64, sketch.snapshot_byte_limit());
    assert!(sketch.charged_bytes() + 3 * bytes.capacity() as u64 <= limit);
    assert_eq!(
        Snapshot::decode(&bytes, usize::MAX).unwrap(),
        sketch.snapshot(NOW)
    );
    let too_long = target(&"x".repeat(usize::try_from(limit).unwrap()), "X");
    assert!(!sketch.observe(too_long.clone(), &memory(1000, NOW)));
    assert!(sketch.lookup(&too_long, NOW).is_none());
    assert!(sketch.charged_bytes() + 3 * bytes.capacity() as u64 <= limit);
}

/// Exercise the largest serialized histograms rather than only sparse ones.
#[test]
fn dense_snapshot_fits_the_reserved_buffer() {
    let sketch = sketch(policy(0, vec![]), small_budget());
    let key = target("//dense", "X");
    let mut entry = Entry::default();
    entry.current.memory = Hist([u64::MAX; 25]);
    entry.current.cpu = entry.current.memory;
    entry.current.disk = entry.current.memory;
    entry.previous = entry.current;
    entry.last_seen_s = NOW;
    assert_eq!(entry.memory_samples(), u64::MAX);
    assert_eq!(entry.memory_p95_upper(), 1000 << 23);
    assert!(sketch.seed(key, entry));
    let bytes = sketch.encode_snapshot(NOW);
    assert_eq!(bytes.capacity() as u64, sketch.snapshot_byte_limit());
    assert_eq!(
        Snapshot::decode(&bytes, usize::MAX).unwrap(),
        sketch.snapshot(NOW)
    );
}

/// Sparse encoding and an untrusted count must not allocate an unbounded
/// array of full histograms before the receiving sketch applies its budget.
#[test]
fn snapshot_decode_rejects_entry_counts_above_the_budget() {
    let sketch = sketch(policy(0, vec![]), small_budget());
    for i in 0..3 {
        sketch.observe(target(&format!("//target-{i}"), "X"), &memory(1000, NOW));
    }
    let mut bytes = sketch.encode_snapshot(NOW);
    assert_eq!(Snapshot::decode(&bytes, 4).unwrap().entries.len(), 4);
    assert!(Snapshot::decode(&bytes, 3).is_err());
    // The old narrow-counter format must not be interpreted as wide counters.
    bytes[4] = 2;
    assert!(Snapshot::decode(&bytes, 4).is_err());
    bytes[4] = 3;
    bytes[17..25].copy_from_slice(&u64::MAX.to_le_bytes());
    assert!(Snapshot::decode(&bytes, usize::MAX).is_err());
}
