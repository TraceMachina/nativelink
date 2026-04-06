// Benchmark: io_uring vs spawn_blocking for reads AND writes at various sizes.
// Answers: at what file size (if any) does io_uring beat spawn_blocking?
//
// Run: cargo run -p nativelink-util --example rw_bench --release --features io-uring

use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::stream::{FuturesUnordered, StreamExt};

const BENCH_DIR: &str = "/tmp/nativelink-rw-bench";
const FILES_PER_SIZE: usize = 200;
const ITERS: usize = 3;

// Test sizes spanning the CAS distribution: p50=255B, p90=472KB, p99=42MB
const SIZES: &[(usize, &str)] = &[
    (100, "100B"),
    (256, "256B"),
    (1_024, "1KB"),
    (4_096, "4KB"),
    (16_384, "16KB"),
    (65_536, "64KB"),
    (262_144, "256KB"),
    (1_048_576, "1MB"),
    (4_194_304, "4MB"),
    (16_777_216, "16MB"),
];

fn setup_files(dir: &Path, size: usize) -> Vec<PathBuf> {
    let sub = dir.join(format!("sz_{size}"));
    std::fs::create_dir_all(&sub).ok();
    let data = vec![0xABu8; size];
    (0..FILES_PER_SIZE)
        .map(|i| {
            let p = sub.join(format!("{i:06}"));
            if !p.exists() {
                let mut f = std::fs::File::create(&p).unwrap();
                f.write_all(&data).unwrap();
            }
            p
        })
        .collect()
}

fn warmup(paths: &[PathBuf]) {
    for p in paths {
        drop(std::fs::read(p));
    }
}

struct Stats {
    avg: Duration,
    p50: Duration,
    p99: Duration,
    wall: Duration,
    throughput_mbps: f64,
}

fn measure(mut latencies: Vec<Duration>, wall: Duration, total_bytes: u64) -> Stats {
    latencies.sort();
    let n = latencies.len();
    let total: Duration = latencies.iter().sum();
    Stats {
        avg: total / n as u32,
        p50: latencies[n / 2],
        p99: latencies[(n as f64 * 0.99) as usize],
        wall,
        throughput_mbps: total_bytes as f64 / (1024.0 * 1024.0) / wall.as_secs_f64(),
    }
}

fn fmt(s: &Stats) -> String {
    format!(
        "avg={:>10.3?} p50={:>10.3?} p99={:>10.3?} wall={:>8.3?} {:.0}MB/s",
        s.avg, s.p50, s.p99, s.wall, s.throughput_mbps
    )
}

// ---- READ benchmarks ----

async fn read_spawn_blocking(paths: &[PathBuf], size: usize) -> Stats {
    let files: Vec<Arc<std::fs::File>> = paths.iter()
        .map(|p| Arc::new(std::fs::File::open(p).unwrap()))
        .collect();
    let concurrency = paths.len();
    let mut lats = Vec::with_capacity(concurrency);
    let mut futs = FuturesUnordered::new();
    let wall = Instant::now();
    for i in 0..concurrency {
        let f = Arc::clone(&files[i]);
        let sz = size;
        let start = Instant::now();
        futs.push(tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; sz];
            let n = unsafe { libc::pread(f.as_raw_fd(), buf.as_mut_ptr() as _, sz, 0) };
            assert!(n > 0);
            start.elapsed()
        }));
    }
    while let Some(r) = futs.next().await { lats.push(r.unwrap()); }
    measure(lats, wall.elapsed(), (concurrency * size) as u64)
}

#[cfg(all(feature = "io-uring", target_os = "linux"))]
fn read_uring_direct(paths: &[PathBuf], size: usize) -> Stats {
    use io_uring::{IoUring, opcode, types};
    let files: Vec<std::fs::File> = paths.iter()
        .map(|p| std::fs::File::open(p).unwrap())
        .collect();
    let conc = paths.len();
    let ring_sz = (conc.next_power_of_two().max(64) as u32).min(4096);
    let mut ring = IoUring::new(ring_sz).unwrap();
    let mut bufs: Vec<Vec<u8>> = (0..conc).map(|_| vec![0u8; size]).collect();
    let mut lats = Vec::with_capacity(conc);
    let mut starts = vec![Instant::now(); conc];
    let mut submitted = 0;
    let mut completed = 0;
    let wall = Instant::now();

    while submitted < conc || completed < conc {
        if submitted < conc {
            let (submitter, mut sq, _) = ring.split();
            sq.sync();
            let space = sq.capacity() - sq.len();
            let batch = (conc - submitted).min(space);
            for _ in 0..batch {
                let i = submitted;
                let fd = files[i % files.len()].as_raw_fd();
                let buf = &mut bufs[i];
                let sqe = opcode::Read::new(types::Fd(fd), buf.as_mut_ptr(), buf.len() as _)
                    .offset(0).build().user_data(i as u64);
                starts[i] = Instant::now();
                unsafe { sq.push(&sqe).unwrap() };
                submitted += 1;
            }
            sq.sync();
            submitter.submit().unwrap();
        }
        ring.completion().sync();
        for cqe in ring.completion() {
            let i = cqe.user_data() as usize;
            assert!(cqe.result() > 0);
            lats.push(starts[i].elapsed());
            completed += 1;
        }
        if completed < conc && submitted >= conc {
            ring.submit_and_wait(1).unwrap();
        }
    }
    measure(lats, wall.elapsed(), (conc * size) as u64)
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
fn read_uring_direct(_: &[PathBuf], _: usize) -> Stats {
    measure(vec![Duration::ZERO], Duration::ZERO, 0)
}

// ---- WRITE benchmarks ----

async fn write_spawn_blocking(dir: &Path, size: usize, count: usize) -> Stats {
    let data = Arc::new(vec![0xCDu8; size]);
    let mut lats = Vec::with_capacity(count);
    let mut futs = FuturesUnordered::new();
    let wall = Instant::now();
    for i in 0..count {
        let p = dir.join(format!("wb_{i:06}"));
        let d = Arc::clone(&data);
        let start = Instant::now();
        futs.push(tokio::task::spawn_blocking(move || {
            let mut f = std::fs::File::create(&p).unwrap();
            f.write_all(&d).unwrap();
            start.elapsed()
        }));
    }
    while let Some(r) = futs.next().await { lats.push(r.unwrap()); }
    measure(lats, wall.elapsed(), (count * size) as u64)
}

#[cfg(all(feature = "io-uring", target_os = "linux"))]
fn write_uring_direct(dir: &Path, size: usize, count: usize) -> Stats {
    use io_uring::{IoUring, opcode, types};

    // Pre-create and open files
    let files: Vec<std::fs::File> = (0..count)
        .map(|i| {
            let p = dir.join(format!("wu_{i:06}"));
            std::fs::File::create(&p).unwrap()
        })
        .collect();

    let data = vec![0xCDu8; size];
    let ring_sz = (count.next_power_of_two().max(64) as u32).min(4096);
    let mut ring = IoUring::new(ring_sz).unwrap();
    let mut lats = Vec::with_capacity(count);
    let mut starts = vec![Instant::now(); count];
    let mut submitted = 0;
    let mut completed = 0;
    let wall = Instant::now();

    while submitted < count || completed < count {
        if submitted < count {
            let (submitter, mut sq, _) = ring.split();
            sq.sync();
            let space = sq.capacity() - sq.len();
            let batch = (count - submitted).min(space);
            for _ in 0..batch {
                let i = submitted;
                let fd = files[i].as_raw_fd();
                let sqe = opcode::Write::new(types::Fd(fd), data.as_ptr(), data.len() as _)
                    .offset(0).build().user_data(i as u64);
                starts[i] = Instant::now();
                unsafe { sq.push(&sqe).unwrap() };
                submitted += 1;
            }
            sq.sync();
            submitter.submit().unwrap();
        }
        ring.completion().sync();
        for cqe in ring.completion() {
            let i = cqe.user_data() as usize;
            assert!(cqe.result() > 0, "write returned {}", cqe.result());
            lats.push(starts[i].elapsed());
            completed += 1;
        }
        if completed < count && submitted >= count {
            ring.submit_and_wait(1).unwrap();
        }
    }
    measure(lats, wall.elapsed(), (count * size) as u64)
}

#[cfg(not(all(feature = "io-uring", target_os = "linux")))]
fn write_uring_direct(_: &Path, _: usize, _: usize) -> Stats {
    measure(vec![Duration::ZERO], Duration::ZERO, 0)
}

#[tokio::main]
async fn main() {
    println!("=== NativeLink R/W Benchmark: io_uring vs spawn_blocking ===");
    println!("Cores: {} | Files per size: {FILES_PER_SIZE} | Iters: {ITERS}",
        std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1));
    println!("CAS file size distribution: p50=255B, p90=472KB, p99=42MB\n");

    let dir = Path::new(BENCH_DIR);
    drop(std::fs::remove_dir_all(dir));
    std::fs::create_dir_all(dir).unwrap();

    println!("{:<8} {:>6} | {:<45} | {:<45}", "OP", "SIZE", "spawn_blocking+pread", "io_uring direct (batched)");
    println!("{}", "-".repeat(115));

    for &(size, label) in SIZES {
        let paths = setup_files(dir, size);
        warmup(&paths);

        // --- READS ---
        let mut best_sb = None;
        let mut best_ur = None;
        for _ in 0..ITERS {
            let sb = read_spawn_blocking(&paths, size).await;
            let ur = read_uring_direct(&paths, size);
            if best_sb.as_ref().map_or(true, |b: &Stats| sb.wall < b.wall) { best_sb = Some(sb); }
            if best_ur.as_ref().map_or(true, |b: &Stats| ur.wall < b.wall) { best_ur = Some(ur); }
        }
        let sb = best_sb.unwrap();
        let ur = best_ur.unwrap();
        let ratio = ur.wall.as_secs_f64() / sb.wall.as_secs_f64();
        let winner = if ratio > 1.0 { "SB" } else { "UR" };
        println!("READ   {:>6} | {} | {} | {winner} {ratio:.1}x",
            label, fmt(&sb), fmt(&ur));

        // --- WRITES ---
        let wdir = dir.join(format!("writes_{size}"));
        std::fs::create_dir_all(&wdir).unwrap();
        let count = FILES_PER_SIZE;
        let mut best_sb_w = None;
        let mut best_ur_w = None;
        for _ in 0..ITERS {
            let sb = write_spawn_blocking(&wdir, size, count).await;
            let ur = write_uring_direct(&wdir, size, count);
            if best_sb_w.as_ref().map_or(true, |b: &Stats| sb.wall < b.wall) { best_sb_w = Some(sb); }
            if best_ur_w.as_ref().map_or(true, |b: &Stats| ur.wall < b.wall) { best_ur_w = Some(ur); }
        }
        let sb = best_sb_w.unwrap();
        let ur = best_ur_w.unwrap();
        let ratio = ur.wall.as_secs_f64() / sb.wall.as_secs_f64();
        let winner = if ratio > 1.0 { "SB" } else { "UR" };
        println!("WRITE  {:>6} | {} | {} | {winner} {ratio:.1}x",
            label, fmt(&sb), fmt(&ur));
    }

    drop(std::fs::remove_dir_all(dir));
    println!("\nDone.");
}
