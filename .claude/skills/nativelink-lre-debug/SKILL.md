---
name: nativelink-lre-debug
description: Use when debugging NativeLink local remote execution, remote cache/executor self-tests, BEP event ingestion, worker scheduling, CAS misses, or Bazel remote execution behavior.
---

# NativeLink Local Remote Execution Debugging

Use this skill when NativeLink is acting as a remote cache, remote executor, local remote execution server, or BEP target and something is missing, slow, or failing.

## Identify The Mode

Start by recording the exact Bazel command and which NativeLink mode is involved:

- Remote cache self-test: `--config=self_test`, usually `grpc://127.0.0.1:50051`.
- Remote executor self-test: `--config=self_execute`, usually `grpc://127.0.0.1:50052`.
- Local remote execution overlays under `local-remote-execution/`.
- BEP ingestion or dashboard testing, which needs Build Event Protocol upload flags.

For rich action-level BEP data, include:

```bash
--build_event_publish_all_actions
```

This matters for helping users find build failures and slow actions faster.

## First Checks

1. Confirm the NativeLink server process is running.
2. Confirm the expected ports aren't occupied by stale processes.
3. Capture NativeLink logs and the exact Bazel stdout/stderr.
4. Check whether the failure is CAS/AC storage, scheduler matching, worker execution, protocol, or client config.

Don't kill user services unless the user explicitly asks. Inspect ports and processes first.

## Subsystem Map

- CAS/AC or cache misses: start in `nativelink-store/`.
- Worker execution failure: start in `nativelink-worker/`.
- Queueing, platform properties, or worker compatibility: start in `nativelink-scheduler/`.
- REAPI, bytestream, action cache, or BEP API behavior: start in `nativelink-service/`.
- Client templates and flags: start in `templates/` and `local-remote-execution/`.

## Useful Verification Commands

Inspect labels before running if unsure.

```bash
bazel build //:nativelink
bazel test //nativelink-store/...
bazel test //nativelink-scheduler/...
bazel test //nativelink-worker/...
bazel test //nativelink-service/...
```

For full confidence:

```bash
bazel test //...
```

## Debug Output To Preserve

When reporting a failure, include:

- NativeLink server config path and relevant ports.
- Bazel remote cache/executor URL.
- Whether `--build_event_publish_all_actions` was set.
- First server-side error and first client-side error.
- The failing target label and action mnemonic, if available.
