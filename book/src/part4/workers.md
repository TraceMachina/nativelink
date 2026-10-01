# Workers

Workers are the execution engines. They receive dispatched actions from the scheduler, fetch inputs from CAS, run commands, upload outputs, and report results. They are stateless and disposable — scale them up, kill them, replace them.

Everything here is upstream NativeLink behaviour (credit to [TraceMachina/nativelink](https://github.com/TraceMachina/nativelink)); this chapter documents the shipped code, not an aspiration. Every config field is checked against `nativelink-config/src/cas_server.rs`, and every worker config block below is accepted by the config loader.

## Lifecycle of an Action

The worker's run loop is one long-lived task per worker (`nativelink-worker/src/local_worker.rs`). When it receives an `Update::StartAction` on its stream from the scheduler (`local_worker.rs:271`), it runs the following sequence (`local_worker.rs:326-364`):

1. **Precondition gate.** If `experimental_precondition_script` is set, run it. A non-zero exit pauses the worker before the action starts (`preconditions_met`, `local_worker.rs:326`).
2. **Fetch inputs.** Download the input `Directory` tree from CAS into the sandbox. NativeLink hardlinks from a local CAS cache when possible instead of copying (`prepare_action`).
3. **Run command.** Execute the action's command via the configured `entrypoint`, with injected environment variables (`RunningAction::execute`).
4. **Signal completion.** Send `ExecuteComplete` to the scheduler (`local_worker.rs:345`). This tells the scheduler the command finished so it can dispatch the next action to this worker *while outputs are still uploading*.
5. **Upload outputs.** Upload output blobs to CAS, compute digests for the `ActionResult`, and — if `upload_action_result.ac_store` is set — publish the `ActionResult` to the Action Cache (`upload_results`).
6. **Cleanup.** Tear down the sandbox (`cleanup`), which runs even if a prior step failed (`local_worker.rs:357-363`).
7. **Report result.** Send `ExecuteResult` back to the scheduler carrying the `ExecuteResponse` and worker-observed `ActionResourceUsage` (`local_worker.rs:393`).

**Source:** [`nativelink-worker/src/running_actions_manager.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-worker/src/running_actions_manager.rs), [`nativelink-worker/src/local_worker.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-worker/src/local_worker.rs)

## The Worker API

Workers do **not** talk to the scheduler over a handful of separate request/response RPCs. The entire `WorkerApi` service is a **single bidirectional streaming RPC** (`worker_api.proto:31-41`):

```proto
service WorkerApi {
    rpc ConnectWorker(stream UpdateForScheduler) returns (stream UpdateForWorker);
}
```

The comment on the proto spells out why it is one stream: it "ensure[s] that the worker is always talking to the same scheduler instance even if there's a load balancer in front" (`worker_api.proto:35-36`). One long-lived stream pins the worker to one scheduler; there is no separate `ConnectWorker` / `GoingAway` / `KeepAlive` / `ExecutionResponse` RPC to call.

The two directions are `oneof` envelopes. **Worker → scheduler** (`UpdateForScheduler`, `worker_api.proto:174-207`):

| Variant | Meaning |
| --- | --- |
| `connect_worker_request` | First message on the stream. Declares supported `properties`, a `worker_id_prefix`, and `max_inflight_tasks`. |
| `keep_alive_request` | Heartbeat so the scheduler does not time the worker out. |
| `going_away_request` | The worker is going offline; stop issuing new actions. It may still send an `ExecuteResult` afterward. |
| `execute_result` | The final result of one action (an `ExecuteResponse` or an internal `Status`), plus `ActionResourceUsage`. |
| `execute_complete` | Execution finished but the result is still uploading. |

**Scheduler → worker** (`UpdateForWorker`, `worker_api.proto:145-171`):

| Variant | Meaning |
| --- | --- |
| `connection_result` | First response. Carries the `worker_id` the worker stamps into its action results. |
| `keep_alive` | Scheduler-side heartbeat. |
| `start_action` | A `StartExecute` — the action to run, its operation ID, queued timestamp, and reserved platform. |
| `disconnect` | The worker has been removed from the pool; it may discard outstanding work. |
| `kill_operation_request` | Kill one running operation by operation ID. |

No `ExecutionResponse` message exists anywhere in the protocol — the real result variant is `ExecuteResult` (`worker_api.proto:81-106`). On the Rust side, `WorkerApiClientWrapper` exposes convenience methods named `keep_alive`, `going_away`, `execution_response`, and `execution_complete`, but each one just pushes an `UpdateForScheduler` message onto the *same* `ConnectWorker` stream via `send_update` (`worker_api_client_wrapper.rs:121-135`). They are ergonomics, not wire RPCs.

The worker API endpoint is configured separately from client-facing services and **must be served on its own listener** — workers hold a different permission set than cache/execution clients, so co-hosting them is a security risk (`cas_server.rs:754-760`).

**Source:** [`worker_api.proto`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-proto/com/github/trace_machina/nativelink/remote_execution/worker_api.proto), [`nativelink-service/src/worker_api_server.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-service/src/worker_api_server.rs)

## Configuration

A worker is a `WorkerConfig` — a tagged enum whose only variant is `local` (`LocalWorkerConfig`, `cas_server.rs:1104`). This block is lifted from `nativelink-config/examples/basic_cas.json5` and augmented with the optional fields; it loads cleanly as part of a full config:

```json5
workers: [
  {
    local: {
      name: "worker-1",                          // optional; defaults to the worker's index
      worker_api_endpoint: {
        uri: "grpc://127.0.0.1:50061",
      },
      cas_fast_slow_store: "WORKER_FAST_SLOW_STORE",
      upload_action_result: {
        ac_store: "AC_MAIN_STORE",
        upload_ac_results_strategy: "success_only",
      },
      work_directory: "/tmp/nativelink/work",
      entrypoint: "/usr/local/bin/worker-entrypoint.sh",
      experimental_precondition_script: "/usr/local/bin/check-resources.sh",
      max_action_timeout_s: 1200,                // reject actions asking for more. Default: 1200 (20m)
      max_upload_timeout_s: 600,                 // result-upload deadline. Default: 600 (10m)
      max_inflight_tasks: 8,                     // 0 = unlimited. Default: 0
      timeout_handled_externally: false,
      use_namespaces: true,                      // Linux only. Default: false
      use_mount_namespace: true,                 // Linux only, requires use_namespaces. Default: false
      directory_cache: {
        max_entries: 10000,                      // Default: 1000
        max_size_bytes: "10gb",                  // 0 = unlimited. Default: 10 GB
        cache_root: "/tmp/nativelink/directory_cache",
      },
      additional_environment: {
        ACTION_DIRECTORY: "action_directory",
        TIMEOUT_MS: "timeout_millis",
        SIDE_CHANNEL: "side_channel_file",
      },
      platform_properties: {
        cpu_count: { query_cmd: "nproc" },
        OSFamily: { values: ["Linux"] },
        ISA: { values: ["x86-64"] },
        "container-image": { values: [""] },
      },
    },
  },
]
```

**Durations are plain integers of seconds or milliseconds, never `{ secs, nanos }` objects.** Feeding a `{ secs, nanos }` map to `max_action_timeout_s` is rejected at parse time with `invalid type: map, expected either a number of seconds as an integer, or a string with a duration format`. The field names to know:

| Field | Type | Default | Source |
| --- | --- | --- | --- |
| `max_action_timeout_s` | seconds (alias `max_action_timeout`) | 1200 (20m) | `cas_server.rs:1119-1124` |
| `max_upload_timeout_s` | seconds (alias `max_upload_timeout`) | 600 (10m) | `cas_server.rs:1131-1136` |
| `max_cleanup_wait_s` | seconds | 30 | `cas_server.rs:1142-1143` |
| `max_cleanup_backoff_ms` | milliseconds | 500 | `cas_server.rs:1149-1150` |
| `max_inflight_tasks` | count (`u64`) | 0 (unlimited) | `cas_server.rs:1152-1156` |

`max_action_timeout_s` bounds how long any one action may run. If a task requests a longer timeout it is rejected; otherwise NativeLink kills the action at the smaller of the requested timeout and this cap — unless `timeout_handled_externally: true`, in which case NativeLink ignores the requested timeout and always force-kills at `max_action_timeout_s` (`cas_server.rs:1158-1176`).

`max_inflight_tasks` caps how many actions this worker runs concurrently; it is sent to the scheduler in the `ConnectWorkerRequest` (`worker_api.proto:74-76`, wired through `local_worker.rs:804`). The default of `0` means unlimited — the worker accepts whatever the scheduler dispatches.

**Source:** [`nativelink-config/src/cas_server.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/src/cas_server.rs) — `LocalWorkerConfig`

## Publishing Results: `upload_action_result`

`upload_action_result` (`UploadActionResultConfig`, `cas_server.rs:1043-1099`) controls whether and how the worker writes finished results straight into the Action Cache, independent of the scheduler:

```json5
upload_action_result: {
  ac_store: "AC_MAIN_STORE",                 // omit to disable result uploads entirely
  upload_ac_results_strategy: "success_only", // success_only (default) | never | everything | failures_only
  historical_results_store: "AC_MAIN_STORE", // optional; defaults to the parent CAS store
  upload_historical_results_strategy: "failures_only",
  success_message_template: "",              // optional ExecuteResponse.message template
  failure_message_template: "",
}
```

- `ac_store` is the AC store the worker publishes into. **Omit the whole `upload_action_result` block (or leave `ac_store` unset) and the worker uploads nothing** — the default is no publishing (`cas_server.rs:1044-1048`).
- `upload_ac_results_strategy` selects which exit codes get cached. `success_only` (the default) caches only exit code 0; the other variants are `never`, `everything`, and `failures_only` (`cas_server.rs:974-987`).
- `historical_results_store` receives serialized `HistoricalExecuteResponse` messages — the failure-diagnosis records that tools like `bb_browser` read. `upload_historical_results_strategy` defaults to `failures_only`.
- `success_message_template` / `failure_message_template` populate `ExecuteResponse.message` with substituted variables such as `{digest_function}`, `{action_digest_hash}`, and `{action_digest_size}` (`cas_server.rs:1073-1098`) — handy for linking to a build browser.

## The Entrypoint

Every action command is prepended with the configured `entrypoint`. If your action command is `["gcc", "-o", "hello", "hello.c"]` and the entrypoint is `/usr/local/bin/run.sh`, the executed argv is (`running_actions_manager.rs:1232-1242`):

```
/usr/local/bin/run.sh gcc -o hello hello.c
```

The entrypoint is a single argv element — one program path — not a shell string, so it is not word-split. Put your logic in the referenced script, not in the config value.

The entrypoint sees the action's environment, including any variables you declared in `additional_environment`. These variables are **not** injected automatically — each one is an explicit mapping from a name to an `EnvironmentSource` (`running_actions_manager.rs:1395-1425`):

| Config value | Source | Injected value |
| --- | --- | --- |
| `{ property: "OSFamily" }` | `Property` | The action's value for that platform property |
| `{ value: "fixed" }` | `Value` | A literal string |
| `"from_environment"` | `FromEnvironment` | The worker process's own env var of the same name |
| `"timeout_millis"` | `TimeoutMillis` | The action's requested timeout in milliseconds |
| `"side_channel_file"` | `SideChannelFile` | Path to a file the action may write to signal an out-of-band failure such as `"timeout"` |
| `"action_directory"` | `ActionDirectory` | A scratch directory purged after the action completes |

The container-based entrypoint below works only because `ACTION_DIRECTORY` and `CONTAINER_IMAGE` are wired through `additional_environment` (the latter as a `Property` reading the `container-image` platform property):

```bash
#!/bin/bash
IMAGE="${CONTAINER_IMAGE:-ubuntu:22.04}"
exec docker run --rm \
  -v "${ACTION_DIRECTORY}:${ACTION_DIRECTORY}" \
  -w "${ACTION_DIRECTORY}" \
  "${IMAGE}" "$@"
```

A minimal entrypoint that just runs the command:

```bash
#!/bin/bash
exec "$@"
```

The entrypoint can launch the command inside a container, set up cgroups or extra isolation, handle timeouts itself (with `timeout_handled_externally: true`), or log execution metadata.

## CAS Store for Workers

Workers need fast access to action inputs and a place to stage outputs before uploading. `cas_fast_slow_store` **must** be a `FastSlowStore` whose `fast` tier is a `FileSystemStore` — the worker hardlinks input files out of the fast store's `content_path` instead of copying them, so the fast store and `work_directory` must sit on the same filesystem (`cas_server.rs:1196-1215`).

This store block is lifted verbatim from `nativelink-config/examples/basic_cas.json5` and loads cleanly:

```json5
stores: [
  {
    name: "WORKER_FAST_SLOW_STORE",
    fast_slow: {
      // "fast" must be a "filesystem" store because the worker uses it to make
      // hardlinks on disk to a directory where the jobs are running.
      fast: {
        filesystem: {
          content_path: "/tmp/nativelink/data-worker-test/content_path-cas",
          temp_path: "/tmp/nativelink/data-worker-test/tmp_path-cas",
          eviction_policy: {
            // 10gb.
            max_bytes: 10000000000,
          },
        },
      },
      slow: {
        // This example runs the CAS and the worker in the same place, so the
        // slow tier is a noop. In production point it at your shared CAS.
        noop: {},
      },
    },
  },
]
```

The `slow` tier eventually resolves to the same store the scheduler and clients use. When an action needs input file `content_path-cas/{digest}`, the worker hardlinks it into the sandbox, making input materialization nearly instant for files already resident in the fast store.

## Platform Properties

Workers advertise their capabilities via `platform_properties` — a map from property name to either a static `values` list or a `query_cmd` run once at startup (`WorkerProperty`, `cas_server.rs:942-952`):

```json5
platform_properties: {
  cpu_count: { query_cmd: "nproc" },   // dynamic: run the command, one value per output line
  OSFamily: { values: ["Linux"] },     // static
  ISA: { values: ["x86-64"] },
}
```

**A worker cannot advertise multiple distinct exact-match values for one property name.** It is tempting to write:

```json5
"supported-lang": { values: ["rust", "cpp", "go"] }   // does NOT mean "matches any of these"
```

expecting the worker to match actions requesting `rust`, `cpp`, *or* `go`. It does not. The worker side does expand each list entry into a repeated `Platform.Property` on the wire (`worker_utils.rs:39-47`), but the scheduler collapses them: it ingests the repeated properties into a `HashMap<String, PlatformPropertyValue>` keyed by property *name* and `insert`s each one (`worker_api_server.rs:166-179`). A `HashMap` insert overwrites, so only the **last** value for a given name survives — here, `go`. The capability index is then built from that already-collapsed map (`worker_capability_index.rs:69-101`), so it never sees `rust` or `cpp` at all.

The same collapse applies to `priority`-typed properties despite the field doc's aspiration that priority keys "may have more than one value" (`cas_server.rs:942-945`) — the `HashMap` keying defeats it. Real multi-value configs like `OSFamily: { values: ["Darwin", ""] }` in `worker_with_redis_scheduler.json5` therefore advertise only `""` (the last entry). If you need a worker to serve several exact values, run several workers, or model the axis as a `priority` pass-through and match on a different exact key. The Platform Properties chapter covers the matching algorithm in full.

## Directory Cache

For actions that share common input subtrees — the same third-party dependencies across many compilation actions — the worker can cache reconstructed input directories and hardlink them in rather than rebuilding the tree from CAS every time. The field is `directory_cache` (a `DirectoryCacheConfig`, `cas_server.rs:1229-1233`, `1256-1278`):

```json5
directory_cache: {
  max_entries: 10000,      // max cached directories. Default: 1000
  max_size_bytes: "10gb",  // total cap; 0 = unlimited. Default: 10 GB
  cache_root: "/tmp/nativelink/directory_cache",  // Default: {work_directory}/../directory_cache
}
```

The three fields are `max_entries`, `max_size_bytes`, and `cache_root`. **No** `experimental_directory_cache` block exists, and **no** `max_bytes` / `max_directories` fields — those names fail `deny_unknown_fields` at parse time. `cache_root` is managed by the worker and must be on the same filesystem as `work_directory` so the hardlinks resolve. If `directory_cache` is omitted the cache is disabled (`cas_server.rs:1229-1233`).

## Precondition Script

The worker can run a health check before accepting new actions:

```json5
experimental_precondition_script: "/usr/local/bin/check-resources.sh"
```

If the script exits non-zero, the worker pauses — it stops pulling from the queue — until a later check passes (`preconditions_met`, invoked at `local_worker.rs:326`). Use it for:

- Disk-space checks (`df /data | awk ...`)
- Load-average checks
- External-dependency availability (can it reach CAS? the container registry?)

## Draining and Shutdown

**No** `graceful_shutdown_timeout` config field exists — it is absent, and adding one fails `deny_unknown_fields`. Draining is signal-driven and the drain wait is unbounded, not clamped by a config timeout.

The main binary installs the signal handlers (`src/bin/nativelink.rs:935-962`):

- **SIGINT** (Ctrl-C) is *not* graceful. The handler calls `std::process::exit(130)` immediately, abandoning in-flight actions.
- **SIGTERM** is graceful. The handler logs `Process terminated via SIGTERM`, broadcasts a `ShutdownGuard` to every worker, waits for all guard clones to drop, then exits `143`.

On receiving that broadcast, the worker's run loop (`local_worker.rs:493-516`) does, in order:

1. **Wait for in-flight actions to drain.** It spins `while actions_in_flight > 0 { notified().await }` — with no timeout. The drain is bounded only indirectly, by `max_action_timeout_s` capping how long any single remaining action can run.
2. **Send `GoingAwayRequest`** — *after* the drain, not before (`local_worker.rs:507`). By the time it is sent there should be no jobs left, as the code comment notes.
3. **Drop the `ShutdownGuard` clone**, which releases the main handler's wait and lets the process exit `143`.

The true sequence is *drain, then announce, then exit* — the reverse of the intuitive "announce, then drain." A worker under SIGTERM keeps its stream open and finishes its current actions before telling the scheduler it is leaving. This lets Kubernetes rolling updates complete outstanding actions instead of failing them, provided the pod's `terminationGracePeriodSeconds` exceeds your longest `max_action_timeout_s`; otherwise Kubernetes escalates to SIGKILL and the in-flight actions die.

## Worker Telemetry

The worker publishes two metrics components through the `MetricsComponent` derive, surfaced on the admin/metrics endpoint. Top-level worker counters (`Metrics`, `local_worker.rs:920-940`):

| Metric | Meaning |
| --- | --- |
| `start_actions_received` | Total actions the scheduler asked this worker to run (received, not necessarily started). |
| `disconnects_received` | Total `disconnect` messages from the scheduler. |
| `keep_alives_received` | Total scheduler keep-alives. |
| `preconditions` | Stats on precondition-script evaluations. |

Nested under it, the running-actions-manager metrics track each phase of execution (`running_actions_manager.rs:3004-3051`): `create_and_add_action`, `make_action_directory`, `prepare_action`, `execute`, `upload_results`, `cleanup`, `get_finished_result`, plus `download_to_directory`, `prepare_output_files`, the `child_process` timing and `child_process_success_error_code` / `child_process_failure_error_code` counters, `upload_stdout` / `upload_stderr` durations, `task_timeouts`, and the cleanup-retry counters `cleanup_waits`, `stale_removals`, and `cleanup_wait_timeouts`.

Beyond aggregate metrics, each finished action reports per-action resource usage back to the scheduler: `ExecuteResult` carries `ActionResourceUsage` with `peak_memory_kb` and a `sampled` flag (`worker_api.proto:102-129`), so the scheduler sees actual peak memory per operation, not just counts.
