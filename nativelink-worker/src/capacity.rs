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

//! What the worker has to spare, reported on each keepalive.

/// `memory.current` and `memory.max` are bytes.
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
