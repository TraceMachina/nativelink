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

use nativelink_config::cas_server::{CapacityConfig, WorkerProperty};
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

/// The observed capacity less the worker's own share, memory divided by the
/// enforcement headroom. Returns `(cpu_millicores, memory_kb)`.
pub const fn advertised(observed: ObservedCapacity, config: &CapacityConfig) -> (u64, u64) {
    let cpu = observed
        .cpu_millicores
        .saturating_sub(config.overhead_cpu_millicores);
    let memory = observed
        .memory_kb
        .saturating_sub(config.overhead_memory_kb)
        .saturating_mul(100)
        / (100 + config.memory_headroom_percent);
    (cpu, memory)
}

/// Reads the worker's own cgroup v2 root. `None` where there is no such
/// cgroup to read (not Linux, cgroup v1, or no permission).
pub fn observe_cgroup() -> Option<ObservedCapacity> {
    let host_cpus = std::thread::available_parallelism().map_or(1, |n| n.get() as u64);
    let host_memory_kb = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|meminfo| parse_meminfo_total_kb(&meminfo))
        .unwrap_or(0);
    let cpu_max = std::fs::read_to_string("/sys/fs/cgroup/cpu.max").ok()?;
    let memory_max = std::fs::read_to_string("/sys/fs/cgroup/memory.max").ok()?;
    Some(ObservedCapacity {
        cpu_millicores: parse_cpu_max(&cpu_max, host_cpus)?,
        memory_kb: parse_memory_max(&memory_max, host_memory_kb)?,
    })
}

/// Sets the CPU and memory properties to what the cgroup allows, or leaves
/// them alone with a warning when nothing can be read. Returns what was
/// advertised.
pub fn apply<S: BuildHasher>(
    config: &CapacityConfig,
    properties: &mut HashMap<String, WorkerProperty, S>,
) -> Option<(u64, u64)> {
    let Some(observed) = observe_cgroup() else {
        warn!(
            "capacity is configured but the cgroup v2 root could not be read; advertising the configured platform_properties instead"
        );
        return None;
    };
    let (cpu_millicores, memory_kb) = advertised(observed, config);
    properties.insert(
        config.cpu_property_name.clone(),
        WorkerProperty::Values(vec![cpu_millicores.to_string()]),
    );
    properties.insert(
        config.memory_property_name.clone(),
        WorkerProperty::Values(vec![memory_kb.to_string()]),
    );
    info!(
        observed_cpu_millicores = observed.cpu_millicores,
        observed_memory_kb = observed.memory_kb,
        cpu_millicores,
        memory_kb,
        "Advertising capacity from the cgroup"
    );
    Some((cpu_millicores, memory_kb))
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
    let limit_kb = std::fs::read_to_string("/sys/fs/cgroup/memory.max")
        .ok()
        .and_then(|max| {
            let max = max.trim();
            (max != "max")
                .then(|| parse_memory_current_kb(max))
                .flatten()
        });
    let current_kb = std::fs::read_to_string("/sys/fs/cgroup/memory.current")
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
