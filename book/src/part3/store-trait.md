# The Store Trait

Everything in NativeLink is built on one abstraction: the store. CAS is a store. AC is a store. Worker artifact storage is a store. The filesystem cache, the S3 backend, the compression layer, the deduplication layer — all stores. Understanding this trait is understanding NativeLink.

## The Interface

The store trait has two layers. `StoreDriver` is what you implement. `StoreLike` is what you call. They're separated to allow the outer layer (`Store`) to provide convenience methods and type erasure without burdening implementors.

The core operations are three:

```rust
// nativelink-util/src/store_trait.rs

/// Check if a key exists. Returns Some(size) if found, None if not.
async fn has(self: Pin<&Self>, key: StoreKey<'_>)
    -> Result<Option<u64>, Error>;

/// Upload content for a key.
async fn update(self: Pin<&Self>, key: StoreKey<'_>,
    reader: DropCloserReadHalf, upload_size: UploadSizeInfo)
    -> Result<u64, Error>;

/// Download content for a key (with optional range).
async fn get_part(self: Pin<&Self>, key: StoreKey<'_>,
    writer: &mut DropCloserWriteHalf, offset: u64, length: Option<u64>)
    -> Result<(), Error>;
```

That's it. `has`, `update`, `get_part`. Every store — from in-memory hash maps to S3 buckets to compression wrappers — implements these three operations.

**Source:** [`nativelink-util/src/store_trait.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-util/src/store_trait.rs)

## Two-Phase Initialization

`StoreDriver` requires one more method that has nothing to do with I/O:

```rust
// nativelink-util/src/store_trait.rs

// Do "all the stores are setup" init e.g. if we need access to the store manager
// for ref stores
async fn post_init(self: Arc<Self>) -> Result<(), Error>;
```

Stores are constructed first and wired second. A `RefStore` names another store by label, and at construction time that target may not exist yet. `post_init` runs after every store in the config has been built, so a store can resolve its references through the store manager without imposing an ordering constraint on the config file. `RefStore` uses exactly this: its `post_init` looks the target up in the `StoreManager` and caches the handle, and any I/O method called before that resolution fails with `"ref_store cannot get store '…', was post_init called?"` (`nativelink-store/src/ref_store.rs:76-97`). Most leaf stores implement `post_init` as a no-op.

## StoreKey

The key is not a raw hash — it's a `StoreKey` enum that can represent different types of content:

```rust
// nativelink-util/src/store_trait.rs

#[derive(Debug, Eq)]
pub enum StoreKey<'a> {
    /// A string key.
    Str(Cow<'a, str>),

    /// A key that is a digest.
    Digest(DigestInfo),
}
```

The `Digest` variant holds a `DigestInfo` — a fixed 32-byte packed hash plus a `u64` size (`nativelink-util/src/common.rs:40-46`). It is owned `Copy` data, not a borrow: the enum's `'a` lifetime is used only by the `Str` variant, so a digest key never borrows anything. For CAS, keys are always digests. For AC, keys are string-encoded action digests (the AC maps an action hash → result, and the "content" is the serialized `ActionResult` proto).

The two variants are not mutually exclusive at the boundary. `into_digest()` bridges them: a `Str` key that reaches a digest-only backend is BLAKE3-hashed into a `DigestInfo` rather than rejected (`store_trait.rs:237-246`). The comparison traits discriminate by variant — `Ord` sorts every `Str` before every `Digest`, `Eq` never equates the two, and `Hash` mixes in a variant tag before hashing the payload — so a string key and a digest key can never collide even if their bytes coincide (`store_trait.rs:275-316`).

## The Store Wrapper

Concrete store implementations are wrapped in a type-erased `Store` struct:

```rust
// nativelink-util/src/store_trait.rs

#[derive(Clone, MetricsComponent)]
#[repr(transparent)]
pub struct Store {
    #[metric]
    inner: Arc<dyn StoreDriver>,
}
```

This is the handle you pass around. It's `Clone`, `Send`, `Sync`, and cheap to copy (it's an `Arc`). The type erasure means any code that accepts a `Store` works with any backend — callers don't know or care what's behind it.

## Streaming, Not Buffering

Notice that `update` takes a `DropCloserReadHalf` (a streaming reader) and `get_part` writes to a `DropCloserWriteHalf` (a streaming writer). Stores do not buffer entire blobs in memory. Data streams through the system — from the gRPC layer, through the store chain, to the backend.

This is critical for large artifacts. A 2GB compiled binary flows through CAS → compression → S3 as a stream, never fully materialized in memory. The `buf_channel` utility provides backpressure-aware channels that connect these streaming layers.

**Source:** [`nativelink-util/src/buf_channel.rs`](https://github.com/TraceMachina/nativelink/blob/main/nativelink-util/src/buf_channel.rs)

## Batch Operations

The trait also provides batch variants for efficiency:

```rust
async fn has_with_results(self: Pin<&Self>,
    digests: &[StoreKey<'_>], results: &mut [Option<u64>])
    -> Result<(), Error>;
```

`FindMissingBlobs` (the most frequent REAPI call) maps to `has_with_results` — checking hundreds of digests in one round-trip. Store backends can implement this with batched I/O (S3 batch HEAD, Redis MGET, etc.).

## Optimization Hints

Streaming through `buf_channel` is the safe default, but some stores can do better when the caller tells them what it has. A store advertises capabilities through one method:

```rust
fn optimized_for(&self, optimization: StoreOptimizations) -> bool;
```

The argument is an enum of hints the caller can query before choosing a code path:

```rust
// nativelink-util/src/store_trait.rs

pub enum StoreOptimizations {
    /// The store can optimize the upload process when it knows the data is coming from a file.
    FileUpdates,

    /// If the store will ignore the data uploads.
    NoopUpdates,

    /// If the store will never serve downloads.
    NoopDownloads,

    /// If the store will determine whether a key has associated data once a read has been
    /// attempted instead of calling `.has()` first.
    LazyExistenceOnSync,

    /// The store provides an optimized `update_oneshot` implementation that bypasses
    /// channel overhead for direct Bytes writes.
    SubscribesToUpdateOneshot,
}
```

`FileUpdates` is the whole-file path. When the content is already a file on disk, the caller skips streaming and hands over the file directly:

```rust
async fn update_with_whole_file(self: Pin<&Self>, key: StoreKey<'_>,
    path: OsString, file: fs::FileSlot, upload_size: UploadSizeInfo)
    -> Result<(u64, Option<fs::FileSlot>), Error>;
```

The filesystem store uses this to hardlink instead of copy. A caller checks `optimized_for(StoreOptimizations::FileUpdates)` and passes the file handle directly, avoiding a read-copy-write cycle entirely — the filesystem store answers `true` for `FileUpdates` and `SubscribesToUpdateOneshot` and `false` for everything else (`nativelink-store/src/filesystem_store.rs:1347-1352`).

The other hints matter for topology. `NoopUpdates` and `NoopDownloads` describe a store that silently drops writes or never serves reads — the `NoopStore` reports both (`nativelink-store/src/noop_store.rs:78-79`), and `FastSlowStore` queries them to skip a leg entirely instead of shuttling data to a dummy backend (`nativelink-store/src/fast_slow_store.rs:383,447`). `LazyExistenceOnSync` lets a slow backend skip a separate `has()` probe and discover absence during the read itself; the cloud backends (S3, GCS, Azure) advertise it, and `FastSlowStore` honors it (`nativelink-store/src/fast_slow_store.rs:253-258`). The default `optimized_for` on the trait returns `false` for every hint, so a store opts in only where it genuinely wins.

## Zero-Length and Missing Files: Filesystem Store Hardening

The whole-file path meets the real filesystem in `FilesystemStore`, where two edge cases the streaming abstraction papers over become correctness bugs if handled naively.

**Zero-length blobs are never persisted.** By REAPI convention the empty digest is assumed to exist, so the store treats it as present without writing a file: `update` short-circuits with `Ok(0)`, and `has_with_results` synthesizes a `Some(0)` result for any zero digest it is asked about (`nativelink-store/src/filesystem_store.rs:1320-1323,1292-1310`). The trap is on the read side. If `get_file_entry_for_digest` handed back a synthetic `FileEntry` whose `content_path` did not exist, a worker would `hard_link` from that path and silently produce a missing or empty output file in its execution directory. It returns `NotFound` for zero digests instead, forcing callers onto the explicit empty-file path (`fs::create_file`) rather than a hardlink that cannot work (`nativelink-store/src/filesystem_store.rs:1061-1075`).

**A map hit followed by an `open()` ENOENT is a race, not a miss.** Under concurrent same-key writes, an evicted generation's `unref()` renames the shared content path away while a fresh generation is still landing, so the entry the eviction map just handed a reader can momentarily have no file on disk. Treating that as a miss would surface spurious `NotFound` errors under load. `get_part` instead retries a bounded number of times, re-fetching the newer entry each pass so the transient race self-heals; only once the miss persists past the retries does it treat the divergence as genuine, remove the stale entry (so a `FastSlowStore` caller re-populates from the slow store), and surface the error (`nativelink-store/src/filesystem_store.rs:1447-1509`).

These are the kind of details that only matter under real load with real eviction — exactly where a cache is most load-bearing and least forgiving.

## Health and Introspection

Every store participates in health checking:

```rust
async fn check_health(self: Pin<&Self>, namespace: Cow<'static, str>)
    -> HealthStatus;
```

And provides introspection for debugging:

```rust
fn inner_store(&self, digest: Option<StoreKey<'_>>) -> &dyn StoreDriver;
```

`inner_store` lets you walk the composition tree — useful for diagnostics when a specific layer is misbehaving.

## Why This Design Works

The power of this design is **substitutability**. Any store can wrap any other store. Any store can be used as CAS, as AC, or as worker artifact storage. The gRPC service layer doesn't know what's behind the `Store` handle it was given.

This means:
- You can add compression without changing any code — wrap the backend in a `CompressionStore`.
- You can add verification without changing any code — wrap in a `VerifyStore`.
- You can tier storage without changing any code — use a `FastSlowStore`.
- You can shard without changing any code — use a `ShardStore`.

The complexity lives in composition, not in individual implementations. Each store does one thing. The config assembles them into the topology you need.
