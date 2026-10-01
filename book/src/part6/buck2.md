# Buck2

Buck2 is a first-class REAPI client. Its remote execution support is production-grade — Meta runs it at enormous scale internally. NativeLink supports Buck2 without compromise, and this chapter shows you how to set it up properly.

If you're coming from Bazel, the model is conceptually identical (both speak REAPI) but the configuration surface is completely different. Buck2 configures remote execution through `.buckconfig` and Starlark platform rules, not command-line flags.

## Basic Configuration

All Buck2 remote execution configuration lives in `.buckconfig`:

```ini
# .buckconfig
[buck2_re_client]
engine_address = nativelink.example.com:50051
action_cache_address = nativelink.example.com:50051
cas_address = nativelink.example.com:50051
tls = true
instance_name = main
```

**Source:** [`integration_tests/buck2/.buckconfig`](https://github.com/TraceMachina/nativelink/blob/main/integration_tests/buck2/.buckconfig)

The checked-in file is the local-test form — `localhost:50051` on every address with `tls = false` — because the integration test runs the server and client on one machine. The block above shows the production shape: a real hostname and `tls = true`.

Key differences from Bazel:
- **Single config file** (not CLI flags). Changes require editing `.buckconfig`.
- **Separate addresses** for engine (execution), AC, and CAS. They can point to different servers (for multi-tier architectures) or the same server.
- **`instance_name` is required.** Buck2 always sends an instance name. Convention is `"main"`.
- **TLS is per-connection**, not per-flag.

## The Instance Name Requirement

Buck2 **always** sends `instance_name: "main"` (or whatever you configure). Your NativeLink server config must match:

```json5
services: {
  cas: [{ instance_name: "main", cas_store: "CAS_STORE" }],
  ac: [{ instance_name: "main", ac_store: "AC_STORE" }],
  execution: [{ instance_name: "main", cas_store: "CAS_STORE", scheduler: "SCHEDULER" }],
  capabilities: [{ instance_name: "main", remote_execution: { scheduler: "SCHEDULER" } }],
  bytestream: [{ instance_name: "main", cas_store: "CAS_STORE" }]
}
```

**Source:** [`integration_tests/buck2/buck2_cas.json5`](https://github.com/TraceMachina/nativelink/blob/main/integration_tests/buck2/buck2_cas.json5)

If instance names don't match, you'll get `NOT_FOUND` errors on every request. Bazel defaults to empty string; Buck2 defaults to `"main"`. This is the most common cross-client configuration mistake.

## Execution Platforms

Buck2's execution platform is defined in Starlark using `ExecutionPlatformInfo`:

```python
# platforms/defs.bzl

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
                "container-image": "docker://toolchain@sha256:abc123",
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

**Source:** [`integration_tests/buck2/platforms/defs.bzl`](https://github.com/TraceMachina/nativelink/blob/main/integration_tests/buck2/platforms/defs.bzl)

The checked-in rule ships with an **empty** `remote_execution_properties = {}`: the integration test's worker advertises empty values for `OSFamily`, `container-image`, and `lre-rs` (`buck2_cas.json5:97-111`), so on a single host there is no toolchain to pin. The properties shown above are what you add once workers diverge and the toolchain has to be identified explicitly.

Then register the platform:

```python
# platforms/BUCK
load(":defs.bzl", "platforms")

platforms(name = "platforms")
```

```ini
# .buckconfig
[build]
execution_platforms = root//platforms:platforms
```

### CommandExecutorConfig Fields

| Field | Purpose |
|-------|---------|
| `local_enabled` | Allow local execution as fallback |
| `remote_enabled` | Enable remote execution |
| `use_limited_hybrid` | Try local first, fall back to remote (or vice versa) |
| `remote_execution_properties` | Platform properties sent in Execute RPC |
| `remote_execution_use_case` | Opaque string for server-side routing |
| `remote_output_paths` | Use `"output_paths"` for REAPI v2.1+ output handling |

### Hybrid Execution

Buck2's `use_limited_hybrid` mode runs some actions locally and some remotely, choosing based on which is likely faster. This is more sophisticated than Bazel's `--remote_local_fallback` — Buck2 can race local vs remote and take whichever finishes first.

Configuration:
```python
executor_config = CommandExecutorConfig(
    local_enabled = True,
    remote_enabled = True,
    use_limited_hybrid = True,
    # When hybrid is enabled, Buck2 decides per-action
    # based on predicted execution time
)
```

For actions that are fast locally (small file copies, small compilations), hybrid mode avoids the network round-trip. For slow actions (linking, testing), it uses remote execution.

## NativeLink Server Config for Buck2

A complete minimal config:

```json5
{
  stores: [
    {
      name: "AC_MAIN_STORE",
      filesystem: {
        content_path: "/data/ac/content",
        temp_path: "/data/ac/tmp",
        eviction_policy: { max_bytes: "1gb" }
      }
    },
    {
      name: "CAS_STORE",
      fast_slow: {
        fast: {
          filesystem: {
            content_path: "/data/cas/content",
            temp_path: "/data/cas/tmp",
            eviction_policy: { max_bytes: "50gb" }
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
      upload_action_result: { ac_store: "AC_MAIN_STORE" },
      work_directory: "/data/work",
      platform_properties: {
        cpu_count: { query_cmd: "nproc" },
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
        ac: [{ instance_name: "main", ac_store: "AC_MAIN_STORE" }],
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
        admin: {},
        health: {}
      }
    }
  ],

  global: { max_open_files: 24576 }
}
```

Note the two server blocks: one public (port 50051) for Buck2 clients, one private (port 50061) for workers. This separation is important for security — the worker API should not be accessible to clients.

## Toolchain Approaches for Buck2

### Container-Based

The most common approach for Buck2. A dedicated container toolchain builder:

**Source:** [`tools/toolchain-buck2/toolchain-buck2.sh`](https://github.com/TraceMachina/nativelink/blob/main/tools/toolchain-buck2/toolchain-buck2.sh)

Set the container image in platform properties:

```python
remote_execution_properties = {
    "container-image": "docker://registry/buck2-toolchain@sha256:...",
}
```

### Nix/LRE for Buck2

LRE's generated configs are Bazel `platform()` rules, so they don't drop into Buck2 unchanged — Buck2 wants `ExecutionPlatformInfo`. The pinning principle carries over, but not the way you might guess: LRE does **not** put Nix store paths in platform properties. It pins the toolchain with a single `container-image` property whose value is the Nix output hash of the worker image, and lets the worker's own Nix closure supply the actual binaries. When the toolchain changes, the tag changes, so the identity is content-addressed:

```python
remote_execution_properties = {
    # C++ toolchain — local-remote-execution/generated-cc/config/BUILD:43
    "container-image": "docker://lre-cc:zms5771rx1yqb4wd6qbj5f9sb2paq75k",
    "OSFamily": "Linux",
}
```

The Rust toolchain uses the same shape with a `ghcr.io/tracemachina/nativelink-worker-lre-rs:<imageTag>` reference (`local-remote-execution/rust/platforms/BUILD.bazel:24-26`).

NativeLink's own Buck2 integration test also carries an `lre-rs` key: the scheduler lists it as a `priority` property (`buck2_cas.json5:59`) — which `schedulers.rs:60` defines as informational pass-through, not a scheduling constraint — and the worker advertises it with an empty value (`buck2_cas.json5:107-111`). The real toolchain identity still travels in `container-image`; `lre-rs` is a routing hook to start matching on once you run more than one Rust toolchain. Either way the worker must have the toolchain on hand — baked into its container image as a Nix closure, or present on the host through Nix.

### Host Toolchain (Simplest)

For single-machine or homogeneous-fleet deployments:

```python
remote_execution_properties = {
    "OSFamily": "Linux",
}
```

No toolchain identity in the properties. This works when all workers are identical (same image, same packages). It breaks the moment workers diverge.

## Buck2 vs Bazel: Key Differences

| Aspect | Bazel | Buck2 |
|--------|-------|-------|
| Config location | CLI flags (`.bazelrc`) | `.buckconfig` + Starlark |
| Instance name default | Empty string | `"main"` |
| Platform properties | `--remote_default_exec_properties` | `remote_execution_properties` dict |
| Hybrid execution | `--remote_local_fallback` | `use_limited_hybrid` (smarter) |
| Output handling | `--remote_download_*` flags | `remote_output_paths` |
| Separate CAS/AC/exec addresses | No (one `--remote_cache` + `--remote_executor`) | Yes (separate fields) |
| Auth | CLI flags + credential helpers | `.buckconfig` + TLS certs |
| Action result upload | Always (or `--remote_upload_local_results=false`) | Via worker config `upload_action_result` |

## Running the Integration Test

NativeLink's Buck2 integration test is a smoke test, not a benchmark. It proves the wiring works end to end and that the server logs no errors — nothing more:

**Source:** [`integration_tests/buck2/buck2-with-nativelink-test.nix`](https://github.com/TraceMachina/nativelink/blob/main/integration_tests/buck2/buck2-with-nativelink-test.nix)

What it actually does (`buck2-with-nativelink-test.nix:17-54`):
1. Starts NativeLink with `integration_tests/buck2/buck2_cas.json5`, teeing the server log to `nativelink.log`.
2. Rewrites the test rules (`tests/defs.bzl`) to call Nix-provided `cat` and `diff` instead of whatever is on `PATH`, so the build is hermetic.
3. Runs `buck2 build //...` **once**. The action graph in `tests/defs.bzl` chains five stages — some marked `local_only`, the rest left to run on remote execution — feeding outputs between them and ending in a `diff` verification stage, so a single build exercises both local and remote paths.
4. Asserts the Buck2 output contains `BUILD SUCCEEDED`.
5. Asserts the server log contains no `ERROR`.

It does **not** re-run the build to measure cache hits, and it does **not** benchmark hybrid scheduling — those are properties you verify against your own workload. The Nix flake attribute is `buck2-with-nativelink-test` (`flake.nix:521`), and CI runs it with `nix run`:

```bash
nix run .#buck2-with-nativelink-test
```

## Common Issues (Buck2-Specific)

### "Connection refused" or "TLS handshake failed"

Buck2's TLS configuration:
```ini
[buck2_re_client]
tls = true
# Buck2 uses the system certificate store by default
# For custom CAs, you may need to set SSL_CERT_FILE
```

### "Instance name mismatch"

If you see `NOT_FOUND` on every request, check that `.buckconfig`'s `instance_name` matches the NativeLink server config's `instance_name` on every service. They must be identical strings.

### "Remote execution not enabled"

Verify:
1. `remote_enabled = True` in the platform's `CommandExecutorConfig`
2. The execution platform is registered in `[build] execution_platforms`
3. NativeLink has an `execution` service configured (not just `cas` + `ac`)

### Inconsistent results between local and remote

This is the toolchain problem (Part V). Buck2's hybrid mode makes it more visible — if local and remote toolchains differ, you'll see non-deterministic results depending on which executor "won" for each action.

Fix: use the same toolchain everywhere, or disable hybrid mode (`local_enabled = False`).
