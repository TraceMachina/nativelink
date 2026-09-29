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

//! What an action leaves behind is the worker's to reap.
//!
//! The worker waits on the one process it spawned per action. Anything that
//! process forked and did not wait for (a shell's background job, a helper
//! that daemonized) is reparented when it exits: to the worker when the
//! worker is the container's PID 1, which it usually is, and otherwise to
//! whatever init the container has. The worker never waited on those, so
//! each became a zombie the moment it exited and stayed one for the life of
//! the pod; a worker seen with hundreds of them had run one build whose
//! actions forked and did not wait. This module makes the worker their
//! parent wherever it runs and reaps them on a timer.
//!
//! Inside `use_namespaces` the stub is the init of the action's PID
//! namespace and reaps there; this covers the actions that run without it.

use core::hash::BuildHasher;
use std::collections::HashSet;

/// How often the worker reaps the zombies its actions left behind.
pub const REAP_INTERVAL: core::time::Duration = core::time::Duration::from_secs(5);

/// Make this process the parent of every descendant an action leaves
/// behind, even when it is not PID 1, so the zombies land here where they
/// can be reaped rather than with an init that never will.
#[cfg(target_os = "linux")]
pub fn become_subreaper() -> Result<(), std::io::Error> {
    // SAFETY: prctl with PR_SET_CHILD_SUBREAPER takes integers only and
    // changes a flag on this process.
    if unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(target_os = "linux"))]
pub const fn become_subreaper() -> Result<(), std::io::Error> {
    Ok(())
}

/// The parent pid out of a `/proc/<pid>/status`, when the process is a
/// zombie; `None` for a live process or unreadable text.
pub fn parse_zombie_ppid(status: &str) -> Option<u32> {
    let mut zombie = false;
    let mut ppid = None;
    for line in status.lines() {
        if let Some(state) = line.strip_prefix("State:") {
            zombie = state.trim_start().starts_with('Z');
        } else if let Some(parent) = line.strip_prefix("PPid:") {
            ppid = parent.trim().parse().ok();
        }
    }
    zombie.then_some(ppid).flatten()
}

/// The zombies whose parent is this process, by pid.
#[cfg(target_os = "linux")]
pub fn zombie_children() -> HashSet<u32> {
    // SAFETY: getpid has no memory safety considerations.
    let me = u32::try_from(unsafe { libc::getpid() }).unwrap_or(0);
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return HashSet::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/status"))
                .ok()
                .and_then(|status| parse_zombie_ppid(&status))
                == Some(me)
        })
        .collect()
}

/// Reaps the zombies that were already zombies at the previous sweep and
/// remembers this sweep's for the next. Two sweeps apart is the point: a
/// process the worker spawned itself is reaped by the runtime within
/// milliseconds of exiting, so a zombie that lasts a whole interval is an
/// orphan nothing else will ever wait for, and the runtime's own children
/// are never stolen from it. Returns how many were reaped.
#[cfg(target_os = "linux")]
pub fn reap_orphaned_zombies<S: BuildHasher>(seen_last: &mut HashSet<u32, S>) -> usize {
    let now = zombie_children();
    let mut reaped = 0;
    for pid in now.intersection(seen_last) {
        let Ok(pid_t) = libc::pid_t::try_from(*pid) else {
            continue;
        };
        let mut status = 0;
        // SAFETY: waitpid takes a pid and a pointer to an int on the stack;
        // a pid that is not our child is reported as ECHILD, not acted on.
        if unsafe { libc::waitpid(pid_t, &raw mut status, libc::WNOHANG) } == pid_t {
            reaped += 1;
        }
    }
    seen_last.clear();
    seen_last.extend(now);
    reaped
}

#[cfg(not(target_os = "linux"))]
pub const fn reap_orphaned_zombies<S: BuildHasher>(_seen_last: &mut HashSet<u32, S>) -> usize {
    0
}
