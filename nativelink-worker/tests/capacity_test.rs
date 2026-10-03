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

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use nativelink_config::cas_server::{
    CapacityConfig, CpuUnit, DiskEnforcement, MemoryEnforcement, ResourceEnforcementConfig,
    WorkerProperty,
};
use nativelink_error::Code;
#[cfg(target_family = "unix")]
use nativelink_worker::capacity::find_limit;
use nativelink_worker::capacity::{
    ObservedCapacity, advertised, free_memory_kb_from, free_memory_kb_with,
    memory_headroom_percent, own_memory_limit_in, parse_cpu_max, parse_meminfo_available_kb,
    parse_meminfo_total_kb, parse_memory_current_kb, parse_memory_max,
    parse_memory_stat_reclaimable_kb, parse_self_cgroup, resolve_cgroup_dir, without_cgroup,
};
use pretty_assertions::assert_eq;

fn config() -> CapacityConfig {
    config_in(CpuUnit::Cores)
}

fn config_in(cpu_unit: CpuUnit) -> CapacityConfig {
    CapacityConfig {
        cpu_property_name: "cpu_count".to_string(),
        memory_property_name: "memory_kb".to_string(),
        cpu_unit,
        overhead_cpu_millicores: 1000,
        overhead_memory_kb: 4 * 1024 * 1024,
        memory_headroom_percent: None,
    }
}

fn enforcement(memory: MemoryEnforcement, headroom: u64) -> ResourceEnforcementConfig {
    ResourceEnforcementConfig {
        memory,
        disk: DiskEnforcement::None,
        disk_property_name: "disk_kb".to_string(),
        memory_property_name: "memory_kb".to_string(),
        memory_headroom_percent: headroom,
        disk_headroom_percent: 20,
    }
}

#[test]
fn cpu_max_is_quota_over_period_in_millicores() {
    assert_eq!(parse_cpu_max("1400000 100000\n", 64), Some(14_000));
    assert_eq!(parse_cpu_max("max 100000\n", 8), Some(8_000));
    assert_eq!(parse_cpu_max("garbage", 8), None);
    assert_eq!(parse_cpu_max("100000 0", 8), None);
}

#[test]
fn memory_max_is_bytes_or_the_host() {
    assert_eq!(parse_memory_max("55834574848\n", 1), Some(54_525_952));
    assert_eq!(parse_memory_max("max\n", 65_536_000), Some(65_536_000));
    assert_eq!(parse_memory_max("", 1), None);
    assert_eq!(
        parse_meminfo_total_kb("MemTotal:       65536000 kB\nMemFree: 1 kB\n"),
        Some(65_536_000)
    );
}

/// The drydock pool: a 52 GiB, 14 core pod, 4 GiB and one core kept back,
/// 20% enforcement headroom, comes out at 13 cores and 40 GiB, which is what
/// the Terraform sizing derives by hand today.
#[test]
fn advertised_takes_off_the_overhead_and_the_headroom() {
    let observed = ObservedCapacity {
        cpu_millicores: 14_000,
        memory_kb: 52 * 1024 * 1024,
    };
    assert_eq!(advertised(observed, &config(), 20), (13, 40 * 1024 * 1024));
    assert_eq!(advertised(observed, &config(), 0), (13, 48 * 1024 * 1024));
}

/// The headroom is typed once: unset, the block follows the enforcement
/// headroom while memory enforcement is on, and is nothing otherwise; set,
/// its own figure wins.
#[test]
fn headroom_follows_enforcement_unless_set() {
    let unset = config();
    assert_eq!(
        memory_headroom_percent(&unset, Some(&enforcement(MemoryEnforcement::Soft, 20))),
        20
    );
    assert_eq!(
        memory_headroom_percent(&unset, Some(&enforcement(MemoryEnforcement::None, 20))),
        0
    );
    assert_eq!(memory_headroom_percent(&unset, None), 0);
    let set = CapacityConfig {
        memory_headroom_percent: Some(10),
        ..config()
    };
    assert_eq!(
        memory_headroom_percent(&set, Some(&enforcement(MemoryEnforcement::Soft, 20))),
        10
    );
}

/// Without a readable cgroup the configured properties stand if they carry
/// both numbers; a worker that would register with neither is refused,
/// naming what is missing, rather than idling forever on a warning.
#[test]
fn without_a_cgroup_the_worker_keeps_its_properties_or_refuses_to_start() {
    let both: HashMap<String, WorkerProperty> = HashMap::from([
        (
            "cpu_count".to_string(),
            WorkerProperty::Values(vec!["16".to_string()]),
        ),
        (
            "memory_kb".to_string(),
            WorkerProperty::Values(vec!["60000000".to_string()]),
        ),
    ]);
    assert!(without_cgroup(&config(), &both).is_ok());

    let only_cpu: HashMap<String, WorkerProperty> = HashMap::from([(
        "cpu_count".to_string(),
        WorkerProperty::Values(vec!["16".to_string()]),
    )]);
    let err = without_cgroup(&config(), &only_cpu).expect_err("memory_kb is missing");
    assert_eq!(err.code, Code::FailedPrecondition);
    assert!(err.to_string().contains("no memory_kb"), "{err}");

    let neither: HashMap<String, WorkerProperty> = HashMap::new();
    let err = without_cgroup(&config(), &neither).expect_err("both are missing");
    assert!(
        err.to_string().contains("no cpu_count or memory_kb"),
        "{err}"
    );
}

/// `cpu_count` is whole cores everywhere else in the configuration (`nproc`,
/// `cpu_count=1` per action), so that is the default scale: a 14-core pod
/// with the default block advertises 13, not the 13000 that would let the
/// scheduler pack thousands of actions onto it. A partial core rounds down.
/// A fleet on thousandths of a core says so and gets the exact figure.
#[test]
fn cpu_is_advertised_in_whole_cores_unless_told_millicores() {
    let fourteen_cores = ObservedCapacity {
        cpu_millicores: 14_000,
        memory_kb: 1,
    };
    let default_block = CapacityConfig {
        cpu_property_name: "cpu_count".to_string(),
        memory_property_name: "memory_kb".to_string(),
        cpu_unit: CpuUnit::default(),
        overhead_cpu_millicores: 1000,
        overhead_memory_kb: 0,
        memory_headroom_percent: None,
    };
    assert_eq!(advertised(fourteen_cores, &default_block, 0).0, 13);
    assert_eq!(
        advertised(fourteen_cores, &config_in(CpuUnit::Millicores), 0).0,
        13_000
    );
    let fourteen_and_a_half = ObservedCapacity {
        cpu_millicores: 14_500,
        memory_kb: 1,
    };
    assert_eq!(advertised(fourteen_and_a_half, &config(), 0).0, 13);
    assert_eq!(
        advertised(fourteen_and_a_half, &config_in(CpuUnit::Millicores), 0).0,
        13_500
    );
}

#[test]
fn overhead_larger_than_the_limit_advertises_zero_not_a_wraparound() {
    let observed = ObservedCapacity {
        cpu_millicores: 500,
        memory_kb: 1024,
    };
    assert_eq!(advertised(observed, &config(), 20), (0, 0));
}

/// The keepalive reports the limit less the working set (current usage
/// less what the kernel can drop) when there is a limit, and what the host
/// has available when there is not. With no `memory.stat` to read, nothing
/// is taken off the usage.
#[test]
fn free_memory_is_limit_less_working_set_or_the_host_available() {
    assert_eq!(
        parse_memory_current_kb("10737418240\n"),
        Some(10 * 1024 * 1024)
    );
    assert_eq!(parse_memory_current_kb("max"), None);
    assert_eq!(
        parse_meminfo_available_kb("MemTotal: 65536000 kB\nMemAvailable:   12345678 kB\n"),
        Some(12_345_678)
    );
    assert_eq!(
        parse_meminfo_total_kb("MemTotal: 65536000 kB\nMemAvailable:   12345678 kB\n"),
        Some(65_536_000)
    );
    assert_eq!(
        free_memory_kb_from(
            Some(52 * 1024 * 1024),
            Some(40 * 1024 * 1024),
            None,
            Some(1)
        ),
        Some(12 * 1024 * 1024)
    );
    assert_eq!(
        free_memory_kb_from(Some(100), Some(200), None, Some(1)),
        Some(0)
    );
    assert_eq!(
        free_memory_kb_from(None, Some(200), Some(50), Some(777)),
        Some(777)
    );
    assert_eq!(free_memory_kb_from(None, None, None, None), None);
}

/// Page cache the worker's own I/O built up does not count against it. The
/// numbers are a 24 GiB worker that declined every 2 GiB action: 22.05 GiB
/// charged, 17.39 GiB of it inactive file cache, 184.7 MiB anonymous.
#[test]
fn free_memory_leaves_out_inactive_page_cache() {
    let memory_stat = "anon 193671168\nfile 22119329792\nactive_file 3446669312\ninactive_file 18672467968\nslab_reclaimable 1352925184\nfile_dirty 0\nfile_writeback 0\n";
    assert_eq!(
        parse_memory_stat_reclaimable_kb(memory_stat),
        Some(18_234_832)
    );
    let limit_kb = 25_769_803_776 / 1024;
    let current_kb = 23_680_266_240 / 1024;
    assert_eq!(
        free_memory_kb_from(Some(limit_kb), Some(current_kb), Some(18_234_832), None),
        Some(20_275_396)
    );
    // Without the cache figure the old reading stands: under 2 GiB free.
    assert_eq!(
        free_memory_kb_from(Some(limit_kb), Some(current_kb), None, None),
        Some(2_040_564)
    );
}

/// Dirty and writeback pages cannot be dropped until they reach the disk,
/// so they come off the inactive cache: a worker that just wrote its
/// outputs does not report them as free.
#[test]
fn dirty_page_cache_is_not_reclaimable() {
    let gib: u64 = 1024 * 1024 * 1024;
    let memory_stat = format!(
        "inactive_file {}\nfile_dirty {}\nfile_writeback {}\n",
        10 * gib,
        3 * gib,
        gib
    );
    assert_eq!(
        parse_memory_stat_reclaimable_kb(&memory_stat),
        Some(6 * 1024 * 1024)
    );
    // More dirty than inactive (dirty pages on the active list) is nothing
    // reclaimable, not a wraparound.
    assert_eq!(
        parse_memory_stat_reclaimable_kb("inactive_file 100\nfile_dirty 4096\n"),
        Some(0)
    );
    // A kernel without the dirty counters still reports the inactive cache.
    assert_eq!(
        parse_memory_stat_reclaimable_kb("inactive_file 2048\n"),
        Some(2)
    );
    // No inactive_file line is nothing known, not nothing reclaimable.
    assert_eq!(parse_memory_stat_reclaimable_kb("anon 1\n"), None);
    // Keys match whole: `inactive_file_extra` is not `inactive_file`.
    assert_eq!(
        parse_memory_stat_reclaimable_kb("inactive_file_extra 4096\ninactive_file 2048\n"),
        Some(2)
    );
    assert_eq!(
        parse_memory_stat_reclaimable_kb("inactive_file_extra 4096\n"),
        None
    );
}

/// The subtraction saturates: a reclaimable figure larger than the usage
/// leaves the whole limit, never more, and usage over the limit leaves
/// nothing.
#[test]
fn free_memory_never_exceeds_the_limit_or_wraps() {
    assert_eq!(
        free_memory_kb_from(Some(100), Some(40), Some(60), None),
        Some(100)
    );
    assert_eq!(
        free_memory_kb_from(Some(100), Some(200), Some(50), None),
        Some(0)
    );
}

#[test]
fn self_cgroup_is_the_v2_line() {
    assert_eq!(parse_self_cgroup("0::/\n"), Some("/"));
    assert_eq!(
        parse_self_cgroup(
            "0::/kubepods.slice/kubepods-burstable.slice/kubepods-burstable-pod5ed6.slice/cri-containerd-e0fd.scope\n"
        ),
        Some(
            "/kubepods.slice/kubepods-burstable.slice/kubepods-burstable-pod5ed6.slice/cri-containerd-e0fd.scope"
        )
    );
    // cgroup v1 lines carry a controller name and no `0::` line.
    assert_eq!(
        parse_self_cgroup("12:memory:/docker/abc\n1:name=systemd:/docker/abc\n"),
        None
    );
    assert_eq!(parse_self_cgroup(""), None);
    assert_eq!(parse_self_cgroup("0::relative\n"), None);
}

#[test]
fn cgroup_dir_is_the_own_path_under_the_mount_when_it_exists() {
    let root = Path::new("/sys/fs/cgroup");
    let scope = "/kubepods.slice/kubepods-burstable-pod5ed6.slice/cri-containerd-e0fd.scope";
    let own = PathBuf::from(
        "/sys/fs/cgroup/kubepods.slice/kubepods-burstable-pod5ed6.slice/cri-containerd-e0fd.scope",
    );
    // A privileged container sees the host's tree: its own scope is a
    // directory below the root.
    assert_eq!(
        resolve_cgroup_dir(root, Some(&format!("0::{scope}\n")), |dir| dir == own),
        own
    );
    // A container with its own cgroup namespace is at the root.
    assert_eq!(
        resolve_cgroup_dir(root, Some("0::/\n"), |dir| dir == root),
        root
    );
    // The path is not under the mount (a cgroup namespace whose root is
    // deeper than the mount shows): the root stands.
    assert_eq!(
        resolve_cgroup_dir(root, Some(&format!("0::{scope}\n")), |_| false),
        root
    );
    // Nothing readable at all: the root, as before.
    assert_eq!(resolve_cgroup_dir(root, None, |_| true), root);
}

// The walk is Linux-only code, and the test's paths are unix strings.
#[cfg(target_family = "unix")]
#[test]
fn limit_comes_from_the_nearest_limited_ancestor() {
    let root = Path::new("/sys/fs/cgroup");
    let own = root.join("kubepods.slice/kubepods-pod1.slice/cri-containerd-abc.scope");
    // The scope itself is unlimited; the pod slice above it carries the limit.
    let read = |path: &Path| -> Option<String> {
        match path.to_str()? {
            "/sys/fs/cgroup/kubepods.slice/kubepods-pod1.slice/cri-containerd-abc.scope/memory.max" => {
                Some("max\n".to_string())
            }
            "/sys/fs/cgroup/kubepods.slice/kubepods-pod1.slice/memory.max" => {
                Some("55834574848\n".to_string())
            }
            "/sys/fs/cgroup/kubepods.slice/kubepods-pod1.slice/cri-containerd-abc.scope/cpu.max" => {
                Some("max 100000\n".to_string())
            }
            "/sys/fs/cgroup/kubepods.slice/cpu.max" => Some("1400000 100000\n".to_string()),
            _ => None,
        }
    };
    assert_eq!(
        find_limit(&own, root, "memory.max", read),
        Some((
            root.join("kubepods.slice/kubepods-pod1.slice"),
            "55834574848\n".to_string()
        ))
    );
    // CPU and memory may be limited at different levels.
    assert_eq!(
        find_limit(&own, root, "cpu.max", read),
        Some((root.join("kubepods.slice"), "1400000 100000\n".to_string()))
    );
}

// The walk is Linux-only code, and the test's paths are unix strings.
#[cfg(target_family = "unix")]
#[test]
fn no_limit_at_any_level_is_no_limit() {
    let root = Path::new("/sys/fs/cgroup");
    let own = root.join("system.slice/nativelink.service");
    // A bare-metal systemd host: every level says max.
    let all_max = |_: &Path| Some("max\n".to_string());
    assert_eq!(find_limit(&own, root, "memory.max", all_max), None);
    // Nothing readable at all.
    assert_eq!(find_limit(&own, root, "memory.max", |_| None), None);
    // The walk stops at the mount root, never above it.
    let outside = |path: &Path| (path == Path::new("/sys/memory.max")).then(|| "1\n".to_string());
    assert_eq!(find_limit(&own, root, "memory.max", outside), None);
    // A directory not under the mount root is only looked at itself.
    let own_only =
        |path: &Path| (path == Path::new("/elsewhere/memory.max")).then(|| "2048\n".to_string());
    assert_eq!(
        find_limit(Path::new("/elsewhere"), root, "memory.max", own_only),
        Some((PathBuf::from("/elsewhere"), "2048\n".to_string()))
    );
}

/// A reader over fixed files, with `memory.current` answering from a list
/// in turn, so a test can move the usage between the reads.
fn fake_cgroup(currents: Vec<u64>, stat: &'static str) -> impl Fn(&Path) -> Option<String> {
    let currents = std::cell::RefCell::new(currents.into_iter());
    move |path: &Path| match path.file_name()?.to_str()? {
        "memory.current" => currents.borrow_mut().next().map(|bytes| bytes.to_string()),
        "memory.stat" => Some(stat.to_string()),
        "meminfo" => Some("MemAvailable: 777 kB\n".to_string()),
        _ => None,
    }
}

/// The usage is read on both sides of the cache and the larger taken, so
/// cache reclaimed or grown between the reads makes the worker report less
/// free, not more; with no limit the host's `MemAvailable` is reported.
#[test]
fn free_memory_reads_bracket_the_cache_and_keep_the_larger_usage() {
    let mib: u64 = 1024 * 1024;
    let limited = (PathBuf::from("/cg"), (24 * 1024 * mib).to_string());
    let stat = "inactive_file 17179869184\n"; // 16 GiB
    // Steady: 22 GiB charged, 16 GiB of it cache: 18 GiB free.
    assert_eq!(
        free_memory_kb_with(Some(&limited), fake_cgroup(vec![22 * 1024 * mib; 2], stat)),
        Some(18 * 1024 * 1024)
    );
    // Reclaimed mid-read (22 GiB, then 17): the 22 is kept, not 17 - 16.
    assert_eq!(
        free_memory_kb_with(
            Some(&limited),
            fake_cgroup(vec![22 * 1024 * mib, 17 * 1024 * mib], stat)
        ),
        Some(18 * 1024 * 1024)
    );
    // Grown mid-read (22 GiB, then 23): the 23 is kept.
    assert_eq!(
        free_memory_kb_with(
            Some(&limited),
            fake_cgroup(vec![22 * 1024 * mib, 23 * 1024 * mib], stat)
        ),
        Some(17 * 1024 * 1024)
    );
    // No limit: the host's MemAvailable.
    assert_eq!(
        free_memory_kb_with(None, fake_cgroup(vec![], stat)),
        Some(777)
    );
    // A limit but no readable usage: the host's MemAvailable too.
    assert_eq!(
        free_memory_kb_with(Some(&limited), fake_cgroup(vec![], stat)),
        Some(777)
    );
}

/// `/proc/meminfo` keys carry their own colon, and a value written right
/// after it still parses.
#[test]
fn meminfo_value_parses_with_or_without_a_space() {
    assert_eq!(
        parse_meminfo_available_kb("MemAvailable:12345678 kB\n"),
        Some(12_345_678)
    );
    assert_eq!(parse_meminfo_total_kb("MemTotal:   42 kB\n"), Some(42));
}

/// Idle admission trusts only a limit on the worker's own cgroup with
/// usage it can read: a limit on an ancestor is shared, and without usage
/// free memory falls back to the host's, and either can come back.
#[test]
fn only_an_own_limit_with_readable_usage_counts_as_limited() {
    let dir = Path::new("/cg/worker");
    let files = |max: Option<&'static str>, current: Option<&'static str>| {
        move |path: &Path| match path.file_name()?.to_str()? {
            "memory.max" => max.map(str::to_string),
            "memory.current" => current.map(str::to_string),
            _ => None,
        }
    };
    assert!(own_memory_limit_in(
        dir,
        files(Some("25769803776\n"), Some("1024\n"))
    ));
    // No limit of its own: an unlimited child of a limited ancestor.
    assert!(!own_memory_limit_in(
        dir,
        files(Some("max\n"), Some("1024\n"))
    ));
    assert!(!own_memory_limit_in(dir, files(None, Some("1024\n"))));
    // A limit, but usage it cannot read.
    assert!(!own_memory_limit_in(
        dir,
        files(Some("25769803776\n"), None)
    ));
}
