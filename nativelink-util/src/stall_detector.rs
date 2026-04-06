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

/// Cooperative signal-based thread stack dumper for Linux.
///
/// Instead of spawning eu-stack (which takes 30s+ and can hang), we:
/// 1. Enumerate threads via /proc/self/task/
/// 2. Collect kernel-level info (comm, wchan, state, kernel stack)
/// 3. Send a realtime signal to each thread via tgkill()
/// 4. Each thread's signal handler captures its own backtrace (unresolved)
/// 5. Collector waits for all threads to respond (with timeout)
/// 6. Resolve symbols in bulk, format output
///
/// Total time: typically <100ms for hundreds of threads.
#[cfg(target_os = "linux")]
mod signal_dumper {
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
    use std::sync::Once;

    /// Maximum threads we can capture backtraces from in a single dump.
    /// Pre-allocated to avoid allocation in the signal handler.
    const MAX_THREADS: usize = 1024;

    /// Signal used for cooperative stack capture. SIGRTMIN is often used
    /// by glibc/pthreads internally, so we use SIGRTMIN + 1.
    fn dump_signal() -> i32 {
        libc::SIGRTMIN() + 1
    }

    /// A single slot for one thread's captured backtrace.
    ///
    /// The signal handler writes raw instruction pointer addresses here.
    /// We avoid using `backtrace::Backtrace` directly in the handler
    /// because its internal Vec allocation may not be async-signal-safe
    /// under all allocators. Instead we capture raw IPs into a fixed
    /// array, then build Backtrace frames after collection.
    struct BacktraceSlot {
        /// Raw instruction pointer addresses captured by the signal handler.
        ips: [usize; 128],
        /// Number of valid entries in `ips`.
        count: usize,
        /// TID that this slot belongs to (set before signaling).
        tid: u32,
        /// Set to true by the signal handler after capture completes.
        captured: AtomicBool,
    }

    impl BacktraceSlot {
        const fn empty() -> Self {
            Self {
                ips: [0; 128],
                count: 0,
                tid: 0,
                captured: AtomicBool::new(false),
            }
        }

        fn reset(&mut self, tid: u32) {
            self.count = 0;
            self.tid = tid;
            self.captured.store(false, Ordering::Release);
        }
    }

    /// Global state for the signal-based backtrace collector.
    ///
    /// Only one dump can be in progress at a time (enforced by
    /// `DUMP_IN_PROGRESS`). The collector thread sets up the slots,
    /// sends signals, and waits. Signal handlers write to their
    /// assigned slot.
    struct Collector {
        slots: [std::cell::UnsafeCell<BacktraceSlot>; MAX_THREADS],
        /// Number of active slots in this dump round.
        active_count: AtomicUsize,
        /// Number of threads that have finished capturing.
        done_count: AtomicUsize,
    }

    // SAFETY: The slots are only written to by their owning thread's
    // signal handler (one writer per slot), and read by the collector
    // after all signal handlers have completed or timed out. The
    // AtomicBool in each slot provides the synchronization barrier.
    unsafe impl Sync for Collector {}
    unsafe impl Send for Collector {}

    impl Collector {
        const fn new() -> Self {
            // Use a macro to repeat the UnsafeCell initialization
            // since UnsafeCell::new is not Copy.
            const EMPTY_CELL: std::cell::UnsafeCell<BacktraceSlot> =
                std::cell::UnsafeCell::new(BacktraceSlot::empty());
            Self {
                slots: [EMPTY_CELL; MAX_THREADS],
                active_count: AtomicUsize::new(0),
                done_count: AtomicUsize::new(0),
            }
        }
    }

    static COLLECTOR: Collector = Collector::new();
    static SIGNAL_INSTALLED: Once = Once::new();
    static DUMP_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

    /// Maps a TID to its slot index. Called from the signal handler
    /// and from the collector setup. Must be consistent.
    ///
    /// We store the TID->index mapping in each slot's `tid` field and
    /// the signal handler searches linearly. With MAX_THREADS=1024 and
    /// typical thread counts of 50-300, this is fast enough for a
    /// signal handler (~1us).
    static SLOT_COUNT: AtomicU32 = AtomicU32::new(0);

    fn find_slot_for_tid(tid: u32) -> Option<usize> {
        let count = SLOT_COUNT.load(Ordering::Acquire) as usize;
        for i in 0..count {
            // SAFETY: We only read the tid field, which was set before
            // signaling and won't be modified until the dump is done.
            let slot = unsafe { &*COLLECTOR.slots[i].get() };
            if slot.tid == tid {
                return Some(i);
            }
        }
        None
    }

    /// Signal handler invoked on the target thread. Captures raw
    /// instruction pointers using `backtrace::trace_unsynchronized`.
    ///
    /// SAFETY requirements for async-signal-safety:
    /// - No heap allocation (we write to pre-allocated fixed array)
    /// - No locks (we use atomic flag for completion)
    /// - `backtrace::trace_unsynchronized` walks the stack using
    ///   frame pointers or DWARF unwind info without allocating
    unsafe extern "C" fn signal_handler(
        _sig: libc::c_int,
        _info: *mut libc::siginfo_t,
        _ctx: *mut libc::c_void,
    ) {
        // SAFETY: SYS_gettid always succeeds and returns the caller's TID.
        let tid = unsafe { libc::syscall(libc::SYS_gettid) } as u32;
        let Some(idx) = find_slot_for_tid(tid) else {
            return;
        };
        // SAFETY: Each slot is exclusively owned by the thread whose TID
        // matches slot.tid. The collector thread set up the slot before
        // sending the signal, and won't read it until captured=true.
        let slot = unsafe { &mut *COLLECTOR.slots[idx].get() };

        // Capture raw instruction pointers without resolving symbols.
        // trace_unsynchronized is the non-locking variant suitable for
        // signal handlers.
        let mut count = 0usize;
        let max = slot.ips.len();
        // SAFETY: We are in a signal handler context. trace_unsynchronized
        // is the correct function here — it skips internal locks that
        // trace() would take (which could deadlock in a signal handler).
        // We write only to pre-allocated stack-local and slot memory.
        unsafe {
            backtrace::trace_unsynchronized(|frame| {
                if count < max {
                    slot.ips[count] = frame.ip() as usize;
                    count += 1;
                    true
                } else {
                    false
                }
            });
        }
        slot.count = count;
        slot.captured.store(true, Ordering::Release);
        COLLECTOR.done_count.fetch_add(1, Ordering::Release);
    }

    /// Install the signal handler (once).
    fn install_signal_handler() {
        SIGNAL_INSTALLED.call_once(|| {
            unsafe {
                let mut sa: libc::sigaction = core::mem::zeroed();
                sa.sa_sigaction = signal_handler as *const () as usize;
                sa.sa_flags = libc::SA_RESTART | libc::SA_SIGINFO;
                libc::sigemptyset(&mut sa.sa_mask);
                let ret = libc::sigaction(dump_signal(), &sa, core::ptr::null_mut());
                if ret != 0 {
                    eprintln!(
                        "failed to install backtrace signal handler: {}",
                        std::io::Error::last_os_error()
                    );
                }
            }
        });
    }

    /// Resolved backtrace for one thread.
    pub(super) struct ThreadBacktrace {
        pub tid: u32,
        pub symbols: Vec<ResolvedFrame>,
    }

    /// A single resolved stack frame.
    pub(super) struct ResolvedFrame {
        pub ip: usize,
        pub name: Option<String>,
        pub filename: Option<String>,
        pub lineno: Option<u32>,
    }

    /// Capture backtraces from all threads cooperatively.
    ///
    /// Returns a vec of per-thread resolved backtraces. Threads that
    /// did not respond within the timeout are omitted.
    pub(super) fn capture_all_backtraces(
        tids: &[u32],
    ) -> Vec<ThreadBacktrace> {
        install_signal_handler();

        // Only one dump at a time.
        if DUMP_IN_PROGRESS.swap(true, Ordering::SeqCst) {
            eprintln!("cooperative stack dump already in progress, skipping");
            return Vec::new();
        }

        // Ensure we clear the in-progress flag when done.
        struct DumpGuard;
        impl Drop for DumpGuard {
            fn drop(&mut self) {
                DUMP_IN_PROGRESS.store(false, Ordering::SeqCst);
            }
        }
        let _guard = DumpGuard;

        let thread_count = tids.len().min(MAX_THREADS);
        SLOT_COUNT.store(thread_count as u32, Ordering::Release);
        COLLECTOR.active_count.store(thread_count, Ordering::Release);
        COLLECTOR.done_count.store(0, Ordering::Release);

        // Initialize slots.
        for (i, &tid) in tids.iter().take(thread_count).enumerate() {
            // SAFETY: No signal handler is accessing these slots yet
            // because we haven't sent any signals.
            unsafe {
                (*COLLECTOR.slots[i].get()).reset(tid);
            }
        }

        // Send signal to each thread.
        let pid = std::process::id() as i32;
        let sig = dump_signal();
        let mut signaled = 0u32;
        for &tid in tids.iter().take(thread_count) {
            let ret = unsafe {
                libc::syscall(libc::SYS_tgkill, pid, tid as i32, sig)
            };
            if ret == 0 {
                signaled += 1;
            }
            // Thread may have exited between enumeration and signal —
            // that's fine, we just won't get its backtrace.
        }

        // Wait for threads to respond, with timeout.
        const TIMEOUT: core::time::Duration = core::time::Duration::from_secs(5);
        const POLL_INTERVAL: core::time::Duration =
            core::time::Duration::from_millis(1);
        let deadline = std::time::Instant::now() + TIMEOUT;

        while COLLECTOR.done_count.load(Ordering::Acquire) < signaled as usize {
            if std::time::Instant::now() >= deadline {
                let done = COLLECTOR.done_count.load(Ordering::Acquire);
                eprintln!(
                    "backtrace capture timeout: {done}/{signaled} threads responded in {TIMEOUT:.0?}"
                );
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }

        // Collect and resolve backtraces.
        let mut results = Vec::with_capacity(thread_count);
        for i in 0..thread_count {
            // SAFETY: Signal handlers have either completed (captured=true)
            // or timed out. We only read slots that are marked captured.
            let slot = unsafe { &*COLLECTOR.slots[i].get() };
            if !slot.captured.load(Ordering::Acquire) {
                // Thread didn't respond (D state, exited, etc.)
                results.push(ThreadBacktrace {
                    tid: slot.tid,
                    symbols: Vec::new(),
                });
                continue;
            }

            // Resolve symbols for each instruction pointer.
            let mut frames = Vec::with_capacity(slot.count);
            for j in 0..slot.count {
                let ip = slot.ips[j];
                let mut resolved = ResolvedFrame {
                    ip,
                    name: None,
                    filename: None,
                    lineno: None,
                };
                // backtrace::resolve takes a *mut c_void pointer.
                backtrace::resolve(ip as *mut core::ffi::c_void, |symbol| {
                    if resolved.name.is_none() {
                        resolved.name =
                            symbol.name().map(|n| n.to_string());
                    }
                    if resolved.filename.is_none() {
                        resolved.filename = symbol
                            .filename()
                            .map(|p| p.display().to_string());
                    }
                    if resolved.lineno.is_none() {
                        resolved.lineno = symbol.lineno();
                    }
                });
                frames.push(resolved);
            }
            results.push(ThreadBacktrace {
                tid: slot.tid,
                symbols: frames,
            });
        }

        results
    }
}

#[cfg(target_os = "linux")]
fn dump_thread_stacks_linux(label: &str) {
    use std::fmt::Write as _;

    let start = std::time::Instant::now();
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

    let mut tids: Vec<u32> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .collect();
    tids.sort();

    let _ = writeln!(output, "Thread count: {}", tids.len());
    let _ = writeln!(output);

    // Phase 1: Collect kernel-level info from /proc (fast, <10ms).
    // Build a map of tid -> (comm, kernel info) for later merging.
    let mut thread_names: std::collections::HashMap<u32, String> =
        std::collections::HashMap::new();

    for &tid in &tids {
        let base = format!("{task_dir}/{tid}");

        // Thread name
        let comm = std::fs::read_to_string(format!("{base}/comm"))
            .unwrap_or_default()
            .trim()
            .to_string();
        if !comm.is_empty() {
            thread_names.insert(tid, comm.clone());
        }
    }

    // Phase 2: Cooperative signal-based backtrace capture.
    let backtraces = signal_dumper::capture_all_backtraces(&tids);
    let capture_elapsed = start.elapsed();

    // Build a lookup from TID -> backtrace for output formatting.
    let bt_map: std::collections::HashMap<u32, &signal_dumper::ThreadBacktrace> =
        backtraces.iter().map(|bt| (bt.tid, bt)).collect();

    // Phase 3: Format combined output (kernel info + userspace backtrace).
    for &tid in &tids {
        let tid_str = tid.to_string();
        let base = format!("{task_dir}/{tid_str}");
        let comm = thread_names
            .get(&tid)
            .map(String::as_str)
            .unwrap_or("<unknown>");

        let _ = writeln!(output, "--- TID {tid} ({comm}) ---");

        // Wait channel
        if let Ok(wchan) = std::fs::read_to_string(format!("{base}/wchan")) {
            let wchan = wchan.trim();
            if !wchan.is_empty() && wchan != "0" {
                let _ = writeln!(output, "  wchan: {wchan}");
            }
        }
        // Status lines
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
        // Kernel stack
        if let Ok(stack) = std::fs::read_to_string(format!("{base}/stack")) {
            let trimmed = stack.trim();
            if !trimmed.is_empty() {
                let _ = writeln!(output, "  kernel stack:");
                for line in trimmed.lines() {
                    let _ = writeln!(output, "    {line}");
                }
            }
        }

        // Userspace backtrace from cooperative capture.
        if let Some(bt) = bt_map.get(&tid) {
            if bt.symbols.is_empty() {
                let _ = writeln!(output, "  userspace backtrace: <no response>");
            } else {
                let _ = writeln!(output, "  userspace backtrace:");
                for (i, frame) in bt.symbols.iter().enumerate() {
                    let name = frame.name.as_deref().unwrap_or("<unknown>");
                    if let (Some(file), Some(line)) =
                        (&frame.filename, frame.lineno)
                    {
                        let _ = writeln!(
                            output,
                            "    #{i:>3} {:#018x} {name}",
                            frame.ip
                        );
                        let _ = writeln!(
                            output,
                            "         at {file}:{line}"
                        );
                    } else {
                        let _ = writeln!(
                            output,
                            "    #{i:>3} {:#018x} {name}",
                            frame.ip
                        );
                    }
                }
            }
        }

        let _ = writeln!(output);
    }

    let total_elapsed = start.elapsed();
    let responded = backtraces.iter().filter(|bt| !bt.symbols.is_empty()).count();
    let _ = writeln!(
        output,
        "=== Dump complete: {responded}/{} threads responded, capture: {capture_elapsed:.1?}, total: {total_elapsed:.1?} ===",
        tids.len()
    );

    match std::fs::write(&path, &output) {
        Ok(()) => eprintln!(
            "Thread dump written to {path} ({responded}/{} threads, {total_elapsed:.1?})",
            tids.len()
        ),
        Err(err) => eprintln!("Failed to write thread dump to {path}: {err}"),
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
