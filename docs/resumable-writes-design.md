# Resumable ByteStream Writes

## Problem

When a `ByteStream.Write` RPC is aborted mid-stream (client disconnect, network
timeout, transient error), NativeLink retains the partial upload state in an
`IdleStream` entry inside the `active_uploads` map for a configurable duration
(`persist_stream_on_disconnect_timeout`, default 60s). If the same client
reconnects using the same upload UUID and a `write_offset` that matches
`bytes_received`, the write resumes from where it left off.

However, the current implementation has several gaps relative to the full REAPI
ByteStream specification:

1. **Partial data lives only in the buf_channel pipeline.** Once the
   `DropCloserWriteHalf` has sent bytes into the channel, those bytes flow into
   the store's `update()` future. If the store is a `FilesystemStore`, the data
   is buffered in a temporary file; if it is a `MemoryStore`, the data is held
   in memory. There is no unified "partial write buffer" that the server
   controls independently of the store.

2. **The idle stream sweeper is time-based only.** There is no memory-pressure
   eviction: a burst of abandoned partial uploads can accumulate significant
   memory (each partial upload holds an open buf_channel, a store update future,
   and the bytes already flushed into the store pipeline).

3. **`QueryWriteStatus` returns `committed_size` from an `AtomicU64` counter**
   that tracks bytes written to the `DropCloserWriteHalf`. This is accurate for
   resumption but does not distinguish between "bytes the server has durably" vs
   "bytes in flight inside the buf_channel". For the purposes of REAPI
   compliance this is acceptable (the spec says `committed_size` is the number
   of bytes the server has received), but it means a crash loses all partial
   state.

4. **No deduplication across concurrent partial uploads for the same digest.**
   Two clients uploading the same blob with different UUIDs maintain fully
   independent state.

This document designs improvements to make resumable writes robust under memory
pressure, crash recovery (optional), and high concurrency.

## Current Architecture

```
Client ──WriteRequest──> ByteStreamServer
                              │
                    ┌─────────┴─────────┐
                    │ active_uploads     │  HashMap<u128, (AtomicU64, Option<IdleStream>)>
                    │  keyed by UUID     │
                    └─────────┬─────────┘
                              │
                    ┌─────────┴─────────┐
                    │ ActiveStreamGuard  │  Holds StreamState { uuid, digest, tx, store_update_fut }
                    └─────────┬─────────┘
                              │
               tx: DropCloserWriteHalf ──────> rx: DropCloserReadHalf
                                                       │
                                               store.update(digest, rx, ...)
                                                       │
                                              FilesystemStore / MemoryStore / ...
```

On client disconnect, `ActiveStreamGuard::drop()` converts the stream into an
`IdleStream` with an `idle_since` timestamp. The global sweeper task removes
entries older than `idle_stream_timeout`.

On reconnect (`create_or_join_upload_stream`), the `IdleStream` is converted
back into an `ActiveStreamGuard`. The `DropCloserWriteHalf` retains its
`bytes_written` counter, so the `write_offset` check in `process_client_stream`
correctly handles overlapping or resuming data.

## Design

### 1. Retain partial data in the streaming blob buffer

When the streaming blob pipeline design is implemented (replacing the current
buf_channel with a `StreamingBlob` that holds a `Vec<Bytes>` chain), partial
writes map naturally onto this structure:

- The `StreamingBlob` accumulates `Bytes` chunks from the client.
- On disconnect, the `StreamingBlob` remains alive (held by the `IdleStream`
  entry), with its data intact.
- On reconnect, the client resumes sending from `write_offset ==
  streaming_blob.len()`.
- On `finish_write`, the `StreamingBlob` is finalized (EOF), the hash is
  verified, and the blob is committed to the store.

Until the streaming blob pipeline lands, the current buf_channel approach works
because `DropCloserWriteHalf` keeps its byte counter and the store's
`update()` future stays suspended (the `rx` end is still open, waiting for more
data). The key invariant is: **as long as the `StreamState` is alive, the
partial upload is resumable.**

### 2. Memory-pressure eviction for partial writes

#### 2a. Per-instance memory budget

Add a configurable `max_partial_write_bytes` to `ByteStreamConfig`:

```rust
/// Maximum total bytes held across all partial (idle) uploads for this
/// instance. When exceeded, the oldest idle streams are evicted first.
/// 0 means unlimited (rely on time-based eviction only).
/// Default: 256 MiB.
#[serde(default = "default_max_partial_write_bytes")]
pub max_partial_write_bytes: u64,
```

`InstanceInfo` tracks `partial_write_bytes: AtomicU64`, incremented when a
stream goes idle (by `bytes_received`) and decremented when it is resumed or
evicted.

#### 2b. Eviction policy

The existing sweeper task is extended with a second eviction pass after the
time-based sweep:

```
loop {
    sleep(sweep_interval).await;

    let mut uploads = active_uploads.lock();

    // Pass 1: evict streams that exceeded idle_stream_timeout (existing logic)
    // Pass 2: if partial_write_bytes > max_partial_write_bytes, evict idle
    //         streams oldest-first until under budget
}
```

Eviction order for the memory-pressure pass: sort idle streams by
`idle_since` ascending (oldest first). This is O(n log n) in the number of
idle streams, but idle streams should be a small fraction of active uploads.

When an idle stream is evicted:
- Drop the `StreamState` (closes the `DropCloserWriteHalf`, which propagates
  EOF/error to the store update future).
- Decrement `partial_write_bytes` by the stream's `bytes_received`.
- Increment `idle_stream_evictions_memory` metric counter.

#### 2c. Metrics

Add to `ByteStreamMetrics`:

```rust
pub partial_write_bytes: AtomicU64,          // current total bytes in idle streams
pub idle_stream_evictions_memory: AtomicU64,  // evictions due to memory pressure
```

### 3. Resumption protocol

The resumption protocol follows the REAPI ByteStream spec exactly. The server
already implements the core logic; this section documents the contract and
tightens edge cases.

#### 3a. `QueryWriteStatus` contract

```
Client: QueryWriteStatus { resource_name: "uploads/{uuid}/blobs/{hash}/{size}" }

Server response (three cases):
  1. UUID found in active_uploads, stream is active:
     → { committed_size: bytes_received, complete: false }
  2. UUID found in active_uploads, stream is idle:
     → { committed_size: bytes_received, complete: false }
  3. UUID not found, but blob exists in store (has() returns Some):
     → { committed_size: size, complete: true }
  4. UUID not found, blob not in store:
     → { committed_size: 0, complete: false }
```

Cases 1-2 are already implemented. Case 3 and 4 are implemented. No changes
needed for `QueryWriteStatus`.

**Cross-reference:** When the streaming blob pipeline is active, resumption
introduces additional correctness concerns. See `streaming-blob-pipeline-design.md`
Section 11, item 3 (no validation that resumed writer matches original client)
for the risk that a different client resuming an upload feeds inconsistent data
to in-flight `StreamingBlobReader`s, and the mitigation (track client identity
or invalidate the `StreamingBlob` on resume).

#### 3b. `Write` resumption contract

When the client sends a `WriteRequest` with `write_offset > 0`:

1. **`write_offset < bytes_received`**: The overlapping prefix is skipped
   (already implemented in `process_client_stream`). This handles the case
   where the client retransmits data it wasn't sure the server received.

2. **`write_offset == bytes_received`**: Normal resumption. Data is appended
   to the existing stream.

3. **`write_offset > bytes_received`**: Gap in the data. The server returns
   `Code::Unavailable` with a message indicating the committed offset. The
   client should call `QueryWriteStatus` and retry.

4. **`finish_write` on an overlapping chunk where `write_offset + len <
   bytes_received`**: Error. The client claims to be done but the server
   already has more data than the client sent in total. This indicates a
   corrupted retry.

All four cases are already handled in the current `process_client_stream`
implementation.

#### 3c. Hash verification

Hash verification happens only at EOF, after all bytes have been received and
`finish_write` is set. This is unchanged. The store's `update()` path performs
the digest check (for `VerifyStore`) or the `FilesystemStore` renames the temp
file using the content hash.

**Important**: when resuming a write, the server does NOT re-hash the
previously received bytes. The hash is computed incrementally by the store
pipeline (the data has already flowed through). This is correct because the
buf_channel / streaming blob holds the data in memory or on disk, and the
store's reader side computes the hash as it consumes.

If the streaming blob pipeline adds a server-side `DigestHasher` (for early
rejection of corrupted uploads), that hasher's state must be preserved across
idle/resume transitions. This is naturally the case since the `StreamState`
(and thus the hasher) survives in the `IdleStream`.

### 4. Configuration

New fields on `ByteStreamConfig`:

```json5
{
  // Existing field (already implemented):
  "persist_stream_on_disconnect_timeout": 60,

  // New fields:
  "max_partial_write_bytes": 268435456,  // 256 MiB default; 0 = unlimited
}
```

`persist_stream_on_disconnect_timeout` already controls how long idle streams
survive. The new `max_partial_write_bytes` adds a memory-based eviction
threshold that works alongside the time-based one.

### 5. Integration with `active_uploads`

The design intentionally extends the existing `active_uploads` mechanism rather
than replacing it:

| Concern | Current | After this design |
|---|---|---|
| Partial state storage | buf_channel + store future | Same (streaming blob later) |
| Time-based eviction | Sweeper task, `idle_stream_timeout` | Unchanged |
| Memory-pressure eviction | None | Sweeper pass 2, `max_partial_write_bytes` |
| Resumption detection | `create_or_join_upload_stream` | Unchanged |
| UUID collision handling | Append nanosecond timestamp | Unchanged |
| `QueryWriteStatus` | Reads `AtomicU64` bytes_received | Unchanged |
| Metrics | `active_uploads`, `resumed_uploads`, `idle_stream_timeouts` | + `partial_write_bytes`, `idle_stream_evictions_memory` |

### 6. Crash recovery (future work)

The current design does not survive a server restart: all partial upload state
is in-memory. For crash recovery:

- `FilesystemStore` already writes to a temporary file during `update()`. If
  the temp file naming scheme includes the upload UUID, a restarting server
  could scan for partial temp files and reconstruct `active_uploads` entries.
- This is out of scope for the initial implementation but the streaming blob
  design should use a naming convention that enables it (e.g.,
  `{uuid}-{hash}-{expected_size}.part`).

### 7. Interaction with mirror writes

When a write is resumed, the mirror channel (`mirror_tx`) is set up fresh on
each `inner_write` call. The mirror receives only the *new* data from the
resumed portion, not the previously-received prefix. This is acceptable because:

- The mirror target (a worker via `WorkerProxyStore`) may have already received
  the earlier data from the original write attempt.
- Mirror writes are best-effort and non-fatal.
- If the mirror worker is different from the original attempt, it will receive a
  partial blob and discard it (the hash won't verify).

A future optimization could track whether the mirror received the full prefix
and skip re-mirroring, but this is not worth the complexity for the initial
implementation.

**Cross-reference:** When the streaming blob pipeline is active, resumed writes
interact with in-flight mirror readers in a more complex way. See
`streaming-blob-pipeline-design.md` Section 11, item 4 (mirror data loss on
resumed writes) for the risk that mirror readers see a gap or duplicate data
when a write is resumed from an offset, and the mitigation (invalidate the
existing `StreamingBlob` on resume and create a new one).

## Implementation Plan

1. **Add `max_partial_write_bytes` config field** to `ByteStreamConfig` in
   `nativelink-config/src/cas_server.rs`.

2. **Add `partial_write_bytes: AtomicU64` and eviction metrics** to
   `ByteStreamMetrics` and `InstanceInfo`.

3. **Extend the sweeper task** with the memory-pressure eviction pass (section
   2b).

4. **Update `ActiveStreamGuard::drop()`** to increment `partial_write_bytes`
   when converting to `IdleStream`.

5. **Update `IdleStream::into_active_stream()`** to decrement
   `partial_write_bytes` when resuming.

6. **Update sweeper eviction** to decrement `partial_write_bytes` when
   evicting idle streams.

7. **Tests**:
   - Unit test: partial write survives disconnect and resumes correctly
     (existing test coverage likely covers this; verify).
   - Unit test: memory-pressure eviction triggers when budget is exceeded.
   - Unit test: `QueryWriteStatus` returns correct `committed_size` for idle
     stream.
   - Integration test: client disconnect + reconnect + finish_write produces
     correct blob.

## Open Questions

1. **Should `max_partial_write_bytes` be per-instance or global?** Per-instance
   is simpler and matches the existing `active_uploads` structure. Global would
   require cross-instance coordination but better reflects actual memory usage.
   Recommendation: per-instance, with a note that operators should sum across
   instances when capacity planning.

2. **Should the sweeper use a priority queue instead of sorting?** For the
   expected number of idle streams (tens to low hundreds), sorting a Vec is
   fine. A priority queue (BinaryHeap) would be warranted at thousands of
   concurrent idle streams, which is unlikely in practice.

3. **Should we cap the number of concurrent partial uploads?** In addition to
   the byte budget, a `max_partial_uploads` count limit could prevent
   pathological cases where many tiny uploads each hold a `StreamState` (which
   includes a `JoinHandleDropGuard` for the store update future). This is
   lightweight to add alongside the byte budget.
