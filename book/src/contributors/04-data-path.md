# The Data Path, End to End

This is the chapter that unlocks the codebase. We are going to follow one
cacheable action — the thing Bazel sends when it wants a compiler invocation run
remotely — through every layer of NativeLink, with `path:line` waypoints at each
boundary. The goal is not to memorize the trace but to learn the *method*: once
you can see where a request crosses a gRPC boundary, where it becomes a trait
call, and where it touches the CAS, you can follow any other request type by the
same reading. We close with exactly that — how to re-run this method for a plain
CAS read and a ByteStream write.

Three kinds of boundary recur, and keeping them distinct is the whole skill:

- **gRPC boundary** — a network hop, a REAPI method, serialized protos. The
  client↔service and scheduler↔worker edges are gRPC.
- **Trait call** — an in-process `.await` through one of the traits from
  Chapter 3 (`StoreDriver`, `ActionScheduler`, `AwaitedActionDb`). No network.
- **The CAS** — content-addressed blob storage, reached as a `Store`. Both the
  service layer and the worker talk to the same CAS; that shared ground truth is
  the point of the whole design.

## The Flow at a Glance

```
 CLIENT (Bazel/Buck2)
   │  gRPC: Execute(action_digest)
   ▼
┌─────────────────────────────────────────────────────────┐
│ ExecutionServer  (nativelink-service/execution_server.rs)│
│  • fetch Action from CAS, validate inputs exist          │
│  • build ActionInfo {Cacheable | Uncacheable}            │
│  • scheduler.add_action(..)         ── trait call ──┐    │
│  • return Operation stream to client ◄──────────┐   │    │
└──────────────────────────────────────────────── │ ──│────┘
                                                   │   ▼
                        ┌──────────────────────────│───────────┐
                        │ CacheLookupScheduler      │           │
                        │  • AC lookup (CAS/AC) ─────┐          │
                        │    hit  → CompletedFromCache│ ◄───────┘  (short-circuit)
                        │    miss → inner.add_action │           │
                        └──────────────────────────── │ ─────────┘
                                                       ▼
                        ┌──────────────────────────────────────┐
                        │ SimpleScheduler + StateManager        │
                        │  • add_action → AwaitedActionDb       │
                        │  • do_try_match: pick worker by       │
                        │    platform properties                │
                        │  • worker_notify_run_action ──┐       │
                        └─────────────────────────────── │ ─────┘
                                      gRPC: StartExecute  ▼
                        ┌──────────────────────────────────────┐
                        │ WorkerApiServer ⇄ LocalWorker         │
                        │  RunningActionsManager:               │
                        │   prepare (CAS→inputs) → execute →    │
                        │   upload (outputs→CAS) → cache (→AC)  │
                        │   → execution_response ──┐            │
                        └──────────────────────────│───────────┘
                                                   ▼
            state update propagates back through AwaitedActionDb
            subscription → Operation stream → CLIENT
```

Everything below is this diagram with line numbers.

## 1. The Client Submits — the Execution Service

Bazel opens a stream to the REAPI `Execution` service and sends an
`ExecuteRequest` carrying the digest of an `Action` it has already uploaded to
the CAS. The handler is `ExecutionServer::execute`
(`nativelink-service/src/execution_server.rs:525`), which delegates to
`inner_execute` (`:353`). **This is the first gRPC boundary.**

`inner_execute` does not blindly schedule. It first fetches the `Action` proto
from the CAS and validates that the action's `Command` and input-root digests
actually exist (`execution_server.rs:365-448`), checking the input blobs with a
`has_many` call (`:420`) and returning `FAILED_PRECONDITION` if anything is
missing (`:434-446`). This is a **CAS** touch: the service reads the same
content-addressed store the client just wrote to.

It then builds an `ActionInfo` via `build_action_info` (`:450-461`), and this is
where cacheability is decided. The `skip_cache_lookup` flag off the request
chooses between `ActionUniqueQualifier::Cacheable` and `::Uncacheable`
(`:260-264`). For our cacheable action it is `Cacheable`. Finally it calls the
scheduler — **a trait call across the `ActionScheduler` boundary**:

```rust
// nativelink-service/src/execution_server.rs:464
let action_listener = instance_info
    .scheduler
    .add_action(OperationId::default(), Arc::new(action_info))
    .await
```

`add_action` returns an `action_listener` — a subscription handle. `inner_execute`
wraps it in `to_execute_stream` (`:470`, defined `:327-351`) and returns that
stream to the client. From here the client simply reads `Operation` messages off
the stream until one is `is_finished()`; the stream is driven by
`action_listener.changed()` (`:336`). Hold onto this handle: it is the wire the
result comes back on in step 5.

## 2. Cache Lookup — AC Hit Returns, Miss Schedules

The `scheduler` the service called is not the raw `SimpleScheduler`. It is
wrapped by a `CacheLookupScheduler`
(`nativelink-scheduler/src/cache_lookup_scheduler.rs:63`), which holds the AC
store (`:66-67`). Its `add_action` spawns a background task
(`cache_lookup_scheduler.rs:233`) that reads the AC for a result at this action's
digest via `get_action_from_store` (`:250`, which calls `get_action_result`,
`:85-105`). **This is a CAS/AC read** — the AC is just a store.

Two outcomes:

- **Hit.** The task resolves the client immediately with
  `ActionStage::CompletedFromCache(action_result)`
  (`cache_lookup_scheduler.rs:268-273`). No worker is ever involved. The
  `action_listener` the client holds fires with a finished operation and the
  stream ends. This is the common case and the entire reason a build cache
  exists.
- **Miss.** The task forwards to the inner scheduler's `add_action`. For a
  cacheable action that returns no result, we proceed into real scheduling.

So the AC-hit short-circuit lives *here*, one layer below the service — not in
`execution_server`, which always calls `add_action` unconditionally. This
indirection is a composition seam: cache lookup is a scheduler decorator, the
same way compression is a store decorator.

## 3. Scheduling — DB, Matching, Dispatch

On a miss we are in `SimpleScheduler::inner_add_action`
(`simple_scheduler.rs:349-363`). It calls the state manager
(`:354`) and wraps the handle so the client never sees the internal operation ID
(`:359`). The state manager — `SimpleSchedulerStateManager` — takes it through
`ClientStateManager::add_action` (`simple_scheduler_state_manager.rs:1409`) →
`inner_add_operation` (`:1202`) → `action_db.add_action` (`:1207`). **These are
all trait calls, no network.**

`action_db` is an `AwaitedActionDb` (`awaited_action_db/mod.rs:147`). Its
`add_action` (`:188`) either enqueues a new `AwaitedAction` in the `Queued`
state or, if an identical cacheable action is already in flight, subscribes to it
rather than duplicating the work. Which backend runs this is a deployment choice
from Chapter 3: in-process `MemoryAwaitedActionDb`
(`memory_awaited_action_db.rs:917`) for a single scheduler, or the persisted
`StoreAwaitedActionDb` (`store_awaited_action_db.rs:836`) when replicas share
state. The action is now queued; the client's stream is parked on its
subscription.

Independently, the scheduler runs a **matching pass**,
`SimpleScheduler::do_try_match_inner` (`simple_scheduler.rs:451`). It pulls
queued operations from the DB in priority order (`get_range_of_actions`,
`awaited_action_db/mod.rs:170`) and for each asks the worker pool
`find_worker_for_action` (`api_worker_scheduler.rs:1425`). The real selection is
`inner_find_worker_for_action` (`:719-782`): it filters the fleet through the
`WorkerCapabilityIndex` by the action's static **platform properties**, then
`find_available_worker` (`:632-717`) applies the live checks — spare capacity, a
free-memory veto, dynamic `Minimum` constraints — and the configured allocation
strategy to pick one worker. Platform-property matching, built on the typed
`PlatformPropertyValue` model (`platform_property_manager.rs:83-100`), is the
entire basis of this decision; Part IV argues it is the whole game.

On a match, the scheduler commits the assignment with `assign_operation`
(`simple_scheduler_state_manager.rs:1511`) — which moves the action to
`Executing` in the DB and can lose a race to a peer scheduler (`Code::Aborted`),
in which case it backs off — and then dispatches with `worker_notify_run_action`
(`api_worker_scheduler.rs:897`). That call puts a `StartExecute` message onto the
chosen worker's `UpdateForWorker` channel (`:905-911`). **This is the second
gRPC boundary**: the message travels over the worker's open stream from the
WorkerApi service. If the channel is full or the send fails, the scheduler
pauses or evicts the worker and requeues the action (`:924-983`) — dispatch is
best-effort and self-healing.

## 4. The Worker — Prepare, Execute, Upload, Cache

The worker side received this `StartExecute` on the stream it opened with
`connect_worker` (`nativelink-service/src/worker_api_server.rs:168-251`). The
`LocalWorker` run loop (`nativelink-worker/src/local_worker.rs:369-763`)
`select!`s on that stream and matches `StartAction` to admit the work
(`:448-701`). It then runs the four-verb lifecycle as a single `and_then` chain
(`local_worker.rs:548-581`):

**Prepare** — `inner_prepare_action`
(`running_actions_manager.rs:1756-1842`). The worker fetches the `Command` proto
from the CAS (`:1765`) and downloads the entire input tree — recursively reading
`Directory` protos and hardlinking file blobs into a work directory — through
`prepare_action_inputs` (`:944-1042`). **This is the worker reading the CAS**:
the same store the client populated and the service validated. Content addressing
is what makes this safe to do blindly — if the digest matches, the bytes are
correct.

**Execute** — `inner_execute` (`:1871-2355`). The worker runs the command as a
subprocess in the prepared directory and captures stdout, stderr, and exit code.
No boundary is crossed here; it is local process execution.

**Upload** — `inner_upload_results` (`:2357-2746`). Every output file and
directory the action produced is written back **into the CAS**, and their digests
are assembled into an `ActionResult` proto (`:2723-2735`). Outputs are now
content-addressed and available to any future reader.

**Cache** — after the chain, the worker writes the `ActionResult` to the **AC**
via `cache_action_result` (`local_worker.rs:597`, impl
`running_actions_manager.rs:3775-3789`). Note the deliberate ordering
(`local_worker.rs:596`): save to the cache *before* notifying the scheduler. This
is what makes the next identical build a step-2 cache hit. `cleanup`
(`running_actions_manager.rs:2881-2909`) then runs unconditionally via the
trailing `.then`, even if execution failed.

Finally the worker reports completion back to the scheduler with
`execution_response` (`local_worker.rs:611`) — **a gRPC message back across the
worker boundary**. The WorkerApi service routes it into
`WorkerScheduler::update_action` (`worker_api_server.rs:423`), which drives the
state manager's `inner_update_operation`
(`simple_scheduler_state_manager.rs:911-1200`) to mark the operation `Completed`.

## 5. The Result Flows Back to the Client

Marking the operation `Completed` in the state manager updates the
`AwaitedActionDb`, which notifies every subscriber. The client's parked
subscription — the `action_listener` from step 1 — fires: its `changed()` inside
`to_execute_stream` (`execution_server.rs:336`) wakes, converts the finished
state to an `Operation` (`:340`), yields it on the stream, and because the stage
`is_finished()` the stream completes (`:341`). Bazel reads the `ExecuteResponse`,
pulls the output blobs it needs from the CAS by digest, and the action is done.

The subscription is the join between the asynchronous world of scheduling and the
synchronous expectation of a client holding a stream open. Nothing polls; the
state transition pushes.

## How to Follow a Different Request Type

The method is always the same: find the gRPC handler, then follow each `.await`
down, labeling every boundary as gRPC / trait call / CAS. Two short examples.

**A plain CAS read (`BatchReadBlobs`).** Start at the handler
`CasServer::batch_read_blobs` (`nativelink-service/src/cas_server.rs:1180`,
inner at `:394-492`). It validates the instance, then for each requested digest
issues a `store.get_part_unchunked` (`:433`) — a single trait call into the CAS
`Store` — and packs the bytes into the response. One gRPC boundary in, one CAS
read per digest, no scheduler, no worker. `FindMissingBlobs` is the same shape
but calls `has_many` (`:307`) instead of reading bytes — which is why it is cheap
enough to be the most frequent REAPI call.

**A ByteStream write.** Start at `ByteStreamServer::write`
(`nativelink-service/src/bytestream_server.rs:1508`). The streaming request is
the gRPC boundary; the handler sets up a `buf_channel` pair, drains the inbound
chunks into the write half, and simultaneously runs `store.update` reading the
read half (`:974`) — the backpressure-aware streaming from Chapter 3, so a 2 GB
blob never materializes in memory. One gRPC boundary, one streaming CAS write.
`Read` is the mirror image: `store.get_part` into a channel the response stream
drains (`:770`).

Every request in NativeLink decomposes this way. The services are thin REAPI
adapters over the `Store` and scheduler traits; the scheduler is a stack of
trait calls over the `AwaitedActionDb`; the worker is a four-verb lifecycle over
the CAS and AC. Learn to spot the three boundaries and the whole system reads as
one coherent machine, with content addressing as the ground truth underneath all
of it.
