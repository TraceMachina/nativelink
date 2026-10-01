# The Core Abstractions You'll Touch

Part III of this book explains the store algebra for *operators* — how to
compose backends into a topology from a config file. This chapter is the other
half of that coin: the view from *inside* the crates, for the person who is
going to add a store, teach the scheduler a new trick, or wire a new gRPC
surface onto the existing machinery. It is a tour of the handful of traits and
idioms that recur everywhere, read straight from the source. Internalize these
and the rest of the codebase stops being a maze and starts being a set of
variations on themes you already know.

If you have not read [The Store Trait](../part3/store-trait.md) yet, read it
first. This chapter assumes it and extends it toward implementation.

## The Store, From the Implementor's Side

The store has two layers, and the split is the first thing to get straight
because the compiler will enforce it on you. `StoreDriver` is the trait you
`impl` when you add a backend; `StoreLike` is the trait callers use, and `Store`
is the type-erased handle (`Arc<dyn StoreDriver>`) that gets passed around the
rest of the system (`nativelink-util/src/store_trait.rs:357-362`, `435-440`,
`620-623`). You almost never name `StoreDriver` as a caller and you almost never
name `StoreLike` as an implementor. The blanket impl at
`store_trait.rs:421-433` makes every `StoreDriver` usable through the ergonomic
`StoreLike` surface for free.

The three operations that matter are `has_with_results`, `update`, and
`get_part` (`store_trait.rs:648-652`, `669-674`, `731-737`). Everything else on
the trait has a default: `has` and `has_many` fan into `has_with_results`
(`store_trait.rs:629-645`), `get` is `get_part` with offset 0 and no length
(`store_trait.rs:740-747`), and `update_oneshot`/`get_part_unchunked` build a
`buf_channel` pair around the streaming methods so a caller who happens to hold
the whole blob in memory doesn't have to care (`store_trait.rs:704-728`,
`750-774`). Implement the three required methods correctly and the convenience
surface comes along.

### The `Pin<&Self>` receiver, and why

Every I/O method on `StoreDriver` takes `self: Pin<&Self>`, not `&self`
(`store_trait.rs:630`, `669`, `731`). This trips up every newcomer. The reason
is that the trait is `Unpin`-bounded (`store_trait.rs:621-622`) and the stores
are addressed through a shared `Arc`, and pinning the receiver lets a store hold
self-referential or address-sensitive state across an `await` without the
borrow checker fighting you — the pin is a promise the store won't be moved out
from under an in-flight operation. In practice you obtain the pinned receiver
through `StoreLike::as_store_driver_pin` (`store_trait.rs:443-446`) and the
`as_pin` helpers (`store_trait.rs:416-418`, `430-432`); as an implementor you
just write `self: Pin<&Self>` in the signature and move on. The one place it
shows through is that you cannot call another store's method without pinning it
first — hence the `Pin::new(inner_store)` dance in the default
`update_with_whole_file` (`store_trait.rs:695-697`).

### `post_init`: construct first, wire second

`StoreDriver` has one method with nothing to do with I/O:

```rust
// nativelink-util/src/store_trait.rs:626
async fn post_init(self: Arc<Self>) -> Result<(), Error>;
```

Stores are built in one pass and wired in a second. This is what lets a config
file reference stores in any order: a `RefStore` names a target by label at
construction time, when that target may not exist yet, and resolves it in
`post_init` once every store in the config has been built
(`nativelink-store/src/ref_store.rs:84-104`). Call an I/O method before
`post_init` runs and `RefStore` fails loudly — `"ref_store cannot get store
'…', was post_init called?"` (`ref_store.rs:75-78`). When you write a leaf
store, `post_init` is almost always a no-op; when you write a composing store
that names others, this is where you grab their handles.

### Keys are `StoreKey`, not hashes

The key type is an enum, not a raw digest (`store_trait.rs:194-201`): `Digest`
for content-addressed data (CAS) and `Str` for string-keyed data (the AC keys
action results by a string-encoded action digest). The variants never collide —
`Ord` sorts all `Str` before all `Digest`, `Eq` never equates across variants,
and `Hash` mixes in a one-byte variant tag before the payload
(`store_trait.rs:276-316`). If your backend only understands digests,
`into_digest()` BLAKE3-hashes a `Str` key down into one rather than rejecting it
(`store_trait.rs:238-247`). The `'a` lifetime exists only for the borrowed `Str`
case; a `Digest` key is `Copy` and owns nothing.

### An implementor's checklist for a new store

To add a backend, in order:

1. Define your struct, derive `MetricsComponent` (see below), and construct it
   from your config spec in `nativelink-store/src/store_factory.rs`.
2. `impl StoreDriver` with the three required methods, taking `self: Pin<&Self>`.
   Stream through the `reader`/`writer` halves — do not buffer whole blobs.
3. Implement `post_init` (no-op for a leaf; resolve references for a composing
   store), plus the mechanical `inner_store`, `as_any`, `as_any_arc`, and
   `register_remove_callback` (`store_trait.rs:859-875`).
4. Override `optimized_for` only where you genuinely win; the default returns
   `false` for every hint (`store_trait.rs:677-679`). If you can take a whole
   file, advertise `FileUpdates` and implement `update_with_whole_file`.
5. Add a config spec in `nativelink-config/src/stores.rs` and wire it into the
   factory.
6. Implement `HealthStatusIndicator` — or lean on the default `check_health`,
   which round-trips a deterministic random blob through your store
   (`store_trait.rs:777-856`).

The payoff of this discipline is substitutability: because every store presents
the same three operations behind the `Store` handle, any store can wrap any
other, and the composition lives in config rather than code. That is the whole
reason Part III's topologies exist.

## The Scheduler Side

The scheduler is not one object; it is a stack of traits with narrow
responsibilities, and the single most useful thing to learn is which trait owns
what.

**`ActionScheduler` / `WorkerScheduler`** are the two client-facing and
worker-facing halves. `WorkerScheduler` (`nativelink-scheduler/src/worker_scheduler.rs:52`)
owns the worker lifecycle: `add_worker` (`:57`), `update_action` for results and
heartbeats (`:78-83`), `worker_dispatch_accepted`/`worker_dispatch_declined`
(`:60-75`), `remove_worker`/`worker_disconnected` (`:115-120`), and
`remove_timedout_workers`/`set_drain_worker` (`:127-130`). The client side's
entry point is `SimpleScheduler::inner_add_action`
(`simple_scheduler.rs:349-363`), which hands straight to the state manager and
wraps the result in a `SimpleSchedulerActionStateResult` that strips the
internal operation ID before the client sees it (`simple_scheduler.rs:118`,
`359`).

**The state manager** — `SimpleSchedulerStateManager`
(`simple_scheduler_state_manager.rs:372`) — is the brain. It implements three
traits for its three audiences: `ClientStateManager` for submissions and queries
(`:1403-1440`), `WorkerStateManager` for worker updates (`:1443-1487`), and
`MatchingEngineStateManager` for the matching pass (`:1490-1534`). Its core is
the state machine in `inner_update_operation` (`:911-1200`) — keepalive refresh,
queued→executing transitions, requeue-or-fail on error, disconnect handling —
plus the version-conflict retry loop that makes concurrent schedulers safe
(`:40-69`) and timeout detection against the worker registry (`:635-707`). A
client submission flows `inner_add_action` → `ClientStateManager::add_action`
(`:1409`) → `inner_add_operation` (`:1202`) → `action_db.add_action` (`:1207`).

**The awaited-action DB** is the state store under the manager. The trait
`AwaitedActionDb` (`awaited_action_db/mod.rs:147`) exposes `add_action` (`:188`),
`update_awaited_action` (`:181`), and `get_range_of_actions` (`:170`), which the
matching engine uses to pull queued work in priority order. There are two
backends and the choice is a correctness/durability tradeoff, not a
micro-optimization: `MemoryAwaitedActionDb`
(`memory_awaited_action_db.rs:917`) keeps everything in `BTreeMap`/`BTreeSet`
structures in process, which is fast and simple but single-node; and
`StoreAwaitedActionDb` (`store_awaited_action_db.rs:836`) persists versioned
records through a `SchedulerStore` with key prefixes `aa_`, `cid_`, and `ck_`
(`store_awaited_action_db.rs:482-489`), which is what lets multiple scheduler
replicas share state. Both detect and subscribe to a duplicate cacheable action
rather than scheduling it twice.

**The matching engine** lives in `SimpleScheduler::do_try_match_inner`
(`simple_scheduler.rs:451`): pull queued operations, and for each ask the worker
pool `find_worker_for_action` (`api_worker_scheduler.rs:1425`). The real
selection is `inner_find_worker_for_action`
(`api_worker_scheduler.rs:719-782`), which filters the fleet through a
`WorkerCapabilityIndex` by static platform properties, then
`find_available_worker` (`:632-717`) applies the live checks — capacity, a
free-memory veto, dynamic `Minimum` properties — and the allocation strategy
(least-recently-used, least-loaded, best-fit). On a match the scheduler calls
`assign_operation` (`simple_scheduler_state_manager.rs:1511`) and then
`worker_notify_run_action` (`api_worker_scheduler.rs:897`), which is what puts a
`StartExecute` onto the worker's gRPC stream.

**Platform properties** are typed, not stringly. `PlatformPropertyManager`
(`platform_property_manager.rs:26`) converts raw config strings into
`PlatformPropertyValue::{Exact, Minimum, Priority, Ignore}` by declared type
(`:83-100`); `Exact` and `Minimum` restrict matching, `Priority` only expresses
preference, and unknown keys are matched dynamically. This typed model is the
entire basis of worker selection — Part IV spends a chapter on why it is the
whole game.

**`api_worker_scheduler`** is the shim between the worker-facing gRPC stream and
the state manager. It owns the pool, runs the matching call, and — critically —
handles dispatch failure: if the worker's channel is full it pauses the worker
and requeues the action (`api_worker_scheduler.rs:924-940`); if the send errors
it evicts the worker and requeues with a disconnect counter (`:942-983`). Its
`update_action` (`:784-893`) routes worker results into the state manager and
releases the worker's slot on completion.

## The Worker Side

A worker is two cooperating objects. `LocalWorker`
(`nativelink-worker/src/local_worker.rs`) is the gRPC client: it connects to the
WorkerApi service, and its run loop (`local_worker.rs:369-763`) `select!`s on the
`UpdateForWorker` stream, matching `StartAction` to admit work (`:448-701`). The
`RunningActionsManager`
(`nativelink-worker/src/running_actions_manager.rs`) is the engine that actually
runs an action.

The lifecycle is a single `and_then` chain in the run loop
(`local_worker.rs:548-581`), and it reads like the four verbs of the
`RunningAction` trait (`running_actions_manager.rs:1476-1504`):

```rust
// nativelink-worker/src/local_worker.rs:554 (abridged)
action.clone()
    .prepare_action()            // fetch Command + inputs from CAS
    .and_then(RunningAction::execute)        // run the subprocess
    .and_then(RunningAction::upload_results) // outputs → CAS
    .and_then(|action| get_finished_result)  // assemble ActionResult
    .then(|result| action.cleanup())         // always, even on failure
```

Each verb has an `inner_*` implementation:
`inner_prepare_action` fetches the `Command` proto and downloads the input tree
from the CAS (`running_actions_manager.rs:1756-1842`, input download at
`:944-1042`); `inner_execute` runs the subprocess and captures stdout/stderr
(`:1871-2355`); `inner_upload_results` uploads output files and directories back
to the CAS and assembles the `ActionResult` (`:2357-2746`); and `cleanup`
(`:2881-2909`) runs unconditionally via the trailing `.then`. After the chain,
the worker writes the result to the AC through `cache_action_result`
(`local_worker.rs:597`, impl at `running_actions_manager.rs:3775-3789`) and
reports back to the scheduler with `execution_response`
(`local_worker.rs:611`). Note the ordering in the comment at
`local_worker.rs:596`: save to the cache *before* telling the scheduler you are
done. Chapter 4 walks this whole path with waypoints.

## The Service Layer

The services in `nativelink-service/src/` are thin REAPI adapters. Each one
validates an instance name, looks up a `Store` or a scheduler handle, and
translates one protocol method into store or scheduler calls — there is almost
no business logic here, which is the point.

- **CAS** (`cas_server.rs:174`, trait impl `:1117`): `find_missing_blobs` →
  `store.has_many` (`:291-319`, call at `:307`); `batch_read_blobs` →
  `get_part_unchunked` per digest (`:394-492`); `batch_update_blobs` →
  `update_oneshot` (`:321-392`).
- **AC** (`ac_server.rs:45`, impl `:171`): `get_action_result` decodes an
  `ActionResult` proto out of the store (`:82-119`, read at `:107`);
  `update_action_result` encodes and `update_oneshot`s it back (`:121-167`),
  honoring a `read_only` flag.
- **Execution** (`execution_server.rs:280`, impl `:515`): `execute` validates
  inputs exist in the CAS (`:365-448`), builds the `ActionInfo` with its
  `Cacheable`/`Uncacheable` qualifier (`:260-264`), calls `scheduler.add_action`
  (`:465`), and returns a stream built by `to_execute_stream` (`:327-351`) that
  pumps `action_listener.changed()` back to the client as `Operation`s.
- **ByteStream** (`bytestream_server.rs:483`, impl `:1384`): `read` →
  `store.get_part` into a channel (`:770`); `write` → `store.update` draining a
  channel the request stream fills (`:974`), with a oneshot fast-path for small
  single-chunk uploads (`:1643`).
- **WorkerApi** (`worker_api_server.rs:56`, impl `:262`): `connect_worker`
  registers the worker via `scheduler.add_worker` and returns the
  `UpdateForWorker` downstream (`:168-251`); the connection handler routes
  `ExecuteResult`, `KeepAlive`, and `GoingAway` back into the `WorkerScheduler`
  (`:286-488`).

One thing worth flagging for a contributor: the AC-hit short-circuit is *not* in
`execution_server`. The Execution service always calls `add_action`; the cache
lookup happens one layer down, in the `CacheLookupScheduler` that wraps the
real scheduler (`nativelink-scheduler/src/cache_lookup_scheduler.rs:63`). On an
AC hit it resolves the client with `ActionStage::CompletedFromCache`
(`cache_lookup_scheduler.rs:268-273`) and never schedules; on a miss it forwards
to the inner scheduler. Chapter 4 traces this.

## Cross-Cutting Idioms

These show up in every file. Learn them once.

**`Error` / `Code`** (`nativelink-error`). The error type is a `Code` plus a
*stack* of messages plus context (`nativelink-error/src/lib.rs:76-83`); `Code`
is the gRPC status set (`:603-620`), so errors map cleanly onto the wire. You
build them with `make_err!`/`make_input_err!` (`:29-45`) and — this is the idiom
you'll use constantly — annotate fallible calls with `.err_tip(|| "context")`
from the `ResultExt` trait (`:461-497`), which pushes a message onto the stack
without discarding the original. Read a NativeLink error bottom-to-top and you
get a breadcrumb trail from the gRPC boundary down to the syscall.

**`MetricsComponent`** (`nativelink-metric`). Observability is derived, not
hand-written. You `#[derive(MetricsComponent)]`
(`nativelink-metric/src/lib.rs:23`) and tag fields with `#[metric]`; the derive
generates a `publish` impl (`:101-121`) that walks the struct and emits every
tagged field, recursing into nested components. This is why `Store` carries
`#[metric] inner: Arc<dyn StoreDriver>` (`store_trait.rs:360-361`) — metrics
compose down the store tree automatically. When you add a store or scheduler
component, derive it and tag the fields you want visible; you get metrics for
free.

**`buf_channel`** (`nativelink-util/src/buf_channel.rs`). The streaming spine.
`make_buf_channel_pair` (`buf_channel.rs:36-59`) returns a
`DropCloserWriteHalf`/`DropCloserReadHalf` pair over a bounded `mpsc` channel
(depth 2, `:41`) that counts bytes and turns a dropped-without-EOF writer into an
error on the reader rather than a silent truncation. This is how blobs stream
through the store chain without being buffered whole: `update` reads one half,
the gRPC layer or a file reader fills the other. Whenever you move bytes between
layers, this is the vehicle — not a `Vec<u8>`.

**`background_spawn!` / `spawn!`** (`nativelink-util/src/task.rs`). Never call
`tokio::spawn` directly; the lint forbids it (`task.rs:38`). `background_spawn!`
(`:61-74`) wraps the future in a tracing span and propagates the current
OpenTelemetry context so a spawned task stays attached to the request that
created it. `spawn!` (`:77-87`) does the same but returns a `JoinHandleDropGuard`
(`:107-131`) that *aborts the task when the handle is dropped* — the structured
form you want when the task's lifetime should be tied to its owner. Reach for
`background_spawn!` for fire-and-forget work (the cache-lookup spawn at
`cache_lookup_scheduler.rs:233` is a good example) and `spawn!` when a parent
should be able to cancel its child by dropping it.

**`nativelink-config`** (JSON5 → typed structs). The whole system is configured
by one JSON5 file deserialized into typed Rust. `CasConfig::try_from_json5_file`
reads the file and `serde_json5::from_str`s it (`cas_server.rs:1357-1364`); every
spec struct carries `#[serde(deny_unknown_fields)]` so a typo in a config key is
a hard error, not a silent default. When you add a store or scheduler, you add a
spec here (`stores.rs`, `schedulers.rs`) and wire it into the matching factory —
the config *is* the public API of a new backend, so design the spec with the
same care as the code.

Everything in the next chapter is these pieces moving together. Once you can
name which trait owns a given responsibility and recognize these idioms on
sight, you can follow any request through the system by reading the call chain.
