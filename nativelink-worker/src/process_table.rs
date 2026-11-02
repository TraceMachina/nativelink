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

//! The host's processes as the action resource sampler sees them: each
//! process's parent and process group, the CPU time it has used, and the
//! memory it holds.
//!
//! Linux reads `/proc`, macOS asks `libproc`, and Windows takes a Toolhelp
//! snapshot. Windows has no process groups, so there every process is a
//! group of its own: an action's membership follows parent links, and the
//! groups remembered from earlier samples (see
//! [`action_membership`](crate::running_actions_manager::action_membership))
//! keep a process whose parent has exited, since Windows never reparents.

use core::time::Duration;
use std::sync::Arc;

use nativelink_util::spawn_blocking;
use tokio::time::Instant;

use crate::running_actions_manager::ProcEntry;

/// Every process with the CPU ticks it has used, as [`live_process_table`]
/// returns it.
pub type ProcessTable = Arc<[(ProcEntry, Option<u64>)]>;

/// A [`live_process_table`] snapshot shared by every action sampler on this
/// worker.
///
/// Each running action samples every `RESOURCE_USAGE_SAMPLE_INTERVAL`, and
/// finding its members means reading every process on the host. Scanning
/// once per action makes the cost (running actions) x (host processes) per
/// interval, which on a busy shared host saturates dozens of cores. Samplers
/// instead reuse a snapshot younger than `max_age`, one interval, so the
/// host is scanned at most once per interval however many actions run.
/// Member memory and CPU time are still read fresh on every sample; only
/// membership can lag, by at most one interval, as it already could between
/// two samples.
/// Kills keep their own live scan
/// ([`action_membership_live`](crate::running_actions_manager::action_membership_live)).
#[derive(Debug)]
pub struct ProcessTableCache {
    max_age: Duration,
    snapshot: tokio::sync::Mutex<Option<(Instant, ProcessTable)>>,
}

impl ProcessTableCache {
    pub const fn new(max_age: Duration) -> Self {
        Self {
            max_age,
            snapshot: tokio::sync::Mutex::const_new(None),
        }
    }

    /// Returns a snapshot younger than `max_age`. Callers that arrive while
    /// a scan is running wait for it rather than starting their own.
    pub async fn current(&self) -> ProcessTable {
        let mut snapshot = self.snapshot.lock().await;
        if let Some((taken_at, table)) = snapshot.as_ref()
            && taken_at.elapsed() < self.max_age
        {
            return table.clone();
        }
        // The scan is thousands of blocking reads; keep it off the async workers.
        let table: ProcessTable = spawn_blocking!("process_table_scan", live_process_table)
            .await
            .map_or_else(|_| Arc::from(Vec::new()), Arc::from);
        *snapshot = Some((Instant::now(), table.clone()));
        table
    }
}

/// Every process on the host, with the CPU ticks each has used so far (see
/// [`ticks_to_millis`]). Blocking; a process that cannot be read is left
/// out, and its ticks are `None` when only they cannot be read.
pub fn live_process_table() -> Vec<(ProcEntry, Option<u64>)> {
    platform::live_process_table()
}

/// The memory `pid` holds, in KiB, as close as the platform gets to the
/// process's share of its resident pages: `Pss` on Linux, the physical
/// footprint on macOS, the private commit charge on Windows. `None` when
/// the process cannot be read.
pub fn read_process_memory_kb(pid: u32) -> Option<u64> {
    platform::read_process_memory_kb(pid)
}

/// The CPU ticks `pid` has used so far, read now rather than from a
/// snapshot (see [`ticks_to_millis`]). `None` when the process cannot be
/// read.
pub fn read_process_cpu_ticks(pid: u32) -> Option<u64> {
    platform::read_process_cpu_ticks(pid)
}

/// Converts the CPU ticks of [`live_process_table`] into milliseconds.
// Only the Windows conversion, a division, could be const.
#[cfg_attr(target_family = "windows", allow(clippy::missing_const_for_fn))]
pub fn ticks_to_millis(ticks: u64) -> u64 {
    platform::ticks_to_millis(ticks)
}

/// Whether `pid` is still a process on the host: on unix including one that
/// has exited but not yet been waited for, on Windows only a running one.
pub fn process_exists(pid: u32) -> bool {
    platform::process_exists(pid)
}

/// The worker's own process group, excluded from every action's members.
/// On Windows, which has no process groups, the worker's own pid.
#[cfg(target_family = "unix")]
pub fn worker_pgid() -> u32 {
    // SAFETY: `getpgrp` takes no arguments and cannot fail.
    u32::try_from(unsafe { libc::getpgrp() }).unwrap_or(0)
}

#[cfg(target_family = "windows")]
pub fn worker_pgid() -> u32 {
    std::process::id()
}

#[cfg(target_os = "linux")]
mod platform {
    use std::path::Path;

    use crate::running_actions_manager::{
        ProcEntry, parse_cpu_ticks_from_stat, parse_kb_field, parse_links_from_stat,
    };

    pub(super) fn live_process_table() -> Vec<(ProcEntry, Option<u64>)> {
        let Ok(entries) = std::fs::read_dir("/proc") else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter_map(|entry| {
                let pid = entry.file_name().to_string_lossy().parse::<u32>().ok()?;
                let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
                let (parent, group) = parse_links_from_stat(&stat)?;
                Some((
                    ProcEntry {
                        pid,
                        ppid: parent,
                        pgrp: group,
                    },
                    parse_cpu_ticks_from_stat(&stat),
                ))
            })
            .collect()
    }

    /// `Pss` from `smaps_rollup` divides a page shared by N processes N
    /// ways, so summed over the group it counts a forked parent's pages,
    /// shared libraries and mapped inputs once. `VmRSS` counts them once per
    /// process, and a test runner that forks eight children would sample at
    /// several times what the cgroup charges. Falls back to `VmRSS` where
    /// `smaps_rollup` cannot be read (kernels before 4.14).
    pub(super) fn read_process_memory_kb(pid: u32) -> Option<u64> {
        if let Ok(rollup) = std::fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))
            && let Some(pss_kb) = parse_kb_field(&rollup, "Pss:")
        {
            return Some(pss_kb);
        }
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        parse_kb_field(&status, "VmRSS:")
    }

    pub(super) fn read_process_cpu_ticks(pid: u32) -> Option<u64> {
        parse_cpu_ticks_from_stat(&std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?)
    }

    /// Ticks are clock ticks from `/proc`. `_SC_CLK_TCK` is 100 on every
    /// Linux we run on, but it is queryable so ask rather than assume, and
    /// fall back to 100 if the query fails.
    pub(super) fn ticks_to_millis(ticks: u64) -> u64 {
        // SAFETY: `sysconf` takes an int and returns a long, with no memory
        // safety considerations.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        let hz = if hz > 0 { hz as u64 } else { 100 };
        ticks.saturating_mul(1_000) / hz
    }

    pub(super) fn process_exists(pid: u32) -> bool {
        Path::new(&format!("/proc/{pid}")).exists()
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use core::ffi::c_int;
    use core::mem::size_of;

    use crate::running_actions_manager::ProcEntry;

    // libc deprecates its binding in favour of the mach2 crate; one call does
    // not warrant that dependency.
    #[repr(C)]
    struct MachTimebaseInfo {
        numer: u32,
        denom: u32,
    }

    unsafe extern "C" {
        fn mach_timebase_info(info: *mut MachTimebaseInfo) -> c_int;
    }

    pub(super) fn live_process_table() -> Vec<(ProcEntry, Option<u64>)> {
        // SAFETY: a null buffer asks only for the number of pids.
        let count = unsafe { libc::proc_listallpids(core::ptr::null_mut(), 0) };
        let Ok(count) = usize::try_from(count) else {
            return Vec::new();
        };
        // Room for the processes that start between the two calls.
        let mut pids: Vec<libc::pid_t> = vec![0; count + 64];
        let Ok(buffer_size) = c_int::try_from(pids.len() * size_of::<libc::pid_t>()) else {
            return Vec::new();
        };
        // SAFETY: the buffer holds `buffer_size` bytes and outlives the call.
        let listed = unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), buffer_size) };
        let Ok(listed) = usize::try_from(listed) else {
            return Vec::new();
        };
        pids.truncate(listed);
        pids.into_iter()
            .filter_map(|pid| {
                let pid = u32::try_from(pid).ok()?;
                let info = bsd_info(pid)?;
                Some((
                    ProcEntry {
                        pid,
                        ppid: info.pbi_ppid,
                        pgrp: info.pbi_pgid,
                    },
                    read_process_cpu_ticks(pid),
                ))
            })
            .collect()
    }

    /// `ri_phys_footprint` is what macOS charges the process: its private
    /// resident and compressed pages, not the shared pages `RSS` counts once
    /// per process.
    pub(super) fn read_process_memory_kb(pid: u32) -> Option<u64> {
        rusage(pid).map(|usage| usage.ri_phys_footprint / 1024)
    }

    pub(super) fn read_process_cpu_ticks(pid: u32) -> Option<u64> {
        rusage(pid).map(|usage| usage.ri_user_time.saturating_add(usage.ri_system_time))
    }

    /// Ticks are Mach absolute time units: nanoseconds on Intel, but not on
    /// Apple silicon, so convert through the timebase.
    pub(super) fn ticks_to_millis(ticks: u64) -> u64 {
        let mut timebase = MachTimebaseInfo { numer: 0, denom: 0 };
        // SAFETY: fills the struct it is given, which outlives the call.
        let status = unsafe { mach_timebase_info(&raw mut timebase) };
        let (numer, denom) = if status == 0 && timebase.denom != 0 {
            (u128::from(timebase.numer), u128::from(timebase.denom))
        } else {
            (1, 1)
        };
        let millis = u128::from(ticks) * numer / denom / 1_000_000;
        u64::try_from(millis).unwrap_or(u64::MAX)
    }

    pub(super) fn process_exists(pid: u32) -> bool {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        // SAFETY: signal 0 only checks that the process exists.
        let delivered = unsafe { libc::kill(pid, 0) } == 0;
        delivered || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }

    fn bsd_info(pid: u32) -> Option<libc::proc_bsdinfo> {
        let pid = c_int::try_from(pid).ok()?;
        let size = c_int::try_from(size_of::<libc::proc_bsdinfo>()).ok()?;
        // SAFETY: an all-zero proc_bsdinfo is valid; it is plain integers.
        let mut info: libc::proc_bsdinfo = unsafe { core::mem::zeroed() };
        // SAFETY: the buffer is a proc_bsdinfo of `size` bytes that outlives
        // the call.
        let read = unsafe {
            libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size)
        };
        (read == size).then_some(info)
    }

    fn rusage(pid: u32) -> Option<libc::rusage_info_v2> {
        let pid = c_int::try_from(pid).ok()?;
        // SAFETY: an all-zero rusage_info_v2 is valid; it is plain integers.
        let mut usage: libc::rusage_info_v2 = unsafe { core::mem::zeroed() };
        // SAFETY: RUSAGE_INFO_V2 fills a rusage_info_v2, which outlives the
        // call.
        let status =
            unsafe { libc::proc_pid_rusage(pid, libc::RUSAGE_INFO_V2, (&raw mut usage).cast()) };
        (status == 0).then_some(usage)
    }
}

#[cfg(target_family = "windows")]
mod platform {
    use core::mem::size_of;
    use std::collections::HashMap;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};

    use windows_sys::Win32::Foundation::{FILETIME, INVALID_HANDLE_VALUE, STILL_ACTIVE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    use crate::running_actions_manager::ProcEntry;

    pub(super) fn live_process_table() -> Vec<(ProcEntry, Option<u64>)> {
        let links = process_links();
        let times: HashMap<u32, ProcessTimes> = links
            .iter()
            .filter_map(|&(pid, _)| Some((pid, process_times(pid)?)))
            .collect();
        links
            .into_iter()
            .map(|(pid, ppid)| {
                let own = times.get(&pid);
                // Windows never reparents, and reuses ids, so a parent id can
                // name a process that started after the child: not its parent.
                let ppid = match (own, times.get(&ppid)) {
                    (Some(own), Some(parent)) if parent.created > own.created => 0,
                    _ => ppid,
                };
                (
                    ProcEntry {
                        pid,
                        ppid,
                        pgrp: pid,
                    },
                    own.map(|times| times.cpu),
                )
            })
            .collect()
    }

    /// The private commit charge: memory the process committed that no
    /// other process shares, which is also what a job object's memory limit
    /// counts. The private working set would be closer to `Pss`, but
    /// Windows Server 2022 reports it as 0.
    pub(super) fn read_process_memory_kb(pid: u32) -> Option<u64> {
        let process = open(pid)?;
        let size = u32::try_from(size_of::<PROCESS_MEMORY_COUNTERS_EX>()).ok()?;
        let mut counters = PROCESS_MEMORY_COUNTERS_EX {
            cb: size,
            ..Default::default()
        };
        // SAFETY: the handle is open and the buffer holds `size` bytes.
        let ok = unsafe {
            GetProcessMemoryInfo(
                process.as_raw_handle(),
                (&raw mut counters).cast::<PROCESS_MEMORY_COUNTERS>(),
                size,
            )
        };
        (ok != 0).then_some(counters.PrivateUsage as u64 / 1024)
    }

    pub(super) fn read_process_cpu_ticks(pid: u32) -> Option<u64> {
        process_times(pid).map(|times| times.cpu)
    }

    /// Ticks are the 100 ns units of `GetProcessTimes`.
    pub(super) const fn ticks_to_millis(ticks: u64) -> u64 {
        ticks / 10_000
    }

    /// The process object outlives the process for as long as anyone holds
    /// a handle to it, which an antivirus scanner may, so ask whether it is
    /// still running instead.
    pub(super) fn process_exists(pid: u32) -> bool {
        let Some(process) = open(pid) else {
            return false;
        };
        let mut exit_code = 0;
        // SAFETY: the handle is open and `exit_code` outlives the call.
        let ok = unsafe { GetExitCodeProcess(process.as_raw_handle(), &raw mut exit_code) };
        ok != 0 && exit_code == STILL_ACTIVE.cast_unsigned()
    }

    struct ProcessTimes {
        created: u64,
        cpu: u64,
    }

    /// Every process's id and its parent's id, from one snapshot.
    fn process_links() -> Vec<(u32, u32)> {
        let Ok(entry_size) = u32::try_from(size_of::<PROCESSENTRY32W>()) else {
            return Vec::new();
        };
        // SAFETY: takes no pointers; the result is checked before use.
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if snapshot == INVALID_HANDLE_VALUE {
            return Vec::new();
        }
        // SAFETY: a valid handle this function now owns.
        let snapshot = unsafe { OwnedHandle::from_raw_handle(snapshot) };
        // SAFETY: an all-zero entry is valid; it is plain integers.
        let mut entry: PROCESSENTRY32W = unsafe { core::mem::zeroed() };
        entry.dwSize = entry_size;
        let mut links = Vec::new();
        // SAFETY: the snapshot is open and `entry` has its size set.
        let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle(), &raw mut entry) } != 0;
        while more {
            links.push((entry.th32ProcessID, entry.th32ParentProcessID));
            // SAFETY: as above.
            more = unsafe { Process32NextW(snapshot.as_raw_handle(), &raw mut entry) } != 0;
        }
        links
    }

    fn process_times(pid: u32) -> Option<ProcessTimes> {
        let process = open(pid)?;
        let mut created = FILETIME::default();
        let mut exited = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        // SAFETY: the handle is open and every out pointer outlives the call.
        let ok = unsafe {
            GetProcessTimes(
                process.as_raw_handle(),
                &raw mut created,
                &raw mut exited,
                &raw mut kernel,
                &raw mut user,
            )
        };
        (ok != 0).then(|| ProcessTimes {
            created: filetime_ticks(created),
            cpu: filetime_ticks(kernel).saturating_add(filetime_ticks(user)),
        })
    }

    fn filetime_ticks(time: FILETIME) -> u64 {
        (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
    }

    fn open(pid: u32) -> Option<OwnedHandle> {
        // SAFETY: takes no pointers; a null result is checked before use.
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        // SAFETY: a non-null handle from OpenProcess is open and ours to close.
        (!handle.is_null()).then(|| unsafe { OwnedHandle::from_raw_handle(handle) })
    }
}
