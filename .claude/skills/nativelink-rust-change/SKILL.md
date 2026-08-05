---
name: nativelink-rust-change
description: Use when implementing or reviewing Rust changes in NativeLink crates, especially storage, scheduler, worker, service, config, and shared utility behavior.
---

# NativeLink Rust Change Workflow

Use this skill for Rust implementation work in `nativelink/`.

## Read The Local Contract First

Before editing, inspect the trait, caller, and tests around the change. NativeLink has several shared contracts where small behavior changes can affect remote execution correctness.

Crate map:

- `nativelink-store/`: storage backends, CAS/AC, compression, Redis/S3/GCS/filesystem/memory stores.
- `nativelink-scheduler/`: scheduling, worker matching, action queues, awaited action state.
- `nativelink-worker/`: worker execution, input materialization, process execution, output upload.
- `nativelink-service/`: REAPI, bytestream, BEP, worker API, health checks.
- `nativelink-config/`: JSON5 config structs and validation.
- `nativelink-util/`: digest, filesystem, proto, retry, metrics, TLS, and async helpers.
- `nativelink-proto/`: protobuf-generated API surfaces.

## Implementation Rules

- Prefer existing traits and helper types over new abstractions.
- Preserve error semantics. Map user/config/input mistakes to explicit error codes and reserve internal errors for unexpected failures.
- Avoid blocking filesystem, process, or network work inside async code unless the surrounding code already does so intentionally.
- Keep metrics labels bounded and consistent with existing metric names.
- For storage and scheduler changes, think through cancellation, retries, duplicate work, and missing CAS entries.
- For worker changes, preserve stdout/stderr, exit status, timeouts, and output upload behavior.
- For service changes, preserve protocol compatibility and avoid changing response shape without tests.

## Tests

Add or update the closest existing tests first. Prefer focused tests that prove the contract:

```bash
bazel test //nativelink-store/...
bazel test //nativelink-scheduler/...
bazel test //nativelink-worker/...
bazel test //nativelink-service/...
```

For narrow Cargo-side checks:

```bash
cargo test --all --profile=smol
```

For server-wide confidence:

```bash
bazel build //:nativelink
bazel test //...
```

## Final Report

Summarize:

- The behavior changed.
- Files touched.
- Focused tests run.
- Broader tests skipped or still needed.
