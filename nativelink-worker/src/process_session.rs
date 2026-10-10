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

//! An action's processes, as the worker measures and ends them: the session
//! the action's own process leads.
//!
//! A process group is not enough. A job-control shell puts each job in a
//! group of its own (`set -m`), and Bazel's test wrapper, `test-setup.sh`,
//! does exactly that to the test it runs, so the test and everything under
//! it left the action's group: the sampler measured the waiting shell (a
//! test's peak memory and CPU read as nothing) and a kill of the group
//! missed the test. `setpgid` cannot move a process into another session,
//! so the session holds every descendant that did not call `setsid` itself,
//! and survives reparenting to the worker as group membership did. The
//! session's leader is also the leader of a new group (`setsid` makes it
//! one), so its id is the child's pid, its group id and its session id.

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

/// Parses the session id (`session`, field 6) from the contents of a
/// `/proc/<pid>/stat` line.
///
/// `comm` (field 2) can contain spaces and parentheses, so fields are parsed
/// relative to the final `)` to avoid miscounting. Returns `None` when the
/// line is malformed.
pub fn parse_session_from_stat(stat: &str) -> Option<u32> {
    let after_comm = stat.rsplit_once(')')?.1;
    // Fields after the final ')': state(0) ppid(1) pgrp(2) session(3) ...
    after_comm.split_whitespace().nth(3)?.parse().ok()
}

/// Whether a `/proc/<pid>/stat` line is a zombie's: it has exited and holds
/// nothing but its entry until it is reaped.
pub fn is_zombie_stat(stat: &str) -> bool {
    stat.rsplit_once(')')
        .and_then(|(_, after_comm)| after_comm.split_whitespace().next())
        == Some("Z")
}

/// Every process in session `sid`, as its pid and `/proc/<pid>/stat` line.
/// A process that exits between the listing and the read is skipped.
#[cfg(target_os = "linux")]
pub fn session_members(sid: u32) -> Vec<(u32, String)> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_string_lossy().parse::<u32>().ok())
        .filter_map(|pid| {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            (parse_session_from_stat(&stat) == Some(sid)).then_some((pid, stat))
        })
        .collect()
}

/// How many times [`signal_session`] lists the session again for SIGKILL:
/// a member can fork between the listing and its kill, and its child is
/// only seen by the next listing. A session that still has live members
/// after this many is reported by the caller's own checks.
#[cfg(target_os = "linux")]
const KILL_PASSES: usize = 16;

/// Sends `signal` to every live process in session `sid`, and returns
/// whether any process got it. For SIGKILL the session is listed again
/// until no live member is left (or [`KILL_PASSES`]), since a member can
/// fork while it is being killed. The kernel has no call that signals a
/// session, and the cgroup a container's worker runs in is not its to
/// divide, so the session is found in `/proc`.
#[cfg(target_os = "linux")]
pub fn signal_session(sid: u32, signal: i32) -> bool {
    let mut signalled = false;
    for _ in 0..if signal == libc::SIGKILL {
        KILL_PASSES
    } else {
        1
    } {
        let mut live = false;
        for (pid, stat) in session_members(sid) {
            if is_zombie_stat(&stat) {
                continue;
            }
            let Ok(pid) = i32::try_from(pid) else {
                continue;
            };
            live = true;
            // SAFETY: kill only takes integers and has no memory safety
            // considerations; a pid that exited since the listing is
            // reported as ESRCH and not acted on, and a pid is not reused
            // while its session, led by `sid`, still has a member.
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
/// before sessions, which is every process that did not move to a group of
/// its own.
#[cfg(all(target_family = "unix", not(target_os = "linux")))]
pub fn signal_session(sid: u32, signal: i32) -> bool {
    let Ok(pgid) = i32::try_from(sid) else {
        return false;
    };
    // SAFETY: killpg only takes integers and has no memory safety
    // considerations; a stale group id is reported as ESRCH, not acted on.
    unsafe { libc::killpg(pgid, signal) == 0 }
}
