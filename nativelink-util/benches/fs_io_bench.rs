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

//! Benchmark comparing io_uring vs spawn_blocking vs mmap file I/O latency
//! across realistic workload scenarios.
//!
//! Test matrix:
//! - File sizes: 8KB (small blob p50), 1MB (mid-range), 12MB (typical large
//!   CAS blob), 100MB (thread pool exhaustion scenario)
//! - Concurrency: 1, 16, 64 concurrent readers
//! - Backends: io_uring (batch pread), spawn_blocking (sequential read),
//!   mmap (MAP_POPULATE + memcpy)
//!
//! Run all benchmarks:
//!   cargo bench -p nativelink-util --bench fs_io_bench
//!
//! Run only backend comparison:
//!   cargo bench -p nativelink-util --bench fs_io_bench -- backend

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::fs;
use rand::Rng;

const READ_BUF_3MIB: usize = 3 * 1024 * 1024;

/// Build a tokio multi-thread runtime for async benchmarks.
fn make_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
}

/// Create a temp directory with `count` files of `size` bytes filled with
/// random data. Pre-warms the page cache for each file. Returns (dir handle,
/// file paths).
fn setup_test_files(size: usize, count: usize) -> (tempfile::TempDir, Vec<PathBuf>) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let mut rng = rand::rng();
    let mut paths = Vec::with_capacity(count);
    for i in 0..count {
        let path = dir.path().join(format!("blob_{size}_{i}"));
        let data: Vec<u8> = (0..size).map(|_| rng.random::<u8>()).collect();
        let mut f = std::fs::File::create(&path).expect("failed to create test file");
        f.write_all(&data).expect("failed to write test data");
        f.sync_all().expect("failed to sync test file");
        // Pre-warm the page cache.
        drop(std::fs::read(&path).expect("failed to pre-warm page cache"));
        paths.push(path);
    }
    (dir, paths)
}

/// Return the appropriate read buffer size for a given file size.
/// 8KB files use 8KB (single chunk); larger files use 3MiB matching
/// production config.
fn read_buf_for_size(file_size: usize) -> usize {
    if file_size <= 8 * 1024 {
        file_size
    } else {
        READ_BUF_3MIB
    }
}

/// Backend selector for read benchmarks.
#[derive(Clone, Copy)]
enum ReadBackend {
    /// Auto-select: io_uring on Linux with feature, else spawn_blocking.
    Default,
    /// Explicit spawn_blocking path.
    Blocking,
    /// mmap + memcpy path (Linux only).
    #[cfg(target_os = "linux")]
    Mmap,
    /// IO_LINK: open+read+close in a single io_uring submission (Linux only).
    /// Only applicable for single-chunk reads (limit <= buf_size).
    #[cfg(target_os = "linux")]
    Linked,
}

impl ReadBackend {
    fn name(self) -> &'static str {
        match self {
            Self::Default => "io_uring",
            Self::Blocking => "blocking",
            #[cfg(target_os = "linux")]
            Self::Mmap => "mmap",
            #[cfg(target_os = "linux")]
            Self::Linked => "linked",
        }
    }
}

/// Run a single file read with the specified backend.
async fn do_read(
    backend: ReadBackend,
    path: &Path,
    file_size: usize,
    buf_size: usize,
    offset: u64,
) {
    // The "linked" backend uses open_read_close which bypasses open_file
    // and read_file_to_channel entirely — single io_uring submission.
    #[cfg(target_os = "linux")]
    if matches!(backend, ReadBackend::Linked) {
        let system = tokio_epoll_uring::thread_local_system().await;
        let mut opts = tokio_epoll_uring::ops::open_at::OpenOptions::new();
        opts.read(true);
        let expected = file_size - offset as usize;
        let buf = Vec::with_capacity(expected);
        let (mut writer, mut reader) = make_buf_channel_pair();
        let drain = tokio::spawn(async move {
            let mut total = 0usize;
            loop {
                match reader.recv().await {
                    Ok(chunk) if !chunk.is_empty() => total += chunk.len(),
                    _ => break,
                }
            }
            total
        });
        let (returned_buf, read_result) = system
            .open_read_close(path, &opts, offset, buf)
            .await
            .expect("open_read_close failed");
        let n = read_result.expect("read failed");
        let mut v = returned_buf;
        v.truncate(n);
        if !v.is_empty() {
            writer
                .send(bytes::Bytes::from(v))
                .await
                .expect("send failed");
        }
        writer.send_eof().expect("eof failed");
        let total = drain.await.expect("drain panicked");
        assert_eq!(total, expected);
        return;
    }

    let file = fs::open_file(path, offset).await.expect("open failed");
    let (mut writer, mut reader) = make_buf_channel_pair();
    let drain = tokio::spawn(async move {
        let mut total = 0usize;
        loop {
            match reader.recv().await {
                Ok(chunk) if !chunk.is_empty() => total += chunk.len(),
                _ => break,
            }
        }
        total
    });

    let expected = file_size - offset as usize;
    let read_len = expected as u64;

    let _file = match backend {
        ReadBackend::Default => {
            fs::read_file_to_channel(file, &mut writer, read_len, buf_size, offset).await
        }
        ReadBackend::Blocking => {
            fs::read_file_to_channel_blocking(file, &mut writer, read_len, buf_size, offset).await
        }
        #[cfg(target_os = "linux")]
        ReadBackend::Mmap => {
            fs::read_file_to_channel_mmap(file, &mut writer, read_len, buf_size, offset).await
        }
        #[cfg(target_os = "linux")]
        ReadBackend::Linked => unreachable!("handled above"),
    }
    .expect("read failed");

    writer.send_eof().expect("eof failed");
    let total = drain.await.expect("drain panicked");
    assert_eq!(total, expected);
}

/// Backend selector for write benchmarks.
#[derive(Clone, Copy)]
enum WriteBackend {
    Default,
    Blocking,
    #[cfg(target_os = "linux")]
    Mmap,
}

impl WriteBackend {
    fn name(self) -> &'static str {
        match self {
            Self::Default => "io_uring",
            Self::Blocking => "blocking",
            #[cfg(target_os = "linux")]
            Self::Mmap => "mmap",
        }
    }
}

// ---------- Backend comparison: concurrent reads ----------

/// Compare all three backends for concurrent reads across file sizes and
/// concurrency levels.
fn bench_backend_reads(c: &mut Criterion) {
    let rt = make_runtime();
    let sizes: &[(usize, &str)] = &[
        (8 * 1024, "8KB"),
        (1024 * 1024, "1MB"),
        (12 * 1024 * 1024, "12MB"),
        (100 * 1024 * 1024, "100MB"),
    ];
    let concurrencies: &[usize] = &[1, 16, 64];

    #[cfg(target_os = "linux")]
    let backends = [ReadBackend::Default, ReadBackend::Blocking, ReadBackend::Mmap, ReadBackend::Linked];
    #[cfg(not(target_os = "linux"))]
    let backends = [ReadBackend::Default, ReadBackend::Blocking];

    let mut group = c.benchmark_group("backend_reads");
    group.sample_size(10);

    for &(size, size_name) in sizes {
        let max_conc = *concurrencies.last().unwrap();
        let (_dir, paths) = setup_test_files(size, max_conc);
        let paths = Arc::new(paths);
        let read_buf = read_buf_for_size(size);

        for &conc in concurrencies {
            for &backend in &backends {
                group.bench_function(
                    BenchmarkId::new(
                        format!("{}/{}", size_name, backend.name()),
                        format!("x{conc}"),
                    ),
                    |b| {
                        let paths = Arc::clone(&paths);
                        b.to_async(&rt).iter(|| {
                            let paths = Arc::clone(&paths);
                            async move {
                                let mut handles = Vec::with_capacity(conc);
                                for i in 0..conc {
                                    let path = paths[i % paths.len()].clone();
                                    handles.push(tokio::spawn(async move {
                                        do_read(backend, &path, size, read_buf, 0).await;
                                    }));
                                }
                                for h in handles {
                                    h.await.expect("task panicked");
                                }
                            }
                        });
                    },
                );
            }
        }
    }
    group.finish();
}

// ---------- Backend comparison: writes ----------

/// Compare all three write backends across file sizes (8KB, 1MB, 12MB).
fn bench_backend_writes(c: &mut Criterion) {
    let rt = make_runtime();
    let sizes: &[(usize, &str)] = &[
        (8 * 1024, "8KB"),
        (1024 * 1024, "1MB"),
        (12 * 1024 * 1024, "12MB"),
    ];

    #[cfg(target_os = "linux")]
    let backends = [WriteBackend::Default, WriteBackend::Blocking, WriteBackend::Mmap];
    #[cfg(not(target_os = "linux"))]
    let backends = [WriteBackend::Default, WriteBackend::Blocking];

    let mut group = c.benchmark_group("backend_writes");

    for &(size, size_name) in sizes {
        let data = {
            let mut rng = rand::rng();
            let v: Vec<u8> = (0..size).map(|_| rng.random::<u8>()).collect();
            Bytes::from(v)
        };
        let write_dir = tempfile::tempdir().expect("failed to create write temp dir");
        let counter = std::sync::atomic::AtomicU64::new(0);

        for &backend in &backends {
            group.bench_function(
                BenchmarkId::new(size_name, backend.name()),
                |b| {
                    b.to_async(&rt).iter(|| {
                        let d = data.clone();
                        let seq = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let p = write_dir.path().join(format!("w_{seq}"));
                        async move {
                            let file = fs::create_file(&p).await.expect("create failed");
                            match backend {
                                WriteBackend::Default => {
                                    fs::write_all_to_file(file, d)
                                        .await
                                        .expect("write failed");
                                }
                                WriteBackend::Blocking => {
                                    fs::write_all_to_file_blocking(file, d)
                                        .await
                                        .expect("write failed");
                                }
                                #[cfg(target_os = "linux")]
                                WriteBackend::Mmap => {
                                    fs::write_all_to_file_mmap(file, d)
                                        .await
                                        .expect("write failed");
                                }
                            }
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

// ---------- Backend comparison: offset reads ----------

/// Compare backends for reading from non-zero offsets.
fn bench_backend_offset_reads(c: &mut Criterion) {
    let rt = make_runtime();
    let file_size = 12 * 1024 * 1024usize;
    let read_len = 3 * 1024 * 1024usize;
    let (_dir, paths) = setup_test_files(file_size, 1);
    let path = paths[0].clone();

    #[cfg(target_os = "linux")]
    let backends = [ReadBackend::Default, ReadBackend::Blocking, ReadBackend::Mmap, ReadBackend::Linked];
    #[cfg(not(target_os = "linux"))]
    let backends = [ReadBackend::Default, ReadBackend::Blocking];

    let mut group = c.benchmark_group("backend_offset_reads");
    group.sample_size(50);

    let offset = 6 * 1024 * 1024u64;
    for &backend in &backends {
        group.bench_function(
            BenchmarkId::new("12MB@6MB", backend.name()),
            |b| {
                let path = path.clone();
                b.to_async(&rt).iter(|| {
                    let path = path.clone();
                    async move {
                        let file = fs::open_file(&path, offset)
                            .await
                            .expect("open failed");
                        let (mut writer, mut reader) = make_buf_channel_pair();
                        let drain = tokio::spawn(async move {
                            let mut total = 0usize;
                            loop {
                                match reader.recv().await {
                                    Ok(chunk) if !chunk.is_empty() => total += chunk.len(),
                                    _ => break,
                                }
                            }
                            total
                        });
                        let _file = match backend {
                            ReadBackend::Default => {
                                fs::read_file_to_channel(
                                    file, &mut writer, read_len as u64, READ_BUF_3MIB, offset,
                                )
                                .await
                            }
                            ReadBackend::Blocking => {
                                fs::read_file_to_channel_blocking(
                                    file, &mut writer, read_len as u64, READ_BUF_3MIB, offset,
                                )
                                .await
                            }
                            #[cfg(target_os = "linux")]
                            ReadBackend::Mmap => {
                                fs::read_file_to_channel_mmap(
                                    file, &mut writer, read_len as u64, READ_BUF_3MIB, offset,
                                )
                                .await
                            }
                            #[cfg(target_os = "linux")]
                            ReadBackend::Linked => unreachable!("handled via do_read"),
                        }
                        .expect("read failed");
                        writer.send_eof().expect("eof failed");
                        let total = drain.await.expect("drain panicked");
                        assert_eq!(total, read_len);
                    }
                });
            },
        );
    }
    group.finish();
}

// ---------- Backend comparison: mixed workload ----------

/// 90% small (8KB) + 10% large (12MB) reads across 64 concurrent tasks.
fn bench_backend_mixed(c: &mut Criterion) {
    let rt = make_runtime();
    let small_size = 8 * 1024usize;
    let large_size = 12 * 1024 * 1024usize;
    let total_tasks = 64usize;
    let large_tasks = 6usize;
    let small_tasks = total_tasks - large_tasks;

    let (_small_dir, small_paths) = setup_test_files(small_size, small_tasks);
    let (_large_dir, large_paths) = setup_test_files(large_size, large_tasks);
    let small_paths = Arc::new(small_paths);
    let large_paths = Arc::new(large_paths);

    #[cfg(target_os = "linux")]
    let backends = [ReadBackend::Default, ReadBackend::Blocking, ReadBackend::Mmap, ReadBackend::Linked];
    #[cfg(not(target_os = "linux"))]
    let backends = [ReadBackend::Default, ReadBackend::Blocking];

    let mut group = c.benchmark_group("backend_mixed");
    group.sample_size(10);

    for &backend in &backends {
        group.bench_function(
            BenchmarkId::new("90pct_8KB_10pct_12MB_x64", backend.name()),
            |b| {
                let small_paths = Arc::clone(&small_paths);
                let large_paths = Arc::clone(&large_paths);
                b.to_async(&rt).iter(|| {
                    let small_paths = Arc::clone(&small_paths);
                    let large_paths = Arc::clone(&large_paths);
                    async move {
                        let mut handles = Vec::with_capacity(total_tasks);
                        for i in 0..small_tasks {
                            let path = small_paths[i % small_paths.len()].clone();
                            handles.push(tokio::spawn(async move {
                                do_read(backend, &path, small_size, small_size, 0).await;
                            }));
                        }
                        for i in 0..large_tasks {
                            let path = large_paths[i % large_paths.len()].clone();
                            handles.push(tokio::spawn(async move {
                                do_read(backend, &path, large_size, READ_BUF_3MIB, 0).await;
                            }));
                        }
                        for h in handles {
                            h.await.expect("task panicked");
                        }
                    }
                });
            },
        );
    }
    group.finish();
}

// ---------- Pre-opened fd reads (skip open cost) ----------

/// Benchmark reading from an already-open fd. This simulates an fd cache
/// where hot files remain open between requests. Tests the pure read
/// overhead without open/close.
fn bench_preopen_reads(c: &mut Criterion) {
    let rt = make_runtime();
    let sizes: &[(usize, &str)] = &[
        (8 * 1024, "8KB"),
        (1024 * 1024, "1MB"),
        (12 * 1024 * 1024, "12MB"),
    ];

    let mut group = c.benchmark_group("preopen_reads");
    group.sample_size(10);

    for &(size, size_name) in sizes {
        let (_dir, paths) = setup_test_files(size, 1);
        let path = paths[0].clone();
        let read_buf = read_buf_for_size(size);

        // io_uring: single pread (small) or batch pread (large)
        group.bench_function(
            BenchmarkId::new(size_name, "io_uring"),
            |b| {
                let path = path.clone();
                b.to_async(&rt).iter(|| {
                    let path = path.clone();
                    async move {
                        // Open once outside the timed region — but criterion
                        // times the whole async block. We keep the open as a
                        // constant overhead that doesn't vary between backends.
                        let file = fs::open_file(&path, 0).await.expect("open failed");
                        let (mut writer, mut reader) = make_buf_channel_pair();
                        let drain = tokio::spawn(async move {
                            let mut total = 0usize;
                            loop {
                                match reader.recv().await {
                                    Ok(chunk) if !chunk.is_empty() => total += chunk.len(),
                                    _ => break,
                                }
                            }
                            total
                        });
                        let _file = fs::read_file_to_channel(
                            file, &mut writer, size as u64, read_buf, 0,
                        )
                        .await
                        .expect("read failed");
                        writer.send_eof().expect("eof failed");
                        let total = drain.await.expect("drain panicked");
                        assert_eq!(total, size);
                    }
                });
            },
        );

        // blocking: spawn_blocking read
        group.bench_function(
            BenchmarkId::new(size_name, "blocking"),
            |b| {
                let path = path.clone();
                b.to_async(&rt).iter(|| {
                    let path = path.clone();
                    async move {
                        let file = fs::open_file(&path, 0).await.expect("open failed");
                        let (mut writer, mut reader) = make_buf_channel_pair();
                        let drain = tokio::spawn(async move {
                            let mut total = 0usize;
                            loop {
                                match reader.recv().await {
                                    Ok(chunk) if !chunk.is_empty() => total += chunk.len(),
                                    _ => break,
                                }
                            }
                            total
                        });
                        let _file = fs::read_file_to_channel_blocking(
                            file, &mut writer, size as u64, read_buf, 0,
                        )
                        .await
                        .expect("read failed");
                        writer.send_eof().expect("eof failed");
                        let total = drain.await.expect("drain panicked");
                        assert_eq!(total, size);
                    }
                });
            },
        );
    }
    group.finish();
}

// ---------- Parallel chunk reads (gRPC pattern) ----------

/// Simulate the gRPC parallel chunk read pattern: split a large file into
/// N chunks and issue N concurrent reads at different offsets, each through
/// its own task. This is what `get_part_parallel` in grpc_store.rs does
/// with concurrent ByteStream::Read RPCs.
fn bench_parallel_chunk_reads(c: &mut Criterion) {
    let rt = make_runtime();
    let file_size = 100 * 1024 * 1024usize; // 100MB
    let chunk_counts: &[(usize, &str)] = &[
        (4, "4_chunks"),
        (16, "16_chunks"),
        (64, "64_chunks"),
    ];
    let (_dir, paths) = setup_test_files(file_size, 1);
    let path = Arc::new(paths[0].clone());

    let mut group = c.benchmark_group("parallel_chunks");
    group.sample_size(10);

    for &(num_chunks, label) in chunk_counts {
        let chunk_size = file_size / num_chunks;

        // io_uring: each chunk reader opens + reads its portion
        group.bench_function(
            BenchmarkId::new(label, "io_uring"),
            |b| {
                let path = Arc::clone(&path);
                b.to_async(&rt).iter(|| {
                    let path = Arc::clone(&path);
                    async move {
                        let mut handles = Vec::with_capacity(num_chunks);
                        for i in 0..num_chunks {
                            let path = Arc::clone(&path);
                            let offset = (i * chunk_size) as u64;
                            let len = chunk_size;
                            handles.push(tokio::spawn(async move {
                                let file = fs::open_file(path.as_path(), offset)
                                    .await.expect("open failed");
                                let (mut writer, mut reader) = make_buf_channel_pair();
                                let drain = tokio::spawn(async move {
                                    let mut total = 0usize;
                                    loop {
                                        match reader.recv().await {
                                            Ok(chunk) if !chunk.is_empty() => total += chunk.len(),
                                            _ => break,
                                        }
                                    }
                                    total
                                });
                                let _file = fs::read_file_to_channel(
                                    file, &mut writer, len as u64, READ_BUF_3MIB, offset,
                                ).await.expect("read failed");
                                writer.send_eof().expect("eof failed");
                                let total = drain.await.expect("drain panicked");
                                assert_eq!(total, len);
                            }));
                        }
                        for h in handles {
                            h.await.expect("chunk task panicked");
                        }
                    }
                });
            },
        );

        // blocking: same pattern with spawn_blocking reads
        group.bench_function(
            BenchmarkId::new(label, "blocking"),
            |b| {
                let path = Arc::clone(&path);
                b.to_async(&rt).iter(|| {
                    let path = Arc::clone(&path);
                    async move {
                        let mut handles = Vec::with_capacity(num_chunks);
                        for i in 0..num_chunks {
                            let path = Arc::clone(&path);
                            let offset = (i * chunk_size) as u64;
                            let len = chunk_size;
                            handles.push(tokio::spawn(async move {
                                let file = fs::open_file(path.as_path(), offset)
                                    .await.expect("open failed");
                                let (mut writer, mut reader) = make_buf_channel_pair();
                                let drain = tokio::spawn(async move {
                                    let mut total = 0usize;
                                    loop {
                                        match reader.recv().await {
                                            Ok(chunk) if !chunk.is_empty() => total += chunk.len(),
                                            _ => break,
                                        }
                                    }
                                    total
                                });
                                let _file = fs::read_file_to_channel_blocking(
                                    file, &mut writer, len as u64, READ_BUF_3MIB, offset,
                                ).await.expect("read failed");
                                writer.send_eof().expect("eof failed");
                                let total = drain.await.expect("drain panicked");
                                assert_eq!(total, len);
                            }));
                        }
                        for h in handles {
                            h.await.expect("chunk task panicked");
                        }
                    }
                });
            },
        );
    }
    group.finish();
}

criterion_group! {
    name = backend_benches;
    config = Criterion::default()
        .significance_level(0.05)
        .sample_size(10)
        .measurement_time(std::time::Duration::from_secs(15));
    targets =
        bench_backend_reads,
        bench_backend_writes,
        bench_backend_offset_reads,
        bench_backend_mixed,
        bench_preopen_reads,
        bench_parallel_chunk_reads,
}

criterion_main!(backend_benches);
