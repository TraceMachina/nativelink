---
name: nativelink-bazel-verification
description: Use when verifying NativeLink changes with Bazel, selecting focused test targets, debugging rustfmt/clippy aspect failures, or preparing a confidence report for Rust/Bazel changes.
---

# NativeLink Bazel Verification

Use this skill when a NativeLink change needs a focused Bazel build/test plan, when CI reports a Bazel failure, or when you need to decide how much verification is enough.

## Start With Scope

1. Inspect the touched files and nearby `BUILD.bazel` files.
2. Map changed Rust files to their crate package:
   - `nativelink-store/` for storage backends, CAS/AC, compression, Redis/S3/GCS/filesystem/memory stores.
   - `nativelink-scheduler/` for action scheduling and worker matching.
   - `nativelink-worker/` for local worker execution and action lifecycle.
   - `nativelink-service/` for REAPI, bytestream, BEP, and worker API services.
   - `nativelink-config/` for JSON5 configuration structs and examples.
   - `nativelink-util/` for shared helpers.
   - `nativelink-proto/` for protobuf-generated surfaces.
3. Prefer focused package tests first, then broaden only when the touched surface is shared.

## Common Commands

Use the repository's Bazel defaults unless the user asked for a specific Bazel version.

```bash
bazel build //:nativelink
bazel build //...
bazel test //...
bazel test //nativelink-store:memory_store_test
bazel run --config=rustfmt @rules_rust//:rustfmt
```

For this workspace, when the user requests Bazelisk 9.0.2:

```bash
USE_BAZEL_VERSION=9.0.2 bazelisk --output_user_root=/tmp/bazelisk-9.0.2 --batch build //:nativelink
USE_BAZEL_VERSION=9.0.2 bazelisk --output_user_root=/tmp/bazelisk-9.0.2 --batch test //...
```

## Failure Handling

- If `rustfmt` fails, run:

```bash
bazel run --config=rustfmt @rules_rust//:rustfmt
```

- If `clippy` fails, fix the code. Don't silence lints unless the suppression is local, justified, and consistent with existing code.
- If a test fails under a broad target like `//...`, rerun the specific failing target with output enabled before editing.
- If Bazel can't find a target, inspect `BUILD.bazel` rather than guessing labels.

## Verification Ladder

Use the smallest rung that proves the change:

1. Single package test for narrow implementation changes.
2. `bazel build //:nativelink` for server-path changes.
3. Package-wide tests for crate-level behavior.
4. `bazel test //...` for shared crates, proto/config surfaces, toolchain changes, or release confidence.

## Final Report

Always report:

- The exact commands run.
- Whether each command passed or failed.
- Any known unverified area.
- Any pre-existing dirty files that weren't part of the task.
