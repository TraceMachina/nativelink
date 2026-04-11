# NativeLink Latency Reduction Opportunities: Protocol-Level Analysis

## Current Architecture Summary

The codebase reveals a highly optimized system with significant work already done:
- **Pipelined input materialization** with concurrent fetcher/producer/consumer (lines 1244-1705 of `running_actions_manager.rs`)
- **Pre-resolved directory trees** sent from scheduler to worker in StartExecute messages (line 2475, `pre_resolved_tree`)
- **Server-side prefetch** pushing small blobs to workers before they request them (line 1936, `spawn_prefetch`)
- **Directory cache** with direct-use mode (symlink-to-cache, line 1729)
- **Deferred remote upload** (output blobs written to local fast store first, background upload to remote CAS, line 3017)
- **Locality-aware scheduling** with tiered cache affinity (line 539-698 of `api_worker_scheduler.rs`)
- **BatchReadBlobs** for small blobs during input materialization (line 1309-1320)
- **gRPC compression support** (zstd and gzip) configurable per listener (line 169-175 of `nativelink.rs`)

Given how much is already implemented, the remaining opportunities are more surgical. Here are the concrete, implementable enhancements:

---

## Opportunity 1: Speculative AC Lookup During FindMissingBlobs

**Current bottleneck:** Bazel's Execute flow is: `FindMissingBlobs` -> `BatchUpdateBlobs` -> `Execute`. The `CacheLookupScheduler` (line 174 of `cache_lookup_scheduler.rs`) performs the AC lookup only after `Execute` is called. This means the client uploads all blobs before discovering the action was already cached.

**Location:** `/path/to/nativelink/nativelink-scheduler/src/cache_lookup_scheduler.rs:174-255` and `/path/to/nativelink/nativelink-service/src/cas_server.rs:696-710`

**Proposal:** When `FindMissingBlobs` is called, the server already receives the list of digests. While the digests alone do not contain the action digest, the server can maintain a reverse index from `(command_digest, input_root_digest)` pairs to AC entries. When FindMissingBlobs includes blobs that match known action components, speculatively check the AC and return a hint header in the response metadata (`x-nativelink-action-cached: true`). Bazel would need client-side changes to honor this, making this lower priority.

**Alternative (no client changes):** The `CacheLookupScheduler` already checks the AC before scheduling. The latency here is the blob upload time. A more practical enhancement: move the AC check to happen in parallel with the `FindMissingBlobs` response, not sequentially after `Execute`. Since the `execute_request` contains `skip_cache_lookup`, and the server resolves the `Action` proto (line 272, `execution_server.rs`), the AC lookup is already on the critical path of `inner_execute`. This is already optimal.

**Latency savings:** 0-200ms (only saves time when action is cached and client would have uploaded blobs unnecessarily)
**Complexity:** Medium (requires reverse index or client protocol extension)
**Risk:** False positives in speculative lookups waste CPU; client must gracefully handle hints

---

## Opportunity 2: Compress Worker-to-Server and Server-to-Worker gRPC Traffic

**Current bottleneck:** The capabilities server at `/path/to/nativelink/nativelink-service/src/capabilities_server.rs:143-144` advertises `supported_compressors: vec![]` and `supported_batch_update_compressors: vec![]`. While tonic-level compression is configurable (line 169-175 of `nativelink.rs`), the REAPI-level compressors are not advertised. This means `BatchReadBlobs` and `BatchUpdateBlobs` use `compressor::Value::Identity` (no compression) as seen at `/path/to/nativelink/nativelink-store/src/grpc_store.rs:285` and `/path/to/nativelink/nativelink-service/src/cas_server.rs:413`.

**Location:**
- `/path/to/nativelink/nativelink-service/src/capabilities_server.rs:143-144`
- `/path/to/nativelink/nativelink-service/src/cas_server.rs:413`
- `/path/to/nativelink/nativelink-store/src/grpc_store.rs:285`

**Proposal:** Enable zstd compression at the REAPI protocol level for `BatchReadBlobs`/`BatchUpdateBlobs` and at the tonic level for all RPCs:

1. Advertise `compressor::Value::Zstd` in `supported_compressors` and `supported_batch_update_compressors`
2. In `inner_batch_read_blobs`, compress response data when client advertises zstd in `acceptable_compressors`
3. In the GrpcStore client, set `acceptable_compressors: [Zstd]` in BatchReadBlobs requests
4. Configure tonic-level zstd for the worker CAS listener (worker<->server traffic)

For the 10GbE setup, tonic-level compression is most valuable for the worker<->server link where source files (highly compressible, ~4:1 ratio) dominate. A 100MB input tree compresses to ~25MB, saving 75ms at 10Gbps. For many small actions with 10-50MB of inputs, this saves 8-38ms per action.

**Latency savings:** 10-80ms per action (depends on input tree size and compressibility)
**Complexity:** Small (mostly configuration changes + a few lines in CAS server)
**Risk:** CPU overhead. On M4 Mac Minis, zstd compression/decompression at ~3GB/s is well within CPU budget. But should be opt-in per listener, not global.

---

## Opportunity 3: Overlap Output Upload with Execution Result Reporting

**Current bottleneck:** The action pipeline at `/path/to/nativelink/nativelink-worker/src/local_worker.rs:1334-1340` is:
```
prepare_action -> execute -> upload_results -> get_finished_result
```

`upload_results` (line 1339) uploads all output blobs to the local fast store, including hashing every output file. Only after this completes does `get_finished_result` return, triggering `execution_response` to the server (line 1391). The actual remote CAS upload is already deferred (line 1474, `spawn_upload_to_remote`).

The bottleneck is the local `upload_results` phase. For a rustc action producing a 50MB .rlib, hashing + writing to FilesystemStore takes 10-30ms. For link actions with larger outputs, this can be 50-200ms.

**Location:** `/path/to/nativelink/nativelink-worker/src/local_worker.rs:1334-1340`

**Proposal:** Overlap the upload_results with the execution itself by watching the output directory for new files while the action is still running. For rustc specifically, .rmeta files are written before .rlib files. The server could stream partial results:

1. Add an `IncrementalUploader` that uses `inotify`/`kqueue` to watch output directories
2. As output files are closed by the action process, immediately begin hashing and uploading to fast store
3. When the action completes, only the final delta needs processing

However, this is complex and fragile (actions may write temporary files that get renamed). A simpler approach: **parallelize hashing with CAS writes** in `upload_file`. Currently at line 1817-1821, the file is hashed first, then uploaded. For large files, the hash and upload could be streamed simultaneously (hash as bytes flow through the upload pipeline).

**Latency savings:** 10-50ms for typical actions, 50-200ms for link actions
**Complexity:** Large (inotify approach) / Medium (streaming hash during upload)
**Risk:** File watching approach: race conditions with temp files. Streaming hash: needs careful error handling if upload fails mid-stream.

---

## Opportunity 4: Multiplexed Input Tree Transfer

**Current bottleneck:** Input materialization at lines 1070-1705 of `running_actions_manager.rs` follows this sequence:
1. GetTree RPC (or use pre-resolved tree from scheduler) -- 1-20ms
2. Batch existence check via `has_with_results` -- 5-50ms
3. Partition into small (BatchReadBlobs) and large (ByteStream) -- 0ms
4. Fetch missing blobs concurrently -- 10-500ms depending on cache hit rate
5. Hardlink to work directory -- 5-50ms

Steps 2-4 are already pipelined, but Step 2 (existence check) and Step 3 (fetch decision) are serialized before any blobs start flowing. With the server's pre-resolved tree and locality map, the server already knows which blobs the worker is missing (via `compute_missing_blobs` at line 1881 of `api_worker_scheduler.rs`).

**Location:**
- `/path/to/nativelink/nativelink-scheduler/src/api_worker_scheduler.rs:1881-1920`
- `/path/to/nativelink/nativelink-worker/src/running_actions_manager.rs:1186-1242`

**Proposal:** Extend the `StartExecute` message to include the list of missing digests (the server already computes this). The worker can skip the `has_with_results` check for these digests and immediately begin fetching, saving the 5-50ms existence check round-trip.

The proto field `pre_resolved_dirs` already exists in StartExecute. Add a `missing_digests` field to indicate which blobs the server believes the worker needs.

For the multiplexed stream concept: The scheduler's `spawn_prefetch` (line 1936) already pushes small blobs via `BatchUpdateBlobs` to the worker's CAS. Extending this to cover large blobs via ByteStream would create a full server-push model. However, this duplicates work if the worker also pulls (race condition). The cleaner approach is the hint-based model above.

**Latency savings:** 5-50ms (eliminates existence check round-trip)
**Complexity:** Small (add field to proto, skip has_with_results when present)
**Risk:** Stale locality data means the server might tell the worker to skip checking blobs that were actually evicted. Mitigation: the worker falls back to full check on any materialization error.

---

## Opportunity 5: Eager Worker Slot Release

**Current bottleneck:** At `/path/to/nativelink/nativelink-worker/src/local_worker.rs:1391-1402`, the worker sends `execution_response` first, then `execution_complete` to release the worker slot:

```rust
grpc_client.execution_response(ExecuteResult{...}).await?;  // line 1391
drop(grpc_client.execution_complete(complete).await);       // line 1402
```

The `execution_response` call includes the full action result (output digests, exit code, etc.) and must complete before `execution_complete` releases the worker. If the gRPC round-trip for `execution_response` takes 1-5ms, that is 1-5ms where the worker cannot accept new work.

**Location:** `/path/to/nativelink/nativelink-worker/src/local_worker.rs:1391-1402`

**Proposal:** Send `execution_complete` in parallel with (or immediately after firing) `execution_response`, without waiting for the response acknowledgement. The server can process both messages independently since they reference the same `operation_id`.

Currently `execution_complete` at line 885 of `worker_api_server.rs` restores platform properties to the worker. It could be fired as soon as the worker decides the action is done, before even starting output upload. The output upload already goes to fast store only. The server gets the result via `execution_response`, and the worker slot is freed via `execution_complete` -- these are independent operations.

Going further: fire `execution_complete` immediately after the action process exits (before `upload_results`), and let the result reporting happen asynchronously. The worker can start accepting new work while the previous action's outputs are still being uploaded to fast store.

**Latency savings:** 10-200ms per action (entire upload_results duration becomes overlapped with next action's prepare_action)
**Complexity:** Medium (need to handle the case where the worker accepts new work before the previous result is fully uploaded; requires tracking concurrent upload capacity)
**Risk:** If the worker accepts a new action whose inputs overlap with the previous action's outputs in the fast store, there could be eviction pressure. Mitigation: pin output digests during upload (already done at line 4159).

---

## Opportunity 6: Eliminate Sequential GetTree RPC on First Encounter

**Current bottleneck:** When the scheduler encounters a new `input_root_digest` for the first time, `resolve_input_tree` at line 1752 of `api_worker_scheduler.rs` returns `None` and spawns a background resolution. The first action with this digest gets no pre-resolved tree. This means the worker must issue its own `GetTree` RPC (line 236-264 of `running_actions_manager.rs`), adding 1-20ms to the critical path.

**Location:**
- `/path/to/nativelink/nativelink-scheduler/src/api_worker_scheduler.rs:1752-1861`
- `/path/to/nativelink/nativelink-worker/src/running_actions_manager.rs:1082-1093`

**Proposal:** Instead of returning None on cache miss, block for a short timeout (e.g., 50ms) waiting for the background resolution to complete. For most actions, 50ms is enough to resolve even large trees. If the timeout expires, fall back to the current behavior. This eliminates the duplicate GetTree RPC that the worker would issue.

Alternatively: On the `Execute` RPC path in `execution_server.rs`, speculatively resolve the tree before the action enters the scheduler queue. The Action proto is already decoded at line 272. This pre-warms the tree cache so that by the time `do_try_match` runs, the tree is likely available.

**Latency savings:** 1-20ms (eliminates duplicate GetTree RPC)
**Complexity:** Small (add a short wait with timeout in resolve_input_tree)
**Risk:** Adding 50ms blocking on the scheduler path could increase queuing latency. Mitigation: only wait when the tree resolution is already in-progress (i.e., the background task was already spawned by a previous attempt), and use a cancellable wait.

---

## Opportunity 7: Batch FindMissingBlobs for Prefetch

**Current bottleneck:** The prefetch path at line 1999-2010 of `api_worker_scheduler.rs` does a bulk `has()` check against the worker's CAS endpoint to filter out blobs the worker already has. This is a synchronous gRPC call from the scheduler to the worker. If the worker has 10,000 blobs, this check alone could take 5-20ms.

**Location:** `/path/to/nativelink/nativelink-scheduler/src/api_worker_scheduler.rs:1999-2026`

**Proposal:** Skip the `has()` check entirely and rely on the locality map data that is already available. The `compute_missing_blobs` function at line 1881 already filters using the locality map. The subsequent `has()` check on the worker is redundant when the locality map is fresh (BlobsAvailable sent every 100ms). Remove the `has()` check in `spawn_prefetch` and push all blobs that the locality map says are missing.

**Latency savings:** 5-20ms per prefetch (eliminates one gRPC round-trip to worker)
**Complexity:** Small (remove ~20 lines of code)
**Risk:** Locality map staleness could cause unnecessary blob pushes. At 10GbE, pushing a few extra small blobs (the prefetch is capped at 2MB batches) costs <1ms, far less than the eliminated round-trip.

---

## Opportunity 8: Persistent Bidirectional Action Channel

**Current bottleneck:** The worker-server communication already uses a bidirectional gRPC stream (`connect_worker` at line 273 of `worker_api_server.rs`). `StartAction` messages are sent from server to worker, and `ExecuteResult`/`ExecuteComplete` are sent from worker to server. This is already a persistent channel.

However, the Bazel client's `Execute` and `WaitExecution` RPCs are separate request-response streams (not persistent). Each `Execute` RPC creates a new server-streaming response.

**Location:** `/path/to/nativelink/nativelink-service/src/execution_server.rs:356-374`

**Assessment:** The worker<->server channel is already persistent and bidirectional. The client<->server channel uses standard REAPI streaming RPCs which cannot be changed without protocol modifications. No action needed here.

**Latency savings:** N/A (already implemented)
**Complexity:** N/A
**Risk:** N/A

---

## Opportunity 9: Scheduler Matching Loop Latency

**Current bottleneck:** The matching loop at line 610-633 of `simple_scheduler.rs` waits for either a task_change or worker_change notification, then runs `do_try_match`. On success, it waits again. On failure, it sleeps 100ms before retrying. The `tokio::time::sleep(Duration::ZERO)` at line 519 between cycles yields to the runtime but does not add artificial delay.

However, `do_try_match` processes up to 8 actions concurrently (MATCH_CONCURRENCY at line 234). For bursts of many queued actions, this limits throughput to 8 actions matched per notification cycle. After matching 8, it yields, then immediately re-enters when the Notify is triggered by the next action addition.

**Location:** `/path/to/nativelink/nativelink-scheduler/src/simple_scheduler.rs:228-321`

**Proposal:** Increase MATCH_CONCURRENCY from 8 to 32 or make it configurable. With 10 workers, matching more than 10 actions per cycle is usually wasted, but during bursts (e.g., build startup), having higher concurrency prevents a backlog. The `find_and_reserve_worker` at line 1517 is already atomic (under write lock), so concurrent matches cannot double-book workers.

Also: pre-compute platform properties once per unique set (already done via `props_cache` at line 239) and per-client fair scheduling (already done via `per_client_matches` at line 247). These are already optimized.

**Latency savings:** 1-10ms during burst scheduling (reduces queue drain time)
**Complexity:** Small (change one constant)
**Risk:** Higher lock contention on the worker registry write lock during matching. Mitigation: the lock is held briefly per match.

---

## Opportunity 10: REAPI-Level BatchReadBlobs Compression

**Current bottleneck:** When the worker fetches small blobs via `BatchReadBlobs` at line 820-891 of `running_actions_manager.rs`, the request sets `acceptable_compressors: vec![]` (empty, meaning no compression accepted). Each blob is transferred uncompressed. For source files averaging 4KB each, 1000 files = 4MB uncompressed. With zstd at 4:1, this becomes 1MB, saving 3ms at 10Gbps.

**Location:**
- `/path/to/nativelink/nativelink-worker/src/running_actions_manager.rs:828` (`acceptable_compressors: vec![]`)
- `/path/to/nativelink/nativelink-service/src/cas_server.rs:413` (`compressor: Identity`)

**Proposal:** Enable zstd compression for BatchReadBlobs:
1. Worker sets `acceptable_compressors: [Zstd]` in the request
2. Server compresses each blob with zstd before including in the response
3. Worker decompresses after receiving

Combined with tonic-level compression (Opportunity 2), this is cumulative: tonic compresses the gRPC frame, and REAPI-level compression compresses individual blobs within BatchReadBlobs responses.

**Latency savings:** 3-30ms per action (depends on number of small files)
**Complexity:** Small (a few lines in worker and server CAS code)
**Risk:** Double compression (REAPI + tonic) wastes CPU. Should use only one layer. If tonic-level zstd is enabled, REAPI-level compression for BatchReadBlobs adds little benefit and wastes CPU. Recommendation: Use tonic-level zstd for the worker listener, and skip REAPI-level compression for BatchReadBlobs.

---

## Summary Table

| # | Opportunity | Latency Savings | Complexity | Risk |
|---|------------|----------------|------------|------|
| 1 | Speculative AC lookup during FindMissingBlobs | 0-200ms | Medium | False positives |
| 2 | Enable zstd compression (tonic-level) for worker traffic | 10-80ms | Small | CPU overhead |
| 3 | Overlap output upload with execution | 10-200ms | Medium-Large | Race conditions |
| 4 | Server-provided missing digest hints in StartExecute | 5-50ms | Small | Stale locality data |
| 5 | Eager worker slot release (fire execution_complete before upload) | 10-200ms | Medium | Eviction pressure |
| 6 | Block briefly for tree resolution instead of returning None | 1-20ms | Small | Scheduler blocking |
| 7 | Skip redundant has() check in prefetch path | 5-20ms | Small | Extra blob pushes |
| 8 | Persistent bidirectional channel | N/A | N/A | Already implemented |
| 9 | Increase MATCH_CONCURRENCY | 1-10ms | Small | Lock contention |
| 10 | Enable compression for BatchReadBlobs/tonic | 3-30ms | Small | Double compression |

## Recommended Priority Order

**Highest impact, lowest effort (do first):**
1. **Opportunity 2** - Enable tonic-level zstd on worker listener. Pure config change.
2. **Opportunity 7** - Remove redundant has() in prefetch. Delete code, save an RPC.
3. **Opportunity 4** - Add missing_digests hint to StartExecute proto. Small proto change + skip has_with_results.
4. **Opportunity 9** - Increase MATCH_CONCURRENCY. One-line change.

**High impact, medium effort (do next):**
5. **Opportunity 5** - Eager worker slot release. Decouples worker pipeline stages.
6. **Opportunity 6** - Brief blocking wait for tree resolution. Eliminates cold-start GetTree.

**Medium impact, higher effort (do last):**
7. **Opportunity 3** - Streaming hash during output upload.
8. **Opportunity 1** - Speculative AC lookup (requires client-side changes or protocol extension).

## Critical Files for Implementation
- `/path/to/nativelink/nativelink-worker/src/running_actions_manager.rs`
- `/path/to/nativelink/nativelink-scheduler/src/api_worker_scheduler.rs`
- `/path/to/nativelink/nativelink-worker/src/local_worker.rs`
- `/path/to/nativelink/nativelink-service/src/capabilities_server.rs`
- `/path/to/nativelink/nativelink-service/src/worker_api_server.rs`
