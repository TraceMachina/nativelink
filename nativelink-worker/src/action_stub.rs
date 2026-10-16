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

//! The process every action runs under: the worker's child, which forks the
//! action and stays as its parent until everything the action started is
//! gone, as Bazel's `process-wrapper` does for a local action
//! (`src/main/tools/process-tools-linux.cc`, `TerminateAndWaitForAll`).
//!
//! The stub is a child subreaper (`PR_SET_CHILD_SUBREAPER`), so a process the
//! action orphans is reparented to it, not to the worker, whatever session
//! or group the process moved to: the action's processes are exactly the
//! stub's descendants (`crate::process_tree`). When the action's own
//! process exits, the stub kills what is left, reaps it, and exits with the
//! action's status, so nothing outlives its action to hold a port, a file
//! or memory that the next action would be charged for.
//!
//! Before it exits it writes a [`StubReport`] to a pipe the worker holds:
//! the CPU time of everything it reaped, which the kernel totals exactly
//! (`getrusage(RUSAGE_CHILDREN)`), including processes too short-lived for
//! any sample to see.
//!
//! Everything here runs between `fork` and `exec` in a copy of a
//! multi-threaded process, so it is async-signal-safe: system calls only,
//! no allocation, no locks.

use std::io::Error;
use std::os::fd::{AsRawFd, OwnedFd};

/// What the stub reports when the action is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StubReport {
    /// User CPU time of every process the stub reaped, and of what those
    /// reaped, in microseconds.
    pub user_cpu_us: u64,
    /// System CPU time of the same, in microseconds.
    pub system_cpu_us: u64,
    /// How many processes were still running, or waiting to be reaped, when
    /// the action's own process exited.
    pub stragglers: u32,
}

/// The report's size on the pipe: two little-endian `u64`s and a `u32`.
pub const REPORT_LEN: usize = 20;

impl StubReport {
    pub fn to_bytes(self) -> [u8; REPORT_LEN] {
        let mut bytes = [0u8; REPORT_LEN];
        bytes[0..8].copy_from_slice(&self.user_cpu_us.to_le_bytes());
        bytes[8..16].copy_from_slice(&self.system_cpu_us.to_le_bytes());
        bytes[16..20].copy_from_slice(&self.stragglers.to_le_bytes());
        bytes
    }

    pub fn from_bytes(bytes: &[u8; REPORT_LEN]) -> Self {
        let u64_at = |at: usize| {
            let mut word = [0u8; 8];
            word.copy_from_slice(&bytes[at..at + 8]);
            u64::from_le_bytes(word)
        };
        let mut stragglers = [0u8; 4];
        stragglers.copy_from_slice(&bytes[16..20]);
        Self {
            user_cpu_us: u64_at(0),
            system_cpu_us: u64_at(8),
            stragglers: u32::from_le_bytes(stragglers),
        }
    }

    /// The total CPU time, in milliseconds.
    pub const fn cpu_time_ms(&self) -> u64 {
        self.user_cpu_us.saturating_add(self.system_cpu_us) / 1_000
    }
}

/// The worker's end of the report pipe, and the stub's.
#[derive(Debug)]
pub struct ReportPipe {
    /// Non-blocking, so a stub that died without writing reads as no
    /// report instead of a wait.
    pub read: OwnedFd,
    /// Close-on-exec, so the stub (which never execs) keeps it and the
    /// action loses it at its `exec`. The worker drops its copy once the
    /// action is spawned.
    pub write: OwnedFd,
}

impl ReportPipe {
    pub fn new() -> Result<Self, Error> {
        let mut fds: [libc::c_int; 2] = [0; 2];
        // SAFETY: fds is a two-element array on the stack, as pipe2 needs.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
            return Err(Error::last_os_error());
        }
        // SAFETY: pipe2 returned two new descriptors that nothing else owns.
        let (read, write) = unsafe {
            use std::os::fd::FromRawFd;
            (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))
        };
        // SAFETY: fcntl on a descriptor we own, with integer arguments.
        let flags = unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETFL) };
        // SAFETY: as above.
        if flags < 0
            || unsafe { libc::fcntl(read.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
        {
            return Err(Error::last_os_error());
        }
        Ok(Self { read, write })
    }

    /// The stub's report, once the stub has exited: `None` when it wrote
    /// none (it was killed, or it could not be started).
    pub fn read_report(read: &OwnedFd) -> Option<StubReport> {
        let mut bytes = [0u8; REPORT_LEN];
        // SAFETY: bytes is on the stack and REPORT_LEN long.
        let n = unsafe { libc::read(read.as_raw_fd(), bytes.as_mut_ptr().cast(), REPORT_LEN) };
        (usize::try_from(n).ok() == Some(REPORT_LEN)).then(|| StubReport::from_bytes(&bytes))
    }
}

fn exit(status: i32) -> ! {
    // SAFETY: It is always safe to _exit.
    unsafe { libc::_exit(status) };
}

fn errno() -> libc::c_int {
    // SAFETY: reading errno is always safe.
    unsafe { *libc::__errno_location() }
}

/// Closes every descriptor except `keep`.
///
/// # Safety
///
/// Only for a process about to run nothing but the stub's own loop.
unsafe fn close_all_fds_except(keep: libc::c_int) {
    // SAFETY: closing descriptors has no memory safety considerations; the
    // stub uses none but `keep`.
    unsafe {
        if keep > 0 {
            libc::syscall(libc::SYS_close_range, 0, keep - 1, 0);
        }
        if libc::syscall(libc::SYS_close_range, keep + 1, libc::c_uint::MAX, 0) == 0 {
            return;
        }
        // Before Linux 5.9: every descriptor up to the limit, one by one.
        let mut rlim = core::mem::MaybeUninit::<libc::rlimit>::uninit();
        let max_fd = if libc::getrlimit(libc::RLIMIT_NOFILE, rlim.as_mut_ptr()) == 0 {
            let cur = rlim.assume_init().rlim_cur;
            if cur == libc::RLIM_INFINITY {
                0x0001_0000
            } else {
                libc::c_int::try_from(cur).unwrap_or(0x0001_0000)
            }
        } else {
            4096
        };
        for fd in 0..max_fd {
            if fd != keep {
                libc::close(fd);
            }
        }
    }
}

/// Sends SIGKILL to every child of this process, as listed by
/// `/proc/thread-self/children` (the stub is single-threaded, so its one
/// thread's children are all of its children). Returns `false` when the
/// list cannot be read (a kernel without `CONFIG_PROC_CHILDREN`).
fn kill_children() -> bool {
    // SAFETY: open with a static path and integer flags.
    let fd = unsafe {
        libc::open(
            c"/proc/thread-self/children".as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return false;
    }
    let mut buffer = [0u8; 512];
    let mut pid: libc::pid_t = 0;
    let mut in_number = false;
    loop {
        // SAFETY: buffer is on the stack and its length is passed.
        let n = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if n < 0 && errno() == libc::EINTR {
            continue;
        }
        let Ok(n) = usize::try_from(n) else { break };
        if n == 0 {
            break;
        }
        for &byte in &buffer[..n] {
            if byte.is_ascii_digit() {
                pid = pid
                    .saturating_mul(10)
                    .saturating_add(libc::pid_t::from(byte - b'0'));
                in_number = true;
            } else if in_number {
                // SAFETY: an unreaped child's pid cannot be reused, so this
                // signals only our own child.
                unsafe { libc::kill(pid, libc::SIGKILL) };
                pid = 0;
                in_number = false;
            }
        }
    }
    if in_number {
        // SAFETY: as above.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
    // SAFETY: fd was opened above.
    unsafe { libc::close(fd) };
    true
}

/// Kills and reaps everything left under the stub once the action's own
/// process is gone: each pass kills the stub's children, whose own children
/// are then reparented to the stub for the next pass, until `waitpid`
/// reports no child left. Returns how many processes it reaped.
fn terminate_and_wait_for_all() -> u32 {
    let mut reaped = 0u32;
    loop {
        if !kill_children() {
            // Without the list there is nothing to kill by; what is left is
            // the worker's to end by the session.
            return reaped;
        }
        let mut status = 0;
        // SAFETY: status is on the stack.
        let res = unsafe { libc::waitpid(-1, &raw mut status, 0) };
        if res > 0 {
            reaped = reaped.saturating_add(1);
        } else if errno() == libc::ECHILD {
            return reaped;
        }
        // EINTR: list and wait again.
    }
}

/// Microseconds in a `timeval`.
fn micros(time: libc::timeval) -> u64 {
    u64::try_from(time.tv_sec)
        .unwrap_or(0)
        .saturating_mul(1_000_000)
        .saturating_add(u64::try_from(time.tv_usec).unwrap_or(0))
}

/// Writes the report and exits as the action did: with its exit code, or by
/// the signal that killed it.
fn report_and_exit(report_fd: libc::c_int, status: libc::c_int, stragglers: u32) -> ! {
    let mut usage = core::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: usage is on the stack; getrusage only writes it, and a failed
    // call leaves the zeroes, which report no CPU.
    unsafe { libc::getrusage(libc::RUSAGE_CHILDREN, usage.as_mut_ptr()) };
    // SAFETY: an all-zero rusage is valid, and getrusage writes a valid one.
    let usage = unsafe { usage.assume_init() };
    let bytes = StubReport {
        user_cpu_us: micros(usage.ru_utime),
        system_cpu_us: micros(usage.ru_stime),
        stragglers,
    }
    .to_bytes();
    // SAFETY: bytes is on the stack. A short or failed write leaves the
    // worker with no report, which it treats as such.
    unsafe { libc::write(report_fd, bytes.as_ptr().cast(), bytes.len()) };

    if libc::WIFSIGNALED(status) {
        let mut sigset = core::mem::MaybeUninit::<libc::sigset_t>::uninit();
        // SAFETY: sigset is on the stack; raise is always safe, and _exit
        // below is the fallback.
        unsafe {
            libc::sigemptyset(sigset.as_mut_ptr());
            libc::sigaddset(sigset.as_mut_ptr(), libc::WTERMSIG(status));
            libc::sigprocmask(libc::SIG_UNBLOCK, sigset.as_ptr(), core::ptr::null_mut());
            libc::signal(libc::WTERMSIG(status), libc::SIG_DFL);
            libc::raise(libc::WTERMSIG(status));
        }
        exit(128 + libc::WTERMSIG(status));
    }
    exit(libc::WEXITSTATUS(status));
}

/// Forks the action under a stub. For `pre_exec`, after any namespace has
/// been entered: in the child, which goes on to exec the action, it
/// returns `Ok`; the parent becomes the stub and never returns.
///
/// The stub passes a SIGTERM on to the action, so the action gets the grace
/// the worker's `kill_grace_ms` promises, reaps as children exit, and when
/// the action's process exits, ends the rest (see the module docs). The
/// action gets `PR_SET_PDEATHSIG`, so a stub that is killed takes the
/// action's own process with it.
///
/// # Safety
///
/// Only in `pre_exec`. `report_fd` is the write end of a [`ReportPipe`],
/// or -1 for no report.
pub unsafe fn fork_stub(report_fd: libc::c_int) -> Result<(), Error> {
    // SAFETY: We are in pre_exec, an async-signal-safe context, and stay
    // in it.
    match unsafe { libc::fork() } {
        0 => {
            // SAFETY: prctl with integer arguments is async-signal-safe.
            if unsafe { libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) } != 0 {
                exit(1);
            }
            Ok(())
        }
        pid if pid > 0 => {
            // SAFETY: prctl with integer arguments is async-signal-safe. If
            // it fails, orphans go to the worker as they did before stubs.
            unsafe { libc::prctl(libc::PR_SET_CHILD_SUBREAPER, 1, 0, 0, 0) };
            // SAFETY: the stub uses no descriptor but the report's from here.
            unsafe { close_all_fds_except(report_fd) };

            let mut sigset = core::mem::MaybeUninit::<libc::sigset_t>::uninit();
            // SAFETY: sigset is on the stack and initialized here.
            unsafe {
                libc::sigemptyset(sigset.as_mut_ptr());
                libc::sigaddset(sigset.as_mut_ptr(), libc::SIGTERM);
                libc::sigaddset(sigset.as_mut_ptr(), libc::SIGCHLD);
                libc::sigprocmask(libc::SIG_BLOCK, sigset.as_ptr(), core::ptr::null_mut());
            }

            loop {
                // Reap whatever has exited; the action's own process ends
                // the action.
                loop {
                    let mut status = 0;
                    // SAFETY: status is on the stack.
                    let res = unsafe { libc::waitpid(-1, &raw mut status, libc::WNOHANG) };
                    if res == pid {
                        let stragglers = terminate_and_wait_for_all();
                        report_and_exit(report_fd, status, stragglers);
                    } else if res == 0 {
                        break;
                    } else if res < 0 {
                        if errno() == libc::EINTR {
                            continue;
                        }
                        // ECHILD with the action never reaped cannot happen;
                        // treat it as an action that vanished.
                        exit(255);
                    }
                }

                let mut siginfo = core::mem::MaybeUninit::<libc::siginfo_t>::uninit();
                // SAFETY: sigset is initialized and siginfo is on the stack.
                let sig = unsafe { libc::sigwaitinfo(sigset.as_ptr(), siginfo.as_mut_ptr()) };
                if sig == libc::SIGTERM {
                    // SAFETY: pid is our unreaped child.
                    unsafe { libc::kill(pid, libc::SIGTERM) };
                }
            }
        }
        _ => Err(Error::last_os_error()),
    }
}
