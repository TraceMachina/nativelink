# The Scheduler

The scheduler receives `Execute` RPCs from clients and dispatches actions to workers. It is the brain of the execution system — it decides who runs what, when, and how to handle failures.

Every scheduler type is a variant of `SchedulerSpec` (`nativelink-config/src/schedulers.rs:30-36`): `Simple`, `Grpc`, `CacheLookup`, `PropertyModifier`, and `HistoricalResource`. The wrappers nest, so a real deployment is usually a small stack of them ending in a `SimpleScheduler`.

## SimpleScheduler

The primary scheduler implementation. Despite the name, it handles production workloads.

**Source:** [`nativelink-scheduler/src/simple_scheduler.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/simple_scheduler.rs)

The SimpleScheduler maintains:
- A **queue** of pending actions (sorted by priority and insertion time)
- A **worker pool** of connected workers with their declared capabilities
- A **capability index** for O(P × log(W)) matching of actions to workers

When an action arrives:
1. Scheduler checks the capability index for workers matching the action's platform properties
2. If a matching worker is idle, dispatch immediately
3. If no matching worker is idle, enqueue the action
4. When a worker completes an action, scheduler checks the queue for the next matching action

### Redis Backend

By default the scheduler keeps its state in memory, which is fine for a single instance. For multiple scheduler instances that share state (stateless horizontal scaling), the SimpleScheduler can put its awaited-action database in Redis by pointing `experimental_backend` at a declared Redis store:

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
        supported_platform_properties: {
          cpu_count: "minimum",
          OSFamily: "priority",
          ISA: "exact"
        },
        experimental_backend: {
          redis: {
            redis_store: "SCHEDULER_REDIS_STORE"
          }
        }
      }
    }
  ]
}
```

Two things this config makes explicit that copy-paste guides often get wrong:

- The backend is a **store reference**, not an inline connection block. `experimental_backend.redis.redis_store` is a `StoreRefName` and it **must** resolve to a `RedisSpec` store declared in `stores` (`schedulers.rs:175-189`). All the Redis connection knobs — addresses, pool size, pub/sub channel, timeouts — live on that store (`RedisSpec`, `stores.rs:1413-1544`), not on the scheduler.
- The feature is spelled `experimental_backend` and it is genuinely experimental. The only two variants are `memory` (the default when the key is omitted) and `redis` (`ExperimentalSimpleSchedulerBackend`, `schedulers.rs:175-180`).

Omit `experimental_backend` entirely and the scheduler uses the in-memory backend — single instance, no shared state.

### Tuning knobs

Every `SimpleSpec` field is optional and defaults are applied in the scheduler factory, not by `serde`, so a `0` in the config means "use the default" for most of them. The full surface (`schedulers.rs:89-170`):

```json5
{
  name: "MAIN_SCHEDULER",
  simple: {
    supported_platform_properties: {
      cpu_count: "minimum",
      memory_kb: "minimum",
      OSFamily: "priority",
      ISA: "exact"
    },

    // Remove a worker from the pool after this many seconds of silence.
    // Default: 5  (simple_scheduler.rs:58)
    worker_timeout_s: 5,

    // Mark an operation completed-with-error if no client updates it within
    // this window. Set it above the client keepalive interval or the scheduler
    // warns and clamps behaviour. Default: 60  (simple_scheduler.rs:63)
    client_action_timeout_s: 60,

    // Force-timeout an action stuck in Executing this long without a worker
    // update, even if the worker is still sending keepalives. 0 disables it and
    // leaves worker_timeout_s as the only backstop. Default: 0 (disabled)
    max_action_executing_timeout_s: 0,

    // Retain completed actions this long so a late WaitExecution still resolves.
    // Default: 60  (default_scheduler_factory.rs:42)
    retain_completed_for_s: 60,

    // Give up and return the last error after this many internal-error/timeout
    // retries, so one poisoned action can't retry forever. Default: 3
    max_job_retries: 3,

    // Worker preference when several can run a job: "least_recently_used"
    // (default) or "most_recently_used". (WorkerAllocationStrategy)
    allocation_strategy: "least_recently_used",

    // Log worker-matching decisions every N seconds; -1 disables. Default: 10
    worker_match_logging_interval_s: 10
  }
}
```

The defaults above are the values the factory substitutes when the field is `0` or absent (`simple_scheduler.rs:483-505`, `default_scheduler_factory.rs:193-194`); they are not what `serde` deserializes. That distinction matters because it means you cannot set, say, `max_job_retries: 0` to disable retries — a `0` is read as "give me the default of 3."

## GrpcScheduler

A forwarding scheduler that proxies `Execute` and `WaitExecution` calls to a remote NativeLink instance.

```json5
{
  name: "REMOTE_SCHEDULER",
  grpc: {
    endpoint: { address: "grpc://scheduler.internal:50051" },
    connections_per_endpoint: 4
  }
}
```

**Use when:** You have a centralized scheduler and want local NativeLink instances (e.g., in CI runners) to forward execution requests to it.

## CacheLookupScheduler

A scheduler wrapper that checks the Action Cache before dispatching. If the AC already has a result for the action, it returns immediately without involving a worker.

```json5
{
  name: "CACHED_SCHEDULER",
  cache_lookup: {
    ac_store: "AC_STORE",
    scheduler: {
      simple: {
        supported_platform_properties: { ISA: "exact" }
      }
    }
  }
}
```

`ac_store` is a `StoreRefName` and must resolve to a store you declared in `stores`; the config docs recommend it be a `CompletenessCheckingSpec` so a partially-evicted result is not served as a hit (`schedulers.rs:223-231`).

This is useful when clients don't check the AC themselves (or when you want server-side deduplication of in-flight identical actions). Most build systems check the AC client-side, so this is mainly relevant for custom clients or pipelines.

## PropertyModifierScheduler

A wrapper that mutates platform properties before forwarding to a nested scheduler. Useful for:
- Adding default properties that clients don't set
- Removing properties that are irrelevant to worker matching
- Renaming or normalizing a property before matching

```json5
{
  name: "MODIFIED_SCHEDULER",
  property_modifier: {
    modifications: [
      { add: { name: "ISA", value: "x86-64" } },
      { remove: "internal-only-property" },
      {
        replace: {
          name: "container-image",
          new_name: "container-image",
          new_value: "docker://toolchain@sha256:abc123"
        }
      }
    ],
    scheduler: {
      simple: {
        supported_platform_properties: { ISA: "exact" }
      }
    }
  }
}
```

The `replace` modification is the one people get wrong, because its field names do not mean what they look like (`PlatformPropertyReplacement`, `schedulers.rs:243-257`; applied at `property_modifier_scheduler.rs:111-131`):

- `name` — the existing property to match and remove.
- `value` — **optional match filter.** If set, the replacement only fires when the current value equals it; if unset, any value matches. It is *not* the new value.
- `new_name` — **required.** The property name to write. To keep the same key, repeat it (as above). This is the field the fabricated `{ replace: { name, value } }` form was missing, and its absence is a hard config error.
- `new_value` — optional; if omitted, the matched property keeps its existing value under `new_name` (a pure rename).

So the example above rewrites `container-image` (any value) to a resolved digest. To rename a key while preserving its value, drop `new_value`: `{ replace: { name: "old-arch", new_name: "ISA" } }`.

**Source:** [`nativelink-scheduler/src/property_modifier_scheduler.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-scheduler/src/property_modifier_scheduler.rs)

## HistoricalResourceScheduler

A scheduler wrapper that injects **minimum** `cpu_count` / `memory_kb` platform properties from a historical hints file, so oversized actions land on workers big enough to run them without the client having to annotate every target.

```json5
{
  name: "SIZED_SCHEDULER",
  historical_resource: {
    hints_file: "/etc/nativelink/resource-hints.json",
    refresh_interval_s: 30,
    cpu_property_name: "cpu_count",
    memory_property_name: "memory_kb",
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

How it works (`historical_resource_scheduler.rs`):

- `hints_file` is a JSON file of hints keyed by Bazel `RequestMetadata` `target_id` and/or `action_mnemonic`, read from the request baggage. Two shapes are accepted — a bare array, or `{ "hints": [ ... ] }` — and each hint carries optional `cpu_count`, `memory_kb`, and `memory_mib` (`schedulers.rs:298-344`):

  ```json
  [
    { "target_id": "//pkg:test", "action_mnemonic": "TestRunner", "cpu_count": 2, "memory_kb": 12582912 }
  ]
  ```

- For each action the scheduler looks up the most specific hint — exact `target_id` + `action_mnemonic` first, then `target_id` alone, then `action_mnemonic` alone (`historical_resource_scheduler.rs:187-206`).
- A matching hint applies as a **floor**: it only raises `cpu_property_name` / `memory_property_name` when the value is absent or lower than the hint, never lowering an explicit request (`apply_minimum_platform_property`, `historical_resource_scheduler.rs:256-271`). The property names are configurable and default to `cpu_count` and `memory_kb`.
- `refresh_interval_s` reloads the file on that cadence (default 30s); set it to `0` to load once at startup. A missing or malformed file logs a warning and the scheduler forwards the action unchanged.

The nested `scheduler` must declare the same property names as `minimum` for the floor to have any effect — the wrapper writes the properties, but it is the underlying `SimpleScheduler` that filters workers on them.

## Scheduler Composition

Like stores, schedulers compose. A typical production setup:

```
CacheLookupScheduler
  └── PropertyModifierScheduler
        └── SimpleScheduler (with Redis backend)
```

This gives you: AC dedup at the top, property normalization in the middle, and persistent multi-instance scheduling at the bottom. Add a `HistoricalResourceScheduler` above the `SimpleScheduler` when you want size hints applied after normalization but before matching.

## Timeouts and Failure

The scheduler handles several failure modes, each governed by a `SimpleSpec` field:

- **Worker disconnect / silence.** A worker that stops responding for `worker_timeout_s` (default 5) is dropped from the pool and its in-flight actions are re-queued for another worker (`schedulers.rs:128-132`).
- **Stuck-but-alive worker.** A worker can keep sending keepalives while wedged on one action. `max_action_executing_timeout_s` (default 0, disabled) is the backstop: an action in `Executing` longer than this, with no worker update, is timed out and re-queued regardless of keepalive status (`schedulers.rs:134-142`).
- **Idle client.** `client_action_timeout_s` (default 60) marks an operation completed-with-error if no client has updated it within the window. Keep it above the client keepalive interval; the scheduler warns and clamps otherwise (`simple_scheduler.rs:488-501`).
- **Poisoned action.** After `max_job_retries` (default 3) internal errors or timeouts on workers, the scheduler stops retrying and returns the last error to the client, so one rogue action cannot retry forever (`schedulers.rs:144-152`).

```json5
{
  name: "MAIN_SCHEDULER",
  simple: {
    supported_platform_properties: { ISA: "exact" },
    worker_timeout_s: 5,               // drop silent workers after 5s
    max_action_executing_timeout_s: 0, // no stuck-worker backstop (default)
    client_action_timeout_s: 60,       // fail idle operations after 60s
    max_job_retries: 3                 // give up after 3 failed attempts
  }
}
```

## Validate before you ship

Scheduler blocks use `deny_unknown_fields`, so a typo or a stale field name is a hard error, not a silent no-op. Boot the binary against any config in CI before deploying it:

```console
$ nativelink my-config.json5
```

On a bad config the process exits non-zero before it serves a request. Startup parses every block, enforces `deny_unknown_fields`, and resolves store references — so it catches both the misspelled Redis backend (`unknown field 'experimental_redis_scheduler_state'`) and a `replace` missing its required `new_name` (`missing field 'new_name'`) before a worker ever connects. Every config in this chapter loads cleanly.

For the full field-by-field reference of each scheduler variant, see [Appendix B: Scheduler Catalog](../appendix/scheduler-catalog.md). With scheduling settled, the next chapter takes up [platform properties](./platform-properties.md) — the `minimum` / `exact` / `priority` matching vocabulary these schedulers filter workers on.
