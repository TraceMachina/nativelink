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

//! What each kind of action used, learned by the scheduler from the usage
//! every attempt reports, and the reservation it shapes from that.
//!
//! The sketch is bounded by construction: a fixed-size entry per key, three
//! LRU families with capacities from a byte budget, and a window after which
//! a key not seen again is forgotten. A sample counts for between half a
//! window and a whole one (two half-window histograms per entry), so what a
//! key used last week stops shaping it, and a kill floor expires with the
//! window. The families are split over
//! [`SHARDS`] by key hash so a snapshot or an age-out never holds a lock the
//! Execute path waits on for long. Nothing on the Execute path reads a file
//! or parses JSON; a lookup is a hash probe and a 25-bucket scan.

use core::hash::{BuildHasher, BuildHasherDefault};
use core::num::NonZeroUsize;
use core::time::Duration;
use std::hash::DefaultHasher;
use std::sync::Arc;

use lru::LruCache;
use nativelink_config::schedulers::SizeClassSpec;
use nativelink_error::{Code, Error, make_err};
use parking_lot::Mutex;

/// Buckets of the log2 histograms, the rollup's `hist_bucket`: bucket 0 for
/// values under 1000, bucket `i` for `1000 * 2^(i-1) <= v < 1000 * 2^i`.
pub const HIST_BUCKETS: usize = 25;

/// Bumped by hand when the worker's sampler changes what a measurement
/// means (the process-group fix of v1.8.2 is the example). A snapshot of
/// another generation is dropped at load and the sketch starts empty.
pub const SAMPLER_GENERATION: u32 = 1;

/// Memory one entry costs, with its key and the map's bookkeeping. The
/// budget divides by this.
pub const ENTRY_BYTES: u64 = 400;

/// Shards the families are split over by key hash. A lookup or a usage
/// report locks one; a snapshot and an age-out lock one at a time, so at
/// the full budget neither holds a lock the Execute path waits on for more
/// than a shard's worth of entries.
pub const SHARDS: usize = 32;

/// Default of `memory_budget_bytes`: about 300k keys.
pub const DEFAULT_MEMORY_BUDGET_BYTES: u64 = 128 << 20;

/// Default of `window_days`.
pub const DEFAULT_WINDOW_DAYS: u64 = 7;

/// Default of `headroom_percent`, the chart's `headroomPercent`.
pub const DEFAULT_HEADROOM_PERCENT: u64 = 20;

/// What a measurement is filed under, chosen from what the client sent.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub enum SketchKey {
    /// The client named a target; `mnemonic` is `None` when it named no
    /// mnemonic, which is its own key.
    Target {
        target: Arc<str>,
        mnemonic: Option<Arc<str>>,
    },
    /// The client named a mnemonic and no target.
    Mnemonic { mnemonic: Arc<str> },
    /// The client named neither. The `Command` digest as `hash-size`: an
    /// identity every client has, and the same program with the same
    /// arguments regardless of its inputs, so one key gathers every run.
    Command { digest: Arc<str> },
}

impl SketchKey {
    /// The key for an attempt: the target when the client sent one, else
    /// the mnemonic, else the command digest. Empty strings count as absent.
    pub fn for_attempt(target: Option<&str>, mnemonic: Option<&str>, command_digest: &str) -> Self {
        let target = target.filter(|s| !s.is_empty());
        let mnemonic = mnemonic.filter(|s| !s.is_empty());
        match (target, mnemonic) {
            (Some(target), mnemonic) => Self::Target {
                target: Arc::from(target),
                mnemonic: mnemonic.map(Arc::from),
            },
            (None, Some(mnemonic)) => Self::Mnemonic {
                mnemonic: Arc::from(mnemonic),
            },
            (None, None) => Self::Command {
                digest: Arc::from(command_digest),
            },
        }
    }

    pub const fn family(&self) -> Family {
        match self {
            Self::Target { .. } => Family::Target,
            Self::Mnemonic { .. } => Family::Mnemonic,
            Self::Command { .. } => Family::Command,
        }
    }
}

/// The three maps; each has its own capacity so a flood of nameless
/// actions cannot evict a single target key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Target,
    Mnemonic,
    Command,
}

impl Family {
    pub const ALL: [Self; 3] = [Self::Target, Self::Mnemonic, Self::Command];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Target => "target",
            Self::Mnemonic => "mnemonic",
            Self::Command => "command",
        }
    }

    const fn index(self) -> usize {
        match self {
            Self::Target => 0,
            Self::Mnemonic => 1,
            Self::Command => 2,
        }
    }
}

/// A log2 histogram with the rollup's buckets, so a snapshot and a rollup
/// row say the same thing about the same samples. Counts saturate at
/// `u16::MAX`: a key with more samples than that in half a window is sized
/// the same either way, and the entry stays within its budget.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hist(pub [u16; HIST_BUCKETS]);

impl Hist {
    /// The bucket a value falls in.
    pub fn bucket(value: u64) -> usize {
        if value < 1000 {
            return 0;
        }
        let exponent = usize::try_from((value / 1000).ilog2()).unwrap_or(HIST_BUCKETS) + 1;
        exponent.min(HIST_BUCKETS - 1)
    }

    pub fn add(&mut self, value: u64) {
        let bucket = &mut self.0[Self::bucket(value)];
        *bucket = bucket.saturating_add(1);
    }

    pub fn total(&self) -> u64 {
        self.0.iter().map(|&n| u64::from(n)).sum()
    }

    /// Upper bound of the bucket the 95th percentile sample falls in; zero
    /// when empty. The upper bound over-reserves by less than 2x against
    /// the true p95; a lower edge or a midpoint could under-reserve, which
    /// a hint must never do.
    pub fn p95_upper(&self) -> u64 {
        self.p95_upper_with(&Self::default())
    }

    /// The same over the samples of `self` and `other` together.
    pub fn p95_upper_with(&self, other: &Self) -> u64 {
        let total = self.total() + other.total();
        if total == 0 {
            return 0;
        }
        let target = (total * 95).div_ceil(100);
        let mut seen = 0u64;
        for (i, (&a, &b)) in self.0.iter().zip(other.0.iter()).enumerate() {
            seen += u64::from(a) + u64::from(b);
            if seen >= target {
                return 1000u64 << i;
            }
        }
        0
    }

    fn add_all(&mut self, other: &Self) {
        for (mine, theirs) in self.0.iter_mut().zip(other.0.iter()) {
            *mine = mine.saturating_add(*theirs);
        }
    }
}

/// The samples of one half-window: histograms and the exact maxima.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Half {
    /// `peak_memory_kb` of every sampled attempt.
    pub memory: Hist,
    /// Millicores: `cpu_time_ms * 1000 / wall_time_ms`.
    pub cpu: Hist,
    /// `peak_disk_kb`; zero until workers measure it.
    pub disk: Hist,
    pub max_memory_kb: u64,
    pub max_cpu_millicores: u64,
    pub max_disk_kb: u64,
}

impl Half {
    fn add_all(&mut self, other: &Self) {
        self.memory.add_all(&other.memory);
        self.cpu.add_all(&other.cpu);
        self.disk.add_all(&other.disk);
        self.max_memory_kb = self.max_memory_kb.max(other.max_memory_kb);
        self.max_cpu_millicores = self.max_cpu_millicores.max(other.max_cpu_millicores);
        self.max_disk_kb = self.max_disk_kb.max(other.max_disk_kb);
    }
}

/// What the sketch knows about one key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub samples: u32,
    /// The samples of the current half-window and of the one before it. A
    /// lookup reads both, so a sample counts for between half a window and
    /// a whole one and never longer: what a key used last week stops
    /// shaping it, however often the key runs.
    pub current: Half,
    pub previous: Half,
    /// The largest reservation an attempt was killed at for memory within
    /// the window. The next hint sits above it.
    pub killed_at_kb: u64,
    /// When that kill was seen; the floor expires a window after it.
    pub killed_at_s: u64,
    /// Seconds since the Unix epoch of the last sample, for the window.
    pub last_seen_s: u64,
    /// The half-window `current` counts: `now_s / (window_s / 2)`; see
    /// [`Entry::settle`].
    pub period: u64,
    /// Set by the hints-file seed; replaced whole by the first real sample.
    pub seeded: bool,
}

/// One attempt's measurement as the sketch takes it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Observation {
    pub peak_memory_kb: u64,
    /// `None` when the worker reported no wall time; `Some(0)` is a real
    /// measurement of no CPU and is kept (the reservation it ran under is
    /// not a measurement).
    pub cpu_millicores: Option<u64>,
    pub peak_disk_kb: u64,
    /// The memory the attempt was killed at, when it was killed for memory
    /// and the reservation is known. A kill at an unknown reservation
    /// records no floor: a floor of zero is no floor.
    pub killed_at_kb: Option<u64>,
    /// Any kill, for the probation rule.
    pub killed: bool,
    pub now_s: u64,
}

/// A reservation shaped from an entry. Zero is a dimension not stated.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shaped {
    /// In the unit of `cpu_property_name`, millicores on the chart.
    pub cpu_count: u64,
    pub memory_kb: u64,
    pub disk_kb: u64,
    pub class: Option<String>,
}

impl Shaped {
    pub const fn is_empty(&self) -> bool {
        self.cpu_count == 0 && self.memory_kb == 0 && self.disk_kb == 0
    }
}

/// The numbers `ShapeOne` takes today, from the scheduler's own spec.
#[derive(Clone, Debug)]
pub struct Policy {
    pub headroom_percent: u64,
    pub classes: Vec<SizeClassSpec>,
    /// Which dimensions the fleet reserves: a number on one it does not
    /// would be a minimum no worker advertises.
    pub reserve_cpu: bool,
    pub reserve_memory: bool,
    pub reserve_disk: bool,
}

/// Entries per family from the byte budget, split 80/10/10, with ten
/// probation slots per command entry (each costs a tenth of an entry).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    pub target: usize,
    pub mnemonic: usize,
    pub command: usize,
    pub probation: usize,
}

impl Budget {
    pub fn from_bytes(bytes: u64) -> Self {
        let entries = usize::try_from(bytes / ENTRY_BYTES).unwrap_or(usize::MAX);
        let tenth = entries / 10;
        Self {
            target: (entries - 2 * tenth).max(1),
            mnemonic: tenth.max(1),
            command: tenth.max(1),
            probation: (tenth * 10).max(1),
        }
    }
}

/// Counters the admin view and the metrics read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub entries: [usize; 3],
    pub capacity: [usize; 3],
    pub evictions: [u64; 3],
    pub samples: [u64; 3],
    pub probation: usize,
}

struct Inner {
    families: [LruCache<SketchKey, Entry>; 3],
    probation: LruCache<Arc<str>, ()>,
    evictions: [u64; 3],
    samples: [u64; 3],
}

/// The sketch: three LRU families per shard, each shard under its own lock
/// held for O(1) work or one shard's copy, never across an await.
pub struct ResourceSketch {
    shards: Box<[Mutex<Inner>]>,
    hasher: BuildHasherDefault<DefaultHasher>,
    policy: Policy,
    window: Duration,
    budget: Budget,
}

impl core::fmt::Debug for ResourceSketch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ResourceSketch")
            .field("budget", &self.budget)
            .field("window", &self.window)
            .field("shards", &self.shards.len())
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

fn cache(capacity: usize) -> LruCache<SketchKey, Entry> {
    LruCache::new(NonZeroUsize::new(capacity.max(1)).unwrap_or(NonZeroUsize::MIN))
}

impl ResourceSketch {
    pub fn new(policy: Policy, budget: Budget, window: Duration) -> Self {
        Self::with_shards(policy, budget, window, SHARDS)
    }

    /// `shards` is [`SHARDS`] in the scheduler; a test that wants one LRU
    /// order passes one. A family's capacity is spread over the shards by
    /// key hash, so it evicts a little before the whole budget is full.
    pub fn with_shards(policy: Policy, budget: Budget, window: Duration, shards: usize) -> Self {
        let shards = shards.max(1);
        let per = |capacity: usize| capacity.div_ceil(shards).max(1);
        let inners = (0..shards)
            .map(|_| {
                Mutex::new(Inner {
                    families: [
                        cache(per(budget.target)),
                        cache(per(budget.mnemonic)),
                        cache(per(budget.command)),
                    ],
                    probation: LruCache::new(
                        NonZeroUsize::new(per(budget.probation)).unwrap_or(NonZeroUsize::MIN),
                    ),
                    evictions: [0; 3],
                    samples: [0; 3],
                })
            })
            .collect();
        Self {
            shards: inners,
            hasher: BuildHasherDefault::default(),
            policy,
            window,
            budget,
        }
    }

    fn shard(&self, key: &SketchKey) -> &Mutex<Inner> {
        let hash = self.hasher.hash_one(key);
        let index = usize::try_from(hash % self.shards.len() as u64).unwrap_or(0);
        &self.shards[index]
    }

    const fn window_s(&self) -> u64 {
        self.window.as_secs()
    }

    pub const fn policy(&self) -> &Policy {
        &self.policy
    }

    pub const fn budget(&self) -> Budget {
        self.budget
    }

    pub const fn window(&self) -> Duration {
        self.window
    }

    /// Records one attempt. A command key with no entry yet earns one on a
    /// kill or on its second sighting: a digest that ran once and was not
    /// killed is an action-cache hit next time, so a first sighting only
    /// goes on probation. An attempt that names a target and a mnemonic is
    /// recorded under both its target key and the mnemonic family, so a
    /// target the sketch has not seen is sized from its kind of action
    /// rather than the cold start. Returns whether a sample was recorded.
    pub fn observe(&self, key: SketchKey, observation: &Observation) -> bool {
        let mnemonic = match &key {
            SketchKey::Target {
                mnemonic: Some(mnemonic),
                ..
            } => Some(mnemonic.clone()),
            _ => None,
        };
        let recorded = self.observe_one(key, observation);
        if recorded && let Some(mnemonic) = mnemonic {
            self.observe_one(SketchKey::Mnemonic { mnemonic }, observation);
        }
        recorded
    }

    fn observe_one(&self, key: SketchKey, observation: &Observation) -> bool {
        let family = key.family();
        let window_s = self.window_s();
        let mut inner = self.shard(&key).lock();
        if family == Family::Command
            && !inner.families[family.index()].contains(&key)
            && !observation.killed
        {
            let SketchKey::Command { digest } = &key else {
                unreachable!("family Command is a Command key");
            };
            if inner.probation.pop(digest).is_none() {
                inner.probation.put(digest.clone(), ());
                return false;
            }
        }
        inner.samples[family.index()] += 1;
        let map = &mut inner.families[family.index()];
        if let Some(entry) = map.get_mut(&key) {
            entry.record(observation, window_s);
            return true;
        }
        let mut entry = Entry::default();
        entry.record(observation, window_s);
        if let Some((_evicted_key, _)) = map.push(key, entry) {
            inner.evictions[family.index()] += 1;
        }
        true
    }

    /// Inserts a seeded entry when the key has none. Seeds never replace
    /// what was measured.
    pub fn seed(&self, key: SketchKey, entry: Entry) -> bool {
        let family = key.family();
        let mut inner = self.shard(&key).lock();
        let map = &mut inner.families[family.index()];
        if map.contains(&key) {
            return false;
        }
        if map.push(key, entry).is_some() {
            inner.evictions[family.index()] += 1;
        }
        true
    }

    /// The entry under `key`, promoted to recently used. A key not seen
    /// for the window is dropped here as well as on the age-out tick, so a
    /// lookup between ticks does not size from it. A seed stands until its
    /// first sample or its eviction.
    pub fn lookup(&self, key: &SketchKey, now_s: u64) -> Option<Entry> {
        let cutoff = now_s.saturating_sub(self.window_s());
        let mut inner = self.shard(key).lock();
        let map = &mut inner.families[key.family().index()];
        let entry = map.get(key)?;
        if !entry.seeded && entry.last_seen_s < cutoff {
            map.pop(key);
            return None;
        }
        Some(entry.clone())
    }

    /// The reservation for an entry: `ShapeOne` of the hints producer,
    /// line for line.
    pub fn shape(&self, entry: &Entry) -> Shaped {
        shape(&self.policy, entry)
    }

    pub fn stats(&self) -> Stats {
        let mut stats = Stats::default();
        for shard in &self.shards {
            let inner = shard.lock();
            stats.probation += inner.probation.len();
            for family in Family::ALL {
                let i = family.index();
                stats.entries[i] += inner.families[i].len();
                stats.capacity[i] += inner.families[i].cap().get();
                stats.evictions[i] += inner.evictions[i];
                stats.samples[i] += inner.samples[i];
            }
        }
        stats
    }

    /// Ages every entry to `now_s` (see [`Entry::settle`]) and drops the
    /// keys not seen for the window, one shard at a time. Runs on the
    /// scheduler's tick whether or not a snapshot store is configured.
    /// Returns how many keys were dropped.
    pub fn age_out(&self, now_s: u64) -> usize {
        let window_s = self.window_s();
        let cutoff = now_s.saturating_sub(window_s);
        let mut dropped = 0usize;
        for shard in &self.shards {
            let mut inner = shard.lock();
            for family in Family::ALL {
                let map = &mut inner.families[family.index()];
                let mut stale = Vec::new();
                for (key, entry) in map.iter_mut() {
                    if !entry.seeded && entry.last_seen_s < cutoff {
                        stale.push(key.clone());
                    } else {
                        entry.settle(now_s, window_s);
                    }
                }
                dropped += stale.len();
                for key in stale {
                    map.pop(&key);
                }
            }
        }
        dropped
    }

    /// Every entry still in the window, least recently used first within
    /// each shard, so a loader that inserts in order ends with the same
    /// recency. One shard is locked at a time, for its copy only.
    pub fn snapshot(&self, now_s: u64) -> Snapshot {
        let cutoff = now_s.saturating_sub(self.window_s());
        let mut entries = Vec::new();
        for shard in &self.shards {
            let inner = shard.lock();
            for family in Family::ALL {
                let map = &inner.families[family.index()];
                let start = entries.len();
                entries.extend(
                    map.iter()
                        .filter(|(_, entry)| entry.seeded || entry.last_seen_s >= cutoff)
                        .map(|(key, entry)| (key.clone(), entry.clone())),
                );
                // `iter` is most recently used first.
                entries[start..].reverse();
            }
        }
        Snapshot {
            sampler_generation: SAMPLER_GENERATION,
            written_at_s: now_s,
            entries,
        }
    }

    /// Loads a snapshot into the sketch, merging a key the sketch already
    /// holds (another replica's view of the same key: counts add, maxima
    /// keep the larger) and skipping a key not seen for the window as of
    /// `now_s`. A snapshot of another sampler generation is refused.
    /// Returns the entries loaded or merged.
    pub fn load(&self, snapshot: Snapshot, now_s: u64) -> Result<usize, Error> {
        if snapshot.sampler_generation != SAMPLER_GENERATION {
            return Err(make_err!(
                Code::FailedPrecondition,
                "resource sketch snapshot is of sampler generation {}, this build is {}",
                snapshot.sampler_generation,
                SAMPLER_GENERATION
            ));
        }
        let window_s = self.window_s();
        let cutoff = now_s.saturating_sub(window_s);
        let mut loaded = 0usize;
        for (key, entry) in snapshot.entries {
            if !entry.seeded && entry.last_seen_s < cutoff {
                continue;
            }
            let family = key.family();
            let mut inner = self.shard(&key).lock();
            let map = &mut inner.families[family.index()];
            if let Some(existing) = map.get_mut(&key) {
                existing.merge(&entry, window_s);
            } else if map.push(key, entry).is_some() {
                inner.evictions[family.index()] += 1;
            }
            loaded += 1;
        }
        Ok(loaded)
    }

    /// The shaped hints of every entry of `family` (every family when
    /// `None`), most recently seen first, at most `limit`.
    pub fn hints(&self, family: Option<Family>, limit: usize) -> Vec<HintRow> {
        let mut rows = Vec::new();
        for shard in &self.shards {
            let inner = shard.lock();
            for f in Family::ALL {
                if family.is_some_and(|wanted| wanted != f) {
                    continue;
                }
                for (key, entry) in inner.families[f.index()].iter().take(limit) {
                    rows.push(HintRow {
                        key: key.clone(),
                        shaped: shape(&self.policy, entry),
                        samples: entry.samples,
                        last_seen_s: entry.last_seen_s,
                        seeded: entry.seeded,
                    });
                }
            }
        }
        rows.sort_by_key(|row| core::cmp::Reverse(row.last_seen_s));
        rows.truncate(limit);
        rows
    }
}

/// One line of the admin view: a key with what the scheduler reserves for
/// it now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HintRow {
    pub key: SketchKey,
    pub shaped: Shaped,
    pub samples: u32,
    pub last_seen_s: u64,
    pub seeded: bool,
}

impl Entry {
    pub fn max_memory_kb(&self) -> u64 {
        self.current.max_memory_kb.max(self.previous.max_memory_kb)
    }

    pub fn max_cpu_millicores(&self) -> u64 {
        self.current
            .max_cpu_millicores
            .max(self.previous.max_cpu_millicores)
    }

    pub fn max_disk_kb(&self) -> u64 {
        self.current.max_disk_kb.max(self.previous.max_disk_kb)
    }

    pub fn memory_p95_upper(&self) -> u64 {
        self.current.memory.p95_upper_with(&self.previous.memory)
    }

    pub fn cpu_p95_upper(&self) -> u64 {
        self.current.cpu.p95_upper_with(&self.previous.cpu)
    }

    pub fn disk_p95_upper(&self) -> u64 {
        self.current.disk.p95_upper_with(&self.previous.disk)
    }

    /// Every sample of the key still counted, both halves together.
    pub fn memory_samples(&self) -> u64 {
        self.current.memory.total() + self.previous.memory.total()
    }

    fn half_s(window_s: u64) -> u64 {
        (window_s / 2).max(1)
    }

    /// Ages the entry to `now_s`. When the half-window turns, the current
    /// samples become the previous ones and the previous ones are gone; two
    /// turns at once clear both. So a sample counts for between half a
    /// window and a whole one, and a rarely seen key keeps its last few
    /// samples that long and no longer. The kill floor expires a window
    /// after the kill. A seed is left as it is: it stands until its first
    /// sample.
    pub fn settle(&mut self, now_s: u64, window_s: u64) {
        if self.seeded {
            return;
        }
        let period = now_s / Self::half_s(window_s);
        if period > self.period {
            if period == self.period + 1 {
                self.previous = core::mem::take(&mut self.current);
            } else {
                self.previous = Half::default();
                self.current = Half::default();
            }
            self.period = period;
        }
        if self.killed_at_kb > 0 && now_s.saturating_sub(self.killed_at_s) > window_s {
            self.killed_at_kb = 0;
            self.killed_at_s = 0;
        }
    }

    fn record(&mut self, observation: &Observation, window_s: u64) {
        if self.seeded {
            // The seed stood in for measurements; the first one replaces it
            // whole, so a seed above the truth neither caps nor lifts it.
            *self = Self::default();
        }
        if self.period == 0 {
            self.period = observation.now_s / Self::half_s(window_s);
        }
        self.settle(observation.now_s, window_s);
        self.samples = self.samples.saturating_add(1);
        self.last_seen_s = self.last_seen_s.max(observation.now_s);
        let half = &mut self.current;
        if observation.peak_memory_kb > 0 {
            half.memory.add(observation.peak_memory_kb);
            half.max_memory_kb = half.max_memory_kb.max(observation.peak_memory_kb);
        }
        if let Some(millicores) = observation.cpu_millicores {
            half.cpu.add(millicores);
            half.max_cpu_millicores = half.max_cpu_millicores.max(millicores);
        }
        if observation.peak_disk_kb > 0 {
            half.disk.add(observation.peak_disk_kb);
            half.max_disk_kb = half.max_disk_kb.max(observation.peak_disk_kb);
        }
        if let Some(killed_at_kb) = observation.killed_at_kb
            && killed_at_kb > 0
            && killed_at_kb >= self.killed_at_kb
        {
            self.killed_at_kb = killed_at_kb;
            self.killed_at_s = observation.now_s;
        }
        self.seeded = false;
    }

    /// Folds `other`, another replica's view of the same key, into this
    /// entry: both are aged to the later of the two, counts add half by
    /// half, the maxima and the kill floor keep the larger. A measured
    /// entry wins over a seed.
    fn merge(&mut self, other: &Self, window_s: u64) {
        if other.seeded {
            return;
        }
        if self.seeded {
            *self = other.clone();
            return;
        }
        let now_s = self.last_seen_s.max(other.last_seen_s);
        let mut other = other.clone();
        self.settle(now_s, window_s);
        other.settle(now_s, window_s);
        self.samples = self.samples.saturating_add(other.samples);
        self.current.add_all(&other.current);
        self.previous.add_all(&other.previous);
        if other.killed_at_kb > self.killed_at_kb {
            self.killed_at_kb = other.killed_at_kb;
            self.killed_at_s = other.killed_at_s;
        }
        self.last_seen_s = now_s;
    }

    /// An entry standing in for a hint from a file: one sample at each
    /// stated number, so the first real sample takes over.
    pub fn seeded(cpu: u64, memory_kb: u64, disk_kb: u64, samples: u32, last_seen_s: u64) -> Self {
        let mut entry = Self {
            samples: samples.max(1),
            last_seen_s,
            seeded: true,
            ..Self::default()
        };
        if memory_kb > 0 {
            entry.current.memory.add(memory_kb);
            entry.current.max_memory_kb = memory_kb;
        }
        if cpu > 0 {
            entry.current.cpu.add(cpu);
            entry.current.max_cpu_millicores = cpu;
        }
        if disk_kb > 0 {
            entry.current.disk.add(disk_kb);
            entry.current.max_disk_kb = disk_kb;
        }
        entry
    }
}

/// The p95 bucket's upper bound, capped by the exact maximum: an entry with
/// one sample reserves what that sample used, one with a thousand less than
/// twice the true p95 and never less than it.
const fn p95_capped(upper: u64, max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    if upper == 0 || max < upper {
        max
    } else {
        upper
    }
}

const fn with_headroom(value: u64, headroom_percent: u64) -> u64 {
    if value == 0 {
        return 0;
    }
    value.saturating_mul(100 + headroom_percent) / 100
}

/// The first class on the ladder that holds the numbers on every dimension
/// it names and sits above a reservation the action was killed at. Past the
/// top of the ladder it is the top class, unless the action was killed at
/// or above the top's memory: no class then sits above the kill, and the
/// raw numbers stand rather than a class whose memory would be killed again.
fn smallest_dominating_class(
    classes: &[SizeClassSpec],
    memory_kb: u64,
    cpu: u64,
    disk_kb: u64,
    killed_at_kb: u64,
) -> Option<&SizeClassSpec> {
    let top = classes.last()?;
    if top.memory_kb <= killed_at_kb {
        return None;
    }
    for class in classes {
        if class.memory_kb < memory_kb || class.memory_kb <= killed_at_kb {
            continue;
        }
        if class.cpu_count > 0 && cpu > 0 && class.cpu_count < cpu {
            continue;
        }
        if class.disk_kb > 0 && disk_kb > 0 && class.disk_kb < disk_kb {
            continue;
        }
        return Some(class);
    }
    Some(top)
}

fn shape(policy: &Policy, entry: &Entry) -> Shaped {
    let headroom = policy.headroom_percent;
    let mut memory_kb = with_headroom(
        p95_capped(entry.memory_p95_upper(), entry.max_memory_kb()),
        headroom,
    );
    if entry.killed_at_kb > 0 && memory_kb <= entry.killed_at_kb {
        // Killed at that reservation: the next one must be larger, however
        // small the samples that survived were, by the same headroom a
        // measurement gets.
        memory_kb = with_headroom(entry.killed_at_kb, headroom) + 1;
    }
    // A CPU p95 of zero is an action that used no measurable CPU or a
    // worker that reported none; neither keeps the action off a small
    // class, so no CPU need is stated and the class goes by memory.
    let mut cpu = with_headroom(
        p95_capped(entry.cpu_p95_upper(), entry.max_cpu_millicores()),
        headroom,
    );
    let mut disk_kb = with_headroom(
        p95_capped(entry.disk_p95_upper(), entry.max_disk_kb()),
        headroom,
    );
    // A number on a dimension the fleet does not reserve says nothing about
    // the ones it does, so it is dropped before anything is decided from
    // it: a disk-only measurement is no measurement, and the cold start
    // applies, rather than the smallest class.
    if !policy.reserve_cpu {
        cpu = 0;
    }
    if !policy.reserve_memory {
        memory_kb = 0;
    }
    if !policy.reserve_disk {
        disk_kb = 0;
    }
    if memory_kb == 0 && cpu == 0 && disk_kb == 0 {
        return Shaped::default();
    }
    let mut class = None;
    if let Some(found) =
        smallest_dominating_class(&policy.classes, memory_kb, cpu, disk_kb, entry.killed_at_kb)
    {
        class = Some(found.name.clone());
        if found.memory_kb > 0 {
            memory_kb = found.memory_kb;
        }
        if found.cpu_count > 0 {
            cpu = found.cpu_count;
        }
        if found.disk_kb > 0 {
            disk_kb = found.disk_kb;
        }
    }
    Shaped {
        cpu_count: if policy.reserve_cpu { cpu } else { 0 },
        memory_kb: if policy.reserve_memory { memory_kb } else { 0 },
        disk_kb: if policy.reserve_disk { disk_kb } else { 0 },
        class,
    }
}

/// What each replica writes every `snapshot_interval_s` under its own key
/// and a start reads from every replica, merged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub sampler_generation: u32,
    pub written_at_s: u64,
    /// Least recently used first within a shard, so a loader that inserts
    /// in order ends with the same recency.
    pub entries: Vec<(SketchKey, Entry)>,
}

const SNAPSHOT_MAGIC: &[u8; 4] = b"NLHS";
const SNAPSHOT_VERSION: u8 = 2;

impl Snapshot {
    /// A compact binary form: histograms as their non-zero buckets, since
    /// most of the 25 are zero. No compression and no dependency; the budget
    /// bounds the size.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32 + self.entries.len() * 160);
        out.extend_from_slice(SNAPSHOT_MAGIC);
        out.push(SNAPSHOT_VERSION);
        out.extend_from_slice(&self.sampler_generation.to_le_bytes());
        out.extend_from_slice(&self.written_at_s.to_le_bytes());
        out.extend_from_slice(
            &u64::try_from(self.entries.len())
                .unwrap_or(u64::MAX)
                .to_le_bytes(),
        );
        for (key, entry) in &self.entries {
            match key {
                SketchKey::Target { target, mnemonic } => {
                    out.push(0);
                    put_str(&mut out, target);
                    match mnemonic {
                        Some(mnemonic) => {
                            out.push(1);
                            put_str(&mut out, mnemonic);
                        }
                        None => out.push(0),
                    }
                }
                SketchKey::Mnemonic { mnemonic } => {
                    out.push(1);
                    put_str(&mut out, mnemonic);
                }
                SketchKey::Command { digest } => {
                    out.push(2);
                    put_str(&mut out, digest);
                }
            }
            out.extend_from_slice(&entry.samples.to_le_bytes());
            for half in [&entry.current, &entry.previous] {
                for hist in [&half.memory, &half.cpu, &half.disk] {
                    put_hist(&mut out, hist);
                }
                for value in [
                    half.max_memory_kb,
                    half.max_cpu_millicores,
                    half.max_disk_kb,
                ] {
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
            for value in [
                entry.killed_at_kb,
                entry.killed_at_s,
                entry.last_seen_s,
                entry.period,
            ] {
                out.extend_from_slice(&value.to_le_bytes());
            }
            out.push(u8::from(entry.seeded));
        }
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut r = Reader { bytes, at: 0 };
        if r.take(4)? != SNAPSHOT_MAGIC {
            return Err(make_err!(
                Code::InvalidArgument,
                "resource sketch snapshot: not a snapshot"
            ));
        }
        let version = r.u8()?;
        if version != SNAPSHOT_VERSION {
            return Err(make_err!(
                Code::InvalidArgument,
                "resource sketch snapshot: version {version}, this build reads {SNAPSHOT_VERSION}"
            ));
        }
        let sampler_generation = r.u32()?;
        let written_at_s = r.u64()?;
        let count = usize::try_from(r.u64()?)
            .map_err(|_| make_err!(Code::InvalidArgument, "resource sketch snapshot: count"))?;
        let mut entries = Vec::with_capacity(count.min(1 << 20));
        for _ in 0..count {
            let key = match r.u8()? {
                0 => {
                    let target = r.str()?;
                    let mnemonic = if r.u8()? == 1 { Some(r.str()?) } else { None };
                    SketchKey::Target { target, mnemonic }
                }
                1 => SketchKey::Mnemonic { mnemonic: r.str()? },
                2 => SketchKey::Command { digest: r.str()? },
                tag => {
                    return Err(make_err!(
                        Code::InvalidArgument,
                        "resource sketch snapshot: key tag {tag}"
                    ));
                }
            };
            let samples = r.u32()?;
            let current = r.half()?;
            let previous = r.half()?;
            let killed_at_kb = r.u64()?;
            let killed_at_s = r.u64()?;
            let last_seen_s = r.u64()?;
            let period = r.u64()?;
            let seeded = r.u8()? == 1;
            entries.push((
                key,
                Entry {
                    samples,
                    current,
                    previous,
                    killed_at_kb,
                    killed_at_s,
                    last_seen_s,
                    period,
                    seeded,
                },
            ));
        }
        if r.at != bytes.len() {
            return Err(make_err!(
                Code::InvalidArgument,
                "resource sketch snapshot: {} trailing bytes",
                bytes.len() - r.at
            ));
        }
        Ok(Self {
            sampler_generation,
            written_at_s,
            entries,
        })
    }
}

fn put_str(out: &mut Vec<u8>, s: &str) {
    let len = u16::try_from(s.len()).unwrap_or(u16::MAX);
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(&s.as_bytes()[..usize::from(len)]);
}

fn put_hist(out: &mut Vec<u8>, hist: &Hist) {
    let nonzero = hist.0.iter().filter(|&&n| n > 0).count();
    out.push(u8::try_from(nonzero).unwrap_or(u8::MAX));
    for (i, &n) in hist.0.iter().enumerate() {
        if n > 0 {
            out.push(u8::try_from(i).unwrap_or(u8::MAX));
            out.extend_from_slice(&n.to_le_bytes());
        }
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], Error> {
        let end = self
            .at
            .checked_add(n)
            .filter(|&end| end <= self.bytes.len());
        let Some(end) = end else {
            return Err(make_err!(
                Code::InvalidArgument,
                "resource sketch snapshot: truncated at byte {}",
                self.at
            ));
        };
        let out = &self.bytes[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, Error> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, Error> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self) -> Result<u64, Error> {
        let b = self.take(8)?;
        let mut arr = [0u8; 8];
        arr.copy_from_slice(b);
        Ok(u64::from_le_bytes(arr))
    }

    fn str(&mut self) -> Result<Arc<str>, Error> {
        let len = usize::from(self.u16()?);
        let b = self.take(len)?;
        core::str::from_utf8(b)
            .map(Arc::from)
            .map_err(|_| make_err!(Code::InvalidArgument, "resource sketch snapshot: utf8"))
    }

    fn half(&mut self) -> Result<Half, Error> {
        Ok(Half {
            memory: self.hist()?,
            cpu: self.hist()?,
            disk: self.hist()?,
            max_memory_kb: self.u64()?,
            max_cpu_millicores: self.u64()?,
            max_disk_kb: self.u64()?,
        })
    }

    fn hist(&mut self) -> Result<Hist, Error> {
        let nonzero = usize::from(self.u8()?);
        if nonzero > HIST_BUCKETS {
            return Err(make_err!(
                Code::InvalidArgument,
                "resource sketch snapshot: {nonzero} histogram buckets"
            ));
        }
        let mut hist = Hist::default();
        for _ in 0..nonzero {
            let i = usize::from(self.u8()?);
            let n = self.u16()?;
            if i >= HIST_BUCKETS {
                return Err(make_err!(
                    Code::InvalidArgument,
                    "resource sketch snapshot: histogram bucket {i}"
                ));
            }
            hist.0[i] = n;
        }
        Ok(hist)
    }
}
