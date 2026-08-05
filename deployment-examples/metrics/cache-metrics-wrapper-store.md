# Store-Level Cache Metrics via a Wrapper `StoreDriver`

## Goal

Expose consistent, low-cardinality cache metrics (CAS/AC/store backends) without needing to implement bespoke instrumentation inside every individual store implementation.

The implementation is intentionally opt-in through the `cache_metrics` store
wrapper. If a store isn't wrapped, NativeLink constructs the same store graph
as before and doesn't add timers, attribute allocation, or OpenTelemetry
recording calls to that store's hot path.

This document focuses on a **wrapper store** (middleware) approach that can be applied to any `StoreDriver`, and compares it with **instrumenting inside each store**.

## Problem Statement

Users expect Prometheus/Grafana to show cache stats such as:
- Cache operation counts (`read`/`write`/`delete`/`evict`)
- Hit/miss rate for reads
- Latency distributions
- Bytes read/written throughput

These should be queryable and composable with low cognitive overhead and consistent labels.

## Two Approaches

### A) Wrapper Store (middleware)

Wrap an existing `Arc<dyn StoreDriver>` with the `cache_metrics` `StoreDriver` that:
1. Starts a timer
2. Calls the inner store method
3. Classifies the outcome (hit/miss/error/etc)
4. Records OpenTelemetry metrics

This produces uniform metrics across all stores (filesystem, memory, Redis, S3, gRPC, for example) with one implementation.

### B) Instrument Inside Each Store

Add metrics to each store implementation directly (for example, `FilesystemStore`, `S3Store`, `GrpcStore`, `FastSlowStore`, `CompletenessCheckingStore`, …), recording the same metric family from each.

This provides deeper store-specific insight but requires repeated work and continued maintenance as stores evolve.

## Pros / Cons

### Wrapper Store

**Pros**
- **Broad coverage fast**: one implementation applies everywhere.
- **Consistent semantics**: identical label keys and values across all stores.
- **Lower ongoing maintenance**: new stores automatically get metrics.
- **Configurable**: can be enabled per “logical cache” (CAS/AC) and/or store name.

**Cons**
- **Double-counting risk**: composite stores (`FastSlowStore`, `DedupStore`, `CompressionStore`, etc.) may call inner stores; wrapping both outer + inner can over-count.
- **Limited store insight**: a wrapper sees "a read happened," but may not know if it was served from fast vs slow tier unless you wrap at that level intentionally.
- **Imperfect hit classification**: for some methods, "hit" vs "miss" is best inferred from result codes (for example, `NotFound`), which may not map perfectly for all stores/operations.
- **Overhead per call**: extra timing + metric recording. Usually small, but measurable at very high QPS.

### Instrumenting Each Store

**Pros**
- **Max fidelity**: store can record store-specific outcomes (for example, S3 HEAD vs GET latency, Redis pipeline stats, filesystem rename failures).
- **Better attribution**: `FastSlowStore` can record whether fast or slow tier served the data.
- **Easier to avoid double counting** because each store "knows" whether it's a leaf or a wrapper.

**Cons**
- **High implementation cost** across many stores.
- **Inconsistent semantics risk** (different developers interpret “hit/miss” differently over time).
- **Harder to keep dashboards/rules stable** when metrics differ across stores.

## Wrapper Store Design Details

### Metric Families (Prometheus-facing names)

Assuming OpenTelemetry metric names like:
- `cache.operations` (counter)
- `cache.operation.duration` (histogram)
- `cache.io` (counter)

Prometheus/OpenMetrics typically exposes:
- `nativelink_cache_operations_total`
- `nativelink_cache_operation_duration_bucket` / `_sum` / `_count`
- `nativelink_cache_io_total`

Recording rules can derive:
- `nativelink:cache_hit_rate`
- `nativelink:cache_read_throughput_bytes`
- `nativelink:cache_operation_latency_p95`, etc.

### Labels (low-cardinality)

Recommended label keys (Prometheus form):
- `cache_type`: `cas`, `ac`, `memory`, `filesystem`, …
- `cache_operation_name`: `read`, `write`, `delete`, `evict`
- `cache_operation_result`: `hit`, `miss`, `expired`, `success`, `error`
- `instance_name`: provided by the OTEL collector transform in `deployment-examples/metrics/otel-collector-config.yaml`

### Where to Wrap (avoid double counting)

You must decide whether metrics represent:

1) **User-visible cache behavior** (recommended default)
   - Wrap only the **stores exposed to services** (for example, CAS service store, AC service store).
   - Do **not** wrap inner leaf stores.
   - Pros: One operation == one metric event, matches client perspective.
   - Cons: less insight into fast/slow tiers.

2) **Store-level behavior**
   - Wrap leaf stores and/or specific tiers (for example, wrap the "fast" and "slow" stores separately).
   - Pros: visibility into where reads are served from.
   - Cons: needs careful config to prevent double counting.

Practical rule: **wrap at exactly one layer of the store graph** for any given request path.

### Operation Mapping

Typical mapping from `StoreDriver` methods:
- `has_with_results`: `read` + `hit/miss/error` (based on `results[i].is_some()` and call result)
- `get_part`: `read` + `hit/miss/error` (`NotFound` => `miss`)
- `update` / `update_with_whole_file`: `write` + `success/error`, bytes from `UploadSizeInfo` where available
- `delete` / remove-like operations: `delete` + `success/miss/error` (store-dependent)

### Performance Considerations

Primary overhead sources:
- Timer reads (`Instant::now()` + elapsed)
- Attribute allocation (avoid per-call `Vec<KeyValue>` where possible)
- Recording calls into OpenTelemetry SDK (batch exporter settings matter)

Mitigations:
- Precompute attribute slices per `(cache_type, op, result)` (attrs cache).
- Keep label cardinality low and stable.
- Avoid attaching digests/paths as labels.

### Failure Semantics

To keep dashboards stable:
- Treat `NotFound` on reads as `miss` (not `error`).
- Treat other errors as `error`.
- Only introduce `expired` if the store layer can definitively identify expiration.

## Docs / Recording Rules Impact

Ideal outcome: **no documentation changes** once wrapper metrics land.

To reach that:
- Keep Prometheus-facing metric names/labels stable (`nativelink_cache_operations_total`, `cache_type`, `cache_operation_name`, `cache_operation_result`).
- Ensure `deployment-examples/metrics/prometheus-recording-rules.yml` references `_total` counter names.
- Keep existing dashboards querying recording rules (for example, `nativelink:cache_hit_rate`) instead of raw high-cardinality series.

Wrapper metrics are config-gated. Wrap only the logical store layer you want to
measure, for example:

```json5
"cache_metrics": {
  "cache_type": "cas",
  "backend": {
    "filesystem": {
      "content_path": "~/.cache/nativelink/content_path-cas",
      "temp_path": "~/.cache/nativelink/tmp_path-cas"
    }
  }
}
```
