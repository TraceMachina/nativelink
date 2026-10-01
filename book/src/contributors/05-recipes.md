# Recipes

This is the "I want to make a change — show me the exact steps and the real files" chapter. Every recipe is derived from how an *existing* feature in the codebase does it. We name the exemplar, list the files you touch, and call out the wiring that often gets forgotten — the match arm, the config variant, the `post_init` — the things that compile fine when omitted and then silently do nothing at runtime.

A note on method: NativeLink leans hard on a factory pattern. You implement a thing, you add a config variant that describes it, and you add one match arm that connects the two. Miss the match arm and your code is dead — it builds, it tests in isolation, and it is never constructed from a real config. Most of the "it doesn't work and I don't know why" in this codebase is a missing registration. Each recipe below ends by pointing at the exact registration site.

---

## Recipe 1: Add a new store backend

**Why/when.** You have a storage medium NativeLink doesn't speak yet — a new object store, a new database, a wrapper that transforms bytes on the way through. Because everything in NativeLink is a store (CAS, AC, worker artifacts — all the same trait), a new backend is immediately usable everywhere a store is, with no changes to the service layer.

**Exemplar to copy.** The two simplest real stores: [`NoopStore`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-store/src/noop_store.rs) for a leaf store that holds no data, and [`MemoryStore`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-store/src/memory_store.rs) for a leaf store that does. Copy `noop_store.rs` for the skeleton; read `memory_store.rs` for how a real one handles streaming, zero-digests, and eviction.

**Files you touch.**

1. `nativelink-store/src/<your_store>.rs` — the implementation.
2. `nativelink-store/src/lib.rs` — add `pub mod <your_store>;`.
3. `nativelink-config/src/stores.rs` — add a config variant and its spec struct.
4. `nativelink-store/src/default_store_factory.rs` — the match arm that constructs it.
5. `nativelink-store/tests/<your_store>_test.rs` — the regression test.

### Step 1 — Implement `StoreDriver`

Three I/O methods plus `post_init`. The entire `NoopStore` trait impl is 60 lines; here is the shape, with the real signatures from `noop_store.rs:48-108`:

```rust
#[async_trait]
impl StoreDriver for MyStore {
    async fn post_init(self: Arc<Self>) -> Result<(), Error> {
        Ok(())  // leaf stores no-op; see Step 1b
    }

    async fn has_with_results(
        self: Pin<&Self>,
        keys: &[StoreKey<'_>],
        results: &mut [Option<u64>],
    ) -> Result<(), Error> { /* fill results[i] = Some(size) or None */ }

    async fn update(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        reader: DropCloserReadHalf,
        size_info: UploadSizeInfo,
    ) -> Result<u64, Error> { /* stream from reader into your backend */ }

    async fn get_part(
        self: Pin<&Self>,
        key: StoreKey<'_>,
        writer: &mut DropCloserWriteHalf,
        offset: u64,
        length: Option<u64>,
    ) -> Result<(), Error> { /* stream your backend into writer, then send_eof */ }

    fn inner_store(&self, _key: Option<StoreKey>) -> &dyn StoreDriver { self }
    fn as_any<'a>(&'a self) -> &'a (dyn Any + Sync + Send + 'static) { self }
    fn as_any_arc(self: Arc<Self>) -> Arc<dyn Any + Sync + Send + 'static> { self }
}
```

Note the `self` receivers: `post_init` takes `Arc<Self>`, the three I/O methods take `Pin<&Self>`. This is not optional — it is how the trait object is pinned for streaming. The constructor returns `Arc<Self>` to match (`noop_store.rs:43-45`, `memory_store.rs:80-87`).

Two things that trip people up, both real in `MemoryStore`:

- **The empty digest.** By REAPI convention the zero-length digest is assumed to exist. `has_with_results` must synthesize `Some(0)` for it and `get_part` must `send_eof()` immediately rather than looking it up (`memory_store.rs:118-125`, `246-251`, using `is_zero_digest`). Skip this and Bazel builds break on empty files.
- **Draining on no-op writes.** If your `update` discards data, you must still drain the reader or the writer errors on a dropped connection — see `NoopStore::update` draining the stream before returning (`noop_store.rs:71-74`).

### Step 1b — `post_init` (the forgettable one)

Stores are constructed first and wired second. `post_init` runs after *every* store in the config is built. A leaf store returns `Ok(())`. A store that references another store *by name* resolves it here — this is why `RefStore` exists and why calling its I/O methods before `post_init` fails loudly. If your store composes another store by label rather than by direct construction, do the lookup in `post_init`, not `new`.

### Step 2 — Add `MetricsComponent`

Two ways, both real:

- **Derive it** when the struct has fields worth publishing. `MemoryStore` derives it and annotates fields (`memory_store.rs:62-77`):
  ```rust
  #[derive(Debug, MetricsComponent)]
  pub struct MemoryStore {
      #[metric(group = "evicting_map")]
      evicting_map: EvictingMap<...>,
      #[metric(help = "Maximum bytes this store will hold before eviction (0 = unbounded)")]
      max_bytes: u64,
  }
  ```
- **Hand-write the empty impl** when there's nothing to publish. `NoopStore` does exactly this (`noop_store.rs:32-40`), returning `MetricPublishKnownKindData::Component`.

Then add health: `default_health_status_indicator!(MyStore);` at the bottom of the file is the one-liner both stores use (`noop_store.rs:110`, `memory_store.rs:302`). A store with a registry (like `MemoryStore`) also overrides `register_health` to register itself (`memory_store.rs:286-288`).

### Step 3 — Config variant

In `nativelink-config/src/stores.rs`, add a variant to the `StoreSpec` enum (`stores.rs:52`) and a spec struct. The enum uses `#[serde(rename_all = "snake_case")]`, so `MyBackend(MyBackendSpec)` becomes `"my_backend": { ... }` in JSON5. Copy the `Noop` variant for a parameterless store (`stores.rs:614-624`, spec struct `pub struct NoopSpec {}`) or `Memory` for one with options (`stores.rs:75-86`). Every spec struct carries `#[serde(deny_unknown_fields)]` — a typo'd key in a config is a startup error, not a silent default, and your struct must preserve that.

The doc comment on the variant is not decoration: it is the config reference. Include a `**Example JSON Config:**` block like every existing variant does.

### Step 4 — Register in the factory (the one everyone forgets)

Open [`nativelink-store/src/default_store_factory.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-store/src/default_store_factory.rs). The `store_factory` function (`default_store_factory.rs:53-57`) is one big `match backend` over `StoreSpec`. Add your arm. The two exemplars are one line each:

```rust
StoreSpec::Memory(spec) => MemoryStore::new(spec),              // :64
StoreSpec::Noop(_) => NoopStore::new(),                         // :129
```

A *wrapping* store recursively calls `store_factory` on its inner spec — see `Verify`, `Compression`, `FastSlow` in the same match (`:92-120`). The factory then calls `Store::new(store)` and, if a health registry was passed, `register_health` (`:143-147`). **If you skip this arm, your store compiles, your tests pass, and no config can ever instantiate it.** This is the single most common mistake.

### Step 5 — Write the regression test

Covered in full in Recipe 6. Minimum: construct the store, round-trip a blob through `update_oneshot` / `get_part`, assert `has` reports the right size. [`memory_store_test.rs:44-64`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-store/tests/memory_store_test.rs) is the pattern.

---

## Recipe 2: Add a scheduler / awaited-action backend

**Why/when.** The scheduler tracks every in-flight action. In-memory tracking is fine for a single scheduler; a multi-scheduler fleet needs shared state so any scheduler can report on any action. That shared state lives behind the `AwaitedActionDb` trait. Add a new impl when you want a new persistence/coordination substrate (the two shipped are in-memory and Redis-via-store).

**Exemplar to copy.** [`MemoryAwaitedActionDb`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/memory_awaited_action_db.rs) for the local case; [`StoreAwaitedActionDb`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/store_awaited_action_db.rs) for the shared case built on a `Store`.

**Files you touch.**

1. `nativelink-scheduler/src/<your>_awaited_action_db.rs` — the impl.
2. `nativelink-config/src/schedulers.rs` — a new `ExperimentalSimpleSchedulerBackend` variant.
3. `nativelink-scheduler/src/default_scheduler_factory.rs` — the branch that constructs it.

### Step 1 — Implement `AwaitedActionDb`

The trait is at [`awaited_action_db/mod.rs:147`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/awaited_action_db/mod.rs). It is an associated-type trait (`type Subscriber: AwaitedActionSubscriber`) with the operations the scheduler needs:

- `add_action` — insert or join an action, return a subscriber (`mod.rs:188-193`)
- `get_awaited_action_by_id`, `get_by_operation_id` — lookups (`mod.rs:151-167`)
- `get_range_of_actions` — sorted range query over a `SortedAwaitedActionState`, which drives matching (`mod.rs:170-178`)
- `update_awaited_action` — commit a state change and notify subscribers (`mod.rs:181-184`)
- `get_all_awaited_actions` — avoid; it's a full scan (`mod.rs:157-161`)

The trait requires `MetricsComponent` as a supertrait (`mod.rs:147`), so your impl needs the derive or a hand-written `publish`. The subscriber half is the push side: when `update_awaited_action` fires, subscribers see the new state — that is how a client blocked in `WaitExecution` wakes up.

### Step 2 — Config variant

The backend selection lives *inside* `SimpleSpec`, not at the top-level `SchedulerSpec`. Add a variant to `ExperimentalSimpleSchedulerBackend` in `nativelink-config/src/schedulers.rs:333` (today: `Memory` and `Redis(...)`). The top-level `SchedulerSpec` enum (`schedulers.rs:27`) is for scheduler *kinds* (`Simple`, `Grpc`, `CacheLookup`, `PropertyModifier`) — you are not adding one of those; you are adding a backend to the `Simple` scheduler.

### Step 3 — Register in the scheduler factory

[`default_scheduler_factory.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/default_scheduler_factory.rs) has two levels. `scheduler_factory` matches `SchedulerSpec` kinds (`:62-113`). Below it, `simple_scheduler_factory` matches the backend (`:124-194`):

```rust
match spec.experimental_backend.as_ref()
        .unwrap_or(&ExperimentalSimpleSchedulerBackend::Memory) {
    ExperimentalSimpleSchedulerBackend::Memory => {
        let awaited_action_db = memory_awaited_action_db_factory(/* ... */);  // :131
        let (action_scheduler, worker_scheduler) =
            SimpleScheduler::new(spec, awaited_action_db, task_change_notify, /* ... */);
        // :136
    }
    ExperimentalSimpleSchedulerBackend::Redis(cfg) => {
        let awaited_action_db = StoreAwaitedActionDb::new(/* ... */).await?;   // :164
        let (action_scheduler, worker_scheduler) =
            SimpleScheduler::new(spec, awaited_action_db, task_change_notify, /* ... */);
    }
}
```

The crucial observation: `SimpleScheduler::new` is **generic over `A: AwaitedActionDb`** (`simple_scheduler.rs:1040-1064`). The scheduler itself does not care which backend you built — construct your db, hand it to the same `SimpleScheduler::new`, return the `(action_scheduler, worker_scheduler)` pair. Your branch is the only new code in the factory.

### Changing a scheduling *behavior* instead

If you want to change *how* actions are selected rather than where they're stored, the matching loop is `do_try_match` / `do_try_match_inner` in [`simple_scheduler.rs:444-451`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/simple_scheduler.rs). It pulls queued actions via `get_range_of_actions` and pairs each with a worker. If your behavior is a *transform* on the request (e.g. rewriting platform properties), the cleaner path is a wrapping scheduler — copy `PropertyModifierScheduler` (`default_scheduler_factory.rs:85-98`), which wraps an inner scheduler and is itself just another `SchedulerSpec` variant. Wrapping schedulers compose exactly like wrapping stores.

---

## Recipe 3: Add a worker platform property / change matching

**Why/when.** Platform properties are the entire matching game: an action declares what it needs (`OSFamily=linux`, `cores>=8`), a worker declares what it offers, and the scheduler pairs them. You add a property when workers vary in a way actions should be able to select on.

**Exemplar to copy.** Any existing property; the machinery is uniform. The types are in [`nativelink-util/src/platform_properties.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-util/src/platform_properties.rs).

**The data flow** — follow it end to end before touching anything:

1. **Scheduler config declares the property's *type*.** `SimpleSpec.supported_platform_properties: Option<HashMap<String, PropertyType>>` (`schedulers.rs:140`). `PropertyType` (`schedulers.rs:43`) is one of `Minimum`, `Exact`, `Priority`, `Ignore`. The type decides how the string value is interpreted and how it matches.
2. **Config → typed values.** `PlatformPropertyManager::make_prop_value` converts each raw `String` to a `PlatformPropertyValue` according to the configured `PropertyType` (`platform_property_manager.rs:83-99`): `Minimum → Minimum(u64)`, `Exact → Exact(String)`, etc. A property not in `supported_platform_properties` becomes `Unknown` (dynamic — matched by exact string against the worker's value).
3. **Worker config declares what it offers.** `WorkerConfig.platform_properties: HashMap<String, WorkerProperty>` (`cas_server.rs:1129`). `WorkerProperty` (`cas_server.rs:749`) is either `Values(Vec<String>)` (static) or `QueryCmd(String)` (a command run at startup — e.g. `nproc` for core count).
4. **Worker registration ships them.** On connect, the worker builds a `ConnectWorkerRequest` from its config, evaluating `QueryCmd` properties by executing the command (`worker_utils.rs:make_connect_worker_request`, `local_worker.rs` registration).
5. **Matching.** `PlatformPropertyValue::is_satisfied_by` is the comparison (`platform_properties.rs:143`), lifted to the whole property set by `PlatformProperties::is_satisfied_by` (`platform_properties.rs:47`): `Exact` requires equality, `Minimum` requires the worker's number ≥ the action's, `Priority`/`Ignore` have their own semantics. The scheduler calls this during `do_try_match` (Recipe 2).

**What you actually touch** depends on the kind of change:

- **A new property using an existing semantics** (e.g. a new `Exact` label): no code. Add it to the scheduler's `supported_platform_properties` config and to each worker's `platform_properties` config. It flows through the uniform machinery above. Then update the deployment examples (Recipe 5).
- **A new *matching semantics*** (something `Exact`/`Minimum`/`Priority`/`Ignore` can't express): this is a code change. Add a `PropertyType` variant (`schedulers.rs:43`), a `PlatformPropertyValue` variant (`platform_properties.rs:132`), the conversion in `make_prop_value` (`platform_property_manager.rs:83`), and the comparison in `is_satisfied_by` (`platform_properties.rs:143`). The compiler's exhaustiveness checking will walk you through every match that needs the new arm — follow it; don't add a catch-all `_`, which would silently mis-match.

The forgettable wiring: a property only matches if it is **declared in `supported_platform_properties`**. An unlisted property falls to `Unknown` and matches by raw string equality, which is almost never what you intended for a numeric or priority property.

---

## Recipe 4: Add or change a gRPC endpoint / REAPI surface

**Why/when.** You're extending the protocol surface — a new RPC on an existing service, or a whole new service. The REAPI protos themselves are vendored and you generally don't touch them; NativeLink's own protos (worker API, origin events) are where custom surface lives.

**Exemplar to copy.** [`AcServer`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-service/src/ac_server.rs) — the smallest full REAPI service (two RPCs). For a NativeLink-defined service, read `worker_api_server.rs` against `worker_api.proto`.

**Files you touch.**

1. `nativelink-proto/.../your_service.proto` — proto definition (only for a new/changed message or service).
2. `nativelink-proto/genproto/lib.rs` — regenerated, not hand-edited.
3. `nativelink-service/src/<your>_server.rs` — the impl.
4. `nativelink-config/src/cas_server.rs` — the config struct and a `ServicesConfig` field.
5. `src/bin/nativelink.rs` — wire it into the router.

### Step 1 — Proto (if the message/service shape changes)

The local protos live under `nativelink-proto/com/github/trace_machina/nativelink/...` — e.g. `worker_api.proto` (`service WorkerApi`, `message ConnectWorkerRequest`). The REAPI/Google protos under `build/bazel/...` and `google/...` are vendored; don't edit them to add NativeLink features.

Generated Rust is **not** written by hand. `nativelink-proto/genproto/lib.rs` `include!`s generated `.pb.rs` files and is produced by the codegen tool (`gen_protos_tool.rs`, driven via `bazel run nativelink-proto:update_protos`). Edit the `.proto`, regenerate, commit the generated output.

### Step 2 — Implement the service

A tonic service impl is three parts. From `ac_server.rs`:

```rust
pub struct AcServer { stores: HashMap<String, AcStoreInfo> }       // :45

impl AcServer {
    pub fn new(configs: &[WithInstanceName<AcStoreConfig>],
               store_manager: &StoreManager) -> Result<Self, Error> { /* :56 */ }
    pub fn into_service(self) -> Server<Self> { Server::new(self) }  // :78
}

#[tonic::async_trait]
impl ActionCache for AcServer {                                     // :171
    async fn get_action_result(&self, req: Request<...>) -> Result<Response<...>, Status> { ... }
    async fn update_action_result(&self, req: Request<...>) -> Result<Response<...>, Status> { ... }
}
```

Two conventions worth copying from `AcServer`:

- The RPC methods are thin: they unwrap the request, delegate to an `inner_*` method, and wrap errors. The real work is in `inner_get_action_result` so it's testable without tonic plumbing (`ac_server.rs:184-191`).
- Each RPC carries an `#[instrument(...)]` with `skip_all` and an explicit `fields(request = ?...)` (`ac_server.rs:172-177`) and runs inside an `error_span!` with a hash-function context (`:186-190`). Match this — the observability is not optional in this codebase.

### Step 3 — Config

Add a config struct in `nativelink-config/src/cas_server.rs` (e.g. `AcStoreConfig` at `:115`) and a field on `ServicesConfig` (`:503`). Service fields are `Option<Vec<WithInstanceName<T>>>` (`:511-529`) so a service can be instantiated per instance name; a singleton service like `worker_api` is `Option<WorkerApiConfig>` (`:567`).

### Step 4 — Wire into the server (the forgettable one)

In `src/bin/nativelink.rs`, the router is built with `Routes::builder().routes()` (`:317`) and each service added via `.add_optional_service(...)`. The AC wiring (`:319-327`):

```rust
.add_optional_service(
    services.ac.map_or(Ok(None), |cfg| {
        AcServer::new(&cfg, &store_manager)
            .map(|v| Some(service_setup!(v.into_service(), http_config)))
    }).err_tip(|| "Could not create AC service")?,
)
```

`service_setup!` (`nativelink.rs:168` region) applies compression and message-size limits uniformly — route every service through it. The whole `Routes` is converted to an axum router with `.into_axum_router()` (`:415`). **A service with no `.add_optional_service` arm is unreachable even when its config is present.**

---

## Recipe 5: Add a config option

**Why/when.** Almost every feature above ends here: a new tunable on an existing spec. The discipline is strict because configs are user-facing and `deny_unknown_fields` means a mistake is a hard startup error.

**Exemplar to copy.** The defaulted, shell-expandable fields on existing specs in `nativelink-config/src/stores.rs`.

**Steps.**

1. **Add the field** to the relevant spec struct. Give it a doc comment stating the default, exactly like the existing fields do — the doc comments *are* the config reference. Example from a store spec: `/// Default: false` above `#[serde(default)] pub evict_page_cache: bool` (`stores.rs:845-846`).
2. **Choose the serde attributes:**
   - Plain default: `#[serde(default)]`.
   - A value that should accept `${ENV_VAR}` substitution: add a deserializer. The helpers are in [`serde_utils.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/src/serde_utils.rs) — `convert_numeric_with_shellexpand` (`:30`), `convert_string_with_shellexpand` (`:148`), `convert_boolean_with_shellexpand` (`:155`), and `Option` variants. Usage: `#[serde(default, deserialize_with = "convert_data_size_with_shellexpand")]` (`stores.rs:1036-1037`). Numeric size fields use the data-size variant so `"10mb"` parses.
   - The struct keeps `#[serde(deny_unknown_fields)]` — if you add a field, this is already there; don't remove it.
3. **Consume it.** The constructor that receives the spec reads the field — e.g. `MemoryStore::new` reading `spec.eviction_policy` (`memory_store.rs:80-87`). An `Option` field with a sensible fallback (`unwrap_or`, `unwrap_or_default`) is the idiom; a required field with no default forces every config to set it, so prefer defaults unless absence is genuinely an error.
4. **Keep docs and examples in sync (the forgettable one).** A new option that no example shows is a new option nobody discovers. Update the deployment examples under `deployment-examples/` (the JSON5 configs, e.g. `deployment-examples/docker-compose/*.json5`) and the config reference if the option is meaningful to operators. The doc comment on the field is the minimum; a worked example in a deployment file is the standard.

---

## Recipe 6: Write a good `nativelink_test`

**Why/when.** Every change above needs one. The bar is not "a test exists" — it's "a test that **fails without the fix**." A test that passes before and after your change tests nothing.

**The macro.** `#[nativelink_test]` from [`nativelink-macro`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-macro/src/lib.rs). It expands to more than `#[tokio::test]` (`lib.rs:36-99`):

- wraps the test in `#[tokio::test]` and `#[tracing_test::traced_test]`, so `logs_contain(...)` / `logs_assert(...)` work inside;
- reseeds the RNG per test for determinism (`reseed_rng_for_test()`, `:80`);
- runs the body inside an `error_span!` named after the function;
- **asserts no un-redacted blob data leaked into logs** (`:82-91`) — if your tracing accidentally logs raw bytes (`" data: b..."`), the test fails. This catches logging regressions for free.

You can pass tokio flavors through: `#[nativelink_test(flavor = "multi_thread")]` feeds the attributes to `tokio::test`.

**Fixtures and mocks.** No monolithic `MockStore` exists. The idiom is a small purpose-built fake in the test file that implements `StoreDriver` and records calls. `compression_store_test.rs` defines a `RecordingStore` with `AtomicUsize` counters and `#[derive(MetricsComponent)]` (`compression_store_test.rs:83-95`), then asserts on the counts after exercising the wrapper. For real-store round-trips, construct the actual store (`MemoryStore::new(&MemorySpec::default())`) and use the `StoreLike` convenience methods — `update_oneshot`, `has`, `get_part_unchunked` (`memory_store_test.rs:44-64`).

**The structure of a good test:**

```rust
use nativelink_macro::nativelink_test;

#[nativelink_test]
async fn insert_one_item_then_update() -> Result<(), Error> {
    let store = MemoryStore::new(&MemorySpec::default());
    store.update_oneshot(DigestInfo::try_new(VALID_HASH1, VALUE1.len() as u64)?, VALUE1.into()).await?;
    assert_eq!(
        store.has(DigestInfo::try_new(VALID_HASH1, VALUE1.len() as u64)?).await,
        Ok(Some(VALUE1.len() as u64)),
    );
    Ok(())
}
```

**The fails-without-the-fix discipline.** For a bug fix, write the test first, watch it fail on `main`, then apply the fix and watch it pass. For the zero-digest and ENOENT-race hardening described in the store-trait chapter, the regression tests assert the *specific* broken behavior (a `NotFound` for the zero digest, a self-healing retry under concurrent writes) — behavior that is wrong without the change. A test that only exercises the happy path proves the code runs, not that the bug is fixed. If you can delete your production change and the test still passes, the test is not a regression test.

**For a behavior test, assert on logs.** The `traced_test` wrapper lets you assert a specific flow executed: `assert!(logs_contain("..."))`. This is how scheduler and worker tests pin down ordering that has no return value to check.

---

## Recipe 7: Add a metric

**Why/when.** You want a new counter or gauge to surface in telemetry. NativeLink's metrics are structural: a `MetricsComponent` tree mirrors the object graph, and publishing walks it.

**Exemplar to copy.** Field-level metrics: `AwaitedAction` (`awaited_action.rs:53-96`). A manual counter: `CounterWithTime` (`metrics_utils.rs:281-327`).

**Two kinds of metric:**

1. **A field on a `MetricsComponent`.** This is the common case and it's declarative. Derive `MetricsComponent` on the struct and annotate the field (`awaited_action.rs:53-96`):
   ```rust
   #[derive(Debug, Clone, MetricsComponent)]
   pub struct AwaitedAction {
       #[metric(help = "The version of the AwaitedAction")]
       version: AwaitedActionVersion,
       #[metric(help = "The state of the AwaitedAction")]
       state: Arc<ActionState>,
   }
   ```
   `#[metric(group = "...")]` nests a sub-component (`memory_store.rs:64`); `#[metric(help = "...")]` documents a leaf. The derive is defined in `nativelink-metric/nativelink-metric-macro-derive/src/lib.rs`; the `publish!` / `group!` machinery is in `nativelink-metric/src/lib.rs`.

2. **A counter you increment on a hot path.** Use `CounterWithTime`, which tracks both a count and the last-touched timestamp. Increment with `.inc()` — `self.counter.fetch_add(1, Ordering::Acquire)` plus a timestamp store (`metrics_utils.rs:287-295`). It carries its own hand-written `MetricsComponent::publish` using the `publish!` macro (`metrics_utils.rs:312-323`), so a `CounterWithTime` field on a derived component publishes automatically. OpenTelemetry-native counters are also used directly on hot paths — e.g. `CACHE_METRICS.cache_operations.add(1, self.attrs.read_hit())` in `cache_metrics_store.rs:246`.

**Where metrics surface (the forgettable wiring).** A metric is only published if its component is **reachable from the root of the metrics tree**. Field-level metrics ride the derive: as long as the owning struct is itself a `#[metric]` field of its parent, up to a root `MetricsComponent`, it publishes. This is why `Store` wraps its inner driver as `#[metric] inner: Arc<dyn StoreDriver>` — the whole store chain is one metrics tree. If you add a metric to a struct that nothing publishes (not referenced as a `#[metric]` field anywhere up to the root), it will never appear. Confirm your struct is on the tree, not just that it has the derive. Operationally, metrics are reachable via the admin surface the server mounts (`src/bin/nativelink.rs:430` onward, under the configured admin path).

---

## The one-sentence version of all seven

Implement the trait, add the config variant, add the one match arm that connects them, derive `MetricsComponent`, and write a test that fails without your change. The match arm is the part everyone forgets — `default_store_factory.rs`, `default_scheduler_factory.rs`, and the `.add_optional_service` block in `nativelink.rs` are the three registration sites where code goes to be reachable or to die quietly.

---

### A note on this fork

The core `nativelink-*` crates cited here track upstream `main`. A few scheduler files on this feature branch (`worker_capability_index.rs`, `historical_resource_scheduler.rs`, `worker_registry.rs`, `unsatisfiable_tracker.rs`, and the `SchedulerSpec::HistoricalResource` / `exchange_fleet_capabilities` surfaces) are fork additions and are deliberately **not** used as exemplars above — the recipes stand on upstream-genuine code so they apply to the mainline tree. Where a line number drifts, the symbol names (`store_factory`, `simple_scheduler_factory`, `AwaitedActionDb`, `is_satisfied_by`, `nativelink_test`) are stable anchors.
