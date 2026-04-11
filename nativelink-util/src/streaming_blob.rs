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

/// Shared, append-only byte buffer with a single writer and multiple
/// concurrent readers.  Designed for streaming CAS blobs to readers
/// before the writer has finished (read-while-write).
///
/// See `docs/streaming-blob-pipeline-design.md` for the full design.
use core::fmt;
use core::sync::atomic::{AtomicU64, Ordering};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use bytes::Bytes;
use nativelink_error::{Code, Error, make_err};
use parking_lot::{Mutex, RwLock};
use tokio::sync::Notify;
use tracing::{debug, warn};

use crate::common::DigestInfo;

/// Inner shared state for a streaming blob.
///
/// The writer appends `Bytes` chunks to the deque and notifies
/// waiting readers.  Each reader maintains its own cursor and
/// advances independently.
pub struct StreamingBlobInner {
    /// Append-only chunk deque.  Writers take a write-lock;
    /// readers take a read-lock (shared access for indexing).
    chunks: RwLock<VecDeque<Bytes>>,

    /// Monotonically increasing count of chunks appended.
    chunk_count: AtomicU64,

    /// Total bytes appended so far.
    bytes_written: AtomicU64,

    /// Wakes readers on new data or terminal state.
    notify: Notify,

    /// Terminal state:
    /// - `None`       — writer still active
    /// - `Some(Ok)` — writer sent EOF (success)
    /// - `Some(Err)` — writer errored or dropped
    terminal: Mutex<Option<Result<(), Error>>>,

    /// Digest for this blob.
    digest: DigestInfo,

    /// Maximum bytes to buffer before evicting old chunks.
    max_buffer_bytes: u64,

    /// Index of the earliest chunk still retained in the deque.
    /// Chunks before this index have been evicted.
    earliest_chunk_idx: AtomicU64,
}

impl fmt::Debug for StreamingBlobInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamingBlobInner")
            .field("digest", &self.digest)
            .field("chunk_count", &self.chunk_count.load(Ordering::Relaxed))
            .field("bytes_written", &self.bytes_written.load(Ordering::Relaxed))
            .field(
                "earliest_chunk_idx",
                &self.earliest_chunk_idx.load(Ordering::Relaxed),
            )
            .field("max_buffer_bytes", &self.max_buffer_bytes)
            .field("terminal", &self.terminal.lock().is_some())
            .finish()
    }
}

impl StreamingBlobInner {
    fn new(digest: DigestInfo, max_buffer_bytes: u64) -> Self {
        Self {
            chunks: RwLock::new(VecDeque::new()),
            chunk_count: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
            notify: Notify::new(),
            terminal: Mutex::new(None),
            digest,
            max_buffer_bytes,
            earliest_chunk_idx: AtomicU64::new(0),
        }
    }

    /// Returns true if the terminal state has been set.
    fn is_terminal(&self) -> bool {
        self.terminal.lock().is_some()
    }

    /// Returns the digest associated with this blob.
    pub fn digest(&self) -> &DigestInfo {
        &self.digest
    }
}

/// Writer handle for a streaming blob.
///
/// There should be exactly one writer per `StreamingBlobInner`.
/// Dropping the writer without calling `send_eof` sets a terminal
/// error so readers do not hang indefinitely.
pub struct StreamingBlobWriter {
    inner: Arc<StreamingBlobInner>,
    eof_sent: bool,
}

impl fmt::Debug for StreamingBlobWriter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamingBlobWriter")
            .field("inner", &self.inner)
            .field("eof_sent", &self.eof_sent)
            .finish()
    }
}

impl StreamingBlobWriter {
    fn new(inner: Arc<StreamingBlobInner>) -> Self {
        Self {
            inner,
            eof_sent: false,
        }
    }

    /// Append a chunk of data and notify waiting readers.
    ///
    /// After appending, evicts the oldest chunks if the total
    /// buffered bytes exceed `max_buffer_bytes`.
    pub async fn send(&self, chunk: Bytes) -> Result<(), Error> {
        if self.inner.is_terminal() {
            return Err(make_err!(
                Code::Internal,
                "cannot send after terminal state"
            ));
        }

        let chunk_len = chunk.len() as u64;

        {
            let mut chunks = self.inner.chunks.write();
            chunks.push_back(chunk);
        }

        self.inner.chunk_count.fetch_add(1, Ordering::Release);
        let total = self.inner.bytes_written.fetch_add(chunk_len, Ordering::Release) + chunk_len;

        // Sliding window eviction: drop oldest chunks while over budget.
        if total > self.inner.max_buffer_bytes {
            let mut chunks = self.inner.chunks.write();
            let mut buffered = {
                // Sum all retained chunk sizes.
                chunks.iter().map(|c| c.len() as u64).sum::<u64>()
            };
            while buffered > self.inner.max_buffer_bytes && !chunks.is_empty() {
                if let Some(evicted) = chunks.pop_front() {
                    buffered -= evicted.len() as u64;
                    self.inner.earliest_chunk_idx.fetch_add(1, Ordering::Release);
                }
            }
        }

        self.inner.notify.notify_waiters();
        Ok(())
    }

    /// Signal successful end-of-file.  After this, readers that have
    /// consumed all chunks will see EOF.
    pub fn send_eof(&mut self) -> Result<(), Error> {
        let mut terminal = self.inner.terminal.lock();
        if terminal.is_some() {
            return Err(make_err!(
                Code::Internal,
                "terminal state already set"
            ));
        }
        *terminal = Some(Ok(()));
        self.eof_sent = true;
        drop(terminal);

        debug!(
            digest = %self.inner.digest,
            bytes_written = %self.inner.bytes_written.load(Ordering::Relaxed),
            "streaming blob writer sent eof"
        );

        self.inner.notify.notify_waiters();
        Ok(())
    }

    /// Signal a write error.  All readers will observe this error.
    pub fn send_error(&mut self, err: Error) {
        let mut terminal = self.inner.terminal.lock();
        if terminal.is_some() {
            return;
        }
        warn!(
            digest = %self.inner.digest,
            ?err,
            "streaming blob writer error"
        );
        *terminal = Some(Err(err));
        self.eof_sent = true;
        drop(terminal);

        self.inner.notify.notify_waiters();
    }
}

impl Drop for StreamingBlobWriter {
    fn drop(&mut self) {
        if !self.eof_sent {
            let mut terminal = self.inner.terminal.lock();
            if terminal.is_none() {
                warn!(
                    digest = %self.inner.digest,
                    "streaming blob writer dropped without eof"
                );
                *terminal = Some(Err(make_err!(
                    Code::Internal,
                    "writer dropped without sending EOF"
                )));
                drop(terminal);
                self.inner.notify.notify_waiters();
            }
        }
    }
}

/// Reader handle for a streaming blob.
///
/// Each reader maintains its own cursor position and advances
/// independently of other readers.  Readers never block the
/// writer or each other.
pub struct StreamingBlobReader {
    inner: Arc<StreamingBlobInner>,
    /// Absolute index of the next chunk to read.
    cursor_chunk_idx: u64,
    /// Byte offset within the current chunk (reserved for future
    /// partial-chunk reads; currently always 0).
    #[allow(dead_code)]
    cursor_byte_offset: u64,
}

impl fmt::Debug for StreamingBlobReader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamingBlobReader")
            .field("digest", &self.inner.digest)
            .field("cursor_chunk_idx", &self.cursor_chunk_idx)
            .field("cursor_byte_offset", &self.cursor_byte_offset)
            .finish()
    }
}

impl StreamingBlobReader {
    fn new(inner: Arc<StreamingBlobInner>) -> Self {
        let earliest = inner.earliest_chunk_idx.load(Ordering::Acquire);
        Self {
            inner,
            cursor_chunk_idx: earliest,
            cursor_byte_offset: 0,
        }
    }

    /// Returns the next chunk of data, waiting if necessary.
    ///
    /// - If the cursor has fallen behind the sliding window,
    ///   returns `Code::Unavailable` (retryable).
    /// - If a chunk is available, returns it and advances the cursor.
    /// - If no chunk is available and the writer is still active,
    ///   waits for notification and retries.
    /// - If the writer sent EOF and no more chunks remain, returns
    ///   empty `Bytes` (signals EOF to the caller).
    /// - If the writer sent an error, returns that error.
    pub async fn next_chunk(&mut self) -> Result<Bytes, Error> {
        loop {
            let earliest = self.inner.earliest_chunk_idx.load(Ordering::Acquire);
            if self.cursor_chunk_idx < earliest {
                return Err(make_err!(
                    Code::Unavailable,
                    "reader fell behind sliding window (cursor={}, earliest={})",
                    self.cursor_chunk_idx,
                    earliest
                ));
            }

            let chunk_count = self.inner.chunk_count.load(Ordering::Acquire);

            // Check if a chunk is available at our cursor position.
            if self.cursor_chunk_idx < chunk_count {
                let chunks = self.inner.chunks.read();
                // Convert absolute index to deque-relative index.
                let deque_idx = (self.cursor_chunk_idx - earliest) as usize;
                if let Some(chunk) = chunks.get(deque_idx) {
                    let data = chunk.clone();
                    self.cursor_chunk_idx += 1;
                    self.cursor_byte_offset = 0;
                    return Ok(data);
                }
                // earliest_chunk_idx advanced between our load and the
                // read-lock acquisition — re-check from the top.
                drop(chunks);
                continue;
            }

            // No chunk available — check terminal state.
            {
                let terminal = self.inner.terminal.lock();
                if let Some(ref result) = *terminal {
                    // Re-check: there might be trailing chunks we missed.
                    let final_count = self.inner.chunk_count.load(Ordering::Acquire);
                    if self.cursor_chunk_idx < final_count {
                        drop(terminal);
                        continue;
                    }
                    return match result {
                        Ok(()) => Ok(Bytes::new()),
                        Err(e) => Err(e.clone()),
                    };
                }
            }

            // Writer still active, no data yet — wait for notification.
            self.inner.notify.notified().await;
        }
    }
}

/// Constructors for the streaming blob primitive.
#[derive(Debug, Clone, Copy)]
pub struct StreamingBlob;

impl StreamingBlob {
    /// Create a new streaming blob with the given digest and memory budget.
    ///
    /// Returns a writer (single owner) and the first reader.  Additional
    /// readers can be created via `new_reader`.
    pub fn new(
        digest: DigestInfo,
        max_buffer_bytes: u64,
    ) -> (StreamingBlobWriter, StreamingBlobReader) {
        let inner = Arc::new(StreamingBlobInner::new(digest, max_buffer_bytes));
        let writer = StreamingBlobWriter::new(Arc::clone(&inner));
        let reader = StreamingBlobReader::new(Arc::clone(&inner));
        (writer, reader)
    }

    /// Create an additional reader from an existing inner handle.
    pub fn new_reader(inner: &Arc<StreamingBlobInner>) -> StreamingBlobReader {
        StreamingBlobReader::new(Arc::clone(inner))
    }
}

/// Registry of in-flight streaming blobs keyed by digest.
///
/// Used at the service layer (e.g. `ByteStreamServer`) to allow
/// readers to discover blobs that are still being written.
pub struct InFlightBlobMap {
    map: RwLock<HashMap<DigestInfo, Arc<StreamingBlobInner>>>,
}

impl fmt::Debug for InFlightBlobMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InFlightBlobMap")
            .field("len", &self.map.read().len())
            .finish()
    }
}

impl InFlightBlobMap {
    pub fn new() -> Self {
        Self {
            map: RwLock::new(HashMap::new()),
        }
    }

    /// Register a new streaming blob.  Returns a writer and reader
    /// pair.  The inner is stored in the map for discovery by other
    /// readers.
    pub fn register(
        &self,
        digest: DigestInfo,
        max_buffer_bytes: u64,
    ) -> (StreamingBlobWriter, StreamingBlobReader) {
        let inner = Arc::new(StreamingBlobInner::new(digest, max_buffer_bytes));
        self.map
            .write()
            .insert(digest, Arc::clone(&inner));
        let writer = StreamingBlobWriter::new(Arc::clone(&inner));
        let reader = StreamingBlobReader::new(inner);
        (writer, reader)
    }

    /// Get a reader for an in-flight blob, if one exists.
    pub fn get_reader(&self, digest: &DigestInfo) -> Option<StreamingBlobReader> {
        let map = self.map.read();
        map.get(digest)
            .map(|inner| StreamingBlobReader::new(Arc::clone(inner)))
    }

    /// Remove a blob from the map, but only if the stored `Arc`
    /// points to the same allocation as `expected`.  This prevents
    /// removing a newer registration for the same digest.
    pub fn remove(&self, digest: &DigestInfo, expected: &Arc<StreamingBlobInner>) {
        let mut map = self.map.write();
        if let Some(existing) = map.get(digest) {
            if Arc::ptr_eq(existing, expected) {
                map.remove(digest);
            }
        }
    }

    /// Number of in-flight blobs currently registered.
    pub fn len(&self) -> usize {
        self.map.read().len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.map.read().is_empty()
    }
}

impl Default for InFlightBlobMap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use nativelink_error::Code;

    use super::*;

    /// Helper: create a DigestInfo from a u8 seed (for test variety).
    fn test_digest(seed: u8) -> DigestInfo {
        let mut hash = [0u8; 32];
        hash[0] = seed;
        DigestInfo::new(hash, 1024)
    }

    // ---------------------------------------------------------------
    // 1. Single writer, single reader — data flows correctly
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn single_writer_single_reader() {
        let (writer, mut reader) = StreamingBlob::new(test_digest(1), 1024 * 1024);

        let data1 = Bytes::from_static(b"hello ");
        let data2 = Bytes::from_static(b"world");

        writer.send(data1.clone()).await.unwrap();
        writer.send(data2.clone()).await.unwrap();

        let chunk1 = reader.next_chunk().await.unwrap();
        assert_eq!(chunk1, data1);

        let chunk2 = reader.next_chunk().await.unwrap();
        assert_eq!(chunk2, data2);

        // Writer hasn't sent EOF yet, so a read should block.
        // We send EOF from a background task to unblock.
        let writer = Arc::new(Mutex::new(writer));
        let w = Arc::clone(&writer);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            w.lock().send_eof().unwrap();
        });

        let eof_chunk = reader.next_chunk().await.unwrap();
        assert!(eof_chunk.is_empty(), "expected empty bytes for EOF");
    }

    // ---------------------------------------------------------------
    // 2. Single writer, multiple readers — all see same data
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn multiple_readers_see_same_data() {
        let (mut writer, mut reader1) = StreamingBlob::new(test_digest(2), 1024 * 1024);

        // Create a second reader from the inner.
        let inner = Arc::clone(&reader1.inner);
        let mut reader2 = StreamingBlob::new_reader(&inner);

        let chunks: Vec<Bytes> = (0..5)
            .map(|i| Bytes::from(format!("chunk-{i}")))
            .collect();

        for c in &chunks {
            writer.send(c.clone()).await.unwrap();
        }
        writer.send_eof().unwrap();

        // Both readers should see all chunks in order.
        for expected in &chunks {
            let r1 = reader1.next_chunk().await.unwrap();
            let r2 = reader2.next_chunk().await.unwrap();
            assert_eq!(&r1, expected);
            assert_eq!(&r2, expected);
        }

        // Both should get EOF.
        assert!(reader1.next_chunk().await.unwrap().is_empty());
        assert!(reader2.next_chunk().await.unwrap().is_empty());
    }

    // ---------------------------------------------------------------
    // 3. Writer error propagates to all readers
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn writer_error_propagates() {
        let (mut writer, mut reader) = StreamingBlob::new(test_digest(3), 1024 * 1024);

        let inner = Arc::clone(&reader.inner);
        let mut reader2 = StreamingBlob::new_reader(&inner);

        writer.send(Bytes::from_static(b"data")).await.unwrap();
        writer.send_error(make_err!(Code::DataLoss, "hash mismatch"));

        // First chunk is still readable.
        let c = reader.next_chunk().await.unwrap();
        assert_eq!(c, Bytes::from_static(b"data"));
        let c2 = reader2.next_chunk().await.unwrap();
        assert_eq!(c2, Bytes::from_static(b"data"));

        // Next read returns the error.
        let err = reader.next_chunk().await.unwrap_err();
        assert_eq!(err.code, Code::DataLoss);

        let err2 = reader2.next_chunk().await.unwrap_err();
        assert_eq!(err2.code, Code::DataLoss);
    }

    // ---------------------------------------------------------------
    // 4. Writer drop without EOF gives readers an error
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn writer_drop_without_eof() {
        let (writer, mut reader) = StreamingBlob::new(test_digest(4), 1024 * 1024);

        writer.send(Bytes::from_static(b"partial")).await.unwrap();
        drop(writer);

        let c = reader.next_chunk().await.unwrap();
        assert_eq!(c, Bytes::from_static(b"partial"));

        let err = reader.next_chunk().await.unwrap_err();
        assert_eq!(err.code, Code::Internal);
        assert!(
            err.messages.iter().any(|m| m.contains("dropped without")),
            "expected 'dropped without' in error messages, got: {:?}",
            err.messages
        );
    }

    // ---------------------------------------------------------------
    // 5. Sliding window eviction — slow reader gets Unavailable
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn sliding_window_eviction() {
        // Buffer limited to 20 bytes.
        let (writer, mut slow_reader) = StreamingBlob::new(test_digest(5), 20);

        // Write 30 bytes in 3 chunks of 10.  The first chunk will
        // be evicted once the third is appended.
        for i in 0..3u8 {
            let data = Bytes::from(vec![i; 10]);
            writer.send(data).await.unwrap();
        }

        // The writer evicts chunks when the buffer exceeds 20 bytes,
        // so after 30 bytes the oldest chunk(s) are gone.
        let earliest = slow_reader
            .inner
            .earliest_chunk_idx
            .load(Ordering::Acquire);
        assert!(
            earliest > 0,
            "expected some eviction, earliest_chunk_idx={earliest}"
        );

        // Slow reader's cursor is at 0, which is < earliest.
        let err = slow_reader.next_chunk().await.unwrap_err();
        assert_eq!(err.code, Code::Unavailable);

        // Create a new reader after eviction — it starts at
        // earliest_chunk_idx and should be able to read.
        let inner = Arc::clone(&slow_reader.inner);
        let mut late_reader = StreamingBlob::new_reader(&inner);
        let chunk = late_reader.next_chunk().await.unwrap();
        assert_eq!(chunk.len(), 10);

        let mut writer = writer;
        writer.send_eof().unwrap();
    }

    // ---------------------------------------------------------------
    // 6. Reader waits for data (does not return None prematurely)
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn reader_waits_for_data() {
        let (writer, mut reader) = StreamingBlob::new(test_digest(6), 1024 * 1024);

        let writer = Arc::new(Mutex::new(Some(writer)));
        let w = Arc::clone(&writer);

        // Spawn a task that writes after a delay.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let w_guard = w.lock();
            let w_ref = w_guard.as_ref().unwrap();
            w_ref.send(Bytes::from_static(b"delayed")).await.unwrap();
        });

        // Reader should block until data arrives, then return it.
        let start = std::time::Instant::now();
        let chunk = reader.next_chunk().await.unwrap();
        let elapsed = start.elapsed();

        assert_eq!(chunk, Bytes::from_static(b"delayed"));
        assert!(
            elapsed >= std::time::Duration::from_millis(20),
            "reader returned too quickly ({elapsed:?}), should have waited"
        );

        // Clean up.
        let mut w_guard = writer.lock();
        w_guard.take().unwrap().send_eof().unwrap();
    }

    // ---------------------------------------------------------------
    // 7. EOF only after terminal-success
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn eof_only_after_terminal_success() {
        let (mut writer, mut reader) = StreamingBlob::new(test_digest(7), 1024 * 1024);

        writer.send(Bytes::from_static(b"a")).await.unwrap();
        writer.send(Bytes::from_static(b"b")).await.unwrap();

        // Read both chunks.
        assert_eq!(reader.next_chunk().await.unwrap(), Bytes::from_static(b"a"));
        assert_eq!(reader.next_chunk().await.unwrap(), Bytes::from_static(b"b"));

        // Send EOF.
        writer.send_eof().unwrap();

        // Now reader gets empty Bytes (EOF).
        let eof = reader.next_chunk().await.unwrap();
        assert!(eof.is_empty());

        // Subsequent reads also return EOF.
        let eof2 = reader.next_chunk().await.unwrap();
        assert!(eof2.is_empty());
    }

    // ---------------------------------------------------------------
    // 8. InFlightBlobMap register / get / remove with Arc pointer check
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn in_flight_blob_map_basic() {
        let map = InFlightBlobMap::new();
        let digest = test_digest(8);

        // Register a blob.
        let (mut writer, mut reader1) = map.register(digest, 1024 * 1024);
        assert_eq!(map.len(), 1);

        // Get a reader for the same digest.
        let mut reader2 = map.get_reader(&digest).expect("blob should be in map");

        // Write and verify both readers work.
        writer.send(Bytes::from_static(b"map-data")).await.unwrap();
        writer.send_eof().unwrap();

        assert_eq!(
            reader1.next_chunk().await.unwrap(),
            Bytes::from_static(b"map-data")
        );
        assert_eq!(
            reader2.next_chunk().await.unwrap(),
            Bytes::from_static(b"map-data")
        );

        // Remove with wrong Arc pointer — should not remove.
        let other_inner = Arc::new(StreamingBlobInner::new(digest, 1024));
        map.remove(&digest, &other_inner);
        assert_eq!(map.len(), 1, "remove with wrong Arc should be a no-op");

        // Remove with correct Arc pointer.
        let correct_inner = Arc::clone(&reader1.inner);
        map.remove(&digest, &correct_inner);
        assert_eq!(map.len(), 0);
        assert!(map.get_reader(&digest).is_none());
    }

    // ---------------------------------------------------------------
    // 9. Cannot send after EOF
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn send_after_eof_fails() {
        let (mut writer, _reader) = StreamingBlob::new(test_digest(9), 1024 * 1024);

        writer.send_eof().unwrap();
        let err = writer.send(Bytes::from_static(b"too late")).await.unwrap_err();
        assert_eq!(err.code, Code::Internal);
    }

    // ---------------------------------------------------------------
    // 10. Double EOF fails
    // ---------------------------------------------------------------
    #[tokio::test]
    async fn double_eof_fails() {
        let (mut writer, _reader) = StreamingBlob::new(test_digest(10), 1024 * 1024);

        writer.send_eof().unwrap();
        let err = writer.send_eof().unwrap_err();
        assert_eq!(err.code, Code::Internal);
    }
}
