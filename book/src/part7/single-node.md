# Single Node

The simplest NativeLink deployment: one binary, one config file, all four roles (CAS, AC, scheduler, worker) in one process. This is how you develop, how you test, and how you run CI without external dependencies.

## The Config

```json5
// nativelink-config.json5 — single-node, all-in-one
{
  stores: [
    {
      name: "AC_STORE",
      filesystem: {
        content_path: "/tmp/nativelink/ac/content",
        temp_path: "/tmp/nativelink/ac/tmp",
        eviction_policy: { max_bytes: "512mb" }
      }
    },
    {
      name: "CAS_STORE",
      filesystem: {
        content_path: "/tmp/nativelink/cas/content",
        temp_path: "/tmp/nativelink/cas/tmp",
        eviction_policy: { max_bytes: "5gb" }
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
      listener: { http: { socket_address: "127.0.0.1:50061" } },
      services: {
        worker_api: { scheduler: "MAIN_SCHEDULER" },
        health: {}
      }
    }
  ]
}
```

Two listeners, one process: the public gRPC surface (CAS, AC, execution,
capabilities, bytestream) on `0.0.0.0:50051`, and a loopback-only `worker_api`
listener on `127.0.0.1:50061` that the in-process worker dials and that carries
the `health` route. Every field above is defined in
[Appendix A: Configuration Reference](../appendix/config-reference.md) — reach
for it whenever you add a field, because `deny_unknown_fields` means a typo is a
hard startup error, not a warning.

## Running It

### From Source (Cargo)

```bash
cargo run --release --bin nativelink -- nativelink-config.json5
```

### From Nix

Run the flake directly:

```bash
nix run github:TraceMachina/nativelink -- nativelink-config.json5
```

### From Docker

```bash
docker run -v $(pwd)/nativelink-config.json5:/config.json5 \
  -p 50051:50051 \
  ghcr.io/tracemachina/nativelink:latest /config.json5
```

### Verify It's Running

Once the process is up, hit the health endpoint. NativeLink has **no gRPC health
service** — there is no `grpc.health.v1.Health`. Health is a plain HTTP route that
returns a JSON per-component report with `200 OK` when everything is healthy and
`503 Service Unavailable` when any component reports `Failed` or `Timeout`
(`health_server.rs:61`). It is mounted only on the server whose `services` block
declares `health: {}` — in this config that's the `worker_api` listener on
**port 50061**, not the public gRPC port 50051:

```bash
curl -i http://127.0.0.1:50061/status
# HTTP/1.1 200 OK
# content-type: application/json; charset=utf-8
# [ ... per-component health report ... ]
```

The default path is `/status` (`HealthConfig`, `cas_server.rs:623`); set
`health: { path: "/healthz" }` to change it.

## Connecting Clients

### Bazel

```bash
# .bazelrc
build --remote_cache=grpc://127.0.0.1:50051
build --remote_executor=grpc://127.0.0.1:50051
build --remote_instance_name=main
build --remote_default_exec_properties=cpu_count=1
```

### Buck2

```ini
# .buckconfig
[buck2_re_client]
engine_address = 127.0.0.1:50051
action_cache_address = 127.0.0.1:50051
cas_address = 127.0.0.1:50051
tls = false
instance_name = main
```

## When to Use This

Single-node is right for:
- **Local development** — test remote execution behavior without a remote server
- **CI runners** — each CI job runs its own NativeLink instance, caching within the job
- **Small teams** — if everyone is on the same machine (or cache sharing isn't needed)
- **Testing NativeLink itself** — the integration tests use this pattern

Single-node is wrong for:
- **Cross-machine cache sharing** — you need a durable, shared backend (S3, etc.)
- **High throughput** — one worker saturates one machine's CPUs
- **Reliability** — no redundancy, no failover

## Upgrading to Shared Cache

The minimal change from single-node to shared cache: replace the filesystem CAS with cloud storage:

```json5
// Replace this:
{ name: "CAS_STORE", filesystem: { ... } }

// With this:
{
  name: "CAS_STORE",
  fast_slow: {
    fast: {
      filesystem: {
        content_path: "/tmp/nativelink/cas/content",
        temp_path: "/tmp/nativelink/cas/tmp",
        eviction_policy: { max_bytes: "5gb" }
      }
    },
    slow: {
      experimental_cloud_object_store: {
        provider: "aws",
        region: "us-east-1",
        bucket: "my-team-nativelink-cas",
        key_prefix: "cas/"
      }
    }
  }
}
```

`provider` is required — it is the enum tag that selects the backend
(`ExperimentalCloudObjectSpec`, `stores.rs:1071`): `aws`, `gcs`, `azure`,
`ontap`, `r2`, or `oci`. Omit it and the store fails to parse; each provider then
takes its own fields (`region`/`bucket` for `aws`, `account_name`/`container` for
`azure`, and so on).

Now multiple machines can share the CAS (each with their own local fast tier). This is the bridge between single-node and multi-worker — you get cache sharing without a distributed scheduler.
