# From Scratch: Bazel + NativeLink

A complete walkthrough: from zero to working remote cache and execution with Bazel and NativeLink. No assumptions beyond "you have a machine with Docker."

## Step 1: Start NativeLink

Create `nativelink.json5`:

```json5
{
  stores: [
    {
      name: "AC_STORE",
      filesystem: {
        content_path: "/tmp/nativelink/ac",
        temp_path: "/tmp/nativelink/ac-tmp",
        eviction_policy: { max_bytes: "512mb" }
      }
    },
    {
      // The worker downcasts its CAS to a `FastSlowStore` at boot
      // (`local_worker.rs:556`) and refuses to start otherwise, so the store
      // that the worker and the servers share MUST be `fast_slow` — even for a
      // single local worker. The `fast` tier must be a `filesystem` store
      // because the worker hardlinks from it into the action sandbox
      // (`cas_server.rs:1197`); `slow` is `noop` here since there is no durable
      // tier behind this box.
      name: "CAS_STORE",
      fast_slow: {
        fast: {
          filesystem: {
            content_path: "/tmp/nativelink/cas",
            temp_path: "/tmp/nativelink/cas-tmp",
            eviction_policy: { max_bytes: "5gb" }
          }
        },
        slow: { noop: {} }
      }
    }
  ],
  schedulers: [{
    name: "MAIN_SCHEDULER",
    simple: {
      supported_platform_properties: {
        cpu_count: "minimum",
        OSFamily: "priority",
        "container-image": "priority"
      }
    }
  }],
  workers: [{
    local: {
      worker_api_endpoint: { uri: "grpc://127.0.0.1:50061" },
      cas_fast_slow_store: "CAS_STORE",
      upload_action_result: { ac_store: "AC_STORE" },
      work_directory: "/tmp/nativelink/work",
      platform_properties: {
        cpu_count: { query_cmd: "nproc" },
        OSFamily: { values: ["Linux"] },
        "container-image": { values: [""] }
      }
    }
  }],
  servers: [
    {
      name: "public",
      listener: { http: { socket_address: "0.0.0.0:50051" } },
      services: {
        cas: [{ instance_name: "", cas_store: "CAS_STORE" }],
        ac: [{ instance_name: "", ac_store: "AC_STORE" }],
        execution: [{
          instance_name: "",
          cas_store: "CAS_STORE",
          scheduler: "MAIN_SCHEDULER"
        }],
        capabilities: [{
          instance_name: "",
          remote_execution: { scheduler: "MAIN_SCHEDULER" }
        }],
        bytestream: [{ instance_name: "", cas_store: "CAS_STORE" }]
      }
    },
    {
      name: "worker_api",
      listener: { http: { socket_address: "0.0.0.0:50061" } },
      services: {
        worker_api: { scheduler: "MAIN_SCHEDULER" },
        health: {}
      }
    }
  ]
}
```

Note: `instance_name: ""` (empty string) matches Bazel's default.

One invariant to watch: the worker requires its CAS to be a `FastSlowStore`
(`cas_server.rs:1197`), enforced when the worker starts (`local_worker.rs:556`),
which is exactly why `CAS_STORE` above is wrapped in `fast_slow` and not a bare
`filesystem`. A plain `filesystem` CAS aborts the worker at Step 1.

### Start it

```bash
docker run -d --name nativelink \
  -v $(pwd)/nativelink.json5:/config.json5 \
  -p 50051:50051 -p 50061:50061 \
  ghcr.io/tracemachina/nativelink:latest /config.json5
```

Verify it is serving. Watch for the two listener lines and the worker
registration — all three come from this one process, since the config runs the
servers and the local worker together:

```bash
docker logs nativelink 2>&1 | grep -E "Ready, listening|Worker registered"
# Ready, listening on 0.0.0.0:50051     <- public gRPC services (CAS/AC/execution)
# Ready, listening on 0.0.0.0:50061     <- worker_api listener
# Worker registered with scheduler      <- the local worker connected; ready to build
```

`Worker registered with scheduler` (`local_worker.rs:884`) is the signal that
remote execution in Step 4 will actually land on a worker rather than hang with
no capacity.

## Step 2: Create a Bazel Project

```bash
mkdir my-project && cd my-project
```

`MODULE.bazel`:
```python
module(name = "my-project", version = "0.0.0")
bazel_dep(name = "rules_cc", version = "0.2.18")
```

`BUILD.bazel`:
```python
cc_binary(
    name = "hello",
    srcs = ["hello.cc"],
)
```

`hello.cc`:
```cpp
#include <iostream>
int main() {
    std::cout << "Hello from remote execution!" << std::endl;
    return 0;
}
```

## Step 3: Configure Remote Cache

`.bazelrc`:
```bash
# Remote cache only (no execution yet)
build --remote_cache=grpc://127.0.0.1:50051
build --remote_upload_local_results=true
```

Build:
```bash
bazel build //:hello
# First build: compiles locally, uploads to cache
# INFO: 2 processes: 1 internal, 1 linux-sandbox

bazel clean
bazel build //:hello
# Second build: everything from cache
# INFO: 2 processes: 1 internal, 1 remote cache hit
```

You now have working remote caching.

## Step 4: Enable Remote Execution

Add to `.bazelrc`:
```bash
# Remote execution
build --remote_executor=grpc://127.0.0.1:50051
build --remote_default_exec_properties=cpu_count=1
```

```bash
bazel clean
bazel build //:hello
# INFO: 2 processes: 1 internal, 1 remote
# The compilation ran on the NativeLink worker
```

## Step 5: Verify Cache Sharing

Open a second terminal. Clone the same project to a different directory:

```bash
cd /tmp && mkdir project-copy && cd project-copy
# Copy the source files and MODULE.bazel/BUILD.bazel/.bazelrc

bazel build //:hello
# INFO: 2 processes: 1 internal, 1 remote cache hit
# Cache hit! The action was already cached from the first build.
```

Two separate workspaces, same cache. This is the value of remote caching.

## Step 6: Add a Hermetic Toolchain

The default setup uses the host toolchain, which breaks cache sharing across machines. Let's add zig-cc for hermetic C++ compilation:

Update `MODULE.bazel`:
```python
module(name = "my-project", version = "0.0.0")
bazel_dep(name = "rules_cc", version = "0.2.18")
bazel_dep(name = "hermetic_cc_toolchain", version = "4.0.1")

zig = use_extension("@hermetic_cc_toolchain//toolchain:ext.bzl", "toolchains")
use_repo(zig, "zig_sdk")
```

Add to `.bazelrc`:
```bash
# Hermetic C++ toolchain
build --extra_toolchains @zig_sdk//toolchain:linux_amd64_gnu.2.28
```

```bash
bazel build //:hello
# Now uses zig-cc. Same binary on every machine.
# Cache hits will work across different developer machines.
```

## What You've Built

```
Client (Bazel) ──grpc──► NativeLink (CAS + AC + Scheduler + Worker)
                         │
                         ├── CAS: /tmp/nativelink/cas (stores artifacts by hash)
                         ├── AC:  /tmp/nativelink/ac  (stores action results)
                         └── Worker: executes actions in /tmp/nativelink/work
```

Next steps:
- Replace filesystem stores with S3 for durability and sharing across machines
- Add more workers (Docker Compose or Kubernetes)
- Add TLS for production security
- Add observability (metrics + alerting)
