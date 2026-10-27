// Copyright 2026 The NativeLink Authors. All rights reserved.
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

//! What each kind of action used, learned by the scheduler from the usage
//! every attempt reports, and the reservation it shapes from that.
//!
//! The sketch is bounded by construction: a fixed-size entry per key, three
//! LRU families with capacities from a byte budget, and a window after which
//! a key not seen again is forgotten. Nothing on the Execute path reads a
//! file or parses JSON; a lookup is a hash probe and a 25-bucket scan.

use core::num::NonZeroUsize;
use core::time::Duration;
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
/// row say the same thing about the same samples.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Hist(pub [u32; HIST_BUCKETS]);

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
        let total = self.total();
        if total == 0 {
            return 0;
        }
        let target = (total * 95).div_ceil(100);
        let mut seen = 0u64;
        for (i, &n) in self.0.iter().enumerate() {
            seen += u64::from(n);
            if seen >= target {
                return 1000u64 << i;
            }
        }
        0
    }
}

/// What the sketch knows about one key.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub samples: u32,
    /// `peak_memory_kb` of every sampled attempt.
    pub memory: Hist,
    /// Millicores: `cpu_time_ms * 1000 / wall_time_ms`.
    pub cpu: Hist,
    /// `peak_disk_kb`; zero until workers measure it.
    pub disk: Hist,
    pub max_memory_kb: u64,
    pub max_cpu_millicores: u64,
    pub max_disk_kb: u64,
    /// The largest reservation an attempt was killed at for memory. The
    /// next hint sits above it.
    pub killed_at_kb: u64,
    /// Seconds since the Unix epoch of the last sample, for the window.
    pub last_seen_s: u64,
    /// Set by the hints-file seed; cleared by the first real sample.
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
    pub fn is_empty(&self) -> bool {
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

/// The sketch: three LRU families under one lock held for O(1) work, never
/// across an await.
pub struct ResourceSketch {
    inner: Mutex<Inner>,
    policy: Policy,
    window: Duration,
    budget: Budget,
}

impl core::fmt::Debug for ResourceSketch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ResourceSketch")
            .field("budget", &self.budget)
            .field("window", &self.window)
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

fn cache(capacity: usize) -> LruCache<SketchKey, Entry> {
    LruCache::new(NonZeroUsize::new(capacity.max(1)).unwrap_or(NonZeroUsize::MIN))
}

impl ResourceSketch {
    pub fn new(policy: Policy, budget: Budget, window: Duration) -> Self {
        Self {
            inner: Mutex::new(Inner {
                families: [
                    cache(budget.target),
                    cache(budget.mnemonic),
                    cache(budget.command),
                ],
                probation: LruCache::new(
                    NonZeroUsize::new(budget.probation.max(1)).unwrap_or(NonZeroUsize::MIN),
                ),
                evictions: [0; 3],
                samples: [0; 3],
            }),
            policy,
            window,
            budget,
        }
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
    /// goes on probation. Returns whether a sample was recorded.
    pub fn observe(&self, key: SketchKey, observation: &Observation) -> bool {
        let family = key.family();
        let mut inner = self.inner.lock();
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
            entry.record(observation);
            return true;
        }
        let mut entry = Entry::default();
        entry.record(observation);
        if let Some((_evicted_key, _)) = map.push(key, entry) {
            inner.evictions[family.index()] += 1;
        }
        true
    }

    /// Inserts a seeded entry when the key has none. Seeds never replace
    /// what was measured.
    pub fn seed(&self, key: SketchKey, entry: Entry) -> bool {
        let family = key.family();
        let mut inner = self.inner.lock();
        let map = &mut inner.families[family.index()];
        if map.contains(&key) {
            return false;
        }
        if map.push(key, entry).is_some() {
            inner.evictions[family.index()] += 1;
        }
        true
    }

    /// The entry under `key`, promoted to recently used.
    pub fn lookup(&self, key: &SketchKey) -> Option<Entry> {
        let mut inner = self.inner.lock();
        inner.families[key.family().index()].get(key).cloned()
    }

    /// The reservation for an entry: `ShapeOne` of the hints producer,
    /// line for line.
    pub fn shape(&self, entry: &Entry) -> Shaped {
        shape(&self.policy, entry)
    }

    pub fn stats(&self) -> Stats {
        let inner = self.inner.lock();
        let mut stats = Stats {
            evictions: inner.evictions,
            samples: inner.samples,
            probation: inner.probation.len(),
            ..Stats::default()
        };
        for family in Family::ALL {
            stats.entries[family.index()] = inner.families[family.index()].len();
            stats.capacity[family.index()] = inner.families[family.index()].cap().get();
        }
        stats
    }

    /// Every entry still in the window, oldest first, and the aged-out
    /// ones dropped on the way. The lock is held for the copy, not for
    /// whatever the caller does with it.
    pub fn snapshot(&self, now_s: u64) -> Snapshot {
        let cutoff = now_s.saturating_sub(self.window.as_secs());
        let mut inner = self.inner.lock();
        let mut entries = Vec::new();
        for family in Family::ALL {
            let map = &mut inner.families[family.index()];
            let stale: Vec<SketchKey> = map
                .iter()
                .filter(|(_, entry)| entry.last_seen_s < cutoff)
                .map(|(key, _)| key.clone())
                .collect();
            for key in stale {
                map.pop(&key);
            }
            // `iter` is most recently used first; the loader puts entries
            // back in the order given, so reverse to keep the recency.
            entries.extend(map.iter().map(|(key, entry)| (key.clone(), entry.clone())));
        }
        entries.reverse();
        Snapshot {
            sampler_generation: SAMPLER_GENERATION,
            written_at_s: now_s,
            entries,
        }
    }

    /// Loads a snapshot into an empty sketch. A snapshot of another sampler
    /// generation is refused. Returns the entries loaded.
    pub fn load(&self, snapshot: Snapshot) -> Result<usize, Error> {
        if snapshot.sampler_generation != SAMPLER_GENERATION {
            return Err(make_err!(
                Code::FailedPrecondition,
                "resource sketch snapshot is of sampler generation {}, this build is {}",
                snapshot.sampler_generation,
                SAMPLER_GENERATION
            ));
        }
        let mut inner = self.inner.lock();
        let mut loaded = 0usize;
        for (key, entry) in snapshot.entries {
            let family = key.family();
            if inner.families[family.index()].push(key, entry).is_some() {
                inner.evictions[family.index()] += 1;
            }
            loaded += 1;
        }
        Ok(loaded)
    }

    /// The shaped hints of every entry of `family` (every family when
    /// `None`), most recently used first, at most `limit`.
    pub fn hints(&self, family: Option<Family>, limit: usize) -> Vec<HintRow> {
        let inner = self.inner.lock();
        let mut rows = Vec::new();
        for f in Family::ALL {
            if family.is_some_and(|wanted| wanted != f) {
                continue;
            }
            for (key, entry) in inner.families[f.index()].iter() {
                if rows.len() >= limit {
                    return rows;
                }
                rows.push(HintRow {
                    key: key.clone(),
                    shaped: shape(&self.policy, entry),
                    samples: entry.samples,
                    last_seen_s: entry.last_seen_s,
                    seeded: entry.seeded,
                });
            }
        }
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
    fn record(&mut self, observation: &Observation) {
        self.samples = self.samples.saturating_add(1);
        self.last_seen_s = self.last_seen_s.max(observation.now_s);
        if observation.peak_memory_kb > 0 {
            self.memory.add(observation.peak_memory_kb);
            self.max_memory_kb = self.max_memory_kb.max(observation.peak_memory_kb);
        }
        if let Some(millicores) = observation.cpu_millicores {
            self.cpu.add(millicores);
            self.max_cpu_millicores = self.max_cpu_millicores.max(millicores);
        }
        if observation.peak_disk_kb > 0 {
            self.disk.add(observation.peak_disk_kb);
            self.max_disk_kb = self.max_disk_kb.max(observation.peak_disk_kb);
        }
        if let Some(killed_at_kb) = observation.killed_at_kb
            && killed_at_kb > 0
        {
            self.killed_at_kb = self.killed_at_kb.max(killed_at_kb);
        }
        self.seeded = false;
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
            entry.memory.add(memory_kb);
            entry.max_memory_kb = memory_kb;
        }
        if cpu > 0 {
            entry.cpu.add(cpu);
            entry.max_cpu_millicores = cpu;
        }
        if disk_kb > 0 {
            entry.disk.add(disk_kb);
            entry.max_disk_kb = disk_kb;
        }
        entry
    }
}

/// The p95 bucket's upper bound, capped by the exact maximum: an entry with
/// one sample reserves what that sample used, one with a thousand less than
/// twice the true p95 and never less than it.
fn p95_capped(hist: &Hist, max: u64) -> u64 {
    if max == 0 {
        return 0;
    }
    let upper = hist.p95_upper();
    if upper == 0 || max < upper {
        max
    } else {
        upper
    }
}

fn with_headroom(value: u64, headroom_percent: u64) -> u64 {
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
    let mut memory_kb = with_headroom(p95_capped(&entry.memory, entry.max_memory_kb), headroom);
    if entry.killed_at_kb > 0 && memory_kb <= entry.killed_at_kb {
        // Killed at that reservation: the next one must be larger, however
        // small the samples that survived were, by the same headroom a
        // measurement gets.
        memory_kb = with_headroom(entry.killed_at_kb, headroom) + 1;
    }
    // A CPU p95 of zero is an action that used no measurable CPU or a
    // worker that reported none; neither keeps the action off a small
    // class, so no CPU need is stated and the class goes by memory.
    let mut cpu = with_headroom(p95_capped(&entry.cpu, entry.max_cpu_millicores), headroom);
    let mut disk_kb = with_headroom(p95_capped(&entry.disk, entry.max_disk_kb), headroom);
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

/// What is written every `snapshot_interval_s` and read once at start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub sampler_generation: u32,
    pub written_at_s: u64,
    /// Least recently used first, so a loader that inserts in order ends
    /// with the same recency.
    pub entries: Vec<(SketchKey, Entry)>,
}

const SNAPSHOT_MAGIC: &[u8; 4] = b"NLHS";
const SNAPSHOT_VERSION: u8 = 1;

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
            for hist in [&entry.memory, &entry.cpu, &entry.disk] {
                put_hist(&mut out, hist);
            }
            for value in [
                entry.max_memory_kb,
                entry.max_cpu_millicores,
                entry.max_disk_kb,
                entry.killed_at_kb,
                entry.last_seen_s,
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
            let memory = r.hist()?;
            let cpu = r.hist()?;
            let disk = r.hist()?;
            let max_memory_kb = r.u64()?;
            let max_cpu_millicores = r.u64()?;
            let max_disk_kb = r.u64()?;
            let killed_at_kb = r.u64()?;
            let last_seen_s = r.u64()?;
            let seeded = r.u8()? == 1;
            entries.push((
                key,
                Entry {
                    samples,
                    memory,
                    cpu,
                    disk,
                    max_memory_kb,
                    max_cpu_millicores,
                    max_disk_kb,
                    killed_at_kb,
                    last_seen_s,
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
            let n = self.u32()?;
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
