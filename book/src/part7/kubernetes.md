# Kubernetes Production

For production deployments — autoscaling, high availability, rolling updates, observability — NativeLink runs on Kubernetes. This chapter grounds every claim in the manifests that actually ship in the repository, not in a generic "here is how you might do it" sketch.

One correction up front, because the rest of the chapter depends on it: **the `kubernetes/` tree in this repository is Kustomize plus Flux, not a Helm chart.** NativeLink also publishes a Helm chart on Artifact Hub, and the in-repo `README.md` points at it, but nothing under `kubernetes/` is a chart — there is no `Chart.yaml`, no `values.yaml`, no `templates/`. What is there is a set of Kustomize components and resources that Flux reconciles from Git.

**Source:** [`kubernetes/`](https://github.com/TraceMachina/nativelink/tree/main/kubernetes)

## What Is Actually in `kubernetes/`

The tree is Kustomize overlays reconciled by Flux:

| Path | What it is |
|---|---|
| `kubernetes/nativelink/` | The cache/scheduler `Deployment` + `Service`, its `nativelink-config.json5` `ConfigMap`, and the demo TLS secret |
| `kubernetes/components/worker/` | A reusable worker `Deployment` component and its `worker.json5` `ConfigMap` |
| `kubernetes/components/kustomization/` | The Flux `Kustomization` object that reconciles the above from a `GitRepository` |
| `kubernetes/components/alerts/` | A Flux `Alert` (CI rebuild notification, not a Prometheus alert) |
| `kubernetes/resources/` | Cluster building blocks: `cert-manager`, `flux`, `gateway-routes` (a `GRPCRoute`), Tekton pipelines, demo certs |
| `kubernetes/workers/` | Concrete worker overlays (`lre-cc`, `lre-rs`, `siso-chromium`) |
| `kubernetes/vendor/` | Pinned third-party operators (OLM, Tekton, telemetry) |

The two workloads you care about are `nativelink/nativelink.yaml` (the server) and `components/worker/worker.yaml` (the workers). Everything else is scaffolding for CI/CD.

## Architecture

```
┌───────────────────────────────────────────────────────────────┐
│  Kubernetes Cluster                                            │
│                                                                │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │  Gateway / GRPCRoute (TLS termination, load balancing)   │ │
│  └───────────────────────────┬──────────────────────────────┘ │
│                              │  :50051 (plaintext) / :50052 (TLS)
│  ┌───────────────────────────▼──────────────────────────────┐ │
│  │  nativelink Deployment (2+ replicas)                     │ │
│  │  :50051/:50052  CAS, AC, Execution, Capabilities, ByteStream │
│  │  :50061         worker_api + health (/status)            │ │
│  └───────────────────────────┬──────────────────────────────┘ │
│                              │  worker_api on :50061           │
│  ┌───────────────────────────▼──────────────────────────────┐ │
│  │  nativelink-worker Deployment (autoscaled)               │ │
│  │  init container copies the binary into a shared emptyDir │ │
│  └──────────────────────────────────────────────────────────┘ │
│                                                                │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │  Redis (scheduler state store — optional, for HA)        │ │
│  └──────────────────────────────────────────────────────────┘ │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │  Cloud object storage (S3/GCS/R2/Azure) — shared slow tier │
│  └──────────────────────────────────────────────────────────┘ │
│  ┌──────────────────────────────────────────────────────────┐ │
│  │  OpenTelemetry collector (OTLP :4317) — metrics/traces/logs │
│  └──────────────────────────────────────────────────────────┘ │
└───────────────────────────────────────────────────────────────┘
```

Two things the old diagram got wrong and this one fixes: Redis is a NativeLink **store** that backs the scheduler, not a magic sidecar the scheduler talks to directly; and telemetry leaves the process over OTLP to a collector, not through a Prometheus scrape endpoint (see [Worker autoscaling](#worker-autoscaling)).

## The Two Workloads

### The server

`kubernetes/nativelink/nativelink.yaml` runs a single `Deployment` that mounts `nativelink-config.json5` from a `ConfigMap` and exposes four container ports:

| Port | Service |
|---|---|
| `50051` | Public gRPC (CAS, AC, execution, capabilities, bytestream), plaintext |
| `50052` | The same public services behind TLS (`tls` block on the second listener) |
| `50061` | `worker_api` **and** the `health` route — a backend port, not public |
| `9090` | Declared in the manifest, but the process does not bind it; see the metrics note below |

The config binds those listeners in three `servers` entries. The third one carries `worker_api` and `health: {}` together — workers hold a different permission set than cache clients, so `worker_api` lives on its own listener (`cas_server.rs:754`). The health route rides that same backend port, which is why every probe in this chapter targets **50061**, not 50051.

### The workers

`kubernetes/components/worker/worker.yaml` is more subtle than a plain `Deployment`. It uses an **init container** (`nativelink-worker-init`) to copy the NativeLink binary into a shared `emptyDir`, then the main container runs that copied binary against `worker.json5`:

```yaml
initContainers:
  - name: nativelink-worker-init
    image: nativelink-worker-init         # patched by kustomize
    args: ["/shared/nativelink"]
    volumeMounts:
      - { name: shared, mountPath: /shared }
containers:
  - name: nativelink-worker
    image: nativelink-worker              # patched by kustomize
    command: ["/shared/nativelink"]
    args: ["/worker.json5"]
    env:
      - { name: NATIVELINK_ENDPOINT, value: nativelink }
    volumeMounts:
      - { name: worker-config, mountPath: /worker.json5, subPath: worker.json5 }
      - { name: shared, mountPath: /shared }
```

`worker.json5` dials the scheduler through `grpc://${NATIVELINK_ENDPOINT}:50061` and declares a `WORKER_FAST_SLOW_STORE` whose fast tier is a `FileSystemStore` and whose slow tier is a `ref_store` back to the server's CAS over gRPC. Two consequences worth stating plainly:

- The worker config has `servers: []`. **Worker pods expose no listener and therefore no `/status` route.** You cannot put an `httpGet` liveness probe on a worker pod. Rely on the container's own liveness (the process exits on a fatal error) or an `exec` probe; do not copy the server's `httpGet` probe onto the workers.
- The worker's fast tier is a local `FileSystemStore`, so it wants a real volume (see [Storage](#storage-for-workers)).

## Validate Before You Apply

Because every NativeLink struct is `#[serde(deny_unknown_fields)]`, a stale or misspelled key is a hard load error, not a warning. Catch it before Flux rolls it out. On startup NativeLink parses the JSON5 and resolves every store and scheduler reference before it binds a socket (`validate_references` at `cas_server.rs:1450`), so a bad config fails fast rather than silently.

Extract the `ConfigMap` payload and boot the binary against it in CI before the manifest is ever reconciled:

```bash
# Pull the config out of the kustomization and load it.
kubectl kustomize kubernetes/nativelink \
  | yq 'select(.kind == "ConfigMap") | .data["nativelink-config.json5"]' \
  > /tmp/nativelink-config.json5

# A mistyped field or dangling reference exits non-zero at load.
nativelink /tmp/nativelink-config.json5
```

Wire the non-zero exit into a pre-merge gate and a mistyped `cas_store` or `scheduler` name fails the pipeline instead of crash-looping a pod.

## High Availability with a Redis-Backed Scheduler

To run more than one scheduler replica, the schedulers must share state. NativeLink does this by pointing the `simple` scheduler's backend at a Redis **store**. The mechanism is `SimpleSpec.experimental_backend` (`schedulers.rs:160`), whose `redis` variant takes a single `redis_store` reference (`schedulers.rs:175-189`) that must resolve to a `redis_store` in your `stores` list.

No `experimental_redis_scheduler_state` block exists, and the Redis connection details (`addresses`, `key_prefix`, pool size) live on the **store**, not on the scheduler:

```json5
{
  stores: [
    // ... CAS_MAIN_STORE, AC_MAIN_STORE ...
    {
      name: "SCHEDULER_REDIS_STORE",
      redis_store: {
        addresses: ["redis://nativelink-redis:6379"],
        connection_pool_size: 10,
        experimental_pub_sub_channel: "scheduler_key_change",
        key_prefix: "sched:",
      },
    },
  ],
  schedulers: [
    {
      name: "MAIN_SCHEDULER",
      simple: {
        supported_platform_properties: {
          cpu_count: "minimum",
          OSFamily: "priority",
          "container-image": "priority",
        },
        worker_timeout_s: 30,
        experimental_backend: {
          redis: {
            redis_store: "SCHEDULER_REDIS_STORE",
          },
        },
      },
    },
  ],
  // ... servers unchanged ...
}
```

The `experimental_pub_sub_channel` is what lets replicas react to each other's writes: every scheduler write publishes on that channel, so a second replica sees actions queued on the first. With this backend in place:

- Workers can connect to any scheduler replica (front them with one `Service`).
- Actions queued on one replica are visible to the others.
- A replica can be restarted independently; its state lives in Redis, not in process memory.

`nativelink-config/examples/worker_with_redis_scheduler.json5` is a complete, checked-in example of this exact wiring. Load the full HA config to confirm it parses and every reference resolves:

```bash
nativelink /tmp/k8s-ha.json5
```

## The `global` Block in the Cluster

The `ConfigMap` carries the full server JSON5, including the `global` block. Pick a
digest function with `default_digest_hash_function` and set it consistently across
every client and worker in the fleet — a mixed fleet of BLAKE3 Bazel clients next
to SHA256 tooling is where a client that omits the field gets its output tree
hashed under the wrong algorithm. Note that `global` requires `max_open_files`; it
has no `serde` default (`cas_server.rs:1310`), so a `global` block that omits it
fails to load with `missing field max_open_files`.

```json5
global: {
  max_open_files: 24576,
  default_digest_hash_function: "blake3",
}
```

## Worker Autoscaling

### Scale on CPU (the robust default)

Workers are CPU-bound during action execution, and CPU utilization is a stable, first-class Kubernetes metric with no coupling to NativeLink internals. Start here:

```yaml
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: nativelink-worker
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: nativelink-worker
  minReplicas: 2
  maxReplicas: 50
  metrics:
    - type: Resource
      resource:
        name: cpu
        target:
          type: Utilization
          averageUtilization: 70
```

### Scale on queue depth (mind the telemetry path)

Queue-depth scaling is more responsive, but be honest about where the numbers come from. NativeLink does **not** expose a Prometheus scrape endpoint. It exports metrics, traces, and logs over **OTLP** to a collector (`telemetry.rs:180-193`); the shipped manifests wire that with `OTEL_EXPORTER_OTLP_ENDPOINT=http://otel-collector-collector.default.svc:4317`. The `9090` port in `nativelink.yaml` is declared but the process never binds it.

No fixed `nativelink_*` metric name therefore exists to hardcode into an HPA or a KEDA `ScaledObject`. The instrument names are derived from NativeLink's internal metrics-component tree and then transformed again by whatever your OpenTelemetry collector exports them as (VictoriaMetrics, Prometheus remote-write, and so on). If you want to scale on queued actions:

1. Route the collector's output into a Prometheus-compatible store.
2. Inspect that store to find the **actual** series name your pipeline produced for the scheduler's queued-operations gauge — do not guess it.
3. Point the KEDA `prometheus` trigger at that verified name.

Anything else is copying a metric name that does not exist in your cluster. The CPU HPA above works today with no such dependency; reach for queue-depth scaling only once your OTLP pipeline is real and you have read the series name off it.

### Draining a worker before it scales in

When the HPA removes a worker, you want it to stop taking new actions first. The server exposes an admin route for exactly this — `POST {admin}/scheduler/{instance_name}/set_drain_worker/{worker_id}/1` marks a worker draining (`nativelink.rs:449-491`). Wire it into a `preStop` hook so scale-in and rolling updates stop dispatching to a pod before its grace period starts.

## Storage for Workers

The worker's fast CAS tier is a `FileSystemStore`, so it needs a real filesystem. Ephemeral `emptyDir` is the common choice — fast local disk, discarded on pod restart:

```yaml
spec:
  containers:
    - name: nativelink-worker
      volumeMounts:
        - { name: work-dir, mountPath: /tmp/nativelink/work }
        - { name: cas-cache, mountPath: /tmp/nativelink/content_path-cas }
  volumes:
    - name: work-dir
      emptyDir: { sizeLimit: 50Gi }
    - name: cas-cache
      emptyDir: { sizeLimit: 100Gi }
```

The mount paths must match `content_path`/`temp_path`/`work_directory` in `worker.json5` (the shipped config uses `/tmp/nativelink/...`). Swap `emptyDir` for a `persistentVolumeClaim` if you want the fast tier to survive restarts. The slow tier is configured in JSON5 (a `ref_store` to the server's CAS, or cloud object storage) and needs no Kubernetes volume.

## Shared CAS on Cloud Object Storage

The slow tier of a shared CAS is a cloud object store. The store variant is `experimental_cloud_object_store`, and it is a **provider-tagged** enum (`stores.rs:1071`): the `provider` field selects one of `aws`, `gcs`, `azure`, `ontap`, `r2`, or `oci`, and each provider has its own field set:

```json5
{
  name: "CAS_MAIN_STORE",
  fast_slow: {
    fast: {
      filesystem: {
        content_path: "/data/cas/content",
        temp_path: "/data/cas/tmp",
        eviction_policy: { max_bytes: "10gb" },
      },
    },
    slow: {
      experimental_cloud_object_store: {
        provider: "aws",
        region: "us-east-1",
        bucket: "my-team-nativelink-cas",
        key_prefix: "cas/",
      },
    },
  },
}
```

`provider` is required — it is the enum discriminant, not an optional hint. Omit it and the config parser cannot pick a variant, so the load fails.

## TLS and mTLS

The shipped `nativelink.yaml` runs a second listener on `50052` behind TLS, mounting certificates from a secret at `/root`:

```json5
listener: {
  http: {
    socket_address: "0.0.0.0:50052",
    tls: {
      cert_file: "/root/example-do-not-use-in-prod-rootca.crt",
      key_file: "/root/example-do-not-use-in-prod-key.pem",
    },
  },
}
```

Those demo certs come from `kubernetes/resources/insecure-certs/` and are exactly what their filenames say — do not ship them. For production, add `client_ca_file` to require client certificates (mTLS is enabled by the presence of `client_ca_file`, not a boolean toggle; there is no `client_auth_optional`), and source the certs from cert-manager. The `kubernetes/resources/cert-manager/` overlay installs cert-manager via a Flux `HelmRelease`; mount the issued secret the same way:

```yaml
spec:
  containers:
    - name: nativelink
      volumeMounts:
        - { name: tls-certs, mountPath: /root, readOnly: true }
  volumes:
    - name: tls-certs
      secret:
        secretName: nativelink-tls
```

## Rolling Updates and Graceful Shutdown

NativeLink handles `SIGTERM` for graceful shutdown, and it is worth knowing exactly what that does because it dictates your grace period. On `SIGTERM` the process broadcasts a shutdown signal (`nativelink.rs:947-962`) and then waits for a priority barrier before exiting with code `143`. Inside the worker, that signal drives a drain loop: **the worker blocks until its in-flight action count reaches zero**, sends a `GoingAway` message, and only then releases the shutdown guard (`local_worker.rs:493-515`).

**No `graceful_shutdown_timeout` field** exists anywhere in the config — earlier drafts of this chapter invented one. The drain is unbounded; it is capped only by how long the longest in-flight action can run, which is `max_action_timeout_s` (default `1200`, i.e. 20 minutes; `cas_server.rs:1124`). Size the pod's grace period to that ceiling so Kubernetes does not `SIGKILL` a worker mid-action:

```yaml
spec:
  # >= the worker's max_action_timeout_s so a draining worker can finish its
  # longest in-flight action before the kubelet escalates to SIGKILL.
  terminationGracePeriodSeconds: 1200
```

Pair that with a `preStop` hook that drains the worker (see [Draining a worker](#draining-a-worker-before-it-scales-in)) and a `PodDisruptionBudget` so voluntary disruptions cannot take out the whole pool at once:

```yaml
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: nativelink-workers
spec:
  minAvailable: 50%
  selector:
    matchLabels:
      app: nativelink-worker
```

## Resource Requests and Limits

Scheduler/cache pods are memory-bound (connection and queue state), workers are CPU-bound (action execution):

```yaml
# nativelink server
resources:
  requests: { cpu: "500m", memory: "1Gi" }
  limits:   { cpu: "2",    memory: "4Gi" }

# nativelink-worker
resources:
  requests: { cpu: "4",  memory: "8Gi" }
  limits:   { cpu: "16", memory: "32Gi" }
```

Keep the worker's CPU request in line with the `cpu_count` it advertises in `worker.json5` — the scheduler matches actions against the advertised value, so a worker that claims more cores than its pod is granted will accept work it cannot run promptly.

## Multi-Cluster / Multi-Region

For global teams, deploy NativeLink per region with a shared or replicated slow tier:

```
Region A: nativelink + workers → S3 (us-east-1)
Region B: nativelink + workers → S3 (eu-west-1)
Cross-region: S3 replication between buckets
```

Each region runs its own scheduler and workers; CAS blobs replicate between regions through your object store's replication, so a hit in region A is available in region B after replication lag. Alternatively, chain the CAS with a `grpc` slow tier so region B falls back to region A on a miss:

```json5
{
  name: "CAS_STORE",
  fast_slow: {
    fast: { filesystem: { content_path: "/data/cas/content", temp_path: "/data/cas/tmp", eviction_policy: { max_bytes: "10gb" } } },
    slow: {
      grpc: {
        instance_name: "main",
        endpoints: [{ address: "grpc://nativelink-region-a:50051" }],
        store_type: "cas",
      },
    },
  },
}
```

The `grpc` store requires `store_type` (`"cas"` or `"ac"`); it is not optional.

## Runnable Images and Flakes

The flake builds a runnable server image:

```bash
# Build and load the server image.
nix run github:TraceMachina/nativelink#nativelink-image.copyToDockerDaemon

# Or run the binary directly against a local config.
nix run github:TraceMachina/nativelink -- nativelink-config.json5
```

Push that image to your registry and set it as the Kustomize image override (`images:` in `kubernetes/nativelink/kustomization.yaml` and `kubernetes/components/worker/kustomization.yaml`), or use the published `ghcr.io/tracemachina/nativelink` image directly.

## Health Checks (the correct probe)

NativeLink has **no gRPC health service**; there is no `grpc.health.v1.Health` to probe. Health is a plain HTTP route that returns a JSON per-component report — `200 OK` when everything is healthy, `503 Service Unavailable` when any component reports `Failed` or `Timeout` (`health_server.rs:61-104`). It is mounted only where a `services` block declares `health: {}` — in the shipped config, the `worker_api` listener on **port 50061**, at the default path `/status` (`HealthConfig`, `cas_server.rs:623`).

The probe is therefore an `httpGet` against `/status` on the health port, not a gRPC probe:

```yaml
# nativelink server pod
readinessProbe:
  httpGet: { path: /status, port: 50061 }
  periodSeconds: 10
livenessProbe:
  httpGet: { path: /status, port: 50061 }
  periodSeconds: 30
```

The route ignores the request method and body, so a bare `GET` is enough. Worker pods, remember, run with `servers: []` and expose no such route — do not attach an `httpGet` probe to them.

## Troubleshooting

Most Kubernetes-surface failures reduce to a config that did not deserialize, a store reference that did not resolve, or a probe pointed at the wrong port. Load the config to catch the first two at startup; then work through [Appendix C: Troubleshooting](../appendix/troubleshooting.md) for the runtime symptoms (`"Store 'NAME' not found"` at startup, `"Connection refused"`, action-queued-forever with no workers). For the meaning of every field referenced above, see [Appendix A: Configuration Reference](../appendix/config-reference.md).
