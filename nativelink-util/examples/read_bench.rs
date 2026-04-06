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

//! Benchmark comparing io_uring pipelined reads vs spawn_blocking for small and
//! medium files at various concurrency levels.
//!
//! Run with:
//!   cargo run -p nativelink-util --example read_bench --release --features io-uring
//!
//! The benchmark answers: at 1K/10K concurrent reads of 100-byte files, is
//! io_uring faster or slower than spawn_blocking? Where is the crossover?

use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::stream::{FuturesUnordered, StreamExt};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

const SMALL_FILE_SIZE: usize = 100;
const MEDIUM_FILE_SIZE: usize = 1_024 * 1_024; // 1 MiB
const NUM_SMALL_FILES: usize = 1_000;
const NUM_MEDIUM_FILES: usize = 100;

const BENCH_DIR: &str = "/tmp/nativelink-bench";

/// Concurrency levels to test for small files.
const SMALL_CONCURRENCIES: &[usize] = &[1_000, 10_000];
/// Concurrency levels to test for medium files.
const MEDIUM_CONCURRENCIES: &[usize] = &[100];

/// Number of warmup iterations before measurement.
const WARMUP_ITERS: usize = 2;
/// Number of measured iterations.
const MEASURE_ITERS: usize = 5;

// ---------------------------------------------------------------------------
// Latency statistics
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct LatencyStats {
    count: usize,
    total: Duration,
    avg: Duration,
    p50: Duration,
    p99: Duration,
    max: Duration,
}

fn compute_stats(mut latencies: Vec<Duration>) -> LatencyStats {
    assert!(!latencies.is_empty());
    latencies.sort();
    let count = latencies.len();
    let total: Duration = latencies.iter().sum();
    let avg = total / count as u32;
    let p50 = latencies[count / 2];
    let p99 = latencies[(count as f64 * 0.99) as usize];
    let max = *latencies.last().unwrap();
    LatencyStats {
        count,
        total,
        avg,
        p50,
        p99,
        max,
    }
}

fn print_stats(label: &str, stats: &LatencyStats) {
    println!(
        "  {label:<55} n={:<6} total={:>10.3?}  avg={:>10.3?}  p50={:>10.3?}  p99={:>10.3?}  max={:>10.3?}",
        stats.count, stats.total, stats.avg, stats.p50, stats.p99, stats.max
    );
}

fn print_throughput(label: &str, total_bytes: u64, wall: Duration) {
    let mb = total_bytes as f64 / (1024.0 * 1024.0);
    let secs = wall.as_secs_f64();
    let mbps = if secs > 0.0 { mb / secs } else { 0.0 };
    println!("  {label:<55} {mb:.2} MiB in {secs:.3}s = {mbps:.1} MiB/s");
}

// ---------------------------------------------------------------------------
// File setup / teardown
// ---------------------------------------------------------------------------

fn setup_files(dir: &Path, prefix: &str, count: usize, size: usize) -> Vec<PathBuf> {
    std::fs::create_dir_all(dir).expect("create bench dir");
    let data = vec![0xABu8; size];
    (0..count)
        .map(|i| {
            let p = dir.join(format!("{prefix}_{i:06}"));
            let mut f = std::fs::File::create(&p).expect("create file");
            f.write_all(&data).expect("write file");
            p
        })
        .collect()
}

fn warmup_page_cache(paths: &[PathBuf]) {
    for p in paths {
        drop(std::fs::read(p));
    }
}

fn cleanup() {
    drop(std::fs::remove_dir_all(BENCH_DIR));
}

// ---------------------------------------------------------------------------
// Benchmark 1: io_uring pipelined reads
// ---------------------------------------------------------------------------

#[cfg(all(feature = "io-uring", target_os = "linux"))]
async fn bench_io_uring(paths: &[PathBuf], file_size: usize, concurrency: usize) -> LatencyStats {
    let system = tokio_epoll_uring::thread_local_system().await;
    let mut opts = tokio_epoll_uring::ops::open_at::OpenOptions::new();
    opts.read(true);

    // Pre-open all files via io_uring.
    let mut fds: Vec<Arc<std::os::fd::OwnedFd>> = Vec::with_capacity(paths.len());
    for path in paths {
        let fd = system
            .open(path, &opts)
            .await
            .expect("io_uring open failed");
        fds.push(Arc::new(fd));
    }

    let mut latencies = Vec::with_capacity(concurrency);
    let mut in_flight = FuturesUnordered::new();

    for i in 0..concurrency {
        let fd = Arc::clone(&fds[i % fds.len()]);
        let buf = Vec::with_capacity(file_size);
        let start = Instant::now();
        let read_fut = system.read(fd, 0u64, buf);
        in_flight.push(async move {
            let ((_fd, returned_buf), result) = read_fut.await;
            let elapsed = start.elapsed();
            let n = result.expect("io_uring read failed");
            assert_eq!(n, returned_buf.len().min(file_size));
            elapsed
        });
    }

    while let Some(elapsed) = in_flight.next().await {
        latencies.push(elapsed);
    }

    compute_stats(latencies)
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
async fn bench_io_uring(_paths: &[PathBuf], _file_size: usize, _concurrency: usize) -> LatencyStats {
    eprintln!("  [SKIPPED] io_uring not available (compile with --features io-uring on Linux)");
    compute_stats(vec![Duration::ZERO])
}

// ---------------------------------------------------------------------------
// Benchmark 2: spawn_blocking + std::fs::read
// ---------------------------------------------------------------------------

async fn bench_spawn_blocking_fs_read(
    paths: &[PathBuf],
    _file_size: usize,
    concurrency: usize,
) -> LatencyStats {
    let mut latencies = Vec::with_capacity(concurrency);
    let mut in_flight = FuturesUnordered::new();

    for i in 0..concurrency {
        let path = paths[i % paths.len()].clone();
        let start = Instant::now();
        in_flight.push(tokio::task::spawn_blocking(move || {
            let data = std::fs::read(&path).expect("fs::read failed");
            let elapsed = start.elapsed();
            assert!(!data.is_empty());
            elapsed
        }));
    }

    while let Some(result) = in_flight.next().await {
        latencies.push(result.expect("spawn_blocking join failed"));
    }

    compute_stats(latencies)
}

// ---------------------------------------------------------------------------
// Benchmark 3: spawn_blocking + pread with pre-opened fd
// ---------------------------------------------------------------------------

async fn bench_spawn_blocking_pread(
    paths: &[PathBuf],
    file_size: usize,
    concurrency: usize,
) -> LatencyStats {
    // Pre-open all files.
    let files: Vec<Arc<std::fs::File>> = paths
        .iter()
        .map(|p| Arc::new(std::fs::File::open(p).expect("open failed")))
        .collect();

    let mut latencies = Vec::with_capacity(concurrency);
    let mut in_flight = FuturesUnordered::new();

    for i in 0..concurrency {
        let file = Arc::clone(&files[i % files.len()]);
        let size = file_size;
        let start = Instant::now();
        in_flight.push(tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; size];
            let n = unsafe {
                libc::pread(
                    file.as_raw_fd(),
                    buf.as_mut_ptr() as *mut libc::c_void,
                    size,
                    0,
                )
            };
            let elapsed = start.elapsed();
            assert!(n > 0, "pread returned {n}");
            elapsed
        }));
    }

    while let Some(result) = in_flight.next().await {
        latencies.push(result.expect("spawn_blocking join failed"));
    }

    compute_stats(latencies)
}

// ---------------------------------------------------------------------------
// Benchmark 5: io_uring direct — batched submission, proper usage
// ---------------------------------------------------------------------------

#[cfg(all(feature = "io-uring", target_os = "linux"))]
fn bench_io_uring_direct(
    paths: &[PathBuf],
    file_size: usize,
    concurrency: usize,
) -> LatencyStats {
    use io_uring::{IoUring, opcode, types};
    use std::os::unix::io::AsRawFd;

    // Pre-open all files.
    let files: Vec<std::fs::File> = paths
        .iter()
        .map(|p| std::fs::File::open(p).expect("open failed"))
        .collect();

    let ring_size = concurrency.next_power_of_two().max(64) as u32;
    let ring_size = ring_size.min(4096); // kernel limit
    let mut ring = IoUring::new(ring_size).expect("io_uring::new failed");

    // Allocate all buffers upfront.
    let mut bufs: Vec<Vec<u8>> = (0..concurrency)
        .map(|_| vec![0u8; file_size])
        .collect();

    let mut latencies = Vec::with_capacity(concurrency);
    let mut submitted = 0usize;
    let mut completed = 0usize;
    let mut starts: Vec<Instant> = vec![Instant::now(); concurrency];

    // Fill the SQ with as many reads as we can, then submit in one batch.
    while submitted < concurrency {
        // Fill SQ
        {
            let (submitter, mut sq, _cq) = ring.split();
            sq.sync();
            let sq_space = sq.capacity() - sq.len();
            let to_submit = (concurrency - submitted).min(sq_space);

            for _ in 0..to_submit {
                let idx = submitted;
                let fd = files[idx % files.len()].as_raw_fd();
                let buf = &mut bufs[idx];
                let sqe = opcode::Read::new(
                    types::Fd(fd),
                    buf.as_mut_ptr(),
                    buf.len() as _,
                )
                .offset(0)
                .build()
                .user_data(idx as u64);

                starts[idx] = Instant::now();
                unsafe { sq.push(&sqe).expect("SQ full despite capacity check") };
                submitted += 1;
            }
            sq.sync();

            // One io_uring_enter for the whole batch.
            submitter.submit().expect("io_uring submit failed");
        }

        // Reap completions.
        let mut cq = ring.completion();
        cq.sync();
        for cqe in cq {
            let idx = cqe.user_data() as usize;
            let elapsed = starts[idx].elapsed();
            let n = cqe.result();
            assert!(n > 0, "io_uring read returned {n} for idx {idx}");
            latencies.push(elapsed);
            completed += 1;
        }
    }

    // Drain remaining completions.
    while completed < concurrency {
        ring.submit_and_wait(1).expect("submit_and_wait failed");
        ring.completion().sync();
        for cqe in ring.completion() {
            let idx = cqe.user_data() as usize;
            let elapsed = starts[idx].elapsed();
            latencies.push(elapsed);
            completed += 1;
        }
    }

    compute_stats(latencies)
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
fn bench_io_uring_direct(
    _paths: &[PathBuf],
    _file_size: usize,
    _concurrency: usize,
) -> LatencyStats {
    eprintln!("  [SKIPPED] io_uring not available");
    compute_stats(vec![Duration::ZERO])
}

// ---------------------------------------------------------------------------
// Benchmark 4: Sequential synchronous baseline
// ---------------------------------------------------------------------------

fn bench_sequential_sync(paths: &[PathBuf], concurrency: usize) -> LatencyStats {
    let mut latencies = Vec::with_capacity(concurrency);

    for i in 0..concurrency {
        let path = &paths[i % paths.len()];
        let start = Instant::now();
        let data = std::fs::read(path).expect("fs::read failed");
        let elapsed = start.elapsed();
        assert!(!data.is_empty());
        latencies.push(elapsed);
    }

    compute_stats(latencies)
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

async fn run_bench_suite(
    label: &str,
    paths: &[PathBuf],
    file_size: usize,
    concurrency: usize,
) {
    let total_bytes = (concurrency * file_size) as u64;

    println!("\n--- {label} | concurrency={concurrency} | file_size={file_size}B ---");

    // --- Benchmark 1: io_uring ---
    for _ in 0..WARMUP_ITERS {
        bench_io_uring(paths, file_size, concurrency).await;
    }
    let mut best_uring: Option<LatencyStats> = None;
    for _ in 0..MEASURE_ITERS {
        let wall_start = Instant::now();
        let stats = bench_io_uring(paths, file_size, concurrency).await;
        let wall = wall_start.elapsed();
        print_stats("io_uring pipelined", &stats);
        print_throughput("io_uring pipelined", total_bytes, wall);
        if best_uring.as_ref().map_or(true, |b| stats.total < b.total) {
            best_uring = Some(stats);
        }
    }

    // --- Benchmark 2: spawn_blocking + fs::read ---
    for _ in 0..WARMUP_ITERS {
        bench_spawn_blocking_fs_read(paths, file_size, concurrency).await;
    }
    let mut best_sb_read: Option<LatencyStats> = None;
    for _ in 0..MEASURE_ITERS {
        let wall_start = Instant::now();
        let stats = bench_spawn_blocking_fs_read(paths, file_size, concurrency).await;
        let wall = wall_start.elapsed();
        print_stats("spawn_blocking + fs::read", &stats);
        print_throughput("spawn_blocking + fs::read", total_bytes, wall);
        if best_sb_read
            .as_ref()
            .map_or(true, |b| stats.total < b.total)
        {
            best_sb_read = Some(stats);
        }
    }

    // --- Benchmark 3: spawn_blocking + pread ---
    for _ in 0..WARMUP_ITERS {
        bench_spawn_blocking_pread(paths, file_size, concurrency).await;
    }
    let mut best_sb_pread: Option<LatencyStats> = None;
    for _ in 0..MEASURE_ITERS {
        let wall_start = Instant::now();
        let stats = bench_spawn_blocking_pread(paths, file_size, concurrency).await;
        let wall = wall_start.elapsed();
        print_stats("spawn_blocking + pread", &stats);
        print_throughput("spawn_blocking + pread", total_bytes, wall);
        if best_sb_pread
            .as_ref()
            .map_or(true, |b| stats.total < b.total)
        {
            best_sb_pread = Some(stats);
        }
    }

    // --- Benchmark 5: io_uring direct (batched) ---
    for _ in 0..WARMUP_ITERS {
        bench_io_uring_direct(paths, file_size, concurrency);
    }
    let mut best_uring_direct: Option<LatencyStats> = None;
    for _ in 0..MEASURE_ITERS {
        let wall_start = Instant::now();
        let stats = bench_io_uring_direct(paths, file_size, concurrency);
        let wall = wall_start.elapsed();
        print_stats("io_uring DIRECT (batched)", &stats);
        print_throughput("io_uring DIRECT (batched)", total_bytes, wall);
        if best_uring_direct
            .as_ref()
            .map_or(true, |b| stats.total < b.total)
        {
            best_uring_direct = Some(stats);
        }
    }

    // --- Benchmark 4: Sequential sync baseline ---
    // Only run at lower concurrency to avoid taking too long.
    if concurrency <= 1_000 {
        let wall_start = Instant::now();
        let stats = bench_sequential_sync(paths, concurrency);
        let wall = wall_start.elapsed();
        print_stats("sequential sync (baseline)", &stats);
        print_throughput("sequential sync (baseline)", total_bytes, wall);
    }

    // --- Summary ---
    println!("\n  BEST results (lowest total latency):");
    if let Some(ref s) = best_uring {
        print_stats("  io_uring (tokio-epoll-uring)", s);
    }
    if let Some(ref s) = best_uring_direct {
        print_stats("  io_uring DIRECT (batched)", s);
    }
    if let Some(ref s) = best_sb_read {
        print_stats("  spawn_blocking+fs::read", s);
    }
    if let Some(ref s) = best_sb_pread {
        print_stats("  spawn_blocking+pread", s);
    }
}

#[tokio::main]
async fn main() {
    println!("=== NativeLink Read I/O Benchmark ===");
    println!(
        "Platform: {} / {} cores / tokio multi-thread",
        std::env::consts::OS,
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    );

    // Setup
    let bench_dir = Path::new(BENCH_DIR);
    cleanup();
    std::fs::create_dir_all(bench_dir).expect("create bench dir");

    let small_paths = setup_files(bench_dir, "small", NUM_SMALL_FILES, SMALL_FILE_SIZE);
    let medium_paths = setup_files(bench_dir, "medium", NUM_MEDIUM_FILES, MEDIUM_FILE_SIZE);

    // Pre-warm page cache
    warmup_page_cache(&small_paths);
    warmup_page_cache(&medium_paths);

    println!(
        "\nCreated {} small files ({}B) and {} medium files ({}B) in {BENCH_DIR}",
        small_paths.len(),
        SMALL_FILE_SIZE,
        medium_paths.len(),
        MEDIUM_FILE_SIZE,
    );

    // Run benchmarks
    for &conc in SMALL_CONCURRENCIES {
        run_bench_suite("small files", &small_paths, SMALL_FILE_SIZE, conc).await;
    }

    for &conc in MEDIUM_CONCURRENCIES {
        run_bench_suite("medium files", &medium_paths, MEDIUM_FILE_SIZE, conc).await;
    }

    // Cleanup
    cleanup();
    println!("\nDone. Temp files cleaned up.");
}
