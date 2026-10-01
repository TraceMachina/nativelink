# Multi-Team Production Config

A production configuration for an organization with multiple teams, multiple languages, and different performance requirements. This is not a tutorial — it is a reference architecture.

The spine of the design is one shared, content-addressed CAS. Every team, region, and CI runner reads and writes the same substrate.

## The Scenario

- **3 teams:** Platform (Rust + C++), Backend (Go + Python), Mobile (Swift + Kotlin)
- **2 regions:** US-East, EU-West
- **~50 developers** + CI fleet
- **Requirements:** Shared cache, remote execution, team isolation, observability, hermetic toolchains

## Architecture

```
                            ┌──────────────────┐
                            │  Global S3 (CAS) │
                            │  Cross-region    │
                            │  replication     │
                            └────────┬─────────┘
                                     │
                 ┌───────────────────┼───────────────────┐
                 │                   │                   │
      ┌──────────▼──────────┐    ┌──▼──────────────┐   │
      │  US-East Cluster    │    │  EU-West Cluster │   │
      │                     │    │                  │   │
      │  Scheduler (HA, 2x) │    │  Scheduler (HA)  │   │
      │  Redis (state)      │    │  Redis (state)   │   │
      │                     │    │                  │   │
      │  Worker Pool:       │    │  Worker Pool:    │   │
      │    compile (8x)     │    │    compile (4x)  │   │
      │    test (12x)       │    │    test (6x)     │   │
      │    mobile (4x)      │    │                  │   │
      └─────────────────────┘    └──────────────────┘   │
                                                        │
                                        ┌───────────────▼──┐
                                        │  CI Runners      │
                                        │  (cache-only,    │
                                        │   no execution)  │
                                        └──────────────────┘
```

## NativeLink Config: Scheduler (US-East)

This is the control-plane node: it serves the client-facing CAS/AC/Execution endpoints and holds scheduler state in Redis. It does not run workers itself.

```json5
{
  stores: [
    // Action Cache: metrics -> completeness checking -> fast/slow tiers.
    {
      name: "AC_STORE",
      cache_metrics: {
        cache_type: "ac",
        backend: {
          completeness_checking: {
            backend: {
              fast_slow: {
                fast: {
                  memory: { eviction_policy: { max_bytes: "2gb" } }
                },
                slow: {
                  experimental_cloud_object_store: {
                    provider: "aws",
                    region: "us-east-1",
                    bucket: "acme-nativelink-prod",
                    key_prefix: "ac/"
                  }
                }
              }
            },
            // completeness_checking.cas_store is a StoreSpec, not a bare
            // name: wrap the shared CAS in a ref_store so it points at
            // CAS_STORE below.
            cas_store: {
              ref_store: { name: "CAS_STORE" }
            }
          }
        }
      }
    },

    // CAS: metrics -> existence cache -> compression -> fast/slow tiers.
    {
      name: "CAS_STORE",
      cache_metrics: {
        cache_type: "cas",
        backend: {
          existence_cache: {
            backend: {
              compression: {
                backend: {
                  fast_slow: {
                    fast: {
                      memory: { eviction_policy: { max_bytes: "8gb" } }
                    },
                    slow: {
                      experimental_cloud_object_store: {
                        provider: "aws",
                        region: "us-east-1",
                        bucket: "acme-nativelink-prod",
                        key_prefix: "cas/"
                      }
                    }
                  }
                },
                compression_algorithm: { lz4: {} }
              }
            },
            eviction_policy: { max_count: 5000000 }
          }
        }
      }
    },

    // Redis store holding scheduler state. The scheduler backend references
    // this store by name; addresses and key_prefix live here, on the store.
    {
      name: "SCHEDULER_REDIS",
      redis_store: {
        addresses: ["redis://redis-primary:6379"],
        key_prefix: "sched-us-east:"
      }
    }
  ],

  schedulers: [
    {
      name: "MAIN_SCHEDULER",
      cache_lookup: {
        ac_store: "AC_STORE",
        scheduler: {
          simple: {
            supported_platform_properties: {
              cpu_count: "minimum",
              memory_gb: "minimum",
              OSFamily: "priority",
              "container-image": "priority",
              ISA: "exact",
              pool: "exact",
              "toolchain-hash": "exact"
            },
            worker_timeout_s: 30,
            // Re-queue an action stuck Executing this long with no worker
            // update (a live worker stalled on one action).
            max_action_executing_timeout_s: 1200,
            experimental_backend: {
              redis: { redis_store: "SCHEDULER_REDIS" }
            }
          }
        }
      }
    }
  ],

  servers: [
    {
      name: "public",
      listener: {
        http: {
          socket_address: "0.0.0.0:50051",
          tls: {
            cert_file: "/certs/tls.crt",
            key_file: "/certs/tls.key",
            client_ca_file: "/certs/ca.crt"
          }
        }
      },
      services: {
        cas: [
          { instance_name: "main", cas_store: "CAS_STORE" },
          { instance_name: "", cas_store: "CAS_STORE" }
        ],
        ac: [
          { instance_name: "main", ac_store: "AC_STORE" },
          { instance_name: "", ac_store: "AC_STORE" }
        ],
        execution: [{
          instance_name: "main",
          cas_store: "CAS_STORE",
          scheduler: "MAIN_SCHEDULER"
        }],
        capabilities: [{
          instance_name: "main",
          remote_execution: { scheduler: "MAIN_SCHEDULER" }
        }],
        bytestream: [
          { instance_name: "main", cas_store: "CAS_STORE" },
          { instance_name: "", cas_store: "CAS_STORE" }
        ]
      }
    },
    {
      name: "worker_api",
      listener: {
        http: { socket_address: "0.0.0.0:50061" }
      },
      services: {
        worker_api: { scheduler: "MAIN_SCHEDULER" },
        health: {},
        admin: {}
      }
    }
  ],

  global: {
    max_open_files: 65536,
    // Requests that omit the digest function fall through to this default, so
    // set it to your fleet's dominant client. A BLAKE3 client that forgets to
    // set the field will otherwise have its blobs hashed as SHA256.
    default_digest_hash_function: "sha256"
  }
}
```

## Worker Config: Compile Pool

Workers are separate processes on separate machines. Each one owns a local filesystem `fast` tier and shares the same S3 `slow` tier as the scheduler, so a blob a worker writes is immediately readable through the scheduler's CAS.

```json5
{
  stores: [{
    name: "WORKER_CAS",
    fast_slow: {
      fast: {
        filesystem: {
          content_path: "/data/cas/content",
          temp_path: "/data/cas/tmp",
          eviction_policy: { max_bytes: "100gb" }
        }
      },
      slow: {
        experimental_cloud_object_store: {
          provider: "aws",
          region: "us-east-1",
          bucket: "acme-nativelink-prod",
          key_prefix: "cas/"
        }
      }
    }
  }],

  workers: [{
    local: {
      worker_api_endpoint: { uri: "grpc://scheduler:50061" },
      cas_fast_slow_store: "WORKER_CAS",
      work_directory: "/data/work",
      entrypoint: "/opt/nativelink/entrypoint.sh",
      use_namespaces: true,
      use_mount_namespace: true,
      // Durations are integer seconds (the *_s field), not {secs, nanos}.
      max_action_timeout_s: 1200,
      platform_properties: {
        cpu_count: { query_cmd: "nproc" },
        // query_cmd runs the command directly (parsed with shlex, then
        // exec'd — NOT a shell). A shell expression must be wrapped in
        // `sh -c "..."`, or command substitution and pipes are passed to
        // the program as literal argv strings.
        memory_gb: { query_cmd: "sh -c \"echo $(($(free -b | awk '/Mem:/{print $2}') / 1073741824))\"" },
        OSFamily: { values: ["Linux"] },
        ISA: { values: ["x86-64"] },
        pool: { values: ["compile"] },
        "container-image": { values: [""] },
        "toolchain-hash": { values: ["lre-2024.1-abc123"] }
      },
      additional_environment: {
        // Property(name) is a newtype variant: { property: "..." }.
        CONTAINER_IMAGE: { property: "container-image" },
        // The rest are unit variants, so they are bare strings, not maps.
        ACTION_DIRECTORY: "action_directory",
        TIMEOUT_MS: "timeout_millis",
        SIDE_CHANNEL: "side_channel_file"
      },
      experimental_precondition_script: "/opt/nativelink/check-disk.sh"
    }
  }],

  servers: []
}
```

Why `sh -c` matters: the worker splits `query_cmd` with `shlex` and `exec`s the first token as a program, feeding the rest as arguments (`worker_utils.rs:50-61`). Handed the bare `echo $(( ... ))`, it runs `echo` with `$(($(free`, `-b`, `|`, ... as literal arguments and reports that string as the property value. Wrapping in `sh -c` gives you a real shell to evaluate the substitution.

## Worker Config: Test Pool

Same shape as the compile pool, but tuned for memory-heavy integration tests:
- Higher memory allocation, lower CPU count
- Different `pool` name so the scheduler routes test actions here
- Network access for integration tests (no `--network=none` in the entrypoint)

```json5
// platform_properties fragment — the rest of the worker config is identical
// to the compile pool above.
platform_properties: {
  cpu_count: { values: ["4"] },
  memory_gb: { values: ["64"] },
  pool: { values: ["test"] },
  // ...
}
```

## Client Configurations

### Platform Team (Bazel + LRE)

```bash
# .bazelrc
try-import %workspace%/lre.bazelrc
build --remote_cache=grpcs://nativelink.acme.internal:50051
build --remote_executor=grpcs://nativelink.acme.internal:50051
build --remote_instance_name=main
build --tls_certificate=/etc/nativelink/ca.crt
build --remote_default_exec_properties=pool=compile
build --remote_default_exec_properties=ISA=x86-64
```

### Backend Team (Bazel + zig-cc)

```bash
# .bazelrc
build --remote_cache=grpcs://nativelink.acme.internal:50051
build --remote_executor=grpcs://nativelink.acme.internal:50051
build --remote_instance_name=main
build --extra_toolchains=@zig_sdk//toolchain:linux_amd64_gnu.2.28
build --remote_default_exec_properties=pool=compile
build --remote_default_exec_properties=toolchain-hash=zig-0.11.0
build --remote_default_exec_properties=ISA=x86-64
```

### Mobile Team (Buck2)

```ini
# .buckconfig
[buck2_re_client]
engine_address = nativelink.acme.internal:50051
action_cache_address = nativelink.acme.internal:50051
cas_address = nativelink.acme.internal:50051
tls = true
instance_name = main
```

### CI Runners (Cache-Only)

```bash
# .bazelrc for CI
build --remote_cache=grpcs://nativelink.acme.internal:50051
build --remote_instance_name=main
# No --remote_executor: CI builds locally, uploads to shared cache
build --remote_upload_local_results=true
```

## Observability Setup

```yaml
# Prometheus scrape config
scrape_configs:
  - job_name: nativelink-scheduler
    static_configs:
      - targets:
          - scheduler-0:9090
          - scheduler-1:9090

  - job_name: nativelink-workers
    kubernetes_sd_configs:
      - role: pod
        selectors:
          - role: pod
            label: app=nativelink-worker
```

The `cache_metrics` wrapper on `AC_STORE` and `CAS_STORE` (with `cache_type` labels `ac` and `cas`) is what emits the per-store cache-operation metrics; stores that are not wrapped pay no timing cost and publish nothing. Key dashboards:

- **Cache hit rate** by `cache_type` (ac vs cas)
- **Queue depth** by pool (compile vs test)
- **Worker utilization** (actions in progress / total capacity)
- **Store latency** p50/p99 (memory tier vs S3 tier)
- **Bytes transferred** (upload/download volume)

## Key Design Decisions

1. **CAS in S3, not on scheduler disk.** Workers upload directly to S3. The scheduler is control-plane only.
2. **Redis for scheduler state.** `experimental_backend: { redis: { redis_store: "SCHEDULER_REDIS" } }` references a declared `redis_store`; the addresses and key prefix live on that store. This enables HA (multiple scheduler replicas) and survives scheduler restarts.
3. **`CacheLookupScheduler`.** Server-side AC check before dispatch. Prevents re-execution of already-cached actions.
4. **Pool-based routing.** `exact` match on `pool` sends compile actions to CPU-heavy workers and test actions to memory-heavy workers.
5. **Both instance names.** `"main"` and `""` map to the same underlying stores, so Buck2 (which sets `instance_name = main`) and a default Bazel client both hit the same cache.
6. **Compression on CAS.** LZ4 reduces S3 storage and transfer, and aborts early on incompressible blobs.
7. **Completeness checking on AC.** Prevents cache hits that reference evicted CAS blobs; the wrapper's `cas_store` is a `ref_store` pointing at `CAS_STORE`.
8. **Existence cache on CAS.** Eliminates network round-trips for `FindMissingBlobs` on the hot path.
9. **mTLS on the gRPC port.** All build-client connections are authenticated, and the worker API sits on a separate port not exposed to clients.
10. **Precondition script.** Workers self-heal by pausing when disk is low.
```
