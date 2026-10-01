# Platform Properties

Platform properties are how actions find workers. An action declares what it needs (`OSFamily: Linux`, `cpu_count: 4`). A worker declares what it has (`OSFamily: Linux`, `cpu_count: 8`). The scheduler matches them.

This sounds simple. In practice, it's where most deployments go wrong.

## The Matching Model

You declare a type for each property in the scheduler config. That type controls how the matcher treats the property:

```rust
// nativelink-config/src/schedulers.rs

#[serde(rename_all = "snake_case")]
pub enum PropertyType {
    Minimum,  // u64 — worker value must be >= requested; consumed on assignment
    Exact,    // string — worker value must equal requested exactly
    Priority, // string — worker must have the key; its value is never compared
    Ignore,   // worker need not have the key at all; skipped entirely
}
```

**Source:** [`nativelink-config/src/schedulers.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/src/schedulers.rs) (the `PropertyType` enum). The matcher that acts on these types lives in [`nativelink-util/src/platform_properties.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-util/src/platform_properties.rs) — `PlatformProperties::is_satisfied_by`.

You declare these types in the scheduler config:

```json5
supported_platform_properties: {
  cpu_count: "minimum",
  memory_mb: "minimum",
  ISA: "exact",
  "toolchain-version": "exact",
  OSFamily: "priority",
  "container-image": "priority",
  InputRootAbsolutePath: "ignore"
}
```

The matcher walks the action's requested properties one at a time and decides whether a candidate worker satisfies each. The behavior splits cleanly along two questions — *must the worker advertise this key?* and *is the value compared?*

| Type | Worker must advertise the key? | Value comparison | Consumed on assignment? |
|------|-------------------------------|------------------|-------------------------|
| `minimum` | yes | worker value `>=` requested (numeric `u64`) | yes — subtracted while the action runs |
| `exact` | yes | worker value `==` requested (string) | no |
| `priority` | **yes** | none — value passed through, never compared | no |
| `ignore` | **no** | none — property skipped before any check | no |

The load-bearing distinction is the middle two rows. `priority` and `ignore` both skip the value comparison, but they are not interchangeable: `priority` still **requires the key to be present** on the worker, while `ignore` requires nothing at all. Getting this backwards is the single most common platform-property mistake, so each type gets its own section below.

### Minimum

Numeric comparison. The action requests a floor; the worker must meet or exceed it (`platform_properties.rs:141-146`).

Action: `cpu_count: 4` → Worker with `cpu_count: 8` → **match**
Action: `cpu_count: 16` → Worker with `cpu_count: 8` → **no match**

Minimum properties are *consumable resources*. When the scheduler assigns an action to a worker, it subtracts the requested amount from the worker's advertised value, so the same worker can run several actions concurrently only while its remaining budget still satisfies each one (`nativelink-scheduler/src/worker.rs:123-138`, `reduce_platform_properties`). This is why `minimum` values are re-checked at assignment time rather than cached in the index — a worker's available `cpu_count` shrinks as jobs land on it.

Workers determine their values dynamically or statically:

```json5
platform_properties: {
  cpu_count: { query_cmd: "nproc" },     // dynamic: runs at worker startup
  memory_mb: { values: ["32768"] }        // static: fixed value
}
```

### Exact

String equality. The worker must advertise the key with the exact same value (`platform_properties.rs:152` — the equal case is handled up front at line 137).

Action: `ISA: x86-64` → Worker with `ISA: x86-64` → **match**
Action: `ISA: aarch64` → Worker with `ISA: x86-64` → **no match**

### Priority

The worker **must advertise the key**, but its value is never compared. The requested value is passed through to the worker as information; any worker that has the key present is eligible, whatever value it holds (`platform_properties.rs:150` returns `true` for `Priority` once presence is confirmed).

This is the type people misread. `priority` is **not** "match any worker." A worker that never advertises `container-image` will **not** match an action that requests `container-image` as a `priority` property — the matcher returns no-match when the key is absent (`platform_properties.rs:70-75`). The source comment on the enum spells it out: "Priority — Means the worker is given this information, but does not restrict what workers can take this value. However, the worker must have the associated key present to be matched." (`platform_properties.rs:115-117`).

If you actually want "clients may request this key, but workers need not have it," that is `ignore`, not `priority`.

`priority` is the right type for `container-image`: the value tells the worker *what image to use* but must not restrict which workers can accept the action. Every worker still advertises the key (commonly with an empty value) so that presence check passes, and the worker's entrypoint script reads the requested value and acts on it.

### Ignore

The only type that does not require the worker to advertise the key. The action may request it; the matcher skips it entirely before any presence or value check (`platform_properties.rs:49-51` — `continue`, with the comment "always matches"). Use this for client-side metadata that has no server-side meaning.

The canonical case is `InputRootAbsolutePath`, which Chromium builds send but NativeLink workers can safely disregard (`platform_properties.rs:121-123`). Declaring it `ignore` lets those requests through without forcing every worker config to advertise the key.

## How a Worker Gets Chosen

Matching runs in two stages, for speed and for correctness.

**Stage one — the capability index.** The `WorkerCapabilityIndex` is an inverted index that narrows thousands of workers to a candidate set without scanning them all:

**Source:** [`nativelink-scheduler/src/worker_capability_index.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/worker_capability_index.rs)

```text
For an action with properties [ISA=x86-64 (exact), cpu_count=4 (minimum),
                               container-image=… (priority), foo=… (ignore)]:

1. ISA=x86-64          → look up the exact (name, value) key in exact_index
2. cpu_count           → look up workers that HAVE the cpu_count key (presence only)
3. container-image     → look up workers that HAVE the container-image key (presence only)
4. foo (ignore)        → skipped; contributes no constraint
5. intersect the resulting worker sets
```

`Exact` and `Unknown` properties resolve through the exact `(name, value)` index (`worker_capability_index.rs:153-183`). `Priority` and `Minimum` resolve through a presence index — the key must exist on the worker, but the value is not compared here (`worker_capability_index.rs:184-212`). `Ignore` contributes nothing (`worker_capability_index.rs:213`). An `Unknown` value is one that reached the matcher without a declared type; it is matched by exact string equality, and in the normal scheduler path an undeclared key is rejected before this point (see gotcha 1 below).

This gives roughly `O(P × log(W))` matching where `P` is the number of properties and `W` is the number of workers — fast even with thousands of workers (`worker_capability_index.rs:21-28`).

**Stage two — the authoritative recheck.** The index returns *candidates*. The scheduler then confirms each candidate with `PlatformProperties::is_satisfied_by` before dispatching (`nativelink-scheduler/src/api_worker_scheduler.rs:258-292`). This second pass is where dynamic `minimum` values are actually compared — the index only knows the key is present, but the worker's *available* budget changes as jobs are assigned, so the true `>=` check has to happen against live state. It also confirms the worker can accept more work at all.

## Common Patterns

### The Container Image Pattern

Workers accept any action and use the `container-image` property to select the execution environment:

```json5
// Scheduler config
supported_platform_properties: {
  "container-image": "priority",  // presence required; value not compared
  OSFamily: "priority",
  cpu_count: "minimum"
}

// Worker config
platform_properties: {
  cpu_count: { query_cmd: "nproc" },
  OSFamily: { values: ["Linux"] },
  "container-image": { values: [""] }  // advertises the key so the presence check passes
}
```

The worker's entrypoint script reads `container-image` from the action's environment (via `EnvironmentSource::Property`, covered below) and launches the action inside that container.

This pattern pulls the image *on the worker*, at execution time.

### The Pool Pattern

Separate worker pools for different workloads, matched by exact properties:

```json5
// Scheduler config
supported_platform_properties: {
  "pool": "exact",
  cpu_count: "minimum"
}

// Fast-compile workers
platform_properties: {
  pool: { values: ["compile"] },
  cpu_count: { query_cmd: "nproc" }
}

// Test workers (more memory, less CPU)
platform_properties: {
  pool: { values: ["test"] },
  cpu_count: { values: ["2"] }
}
```

Clients set `--remote_default_exec_properties=pool=compile` or `pool=test` to route actions. Because `pool` is `exact`, an action requesting `pool=compile` will never land on a `pool=test` worker — the value comparison enforces the split.

### The Nix/LRE Pattern

Workers advertise Nix store paths as platform properties:

```json5
supported_platform_properties: {
  "lre-cc": "exact",
  "lre-rs": "priority",
  OSFamily: "priority"
}

platform_properties: {
  "lre-cc": { values: ["/nix/store/abc123-clang-17/bin/clang"] },
  "lre-rs": { values: ["/nix/store/def456-rust-1.75/bin/rustc"] },
  OSFamily: { values: ["Linux"] }
}
```

The toolchain path is content-addressed by Nix: a `/nix/store/<hash>-clang-17.../bin/clang` path *is* a byte-identical compiler. Change the toolchain and the hash changes with it.

The correctness guarantee here comes from the **action digest**, not from the match type. The Nix store path is a `Platform` property, and the platform is part of the action the client hashes — so a mismatched toolchain produces a different action digest and therefore a different cache key. You cannot get a false cache hit from the wrong toolchain, whatever type you declare the property.

The match type controls *routing*, which is a separate decision:

- Declare the toolchain property **`exact`** (as with `lre-cc` above) when you run mixed pools and want scheduling to refuse a worker that lacks the exact path. A stale action then gets "no matching worker" rather than running on the wrong toolchain.
- Declare it **`priority`** when the toolchain is baked into every worker (for example an LRE Nix closure) and the pool is uniform. Value-based routing buys nothing there, so the property is informational and the workers advertise it with an empty value.

## Environment Injection

Workers can inject values into the action's environment, making them available to entrypoint scripts. The map is keyed by environment-variable name, and each value is an `EnvironmentSource`:

```json5
// Worker config (nativelink-config/src/cas_server.rs)
additional_environment: {
  CONTAINER_IMAGE: { property: "container-image" },  // value from a platform property
  RAW_VALUE: { value: "something" },                 // a literal string
  INHERITED: "from_environment",                     // inherit from the worker's env
  ACTION_TIMEOUT: "timeout_millis",                  // the action's timeout in ms
  SIDE_CHANNEL: "side_channel_file",                 // path to the side-channel file
  WORK_DIR: "action_directory"                       // path to the action's scratch dir
}
```

**Source:** [`nativelink-config/src/cas_server.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/src/cas_server.rs) — the `EnvironmentSource` enum (`cas_server.rs:992-1038`).

Watch the two shapes. `Property` and `Value` carry a string, so they are written as a single-key map: `{ property: "container-image" }`, `{ value: "something" }`. The other four variants — `FromEnvironment`, `TimeoutMillis`, `SideChannelFile`, `ActionDirectory` — carry no data, so under `#[serde(rename_all = "snake_case")]` they are **bare strings**: `"from_environment"`, `"timeout_millis"`, `"side_channel_file"`, `"action_directory"`. Writing one of them as `{ timeout_millis: {} }` fails validation with `invalid type: map, expected unit`; the config is rejected because `additional_environment` uses `deny_unknown_fields` semantics for the whole tree. The shipped `nativelink-config/examples/basic_cas.json5:79-87` uses exactly these bare-string forms.

This is how the `container-image` pattern works end to end: the platform property value flows from the action, through the scheduler, into the worker's environment as `CONTAINER_IMAGE`, and the entrypoint script uses it to `docker run` the right image.

## Gotchas

1. **Unknown properties fail the request.** If a client sends a property not listed in `supported_platform_properties`, the action is rejected with `Unknown platform property` (`nativelink-scheduler/src/platform_property_manager.rs:78-95`). Add `"new-property": "ignore"` *before* clients start sending it.

2. **Empty string values are about presence, not wildcards.** A worker with `container-image: { values: [""] }` advertises the *key* with an empty value. What that matches depends on the type. Under `priority`, the value is never compared, so this worker matches any `container-image` request (including `docker://something`) purely because the key is present. Under `exact`, the same worker matches only an action that requests `container-image: ""` (or omits it entirely). Use `priority` for pass-through properties; reach for the empty value only to make the presence check pass.

3. **Dynamic queries run once, at worker connect.** `query_cmd: "nproc"` runs when the worker registers with the scheduler, not per action (`nativelink-worker/src/worker_utils.rs:29-108`; the command is split with `shlex` and executed directly, not through a shell). If a worker's resources change while it is running, the scheduler will not learn the new value until the worker reconnects.
