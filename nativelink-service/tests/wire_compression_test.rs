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

use bytes::Bytes;
use nativelink_error::{Code, Error};
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::compressor;
use nativelink_service::wire_compression::{
    BufChannelReader, compress, decompress, decompress_batch_update, resolve_wire_compressor,
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
