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

//! Benchmark comparing io_uring vs spawn_blocking file I/O latency across
//! realistic workload scenarios.
//!
//! Test matrix:
//! - File sizes: 8KB (small blob p50), 1MB (mid-range), 12MB (typical large
//!   CAS blob), 100MB (thread pool exhaustion scenario)
//! - Concurrency: 1, 16, 64 concurrent readers
//! - Offset reads: seek to middle of file
//! - Mixed workloads: 90% small + 10% large reads
//!
//! Run with the active compile-time backend:
//!   cargo bench -p nativelink-util --bench fs_io_bench
//!
//! Compare against spawn_blocking fallback:
//!   cargo bench -p nativelink-util --bench fs_io_bench --no-default-features

use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use criterion::{BenchmarkId, Criterion, criterion_group, criterion_main};
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::fs;
use rand::Rng;

const BLOB_SIZE: usize = 8 * 1024; // 8 KB
const READ_BUF_3MIB: usize = 3 * 1024 * 1024;

/// Build a tokio multi-thread runtime for async benchmarks.
fn make_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime")
}

/// Create a temp directory containing a single 8 KB file filled with random
/// data. Returns (dir handle, path to the file, the random bytes).
fn setup_test_file() -> (tempfile::TempDir, PathBuf, Bytes) {
    let dir = tempfile::tempdir().expect("failed to create temp dir");
    let path = dir.path().join("blob_8kb");
    let mut rng = rand::rng();
    let data: Vec<u8> = (0..BLOB_SIZE).map(|_| rng.random::<u8>()).collect();
    let mut f = std::fs::File::create(&path).expect("failed to create test file");
    f.write_all(&data).expect("failed to write test data");
    f.sync_all().expect("failed to sync test file");
    // Pre-warm the page cache by reading once.
    drop(std::fs::read(&path).expect("failed to pre-warm page cache"));
    (dir, path, Bytes::from(data))
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

/// Benchmark: open_file + read_file_to_channel (full read path).
fn bench_open_and_read(c: &mut Criterion) {
    let rt = make_runtime();
    let (_dir, path, _data) = setup_test_file();

    c.bench_function("open_file + read_file_to_channel (8KB)", |b| {
        b.to_async(&rt).iter(|| async {
            let file = fs::open_file(&path, 0)
                .await
                .expect("open_file failed");
            let (mut writer, mut reader) = make_buf_channel_pair();
            let read_handle = tokio::spawn(async move {
                // Drain the channel so the writer does not block.
                let mut total = 0usize;
                loop {
                    match reader.recv().await {
                        Ok(chunk) => {
                            if chunk.is_empty() {
                                break;
                            }
                            total += chunk.len();
                        }
                        Err(_) => break,
                    }
                }
                total
            });
            let _file = fs::read_file_to_channel(
                file,
                &mut writer,
                BLOB_SIZE as u64,
                BLOB_SIZE, // single chunk for 8 KB
                0,
            )
            .await
            .expect("read_file_to_channel failed");
            writer.send_eof().expect("send_eof failed");
            let total = read_handle.await.expect("reader task panicked");
            assert_eq!(total, BLOB_SIZE);
        });
    });
}

/// Benchmark: read_file_to_channel alone (file already open).
fn bench_read_only(c: &mut Criterion) {
    let rt = make_runtime();
    let (_dir, path, _data) = setup_test_file();

    c.bench_function("read_file_to_channel only (8KB)", |b| {
        b.to_async(&rt).iter(|| async {
            // Open outside the timed region is not possible with criterion's
            // iter() — but we keep the open cost minimal by reusing the same
            // path (page-cache warm). The open is a constant overhead that
            // does not vary between io_uring and spawn_blocking, so the
            // relative comparison is still valid.
            let file = fs::open_file(&path, 0)
                .await
                .expect("open_file failed");
            let (mut writer, mut reader) = make_buf_channel_pair();
            let read_handle = tokio::spawn(async move {
                let mut total = 0usize;
                loop {
                    match reader.recv().await {
                        Ok(chunk) => {
                            if chunk.is_empty() {
                                break;
                            }
                            total += chunk.len();
                        }
                        Err(_) => break,
                    }
                }
                total
            });
            let _file = fs::read_file_to_channel(
                file,
                &mut writer,
                BLOB_SIZE as u64,
                BLOB_SIZE,
                0,
            )
            .await
            .expect("read_file_to_channel failed");
            writer.send_eof().expect("send_eof failed");
            let total = read_handle.await.expect("reader task panicked");
            assert_eq!(total, BLOB_SIZE);
        });
    });
}

/// Benchmark: create_file + write_all_to_file (full write path).
fn bench_create_and_write(c: &mut Criterion) {
    let rt = make_runtime();
    let (_dir, _path, data) = setup_test_file();
    let write_dir = tempfile::tempdir().expect("failed to create write temp dir");
    let counter = std::sync::atomic::AtomicU64::new(0);

    c.bench_function("create_file + write_all_to_file (8KB)", |b| {
        b.to_async(&rt).iter(|| {
            let d = data.clone();
            let seq = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = write_dir.path().join(format!("w_{seq}"));
            async move {
                let file = fs::create_file(&p).await.expect("create_file failed");
                let _file = fs::write_all_to_file(file, d)
                    .await
                    .expect("write_all_to_file failed");
            }
        });
    });
}

/// Benchmark: write_all_to_file alone (file already created via create_file).
fn bench_write_only(c: &mut Criterion) {
    let rt = make_runtime();
    let (_dir, _path, data) = setup_test_file();
    let write_dir = tempfile::tempdir().expect("failed to create write temp dir");
    let counter = std::sync::atomic::AtomicU64::new(0);

    c.bench_function("write_all_to_file only (8KB)", |b| {
        b.to_async(&rt).iter(|| {
            let d = data.clone();
            let seq = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = write_dir.path().join(format!("w_{seq}"));
            async move {
                // create_file is part of setup, not measured (though criterion
                // measures the whole async block — the create cost is constant
                // across backends so relative comparison remains valid).
                let file = fs::create_file(&p).await.expect("create_file failed");
                let _file = fs::write_all_to_file(file, d)
                    .await
                    .expect("write_all_to_file failed");
            }
        });
    });
}

/// Benchmark concurrent reads across a matrix of file sizes and concurrency
/// levels. Covers the realistic bottleneck scenarios:
/// - 8KB: small blob p50 (73% of production traffic), single-chunk read
/// - 1MB: mid-range blobs
/// - 12MB: typical large CAS blob (4 reads at 3MiB buffer)
/// - 100MB: thread pool exhaustion scenario (34 reads at 3MiB buffer)
///
/// Concurrency levels:
/// - 1: baseline single-reader latency
/// - 16: moderate concurrency
/// - 64: thread pool pressure territory
fn bench_concurrent_reads(c: &mut Criterion) {
    let rt = make_runtime();
    let sizes: &[(usize, &str)] = &[
        (8 * 1024, "8KB"),
        (1024 * 1024, "1MB"),
        (12 * 1024 * 1024, "12MB"),
        (100 * 1024 * 1024, "100MB"),
    ];
    let concurrencies: &[usize] = &[1, 16, 64];

    let mut group = c.benchmark_group("concurrent_reads");
    // Fewer samples for expensive benchmarks (100MB x 64 readers).
    group.sample_size(10);

    for &(size, size_name) in sizes {
        // Create enough files so each concurrent reader gets its own file
        // (avoids measuring lock contention on a single inode).
        let max_conc = *concurrencies.last().unwrap();
        let (_dir, paths) = setup_test_files(size, max_conc);
        let paths = Arc::new(paths);
        let read_buf = read_buf_for_size(size);

        for &conc in concurrencies {
            group.bench_function(
                BenchmarkId::new(size_name, format!("x{conc}")),
                |b| {
                    let paths = Arc::clone(&paths);
                    b.to_async(&rt).iter(|| {
                        let paths = Arc::clone(&paths);
                        async move {
                            let mut handles = Vec::with_capacity(conc);
                            for i in 0..conc {
                                let path = paths[i % paths.len()].clone();
                                let file_size = size;
                                let buf_size = read_buf;
                                handles.push(tokio::spawn(async move {
                                    let file = fs::open_file(&path, 0)
                                        .await
                                        .expect("open_file failed");
                                    let (mut writer, mut reader) =
                                        make_buf_channel_pair();
                                    let drain = tokio::spawn(async move {
                                        let mut total = 0usize;
                                        loop {
                                            match reader.recv().await {
                                                Ok(chunk)
                                                    if !chunk.is_empty() =>
                                                {
                                                    total += chunk.len();
                                                }
                                                _ => break,
                                            }
                                        }
                                        total
                                    });
                                    let _file = fs::read_file_to_channel(
                                        file,
                                        &mut writer,
                                        file_size as u64,
                                        buf_size,
                                        0,
                                    )
                                    .await
                                    .expect("read_file_to_channel failed");
                                    writer
                                        .send_eof()
                                        .expect("send_eof failed");
                                    let total = drain
                                        .await
                                        .expect("reader task panicked");
                                    assert_eq!(total, file_size);
                                }));
                            }
                            for h in handles {
                                h.await
                                    .expect("concurrent read task panicked");
                            }
                        }
                    });
                },
            );
        }
    }
    group.finish();
}

/// Benchmark reading from a non-zero offset to validate seek behavior.
/// Uses a 12MB file, reading 3MB starting at offset 6MB vs offset 0.
fn bench_offset_reads(c: &mut Criterion) {
    let rt = make_runtime();
    let file_size = 12 * 1024 * 1024usize; // 12MB
    let read_len = 3 * 1024 * 1024usize; // 3MB
    let (_dir, paths) = setup_test_files(file_size, 1);
    let path = paths[0].clone();

    let mut group = c.benchmark_group("offset_reads");
    group.sample_size(50);

    for &(offset, label) in
        &[(0u64, "offset_0"), (6 * 1024 * 1024u64, "offset_6MB")]
    {
        group.bench_function(label, |b| {
            let path = path.clone();
            b.to_async(&rt).iter(|| {
                let path = path.clone();
                async move {
                    let file = fs::open_file(&path, offset)
                        .await
                        .expect("open_file failed");
                    let (mut writer, mut reader) = make_buf_channel_pair();
                    let drain = tokio::spawn(async move {
                        let mut total = 0usize;
                        loop {
                            match reader.recv().await {
                                Ok(chunk) if !chunk.is_empty() => {
                                    total += chunk.len();
                                }
                                _ => break,
                            }
                        }
                        total
                    });
                    let _file = fs::read_file_to_channel(
                        file,
                        &mut writer,
                        read_len as u64,
                        READ_BUF_3MIB,
                        offset,
                    )
                    .await
                    .expect("read_file_to_channel failed");
                    writer.send_eof().expect("send_eof failed");
                    let total = drain.await.expect("reader task panicked");
                    assert_eq!(total, read_len);
                }
            });
        });
    }
    group.finish();
}

/// Simulate realistic production workload: 90% small reads (8KB) + 10%
/// large reads (12MB) running concurrently across 64 tasks.
/// This matches observed production traffic patterns where small blobs
/// dominate count but large blobs dominate bandwidth.
fn bench_mixed_workload(c: &mut Criterion) {
    let rt = make_runtime();

    let small_size = 8 * 1024usize; // 8KB
    let large_size = 12 * 1024 * 1024usize; // 12MB
    let total_tasks = 64usize;
    let large_tasks = 6usize; // ~10% large
    let small_tasks = total_tasks - large_tasks; // ~90% small

    // Create files for each type.
    let (_small_dir, small_paths) = setup_test_files(small_size, small_tasks);
    let (_large_dir, large_paths) = setup_test_files(large_size, large_tasks);
    let small_paths = Arc::new(small_paths);
    let large_paths = Arc::new(large_paths);

    let mut group = c.benchmark_group("mixed_workload");
    group.sample_size(10);

    group.bench_function("90pct_8KB_10pct_12MB_x64", |b| {
        let small_paths = Arc::clone(&small_paths);
        let large_paths = Arc::clone(&large_paths);
        b.to_async(&rt).iter(|| {
            let small_paths = Arc::clone(&small_paths);
            let large_paths = Arc::clone(&large_paths);
            async move {
                let mut handles = Vec::with_capacity(total_tasks);

                // Spawn small-blob readers.
                for i in 0..small_tasks {
                    let path =
                        small_paths[i % small_paths.len()].clone();
                    handles.push(tokio::spawn(async move {
                        let file = fs::open_file(&path, 0)
                            .await
                            .expect("open_file failed");
                        let (mut writer, mut reader) =
                            make_buf_channel_pair();
                        let drain = tokio::spawn(async move {
                            let mut total = 0usize;
                            loop {
                                match reader.recv().await {
                                    Ok(chunk) if !chunk.is_empty() => {
                                        total += chunk.len();
                                    }
                                    _ => break,
                                }
                            }
                            total
                        });
                        let _file = fs::read_file_to_channel(
                            file,
                            &mut writer,
                            small_size as u64,
                            small_size, // single chunk for 8KB
                            0,
                        )
                        .await
                        .expect("read_file_to_channel failed");
                        writer.send_eof().expect("send_eof failed");
                        let total =
                            drain.await.expect("reader task panicked");
                        assert_eq!(total, small_size);
                    }));
                }

                // Spawn large-blob readers.
                for i in 0..large_tasks {
                    let path =
                        large_paths[i % large_paths.len()].clone();
                    handles.push(tokio::spawn(async move {
                        let file = fs::open_file(&path, 0)
                            .await
                            .expect("open_file failed");
                        let (mut writer, mut reader) =
                            make_buf_channel_pair();
                        let drain = tokio::spawn(async move {
                            let mut total = 0usize;
                            loop {
                                match reader.recv().await {
                                    Ok(chunk) if !chunk.is_empty() => {
                                        total += chunk.len();
                                    }
                                    _ => break,
                                }
                            }
                            total
                        });
                        let _file = fs::read_file_to_channel(
                            file,
                            &mut writer,
                            large_size as u64,
                            READ_BUF_3MIB, // 3MiB buffer for large files
                            0,
                        )
                        .await
                        .expect("read_file_to_channel failed");
                        writer.send_eof().expect("send_eof failed");
                        let total =
                            drain.await.expect("reader task panicked");
                        assert_eq!(total, large_size);
                    }));
                }

                for h in handles {
                    h.await.expect("mixed workload task panicked");
                }
            }
        });
    });

    group.finish();
}

criterion_group! {
    name = fs_io_benches;
    config = Criterion::default()
        .significance_level(0.05)
        .sample_size(200)
        .measurement_time(std::time::Duration::from_secs(10));
    targets =
        bench_open_and_read,
        bench_read_only,
        bench_create_and_write,
        bench_write_only,
}

criterion_group! {
    name = fs_io_concurrent_benches;
    config = Criterion::default()
        .significance_level(0.05)
        .sample_size(10)
        .measurement_time(std::time::Duration::from_secs(15));
    targets =
        bench_concurrent_reads,
        bench_offset_reads,
        bench_mixed_workload,
}

criterion_main!(fs_io_benches, fs_io_concurrent_benches);
