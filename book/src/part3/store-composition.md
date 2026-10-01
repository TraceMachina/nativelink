# Store Composition

NativeLink's stores compose. You build complex storage topologies by nesting stores inside stores in config. No code changes, no plugins. This is the store algebra.

## The Algebra

Every composite store takes one or more inner stores as children. The children can be any store — including other composites. This gives you a tree:

```
CacheMetrics
  └── Verify
        └── Compression
              └── FastSlow
                    ├── fast: Memory
                    └── slow: ExperimentalCloudObjectStore (S3)
```

This tree says: wrap S3 with a memory cache (fast/slow), compress everything going to S3, verify integrity on upload, and collect metrics at the top.

In JSON5 config:

```json5
stores: [
  {
    name: "MY_CAS",
    cache_metrics: {
      // Low-cardinality metrics label; "cas" or "ac". Required —
      // `CacheMetricsSpec` uses `deny_unknown_fields` with no default.
      cache_type: "cas",
      backend: {
        verify: {
          backend: {
            compression: {
              backend: {
                fast_slow: {
                  fast: { memory: { eviction_policy: { max_bytes: 1073741824 } } },
                  slow: {
                    experimental_cloud_object_store: {
                      provider: "aws",
                      region: "us-east-1",
                      bucket: "my-cas-bucket"
                    }
                  }
                }
              },
              compression_algorithm: { lz4: {} }
            }
          },
          verify_size: true,
          verify_hash: true
        }
      }
    }
  }
]
```

Every JSON5 block in this chapter is accepted by the config loader (it parses
and resolves its references at startup). `deny_unknown_fields` means a stray
or misspelled key is a hard error, so copy-paste actually works.

## FastSlowStore: The Tier Pattern

The most common composition. A fast local cache in front of a slow durable backend.

**Source:** [`nativelink-store/src/fast_slow_store.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-store/src/fast_slow_store.rs)

Semantics (`FastSlowStore` in `fast_slow_store.rs`):
- **`has`** → queries the **slow** store *only*. It deliberately never
  consults the fast store: a blob that lives only in fast (its slow write
  is still in flight, or previously failed) must still be re-uploaded to
  slow, so reporting it present would be wrong for remote execution, where
  workers require the blob in the durable tier. It also consults an
  in-flight-slow-writes map (`in_flight_slow_writes`), so a concurrent
  upload that has not yet landed in slow is still visible to a concurrent
  existence check — this prevents redundant duplicate uploads of the same
  blob. The one exception: if the slow store is a `noop` store (which
  always answers 404), `has` checks the fast store instead.
- **`get`** → tries fast first; on miss, reads from slow and populates fast concurrently. Unlike `has`, the read path *does* assume that a blob present in fast is present in slow.
- **`update`** → writes to **both** fast and slow simultaneously (multiplexed stream). With `slow_store_write_back = true`, the slow-tier copy is deferred to a background write instead: the update completes when the fast tier has the bytes, and the slow store is populated asynchronously. This is the fleet posture for a NVMe-fast / object-storage-slow CAS — it exists because a stalled slow tier in the synchronous tee freezes the whole write (the multiplexed stream advances at the slower consumer's pace). Trade-off: a durability window in which the blob exists only in fast (see the option's doc in `nativelink-config/src/stores.rs`).

The get path uses leader/follower deduplication: if multiple concurrent
requests miss the fast cache for the same digest, only one — the
*leader* — reads from the slow store while streaming to the caller and
filling fast. The others — *followers* — block on the leader's shared
`OnceCell` (keyed in `populating_digests`) and then read from the warm
fast cache. A follower's wait is bounded by `LEADER_WAIT_TIMEOUT` (one
minute); past that it bypasses the dedup map and reads the slow store
directly, so a single wedged leader cannot fan a slow read out into a
storm of `DEADLINE_EXCEEDED` errors across every concurrent reader.

Configuration options:
- `fast_direction` / `slow_direction` — a `StoreDirection` (`both`, `update`, `get`, or `read_only`) controlling whether each tier participates in updates, gets, both, or neither. Setting the fast tier to `get` on a worker, for example, persists results only to the slow store.
- `bypass_dedup_threshold_bytes` — reads at or above this size skip the leader/follower dedup map and stream straight from slow. `0` (the default) disables the bypass so every read goes through dedup.

```json5
fast_slow: {
  fast: { memory: { eviction_policy: { max_bytes: "4gb" } } },
  slow: { filesystem: { content_path: "/data/cas", temp_path: "/data/tmp" } },
  fast_direction: "both",
  slow_direction: "both"
}
```

## CompressionStore: Transparent Compression

Wraps any store and compresses content on write, decompresses on read. The caller sees uncompressed bytes; the backend stores compressed bytes.

**Source:** [`nativelink-store/src/compression_store.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-store/src/compression_store.rs)

`CompressionAlgorithm` has exactly one variant: `lz4` (`stores.rs`). There
is no ZSTD option on this store — a `compression_algorithm: { zstd: {} }`
fails to load with `unknown variant zstd, expected lz4`. LZ4 is extremely
fast in both directions and aborts early on incompressible input, which
suits build artifacts. `Lz4Config` exposes `block_size` (default 64 KiB)
and `max_decode_block_size`. Compression uses a custom framed format — the
blob is split into independently-compressed blocks with a footer index —
so partial reads don't require decompressing the whole blob.

```json5
compression: {
  backend: { memory: {} },  // any inner store
  compression_algorithm: {
    lz4: { block_size: 65536 }
  }
}
```

## DedupStore: Content-Level Deduplication

Splits blobs into content-defined chunks (using FastCDC), stores each chunk separately, and stores an index mapping the original digest to its chunk sequence.

**Source:** [`nativelink-store/src/dedup_store.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-store/src/dedup_store.rs)

Two inner stores:
- `index_store` — maps original digest → list of chunk digests
- `content_store` — stores the actual chunks

If two blobs share byte sequences (e.g., two slightly different binaries), the shared chunks are stored once. This reduces storage significantly for incremental builds where most of the binary doesn't change between versions.

Note the `has` asymmetry: a `has` on a dedup store checks only that the
*index* exists in `index_store`; it does not verify that every chunk still
exists in `content_store`. Keep this in mind when a dedup store sits under
a completeness check: a completeness check over a dedup store amplifies one metadata request into per-chunk existence probes.

```json5
dedup: {
  index_store: { memory: { eviction_policy: { max_bytes: "100mb" } } },
  content_store: {
    experimental_cloud_object_store: {
      provider: "aws",
      region: "us-east-1",
      bucket: "my-cas-bucket"
    }
  },
  min_size: 8192,
  normal_size: 32768,
  max_size: 131072
}
```

## VerifyStore: Verify On The Way In

Wraps a store and verifies content **on `update` — the write path — not on
read.** As bytes stream to the inner store, `VerifyStore` tallies the size
and (when `verify_hash` is set) hashes the stream; at EOF it compares
against the digest the client claimed. A mismatch fails the upload with a
specific error, so bad data is rejected before it is ever stored. This is
the honest place to spend the cost: you pay it once, at admission.

`get_part` is a pure pass-through to the inner store (`verify_store.rs`) —
reads are **not** re-hashed. The trust boundary is the client that uploads,
not the backend that stores; once verified in, content is served back at
full speed.

**Source:** [`nativelink-store/src/verify_store.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-store/src/verify_store.rs)

```json5
verify: {
  backend: { memory: {} },  // any inner store
  verify_size: true,
  verify_hash: true
}
```

`verify_hash` picks the hash function from the request context
(`DigestHasherFunc`), falling back to the global default — it does not need
its own algorithm field. The recommended split is `verify_size` and
`verify_hash` both `true` for CAS stores and both `false` for AC stores
(an AC key is not a hash of its value, so hashing it is meaningless).
`VerifyStore` accepts only digest keys and rejects string keys, so never
wrap a string-keyed store (path-info, alias) in `verify`.

## ExistenceCacheStore: Cheap `has` Calls

Caches the results of `has` calls in memory. Useful when the backend `has` is expensive (e.g., S3 HEAD request with network latency).

**Source:** [`nativelink-store/src/existence_cache_store.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-store/src/existence_cache_store.rs)

```json5
existence_cache: {
  backend: { memory: {} },  // any inner store
  eviction_policy: { max_count: 1000000 }
}
```

Intended for CAS stores only. This is pure optimization — it never changes correctness (a negative `has` result might be stale if content was uploaded by another path, but this only causes a redundant upload, not data loss). It caches only *positive* existence, and it drops overwrites, so never wrap a store whose keys can be rewritten with new values (e.g. a Nix path-info store) in `existence_cache`.

## SizePartitioningStore: Route by Size

Sends small blobs to one store and large blobs to another. Useful for optimizing I/O patterns: small blobs to a low-latency store (memory, Redis), large blobs to a high-throughput store (S3).

**Source:** [`nativelink-store/src/size_partitioning_store.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-store/src/size_partitioning_store.rs)

```json5
size_partitioning: {
  size: 1048576,  // 1MB threshold; blobs < size go lower, >= size go upper
  lower_store: { memory: { eviction_policy: { max_bytes: "2gb" } } },
  upper_store: {
    experimental_cloud_object_store: {
      provider: "aws",
      region: "us-east-1",
      bucket: "my-cas-bucket"
    }
  }
}
```

Like `verify_size`, this store trusts that the digest size is the real
content size, so use it on CAS stores only, never on AC stores.

## ShardStore: Horizontal Scaling

Distributes blobs across multiple stores by hashing the key. Each shard handles a fraction of the keyspace.

**Source:** [`nativelink-store/src/shard_store.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-store/src/shard_store.rs)

A `grpc` store needs `endpoints` (a list of `{ address }` objects, not a
single `endpoint` string) and a `store_type` of `cas` or `ac` — the latter
tells the store which upstream RPCs to make. Both are enforced at config
load; the older single-`endpoint` shorthand is not a real field.

```json5
shard: {
  stores: [
    { store: { grpc: { store_type: "cas", endpoints: [{ address: "grpc://cas-1:50051" }] } }, weight: 1 },
    { store: { grpc: { store_type: "cas", endpoints: [{ address: "grpc://cas-2:50051" }] } }, weight: 1 },
    { store: { grpc: { store_type: "cas", endpoints: [{ address: "grpc://cas-3:50051" }] } }, weight: 1 },
  ]
}
```

## CompletenessCheckingStore: Eviction-Coherent AC

Wraps an **AC** store and, before returning an `ActionResult`, decodes it
and confirms every referenced digest — output files, `stdout`/`stderr`,
and the trees behind output directories — still exists in a companion CAS
store. If any referent is gone (evicted, never uploaded), the action reads
as a miss. This keeps the action cache coherent with the CAS: a cache hit
is only served when its outputs can actually be fetched.

**Source:** [`nativelink-store/src/completeness_checking_store.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-store/src/completeness_checking_store.rs)

```json5
completeness_checking: {
  backend: { memory: {} },              // the AC store being wrapped
  cas_store: { ref_store: { name: "CAS_MAIN" } }
}
```

Use it on AC stores only. `cas_store` is typically a `ref_store` pointing
at the shared CAS so both wrappers see the same content. This is how
eviction turns into garbage collection: once the CAS evicts an action's
outputs, the completeness check makes the AC record read as absent, so a
stale result is never served against bytes that are gone.

## The Factory

Store trees are constructed recursively from config at startup:

```rust
// nativelink-store/src/default_store_factory.rs

pub fn store_factory<'a>(
    backend: &'a StoreSpec,
    store_manager: &'a Arc<StoreManager>,
    maybe_health_registry_builder: Option<&'a mut HealthRegistryBuilder>,
) -> Pin<FutureMaybeStore<'a>> {
    Box::pin(async move {
        let store: Arc<dyn StoreDriver> = match backend {
            StoreSpec::FastSlow(spec) => FastSlowStore::new(
                spec,
                store_factory(&spec.fast, store_manager, None).await?,
                store_factory(&spec.slow, store_manager, None).await?,
            ),
            StoreSpec::Compression(spec) => CompressionStore::new(
                &spec.clone(),
                store_factory(&spec.backend, store_manager, None).await?,
            )?,
            // ... recursive construction for all composite types
        };
        Ok(Store::new(store))
    })
}
```

**Source:** [`nativelink-store/src/default_store_factory.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-store/src/default_store_factory.rs)

The factory is recursive: composite stores call `store_factory` on their children, building the tree from leaves to root. After all named stores are constructed, a `post_init` pass resolves `RefStore` cross-references (stores that point to other named stores by name).

## Patterns

**The production CAS pattern:**
```
ExistenceCache → Compression → FastSlow(Memory, S3)
```

**The CI cache pattern:**
```
FastSlow(Filesystem, Noop)  // local disk, no durable backend
```

**The multi-region pattern:**
```
FastSlow(
  fast: Memory,
  slow: Shard([GrpcStore(region-1), GrpcStore(region-2)])
)
```

**The hardened-CAS pattern:**
```
Verify → ExistenceCache → FastSlow(Memory, S3)
```
`verify` checks hash and size on upload so corrupt bytes never land; the
existence cache spares the slow tier a `has` on every write. Note that
`verify` and `completeness_checking` both reject non-digest keys, so a
string-keyed store must sit outside them.

The algebra is small. The compositions are infinite.
