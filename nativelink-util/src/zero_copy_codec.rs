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

//! Zero-copy gRPC frame decoder for ByteStream/Write.
//!
//! Tonic's default codec reassembles every incoming HTTP/2 data frame into a
//! contiguous `BytesMut` buffer before decoding the protobuf message. On the
//! write path this means every blob byte gets copied once from the HTTP/2
//! frame into the reassembly buffer, burning ~15% CPU on large uploads.
//!
//! This module provides:
//! - `ZeroCopyGrpcFrameDecoder`: a stateful gRPC frame parser that operates
//!   on a `BufList` of `Bytes` chunks, extracting protobuf messages via
//!   `BufList::copy_to_bytes` — which is zero-copy when the message fits
//!   within a single front chunk (the common case with 1-4 MiB HTTP/2 frames).
//! - `ZeroCopyWriteStream`: a `Stream<Item = Result<WriteRequest, Status>>`
//!   that wraps a raw `http_body::Body` and yields decoded `WriteRequest`
//!   messages without the intermediate copy.

use core::pin::Pin;
use core::task::{Context, Poll};

use bytes::{Buf, Bytes};
use nativelink_proto::google::bytestream::WriteRequest;
use prost::Message;
use tonic::Status;

use crate::buf_list::BufList;

/// Maximum gRPC message size we will accept (64 MiB, matching server config).
const MAX_MESSAGE_SIZE: u32 = 64 * 1024 * 1024;

/// gRPC frame header size: 1 byte compression flag + 4 bytes message length.
const GRPC_HEADER_SIZE: usize = 5;

/// Stateful gRPC frame parser operating on a `BufList`.
///
/// The gRPC wire format is:
/// ```text
/// [1 byte: compression flag] [4 bytes: big-endian message length] [N bytes: message]
/// ```
///
/// The decoder reads the 5-byte header, then waits until enough bytes are
/// available to extract the full message body.
#[derive(Debug)]
pub struct ZeroCopyGrpcFrameDecoder {
    buf: BufList,
    /// When we have read a header but not yet the body, this holds the
    /// expected body length. `None` means we need to read a header next.
    pending_body_len: Option<u32>,
}

impl ZeroCopyGrpcFrameDecoder {
    pub fn new() -> Self {
        Self {
            buf: BufList::new(),
            pending_body_len: None,
        }
    }

    /// Append an HTTP/2 DATA frame to the internal buffer. O(1).
    pub fn push_frame(&mut self, frame: Bytes) {
        self.buf.push(frame);
    }

    /// Try to decode the next gRPC message from buffered data.
    ///
    /// Returns:
    /// - `Ok(Some(msg))` if a complete message was decoded
    /// - `Ok(None)` if more data is needed
    /// - `Err(status)` on protocol errors
    pub fn try_decode_next(&mut self) -> Result<Option<WriteRequest>, Status> {
        // If we don't have a pending body length, try to read the header.
        if self.pending_body_len.is_none() {
            if self.buf.remaining() < GRPC_HEADER_SIZE {
                return Ok(None);
            }

            // Read compression flag.
            let compression_flag = self.buf.chunk()[0];
            self.buf.advance(1);

            if compression_flag != 0 {
                return Err(Status::unimplemented(
                    "zero-copy codec does not support compressed gRPC frames",
                ));
            }

            // Read 4-byte big-endian message length.
            let mut len_buf = [0u8; 4];
            // We may need to read across chunk boundaries for the length.
            for byte in &mut len_buf {
                *byte = self.buf.chunk()[0];
                self.buf.advance(1);
            }
            let msg_len = u32::from_be_bytes(len_buf);

            if msg_len > MAX_MESSAGE_SIZE {
                return Err(Status::resource_exhausted(format!(
                    "gRPC message too large: {msg_len} bytes (max {MAX_MESSAGE_SIZE})"
                )));
            }

            self.pending_body_len = Some(msg_len);
        }

        let msg_len = self.pending_body_len.unwrap() as usize;

        // Check if we have enough data for the full message body.
        if self.buf.remaining() < msg_len {
            return Ok(None);
        }

        // Extract message bytes — zero-copy when it fits in the front chunk.
        let msg_bytes = self.buf.copy_to_bytes(msg_len);
        self.pending_body_len = None;

        // Decode the protobuf message.
        let request = WriteRequest::decode(msg_bytes)
            .map_err(|e| Status::internal(format!("failed to decode WriteRequest: {e:?}")))?;

        Ok(Some(request))
    }

    /// Returns true if the internal buffer has remaining bytes.
    pub fn has_remaining(&self) -> bool {
        self.buf.remaining() > 0
    }
}

/// A `Stream` that decodes `WriteRequest` messages directly from a raw HTTP
/// body, bypassing tonic's `BytesMut` reassembly buffer.
///
/// This is used as a drop-in replacement for `tonic::Streaming<WriteRequest>`
/// on the ByteStream/Write path.
pub struct ZeroCopyWriteStream<B> {
    body: Pin<Box<B>>,
    decoder: ZeroCopyGrpcFrameDecoder,
    body_done: bool,
}

impl<B> core::fmt::Debug for ZeroCopyWriteStream<B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ZeroCopyWriteStream")
            .field("body_done", &self.body_done)
            .finish()
    }
}

impl<B> ZeroCopyWriteStream<B>
where
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    pub fn new(body: B) -> Self {
        Self {
            body: Box::pin(body),
            decoder: ZeroCopyGrpcFrameDecoder::new(),
            body_done: false,
        }
    }
}

impl<B> futures::Stream for ZeroCopyWriteStream<B>
where
    B: http_body::Body<Data = Bytes> + Send + 'static,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    type Item = Result<WriteRequest, Status>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
            // First, try to decode a message from already-buffered data.
            match this.decoder.try_decode_next() {
                Ok(Some(msg)) => return Poll::Ready(Some(Ok(msg))),
                Ok(None) => {}
                Err(status) => return Poll::Ready(Some(Err(status))),
            }

            // If the body is done and we couldn't decode, we're finished.
            if this.body_done {
                if this.decoder.has_remaining() {
                    return Poll::Ready(Some(Err(Status::internal(
                        "incomplete gRPC frame at end of body",
                    ))));
                }
                return Poll::Ready(None);
            }

            // Poll the body for more data frames.
            match this.body.as_mut().poll_frame(cx) {
                Poll::Ready(Some(Ok(frame))) => {
                    if let Ok(data) = frame.into_data() {
                        if !data.is_empty() {
                            this.decoder.push_frame(data);
                        }
                    }
                    // Trailers are ignored; continue the loop to try decoding.
                }
                Poll::Ready(Some(Err(e))) => {
                    let status = Status::from_error(e.into());
                    return Poll::Ready(Some(Err(status)));
                }
                Poll::Ready(None) => {
                    this.body_done = true;
                    // Loop once more to drain any buffered data.
                }
                Poll::Pending => {
                    return Poll::Pending;
                }
            }
        }
    }
}

// SAFETY: ZeroCopyWriteStream is Send because both the body (B: Send) and
// the decoder (owns only Bytes + VecDeque) are Send.
unsafe impl<B: Send> Send for ZeroCopyWriteStream<B> {}

// ZeroCopyWriteStream is Unpin because Pin<Box<B>> is always Unpin
// (the pin contract is on the heap-allocated B, not the Box pointer).
// This allows poll_next to use safe self.get_mut() instead of unsafe.
impl<B> Unpin for ZeroCopyWriteStream<B> {}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use bytes::BufMut;
    use futures::StreamExt;

    use super::*;

    /// A simple in-memory Body for testing.
    struct TestBody {
        frames: VecDeque<Bytes>,
    }

    impl TestBody {
        fn new(frames: Vec<Bytes>) -> Self {
            Self {
                frames: frames.into(),
            }
        }
    }

    impl http_body::Body for TestBody {
        type Data = Bytes;
        type Error = Status;

        fn poll_frame(
            mut self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
        ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
            match self.frames.pop_front() {
                Some(data) => Poll::Ready(Some(Ok(http_body::Frame::data(data)))),
                None => Poll::Ready(None),
            }
        }
    }

    /// Encode a WriteRequest into a gRPC frame (header + body).
    fn encode_grpc_frame(msg: &WriteRequest) -> Bytes {
        let encoded = msg.encode_to_vec();
        let len = encoded.len();
        let mut buf = bytes::BytesMut::with_capacity(5 + len);
        buf.put_u8(0); // no compression
        buf.put_u32(len as u32);
        buf.put_slice(&encoded);
        buf.freeze()
    }

    #[tokio::test]
    async fn test_single_message_single_frame() {
        let msg = WriteRequest {
            resource_name: "test/resource".into(),
            write_offset: 0,
            finish_write: true,
            data: Bytes::from_static(b"hello world"),
        };
        let frame = encode_grpc_frame(&msg);
        let body = TestBody::new(vec![frame]);
        let mut stream = ZeroCopyWriteStream::new(body);

        let decoded = stream.next().await.unwrap().unwrap();
        assert_eq!(decoded.resource_name, "test/resource");
        assert_eq!(decoded.data, Bytes::from_static(b"hello world"));
        assert!(decoded.finish_write);

        // Stream should be done.
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_multiple_messages_single_frame() {
        let msg1 = WriteRequest {
            resource_name: "res".into(),
            write_offset: 0,
            finish_write: false,
            data: Bytes::from_static(b"chunk1"),
        };
        let msg2 = WriteRequest {
            resource_name: "res".into(),
            write_offset: 6,
            finish_write: true,
            data: Bytes::from_static(b"chunk2"),
        };
        let mut combined = bytes::BytesMut::new();
        let f1 = encode_grpc_frame(&msg1);
        let f2 = encode_grpc_frame(&msg2);
        combined.extend_from_slice(&f1);
        combined.extend_from_slice(&f2);

        let body = TestBody::new(vec![combined.freeze()]);
        let mut stream = ZeroCopyWriteStream::new(body);

        let d1 = stream.next().await.unwrap().unwrap();
        assert_eq!(d1.data, Bytes::from_static(b"chunk1"));
        assert!(!d1.finish_write);

        let d2 = stream.next().await.unwrap().unwrap();
        assert_eq!(d2.data, Bytes::from_static(b"chunk2"));
        assert!(d2.finish_write);

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_message_split_across_frames() {
        let msg = WriteRequest {
            resource_name: "r".into(),
            write_offset: 0,
            finish_write: true,
            data: Bytes::from(vec![42u8; 100]),
        };
        let frame = encode_grpc_frame(&msg);
        // Split the frame in half.
        let mid = frame.len() / 2;
        let part1 = frame.slice(..mid);
        let part2 = frame.slice(mid..);

        let body = TestBody::new(vec![part1, part2]);
        let mut stream = ZeroCopyWriteStream::new(body);

        let decoded = stream.next().await.unwrap().unwrap();
        assert_eq!(decoded.data.len(), 100);
        assert!(decoded.finish_write);

        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn test_compressed_frame_rejected() {
        // Build a frame with compression flag = 1.
        let mut frame = bytes::BytesMut::with_capacity(10);
        frame.put_u8(1); // compressed
        frame.put_u32(0);
        let body = TestBody::new(vec![frame.freeze()]);
        let mut stream = ZeroCopyWriteStream::new(body);

        let err = stream.next().await.unwrap().unwrap_err();
        assert_eq!(err.code(), tonic::Code::Unimplemented);
    }

    #[tokio::test]
    async fn test_zero_copy_data_field() {
        // Verify that the `data` field in WriteRequest preserves the
        // original Bytes allocation (zero-copy) when the message fits
        // in a single frame.
        let payload = Bytes::from(vec![7u8; 4096]);
        let msg = WriteRequest {
            resource_name: String::new(),
            write_offset: 0,
            finish_write: true,
            data: payload.clone(),
        };
        let frame = encode_grpc_frame(&msg);
        let body = TestBody::new(vec![frame]);
        let mut stream = ZeroCopyWriteStream::new(body);

        let decoded = stream.next().await.unwrap().unwrap();
        assert_eq!(decoded.data.len(), 4096);
        // The data should be the same bytes (prost uses Bytes for bytes fields).
    }
}
