# Multi-Worker Docker Compose Deployment

This guide explains how to run NativeLink with multiple workers using Docker Compose for distributed build execution but for single workers you can use the [docker-compose.yml](./docker-compose.yml) instead.

## Prerequisites

- **Architecture**: This setup currently only works on **x86_64/amd64** architectures. ARM64/Apple Silicon isn't supported for the test client container.
- **Google Cloud CLI**: If using the test client container from `gcr.io/bazel-public/bazel`, you'll need:
  1. [Google Cloud SDK](https://cloud.google.com/sdk/docs/install) installed
  2. Authentication via `gcloud auth login`
  3. Docker credential helper configured: `gcloud auth configure-docker`

  Alternatively, you can comment out the test-client service in the docker-compose file and run Bazel tests locally.

## Quick Start

0. Build the Docker container:

```sh
docker compose -f
      docker-compose-multi-worker.yml build
```

1. Start the multi-worker deployment:
```sh
docker compose -f docker-compose-multi-worker.yml up -d
```

2. Test with Bazel:
```sh
bazel build //... \
  --remote_cache=grpc://127.0.0.1:50051 \
  --remote_executor=grpc://127.0.0.1:50052 \
  --remote_default_exec_properties=cpu_count=2
```

## Configuration Requirements

### Critical: Shared CAS Storage

**All workers MUST share the same CAS storage path.** This is the most common configuration error.

**Important:** When starting workers, you'll see this validation warning:
```
Starting worker 'worker-1'. IMPORTANT: If running multiple workers, all workers
must share the same CAS storage path to avoid 'Object not found' errors.
```

✅ **Correct Configuration:**
```yaml
services:
  worker-1:
    volumes:
      - cas-data:/data/cas  # Shared volume
  worker-2:
    volumes:
      - cas-data:/data/cas  # Same shared volume
  worker-3:
    volumes:
      - cas-data:/data/cas  # Same shared volume

volumes:
  cas-data:  # Single shared volume for all workers
```

❌ **Incorrect Configuration (causes "Object not found" errors):**
```yaml
services:
  worker-1:
    volumes:
      - ./data/worker-1/cas:/data/cas  # Isolated path
  worker-2:
    volumes:
      - ./data/worker-2/cas:/data/cas  # Different isolated path
```

### Worker Configuration (worker-shared-cas.json5)

```json5
{
  stores: [
    {
      name: "SHARED_CAS",
      filesystem: {
        // All workers MUST use the same path
        content_path: "/data/cas/content",
        // Temp can be worker-specific
        temp_path: "/tmp/${WORKER_NAME}/temp",
        eviction_policy: {
          max_bytes: 10000000000,
        }
      }
    }
  ],
  workers: [
    {
      local: {
        name: "${WORKER_NAME}",
        cas_fast_slow_store: "SHARED_CAS",  // All workers use same store
        // Work directory should be worker-specific
        work_directory: "/tmp/${WORKER_NAME}/work",
        platform_properties: {
          cpu_count: { values: ["2"] },
          memory_gb: { values: ["2"] }
        }
      }
    }
  ]
}
```

## Files

- `test-multi-worker-simple.json5` - All-in-one configuration for testing (validated working configuration)

## Scaling Workers

### Dynamic Scaling
```sh
# Scale to 5 workers
docker-compose -f docker-compose-multi-worker.yml up -d --scale worker=5

# Scale down to 2 workers
docker-compose -f docker-compose-multi-worker.yml up -d --scale worker=2
```

### Resource Limits
Each worker is configured with:
- CPU: 2 cores (configurable)
- Memory: 2GB (configurable)

Adjust in `docker-compose-multi-worker.yml`:
```yaml
deploy:
  resources:
    limits:
      cpus: '4'    # Increase to 4 cores
      memory: 4G   # Increase to 4GB
```

## Monitoring

### View Logs
```sh
# All workers
docker-compose -f docker-compose-multi-worker.yml logs -f

# Specific worker
docker-compose -f docker-compose-multi-worker.yml logs -f worker-1

# Scheduler
docker-compose -f docker-compose-multi-worker.yml logs -f scheduler
```

### Check Status
```sh
# View all containers
docker-compose -f docker-compose-multi-worker.yml ps

# Check CAS storage usage
docker exec -it $(docker-compose -f docker-compose-multi-worker.yml ps -q worker-1) \
  du -sh /data/cas/content
```

## Troubleshooting

### "Object not found" Errors
**Symptom:** Workers fail with enhanced error message:
```
Object 7fd25e01d12373a2d1712e446881c9246a9698da4e7eafecdaeeaaff62195a82-148
not found in either fast or slow store. If using multiple workers, ensure
all workers share the same CAS storage path.
```

**Cause:** Workers are using different CAS paths

**Solution:**
1. Check volume mounts: `docker inspect <worker-container> | grep -A 5 Mounts`
2. Ensure all workers mount the same `cas-data` volume
3. Verify `content_path` is identical in all worker configurations
4. Use the validated `test-multi-worker-simple.json5` as reference

### Workers Not Executing Jobs
**Symptom:** Jobs queue but workers remain idle

**Solution:**
1. Check scheduler connectivity: `docker logs scheduler | grep -i error`
2. Verify worker registration: `docker logs worker-1 | grep "Connected to scheduler"`
3. Check network: `docker network ls`

### Performance Issues
**Symptom:** Builds slower with multiple workers

**Solution:**
1. Check disk I/O: `docker stats`
2. Consider using SSD-backed storage
3. Increase cache size in `eviction_policy`
4. Monitor with: `docker-compose -f docker-compose-multi-worker.yml logs -f | grep -i slow`

## Testing Multi-Worker Setup

### Local Testing with All-in-One Configuration

#### Option 1: Using Docker (Recommended)
```sh
# Run nativelink using the Docker image you built
docker run --rm -it \
  -v $(pwd)/test-multi-worker-simple.json5:/config.json5 \
  -p 50051:50051 \
  -p 50052:50052 \
  nativelink:latest /config.json5

# In another terminal, test with Bazel
bazel build //:nativelink \
  --remote_cache=grpc://127.0.0.1:50051 \
  --remote_executor=grpc://127.0.0.1:50051 \
  --remote_default_exec_properties=cpu_count=1 \
  --jobs=3
```

#### Option 2: Build and Run Locally (Advanced)
```sh
# Build nativelink locally first
cargo build --release

# Start multi-worker server (3 workers, shared CAS)
./target/release/nativelink test-multi-worker-simple.json5

# In another terminal, test with Bazel
bazel build //:nativelink \
  --remote_cache=grpc://127.0.0.1:50051 \
  --remote_executor=grpc://127.0.0.1:50051 \
  --remote_default_exec_properties=cpu_count=1 \
  --jobs=3
```

### Basic Test
```sh
# Build a simple target
bazel build //src:hello_world \
  --remote_cache=grpc://127.0.0.1:50051 \
  --remote_executor=grpc://127.0.0.1:50052

# Check which worker executed
docker-compose -f docker-compose-multi-worker.yml logs | grep "Executing action"
```

### Stress Test
```sh
# Build entire project with parallel jobs
bazel build //... \
  --remote_cache=grpc://127.0.0.1:50051 \
  --remote_executor=grpc://127.0.0.1:50052 \
  --jobs=50  # High parallelism to distribute across workers
```

## Production Considerations

For production deployments:
1. Use persistent volumes backed by SSD
2. Implement health checks in docker-compose.yml
3. Use external storage (NFS, S3, etc.) for CAS
4. Monitor worker metrics
5. Set up log aggregation

See [Kubernetes deployment](../kubernetes/) for production-grade configurations.
