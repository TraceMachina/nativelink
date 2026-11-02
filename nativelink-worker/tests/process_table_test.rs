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

use core::hint::black_box;
use core::time::Duration;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

use nativelink_macro::nativelink_test;
use nativelink_worker::process_table::{
    ProcessTableCache, live_process_table, process_exists, read_process_cpu_ticks,
    read_process_memory_kb, ticks_to_millis, worker_pgid,
};
use nativelink_worker::running_actions_manager::{ProcEntry, action_membership_live};

/// A child that idles for longer than any test here runs, spawned the way
/// an action is: as its own process group where the platform has them.
fn spawn_idle_child() -> Child {
    #[cfg(target_family = "unix")]
    let mut command = {
        use std::os::unix::process::CommandExt;
        let mut command = Command::new("sleep");
        command.arg("30").process_group(0);
        command
    };
    #[cfg(target_family = "windows")]
    let mut command = {
        let mut command = Command::new("ping");
        command.args(["-n", "30", "127.0.0.1"]);
        command
    };
    command
        .stdout(Stdio::null())
        .spawn()
        .expect("the idle child spawns")
}

/// Keeps this thread busy for `duration`.
fn spin_for(duration: Duration) {
    let busy_until = std::time::Instant::now() + duration;
    let mut spin = 0u64;
    while std::time::Instant::now() < busy_until {
        spin = black_box(spin.wrapping_add(1));
    }
}

/// The scan finds this process in the worker's own group, and a child it
/// spawned as its own group with this process as its parent; the child
/// alone is an action's member, holds memory, and stops existing once it is
/// killed and waited for.
#[test]
fn the_table_links_a_spawned_child_to_its_parent() {
    let mut child = spawn_idle_child();
    let child_pid = child.id();
    let own_pid = std::process::id();

    let table = live_process_table();
    let find = |pid: u32| {
        table
            .iter()
            .find(|(entry, _)| entry.pid == pid)
            .map(|(entry, _)| *entry)
    };
    let own = find(own_pid).expect("the scan lists this process");
    let listed_child = find(child_pid).expect("the scan lists the child");
    let members = action_membership_live(&[child_pid]).pids;
    let child_memory_kb = read_process_memory_kb(child_pid);
    let existed = process_exists(child_pid);

    child.kill().expect("the child is killed");
    child.wait().expect("the child is waited for");
    drop(child);

    assert_eq!(own.pgrp, worker_pgid(), "{own:?}");
    #[cfg(target_family = "unix")]
    assert_eq!(own.ppid, std::os::unix::process::parent_id(), "{own:?}");
    assert_eq!(listed_child.ppid, own_pid, "{listed_child:?}");
    assert_eq!(listed_child.pgrp, child_pid, "{listed_child:?}");
    assert!(members.contains(&child_pid), "{members:?}");
    assert!(!members.contains(&own_pid), "{members:?}");
    assert!(
        child_memory_kb.is_some_and(|kb| kb > 0),
        "{child_memory_kb:?}"
    );
    assert!(existed);
    assert!(!process_exists(child_pid));
}

/// The CPU time the scan reports for this process grows by about the work
/// it does, in milliseconds once converted.
#[test]
fn cpu_time_grows_with_the_work_done() {
    let own_pid = std::process::id();
    let own_ticks = || {
        live_process_table()
            .into_iter()
            .find(|(entry, _)| entry.pid == own_pid)
            .and_then(|(_, ticks)| ticks)
            .expect("this process's CPU time is readable")
    };
    let before = own_ticks();
    spin_for(Duration::from_millis(300));
    let spent_ms = ticks_to_millis(own_ticks()) - ticks_to_millis(before);
    // Clock ticks are 10 ms on Linux and the spin shares a core with the
    // rest of the test run, so allow well under the 300 ms spun.
    assert!(spent_ms >= 100, "{spent_ms} ms");
    assert!(read_process_memory_kb(own_pid).is_some_and(|kb| kb > 0));
}

/// Samplers that ask at once while no snapshot is fresh share the one scan
/// the first of them starts, and later ones reuse it while it is young.
#[nativelink_test]
async fn concurrent_callers_share_one_scan() {
    let cache = Arc::new(ProcessTableCache::new(Duration::MAX));
    let tables = futures::future::join_all((0..8).map(|_| {
        let cache = cache.clone();
        async move { cache.current().await }
    }))
    .await;
    let first = &tables[0];
    assert!(!first.is_empty());
    for table in &tables {
        assert!(Arc::ptr_eq(first, table));
    }
    assert!(Arc::ptr_eq(first, &cache.current().await));
}

/// A snapshot older than the cache's age is never served; the next caller
/// scans again.
#[nativelink_test]
async fn a_stale_snapshot_is_scanned_again() {
    let cache = ProcessTableCache::new(Duration::ZERO);
    let first = cache.current().await;
    let second = cache.current().await;
    assert!(!Arc::ptr_eq(&first, &second));
}

/// A snapshot keeps the CPU time it was taken with, so work done since only
/// shows in a fresh read, which is why the sampler reads its members' CPU
/// time fresh.
#[nativelink_test]
async fn cpu_time_read_fresh_moves_past_a_held_snapshot() {
    let own_pid = std::process::id();
    let own_ticks = |table: &[(ProcEntry, Option<u64>)]| {
        table
            .iter()
            .find(|(entry, _)| entry.pid == own_pid)
            .and_then(|(_, ticks)| *ticks)
            .expect("this process's CPU time is in the snapshot")
    };
    let cache = ProcessTableCache::new(Duration::MAX);
    let before = own_ticks(&cache.current().await);
    spin_for(Duration::from_millis(300));
    let held = own_ticks(&cache.current().await);
    let fresh = read_process_cpu_ticks(own_pid).expect("this process's CPU time is readable");
    assert_eq!(held, before);
    let spent_ms = ticks_to_millis(fresh) - ticks_to_millis(before);
    assert!(spent_ms >= 100, "{spent_ms} ms");
}
