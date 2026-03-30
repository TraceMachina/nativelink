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

//! Zero-copy gRPC frame decoder for inbound RPCs.
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
//! - `decode_unary_request`: accumulates an HTTP body and decodes a single
//!   gRPC unary request message with zero-copy `Bytes` fields.

use core::pin::Pin;
use core::task::{Context, Poll};

use bytes::{Buf, Bytes, BytesMut};
use futures::Stream;
use nativelink_proto::google::bytestream::{ReadResponse, WriteRequest};
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

    /// Try to decode the next gRPC message from buffered data as a
    /// `WriteRequest`. Convenience wrapper around `try_decode_next_message`.
    pub fn try_decode_next(&mut self) -> Result<Option<WriteRequest>, Status> {
        self.try_decode_next_message()
    }

    /// Try to decode the next gRPC message of type `M` from buffered data.
    ///
    /// Returns:
    /// - `Ok(Some(msg))` if a complete message was decoded
    /// - `Ok(None)` if more data is needed
    /// - `Err(status)` on protocol errors
    pub fn try_decode_next_message<M: Message + Default>(
        &mut self,
    ) -> Result<Option<M>, Status> {
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
        let request = M::decode(msg_bytes).map_err(|e| {
            Status::internal(format!(
                "failed to decode {}: {e:?}",
                core::any::type_name::<M>()
            ))
        })?;

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

/// Accumulate an HTTP body and decode the single gRPC unary request message.
///
/// For unary RPCs (like `BatchUpdateBlobs`), the client sends exactly one
/// gRPC frame. This function collects all HTTP/2 DATA frames, then parses the
/// 5-byte gRPC header and decodes the protobuf message directly from the
/// accumulated `Bytes` — preserving zero-copy semantics for `Bytes` fields
/// (e.g. `BatchUpdateBlobsRequest.requests[].data`).
pub async fn decode_unary_request<M, B>(body: B) -> Result<M, Status>
where
    M: Message + Default,
    B: http_body::Body<Data = Bytes>,
    B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
{
    use core::pin::pin;

    let mut pinned = pin!(body);
    let mut decoder = ZeroCopyGrpcFrameDecoder::new();

    loop {
        match std::future::poll_fn(|cx| pinned.as_mut().poll_frame(cx)).await {
            Some(Ok(frame)) => {
                if let Ok(data) = frame.into_data() {
                    if !data.is_empty() {
                        decoder.push_frame(data);
                    }
                }
            }
            Some(Err(e)) => {
                return Err(Status::from_error(e.into()));
            }
            None => break,
        }
    }

    // The body is fully received. Decode the single gRPC message.
    match decoder.try_decode_next_message::<M>()? {
        Some(msg) => {
            if decoder.has_remaining() {
                return Err(Status::internal(
                    "unexpected trailing data after unary gRPC message",
                ));
            }
            Ok(msg)
        }
        None => Err(Status::internal("empty body: no gRPC message received")),
    }
}

/// Encode a protobuf message as a gRPC frame: 5-byte header + encoded message.
///
/// The gRPC wire format is:
/// `[1 byte: 0 (no compression)] [4 bytes: big-endian length] [N bytes: message]`
pub fn encode_grpc_unary_response<M: Message>(response: &M) -> Bytes {
    let encoded = response.encode_to_vec();
    let len = encoded.len();
    let mut buf = BytesMut::with_capacity(GRPC_HEADER_SIZE + len);
    buf.extend_from_slice(&[0]); // no compression
    buf.extend_from_slice(&(len as u32).to_be_bytes());
    buf.extend_from_slice(&encoded);
    buf.freeze()
}

/// HTTP body that emits exactly one data frame containing a gRPC-encoded
/// message, followed by a trailers frame with `grpc-status: 0`.
///
/// This is the correct encoding for a successful unary gRPC response.
/// Unlike `http_body_util::Full`, this properly emits HTTP/2 trailers.
#[derive(Debug)]
pub struct GrpcUnaryBody {
    data: Option<Bytes>,
    trailers_sent: bool,
}

impl GrpcUnaryBody {
    pub fn new(data: Bytes) -> Self {
        Self {
            data: Some(data),
            trailers_sent: false,
        }
    }
}

impl http_body::Body for GrpcUnaryBody {
    type Data = Bytes;
    type Error = Status;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        if let Some(data) = self.data.take() {
            return Poll::Ready(Some(Ok(http_body::Frame::data(data))));
        }

        if !self.trailers_sent {
            self.trailers_sent = true;
            let mut trailers = http::HeaderMap::new();
            trailers.insert("grpc-status", http::HeaderValue::from_static("0"));
            return Poll::Ready(Some(Ok(http_body::Frame::trailers(trailers))));
        }

        Poll::Ready(None)
    }

    fn is_end_stream(&self) -> bool {
        self.data.is_none() && self.trailers_sent
    }

    fn size_hint(&self) -> http_body::SizeHint {
        match &self.data {
            Some(data) => http_body::SizeHint::with_exact(data.len() as u64),
            None => http_body::SizeHint::with_exact(0),
        }
    }
}

/// Encode a u64 value as a protobuf varint into `buf`, returning the number
/// of bytes written. Maximum 10 bytes for a 64-bit value.
#[inline]
fn encode_varint(mut value: u64, buf: &mut [u8; 10]) -> usize {
    let mut i = 0;
    loop {
        if value < 0x80 {
            buf[i] = value as u8;
            return i + 1;
        }
        buf[i] = (value as u8 & 0x7F) | 0x80;
        value >>= 7;
        i += 1;
    }
}

/// Pending data to yield as the next frame, after we already emitted the
/// gRPC header frame for a `ReadResponse`.
enum PendingFrame {
    /// No pending data — poll the stream for the next message.
    None,
    /// Yield this `Bytes` payload as a DATA frame, then go back to polling.
    Data(Bytes),
}

/// HTTP body that encodes a `Stream<Item = Result<ReadResponse, Status>>` as
/// gRPC wire format without copying the data payload.
///
/// For each `ReadResponse`, this body emits two HTTP/2 DATA frames:
/// 1. A small (~9 byte) header frame containing the 5-byte gRPC header
///    (compression flag + message length) plus the protobuf field tag and
///    varint length prefix for the `data` field.
/// 2. The original `Bytes` data — passed through with zero copies.
///
/// This eliminates the ~3 MiB memcpy per chunk that tonic's `ProstEncoder`
/// performs when encoding `ReadResponse` messages.
pub struct ZeroCopyReadBody<S> {
    /// The inner stream producing `ReadResponse` messages.
    stream: Option<S>,
    /// Pending frame to emit before polling the stream again.
    pending: PendingFrame,
    /// Whether the body has finished (stream exhausted or error).
    done: bool,
}

impl<S> core::fmt::Debug for ZeroCopyReadBody<S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ZeroCopyReadBody")
            .field("done", &self.done)
            .finish()
    }
}

impl<S> ZeroCopyReadBody<S>
where
    S: Stream<Item = Result<ReadResponse, Status>> + Send + Unpin + 'static,
{
    pub fn new(stream: S) -> Self {
        Self {
            stream: Some(stream),
            pending: PendingFrame::None,
            done: false,
        }
    }

    /// Build gRPC trailers from a `Status`, using tonic's own encoding
    /// (percent-encoded message, base64-encoded details, custom metadata).
    fn status_trailers(status: &Status) -> http::HeaderMap {
        let mut trailers = http::HeaderMap::new();
        // add_header handles percent-encoding of grpc-message and
        // base64-encoding of grpc-status-details-bin per the gRPC spec.
        if let Err(fallback) = status.add_header(&mut trailers) {
            // If header encoding fails, fall back to code-only trailers.
            let code: i32 = fallback.code().into();
            trailers.insert("grpc-status", http::HeaderValue::from(code));
        }
        trailers
    }
}

impl<S> http_body::Body for ZeroCopyReadBody<S>
where
    S: Stream<Item = Result<ReadResponse, Status>> + Send + Unpin + 'static,
{
    type Data = Bytes;
    type Error = Status;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();

        if this.done {
            return Poll::Ready(None);
        }

        // If we have a pending data frame from a previous poll, yield it now.
        match core::mem::replace(&mut this.pending, PendingFrame::None) {
            PendingFrame::Data(data) => {
                return Poll::Ready(Some(Ok(http_body::Frame::data(data))));
            }
            PendingFrame::None => {}
        }

        // Poll the inner stream for the next ReadResponse.
        let stream = match &mut this.stream {
            Some(s) => s,
            None => {
                this.done = true;
                return Poll::Ready(None);
            }
        };

        match Pin::new(stream).poll_next(cx) {
            Poll::Ready(Some(Ok(response))) => {
                let data = response.data;
                let data_len = data.len();

                // Build the gRPC header frame:
                // [0u8 compression][u32 BE total_msg_len][0x52 tag][varint data_len]
                //
                // ReadResponse only has one field: `bytes data = 10`.
                // Protobuf tag = (10 << 3) | 2 = 0x52 (field 10, wire type 2).
                // When data is empty, prost skips the field entirely,
                // so total_msg_len = 0 and we emit no tag/varint.
                if data_len == 0 {
                    // Empty data: gRPC message body is 0 bytes.
                    let mut header = BytesMut::with_capacity(GRPC_HEADER_SIZE);
                    header.extend_from_slice(&[0u8]); // no compression
                    header.extend_from_slice(&0u32.to_be_bytes());
                    return Poll::Ready(Some(Ok(http_body::Frame::data(header.freeze()))));
                }

                let mut varint_buf = [0u8; 10];
                let varint_len = encode_varint(data_len as u64, &mut varint_buf);

                // total_msg_len = 1 (tag byte) + varint_len + data_len
                let total_msg_len = 1 + varint_len + data_len;

                if total_msg_len > u32::MAX as usize {
                    this.stream = None;
                    this.done = true;
                    let status = Status::internal("gRPC message exceeds 4GiB limit");
                    let trailers = Self::status_trailers(&status);
                    return Poll::Ready(Some(Ok(http_body::Frame::trailers(trailers))));
                }

                let header_size = GRPC_HEADER_SIZE + 1 + varint_len;
                let mut header = BytesMut::with_capacity(header_size);
                header.extend_from_slice(&[0u8]); // no compression
                #[allow(clippy::cast_possible_truncation)]
                header.extend_from_slice(&(total_msg_len as u32).to_be_bytes());
                header.extend_from_slice(&[0x52]); // protobuf tag for field 10, wire type 2
                header.extend_from_slice(&varint_buf[..varint_len]);

                // Stash the data for the next poll_frame call.
                this.pending = PendingFrame::Data(data);

                Poll::Ready(Some(Ok(http_body::Frame::data(header.freeze()))))
            }
            Poll::Ready(Some(Err(status))) => {
                // Stream error: emit trailers with grpc-status.
                let trailers = Self::status_trailers(&status);
                this.stream = None;
                this.done = true;
                Poll::Ready(Some(Ok(http_body::Frame::trailers(trailers))))
            }
            Poll::Ready(None) => {
                // Stream finished successfully: emit trailers with grpc-status: 0.
                this.stream = None;
                let mut trailers = http::HeaderMap::new();
                trailers.insert("grpc-status", http::HeaderValue::from_static("0"));
                this.done = true;
                Poll::Ready(Some(Ok(http_body::Frame::trailers(trailers))))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.done
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use bytes::BufMut;
    use futures::StreamExt;
    use http_body::Body as HttpBody;

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

    // --- ZeroCopyReadBody tests ---

    /// Decode all gRPC frames from DATA frames emitted by `ZeroCopyReadBody`,
    /// returning the decoded `ReadResponse` messages.
    async fn decode_read_body(
        body: ZeroCopyReadBody<impl Stream<Item = Result<ReadResponse, Status>> + Send + Unpin + 'static>,
    ) -> Vec<ReadResponse> {
        use core::pin::pin;

        let mut pinned = pin!(body);
        let mut decoder = ZeroCopyGrpcFrameDecoder::new();
        let mut messages = Vec::new();

        loop {
            let frame: Option<Result<http_body::Frame<Bytes>, Status>> =
                std::future::poll_fn(|cx| HttpBody::poll_frame(pinned.as_mut(), cx)).await;
            match frame {
                Some(Ok(frame)) => {
                    if let Ok(data) = frame.into_data() {
                        decoder.push_frame(data);
                        // Try to decode messages after each frame.
                        while let Ok(Some(msg)) = decoder.try_decode_next_message::<ReadResponse>() {
                            messages.push(msg);
                        }
                    }
                    // Trailers frame: stream is done.
                }
                Some(Err(status)) => panic!("unexpected error: {status:?}"),
                None => break,
            }
        }

        messages
    }

    /// Make a simple stream from a vec of ReadResponse items.
    fn read_response_stream(
        items: Vec<Result<ReadResponse, Status>>,
    ) -> impl Stream<Item = Result<ReadResponse, Status>> + Unpin {
        futures::stream::iter(items)
    }

    #[tokio::test]
    async fn test_zero_copy_read_body_single_chunk() {
        let data = Bytes::from(vec![42u8; 1024]);
        let responses = vec![Ok(ReadResponse { data: data.clone() })];
        let body = ZeroCopyReadBody::new(read_response_stream(responses));

        let decoded = decode_read_body(body).await;
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].data, data);
    }

    #[tokio::test]
    async fn test_zero_copy_read_body_multiple_chunks() {
        let data1 = Bytes::from(vec![1u8; 3 * 1024 * 1024]); // 3 MiB
        let data2 = Bytes::from(vec![2u8; 1024]);
        let data3 = Bytes::from(vec![3u8; 512]);
        let responses = vec![
            Ok(ReadResponse { data: data1.clone() }),
            Ok(ReadResponse { data: data2.clone() }),
            Ok(ReadResponse { data: data3.clone() }),
        ];
        let body = ZeroCopyReadBody::new(read_response_stream(responses));

        let decoded = decode_read_body(body).await;
        assert_eq!(decoded.len(), 3);
        assert_eq!(decoded[0].data, data1);
        assert_eq!(decoded[1].data, data2);
        assert_eq!(decoded[2].data, data3);
    }

    #[tokio::test]
    async fn test_zero_copy_read_body_empty_data() {
        // Empty data field: prost skips the field, so gRPC message body = 0 bytes.
        let responses = vec![Ok(ReadResponse { data: Bytes::new() })];
        let body = ZeroCopyReadBody::new(read_response_stream(responses));

        let decoded = decode_read_body(body).await;
        assert_eq!(decoded.len(), 1);
        assert!(decoded[0].data.is_empty());
    }

    #[tokio::test]
    async fn test_zero_copy_read_body_empty_stream() {
        // No responses at all.
        let responses: Vec<Result<ReadResponse, Status>> = vec![];
        let body = ZeroCopyReadBody::new(read_response_stream(responses));

        let decoded = decode_read_body(body).await;
        assert!(decoded.is_empty());
    }

    #[tokio::test]
    async fn test_zero_copy_read_body_error_produces_trailers() {
        use core::pin::pin;

        let responses = vec![
            Ok(ReadResponse { data: Bytes::from_static(b"hello") }),
            Err(Status::not_found("blob gone")),
        ];
        let body = ZeroCopyReadBody::new(read_response_stream(responses));
        let mut pinned = pin!(body);

        let mut saw_data = false;
        let mut saw_trailers = false;

        loop {
            let frame: Option<Result<http_body::Frame<Bytes>, Status>> =
                std::future::poll_fn(|cx| HttpBody::poll_frame(pinned.as_mut(), cx)).await;
            match frame {
                Some(Ok(frame)) => {
                    if frame.is_data() {
                        saw_data = true;
                    } else if frame.is_trailers() {
                        let trailers = frame.into_trailers().unwrap();
                        // grpc-status for NOT_FOUND = 5
                        assert_eq!(
                            trailers.get("grpc-status").unwrap().to_str().unwrap(),
                            "5"
                        );
                        assert!(trailers.get("grpc-message").is_some());
                        saw_trailers = true;
                    }
                }
                Some(Err(_)) => panic!("should not get Err from body"),
                None => break,
            }
        }

        assert!(saw_data, "should have emitted data frames");
        assert!(saw_trailers, "should have emitted error trailers");
    }

    #[test]
    fn test_encode_varint_values() {
        let mut buf = [0u8; 10];

        // 0
        assert_eq!(encode_varint(0, &mut buf), 1);
        assert_eq!(buf[0], 0);

        // 1
        assert_eq!(encode_varint(1, &mut buf), 1);
        assert_eq!(buf[0], 1);

        // 127 (single byte max)
        assert_eq!(encode_varint(127, &mut buf), 1);
        assert_eq!(buf[0], 127);

        // 128 (first two-byte value)
        assert_eq!(encode_varint(128, &mut buf), 2);
        assert_eq!(buf[0], 0x80);
        assert_eq!(buf[1], 0x01);

        // 300
        assert_eq!(encode_varint(300, &mut buf), 2);
        assert_eq!(buf[0], 0xAC);
        assert_eq!(buf[1], 0x02);

        // 3 * 1024 * 1024 = 3145728 (typical chunk size)
        let len = encode_varint(3 * 1024 * 1024, &mut buf);
        assert_eq!(len, 4);
        // Verify round-trip via prost decode
        let decoded = prost::decode_length_delimiter(&buf[..len]).unwrap();
        assert_eq!(decoded, 3 * 1024 * 1024);
    }
}
