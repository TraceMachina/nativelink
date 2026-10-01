# The Store Catalog

Every store type in NativeLink, what it does, and when to use it.

**Source:** [`nativelink-config/src/stores.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/src/stores.rs)

Config keys are the `snake_case` form of each `StoreSpec` variant (`stores.rs:52`).
Every spec struct is `#[serde(deny_unknown_fields)]`, so a misspelled or extra key
is a hard error at config load — booting `nativelink <config>` surfaces it before
the server serves a request.
Each entry below lists its fields as **required** (no default; omitting it fails
to load) or **optional** (with the default the code applies when omitted).

## Leaf Stores (Terminal — No Children)

### `memory`

In-process hash map with LRU eviction. Lost on restart.

```json5
memory: {
  eviction_policy: {
    max_bytes: "4gb",
    max_count: 0,        // 0 = unlimited count
    max_seconds: 0,      // 0 = no TTL
    evict_bytes: "512mb" // evict down to (max_bytes - evict_bytes) when full
  }
}
```

**Fields:**

- `eviction_policy` — optional (default: none). Without it the store never evicts and
  grows without bound. Every field of `EvictionPolicy` defaults to `0` (disabled):
  `max_bytes`, `max_count`, `max_seconds`, `evict_bytes`.

**Use when:** Hot cache tier, development, testing.

### `filesystem`

Files on local disk, one file per digest. Supports hardlinks for zero-copy access by
workers. On startup the `content_path` is scanned and re-indexed; files that no longer
match the digest naming are deleted.

```json5
filesystem: {
  content_path: "/data/cas/content",
  temp_path: "/data/cas/tmp",
  eviction_policy: { max_bytes: "100gb" },
  block_size: 4096,
  read_buffer_size: 32768
}
```

**Fields:**

- `content_path` — **required**. Bulk content lives here.
- `temp_path` — **required**. In-flight uploads land here; must be on the same block
  device as `content_path` so completed files move atomically. Wiped on every startup.
- `eviction_policy` — optional (default: none → unbounded growth).
- `read_buffer_size` — optional (default: `32k`).
- `block_size` — optional (default: `4kb`). Used to charge each entry its real
  on-disk footprint against the eviction budget.
- `max_concurrent_writes` — optional (default: `0` = unlimited). Caps concurrent
  `sync_all()` writes so a burst of uploads cannot saturate disk I/O.

**Use when:** Single-node deployments, workers (local artifact cache), CI runners.

### `experimental_cloud_object_store`

Object storage backend. The `provider` tag selects the implementation: `aws` (S3),
`gcs`, `azure`, `ontap`, `r2`, or `oci` (`ExperimentalCloudObjectSpec`, `stores.rs:1073`).
`provider` is **required**; the remaining fields depend on the provider. This store never
deletes objects — expire them with the bucket's own lifecycle tooling and
`consider_expired_after_s`. See [Cloud Backends](./cloud-backends.md) for the per-provider
field tables.

### `ontap_s3_existence_cache`

An ONTAP-specific caching layer over the ONTAP S3 store. It keeps an in-memory set of
object digests and periodically persists it to `index_path`, so repeated `has` checks do
not round-trip to ONTAP. This is **not** a general-purpose wrapper: its `backend` is an
inline ONTAP S3 spec (the same fields as the `ontap` provider of
`experimental_cloud_object_store`), not a `StoreSpec` (`OntapS3ExistenceCacheSpec`,
`stores.rs:800`).

```json5
ontap_s3_existence_cache: {
  index_path: "/var/cache/nativelink/ontap-index.json",
  sync_interval_seconds: 300,
  backend: {
    endpoint: "https://ontap-s3-endpoint:443",
    vserver_name: "your-vserver",
    bucket: "your-bucket",
    key_prefix: "cas/"       // optional
  }
}
```

**Fields:**

- `index_path` — **required**. On-disk path for the persisted digest index.
- `sync_interval_seconds` — **required**. How often the in-memory index is flushed to
  `index_path`.
- `backend` — **required**. Inline ONTAP S3 spec: `endpoint`, `vserver_name`, and
  `bucket` are required; `root_certificates` and the shared object fields
  (`key_prefix`, `retry`, `consider_expired_after_s`, …) are optional. Credentials come
  from `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_DEFAULT_REGION`.

**Use when:** ONTAP S3 is your durable tier and `has` latency dominates (typically the
fast side of a `fast_slow`, as in [`examples/ontap_backend.json5`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-config/examples/ontap_backend.json5)).

### `redis_store`

Redis (standalone, Cluster, or Sentinel) as a store backend.

```json5
redis_store: {
  addresses: ["redis://redis-1:6379", "redis://redis-2:6379"],
  key_prefix: "nl:",
  connection_pool_size: 16,
  experimental_pub_sub_channel: "nativelink-events"
}
```

**Fields:**

- `addresses` — **required**. One or more `redis://` URLs (host, port, optional
  database id and credentials).
- `mode` — optional (default: `standard`; also `cluster`, `sentinel`).
- `key_prefix` — optional (default: empty).
- `connection_pool_size` — optional (default: `3`).
- `read_chunk_size` — optional (default: `64KiB`).
- `max_client_permits` — optional (default: `500`). Caps in-flight commands.
- `command_timeout_ms` — optional (default: `10000`).
- `connection_timeout_ms` — optional (default: `3000`).
- `experimental_pub_sub_channel` — optional (default: none). Publishes a write event
  per update.

**Use when:** Shared state that needs sub-millisecond access (scheduler state, existence
metadata). Not ideal for large blobs — Redis holds everything in memory, and most Redis
services cap a value at 256–512 MB. Pair with `size_partitioning` and/or `fast_slow`.

### `experimental_mongo`

MongoDB as a store backend for CAS and/or scheduler data. Each blob is stored as a
single BSON document with the payload in a binary field — there is **no** GridFS
chunking, so an individual object is bounded by MongoDB's 16 MB document limit.
`read_chunk_size` only governs how much of that field is streamed back at a time.

```json5
experimental_mongo: {
  connection_string: "mongodb://mongo:27017",
  database: "nativelink",
  cas_collection: "cas"
}
```

**Fields** (`ExperimentalMongoSpec`, `stores.rs:1629`):

- `connection_string` — **required**. A `mongodb://` or `mongodb+srv://` URI.
- `database` — optional (default: `"nativelink"`).
- `cas_collection` — optional (default: `"cas"`). Collection for CAS blobs.
- `scheduler_collection` — optional (default: `"scheduler"`). Collection for scheduler
  state.
- `key_prefix` — optional (default: none).
- `enable_change_streams` — optional (default: `false`). **Required for scheduler
  subscriptions** (real-time updates).
- `read_chunk_size` — optional (default: `64KB`).
- `connection_timeout_ms` / `command_timeout_ms` — optional (defaults: `3000` /
  `10000`).
- `write_concern_w` / `write_concern_j` / `write_concern_timeout_ms` — optional
  (default: MongoDB's own defaults).
- `max_requests` — optional (default: unlimited).

**Use when:** You already run MongoDB and want to avoid adding another storage system.
Divert objects near the 16 MB limit elsewhere with `size_partitioning`.

### `grpc`

Proxies store operations to a remote NativeLink instance (or any REAPI-compatible
server).

```json5
grpc: {
  instance_name: "main",
  endpoints: [{ address: "grpc://remote-cas:50051" }],
  store_type: "cas",           // required: "cas" or "ac"
  connections_per_endpoint: 4
}
```

**Fields:**

- `endpoints` — **required**. Non-empty list; each entry needs an `address`
  (`grpc://` or `grpcs://`).
- `store_type` — **required**. `"cas"` or `"ac"`; it selects which upstream RPCs are
  issued. Omitting it fails to load with `missing field 'store_type'`
  (`GrpcSpec`, `stores.rs:1322`).
- `instance_name` — optional (default: empty). Outgoing calls are rewritten to this
  instance name.
- `connections_per_endpoint` — optional (default: `1`).
- `max_concurrent_requests` — optional (default: `0` = unlimited).
- `rpc_timeout_s` — optional (default: `0` = disabled).
- `use_legacy_resource_names` — optional (default: `false`). Set `true` for older
  backends (e.g. Buildbarn pre-v0.3) that reject the digest-function path segment.
- `headers` — optional (default: none). Static headers on every upstream request.
- `forward_headers` — optional (default: none). Header names copied from the inbound
  client request (e.g. a build client's JWT).

**Use when:** Distributed deployments where CAS is centralized, or to proxy to a
third-party REAPI server.

### `noop`

Silently discards all writes; returns "not found" for all reads.

```json5
noop: {}
```

**Fields:** none.

**Use when:** The "slow" side of a `fast_slow` when you do not want durability, the
`upper_store` of a `size_partitioning` that drops oversized blobs, or the backend of a
test configuration.

## Composite Stores (Wrappers — Have Children)

### `fast_slow`

Two-tier cache. Reads try `fast`, fall back to `slow`, and backfill `fast`; writes
mirror to both. See [Store Composition](./store-composition.md).

```json5
fast_slow: {
  fast: { /* store */ },
  slow: { /* store */ }
}
```

**Fields:**

- `fast` — **required**.
- `slow` — **required**.
- `fast_direction` / `slow_direction` — optional (default: `both`; also `update`, `get`,
  `read_only`). Use `get` on the fast store of a worker so results persist only to
  `slow`.
- `bypass_dedup_threshold_bytes` — optional (default: `0` = disabled). Reads at or above
  this size stream straight from `slow` without populating `fast`.

> **WARNING:** `fast_slow` assumes an object present in `fast` is also in `slow`; it
> never re-checks `slow`. Keep that in mind when `slow` must hold everything (e.g. for
> remote execution).

### `compression`

Transparent compression/decompression layer.

```json5
compression: {
  backend: { /* store */ },
  compression_algorithm: {
    lz4: { block_size: 65536 }
  }
}
```

**Fields:**

- `backend` — **required**.
- `compression_algorithm` — **required**. The only variant is `lz4`
  (`CompressionAlgorithm`, `stores.rs:1012`); there is no `zstd` option, and naming one
  fails to load with `unknown variant 'zstd', expected 'lz4'`. `lz4` accepts
  `block_size` (optional, default `65536`) and `max_decode_block_size` (optional,
  default: the `block_size` value).

### `dedup`

Content-defined chunking with separate index and content stores. Slices input with a
rolling hash, stores unique chunks in `content_store`, and writes a chunk manifest to
`index_store`.

```json5
dedup: {
  index_store: { /* store for chunk manifests */ },
  content_store: { /* store for chunk data */ }
}
```

**Fields:**

- `index_store` — **required**. Should be fast and small.
- `content_store` — **required**. The larger/slower tier; pairs well with `compression`.
- `min_size` — optional (default: `64k`).
- `normal_size` — optional (default: `256k`). Also the rough threshold below which an
  entry is forwarded whole rather than deduped.
- `max_size` — optional (default: `512k`).
- `max_concurrent_fetch_per_get` — optional (default: `10`).

> `.has()` on a dedup store only checks the `index_store`; it does not confirm every
> chunk still exists in `content_store`.

### `verify`

Re-derives the digest on write to reject corrupt or mislabeled data before it reaches
the backend.

```json5
verify: {
  backend: { /* store */ },
  verify_size: true,   // CAS: true, AC: false
  verify_hash: true    // CAS: true, AC: false
}
```

**Fields:**

- `backend` — **required**.
- `verify_size` — optional (default: `false`). Rejects an upload whose byte count does
  not match the digest size. Enable on CAS, leave off on AC.
- `verify_hash` — optional (default: `false`). Recomputes the hash and rejects the write
  if it does not match the key. Enable on CAS, leave off on AC.

**Digest safety.** `verify_hash` does not name a hash function: it uses the digest
function the client declared on the request, falling back to the global
`default_digest_hash_function` when the request leaves the field unset
(`VerifySpec`, `stores.rs:964`). Mind the silent fallback — a BLAKE3 client that omits
the digest-function field gets verified (and stored) under the configured default (e.g.
SHA256), so `verify_hash` is only a genuine corruption backstop when clients and the
`default_digest_hash_function` agree on the algorithm.

### `existence_cache`

Caches `has` results so subsequent existence checks skip the backend. CAS-only.

```json5
existence_cache: {
  backend: { /* store */ },
  eviction_policy: { max_count: 1000000 }
}
```

**Fields:**

- `backend` — **required**.
- `eviction_policy` — optional (default: none → unbounded). This governs the cache
  itself, independent of any policy on the `backend` store.

### `size_partitioning`

Routes each blob to a different store by digest size. Safe only where the digest size is
the real content size (CAS, not AC).

```json5
size_partitioning: {
  size: 1048576,               // threshold in bytes (accepts "1mib")
  lower_store: { /* < size */ },
  upper_store: { /* >= size */ }
}
```

**Fields:**

- `size` — **required**. Blobs smaller than this go to `lower_store`; the rest to
  `upper_store`.
- `lower_store` — **required**.
- `upper_store` — **required**.

### `shard`

Distributes across multiple stores by digest hash.

```json5
shard: {
  stores: [
    { store: { /* store */ }, weight: 1 },
    { store: { /* store */ }, weight: 1 }
  ]
}
```

**Fields:**

- `stores` — **required**. Each entry has a `store` (**required**) and a `weight`
  (optional, default `1`). A store's share is its weight over the sum of all weights.

### `completeness_checking`

Before returning an `ActionResult`, decodes it and confirms every referenced
output blob still exists in the CAS. Prevents handing back a cached result whose
outputs have been evicted.

```json5
completeness_checking: {
  backend: { /* AC store */ },
  cas_store: {
    ref_store: { name: "CAS_MAIN_STORE" }
  }
}
```

**Fields:**

- `backend` — **required**. The AC store whose results are validated.
- `cas_store` — **required**, and it is a full `StoreSpec`, not a store name. Reference
  the shared CAS with `{ ref_store: { name: "…" } }` (`CompletenessCheckingSpec`,
  `stores.rs:976`). Passing a bare string fails to load with
  `unknown variant 'CAS_MAIN_STORE'`, because the value is parsed as an inline store.

**Use when:** Wrapping an AC store, so cached results can never reference CAS blobs that
have already been evicted.

### `cache_metrics`

Wraps a store and records low-cardinality OpenTelemetry hit/miss/upload/download metrics.
Opt-in: unwrapped stores pay none of its hot-path timing cost.

```json5
cache_metrics: {
  cache_type: "cas",
  backend: { /* store */ }
}
```

**Fields:**

- `cache_type` — **required**. The metric label, e.g. `"cas"` or `"ac"`. Omitting it
  fails to load with `missing field 'cache_type'` (`CacheMetricsSpec`,
  `stores.rs:622`).
- `backend` — **required**. The store to instrument.

**Use when:** You want per-store observability via the OpenTelemetry exporter.

### `ref_store`

References another named store by name, avoiding a duplicated config subtree.

```json5
ref_store: {
  name: "SOME_OTHER_STORE"
}
```

**Fields:**

- `name` — **required**. Must match a store `name` under the root `stores` list.
  References are resolved after all stores are constructed, and the server
  fails to start if a name does not resolve.

**Use when:** Multiple services (or nested stores) need to share one physical store
without duplicating its config.

## Composition Cheat Sheet

| I want to... | Use... |
|---|---|
| Cache hot artifacts in memory | `fast_slow: { fast: memory, slow: ... }` |
| Reduce storage costs | `compression: { backend: ... }` |
| Detect corruption | `verify: { backend: ..., verify_hash: true }` |
| Reduce latency on `has` calls | `existence_cache: { backend: ... }` |
| Cache `has` over ONTAP S3 | `ontap_s3_existence_cache: { backend: ... }` |
| Save storage for similar artifacts | `dedup: { index_store: ..., content_store: ... }` |
| Route small/large blobs differently | `size_partitioning: { lower_store: ..., upper_store: ... }` |
| Scale storage horizontally | `shard: { stores: [...] }` |
| Ensure AC points to existing CAS data | `completeness_checking: { backend: ..., cas_store: { ref_store: { name: "..." } } }` |
| Get metrics on store operations | `cache_metrics: { cache_type: "...", backend: ... }` |
| Reference a shared store | `ref_store: { name: "..." }` |
| Discard data intentionally | `noop: {}` |
