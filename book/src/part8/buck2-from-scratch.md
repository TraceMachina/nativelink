# From Scratch: Buck2 + NativeLink

A complete walkthrough: from zero to working remote cache and execution with Buck2 and NativeLink. Same NativeLink server, different client configuration.

## Step 1: Start NativeLink

The only difference from the Bazel config: Buck2 requires `instance_name: "main"`.

Create `nativelink-buck2.json5`:

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
      name: "CAS_STORE",
      fast_slow: {
        fast: {
          filesystem: {
            content_path: "/tmp/nativelink/cas",
            temp_path: "/tmp/nativelink/cas-tmp",
            eviction_policy: { max_bytes: "10gb" }
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
        "container-image": "priority",
        ISA: "exact"
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
        cpu_count: { values: ["16"] },
        OSFamily: { values: [""] },
        "container-image": { values: [""] },
        ISA: { values: ["x86-64"] }
      }
    }
  }],
  servers: [
    {
      name: "public",
      listener: { http: { socket_address: "0.0.0.0:50051" } },
      services: {
        cas: [{ instance_name: "main", cas_store: "CAS_STORE" }],
        ac: [{ instance_name: "main", ac_store: "AC_STORE" }],
        execution: [{
          instance_name: "main",
          cas_store: "CAS_STORE",
          scheduler: "MAIN_SCHEDULER"
        }],
        capabilities: [{
          instance_name: "main",
          remote_execution: { scheduler: "MAIN_SCHEDULER" }
        }],
        bytestream: [{ instance_name: "main", cas_store: "CAS_STORE" }]
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

Start:

```bash
docker run -d --name nativelink \
  -v $(pwd)/nativelink-buck2.json5:/config.json5 \
  -p 50051:50051 -p 50061:50061 \
  ghcr.io/tracemachina/nativelink:latest /config.json5
```

## Step 2: Create a Buck2 Project

```bash
mkdir my-buck2-project && cd my-buck2-project
```

`.buckconfig`:
```ini
[cells]
root = .
prelude = prelude

[cell_aliases]
config = prelude
fbcode = prelude
fbsource = prelude
buck = prelude

[buck2_re_client]
engine_address = 127.0.0.1:50051
action_cache_address = 127.0.0.1:50051
cas_address = 127.0.0.1:50051
tls = false
instance_name = main

[build]
execution_platforms = root//platforms:platforms
```

`platforms/BUCK`:
```python
load(":defs.bzl", "platforms")

platforms(name = "platforms")
```

`platforms/defs.bzl`:
```python
def _platforms(ctx):
    configuration = ConfigurationInfo(
        constraints = {},
        values = {},
    )

    platform = ExecutionPlatformInfo(
        label = ctx.label.raw_target(),
        configuration = configuration,
        executor_config = CommandExecutorConfig(
            local_enabled = True,
            remote_enabled = True,
            use_limited_hybrid = True,
            remote_execution_properties = {
                "OSFamily": "Linux",
            },
            remote_execution_use_case = "buck2-default",
            remote_output_paths = "output_paths",
        ),
    )

    return [
        DefaultInfo(),
        ExecutionPlatformRegistrationInfo(platforms = [platform]),
    ]

platforms = rule(attrs = {}, impl = _platforms)
```

You'll also need a prelude (Buck2's rule definitions). For testing, clone the Buck2 prelude:

```bash
git clone https://github.com/facebook/buck2-prelude prelude
```

`BUCK`:
```python
cxx_binary(
    name = "hello",
    srcs = ["hello.cpp"],
)
```

`hello.cpp`:
```cpp
#include <iostream>
int main() {
    std::cout << "Hello from Buck2 + NativeLink!" << std::endl;
    return 0;
}
```

## Step 3: Build with Remote Execution

```bash
buck2 build //:hello
```

Buck2 will:
1. Analyze the build graph (Starlark evaluation)
2. Upload inputs to NativeLink's CAS
3. Submit the compilation action via Execute RPC
4. Worker compiles, uploads output
5. Buck2 downloads the binary

## Step 4: Verify Cache Hits

```bash
buck2 clean
buck2 build //:hello
# Second build: cache hit, no recompilation
```

## Step 5: Hybrid Execution

The platform config above has `local_enabled = True` and `use_limited_hybrid = True`. This means Buck2 decides per-action whether to run locally or remotely:

- Fast actions (file copies, small computations) → local
- Slow actions (compilation, linking) → remote

You can force all-remote:

```python
executor_config = CommandExecutorConfig(
    local_enabled = False,
    remote_enabled = True,
    remote_execution_properties = { "OSFamily": "Linux" },
    remote_execution_use_case = "buck2-default",
    remote_output_paths = "output_paths",
)
```

## Step 6: Add Toolchain Identity

For cache correctness across machines, add toolchain identity to platform properties:

```python
# For container-based toolchain:
remote_execution_properties = {
    "container-image": "docker://registry/buck2-toolchain@sha256:abc123",
    "OSFamily": "Linux",
},

# For Nix/LRE:
remote_execution_properties = {
    "lre-cc": "/nix/store/abc123-clang-17/bin/clang",
    "OSFamily": "Linux",
},
```

## Key Differences from Bazel Setup

| Aspect | Bazel | Buck2 |
|--------|-------|-------|
| Instance name | `""` (empty) or configurable | `"main"` (convention) |
| Config location | `.bazelrc` (CLI flags) | `.buckconfig` (INI) |
| Platform definition | `BUILD.bazel` + `--extra_execution_platforms` | Starlark rule in `defs.bzl` |
| Toolchain registration | `register_toolchains()` in MODULE.bazel | Prelude toolchain resolution |
| Hybrid execution | `--remote_local_fallback` (basic) | `use_limited_hybrid` (intelligent) |
| Prelude/rules | Built-in + external repos | Must be present as a cell |

## Troubleshooting

### "Failed to connect to RE"

Check:
- Is NativeLink running? `docker logs nativelink`
- Is port 50051 accessible? `nc -z 127.0.0.1 50051`
- Is `tls = false` set in `.buckconfig`? (NativeLink without TLS config = plaintext)

### "Instance name mismatch" / NOT_FOUND errors

The NativeLink config must have `instance_name: "main"` on every service. Check all five service types (cas, ac, execution, capabilities, bytestream).

### "No execution platform found"

Check:
- `execution_platforms` in `.buckconfig` points to the right target
- The platform rule returns `ExecutionPlatformRegistrationInfo`
- The platform has `remote_enabled = True`

### Actions run locally despite remote config

Check:
- `use_limited_hybrid = True` — Buck2 may choose local if it predicts local is faster
- Try `local_enabled = False` to force remote
- Check that NativeLink has a connected worker: look for "worker connected" in logs
