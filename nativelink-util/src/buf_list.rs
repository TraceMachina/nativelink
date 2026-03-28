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

use std::collections::VecDeque;

use bytes::{Buf, Bytes, BytesMut};

/// A `Buf`-implementing linked list of `Bytes` chunks.
///
/// This allows O(1) append of incoming HTTP/2 data frames and zero-copy
/// extraction when a gRPC message fits within a single front chunk (the
/// common case with 1-4 MiB HTTP/2 frames).
#[derive(Debug)]
pub struct BufList {
    bufs: VecDeque<Bytes>,
    remaining: usize,
}

impl BufList {
    pub fn new() -> Self {
        Self {
            bufs: VecDeque::new(),
            remaining: 0,
        }
    }

    /// Append a chunk to the back of the list. O(1).
    pub fn push(&mut self, bytes: Bytes) {
        if bytes.is_empty() {
            return;
        }
        self.remaining += bytes.len();
        self.bufs.push_back(bytes);
    }
}

impl Default for BufList {
    fn default() -> Self {
        Self::new()
    }
}

impl Buf for BufList {
    #[inline]
    fn remaining(&self) -> usize {
        self.remaining
    }

    #[inline]
    fn chunk(&self) -> &[u8] {
        self.bufs.front().map_or(&[], |b| b.chunk())
    }

    fn advance(&mut self, mut cnt: usize) {
        assert!(
            cnt <= self.remaining,
            "advance past end of BufList: cnt={cnt}, remaining={}",
            self.remaining
        );
        self.remaining -= cnt;
        while cnt > 0 {
            let front = self.bufs.front_mut().expect("bufs empty but cnt > 0");
            let front_len = front.len();
            if cnt >= front_len {
                cnt -= front_len;
                self.bufs.pop_front();
            } else {
                front.advance(cnt);
                cnt = 0;
            }
        }
    }

    /// Zero-copy extraction when the requested length fits within the front
    /// chunk. Falls back to assembling into a `BytesMut` when the message
    /// spans multiple chunks.
    fn copy_to_bytes(&mut self, len: usize) -> Bytes {
        assert!(
            len <= self.remaining,
            "copy_to_bytes past end: len={len}, remaining={}",
            self.remaining
        );

        if len == 0 {
            return Bytes::new();
        }

        // Fast path: front chunk covers the entire request.
        let front_len = self.bufs.front().map_or(0, Bytes::len);
        if len <= front_len {
            self.remaining -= len;
            let front = self.bufs.front_mut().unwrap();
            let result = front.split_to(len);
            if front.is_empty() {
                self.bufs.pop_front();
            }
            return result;
        }

        // Slow path: assemble from multiple chunks.
        let mut buf = BytesMut::with_capacity(len);
        let mut needed = len;
        self.remaining -= len;
        while needed > 0 {
            let front = self.bufs.front_mut().expect("bufs empty but needed > 0");
            let take = needed.min(front.len());
            buf.extend_from_slice(&front[..take]);
            front.advance(take);
            if front.is_empty() {
                self.bufs.pop_front();
            }
            needed -= take;
        }
        buf.freeze()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty() {
        let buf = BufList::new();
        assert_eq!(buf.remaining(), 0);
        assert!(buf.chunk().is_empty());
    }

    #[test]
    fn test_push_and_remaining() {
        let mut buf = BufList::new();
        buf.push(Bytes::from_static(b"hello"));
        buf.push(Bytes::from_static(b" world"));
        assert_eq!(buf.remaining(), 11);
    }

    #[test]
    fn test_zero_copy_single_chunk() {
        let mut buf = BufList::new();
        let original = Bytes::from(vec![1u8; 1024]);
        let data_ptr = original.as_ptr();
        buf.push(original);

        let extracted = buf.copy_to_bytes(512);
        // Should be zero-copy: same underlying allocation.
        assert_eq!(extracted.as_ptr(), data_ptr);
        assert_eq!(extracted.len(), 512);
        assert_eq!(buf.remaining(), 512);
    }

    #[test]
    fn test_copy_spanning_chunks() {
        let mut buf = BufList::new();
        buf.push(Bytes::from_static(b"hel"));
        buf.push(Bytes::from_static(b"lo "));
        buf.push(Bytes::from_static(b"world"));
        assert_eq!(buf.remaining(), 11);

        let extracted = buf.copy_to_bytes(6);
        assert_eq!(&extracted[..], b"hello ");
        assert_eq!(buf.remaining(), 5);

        let rest = buf.copy_to_bytes(5);
        assert_eq!(&rest[..], b"world");
        assert_eq!(buf.remaining(), 0);
    }

    #[test]
    fn test_advance() {
        let mut buf = BufList::new();
        buf.push(Bytes::from_static(b"abc"));
        buf.push(Bytes::from_static(b"def"));

        buf.advance(4);
        assert_eq!(buf.remaining(), 2);
        assert_eq!(buf.chunk(), b"ef");
    }

    #[test]
    fn test_push_empty_ignored() {
        let mut buf = BufList::new();
        buf.push(Bytes::new());
        assert_eq!(buf.remaining(), 0);
        assert!(buf.bufs.is_empty());
    }
}
