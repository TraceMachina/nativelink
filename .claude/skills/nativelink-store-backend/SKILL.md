---
name: nativelink-store-backend
description: Use when implementing or modifying a NativeLink store — a base backend (filesystem, memory, redis, s3, gcs, azure_blob, mongo, ontap_s3, grpc) or a decorator that wraps another store (fast_slow, compression, dedup, verify, existence_cache, completeness_checking, shard, size_partitioning, ref). Covers the StoreDriver trait, streaming reads/writes, the zero-digest fast-path, wiring a new StoreSpec variant, config + tests.
---

# NativeLink Store Backend Workflow

Use this skill when adding a new store, changing an existing store's behavior, or reviewing store code in `nativelink-store/`. Stores are a shared correctness contract for CAS/AC, so read the trait and the nearest existing store before editing.

## The Store Contract

Everything implements `StoreDriver` in `nativelink-util/src/store_trait.rs`. The three methods every store must provide:

- `has_with_results(keys, results)`: existence + size for each key (`None` = absent).
- `update(key, reader, size_info)`: write, consuming a streaming `DropCloserReadHalf`.
- `get_part(key, writer, offset, length)`: read a byte range into a streaming `DropCloserWriteHalf`, ending with `writer.send_eof()`.

Keys are `StoreKey` = `Digest(DigestInfo)` (CAS/AC content) or `Str(Cow)` (named keys). Data is **streamed** via `buf_channel`; never buffer whole blobs in memory, they can be gigabytes. Optional optimizations you may override: `optimized_for`, `update_with_whole_file`, `update_oneshot`, `get_part_unchunked`.

Two kinds of store:

- **Base stores** persist bytes themselves (`filesystem`, `memory`, `redis_store`, the cloud providers under `experimental_cloud_object_store`, `experimental_mongo`, `ontap_s3`, `grpc`, `noop`).
- **Decorator stores** wrap an inner `Arc<dyn StoreDriver>` and add behavior (`fast_slow`, `compression`, `dedup`, `verify`, `existence_cache`, `completeness_checking`, `shard`, `size_partitioning`, `ref_store`).

## Zero-Digest Fast-Path (issue #2034)

The empty blob has a fixed, universally-known digest. Base stores must short-circuit it in all three ops instead of hitting the backend. See `is_zero_digest` / `ZERO_BYTE_DIGESTS` in `nativelink-store/src/cas_utils.rs`, and the pattern in `nativelink-store/src/mongo_store.rs`:

- `has_with_results`: report size `0` (present), skip the backend query.
- `update`: drain/peek the reader, write nothing.
- `get_part`: `writer.send_eof()` immediately, no backend read.

Pure pass-through decorators (`shard`, `size_partitioning`, `ref_store`, `noop`) usually don't need it.

## Adding A New Store

1. Add a `Spec` struct + a `StoreSpec` variant in `nativelink-config/src/stores.rs`: `#[serde(...)]`, `#[cfg_attr(feature = "dev-schema", derive(JsonSchema))]`, and a `///` doc comment with an example JSON block (the doc comment becomes the config reference text and the `store-specs.json` description).
2. Implement the store in `nativelink-store/src/<name>_store.rs` and export it from `nativelink-store/src/lib.rs`.
3. Wire the variant in `nativelink-store/src/default_store_factory.rs` (`match backend { StoreSpec::YourStore(spec) => ... }`).
4. Regenerate the schema snapshot so docs/llms.txt stay in sync — the `generate-store-specs` pre-commit hook does this. Never hand-edit `web/apps/docs/lib/store-specs.json` or the generated config reference.
5. Add a runnable example under `nativelink-config/examples/` (these are syntax-checked by tests).

## Implementation Rules

- Stream; don't collect whole blobs. Respect `offset`/`length` in `get_part`.
- Preserve error semantics: `make_err!(Code::..., ...)` — config/input mistakes get explicit codes; reserve `Code::Internal` for unexpected failures.
- Never call `tokio::spawn` / `std::thread::spawn` / `block_on` — use `nativelink-util::task` (enforced by `clippy.toml`).
- Network stores: use the shared retry helpers, bound concurrency, and think through cancellation, retries, and missing entries.
- Local stores: use `EvictingMap` (`nativelink-util/src/evicting_map.rs`) and honor the configured `eviction_policy`.
- Expose metrics via `MetricsComponent` with bounded, consistent labels; register health if the backend can be unhealthy.
- Mind CAS vs AC intent: some stores are CAS-only (`existence_cache`) or AC-only (`completeness_checking`), and an instance's `cas_store` and `ac_store` may not be the same store (enforced since v1.5.0).
- Copy the license header from any existing source file when creating a new file.

## Tests

Model a new test on the closest existing one in `nativelink-store/tests/` (e.g. `memory_store_test.rs`, `filesystem_store_test.rs`), covering has/get/update, byte ranges, and the zero-digest fast-path.

```bash
bazel test //nativelink-store/...
bazel test //nativelink-store:memory_store_test   # single target
cargo test --all --profile=smol                   # faster Cargo-side loop
bazel run --config=rustfmt @rules_rust//:rustfmt  # format
```

## Final Report

Summarize:

- The store added/changed and its base-vs-decorator role.
- Whether the zero-digest fast-path is handled.
- Config/schema wiring done (StoreSpec variant, factory arm, regenerated snapshot, example).
- Focused tests run and their result; any broader tests still needed.
