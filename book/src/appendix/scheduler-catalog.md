# Appendix B: Scheduler Catalog

Field-by-field configuration reference for every scheduler variant, with
labeled defaults verified against the source. For the runtime semantics —
how matching, timeouts, retries, and the Redis backend actually behave — see
[The Scheduler](../part4/scheduler.md) in Part IV. For the surrounding config
surface (stores, servers, workers) see
[Appendix A: Configuration Reference](./config-reference.md).

**Source:** [`nativelink-config/src/schedulers.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/src/schedulers.rs)

## The five variants

`SchedulerSpec` (`schedulers.rs:30-36`) is a tagged enum with exactly five
variants. The config key is the `snake_case` form of the variant name.

| Variant | Config key | Kind | Nests a `scheduler`? |
|---------|-----------|------|----------------------|
| `Simple` | `simple` | leaf | no |
| `Grpc` | `grpc` | leaf | no |
| `CacheLookup` | `cache_lookup` | wrapper | yes |
| `PropertyModifier` | `property_modifier` | wrapper | yes |
| `HistoricalResource` | `historical_resource` | wrapper | yes |

Two of these are **leaf** schedulers that terminate a stack: `simple` (the
real scheduler that owns the queue and worker pool) and `grpc` (a forwarding
proxy). The other three are **wrappers** that hold a nested `scheduler` and
delegate to it. A production config is usually a short wrapper chain ending in
a `simple`. See [Scheduler Composition](#scheduler-composition) below.

Not every field is shared across variants. Only `simple` matches on platform
properties (`supported_platform_properties`); only the three wrappers carry a
nested `scheduler`; only `simple` has the experimental Redis backend. Do not
copy a field from one variant into another — every scheduler struct is
`#[serde(deny_unknown_fields)]`, so a stray key is a hard load error, not a
silent no-op.

### A note on defaults: `0` usually means "use the default"

For most `SimpleSpec` numeric fields the `serde` default is `0`, and the real
default is substituted later in the scheduler factory (`simple_scheduler.rs`,
`default_scheduler_factory.rs`). The practical consequence is that a literal
`0` in the config is read as "give me the default," so you **cannot** set,
say, `max_job_retries: 0` to disable retries — it is indistinguishable from an
absent field and resolves to `3`. The **effective** defaults are labeled
below. The one field with a genuine non-zero `serde` default is
`worker_match_logging_interval_s` (defaults to `10`; `-1` disables), and
`max_action_executing_timeout_s` is the one field where `0` is meaningful and
means "disabled." Part IV documents the factory substitution with line-level
cites (`part4/scheduler.md`).

## SimpleScheduler (`simple`)

The primary scheduler: owns the action queue, the connected-worker pool, and
the capability index that matches actions to workers (`SimpleSpec`,
`schedulers.rs:89-170`). Every field is optional.

```json5
{
  name: "MAIN_SCHEDULER",
  simple: {
    // Platform properties this scheduler matches on, and how. OPTIONAL — if
    // omitted, the scheduler advertises no properties and matches every worker.
    // Values are "minimum" | "exact" | "priority" | "ignore" (see PropertyType
    // in Appendix A and Part IV's platform-properties chapter). (schedulers.rs:114)
    supported_platform_properties: {
      cpu_count: "minimum",
      memory_kb: "minimum",
      OSFamily: "priority",
      ISA: "exact"
    },

    // Remove a worker from the pool after this many seconds of silence; its
    // in-flight actions are re-queued. Effective default: 5 (schedulers.rs:131)
    worker_timeout_s: 5,

    // Mark an operation completed-with-error if no client updates it within
    // this window. Keep it above the client keepalive interval.
    // Effective default: 60 (schedulers.rs:125)
    client_action_timeout_s: 60,

    // Force-timeout an action stuck in Executing this long without a worker
    // update, even while the worker keeps sending keepalives. 0 disables it,
    // leaving worker_timeout_s as the only backstop.
    // Default: 0 (disabled) (schedulers.rs:141)
    max_action_executing_timeout_s: 0,

    // Retain completed actions this long so a late WaitExecution still resolves.
    // Effective default: 60 (schedulers.rs:119)
    retain_completed_for_s: 60,

    // Give up and return the last error after this many internal-error/timeout
    // retries on workers, so one poisoned action can't retry forever.
    // Effective default: 3 (schedulers.rs:151)
    max_job_retries: 3,

    // Worker preference when several can run a job:
    // "least_recently_used" (default) or "most_recently_used".
    // (WorkerAllocationStrategy, schedulers.rs:70-79, 155)
    allocation_strategy: "least_recently_used",

    // Log worker-matching decisions ("worker busy", "can't find any worker")
    // every N seconds. Default: 10. Set to -1 to disable. (schedulers.rs:82, 165)
    worker_match_logging_interval_s: 10,

    // Where the scheduler keeps its awaited-action state. Omit for in-memory
    // (single instance). See the Redis backend below. (schedulers.rs:160)
    experimental_backend: { memory: {} }
  }
}
```

Loads cleanly: the config parses and every store/scheduler reference resolves at startup.

### The experimental Redis backend

By default the scheduler holds its state in memory, which is correct for a
single instance. To share state across multiple stateless scheduler instances,
point `experimental_backend` at a declared Redis store
(`ExperimentalSimpleSchedulerBackend`, `schedulers.rs:172-189`):

```json5
{
  stores: [
    {
      name: "SCHEDULER_REDIS_STORE",
      redis_store: {
        addresses: ["redis://127.0.0.1:6379"],
        connection_pool_size: 10,
        experimental_pub_sub_channel: "scheduler_key_change"
      }
    }
  ],
  schedulers: [
    {
      name: "MAIN_SCHEDULER",
      simple: {
        supported_platform_properties: { cpu_count: "minimum", ISA: "exact" },
        experimental_backend: {
          redis: {
            redis_store: "SCHEDULER_REDIS_STORE"  // MUST resolve to a RedisSpec
          }
        }
      }
    }
  ],
  servers: []
}
```

Loads cleanly: the config parses and every store/scheduler reference resolves at startup.

The backend is a **store reference**, not an inline connection block.
`experimental_backend.redis.redis_store` is a `StoreRefName` and it must
resolve to a `RedisSpec` store declared in `stores`
(`ExperimentalRedisSchedulerBackend`, `schedulers.rs:182-189`). Every Redis
connection knob — addresses, pool size, pub/sub channel, timeouts — lives on
that store, not on the scheduler. The two variants are `memory` (the default
when the key is omitted; accepted as the bare string `"memory"` or as
`{ memory: {} }`) and `redis`. No `experimental_redis_scheduler_state`
field exists — that name fails to load with
`unknown field 'experimental_redis_scheduler_state'`. A worked HA deployment
lives in
[`nativelink-config/examples/worker_with_redis_scheduler.json5`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/examples/worker_with_redis_scheduler.json5).

**Source:** [`nativelink-scheduler/src/simple_scheduler.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/simple_scheduler.rs)

## GrpcScheduler (`grpc`)

Forwards `Execute` and `WaitExecution` to an upstream NativeLink instance
(`GrpcSpec`, `schedulers.rs:195-218`). Use it when a local instance handles
CAS/AC but delegates execution to a centralized scheduler; pointing the build
at the upstream scheduler directly is usually more efficient.

```json5
{
  name: "REMOTE_SCHEDULER",
  grpc: {
    // The upstream endpoint. Required. NOTE: singular `endpoint` — this is the
    // scheduler GrpcSpec, distinct from the *store* grpc spec which takes a
    // plural `endpoints` list. (schedulers.rs:200)
    endpoint: {
      address: "grpc://scheduler.internal:50051",  // grpc:// or grpcs://; required
      // tls_config: { ca_file, cert_file, key_file, use_native_roots }, // optional
      // concurrency_limit: 0,        // per-endpoint cap; optional
      // connect_timeout_s: 30,       // TCP connect timeout. Default: 30
      // tcp_keepalive_s: 30,         // OS-level keepalive. Default: 30
      // http2_keepalive_interval_s: 30, // HTTP/2 PING interval. Default: 30
      // http2_keepalive_timeout_s: 20   // HTTP/2 PING timeout. Default: 20
    },

    // Cap on simultaneous upstream requests; 0 is unlimited. Over-limit
    // requests queue. Default: 0 (unlimited) (schedulers.rs:210)
    max_concurrent_requests: 0,

    // TCP connections opened per endpoint to spread load. Default: 1 (schedulers.rs:216)
    connections_per_endpoint: 4,

    // Exponential-backoff retry for failed network requests. Default: no
    // retries (max_retries 0, delay 0, jitter 0). (Retry, stores.rs:1589) (schedulers.rs:203)
    retry: { max_retries: 5, delay: 0.1, jitter: 0.5 }
  }
}
```

Loads cleanly: the config parses and every store/scheduler reference resolves at startup.

`endpoint` is a `GrpcEndpoint` (`stores.rs:1273-1308`): only `address` is
required; the four timeout knobs each default as noted, and `tls_config` is
required only for `grpcs://` targets. `GrpcScheduler` does not match on
platform properties and cannot nest another scheduler — it is a terminal
forwarder.

**Source:** [`nativelink-scheduler/src/grpc_scheduler.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/grpc_scheduler.rs)

## CacheLookupScheduler (`cache_lookup`)

A wrapper that checks the Action Cache before dispatching; on a hit it returns
the cached result without involving a worker (`CacheLookupSpec`,
`schedulers.rs:220-231`).

```json5
{
  stores: [
    { name: "CAS", memory: {} },
    {
      name: "AC_STORE",
      completeness_checking: {                // recommended wrapper — see below
        backend: { memory: {} },
        cas_store: { ref_store: { name: "CAS" } }
      }
    }
  ],
  schedulers: [
    {
      name: "CACHED_SCHEDULER",
      cache_lookup: {
        ac_store: "AC_STORE",                 // StoreRefName; required (schedulers.rs:227)
        scheduler: {                          // nested scheduler on a miss; required (schedulers.rs:230)
          simple: { supported_platform_properties: { ISA: "exact" } }
        }
      }
    }
  ],
  servers: []
}
```

Loads cleanly: the config parses and every store/scheduler reference resolves at startup.

`ac_store` is a `StoreRefName` and must resolve to a store declared in
`stores`. The config docs recommend it be a `CompletenessCheckingSpec` so a
partially-evicted result is never served as a hit (`schedulers.rs:226`). Both
fields are required. Use this when clients do not check the AC themselves, or
for server-side deduplication of identical in-flight actions; most build
systems check the AC client-side, so it is mainly for custom clients.

**Source:** [`nativelink-scheduler/src/cache_lookup_scheduler.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/cache_lookup_scheduler.rs)

## PropertyModifierScheduler (`property_modifier`)

A wrapper that rewrites platform properties before forwarding to a nested
scheduler (`PropertyModifierSpec`, `schedulers.rs:271-284`). Modifications
apply in order and blindly: removing a property that does not exist is fine,
and overwriting an existing one is fine.

```json5
{
  name: "MODIFIED_SCHEDULER",
  property_modifier: {
    modifications: [                                        // required, ordered
      { add: { name: "ISA", value: "x86-64" } },           // both fields required
      { remove: "internal-only-property" },                // a bare string
      { replace: {
          name: "container-image",                         // property to match & remove; required
          new_name: "container-image",                     // property to write; REQUIRED
          new_value: "docker://toolchain@sha256:abc123"    // optional new value
      } },
      { replace: { name: "old-arch", new_name: "ISA" } }   // pure rename (keeps value)
    ],
    scheduler: {                                            // nested scheduler; required
      simple: { supported_platform_properties: { ISA: "exact" } }
    }
  }
}
```

Loads cleanly: the config parses and every store/scheduler reference resolves at startup.

The three modification variants (`PropertyModification`,
`schedulers.rs:259-269`):

- **`add`** takes a `PlatformPropertyAddition` (`schedulers.rs:233-241`): both
  `name` and `value` are required.
- **`remove`** takes a bare string — the property name to strip.
- **`replace`** takes a `PlatformPropertyReplacement`
  (`schedulers.rs:243-257`), and its field names do not mean what they look
  like:
  - `name` — the existing property to match and remove. Required.
  - `value` — an **optional match filter.** If set, the replacement fires only
    when the current value equals it; if unset, any value matches. It is *not*
    the new value.
  - `new_name` — the property name to write. **Required.** To keep the same
    key, repeat it. Its absence fails to load with `missing field 'new_name'`.
  - `new_value` — optional; if omitted, the matched property keeps its existing
    value under `new_name` (a pure rename).

A `replace` with only `{ name, value }` is therefore invalid — that shorthand was a
fabrication in earlier drafts. Applied in
`property_modifier_scheduler.rs:111-131`.

**Source:** [`nativelink-scheduler/src/property_modifier_scheduler.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/property_modifier_scheduler.rs)

## HistoricalResourceScheduler (`historical_resource`)

A scheduler wrapper that injects **minimum** CPU and memory platform
properties from a hints file before delegating to a nested scheduler, so
oversized actions land on workers large enough to run them without the client
annotating every target (`HistoricalResourceSpec`, `schedulers.rs:298-344`).

```json5
{
  name: "SIZED_SCHEDULER",
  historical_resource: {
    // JSON file of resource hints. Required. Shell-expanded. (schedulers.rs:315)
    hints_file: "/etc/nativelink/resource-hints.json",

    // Reload cadence for hints_file. 0 loads once at startup.
    // Default: 30 seconds (schedulers.rs:286, 320)
    refresh_interval_s: 30,

    // Platform property raised for the CPU floor. Default: "cpu_count" (schedulers.rs:290, 328)
    cpu_property_name: "cpu_count",

    // Platform property raised for the memory floor, in KiB. Default: "memory_kb" (schedulers.rs:294, 336)
    memory_property_name: "memory_kb",

    // Nested scheduler applied after the hint is written. Required. (schedulers.rs:343)
    scheduler: {
      simple: {
        supported_platform_properties: {
          cpu_count: "minimum",
          memory_kb: "minimum"
        }
      }
    }
  }
}
```

Loads cleanly: the config parses and every store/scheduler reference resolves at startup.

Behavior (`historical_resource_scheduler.rs`):

- `hints_file` accepts a bare array or a `{ "hints": [ ... ] }` object. Each
  hint carries optional `target_id`, `action_mnemonic`, `cpu_count`,
  `memory_kb`, and `memory_mib`; `memory_mib` is folded into `memory_kb` by
  multiplying by 1024 (`historical_resource_scheduler.rs:71-74`).

  ```json
  [
    { "target_id": "//pkg:test", "action_mnemonic": "TestRunner", "cpu_count": 2, "memory_kb": 12582912 }
  ]
  ```

- For each action the scheduler reads the Bazel `RequestMetadata` from the
  request and looks up the most specific hint — exact `target_id` +
  `action_mnemonic` first, then `target_id` alone, then `action_mnemonic`
  alone (`historical_resource_scheduler.rs:187-206`).
- A matching hint applies as a **floor**: it only raises `cpu_property_name` /
  `memory_property_name` when the value is absent or lower than the hint, never
  lowering an explicit request.
- A missing or malformed `hints_file` logs a warning and forwards the action
  unchanged. The nested `scheduler` must declare the same property names as
  `minimum` for the floor to filter workers — the wrapper writes the property,
  the `simple` scheduler enforces it.

**Source:** [`nativelink-scheduler/src/historical_resource_scheduler.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/historical_resource_scheduler.rs)

## Scheduler Composition

Wrappers nest by holding a `scheduler`, so a real deployment is a short stack
ending in a `simple` leaf. A full production stack:

```json5
{
  stores: [
    { name: "CAS", memory: {} },
    { name: "AC_STORE", completeness_checking: { backend: { memory: {} }, cas_store: { ref_store: { name: "CAS" } } } },
    { name: "SCHEDULER_REDIS_STORE", redis_store: { addresses: ["redis://127.0.0.1:6379"] } }
  ],
  schedulers: [
    {
      name: "PROD_SCHEDULER",
      cache_lookup: {                          // 1. AC dedup at the top
        ac_store: "AC_STORE",
        scheduler: {
          property_modifier: {                 // 2. normalize properties
            modifications: [ { add: { name: "ISA", value: "x86-64" } } ],
            scheduler: {
              historical_resource: {           // 3. apply size hints
                hints_file: "/etc/nativelink/resource-hints.json",
                scheduler: {
                  simple: {                     // 4. the real scheduler (Redis-backed)
                    supported_platform_properties: { cpu_count: "minimum", memory_kb: "minimum", ISA: "exact" },
                    experimental_backend: { redis: { redis_store: "SCHEDULER_REDIS_STORE" } }
                  }
                }
              }
            }
          }
        }
      }
    }
  ],
  servers: []
}
```

Loads cleanly: the config parses and every store/scheduler reference resolves at startup.

Reading top to bottom: AC deduplication, then property normalization, then
size hints, then the Redis-backed `simple` scheduler that owns matching and the
worker pool. Simpler common shapes:

```text
CacheLookupScheduler                 PropertyModifierScheduler
  └── SimpleScheduler (Redis)          └── SimpleScheduler

CacheLookupScheduler (local AC)      HistoricalResourceScheduler
  └── GrpcScheduler (remote exec)      └── SimpleScheduler
```

## Validate before you ship

Scheduler blocks are `#[serde(deny_unknown_fields)]`, so a typo or a stale
field name is a hard error. Extract the scheduler block into a config with the
stores it references and boot the binary against it in CI (build it with
`cargo build -p nativelink --profile=smol` if the binary is missing):

```console
$ nativelink my-config.json5
```

On a bad config the process exits non-zero before it serves a request. Startup
parses every block, enforces `deny_unknown_fields`, and resolves
store references, so it catches both the retired
`experimental_redis_scheduler_state` (as `unknown field ...`) and a `replace`
missing its required `new_name` (as `missing field 'new_name'`) before a worker
ever connects. Every config in this appendix loads cleanly.

For the matching vocabulary these schedulers filter on, see
[Platform Properties](../part4/platform-properties.md); for the full config
surface around them, see
[Appendix A: Configuration Reference](./config-reference.md).
