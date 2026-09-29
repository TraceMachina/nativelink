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

use nativelink_config::cas_server::{CapacityConfig, CpuUnit};
use nativelink_worker::capacity::{
    ObservedCapacity, advertised, free_memory_kb_from, parse_cpu_max, parse_meminfo_available_kb,
    parse_meminfo_total_kb, parse_memory_current_kb, parse_memory_max,
};
use pretty_assertions::assert_eq;

fn config(headroom: u64) -> CapacityConfig {
    config_in(CpuUnit::Cores, headroom)
}

fn config_in(cpu_unit: CpuUnit, headroom: u64) -> CapacityConfig {
    CapacityConfig {
        cpu_property_name: "cpu_count".to_string(),
        memory_property_name: "memory_kb".to_string(),
        cpu_unit,
        overhead_cpu_millicores: 1000,
        overhead_memory_kb: 4 * 1024 * 1024,
        memory_headroom_percent: headroom,
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
    assert_eq!(advertised(observed, &config(20)), (13, 40 * 1024 * 1024));
    assert_eq!(advertised(observed, &config(0)), (13, 48 * 1024 * 1024));
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
        memory_headroom_percent: 0,
    };
    assert_eq!(advertised(fourteen_cores, &default_block).0, 13);
    assert_eq!(
        advertised(fourteen_cores, &config_in(CpuUnit::Millicores, 0)).0,
        13_000
    );
    let fourteen_and_a_half = ObservedCapacity {
        cpu_millicores: 14_500,
        memory_kb: 1,
    };
    assert_eq!(advertised(fourteen_and_a_half, &config(0)).0, 13);
    assert_eq!(
        advertised(fourteen_and_a_half, &config_in(CpuUnit::Millicores, 0)).0,
        13_500
    );
}

#[test]
fn overhead_larger_than_the_limit_advertises_zero_not_a_wraparound() {
    let observed = ObservedCapacity {
        cpu_millicores: 500,
        memory_kb: 1024,
    };
    assert_eq!(advertised(observed, &config(20)), (0, 0));
}

/// The keepalive reports the limit less current usage when there is a
/// limit, and what the host has available when there is not.
#[test]
fn free_memory_is_limit_less_current_or_the_host_available() {
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
        free_memory_kb_from(Some(52 * 1024 * 1024), Some(40 * 1024 * 1024), Some(1)),
        Some(12 * 1024 * 1024)
    );
    assert_eq!(free_memory_kb_from(Some(100), Some(200), Some(1)), Some(0));
    assert_eq!(free_memory_kb_from(None, Some(200), Some(777)), Some(777));
    assert_eq!(free_memory_kb_from(None, None, None), None);
}
