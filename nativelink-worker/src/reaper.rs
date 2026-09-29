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

use std::collections::{BTreeSet, HashSet};
use std::sync::Mutex;

/// How often the worker reaps the zombies its actions left behind.
pub const REAP_INTERVAL: core::time::Duration = core::time::Duration::from_secs(5);

/// The pids of every child this process spawned and still holds a handle
/// for. The reaper never touches these: their exit status belongs to the
/// code that waits on them, however long it takes to get round to it (a
/// persistent worker is only `try_wait`ed on demand, an action's wait can
/// be starved). Ownership is decided here, not by timing.
static OWNED: Mutex<Option<HashSet<u32>>> = Mutex::new(None);

/// A child this process owns, for as long as the guard lives.
#[derive(Debug)]
pub struct OwnedChild(Option<u32>);

impl OwnedChild {
    /// Registers `pid` as ours; `None` (a child that already exited at
    /// spawn) registers nothing. Drop the guard once the child has been
    /// waited on, or with the handle that will wait on it.
    pub fn new(pid: Option<u32>) -> Self {
        if let Some(pid) = pid {
            OWNED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get_or_insert_with(HashSet::new)
                .insert(pid);
        }
        Self(pid)
    }
}

impl Drop for OwnedChild {
    fn drop(&mut self) {
        if let Some(pid) = self.0
            && let Some(owned) = OWNED
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_mut()
        {
            owned.remove(&pid);
        }
    }
}

#[cfg(target_os = "linux")]
fn is_owned(pid: u32) -> bool {
    OWNED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_some_and(|owned| owned.contains(&pid))
}

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

/// The parent pid out of a `/proc/<pid>/stat` line, when the process is a
/// zombie; `None` for a live process or unreadable text. `comm` can hold
/// spaces and parentheses, so the fields are read after the final `)`:
/// state first, then the parent pid.
pub fn parse_zombie_ppid(stat: &str) -> Option<u32> {
    let after_comm = stat.rsplit_once(')')?.1;
    let mut fields = after_comm.split_whitespace();
    let state = fields.next()?;
    let ppid = fields.next()?.parse().ok()?;
    (state == "Z").then_some(ppid)
}

/// The zombies whose parent is this process, by pid. One short read per
/// process; call it off the runtime, since `/proc` can be large.
#[cfg(target_os = "linux")]
pub fn zombie_children() -> BTreeSet<u32> {
    // SAFETY: getpid has no memory safety considerations.
    let me = u32::try_from(unsafe { libc::getpid() }).unwrap_or(0);
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return BTreeSet::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
        .filter(|pid| {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| parse_zombie_ppid(&stat))
                == Some(me)
        })
        .collect()
}

/// Reaps the zombies that are not ours to wait on and were already zombies
/// at the previous sweep; returns how many, and this sweep's zombies for
/// the next call. A child this process spawned is never touched, whatever
/// its state: its exit status belongs to whoever holds its handle. The
/// second sweep is a guard on top of that, for a pid we never registered
/// (a fork of ours outside these paths); a pid reused within an interval
/// starts over, since a reaped pid is not carried forward. Blocking; run it
/// on the blocking pool.
#[cfg(target_os = "linux")]
pub fn reap_orphaned_zombies(seen_last: BTreeSet<u32>) -> (usize, BTreeSet<u32>) {
    let mut now = zombie_children();
    let mut reaped = 0;
    let candidates: Vec<u32> = now
        .iter()
        .copied()
        .filter(|pid| seen_last.contains(pid) && !is_owned(*pid))
        .collect();
    for pid in candidates {
        let Ok(pid_t) = libc::pid_t::try_from(pid) else {
            continue;
        };
        let mut status = 0;
        // SAFETY: waitpid takes a pid and a pointer to an int on the stack;
        // a pid that is not our child is reported as ECHILD, not acted on.
        if unsafe { libc::waitpid(pid_t, &raw mut status, libc::WNOHANG) } == pid_t {
            reaped += 1;
            now.remove(&pid);
        }
    }
    (reaped, now)
}

#[cfg(not(target_os = "linux"))]
pub fn reap_orphaned_zombies(_seen_last: BTreeSet<u32>) -> (usize, BTreeSet<u32>) {
    (0, BTreeSet::new())
}
