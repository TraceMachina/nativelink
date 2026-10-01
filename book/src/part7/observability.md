# Observability

You can't operate what you can't observe. NativeLink emits metrics, traces, and
logs through OpenTelemetry (OTLP) and serves an HTTP health endpoint. This
chapter covers the telemetry model as it actually works, the metric names the
code actually emits, and the monitoring artifacts the repository actually ships.

## The Telemetry Model: OTLP Push, No Scrape Endpoint

The single most important fact about NativeLink observability: **NativeLink does
not expose an in-process `/metrics` scrape endpoint.** It is an OTLP *push*
client. At startup the binary unconditionally calls `init_tracing()`
(`src/bin/nativelink.rs:898`), which builds three OTLP exporters — logs, traces,
and metrics — over gRPC (`nativelink-util/src/telemetry.rs:139-204`). The metrics
exporter is a *periodic* exporter (`with_periodic_exporter`,
`telemetry.rs:186`): NativeLink pushes metrics to a collector on an interval.
Nothing scrapes NativeLink.

If you point a Prometheus scrape job at a NativeLink port expecting a text
exposition on `/metrics`, you get nothing. Metrics reach Prometheus only through
a collector (or Prometheus' own OTLP receiver).

### The Two Environment Knobs That Matter

| Variable | Controls | Source |
|---|---|---|
| `NL_OTEL_ENDPOINT` | Where OTLP is pushed (host:port of the collector) | `telemetry.rs:212` |
| `NL_LOG` | `stdout` log format: `pretty` (default), `compact`, or `json` | `telemetry.rs:83-103` |

`NL_OTEL_ENDPOINT` is the primary knob. When set, NativeLink builds a
load-balanced gRPC channel (via `ginepro`) to that endpoint and hands it to all
three exporters (`telemetry.rs:219-247`), giving client-side DNS load balancing
across collector replicas:

```bash
NL_OTEL_ENDPOINT=http://otel-collector:4317 nativelink config.json5
```

When `NL_OTEL_ENDPOINT` is unset, `maybe_load_balanced_channel()` returns `None`
(`telemetry.rs:245`) and each exporter is built with a plain `tonic` channel. In
that case the exporter falls back to the standard OpenTelemetry SDK environment
variables — it honors `OTEL_EXPORTER_OTLP_ENDPOINT` and defaults to
`http://localhost:4317`. Both paths work; `NL_OTEL_ENDPOINT` is the one you want in
production.

Because `init_tracing()` runs unconditionally, NativeLink *always* attempts to
push OTLP. With no collector listening on `:4317`, the batch exporter just logs
periodic export failures and the server otherwise runs normally. No
"metrics off" switch exists short of pointing the endpoint at a sink.

**Source:** [`nativelink-util/src/telemetry.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-util/src/telemetry.rs)

## The Metrics Pipeline

The repository ships a complete reference stack under
[`deployment-examples/metrics/`](https://github.com/TraceMachina/nativelink/tree/main/deployment-examples/metrics)
(Docker Compose and Kubernetes manifests, a collector config, recording rules, a
Grafana dashboard, and alert-routing config):

```
NativeLink ──OTLP/gRPC──▶ OpenTelemetry Collector ──▶ Prometheus ──▶ Grafana
                             (:4317 in, :9090 out)
```

### How OTLP Names Become Prometheus Names

The instruments in the code use OpenTelemetry dotted names like
`execution.completed.count`. By the time they reach Prometheus they look
different, and three transforms are responsible:

1. **Dots become underscores.** `execution.completed.count` →
   `execution_completed_count`. This is standard OpenMetrics normalization.
2. **A `nativelink_` prefix is added.** This comes from the collector's
   Prometheus exporter `namespace: nativelink`
   (`otel-collector-config.yaml:52`), not from the code.
3. **Counters gain a `_total` suffix.** With `enable_open_metrics: true`
   (`otel-collector-config.yaml:57`), counters are exposed as
   `nativelink_execution_completed_count_total`. Histograms expose
   `_bucket` / `_sum` / `_count` series.

The code's `execution.completed.count` counter is therefore queried in PromQL as
`nativelink_execution_completed_count_total`. Every recording rule and dashboard
panel in the repo assumes exactly this scheme; the recording-rules file says so
in its header comment (`prometheus-recording-rules.yml:3-6`).

Two labels are worth knowing about because they are *added by the collector*, not
the code:

- `instance_name` — set by the `transform/nativelink` processor from the
  `nativelink.instance_name` resource attribute
  (`otel-collector-config.yaml:28-34`). If you don't set that resource
  attribute, the label is absent.
- `service_instance_id` — derived from the `service.instance.id` resource
  attribute, a fresh UUIDv4 assigned to each process at startup
  (`telemetry.rs:123-129`). The dashboard's "Failures by Instance" panel groups
  on it.

## Execution Metrics (Always Emitted)

Remote-execution metrics come from the global `EXECUTION_METRICS` instruments and
are emitted whenever execution runs — no configuration required. The instrument
names and units are defined in `nativelink-util/src/metrics.rs:461-608`.

| Prometheus series | Type | Unit | Instrument (`metrics.rs`) |
|---|---|---|---|
| `nativelink_execution_completed_count_total` | Counter | actions | `execution.completed.count` (:542) |
| `nativelink_execution_active_count` | Gauge | actions | `execution.active.count` (:536) |
| `nativelink_execution_stage_transitions_total` | Counter | transitions | `execution.stage.transitions` (:548) |
| `nativelink_execution_stage_duration_bucket` | Histogram | seconds | `execution.stage.duration` (:466) |
| `nativelink_execution_total_duration_bucket` | Histogram | seconds | `execution.total.duration` (:491) |
| `nativelink_execution_queue_time_bucket` | Histogram | seconds | `execution.queue.time` (:516) |
| `nativelink_execution_output_size_bucket` | Histogram | bytes | `execution.output.size` (:554) |
| `nativelink_execution_cpu_time_bucket` | Histogram | seconds | `execution.cpu.time` (:570) |
| `nativelink_execution_memory_usage_bucket` | Histogram | bytes | `execution.memory.usage` (:587) |
| `nativelink_execution_retry_count_total` | Counter | retries | `execution.retry.count` (:603) |

The dimensioning labels (Prometheus form) are `execution_stage`,
`execution_result`, `execution_instance`, `execution_priority`,
`execution_worker_id`, and `execution_action_digest`
(`metrics.rs:29-35`). Their enumerated values are fixed in code:

- **`execution_stage`**: `unknown`, `cache_check`, `queued`, `executing`,
  `completed` (`metrics.rs:107-116`).
- **`execution_result`**: `success`, `failure`, `cancelled`, `timeout`,
  `cache_hit` (`metrics.rs:158-167`).

These are the only valid label values. A query like
`execution_stage="running"` matches nothing — the value is `executing`.

The shipped Grafana dashboard (`grafana/dashboards/nativelink-overview.json`,
titled "NativeLink Overview") is built entirely from these series. A few of its
panels, verbatim, are the canonical way to ask common questions:

```promql
# Queued actions (dashboard: "Queued Actions")
sum(nativelink_execution_active_count{execution_stage="queued"})

# Executing actions (dashboard: "Executing Actions")
sum(nativelink_execution_active_count{execution_stage="executing"})

# Success rate over 5m (dashboard: "Success Rate (%)")
sum(increase(nativelink_execution_completed_count_total{execution_result="success"}[5m]))
  / sum(increase(nativelink_execution_completed_count_total[5m])) * 100
```

**No direct "connected workers" gauge** exists. Worker liveness is inferred:
the recording rule `nativelink:worker_utilization` counts workers with an
executing action against workers seen at all, and `nativelink:actions_per_worker`
groups `execution.active.count` by `execution_worker_id`
(`prometheus-recording-rules.yml:211-233`). If you need a hard worker count,
that signal lives in the scheduler's health/admin surface, not in the OTLP
metric stream.

## Cache Metrics (Opt-In)

Cache hit/miss and throughput metrics are **not** emitted by default. They exist
only for stores you explicitly wrap with the `cache_metrics` store
(`nativelink-store/src/cache_metrics_store.rs`). Configuring OTLP does not turn
them on; wrapping a store does. If a store isn't wrapped, NativeLink builds the
same store graph as before and adds no timer, attribute allocation, or recording
call to that store's hot path (`stores.rs:53-73`).

The wrapper takes a required `cache_type` label and a `backend` store. The
`cache_type` field is mandatory — omitting it fails config load, because
`CacheMetricsSpec` uses `deny_unknown_fields` and `cache_type` has no default
(`nativelink-config/src/stores.rs:619-629`). Keep the value low-cardinality
(`cas`, `ac`); it is attached to every emitted series (`cache_metrics_store.rs:50`).

```json5
{
  name: "CAS_MAIN_STORE",
  cache_metrics: {
    cache_type: "cas",
    backend: {
      filesystem: {
        content_path: "/tmp/nativelink/content_path-cas",
        temp_path: "/tmp/nativelink/tmp_path-cas",
        eviction_policy: { max_bytes: 10000000000 },
      },
    },
  },
}
```

Wrap at exactly one layer of the store graph per request path. Composite stores
(`fast_slow`, `dedup`, `compression`) call their inner stores, so wrapping both
the outer and an inner store double-counts. The recommended default is to wrap
only the stores a service points at (the CAS/AC service store), matching the
client's view of a cache operation.

### What the Wrapper Actually Emits

The instruments are declared in `metrics.rs:373-458`, but the wrapper populates
only a subset of them. Reading `cache_metrics_store.rs` (the `StoreDriver` impl,
`:109-260`), here is exactly what is recorded:

| Prometheus series | Type | Emitted for |
|---|---|---|
| `nativelink_cache_operations_total` | Counter | `read` (hit/miss/error), `write` (success/error) |
| `nativelink_cache_io_total` | Counter | bytes on read hits and on writes |
| `nativelink_cache_operation_duration_bucket` | Histogram | every read and write (**milliseconds**) |
| `nativelink_cache_item_size_bucket` | Histogram | bytes per write |

Labels are `cache_type`, `cache_operation_name`, and `cache_operation_result`
(`metrics.rs:24-26`). Their values are fixed in code
(`metrics.rs:50-90`): operation names are `read`, `write`, `delete`, `evict`;
results are `hit`, `miss`, `expired`, `success`, `error`.

Be honest about the gaps, because the shipped README metric catalog and several
recording rules imply more than the wrapper delivers:

- **`delete` and `evict` operations are never emitted.** The wrapper's
  `StoreDriver` methods only classify reads and writes; nothing calls the
  `delete_*` or `evict_*` attribute sets. So
  `nativelink:cache_eviction_rate` (`prometheus-recording-rules.yml:171-175`)
  produces no series.
- **`nativelink_cache_size` and `nativelink_cache_entries` are never
  populated.** The `cache.size` and `cache.entries` up/down counters are
  declared (`metrics.rs:423-433`) but no code writes to them, so
  `nativelink:cache_size_bytes` and `nativelink:cache_entry_count` stay empty.
- **The `expired` result is never emitted by this wrapper.** It exists in the
  enum for stores that can definitively identify expiration; the generic
  wrapper cannot.

### A Unit Trap Worth Flagging

`cache.operation.duration` is recorded in **milliseconds**
(`metrics.rs:380`, `.with_unit("ms")`), while every `execution.*` duration
histogram is in **seconds** (`metrics.rs:467` and following). When you write
PromQL against `nativelink_cache_operation_duration_bucket`, the `le` bucket
boundaries are milliseconds; against `nativelink_execution_queue_time_bucket`
they are seconds. Mixing the two units is the most common way to write a cache
latency alert that never fires.

## Recording Rules and Dashboards (Shipped)

Rather than reproduce query text, use what the repo ships and query the recorded
series:

- **`prometheus-recording-rules.yml`** pre-computes success rates, cache hit
  rates, latency percentiles, queue depth, and a small set of SLO series
  (`nativelink:slo_execution_success_rate`,
  `nativelink:slo_cache_read_latency`, `nativelink:slo_queue_time`,
  `nativelink:error_budget_remaining`).
- **`grafana/dashboards/nativelink-overview.json`** is the "NativeLink Overview"
  dashboard: executions, success rate, queued/executing counts, stage
  transitions, and failures by instance and by action digest.

The cache-oriented recording rules (`nativelink:cache_hit_rate`,
`nativelink:cache_operation_latency_p95`, and friends,
`prometheus-recording-rules.yml:123-195`) yield series only once you enable the
`cache_metrics` wrapper on the relevant store — otherwise their inputs don't
exist.

## Structured Logging

NativeLink logs through `tracing`. Two independent knobs control it:

**Format** is `NL_LOG` (`telemetry.rs:83`). The default is `pretty`
(human-oriented, multi-line); `compact` is a denser single line; `json` emits
one JSON object per event for log aggregators (Loki, Elasticsearch, CloudWatch):

```bash
NL_LOG=json nativelink config.json5
```

Note the correction from earlier drafts: JSON logging is controlled by
`NL_LOG=json`, not by whether OTLP is configured. The two are orthogonal — the
same log events are also exported over OTLP as structured log records regardless
of the `stdout` format (`telemetry.rs:144-156`).

**Verbosity** is the standard `RUST_LOG` `EnvFilter`, defaulting to `info`
(`telemetry.rs:68-76`):

```bash
# Verbose store operations:
RUST_LOG=nativelink_store=debug nativelink config.json5

# Trace service-level gRPC handling:
RUST_LOG=nativelink_service=trace nativelink config.json5
```

The filter force-disables a handful of noisy upstream crates regardless of
`RUST_LOG` — `hyper`, `tonic`, `h2`, `reqwest`, and `tower` are pinned `off`
(`telemetry.rs:71-75`) — so you won't drown in transport chatter at `debug`.

## Health Endpoint

NativeLink's health check is **HTTP, not gRPC.** No
`grpc.health.v1.Health` service exists anywhere in the binary. The health service is a
plain HTTP handler (`nativelink-service/src/health_server.rs`) mounted with
`route_service` onto the `axum` router of whichever server block declares it
(`src/bin/nativelink.rs:433-439`). It answers on that server's listener port and
path.

Enable it in the `services` map. Both fields are optional; the path defaults to
`/status` and the timeout to 5 seconds (`cas_server.rs:620-635`,
`health_server.rs:39`):

```json5
services: {
  health: {
    path: "/status",       // default "/status"
    timeout_seconds: 5,    // default 5
  },
}
```

Because the handler mounts on the same `axum` router as the block's other
services, it shares that block's port. In the shipped examples the health
service lives on the private worker-facing server — for instance
`0.0.0.0:50061` in
[`basic_cas.json5`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/examples/basic_cas.json5)
— so the check is reachable at `http://localhost:50061/status`.

A `GET` returns a JSON array of per-component `HealthStatusDescription` objects.
The status code encodes overall health: `200 OK` when every component is healthy,
`503 Service Unavailable` if any component reports `Failed` or `Timeout`
(`health_server.rs:81-97`). That makes it a drop-in Kubernetes probe:

```bash
curl -i http://localhost:50061/status
```

```yaml
livenessProbe:
  httpGet:
    path: /status
    port: 50061
  initialDelaySeconds: 5
  periodSeconds: 10

readinessProbe:
  httpGet:
    path: /status
    port: 50061
  initialDelaySeconds: 5
  periodSeconds: 5
```

Use `httpGet`, not `grpc` — an earlier version of this chapter documented a gRPC
probe against a service that does not exist.

## Alerting

The repository ships alert *rules* (as opposed to the recording rules) in the
Kubernetes bundle
([`kubernetes/prometheus.yaml:113-178`](https://github.com/TraceMachina/nativelink/blob/main/deployment-examples/metrics/kubernetes/prometheus.yaml)),
plus severity-based *routing* in `alertmanager-config.yml` (routes only, no
rules of its own). The shipped alerts, grounded in the real metric names:

| Alert | Fires when | Built on |
|---|---|---|
| `NativeLinkHighErrorRate` | `execution_result="success"` fraction drops below 95% for 5m | `nativelink_execution_completed_count_total` |
| `NativeLinkQueueBacklog` | over 100 actions in `execution_stage="queued"` for 15m | `nativelink_execution_active_count` |
| `NativeLinkWorkerUtilizationLow` | fewer than 30% of seen workers executing for 30m | `nativelink_execution_active_count` by `execution_worker_id` |
| `NativeLinkCacheMissRateHigh` | read miss rate over 50% per `cache_type` for 10m | `nativelink_cache_operations_total` |
| `NativeLinkCacheEvictionRateHigh` | over 10 evictions/sec per `cache_type` for 10m | `nativelink_cache_operations_total` |

The two cache alerts only fire for stores wrapped with `cache_metrics`, and
`NativeLinkCacheEvictionRateHigh` in particular will never fire today because the
wrapper does not emit `evict` operations (see above) — treat it as a placeholder
for when eviction instrumentation lands.

## Debugging Playbooks

### "Why is my build slow?"

1. **Queue time** — `histogram_quantile(0.95, sum by (le)
   (rate(nativelink_execution_queue_time_bucket[5m])))`. High p95 means actions
   wait; add workers or check platform matching.
2. **Stage duration** — `nativelink:execution_stage_duration_p95` broken out by
   `execution_stage` tells you whether time goes to `cache_check`, `queued`, or
   `executing`.
3. **Output size** — `nativelink_execution_output_size_bucket`. Large outputs
   inflate upload time.

### "Why is my cache hit rate low?"

Requires the `cache_metrics` wrapper on the CAS/AC store.

1. **Hit rate** — `nativelink:cache_hit_rate` by `cache_type`. A low `ac` hit
   rate with a healthy `cas` hit rate points at action-key mismatch, not blob
   loss.
2. **Miss breakdown** —
   `rate(nativelink_cache_operations_total{cache_operation_result="miss"}[5m])`.
3. **Client comparison** — compare action digests between machines; a toolchain
   or environment difference changes the action hash. See
   [Appendix C: Troubleshooting](../appendix/troubleshooting.md).

### "Why did the scheduler stop dispatching?"

1. **Actions by stage** — `sum by (execution_stage)
   (nativelink_execution_active_count)`. Actions stuck in `queued` with none
   `executing` means no worker matched.
2. **Worker signal** — `nativelink:actions_per_worker`; an empty result means no
   worker is running anything.
3. **Health** — hit the `/status` endpoint; a `503` names the failing component
   in its JSON body.

## Cross-References

- [Appendix A: Configuration Reference](../appendix/config-reference.md) — the
  full `cache_metrics` and `health` config surfaces.
- [Appendix C: Troubleshooting](../appendix/troubleshooting.md) — cache-miss
  diagnosis, action-digest comparison, and health-check failures.
- [`deployment-examples/metrics/`](https://github.com/TraceMachina/nativelink/tree/main/deployment-examples/metrics)
  — the collector config, recording rules, dashboard, and alert rules referenced
  throughout this chapter.

## Code Map

| File | Purpose |
|---|---|
| `nativelink-util/src/telemetry.rs` | OTLP exporter setup, `NL_OTEL_ENDPOINT`, `NL_LOG`, log/trace/metric layers |
| `nativelink-util/src/metrics.rs` | Global cache and execution instruments, label keys, enum values |
| `nativelink-store/src/cache_metrics_store.rs` | The opt-in `cache_metrics` wrapper `StoreDriver` |
| `nativelink-config/src/stores.rs` | `CacheMetricsSpec` (`cache_type`, `backend`) |
| `nativelink-config/src/cas_server.rs` | `HealthConfig` (`path`, `timeout_seconds`) |
| `nativelink-service/src/health_server.rs` | HTTP health handler; `200`/`503` semantics |
| `deployment-examples/metrics/` | Collector, recording rules, Grafana dashboard, alert rules |
