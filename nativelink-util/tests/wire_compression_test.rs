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

use bytes::Bytes;
use nativelink_error::Error;
use nativelink_macro::nativelink_test;
use nativelink_proto::build::bazel::remote::execution::v2::compressor;
use nativelink_util::buf_channel::make_buf_channel_pair;
use nativelink_util::common::DigestInfo;
use nativelink_util::digest_hasher::{DigestHasher, DigestHasherFunc};
use nativelink_util::wire_compression::{
    ZSTD_COMPRESSION_LEVEL, stream_decode_compressed_upload, stream_encode_compressed_download,
};
use pretty_assertions::assert_eq;

fn make_content_with_digest(tag: u8, len: usize) -> (Bytes, DigestInfo) {
    let mut data = vec![0u8; len];
    for (i, b) in data.iter_mut().enumerate() {
        #[allow(clippy::cast_possible_truncation)]
        let byte = match i % 8 {
            0..=4 => tag,
            5 => (i / 8) as u8,
            _ => (i / 4096) as u8,
        };
        *b = byte;
    }
    let mut hasher = DigestHasherFunc::Sha256.hasher();
    hasher.update(&data);
    let digest = hasher.finalize_digest();
    (Bytes::from(data), digest)
}

async fn decode_chunks(
    frame: Bytes,
    split_at: usize,
    digest: DigestInfo,
) -> (Result<(), Error>, Result<usize, Error>) {
    let (mut compressed_tx, compressed_rx) = make_buf_channel_pair();
    let (decoded_tx, mut decoded_rx) = make_buf_channel_pair();
    let decode_fut = stream_decode_compressed_upload(
        compressed_rx,
        compressor::Value::Zstd,
        digest,
        DigestHasherFunc::Sha256,
        decoded_tx,
    );
    let feed_fut = async move {
        compressed_tx.send(frame.slice(0..split_at)).await?;
        if split_at < frame.len() {
            compressed_tx.send(frame.slice(split_at..)).await?;
        }
        compressed_tx.send_eof()
    };
    let pump_fut = async move {
        let mut total = 0usize;
        loop {
            let chunk = decoded_rx.recv().await?;
            if chunk.is_empty() {
                return Ok(total);
            }
            total += chunk.len();
        }
    };
    let (feed_result, decode_result, pump_result) = tokio::join!(feed_fut, decode_fut, pump_fut);
    feed_result.expect("feed must succeed");
    (decode_result, pump_result)
}

// Regression: a blob whose decompressed size is an exact multiple of the
// decoder output buffer (128KiB) made the final `run` fill the output
// exactly with `hint == 0` (frame done); the loop then polled the finished
// decoder once more, received the new-frame-header hint, and the EOF check
// misreported the complete stream as truncated.
#[nativelink_test]
async fn decode_accepts_exact_output_buffer_multiple() -> Result<(), Error> {
    let (content, digest) = make_content_with_digest(0xB2, 1024 * 1024);
    let frame =
        Bytes::from(zstd::bulk::compress(&content, ZSTD_COMPRESSION_LEVEL).expect("compress"));
    let (decode_result, pump_result) = decode_chunks(frame, 65536, digest).await;
    decode_result.expect("exact-multiple frame must decode");
    assert_eq!(pump_result.expect("pump"), content.len());
    Ok(())
}

// Encode -> decode round trip through the streaming codecs.
#[nativelink_test]
async fn stream_encode_decode_round_trip() -> Result<(), Error> {
    let (content, digest) = make_content_with_digest(0xC4, 300_000);
    let (mut raw_tx, raw_rx) = make_buf_channel_pair();
    let (compressed_tx, mut compressed_rx) = make_buf_channel_pair();
    let encode_fut = stream_encode_compressed_download(
        raw_rx,
        compressor::Value::Zstd,
        ZSTD_COMPRESSION_LEVEL,
        compressed_tx,
    );
    let content_for_send = content.clone();
    let send_fut = async move {
        raw_tx.send(content_for_send).await?;
        raw_tx.send_eof()
    };
    let collect_fut = async move {
        let mut frame = Vec::new();
        loop {
            let chunk = compressed_rx.recv().await?;
            if chunk.is_empty() {
                return Ok::<_, Error>(Bytes::from(frame));
            }
            frame.extend_from_slice(&chunk);
        }
    };
    let (send_result, encode_result, frame) = tokio::join!(send_fut, encode_fut, collect_fut);
    send_result.expect("send");
    encode_result.expect("encode");
    let frame = frame.expect("collect");

    let (decode_result, pump_result) = decode_chunks(frame, 1, digest).await;
    decode_result.expect("decode");
    assert_eq!(pump_result.expect("pump"), content.len());
    Ok(())
}
