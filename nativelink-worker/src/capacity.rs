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

//! What this worker can see of its own CPU and memory, and what it should
//! advertise from that.
//!
//! Every deployment so far typed the advertisement by hand next to the pod's
//! limits, and the two drifted: chinchilla advertised 16 cores and 60 GiB
//! inside 15-core, 56 GiB limits. The limit is the number the kernel
//! enforces, so the advertisement is derived from it here.

use core::hash::BuildHasher;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nativelink_config::cas_server::{
    CapacityConfig, CpuUnit, MemoryEnforcement, ResourceEnforcementConfig, WorkerProperty,
};
use nativelink_error::{Code, Error, make_err};
use tracing::{info, warn};

/// CPU and memory as the cgroup (or, failing a limit, the host) reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedCapacity {
    pub cpu_millicores: u64,
    pub memory_kb: u64,
}

/// `cpu.max` is `"<quota> <period>"` in microseconds, or `"max <period>"` when
/// unlimited; unlimited means the host's cores.
pub fn parse_cpu_max(cpu_max: &str, host_cpus: u64) -> Option<u64> {
    let mut fields = cpu_max.split_whitespace();
    let quota = fields.next()?;
    if quota == "max" {
        return Some(host_cpus.saturating_mul(1000));
    }
    let quota: u64 = quota.parse().ok()?;
    let period: u64 = fields.next()?.parse().ok()?;
    if period == 0 {
        return None;
    }
    Some(quota.saturating_mul(1000) / period)
}

/// `memory.max` is bytes, or `"max"` when unlimited; unlimited means the
/// host's memory.
pub fn parse_memory_max(memory_max: &str, host_memory_kb: u64) -> Option<u64> {
    let value = memory_max.trim();
    if value == "max" {
        return Some(host_memory_kb);
    }
    value.parse::<u64>().ok().map(|bytes| bytes / 1024)
}

/// `MemTotal` from `/proc/meminfo`, in KiB.
pub fn parse_meminfo_total_kb(meminfo: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix("MemTotal:")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

/// The headroom the advertisement is divided by: the block's own figure
/// when set, else the enforcement headroom while memory enforcement is on,
/// so the two numbers cannot drift apart, else nothing.
pub fn memory_headroom_percent(
    config: &CapacityConfig,
    enforcement: Option<&ResourceEnforcementConfig>,
) -> u64 {
    config.memory_headroom_percent.unwrap_or_else(|| {
        enforcement
            .filter(|enforcement| enforcement.memory == MemoryEnforcement::Soft)
            .map_or(0, |enforcement| enforcement.memory_headroom_percent)
    })
}

/// The observed capacity less the worker's own share, memory divided by
/// `memory_headroom_percent`. Returns `(cpu, memory_kb)` with the CPU on
/// the scale `cpu_unit` names: whole cores rounded down, so a 14-core pod
/// keeping one core back advertises `13`, not `13000`, to a scheduler
/// whose actions ask for `cpu_count=1`.
pub const fn advertised(
    observed: ObservedCapacity,
    config: &CapacityConfig,
    memory_headroom_percent: u64,
) -> (u64, u64) {
    let cpu_millicores = observed
        .cpu_millicores
        .saturating_sub(config.overhead_cpu_millicores);
    let cpu = match config.cpu_unit {
        CpuUnit::Cores => cpu_millicores / 1000,
        CpuUnit::Millicores => cpu_millicores,
    };
    let memory = observed
        .memory_kb
        .saturating_sub(config.overhead_memory_kb)
        .saturating_mul(100)
        / (100 + memory_headroom_percent);
    (cpu, memory)
}

/// What to do when the cgroup cannot be read: the configured properties
/// stand if they carry both numbers, since an operator who kept them has a
/// worker that still takes work; a worker with neither would register and
/// then satisfy no action that asks for CPU or memory, idling for good on
/// one warning line, so that is refused.
pub fn without_cgroup<S: BuildHasher>(
    config: &CapacityConfig,
    properties: &HashMap<String, WorkerProperty, S>,
) -> Result<(), Error> {
    let missing: Vec<&str> = [&config.cpu_property_name, &config.memory_property_name]
        .into_iter()
        .filter(|name| !properties.contains_key(name.as_str()))
        .map(String::as_str)
        .collect();
    if missing.is_empty() {
        warn!(
            "capacity is configured but the cgroup v2 root could not be read; advertising the configured platform_properties instead"
        );
        return Ok(());
    }
    Err(make_err!(
        Code::FailedPrecondition,
        "capacity is configured but the cgroup v2 root could not be read (not Linux, cgroup v1, or no permission), and platform_properties carries no {}: set them, or drop the capacity block",
        missing.join(" or ")
    ))
}

/// The cgroup v2 path of this process from `/proc/self/cgroup`: the `0::`
/// line, which is `/` inside a container with its own cgroup namespace and
/// the full `/kubepods.slice/.../cri-containerd-<id>.scope` path in one that
/// shares the host's, as a privileged container does.
pub fn parse_self_cgroup(self_cgroup: &str) -> Option<&str> {
    self_cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .map(str::trim)
        .filter(|path| path.starts_with('/'))
}

/// Where this process's own `cpu.max`, `memory.max` and `memory.current`
/// live: the mount root joined with the path from `/proc/self/cgroup` when
/// that directory exists under the mount, the mount root otherwise. A
/// container with a private cgroup namespace sees itself at the root, so its
/// path is `/` and both agree. A privileged container sees the host's tree,
/// where the root's files are the host's (or absent, as on a systemd host),
/// and its own numbers sit further down.
pub fn resolve_cgroup_dir(
    mount_root: &Path,
    self_cgroup: Option<&str>,
    is_dir: impl Fn(&Path) -> bool,
) -> PathBuf {
    if let Some(path) = self_cgroup.and_then(parse_self_cgroup) {
        let own = mount_root.join(path.trim_start_matches('/'));
        if is_dir(&own) {
            return own;
        }
    }
    mount_root.to_path_buf()
}

/// This process's cgroup directory on the live system.
fn cgroup_dir() -> PathBuf {
    let self_cgroup = std::fs::read_to_string("/proc/self/cgroup").ok();
    resolve_cgroup_dir(Path::new("/sys/fs/cgroup"), self_cgroup.as_deref(), |dir| {
        dir.join("memory.max").is_file()
    })
}

/// Reads the worker's own cgroup v2 limits. `None` where there is no such
/// cgroup to read (not Linux, cgroup v1, or no permission).
pub fn observe_cgroup() -> Option<ObservedCapacity> {
    let host_cpus = std::thread::available_parallelism().map_or(1, |n| n.get() as u64);
    let host_memory_kb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|meminfo| parse_meminfo_total_kb(&meminfo))
        .unwrap_or(0);
    let dir = cgroup_dir();
    let cpu_max = std::fs::read_to_string(dir.join("cpu.max")).ok()?;
    let memory_max = std::fs::read_to_string(dir.join("memory.max")).ok()?;
    Some(ObservedCapacity {
        cpu_millicores: parse_cpu_max(&cpu_max, host_cpus)?,
        memory_kb: parse_memory_max(&memory_max, host_memory_kb)?,
    })
}

/// Sets the CPU and memory properties to what the cgroup allows. When
/// nothing can be read, the configured properties stand if they carry both,
/// and the worker fails to start if they do not. Returns what was
/// advertised.
pub fn apply<S: BuildHasher>(
    config: &CapacityConfig,
    memory_headroom_percent: u64,
    properties: &mut HashMap<String, WorkerProperty, S>,
) -> Result<Option<(u64, u64)>, Error> {
    let Some(observed) = observe_cgroup() else {
        without_cgroup(config, properties)?;
        return Ok(None);
    };
    let (cpu, memory_kb) = advertised(observed, config, memory_headroom_percent);
    properties.insert(
        config.cpu_property_name.clone(),
        WorkerProperty::Values(vec![cpu.to_string()]),
    );
    properties.insert(
        config.memory_property_name.clone(),
        WorkerProperty::Values(vec![memory_kb.to_string()]),
    );
    info!(
        observed_cpu_millicores = observed.cpu_millicores,
        observed_memory_kb = observed.memory_kb,
        cpu,
        cpu_unit = ?config.cpu_unit,
        memory_kb,
        memory_headroom_percent,
        "Advertising capacity from the cgroup"
    );
    Ok(Some((cpu, memory_kb)))
}

pub fn parse_memory_current_kb(memory_current: &str) -> Option<u64> {
    memory_current
        .trim()
        .parse::<u64>()
        .ok()
        .map(|bytes| bytes / 1024)
}

/// `MemAvailable` from `/proc/meminfo`, in KiB.
pub fn parse_meminfo_available_kb(meminfo: &str) -> Option<u64> {
    meminfo.lines().find_map(|line| {
        let rest = line.strip_prefix("MemAvailable:")?;
        rest.split_whitespace().next()?.parse().ok()
    })
}

/// Memory the worker could still give an action: the cgroup limit less its
/// current usage when there is a limit, the host's `MemAvailable` when
/// there is not, nothing when neither can be read.
pub const fn free_memory_kb_from(
    limit_kb: Option<u64>,
    current_kb: Option<u64>,
    available_kb: Option<u64>,
) -> Option<u64> {
    match (limit_kb, current_kb) {
        (Some(limit), Some(current)) => Some(limit.saturating_sub(current)),
        _ => available_kb,
    }
}

/// What the worker reports on each keepalive. Linux only; elsewhere the
/// worker reports nothing and is never vetoed.
#[cfg(target_os = "linux")]
pub fn free_memory_kb() -> Option<u64> {
    let dir = cgroup_dir();
    let limit_kb = std::fs::read_to_string(dir.join("memory.max"))
        .ok()
        .and_then(|max| {
            let max = max.trim();
            (max != "max")
                .then(|| parse_memory_current_kb(max))
                .flatten()
        });
    let current_kb = std::fs::read_to_string(dir.join("memory.current"))
        .ok()
        .and_then(|current| parse_memory_current_kb(&current));
    let available_kb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|meminfo| parse_meminfo_available_kb(&meminfo));
    free_memory_kb_from(limit_kb, current_kb, available_kb)
}

#[cfg(not(target_os = "linux"))]
pub const fn free_memory_kb() -> Option<u64> {
    None
}
