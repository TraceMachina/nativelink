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

//! Unit tests for the REAPI wire-compression helpers shared by the
//! `ByteStream` and CAS services: compressor URI resolution, zstd
//! encode/decode with size validation, the zero-copy identity batch-update
//! path, and the blocking [`BufChannelReader`] adapter used by the streaming
//! zstd encoder/decoder.

use std::io::Read;
use std::time::Duration;

use bytes::Bytes;
use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::compressor;
use nativelink_service::wire_compression::{
    BufChannelReader, compress, decompress, decompress_batch_update, resolve_wire_compressor,
    stream_encode_compressed_download,
};
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::spawn_blocking;
use pretty_assertions::assert_eq;

#[nativelink_test]
async fn compress_decompress_identity() -> Result<(), Error> {
    let data = Bytes::from_static(b"hello world");
    let compressed = compress(data.clone(), compressor::Value::Identity)?;
    assert_eq!(compressed, data);
    let decompressed = decompress(&compressed, compressor::Value::Identity, data.len())?;
    assert_eq!(decompressed, data);
    Ok(())
}

#[nativelink_test]
async fn compress_decompress_zstd() -> Result<(), Error> {
    // Use repetitive data that zstd can actually compress.
    let data = Bytes::from("hello world ".repeat(100));
    assert!(data.len() > 1000, "test data should be reasonably large");
    let compressed = compress(data.clone(), compressor::Value::Zstd)?;
    // Zstd should compress this repetitive data significantly.
    assert!(
        compressed.len() < data.len(),
        "Zstd compressed {} bytes should be less than original {} bytes",
        compressed.len(),
        data.len()
    );
    let decompressed = decompress(&compressed, compressor::Value::Zstd, data.len())?;
    assert_eq!(decompressed, data);
    Ok(())
}

#[nativelink_test]
async fn decompress_zstd_size_mismatch() -> Result<(), Error> {
    let data = Bytes::from_static(b"hello world");
    let compressed = compress(data, compressor::Value::Zstd)?;
    let result = decompress(&compressed, compressor::Value::Zstd, 999);
    assert_eq!(
        result
            .expect_err("oversized expected_size must be rejected")
            .code,
        Code::InvalidArgument
    );
    Ok(())
}

#[nativelink_test]
async fn decompress_zstd_small_payload_large_claimed_size_is_rejected() -> Result<(), Error> {
    // A tiny payload that claims a huge uncompressed size must be rejected
    // without pre-allocating the claimed size: `expected_size` is the client's
    // digest size and must never be trusted as an allocation hint. The decoder
    // bounds output at expected_size and the length check rejects the lie.
    let data = Bytes::from_static(b"hello world");
    let compressed = compress(data, compressor::Value::Zstd)?;
    let huge_claimed_size = 8 * 1024 * 1024 * 1024; // 8 GiB
    let result = decompress(&compressed, compressor::Value::Zstd, huge_claimed_size);
    assert_eq!(
        result
            .expect_err("small payload claiming a huge size must be rejected")
            .code,
        Code::InvalidArgument
    );
    Ok(())
}

#[nativelink_test]
async fn decompress_zstd_bomb_exceeding_claimed_size_is_rejected() -> Result<(), Error> {
    // A payload that decompresses to MORE than its claimed size (a bomb whose
    // digest also lies low) must be rejected. `take(expected_size + 1)` lets
    // the decoder produce one byte past the cap so the size check trips instead
    // of silently truncating.
    let real = Bytes::from("A".repeat(4096));
    let compressed = compress(real, compressor::Value::Zstd)?;
    let understated_size = 10;
    let result = decompress(&compressed, compressor::Value::Zstd, understated_size);
    assert_eq!(
        result
            .expect_err("output exceeding claimed size must be rejected")
            .code,
        Code::InvalidArgument
    );
    Ok(())
}

#[nativelink_test]
async fn decompress_identity_size_mismatch() -> Result<(), Error> {
    let result = decompress(b"hello world", compressor::Value::Identity, 999);
    assert_eq!(
        result
            .expect_err("wrong identity size must be rejected")
            .code,
        Code::InvalidArgument
    );
    Ok(())
}

#[nativelink_test]
async fn identity_batch_update_keeps_bytes_zero_copy() -> Result<(), Error> {
    let data = Bytes::from_static(b"identity batch update payload");
    let original_ptr = data.as_ptr();

    let decompressed = decompress_batch_update(
        data.clone(),
        compressor::Value::Identity.into(),
        data.len(),
        false,
    )
    .await?;

    assert_eq!(decompressed, data);
    assert_eq!(
        decompressed.as_ptr(),
        original_ptr,
        "identity batch update should validate without copying"
    );
    Ok(())
}

#[nativelink_test]
async fn resolve_wire_compressor_accepts_only_advertised_compressors() -> Result<(), Error> {
    assert_eq!(
        resolve_wire_compressor(None, false)?,
        compressor::Value::Identity
    );
    assert_eq!(
        resolve_wire_compressor(Some("identity"), false)?,
        compressor::Value::Identity
    );
    assert_eq!(
        resolve_wire_compressor(Some("zstd"), true)?,
        compressor::Value::Zstd
    );
    assert!(resolve_wire_compressor(Some("zstd"), false).is_err());
    assert!(resolve_wire_compressor(Some("brotli"), true).is_err());
    Ok(())
}

#[nativelink_test]
async fn buf_channel_reader_reads_partial_and_multiple_chunks() -> Result<(), Error> {
    let (mut tx, rx) = make_buf_channel_pair();

    // BufChannelReader is a blocking `Read` adapter (it uses
    // `blocking_recv`), so it must run off the async runtime — exactly how
    // the streaming zstd encoder/decoder drive it in production.
    let reader_task = spawn_blocking!("buf_channel_reader_test", move || {
        let mut reader = BufChannelReader::new(rx);

        let mut first_partial_chunk = [0; 2];
        reader
            .read_exact(&mut first_partial_chunk)
            .expect("failed to read first partial chunk");
        assert_eq!(&first_partial_chunk, b"ab");

        let mut remaining_chunks = [0; 6];
        reader
            .read_exact(&mut remaining_chunks)
            .expect("failed to read remaining chunks");
        assert_eq!(&remaining_chunks, b"cdefgh");

        let mut eof_probe = [0; 1];
        assert_eq!(reader.read(&mut eof_probe).expect("failed to read EOF"), 0);
    });

    tx.send(Bytes::from_static(b"abc")).await?;
    tx.send(Bytes::from_static(b"defgh")).await?;
    tx.send_eof()?;

    reader_task
        .await
        .map_err(|e| Error::new(Code::Internal, format!("reader task panicked: {e:?}")))?;
    Ok(())
}

/// A compressed-download encode stream lives at the gRPC CLIENT's drain rate:
/// its output channel only empties as fast as the client reads, and a blob can
/// take arbitrarily long to stream out. If each such stream occupies a tokio
/// blocking-pool thread for its whole lifetime, N concurrent downloads with
/// slow consumers occupy N threads and every unrelated `spawn_blocking` user
/// (filesystem store I/O, upload decode, credential providers) queues behind
/// streams that may not finish for minutes. This test wires up encode streams
/// exactly the way `ByteStreamServer::inner_read_compressed` does, with more
/// held streams than blocking-pool threads, and asserts that a trivial
/// unrelated `spawn_blocking` closure still gets to run.
#[test]
fn held_compressed_download_streams_must_not_starve_blocking_pool() {
    // More encode streams than blocking-pool threads. The streams never
    // complete during the test: their input never reaches EOF and their
    // consumer never reads, so any per-stream thread is held indefinitely.
    // The blocking pool schedules FIFO, so if each stream occupies a thread
    // the sentinel spawned afterwards can never run — no sleeps or timing
    // races are needed for the starvation to be deterministic.
    const MAX_BLOCKING_THREADS: usize = 4;
    const NUM_STREAMS: usize = 8;
    const SENTINEL_TIMEOUT: Duration = Duration::from_secs(2);
    const OVERALL_TIMEOUT: Duration = Duration::from_secs(30);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        .enable_all()
        .build()
        .expect("failed to build test runtime");

    runtime.block_on(async {
        tokio::time::timeout(OVERALL_TIMEOUT, async {
            // Held alive for the duration of the sentinel probe: dropping the
            // feeders, encode handles, or receivers early would tear the
            // streams down and release any held threads.
            let mut held_streams = Vec::with_capacity(NUM_STREAMS);
            for _ in 0..NUM_STREAMS {
                let (mut raw_tx, raw_rx) = make_buf_channel_pair();
                let (compressed_tx, compressed_rx) = make_buf_channel_pair();

                // Feed incompressible pseudo-random data forever (no EOF) so
                // the encoder always has input and keeps producing output.
                // The output channel has a small fixed capacity and nothing
                // reads `compressed_rx`, so the encoder soon waits on a full
                // output channel — the "slow client" that holds the stream
                // open. Async sends here mean the feeders themselves never
                // occupy blocking-pool threads.
                let feeder = tokio::spawn(async move {
                    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
                    loop {
                        let mut chunk = vec![0u8; 64 * 1024];
                        for byte in &mut chunk {
                            state = state
                                .wrapping_mul(6_364_136_223_846_793_005)
                                .wrapping_add(1_442_695_040_888_963_407);
                            *byte = (state >> 33) as u8;
                        }
                        if raw_tx.send(Bytes::from(chunk)).await.is_err() {
                            break;
                        }
                    }
                });

                // Start the encode stream the same way
                // `ByteStreamServer::inner_read_compressed` does.
                let encode_task = spawn_blocking!("test_encode_compressed_download", move || {
                    stream_encode_compressed_download(
                        raw_rx,
                        compressor::Value::Zstd,
                        compressed_tx,
                    )
                });

                held_streams.push((feeder, encode_task, compressed_rx));
            }

            // Unrelated blocking work must not starve behind held
            // compressed-download streams.
            let sentinel = spawn_blocking!("test_blocking_pool_sentinel", || ());
            let sentinel_result = tokio::time::timeout(SENTINEL_TIMEOUT, sentinel).await;
            assert!(
                sentinel_result.is_ok(),
                "unrelated blocking work must not starve behind {NUM_STREAMS} held \
                 compressed-download streams on a {MAX_BLOCKING_THREADS}-thread blocking pool"
            );

            drop(held_streams);
        })
        .await
        .expect("test exceeded overall timeout");
    });

    // Encode streams stuck on a full output channel park their blocking-pool
    // threads indefinitely; a normal runtime drop would join them and hang.
    runtime.shutdown_background();
}
