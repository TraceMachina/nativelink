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

//! An action's processes, as the worker measures and ends them: every
//! descendant of the action's stub (`crate::action_stub`).
//!
//! Neither a process group nor a session is enough. A job-control shell puts
//! each job in a group of its own (`set -m`, which Bazel's `test-setup.sh`
//! does to every test), and a process that detaches its children calls
//! `setsid` for each (Node's `spawn(..., { detached: true })`, a daemonizing
//! server), so a test and everything under it can leave both. Ancestry is
//! the one membership a process cannot leave: the stub is a child
//! subreaper, so an orphan in the action is reparented to it rather than to
//! the worker, and every process of the action stays a descendant of the
//! stub for as long as the stub lives.
//!
//! The stub leads a new session too (`become_session_leader`), so the
//! action never shares the worker's, and if the stub itself is killed the
//! session is what the worker can still reach.

use std::collections::{HashMap, HashSet};
use std::io::Error;

/// Makes the calling process the leader of a new session, and of a new
/// process group in it. For `pre_exec`: `setsid` is async-signal-safe. It
/// fails for a process that already leads a group, so the command must not
/// also ask for `process_group(0)`.
#[cfg(target_family = "unix")]
pub fn become_session_leader() -> Result<(), Error> {
    // SAFETY: setsid takes no arguments and has no memory safety
    // considerations.
    if unsafe { libc::setsid() } == -1 {
        return Err(Error::last_os_error());
    }
    Ok(())
}

/// The fields of a `/proc/<pid>/stat` line after `comm` (field 2), which can
/// contain spaces and parentheses, so the fields are counted from the final
/// `)`: state(0) ppid(1) pgrp(2) session(3) ...
fn after_comm(stat: &str) -> Option<core::str::SplitWhitespace<'_>> {
    Some(stat.rsplit_once(')')?.1.split_whitespace())
}

/// Parses the parent pid (`ppid`, field 4) from the contents of a
/// `/proc/<pid>/stat` line. Returns `None` when the line is malformed.
pub fn parse_ppid_from_stat(stat: &str) -> Option<u32> {
    after_comm(stat)?.nth(1)?.parse().ok()
}

/// Parses the session id (`session`, field 6) from the contents of a
/// `/proc/<pid>/stat` line. Returns `None` when the line is malformed.
pub fn parse_session_from_stat(stat: &str) -> Option<u32> {
    after_comm(stat)?.nth(3)?.parse().ok()
}

/// Whether a `/proc/<pid>/stat` line is a zombie's: it has exited and holds
/// nothing but its entry until it is reaped.
pub fn is_zombie_stat(stat: &str) -> bool {
    after_comm(stat).and_then(|mut fields| fields.next()) == Some("Z")
}

/// Every process on the host this worker can see, as its pid and
/// `/proc/<pid>/stat` line. A process that exits between the listing and
/// the read is skipped.
#[cfg(target_os = "linux")]
fn all_processes() -> Vec<(u32, String)> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
        .filter_map(|pid| {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            Some((pid, stat))
        })
        .collect()
}

/// `root` and every descendant of it, as its pid and `/proc/<pid>/stat`
/// line, `root` first when it is still there. One pass over `/proc`: a
/// process forked after its parent was read is seen by the next pass, which
/// is why the kills below repeat.
#[cfg(target_os = "linux")]
pub fn tree_members(root: u32) -> Vec<(u32, String)> {
    let processes = all_processes();
    let mut children: HashMap<u32, Vec<usize>> = HashMap::new();
    let mut root_index = None;
    for (index, (pid, stat)) in processes.iter().enumerate() {
        if *pid == root {
            root_index = Some(index);
        } else if let Some(ppid) = parse_ppid_from_stat(stat) {
            children.entry(ppid).or_default().push(index);
        }
    }
    let mut members = Vec::new();
    if let Some(index) = root_index {
        members.push(processes[index].clone());
    }
    let mut pending = vec![root];
    let mut seen = HashSet::from([root]);
    while let Some(parent) = pending.pop() {
        for &index in children.get(&parent).map_or(&[][..], Vec::as_slice) {
            let (pid, stat) = &processes[index];
            if seen.insert(*pid) {
                members.push((*pid, stat.clone()));
                pending.push(*pid);
            }
        }
    }
    members
}

/// Every process in session `sid`, as its pid and `/proc/<pid>/stat` line.
/// What the worker can still reach of an action whose stub was killed: its
/// orphans then belong to the worker, and the session is all that marks
/// them as the action's.
#[cfg(target_os = "linux")]
pub fn session_members(sid: u32) -> Vec<(u32, String)> {
    all_processes()
        .into_iter()
        .filter(|(_, stat)| parse_session_from_stat(stat) == Some(sid))
        .collect()
}

/// How many times a SIGKILL lists the processes again: a member can fork
/// between the listing and its kill, and its child is only seen by the next
/// listing. Whatever is still alive after this many is reported by the
/// caller's own checks.
#[cfg(target_os = "linux")]
const KILL_PASSES: usize = 16;

/// Sends `signal` to `pid` through a pidfd, after reading again that its
/// parent is `parent`: between the listing and the signal the process could
/// have been reaped and its pid given to an unrelated process, and the
/// pidfd pins the process the check read. Falls back to `kill` on a kernel
/// without pidfds (before 5.3), where a pid is not reused while its parent,
/// in the tree, has not reaped it.
#[cfg(target_os = "linux")]
fn signal_checked(pid: u32, parent: u32, signal: i32) -> bool {
    let Ok(raw_pid) = libc::pid_t::try_from(pid) else {
        return false;
    };
    // SAFETY: pidfd_open takes integers and returns a new descriptor or -1.
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, raw_pid, 0) };
    if pidfd < 0 {
        // SAFETY: kill only takes integers and has no memory safety
        // considerations; a pid that exited is reported as ESRCH.
        return unsafe { libc::kill(raw_pid, signal) } == 0;
    }
    let Ok(pidfd) = libc::c_int::try_from(pidfd) else {
        return false;
    };
    let still_ours = std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|stat| parse_ppid_from_stat(&stat))
        == Some(parent);
    let signalled = still_ours
        // SAFETY: the pidfd is ours and open; a null siginfo is the plain
        // kill(2) form.
        && unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                pidfd,
                signal,
                core::ptr::null::<libc::siginfo_t>(),
                0,
            )
        } == 0;
    // SAFETY: the pidfd was opened above and is closed once.
    unsafe { libc::close(pidfd) };
    signalled
}

/// Sends `signal` to every live descendant of `root`, not to `root` itself,
/// and returns whether any process got it. For SIGKILL the tree is listed
/// again until no live descendant is left (or [`KILL_PASSES`]), since a
/// descendant can fork while it is being killed. `root` is the action's
/// stub, which ends the action itself once its child is gone, or a pooled
/// worker process that outlives the request.
#[cfg(target_os = "linux")]
pub fn signal_descendants(root: u32, signal: i32) -> bool {
    let passes = if signal == libc::SIGKILL {
        KILL_PASSES
    } else {
        1
    };
    let mut signalled = false;
    for _ in 0..passes {
        let mut live = false;
        for (pid, stat) in tree_members(root) {
            if pid == root || is_zombie_stat(&stat) {
                continue;
            }
            let Some(parent) = parse_ppid_from_stat(&stat) else {
                continue;
            };
            live = true;
            signalled |= signal_checked(pid, parent, signal);
        }
        if !live {
            break;
        }
    }
    signalled
}

/// Sends `signal` to every live process in session `sid`, and returns
/// whether any process got it, listing the session again for SIGKILL as
/// [`signal_descendants`] does. The fallback for an action whose stub is
/// gone.
#[cfg(target_os = "linux")]
pub fn signal_session(sid: u32, signal: i32) -> bool {
    let passes = if signal == libc::SIGKILL {
        KILL_PASSES
    } else {
        1
    };
    let mut signalled = false;
    for _ in 0..passes {
        let mut live = false;
        for (pid, stat) in session_members(sid) {
            if is_zombie_stat(&stat) {
                continue;
            }
            let Ok(pid) = libc::pid_t::try_from(pid) else {
                continue;
            };
            live = true;
            // SAFETY: kill only takes integers and has no memory safety
            // considerations; a pid that exited since the listing is
            // reported as ESRCH and not acted on.
            if unsafe { libc::kill(pid, signal) } == 0 {
                signalled = true;
            }
        }
        if !live {
            break;
        }
    }
    signalled
}

/// Without `/proc`, the session's own group: what a group kill reached
/// before, which is every process that did not move to a group of its own.
#[cfg(all(target_family = "unix", not(target_os = "linux")))]
pub fn signal_session(sid: u32, signal: i32) -> bool {
    let Ok(pgid) = i32::try_from(sid) else {
        return false;
    };
    // SAFETY: killpg only takes integers and has no memory safety
    // considerations; a stale group id is reported as ESRCH, not acted on.
    unsafe { libc::killpg(pgid, signal) == 0 }
}
