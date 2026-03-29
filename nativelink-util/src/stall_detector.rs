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

//! Stall detection and thread dump utilities.
//!
//! When an async operation takes longer than a configured threshold,
//! [`StallGuard`] dumps all thread stacks to a file for post-mortem analysis.

use core::time::Duration;
use std::sync::atomic::{AtomicU64, Ordering};

/// Minimum interval between consecutive stack dumps (seconds).
/// Prevents flooding /tmp with dumps during a sustained stall.
const MIN_DUMP_INTERVAL_SECS: u64 = 30;

/// Unix epoch seconds of the last dump. Used for rate-limiting.
static LAST_DUMP_EPOCH: AtomicU64 = AtomicU64::new(0);

/// Default stall threshold for store operations.
pub const DEFAULT_STALL_THRESHOLD: Duration = Duration::from_secs(30);

/// A guard that spawns a background task to detect stalls. When the
/// guarded operation completes (i.e., the guard is dropped), the
/// background task is cancelled. If the operation exceeds `threshold`,
/// a thread dump is written to `/tmp/nativelink-stall-<ts>.txt`.
///
/// This relies on tokio's timer infrastructure, so it cannot detect
/// stalls caused by the tokio runtime itself being blocked. The
/// runtime-watchdog OS thread in nativelink.rs covers that case.
#[must_use = "StallGuard is immediately cancelled if not held in a variable"]
#[derive(Debug)]
pub struct StallGuard {
    handle: tokio::task::JoinHandle<()>,
}

impl StallGuard {
    /// Create a stall guard for an operation with the given label.
    /// If the guard is not dropped within `threshold`, a stack dump fires.
    pub fn new(threshold: Duration, label: &'static str) -> Self {
        Self::new_inner(threshold, label, None)
    }

    /// Create a stall guard with additional dynamic context (e.g. digest
    /// hash, size, operation details). The context string is included in
    /// the stall message and thread dump header when the threshold fires.
    pub fn with_context(threshold: Duration, label: &'static str, context: String) -> Self {
        Self::new_inner(threshold, label, Some(context))
    }

    fn new_inner(threshold: Duration, label: &'static str, context: Option<String>) -> Self {
        let handle = tokio::spawn(async move {
            tokio::time::sleep(threshold).await;
            let ctx_suffix = context
                .as_deref()
                .map_or_else(String::new, |c| format!(" [{c}]"));
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let prev = LAST_DUMP_EPOCH.load(Ordering::Relaxed);
            if now.saturating_sub(prev) >= MIN_DUMP_INTERVAL_SECS
                && LAST_DUMP_EPOCH
                    .compare_exchange(prev, now, Ordering::SeqCst, Ordering::Relaxed)
                    .is_ok()
            {
                eprintln!(
                    "STORE OPERATION STALL: {label}{ctx_suffix} has been running for >{threshold:.0?} — dumping thread stacks",
                );
                let dump_label = if ctx_suffix.is_empty() {
                    label.to_string()
                } else {
                    format!("{label}{ctx_suffix}")
                };
                dump_thread_stacks(&dump_label);
            } else {
                eprintln!(
                    "STORE OPERATION STALL: {label}{ctx_suffix} has been running for >{threshold:.0?} (dump rate-limited)",
                );
            }
        });
        Self { handle }
    }
}

impl Drop for StallGuard {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// Dump all thread stacks to `/tmp/nativelink-stall-<timestamp>.txt`.
///
/// On Linux, reads `/proc/self/task/` to enumerate threads and collects
/// thread name, wait channel, state, context switches, and kernel stack.
///
/// On macOS, enumerates threads via Mach APIs (`task_threads`,
/// `thread_info`) and captures the calling thread's Rust backtrace.
/// Optionally runs the `sample` tool for full userspace stack traces.
///
/// On other platforms, this is a no-op (logs a message).
pub fn dump_thread_stacks(label: &str) {
    #[cfg(target_os = "linux")]
    dump_thread_stacks_linux(label);

    #[cfg(target_os = "macos")]
    dump_thread_stacks_macos(label);

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        eprintln!(
            "Thread dump not available on this platform (trigger: {label}, ts: {timestamp})"
        );
    }
}

#[cfg(target_os = "linux")]
fn dump_thread_stacks_linux(label: &str) {
    use std::fmt::Write as _;

    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = format!("/tmp/nativelink-stall-{timestamp_ms}.txt");
    let mut output = String::new();

    let _ = writeln!(output, "=== STORE OPERATION STALL THREAD DUMP ===");
    let _ = writeln!(output, "Trigger: {label}");
    let _ = writeln!(output, "Timestamp: {timestamp_ms}");
    let _ = writeln!(output, "PID: {}", std::process::id());
    let _ = writeln!(output);

    let task_dir = "/proc/self/task";
    let entries = match std::fs::read_dir(task_dir) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("Failed to read {task_dir}: {err}");
            return;
        }
    };

    let mut tids: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .collect();
    tids.sort();

    let _ = writeln!(output, "Thread count: {}", tids.len());
    let _ = writeln!(output);

    for tid in &tids {
        let _ = writeln!(output, "--- TID {tid} ---");
        let base = format!("{task_dir}/{tid}");

        // Thread name
        if let Ok(comm) = std::fs::read_to_string(format!("{base}/comm")) {
            let _ = write!(output, "  comm: {comm}");
        }
        // Wait channel (kernel function the thread is sleeping in)
        if let Ok(wchan) = std::fs::read_to_string(format!("{base}/wchan")) {
            let _ = writeln!(output, "  wchan: {wchan}");
        }
        // Status (state, voluntary/involuntary context switches)
        if let Ok(status) = std::fs::read_to_string(format!("{base}/status")) {
            for line in status.lines() {
                if line.starts_with("State:")
                    || line.starts_with("voluntary_ctxt_switches:")
                    || line.starts_with("nonvoluntary_ctxt_switches:")
                {
                    let _ = writeln!(output, "  {line}");
                }
            }
        }
        // Kernel stack (requires CAP_SYS_PTRACE or permissive ptrace_scope)
        if let Ok(stack) = std::fs::read_to_string(format!("{base}/stack")) {
            if !stack.trim().is_empty() {
                let _ = writeln!(output, "  kernel stack:");
                for line in stack.lines() {
                    let _ = writeln!(output, "    {line}");
                }
            }
        }
        let _ = writeln!(output);
    }

    match std::fs::write(&path, &output) {
        Ok(()) => eprintln!("Thread dump written to {path}"),
        Err(err) => eprintln!("Failed to write thread dump to {path}: {err}"),
    }

    // Capture userspace backtraces via eu-stack for full Rust call stacks.
    // eu-stack can hang indefinitely if the target process is wedged, so
    // we spawn it as a child and poll with a 30-second timeout.
    let bt_path = format!("/tmp/nativelink-stall-{timestamp_ms}-bt.txt");
    let pid = std::process::id();
    match std::process::Command::new("eu-stack")
        .args(["-p", &pid.to_string(), "-l"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(mut child) => {
            const EU_STACK_TIMEOUT: Duration = Duration::from_secs(30);
            const POLL_INTERVAL: Duration = Duration::from_millis(250);
            let deadline = std::time::Instant::now() + EU_STACK_TIMEOUT;
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) => {
                        if std::time::Instant::now() >= deadline {
                            eprintln!(
                                "eu-stack timed out after {EU_STACK_TIMEOUT:.0?}, killing child process"
                            );
                            drop(child.kill());
                            // Reap the zombie
                            drop(child.wait());
                            break None;
                        }
                        std::thread::sleep(POLL_INTERVAL);
                    }
                    Err(err) => {
                        eprintln!("eu-stack wait error: {err}");
                        drop(child.kill());
                        drop(child.wait());
                        break None;
                    }
                }
            };
            if status.is_some() {
                let stdout = child
                    .stdout
                    .take()
                    .map(|mut r| {
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut r, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();
                let stderr = child
                    .stderr
                    .take()
                    .map(|mut r| {
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut r, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();
                let combined =
                    [&stdout[..], b"\n--- stderr ---\n", &stderr[..]].concat();
                match std::fs::write(&bt_path, &combined) {
                    Ok(()) => eprintln!("Userspace backtrace written to {bt_path}"),
                    Err(err) => {
                        eprintln!("Failed to write backtrace to {bt_path}: {err}");
                    }
                }
            }
        }
        Err(err) => eprintln!("Failed to run eu-stack: {err}"),
    }

    cleanup_old_stall_dumps();
}

/// Dump thread info on macOS using Mach APIs and `std::backtrace`.
///
/// Enumerates all threads via `task_threads()`, retrieves thread names
/// via `pthread_from_mach_thread_np` + `pthread_getname_np`, and collects
/// CPU usage and run state from `thread_info(THREAD_BASIC_INFO)`.
///
/// The calling thread's Rust backtrace is captured via
/// `std::backtrace::Backtrace::force_capture()`. For full userspace
/// stack traces of all threads, the `sample` command is invoked (the
/// macOS equivalent of `eu-stack`).
#[cfg(target_os = "macos")]
fn dump_thread_stacks_macos(label: &str) {
    use std::fmt::Write as _;

    let timestamp_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = format!("/tmp/nativelink-stall-{timestamp_ms}.txt");
    let pid = std::process::id();
    let mut output = String::new();

    let _ = writeln!(output, "=== STORE OPERATION STALL THREAD DUMP (macOS) ===");
    let _ = writeln!(output, "Trigger: {label}");
    let _ = writeln!(output, "Timestamp: {timestamp_ms}");
    let _ = writeln!(output, "PID: {pid}");
    let _ = writeln!(output);

    // Capture the calling thread's backtrace (typically the runtime-watchdog
    // or a tokio worker that triggered the stall guard).
    let bt = std::backtrace::Backtrace::force_capture();
    let _ = writeln!(output, "=== Calling thread backtrace ===");
    let _ = writeln!(output, "{bt}");
    let _ = writeln!(output);

    // Enumerate threads via Mach APIs
    enumerate_mach_threads(&mut output);

    match std::fs::write(&path, &output) {
        Ok(()) => eprintln!("Thread dump written to {path}"),
        Err(err) => eprintln!("Failed to write thread dump to {path}: {err}"),
    }

    // Capture full userspace backtraces via `sample` (macOS built-in).
    // `sample <pid> 1` captures a 1-second sampling profile of all threads
    // including symbolicated call stacks. This is the macOS equivalent of
    // eu-stack on Linux.
    let bt_path = format!("/tmp/nativelink-stall-{timestamp_ms}-bt.txt");
    match std::process::Command::new("sample")
        .args([&pid.to_string(), "1", "-mayDie"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(mut child) => {
            const SAMPLE_TIMEOUT: Duration = Duration::from_secs(30);
            const POLL_INTERVAL: Duration = Duration::from_millis(250);
            let deadline = std::time::Instant::now() + SAMPLE_TIMEOUT;
            let status = loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Some(status),
                    Ok(None) => {
                        if std::time::Instant::now() >= deadline {
                            eprintln!(
                                "sample timed out after {SAMPLE_TIMEOUT:.0?}, killing child process"
                            );
                            drop(child.kill());
                            drop(child.wait());
                            break None;
                        }
                        std::thread::sleep(POLL_INTERVAL);
                    }
                    Err(err) => {
                        eprintln!("sample wait error: {err}");
                        drop(child.kill());
                        drop(child.wait());
                        break None;
                    }
                }
            };
            if status.is_some() {
                let stdout = child
                    .stdout
                    .take()
                    .map(|mut r| {
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut r, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();
                let stderr = child
                    .stderr
                    .take()
                    .map(|mut r| {
                        let mut buf = Vec::new();
                        std::io::Read::read_to_end(&mut r, &mut buf).ok();
                        buf
                    })
                    .unwrap_or_default();
                let combined = [&stdout[..], b"\n--- stderr ---\n", &stderr[..]].concat();
                match std::fs::write(&bt_path, &combined) {
                    Ok(()) => eprintln!("Userspace sample written to {bt_path}"),
                    Err(err) => eprintln!("Failed to write sample to {bt_path}: {err}"),
                }
            }
        }
        Err(err) => eprintln!("Failed to run sample: {err}"),
    }

    cleanup_old_stall_dumps();
}

/// Enumerate all threads in the current task using Mach APIs and write
/// their names and basic info to the output buffer.
#[cfg(target_os = "macos")]
fn enumerate_mach_threads(output: &mut String) {
    use std::fmt::Write as _;

    // Mach types and constants
    type MachPort = u32;
    type KernReturn = i32;
    const KERN_SUCCESS: KernReturn = 0;
    const THREAD_BASIC_INFO: u32 = 3;
    const THREAD_BASIC_INFO_COUNT: u32 = 10; // sizeof(thread_basic_info) / sizeof(natural_t)

    // Mach thread run states
    const TH_STATE_RUNNING: i32 = 1;
    const TH_STATE_STOPPED: i32 = 2;
    const TH_STATE_WAITING: i32 = 3;
    const TH_STATE_UNINTERRUPTIBLE: i32 = 4;
    const TH_STATE_HALTED: i32 = 5;

    #[repr(C)]
    #[derive(Default)]
    struct ThreadBasicInfo {
        user_time_sec: i32,
        user_time_usec: i32,
        system_time_sec: i32,
        system_time_usec: i32,
        cpu_usage: i32,   // scaled to TH_USAGE_SCALE (1000)
        policy: i32,
        run_state: i32,
        flags: i32,
        suspend_count: i32,
        sleep_time: i32,
    }

    unsafe extern "C" {
        fn mach_task_self() -> MachPort;
        fn task_threads(
            task: MachPort,
            thread_list: *mut *mut MachPort,
            thread_count: *mut u32,
        ) -> KernReturn;
        fn thread_info(
            thread: MachPort,
            flavor: u32,
            info: *mut i32,
            count: *mut u32,
        ) -> KernReturn;
        // Returns the pthread_t for the given Mach thread port, or 0 if
        // the port does not correspond to a known pthread.
        fn pthread_from_mach_thread_np(thread: MachPort) -> libc::pthread_t;
        fn mach_port_deallocate(task: MachPort, name: MachPort) -> KernReturn;
        fn vm_deallocate(task: MachPort, address: usize, size: usize) -> KernReturn;
    }

    let task = unsafe { mach_task_self() };
    let mut thread_list: *mut MachPort = core::ptr::null_mut();
    let mut thread_count: u32 = 0;

    let kr = unsafe { task_threads(task, &mut thread_list, &mut thread_count) };
    if kr != KERN_SUCCESS {
        let _ = writeln!(output, "Failed to enumerate threads: mach error {kr}");
        return;
    }

    let _ = writeln!(output, "Thread count: {thread_count}");
    let _ = writeln!(output);

    let threads =
        unsafe { core::slice::from_raw_parts(thread_list, thread_count as usize) };

    for (idx, &thread_port) in threads.iter().enumerate() {
        let _ = write!(output, "--- Thread {idx} (mach port {thread_port}) ---");

        // Get thread name via pthread. pthread_from_mach_thread_np returns
        // 0 (null pthread_t) if the Mach thread has no associated pthread.
        let pthread = unsafe { pthread_from_mach_thread_np(thread_port) };
        if pthread != 0 {
            let mut name_buf = [0u8; 64];
            let ret = unsafe {
                libc::pthread_getname_np(
                    pthread,
                    name_buf.as_mut_ptr().cast(),
                    name_buf.len(),
                )
            };
            if ret == 0 {
                let name = std::ffi::CStr::from_bytes_until_nul(&name_buf)
                    .map(|c| c.to_string_lossy())
                    .unwrap_or_default();
                if !name.is_empty() {
                    let _ = write!(output, "  name: {name}");
                }
            }
        }
        let _ = writeln!(output);

        // Get thread basic info (CPU time, run state)
        let mut info = ThreadBasicInfo::default();
        let mut count = THREAD_BASIC_INFO_COUNT;
        let kr = unsafe {
            thread_info(
                thread_port,
                THREAD_BASIC_INFO,
                core::ptr::from_mut(&mut info).cast(),
                &mut count,
            )
        };
        if kr == KERN_SUCCESS {
            let user_ms =
                i64::from(info.user_time_sec) * 1000 + i64::from(info.user_time_usec) / 1000;
            let sys_ms = i64::from(info.system_time_sec) * 1000
                + i64::from(info.system_time_usec) / 1000;
            let state_str = match info.run_state {
                TH_STATE_RUNNING => "running",
                TH_STATE_STOPPED => "stopped",
                TH_STATE_WAITING => "waiting",
                TH_STATE_UNINTERRUPTIBLE => "uninterruptible",
                TH_STATE_HALTED => "halted",
                _ => "unknown",
            };
            let _ = writeln!(
                output,
                "  state: {state_str}  cpu_usage: {:.1}%  user: {user_ms}ms  sys: {sys_ms}ms  suspend_count: {}",
                f64::from(info.cpu_usage) / 10.0,
                info.suspend_count,
            );
        }

        // Deallocate the thread port send right
        unsafe {
            mach_port_deallocate(task, thread_port);
        }

        let _ = writeln!(output);
    }

    // Deallocate the thread list memory (allocated by Mach)
    if !thread_list.is_null() && thread_count > 0 {
        unsafe {
            vm_deallocate(
                task,
                thread_list as usize,
                thread_count as usize * core::mem::size_of::<MachPort>(),
            );
        }
    }
}

/// Maximum number of stall dump file pairs to retain. Older dumps are
/// deleted after each new dump is written.
const MAX_STALL_DUMPS: usize = 10;

/// Remove old stall dump files, keeping the newest [`MAX_STALL_DUMPS`] pairs.
/// Each dump produces two files (`-<ts>.txt` and `-<ts>-bt.txt`), so we
/// keep up to `MAX_STALL_DUMPS * 2` files total.
fn cleanup_old_stall_dumps() {
    let tmp = std::path::Path::new("/tmp");
    let entries = match std::fs::read_dir(tmp) {
        Ok(e) => e,
        Err(err) => {
            eprintln!("stall dump cleanup: failed to read /tmp: {err}");
            return;
        }
    };

    let mut stall_files: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map_or(false, |n| n.starts_with("nativelink-stall-") && n.ends_with(".txt"))
        })
        .collect();

    // Each dump pair shares a timestamp, so sorting by filename (which
    // embeds the millisecond timestamp) gives chronological order.
    stall_files.sort();

    let max_files = MAX_STALL_DUMPS * 2;
    if stall_files.len() <= max_files {
        return;
    }

    let to_remove = stall_files.len() - max_files;
    for file in &stall_files[..to_remove] {
        if let Err(err) = std::fs::remove_file(file) {
            eprintln!("stall dump cleanup: failed to remove {}: {err}", file.display());
        }
    }
    eprintln!("stall dump cleanup: removed {to_remove} old dump files, kept {MAX_STALL_DUMPS} newest pairs");
}
