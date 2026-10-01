# Container-Based Toolchains

If you can't or won't use Nix, containers are the next-best approach to toolchain management. The idea: package your toolchain into a container image, pin it by digest, and run every action inside that image.

## The Model

```
┌─────────────────────────────────────────────────────┐
│  Dockerfile (defines toolchain)                      │
├─────────────────────────────────────────────────────┤
│  Container Registry (stores images by digest)        │
├─────────────────────────────────────────────────────┤
│  Platform Property: container-image=sha256:abc...    │
├─────────────────────────────────────────────────────┤
│  Worker Entrypoint (pulls image, runs action inside) │
└─────────────────────────────────────────────────────┘
```

The container image is the toolchain. The image digest is the identity. If two workers pull the same digest, they have the same toolchain.

## Building Toolchain Images

A minimal C++ toolchain image:

```dockerfile
FROM ubuntu:22.04@sha256:abc123...
RUN apt-get update && apt-get install -y \
    clang-17 \
    lld-17 \
    libc++-17-dev \
    && rm -rf /var/lib/apt/lists/*
ENV CC=/usr/bin/clang-17
ENV CXX=/usr/bin/clang++-17
```

**Critical: pin the base image by digest, not tag.** `ubuntu:22.04` is mutable (it changes with security updates). `ubuntu:22.04@sha256:abc123...` is immutable. Mutable tags break cache correctness.

## NativeLink Worker Image Builder

Two distinct models exist here, and the shipped tooling implements only one of them. The first bakes the toolchain *into* the worker image, so the worker's own root filesystem **is** the toolchain — there is no nested container per action. The second (the [`container-image` property](#worker-configuration) below) keeps a generic worker and runs each action inside a container the entrypoint launches. NativeLink ships the first; the second is yours to wire up.

NativeLink provides a Nix function that packages a set of toolchain derivations together with worker scaffolding into an OCI image:

**Source:** [`tools/create-worker-experimental.nix`](https://github.com/TraceMachina/nativelink/blob/main/tools/create-worker-experimental.nix)

Given `packagesForImage` (the toolchain), it produces an image containing:
- The toolchain derivations, flattened to `/` via `buildEnv` (`create-worker-experimental.nix:82-88`)
- A `nativelink` user (uid/gid 1000) and the `/etc/passwd`, `/etc/group`, PAM scaffolding a login needs (`create-worker-experimental.nix:38-63`)
- `/tmp` (mode `1777`) and a `/usr/bin/env` → `/bin/env` symlink so `#!/usr/bin/env` shebangs resolve (`create-worker-experimental.nix:13-30`)

Two caveats keep this honest. It is **experimental** — the filename says so, and the API is not stable. And it is an **abridged worker image**: the image config sets only `User`, `WorkingDir`, and OCI `Labels` (`create-worker-experimental.nix:96-108`) — no `Cmd`, no `Entrypoint`, and no `docker`/`podman` binary. The toolchain is the worker's filesystem, not something the worker shells out to a container to reach. You still supply the runtime worker config (`worker_api_endpoint`, `cas_fast_slow_store`, `work_directory`) when you launch it.

## Buck2 Container Toolchain

For Buck2, NativeLink provides a complete toolchain image builder:

**Source:** [`tools/toolchain-buck2/toolchain-buck2.sh`](https://github.com/TraceMachina/nativelink/blob/main/tools/toolchain-buck2/toolchain-buck2.sh)

This script:
1. `docker buildx build --push` builds the base Buck2 toolchain image and pins its `RepoDigest` into `flake.nix` (`toolchain-buck2.sh:41-59`)
2. Wraps that toolchain into a NativeLink *worker* image via the `create-worker-experimental.nix` builder — `nix run .#nativelink-worker-toolchain-buck2.copyTo` (`toolchain-buck2.sh:104-107`)
3. Publishes the worker image to Amazon ECR when `ECR_PUBLISH=true` (`toolchain-buck2.sh:109-115`)

The output is again the baked model: a worker whose filesystem carries the Buck2 toolchain, not a generic worker that `docker run`s a Buck2 image per action.

## Worker Configuration

The second model keeps a generic worker and runs each action inside a container the worker's `entrypoint` launches. **NativeLink ships no `docker run` wrapper for this.** The worker's execution path builds the action command directly — `process::Command::new(program)` (`running_actions_manager.rs:1373`), where `program` is the action's own arguments, optionally prefixed by the configured `entrypoint` (`running_actions_manager.rs:1232-1242`). No `container-run.sh` exists in the tree; the worker never invokes `docker` on its own. To containerize each action you write that entrypoint script yourself and point `entrypoint` at it.

The worker advertises a `container-image` platform property and passes the requested image to your entrypoint through the environment:

```json5
workers: [{
  local: {
    // ...worker_api_endpoint, cas_fast_slow_store, work_directory elided...
    entrypoint: "/usr/local/bin/container-run.sh",  // YOUR script, not shipped
    platform_properties: {
      OSFamily: { values: ["Linux"] },
      "container-image": { values: [""] },  // advertise the KEY (see below)
      cpu_count: { query_cmd: "nproc" }
    },
    additional_environment: {
      CONTAINER_IMAGE: { property: "container-image" },
      ACTION_DIRECTORY: "action_directory",
      ACTION_TIMEOUT: "timeout_millis"
    }
  }
}]
```

The `additional_environment` values follow the `EnvironmentSource` enum (`cas_server.rs:992-1037`). Only the two data-carrying variants take a map: `Property` → `{ property: "…" }` and `Value` → `{ value: "…" }`. The unit variants are **bare strings**: `"from_environment"`, `"timeout_millis"`, `"side_channel_file"`, `"action_directory"` (the `snake_case` rename of each variant). Writing `{ action_directory: {} }` for a unit variant fails config load with `invalid type: map, expected unit`. This mirrors the shipped `nativelink-config/examples/basic_cas.json5:79-86`.

The scheduler classifies `container-image` as a `priority` property (`basic_cas.json5:60`). Priority does not compare the *value* — any worker value satisfies any requested value (`platform_properties.rs:150`) — **but the worker must still advertise the key**. `PlatformProperties::is_satisfied_by` returns `false` for a requested property the worker does not declare at all (`platform_properties.rs:70-75`), and the enum's own doc comment is explicit: "the worker must have the associated key present to be matched" (`platform_properties.rs:115-117`). That is why the worker config above lists `"container-image": { values: [""] }` — the empty value is irrelevant, but the key's *presence* is what makes the match succeed. If you want a key jobs may request that workers need not declare, that is the separate `ignore` type (`schedulers.rs:62-64`), not `priority`.

Your entrypoint script reads the image from the environment and does the containerizing:

```bash
#!/bin/bash
# container-run.sh — operator-supplied; NativeLink does not ship this.
set -euo pipefail

IMAGE="${CONTAINER_IMAGE}"
WORKDIR="${ACTION_DIRECTORY}"
TIMEOUT_MS="${ACTION_TIMEOUT:-600000}"
TIMEOUT_S=$((TIMEOUT_MS / 1000))

exec timeout "${TIMEOUT_S}" docker run --rm \
  --network=none \
  -v "${WORKDIR}:${WORKDIR}" \
  -w "${WORKDIR}" \
  "${IMAGE}" "$@"
```

Config load parses every block under `deny_unknown_fields`, enforces the `EnvironmentSource` variant shapes above, and resolves store references before the process binds a socket — so a bad variant shape fails fast at startup rather than silently.

## Client Configuration

### Bazel

```bash
# .bazelrc
build --remote_executor=grpc://nativelink:50051
build --remote_default_exec_properties=container-image=docker://registry.example.com/toolchain@sha256:abc123
build --remote_default_exec_properties=OSFamily=Linux
```

### Buck2

```ini
# .buckconfig
[buck2_re_client]
engine_address = nativelink:50051
action_cache_address = nativelink:50051
cas_address = nativelink:50051
instance_name = main
```

```python
# platforms/defs.bzl
platform = ExecutionPlatformInfo(
    label = ctx.label.raw_target(),
    configuration = configuration,
    executor_config = CommandExecutorConfig(
        local_enabled = False,
        remote_enabled = True,
        remote_execution_properties = {
            "container-image": "docker://registry.example.com/toolchain@sha256:abc123",
            "OSFamily": "Linux",
        },
        remote_execution_use_case = "buck2-default",
    ),
)
```

## The Digest Pinning Problem

Tags are mutable. Digests are not. This is critical.

```
# BAD - tag can change, breaking cache correctness:
container-image: docker://my-toolchain:latest

# BAD - tag can change even with a version:
container-image: docker://my-toolchain:v1.2.3

# GOOD - digest is immutable content-addressing:
container-image: docker://my-toolchain@sha256:a1b2c3d4e5f6...
```

If you use a mutable tag:
1. Monday: worker pulls `my-toolchain:latest` (resolves to digest A)
2. Tuesday: you push a new image with the same tag (now resolves to digest B)
3. Tuesday: some workers have A, some have B
4. Action hashes are identical (same `container-image` property value)
5. Cache serves results built with A to machines running B
6. Builds are silently broken

With digest pinning, step 2 would change the property value (different digest = different property = different action hash = cache miss). Correctness is maintained.

## Container Startup Overhead

The main cost of container-based toolchains is startup time. Every action incurs:
- Container creation: ~50-200ms
- Image layer mount: ~10-50ms (if already pulled)
- Image pull: seconds to minutes (first time only)

For builds with thousands of sub-second actions (e.g., individual C++ compilations), this overhead is unacceptable. Options:
- **Persistent workers** — keep the container running, send multiple actions to it (see deployment chapter)
- **Larger action granularity** — compile multiple files per action
- **Local execution for fast actions** — only use remote execution for slow actions (tests, links)
- **Pre-pulled images** — ensure workers have the image cached before accepting actions

## Multi-Language, Multi-Image

Different actions can use different container images. A C++ compilation might use `clang-toolchain@sha256:...` while a Rust build uses `rust-toolchain@sha256:...`. The scheduler's `priority` type for `container-image` passes the value through without restricting worker matching on it — so the same worker can handle both, provided it advertises the `container-image` key at all (`platform_properties.rs:70-75`, as discussed under [Worker Configuration](#worker-configuration)).

```python
# Bazel platform for C++
platform(
    name = "cpp_platform",
    exec_properties = {
        "container-image": "docker://registry/cpp-toolchain@sha256:aaa...",
    },
)

# Bazel platform for Rust
platform(
    name = "rust_platform",
    exec_properties = {
        "container-image": "docker://registry/rust-toolchain@sha256:bbb...",
    },
)
```

Each platform produces different action hashes (different `container-image` property) even for the same source files. Actions land on the same workers but execute in different containers. Cache correctness is maintained because the platform is part of the action hash.
