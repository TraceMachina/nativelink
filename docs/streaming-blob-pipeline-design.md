# Streaming Blob Pipeline Design

## 1. Problem Statement

NativeLink's store chain is fully sequential: data must be completely received and written before any reader can access it. In the current server store chain (WorkerProxyStore -> VerifyStore -> ExistenceCache -> SizePartitioning -> MemoryStore/FilesystemStore), every layer collects the full blob before passing to the next. This means:

- A worker needing a blob during input materialization must wait for the full upload + verify + insert before the server's `get_part` can serve it.
- Mirror writes to workers must wait for the full blob to be received before starting the mirror stream.
- Worker-to-worker P2P sharing requires the source peer to have fully read a blob from its CAS before forwarding chunks.
- Server proxy reads from workers (via WorkerProxyStore) buffer the entire blob before streaming to the requesting client.

**Estimated improvement:** For read-while-write (Requirement 1), readers can begin consuming data as soon as the first chunk is appended to the `StreamingBlob`, eliminating the full-upload wait. For a typical blob, this saves approximately **~50ms per blob** (the time between first chunk received and store commit), which compounds across the hundreds of blobs in a typical input tree materialization.

## 2. Core Abstraction: `StreamingBlob`

The central data structure is a **shared, append-only byte buffer with multiple concurrent readers and a single writer**, conceptually similar to a `tokio::sync::broadcast` channel but designed for byte streams rather than discrete messages.

**Key properties:**

- **Single writer, multiple readers.** The writer appends `Bytes` chunks. Each reader maintains its own independent cursor position.
- **Readers at different speeds.** A fast reader can be at chunk N+5 while a slow reader is at chunk N. Readers never block the writer or each other.
- **Bounded memory via a sliding window.** The buffer retains only a configurable window of the most recent chunks (e.g., 32 MiB). Chunks behind the slowest reader are eligible for eviction. Readers that fall behind the window receive an error (`Code::Unavailable`, retryable) rather than blocking the writer.
- **Terminal state (success or error).** The writer either sends EOF (success) or drops/errors. All readers observe the same terminal state. No reader ever sees partial data followed by silence.
- **Post-EOF materialization.** After successful EOF + hash verification, the blob becomes a normal committed entry in the store. The `StreamingBlob` handle can be discarded once committed.

**Data structure sketch:**

```
StreamingBlob {
    inner: Arc<StreamingBlobInner>,
}

StreamingBlobInner {
    // Append-only chunk deque. Protected by RwLock: the writer takes
    // a write lock to append/evict, readers take a shared read lock
    // to index into the deque. VecDeque gives O(1) front eviction
    // when the sliding window advances.
    chunks: RwLock<VecDeque<Bytes>>,
    // Monotonically increasing count of chunks appended.
    chunk_count: AtomicU64,
    // Total bytes written so far.
    bytes_written: AtomicU64,
    // Wakes readers when new data or terminal state is available.
    notify: Notify,
    // Terminal state: None = still writing, Some(Ok(())) = EOF,
    // Some(Err(..)) = writer error.
    terminal: Mutex<Option<Result<(), Error>>>,
    // Digest for this blob (for verification and keying).
    digest: DigestInfo,
    // Configuration
    max_buffer_bytes: u64,
    // Offset of the earliest retained chunk (for sliding window).
    // Chunks before this index have been dropped.
    earliest_chunk_idx: AtomicU64,
}
```

**Reader handle:**

```
StreamingBlobReader {
    inner: Arc<StreamingBlobInner>,
    cursor_chunk_idx: u64,
    cursor_byte_offset: u64,  // offset within current chunk
}
```

A reader calls `async fn next_chunk(&mut self) -> Result<Option<Bytes>, Error>` which either returns immediately if data is available at its cursor, or waits on `notify`. It returns `Ok(Some(chunk))` when data is available, `Ok(None)` only on terminal-success (EOF after hash verification), and `Err(..)` on terminal-error. **Critically, when chunks are exhausted but no terminal state has been set, `next_chunk()` must return `Poll::Pending` (i.e., the future does not resolve), NOT `Ok(None)`.** Returning `None` prematurely would cause the reader to interpret the partial data as a successful completion (the "silent-success" bug). If the reader's cursor is behind `earliest_chunk_idx`, it returns `Code::Unavailable`.

**Why not `tokio::sync::broadcast`?** Broadcast channels are message-oriented and drop messages for slow receivers (or require unbounded capacity). We need byte-level offset tracking, a sliding window with explicit memory bounds, and the ability for readers to start at arbitrary offsets (not just the live head).

**Why not existing `buf_channel`?** The existing `DropCloserWriteHalf`/`DropCloserReadHalf` in `/path/to/nativelink/nativelink-util/src/buf_channel.rs` is a 1:1 channel (single producer, single consumer). It uses `mpsc::channel` internally. The streaming blob needs 1:N fan-out. The existing buf_channel should remain as-is for point-to-point streaming; `StreamingBlob` is a new primitive for the concurrent-read case.

## 3. Registration Layer: `InFlightBlobMap`

The streaming blobs must be discoverable. A new `InFlightBlobMap` maps `DigestInfo -> Arc<StreamingBlobInner>` at the server level (inside `InstanceInfo` in bytestream_server, analogous to the existing `in_flight_writes` map).

```
InFlightBlobMap {
    map: DashMap<DigestInfo, Arc<StreamingBlobInner>>,
}
```

- **On write start:** The `bytestream_write` method registers a `StreamingBlob` in the map before the first chunk is written to the store chain. This replaces/extends the existing `in_flight_writes: HashMap<DigestInfo, watch::Receiver<Option<bool>>>`.
- **On write complete (success):** The blob is committed to the store. The `StreamingBlob` transitions to terminal-success. The map entry is removed after a short grace period (e.g., 5 seconds) to let in-progress readers finish.
- **On write error:** The `StreamingBlob` transitions to terminal-error. Readers get the error. The map entry is removed immediately.
- **On read request:** The `inner_read` / `get_part` path first checks `InFlightBlobMap`. If a `StreamingBlob` exists for the digest, it creates a `StreamingBlobReader` and streams from it, without touching the store chain at all.

## 4. How Each Requirement Maps

### Requirement 1: Server Stream-through CAS Writes

**Current flow:**
```
Bazel -> ByteStream Write -> VerifyStore -> ExistenceCache -> SizePartition -> MemoryStore
                                                                             (collect all, then insert)
Worker -> ByteStream Read -> store.get_part() -> NotFound (blob not committed yet)
```

**New flow:**
```
Bazel -> ByteStream Write -> register in InFlightBlobMap
                          -> VerifyStore -> ... (unchanged store chain)
                          -> each chunk also appended to StreamingBlob

Worker -> ByteStream Read -> check InFlightBlobMap -> found!
                          -> create StreamingBlobReader -> stream chunks as they arrive
                          -> when StreamingBlob reaches terminal-success, read completes
                          -> if terminal-error, reader gets error
```

**Integration point:** `bytestream_server.rs` line ~1415 where `in_flight_writes` is checked. Extend this to register a `StreamingBlob`. The `inner_read` method (line ~801) checks the `InFlightBlobMap` before calling `store.get_part()`.

**Hash verification:** The `StreamingBlob` writer is the `process_client_stream` function inside `inner_write`. The `VerifyStore` still verifies the hash at EOF. The `StreamingBlob` does NOT transition to terminal-success until `VerifyStore` passes. This means readers streaming from the `StreamingBlob` see data chunks in real time but the final EOF is delayed until verification completes. If verification fails, readers get an error.

Implementation detail: The `StreamingBlob`'s writer is fed chunks by tapping into the same data flow that feeds the `DropCloserWriteHalf tx` in `create_or_join_upload_stream`. Each chunk sent to `tx` is also appended to the `StreamingBlob`. The `store_update_fut` completing successfully signals that the blob is committed, which triggers the `StreamingBlob`'s terminal-success.

### Requirement 2: Server Stream Mirror to Workers

**Current flow (streaming path in `inner_write`, line ~1031):**
```
Bazel chunk -> clone to mirror_tx (buf_channel) -> background task -> WorkerProxyStore.mirror_blob_via_stream()
```

This already streams chunks to the mirror channel as they arrive. The mirror channel has a small buffer (16 slots) and a 100ms send timeout. This is already close to streaming.

**Gap:** The mirror `buf_channel` is 1:1 (one mirror target). For multi-worker mirroring, each additional worker would need another tee. More importantly, the mirror setup at line ~1112 creates the channel at write start, but if the mirror task is slow, chunks are dropped (timeout at line 1044).

**New flow with StreamingBlob:** Instead of a dedicated mirror tee channel, the mirror background task creates a `StreamingBlobReader` from the same `StreamingBlob` registered in Requirement 1. It reads at its own pace and streams to the worker via `GrpcStore.update()`. If it falls behind the sliding window, it gets an error and the mirror is abandoned (non-fatal, same as today's timeout behavior). Multiple mirror targets can each have their own reader.

**Integration point:** Replace the mirror tee in `inner_write` (lines 1099-1145) with a reader from the `StreamingBlob`. The `mirror_blob_via_stream` method in `WorkerProxyStore` (line 776) stays the same -- it receives a `DropCloserReadHalf` -- but the source of that read half changes from a dedicated tee channel to a `StreamingBlobReader` adapted into a `DropCloserReadHalf` (via a thin adapter that implements the same recv/EOF protocol).

### Requirement 3: Worker P2P Streaming

**Current flow (WorkerProxyStore.get_part_and_cache, line 368):**
```
Worker A requests blob from Worker B (peer)
-> Worker B's get_part reads from FilesystemStore (entire blob)
-> streams to Worker A via gRPC
-> Worker A tees to inner store (cache) and to requester
```

Worker B must fully read the blob from disk/memory before the first gRPC ReadResponse goes to Worker A. This is inherent to the FilesystemStore read path, not a buffering issue -- `fs::read_file_to_channel` does stream in chunks.

**Actual gap:** When Worker B is itself still receiving a blob (e.g., from the server mirror), Worker A requesting that same blob must wait for Worker B's write to complete. Worker B's `FastSlowStore.get_part` checks `in_flight_slow_writes` (line 1277) which serves the blob from the buffered `Vec<Bytes>`, but only after the writer collected all data.

**New flow:** Worker B registers a local `StreamingBlob` in its own `InFlightBlobMap` when it starts receiving a blob (via server mirror or its own build output). Worker A's request, proxied through gRPC to Worker B's ByteStream Read, hits Worker B's `InFlightBlobMap` and gets a `StreamingBlobReader`, streaming chunks as Worker B receives them.

**Integration point:** `FastSlowStore.update()` (line 613) is where mirror blobs arrive on workers. Instead of collecting into `mirror_blobs: HashMap<DigestInfo, (Bytes, Instant)>`, register a `StreamingBlob`. The `get_part` path (line 1189) checks the `InFlightBlobMap` before checking `mirror_blobs`, `in_flight_slow_writes`, or the fast store.

### Requirement 4: Server Stream Proxy from Peers

**Current flow (WorkerProxyStore.get_part_sequential, line 512):**
```
Client -> Server ByteStream Read -> WorkerProxyStore.get_part
       -> inner store miss (NotFound)
       -> consult locality map -> found on Worker C
       -> GrpcStore.get_part to Worker C -> stream all data
       -> tee to inner store + forward to client
```

The `get_part_and_cache` method (line 368) already streams this way -- it creates an intermediate buf_channel, tees to cache and to the caller's writer. This is already streaming.

**Gap:** The tee in `get_part_and_cache` creates point-to-point channels. If two clients request the same blob simultaneously, each triggers a separate fetch from the peer worker. There is no dedup or sharing.

**New flow:** When the first client triggers a proxy fetch, register a `StreamingBlob` in the server's `InFlightBlobMap`. The proxy fetch from the worker writes chunks into the `StreamingBlob`. The first client and any subsequent clients all create `StreamingBlobReader`s from the same `StreamingBlob`. The `get_part_and_cache` tee to inner store is also a reader.

**Integration point:** `WorkerProxyStore.get_part_sequential` (line 512) and `get_part_and_cache` (line 368). Before initiating a peer fetch, check `InFlightBlobMap`. If found, create a reader. If not found, register a `StreamingBlob`, start the peer fetch, and create a reader.

## 5. Error Propagation

The error model follows directly from the `StreamingBlobInner.terminal` field:

1. **Writer sends error or drops:** `terminal` is set to `Some(Err(..))`. `notify.notify_waiters()` wakes all readers. Each reader, on its next `next_chunk()` call, sees the terminal error and returns it.

2. **Writer drops without EOF:** The `Drop` impl for the writer handle sets `terminal` to `Some(Err(make_err!(Code::Internal, "Writer dropped without sending EOF")))`.

3. **VerifyStore hash mismatch:** The store update future returns an error. The `StreamingBlob` writer observes this (since `process_client_stream` and `store_update_fut` are joined in `try_join!`) and propagates to the terminal state. All readers get the hash mismatch error.

4. **Reader observes error:** The reader returns the error to its caller (the gRPC stream, the mirror task, etc.). The reader drops. If all readers drop, the `StreamingBlobInner` arc count may go to 1 (just the map entry), which is fine -- the map entry is cleaned up on write completion or error.

5. **Partial data guarantee:** A reader never receives an implicit "success" with partial data. The protocol is: data chunks arrive, then either EOF (explicit success from the writer after hash verification) or error. The `send_eof` on the adapted `DropCloserWriteHalf` is only called after the `StreamingBlob` reaches terminal-success.

## 6. Memory Management

**Sliding window eviction:** The `StreamingBlobInner` maintains `earliest_chunk_idx`. When `bytes_written` exceeds `max_buffer_bytes`, the oldest chunks are dropped and `earliest_chunk_idx` advances. A reader whose `cursor_chunk_idx < earliest_chunk_idx` receives `Code::Unavailable` (retryable -- the reader can fall back to reading from the committed store once the write completes).

**Configurable per use case:**
- Server-side `InFlightBlobMap`: `max_buffer_bytes = 64 MiB` per blob. At 10 Gbps, a 64 MiB buffer provides ~50ms of buffering, sufficient for readers that are only slightly slower than the writer.
- Worker-side `InFlightBlobMap`: `max_buffer_bytes = 32 MiB` per blob. Workers have less RAM.
- Global cap: The `InFlightBlobMap` enforces a total memory cap across all active streaming blobs (e.g., 2 GiB, matching the existing `MIRROR_BLOBS_MAX_BYTES`). When the cap is exceeded, new streaming blobs fall back to the non-streaming path.

**Chunk lifetime:** Each chunk is a `Bytes` (reference-counted). When a chunk is evicted from the sliding window but a reader still holds a reference, the reader's `Bytes` clone keeps the data alive until the reader processes it. This means actual memory usage can temporarily exceed `max_buffer_bytes` by at most `(num_readers * chunk_size)`, which is bounded.

**Comparison with existing patterns:** The current `in_flight_slow_writes` in `FastSlowStore` (line 808) holds `Vec<Bytes>` for the entire blob with no eviction -- unbounded memory. The `mirror_blobs` map (line 97) has a 2 GiB cap but holds complete blobs. The `StreamingBlob` approach is strictly better: it bounds per-blob memory and allows serving data incrementally.

## 7. Integration Points (Detailed)

### File: `nativelink-util/src/buf_channel.rs`
- No changes. The existing 1:1 channel remains for point-to-point streaming within the store chain.

### New file: `nativelink-util/src/streaming_blob.rs`
- `StreamingBlob`, `StreamingBlobWriter`, `StreamingBlobReader`, `InFlightBlobMap`.
- Adapter: `StreamingBlobReader -> DropCloserReadHalf` so readers integrate with existing store APIs.

### File: `nativelink-service/src/bytestream_server.rs`
- `InstanceInfo`: Replace `in_flight_writes: HashMap<DigestInfo, watch::Receiver<Option<bool>>>` with `InFlightBlobMap` (or keep both during transition).
- `bytestream_write` (line 1361): On write start, register `StreamingBlob` in `InFlightBlobMap`. Modify `process_client_stream` to append each chunk to the `StreamingBlob` in addition to the `tx` channel.
- `inner_read` (line 801): Before calling `store.get_part()`, check `InFlightBlobMap`. If found, create reader and stream.
- `inner_write` (lines 1099-1145): Replace mirror tee with `StreamingBlobReader`.

### File: `nativelink-store/src/fast_slow_store.rs`
- `update` (line 613, mirror path): Register `StreamingBlob` on mirror write start. Replace `mirror_blobs: HashMap<DigestInfo, (Bytes, Instant)>` with `InFlightBlobMap`.
- `get_part` (line 1189): Check `InFlightBlobMap` before checking `mirror_blobs` and `in_flight_slow_writes`. Eventually deprecate both in favor of `StreamingBlob`.
- `update` (line 706, normal path): Register `StreamingBlob` when accumulating chunks for background slow write. Replace `in_flight_slow_writes: HashMap<StoreKey, Vec<Bytes>>` with `InFlightBlobMap`.

### File: `nativelink-store/src/worker_proxy_store.rs`
- `get_part_sequential` / `get_part_and_cache`: Check `InFlightBlobMap` before initiating peer fetch. Register `StreamingBlob` when starting a proxy fetch.
- `mirror_blob_via_stream` (line 776): Accept `StreamingBlobReader` adapted to `DropCloserReadHalf`.

### File: `nativelink-store/src/memory_store.rs`
- No changes needed initially. The `StreamingBlob` operates above the store layer. The MemoryStore still receives the complete blob via its `update` method (the buf_channel between VerifyStore and MemoryStore still works as before). The streaming benefit comes from readers being able to read from the `StreamingBlob` before the store `update` completes.

## 8. Migration Strategy (Incremental)

**Phase 1: Core primitive (`streaming_blob.rs`)**
- Implement `StreamingBlob`, `StreamingBlobWriter`, `StreamingBlobReader`, `InFlightBlobMap`.
- Implement `StreamingBlobReader -> DropCloserReadHalf` adapter.
- Unit tests with concurrent readers, error propagation, sliding window eviction.
- No changes to any existing code paths.

**Phase 2: Server read-while-writing (Requirement 1)**
- Add `InFlightBlobMap` to `InstanceInfo` in `bytestream_server.rs`.
- Modify `bytestream_write` to register `StreamingBlob` and append chunks.
- Modify `inner_read` to check `InFlightBlobMap` first.
- This is the highest-impact change: workers can start materializing inputs before the upload completes.
- Feature flag: `streaming_read_while_write: bool` in config, defaulting to `false`.

**Phase 3: Server mirror streaming (Requirement 2)**
- Replace mirror tee in `inner_write` with `StreamingBlobReader`.
- This is relatively low risk because mirror errors are already non-fatal.

**Phase 4: Worker-side `InFlightBlobMap` (Requirement 3)**
- Add `InFlightBlobMap` to `FastSlowStore` or to the worker's `ByteStreamServer` instance.
- Replace `mirror_blobs` and `in_flight_slow_writes` with `StreamingBlob` entries.
- Worker P2P: peers that are still receiving a blob can serve it.

**Phase 5: Server proxy dedup (Requirement 4)**
- Modify `WorkerProxyStore.get_part_*` to register `StreamingBlob` for proxy fetches.
- Multiple clients requesting the same blob share a single peer fetch.

## 9. Key Design Decisions and Trade-offs

**Decision: StreamingBlob lives above the store trait, not inside it.**
The `StoreDriver` trait's `update` and `get_part` signatures take `DropCloserReadHalf` / `DropCloserWriteHalf` -- 1:1 channels. Adding multi-reader support inside the store trait would require changing every store implementation. Instead, the `StreamingBlob` sits at the service layer (bytestream_server) and hooks into the data flow before it enters the store chain. Stores remain unchanged.

**Decision: Sliding window, not unbounded buffer.**
An unbounded buffer is simpler but risks OOM under load (many concurrent large uploads). The sliding window bounds memory but introduces the possibility that slow readers get `Unavailable` errors. This is acceptable because: (a) the reader can retry from the committed store after the write completes, (b) the window size is tunable, (c) the existing system doesn't serve these readers at all, so any streaming is an improvement.

**Decision: Hash verification gates the terminal-success, not the data flow.**
Readers see data chunks before the hash is verified. This is safe because: (a) if the hash fails, readers get an error and must discard the data, (b) the store chain does not commit the blob until verification passes.

**HARD REQUIREMENT: Workers MUST wait for terminal-success before materializing inputs.** A worker receiving chunks via a `StreamingBlobReader` may buffer them locally (e.g., write to a temp file), but it MUST NOT make the data available for action execution until the `StreamingBlob` reaches terminal-success (EOF after hash verification). This prevents workers from executing actions against corrupt or incomplete data. The streaming benefit comes from overlapping the network transfer with the local write, not from using unverified data. This is not an optional mitigation — it is a correctness invariant.

**Decision: Adapting to `DropCloserReadHalf` rather than changing all call sites.**
The adapter converts `StreamingBlobReader` into the `recv() -> Bytes` / EOF protocol expected by existing code. This avoids changing `StoreDriver`, `GrpcStore`, etc. The adapter is lightweight: it calls `next_chunk()` on recv and sends `Bytes::new()` for EOF.

## 10. Critical Files for Implementation

- `nativelink-util/src/buf_channel.rs`
- `nativelink-service/src/bytestream_server.rs`
- `nativelink-store/src/fast_slow_store.rs`
- `nativelink-store/src/worker_proxy_store.rs`
- `nativelink-store/src/memory_store.rs`

## 11. Open Issues

Issues identified during review that need resolution before or during implementation.

### Correctness

1. **Reader sees unverified data.** Read-while-write readers act on data chunks before hash verification completes. If the hash check ultimately fails, those readers have already consumed and potentially acted on corrupt data. **Resolution:** Per the hard requirement in Section 9, workers MUST wait for terminal-success before materializing inputs for execution. Workers may buffer data locally (e.g., write to a temp file) while streaming, but MUST NOT make it available for action execution until terminal-success. Non-worker readers (e.g., mirror streams) accept the risk since verification failures are rare and they will receive the error.

2. **Sliding window vs. store commit race.** A reader that falls behind the sliding window receives `Code::Unavailable` and is expected to retry from the committed store. However, the store may not have committed the blob yet (the write is still in progress). The reader would get `NotFound` on retry. Mitigation: the `Unavailable` error returned to the reader must carry the `StreamingBlob` handle (or a commit notification future derived from it). The reader's retry logic then: (a) awaits the commit notification (terminal-success on the handle), (b) once the blob is confirmed committed, retries `get_part` from the store which now has the data. This avoids blind exponential backoff and guarantees the retry succeeds on the first attempt after commit. The `InFlightBlobMap` lookup is not needed on retry because the reader already holds the handle.

3. **No validation that resumed writer matches original client.** ByteStream Write supports resumable uploads via `write_offset`. If a different client resumes an upload registered with a `StreamingBlob`, there is no verification that the resumed data is consistent with what was already streamed to readers. Mitigation: track the client identity (e.g., resource name UUID) and reject mismatched resumes, or invalidate the `StreamingBlob` on resume and force readers to restart.

4. **Mirror data loss on resumed writes.** If a write is resumed, only the new portion (from `write_offset` onward) is appended to the `StreamingBlob`. Mirror readers that started from the beginning have already received the pre-resume data, but the resumed writer may be sending from a different offset. The mirror stream would have a gap or duplicate data. Mitigation: on resume, invalidate the existing `StreamingBlob` and create a new one, or track the resume offset and only allow new readers to start from that point.

5. **Adapter must explicitly propagate terminal-error.** The `StreamingBlobReader -> DropCloserReadHalf` adapter must not simply drop on terminal-error. If the underlying `StreamingBlob` transitions to terminal-error, the adapter MUST propagate that error through the `DropCloserReadHalf` (e.g., via `send_err()` or by returning the error from `recv()`). Silently dropping the adapter would cause the downstream consumer to see an unexpected EOF, which it may interpret as success. The adapter's `Drop` impl should check for terminal-error and propagate if the read half is still connected.

6. **DashMap grace-period removal must compare Arc pointers.** When a `StreamingBlob` reaches terminal-success, the `InFlightBlobMap` entry is removed after a grace period. However, during the grace period, a new write for the same digest may register a new `StreamingBlob`. The grace-period removal must compare `Arc::ptr_eq` on the `StreamingBlobInner`, not just match the digest key, to avoid removing the new entry. Without this, a stale grace-period timer could delete a live `StreamingBlob` for a subsequent upload of the same digest.

### Resource Management

7. **Memory accounting race with sweeper.** `partial_write_bytes` (or equivalent budget tracking) increments are not atomic with respect to the sweeper's eviction decisions. A burst of new `StreamingBlob` registrations could exceed the global memory cap before the sweeper runs. Mitigation: budget checks MUST be inline at `StreamingBlob` registration time, not deferred to the sweeper. Use `fetch_add` on an atomic counter at registration and check the cap synchronously, rejecting new streaming blobs with `Code::ResourceExhausted` when the budget is exhausted. The sweeper handles cleanup of abandoned entries but is NOT the primary budget enforcement mechanism. Additionally, the per-chunk append path should check `bytes_written` against `max_buffer_bytes` inline (not just in the sweeper) to enforce the per-blob sliding window bound synchronously.

8. **Store update future leak on idle stream eviction.** If an idle `StreamingBlob` is evicted from the `InFlightBlobMap` (e.g., by the sweeper or timeout), the background `store_update_fut` may still be running and holding resources (file handles, gRPC streams, buf_channel slots). The eviction must cancel the store update future or the future must observe that its `StreamingBlob` has been evicted and abort. Mitigation: use a `CancellationToken` or `AbortHandle` associated with each `StreamingBlob` that is triggered on eviction.

### Performance

9. **RwLock + VecDeque for chunks (resolved).** The data structure sketch in Section 2 uses `RwLock<VecDeque<Bytes>>` instead of `Mutex<Vec<Bytes>>`. Readers take a shared read lock to index into the deque, so concurrent readers never block each other. The writer takes a write lock only to append or evict. `VecDeque` provides O(1) `pop_front` for sliding window eviction (vs O(n) `Vec::remove(0)`). For extremely hot blobs (10+ concurrent readers), a lock-free segmented list could further reduce contention, but `RwLock<VecDeque>` is sufficient for the initial implementation.

10. **Per-blob overhead at scale.** Each `StreamingBlobInner` contains atomics, an RwLock, a Notify, and a VecDeque. At 20K concurrent in-flight blobs (plausible during large build uploads), this is non-trivial overhead. Mitigation: (a) pool or arena-allocate `StreamingBlobInner` instances, (b) use a more compact representation for small blobs (e.g., inline the single chunk case), (c) set a hard cap on concurrent streaming blobs and fall back to non-streaming for overflow.

11. **Bytes ref-counting delays chunk memory reclamation.** When a chunk is evicted from the sliding window, its memory is not freed until all readers that hold a `Bytes` clone drop their references. Under high fan-out (many readers) with large chunks, this can significantly delay memory reclamation. Mitigation: (a) document that actual memory usage = `max_buffer_bytes + (num_readers * chunk_size)`, (b) use smaller chunk sizes to reduce per-reader overhead, (c) consider `Bytes::slice()` to share the underlying allocation with more granular lifetime.

12. **Budget check should be inline, not only in sweeper (resolved).** Per item 7, budget checks are performed inline at `StreamingBlob` registration time and at each chunk append. The sweeper handles cleanup of abandoned entries only. This item is resolved by the mitigation in item 7.

13. **Cap `max_partial_uploads` from day one.** There should be a hard limit on the number of concurrent `StreamingBlob` entries in the `InFlightBlobMap` from the initial implementation. Without this, a misbehaving client (or a burst of uploads) can create unbounded entries. Mitigation: add a `max_concurrent_streaming_blobs` config parameter with a conservative default (e.g., 1000) and reject new streaming blob registrations with `Code::ResourceExhausted` when the limit is reached.
