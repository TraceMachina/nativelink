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
        let handle = tokio::spawn(async move {
            tokio::time::sleep(threshold).await;
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
                    "STORE OPERATION STALL: {label} has been running for >{threshold:.0?} — dumping thread stacks",
                );
                dump_thread_stacks(label);
            } else {
                eprintln!(
                    "STORE OPERATION STALL: {label} has been running for >{threshold:.0?} (dump rate-limited)",
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
/// On non-Linux platforms, this is a no-op (logs a message).
pub fn dump_thread_stacks(label: &str) {
    #[cfg(target_os = "linux")]
    dump_thread_stacks_linux(label);

    #[cfg(not(target_os = "linux"))]
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
    let bt_path = format!("/tmp/nativelink-stall-{timestamp_ms}-bt.txt");
    let pid = std::process::id();
    match std::process::Command::new("eu-stack")
        .args(["-p", &pid.to_string(), "-l"])
        .output()
    {
        Ok(out) => {
            let combined = [&out.stdout[..], b"\n--- stderr ---\n", &out.stderr[..]].concat();
            match std::fs::write(&bt_path, &combined) {
                Ok(()) => eprintln!("Userspace backtrace written to {bt_path}"),
                Err(err) => eprintln!("Failed to write backtrace to {bt_path}: {err}"),
            }
        }
        Err(err) => eprintln!("Failed to run eu-stack: {err}"),
    }
}
