# NativeLink CRI-O Warm Worker Pools

This crate provides pre-warmed worker pools backed by CRI-O containers for NativeLink, dramatically reducing build times for language runtimes with significant cold-start overhead.

## Problem Statement

Language runtime builds suffer from severe cold-start penalties:

### Java/JVM Builds
- **JIT compilation warmup**: 10-30 seconds per worker
- **Class loading**: 2-5 seconds
- **Cache population**: 5-15 seconds
- **Total overhead**: 40-60% of short build times

### TypeScript/Node.js Builds
- **V8 optimization warmup**: 5-15 seconds per worker
- **Module loading**: 1-3 seconds
- **Cache population**: 2-5 seconds
- **Total overhead**: 30-50% of short build times

## Solution

Maintain pools of **pre-warmed workers** using CRI-O containers that:

1. **Warm up once** during pool initialization with language-specific strategies
2. **Reuse across multiple jobs** (like persistent connections)
3. **Trigger GC periodically** to prevent memory bloat
4. **Recycle workers** after N jobs or timeout
5. **Maintain minimum ready workers** for instant job assignment

## Performance Improvements

| Build Type | Cold Start | Warm Pool | Improvement |
|------------|------------|-----------|-------------|
| Java (100s build) | 100s | 40s | **60% faster** |
| TypeScript (50s build) | 50s | 25s | **50% faster** |
| Java worker startup | 45s | 8s | **82% faster** |
| TS worker startup | 25s | 5s | **80% faster** |

## Architecture

```
┌─────────────────────────────────┐
│   NativeLink Scheduler          │
└────────────┬────────────────────┘
             │
             ↓
┌─────────────────────────────────┐
│  WarmWorkerPoolManager          │
│  ┌──────────┐  ┌──────────┐    │
│  │Java Pool │  │  TS Pool │    │
│  └──────────┘  └──────────┘    │
└────────────┬────────────────────┘
             │ (CRI-O via crictl)
             ↓
┌─────────────────────────────────┐
│        CRI-O Runtime            │
│  ┌────────┐ ┌────────┐          │
│  │Worker 1│ │Worker 2│  ...     │
│  │ Warmed │ │ Active │          │
│  └────────┘ └────────┘          │
└─────────────────────────────────┘
```

## Quick Start

### Prerequisites

1. **CRI-O installed and running**
   ```bash
   # Ubuntu/Debian
   sudo apt-get install cri-o cri-tools
   sudo systemctl start crio

   # Verify
   sudo crictl --runtime-endpoint unix:///var/run/crio/crio.sock version
   ```

2. **Build worker container images**
   ```bash
   # Java worker
   cd nativelink-crio-worker-pool/docker/java
   docker build -t ghcr.io/tracemachina/nativelink-worker-java:latest .

   # TypeScript worker
   cd ../typescript
   docker build -t ghcr.io/tracemachina/nativelink-worker-node:latest .
   ```

### Configuration

Create a worker pool configuration file (see `examples/java-typescript-pools.json5`):

```json5
{
  pools: [
    {
      name: "java-pool",
      language: "jvm",
      cri_socket: "unix:///var/run/crio/crio.sock",
      container_image: "ghcr.io/tracemachina/nativelink-worker-java:latest",
      min_warm_workers: 5,
      max_workers: 50,
      warmup: {
        commands: [
          { argv: ["/opt/warmup/jvm-warmup.sh"], timeout_s: 60 }
        ],
        post_job_cleanup: [
          { argv: ["jcmd", "1", "GC.run"], timeout_s: 30 }
        ],
      },
      lifecycle: {
        worker_ttl_seconds: 3600,
        max_jobs_per_worker: 200,
        gc_job_frequency: 20,
      },
    },
  ],
}
```

### Usage

```rust
use nativelink_crio_worker_pool::{
    WarmWorkerPoolManager, PoolCreateOptions, WarmWorkerPoolsConfig,
    WorkerOutcome,
};

// Load configuration
let config: WarmWorkerPoolsConfig = serde_json5::from_str(&config_content)?;

// Create pool manager
let pool_manager = WarmWorkerPoolManager::new(
    PoolCreateOptions::new(config)
).await?;

// Acquire a warm worker
let lease = pool_manager.acquire("java-pool").await?;
let worker_id = lease.worker_id();

// Execute job on the worker
// ... (use worker_id to run build commands)

// Release worker back to pool
lease.release(WorkerOutcome::Completed).await?;
```

## Configuration Reference

### Pool Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | string | required | Pool identifier |
| `language` | enum | required | `jvm`, `nodejs`, or `custom(...)` |
| `cri_socket` | string | required | CRI-O socket path |
| `container_image` | string | required | OCI image reference |
| `min_warm_workers` | number | 2 | Minimum ready workers |
| `max_workers` | number | 20 | Maximum total workers |
| `worker_command` | array | `["/usr/local/bin/nativelink-worker"]` | Worker process command |
| `env` | object | `{}` | Environment variables |

### Warmup Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `commands` | array | `[]` | Warmup commands to execute |
| `verification` | array | `[]` | Verification commands after warmup |
| `post_job_cleanup` | array | `[]` | Cleanup commands after each job |
| `default_timeout_s` | number | 30 | Default command timeout |

### Lifecycle Configuration

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `worker_ttl_seconds` | number | 3600 | Max worker lifetime (1 hour) |
| `max_jobs_per_worker` | number | 200 | Max jobs before recycling |
| `gc_job_frequency` | number | 25 | Force GC every N jobs |

## Worker Lifecycle

```
Start → Warming (30s) → Ready → Active → Cooling (GC) → Ready
                         ↑___________________________|
                         (Reuse for multiple jobs)

After 200 jobs or 1 hour: → Recycling → Start new worker
```

## Language-Specific Optimizations

### Java/JVM

**Container Environment:**
```dockerfile
ENV JAVA_OPTS="\
    -XX:+TieredCompilation \
    -XX:TieredStopAtLevel=1 \
    -XX:+UseG1GC \
    -XX:MaxGCPauseMillis=200 \
    -XX:+UseStringDeduplication \
    -XX:+AlwaysPreTouch \
    -XX:InitiatingHeapOccupancyPercent=45 \
    -XX:MaxRAMPercentage=75.0"
```

**Warmup Strategy:**
- Run synthetic Java workload (100-1000 iterations)
- Exercise JIT compiler with hot functions
- Load common classes
- Trigger initial GC

**GC Management:**
- Force GC every 20 jobs: `jcmd 1 GC.run`
- Monitor memory usage
- Recycle if memory leaks detected

### TypeScript/Node.js

**Container Environment:**
```dockerfile
ENV NODE_OPTIONS="\
    --expose-gc \
    --max-old-space-size=4096 \
    --max-semi-space-size=128"
```

**Warmup Strategy:**
- Pre-load common modules (fs, path, crypto)
- Exercise V8 TurboFan with hot functions (1000+ iterations)
- Simulate typical build operations
- Trigger initial GC

**GC Management:**
- Force GC every 30 jobs: `node -e "global.gc()"`
- Monitor V8 heap usage
- Recycle workers proactively

## Monitoring

The pool manager exposes metrics via `WarmWorkerPoolMetrics`:

```rust
pub struct WarmWorkerPoolMetrics {
    pub ready_workers: AtomicUsize,
    pub active_workers: AtomicUsize,
    pub provisioning_workers: AtomicUsize,
    pub recycled_workers: AtomicUsize,
}
```

## Testing

```bash
# Run unit tests
cargo test -p nativelink-crio-worker-pool

# Run with Bazel
bazel test //nativelink-crio-worker-pool/...
```

## Docker Image Build

```bash
# Build Java worker image
cd docker/java
docker build -t nativelink-worker-java:latest .

# Build TypeScript worker image
cd ../typescript
docker build -t nativelink-worker-node:latest .

# Tag and push to registry
docker tag nativelink-worker-java:latest ghcr.io/tracemachina/nativelink-worker-java:latest
docker push ghcr.io/tracemachina/nativelink-worker-java:latest
```

## Troubleshooting

### Workers not starting

1. **Check CRI-O is running:**
   ```bash
   sudo systemctl status crio
   sudo crictl --runtime-endpoint unix:///var/run/crio/crio.sock ps
   ```

2. **Verify image is pulled:**
   ```bash
   sudo crictl --runtime-endpoint unix:///var/run/crio/crio.sock images
   ```

3. **Check permissions:**
   ```bash
   sudo chmod 666 /var/run/crio/crio.sock  # For development only
   ```

### Warmup failing

1. **Check container logs:**
   ```bash
   sudo crictl --runtime-endpoint unix:///var/run/crio/crio.sock logs <container-id>
   ```

2. **Exec into container:**
   ```bash
   sudo crictl --runtime-endpoint unix:///var/run/crio/crio.sock exec -it <container-id> /bin/bash
   ```

3. **Test warmup scripts manually:**
   ```bash
   sudo crictl exec <container-id> /opt/warmup/jvm-warmup.sh
   ```

### Memory issues

1. **Check container resource limits:**
   ```json
   {
     "linux": {
       "resources": {
         "memory_limit_in_bytes": 8589934592  // 8 GB
       }
     }
   }
   ```

2. **Monitor GC behavior:**
   ```bash
   # For Java
   sudo crictl exec <container-id> jcmd 1 GC.heap_info

   # For Node.js
   sudo crictl exec <container-id> node -e "console.log(process.memoryUsage())"
   ```

## Performance Tuning

### Pool Sizing

- **Development**: 2-5 workers per pool
- **Small team** (< 10 devs): 5-10 workers
- **Medium team** (10-50 devs): 10-30 workers
- **Large team** (50+ devs): 30-100 workers

### Warmup Iterations

- **Java**: 100-1000 iterations (20-60s warmup)
- **TypeScript**: 50-500 iterations (10-30s warmup)

### GC Frequency

- **Memory-constrained**: GC every 10 jobs
- **Balanced**: GC every 20-30 jobs
- **Performance-optimized**: GC every 50+ jobs

## Future Enhancements

- [ ] gRPC-based CRI client (instead of crictl)
- [ ] Predictive scaling based on load patterns
- [ ] ML-based warmup optimization
- [ ] Multi-region deployment support
- [ ] Support for Rust, Go, Python workers
- [ ] Integration with NativeLink scheduler
- [ ] Prometheus metrics export
- [ ] Grafana dashboards

## References

- [CRI-O Documentation](https://cri-o.io/)
- [Kubernetes CRI](https://kubernetes.io/docs/concepts/architecture/cri/)
- [JVM Warmup Best Practices](https://www.baeldung.com/java-jvm-warmup)
- [V8 Optimization](https://v8.dev/docs/turbofan)
- [G1GC Tuning](https://docs.oracle.com/en/java/javase/21/gctuning/)

## License

Apache-2.0
