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

//! Benchmark for REAPI wire compression (zstd).
//!
//! Measures:
//!   1. Compression ratio (original vs compressed bytes)
//!   2. Compression throughput (MB/s)
//!   3. Decompression throughput (MB/s)
//!   4. Break-even network speed where compression saves wall-clock time
//!
//! Data patterns simulate realistic REAPI workloads:
//!   - Zeros:     worst-case for identity, best-case for compression (sparse files)
//!   - Repetitive: build log output, repeated symbols (typical for object files)
//!   - Semi-random: compiled binary with some structure (most common real case)
//!   - Protobuf-like: small structured messages with field tags and varints

use nativelink_proto::build::bazel::remote::execution::v2::compressor;
use nativelink_service::wire_compression::{self, ZSTD_COMPRESSION_LEVEL};
use std::time::Instant;

// --- Data generators ---

fn gen_zeros(size: usize) -> Vec<u8> {
    vec![0u8; size]
}

fn gen_repetitive(size: usize) -> Vec<u8> {
    // Simulates build output / object files with repeated patterns
    let pattern = b"hello world this is a build artifact ";
    let mut data = Vec::with_capacity(size);
    while data.len() < size {
        let remaining = size - data.len();
        let chunk_len = remaining.min(pattern.len());
        data.extend_from_slice(&pattern[..chunk_len]);
    }
    data
}

#[allow(clippy::cast_possible_truncation)]
fn gen_semi_random(size: usize) -> Vec<u8> {
    // Simulates compiled binary: mix of structured and random bytes.
    // Uses a simple PRNG for reproducibility.
    let mut data = Vec::with_capacity(size);
    let mut state: u64 = 0x1234567890ABCDEF;
    let mut i = 0;
    while i < size {
        // Every 256 bytes, insert a structured region (zeros or repetitive)
        if i % 256 < 64 {
            data.push(0u8);
        } else if i % 256 < 96 {
            data.push(b'A' + ((i % 26) as u8));
        } else {
            // xorshift64
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            data.push(state as u8);
        }
        i += 1;
    }
    data
}

#[allow(clippy::cast_possible_truncation)]
fn gen_protobuf_like(size: usize) -> Vec<u8> {
    // Simulates protobuf-encoded CAS messages: field tags + varint lengths + data
    let mut data = Vec::with_capacity(size);
    let mut state: u64 = 0xDEADBEEFCAFEBABE;
    while data.len() < size {
        // Field tag (1 byte: field number + wire type)
        data.push(0x0A); // field 1, length-delimited
        // Varint length (1-5 bytes)
        let content_len = ((state as usize) % 64) + 8;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let mut len = content_len;
        loop {
            let byte = (len & 0x7F) as u8;
            len >>= 7;
            if len == 0 {
                data.push(byte);
                break;
            }
            data.push(byte | 0x80);
        }
        // Content (mostly bytes from hash values + some repetition)
        for j in 0..content_len {
            if j < 32 {
                // SHA256 hash-like bytes
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                data.push(state as u8);
            } else {
                data.push(b'x');
            }
        }
    }
    data.truncate(size);
    data
}

// --- Measurement ---

struct BenchResult {
    label: String,
    original_size: usize,
    compressed_size: usize,
    compress_ns: u64,
    decompress_ns: u64,
}

#[allow(clippy::cast_possible_truncation)]
fn bench(label: &str, data: &[u8]) -> BenchResult {
    // Warmup
    let _warmup = wire_compression::compress(data, compressor::Value::Zstd);

    // Compress
    let start = Instant::now();
    let compressed = wire_compression::compress(data, compressor::Value::Zstd)
        .expect("compression should not fail");
    let compress_ns = start.elapsed().as_nanos() as u64;

    // Decompress
    let start = Instant::now();
    let decompressed =
        wire_compression::decompress(&compressed, compressor::Value::Zstd, data.len())
            .expect("decompression should not fail");
    let decompress_ns = start.elapsed().as_nanos() as u64;

    assert_eq!(&decompressed[..], data, "round-trip must be lossless");

    BenchResult {
        label: label.to_string(),
        original_size: data.len(),
        compressed_size: compressed.len(),
        compress_ns,
        decompress_ns,
    }
}

fn format_bytes(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{:.1} KB", b / KB)
    } else {
        format!("{} B", bytes)
    }
}

fn format_throughput(bytes: usize, ns: u64) -> String {
    if ns == 0 {
        return "inf MB/s".to_string();
    }
    let mb = bytes as f64 / (1024.0 * 1024.0);
    let secs = ns as f64 / 1_000_000_000.0;
    format!("{:.1} MB/s", mb / secs)
}

#[allow(clippy::print_stdout)]
fn main() {
    let sizes: &[(usize, &str)] = &[
        (1_024, "1 KB"),
        (10_240, "10 KB"),
        (102_400, "100 KB"),
        (1_048_576, "1 MB"),
        (10_485_760, "10 MB"),
    ];

    let patterns: &[(&str, fn(usize) -> Vec<u8>)] = &[
        ("zeros", gen_zeros),
        ("repetitive", gen_repetitive),
        ("semi-random", gen_semi_random),
        ("protobuf-like", gen_protobuf_like),
    ];

    let mut results: Vec<BenchResult> = Vec::new();

    for &(size, size_label) in sizes {
        for &(pattern_name, generator) in patterns {
            let data = generator(size);
            let label = format!("{}/{}", pattern_name, size_label);
            let result = bench(&label, &data);
            results.push(result);
        }
    }

    // Print results table
    println!();
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!(
        "  REAPI Wire Compression Benchmark  (zstd level {})",
        ZSTD_COMPRESSION_LEVEL
    );
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!();
    println!(
        "{:<22} {:>10} {:>10} {:>8} {:>14} {:>14} {:>12}",
        "Pattern/Size", "Original", "Compressed", "Ratio", "Compress", "Decompress", "Break-even"
    );
    println!(
        "{:<22} {:>10} {:>10} {:>8} {:>14} {:>14} {:>12}",
        "", "", "", "", "throughput", "throughput", "net speed"
    );
    println!("{}", "─".repeat(92));

    for r in &results {
        let ratio = r.compressed_size as f64 / r.original_size as f64;
        let savings = 1.0 - ratio;

        // Break-even: compression saves wall-clock time when the network
        // is slow enough that the time saved transmitting fewer bytes
        // exceeds the CPU time spent compressing+decompressing.
        //
        // time_with_compression    = (compress_ns + decompress_ns) + compressed_size / net_speed
        // time_without_compression = original_size / net_speed
        //
        // Solve for net_speed where they're equal:
        // net_speed = (original_size - compressed_size) / (compress_ns + decompress_ns)
        let bytes_saved = r.original_size - r.compressed_size;
        let cpu_ns = r.compress_ns + r.decompress_ns;
        let breakeven_bps: f64 = if cpu_ns == 0 || bytes_saved == 0 {
            f64::INFINITY
        } else {
            // bytes per nanosecond -> bits per second
            (bytes_saved as f64 / cpu_ns as f64) * 8.0 * 1_000_000_000.0
        };

        let breakeven_str = if breakeven_bps == f64::INFINITY {
            "never".to_string()
        } else if breakeven_bps >= 1_000_000_000.0 {
            format!("{:.1} Gbps", breakeven_bps / 1_000_000_000.0)
        } else if breakeven_bps >= 1_000_000.0 {
            format!("{:.0} Mbps", breakeven_bps / 1_000_000.0)
        } else {
            format!("{:.0} Kbps", breakeven_bps / 1_000.0)
        };

        let ratio_str = format!("{:.1}% (−{:.0}%)", ratio * 100.0, savings * 100.0);

        println!(
            "{:<22} {:>10} {:>10} {:>8} {:>14} {:>14} {:>12}",
            r.label,
            format_bytes(r.original_size),
            format_bytes(r.compressed_size),
            ratio_str,
            format_throughput(r.original_size, r.compress_ns),
            format_throughput(r.original_size, r.decompress_ns),
            breakeven_str,
        );
    }

    // Print a summary for common network speeds
    println!();
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!("  Wall-Clock Time Comparison: Compressed vs Identity at Various Network Speeds");
    println!(
        "═══════════════════════════════════════════════════════════════════════════════════════════"
    );
    println!();

    let net_speeds: &[(f64, &str)] = &[
        (10.0, "10 Mbps"),
        (50.0, "50 Mbps"),
        (100.0, "100 Mbps"),
        (500.0, "500 Mbps"),
        (1000.0, "1 Gbps"),
        (10000.0, "10 Gbps"),
    ];

    // Pick representative sizes/patterns for the summary
    let summary_indices: Vec<usize> = results
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            // 100 KB and 1 MB, semi-random and protobuf-like
            if (r.label.contains("100 KB") || r.label.contains("1 MB"))
                && (r.label.starts_with("semi-random") || r.label.starts_with("protobuf-like"))
            {
                Some(i)
            } else {
                None
            }
        })
        .collect();

    print!("{:<28}", "Pattern/Size → Net Speed");
    for &(_, speed_label) in net_speeds {
        print!("{:>14}", speed_label);
    }
    println!();
    println!("{}", "─".repeat(28 + 14 * net_speeds.len()));

    for &idx in &summary_indices {
        let r = &results[idx];
        print!("{:<28}", r.label);
        for &(speed_mbps, _) in net_speeds {
            let speed_bytes_per_ns = (speed_mbps * 1_000_000.0 / 8.0) / 1_000_000_000.0;

            // Time with compression (ns): CPU + network transfer of compressed data
            let time_compressed_ns = (r.compress_ns + r.decompress_ns) as f64
                + (r.compressed_size as f64 / speed_bytes_per_ns);

            // Time without compression (ns): network transfer of original data
            let time_identity_ns = r.original_size as f64 / speed_bytes_per_ns;

            let speedup = time_identity_ns / time_compressed_ns;
            let pct = (speedup - 1.0) * 100.0;

            if pct >= 0.0 {
                print!("{:>13.1}%+", pct);
            } else {
                print!("{:>13.1}%-", -pct);
            }
        }
        println!();
    }

    println!();
    println!("  (+) = compression is faster overall   (-) = compression is slower overall");
    println!("  Break-even = the network speed where compression stops saving wall-clock time");
    println!();
}
